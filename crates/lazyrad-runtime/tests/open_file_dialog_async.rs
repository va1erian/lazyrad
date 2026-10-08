//! `open_file_dialog` does not block the window: the platform answers later.
//!
//! The fake platform keeps the `done` callback a dialog was given until the
//! test releases it, like a native dialog still on screen. `platform::install`
//! is a process-wide one-shot, so one test drives every case in turn.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;

use lazyrad_project::{FormDoc, Node};
use lazyrad_runtime::form::{FormApp, FormRuntime, FormSource, Msg};
use lazyrad_runtime::fs_policy::{FsPolicy, Sandbox};
use lazyrad_runtime::platform::{self, Dialogs, FileDone, FileFilter, Filter, Platform};
use xui_canvas::OffscreenBackend;
use xui_core::app::{Ui, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;
use xui_form::{LiveForm, Value};

/// The `done` callbacks of the dialogs that are still "on screen".
static HELD: Mutex<Vec<FileDone>> = Mutex::new(Vec::new());
/// The sandbox root the fake platform hands to a runtime.
static ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

struct FakeDialogs;

impl Dialogs for FakeDialogs {
    fn open_file(&self, _title: &str, _filter: Option<Filter<'_>>) -> Option<PathBuf> {
        None
    }

    fn open_file_async(&self, _title: &str, _filters: &[FileFilter], done: FileDone) {
        HELD.lock().expect("not poisoned").push(done);
    }

    fn choose_folder(&self, _title: &str) -> Option<PathBuf> {
        None
    }

    fn save_file(
        &self,
        _title: &str,
        _file_name: &str,
        _filter: Option<Filter<'_>>,
    ) -> Option<PathBuf> {
        None
    }

    fn show_error(&self, _title: &str, _message: &str) {}
}

struct FakePlatform;

impl Platform for FakePlatform {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn dialogs(&self) -> &dyn Dialogs {
        &FakeDialogs
    }

    fn fs_policy(&self) -> FsPolicy {
        let root = ROOT
            .lock()
            .expect("not poisoned")
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        FsPolicy::Sandboxed(Sandbox::new(root))
    }
}

fn scratch(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "lazyrad-async-dialog-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("scratch directory is created");
    path
}

fn label(name: &str, top: i64) -> Node {
    let mut label = Node::new("Label", name);
    label.set_prop("left", Value::Int(10));
    label.set_prop("top", Value::Int(top));
    label.set_prop("width", Value::Int(200));
    label
}

const SCRIPT: &str = r#"
fn form_load() {
    open_file_dialog("Open a song", "MOD files|*.mod;All files|*.*", Fn("picked"));
}
fn go_click() {
    status_label.text = "alive:" + result_label.text;
}
fn picked(path) {
    if type_of(path) == "()" {
        result_label.text = "cancelled";
    } else {
        result_label.text = "got:" + status_label.text + ":" + file_read_text(path);
    }
}
"#;

fn runtime() -> Rc<FormRuntime> {
    let mut doc = FormDoc::new("main_form");
    doc.insert(label("result_label", 10));
    doc.insert(label("status_label", 40));
    let mut go = Node::new("Button", "go");
    go.set_prop("left", Value::Int(10));
    go.set_prop("top", Value::Int(70));
    doc.insert(go);
    FormRuntime::from_sources(vec![FormSource::new("main_form", doc, SCRIPT)], Vec::new())
}

/// What the window looked like at the end of a run.
struct Outcome {
    form: Rc<LiveForm<Msg>>,
}

/// Runs the form, optionally handling a click after the dialog opened.
///
/// `release` runs right after the click handler, standing for the user
/// answering a dialog that stayed up while the click was handled.
fn run(click: bool, release: impl Fn() + 'static) -> Outcome {
    let runtime = runtime();
    runtime.set_handler_observer(Rc::new(move |_form, control, event| {
        if control == "go" && event == "Click" {
            release();
        }
    }));
    let backend = Rc::new(OffscreenBackend::new());
    let capture: Rc<RefCell<Option<Outcome>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&capture);
    run_app(
        backend as Rc<dyn Backend>,
        PlatformSpec::new("async dialog").size(Dip(320.0), Dip(200.0)),
        move |ui: &mut Ui<Msg>| {
            let app: FormApp = runtime
                .build_app(ui, "main_form")
                .expect("main_form builds");
            // `form_load` queued the dialog request; the click is queued after it.
            if click {
                ui.emit(Msg::Event {
                    form: "main_form".to_owned(),
                    control: "go".to_owned(),
                    event: "Click".to_owned(),
                    args: Vec::new(),
                });
            }
            // The timer mapper's tick for a window waiting on a dialog.
            ui.emit(Msg::Poll);
            *slot.borrow_mut() = Some(Outcome {
                form: app.root_form().expect("the form is live").clone(),
            });
            app
        },
    )
    .expect("the event loop runs");
    capture.borrow_mut().take().expect("the form was captured")
}

/// Answers the oldest dialog still held.
fn answer(path: Option<PathBuf>) {
    let done = HELD.lock().expect("not poisoned").remove(0);
    done(path);
}

#[test]
fn the_window_keeps_running_while_the_dialog_is_up() {
    let root = scratch("root");
    let outside = scratch("outside");
    let picked = outside.join("song.mod");
    fs::write(&picked, b"MODDATA").expect("picked file writes");
    *ROOT.lock().expect("not poisoned") = Some(root.clone());
    // Ignore a refusal: this test binary installs the fake exactly once.
    let _ = platform::install(Box::new(FakePlatform));

    // The dialog stays up while the click is handled, then the user picks. The
    // callback reads the label the click set, so it ran after the window
    // handled an event, and the picked file is readable through the grant.
    let for_release = picked.clone();
    let outcome = run(true, move || answer(Some(for_release.clone())));
    assert_eq!(
        outcome.form.get("status_label", "text"),
        Some(Value::Text("alive:".to_owned())),
        "the click was handled before the dialog was answered"
    );
    assert_eq!(
        outcome.form.get("result_label", "text"),
        Some(Value::Text("got:alive::MODDATA".to_owned())),
        "the callback ran afterwards, with the picked file readable"
    );
    assert!(HELD.lock().expect("not poisoned").is_empty());

    // A cancelled dialog passes `()`.
    let outcome = run(true, || answer(None));
    assert_eq!(
        outcome.form.get("result_label", "text"),
        Some(Value::Text("cancelled".to_owned()))
    );

    // The window closes (the loop ends and drops it) before the answer comes:
    // the answer is dropped, quietly.
    let outcome = run(false, || {});
    assert_eq!(
        outcome.form.get("result_label", "text"),
        Some(Value::Text(String::new())),
        "nothing answered yet, so the callback has not run"
    );
    assert_eq!(
        HELD.lock().expect("not poisoned").len(),
        1,
        "still on screen"
    );
    answer(Some(picked.clone()));
    assert_eq!(
        outcome.form.get("result_label", "text"),
        Some(Value::Text(String::new())),
        "a late answer reaches no callback"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside);
}
