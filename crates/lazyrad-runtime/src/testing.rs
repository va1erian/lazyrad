#![forbid(unsafe_code)]

//! A host-side test kit for LazyRAD apps (behind the `testing` feature).
//!
//! A host test that runs a real project on xui's offscreen backend needs the
//! same plumbing every time: build the startup form, deliver `Msg::Event` and
//! `Msg::Poll`, read widgets back through [`LiveForm::get`], and collect the
//! errors a handler raised. [`TestApp`] packages that into a fluent API:
//!
//! ```no_run
//! use lazyrad_runtime::testing::{TestApp, run_on_large_stack};
//! run_on_large_stack(|| {
//!     TestApp::run("samples/todo", |app| {
//!         app.edit("todo_edit", "milk").click("add_button");
//!         assert_eq!(app.items("todo_list"), ["milk"]);
//!         assert!(app.errors().is_empty());
//!     })
//!     .expect("the project loads");
//! });
//! ```
//!
//! # How a test runs
//!
//! xui's event loop is blocking and owns the built form, so the app lives for
//! exactly one call of [`TestApp::run`] (or [`TestApp::run_sources`]): the
//! startup form is built offscreen, its `form_load` runs, and then the test
//! body drives the live app. Every call acts at once on that one app, so a
//! handler with side effects (a file it writes, a random number it draws) runs
//! exactly once per event, as it would for a user.
//!
//! # Failing loudly
//!
//! A handler error is a test failure by default: the call that raised it
//! panics with the located error, so a broken app never passes silently (the
//! failure the MOD player sample hid until a QEMU screenshot). Call
//! [`TestApp::allow_errors`] first when the test is *about* the error, then read
//! it back with [`TestApp::errors`]. An error raised after the body's last call
//! (a message box callback, say) fails the run when the body returns.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use xui_canvas::OffscreenBackend;
use xui_core::app::{Ui, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;
use xui_form::{LiveForm, Value};

use crate::ScriptError;
use crate::form::{FormApp, FormRuntime, FormSource, ModuleSource, Msg, RuntimeError};

/// The stack a test app runs on, in bytes.
///
/// Rhai's debug build recurses deeply for a handler only a few script calls
/// down, so the default 8 MiB main-thread stack can overflow; a release build
/// would not. Tests that build a [`TestApp`] should run inside
/// [`run_on_large_stack`].
pub const TEST_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Runs `body` on a thread with a large stack, propagating a panic.
///
/// Debug-build Rhai needs more than the default stack for a handler a few
/// script calls deep, so a test that loads a project should wrap its body:
///
/// ```no_run
/// # use lazyrad_runtime::testing::run_on_large_stack;
/// run_on_large_stack(|| {
///     // build and drive a TestApp here
/// });
/// ```
pub fn run_on_large_stack<F, R>(body: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let handle = std::thread::Builder::new()
        .name("lazyrad-test-app".to_owned())
        .stack_size(TEST_STACK_SIZE)
        .spawn(body)
        .expect("the test thread starts");
    match handle.join() {
        Ok(value) => value,
        // Re-raise the body's panic on the caller's thread, so the test fails
        // with its own message rather than "the thread panicked".
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// A live LazyRAD app driven by a host test, inside [`TestApp::run`].
///
/// See the [module documentation](self) for how a test runs and how errors
/// fail it by default.
pub struct TestApp {
    ui: Ui<Msg>,
    backend: Rc<OffscreenBackend>,
    form: Rc<LiveForm<Msg>>,
    startup: String,
    allow_errors: bool,
    /// Every handler error so far, in order.
    errors: Rc<RefCell<Vec<ScriptError>>>,
    /// How many of `errors` a call has already checked.
    checked: usize,
    /// The text of every message box opened so far.
    message_boxes: Rc<RefCell<Vec<String>>>,
}

impl TestApp {
    /// Loads the project named by `dir` (a project directory or an `.lrp`),
    /// builds its startup form offscreen (running `form_load`) and runs `test`
    /// on the live app.
    ///
    /// A project that cannot be loaded, or whose startup form fails to build,
    /// is an [`Err`] and `test` does not run.
    pub fn run(
        dir: impl AsRef<Path>,
        test: impl FnOnce(&mut TestApp) + 'static,
    ) -> Result<(), RuntimeError> {
        let runtime = FormRuntime::load(dir.as_ref())?;
        let startup = runtime.startup_name()?;
        run_session(&runtime, &startup, test)
    }

    /// Like [`TestApp::run`], for in-memory sources that never touch disk.
    ///
    /// The startup form is the first form in name order (an in-memory runtime
    /// has no `.lrp` to name one).
    pub fn run_sources(
        forms: Vec<FormSource>,
        modules: Vec<ModuleSource>,
        test: impl FnOnce(&mut TestApp) + 'static,
    ) -> Result<(), RuntimeError> {
        let runtime = FormRuntime::from_sources(forms, modules);
        let startup = match runtime.startup_name() {
            Ok(name) => name,
            Err(_) => runtime
                .form_names()
                .into_iter()
                .next()
                .ok_or_else(|| RuntimeError::UnknownForm(String::new()))?,
        };
        run_session(&runtime, &startup, test)
    }

    /// Lets handler errors pass instead of panicking, so a test can inspect
    /// them with [`TestApp::errors`].
    pub fn allow_errors(&mut self) -> &mut TestApp {
        self.allow_errors = true;
        self
    }

    /// Sets `control`'s `text` (the common case: typing into an edit).
    pub fn edit(&mut self, control: &str, text: &str) -> &mut TestApp {
        self.set(control, "text", Value::Text(text.to_owned()))
    }

    /// Sets any writable property on `control`. A property the control does
    /// not have, or a value of the wrong type, fails the test.
    pub fn set(&mut self, control: &str, property: &str, value: Value) -> &mut TestApp {
        if let Err(error) = self.form.set(control, property, &value) {
            panic!("lazyrad test app: cannot set `{control}.{property}`: {error}");
        }
        self.settle()
    }

    /// Clicks `control` (its `Click` event).
    pub fn click(&mut self, control: &str) -> &mut TestApp {
        self.event(control, "Click", Vec::new())
    }

    /// Delivers `control`'s `event` with `args`, and everything it queues.
    pub fn event(&mut self, control: &str, event: &str, args: Vec<Value>) -> &mut TestApp {
        self.ui.emit(Msg::Event {
            form: self.startup.clone(),
            control: control.to_owned(),
            event: event.to_owned(),
            args,
        });
        self.settle()
    }

    /// Delivers `count` poll messages to the host's event sources.
    pub fn poll(&mut self, count: usize) -> &mut TestApp {
        for _ in 0..count {
            self.ui.emit(Msg::Poll);
            self.backend.pump(self.ui.window());
        }
        self.settle()
    }

    /// Handles every queued message, then fails the test on a handler error
    /// it has not yet checked, unless errors are allowed.
    fn settle(&mut self) -> &mut TestApp {
        self.backend.pump(self.ui.window());
        self.check_errors();
        self
    }

    /// Panics on the errors raised since the last check, unless allowed.
    fn check_errors(&mut self) {
        let errors = self.errors.borrow();
        let fresh = &errors[self.checked.min(errors.len())..];
        if !self.allow_errors && !fresh.is_empty() {
            let rendered: Vec<String> = fresh.iter().map(ScriptError::to_string).collect();
            let message = format!(
                "lazyrad test app: {} handler error(s):\n{}",
                rendered.len(),
                rendered.join("\n")
            );
            drop(errors);
            self.checked = self.errors.borrow().len();
            panic!("{message}");
        }
        self.checked = errors.len();
    }

    /// The `text` of `control`.
    pub fn text(&self, control: &str) -> String {
        match self.form.get(control, "text") {
            Some(Value::Text(text)) => text,
            _ => String::new(),
        }
    }

    /// The `items` of a list-like `control` (a `ListView`, `ComboBox` or
    /// `RadioGroup`).
    pub fn items(&self, control: &str) -> Vec<String> {
        match self.form.get(control, "items") {
            Some(Value::List(items)) => items,
            _ => Vec::new(),
        }
    }

    /// Any property of `control`, or `None` when it or the property is unknown.
    pub fn get(&self, control: &str, property: &str) -> Option<Value> {
        self.form.get(control, property)
    }

    /// Every handler error so far, in order.
    pub fn errors(&self) -> Vec<ScriptError> {
        self.errors.borrow().clone()
    }

    /// The text of every message box opened so far, in order.
    pub fn message_boxes(&self) -> Vec<String> {
        self.message_boxes.borrow().clone()
    }

    /// The startup form's live widgets, for a property the readers do not
    /// cover.
    pub fn form(&self) -> &Rc<LiveForm<Msg>> {
        &self.form
    }
}

/// The window and startup form the build hands to the test body.
type Built = (Ui<Msg>, Rc<LiveForm<Msg>>);

/// Builds the startup form offscreen and runs `test` on the live app.
fn run_session(
    runtime: &Rc<FormRuntime>,
    startup: &str,
    test: impl FnOnce(&mut TestApp) + 'static,
) -> Result<(), RuntimeError> {
    let errors: Rc<RefCell<Vec<ScriptError>>> = Rc::new(RefCell::new(Vec::new()));
    let boxes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let sink = Rc::clone(&errors);
        runtime.set_error_observer(Rc::new(move |_form, _control, _event, error| {
            sink.borrow_mut().push(error.clone());
        }));
    }
    {
        let sink = Rc::clone(&boxes);
        runtime.set_msg_box_observer(Rc::new(move |_form, text, _title| {
            sink.borrow_mut().push(text.to_owned());
        }));
    }

    let backend = Rc::new(OffscreenBackend::new());
    // Filled by the build closure, read by the run hook that runs right after.
    let built: Rc<RefCell<Option<Built>>> = Rc::new(RefCell::new(None));
    let failure: Rc<RefCell<Option<RuntimeError>>> = Rc::new(RefCell::new(None));

    let built_for_hook = Rc::clone(&built);
    let backend_for_hook = Rc::clone(&backend);
    let startup_for_hook = startup.to_owned();
    let errors_for_hook = Rc::clone(&errors);
    let boxes_for_hook = Rc::clone(&boxes);
    backend.set_run_hook(move || {
        let Some((ui, form)) = built_for_hook.borrow_mut().take() else {
            return;
        };
        let mut app = TestApp {
            ui,
            backend: backend_for_hook,
            form,
            startup: startup_for_hook,
            allow_errors: false,
            errors: errors_for_hook,
            checked: 0,
            message_boxes: boxes_for_hook,
        };
        // Whatever `form_load` queued (a message box, say) is handled first.
        app.backend.pump(app.ui.window());
        test(&mut app);
        app.settle();
    });

    let runtime_for_app = Rc::clone(runtime);
    let startup_for_app = startup.to_owned();
    let failure_for_app = Rc::clone(&failure);
    let spec = PlatformSpec::new(startup).size(Dip(640.0), Dip(480.0));
    run_app(
        backend as Rc<dyn Backend>,
        spec,
        move |ui| match runtime_for_app.build_app(ui, &startup_for_app) {
            Ok(app) => {
                if let Some(form) = app.root_form().cloned() {
                    *built.borrow_mut() = Some((ui.clone(), form));
                }
                app
            }
            Err(error) => {
                *failure_for_app.borrow_mut() = Some(error);
                FormApp::failed(Rc::clone(&runtime_for_app))
            }
        },
    )?;

    if let Some(error) = failure.borrow_mut().take() {
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazyrad_project::{FormDoc, Node};
    use rhai::{Dynamic, EvalAltResult, FnPtr};

    use crate::extensions::{self, EventSource, ScriptCall};

    /// A host that lets a script `watch(handler)` and later delivers queued
    /// payloads to it when the window polls.
    #[derive(Default)]
    struct FakeSource {
        handlers: RefCell<Vec<(String, FnPtr)>>,
        queued: RefCell<Vec<String>>,
    }

    impl EventSource for FakeSource {
        fn interval_ms(&self) -> u32 {
            25
        }

        fn active(&self, form: &str) -> bool {
            self.handlers
                .borrow()
                .iter()
                .any(|(owner, _)| owner == form)
        }

        fn poll(&self, form: &str, call: &mut ScriptCall<'_>) -> Vec<Box<EvalAltResult>> {
            let payloads: Vec<String> = self.queued.borrow_mut().drain(..).collect();
            let handlers: Vec<FnPtr> = self
                .handlers
                .borrow()
                .iter()
                .filter(|(owner, _)| owner == form)
                .map(|(_, handler)| handler.clone())
                .collect();
            let mut errors = Vec::new();
            for payload in payloads {
                for handler in &handlers {
                    if let Err(error) = call(handler, vec![Dynamic::from(payload.clone())]) {
                        errors.push(error);
                    }
                }
            }
            errors
        }

        fn release(&self, form: &str) {
            self.handlers
                .borrow_mut()
                .retain(|(owner, _)| owner != form);
        }
    }

    /// Registers `source` and a `watch(handler)` script function for this test
    /// thread.
    fn install_source(source: &Rc<FakeSource>) {
        extensions::clear();
        let for_scripts = Rc::clone(source);
        extensions::add_scoped(move |engine, scope| {
            let source = Rc::clone(&for_scripts);
            let form = scope.form.to_owned();
            engine.register_fn("watch", move |handler: FnPtr| {
                source.handlers.borrow_mut().push((form.clone(), handler));
            });
        });
        extensions::add_event_source(Rc::clone(source) as Rc<dyn EventSource>);
    }

    /// A todo-like form: an edit, an add button and a list.
    fn todo_doc() -> FormDoc {
        let mut doc = FormDoc::new("runtime");
        let mut edit = Node::new("Edit", "todo_edit");
        edit.set_prop("left", Value::Int(10));
        edit.set_prop("top", Value::Int(10));
        edit.set_prop("width", Value::Int(200));
        doc.insert(edit);
        let mut button = Node::new("Button", "add_button");
        button.set_prop("left", Value::Int(10));
        button.set_prop("top", Value::Int(40));
        button.set_prop("width", Value::Int(80));
        button.set_prop("height", Value::Int(28));
        doc.insert(button);
        let mut list = Node::new("ListView", "todo_list");
        list.set_prop("left", Value::Int(10));
        list.set_prop("top", Value::Int(80));
        list.set_prop("width", Value::Int(200));
        list.set_prop("items", Value::List(Vec::new()));
        doc.insert(list);
        doc
    }

    const TODO_CODE: &str = r#"fn form_load() { form.state.items = []; }
fn add_button_click() {
    let text = todo_edit.text;
    if text != "" {
        form.state.items.push(text);
        todo_list.items = form.state.items;
        todo_edit.text = "";
    }
}"#;

    /// Runs `test` on a form whose code is `code`.
    fn with_code(code: &str, test: impl FnOnce(&mut TestApp) + 'static) {
        TestApp::run_sources(
            vec![FormSource::new("runtime", todo_doc(), code)],
            Vec::new(),
            test,
        )
        .expect("the app loads");
    }

    #[test]
    fn a_todo_like_app_adds_an_item() {
        with_code(TODO_CODE, |app| {
            app.edit("todo_edit", "milk").click("add_button");
            assert_eq!(app.items("todo_list"), ["milk"]);
            app.edit("todo_edit", "eggs").click("add_button");
            assert_eq!(app.items("todo_list"), ["milk", "eggs"]);
            assert!(app.errors().is_empty());
        });
    }

    #[test]
    fn each_event_runs_its_handler_exactly_once() {
        // A side effect outside the form (here a message box) must not repeat
        // when later events are delivered.
        with_code("fn add_button_click() { msg_box(\"click\"); }", |app| {
            app.click("add_button")
                .click("add_button")
                .click("add_button");
            assert_eq!(app.message_boxes(), ["click", "click", "click"]);
        });
    }

    #[test]
    fn a_failing_handler_panics_by_default() {
        let result = std::panic::catch_unwind(|| {
            with_code("fn add_button_click() { let d = 0; 1 / d }", |app| {
                app.click("add_button");
            });
        });
        assert!(result.is_err(), "a handler error fails the test");
    }

    #[test]
    fn allow_errors_records_the_failure() {
        with_code("fn add_button_click() { ghost = 1; }", |app| {
            app.allow_errors().click("add_button");
            let errors = app.errors();
            assert_eq!(errors.len(), 1);
            assert!(errors[0].message.contains("ghost"), "{}", errors[0]);
        });
    }

    #[test]
    fn a_message_box_is_recorded_not_blocking() {
        with_code("fn add_button_click() { msg_box(\"saved\"); }", |app| {
            app.click("add_button");
            assert_eq!(app.message_boxes(), ["saved"]);
        });
    }

    #[test]
    fn setting_an_unknown_property_fails_the_test() {
        let result = std::panic::catch_unwind(|| {
            with_code(TODO_CODE, |app| {
                app.set("todo_edit", "colour", Value::Int(1));
            });
        });
        assert!(result.is_err());
    }

    #[test]
    fn poll_delivers_a_host_sources_work() {
        let source = Rc::new(FakeSource::default());
        install_source(&source);
        let mut doc = FormDoc::new("runtime");
        let mut label = Node::new("Label", "result_label");
        label.set_prop("left", Value::Int(10));
        label.set_prop("top", Value::Int(10));
        label.set_prop("width", Value::Int(200));
        label.set_prop("text", Value::Text("before".to_owned()));
        doc.insert(label);

        let queue = Rc::clone(&source);
        TestApp::run_sources(
            vec![FormSource::new(
                "runtime",
                doc,
                "fn form_load() { watch(|x| result_label.text = \"got \" + x); }",
            )],
            Vec::new(),
            move |app| {
                queue.queued.borrow_mut().push("ping".to_owned());
                app.poll(1);
                assert_eq!(app.text("result_label"), "got ping");
                queue.queued.borrow_mut().push("pong".to_owned());
                app.poll(1);
                assert_eq!(app.text("result_label"), "got pong");
            },
        )
        .expect("the app loads");
        extensions::clear();
    }

    #[test]
    fn a_missing_startup_form_is_an_error() {
        let error = TestApp::run_sources(Vec::new(), Vec::new(), |_| {
            panic!("the test body must not run");
        })
        .expect_err("an empty runtime has no startup form");
        assert!(matches!(error, RuntimeError::UnknownForm(_)));
    }

    #[test]
    fn run_on_large_stack_returns_the_value() {
        assert_eq!(run_on_large_stack(|| 40 + 2), 42);
    }

    #[test]
    fn run_on_large_stack_propagates_a_panic() {
        let result = std::panic::catch_unwind(|| {
            run_on_large_stack(|| panic!("boom"));
        });
        assert!(result.is_err());
    }
}
