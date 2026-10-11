#![forbid(unsafe_code)]

//! The player-side half of the debugger (PLAN.md §7, M4).
//!
//! A [`DebugSession`] is installed on a [`FormRuntime`](crate::FormRuntime)
//! with [`set_debugger`](crate::FormRuntime::set_debugger); every form's engine
//! then gets Rhai's debugger callback ([`DebugSession::attach`]). Nothing here
//! runs in a player started without `--debug`: no callback is installed, so the
//! normal path pays nothing.
//!
//! # How a run is watched
//!
//! In debug mode the callback asks Rhai for every node ("step into" status), so
//! it sees each statement and expression. That is what makes line breakpoints,
//! all three step commands, break-on-error with the failing frame's locals, and
//! pause of a runaway loop work with one uniform rule, at the cost of a small
//! constant per script node. The callback tracks the *line instance* it is on
//! so a breakpoint fires once when a line is reached, not once per expression on
//! it.
//!
//! * **Breakpoints** are `(file, line)` pairs held by the session, not by a
//!   compiled script. A hot reload (`--watch`) recompiles the scripts and the
//!   new engines get the callback again, so breakpoints survive a reload
//!   unchanged: they stay on the same file and line, which is what the IDE
//!   expects (it re-sends the set when a file is saved). A line past the end of
//!   the file simply never matches.
//! * **Stepping** compares the call depth at the stop with the depth at each
//!   later line: step over stays at or above it, step out waits for a lower
//!   depth. Stepping past the end of a handler ends the stop and the program is
//!   idle again (there is no "next handler" stop); the same goes for stepping
//!   out of the outermost frame.
//! * **Break on error**: a script error stops at the failing position, in the
//!   frame that raised it, with that frame's locals. The stop reports reason
//!   `exception`; resuming lets the error follow its normal route (the
//!   player shows it in a message box).
//! * **Pause** and every other command that arrives while a script runs are
//!   read by the callback every few hundred nodes, so a runaway `while true {}`
//!   can be interrupted. When no script is running a pause stays armed and takes
//!   effect at the first line of the next handler.
//! * **Locals** are the innermost function's scope, in declaration order, with
//!   the host's control handles left out. Rhai gives every call its own scope
//!   and keeps no record of an outer frame's, so only the innermost frame has
//!   locals and **evaluate** works in frame `0` only; the call stack still
//!   lists every frame with its file and line.
//!
//! # Blocking
//!
//! A stop **blocks the player's UI thread** reading the transport until a
//! resume command, as VB6 froze the program on a break. The window cannot
//! repaint or answer the OS meanwhile, so a window manager may mark it "not
//! responding" until the IDE resumes it; the IDE stays responsive because it is
//! another process. Nothing can be done about that from inside the engine
//! callback.
//!
//! # When the IDE goes away
//!
//! If the transport reports end-of-stream (the IDE closed the pipe or died),
//! the session *detaches*: a stop in progress resumes, and the program then runs
//! with the debugger switched off. It is never left blocked on a dead pipe.
//! (A killed IDE also kills the player it started; this covers every other
//! way the pipe can close.)

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use lazyrad_debug_proto::{
    Command, DecodeError, Event, Frame, Stop, StopReason, Variable, decode_command, normalize_file,
};
use rhai::debugger::{DebuggerCommand, DebuggerEvent};
use rhai::{Dynamic, Engine, EvalAltResult, EvalContext, Position};
use xui_rhai::control::is_host_handle;
use xui_rhai::engine::StopHandle;
use xui_rhai::{EngineHost, ScriptError};

/// How many script nodes pass between looks at the transport while running.
const POLL_EVERY: u32 = 128;
/// The most locals one stop reports.
const MAX_LOCALS: usize = 64;
/// The most stack frames one stop reports.
const MAX_FRAMES: usize = 48;
/// The longest rendered value, in characters.
const MAX_VALUE_CHARS: usize = 120;
/// The longest variable or function name reported, in characters.
const MAX_NAME_CHARS: usize = 48;
/// The most elements or keys of a container shown in a rendered value.
const MAX_PREVIEW_ITEMS: usize = 8;
/// The most lines accepted for one file.
const MAX_BREAKPOINTS_PER_FILE: usize = 4096;
/// The most distinct files that may hold breakpoints.
const MAX_BREAKPOINT_FILES: usize = 256;
/// The name of the frame for code outside any function.
pub const TOP_LEVEL: &str = "<top level>";

