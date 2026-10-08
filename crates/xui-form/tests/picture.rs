//! Offscreen tests of the `PictureBox` control: loading, fitting, zooming,
//! rotating, panning by drag, the rendered pixels and the events it raises.

use std::cell::RefCell;
use std::rc::Rc;

use xui_canvas::OffscreenBackend;
use xui_core::Dip;
use xui_core::app::{App, Ui, run_app};
use xui_core::backend::{Backend, Event, PlatformSpec, WindowId};
use xui_core::image::Image;
use xui_core::message::{Key, Modifiers, MouseButton};
use xui_form::{
    Binder, BuildOptions, CallError, Catalog, EventHandler, EventRef, Factories, FormDoc, LiveForm,
    Node, SetError, Value, build_with,
};

/// An event the binder turned into a message.
#[derive(Clone, Debug, PartialEq)]
struct Raised {
    event: String,
    args: Vec<Value>,
}

/// Binds every event.
struct AllEvents;

impl Binder<Raised> for AllEvents {
    fn bind(&self, event: EventRef<'_>) -> Option<EventHandler<Raised>> {
        let name = event.event.to_owned();
        Some(Rc::new(move |args| {
            Some(Raised {
                event: name.clone(),
                args: args.to_vec(),
            })
        }))
    }
}

/// Records every message.
struct Recorder(Rc<RefCell<Vec<Raised>>>);

impl App for Recorder {
    type Msg = Raised;

    fn update(&mut self, msg: Raised, _ui: &mut Ui<Raised>) {
        self.0.borrow_mut().push(msg);
    }
}

/// A form with one 200x100 picture box at (0, 0) on a black background.
fn build(ui: &Ui<Raised>, design_mode: bool) -> LiveForm<Raised> {
    let mut node = Node::new("PictureBox", "picture1");
    for (prop, value) in [("left", 0), ("top", 0), ("width", 200), ("height", 100)] {
        node.set_prop(prop, Value::Int(value));
    }
    node.set_prop("background", Value::Color(xui_core::Color::rgb(0, 0, 0)));
    let mut doc = FormDoc::new("main_form");
    doc.insert(node);
    let factories: Factories<Raised> = Factories::xui();
    build_with(
        ui,
        &doc,
        &Catalog::xui(),
        &factories,
        &AllEvents,
        BuildOptions {
            design_mode,
            container: None,
        },
    )
    .expect("the form builds")
}

/// Runs `check` in a 200x100 window and returns the messages raised.
fn session(check: impl FnOnce(&Rc<OffscreenBackend>, &Ui<Raised>) + 'static) -> Vec<Raised> {
    let backend = Rc::new(OffscreenBackend::new());
    let messages = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::clone(&messages);
    let inner = Rc::clone(&backend);
    let spec = PlatformSpec::new("picture tests").size(Dip(200.0), Dip(100.0));
    run_app(backend as Rc<dyn Backend>, spec, move |ui| {
        check(&inner, ui);
        Recorder(log)
    })
    .expect("run_app succeeds");
    messages.take()
}

/// A PNG `w`x`h` whose left half is red and right half is blue.
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut pixels = Vec::new();
    for _y in 0..h {
        for x in 0..w {
            let color = if x < w / 2 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            };
            pixels.extend_from_slice(&color);
        }
    }
    Image::from_rgba(w, h, pixels)
        .expect("a valid image")
        .encode_png()
        .expect("encodes")
}

fn load(form: &LiveForm<Raised>, bytes: Vec<u8>) -> Result<Option<Value>, CallError> {
    form.call("picture1", "load", &[Value::Bytes(bytes.into())])
}

fn pixel(backend: &OffscreenBackend, window: WindowId, x: u32, y: u32) -> [u8; 3] {
    let image = backend.render(window).expect("the window renders");
    let [r, g, b, _] = image.pixel(x, y).expect("inside the window");
    [r, g, b]
}

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BLACK: [u8; 3] = [0, 0, 0];

