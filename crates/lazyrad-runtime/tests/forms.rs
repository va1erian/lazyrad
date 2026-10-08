//! Integration tests for the form loader and VB-style event wiring.
//!
//! The click tests build a form on the offscreen backend, inject a click at the
//! control's centre and check the handler ran. One test loads the shipped
//! "Hello" sample; the others use in-memory forms built with
//! [`FormRuntime::from_sources`], so they need no files.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use lazyrad_project::{FormDoc, Node};
use lazyrad_runtime::form::{FormRuntime, FormSource, ModuleSource, Msg};
use xui_canvas::OffscreenBackend;
use xui_core::app::{Ui, run_app};
use xui_core::backend::{Backend, Event, PlatformSpec};
use xui_core::message::{Modifiers, MouseButton};
use xui_core::units::Dip;
use xui_form::{LiveForm, Value};

/// The sample project shipped with the repository.
fn sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/hello")
}

/// The offscreen window spec the tests use.
fn spec() -> PlatformSpec {
    PlatformSpec::new("lazyrad-runtime forms").size(Dip(320.0), Dip(200.0))
}

/// Injects a left click at the centre of `name`.
fn click(backend: &OffscreenBackend, ui: &Ui<Msg>, form: &LiveForm<Msg>, name: &str) {
    let id = form
        .widget(name)
        .unwrap_or_else(|| panic!("`{name}` exists"))
        .id();
    let bounds = ui.bounds(id);
    let window = ui.window();
    let x = bounds.left + bounds.width() / 2;
    let y = bounds.top + bounds.height() / 2;
    let modifiers = Modifiers::NONE;
    let _ = backend.inject(
        window,
        Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            modifiers,
        },
    );
    let _ = backend.inject(
        window,
        Event::MouseMove {
            x: x + 1,
            y,
            modifiers,
        },
    );
    let _ = backend.inject(
        window,
        Event::MouseUp {
            x,
            y,
            button: MouseButton::Left,
            modifiers,
        },
    );
}

/// Builds `form` offscreen and returns the live form once the loop has run.
fn capture_form(runtime: Rc<FormRuntime>, form: &str) -> Rc<LiveForm<Msg>> {
    let backend = Rc::new(OffscreenBackend::new());
    let capture: Rc<RefCell<Option<Rc<LiveForm<Msg>>>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&capture);
    run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
        let app = runtime.build_app(ui, form).expect("the form builds");
        *slot.borrow_mut() = Some(app.root_form().expect("the form is live").clone());
        app
    })
    .expect("the event loop runs");

    capture.borrow_mut().take().expect("the form was captured")
}

#[test]
fn clicking_the_sample_hello_button_updates_the_label() {
    let runtime = FormRuntime::load(sample_dir()).expect("the sample project loads");
    let backend = Rc::new(OffscreenBackend::new());
    let backend_for_click = Rc::clone(&backend);
    let capture: Rc<RefCell<Option<Rc<LiveForm<Msg>>>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&capture);

    run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
        let app = runtime
            .build_app(ui, "main_form")
            .expect("main_form builds");
        let form = app.root_form().expect("the form is live").clone();
        form.set("name_edit", "text", &Value::Text("World".to_owned()))
            .expect("the name field is writable");
        click(&backend_for_click, ui, &form, "hello_button");
        *slot.borrow_mut() = Some(form);
        app
    })
    .expect("the event loop runs");

    let form = capture.borrow_mut().take().expect("the form was captured");
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("Hello, World!".to_owned()))
    );
}

