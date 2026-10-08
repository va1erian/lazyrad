//! Hot reload on a real form window (issue #91).
//!
//! The tests drive the reload entry points directly ([`FormRuntime::check_for_changes`]
//! and [`FormApp::reload_root`]) and the window's own `Msg::WatchTick`, so they
//! never sleep. The offscreen backend fires no timers, so a test that wants the
//! watch path calls [`App::update`] with `Msg::WatchTick` itself.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use lazyrad_runtime::{FormRuntime, Msg, ReloadOutcome};
use xui_canvas::OffscreenBackend;
use xui_core::app::{App, Ui, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;
use xui_form::{LiveForm, Value};

/// The offscreen window spec the tests use.
fn spec() -> PlatformSpec {
    PlatformSpec::new("lazyrad-runtime reload tests").size(Dip(320.0), Dip(200.0))
}

/// A scratch project directory for one test, emptied first.
fn scratch(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "lazyrad-runtime-reload-it-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("scratch directory is created");
    path
}

/// Writes a one-form project named `check` whose form has a `result_label`.
fn write_project(dir: &Path, code: &str) {
    fs::write(
        dir.join("check.lrp"),
        "name = \"check\"\nversion = \"0.1.0\"\nstartup = \"main_form\"\n\n\
         [[items]]\nkind = \"form\"\nname = \"main_form\"\n\
         layout = \"main_form.lfm\"\ncode = \"main_form.rhai\"\n",
    )
    .expect("project writes");
    fs::write(
        dir.join("main_form.lfm"),
        "format = 1\n\n[window]\nname = \"main_form\"\ntitle = \"Check\"\n\n\
         [[node]]\nkind = \"Label\"\nname = \"result_label\"\n\
         left = 10\ntop = 10\nwidth = 200\nheight = 20\ntext = \"before\"\n",
    )
    .expect("form writes");
    fs::write(dir.join("main_form.rhai"), code).expect("code writes");
}

/// A slot the test's closure fills with the form it wants to inspect.
type Captured = Rc<RefCell<Option<Rc<LiveForm<Msg>>>>>;

/// Builds `runtime`'s startup form offscreen, runs `body`, and returns the
/// captured form.
fn run_form(
    runtime: Rc<FormRuntime>,
    body: impl FnOnce(&mut lazyrad_runtime::FormApp, &mut Ui<Msg>),
) -> Rc<LiveForm<Msg>> {
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let captured: Captured = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&captured);
    run_app(backend, spec(), move |ui| {
        let mut app = runtime.build_app(ui, "main_form").expect("the form builds");
        body(&mut app, ui);
        *slot.borrow_mut() = Some(app.root_form().expect("the form is live").clone());
        app
    })
    .expect("the event loop runs");
    captured.borrow_mut().take().expect("the form was captured")
}

