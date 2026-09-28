#![forbid(unsafe_code)]

//! The editor's event mapper: pointer and keyboard input to caret, selection,
//! scrolling and edits.
//!
//! The mapper is generic over the app's message type for the [`Ui`] it needs to
//! measure, focus and repaint; the widget wraps it and raises `on_change`
//! (PLAN.md §5).

use xui_core::app::Ui;
use xui_core::backend::{Event, WidgetId};
use xui_core::message::{Key, MouseButton};

use crate::edit;
use crate::metrics::{CELL_PROBE, Metrics, Viewport};
use crate::scrollbar::{self, Orientation, Scroll};
use crate::state::{Drag, EditorState, Effect};
use crate::text::char_col_for_display;
use crate::view::word_range_at;

/// Rows a wheel notch scrolls.
const WHEEL_ROWS: i32 = 3;
/// Columns a horizontal wheel notch scrolls.
const WHEEL_COLS: i32 = 3;

/// What an event did, so the widget knows whether to raise `on_change`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// Whether the text changed and `on_change` should fire.
    pub changed: bool,
}

/// The metrics and viewport for the current bounds and buffer.
fn viewport<M: 'static>(ui: &Ui<M>, id: WidgetId, state: &EditorState) -> Viewport {
    let dpi = ui.dpi();
    let style = state.options.font.style(xui_core::Color::rgb(0, 0, 0));
    let measured = ui.measure_text(CELL_PROBE, &style, dpi);
    let line_count = state.buffer.line_count();
    let metrics = Metrics::new(measured, line_count, state.options.show_gutter, dpi);
    Viewport::split(
        ui.bounds(id),
        metrics,
        line_count,
        state.buffer.max_line_chars(),
        dpi,
    )
}

/// Handles one event, returning `None` when it is not the editor's.
pub(crate) fn handle<M: 'static>(
    state: &mut EditorState,
    ui: &Ui<M>,
    id: WidgetId,
    event: &Event,
) -> Option<Outcome> {
    match event {
        Event::SetFocus => {
            state.focused = true;
            state.reset_blink();
            Some(Outcome::default())
        }
        Event::KillFocus => {
            state.focused = false;
            state.view.dragging = false;
            Some(Outcome::default())
        }
        Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            modifiers,
        } => Some(mouse_press(state, ui, id, *x, *y, modifiers.shift, None)),
        Event::MouseDoubleClick {
            x,
            y,
            button: MouseButton::Left,
            ..
        } => Some(mouse_press(state, ui, id, *x, *y, false, Some(2))),
        Event::MouseMove { x, y, .. } => Some(mouse_move(state, ui, id, *x, *y)),
        Event::MouseUp {
            button: MouseButton::Left,
            ..
        } => {
            state.view.dragging = false;
            state.v_drag = None;
            state.h_drag = None;
            if state.captured {
                state.captured = false;
                state.effects.push(Effect::ReleaseCapture);
            }
            Some(Outcome::default())
        }
        Event::CaptureChanged => {
            state.view.dragging = false;
            state.v_drag = None;
            state.h_drag = None;
            state.captured = false;
            Some(Outcome::default())
        }
        Event::MouseWheel {
            delta,
            horizontal,
            modifiers,
            ..
        } => Some(wheel(state, ui, id, *delta, *horizontal, modifiers.shift)),
        Event::KeyDown {
            key,
            modifiers,
            system,
            ..
        } if !*system => {
            if !state.focused {
                return None;
            }
            key_down(state, ui, id, *key, modifiers.ctrl, modifiers.shift)
        }
        Event::Char(character) if state.focused => {
            if character.is_control() {
                return None;
            }
            edit::type_char(&mut state.buffer, &mut state.view, *character);
            finish_edit(state, ui, id);
            Some(Outcome { changed: true })
        }
        Event::Timer { .. } => {
            if state.focused {
                state.toggle_blink();
            }
            Some(Outcome::default())
        }
        Event::Resize { .. } => {
            ensure_visible(state, ui, id);
            Some(Outcome::default())
        }
        _ => None,
    }
}

