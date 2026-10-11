//! Headless tests of the debugger session: a scripted transport stands in for
//! the IDE and the host test kit drives real handlers on an offscreen form.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use lazyrad_debug_proto::{Command, Event, Stop, StopReason};
use lazyrad_project::{FormDoc, Node};
use xui_form::Value;

use super::{DebugSession, DebugTransport, Incoming};
use crate::form::{FormRuntime, FormSource, ModuleSource};
use crate::testing::{TestApp, run_on_large_stack};

/// A transport that plays back a script.
///
/// * `queued` is what `poll` (and a `wait` with nothing else) returns first;
/// * `timed` delivers an item on one specific `poll` call, to arrive while a
///   script is mid-run;
/// * `reactions` holds one batch of commands per `stopped` event: when the
///   session reports a stop, the next batch becomes what `wait` returns.
///
/// A `wait` with nothing scripted reports the pipe closed, so a test that
/// under-scripts a stop detaches instead of hanging.
#[derive(Default)]
struct Script {
    sent: RefCell<Vec<Event>>,
    polls: Cell<usize>,
    timed: RefCell<Vec<(usize, Incoming)>>,
    queued: RefCell<VecDeque<Incoming>>,
    reactions: RefCell<VecDeque<Vec<Incoming>>>,
    waiting: RefCell<VecDeque<Incoming>>,
}

impl Script {
    fn new() -> Rc<Script> {
        Rc::new(Script::default())
    }

    fn on_stop(&self, commands: Vec<Command>) {
        self.reactions
            .borrow_mut()
            .push_back(commands.into_iter().map(Incoming::Command).collect());
    }

    fn queue(&self, command: Command) {
        self.queued
            .borrow_mut()
            .push_back(Incoming::Command(command));
    }

    fn stops(&self) -> Vec<Stop> {
        self.sent
            .borrow()
            .iter()
            .filter_map(|event| match event {
                Event::Stopped(stop) => Some(stop.clone()),
                _ => None,
            })
            .collect()
    }

    fn events(&self) -> Vec<Event> {
        self.sent.borrow().clone()
    }

    fn errors(&self) -> Vec<String> {
        self.sent
            .borrow()
            .iter()
            .filter_map(|event| match event {
                Event::Error { message } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }
}

impl DebugTransport for Script {
    fn send(&self, event: &Event) {
        self.sent.borrow_mut().push(event.clone());
        if matches!(event, Event::Stopped(_))
            && let Some(batch) = self.reactions.borrow_mut().pop_front()
        {
            self.waiting.borrow_mut().extend(batch);
        }
    }

    fn poll(&self) -> Incoming {
        let call = self.polls.get();
        self.polls.set(call + 1);
        let due = self.timed.borrow().iter().position(|(at, _)| *at == call);
        if let Some(index) = due {
            return self.timed.borrow_mut().remove(index).1;
        }
        self.queued
            .borrow_mut()
            .pop_front()
            .unwrap_or(Incoming::Empty)
    }

    fn wait(&self) -> Incoming {
        if let Some(item) = self.waiting.borrow_mut().pop_front() {
            return item;
        }
        self.queued
            .borrow_mut()
            .pop_front()
            .unwrap_or(Incoming::Closed)
    }
}

/// A form with one button, `go_button`.
fn form_doc() -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    let mut button = Node::new("Button", "go_button");
    button.set_prop("left", Value::Int(10));
    button.set_prop("top", Value::Int(10));
    button.set_prop("width", Value::Int(80));
    button.set_prop("height", Value::Int(28));
    doc.insert(button);
    doc
}

/// Runs `drive` on a form whose code is `code`, under a session over `script`.
fn debug_run(
    code: &str,
    modules: Vec<ModuleSource>,
    script: &Rc<Script>,
    drive: impl FnOnce(&mut TestApp, &Rc<DebugSession>) + 'static,
) -> Rc<DebugSession> {
    let session = DebugSession::new(Rc::clone(script) as Rc<dyn DebugTransport>);
    let runtime = FormRuntime::from_sources(
        vec![FormSource::new("main_form", form_doc(), code)],
        modules,
    );
    runtime.set_debugger(Rc::clone(&session));
    let handle = Rc::clone(&session);
    TestApp::run_runtime(runtime, move |app| drive(app, &handle)).expect("the app runs");
    session
}

fn breakpoints(file: &str, lines: &[u32]) -> Command {
    Command::SetBreakpoints {
        file: file.to_owned(),
        lines: lines.to_vec(),
    }
}

/// The stop's innermost function and line.
fn at(stop: &Stop) -> (&str, u32) {
    (stop.stack[0].function.as_str(), stop.line)
}

fn local<'a>(stop: &'a Stop, name: &str) -> Option<&'a str> {
    stop.locals
        .iter()
        .find(|variable| variable.name == name)
        .map(|variable| variable.value.as_str())
}