#[test]
fn a_loaded_picture_reports_its_size_and_format() {
    session(|_, ui| {
        let form = build(ui, false);
        assert_eq!(form.get("picture1", "has_image"), Some(Value::Bool(false)));
        load(&form, png(40, 20)).expect("loads");
        assert_eq!(form.get("picture1", "has_image"), Some(Value::Bool(true)));
        assert_eq!(form.get("picture1", "image_width"), Some(Value::Int(40)));
        assert_eq!(form.get("picture1", "image_height"), Some(Value::Int(20)));
        assert_eq!(
            form.get("picture1", "format"),
            Some(Value::Text("PNG".to_owned()))
        );
        form.call("picture1", "clear", &[]).expect("clears");
        assert_eq!(form.get("picture1", "has_image"), Some(Value::Bool(false)));
    });
}

#[test]
fn a_bad_file_is_an_error_and_keeps_the_picture() {
    session(|_, ui| {
        let form = build(ui, false);
        load(&form, png(10, 10)).expect("loads");
        let error = load(&form, b"not a picture".to_vec()).expect_err("refused");
        assert!(matches!(error, CallError::Failed(ref text) if text.contains("not a PNG")));
        assert_eq!(form.get("picture1", "image_width"), Some(Value::Int(10)));
        let error = form
            .call("picture1", "load", &[Value::Text("x".to_owned())])
            .expect_err("a string is not a blob");
        assert!(matches!(error, CallError::WrongArgs(_)), "{error:?}");
    });
}

#[test]
fn a_small_picture_is_centred_at_its_own_size() {
    session(|backend, ui| {
        let form = build(ui, false);
        load(&form, png(40, 20)).expect("loads");
        let window = ui.window();
        // Centred: x 80..120, y 40..60; red on the left half.
        assert_eq!(pixel(backend, window, 85, 50), RED);
        assert_eq!(pixel(backend, window, 115, 50), BLUE);
        assert_eq!(pixel(backend, window, 75, 50), BLACK, "left of the picture");
        assert_eq!(pixel(backend, window, 100, 35), BLACK, "above the picture");
        assert_eq!(form.get("picture1", "zoom"), Some(Value::Float(100.0)));
    });
}

#[test]
fn a_large_picture_is_fitted_and_zoom_turns_fit_off() {
    session(|backend, ui| {
        let form = build(ui, false);
        load(&form, png(400, 100)).expect("loads");
        let window = ui.window();
        // Fitted at 50%: 200x50, centred vertically (y 25..75).
        assert_eq!(form.get("picture1", "zoom"), Some(Value::Float(50.0)));
        assert_eq!(pixel(backend, window, 5, 50), RED);
        assert_eq!(pixel(backend, window, 195, 50), BLUE);
        assert_eq!(pixel(backend, window, 100, 20), BLACK);
        form.call("picture1", "zoom_in", &[]).expect("zooms");
        assert_eq!(form.get("picture1", "zoom"), Some(Value::Float(66.0)));
        assert_eq!(form.get("picture1", "fit"), Some(Value::Bool(false)));
        form.call("picture1", "actual_size", &[]).expect("100%");
        // At 100% the 400 px picture overflows: the centre column is the
        // red/blue boundary.
        assert_eq!(pixel(backend, window, 95, 50), RED);
        assert_eq!(pixel(backend, window, 105, 50), BLUE);
        form.call("picture1", "best_fit", &[]).expect("fits");
        assert_eq!(form.get("picture1", "zoom"), Some(Value::Float(50.0)));
        form.set("picture1", "zoom", &Value::Float(25.0))
            .expect("zoom");
        assert_eq!(form.get("picture1", "fit"), Some(Value::Bool(false)));
        assert_eq!(form.get("picture1", "zoom"), Some(Value::Float(25.0)));
    });
}

#[test]
fn stretch_enlarges_a_small_picture() {
    session(|backend, ui| {
        let form = build(ui, false);
        load(&form, png(20, 10)).expect("loads");
        form.set("picture1", "stretch", &Value::Bool(true))
            .expect("stretch");
        assert_eq!(form.get("picture1", "zoom"), Some(Value::Float(1000.0)));
        assert_eq!(pixel(backend, ui.window(), 5, 5), RED, "fills the box");
        assert_eq!(pixel(backend, ui.window(), 195, 95), BLUE);
    });
}

