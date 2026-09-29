#![forbid(unsafe_code)]

//! The Start Page: the LazyRAD logo above a line of welcome text and the
//! "Getting started" [`tutorial`], in one column that scrolls with the wheel.
//!
//! The tutorial's code samples are drawn with a simple monospace painter that
//! reuses the code editor's Rhai lexer and colours ([`RhaiHighlighter`],
//! [`EditorTheme`]) rather than embedding read-only `Editor` widgets: each
//! `Editor` is a focusable, input-handling node with its own scrolling and
//! caret timer, all of which a static sample does not want.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;

use xui_code_editor::{EditorTheme, FontConfig, Highlighter, LineState, RhaiHighlighter, Token};
use xui_core::app::Ui;
use xui_core::backend::{
    Backend, Canvas, Event, NodeKind, NodeSpec, Result as UiResult, TextStyle, WidgetId, WindowId,
};
use xui_core::geometry::Rect;
use xui_core::units::{Dip, dip};
use xui_core::widget::Control;
use xui_core::{Image, Theme};

use crate::tutorial;

/// The logo, embedded at build time.
const LOGO_PNG: &[u8] = include_bytes!("../../../assets/rye-shades.png");
/// The logo's side on screen, in design units.
const LOGO_SIZE: Dip = dip(96.0);
/// The space between the logo and the welcome text, in design units.
const GAP: Dip = dip(16.0);
/// The welcome text's size, in design units.
const TEXT_SIZE: Dip = dip(14.0);
/// The margin kept clear above the logo when the page is short, in design units.
const MARGIN: Dip = dip(16.0);

/// The decoded logo, shared by every Start Page. `None` if the embedded PNG
/// cannot be decoded, in which case the page shows only its text.
fn logo() -> Option<&'static Image> {
    static LOGO: OnceLock<Option<Image>> = OnceLock::new();
    LOGO.get_or_init(|| Image::decode_png(LOGO_PNG).ok())
        .as_ref()
}

/// The side of the window icon, in pixels. The platform scales it for the title
/// bar and the taskbar; 64 keeps it sharp at 200% without being large.
const ICON_SIDE: u32 = 64;

/// Sets the logo as `window`'s icon (title bar and taskbar). Does nothing if the
/// embedded PNG cannot be decoded.
pub(crate) fn install_window_icon(backend: &dyn Backend, window: WindowId) {
    if let Some(icon) = logo().and_then(|logo| scaled(logo, ICON_SIDE)) {
        backend.set_window_icon(window, &icon);
    }
}

/// The logo scaled to `side` pixels square. Halves repeatedly first, so a large
/// source is averaged down instead of point-sampled, then resamples to `side`.
fn scaled(source: &Image, side: u32) -> Option<Image> {
    let mut image = source.clone();
    while image.width() >= side.saturating_mul(2) {
        image = image.resized(image.width() / 2, image.height() / 2).ok()?;
    }
    image.resized(side, side).ok()
}

/// One wheel notch scrolls the page by this many design units.
const WHEEL_STEP: Dip = dip(60.0);
/// The wheel's notch, matching the platform's `MouseWheel` delta unit.
const WHEEL_NOTCH: i32 = 120;
/// The tutorial column's widest extent, in design units.
const COLUMN: Dip = dip(760.0);
/// The gap between blocks, in design units.
const BLOCK_GAP: Dip = dip(10.0);
/// The extra space above a heading, in design units.
const HEADING_GAP: Dip = dip(14.0);
/// The padding inside a code block or callout, in design units.
const PADDING: Dip = dip(10.0);
/// The heading text's size, in design units.
const HEADING_SIZE: Dip = dip(17.0);
/// The width of the scrollbar thumb, in design units.
const THUMB: Dip = dip(6.0);

/// The Start Page node. It is one custom node that fills its page, so it paints
/// its own background. It shows the logo, the welcome text and the
/// [`tutorial`], and scrolls with the mouse wheel when they do not fit.
pub struct StartPage<M: 'static> {
    control: Control<M>,
}