const CALLS: &str = "fn go_button_click() {
    let a = 1;
    let b = a + 1;
    let c = helper(b);
    c
}
fn helper(x) {
    let y = x * 10;
    y + 1
}";

#[test]
fn a_breakpoint_stops_on_its_line_with_locals_and_stack() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[3]));
        script.on_stop(vec![Command::Continue]);
        let session = debug_run(CALLS, Vec::new(), &script, |app, _| {
            app.click("go_button");
        });

        let stops = script.stops();
        assert_eq!(stops.len(), 1, "{:?}", script.events());
        let stop = &stops[0];
        assert_eq!(stop.reason, StopReason::Breakpoint);
        assert_eq!(stop.file, "main_form.rhai");
        assert_eq!(at(stop), ("go_button_click", 3));
        assert_eq!(local(stop, "a"), Some("1"));
        assert_eq!(local(stop, "b"), None, "b is declared on the stopped line");
        assert_eq!(stop.stack.len(), 1);
        assert!(matches!(script.events().last(), Some(Event::Resumed)));
        assert_eq!(session.stop_count(), 1);
        assert!(!session.is_stopped());
    });
}

#[test]
fn a_breakpoint_in_a_called_function_shows_both_frames() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[8]));
        script.on_stop(vec![Command::Continue]);
        debug_run(CALLS, Vec::new(), &script, |app, _| {
            app.click("go_button");
        });
        let stop = &script.stops()[0];
        assert_eq!(at(stop), ("helper", 8));
        assert_eq!(stop.stack.len(), 2);
        assert_eq!(stop.stack[1].function, "go_button_click");
        assert_eq!(stop.stack[1].file, "main_form.rhai");
        assert_eq!(stop.stack[1].line, 4, "the call site");
        assert_eq!(local(stop, "x"), Some("2"));
        assert_eq!(local(stop, "a"), None, "a call has its own scope");
    });
}

#[test]
fn step_over_stays_in_the_function_and_step_into_enters_the_call() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[4]));
        script.on_stop(vec![Command::StepOver]);
        script.on_stop(vec![Command::Continue]);
        debug_run(CALLS, Vec::new(), &script, |app, _| {
            app.click("go_button");
        });
        let stops = script.stops();
        assert_eq!(stops.len(), 2, "{:?}", script.events());
        assert_eq!(at(&stops[1]), ("go_button_click", 5));
        assert_eq!(stops[1].reason, StopReason::Step);
        assert_eq!(local(&stops[1], "c"), Some("21"));

        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[4]));
        script.on_stop(vec![Command::StepInto]);
        script.on_stop(vec![Command::StepOut]);
        script.on_stop(vec![Command::Continue]);
        debug_run(CALLS, Vec::new(), &script, |app, _| {
            app.click("go_button");
        });
        let stops = script.stops();
        assert_eq!(stops.len(), 3, "{:?}", script.events());
        assert_eq!(stops[1].stack[0].function, "helper", "stepped in");
        assert_eq!(stops[2].stack.len(), 1, "stepped out");
        // Back in the caller, on the line after the call.
        assert_eq!(at(&stops[2]), ("go_button_click", 5));
    });
}