#[test]
fn a_missing_handler_leaves_the_event_unwired() {
    let mut doc = FormDoc::new("main_form");
    let mut button = Node::new("Button", "cmdNo");
    button.set_prop("left", Value::Int(10));
    button.set_prop("top", Value::Int(10));
    button.set_prop("width", Value::Int(100));
    button.set_prop("height", Value::Int(28));
    button.set_prop("text", Value::Text("No handler".to_owned()));
    doc.insert(button);

    let mut label = Node::new("Label", "result_label");
    label.set_prop("left", Value::Int(10));
    label.set_prop("top", Value::Int(50));
    label.set_prop("width", Value::Int(160));
    label.set_prop("text", Value::Text("before".to_owned()));
    doc.insert(label);

    let runtime = FormRuntime::from_sources(
        vec![FormSource::new(
            "main_form",
            doc,
            "fn something_else() { result_label.text = \"after\"; }",
        )],
        Vec::new(),
    );

    let backend = Rc::new(OffscreenBackend::new());
    let backend_for_click = Rc::clone(&backend);
    let capture: Rc<RefCell<Option<Rc<LiveForm<Msg>>>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&capture);

    run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
        let app = runtime
            .build_app(ui, "main_form")
            .expect("main_form builds");
        let form = app.root_form().expect("the form is live").clone();
        click(&backend_for_click, ui, &form, "cmdNo");
        *slot.borrow_mut() = Some(form);
        app
    })
    .expect("the event loop runs");

    let form = capture.borrow_mut().take().expect("the form was captured");
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("before".to_owned())),
        "clicking a control with no handler must not run anything"
    );
}

#[test]
fn showing_another_form_opens_a_secondary_window() {
    let mut main = FormDoc::new("main_form");
    let mut open = Node::new("Button", "open_button");
    open.set_prop("left", Value::Int(10));
    open.set_prop("top", Value::Int(10));
    open.set_prop("width", Value::Int(100));
    open.set_prop("height", Value::Int(28));
    open.set_prop("text", Value::Text("Open".to_owned()));
    main.insert(open);

    let runtime = FormRuntime::from_sources(
        vec![
            FormSource::new(
                "main_form",
                main,
                "fn open_button_click() { other_form.show(); }",
            ),
            FormSource::new("other_form", FormDoc::new("other_form"), ""),
        ],
        Vec::new(),
    );

    let backend = Rc::new(OffscreenBackend::new());
    let backend_for_click = Rc::clone(&backend);
    let runtime_for_app = Rc::clone(&runtime);
    run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
        let app = runtime_for_app
            .build_app(ui, "main_form")
            .expect("main_form builds");
        let form = app.root_form().expect("the form is live").clone();
        click(&backend_for_click, ui, &form, "open_button");
        app
    })
    .expect("the event loop runs");

    assert!(
        runtime.is_open("other_form"),
        "other_form.show() opens a secondary window"
    );
}

#[test]
fn a_standard_module_can_be_imported_by_name() {
    let mut doc = FormDoc::new("main_form");
    let mut label = Node::new("Label", "result_label");
    label.set_prop("left", Value::Int(10));
    label.set_prop("top", Value::Int(10));
    label.set_prop("width", Value::Int(160));
    doc.insert(label);

    let runtime = FormRuntime::from_sources(
        vec![FormSource::new(
            "main_form",
            doc,
            "fn form_load() { import \"util\" as util; result_label.text = util::greeting(\"Grace\"); }",
        )],
        vec![ModuleSource::new(
            "util",
            "fn greeting(name) { `Hello, ${name}!` }",
        )],
    );

    let form = capture_form(runtime, "main_form");
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("Hello, Grace!".to_owned()))
    );
}

#[test]
fn a_message_box_does_not_block_the_handler() {
    let mut doc = FormDoc::new("main_form");
    let mut label = Node::new("Label", "result_label");
    label.set_prop("left", Value::Int(10));
    label.set_prop("top", Value::Int(10));
    label.set_prop("width", Value::Int(160));
    doc.insert(label);

    let runtime = FormRuntime::from_sources(
        vec![FormSource::new(
            "main_form",
            doc,
            "fn form_load() { msg_box(\"Hello\"); result_label.text = \"after\"; }",
        )],
        Vec::new(),
    );

    let form = capture_form(runtime, "main_form");
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("after".to_owned())),
        "the handler continues past a non-blocking msg_box"
    );
}

#[test]
fn a_form_script_can_use_the_standard_library() {
    let mut doc = FormDoc::new("main_form");
    let mut label = Node::new("Label", "result_label");
    label.set_prop("left", Value::Int(10));
    label.set_prop("top", Value::Int(10));
    label.set_prop("width", Value::Int(160));
    doc.insert(label);

    let runtime = FormRuntime::from_sources(
        vec![FormSource::new(
            "main_form",
            doc,
            "fn form_load() { result_label.text = [(144.0).sqrt().to_int(), today().len()].join(\"/\"); }",
        )],
        Vec::new(),
    );

    let form = capture_form(runtime, "main_form");
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("12/10".to_owned()))
    );
}

