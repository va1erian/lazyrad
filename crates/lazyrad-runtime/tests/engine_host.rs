//! Integration tests for the Rhai engine host.
//!
//! Each test builds a form on the offscreen backend, wires an [`EngineHost`] to
//! it, and runs a handler. The backend renders headlessly, so the whole path
//! runs on CI without a display.

use std::cell::RefCell;
use std::rc::Rc;

use lazyrad_project::lazyrad_catalog;
use lazyrad_runtime::{EngineHost, FormHost, StdlibContext};
use xui_canvas::OffscreenBackend;
use xui_core::app::{App, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;
use xui_form::{
    Binder, BuildOptions, EventHandler, EventRef, Factories, FormDoc, LiveForm, Node, Value,
    build_with,
};

/// A binder that wires nothing; these tests exercise the engine, not events.
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

    fn update(&mut self, _msg: (), _ui: &mut xui_core::Ui<()>) {}
}

/// A form with an `Edit` named `name_edit` (holding "Ada") and a `Label` named
/// `result_label`.
fn greeting_doc() -> FormDoc {
    let mut doc = FormDoc::new("frmMain");

    let mut name = Node::new("Edit", "name_edit");
    name.set_prop("left", Value::Int(10));
    name.set_prop("top", Value::Int(10));
    name.set_prop("width", Value::Int(160));
    name.set_prop("height", Value::Int(24));
    name.set_prop("text", Value::Text("Ada".to_owned()));
    doc.insert(name);

    let mut out = Node::new("Label", "result_label");
    out.set_prop("left", Value::Int(10));
    out.set_prop("top", Value::Int(40));
    out.set_prop("width", Value::Int(160));
    out.set_prop("height", Value::Int(20));
    doc.insert(out);

    doc
}

/// Builds `doc` offscreen, wires an [`EngineHost`] to it, runs `check` and
/// returns its result.
fn run_form<R>(doc: &FormDoc, check: impl FnOnce(&EngineHost, &Rc<LiveForm<()>>) -> R) -> R {
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let catalog = lazyrad_catalog();
    let factories: Factories<()> = Factories::xui();
    let binder = NullBinder;
    let slot: Rc<RefCell<Option<R>>> = Rc::new(RefCell::new(None));
    let slot_inner = Rc::clone(&slot);
    let spec = PlatformSpec::new("lazyrad-runtime test").size(Dip(320.0), Dip(200.0));

    run_app(backend, spec, move |ui| {
        let form = Rc::new(
            build_with(
                ui,
                doc,
                &catalog,
                &factories,
                &binder,
                BuildOptions::default(),
            )
            .expect("the form builds"),
        );
        let host = EngineHost::new(
            Rc::clone(&form) as Rc<dyn FormHost>,
            &catalog,
            "frmMain.rhai",
            StdlibContext::headless("frmMain"),
        );
        *slot_inner.borrow_mut() = Some(check(&host, &form));
        TestApp
    })
    .expect("run_app succeeds");

    slot.borrow_mut().take().expect("the check ran")
}

#[test]
fn a_handler_copies_a_text_field_into_a_label() {
    let doc = greeting_doc();
    run_form(&doc, |host, form| {
        let ast = host
            .compile("fn hello_button_click() { result_label.text = name_edit.text; }")
            .expect("the handler compiles");
        let _ = host
            .call(&ast, "hello_button_click")
            .expect("the handler runs");

        assert_eq!(
            form.get("result_label", "text"),
            Some(Value::Text("Ada".to_owned()))
        );
    });
}

#[test]
fn form_state_outlives_a_single_event() {
    let doc = greeting_doc();
    run_form(&doc, |host, form| {
        let ast = host
            .compile(
                "fn first() { form.state.count = 1; }\n\
                 fn bump() { form.state.count += 1; }\n\
                 fn report() { result_label.text = `${form.state.count}`; }",
            )
            .expect("the handlers compile");
        let _ = host.call(&ast, "first").expect("first runs");
        let _ = host.call(&ast, "bump").expect("bump runs");
        let _ = host.call(&ast, "report").expect("report runs");

        assert_eq!(
            form.get("result_label", "text"),
            Some(Value::Text("2".to_owned()))
        );
    });
}

#[test]
fn assigning_a_non_map_to_form_state_is_an_error() {
    let doc = greeting_doc();
    run_form(&doc, |host, _form| {
        let ast = host
            .compile("fn bad() { form.state = 5; }")
            .expect("the handler compiles");
        let error = host.call(&ast, "bad").expect_err("a non-map state fails");
        assert!(error.message.contains("must be a map"), "{}", error.message);
    });
}