#[test]
fn stepping_out_of_the_outermost_frame_just_finishes_the_handler() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[3]));
        script.on_stop(vec![Command::StepOut]);
        debug_run(CALLS, Vec::new(), &script, |app, _| {
            app.click("go_button");
            app.click("go_button");
        });
        // Each click stops once (the breakpoint), step out leaves no more
        // stops; the second click's stop has no scripted reaction and the
        // pipe-closed answer detaches the session.
        assert!(script.stops().len() <= 2);
        assert_eq!(script.stops()[0].line, 3);
        assert!(
            script.events().iter().any(|e| matches!(e, Event::Resumed)),
            "{:?}",
            script.events()
        );
    });
}

#[test]
fn stepping_past_the_last_line_ends_the_stop() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[5]));
        script.on_stop(vec![Command::StepOver]);
        let session = debug_run(CALLS, Vec::new(), &script, |app, _| {
            app.click("go_button");
        });
        // `c` is the last statement: stepping over it leaves the handler, and
        // the program is idle rather than stopped again.
        assert_eq!(script.stops().len(), 1, "{:?}", script.events());
        assert!(!session.is_stopped());
    });
}

#[test]
fn a_script_error_stops_at_the_failing_position() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.on_stop(vec![Command::Continue]);
        let session = debug_run(
            "fn go_button_click() {
    let a = 5;
    let b = 0;
    a / b
}",
            Vec::new(),
            &script,
            |app, _| {
                app.allow_errors().click("go_button");
                assert_eq!(app.errors().len(), 1, "the error still follows its route");
            },
        );
        let stops = script.stops();
        assert_eq!(stops.len(), 1, "{:?}", script.events());
        assert_eq!(stops[0].reason, StopReason::Exception);
        assert_eq!(at(&stops[0]), ("go_button_click", 4));
        assert!(
            stops[0]
                .message
                .as_deref()
                .is_some_and(|m| m.starts_with("Division by zero")),
            "{:?}",
            stops[0].message
        );
        assert_eq!(local(&stops[0], "a"), Some("5"));
        assert_eq!(local(&stops[0], "b"), Some("0"));
        assert!(!session.is_stopped());
    });
}

#[test]
fn an_error_in_a_helper_stops_once_in_the_failing_frame() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.on_stop(vec![Command::StepOver]);
        debug_run(
            "fn go_button_click() {
    let a = helper(2);
    a
}
fn helper(n) {
    let zero = 0;
    n / zero
}",
            Vec::new(),
            &script,
            |app, _| {
                app.allow_errors().click("go_button");
            },
        );
        let stops = script.stops();
        assert_eq!(
            stops.len(),
            1,
            "once, not once per frame: {:?}",
            script.events()
        );
        assert_eq!(at(&stops[0]), ("helper", 7));
        assert_eq!(stops[0].stack[1].function, "go_button_click");
        assert_eq!(stops[0].stack[1].line, 2);
        assert_eq!(local(&stops[0], "n"), Some("2"));
    });
}

#[test]
fn a_second_identical_error_stops_again() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.on_stop(vec![Command::Continue]);
        script.on_stop(vec![Command::Continue]);
        debug_run(
            "fn go_button_click() {\n    let z = 0;\n    1 / z\n}",
            Vec::new(),
            &script,
            |app, _| {
                app.allow_errors().click("go_button").click("go_button");
            },
        );
        assert_eq!(script.stops().len(), 2);
    });
}

#[test]
fn a_runaway_loop_with_a_breakpoint_stops_and_then_breaks_on_the_budget() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[4]));
        // First the breakpoint; clear it and let the loop run away, which the
        // operation budget ends as an error the debugger stops on.
        script.on_stop(vec![breakpoints("main_form.rhai", &[]), Command::Continue]);
        script.on_stop(vec![Command::Continue]);
        debug_run(
            "fn go_button_click() {
    let n = 0;
    loop {
        n += 1;
    }
}",
            Vec::new(),
            &script,
            |app, _| {
                app.allow_errors().click("go_button");
            },
        );
        let stops = script.stops();
        assert_eq!(stops.len(), 2, "{:?}", script.events());
        assert_eq!(stops[0].reason, StopReason::Breakpoint);
        assert_eq!(stops[0].line, 4);
        assert_eq!(stops[1].reason, StopReason::Exception);
        assert!(
            stops[1]
                .message
                .as_deref()
                .is_some_and(|m| m.contains("operation budget")),
            "{:?}",
            stops[1].message
        );
    });
}

