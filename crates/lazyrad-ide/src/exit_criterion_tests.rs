//! The Iteration 1 exit criterion as one test (issue #18).
//!
//! "In the IDE, create a project, draw a form with a button and a text box,
//! double-click the button, write a handler, press F5, and see it work."
//!
//! The IDE half runs headlessly through the app's own messages and editor. F5
//! goes through the real Start path (save, compile check, launch) with a
//! launcher that records what it was asked to run instead of opening the
//! player's window, which CI has no display for. "See it work" then loads the
//! saved project into the runtime on the offscreen backend, clicks the button
//! and reads the text box, which is what the player would show.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use lazyrad_designer::{DesignerMsg, Target, Tool, ToolboxMsg};
use lazyrad_runtime::form::{FormRuntime, Msg as RuntimeMsg};
use xui_canvas::OffscreenBackend;
use xui_core::app::{App, Ui, run_app};
use xui_core::backend::{Backend, Event, PlatformSpec};
use xui_core::message::{Modifiers, MouseButton};
use xui_core::units::Dip;
use xui_form::{LiveForm, Value};

use super::{DocKind, IdeApp, Msg, default_platform_spec};
use crate::project::{DEFAULT_FORM, ProjectSession};
use crate::run::{ChildProcess, EventSink, LaunchError, Launcher, RunId};
use crate::settings::Settings;

/// Records the one launch Start asks for, instead of opening a window.
#[derive(Default)]
struct RecordingLauncher {
    launched: RefCell<Vec<PathBuf>>,
}

impl Launcher for RecordingLauncher {
    fn launch(
        &self,
        _player: &Path,
        project_dir: &Path,
        _run: RunId,
        _sink: EventSink,
    ) -> Result<Box<dyn ChildProcess>, LaunchError> {
        self.launched.borrow_mut().push(project_dir.to_path_buf());
        Ok(Box::new(IdleChild))
    }
}

/// A launched program that never reports anything.
struct IdleChild;

impl ChildProcess for IdleChild {
    fn kill(&mut self) {}

    fn is_running(&mut self) -> bool {
        true
    }
}

/// What the handler writes into the text box.
const GREETING: &str = "Hello from LazyRAD!";