/// What a [`DebugTransport`] has for the session right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incoming {
    /// A command from the IDE.
    Command(Command),
    /// A line that was not a command; the text says why. The session reports
    /// it back as an `error` event and carries on.
    Rejected(String),
    /// Nothing is waiting (only from [`DebugTransport::poll`]).
    Empty,
    /// The IDE end is gone: no further command will ever arrive.
    Closed,
}

/// Decodes one line a transport read into an [`Incoming`], or `None` for a
/// blank line (which is simply skipped).
pub fn decode_incoming(line: &str) -> Option<Incoming> {
    match decode_command(line) {
        Ok(command) => Some(Incoming::Command(command)),
        Err(DecodeError::Empty) => None,
        Err(error) => Some(Incoming::Rejected(format!("{error}"))),
    }
}

/// The session's channel to the IDE.
///
/// The player implements it over stdin and stdout; tests script it. It uses
/// `&self` and no threads: the implementation decides how `poll` avoids
/// blocking (a reader thread on the desktop, a non-blocking descriptor on
/// LazyOS).
pub trait DebugTransport {
    /// Writes one event. Best effort: a transport whose peer is gone drops it.
    fn send(&self, event: &Event);
    /// The next command if one is already available, without blocking.
    fn poll(&self) -> Incoming;
    /// Blocks until a command arrives or the peer closes. Never returns
    /// [`Incoming::Empty`]; a transport that cannot block reports
    /// [`Incoming::Closed`] instead.
    fn wait(&self) -> Incoming;
}

/// What the program does after a stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resume {
    /// Run to the next breakpoint, error or pause.
    Run,
    /// Stop at the next line.
    Into,
    /// Stop at the next line at this call depth or shallower.
    Over(usize),
    /// Stop at the next line shallower than this depth.
    Out(usize),
}

/// The line each call level was last seen on. A breakpoint or step fires when a
/// level reaches a *different* line, so the several expressions of one line,
/// and the rest of a line after a call on it returns, are one visit.
#[derive(Debug, Default)]
struct LineTrail {
    /// One entry per call level, outermost first: the file and line.
    levels: Vec<(String, usize)>,
}

impl LineTrail {
    /// Records that call level `depth` (one-based) is on `file:line` and
    /// returns whether that is a new line for the level.
    fn enter(&mut self, file: &str, line: usize, depth: usize) -> bool {
        let level = depth.max(1) - 1;
        if self.levels.len() <= level {
            self.levels.resize(level + 1, (String::new(), 0));
        }
        let entry = &mut self.levels[level];
        if entry.1 == line && entry.0 == file {
            return false;
        }
        entry.0.clear();
        entry.0.push_str(file);
        entry.1 = line;
        true
    }

    /// The line the innermost known level is on, `0` before any.
    fn current_line(&self) -> usize {
        self.levels.last().map_or(0, |entry| entry.1)
    }

    /// Forgets the levels at and below `depth`: that function returned.
    fn leave(&mut self, depth: usize) {
        self.levels.truncate(depth.max(1) - 1);
    }

    fn clear(&mut self) {
        self.levels.clear();
    }
}

/// One remembered local: scalars and shared handles are kept as the value,
/// containers as their length (cloning an array on every line would make a hot
/// loop quadratic).
#[derive(Debug)]
enum HeldValue {
    Value(Dynamic),
    Array(usize),
    Map(usize),
}

/// The innermost function's locals as of the last line change.
///
/// Rhai rewinds a function's scope *before* it reports the function's exit, so
/// the locals an error stop wants are gone by then. The callback therefore
/// notes them once per line visit, reusing its buffers, and an exception stop
/// reports this note instead of the (by then empty) live scope.
#[derive(Debug, Default)]
struct Held {
    depth: usize,
    items: Vec<(String, HeldValue)>,
}