#[test]
fn a_pause_reaches_a_loop_that_never_returns() {
    run_on_large_stack(|| {
        let script = Script::new();
        // Poll 0 is the handler's start; poll 3 is some hundreds of nodes in.
        script
            .timed
            .borrow_mut()
            .push((3, Incoming::Command(Command::Pause)));
        script.on_stop(vec![Command::Stop]);
        let stopped = Rc::new(Cell::new(0));
        let hook_count = Rc::clone(&stopped);
        let session = debug_run(
            "fn go_button_click() {
    let n = 0;
    loop {
        n += 1;
        let m = n * 2;
    }
}",
            Vec::new(),
            &script,
            move |app, session| {
                let hook_count = Rc::clone(&hook_count);
                session.set_stop_hook(Rc::new(move || hook_count.set(hook_count.get() + 1)));
                app.allow_errors().click("go_button");
            },
        );
        let stops = script.stops();
        assert_eq!(stops.len(), 1, "{:?}", script.events());
        assert_eq!(stops[0].reason, StopReason::Pause);
        assert!(local(&stops[0], "n").is_some());
        assert_eq!(stopped.get(), 1, "stop ran the host's hook once");
        assert!(!session.is_stopped());
        // The terminated handler is not reported as a second stop.
        assert!(script.errors().is_empty(), "{:?}", script.errors());
    });
}

#[test]
fn a_pause_with_nothing_running_waits_for_the_next_handler() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(Command::Pause);
        script.on_stop(vec![Command::Continue]);
        let session = debug_run(CALLS, Vec::new(), &script, |app, session| {
            // Idle: the pause is read by the pump and nothing stops.
            session.pump();
            assert!(!session.is_stopped());
            assert_eq!(app.errors().len(), 0);
            app.click("go_button");
        });
        let stops = script.stops();
        assert_eq!(stops.len(), 1, "{:?}", script.events());
        assert_eq!(stops[0].reason, StopReason::Pause);
        assert_eq!(session.stop_count(), 1);
    });
}

#[test]
fn breakpoints_set_while_idle_apply_at_the_next_handler() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.on_stop(vec![Command::Continue]);
        let in_test = Rc::clone(&script);
        debug_run(CALLS, Vec::new(), &script, move |app, session| {
            app.click("go_button");
            assert!(in_test.stops().is_empty(), "no breakpoint yet");
            in_test.queue(breakpoints("main_form.rhai", &[2]));
            app.click("go_button");
            assert_eq!(session.breakpoints("main_form.rhai"), [2]);
        });
        assert_eq!(script.stops().len(), 1);
        assert_eq!(script.stops()[0].line, 2);
    });
}

#[test]
fn evaluate_reads_assigns_and_calls_script_functions() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[4]));
        let evaluate = |expr: &str, frame: u32, id: u64| Command::Evaluate {
            expr: expr.to_owned(),
            frame,
            id,
        };
        script.on_stop(vec![
            evaluate("a + b * 10", 0, 1),
            evaluate("helper(2)", 0, 2),
            evaluate("nothing_here + 1", 0, 3),
            evaluate("a", 1, 4),
            evaluate("a = 9; a", 0, 5),
            evaluate("((", 0, 6),
            evaluate("go_button.text", 0, 7),
            Command::Continue,
        ]);
        debug_run(CALLS, Vec::new(), &script, |app, _| {
            app.click("go_button");
        });
        let answers: Vec<(u64, bool, String, String)> = script
            .events()
            .into_iter()
            .filter_map(|event| match event {
                Event::Evaluated { id, ok, value, ty } => Some((id, ok, value, ty)),
                _ => None,
            })
            .collect();
        assert_eq!(answers.len(), 7, "{answers:?}");
        assert_eq!(answers[0], (1, true, "21".to_owned(), "i64".to_owned()));
        assert_eq!(answers[1].2, "21", "helper(2) = 2 * 10 + 1");
        assert!(answers[1].1);
        assert!(!answers[2].1, "an unknown variable is an error");
        assert!(!answers[3].1 && answers[3].2.contains("innermost"));
        assert_eq!(answers[4].2, "9", "assignment is allowed");
        assert!(!answers[5].1, "a syntax error is reported");
        assert!(answers[6].1, "controls resolve: {:?}", answers[6]);
        assert!(!script.stops().is_empty());
    });
}