/// What the painter and the wheel handler share.
struct Scroll {
    /// How far the page is scrolled, in device pixels.
    offset: Cell<i32>,
    /// The furthest it can scroll, as of the last paint.
    max: Cell<i32>,
}

impl<M: 'static> StartPage<M> {
    /// Creates a Start Page showing `welcome` under the logo, then the
    /// tutorial with its code in `font` (a monospace face).
    pub fn new(ui: &Ui<M>, welcome: &str, font: &FontConfig) -> UiResult<StartPage<M>> {
        let control = Control::new(ui, &NodeSpec::new(NodeKind::Custom, Rect::default()))?;
        let theme = ui.theme_handle();
        let welcome = welcome.to_string();
        let font = font.clone();
        // The logo at the last size it was drawn, so a repaint does not resample
        // it; a DPI change draws a different size and replaces it.
        let cache: RefCell<Option<Image>> = RefCell::new(None);
        let scroll = Rc::new(Scroll {
            offset: Cell::new(0),
            max: Cell::new(0),
        });

        let painted = Rc::clone(&scroll);
        control.set_painter(Rc::new(move |canvas| {
            let theme = theme.get();
            canvas.clear(theme.background);
            paint(canvas, &cache, &welcome, &font, &theme, &painted);
        }));

        let id = control.id();
        let ui = ui.clone();
        control.on_events(move |event| {
            if let Event::MouseWheel {
                delta,
                horizontal: false,
                ..
            } = event
            {
                let step = WHEEL_STEP.to_px(ui.dpi()).value();
                let target = scroll.offset.get() - i32::from(*delta) * step / WHEEL_NOTCH;
                let target = target.clamp(0, scroll.max.get());
                if target != scroll.offset.get() {
                    scroll.offset.set(target);
                    ui.invalidate(id);
                }
            }
            None
        });
        Ok(StartPage { control })
    }

    /// The node's identity, for placing it on a page.
    pub fn id(&self) -> WidgetId {
        self.control.id()
    }
}