impl Held {
    fn remember(&mut self, scope: &rhai::Scope<'_>, depth: usize) {
        self.depth = depth;
        let mut count = 0;
        for (name, _constant, value) in scope.iter_raw() {
            if name == "app" || (!value.is_shared() && is_host_handle(value)) {
                continue;
            }
            if count >= MAX_LOCALS * 4 {
                break;
            }
            let held = if value.is_array() {
                HeldValue::Array(
                    value
                        .read_lock::<rhai::Array>()
                        .map_or(0, |array| array.len()),
                )
            } else if value.is_map() {
                HeldValue::Map(value.read_lock::<rhai::Map>().map_or(0, |map| map.len()))
            } else {
                HeldValue::Value(value.clone())
            };
            match self.items.get_mut(count) {
                Some(entry) => {
                    entry.0.clear();
                    entry.0.push_str(name);
                    entry.1 = held;
                }
                None => self.items.push((name.to_owned(), held)),
            }
            count += 1;
        }
        self.items.truncate(count);
    }
}

/// Everything the callback mutates. Never borrowed across a transport call, an
/// evaluation or a hook, so re-entrancy cannot find it borrowed.
#[derive(Debug)]
struct State {
    breakpoints: BTreeMap<String, BTreeSet<u32>>,
    resume: Resume,
    pause: bool,
    stopped: bool,
    detached: bool,
    terminating: bool,
    evaluating: bool,
    last: LineTrail,
    /// The locals as of the last line change (see [`Held`]).
    held: Held,
    /// The message and node count of the error already reported. An error
    /// unwinding through several frames evaluates no node on the way out, so it
    /// is the same error while the count has not moved.
    reported_error: Option<(String, u64)>,
    /// Nodes seen so far, across the session.
    nodes: u64,
    ticks: u32,
}

impl State {
    fn new() -> State {
        State {
            breakpoints: BTreeMap::new(),
            resume: Resume::Run,
            pause: false,
            stopped: false,
            detached: false,
            terminating: false,
            evaluating: false,
            last: LineTrail::default(),
            held: Held::default(),
            reported_error: None,
            nodes: 0,
            ticks: 0,
        }
    }
}

/// How a stop ended.
enum Outcome {
    Resume,
    Detached,
    Terminate,
}

/// One debugging session, shared by every form's engine.
pub struct DebugSession {
    transport: Rc<dyn DebugTransport>,
    state: RefCell<State>,
    handles: RefCell<Vec<StopHandle>>,
    on_stop: RefCell<Option<Rc<dyn Fn()>>>,
    stops: Cell<u64>,
}

impl DebugSession {
    /// A session reporting through `transport`, with no breakpoints.
    pub fn new(transport: Rc<dyn DebugTransport>) -> Rc<DebugSession> {
        Rc::new(DebugSession {
            transport,
            state: RefCell::new(State::new()),
            handles: RefCell::new(Vec::new()),
            on_stop: RefCell::new(None),
            stops: Cell::new(0),
        })
    }

    /// Installs the debugger callback on `host`'s engine and remembers its
    /// stop handle. Called for every engine the runtime builds, so a hot
    /// reload's new engines are watched too.
    #[allow(deprecated)] // `register_debugger` is flagged volatile, not deprecated.
    pub fn attach(self: &Rc<Self>, host: &mut EngineHost) {
        self.handles.borrow_mut().push(host.stop_handle());
        let session = Rc::clone(self);
        host.engine_mut().register_debugger(
            |_engine, debugger| debugger,
            move |mut context, event, _node, source, position| {
                session.on_event(&mut context, event, source, position)
            },
        );
    }

    /// Sets what runs when a `stop` command arrives, after the running script
    /// has been asked to end. The player exits the process here.
    pub fn set_stop_hook(&self, hook: Rc<dyn Fn()>) {
        *self.on_stop.borrow_mut() = Some(hook);
    }

