//! Script method calls on controls: `canvas1.fill_rect(...)` and friends.
//!
//! Each test builds a form with a `Canvas` and a `Label` on the offscreen
//! backend, wires an [`EngineHost`] to it and runs a script; drawing is checked
//! on the rendered pixels, so the display list is observed end to end.

use std::cell::RefCell;
use std::rc::Rc;

use xui_canvas::{OffscreenBackend, RgbaImage};
use xui_core::app::{App, Ui, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;
use xui_form::{
    Binder, BuildOptions, Catalog, EventHandler, EventRef, Factories, FormDoc, LiveForm, Node,
    Value, build_with,
};
use xui_rhai::{EngineHost, FormHost, ScriptError};

/// A binder that wires nothing; these tests exercise method calls.
struct NullBinder;

impl Binder<()> for NullBinder {
    fn bind(&self, _event: EventRef<'_>) -> Option<EventHandler<()>> {
        None
    }
}

/// An application with no messages of its own.
struct TestApp;

impl App for TestApp {
    type Msg = ();

    fn update(&mut self, _msg: (), _ui: &mut Ui<()>) {}
}

/// A form with a 100x80 `Canvas` named `canvas1` at the origin and a `Label`
/// named `label1` beside it.
fn canvas_doc() -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    let mut canvas = Node::new("Canvas", "canvas1");
    for (name, value) in [("left", 0), ("top", 0), ("width", 100), ("height", 80)] {
        canvas.set_prop(name, Value::Int(value));
    }
    doc.insert(canvas);
    let mut label = Node::new("Label", "label1");
    for (name, value) in [("left", 120), ("top", 0), ("width", 80), ("height", 20)] {
        label.set_prop(name, Value::Int(value));
    }
    doc.insert(label);
    doc
}

/// What a check receives: the engine host, the live form and a renderer.
struct Fixture<'a> {
    host: &'a EngineHost,
    form: &'a Rc<LiveForm<()>>,
    render: &'a dyn Fn() -> RgbaImage,
}

impl Fixture<'_> {
    /// Compiles `script` and runs its `fn run()`.
    fn run(&self, script: &str) -> Result<rhai::Dynamic, ScriptError> {
        let ast = self.host.compile(script)?;
        self.host.call(&ast, "run")
    }

    /// The error message `script`'s `run` fails with.
    fn error(&self, script: &str) -> String {
        self.run(script).expect_err("the script fails").message
    }
}

/// Builds the canvas form offscreen and runs `check` against it.
fn with_canvas<R>(check: impl FnOnce(&Fixture<'_>) -> R) -> R {
    let backend = Rc::new(OffscreenBackend::new());
    let render_backend = Rc::clone(&backend);
    let slot: Rc<RefCell<Option<R>>> = Rc::new(RefCell::new(None));
    let slot_inner = Rc::clone(&slot);
    let spec = PlatformSpec::new("xui-rhai methods").size(Dip(240.0), Dip(120.0));
    run_app(backend as Rc<dyn Backend>, spec, move |ui| {
        let catalog = Catalog::xui();
        let factories: Factories<()> = Factories::xui();
        let form = Rc::new(
            build_with(
                ui,
                &canvas_doc(),
                &catalog,
                &factories,
                &NullBinder,
                BuildOptions::default(),
            )
            .expect("the form builds"),
        );
        let host = EngineHost::new(
            Rc::clone(&form) as Rc<dyn FormHost>,
            &catalog,
            "main_form.rhai",
            (),
        );
        let window = ui.window();
        let render = move || render_backend.render(window).expect("the window renders");
        let fixture = Fixture {
            host: &host,
            form: &form,
            render: &render,
        };
        *slot_inner.borrow_mut() = Some(check(&fixture));
        TestApp
    })
    .expect("run_app succeeds");
    slot.borrow_mut().take().expect("the check ran")
}

/// The RGB of a pixel.
fn rgb(image: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let [r, g, b, _] = image.pixel(x, y).expect("inside the image");
    [r, g, b]
}

#[test]
fn drawing_calls_are_replayed_over_the_background() {
    with_canvas(|fixture| {
        let _ = fixture
            .run(
                r##"fn run() {
                    canvas1.clear(0x0000ff);
                    canvas1.fill_rect(10, 10.5, 20.0, 20, "#ff0000");
                    canvas1.fill_circle(70, 40, 8, rgb_green());
                }
                fn rgb_green() { 0x00ff00 }"##,
            )
            .expect("the drawing runs");
        let image = (fixture.render)();
        assert_eq!(rgb(&image, 5, 5), [0, 0, 255], "clear fills the canvas");
        assert_eq!(rgb(&image, 20, 20), [255, 0, 0], "a string colour");
        assert_eq!(rgb(&image, 70, 40), [0, 255, 0], "an int colour");
    });
}

#[test]
fn clear_replaces_everything_drawn_before() {
    with_canvas(|fixture| {
        let _ = fixture
            .run(
                r##"fn run() {
                    canvas1.fill_rect(0, 0, 50, 50, "#ff0000");
                    canvas1.clear("#202020");
                }"##,
            )
            .expect("the drawing runs");
        let image = (fixture.render)();
        assert_eq!(rgb(&image, 20, 20), [0x20, 0x20, 0x20]);
    });
}