#[test]
fn draw_a_form_write_a_handler_press_f5_and_see_it_work() {
    let dir = std::env::temp_dir().join(format!("lazyrad-exit-criterion-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let project_dir = dir.clone();
    let launcher = Rc::new(RecordingLauncher::default());
    let launcher_for_app = Rc::clone(&launcher);

    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    run_app(backend, default_platform_spec(), move |ui| {
        let mut app = IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");

        // 1. Create a project; its startup form opens in the designer.
        let session = ProjectSession::create("exit_criterion", &project_dir).expect("create");
        app.session = Some(session);
        app.dispatcher.set_project_open(true);
        app.refresh_explorer(ui);
        let form = DEFAULT_FORM.to_owned();
        app.open_document(&form, DocKind::Designer)
            .expect("the form opens in the designer");

        // 2. Draw a button and a text box: pick each tool in the toolbox, then
        //    drag its rectangle on the form (design units, form-relative).
        for (kind, (left, top, right, bottom)) in
            [("Button", (16, 16, 116, 40)), ("Edit", (16, 56, 216, 80))]
        {
            app.update(Msg::Toolbox(ToolboxMsg::Select(Tool::control(kind))), ui);
            for msg in [
                DesignerMsg::PointerDown {
                    x: left,
                    y: top,
                    ctrl: false,
                },
                DesignerMsg::PointerMove {
                    x: right,
                    y: bottom,
                    ctrl: false,
                },
                DesignerMsg::PointerUp {
                    x: right,
                    y: bottom,
                    ctrl: false,
                },
            ] {
                app.update(
                    Msg::Designer {
                        document: form.clone(),
                        msg,
                    },
                    ui,
                );
            }
        }
        let doc = app
            .session
            .as_ref()
            .and_then(|session| session.form(&form))
            .cloned()
            .expect("the form is in the session");
        assert!(doc.node("button1").is_some(), "a button was drawn");
        assert!(doc.node("edit1").is_some(), "a text box was drawn");

        // 3. Double-click the button: its click handler opens, caret inside.
        app.update(
            Msg::OpenDefaultHandler {
                form: form.clone(),
                target: Target::Node("button1".to_owned()),
            },
            ui,
        );
        let editor = app.code_editor(&form).expect("the code tab is open");
        assert!(editor.text().contains("fn button1_click() {"));

        // 4. Write the handler body at the caret, as typing would.
        editor.insert_text(&format!("\n    edit1.text = \"{GREETING}\";"));
        app.after_programmatic_edit(&form, ui);

        // 5. Press F5: Start saves, compile-checks and launches the project.
        app.settings.player_path = Some(player_placeholder(&project_dir));
        app.launcher = launcher_for_app;
        app.update(Msg::Command(crate::command::Command::RunStart), ui);
        assert!(app.is_running(), "the program started");
        assert!(
            app.errors.is_empty(),
            "the project compiled: {:?}",
            app.errors
        );
        app
    })
    .expect("the IDE runs to completion");

    assert_eq!(
        *launcher.launched.borrow(),
        std::slice::from_ref(&dir),
        "F5 launched the player on the project folder"
    );

    // 6. See it work: run the saved project, click the button, read the box.
    let edit_text = click_and_read(&dir, "button1", "edit1");
    assert_eq!(edit_text, Some(Value::Text(GREETING.to_owned())));

    let _ = std::fs::remove_dir_all(&dir);
}

/// A file standing in for the player binary, so Start finds one to launch.
fn player_placeholder(dir: &Path) -> PathBuf {
    let player = dir.join("lazyrad-player-placeholder");
    std::fs::write(&player, b"placeholder").expect("write the placeholder");
    player
}

/// Runs the project in `dir` offscreen, clicks `button` and returns the
/// `text` of `edit` afterwards.
fn click_and_read(dir: &Path, button: &str, edit: &str) -> Option<Value> {
    let runtime = FormRuntime::load(dir).expect("the saved project loads");
    let startup = runtime
        .startup_name()
        .expect("the project names a startup form");
    let backend = Rc::new(OffscreenBackend::new());
    let backend_for_click = Rc::clone(&backend);
    let captured: Rc<RefCell<Option<Rc<LiveForm<RuntimeMsg>>>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&captured);
    let button = button.to_owned();
    let spec = PlatformSpec::new("exit criterion").size(Dip(400.0), Dip(300.0));
    run_app(backend as Rc<dyn Backend>, spec, move |ui| {
        let app = runtime.build_app(ui, &startup).expect("the form builds");
        let form = app.root_form().expect("the form is live").clone();
        click(&backend_for_click, ui, &form, &button);
        *slot.borrow_mut() = Some(form);
        app
    })
    .expect("the program runs");
    let form = captured.borrow_mut().take().expect("the form was captured");
    form.get(edit, "text")
}

/// Injects a left click at the centre of the control `name`.
fn click<M: 'static>(backend: &OffscreenBackend, ui: &Ui<M>, form: &LiveForm<M>, name: &str) {
    let id = form
        .widget(name)
        .unwrap_or_else(|| panic!("`{name}` exists"))
        .id();
    let bounds = ui.bounds(id);
    let (x, y) = (
        bounds.left + bounds.width() / 2,
        bounds.top + bounds.height() / 2,
    );
    let modifiers = Modifiers::NONE;
    for event in [
        Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            modifiers,
        },
        Event::MouseUp {
            x,
            y,
            button: MouseButton::Left,
            modifiers,
        },
    ] {
        let _ = backend.inject(ui.window(), event);
    }
}