/// The greedy word wrap of `text` into lines no wider than `width` pixels.
fn wrap(canvas: &mut dyn Canvas, text: &str, style: &TextStyle, width: i32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if !line.is_empty() && canvas.measure_text(&candidate, style).width > width {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The colour runs of one code line: `(start, end, colour)` in chars, covering
/// the whole line, with untokenised gaps in the plain text colour.
fn runs(
    tokens: &[Token],
    length: usize,
    editor: &EditorTheme,
) -> Vec<(usize, usize, xui_core::Color)> {
    let mut runs = Vec::new();
    let mut cursor = 0;
    for token in tokens {
        let (start, end) = (token.start.min(length), token.end.min(length));
        if start < cursor || end <= start {
            continue;
        }
        if start > cursor {
            runs.push((cursor, start, editor.text));
        }
        runs.push((start, end, editor.token_color(token.class)));
        cursor = end;
    }
    if cursor < length {
        runs.push((cursor, length, editor.text));
    }
    runs
}

/// The sizes every block is laid out with, in device pixels.
struct Metrics {
    dpi: u32,
    left: i32,
    width: i32,
}

impl Metrics {
    fn px(&self, value: Dip) -> i32 {
        value.to_px(self.dpi).value()
    }
}

/// Draws the logo, `welcome` and the tutorial in one scrolling column, and
/// records how far the page can scroll.
fn paint(
    canvas: &mut dyn Canvas,
    cache: &RefCell<Option<Image>>,
    welcome: &str,
    font: &FontConfig,
    theme: &Theme,
    scroll: &Scroll,
) {
    let bounds = canvas.bounds();
    canvas.push_clip(bounds);
    let metrics = Metrics {
        dpi: canvas.dpi(),
        left: 0,
        width: 0,
    };
    let column = metrics
        .px(COLUMN)
        .min(bounds.width() - 2 * metrics.px(MARGIN))
        .max(1);
    let metrics = Metrics {
        left: bounds.left + (bounds.width() - column) / 2,
        width: column,
        ..metrics
    };
    let top = bounds.top + metrics.px(MARGIN) - scroll.offset.get();
    let bottom = paint_header(canvas, cache, welcome, theme, bounds, &metrics, top);
    let end = paint_tutorial(canvas, font, theme, bounds, &metrics, bottom);

    let content = end + scroll.offset.get() + metrics.px(MARGIN) - bounds.top;
    let max = (content - bounds.height()).max(0);
    scroll.max.set(max);
    if scroll.offset.get() > max {
        scroll.offset.set(max);
    }
    if max > 0 {
        let track = bounds.height();
        let thumb = (track * track / content).max(metrics.px(MARGIN));
        let y = bounds.top + (track - thumb) * scroll.offset.get() / max;
        let width = metrics.px(THUMB);
        let rect = Rect::new(bounds.right - width - 2, y, bounds.right - 2, y + thumb);
        canvas.fill_rounded_rect(rect, width as f32 / 2.0, theme.text_disabled);
    }
    canvas.pop_clip();
}

/// Draws the logo and `welcome`, centred; returns the y just below them.
fn paint_header(
    canvas: &mut dyn Canvas,
    cache: &RefCell<Option<Image>>,
    welcome: &str,
    theme: &Theme,
    bounds: Rect,
    metrics: &Metrics,
    top: i32,
) -> i32 {
    let style = TextStyle::new(theme.text, TEXT_SIZE).centered();
    let text_height = canvas.measure_text(welcome, &style).height;
    let side = metrics.px(LOGO_SIZE).max(1);
    let mut text_top = top;
    if let Some(source) = logo() {
        let left = bounds.left + (bounds.width() - side) / 2;
        let rect = Rect::new(left, top, left + side, top + side);
        let mut cached = cache.borrow_mut();
        let side_px = u32::try_from(side).unwrap_or(1);
        if cached.as_ref().is_none_or(|image| image.width() != side_px) {
            *cached = scaled(source, side_px);
        }
        if let Some(image) = cached.as_ref() {
            canvas.draw_image(image, rect);
        }
        text_top = top + side + metrics.px(GAP);
    }
    let text = Rect::new(bounds.left, text_top, bounds.right, text_top + text_height);
    canvas.draw_text(welcome, text, &style);
    text_top + text_height + metrics.px(GAP) * 2
}

/// Draws the tutorial blocks from `top`; returns the y just below the last.
fn paint_tutorial(
    canvas: &mut dyn Canvas,
    font: &FontConfig,
    theme: &Theme,
    bounds: Rect,
    metrics: &Metrics,
    top: i32,
) -> i32 {
    let editor = EditorTheme::from_theme(*theme);
    let body = TextStyle::new(theme.text, TEXT_SIZE);
    let heading = TextStyle::new(theme.text, HEADING_SIZE).bold();
    let mono = font.style(editor.text);
    let gap = metrics.px(BLOCK_GAP);
    let pad = metrics.px(PADDING);
    let mut y = top;
    let visible = |from: i32, to: i32| to >= bounds.top && from <= bounds.bottom;

    for block in tutorial::BLOCKS {
        match block {
            tutorial::Block::Heading(text) => {
                y += metrics.px(HEADING_GAP);
                let height = canvas.measure_text(text, &heading).height;
                if visible(y, y + height) {
                    let rect = Rect::new(metrics.left, y, metrics.left + metrics.width, y + height);
                    canvas.draw_text(text, rect, &heading);
                    let rule = y + height + 2;
                    canvas.fill_rect(
                        Rect::new(metrics.left, rule, metrics.left + metrics.width, rule + 1),
                        theme.border,
                    );
                }
                y += height + gap;
            }
            tutorial::Block::Para(text) => {
                y = paint_lines(canvas, &body, text, metrics, y, 0, bounds) + gap;
            }
            tutorial::Block::Callout(text) => {
                let lines = wrap(canvas, text, &body, metrics.width - 2 * pad);
                let line_height = canvas.measure_text("Mg", &body).height;
                let height = lines.len() as i32 * line_height + 2 * pad;
                if visible(y, y + height) {
                    let rect = Rect::new(metrics.left, y, metrics.left + metrics.width, y + height);
                    canvas.fill_rect(rect, theme.surface);
                    let bar = metrics.px(dip(4.0));
                    canvas.fill_rect(
                        Rect::new(rect.left, rect.top, rect.left + bar, rect.bottom),
                        theme.accent,
                    );
                    for (index, line) in lines.iter().enumerate() {
                        let row_top = y + pad + index as i32 * line_height;
                        let row = Rect::new(
                            metrics.left + bar + pad,
                            row_top,
                            rect.right - pad,
                            row_top + line_height,
                        );
                        canvas.draw_text(line, row, &body);
                    }
                }
                y += height + gap;
            }
            tutorial::Block::Code(code) => {
                y = paint_code(canvas, &mono, &editor, code, metrics, y, bounds) + gap;
            }
        }
    }
    y
}

/// Draws wrapped `text` in `style` starting at `y`, indented by `indent`
/// pixels; returns the y just below it.
fn paint_lines(
    canvas: &mut dyn Canvas,
    style: &TextStyle,
    text: &str,
    metrics: &Metrics,
    y: i32,
    indent: i32,
    bounds: Rect,
) -> i32 {
    let lines = wrap(canvas, text, style, metrics.width - indent);
    let line_height = canvas.measure_text("Mg", style).height;
    for (index, line) in lines.iter().enumerate() {
        let row_top = y + index as i32 * line_height;
        if row_top + line_height >= bounds.top && row_top <= bounds.bottom {
            let row = Rect::new(
                metrics.left + indent,
                row_top,
                metrics.left + metrics.width,
                row_top + line_height,
            );
            canvas.draw_text(line, row, style);
        }
    }
    y + lines.len() as i32 * line_height
}

/// Draws a highlighted code block from `y`; returns the y just below it.
fn paint_code(
    canvas: &mut dyn Canvas,
    style: &TextStyle,
    editor: &EditorTheme,
    code: &str,
    metrics: &Metrics,
    y: i32,
    bounds: Rect,
) -> i32 {
    let pad = metrics.px(PADDING);
    let line_height = canvas.measure_text("Mg", style).height;
    let lines: Vec<&str> = code.lines().collect();
    let height = lines.len() as i32 * line_height + 2 * pad;
    if y + height < bounds.top || y > bounds.bottom {
        return y + height;
    }
    let rect = Rect::new(metrics.left, y, metrics.left + metrics.width, y + height);
    canvas.fill_rect(rect, editor.background);
    canvas.stroke_rect(rect, editor.border, 1.0);
    canvas.push_clip(rect);
    let highlighter = RhaiHighlighter;
    let mut state = LineState::default();
    for (index, line) in lines.iter().enumerate() {
        let (tokens, next) = highlighter.lex_line(line, &state);
        state = next;
        let chars: Vec<char> = line.chars().collect();
        let row_top = y + pad + index as i32 * line_height;
        for (start, end, color) in runs(&tokens, chars.len(), editor) {
            let text: String = chars[start..end].iter().collect();
            if text.trim().is_empty() {
                continue;
            }
            // Measured with a trailing probe glyph, in case the backend trims
            // trailing spaces from a run's width.
            let prefix: String = chars[..start].iter().chain(&['x']).collect();
            let probe = canvas.measure_text("x", style).width;
            let x = metrics.left + pad + canvas.measure_text(&prefix, style).width - probe;
            let mut run_style = style.clone();
            run_style.color = color;
            let row = Rect::new(x, row_top, rect.right, row_top + line_height);
            canvas.draw_text(&text, row, &run_style);
        }
    }
    canvas.pop_clip();
    y + height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_gets_the_logo_as_its_icon() {
        use xui_core::backend::PlatformSpec;

        let backend = xui_canvas::OffscreenBackend::new();
        let window = backend
            .open_window(&PlatformSpec::new("icon"))
            .expect("the offscreen window opens");
        install_window_icon(&backend, window);
        let icon = backend.window_icon(window).expect("an icon was set");
        assert_eq!((icon.width(), icon.height()), (ICON_SIDE, ICON_SIDE));
    }
}
