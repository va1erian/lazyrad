#![forbid(unsafe_code)]

//! The editor's painter: a monospace grid.
//!
//! One character cell's advance and one line's height are measured once per
//! paint; every other position is `col * advance`. Only the visible lines are
//! drawn, so a 5,000-line file costs the same as a short one (PLAN.md §5,
//! gap G7: xui has no caret-from-offset query, so the grid does its own).

use xui_core::Color;
use xui_core::backend::{Canvas, TextAlign, TextStyle, TextVAlign};
use xui_core::geometry::{Point, Rect};
use xui_core::theme::Theme;

use crate::markers::MarkerKind;
use crate::metrics::{CELL_PROBE, Metrics, Viewport};
use crate::scrollbar::{self, Orientation, Scroll};
use crate::state::EditorState;
use crate::text::{display_col, expand_tabs};
use crate::theme::EditorTheme;
use crate::view::caret_display_col;

/// Draws `state` into `canvas`.
pub(crate) fn paint(
    canvas: &mut dyn Canvas,
    state: &EditorState,
    theme: &EditorTheme,
    xui_theme: &Theme,
) {
    let bounds = canvas.bounds();
    let dpi = canvas.dpi();
    let style = state.options.font.style(theme.text);
    let measured = canvas.measure_text(CELL_PROBE, &style);
    let line_count = state.buffer.line_count();
    let metrics = Metrics::new(measured, line_count, state.options.show_gutter, dpi);
    let viewport = Viewport::split(
        bounds,
        metrics,
        line_count,
        state.buffer.max_line_chars(),
        dpi,
    );
    let first_line = state.view.first_line.min(line_count.saturating_sub(1));
    let last_line = (first_line + viewport.visible_lines).min(line_count);
    let first_col = state.view.first_col;

    canvas.fill_rect(bounds, theme.background);
    if state.options.show_gutter {
        canvas.fill_rect(viewport.gutter, theme.gutter_background);
    }

    paint_current_line(canvas, state, theme, &viewport, first_line, line_count);
    paint_marker_tints(canvas, state, theme, &viewport, first_line, last_line);

    if let Some((start, end)) = state.view.selection() {
        let color = if state.focused {
            theme.selection
        } else {
            theme.selection_unfocused
        };
        paint_selection(
            canvas, state, &viewport, first_line, last_line, start, end, color,
        );
    }

    paint_lines(canvas, state, &viewport, &style, first_line, last_line);
    paint_squiggles(canvas, state, theme, &viewport, first_line, last_line);
    paint_gutter(canvas, state, theme, &viewport, first_line, last_line);
    paint_caret(canvas, state, theme, &viewport, first_line, last_line);

    paint_scrollbars(canvas, state, &viewport, first_line, first_col, xui_theme);

    let border = if state.focused {
        theme.border_focused
    } else {
        theme.border
    };
    canvas.stroke_rect(bounds, border, 1.0);
    if state.selected {
        canvas.stroke_rect(bounds, xui_theme.accent, 2.0);
    }
}

/// The current line's highlight, behind the text.
fn paint_current_line(
    canvas: &mut dyn Canvas,
    state: &EditorState,
    theme: &EditorTheme,
    viewport: &Viewport,
    first_line: usize,
    line_count: usize,
) {
    if !state.focused {
        return;
    }
    let line = state.buffer.line_of_char(state.view.caret);
    if line < first_line || line >= first_line + viewport.visible_lines || line >= line_count {
        return;
    }
    let y = viewport.metrics.y_of_line(viewport.text, line, first_line);
    let row = Rect::new(
        viewport.text.left,
        y,
        viewport.text.right,
        y + viewport.metrics.line_height,
    );
    canvas.fill_rect(row, theme.current_line);
}

/// Faint tints behind lines carrying error or warning markers.
fn paint_marker_tints(
    canvas: &mut dyn Canvas,
    state: &EditorState,
    theme: &EditorTheme,
    viewport: &Viewport,
    first_line: usize,
    last_line: usize,
) {
    for marker in &state.markers {
        if marker.line < first_line || marker.line >= last_line {
            continue;
        }
        let color = match marker.kind {
            MarkerKind::Error => theme.error,
            MarkerKind::Warning => theme.warning,
            _ => continue,
        };
        let y = viewport
            .metrics
            .y_of_line(viewport.text, marker.line, first_line);
        let row = Rect::new(
            viewport.gutter.left,
            y,
            viewport.text.right,
            y + viewport.metrics.line_height,
        );
        canvas.fill_rect(row, color.lerp(theme.background, 0.85));
    }
}

