#![forbid(unsafe_code)]

//! Running the open project from the IDE (issue #16).
//!
//! *Run → Start* (F5) launches the separate `lazyrad-player` process on the
//! project directory, so the player owns the user program's event loop and a
//! crash or a stuck loop never freezes the IDE (PLAN.md §1, §4). The IDE reads
//! the child's **stdout** as the program's `debug(...)` output and parses its
//! **stderr** JSON diagnostics into the Error List. A runtime diagnostic also
//! opens the failing file and places the caret on the line. *Run → End* kills
//! the child.
//!
//! # Save before run
//!
//! Start saves every dirty document through the existing save path before it
//! spawns anything: the player reads the project from disk, so the code that
//! runs is what the user sees. A save that fails cancels the run and leaves the
//! previous state intact (checklist 3), and a failed compile check never spawns
//! a process at all.
//!
//! # One run at a time
//!
//! [`RunState`] owns at most one child. Every message a child delivers is
//! stamped with its [`RunId`], so output, diagnostics and the exit notice of a
//! run that has already ended (or of a previous run after End and Start again)
//! are dropped rather than applied to the new run (checklist 1). The process
//! launch sits behind the [`Launcher`] trait so the state machine is driven
//! headlessly by a fake child in tests.
//!
//! # Locating the player
//!
//! [`resolve_player`] prefers the `player_path` setting (a development
//! override) and otherwise looks for `lazyrad-player[.exe]` next to the IDE
//! executable.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use lazyrad_debug_proto::{DecodeError, Event, decode_event};
use lazyrad_player::Report;
use lazyrad_runtime::check_project;

use lazyrad_runtime::platform;

/// How long the exit watcher sleeps between polls of the child.
const POLL_INTERVAL: Duration = Duration::from_millis(30);

/// Identifies one child process. A message that names an older id is stale.
pub type RunId = u64;

/// What a child process reports back to the IDE.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunEvent {
    /// A line the program printed on stdout (`debug(...)`).
    Output(String),
    /// A JSON diagnostic the player wrote on stderr.
    Diagnostic(Report),
    /// A debug-protocol event the player wrote on stdout (debug runs only).
    Debug(Event),
    /// The process ended, with its exit code when the platform reports one.
    Exited(Option<i32>),
}

/// Where a launcher delivers its child's events. The real launcher wraps the
/// window's [`Proxy`](xui_core::app::Proxy); tests capture into a vector.
pub type EventSink = Arc<dyn Fn(RunId, RunEvent) + Send + Sync + 'static>;

/// How the player is started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchMode {
    /// `--watch`: run the program with hot reload. The player's stdin is not
    /// used.
    Run,
    /// `--debug`: run it under the debugger. The player's stdin carries
    /// commands ([`ChildProcess::send_line`]) and its stdout carries protocol
    /// events ([`RunEvent::Debug`]).
    Debug,
}

/// Why a line could not be handed to the child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    /// This launcher's children cannot receive input.
    Unsupported,
    /// There is no running child.
    NotRunning,
    /// The child is gone or closed its input.
    Closed,
    /// The child is alive but not reading: its input queue is full, so the
    /// line was not queued. A polling launcher can say this; a threaded one
    /// never does.
    Full,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            SendError::Unsupported => "this platform's launcher cannot send input to the program",
            SendError::NotRunning => "no program is running",
            SendError::Closed => "the program has closed its input",
            SendError::Full => "the program is not reading its input",
        })
    }
}

impl std::error::Error for SendError {}

/// A child process the run state can end and observe.
pub trait ChildProcess {
    /// Asks the process to terminate. Idempotent.
    fn kill(&mut self);
    /// Whether the process is still alive.
    fn is_running(&mut self) -> bool;
    /// Gives a launcher without worker threads its turn on the window's timer:
    /// drain the pipes, reap an exited child and report through the sink. The
    /// threaded [`PlayerLauncher`] needs nothing here. LazyOS cannot share a
    /// pipe descriptor with another thread, so its launcher polls instead.
    fn poll(&mut self) {}
    /// Queues one line (the newline is added) for the child's stdin and
    /// returns at once, without waiting for the child to read it.
    ///
    /// A launcher that cannot send input keeps this default, so a launcher
    /// written before debugging existed still compiles; it also cannot start a
    /// [`LaunchMode::Debug`] child. A partial write is the implementation's to
    /// queue; `Err` means the line was not and will not be sent.
    fn send_line(&mut self, _line: &str) -> Result<(), SendError> {
        Err(SendError::Unsupported)
    }
}