#[test]
fn form_title_persists_across_calls() {
    let doc = greeting_doc();
    run_form(&doc, |host, form| {
        let ast = host
            .compile(
                "fn set_cap() { form.title = \"Greeted\"; }\n\
                 fn read_cap() { result_label.text = form.title; }",
            )
            .expect("the handlers compile");
        let _ = host.call(&ast, "set_cap").expect("set_cap runs");
        let _ = host.call(&ast, "read_cap").expect("read_cap runs");

        assert_eq!(
            form.get("result_label", "text"),
            Some(Value::Text("Greeted".to_owned()))
        );
    });
}

#[test]
fn form_hide_hides_every_control() {
    let doc = greeting_doc();
    run_form(&doc, |host, form| {
        let ast = host
            .compile("fn hide_all() { form.hide(); }")
            .expect("the handler compiles");
        let _ = host.call(&ast, "hide_all").expect("hide_all runs");

        assert_eq!(
            form.get("result_label", "visible"),
            Some(Value::Bool(false))
        );
        assert_eq!(form.get("name_edit", "visible"), Some(Value::Bool(false)));
    });
}

#[test]
fn a_runtime_error_reports_the_file_and_line() {
    let doc = greeting_doc();
    run_form(&doc, |host, _form| {
        let ast = host
            .compile("fn bad() {\n    result_label.nope = 1;\n}")
            .expect("the handler compiles");
        let error = host
            .call(&ast, "bad")
            .expect_err("`nope` is not a property");

        assert_eq!(error.file, "frmMain.rhai");
        assert_eq!(error.line, 2);
        assert!(error.column > 0);
    });
}

#[test]
fn an_unknown_control_name_is_a_variable_error() {
    let doc = greeting_doc();
    run_form(&doc, |host, _form| {
        let ast = host
            .compile("fn bad() {\n    ghost.text = \"x\";\n}")
            .expect("the handler compiles");
        let error = host.call(&ast, "bad").expect_err("`ghost` is unknown");

        assert_eq!(error.file, "frmMain.rhai");
        assert_eq!(error.line, 2);
        assert!(error.message.contains("ghost"), "{}", error.message);
    });
}

#[test]
fn a_parse_error_reports_the_file_and_line() {
    let doc = greeting_doc();
    run_form(&doc, |host, _form| {
        let error = host
            .compile("fn bad() {\n    let x = ;\n}")
            .expect_err("the script does not compile");

        assert_eq!(error.file, "frmMain.rhai");
        assert_eq!(error.line, 2);
        assert!(error.column > 0);
    });
}

#[test]
fn a_runaway_handler_is_stopped_by_the_operation_budget() {
    let doc = greeting_doc();
    run_form(&doc, |host, _form| {
        host.set_operation_budget(10_000);
        let ast = host
            .compile("fn spin() { let i = 0; while i < 1000000000 { i += 1; } }")
            .expect("the handler compiles");
        let error = host
            .call(&ast, "spin")
            .expect_err("the budget stops the loop");

        assert!(
            error.message.contains("operation budget exceeded"),
            "{}",
            error.message
        );
    });
}

#[test]
fn stop_terminates_a_running_script() {
    let doc = greeting_doc();
    run_form(&doc, |host, _form| {
        let ast = host
            .compile("fn spin() { let i = 0; while i < 1000000000 { i += 1; } }")
            .expect("the handler compiles");

        host.stop();
        let error = host.call(&ast, "spin").expect_err("a stopped script ends");
        assert!(
            error.message.contains("script stopped"),
            "{}",
            error.message
        );

        host.resume();
        assert!(!host.is_stopped());
    });
}

#[test]
fn a_registered_global_resolves_in_a_handler() {
    let doc = greeting_doc();
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let catalog = lazyrad_catalog();
    let factories: Factories<()> = Factories::xui();
    let binder = NullBinder;
    let spec = PlatformSpec::new("lazyrad-runtime globals").size(Dip(320.0), Dip(200.0));

    run_app(backend, spec, move |ui| {
        let form = Rc::new(
            build_with(
                ui,
                &doc,
                &catalog,
                &factories,
                &binder,
                BuildOptions::default(),
            )
            .expect("the form builds"),
        );
        let mut host = EngineHost::new(
            Rc::clone(&form) as Rc<dyn FormHost>,
            &catalog,
            "frmMain.rhai",
            StdlibContext::headless("frmMain"),
        );
        host.set_global("app", rhai::Dynamic::from("LazyRAD".to_owned()));

        let ast = host
            .compile("fn copy() { result_label.text = app; }")
            .expect("the handler compiles");
        let _ = host.call(&ast, "copy").expect("copy runs");

        assert_eq!(
            form.get("result_label", "text"),
            Some(Value::Text("LazyRAD".to_owned()))
        );
        TestApp
    })
    .expect("run_app succeeds");
}