/// Handles a left-button press: caret placement, word/line selection or a
/// scrollbar drag. `forced_count` overrides the tracked click run when a
/// backend reports a double click as its own event.
fn mouse_press<M: 'static>(
    state: &mut EditorState,
    ui: &Ui<M>,
    id: WidgetId,
    x: i32,
    y: i32,
    shift: bool,
    forced_count: Option<u8>,
) -> Outcome {
    state.effects.push(Effect::Focus);
    state.focused = true;
    let layout = viewport(ui, id, state);
    let first_line = clamped_first_line(state, &layout);
    if scrollbar_down(state, ui, &layout, first_line, x, y) {
        return Outcome::default();
    }

    let count = match forced_count {
        Some(count) => {
            state.click.set_count(count, x, y);
            count
        }
        None => state.click.register(x, y),
    };
    state.buffer.break_coalescing();
    let position = position_at(state, &layout, first_line, x, y);
    match count {
        2 => {
            let line = state.buffer.line_of_char(position);
            let column = layout.metrics.col_at(layout.text, x, state.view.first_col);
            let (start, end) = word_range_at(&state.buffer, line, column, state.options.tab_width);
            state.view.anchor = start;
            state.view.caret = end;
            state.view.goal_col = None;
            state.view.dragging = true;
        }
        3 => {
            let line = state.buffer.line_of_char(position);
            let start = state.buffer.line_start(line);
            let end = if line + 1 < state.buffer.line_count() {
                state.buffer.line_start(line + 1)
            } else {
                state.buffer.len_chars()
            };
            state.view.anchor = start;
            state.view.caret = end;
            state.view.goal_col = None;
            state.view.dragging = false;
        }
        _ => {
            state.view.caret = position;
            if !shift {
                state.view.anchor = position;
            }
            state.view.goal_col = None;
            state.view.dragging = true;
        }
    }
    state.captured = true;
    state.effects.push(Effect::Capture);
    state.reset_blink();
    ensure_visible(state, ui, id);
    Outcome::default()
}

/// Handles a mouse move during a text or scrollbar drag.
fn mouse_move<M: 'static>(
    state: &mut EditorState,
    ui: &Ui<M>,
    id: WidgetId,
    x: i32,
    y: i32,
) -> Outcome {
    let layout = viewport(ui, id, state);
    let first_line = clamped_first_line(state, &layout);
    if scrollbar_move(state, ui, id, &layout, first_line, x, y) {
        return Outcome::default();
    }
    if state.view.dragging {
        state.buffer.break_coalescing();
        let position = position_at(state, &layout, first_line, x, y);
        state.view.caret = position;
        state.view.goal_col = None;
        state.reset_blink();
        ensure_visible(state, ui, id);
    }
    Outcome::default()
}

/// Handles a wheel notch, vertically or horizontally.
fn wheel<M: 'static>(
    state: &mut EditorState,
    ui: &Ui<M>,
    id: WidgetId,
    delta: i16,
    horizontal: bool,
    shift: bool,
) -> Outcome {
    let layout = viewport(ui, id, state);
    if horizontal || shift {
        let step = i32::from(delta) * WHEEL_COLS;
        let max = max_first_col(state, &layout);
        state.view.first_col = (state.view.first_col as i32 - step).clamp(0, max) as usize;
    } else {
        let step = i32::from(delta) * WHEEL_ROWS;
        let max = state
            .buffer
            .line_count()
            .saturating_sub(layout.visible_lines);
        state.view.first_line = (state.view.first_line as i32 - step).clamp(0, max as i32) as usize;
        state.view.goal_col = None;
    }
    Outcome::default()
}