/// Spawns the player and streams its output back through an [`EventSink`].
pub trait Launcher {
    /// Launches `player` on `project_dir` in [`LaunchMode::Run`], stamping every
    /// event with `run`.
    fn launch(
        &self,
        player: &Path,
        project_dir: &Path,
        run: RunId,
        sink: EventSink,
    ) -> Result<Box<dyn ChildProcess>, LaunchError>;

    /// Launches `player` in `mode`.
    ///
    /// The default serves [`LaunchMode::Run`] through [`Launcher::launch`] and
    /// refuses [`LaunchMode::Debug`], so a launcher that does not know about
    /// debugging fails loudly instead of running without a debugger.
    fn launch_with(
        &self,
        player: &Path,
        project_dir: &Path,
        run: RunId,
        sink: EventSink,
        mode: LaunchMode,
    ) -> Result<Box<dyn ChildProcess>, LaunchError> {
        match mode {
            LaunchMode::Run => self.launch(player, project_dir, run, sink),
            LaunchMode::Debug => Err(LaunchError::new(
                player,
                std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "this platform's launcher cannot run a program under the debugger",
                ),
            )),
        }
    }
}

/// Why a child could not be started.
#[derive(Debug, thiserror::Error)]
#[error("cannot start `{player}`: {source}")]
pub struct LaunchError {
    /// The player path that failed to start.
    pub player: PathBuf,
    /// The underlying operating-system error.
    pub source: std::io::Error,
}

impl LaunchError {
    /// A launch error for `player` caused by `source`.
    pub fn new(player: impl Into<PathBuf>, source: std::io::Error) -> LaunchError {
        LaunchError {
            player: player.into(),
            source,
        }
    }
}

/// The real launcher: [`std::process::Command`] plus two reader threads and an
/// exit watcher.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlayerLauncher;

/// The command that launches `player` on `project_dir`.
///
/// [`LaunchMode::Run`] passes `--watch`, so a save reloads the running program
/// instead of needing a restart (issue #91). [`LaunchMode::Debug`] passes
/// `--debug` (which watches too) and pipes stdin for the debug commands.
fn player_command(player: &Path, project_dir: &Path, mode: LaunchMode) -> Command {
    let mut command = Command::new(player);
    command
        .arg(match mode {
            LaunchMode::Run => "--watch",
            LaunchMode::Debug => "--debug",
        })
        .arg(project_dir)
        .stdin(match mode {
            LaunchMode::Run => Stdio::null(),
            LaunchMode::Debug => Stdio::piped(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The IDE is a GUI-subsystem app, so Windows would give the console-subsystem
    // player a console window of its own; its output is piped back to the IDE
    // instead, so don't create one.
    platform::current().prepare_player_command(&mut command);
    command
}

impl Launcher for PlayerLauncher {
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
        project_dir: &Path,
        run: RunId,
        sink: EventSink,
        mode: LaunchMode,
    ) -> Result<Box<dyn ChildProcess>, LaunchError> {
        let mut child = player_command(player, project_dir, mode)
            .spawn()
            .map_err(|source| LaunchError::new(player, source))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdin = child.stdin.take();
        let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LaunchError::new(
                player,
                std::io::Error::other("the player's output pipes are unavailable"),
            ));
        };
        if mode == LaunchMode::Debug && stdin.is_none() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LaunchError::new(
                player,
                std::io::Error::other("the player's input pipe is unavailable"),
            ));
        }
        let input = stdin.map(StdinWriter::start);

        let child = Arc::new(Mutex::new(child));
        let running = Arc::new(AtomicBool::new(true));

        let stdout_sink = Arc::clone(&sink);
        let debugging = mode == LaunchMode::Debug;
        let stdout_reader = spawn_thread("lazyrad-player-stdout", move || {
            read_stdout(stdout, run, stdout_sink, debugging);
        });
        let stderr_sink = Arc::clone(&sink);
        let stderr_reader = spawn_thread("lazyrad-player-stderr", move || {
            read_stderr(stderr, run, stderr_sink);
        });
        let (stdout_reader, stderr_reader) = match (stdout_reader, stderr_reader) {
            (Ok(stdout_reader), Ok(stderr_reader)) => (stdout_reader, stderr_reader),
            (first, second) => {
                let _ = kill(&child);
                let error = first.err().or(second.err()).unwrap_or_else(|| {
                    std::io::Error::other("the player's reader threads could not start")
                });
                return Err(LaunchError::new(player, error));
            }
        };

        let watcher = spawn_thread("lazyrad-player-exit", {
            let child = Arc::clone(&child);
            let running = Arc::clone(&running);
            let sink = Arc::clone(&sink);
            move || {
                watch_exit(
                    child,
                    running,
                    run,
                    sink,
                    vec![stdout_reader, stderr_reader],
                );
            }
        });
        if let Err(error) = watcher {
            let _ = kill(&child);
            return Err(LaunchError::new(player, error));
        }

        Ok(Box::new(SpawnedChild {
            child,
            running,
            input,
        }))
    }
}