#[test]
fn rotation_turns_the_picture_and_to_png_exports_it() {
    session(|backend, ui| {
        let form = build(ui, false);
        load(&form, png(40, 20)).expect("loads");
        form.call("picture1", "rotate_cw", &[]).expect("rotates");
        assert_eq!(form.get("picture1", "rotation"), Some(Value::Int(90)));
        let window = ui.window();
        // Now 20 wide and 40 tall (x 90..110, y 30..70); red went to the top.
        assert_eq!(pixel(backend, window, 100, 35), RED);
        assert_eq!(pixel(backend, window, 100, 65), BLUE);
        assert_eq!(
            form.get("picture1", "image_width"),
            Some(Value::Int(40)),
            "the size is the picture's own"
        );
        let Ok(Some(Value::Bytes(exported))) = form.call("picture1", "to_png", &[]) else {
            panic!("to_png returns bytes");
        };
        let exported = Image::decode(&exported).expect("a PNG");
        assert_eq!(exported.size(), (20, 40));
        form.call("picture1", "rotate_ccw", &[])
            .expect("rotates back");
        form.call("picture1", "rotate_ccw", &[]).expect("rotates");
        assert_eq!(form.get("picture1", "rotation"), Some(Value::Int(270)));
        form.set("picture1", "rotation", &Value::Int(180))
            .expect("set");
        assert_eq!(pixel(backend, window, 85, 50), BLUE, "upside down");
        assert_eq!(
            form.set("picture1", "rotation", &Value::Int(45)),
            Err(SetError::TypeMismatch)
        );
        assert_eq!(
            form.set("picture1", "image_width", &Value::Int(1)),
            Err(SetError::ReadOnly)
        );
    });
}

#[test]
fn dragging_pans_a_zoomed_picture_and_a_still_press_is_a_click() {
    let messages = session(|backend, ui| {
        let form = build(ui, false);
        load(&form, png(400, 100)).expect("loads");
        form.call("picture1", "actual_size", &[]).expect("100%");
        let window = ui.window();
        let mouse = |event: fn(i32, i32) -> Event, x, y| backend.inject(window, event(x, y));
        let down = |x, y| Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            modifiers: Modifiers::NONE,
        };
        let moved = |x, y| Event::MouseMove {
            x,
            y,
            modifiers: Modifiers::NONE,
        };
        let up = |x, y| Event::MouseUp {
            x,
            y,
            button: MouseButton::Left,
            modifiers: Modifiers::NONE,
        };
        // Drag far right: the picture's left edge (red) comes into view and
        // stops there.
        mouse(down, 100, 50);
        mouse(moved, 150, 50);
        mouse(moved, 900, 50);
        mouse(up, 900, 50);
        assert_eq!(pixel(backend, window, 1, 50), RED);
        assert_eq!(pixel(backend, window, 199, 50), RED, "clamped at the edge");
        // A press and release in place is a click, and focuses the box.
        mouse(down, 30, 40);
        mouse(up, 31, 40);
        backend.inject(
            window,
            Event::KeyDown {
                key: Key::RIGHT,
                modifiers: Modifiers::NONE,
                repeat: 1,
                system: false,
            },
        );
        backend.inject(
            window,
            Event::MouseWheel {
                delta: -1,
                horizontal: false,
                x: 10,
                y: 10,
                modifiers: Modifiers {
                    ctrl: true,
                    ..Modifiers::NONE
                },
            },
        );
        drop(form);
    });
    let events: Vec<(&str, &[Value])> = messages
        .iter()
        .map(|msg| (msg.event.as_str(), msg.args.as_slice()))
        .collect();
    assert_eq!(
        events,
        [
            ("Click", &[Value::Float(31.0), Value::Float(40.0)][..]),
            ("KeyDown", &[Value::Text("right".to_owned())][..]),
            ("Wheel", &[Value::Float(-1.0), Value::Bool(true)][..]),
        ],
        "the drag raised no click"
    );
}

#[test]
fn design_mode_shows_a_placeholder_and_refuses_methods() {
    session(|_, ui| {
        let form = build(ui, true);
        assert!(
            load(&form, png(4, 4)).is_err(),
            "methods act on a running form"
        );
        form.set("picture1", "zoom", &Value::Float(200.0))
            .expect("zoom is a design property");
    });
}