/// Handles a navigation or editing key.
fn key_down<M: 'static>(
    state: &mut EditorState,
    ui: &Ui<M>,
    id: WidgetId,
    key: Key,
    ctrl: bool,
    shift: bool,
) -> Option<Outcome> {
    let tab = state.options.tab_width;
    let layout = viewport(ui, id, state);
    let page = layout.visible_lines as i64;
    let mut changed = false;
    if key != Key::BACK && key != Key::DELETE {
        state.buffer.break_coalescing();
    }
    match key {
        Key::LEFT if ctrl => state.view.word_left(&state.buffer, shift),
        Key::LEFT => state.view.left(shift),
        Key::RIGHT if ctrl => state.view.word_right(&state.buffer, shift),
        Key::RIGHT => state.view.right(&state.buffer, shift),
        Key::UP => state.view.up(&state.buffer, tab, shift),
        Key::DOWN => state.view.down(&state.buffer, tab, shift),
        Key::HOME if ctrl => state.view.document_home(shift),
        Key::HOME => state.view.home(&state.buffer, shift),
        Key::END if ctrl => state.view.document_end(&state.buffer, shift),
        Key::END => state.view.end(&state.buffer, shift),
        Key::PAGE_UP => state.view.page(&state.buffer, tab, -page, shift),
        Key::PAGE_DOWN => state.view.page(&state.buffer, tab, page, shift),
        Key::BACK => {
            edit::backspace(&mut state.buffer, &mut state.view);
            changed = true;
        }
        Key::DELETE => {
            edit::delete_forward(&mut state.buffer, &mut state.view);
            changed = true;
        }
        Key::RETURN => {
            edit::enter(&mut state.buffer, &mut state.view);
            changed = true;
        }
        Key::TAB if shift => edit::outdent(&mut state.buffer, &mut state.view, &state.options),
        Key::TAB => edit::indent(&mut state.buffer, &mut state.view, &state.options),
        Key::A if ctrl => state.view.select_all(&state.buffer),
        Key::C if ctrl => {
            edit::copy(&state.buffer, &state.view, state.clipboard.as_ref());
        }
        Key::X if ctrl => {
            changed = edit::cut(&mut state.buffer, &mut state.view, state.clipboard.as_ref())
        }
        Key::V if ctrl => {
            changed = edit::paste(&mut state.buffer, &mut state.view, state.clipboard.as_ref())
        }
        Key::Z if ctrl && shift => changed = edit::redo(&mut state.buffer, &mut state.view),
        Key::Z if ctrl => changed = edit::undo(&mut state.buffer, &mut state.view),
        Key::Y if ctrl => changed = edit::redo(&mut state.buffer, &mut state.view),
        Key::ESCAPE => {
            state.view.collapse();
            state.view.dragging = false;
        }
        _ => return None,
    }
    finish_edit(state, ui, id);
    Some(Outcome { changed })
}

/// Resets the blink and scrolls the caret into view after an edit or move.
fn finish_edit<M: 'static>(state: &mut EditorState, ui: &Ui<M>, id: WidgetId) {
    state.reset_blink();
    ensure_visible(state, ui, id);
}

/// Scrolls so the caret is visible, then clamps both axes to their content.
pub(crate) fn ensure_visible<M: 'static>(state: &mut EditorState, ui: &Ui<M>, id: WidgetId) {
    let layout = viewport(ui, id, state);
    let tab = state.options.tab_width;
    state.view.ensure_caret_visible(
        &state.buffer,
        tab,
        layout.visible_lines,
        layout.visible_cols,
    );
    let max_line = state
        .buffer
        .line_count()
        .saturating_sub(layout.visible_lines);
    state.view.first_line = state.view.first_line.min(max_line);
    let max_col = state
        .buffer
        .max_line_chars()
        .saturating_sub(layout.visible_cols);
    state.view.first_col = state.view.first_col.min(max_col);
}

/// The buffer position at a point.
fn position_at(state: &EditorState, layout: &Viewport, first_line: usize, x: i32, y: i32) -> usize {
    let line_count = state.buffer.line_count();
    let line = layout
        .metrics
        .line_at(layout.text, y, first_line, line_count);
    let column = layout.metrics.col_at(layout.text, x, state.view.first_col);
    let text = state.buffer.line_string(line);
    let char_col = char_col_for_display(&text, column, state.options.tab_width);
    state.buffer.line_start(line) + char_col
}