/// The child a [`PlayerLauncher`] returns. Dropping it kills the process, so a
/// run that is never explicitly ended cannot outlive the IDE.
struct SpawnedChild {
    child: Arc<Mutex<Child>>,
    running: Arc<AtomicBool>,
    /// The debug run's stdin; `None` for a plain run.
    input: Option<StdinWriter>,
}

impl ChildProcess for SpawnedChild {
    fn kill(&mut self) {
        let _ = kill(&self.child);
    }

    fn is_running(&mut self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn send_line(&mut self, line: &str) -> Result<(), SendError> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(SendError::Closed);
        }
        self.input
            .as_ref()
            .ok_or(SendError::Unsupported)?
            .send(line)
    }
}

/// Writes lines to the child's stdin from a thread, so the window never waits
/// on a full pipe. Dropping it closes the pipe, which tells a debugging player
/// that its IDE is gone.
struct StdinWriter {
    lines: Sender<String>,
    broken: Arc<AtomicBool>,
}

impl StdinWriter {
    fn start(mut stdin: ChildStdin) -> StdinWriter {
        let (lines, queue) = mpsc::channel::<String>();
        let broken = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&broken);
        let spawned = spawn_thread("lazyrad-player-stdin", move || {
            for line in queue {
                let written = stdin
                    .write_all(line.as_bytes())
                    .and_then(|()| stdin.write_all(b"\n"))
                    .and_then(|()| stdin.flush());
                if written.is_err() {
                    // The child closed its end (it exited or died).
                    flag.store(true, Ordering::SeqCst);
                    return;
                }
            }
        });
        if spawned.is_err() {
            broken.store(true, Ordering::SeqCst);
        }
        StdinWriter { lines, broken }
    }

    fn send(&self, line: &str) -> Result<(), SendError> {
        if self.broken.load(Ordering::SeqCst) {
            return Err(SendError::Closed);
        }
        self.lines.send(line.to_owned()).map_err(|_| {
            self.broken.store(true, Ordering::SeqCst);
            SendError::Closed
        })
    }
}

impl Drop for SpawnedChild {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Kills the child behind `child` and reaps it, ignoring a lock poisoned by a
/// panicking reader.
fn kill(child: &Arc<Mutex<Child>>) -> std::io::Result<()> {
    let mut guard = child.lock().unwrap_or_else(|poison| poison.into_inner());
    guard.kill()?;
    // Reap the process now that it is dead, so End leaves no zombie behind.
    // The exit watcher's next `try_wait` reports the process gone.
    guard.wait().map(|_| ())
}

/// Reads `pipe` line by line, reporting each through [`stdout_event`].
fn read_stdout(pipe: ChildStdout, run: RunId, sink: EventSink, debugging: bool) {
    for line in BufReader::new(pipe).lines().map_while(Result::ok) {
        sink(run, stdout_event(line, debugging));
    }
}

/// How a line the player wrote on stdout is reported.
///
/// In a debug run a line that is a protocol event becomes
/// [`RunEvent::Debug`]; a line that looks like one but speaks another protocol
/// version is reported as a notice, so skew is visible; everything else is the
/// program's own output.
pub fn stdout_event(line: String, debugging: bool) -> RunEvent {
    if !debugging || !line.starts_with('{') {
        return RunEvent::Output(line);
    }
    match decode_event(&line) {
        Ok(event) => RunEvent::Debug(event),
        Err(DecodeError::Version { found: Some(found) }) => RunEvent::Output(format!(
            "[debugger] the player speaks debug protocol {found}, this IDE speaks {}",
            lazyrad_debug_proto::PROTOCOL_VERSION
        )),
        // No version at all is a program printing its own JSON.
        Err(_) => RunEvent::Output(line),
    }
}

/// Reads `pipe` line by line, reporting a JSON diagnostic as
/// [`RunEvent::Diagnostic`] and anything else as output.
fn read_stderr(pipe: ChildStderr, run: RunId, sink: EventSink) {
    for line in BufReader::new(pipe).lines().map_while(Result::ok) {
        sink(run, stderr_event(line));
    }
}

/// How a line the player wrote on stderr is reported: a JSON diagnostic as
/// [`RunEvent::Diagnostic`], anything else as plain output.
pub fn stderr_event(line: String) -> RunEvent {
    match Report::from_json(&line) {
        Some(report) => RunEvent::Diagnostic(report),
        None => RunEvent::Output(line),
    }
}

/// Splits a byte stream read in arbitrary chunks into lines, for launchers that
/// poll a non-blocking pipe instead of blocking a reader thread.
///
/// A partial line is held until its newline arrives (or [`LineBuffer::finish`]).
/// Invalid UTF-8 is replaced, never an error, and a line longer than
/// [`LineBuffer::MAX_LINE`] is cut there, so a runaway program cannot make the
/// IDE buffer without bound.
#[derive(Debug, Default)]
pub struct LineBuffer {
    pending: Vec<u8>,
}

impl LineBuffer {
    /// The longest line kept, in bytes; the rest of a longer line starts a new one.
    pub const MAX_LINE: usize = 64 * 1024;

