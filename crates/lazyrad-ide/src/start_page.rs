#![forbid(unsafe_code)]

//! The Start Page: the LazyRAD logo above a line of welcome text, centred in
//! the document area.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use xui_core::Image;
use xui_core::app::Ui;
use xui_core::backend::{
    Backend, Canvas, NodeKind, NodeSpec, Result as UiResult, TextStyle, WidgetId, WindowId,
};
use xui_core::geometry::Rect;
use xui_core::units::{Dip, dip};
use xui_core::widget::Control;

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

/// The Start Page node. It is one custom node that fills its page, so it paints
/// its own background.
pub struct StartPage<M: 'static> {
    control: Control<M>,
}

impl<M: 'static> StartPage<M> {
    /// Creates a Start Page showing `welcome` under the logo.
    pub fn new(ui: &Ui<M>, welcome: &str) -> UiResult<StartPage<M>> {
        let control = Control::new(ui, &NodeSpec::new(NodeKind::Custom, Rect::default()))?;
        let theme = ui.theme_handle();
        let welcome = welcome.to_string();
        // The logo at the last size it was drawn, so a repaint does not resample
        // it; a DPI change draws a different size and replaces it.
        let cache: RefCell<Option<Image>> = RefCell::new(None);
        control.set_painter(Rc::new(move |canvas| {
            let theme = theme.get();
            canvas.clear(theme.background);
            paint(canvas, &cache, &welcome, theme.text);
        }));
        Ok(StartPage { control })
    }

    /// The node's identity, for placing it on a page.
    pub fn id(&self) -> WidgetId {
        self.control.id()
    }
}

/// Draws the logo and `welcome`, centred as one block.
fn paint(
    canvas: &mut dyn Canvas,
    cache: &RefCell<Option<Image>>,
    welcome: &str,
    color: xui_core::Color,
) {
    let bounds = canvas.bounds();
    let dpi = canvas.dpi();
    let px = |value: Dip| value.to_px(dpi).value();
    let style = TextStyle::new(color, TEXT_SIZE).centered();
    let text_height = canvas.measure_text(welcome, &style).height;
    let side = px(LOGO_SIZE).max(1);
    let mut block = text_height;
    if logo().is_some() {
        block += side + px(GAP);
    }
    let top = (bounds.top + (bounds.height() - block) / 2).max(bounds.top + px(MARGIN));
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
        text_top = top + side + px(GAP);
    }
    let text = Rect::new(bounds.left, text_top, bounds.right, text_top + text_height);
    canvas.draw_text(welcome, text, &style);
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
