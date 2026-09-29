//! Integration tests for [`ScriptForm`]: building a form on the offscreen
//! backend, running its `.rhai` script and routing events.
//!
//! These exercise the reusable path with no LazyRAD project: a `FormDoc`, a
//! script string and `xui-rhai` alone.

use std::cell::RefCell;
use std::rc::Rc;

use xui_canvas::OffscreenBackend;
use xui_core::app::{App, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;
use xui_form::{Catalog, FormDoc, Node, Value};
use xui_rhai::form::{FormError, ScriptForm, ScriptSource};
use xui_rhai::{EngineHost, Msg, ScriptError};

/// An application with no messages of its own; the test runs the script inline.
struct TestApp;

impl App for TestApp {
    type Msg = Msg;

    fn update(&mut self, _msg: Msg, _ui: &mut xui_core::Ui<Msg>) {}
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
    out.set_prop("text", Value::Text("before".to_owned()));
    doc.insert(out);

    doc
}

/// Builds `doc` with `code`, runs `configure` on the engine, then runs `check`
/// against the built [`ScriptForm`] and returns its result.
fn run<R>(
    doc: &FormDoc,
    code: &str,
    configure: impl FnOnce(&mut EngineHost) -> Result<(), ScriptError> + 'static,
    check: impl FnOnce(&ScriptForm) -> R + 'static,
) -> Result<R, FormError> {
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let catalog = Catalog::xui();
    let spec = PlatformSpec::new("xui-rhai test").size(Dip(320.0), Dip(200.0));
    let slot: Rc<RefCell<Option<Result<R, FormError>>>> = Rc::new(RefCell::new(None));
    let slot_inner = Rc::clone(&slot);
    let doc = doc.clone();
    let code = code.to_owned();
    let catalog_for_app = catalog.clone();

    run_app(backend, spec, move |ui| {
        let result = ScriptForm::build(
            ui,
            &doc,
            &catalog_for_app,
            ScriptSource {
                name: "frmMain",
                code: &code,
                file: "frmMain.rhai",
            },
            (),
            configure,
        )
        .map(|script| check(&script));
        *slot_inner.borrow_mut() = Some(result);
        TestApp
    })
    .expect("run_app succeeds");

    slot.borrow_mut().take().expect("the build ran")
}

#[test]
fn form_load_runs_and_a_click_handler_updates_a_label() {
    let doc = greeting_doc();
    let form = run(
        &doc,
        "fn form_load() { result_label.text = name_edit.text; }\n\
         fn hello_button_click() { result_label.text = `Hi, ${name_edit.text}`; }",
        |_| Ok(()),
        |script| {
            script
                .run("hello_button", "Click", &[])
                .expect("the click handler runs");
            script.live_form().get("result_label", "text")
        },
    )
    .expect("the form builds");

    assert_eq!(form, Some(Value::Text("Hi, Ada".to_owned())));
}

#[test]
fn a_missing_handler_is_not_an_error() {
    let doc = greeting_doc();
    run(
        &doc,
        "",
        |_| Ok(()),
        |script| {
            script
                .run("hello_button", "Click", &[])
                .expect("a missing handler is ignored");
            assert_eq!(
                script.live_form().get("result_label", "text"),
                Some(Value::Text("before".to_owned())),
                "nothing ran"
            );
        },
    )
    .expect("the form builds");
}

#[test]
fn a_handler_with_an_extra_parameter_receives_unit() {
    let doc = greeting_doc();
    let value = run(
        &doc,
        "fn go_button_click(sender) { \
             result_label.text = if sender == () { \"unit\" } else { \"other\" }; \
         }",
        |_| Ok(()),
        |script| {
            script
                .run("go_button", "Click", &[])
                .expect("the click handler runs");
            script.live_form().get("result_label", "text")
        },
    )
    .expect("the form builds");

    assert_eq!(value, Some(Value::Text("unit".to_owned())));
}

#[test]
fn configure_registers_a_module_the_script_imports() {
    let doc = greeting_doc();
    let label = run(
        &doc,
        "fn form_load() { result_label.text = greeting(\"Grace\"); }",
        |host| {
            host.register_module(
                "util",
                "util.rhai",
                "fn greeting(name) { `Hello, ${name}!` }",
            )
        },
        |script| script.live_form().get("result_label", "text"),
    )
    .expect("the form builds");

    assert_eq!(label, Some(Value::Text("Hello, Grace!".to_owned())));
}

#[test]
fn a_form_load_error_is_located_and_nothing_runs() {
    let doc = greeting_doc();
    let error = run(
        &doc,
        "fn form_load() {\n    result_label.nope = 1;\n}",
        |_| Ok(()),
        |_script| unreachable!("the form must not build"),
    )
    .expect_err("form_load fails");

    let FormError::Script(error) = error else {
        panic!("a located script error is expected, got {error:?}");
    };
    assert_eq!(error.file, "frmMain.rhai");
    assert_eq!(error.line, 2);
    assert!(error.column > 0);
}

#[test]
fn a_parse_error_is_located_before_the_form_is_built() {
    let doc = greeting_doc();
    let error = run(
        &doc,
        "fn form_load() {\n    let x = ;\n}",
        |_| Ok(()),
        |_script| unreachable!("the form must not build"),
    )
    .expect_err("the script does not parse");

    let FormError::Script(error) = error else {
        panic!("a located script error is expected, got {error:?}");
    };
    assert_eq!((error.file.as_str(), error.line), ("frmMain.rhai", 2));
    assert!(error.column > 0);
}