/// The selection fill, clipped to the visible text area.
#[allow(clippy::too_many_arguments)]
fn paint_selection(
    canvas: &mut dyn Canvas,
    state: &EditorState,
    viewport: &Viewport,
    first_line: usize,
    last_line: usize,
    start: usize,
    end: usize,
    color: Color,
) {
    let text = viewport.text;
    let metrics = viewport.metrics;
    let tab = state.options.tab_width;
    let first_col = state.view.first_col;
    let start_line = state.buffer.line_of_char(start);
    let end_line = state.buffer.line_of_char(end);
    canvas.push_clip(text);
    for line in start_line..=end_line {
        if line < first_line || line >= last_line {
            continue;
        }
        let line_start = state.buffer.line_start(line);
        let line_text = state.buffer.line_string(line);
        let line_len = line_text.chars().count();
        let from = if line == start_line {
            start.saturating_sub(line_start)
        } else {
            0
        };
        let to = if line == end_line {
            end.saturating_sub(line_start)
        } else {
            line_len
        };
        let x0 = metrics.x_of_col(text, display_col(&line_text, from, tab), first_col);
        let mut end_col = display_col(&line_text, to, tab);
        if to == line_len && line < end_line {
            // The newline cell is selected too.
            end_col += 1;
        }
        let mut x1 = metrics.x_of_col(text, end_col, first_col);
        if x1 <= x0 {
            x1 = x0 + metrics.advance;
        }
        let y = metrics.y_of_line(text, line, first_line);
        let cell = Rect::new(x0, y, x1, y + metrics.line_height);
        if let Some(cell) = intersect(cell, text) {
            canvas.fill_rect(cell, color);
        }
    }
    canvas.pop_clip();
}

/// The visible lines' text.
fn paint_lines(
    canvas: &mut dyn Canvas,
    state: &EditorState,
    viewport: &Viewport,
    style: &TextStyle,
    first_line: usize,
    last_line: usize,
) {
    let text = viewport.text;
    let metrics = viewport.metrics;
    let tab = state.options.tab_width;
    let first_col = state.view.first_col;
    canvas.push_clip(text);
    for line in first_line..last_line {
        let expanded = expand_tabs(&state.buffer.line_string(line), tab);
        let visible = skip_columns(&expanded, first_col);
        if visible.is_empty() {
            continue;
        }
        let y = metrics.y_of_line(text, line, first_line);
        let row = Rect::new(text.left, y, text.right, y + metrics.line_height);
        canvas.draw_text(visible, row, style);
    }
    canvas.pop_clip();
}

/// The part of `line` from display column `first_col` on.
fn skip_columns(line: &str, first_col: usize) -> &str {
    match line.char_indices().nth(first_col) {
        Some((index, _)) => &line[index..],
        None if first_col == 0 => line,
        None => "",
    }
}

/// A squiggle under each spanned marker.
fn paint_squiggles(
    canvas: &mut dyn Canvas,
    state: &EditorState,
    theme: &EditorTheme,
    viewport: &Viewport,
    first_line: usize,
    last_line: usize,
) {
    let text = viewport.text;
    let metrics = viewport.metrics;
    let tab = state.options.tab_width;
    let first_col = state.view.first_col;
    canvas.push_clip(text);
    for marker in &state.markers {
        if !marker.has_span() || marker.line < first_line || marker.line >= last_line {
            continue;
        }
        let color = match marker.kind {
            MarkerKind::Error => theme.error,
            MarkerKind::Warning => theme.warning,
            _ => theme.gutter_text,
        };
        let line_text = state.buffer.line_string(marker.line);
        let start = display_col(&line_text, marker.start, tab);
        let end = display_col(&line_text, marker.end, tab).max(start + 1);
        let x0 = metrics.x_of_col(text, start, first_col);
        let x1 = metrics.x_of_col(text, end, first_col);
        let y = metrics.y_of_line(text, marker.line, first_line) + metrics.line_height - 2;
        if let Some(span) = intersect(Rect::new(x0, y, x1, y + 2), text) {
            canvas.draw_line(
                Point::new(span.left, span.top),
                Point::new(span.right, span.top),
                color,
                1.0,
            );
        }
    }
    canvas.pop_clip();
}

/// Line numbers and breakpoint dots.
fn paint_gutter(
    canvas: &mut dyn Canvas,
    state: &EditorState,
    theme: &EditorTheme,
    viewport: &Viewport,
    first_line: usize,
    last_line: usize,
) {
    if !state.options.show_gutter {
        return;
    }
    let gutter = viewport.gutter;
    let metrics = viewport.metrics;
    let pad = 4;
    let mut number_style = state.options.font.style(theme.gutter_text);
    number_style.align = TextAlign::End;
    number_style.valign = TextVAlign::Middle;
    for line in first_line..last_line {
        let y = metrics.y_of_line(viewport.text, line, first_line);
        let row = Rect::new(
            gutter.left + pad,
            y,
            gutter.right - pad,
            y + metrics.line_height,
        );
        canvas.draw_text(&(line + 1).to_string(), row, &number_style);
    }
    for marker in &state.markers {
        if marker.kind != MarkerKind::Breakpoint
            || marker.line < first_line
            || marker.line >= last_line
        {
            continue;
        }
        let center = Point::new(
            gutter.left + pad,
            metrics.y_of_line(viewport.text, marker.line, first_line) + metrics.line_height / 2,
        );
        canvas.fill_ellipse(center, 3.0, 3.0, theme.breakpoint);
    }
}