    /// An empty buffer.
    pub fn new() -> LineBuffer {
        LineBuffer::default()
    }

    /// Adds `chunk` and returns every line it completed (without the newline,
    /// and without a trailing carriage return).
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in chunk {
            if byte == b'\n' {
                lines.push(self.take_line());
            } else {
                self.pending.push(byte);
                if self.pending.len() >= Self::MAX_LINE {
                    lines.push(self.take_line());
                }
            }
        }
        lines
    }

    /// The held partial line at end of stream, if any.
    pub fn finish(&mut self) -> Option<String> {
        (!self.pending.is_empty()).then(|| self.take_line())
    }

    fn take_line(&mut self) -> String {
        let mut bytes = std::mem::take(&mut self.pending);
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// Waits for the child to exit, joins its readers so every line is delivered
/// first, and then reports [`RunEvent::Exited`]. This is also what reaps the
/// process after [`ChildProcess::kill`].
fn watch_exit(
    child: Arc<Mutex<Child>>,
    running: Arc<AtomicBool>,
    run: RunId,
    sink: EventSink,
    readers: Vec<JoinHandle<()>>,
) {
    loop {
        let status = {
            let mut guard = child.lock().unwrap_or_else(|poison| poison.into_inner());
            guard.try_wait()
        };
        match status {
            Ok(Some(status)) => {
                for reader in readers {
                    let _ = reader.join();
                }
                running.store(false, Ordering::SeqCst);
                sink(run, RunEvent::Exited(status.code()));
                break;
            }
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(_) => {
                running.store(false, Ordering::SeqCst);
                sink(run, RunEvent::Exited(None));
                break;
            }
        }
    }
}

/// Spawns a named background thread.
fn spawn_thread(
    name: &str,
    body: impl FnOnce() + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new().name(name.to_owned()).spawn(body)
}

/// Owns the one running child, if any, and hands out fresh run ids.
#[derive(Debug, Default)]
pub struct RunState {
    next: RunId,
    active: Option<Active>,
}

/// The currently running child and the id its messages carry.
struct Active {
    id: RunId,
    child: Box<dyn ChildProcess>,
}

impl std::fmt::Debug for Active {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Active")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl RunState {
    /// An idle run state.
    pub fn new() -> RunState {
        RunState::default()
    }

    /// Whether a program is running.
    pub fn is_running(&self) -> bool {
        self.active.is_some()
    }

    /// The active run's id.
    pub fn run_id(&self) -> Option<RunId> {
        self.active.as_ref().map(|active| active.id)
    }

    /// Launches the player for `project_dir`. Only after the child exists does
    /// the state become running, so a failed launch leaves the old state intact
    /// (checklist 3).
    pub fn start(
        &mut self,
        launcher: &dyn Launcher,
        player: &Path,
        project_dir: &Path,
        sink: EventSink,
    ) -> Result<RunId, LaunchError> {
        self.start_with(launcher, player, project_dir, sink, LaunchMode::Run)
    }

    /// [`RunState::start`] in an explicit [`LaunchMode`].
    pub fn start_with(
        &mut self,
        launcher: &dyn Launcher,
        player: &Path,
        project_dir: &Path,
        sink: EventSink,
        mode: LaunchMode,
    ) -> Result<RunId, LaunchError> {
        let id = self.next;
        let child = launcher.launch_with(player, project_dir, id, sink, mode)?;
        self.next += 1;
        self.active = Some(Active { id, child });
        Ok(id)
    }

    /// Sends one line to the running child's stdin without blocking.
    pub fn send_line(&mut self, line: &str) -> Result<(), SendError> {
        match self.active.as_mut() {
            Some(active) => active.child.send_line(line),
            None => Err(SendError::NotRunning),
        }
    }

    /// Lets the running child do its polling work (see [`ChildProcess::poll`]).
    pub fn poll(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.child.poll();
        }
    }

    /// Kills the running child, if any, and returns to idle.
    pub fn end(&mut self) {
        if let Some(mut active) = self.active.take() {
            active.child.kill();
        }
    }

    /// Whether `run` is the active run; a message naming any other id is stale.
    pub fn accepts(&self, run: RunId) -> bool {
        self.run_id() == Some(run)
    }

    /// Records that the active run exited on its own. Returns `false` (and
    /// changes nothing) for a stale id, so a late exit notice cannot clear the
    /// run that replaced it (checklist 1).
    pub fn finished(&mut self, run: RunId) -> bool {
        if self.accepts(run) {
            self.active = None;
            true
        } else {
            false
        }
    }
}

impl Drop for RunState {
    fn drop(&mut self) {
        self.end();
    }
}

/// One problem the compile check found, in plain data for the Error List.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunDiagnostic {
    /// The source file the problem is in.
    pub file: String,
    /// The one-based line, or `0` when unknown.
    pub line: usize,
    /// The one-based column, or `0` when unknown.
    pub col: usize,
    /// The human-readable description.
    pub message: String,
    /// Whether this is a lint warning rather than an error: a warning is shown
    /// but does not stop the project from running.
    pub warning: bool,
}

