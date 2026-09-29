#![forbid(unsafe_code)]

//! Headless end-to-end test for hosting the designer, toolbox and property grid
//! in the IDE (issue #47).
//!
//! The app is built and driven entirely through its public messages: the
//! startup form opens as a designer tab, a Button is added by activating a
//! toolbox tile, the new control is renamed through the property grid (its
//! synthetic `(Name)` row is always the first row), and the project is saved.
//! The test then reloads the project from disk with [`ProjectSession`], so the
//! `.lfm` round-trip is what is asserted.

use std::path::PathBuf;
use std::rc::Rc;

use lazyrad_designer::{PropertyGridMsg, RowMove, Tool, ToolboxMsg};
use lazyrad_ide::{Command, IdeApp, Msg, ProjectSession, Settings};
use xui_canvas::OffscreenBackend;
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;

/// A fresh scratch directory for a test.
fn scratch(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "lazyrad-ide-designer-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    path
}

#[test]
fn a_toolbox_control_and_a_property_edit_round_trip_through_save_and_reload() {
    let dir = scratch("round-trip");
    ProjectSession::create("MyApp", &dir).expect("the project is created");
    let root = dir.clone();
    let cleanup = dir.clone();

    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    xui_core::run_app(
        backend,
        PlatformSpec::new("ide").size(Dip(1000.0), Dip(700.0)),
        move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            app.open_project(&root, ui);

            // Add a Button through the toolbox (a double-click on its tile).
            ui.emit(Msg::Toolbox(ToolboxMsg::Activate(Tool::control("Button"))));

            // Edit the new control's `(Name)` property through the grid: the
            // synthetic name row is always first, so one Down selects it.
            let form = "main_form".to_owned();
            ui.emit(Msg::PropertyGrid {
                form: form.clone(),
                msg: PropertyGridMsg::MoveRow(RowMove::Down),
            });
            ui.emit(Msg::PropertyGrid {
                form: form.clone(),
                msg: PropertyGridMsg::Activate,
            });
            ui.emit(Msg::PropertyGrid {
                form,
                msg: PropertyGridMsg::CommitText("go_button".into()),
            });

            ui.emit(Msg::Command(Command::Save));
            app
        },
    )
    .expect("the offscreen backend runs to completion");

    let reopened = ProjectSession::open(&dir).expect("the project reopens");
    let form = reopened.form("main_form").expect("the form persisted");
    let node = form
        .node("go_button")
        .expect("the toolbox control and its rename persisted");
    assert_eq!(node.kind, "Button");
    assert!(
        form.node("button1").is_none(),
        "the old auto-name was not written as well"
    );

    let _ = std::fs::remove_dir_all(&cleanup);
}

#[test]
fn a_late_designer_message_for_a_closed_form_does_not_reach_another_form() {
    let dir = scratch("stale-message");
    ProjectSession::create("MyApp", &dir).expect("the project is created");
    let root = dir.clone();
    let cleanup = dir.clone();

    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    xui_core::run_app(
        backend,
        PlatformSpec::new("ide").size(Dip(1000.0), Dip(700.0)),
        move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            app.open_project(&root, ui);

            // A message tagged with a form that was never open is dropped.
            ui.emit(Msg::Designer {
                document: "no_such_form".to_owned(),
                msg: lazyrad_designer::DesignerMsg::PointerDown {
                    x: 4,
                    y: 4,
                    ctrl: false,
                },
            });
            ui.emit(Msg::PropertyGrid {
                form: "no_such_form".to_owned(),
                msg: PropertyGridMsg::ToggleObjects,
            });
            app
        },
    )
    .expect("the offscreen backend runs to completion");

    let reopened = ProjectSession::open(&dir).expect("the project reopens");
    assert!(
        reopened.form("main_form").is_some(),
        "the real form is untouched by the stale messages"
    );

    let _ = std::fs::remove_dir_all(&cleanup);
}
