//! `Timer` control tests on a real form window.
//!
//! The offscreen backend neither fires timers nor lets a test inject a window
//! event, so a tick is delivered as the message the window's timer mapper would
//! produce (`Msg::Tick`), exactly as the event-source tests deliver `Msg::Poll`.

use std::cell::RefCell;
use std::rc::Rc;

use lazyrad_project::{FormDoc, Node};
use lazyrad_runtime::form::{FormRuntime, FormSource, Msg};
use xui_canvas::OffscreenBackend;
use xui_core::app::{Ui, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;
use xui_form::{LiveForm, Value};

/// The offscreen window spec the tests use.
fn spec() -> PlatformSpec {
    PlatformSpec::new("lazyrad-runtime timers").size(Dip(320.0), Dip(200.0))
}

/// A form with a label and two `Timer` controls.
fn form_doc() -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    let mut label = Node::new("Label", "result_label");
    label.set_prop("left", Value::Int(10));
    label.set_prop("top", Value::Int(10));
    label.set_prop("width", Value::Int(200));
    label.set_prop("text", Value::Text(String::new()));
    doc.insert(label);
    doc.insert(Node::new("Timer", "timer1"));
    doc.insert(Node::new("Timer", "timer2"));
    doc
}

/// Builds `code`'s form, delivers `ticks`, and returns the live form.
fn run(code: &str, ticks: &[&str]) -> Rc<LiveForm<Msg>> {
    let runtime = FormRuntime::from_sources(
        vec![FormSource::new("main_form", form_doc(), code)],
        Vec::new(),
    );
    let backend = Rc::new(OffscreenBackend::new());
    let capture: Rc<RefCell<Option<Rc<LiveForm<Msg>>>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&capture);
    let ticks: Vec<String> = ticks.iter().map(|name| (*name).to_owned()).collect();
    run_app(
        backend as Rc<dyn Backend>,
        spec(),
        move |ui: &mut Ui<Msg>| {
            let app = runtime
                .build_app(ui, "main_form")
                .expect("main_form builds");
            for control in &ticks {
                ui.emit(Msg::Tick {
                    control: control.clone(),
                });
            }
            *slot.borrow_mut() = Some(app.root_form().expect("the form is live").clone());
            app
        },
    )
    .expect("the event loop runs");
    capture.borrow_mut().take().expect("the form was captured")
}

#[test]
fn several_timers_tick_independently() {
    let form = run(
        "fn form_load() { timer1.enabled = true; timer2.enabled = true; }
         fn timer1_tick() { result_label.text += \"a\"; }
         fn timer2_tick() { result_label.text += \"b\"; }",
        &["timer1", "timer2", "timer1"],
    );
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("aba".to_owned()))
    );
}

#[test]
fn a_tick_for_an_unknown_control_is_harmless() {
    let form = run(
        "fn timer1_tick() { result_label.text += \"a\"; }",
        &["ghost"],
    );
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text(String::new()))
    );
}

#[test]
fn a_failing_tick_handler_disables_its_timer() {
    let form = run(
        "fn form_load() { timer1.enabled = true; }
         fn timer1_tick() { throw \"boom\"; }",
        &["timer1"],
    );
    assert_eq!(
        form.get("timer1", "enabled"),
        Some(Value::Bool(false)),
        "a failing tick handler disables the timer instead of repeating"
    );
}

#[test]
fn a_timer_runs_after_form_load_enables_it() {
    let runtime = FormRuntime::from_sources(
        vec![FormSource::new(
            "main_form",
            form_doc(),
            "fn form_load() { timer1.enabled = true; }",
        )],
        Vec::new(),
    );
    let backend = Rc::new(OffscreenBackend::new());
    let capture: Rc<RefCell<Option<(bool, bool)>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&capture);
    run_app(
        backend as Rc<dyn Backend>,
        spec(),
        move |ui: &mut Ui<Msg>| {
            let app = runtime
                .build_app(ui, "main_form")
                .expect("main_form builds");
            *slot.borrow_mut() = Some((
                app.is_timer_running("timer1"),
                app.is_timer_running("timer2"),
            ));
            app
        },
    )
    .expect("the event loop runs");
    let (timer1, timer2) = capture.borrow_mut().take().expect("captured");
    assert!(timer1, "enabled in form_load starts the timer");
    assert!(!timer2, "a disabled timer is not running");
}