/// Compile-checks the project on disk.
///
/// `Ok(empty)` means the project is ready to run; `Ok(problems)` lists every
/// problem and the caller must not start. An `Err` is a project that could not
/// be loaded at all.
pub fn check(project_dir: &Path) -> Result<Vec<RunDiagnostic>, String> {
    let report = check_project(project_dir).map_err(|error| error.to_string())?;
    let mut problems: Vec<RunDiagnostic> = report
        .diagnostics
        .iter()
        .map(|diagnostic| RunDiagnostic {
            file: file_name(&diagnostic.file),
            line: diagnostic.line.unwrap_or(0),
            col: 0,
            message: diagnostic.message.clone(),
            warning: false,
        })
        .collect();
    problems.extend(report.scripts.iter().map(|error| RunDiagnostic {
        file: error.file.clone(),
        line: error.line,
        col: error.column,
        message: error.message.clone(),
        warning: false,
    }));
    problems.extend(report.lints.iter().map(|lint| RunDiagnostic {
        file: lint.file.clone(),
        line: lint.line,
        col: lint.column,
        message: lint.message.clone(),
        warning: true,
    }));
    Ok(problems)
}

/// The last path component of `path`, for the Error List.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Why the player executable could not be found.
#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    /// The `player_path` setting names a file that does not exist.
    #[error("the configured player `{0}` does not exist")]
    OverrideMissing(PathBuf),
    /// The platform has no place to look for the player (the IDE's own
    /// executable location is unknown or has no folder).
    #[error("the IDE executable has no folder, so the player cannot be located")]
    NoExeDir,
    /// No player sits next to the IDE, and none was configured.
    #[error("the player `{0}` was not found next to the IDE; set `player_path` in settings")]
    NotFound(PathBuf),
}