#[test]
fn a_standard_module_function_is_callable_from_a_form() {
    let mut doc = FormDoc::new("main_form");
    let mut label = Node::new("Label", "result_label");
    label.set_prop("left", Value::Int(10));
    label.set_prop("top", Value::Int(10));
    label.set_prop("width", Value::Int(160));
    doc.insert(label);

    let runtime = FormRuntime::from_sources(
        vec![FormSource::new(
            "main_form",
            doc,
            "fn form_load() { result_label.text = greeting(\"Ada\"); }",
        )],
        vec![ModuleSource::new(
            "util",
            "fn greeting(name) { `Hello, ${name}!` }",
        )],
    );

    let form = capture_form(runtime, "main_form");
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("Hello, Ada!".to_owned()))
    );
}

#[test]
fn a_top_level_import_alias_resolves_in_handlers() {
    let mut doc = FormDoc::new("main_form");
    let mut label = Node::new("Label", "result_label");
    label.set_prop("left", Value::Int(10));
    label.set_prop("top", Value::Int(10));
    label.set_prop("width", Value::Int(160));
    doc.insert(label);

    let runtime = FormRuntime::from_sources(
        vec![FormSource::new(
            "main_form",
            doc,
            "import \"util\" as u;
fn form_load() { result_label.text = u::greeting(\"Lin\"); }",
        )],
        vec![ModuleSource::new(
            "util",
            "fn greeting(name) { `Hello, ${name}!` }",
        )],
    );

    let form = capture_form(runtime, "main_form");
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("Hello, Lin!".to_owned()))
    );
}

#[test]
fn top_level_code_runs_once_and_extra_handler_parameters_are_unit() {
    let mut doc = FormDoc::new("main_form");
    let mut button = Node::new("Button", "go_button");
    button.set_prop("left", Value::Int(10));
    button.set_prop("top", Value::Int(10));
    button.set_prop("width", Value::Int(100));
    button.set_prop("height", Value::Int(28));
    doc.insert(button);
    let mut label = Node::new("Label", "result_label");
    label.set_prop("left", Value::Int(10));
    label.set_prop("top", Value::Int(50));
    label.set_prop("width", Value::Int(160));
    doc.insert(label);

    // `sender` is not supplied by `Click`; it arrives as `()`.
    let runtime = FormRuntime::from_sources(
        vec![FormSource::new(
            "main_form",
            doc,
            "result_label.text += \"x\";
             fn go_button_click(sender) { if sender == () { result_label.text += \"c\"; } }",
        )],
        Vec::new(),
    );
    let backend = Rc::new(OffscreenBackend::new());
    let backend_for_click = Rc::clone(&backend);
    let capture: Rc<RefCell<Option<Rc<LiveForm<Msg>>>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&capture);
    run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
        let app = runtime.build_app(ui, "main_form").expect("the form builds");
        let form = app.root_form().expect("the form is live").clone();
        click(&backend_for_click, ui, &form, "go_button");
        click(&backend_for_click, ui, &form, "go_button");
        *slot.borrow_mut() = Some(form);
        app
    })
    .expect("the event loop runs");

    let form = capture.borrow_mut().take().expect("the form was captured");
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("xcc".to_owned()))
    );
}

/// The tutorial's "Remember values between events" example, verbatim: a
/// counter kept in `form.state`, bumped by two clicks.
pub const FORM_STATE_COUNTER: &str = r#"fn form_load() { form.state.count = 0; }
fn button1_click() {
    form.state.count += 1;
    edit1.text = `Clicked ${form.state.count} times`;
}"#;

