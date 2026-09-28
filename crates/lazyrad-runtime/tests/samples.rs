//! End-to-end tests for the shipped sample projects.
//!
//! Each test loads a sample directory through [`FormRuntime::load`], builds its
//! startup form on the [`OffscreenBackend`], injects the clicks a user would
//! make and asserts on the widgets' resulting values. This is the same path
//! `lazyrad-player` takes, so a passing test means the sample runs headlessly.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use lazyrad_runtime::form::{FormRuntime, Msg};
use xui_canvas::OffscreenBackend;
use xui_core::app::{Ui, run_app};
use xui_core::backend::{Backend, Event, PlatformSpec};
use xui_core::message::{Modifiers, MouseButton};
use xui_core::units::Dip;
use xui_form::{LiveForm, Value};

/// The directory of the sample named `sample`.
fn sample_dir(sample: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(sample)
}

/// The offscreen window spec the tests use.
fn spec() -> PlatformSpec {
    PlatformSpec::new("lazyrad-runtime samples").size(Dip(320.0), Dip(320.0))
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

/// Loads `sample`, builds `main_form` offscreen, runs `simulate` against the
/// live form and returns it once the event loop has processed every click.
fn run_sample(
    sample: &str,
    simulate: impl FnOnce(&OffscreenBackend, &Ui<Msg>, &Rc<LiveForm<Msg>>),
) -> Rc<LiveForm<Msg>> {
    let runtime = FormRuntime::load(sample_dir(sample)).expect("the sample project loads");
    let backend = Rc::new(OffscreenBackend::new());
    let capture: Rc<RefCell<Option<Rc<LiveForm<Msg>>>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&capture);
    let backend_for_sim = Rc::clone(&backend);

    run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
        let app = runtime
            .build_app(ui, "main_form")
            .expect("main_form builds");
        let form = app.root_form().expect("the form is live").clone();
        simulate(&backend_for_sim, ui, &form);
        *slot.borrow_mut() = Some(form);
        app
    })
    .expect("the event loop runs");

    capture.borrow_mut().take().expect("the form was captured")
}

#[test]
fn every_sample_project_validates() {
    for sample in ["hello", "calculator", "todo"] {
        let dir = sample_dir(sample);
        let project = lazyrad_project::Project::load(&dir).expect("the project loads");
        let diagnostics = project.validate(&dir);
        assert!(diagnostics.is_empty(), "{sample}: {diagnostics:?}");
    }
}

#[test]
fn the_hello_sample_greets_the_name_in_the_edit() {
    let form = run_sample("hello", |backend, ui, form| {
        form.set("name_edit", "text", &Value::Text("World".to_owned()))
            .expect("the name field is writable");
        click(backend, ui, form, "hello_button");
    });

    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("Hello, World!".to_owned()))
    );
}

#[test]
fn the_calculator_sample_adds_two_numbers() {
    let form = run_sample("calculator", |backend, ui, form| {
        for control in [
            "digit_1",
            "digit_2",
            "add_button",
            "digit_3",
            "equals_button",
        ] {
            click(backend, ui, form, control);
        }
    });

    assert_eq!(
        form.get("display_edit", "text"),
        Some(Value::Text("15".to_owned()))
    );
}

#[test]
fn the_calculator_sample_divides_two_numbers() {
    let form = run_sample("calculator", |backend, ui, form| {
        for control in ["digit_8", "divide_button", "digit_2", "equals_button"] {
            click(backend, ui, form, control);
        }
    });

    assert_eq!(
        form.get("display_edit", "text"),
        Some(Value::Text("4".to_owned()))
    );
}

#[test]
fn the_todo_sample_adds_the_edit_text_as_a_row() {
    let form = run_sample("todo", |backend, ui, form| {
        form.set("todo_edit", "text", &Value::Text("Buy milk".to_owned()))
            .expect("the todo field is writable");
        click(backend, ui, form, "add_button");
    });

    assert_eq!(
        form.get("todo_list", "items"),
        Some(Value::List(vec!["Buy milk".to_owned()]))
    );
}

#[test]
fn the_todo_sample_removes_the_selected_row() {
    let form = run_sample("todo", |backend, ui, form| {
        form.set("todo_edit", "text", &Value::Text("Buy milk".to_owned()))
            .expect("the todo field is writable");
        click(backend, ui, form, "add_button");
        click(backend, ui, form, "remove_button");
    });

    assert_eq!(
        form.get("todo_list", "items"),
        Some(Value::List(Vec::new())),
        "removing the row the list selected leaves the list empty"
    );
}
