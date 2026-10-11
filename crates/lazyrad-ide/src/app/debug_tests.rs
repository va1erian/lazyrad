//! The IDE's debugger behaviour, driven headlessly: a fake launcher stands in
//! for the player process and the debug events are delivered as the pipe reader
//! would deliver them.

use std::cell::{Cell, RefCell};

use lazyrad_debug_proto::{Command as DebugCommand, Event, Frame, Stop, StopReason, Variable};
use xui_canvas::OffscreenBackend;
use xui_core::backend::Backend;
use xui_core::run_app;

use super::*;
use crate::run::{ChildProcess, EventSink, LaunchError, Launcher, SendError};

/// What the fake launcher saw and what it should do.
#[derive(Default)]
struct Wire {
    /// Every line sent to a child's stdin, across runs.
    sent: RefCell<Vec<String>>,
    /// The mode of every launch.
    modes: RefCell<Vec<LaunchMode>>,
    /// Sends fail like a dead child's pipe.
    broken: Cell<bool>,
    /// Debug launches are refused (a platform without debug support).
    no_debug: Cell<bool>,
    kills: Cell<usize>,
}

impl Wire {
    fn commands(&self) -> Vec<DebugCommand> {
        self.sent
            .borrow()
            .iter()
            .filter_map(|line| lazyrad_debug_proto::decode_command(line).ok())
            .collect()
    }

    fn last_command(&self) -> Option<DebugCommand> {
        self.commands().pop()
    }

    fn clear(&self) {
        self.sent.borrow_mut().clear();
    }
}

struct FakeLauncher(Rc<Wire>);

struct FakeChild(Rc<Wire>);

impl Launcher for FakeLauncher {
    fn launch(
        &self,
        player: &Path,
        project_dir: &Path,
        run: RunId,
        sink: EventSink,
    ) -> Result<Box<dyn ChildProcess>, LaunchError> {
        self.launch_with(player, project_dir, run, sink, LaunchMode::Run)
    }

    fn launch_with(
        &self,
        player: &Path,
        _: &Path,
        _: RunId,
        _: EventSink,
        mode: LaunchMode,
    ) -> Result<Box<dyn ChildProcess>, LaunchError> {
        if mode == LaunchMode::Debug && self.0.no_debug.get() {
            return Err(LaunchError::new(
                player,
                std::io::Error::new(std::io::ErrorKind::Unsupported, "no debugger here"),
            ));
        }
        self.0.modes.borrow_mut().push(mode);
        Ok(Box::new(FakeChild(Rc::clone(&self.0))))
    }
}

impl ChildProcess for FakeChild {
    fn kill(&mut self) {
        self.0.kills.set(self.0.kills.get() + 1);
    }

    fn is_running(&mut self) -> bool {
        true
    }

    fn send_line(&mut self, line: &str) -> Result<(), SendError> {
        if self.0.broken.get() {
            return Err(SendError::Closed);
        }
        self.0.sent.borrow_mut().push(line.to_owned());
        Ok(())
    }
}