#[test]
fn form_state_keeps_a_counter_between_two_clicks() {
    let mut doc = FormDoc::new("main_form");
    let mut button = Node::new("Button", "button1");
    button.set_prop("left", Value::Int(10));
    button.set_prop("top", Value::Int(10));
    button.set_prop("width", Value::Int(100));
    button.set_prop("height", Value::Int(28));
    doc.insert(button);
    let mut edit = Node::new("Edit", "edit1");
    edit.set_prop("left", Value::Int(10));
    edit.set_prop("top", Value::Int(50));
    edit.set_prop("width", Value::Int(200));
    doc.insert(edit);

    let runtime = FormRuntime::from_sources(
        vec![FormSource::new("main_form", doc, FORM_STATE_COUNTER)],
        Vec::new(),
    );
    let backend = Rc::new(OffscreenBackend::new());
    let backend_for_click = Rc::clone(&backend);
    let capture: Rc<RefCell<Option<Rc<LiveForm<Msg>>>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&capture);
    run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
        let app = runtime.build_app(ui, "main_form").expect("the form builds");
        let form = app.root_form().expect("the form is live").clone();
        click(&backend_for_click, ui, &form, "button1");
        click(&backend_for_click, ui, &form, "button1");
        *slot.borrow_mut() = Some(form);
        app
    })
    .expect("the event loop runs");

    let form = capture.borrow_mut().take().expect("the form was captured");
    assert_eq!(
        form.get("edit1", "text"),
        Some(Value::Text("Clicked 2 times".to_owned()))
    );
}

#[test]
fn a_script_reads_a_project_asset_from_a_folder() {
    let dir = std::env::temp_dir().join(format!("lazyrad-runtime-asset-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("songs")).expect("scratch directory is created");
    std::fs::write(
        dir.join("app.lrp"),
        "name = \"app\"\nversion = \"1\"\nstartup = \"main_form\"\nassets = [\"songs/*.mod\"]\n\n\
         [[items]]\nkind = \"form\"\nname = \"main_form\"\n\
         layout = \"main_form.lfm\"\ncode = \"main_form.rhai\"\n",
    )
    .expect("project writes");
    std::fs::write(
        dir.join("main_form.lfm"),
        "format = 1\n\n[window]\nname = \"main_form\"\ntitle = \"A\"\n\n\
         [[node]]\nkind = \"Label\"\nname = \"result_label\"\n\
         left = 10\ntop = 10\nwidth = 200\n",
    )
    .expect("form writes");
    std::fs::write(
        dir.join("main_form.rhai"),
        "fn form_load() { result_label.text = file_read_text(\"songs/song.mod\"); }",
    )
    .expect("code writes");
    std::fs::write(dir.join("songs/song.mod"), b"MODDATA").expect("asset writes");

    let runtime = FormRuntime::load(&dir).expect("the project loads");
    let form = capture_form(runtime, "main_form");
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("MODDATA".to_owned())),
        "a project-relative asset is readable from a folder"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_handler_observer_hears_each_successful_handler_once() {
    let runtime = FormRuntime::load(sample_dir()).expect("the sample project loads");
    let heard: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&heard);
    runtime.set_handler_observer(Rc::new(move |form, control, event| {
        sink.borrow_mut().push(format!("{form}.{control}.{event}"));
    }));
    let backend = Rc::new(OffscreenBackend::new());
    let backend_for_click = Rc::clone(&backend);

    run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
        let app = runtime
            .build_app(ui, "main_form")
            .expect("main_form builds");
        let form = app.root_form().expect("the form is live").clone();
        click(&backend_for_click, ui, &form, "hello_button");
        app
    })
    .expect("the event loop runs");

    let heard = heard.borrow();
    assert_eq!(
        heard
            .iter()
            .filter(|e| e.ends_with("hello_button.Click"))
            .count(),
        1,
        "heard: {heard:?}"
    );
}

#[test]
fn a_form_script_can_call_a_host_extension() {
    // The LazyOS player registers its `msg` module this way; a plain function
    // stands in for it here.
    lazyrad_runtime::extensions::clear();
    lazyrad_runtime::extensions::add(|engine| {
        engine.register_fn("host_greeting", |name: &str| {
            format!("hi {name} from the host")
        });
    });

    let mut doc = FormDoc::new("main_form");
    let mut label = Node::new("Label", "result_label");
    label.set_prop("left", Value::Int(10));
    label.set_prop("top", Value::Int(10));
    label.set_prop("width", Value::Int(200));
    doc.insert(label);
    let runtime = FormRuntime::from_sources(
        vec![FormSource::new(
            "main_form",
            doc,
            "fn form_load() { result_label.text = host_greeting(\"form\"); }",
        )],
        Vec::new(),
    );

    let form = capture_form(runtime, "main_form");
    lazyrad_runtime::extensions::clear();
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("hi form from the host".to_owned()))
    );
}