/// The first visible line, clamped to the buffer.
fn clamped_first_line(state: &EditorState, layout: &Viewport) -> usize {
    state.view.first_line.min(
        state
            .buffer
            .line_count()
            .saturating_sub(layout.visible_lines),
    )
}

/// Starts a scrollbar drag or a page jump when `(x, y)` is on a bar, returning
/// whether it was handled.
fn scrollbar_down<M: 'static>(
    state: &mut EditorState,
    ui: &Ui<M>,
    layout: &Viewport,
    first_line: usize,
    x: i32,
    y: i32,
) -> bool {
    let dpi = ui.dpi();
    if let Some(track) = layout.vbar
        && track.contains(xui_core::geometry::Point::new(x, y))
    {
        let scroll = vertical_scroll(state, layout, first_line);
        match scrollbar::thumb(track, scroll, Orientation::Vertical, dpi) {
            Some(thumb) if y >= thumb.top && y < thumb.bottom => {
                state.v_drag = Some(Drag {
                    start_offset: first_line as i32 * layout.metrics.line_height,
                    start_pointer: y,
                });
            }
            thumb => {
                let page = layout.visible_lines as i32;
                let above = thumb.is_some_and(|thumb| y < thumb.top);
                let target = if above {
                    first_line as i32 - page
                } else {
                    first_line as i32 + page
                };
                let max = state
                    .buffer
                    .line_count()
                    .saturating_sub(layout.visible_lines);
                state.view.first_line = target.clamp(0, max as i32) as usize;
            }
        }
        state.captured = true;
        state.effects.push(Effect::Capture);
        return true;
    }
    if let Some(track) = layout.hbar
        && track.contains(xui_core::geometry::Point::new(x, y))
    {
        let scroll = horizontal_scroll(state, layout);
        match scrollbar::thumb(track, scroll, Orientation::Horizontal, dpi) {
            Some(thumb) if x >= thumb.left && x < thumb.right => {
                state.h_drag = Some(Drag {
                    start_offset: state.view.first_col as i32 * layout.metrics.advance,
                    start_pointer: x,
                });
            }
            thumb => {
                let page = layout.visible_cols as i32;
                let before = thumb.is_some_and(|thumb| x < thumb.left);
                let target = if before {
                    state.view.first_col as i32 - page
                } else {
                    state.view.first_col as i32 + page
                };
                state.view.first_col = target.clamp(0, max_first_col(state, layout)) as usize;
            }
        }
        state.captured = true;
        state.effects.push(Effect::Capture);
        return true;
    }
    false
}

/// Applies an in-progress scrollbar drag, returning whether it is dragging.
fn scrollbar_move<M: 'static>(
    state: &mut EditorState,
    ui: &Ui<M>,
    _id: WidgetId,
    layout: &Viewport,
    first_line: usize,
    x: i32,
    y: i32,
) -> bool {
    let dpi = ui.dpi();
    let metrics = layout.metrics;
    if let (Some(drag), Some(track)) = (state.v_drag, layout.vbar) {
        let scroll = vertical_scroll(state, layout, first_line);
        let pixels = scrollbar::offset_from_drag(
            track,
            scroll,
            Orientation::Vertical,
            drag.start_offset,
            drag.start_pointer,
            y,
            dpi,
        );
        let max = state
            .buffer
            .line_count()
            .saturating_sub(layout.visible_lines);
        state.view.first_line = ((pixels / metrics.line_height).max(0) as usize).min(max);
        return true;
    }
    if let (Some(drag), Some(track)) = (state.h_drag, layout.hbar) {
        let scroll = horizontal_scroll(state, layout);
        let pixels = scrollbar::offset_from_drag(
            track,
            scroll,
            Orientation::Horizontal,
            drag.start_offset,
            drag.start_pointer,
            x,
            dpi,
        );
        state.view.first_col = (pixels / metrics.advance).max(0) as usize;
        return true;
    }
    false
}

