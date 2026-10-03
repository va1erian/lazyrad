//! Event sources (`lazyrad_runtime::extensions::EventSource`) on a real form
//! window: a script hands a handler to a host extension, the window polls
//! while the form has work, the handler runs in the form's script, and the
//! form's registrations are released when its window goes away.
//!
//! The LazyOS player delivers Messenger events this way; a fake source stands
//! in for it here. The offscreen backend neither fires timers nor lets a test
//! inject a window-level event, so the tests emit the message the window's
//! timer mapping produces (`Msg::Poll`).

use std::cell::RefCell;
use std::rc::Rc;

use lazyrad_project::{FormDoc, Node};
use lazyrad_runtime::extensions::{self, EventSource, ScriptCall};
use lazyrad_runtime::form::{FormRuntime, FormSource, Msg};
use rhai::{Dynamic, EvalAltResult, FnPtr};
use xui_canvas::OffscreenBackend;
use xui_core::app::{Ui, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;
use xui_form::{LiveForm, Value};

/// A host that lets scripts `watch(handler)` and later delivers queued
/// payloads to every handler of a form.
#[derive(Default)]
struct FakeSource {
    handlers: RefCell<Vec<(String, FnPtr)>>,
    queued: RefCell<Vec<String>>,
    released: RefCell<Vec<String>>,
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
        self.released.borrow_mut().push(form.to_owned());
    }
}

/// Registers the fake source and the `watch(handler)` script function.
fn install(source: &Rc<FakeSource>) {
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

fn spec() -> PlatformSpec {
    PlatformSpec::new("lazyrad-runtime events").size(Dip(320.0), Dip(200.0))
}

fn label_form(code: &str) -> Rc<FormRuntime> {
    let mut doc = FormDoc::new("main_form");
    let mut label = Node::new("Label", "result_label");
    label.set_prop("left", Value::Int(10));
    label.set_prop("top", Value::Int(10));
    label.set_prop("width", Value::Int(200));
    label.set_prop("text", Value::Text("before".to_owned()));
    doc.insert(label);
    FormRuntime::from_sources(vec![FormSource::new("main_form", doc, code)], Vec::new())
}

/// What one offscreen run observed.
struct Run {
    form: Rc<LiveForm<Msg>>,
    polling_after_load: bool,
}

/// Builds `main_form`, queues `payload` on the source, delivers one poll
/// and returns once the loop has drained.
fn run_once(runtime: Rc<FormRuntime>, source: &Rc<FakeSource>, payload: &str) -> Run {
    let backend = Rc::new(OffscreenBackend::new());
    let source = Rc::clone(source);
    let payload = payload.to_owned();
    let seen: Rc<RefCell<Option<Run>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&seen);
    run_app(
        backend as Rc<dyn Backend>,
        spec(),
        move |ui: &mut Ui<Msg>| {
            let app = runtime
                .build_app(ui, "main_form")
                .expect("main_form builds");
            source.queued.borrow_mut().push(payload.clone());
            ui.emit(Msg::Poll);
            *slot.borrow_mut() = Some(Run {
                form: app.root_form().expect("the form is live").clone(),
                polling_after_load: app.is_polling(),
            });
            app
        },
    )
    .expect("the event loop runs");
    seen.borrow_mut().take().expect("the app was built")
}

#[test]
fn a_handler_a_script_registered_runs_on_the_window_timer() {
    let source = Rc::new(FakeSource::default());
    install(&source);
    let runtime = label_form("fn form_load() { watch(|x| result_label.text = \"got \" + x); }");
    let run = run_once(runtime, &source, "ping");
    extensions::clear();

    assert!(
        run.polling_after_load,
        "form_load registered work, so the timer runs"
    );
    assert_eq!(
        run.form.get("result_label", "text"),
        Some(Value::Text("got ping".to_owned()))
    );
    assert_eq!(source.released.borrow().as_slice(), ["main_form"]);
    assert!(
        source.handlers.borrow().is_empty(),
        "released with the window"
    );
}

#[test]
fn a_named_function_works_as_a_handler_too() {
    let source = Rc::new(FakeSource::default());
    install(&source);
    let runtime = label_form(
        "fn on_ping(x) { result_label.text = `named ${x}`; }\n\
         fn form_load() { watch(Fn(\"on_ping\")); }",
    );
    let run = run_once(runtime, &source, "pong");
    extensions::clear();
    assert_eq!(
        run.form.get("result_label", "text"),
        Some(Value::Text("named pong".to_owned()))
    );
}

#[test]
fn a_form_without_work_runs_no_timer() {
    let source = Rc::new(FakeSource::default());
    install(&source);
    let runtime = label_form("fn form_load() { result_label.text = \"idle\"; }");
    let run = run_once(runtime, &source, "ignored");
    extensions::clear();
    assert!(!run.polling_after_load);
    assert_eq!(
        run.form.get("result_label", "text"),
        Some(Value::Text("idle".to_owned()))
    );
}

#[test]
fn a_failing_handler_is_reported_and_the_program_keeps_running() {
    let source = Rc::new(FakeSource::default());
    install(&source);
    let runtime = label_form(
        "fn form_load() { watch(|x| { result_label.text = \"tried\"; throw \"bad \" + x; }); }",
    );
    let run = run_once(runtime, &source, "event");
    extensions::clear();
    assert_eq!(
        run.form.get("result_label", "text"),
        Some(Value::Text("tried".to_owned()))
    );
    assert_eq!(source.released.borrow().as_slice(), ["main_form"]);
}