/// A timing probe, not a check: how much the per-node callback costs on a
/// tight loop. Run with
/// `cargo test --release -p lazyrad-runtime --lib timing -- --ignored --nocapture`.
#[test]
#[ignore = "timing probe"]
fn timing_of_a_hot_loop_with_and_without_the_debugger() {
    run_on_large_stack(|| {
        let code = "fn go_button_click() {
    let n = 0;
    let total = 0;
    while n < 40000 {
        n += 1;
        total += n % 7;
    }
    total
}";
        let plain = std::time::Instant::now();
        TestApp::run_sources(
            vec![FormSource::new("main_form", form_doc(), code)],
            Vec::new(),
            |app| {
                app.click("go_button");
            },
        )
        .expect("runs");
        let plain = plain.elapsed();

        let script = Script::new();
        let debugged = std::time::Instant::now();
        debug_run(code, Vec::new(), &script, |app, _| {
            app.click("go_button");
        });
        let debugged = debugged.elapsed();
        println!("plain {plain:?}, debugger attached {debugged:?}");
    });
}

#[test]
fn an_evaluation_that_runs_a_function_with_a_breakpoint_does_not_stop_again() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[4, 8]));
        script.on_stop(vec![
            Command::Evaluate {
                expr: "helper(5)".to_owned(),
                frame: 0,
                id: 1,
            },
            Command::Continue,
        ]);
        script.on_stop(vec![Command::Continue]);
        let session = debug_run(CALLS, Vec::new(), &script, |app, _| {
            app.click("go_button");
        });
        let events = script.events();
        let answer = events.iter().find_map(|event| match event {
            Event::Evaluated { value, ok, .. } => Some((value.clone(), *ok)),
            _ => None,
        });
        assert_eq!(answer, Some(("51".to_owned(), true)), "{events:?}");
        // Line 4 (the first stop), then line 8 when the program itself calls
        // helper: the evaluation's own call of helper did not stop.
        let lines: Vec<u32> = script.stops().iter().map(|stop| stop.line).collect();
        assert_eq!(lines, [4, 8], "{events:?}");
        assert!(!session.is_stopped());
    });
}

#[test]
fn evaluate_while_running_is_answered_not_run() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(Command::Evaluate {
            expr: "1".to_owned(),
            frame: 0,
            id: 11,
        });
        debug_run(CALLS, Vec::new(), &script, |app, _| {
            app.click("go_button");
        });
        let answer = script.events().into_iter().find_map(|event| match event {
            Event::Evaluated { id, ok, value, .. } => Some((id, ok, value)),
            _ => None,
        });
        let (id, ok, value) = answer.expect("answered");
        assert_eq!(id, 11);
        assert!(!ok);
        assert!(value.contains("running"), "{value}");
    });
}

#[test]
fn resume_commands_while_running_are_refused_not_queued() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(Command::Continue);
        script.queue(Command::StepInto);
        debug_run(CALLS, Vec::new(), &script, |app, _| {
            app.click("go_button");
        });
        assert_eq!(script.errors().len(), 2, "{:?}", script.events());
        assert!(script.stops().is_empty(), "nothing was left pending");
    });
}

