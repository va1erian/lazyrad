#![forbid(unsafe_code)]

//! `lazyrad-player --debug` end to end on an offscreen backend: the command
//! line, the debugger installed on the runtime, and the events a session
//! writes. The IDE's end of the pipe is a scripted transport.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;

use lazyrad_debug_proto::{Command, Event, StopReason};
use lazyrad_player::debug::install_transport;
use lazyrad_player::{EXIT_OK, run_with_backend};
use lazyrad_runtime::{DebugTransport, Incoming};
use xui_canvas::OffscreenBackend;
use xui_core::backend::Backend;

/// What the "IDE" says, and what it hears.
#[derive(Default)]
struct Ide {
    heard: RefCell<Vec<Event>>,
    /// Answers to the first stop, then to the next, and so on.
    on_stop: RefCell<VecDeque<Vec<Incoming>>>,
    waiting: RefCell<VecDeque<Incoming>>,
    /// Delivered by `poll` (before the program runs).
    at_start: RefCell<VecDeque<Incoming>>,
}

impl DebugTransport for Ide {
    fn send(&self, event: &Event) {
        self.heard.borrow_mut().push(event.clone());
        if matches!(event, Event::Stopped(_))
            && let Some(batch) = self.on_stop.borrow_mut().pop_front()
        {
            self.waiting.borrow_mut().extend(batch);
        }
    }

    fn poll(&self) -> Incoming {
        self.at_start
            .borrow_mut()
            .pop_front()
            .unwrap_or(Incoming::Empty)
    }

    fn wait(&self) -> Incoming {
        self.waiting
            .borrow_mut()
            .pop_front()
            .unwrap_or(Incoming::Closed)
    }
}

/// A one-form project whose `form_load` has a loop worth stopping in.
fn project(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lazyrad-player-debug-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    std::fs::write(
        dir.join("dbg.lrp"),
        "name = \"dbg\"\nversion = \"0.1.0\"\nstartup = \"main_form\"\n\n[[items]]\nkind = \"form\"\nname = \"main_form\"\nlayout = \"main_form.lfm\"\ncode = \"main_form.rhai\"\n",
    )
    .expect("project file");
    std::fs::write(
        dir.join("main_form.lfm"),
        "format = 1\n\n[window]\nname = \"main_form\"\ntitle = \"Dbg\"\n",
    )
    .expect("form file");
    std::fs::write(
        dir.join("main_form.rhai"),
        "fn form_load() {\n    let total = 0;\n    for i in 1..4 {\n        total += i;\n        let step = total * 2;\n    }\n    debug(`total=${total}`);\n}\n",
    )
    .expect("code file");
    dir
}

fn run(dir: &std::path::Path, ide: &Rc<Ide>) -> (i32, bool) {
    let transport = Rc::clone(ide);
    install_transport(move || transport);
    let mut watching = false;
    let code = run_with_backend(
        &["--debug".to_owned(), dir.display().to_string()],
        &mut |runtime| {
            let runtime = runtime.expect("a project run hands over its runtime");
            watching = runtime.watch_enabled();
            assert!(runtime.debugger().is_some(), "--debug installs the session");
            Ok(Rc::new(OffscreenBackend::new()) as Rc<dyn Backend>)
        },
    );
    (code, watching)
}

#[test]
fn a_breakpoint_in_form_load_stops_the_player_and_continue_finishes_it() {
    let dir = project("bp");
    let ide = Rc::new(Ide::default());
    ide.at_start
        .borrow_mut()
        .push_back(Incoming::Command(Command::SetBreakpoints {
            file: "main_form.rhai".to_owned(),
            lines: vec![7],
        }));
    ide.on_stop
        .borrow_mut()
        .push_back(vec![Incoming::Command(Command::Continue)]);

    let (code, watching) = run(&dir, &ide);

    assert_eq!(code, EXIT_OK);
    assert!(watching, "--debug keeps hot reload on");
    let heard = ide.heard.borrow();
    assert!(matches!(heard.first(), Some(Event::Ready { player }) if player.starts_with("lazyrad-player")));
    let stops: Vec<_> = heard
        .iter()
        .filter_map(|event| match event {
            Event::Stopped(stop) => Some(stop),
            _ => None,
        })
        .collect();
    assert_eq!(stops.len(), 1, "{heard:?}");
    assert_eq!(stops[0].reason, StopReason::Breakpoint);
    assert_eq!(stops[0].file, "main_form.rhai");
    assert_eq!(stops[0].line, 7);
    assert_eq!(stops[0].stack[0].function, "form_load");
    assert!(
        stops[0]
            .locals
            .iter()
            .any(|variable| variable.name == "total" && variable.value == "6"),
        "{:?}",
        stops[0].locals
    );
    assert!(matches!(heard.last(), Some(Event::Exited { code: 0 })), "{heard:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_debugger_that_never_answers_does_not_hang_the_player() {
    // The pipe is closed from the start: the first breakpoint detaches the
    // session and the program runs to the end.
    let dir = project("eof");
    let ide = Rc::new(Ide::default());
    ide.at_start
        .borrow_mut()
        .push_back(Incoming::Command(Command::SetBreakpoints {
            file: "main_form.rhai".to_owned(),
            lines: vec![4, 5],
        }));
    let (code, _) = run(&dir, &ide);
    assert_eq!(code, EXIT_OK);
    let stops = ide
        .heard
        .borrow()
        .iter()
        .filter(|event| matches!(event, Event::Stopped(_)))
        .count();
    assert_eq!(stops, 1, "one stop, then the session detached");
    let _ = std::fs::remove_dir_all(&dir);
}