#[test]
fn editing_a_script_changes_handler_behaviour_on_reload() {
    let dir = scratch("handler");
    write_project(&dir, "fn form_load() { result_label.text = \"one\"; }");
    let runtime = FormRuntime::load(&dir).expect("the project loads");
    runtime.enable_watch(&dir);

    let runtime_for_run = Rc::clone(&runtime);
    let dir_for_run = dir.clone();
    let form = run_form(runtime, move |app, ui| {
        fs::write(
            dir_for_run.join("main_form.rhai"),
            "fn form_load() { result_label.text = \"two\"; }",
        )
        .expect("the script is rewritten");
        assert_eq!(runtime_for_run.check_for_changes(), ReloadOutcome::Reloaded);
        assert!(app.reload_root(ui), "the new form builds");
    });

    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("two".to_owned())),
        "the reloaded form runs the new form_load"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_broken_script_keeps_the_old_form_and_sets_the_banner() {
    let dir = scratch("broken-banner");
    write_project(&dir, "fn form_load() { result_label.text = \"one\"; }");
    let runtime = FormRuntime::load(&dir).expect("the project loads");
    runtime.enable_watch(&dir);

    let dir_for_run = dir.clone();
    let form = run_form(runtime, move |app, ui| {
        fs::write(
            dir_for_run.join("main_form.rhai"),
            "fn broken() { let x = ; }",
        )
        .expect("the script is broken");
        // The window's own watch tick checks and, failing, shows the banner.
        app.update(Msg::WatchTick, ui);
        assert!(
            app.banner_text().is_some(),
            "a failed reload sets the banner"
        );
    });

    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("one".to_owned())),
        "a broken reload keeps the running form"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn fixing_a_broken_script_clears_the_banner_and_reloads() {
    let dir = scratch("fixed-banner");
    write_project(&dir, "fn form_load() { result_label.text = \"one\"; }");
    let runtime = FormRuntime::load(&dir).expect("the project loads");
    runtime.enable_watch(&dir);

    let dir_for_run = dir.clone();
    let form = run_form(runtime, move |app, ui| {
        fs::write(
            dir_for_run.join("main_form.rhai"),
            "fn broken() { let x = ; }",
        )
        .expect("the script is broken");
        app.update(Msg::WatchTick, ui);
        assert!(app.banner_text().is_some());

        fs::write(
            dir_for_run.join("main_form.rhai"),
            "fn form_load() { result_label.text = \"fixed\"; }",
        )
        .expect("the script is fixed");
        app.update(Msg::WatchTick, ui);
        assert!(
            app.banner_text().is_none(),
            "a successful reload clears the banner"
        );
        // The tick broadcast `Msg::Reload`; run it to rebuild this window.
        app.update(Msg::Reload, ui);
    });

    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("fixed".to_owned()))
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn form_reload_receives_the_old_state() {
    let dir = scratch("state");
    write_project(&dir, "fn form_load() { form.state.count = 5; }");
    let runtime = FormRuntime::load(&dir).expect("the project loads");
    runtime.enable_watch(&dir);

    let runtime_for_run = Rc::clone(&runtime);
    let dir_for_run = dir.clone();
    let form = run_form(runtime, move |app, ui| {
        fs::write(
            dir_for_run.join("main_form.rhai"),
            "fn form_reload(old_state) { result_label.text = `${old_state.count}`; }\n\
             fn form_load() { }",
        )
        .expect("the script is rewritten");
        assert_eq!(runtime_for_run.check_for_changes(), ReloadOutcome::Reloaded);
        assert!(app.reload_root(ui));
    });

    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("5".to_owned())),
        "form_reload sees the state the old form left behind"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_watch_tick_on_an_unchanged_project_does_nothing() {
    let dir = scratch("no-change");
    write_project(&dir, "fn form_load() { result_label.text = \"one\"; }");
    let runtime = FormRuntime::load(&dir).expect("the project loads");
    runtime.enable_watch(&dir);

    let runtime_for_run = Rc::clone(&runtime);
    let form = run_form(runtime, move |app, ui| {
        app.update(Msg::WatchTick, ui);
        assert!(app.banner_text().is_none());
        assert_eq!(
            runtime_for_run.check_for_changes(),
            ReloadOutcome::Unchanged
        );
    });

    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("one".to_owned()))
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_watch_tick_swaps_the_runtime_sources_through_the_message_path() {
    let dir = scratch("message-path");
    write_project(&dir, "fn form_load() { result_label.text = \"one\"; }");
    let runtime = FormRuntime::load(&dir).expect("the project loads");
    runtime.enable_watch(&dir);

    let dir_for_run = dir.clone();
    let runtime_after = Rc::clone(&runtime);
    run_form(runtime, move |_app, ui| {
        fs::write(
            dir_for_run.join("main_form.rhai"),
            "fn form_load() { result_label.text = \"two\"; }",
        )
        .expect("the script is rewritten");
        // Queued here, drained after the closure: the tick must reload.
        ui.emit(Msg::WatchTick);
    });

    assert!(
        runtime_after
            .form("main_form")
            .expect("the form is there")
            .code
            .contains("two"),
        "the watch tick swapped in the new source"
    );
    let _ = fs::remove_dir_all(&dir);
}