/// The player binary to launch: `override_path` when set, otherwise
/// `lazyrad-player[.exe]` next to the running IDE executable.
pub fn resolve_player(override_path: Option<&Path>) -> Result<PathBuf, PlayerError> {
    if let Some(path) = override_path {
        return if path.is_file() {
            Ok(path.to_path_buf())
        } else {
            Err(PlayerError::OverrideMissing(path.to_path_buf()))
        };
    }
    // The platform knows where its player lives: next to the IDE on the
    // desktop, a fixed path on LazyOS.
    let candidate = platform::current()
        .player_executable()
        .ok_or(PlayerError::NoExeDir)?;
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(PlayerError::NotFound(candidate))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use super::*;

    #[test]
    fn a_line_buffer_joins_chunks_and_splits_lines() {
        let mut buffer = LineBuffer::new();
        assert!(buffer.push(b"hel").is_empty());
        assert_eq!(buffer.push(b"lo\nwor"), ["hello"]);
        assert_eq!(buffer.push(b"ld\r\n\nx"), ["world", ""]);
        assert_eq!(buffer.finish().as_deref(), Some("x"));
        assert_eq!(buffer.finish(), None);
    }

    #[test]
    fn a_line_buffer_survives_hostile_input() {
        let mut buffer = LineBuffer::new();
        let huge = vec![b'a'; LineBuffer::MAX_LINE * 3 + 5];
        let lines = buffer.push(&huge);
        assert_eq!(lines.len(), 3, "a runaway line is cut, not buffered");
        assert!(lines.iter().all(|l| l.len() == LineBuffer::MAX_LINE));
        let lines = buffer.push(&[0xff, 0xfe, b'\n']);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains('\u{fffd}'), "invalid UTF-8 is replaced");
    }

    #[test]
    fn stderr_lines_are_diagnostics_only_when_they_parse() {
        let report = Report::from_json(r#"{"kind":"runtime","message":"boom"}"#).unwrap();
        assert_eq!(stderr_event(report.to_json()), RunEvent::Diagnostic(report));
        assert_eq!(
            stderr_event("plain text".to_owned()),
            RunEvent::Output("plain text".to_owned())
        );
    }

    /// The events a fake child delivered, in order, paired with their run id.
    type CapturedEvents = Arc<Mutex<Vec<(RunId, RunEvent)>>>;

    /// Records what a fake launch was asked to do and shares the child's flags.
    #[derive(Default)]
    struct FakeLauncher {
        calls: RefCell<Vec<(PathBuf, PathBuf, RunId)>>,
        killed: Rc<Cell<bool>>,
        running: Rc<Cell<bool>>,
        fail: RefCell<Option<String>>,
        sink: RefCell<Option<EventSink>>,
    }

    impl FakeLauncher {
        fn new() -> FakeLauncher {
            FakeLauncher {
                running: Rc::new(Cell::new(false)),
                ..FakeLauncher::default()
            }
        }

        /// Makes the next launch fail with `message`.
        fn failing(message: &str) -> FakeLauncher {
            FakeLauncher {
                fail: RefCell::new(Some(message.to_owned())),
                ..FakeLauncher::new()
            }
        }

        /// Delivers `event` as the launched child would.
        fn emit(&self, run: RunId, event: RunEvent) {
            let sink = self
                .sink
                .borrow()
                .clone()
                .expect("a child has been launched");
            sink(run, event);
        }
    }

    impl Launcher for FakeLauncher {
        fn launch(
            &self,
            player: &Path,
            project_dir: &Path,
            run: RunId,
            sink: EventSink,
        ) -> Result<Box<dyn ChildProcess>, LaunchError> {
            if let Some(message) = self.fail.borrow().as_ref() {
                return Err(LaunchError::new(
                    player,
                    std::io::Error::other(message.clone()),
                ));
            }
            self.calls
                .borrow_mut()
                .push((player.to_path_buf(), project_dir.to_path_buf(), run));
            *self.sink.borrow_mut() = Some(sink);
            self.running.set(true);
            self.killed.set(false);
            Ok(Box::new(FakeChild {
                killed: Rc::clone(&self.killed),
                running: Rc::clone(&self.running),
            }))
        }
    }

    struct FakeChild {
        killed: Rc<Cell<bool>>,
        running: Rc<Cell<bool>>,
    }

    impl ChildProcess for FakeChild {
        fn kill(&mut self) {
            self.killed.set(true);
            self.running.set(false);
        }

        fn is_running(&mut self) -> bool {
            self.running.get()
        }
    }

    /// A sink that collects every event it is handed.
    fn collector() -> (EventSink, CapturedEvents) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        let sink: EventSink = Arc::new(move |run, event| {
            captured.lock().unwrap().push((run, event));
        });
        (sink, events)
    }

    #[test]
    fn starting_runs_the_child_and_keeps_its_id() {
        let launcher = FakeLauncher::new();
        let (sink, _) = collector();
        let mut state = RunState::new();

        let id = state
            .start(&launcher, Path::new("player"), Path::new("proj"), sink)
            .expect("the fake launch succeeds");

        assert!(state.is_running());
        assert_eq!(state.run_id(), Some(id));
        assert!(state.accepts(id));
        assert_eq!(
            launcher.calls.borrow()[0],
            (PathBuf::from("player"), PathBuf::from("proj"), id)
        );
    }

    #[test]
    fn polling_reaches_the_active_child_and_is_harmless_when_idle() {
        struct Polled(Rc<Cell<u32>>);
        impl ChildProcess for Polled {
            fn kill(&mut self) {}
            fn is_running(&mut self) -> bool {
                true
            }
            fn poll(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        struct PolledLauncher(Rc<Cell<u32>>);
        impl Launcher for PolledLauncher {
            fn launch(
                &self,
                _: &Path,
                _: &Path,
                _: RunId,
                _: EventSink,
            ) -> Result<Box<dyn ChildProcess>, LaunchError> {
                Ok(Box::new(Polled(Rc::clone(&self.0))))
            }
        }
        let polls = Rc::new(Cell::new(0));
        let mut state = RunState::new();
        state.poll();
        assert_eq!(polls.get(), 0, "nothing to poll while idle");
        let (sink, _) = collector();
        state
            .start(
                &PolledLauncher(Rc::clone(&polls)),
                Path::new("p"),
                Path::new("d"),
                sink,
            )
            .unwrap();
        state.poll();
        state.poll();
        assert_eq!(polls.get(), 2);
    }

    #[test]
    fn output_and_diagnostics_carry_the_run_id() {
        let launcher = FakeLauncher::new();
        let (sink, events) = collector();
        let mut state = RunState::new();
        let id = state
            .start(&launcher, Path::new("player"), Path::new("proj"), sink)
            .expect("the fake launch succeeds");

        launcher.emit(id, RunEvent::Output("hello".to_owned()));
        launcher.emit(
            id,
            RunEvent::Diagnostic(Report {
                kind: lazyrad_player::Kind::Runtime,
                file: "main_form.rhai".to_owned(),
                line: 3,
                col: 1,
                message: "boom".to_owned(),
            }),
        );

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|(run, _)| *run == id));
        assert_eq!(events[0].1, RunEvent::Output("hello".to_owned()));
        assert!(matches!(&events[1].1, RunEvent::Diagnostic(report) if report.line == 3));
    }

    #[test]
    fn a_failed_launch_leaves_the_state_idle() {
        let launcher = FakeLauncher::failing("no such file");
        let (sink, _) = collector();
        let mut state = RunState::new();

        assert!(
            state
                .start(&launcher, Path::new("player"), Path::new("proj"), sink)
                .is_err()
        );
        assert!(!state.is_running(), "a failed spawn must not look running");
        assert!(launcher.calls.borrow().is_empty());
    }

    #[test]
    fn end_kills_the_child_and_returns_to_idle() {
        let launcher = FakeLauncher::new();
        let (sink, _) = collector();
        let mut state = RunState::new();
        state
            .start(&launcher, Path::new("player"), Path::new("proj"), sink)
            .expect("launch");

        state.end();

        assert!(!state.is_running());
        assert!(launcher.killed.get(), "End kills the child");
    }

    #[test]
    fn the_active_run_can_report_that_it_finished() {
        let launcher = FakeLauncher::new();
        let (sink, _) = collector();
        let mut state = RunState::new();
        let id = state
            .start(&launcher, Path::new("player"), Path::new("proj"), sink)
            .expect("launch");

        assert!(state.finished(id));
        assert!(!state.is_running());
    }

    #[test]
    fn a_stale_exit_notice_never_clears_the_new_run() {
        let launcher = FakeLauncher::new();
        let (sink, _) = collector();
        let mut state = RunState::new();
        let first = state
            .start(
                &launcher,
                Path::new("player"),
                Path::new("proj"),
                sink.clone(),
            )
            .expect("first launch");
        state.end();

        let second = state
            .start(&launcher, Path::new("player"), Path::new("proj"), sink)
            .expect("second launch");
        assert_ne!(first, second, "a run id is never reused");

        assert!(!state.accepts(first), "the old id is stale");
        assert!(
            !state.finished(first),
            "the old run's exit notice is dropped"
        );
        assert!(state.is_running(), "the new run is untouched");
        assert!(state.accepts(second));
    }

    #[test]
    fn dropping_the_state_kills_the_child() {
        let launcher = FakeLauncher::new();
        let (sink, _) = collector();
        let killed = Rc::clone(&launcher.killed);
        {
            let mut state = RunState::new();
            state
                .start(&launcher, Path::new("player"), Path::new("proj"), sink)
                .expect("launch");
        }
        assert!(
            killed.get(),
            "a dropped IDE does not leave a program running"
        );
    }

    #[test]
    fn the_player_is_launched_watching_the_project() {
        let command = player_command(Path::new("player"), Path::new("proj"), LaunchMode::Run);
        let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
        assert_eq!(
            args,
            [
                std::ffi::OsStr::new("--watch"),
                std::ffi::OsStr::new("proj")
            ],
            "the IDE runs the player with hot reload on"
        );
    }

    #[test]
    fn a_debug_run_passes_debug_instead_of_watch() {
        let command = player_command(Path::new("player"), Path::new("proj"), LaunchMode::Debug);
        let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
        assert_eq!(
            args,
            [
                std::ffi::OsStr::new("--debug"),
                std::ffi::OsStr::new("proj")
            ]
        );
    }

    #[test]
    fn stdout_lines_are_protocol_only_in_a_debug_run() {
        let line = lazyrad_debug_proto::encode_event(&Event::Resumed);
        assert_eq!(
            stdout_event(line.clone(), true),
            RunEvent::Debug(Event::Resumed)
        );
        assert_eq!(
            stdout_event(line.clone(), false),
            RunEvent::Output(line),
            "a plain run never decodes"
        );
        // Program output, including JSON that is not the protocol.
        for text in [
            "hello",
            r#"{"name":"Ada"}"#,
            "{",
            r#"{"v":1,"event":"nope"}"#,
        ] {
            assert_eq!(
                stdout_event(text.to_owned(), true),
                RunEvent::Output(text.to_owned())
            );
        }
    }

    #[test]
    fn a_player_speaking_another_version_is_reported_not_misread() {
        let skewed = r#"{"v":7,"event":"resumed"}"#.to_owned();
        let RunEvent::Output(text) = stdout_event(skewed, true) else {
            panic!("a notice");
        };
        assert!(
            text.contains("protocol 7") && text.contains("[debugger]"),
            "{text}"
        );
    }

    #[test]
    fn a_launcher_without_debug_support_refuses_to_debug_and_cannot_send() {
        let launcher = FakeLauncher::new();
        let (sink, _) = collector();
        let mut state = RunState::new();
        let refused = state.start_with(
            &launcher,
            Path::new("player"),
            Path::new("proj"),
            sink.clone(),
            LaunchMode::Debug,
        );
        assert!(refused.is_err(), "no silent run without a debugger");
        assert!(!state.is_running());
        assert!(launcher.calls.borrow().is_empty());

        // A plain run still works, and its child cannot take input.
        state
            .start_with(
                &launcher,
                Path::new("player"),
                Path::new("proj"),
                sink,
                LaunchMode::Run,
            )
            .expect("a plain run launches");
        assert_eq!(state.send_line("{}"), Err(SendError::Unsupported));
    }

    #[test]
    fn sending_with_nothing_running_is_an_error() {
        let mut state = RunState::new();
        assert_eq!(state.send_line("{}"), Err(SendError::NotRunning));
    }

    /// A child that records what it was sent.
    struct Recording {
        sent: Rc<RefCell<Vec<String>>>,
        fail_after: usize,
    }

    impl ChildProcess for Recording {
        fn kill(&mut self) {}
        fn is_running(&mut self) -> bool {
            true
        }
        fn send_line(&mut self, line: &str) -> Result<(), SendError> {
            if self.sent.borrow().len() >= self.fail_after {
                return Err(SendError::Closed);
            }
            self.sent.borrow_mut().push(line.to_owned());
            Ok(())
        }
    }

    struct RecordingLauncher {
        sent: Rc<RefCell<Vec<String>>>,
        fail_after: usize,
    }

    impl Launcher for RecordingLauncher {
        fn launch(
            &self,
            _: &Path,
            _: &Path,
            _: RunId,
            _: EventSink,
        ) -> Result<Box<dyn ChildProcess>, LaunchError> {
            Ok(Box::new(Recording {
                sent: Rc::clone(&self.sent),
                fail_after: self.fail_after,
            }))
        }

        fn launch_with(
            &self,
            player: &Path,
            project_dir: &Path,
            run: RunId,
            sink: EventSink,
            _: LaunchMode,
        ) -> Result<Box<dyn ChildProcess>, LaunchError> {
            self.launch(player, project_dir, run, sink)
        }
    }

    #[test]
    fn lines_reach_the_active_child_and_a_closed_child_reports_it() {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let launcher = RecordingLauncher {
            sent: Rc::clone(&sent),
            fail_after: 2,
        };
        let (sink, _) = collector();
        let mut state = RunState::new();
        state
            .start_with(
                &launcher,
                Path::new("p"),
                Path::new("d"),
                sink,
                LaunchMode::Debug,
            )
            .unwrap();
        assert_eq!(state.send_line("one"), Ok(()));
        assert_eq!(state.send_line("two"), Ok(()));
        assert_eq!(state.send_line("three"), Err(SendError::Closed));
        assert_eq!(*sent.borrow(), ["one", "two"]);
    }

    /// A command that exits at once, so its stdin pipe is closed by the time
    /// anything is written: this test binary, asked only to list its tests.
    fn exits_immediately() -> Command {
        let mut command = Command::new(std::env::current_exe().expect("the test binary"));
        command.args(["--list", "--quiet"]);
        command
    }

    #[test]
    fn writing_to_a_dead_child_fails_instead_of_blocking_or_panicking() {
        let mut child = exits_immediately()
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("a shell starts");
        let stdin = child.stdin.take().expect("piped stdin");
        let _ = child.wait();
        let writer = StdinWriter::start(stdin);

        // The first write may be accepted by the OS before the broken pipe is
        // noticed; within a moment every send must report the closed pipe.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut failed = false;
        while std::time::Instant::now() < deadline {
            if writer.send("{\"v\":1,\"cmd\":\"continue\"}") == Err(SendError::Closed) {
                failed = true;
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(failed, "a write to a dead child must surface as Closed");
        assert_eq!(writer.send("again"), Err(SendError::Closed), "and stays so");
    }

    #[test]
    fn a_configured_player_must_exist() {
        let missing = Path::new("definitely/not/here/lazyrad-player");
        assert!(matches!(
            resolve_player(Some(missing)),
            Err(PlayerError::OverrideMissing(_))
        ));
    }

    #[test]
    fn an_existing_player_override_is_used() {
        let dir = std::env::temp_dir().join(format!("lazyrad-ide-player-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        let player = dir.join(format!("lazyrad-player{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&player, b"stub").expect("write the stub");
        assert_eq!(resolve_player(Some(&player)).ok(), Some(player.clone()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
