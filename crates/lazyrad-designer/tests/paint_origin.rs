//! The designer's `Custom` painters draw inside their own node.
//!
//! Each widget is placed away from the window origin and rendered on the
//! offscreen backend. Its painter must land inside its bounds, and nothing may
//! leak into the window's top-left corner, where a painter that ignored its
//! node's origin would draw.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use xui_canvas::{OffscreenBackend, RgbaImage};
use xui_core::app::{App, Ui, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::geometry::Rect;
use xui_core::units::Dip;
use xui_core::{Color, Theme};
use xui_form::{FormDoc, Node, Value};

use lazyrad_designer::{Designer, PropertyGrid, Toolbox};

/// An app that only keeps its widgets alive.
struct Holder(#[allow(dead_code)] Vec<Box<dyn Any>>);

impl App for Holder {
    type Msg = ();

    fn update(&mut self, _msg: (), _ui: &mut Ui<()>) {}
}

/// The window's size in design units (96dpi, so also pixels).
const WIDTH: u32 = 700;
const HEIGHT: u32 = 400;
/// The window corner no widget covers.
const CORNER: Rect = Rect {
    left: 0,
    top: 0,
    right: 32,
    bottom: 32,
};

/// Builds widgets with `build` and renders the window.
fn render(build: impl FnOnce(&mut Ui<()>) -> Vec<Box<dyn Any>> + 'static) -> RgbaImage {
    let backend = Rc::new(OffscreenBackend::new());
    let trait_backend: Rc<dyn Backend> = Rc::clone(&backend) as Rc<dyn Backend>;
    let image: Rc<RefCell<Option<RgbaImage>>> = Rc::new(RefCell::new(None));
    let image_for_app = Rc::clone(&image);
    let spec = PlatformSpec::new("paint").size(Dip(WIDTH as f32), Dip(HEIGHT as f32));
    run_app(trait_backend, spec, move |ui| {
        let widgets = build(ui);
        *image_for_app.borrow_mut() = backend.render(ui.window());
        Holder(widgets)
    })
    .expect("run_app succeeds");
    let image = image.borrow_mut().take();
    image.expect("the window renders")
}

/// A colour as the image's RGBA bytes.
fn rgba(color: Color) -> [u8; 4] {
    [color.r, color.g, color.b, 255]
}

/// Whether any pixel in `rect` differs from `background`.
fn painted(image: &RgbaImage, rect: Rect, background: [u8; 4]) -> bool {
    let (left, top) = (rect.left as u32, rect.top as u32);
    let (right, bottom) = (rect.right as u32, rect.bottom as u32);
    (top..bottom).any(|y| (left..right).any(|x| image.pixel(x, y) != Some(background)))
}

/// A form with one button.
fn button_doc() -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    let mut button = Node::new("Button", "ok_button");
    button.set_prop("left", Value::Int(16));
    button.set_prop("top", Value::Int(16));
    button.set_prop("width", Value::Int(80));
    button.set_prop("height", Value::Int(24));
    button.set_prop("text", Value::Text("Go".into()));
    doc.insert(button);
    doc
}

#[test]
fn the_toolbox_paints_inside_its_bounds() {
    let bounds = Rect::new(300, 40, 460, 380);
    let image = render(move |ui| {
        let toolbox = Toolbox::new(ui, bounds, |_| ()).expect("the toolbox builds");
        vec![Box::new(toolbox)]
    });
    let theme = Theme::light();
    assert!(
        !painted(&image, CORNER, rgba(theme.background)),
        "the toolbox painted into the window corner"
    );
    assert!(
        painted(&image, bounds.shrink(1), rgba(theme.surface)),
        "the toolbox painted no tiles inside its bounds"
    );
}

#[test]
fn the_property_grid_paints_inside_its_bounds() {
    let designer_bounds = Rect::new(40, 40, 400, 300);
    let grid_bounds = Rect::new(440, 40, 656, 380);
    let image = render(move |ui| {
        let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
        let designer = Designer::new(
            ui,
            designer_bounds,
            button_doc(),
            Rc::clone(&catalog),
            |_| (),
        )
        .expect("the designer builds");
        let designer = Rc::new(RefCell::new(designer));
        let grid = PropertyGrid::new(ui, grid_bounds, Rc::clone(&designer), catalog, |_| ())
            .expect("the grid builds");
        designer.borrow().select_node("ok_button", ui);
        vec![Box::new(designer), Box::new(grid)]
    });
    let theme = Theme::light();
    assert!(
        painted(&image, grid_bounds.shrink(1), rgba(theme.surface)),
        "the grid painted no combo, tabs or rows inside its bounds"
    );
}

#[test]
fn the_designer_overlay_paints_inside_its_bounds() {
    let bounds = Rect::new(200, 100, 560, 360);
    let image = render(move |ui| {
        let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
        let designer =
            Designer::new(ui, bounds, button_doc(), catalog, |_| ()).expect("the designer builds");
        vec![Box::new(designer)]
    });
    let theme = Theme::light();
    assert!(
        !painted(&image, CORNER, rgba(theme.background)),
        "the overlay painted its grid or outlines into the window corner"
    );
}