/// The blinking caret.
fn paint_caret(
    canvas: &mut dyn Canvas,
    state: &EditorState,
    theme: &EditorTheme,
    viewport: &Viewport,
    first_line: usize,
    last_line: usize,
) {
    if !state.focused || !state.blink_on {
        return;
    }
    let line = state.buffer.line_of_char(state.view.caret);
    if line < first_line || line >= last_line {
        return;
    }
    let col = caret_display_col(&state.buffer, state.options.tab_width, state.view.caret);
    let x = viewport
        .metrics
        .x_of_col(viewport.text, col, state.view.first_col);
    if x < viewport.text.left || x >= viewport.text.right {
        return;
    }
    let y = viewport.metrics.y_of_line(viewport.text, line, first_line);
    canvas.draw_line(
        Point::new(x, y),
        Point::new(x, y + viewport.metrics.line_height),
        theme.caret,
        1.0,
    );
}

/// The vertical and horizontal scrollbars.
fn paint_scrollbars(
    canvas: &mut dyn Canvas,
    state: &EditorState,
    viewport: &Viewport,
    first_line: usize,
    first_col: usize,
    xui_theme: &Theme,
) {
    let metrics = viewport.metrics;
    let line_count = state.buffer.line_count();
    if let Some(track) = viewport.vbar {
        let scroll = Scroll {
            viewport: viewport.text.height(),
            content: line_count as i32 * metrics.line_height,
            offset: first_line as i32 * metrics.line_height,
        };
        scrollbar::paint(canvas, track, scroll, Orientation::Vertical, *xui_theme);
    }
    if let Some(track) = viewport.hbar {
        let scroll = Scroll {
            viewport: viewport.text.width(),
            content: state.buffer.max_line_chars() as i32 * metrics.advance,
            offset: first_col as i32 * metrics.advance,
        };
        scrollbar::paint(canvas, track, scroll, Orientation::Horizontal, *xui_theme);
    }
}

/// The intersection of two rectangles, or `None` when they do not overlap.
fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let left = a.left.max(b.left);
    let top = a.top.max(b.top);
    let right = a.right.min(b.right);
    let bottom = a.bottom.min(b.bottom);
    (left < right && top < bottom).then(|| Rect::new(left, top, right, bottom))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markers::Marker;
    use crate::options::Options;
    use crate::platform::InProcessClipboard;
    use crate::state::EditorState;
    use crate::theme::EditorTheme;
    use xui_canvas::{RgbaImage, Surface};

    fn render(state: &EditorState) -> RgbaImage {
        let xui_theme = Theme::light();
        let editor_theme = EditorTheme::from_theme(xui_theme);
        let mut surface = Surface::new(240, 80);
        surface.with_canvas_at(Rect::new(0, 0, 240, 80), 96, |canvas| {
            paint(canvas, state, &editor_theme, &xui_theme);
        });
        surface.to_image()
    }

    fn editor_state(text: &str) -> EditorState {
        let mut state = EditorState::new(text, Options::default(), Box::new(InProcessClipboard));
        state.focused = true;
        state
    }

    fn count_color(image: &RgbaImage, color: Color) -> usize {
        let mut count = 0;
        for y in 0..image.height {
            for x in 0..image.width {
                if image.pixel(x, y) == Some([color.r, color.g, color.b, 0xFF]) {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn text_and_selection_are_painted() {
        let mut state = editor_state("hello\nworld");
        state.view.anchor = 0;
        state.view.caret = 5;
        let theme = EditorTheme::from_theme(Theme::light());
        let image = render(&state);

        assert!(
            count_color(&image, theme.selection) > 0,
            "the selection fill is drawn"
        );
        let blank = render(&editor_state(""));
        assert_ne!(
            image.pixels, blank.pixels,
            "a selected document differs from a blank one"
        );
    }

    #[test]
    fn the_caret_is_painted_only_when_visible() {
        let mut state = editor_state("abc");
        state.view.caret = 1;
        state.view.anchor = 1;
        state.blink_on = true;
        let shown = render(&state);
        state.blink_on = false;
        let hidden = render(&state);
        assert_ne!(shown.pixels, hidden.pixels, "the blink hides the caret");
    }

    #[test]
    fn markers_paint_a_squiggle() {
        let mut state = editor_state("let x = ;");
        let plain = render(&state);
        state.markers = vec![Marker::new(0, 8, 8, MarkerKind::Error)];
        let marked = render(&state);
        assert_ne!(
            plain.pixels, marked.pixels,
            "the squiggle changes the rendered pixels"
        );
    }
}