/// The vertical scroll state for `first_line`.
fn vertical_scroll(state: &EditorState, layout: &Viewport, first_line: usize) -> Scroll {
    Scroll {
        viewport: layout.text.height(),
        content: state.buffer.line_count() as i32 * layout.metrics.line_height,
        offset: first_line as i32 * layout.metrics.line_height,
    }
}

/// The horizontal scroll state.
/// The largest `first_col` that still shows text: the longest line's length
/// minus the visible columns.
fn max_first_col(state: &EditorState, layout: &Viewport) -> i32 {
    state
        .buffer
        .max_line_chars()
        .saturating_sub(layout.visible_cols) as i32
}

fn horizontal_scroll(state: &EditorState, layout: &Viewport) -> Scroll {
    Scroll {
        viewport: layout.text.width(),
        content: state.buffer.max_line_chars() as i32 * layout.metrics.advance,
        offset: state.view.first_col as i32 * layout.metrics.advance,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use xui_canvas::OffscreenBackend;
    use xui_core::backend::{Event, NodeKind, NodeSpec, PlatformSpec};
    use xui_core::geometry::Rect;
    use xui_core::message::{Modifiers, MouseButton};
    use xui_core::units::Dip;
    use xui_core::{App, Ui, run_app};

    use super::handle;
    use crate::options::Options;
    use crate::platform::InProcessClipboard;
    use crate::state::{EditorState, Effect};

    struct Empty;

    impl App for Empty {
        type Msg = ();
        fn update(&mut self, _msg: (), _ui: &mut Ui<()>) {}
    }

    /// Runs `check` with a live offscreen `Ui` and an editor-sized node.
    fn with_ui(check: impl FnOnce(&Ui<()>, xui_core::backend::WidgetId) + 'static) {
        let check = Rc::new(RefCell::new(Some(check)));
        run_app(
            Rc::new(OffscreenBackend::new()),
            PlatformSpec::new("events").size(Dip(300.0), Dip(200.0)),
            move |ui| {
                let id = ui
                    .create_node(&NodeSpec::new(NodeKind::Custom, Rect::new(0, 0, 300, 200)))
                    .expect("node");
                if let Some(check) = check.borrow_mut().take() {
                    check(ui, id);
                }
                Empty
            },
        )
        .expect("run_app");
    }

    fn state(text: &str) -> EditorState {
        EditorState::new(text, Options::default(), Box::new(InProcessClipboard))
    }

    #[test]
    fn focus_and_capture_are_deferred_not_called_under_the_borrow() {
        // The canvas backend delivers SetFocus / CaptureChanged synchronously
        // back into the mapper, so the handler must only record them.
        with_ui(|ui, id| {
            let mut state = state("hello\nworld");
            let press = Event::MouseDown {
                x: 80,
                y: 5,
                button: MouseButton::Left,
                modifiers: Modifiers::default(),
            };
            handle(&mut state, ui, id, &press).expect("press is handled");
            assert_eq!(state.effects, [Effect::Focus, Effect::Capture]);
            assert!(state.focused);

            state.effects.clear();
            let release = Event::MouseUp {
                x: 80,
                y: 5,
                button: MouseButton::Left,
                modifiers: Modifiers::default(),
            };
            handle(&mut state, ui, id, &release).expect("release is handled");
            assert_eq!(state.effects, [Effect::ReleaseCapture]);
        });
    }

    #[test]
    fn typing_while_focused_changes_the_text() {
        with_ui(|ui, id| {
            let mut state = state("");
            handle(&mut state, ui, id, &Event::SetFocus);
            let outcome = handle(&mut state, ui, id, &Event::Char('x')).expect("char is handled");
            assert!(outcome.changed);
            assert_eq!(state.buffer.text(), "x");
        });
    }
}