#[test]
fn the_background_is_painted_under_the_drawing() {
    with_canvas(|fixture| {
        let _ = fixture
            .run(r##"fn run() { canvas1.background = 0x336699; }"##)
            .expect("the background is writable");
        assert_eq!(
            fixture.form.get("canvas1", "background"),
            Some(Value::Color(xui_core::Color::rgb(0x33, 0x66, 0x99)))
        );
        let image = (fixture.render)();
        assert_eq!(rgb(&image, 50, 40), [0x33, 0x66, 0x99]);
    });
}

#[test]
fn a_method_on_a_kind_without_it_is_a_clear_error() {
    with_canvas(|fixture| {
        let message = fixture.error(r##"fn run() { label1.fill_rect(0, 0, 1, 1, "#ff0000"); }"##);
        assert!(
            message.contains("unknown method 'fill_rect' on label1 (Label); methods: none"),
            "{message}"
        );
    });
}

#[test]
fn a_wrong_argument_count_names_the_signature() {
    with_canvas(|fixture| {
        let message = fixture.error("fn run() { canvas1.fill_rect(1, 2); }");
        assert!(
            message.contains("canvas1.fill_rect takes 5 arguments (x, y, w, h, color), got 2"),
            "{message}"
        );
        let message = fixture.error("fn run() { canvas1.focus(1); }");
        assert!(message.contains("focus takes 0 arguments"), "{message}");
    });
}

#[test]
fn a_bad_argument_leaves_the_drawing_untouched() {
    with_canvas(|fixture| {
        let _ = fixture
            .run(r##"fn run() { canvas1.clear("#000080"); }"##)
            .expect("the clear runs");
        let before = (fixture.render)();
        for script in [
            r##"fn run() { canvas1.fill_rect(0, 0, 50, 50, "not a colour"); }"##,
            r##"fn run() { canvas1.fill_rect("x", 0, 50, 50, "#ff0000"); }"##,
            r##"fn run() { canvas1.fill_circle(10, 10, -5, "#ff0000"); }"##,
            r##"fn run() { canvas1.text(0, 0, "hi", "#ff0000", 0); }"##,
            r##"fn run() { canvas1.fill_rect(0, 0, 1.0 / 0.0, 50, "#ff0000"); }"##,
        ] {
            let message = fixture.error(script);
            assert!(message.contains("argument"), "{script}: {message}");
        }
        let after = (fixture.render)();
        assert_eq!(before.pixels, after.pixels, "no failed call drew anything");
    });
}

#[test]
fn ints_and_floats_are_interchangeable_arguments() {
    with_canvas(|fixture| {
        let width = fixture
            .run(r##"fn run() { canvas1.text_width("Score", 16) }"##)
            .expect("an int size is widened");
        let width = width.as_float().expect("text_width returns a float");
        assert!(width > 0.0, "{width}");
        let wider = fixture
            .run(r##"fn run() { canvas1.text_width("Score: 100", 16.0) }"##)
            .expect("a float size")
            .as_float()
            .expect("a float");
        assert!(wider > width);
        let empty = fixture
            .run(r##"fn run() { canvas1.text_width("", 16) }"##)
            .expect("empty text")
            .as_float()
            .expect("a float");
        assert_eq!(empty, 0.0);
    });
}

#[test]
fn non_ascii_text_draws_and_measures() {
    with_canvas(|fixture| {
        let width = fixture
            .run(
                r##"fn run() {
                    canvas1.clear("#000000");
                    canvas1.text(4, 4, "Héllo ✓ 日本", "#ffffff", 20);
                    canvas1.text_width("Héllo ✓ 日本", 20)
                }"##,
            )
            .expect("non-ASCII text is fine")
            .as_float()
            .expect("a float");
        assert!(width > 20.0, "{width}");
        let image = (fixture.render)();
        let lit = (4..60).any(|x| (4..30).any(|y| rgb(&image, x, y) != [0, 0, 0]));
        assert!(lit, "the text painted something");
    });
}

#[test]
fn a_script_function_named_like_a_method_does_not_hide_it() {
    with_canvas(|fixture| {
        let _ = fixture
            .run(
                r##"fn clear() { 42 }
                fn text() { "mine" }
                fn run() {
                    canvas1.clear("#00ff00");
                    if clear() != 42 || text() != "mine" { throw "shadowed"; }
                }"##,
            )
            .expect("both the method and the script function run");
        let image = (fixture.render)();
        assert_eq!(rgb(&image, 50, 40), [0, 255, 0]);
    });
}

#[test]
fn query_methods_return_values() {
    with_canvas(|fixture| {
        let down = fixture
            .run(r##"fn run() { canvas1.is_key_down("left") }"##)
            .expect("is_key_down runs");
        assert_eq!(down.as_bool(), Ok(false));
        let unit = fixture
            .run("fn run() { canvas1.focus() }")
            .expect("focus runs");
        assert!(unit.is_unit(), "a method without a result returns ()");
        assert_eq!(
            fixture.form.get("canvas1", "mouse_x"),
            Some(Value::Float(0.0))
        );
    });
}

#[test]
fn read_only_properties_refuse_writes() {
    with_canvas(|fixture| {
        let message = fixture.error("fn run() { canvas1.mouse_x = 3.0; }");
        assert!(message.contains("read-only"), "{message}");
    });
}
