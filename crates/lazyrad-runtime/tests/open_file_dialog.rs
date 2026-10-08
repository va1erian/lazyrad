//! `open_file_dialog` on a real form window, with a fake platform.
//!
//! `platform::install` is a process-wide one-shot, so this file installs one
//! fake platform and drives every case from a single test: a picked file is
//! granted read access to the sandbox and passed to the callback, and a
//! cancellation passes `()`.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;

use lazyrad_project::{FormDoc, Node};
use lazyrad_runtime::form::{FormRuntime, FormSource, Msg};
use lazyrad_runtime::fs_policy::{Access, FsPolicy, Sandbox};
use lazyrad_runtime::platform::{self, Dialogs, Filter, Platform};
use xui_canvas::OffscreenBackend;
use xui_core::app::{Ui, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;
use xui_form::{LiveForm, Value};

/// What the fake dialog returns: `Some` picks that path, `None` cancels.
static PICKED: Mutex<Option<PathBuf>> = Mutex::new(None);
/// The sandbox root the fake platform hands to a runtime.
static ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

struct FakeDialogs;

impl Dialogs for FakeDialogs {
    fn open_file(&self, _title: &str, _filter: Option<Filter<'_>>) -> Option<PathBuf> {
        PICKED.lock().expect("not poisoned").clone()
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
        "lazyrad-open-dialog-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("scratch directory is created");
    path
}

/// Builds the form and returns its live form after the loop.
fn build(code: &str) -> Rc<LiveForm<Msg>> {
    let mut doc = FormDoc::new("main_form");
    let mut label = Node::new("Label", "result_label");
    label.set_prop("left", Value::Int(10));
    label.set_prop("top", Value::Int(10));
    label.set_prop("width", Value::Int(200));
    doc.insert(label);
    let runtime =
        FormRuntime::from_sources(vec![FormSource::new("main_form", doc, code)], Vec::new());
    let backend = Rc::new(OffscreenBackend::new());
    let capture: Rc<RefCell<Option<Rc<LiveForm<Msg>>>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&capture);
    run_app(
        backend as Rc<dyn Backend>,
        PlatformSpec::new("open dialog").size(Dip(320.0), Dip(200.0)),
        move |ui: &mut Ui<Msg>| {
            let app = runtime
                .build_app(ui, "main_form")
                .expect("main_form builds");
            *slot.borrow_mut() = Some(app.root_form().expect("the form is live").clone());
            app
        },
    )
    .expect("the event loop runs");
    capture.borrow_mut().take().expect("the form was captured")
}

const SCRIPT: &str = r#"
fn form_load() {
    open_file_dialog("Open a song", "MOD files|*.mod;All files|*.*", Fn("picked"));
}
fn picked(path) {
    if type_of(path) == "()" {
        result_label.text = "cancelled";
    } else {
        result_label.text = file_read_text(path);
    }
}
"#;

#[test]
fn a_picked_file_is_granted_read_and_passed_to_the_callback() {
    // The sandbox root is empty; the picked file lives outside it.
    let root = scratch("root");
    let outside = scratch("outside");
    let picked = outside.join("song.mod");
    fs::write(&picked, b"MODDATA").expect("picked file writes");
    *ROOT.lock().expect("not poisoned") = Some(root.clone());
    *PICKED.lock().expect("not poisoned") = Some(picked.clone());
    // Ignore a refusal: this test binary installs the fake exactly once.
    let _ = platform::install(Box::new(FakePlatform));

    let form = build(SCRIPT);
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("MODDATA".to_owned())),
        "the picked file is readable through the sandbox grant"
    );

    // The grant is read-only: a write to the picked file is still denied.
    let policy = FakePlatform.fs_policy();
    let sandbox = FsPolicy::Sandboxed(Sandbox::new(root.clone()));
    policy.allow_runtime(picked.clone(), Access::Read);
    assert!(
        policy
            .resolve(&picked.to_string_lossy(), Access::Read)
            .is_ok()
    );
    assert!(
        sandbox
            .resolve(&picked.to_string_lossy(), Access::Read)
            .is_err()
    );

    // Cancelling passes `()`.
    *PICKED.lock().expect("not poisoned") = None;
    let form = build(SCRIPT);
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("cancelled".to_owned()))
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside);
}