/// Runs `body` on a fresh IDE with a project (`main_form` plus `module1`) and
/// the fake launcher installed.
fn with_ide(label: &str, body: impl FnOnce(&mut IdeApp, &mut Ui<Msg>, &Rc<Wire>) + 'static) {
    let dir =
        std::env::temp_dir().join(format!("lazyrad-ide-debug-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cleanup = dir.clone();
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    run_app(backend, default_platform_spec(), move |ui| {
        let mut app = IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
        let mut session = ProjectSession::create("MyApp", &dir).expect("create the project");
        let module = session.add_module();
        session.set_code(&module, "fn helper(x) {\n    x + 1\n}\n".to_owned());
        session.set_code(
            crate::project::DEFAULT_FORM,
            "fn go_button_click() {\n    let a = 1;\n    let b = a + 1;\n    b\n}\n".to_owned(),
        );
        session.save().expect("the project saves");
        app.session = Some(session);
        app.dispatcher.set_project_open(true);
        app.refresh_explorer(ui);
        let player = dir.join("player-stub");
        std::fs::write(&player, b"stub").expect("write the player stub");
        app.settings.player_path = Some(player);
        let wire = Rc::new(Wire::default());
        app.launcher = Rc::new(FakeLauncher(Rc::clone(&wire)));
        body(&mut app, ui, &wire);
        app
    })
    .expect("the offscreen backend runs to completion");
    let _ = std::fs::remove_dir_all(&cleanup);
}

const FORM: &str = crate::project::DEFAULT_FORM;

/// Opens `item`'s code and puts the caret on one-based `line`.
fn caret_on(app: &mut IdeApp, ui: &mut Ui<Msg>, item: &str, line: usize) {
    app.open_code(item, ui);
    let editor = app.code_editor(item).expect("the code window is open");
    editor.goto(line - 1, 0);
}

fn stop_at(file: &str, line: u32, reason: StopReason) -> Stop {
    Stop {
        reason,
        file: file.to_owned(),
        line,
        message: (reason == StopReason::Exception).then(|| "Division by zero".to_owned()),
        stack: vec![
            Frame {
                function: "go_button_click".to_owned(),
                file: file.to_owned(),
                line,
            },
            Frame {
                function: "caller".to_owned(),
                file: "module1.rhai".to_owned(),
                line: 2,
            },
        ],
        locals: vec![Variable {
            name: "a".to_owned(),
            ty: "i64".to_owned(),
            value: "1".to_owned(),
        }],
        truncated: false,
    }
}

fn deliver(app: &mut IdeApp, ui: &mut Ui<Msg>, event: Event) {
    let id = app.run.run_id().expect("a run is active");
    app.on_run_event(id, RunEvent::Debug(event), ui);
}

fn has_marker(app: &IdeApp, item: &str, line0: usize, kind: MarkerKind) -> bool {
    app.markers_for(item)
        .iter()
        .any(|marker| marker.line == line0 && marker.kind == kind)
}

#[test]
fn toggling_a_breakpoint_marks_the_line_without_touching_the_document() {
    with_ide("toggle", |app, ui, _| {
        caret_on(app, ui, FORM, 3);
        let editor = app.code_editor(FORM).expect("editor");
        let before = (editor.text(), editor.revision(), editor.can_undo());

        app.run_command(Command::ToggleBreakpoint, ui);
        assert_eq!(app.breakpoints.lines(FORM), [3]);
        assert!(
            has_marker(app, FORM, 2, MarkerKind::Breakpoint),
            "gutter dot, zero-based"
        );

        let after = (editor.text(), editor.revision(), editor.can_undo());
        assert_eq!(before, after, "the text, revision and undo are untouched");
        assert!(
            app.documents.iter().all(|document| !document.dirty),
            "a breakpoint does not dirty the document"
        );
        assert!(!app.project_dirty());

        app.run_command(Command::ToggleBreakpoint, ui);
        assert!(app.breakpoints.is_empty());
        assert!(!has_marker(app, FORM, 2, MarkerKind::Breakpoint));
    });
}

#[test]
fn a_breakpoint_needs_a_code_window() {
    with_ide("nocode", |app, ui, _| {
        app.run_command(Command::ToggleBreakpoint, ui);
        assert!(app.breakpoints.is_empty());
        assert!(
            app.output_lines
                .iter()
                .any(|line| line.contains("code window")),
            "{:?}",
            app.output_lines
        );
    });
}

#[test]
fn start_runs_plain_without_breakpoints_and_under_the_debugger_with_them() {
    with_ide("modes", |app, ui, wire| {
        app.run_command(Command::RunStart, ui);
        assert_eq!(*wire.modes.borrow(), [LaunchMode::Run]);
        assert_eq!(app.dispatcher.debug_phase(), DebugPhase::Off);
        assert!(app.is_running());
        app.run_command(Command::RunEnd, ui);

        caret_on(app, ui, FORM, 2);
        app.run_command(Command::ToggleBreakpoint, ui);
        app.run_command(Command::RunStart, ui);
        assert_eq!(*wire.modes.borrow(), [LaunchMode::Run, LaunchMode::Debug]);
        assert_eq!(app.dispatcher.debug_phase(), DebugPhase::Running);
        assert_eq!(
            wire.commands(),
            [DebugCommand::SetBreakpoints {
                file: "main_form.rhai".to_owned(),
                lines: vec![2],
            }],
            "the breakpoints go out right after the launch"
        );
    });
}

#[test]
fn start_with_debugging_debugs_even_without_breakpoints() {
    with_ide("explicit", |app, ui, wire| {
        app.run_command(Command::RunDebug, ui);
        assert_eq!(*wire.modes.borrow(), [LaunchMode::Debug]);
        assert_eq!(app.dispatcher.debug_phase(), DebugPhase::Running);
        assert!(wire.commands().is_empty());
        // Break is now offered.
        assert!(app.dispatcher.is_enabled(Command::RunBreak));
        app.run_command(Command::RunBreak, ui);
        assert_eq!(wire.last_command(), Some(DebugCommand::Pause));
    });
}

#[test]
fn a_platform_that_cannot_debug_runs_plain_for_implicit_and_refuses_explicit() {
    with_ide("nodebug", |app, ui, wire| {
        wire.no_debug.set(true);
        caret_on(app, ui, FORM, 2);
        app.run_command(Command::ToggleBreakpoint, ui);

        app.run_command(Command::RunStart, ui);
        assert!(app.is_running(), "falls back to a plain run");
        assert_eq!(*wire.modes.borrow(), [LaunchMode::Run]);
        assert_eq!(app.dispatcher.debug_phase(), DebugPhase::Off);
        assert!(
            app.output_lines
                .iter()
                .any(|line| line.contains("breakpoints are ignored")),
            "the user is told why nothing will stop"
        );
        app.run_command(Command::RunEnd, ui);

        app.run_command(Command::RunDebug, ui);
        assert!(!app.is_running(), "an explicit request does not fall back");
        assert!(
            app.output_lines
                .iter()
                .any(|line| line.contains("could not start")),
        );
    });
}

#[test]
fn a_stop_shows_the_line_the_panes_and_continue_resumes() {
    with_ide("stop", |app, ui, wire| {
        caret_on(app, ui, FORM, 3);
        app.run_command(Command::ToggleBreakpoint, ui);
        app.run_command(Command::RunStart, ui);
        wire.clear();

        deliver(
            app,
            ui,
            Event::Stopped(stop_at("main_form.rhai", 3, StopReason::Breakpoint)),
        );

        assert_eq!(app.dispatcher.debug_phase(), DebugPhase::Stopped);
        assert_eq!(app.status.text(), "Break");
        assert_eq!(app.debug.at, Some((FORM.to_owned(), 3)));
        assert!(
            has_marker(app, FORM, 2, MarkerKind::Warning),
            "the stop line is tinted"
        );
        assert!(
            has_marker(app, FORM, 2, MarkerKind::Breakpoint),
            "and keeps its dot"
        );
        assert_eq!(app.locals_list.len(), 1);
        assert_eq!(
            app.locals_list.cell_text(0, 0).as_deref(),
            Some("a = 1   (i64)")
        );
        assert_eq!(app.stack_list.len(), 2);
        assert!(
            app.stack_list.cell_text(0, 0).is_some_and(
                |row| row.contains("go_button_click") && row.contains("main_form.rhai:3")
            ),
        );
        assert!(app.dispatcher.is_enabled(Command::StepOver));
        assert!(app.dispatcher.is_enabled(Command::RunStart), "F5 continues");

        // F5 while stopped is Continue, not a second launch.
        app.run_command(Command::RunStart, ui);
        assert_eq!(wire.last_command(), Some(DebugCommand::Continue));
        assert_eq!(wire.modes.borrow().len(), 1, "no second process");
        assert_eq!(app.dispatcher.debug_phase(), DebugPhase::Running);
        assert!(app.debug.at.is_none(), "the stop line is cleared at once");
        assert!(!has_marker(app, FORM, 2, MarkerKind::Warning));
        assert_eq!(app.locals_list.len(), 0, "and so are the panes");
        assert_eq!(app.stack_list.len(), 0);
        assert_eq!(app.status.text(), "Run");
    });
}

#[test]
fn the_step_commands_send_their_protocol_command_only_when_stopped() {
    with_ide("steps", |app, ui, wire| {
        app.run_command(Command::RunDebug, ui);
        wire.clear();
        // Running: refused locally, nothing sent.
        for command in [Command::StepInto, Command::StepOver, Command::StepOut] {
            app.run_command(command, ui);
        }
        assert!(wire.commands().is_empty());
        assert!(
            app.output_lines
                .iter()
                .any(|line| line == "The program is not stopped.")
        );

        for (command, expected) in [
            (Command::StepInto, DebugCommand::StepInto),
            (Command::StepOver, DebugCommand::StepOver),
            (Command::StepOut, DebugCommand::StepOut),
        ] {
            deliver(
                app,
                ui,
                Event::Stopped(stop_at("main_form.rhai", 2, StopReason::Step)),
            );
            app.run_command(command, ui);
            assert_eq!(wire.last_command(), Some(expected));
            assert_eq!(app.dispatcher.debug_phase(), DebugPhase::Running);
        }
    });
}

#[test]
fn a_stop_in_a_file_that_is_not_open_opens_it() {
    with_ide("openstop", |app, ui, _| {
        app.run_command(Command::RunDebug, ui);
        assert!(app.code_editor("module1").is_none(), "not open yet");

        deliver(
            app,
            ui,
            Event::Stopped(stop_at("module1.rhai", 2, StopReason::Breakpoint)),
        );

        assert!(app.code_editor("module1").is_some(), "the stop opened it");
        assert_eq!(app.debug.at, Some(("module1".to_owned(), 2)));
        assert!(has_marker(app, "module1", 1, MarkerKind::Warning));
        assert!(
            !has_marker(app, FORM, 1, MarkerKind::Warning),
            "the other document is untouched"
        );
    });
}

#[test]
fn a_stop_in_an_unknown_file_is_reported_without_marking_anything() {
    with_ide("unknownstop", |app, ui, _| {
        app.run_command(Command::RunDebug, ui);
        deliver(
            app,
            ui,
            Event::Stopped(stop_at("elsewhere.rhai", 9, StopReason::Breakpoint)),
        );
        assert_eq!(app.dispatcher.debug_phase(), DebugPhase::Stopped);
        assert!(app.debug.at.is_none());
        assert_eq!(app.stack_list.len(), 2, "the stack still shows");
        // Activating a frame whose file is not a project item is a no-op.
        app.update(Msg::StackActivated(1), ui);
        app.update(Msg::StackActivated(99), ui);
    });
}

#[test]
fn an_error_stop_tints_the_line_as_an_error() {
    with_ide("errstop", |app, ui, _| {
        app.run_command(Command::RunDebug, ui);
        deliver(
            app,
            ui,
            Event::Stopped(stop_at("main_form.rhai", 4, StopReason::Exception)),
        );
        assert!(has_marker(app, FORM, 3, MarkerKind::Error));
        assert!(
            app.output_lines
                .iter()
                .any(|line| line.contains("Division by zero")),
            "{:?}",
            app.output_lines
        );
    });
}

#[test]
fn ending_the_run_while_stopped_clears_everything_it_showed() {
    with_ide("endstop", |app, ui, wire| {
        caret_on(app, ui, FORM, 3);
        app.run_command(Command::ToggleBreakpoint, ui);
        app.run_command(Command::RunStart, ui);
        deliver(
            app,
            ui,
            Event::Stopped(stop_at("main_form.rhai", 3, StopReason::Breakpoint)),
        );
        // An evaluation is in flight.
        app.debug.input = "a + 1".to_owned();
        app.update(Msg::ImmediateEvaluate, ui);
        assert_eq!(app.debug.in_flight.len(), 1);
        let old_run = app.run.run_id().expect("run");

        app.run_command(Command::RunEnd, ui);

        assert!(!app.is_running());
        assert_eq!(app.dispatcher.debug_phase(), DebugPhase::Off);
        assert!(app.debug.at.is_none());
        assert!(app.debug.locals.is_empty() && app.debug.stack.is_empty());
        assert!(app.debug.in_flight.is_empty());
        assert_eq!(app.locals_list.len(), 0);
        assert_eq!(app.stack_list.len(), 0);
        assert!(!has_marker(app, FORM, 2, MarkerKind::Warning));
        assert!(
            has_marker(app, FORM, 2, MarkerKind::Breakpoint),
            "dots survive a run"
        );
        assert_eq!(app.status.text(), "Design");
        assert!(!app.dispatcher.is_enabled(Command::StepOver));
        assert_eq!(wire.kills.get(), 1);

        // Late events of the ended run are dropped, not applied to the next.
        app.run_command(Command::RunDebug, ui);
        assert_ne!(app.run.run_id(), Some(old_run));
        app.on_run_event(
            old_run,
            RunEvent::Debug(Event::Stopped(stop_at(
                "main_form.rhai",
                2,
                StopReason::Pause,
            ))),
            ui,
        );
        assert_eq!(app.dispatcher.debug_phase(), DebugPhase::Running);
        assert!(app.debug.at.is_none());
        app.on_run_event(
            old_run,
            RunEvent::Debug(Event::Evaluated {
                id: 0,
                ok: true,
                value: "2".to_owned(),
                ty: "i64".to_owned(),
            }),
            ui,
        );
        assert!(
            app.debug.immediate.iter().all(|line| line != "= 2   (i64)"),
            "{:?}",
            app.debug.immediate
        );
    });
}

#[test]
fn the_program_exiting_on_its_own_clears_the_stop_too() {
    with_ide("exitstop", |app, ui, _| {
        app.run_command(Command::RunDebug, ui);
        deliver(
            app,
            ui,
            Event::Stopped(stop_at("main_form.rhai", 2, StopReason::Pause)),
        );
        let id = app.run.run_id().expect("run");
        app.on_run_event(id, RunEvent::Exited(Some(1)), ui);
        assert!(!app.is_running());
        assert_eq!(app.dispatcher.debug_phase(), DebugPhase::Off);
        assert!(app.debug.at.is_none());
        assert_eq!(app.locals_list.len(), 0);
    });
}

#[test]
fn a_dead_player_leaves_the_ide_stopped_until_its_exit_arrives() {
    with_ide("dead", |app, ui, wire| {
        app.run_command(Command::RunDebug, ui);
        deliver(
            app,
            ui,
            Event::Stopped(stop_at("main_form.rhai", 2, StopReason::Pause)),
        );
        wire.broken.set(true);
        app.run_command(Command::StepOver, ui);
        assert!(
            app.output_lines
                .iter()
                .any(|line| line.contains("could not reach the program")),
            "{:?}",
            app.output_lines
        );
        assert_eq!(
            app.dispatcher.debug_phase(),
            DebugPhase::Stopped,
            "a command that was not sent changed nothing"
        );
        let id = app.run.run_id().expect("run");
        app.on_run_event(id, RunEvent::Exited(None), ui);
        assert_eq!(app.dispatcher.debug_phase(), DebugPhase::Off);
    });
}

#[test]
fn edits_move_breakpoints_and_the_stop_line_and_drop_deleted_lines() {
    with_ide("drift", |app, ui, _| {
        caret_on(app, ui, FORM, 3);
        app.run_command(Command::ToggleBreakpoint, ui);
        caret_on(app, ui, FORM, 5);
        app.run_command(Command::ToggleBreakpoint, ui);
        assert_eq!(app.breakpoints.lines(FORM), [3, 5]);

        let original = app
            .session
            .as_ref()
            .and_then(|session| session.code(FORM))
            .expect("code")
            .to_owned();

        // Two lines inserted at the top: both move down.
        let inserted = format!("// one\n// two\n{original}");
        app.update(Msg::DocumentEdited(FORM.to_owned(), inserted.clone()), ui);
        assert_eq!(app.breakpoints.lines(FORM), [5, 7]);
        assert!(has_marker(app, FORM, 4, MarkerKind::Breakpoint));
        assert!(!has_marker(app, FORM, 2, MarkerKind::Breakpoint));

        // The first breakpoint's line is deleted: it goes, the other follows.
        let mut lines: Vec<&str> = inserted.split('\n').collect();
        lines.remove(4);
        app.update(Msg::DocumentEdited(FORM.to_owned(), lines.join("\n")), ui);
        assert_eq!(app.breakpoints.lines(FORM), [6]);

        // The document shrinks below the last breakpoint's line.
        app.update(Msg::DocumentEdited(FORM.to_owned(), "x".to_owned()), ui);
        assert!(
            app.breakpoints.lines(FORM).iter().all(|line| *line == 1),
            "nothing is left past the end: {:?}",
            app.breakpoints.lines(FORM)
        );
    });
}

#[test]
fn crlf_edits_shift_breakpoints_like_lf_ones() {
    with_ide("crlf", |app, ui, _| {
        let crlf = "fn a() {\r\n    1\r\n}\r\nfn b() {\r\n    2\r\n}\r\n";
        app.update(Msg::DocumentEdited(FORM.to_owned(), crlf.to_owned()), ui);
        app.breakpoints.toggle(FORM, 5);
        let edited = format!("// header\r\n{crlf}");
        app.update(Msg::DocumentEdited(FORM.to_owned(), edited), ui);
        assert_eq!(app.breakpoints.lines(FORM), [6]);
    });
}

#[test]
fn breakpoints_follow_a_renamed_item_and_go_with_a_removed_one() {
    with_ide("rename", |app, ui, wire| {
        app.breakpoints.toggle("module1", 2);
        app.rename_item("module1", "util", ui);
        assert!(app.breakpoints.lines("module1").is_empty());
        assert_eq!(app.breakpoints.lines("util"), [2]);

        app.run_command(Command::RunDebug, ui);
        wire.clear();
        // Removal is refused while the program runs; end it first.
        app.run_command(Command::RunEnd, ui);
        app.remove_item("util", ui);
        assert!(app.breakpoints.is_empty());
    });
}

#[test]
fn closing_the_project_forgets_its_breakpoints() {
    with_ide("close", |app, ui, _| {
        caret_on(app, ui, FORM, 2);
        app.run_command(Command::ToggleBreakpoint, ui);
        app.close_project(ui);
        assert!(app.breakpoints.is_empty());
        assert!(!app.is_running());
    });
}

#[test]
fn many_files_keep_their_own_sets_and_markers() {
    with_ide("many", |app, ui, wire| {
        caret_on(app, ui, FORM, 2);
        app.run_command(Command::ToggleBreakpoint, ui);
        caret_on(app, ui, FORM, 4);
        app.run_command(Command::ToggleBreakpoint, ui);
        caret_on(app, ui, "module1", 2);
        app.run_command(Command::ToggleBreakpoint, ui);
        assert_eq!(app.breakpoints.lines(FORM), [2, 4]);
        assert_eq!(app.breakpoints.lines("module1"), [2]);

        app.run_command(Command::RunStart, ui);
        let mut sets: Vec<(String, Vec<u32>)> = wire
            .commands()
            .into_iter()
            .filter_map(|command| match command {
                DebugCommand::SetBreakpoints { file, lines } => Some((file, lines)),
                _ => None,
            })
            .collect();
        sets.sort();
        assert_eq!(
            sets,
            [
                ("main_form.rhai".to_owned(), vec![2, 4]),
                ("module1.rhai".to_owned(), vec![2]),
            ]
        );
        assert!(has_marker(app, "module1", 1, MarkerKind::Breakpoint));
        assert!(!has_marker(app, "module1", 3, MarkerKind::Breakpoint));
    });
}

#[test]
fn toggling_during_a_debug_run_updates_the_player() {
    with_ide("live", |app, ui, wire| {
        app.run_command(Command::RunDebug, ui);
        wire.clear();
        caret_on(app, ui, FORM, 3);
        app.run_command(Command::ToggleBreakpoint, ui);
        assert_eq!(
            wire.last_command(),
            Some(DebugCommand::SetBreakpoints {
                file: "main_form.rhai".to_owned(),
                lines: vec![3],
            })
        );
        app.run_command(Command::ToggleBreakpoint, ui);
        assert_eq!(
            wire.last_command(),
            Some(DebugCommand::SetBreakpoints {
                file: "main_form.rhai".to_owned(),
                lines: Vec::new(),
            }),
            "clearing the last one sends an empty set"
        );
    });
}

#[test]
fn a_plain_run_tells_the_user_why_a_new_breakpoint_will_not_stop() {
    with_ide("plain", |app, ui, wire| {
        app.run_command(Command::RunStart, ui);
        caret_on(app, ui, FORM, 3);
        app.run_command(Command::ToggleBreakpoint, ui);
        assert!(
            wire.commands().is_empty(),
            "nothing can be sent to a plain run"
        );
        assert!(
            app.output_lines
                .iter()
                .any(|line| line.contains("without the debugger")),
        );
    });
}

#[test]
fn saving_in_a_debug_run_resends_the_breakpoints() {
    with_ide("resave", |app, ui, wire| {
        caret_on(app, ui, FORM, 3);
        app.run_command(Command::ToggleBreakpoint, ui);
        app.run_command(Command::RunStart, ui);
        wire.clear();
        app.run_command(Command::Save, ui);
        assert_eq!(
            wire.commands(),
            [DebugCommand::SetBreakpoints {
                file: "main_form.rhai".to_owned(),
                lines: vec![3],
            }]
        );
    });
}

#[test]
fn the_immediate_pane_evaluates_only_in_a_stopped_frame() {
    with_ide("immediate", |app, ui, wire| {
        app.run_command(Command::RunDebug, ui);
        wire.clear();

        // Running: no-op with an explanation, nothing sent.
        app.update(Msg::ImmediateChanged("a + 1".to_owned()), ui);
        app.update(Msg::ImmediateEvaluate, ui);
        assert!(wire.commands().is_empty());
        assert!(
            app.debug
                .immediate
                .iter()
                .any(|line| line.contains("not available while the program runs"))
        );
        // The input is kept so the user can try again once stopped.
        assert_eq!(app.debug.input, "a + 1");

        deliver(
            app,
            ui,
            Event::Stopped(stop_at("main_form.rhai", 2, StopReason::Pause)),
        );
        app.update(Msg::ImmediateEvaluate, ui);
        let Some(DebugCommand::Evaluate { expr, frame, id }) = wire.last_command() else {
            panic!("an evaluate was sent: {:?}", wire.commands());
        };
        assert_eq!((expr.as_str(), frame), ("a + 1", 0));
        assert!(app.debug.input.is_empty(), "the input is consumed");

        deliver(
            app,
            ui,
            Event::Evaluated {
                id,
                ok: true,
                value: "2".to_owned(),
                ty: "i64".to_owned(),
            },
        );
        assert_eq!(
            app.debug.immediate.last().map(String::as_str),
            Some("= 2   (i64)")
        );
        assert_eq!(app.immediate_list.len(), app.debug.immediate.len());

        // An unknown id (a duplicate or stale answer) is ignored.
        let count = app.debug.immediate.len();
        deliver(
            app,
            ui,
            Event::Evaluated {
                id: 999,
                ok: false,
                value: "late".to_owned(),
                ty: String::new(),
            },
        );
        assert_eq!(app.debug.immediate.len(), count);

        // An empty input does nothing.
        app.update(Msg::ImmediateChanged("   ".to_owned()), ui);
        app.update(Msg::ImmediateEvaluate, ui);
        assert_eq!(app.debug.immediate.len(), count);
    });
}

#[test]
fn shortcuts_follow_the_enabled_state() {
    with_ide("shortcuts", |app, ui, wire| {
        // Idle: step shortcuts do nothing.
        app.update(Msg::Shortcut(Command::StepOver), ui);
        assert!(wire.commands().is_empty());
        app.update(Msg::Shortcut(Command::RunDebug), ui);
        assert_eq!(app.dispatcher.debug_phase(), DebugPhase::Running);
        deliver(
            app,
            ui,
            Event::Stopped(stop_at("main_form.rhai", 2, StopReason::Pause)),
        );
        app.update(Msg::Shortcut(Command::StepOver), ui);
        assert_eq!(wire.last_command(), Some(DebugCommand::StepOver));
    });
}

#[test]
fn output_and_errors_from_the_debugger_reach_the_output_pane() {
    with_ide("output", |app, ui, _| {
        app.run_command(Command::RunDebug, ui);
        deliver(
            app,
            ui,
            Event::Error {
                message: "the program is not stopped".to_owned(),
            },
        );
        deliver(
            app,
            ui,
            Event::Output {
                text: "note".to_owned(),
                stream: lazyrad_debug_proto::Stream::Debugger,
            },
        );
        assert!(
            app.output_lines
                .iter()
                .any(|line| line == "[debugger] the program is not stopped")
        );
        assert!(app.output_lines.iter().any(|line| line == "note"));
    });
}