    /// Writes `event` through the transport.
    pub fn send(&self, event: &Event) {
        self.transport.send(event);
    }

    /// Whether the program is stopped at a breakpoint, step or error.
    pub fn is_stopped(&self) -> bool {
        self.state.borrow().stopped
    }

    /// Whether the IDE end closed and the debugger switched itself off.
    pub fn is_detached(&self) -> bool {
        self.state.borrow().detached
    }

    /// How many times the program has stopped, for tests and statistics.
    pub fn stop_count(&self) -> u64 {
        self.stops.get()
    }

    /// The breakpoint lines of `file`, ascending.
    pub fn breakpoints(&self, file: &str) -> Vec<u32> {
        self.state
            .borrow()
            .breakpoints
            .get(&normalize_file(file))
            .map(|lines| lines.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Handles the commands already waiting, without blocking. The player
    /// calls it between events so breakpoints set while idle apply at once.
    pub fn pump(&self) {
        self.drain();
    }

    /// Replaces `file`'s breakpoints. Lines are one-based; `0` is ignored.
    fn set_breakpoints(&self, file: &str, lines: &[u32]) {
        let key = normalize_file(file);
        let mut state = self.state.borrow_mut();
        if lines.is_empty() {
            state.breakpoints.remove(&key);
            return;
        }
        if !state.breakpoints.contains_key(&key) && state.breakpoints.len() >= MAX_BREAKPOINT_FILES
        {
            return;
        }
        let set: BTreeSet<u32> = lines
            .iter()
            .copied()
            .filter(|line| *line > 0)
            .take(MAX_BREAKPOINTS_PER_FILE)
            .collect();
        if set.is_empty() {
            state.breakpoints.remove(&key);
        } else {
            state.breakpoints.insert(key, set);
        }
    }

    /// Reads every command that is already waiting and applies it as a running
    /// program should.
    fn drain(&self) {
        loop {
            match self.transport.poll() {
                Incoming::Empty => return,
                Incoming::Closed => {
                    self.detach();
                    return;
                }
                Incoming::Rejected(message) => self.report(&message),
                Incoming::Command(command) => self.running_command(command),
            }
        }
    }

    /// Applies `command` received while the program is running.
    fn running_command(&self, command: Command) {
        match command {
            Command::SetBreakpoints { file, lines } => self.set_breakpoints(&file, &lines),
            Command::Pause => self.state.borrow_mut().pause = true,
            Command::Stop => self.terminate(),
            Command::Continue | Command::StepInto | Command::StepOver | Command::StepOut => {
                self.report("the program is not stopped");
            }
            Command::Evaluate { id, .. } => {
                self.transport.send(&Event::Evaluated {
                    id,
                    ok: false,
                    value: "the program is running; break first".to_owned(),
                    ty: String::new(),
                });
            }
        }
    }

    /// Sends an `error` event.
    fn report(&self, message: &str) {
        self.transport.send(&Event::Error {
            message: message.to_owned(),
        });
    }

    /// Switches the debugger off: no further stops, and a stop in progress
    /// resumes.
    fn detach(&self) {
        let mut state = self.state.borrow_mut();
        state.detached = true;
        state.pause = false;
    }

    /// Ends the program: every attached engine's script is asked to stop and
    /// the host's stop hook runs.
    fn terminate(&self) {
        self.state.borrow_mut().terminating = true;
        for handle in self.handles.borrow().iter() {
            handle.stop();
        }
        // Cloned out so a hook that re-enters the session finds nothing held.
        let hook = self.on_stop.borrow().clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// The error a callback returns to end the running script.
    fn terminated(position: Position) -> Box<EvalAltResult> {
        Box::new(EvalAltResult::ErrorTerminated(
            Dynamic::from("stopped by the debugger"),
            position,
        ))
    }

    /// The engine callback.
    fn on_event(
        &self,
        context: &mut EvalContext<'_, '_, '_, '_, '_, '_>,
        event: DebuggerEvent<'_>,
        source: Option<&str>,
        position: Position,
    ) -> Result<DebuggerCommand, Box<EvalAltResult>> {
        {
            let state = self.state.borrow();
            // An evaluation started by this session re-enters the engine; it
            // runs undisturbed. A detached session watches nothing.
            if state.evaluating || state.detached {
                return Ok(DebuggerCommand::Continue);
            }
            if state.terminating {
                return Err(Self::terminated(position));
            }
        }
        match event {
            DebuggerEvent::Start => {
                {
                    let mut state = self.state.borrow_mut();
                    state.last.clear();
                    state.reported_error = None;
                }
                // Commands that arrived while idle (breakpoints, a pause).
                self.drain();
                if self.state.borrow().terminating {
                    return Err(Self::terminated(position));
                }
                // The first node is the function's own header line: a
                // breakpoint there, a pending pause or a step applies.
                self.on_node(context, source, position)
            }
            DebuggerEvent::End => Ok(DebuggerCommand::Continue),
            DebuggerEvent::FunctionExitWithError(error) => {
                let command = self.on_error(context, error, source);
                self.left_function(context);
                command
            }
            DebuggerEvent::FunctionExitWithValue(_) => {
                self.left_function(context);
                Ok(self.keep_watching())
            }
            DebuggerEvent::Step | DebuggerEvent::BreakPoint(_) => {
                self.on_node(context, source, position)
            }
            // `DebuggerEvent` is non-exhaustive.
            _ => Ok(self.keep_watching()),
        }
    }

    /// A function is returning: its level's line is forgotten, so a later call
    /// of it starts fresh.
    fn left_function(&self, context: &EvalContext<'_, '_, '_, '_, '_, '_>) {
        let depth = context
            .debugger()
            .map_or(0, |debugger| debugger.call_stack().len());
        if depth > 0 {
            self.state.borrow_mut().last.leave(depth);
        }
    }

    /// The command that leaves the engine reporting every node.
    fn keep_watching(&self) -> DebuggerCommand {
        if self.state.borrow().detached {
            DebuggerCommand::Continue
        } else {
            DebuggerCommand::StepInto
        }
    }

    /// A statement or expression is about to run.
    fn on_node(
        &self,
        context: &mut EvalContext<'_, '_, '_, '_, '_, '_>,
        source: Option<&str>,
        position: Position,
    ) -> Result<DebuggerCommand, Box<EvalAltResult>> {
        let Some(line) = position.line() else {
            return Ok(DebuggerCommand::StepInto);
        };
        // Look at the transport now and then so a pause or a stop can reach a
        // script that never returns.
        let due = {
            let mut state = self.state.borrow_mut();
            state.ticks = state.ticks.wrapping_add(1);
            state.ticks % POLL_EVERY == 0
        };
        if due {
            self.drain();
            let state = self.state.borrow();
            if state.terminating {
                return Err(Self::terminated(position));
            }
            if state.detached {
                return Ok(DebuggerCommand::Continue);
            }
        }

        let file = source.unwrap_or("");
        let depth = context
            .debugger()
            .map_or(0, |debugger| debugger.call_stack().len());
        let reason = {
            let mut state = self.state.borrow_mut();
            state.nodes += 1;
            if !state.last.enter(file, line, depth) {
                None
            } else {
                state.held.remember(context.scope(), depth);
                let breakpoint = state
                    .breakpoints
                    .get(file)
                    .is_some_and(|lines| lines.contains(&to_line(line)));
                if breakpoint {
                    Some(StopReason::Breakpoint)
                } else if state.pause {
                    Some(StopReason::Pause)
                } else {
                    let step = match state.resume {
                        Resume::Run => false,
                        Resume::Into => true,
                        Resume::Over(from) => depth <= from,
                        Resume::Out(from) => depth < from,
                    };
                    step.then_some(StopReason::Step)
                }
            }
        };
        match reason {
            Some(reason) => self.stop_at(context, reason, file, line, None, position),
            None => Ok(DebuggerCommand::StepInto),
        }
    }

    /// A function is exiting with `error`: stop at the failing position, once
    /// per error, with the failing frame still on the stack.
    fn on_error(
        &self,
        context: &mut EvalContext<'_, '_, '_, '_, '_, '_>,
        error: &EvalAltResult,
        source: Option<&str>,
    ) -> Result<DebuggerCommand, Box<EvalAltResult>> {
        let file = source.unwrap_or("");
        let message = ScriptError::from_eval(file, error).message;
        let line = {
            let mut state = self.state.borrow_mut();
            let key = (message.clone(), state.nodes);
            if state.reported_error.as_ref() == Some(&key) {
                return Ok(DebuggerCommand::StepInto);
            }
            state.reported_error = Some(key);
            // The error usually has no position of its own yet (Rhai fills it
            // in as it unwinds), but the failing node was the last one seen.
            innermost_position(error)
                .and_then(|position| position.line())
                .unwrap_or_else(|| state.last.current_line())
        };
        self.stop_at(
            context,
            StopReason::Exception,
            file,
            line,
            Some(message),
            Position::NONE,
        )
    }

    /// Reports a stop and blocks until the IDE resumes the program.
    fn stop_at(
        &self,
        context: &mut EvalContext<'_, '_, '_, '_, '_, '_>,
        reason: StopReason,
        file: &str,
        line: usize,
        message: Option<String>,
        position: Position,
    ) -> Result<DebuggerCommand, Box<EvalAltResult>> {
        let depth = context
            .debugger()
            .map_or(0, |debugger| debugger.call_stack().len());
        let payload = {
            let state = self.state.borrow();
            // An error stop reports the remembered locals when they belong to
            // the frame that is exiting.
            let held = (reason == StopReason::Exception && state.held.depth == depth)
                .then_some(&state.held);
            snapshot(context, reason, file, line, message, held)
        };
        {
            let mut state = self.state.borrow_mut();
            state.stopped = true;
            state.pause = false;
        }
        self.stops.set(self.stops.get() + 1);
        self.transport.send(&Event::Stopped(payload));

        let outcome = self.wait_for_resume(context, reason, depth);
        self.state.borrow_mut().stopped = false;
        match outcome {
            Outcome::Resume => {
                self.transport.send(&Event::Resumed);
                Ok(self.keep_watching())
            }
            Outcome::Detached => Ok(DebuggerCommand::Continue),
            Outcome::Terminate => Err(Self::terminated(position)),
        }
    }

    /// The blocking half of a stop: serves commands until one resumes the
    /// program.
    fn wait_for_resume(
        &self,
        context: &mut EvalContext<'_, '_, '_, '_, '_, '_>,
        reason: StopReason,
        depth: usize,
    ) -> Outcome {
        loop {
            match self.transport.wait() {
                // A transport that cannot block ends the session like a
                // closed pipe rather than spinning.
                Incoming::Closed | Incoming::Empty => {
                    self.detach();
                    return Outcome::Detached;
                }
                Incoming::Rejected(message) => self.report(&message),
                Incoming::Command(command) => {
                    if self.state.borrow().detached {
                        return Outcome::Detached;
                    }
                    match command {
                        Command::SetBreakpoints { file, lines } => {
                            self.set_breakpoints(&file, &lines);
                        }
                        Command::Continue => return self.resume_with(Resume::Run),
                        // After an error there is nothing to step: every
                        // resume lets the error carry on.
                        Command::StepInto | Command::StepOver | Command::StepOut
                            if reason == StopReason::Exception =>
                        {
                            return self.resume_with(Resume::Run);
                        }
                        Command::StepInto => return self.resume_with(Resume::Into),
                        Command::StepOver => return self.resume_with(Resume::Over(depth)),
                        Command::StepOut => return self.resume_with(Resume::Out(depth)),
                        // Already stopped; a request to stop is a no-op.
                        Command::Pause => {}
                        Command::Stop => {
                            self.terminate();
                            return Outcome::Terminate;
                        }
                        Command::Evaluate { expr, frame, id } => {
                            let answer = self.evaluate(context, &expr, frame, id);
                            self.transport.send(&answer);
                        }
                    }
                }
            }
        }
    }

    fn resume_with(&self, resume: Resume) -> Outcome {
        self.state.borrow_mut().resume = resume;
        Outcome::Resume
    }

    /// Evaluates `expr` in the stopped frame's scope.
    ///
    /// The script may use the scope's variables, the form's controls and the
    /// script's own functions, and may assign (`count = 0`); a `let` it
    /// declares lives until the frame exits.
    fn evaluate(
        &self,
        context: &mut EvalContext<'_, '_, '_, '_, '_, '_>,
        expr: &str,
        frame: u32,
        id: u64,
    ) -> Event {
        let failed = |message: String| Event::Evaluated {
            id,
            ok: false,
            value: message,
            ty: String::new(),
        };
        if frame != 0 {
            return failed("only the innermost frame can be evaluated".to_owned());
        }
        // The nested engine call reaches the callback again; the flag lets it
        // through untouched. No borrow is held while it runs.
        self.state.borrow_mut().evaluating = true;
        let result = run_expression(context, expr);
        self.state.borrow_mut().evaluating = false;
        match result {
            Ok(value) => {
                let (ty, text) = describe(context.engine(), &value);
                Event::Evaluated {
                    id,
                    ok: true,
                    value: text,
                    ty,
                }
            }
            Err(message) => failed(message),
        }
    }
}

/// Compiles and runs `expr` against the context's scope, with the script's
/// functions visible.
fn run_expression(
    context: &mut EvalContext<'_, '_, '_, '_, '_, '_>,
    expr: &str,
) -> Result<Dynamic, String> {
    let engine = context.engine();
    let mut ast = engine
        .compile_with_scope(context.scope(), expr)
        .map_err(|error| error.to_string())?;
    // The function definitions in play, so `helper(2)` resolves.
    for module in context.namespaces() {
        ast.combine(rhai::AST::new(
            std::iter::empty::<rhai::Stmt>(),
            module.clone(),
        ));
    }
    engine
        .eval_ast_with_scope::<Dynamic>(context.scope_mut(), &ast)
        .map_err(|error| ScriptError::from_eval("", &error).message)
}

/// A local as the report will show it.
enum Shown<'a> {
    Live(&'a Dynamic),
    Array(usize),
    Map(usize),
}

/// The report for a stop: location, call stack and locals. `held` replaces the
/// live scope with the remembered locals (an error stop, see [`Held`]).
fn snapshot(
    context: &EvalContext<'_, '_, '_, '_, '_, '_>,
    reason: StopReason,
    file: &str,
    line: usize,
    message: Option<String>,
    held: Option<&Held>,
) -> Stop {
    let engine = context.engine();
    let file = normalize_file(file);
    let mut truncated = false;

    let mut stack = Vec::new();
    match context.debugger() {
        Some(debugger) if !debugger.call_stack().is_empty() => {
            let frames = debugger.call_stack();
            let count = frames.len();
            // Innermost: the function at the stop position.
            stack.push(Frame {
                function: clip(frames[count - 1].fn_name.as_str(), MAX_NAME_CHARS),
                file: file.clone(),
                line: to_line(line),
            });
            // Each caller is stopped where it called the frame above it: that
            // call's position, in the file the caller is defined in.
            for index in (0..count - 1).rev() {
                stack.push(Frame {
                    function: clip(frames[index].fn_name.as_str(), MAX_NAME_CHARS),
                    file: frames[index]
                        .source
                        .as_deref()
                        .map(normalize_file)
                        .unwrap_or_default(),
                    line: frames[index + 1].pos.line().map_or(0, to_line),
                });
                if stack.len() >= MAX_FRAMES {
                    truncated = index > 0;
                    break;
                }
            }
        }
        _ => stack.push(Frame {
            function: TOP_LEVEL.to_owned(),
            file: file.clone(),
            line: to_line(line),
        }),
    }

    // A later declaration shadows an earlier one of the same name.
    let mut order: Vec<&str> = Vec::new();
    let mut by_name: BTreeMap<&str, Shown<'_>> = BTreeMap::new();
    match held {
        Some(held) => {
            for (name, value) in &held.items {
                if !by_name.contains_key(name.as_str()) {
                    order.push(name);
                }
                by_name.insert(
                    name,
                    match value {
                        HeldValue::Value(value) => Shown::Live(value),
                        HeldValue::Array(len) => Shown::Array(*len),
                        HeldValue::Map(len) => Shown::Map(*len),
                    },
                );
            }
        }
        None => {
            for (name, _constant, value) in context.scope().iter_raw().take(MAX_LOCALS * 8) {
                if !by_name.contains_key(name) {
                    order.push(name);
                }
                by_name.insert(name, Shown::Live(value));
            }
        }
    }
    let mut locals = Vec::new();
    for name in order {
        let Some(shown) = by_name.get(name) else {
            continue;
        };
        let (ty, text) = match shown {
            Shown::Live(value) => {
                let value = if value.is_shared() {
                    value.flatten_clone()
                } else {
                    (*value).clone()
                };
                if is_host_handle(&value) || name == "app" {
                    continue;
                }
                describe(engine, &value)
            }
            Shown::Array(len) => ("array".to_owned(), format!("[…{len} items]")),
            Shown::Map(len) => ("map".to_owned(), format!("#{{…{len} keys}}")),
        };
        if locals.len() >= MAX_LOCALS {
            truncated = true;
            break;
        }
        locals.push(Variable {
            name: clip(name, MAX_NAME_CHARS),
            ty,
            value: text,
        });
    }

    Stop {
        reason,
        file,
        line: to_line(line),
        message,
        stack,
        locals,
        truncated,
    }
}

/// The position of the error at the bottom of Rhai's call wrappers.
fn innermost_position(error: &EvalAltResult) -> Option<Position> {
    let mut current = error;
    while let EvalAltResult::ErrorInFunctionCall(_, _, inner, _) = current {
        current = inner;
    }
    let position = current.position();
    (!position.is_none()).then_some(position)
}

/// A one-based line as the protocol's `u32`.
fn to_line(line: usize) -> u32 {
    u32::try_from(line).unwrap_or(u32::MAX)
}

/// `text` cut to `max` characters, with an ellipsis when it was cut.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

/// A value's type name and a short rendering of it.
fn describe(engine: &Engine, value: &Dynamic) -> (String, String) {
    let ty = engine.map_type_name(value.type_name()).to_owned();
    (ty, clip(&render(value, 0), MAX_VALUE_CHARS))
}

/// A value as text: strings quoted, containers summarised.
fn render(value: &Dynamic, depth: usize) -> String {
    if value.is_unit() {
        return "()".to_owned();
    }
    if value.is_string() {
        let shown = value.clone().flatten().into_immutable_string();
        return match shown {
            Ok(text) => format!("{:?}", text.as_str()),
            Err(_) => value.to_string(),
        };
    }
    if let Some(array) = value.read_lock::<rhai::Array>() {
        if depth > 0 {
            return format!("[…{}]", array.len());
        }
        let mut parts: Vec<String> = array
            .iter()
            .take(MAX_PREVIEW_ITEMS)
            .map(|item| render(item, depth + 1))
            .collect();
        if array.len() > MAX_PREVIEW_ITEMS {
            parts.push(format!("… {} items", array.len()));
        }
        return format!("[{}]", parts.join(", "));
    }
    if let Some(map) = value.read_lock::<rhai::Map>() {
        if depth > 0 {
            return format!("#{{…{}}}", map.len());
        }
        let mut parts: Vec<String> = map
            .iter()
            .take(MAX_PREVIEW_ITEMS)
            .map(|(key, item)| format!("{key}: {}", render(item, depth + 1)))
            .collect();
        if map.len() > MAX_PREVIEW_ITEMS {
            parts.push(format!("… {} keys", map.len()));
        }
        return format!("#{{{}}}", parts.join(", "));
    }
    value.to_string()
}

impl std::fmt::Debug for DebugSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DebugSession")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