#[test]
fn a_rejected_line_is_reported_and_the_program_carries_on() {
    run_on_large_stack(|| {
        let script = Script::new();
        script
            .queued
            .borrow_mut()
            .push_back(Incoming::Rejected("unknown message `bogus`".to_owned()));
        script.queue(breakpoints("main_form.rhai", &[3]));
        script.on_stop(vec![Command::Continue]);
        debug_run(CALLS, Vec::new(), &script, |app, _| {
            app.click("go_button");
        });
        assert_eq!(script.errors(), ["unknown message `bogus`"]);
        assert_eq!(script.stops().len(), 1, "the next command still applied");
    });
}

#[test]
fn a_closed_pipe_while_stopped_detaches_and_resumes() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[3]));
        script
            .reactions
            .borrow_mut()
            .push_back(vec![Incoming::Closed]);
        let session = debug_run(CALLS, Vec::new(), &script, |app, session| {
            app.click("go_button");
            assert!(session.is_detached());
            assert!(!session.is_stopped());
            // The debugger is off: the breakpoint no longer stops anything.
            app.click("go_button");
            assert!(app.errors().is_empty());
        });
        assert_eq!(script.stops().len(), 1);
        assert!(session.is_detached());
    });
}

#[test]
fn a_stop_command_ends_the_script_and_runs_the_hook() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[3]));
        script.on_stop(vec![Command::Stop]);
        let hooked = Rc::new(Cell::new(0));
        let hook = Rc::clone(&hooked);
        debug_run(CALLS, Vec::new(), &script, move |app, session| {
            session.set_stop_hook(Rc::new(move || hook.set(hook.get() + 1)));
            app.allow_errors().click("go_button");
            assert_eq!(app.errors().len(), 1, "the handler was terminated");
        });
        assert_eq!(hooked.get(), 1);
        assert_eq!(
            script.stops().len(),
            1,
            "an error from our own stop is not a stop"
        );
    });
}

#[test]
fn a_module_function_reports_its_own_file() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("util.rhai", &[2]));
        script.on_stop(vec![Command::Continue]);
        let util = ModuleSource::new("util", "fn double(x) {\n    let r = x * 2;\n    r\n}");
        debug_run(
            "fn go_button_click() {\n    let a = util::double(4);\n    a\n}",
            vec![util],
            &script,
            |app, _| {
                app.click("go_button");
            },
        );
        let stops = script.stops();
        assert_eq!(stops.len(), 1, "{:?}", script.events());
        assert_eq!(stops[0].file, "util.rhai");
        assert_eq!(at(&stops[0]), ("double", 2));
        assert_eq!(stops[0].stack[1].function, "go_button_click");
        assert_eq!(stops[0].stack[1].file, "main_form.rhai");
        assert_eq!(stops[0].stack[1].line, 2);
    });
}

#[test]
fn an_unqualified_module_call_is_attributed_to_the_module_too() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("util.rhai", &[2]));
        script.on_stop(vec![Command::Continue]);
        let util = ModuleSource::new(
            "util",
            "fn double(x) {
    let r = x * 2;
    r
}",
        );
        debug_run(
            "fn go_button_click() {
    let a = double(4);
    a
}",
            vec![util],
            &script,
            |app, _| {
                app.click("go_button");
            },
        );
        let stops = script.stops();
        assert_eq!(stops.len(), 1, "{:?}", script.events());
        assert_eq!(stops[0].file, "util.rhai");
        assert_eq!(stops[0].stack[1].file, "main_form.rhai");
    });
}

#[test]
fn crlf_sources_count_lines_the_same() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[3]));
        script.on_stop(vec![Command::Continue]);
        let crlf = CALLS.replace('\n', "\r\n");
        debug_run(&crlf, Vec::new(), &script, |app, _| {
            app.click("go_button");
        });
        let stops = script.stops();
        assert_eq!(stops.len(), 1, "{:?}", script.events());
        assert_eq!(stops[0].line, 3);
    });
}

#[test]
fn breakpoints_belong_to_the_session_and_survive_a_rebuild() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[3]));
        script.on_stop(vec![Command::Continue]);
        let session = debug_run(CALLS, Vec::new(), &script, |app, _| {
            app.click("go_button");
        });
        assert_eq!(session.breakpoints("main_form.rhai"), [3]);

        // A hot reload builds a new runtime and engines for the same session.
        // The new text moved the code down two lines; the breakpoint stays on
        // line 3 (the IDE re-sends its set when the file is saved).
        script.on_stop(vec![Command::Continue]);
        let shifted = format!("// one\n// two\n{CALLS}");
        let runtime = FormRuntime::from_sources(
            vec![FormSource::new("main_form", form_doc(), shifted)],
            Vec::new(),
        );
        runtime.set_debugger(Rc::clone(&session));
        TestApp::run_runtime(runtime, |app| {
            app.click("go_button");
        })
        .expect("runs");
        let stops = script.stops();
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[1].line, 3, "still the same file and line");
        assert_eq!(local(&stops[1], "a"), None, "a different statement now");
    });
}

#[test]
fn breakpoint_limits_and_odd_input_do_not_panic() {
    run_on_large_stack(|| {
        let script = Script::new();
        let session = DebugSession::new(Rc::clone(&script) as Rc<dyn DebugTransport>);
        session.set_breakpoints("a.rhai", &[0, 0]);
        assert!(
            session.breakpoints("a.rhai").is_empty(),
            "line 0 is no line"
        );
        session.set_breakpoints(".\\sub\\a.rhai", &[5, 5, 3]);
        assert_eq!(session.breakpoints("sub/a.rhai"), [3, 5]);
        session.set_breakpoints("sub/a.rhai", &[]);
        assert!(session.breakpoints("sub/a.rhai").is_empty());
        for index in 0..(super::MAX_BREAKPOINT_FILES + 10) {
            session.set_breakpoints(&format!("f{index}.rhai"), &[1]);
        }
        assert!(
            session
                .breakpoints(&format!("f{}.rhai", super::MAX_BREAKPOINT_FILES + 5))
                .is_empty()
        );
        let fresh = DebugSession::new(Rc::clone(&script) as Rc<dyn DebugTransport>);
        let huge: Vec<u32> =
            (1..=u32::try_from(super::MAX_BREAKPOINTS_PER_FILE).unwrap() + 100).collect();
        fresh.set_breakpoints("big.rhai", &huge);
        assert_eq!(
            fresh.breakpoints("big.rhai").len(),
            super::MAX_BREAKPOINTS_PER_FILE
        );
    });
}

#[test]
fn a_runtime_without_a_session_never_touches_a_transport() {
    run_on_large_stack(|| {
        let script = Script::new();
        TestApp::run_sources(
            vec![FormSource::new("main_form", form_doc(), CALLS)],
            Vec::new(),
            |app| {
                app.click("go_button");
            },
        )
        .expect("runs");
        assert_eq!(script.polls.get(), 0);
        assert!(script.events().is_empty());
    });
}

#[test]
fn values_are_rendered_for_display() {
    run_on_large_stack(|| {
        let script = Script::new();
        script.queue(breakpoints("main_form.rhai", &[8]));
        script.on_stop(vec![Command::Continue]);
        debug_run(
            "fn go_button_click() {
    let text = \"hi \\\"there\\\"\";
    let list = [1, 2, 3];
    let map = #{ a: 1, b: \"x\" };
    let nothing = ();
    let big = [];
    for i in 0..20 { big.push(i); }
    let done = true;
    done
}",
            Vec::new(),
            &script,
            |app, _| {
                app.click("go_button");
            },
        );
        let stop = &script.stops()[0];
        assert_eq!(local(stop, "text"), Some("\"hi \\\"there\\\"\""));
        assert_eq!(local(stop, "list"), Some("[1, 2, 3]"));
        assert_eq!(local(stop, "map"), Some("#{a: 1, b: \"x\"}"));
        assert_eq!(local(stop, "nothing"), Some("()"));
        let big = local(stop, "big").expect("big");
        assert!(big.contains("20 items"), "{big}");
        assert!(big.chars().count() <= super::MAX_VALUE_CHARS);
        assert!(
            stop.locals
                .iter()
                .all(|variable| variable.name != "go_button"),
            "controls are not locals"
        );
    });
}
