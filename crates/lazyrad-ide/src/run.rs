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

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// `CREATE_NO_WINDOW` from the Windows SDK's `WinBase.h` process creation
/// flags: start a console application without a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

use lazyrad_player::Report;
use lazyrad_runtime::check_project;

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
    /// The process ended, with its exit code when the platform reports one.
    Exited(Option<i32>),
}

/// Where a launcher delivers its child's events. The real launcher wraps the
/// window's [`Proxy`](xui_core::app::Proxy); tests capture into a vector.
pub type EventSink = Arc<dyn Fn(RunId, RunEvent) + Send + Sync + 'static>;

/// A child process the run state can end and observe.
pub trait ChildProcess {
    /// Asks the process to terminate. Idempotent.
    fn kill(&mut self);
    /// Whether the process is still alive.
    fn is_running(&mut self) -> bool;
}

/// Spawns the player and streams its output back through an [`EventSink`].
pub trait Launcher {
    /// Launches `player` on `project_dir`, stamping every event with `run`.
    fn launch(
        &self,
        player: &Path,
        project_dir: &Path,
        run: RunId,
        sink: EventSink,
    ) -> Result<Box<dyn ChildProcess>, LaunchError>;
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

impl Launcher for PlayerLauncher {
    fn launch(
        &self,
        player: &Path,
        project_dir: &Path,
        run: RunId,
        sink: EventSink,
    ) -> Result<Box<dyn ChildProcess>, LaunchError> {
        let mut command = Command::new(player);
        command
            .arg(project_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // The IDE is a GUI-subsystem app, so Windows would give the
        // console-subsystem player a console window of its own; its output is
        // piped back to the IDE instead, so don't create one.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command
            .spawn()
            .map_err(|source| LaunchError::new(player, source))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LaunchError::new(
                player,
                std::io::Error::other("the player's output pipes are unavailable"),
            ));
        };

        let child = Arc::new(Mutex::new(child));
        let running = Arc::new(AtomicBool::new(true));

        let stdout_sink = Arc::clone(&sink);
        let stdout_reader = spawn_thread("lazyrad-player-stdout", move || {
            read_stdout(stdout, run, stdout_sink);
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

        Ok(Box::new(SpawnedChild { child, running }))
    }
}

/// The child a [`PlayerLauncher`] returns. Dropping it kills the process, so a
/// run that is never explicitly ended cannot outlive the IDE.
struct SpawnedChild {
    child: Arc<Mutex<Child>>,
    running: Arc<AtomicBool>,
}

impl ChildProcess for SpawnedChild {
    fn kill(&mut self) {
        let _ = kill(&self.child);
    }

    fn is_running(&mut self) -> bool {
        self.running.load(Ordering::SeqCst)
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

/// Reads `pipe` line by line, reporting each as [`RunEvent::Output`].
fn read_stdout(pipe: ChildStdout, run: RunId, sink: EventSink) {
    for line in BufReader::new(pipe).lines().map_while(Result::ok) {
        sink(run, RunEvent::Output(line));
    }
}

/// Reads `pipe` line by line, reporting a JSON diagnostic as
/// [`RunEvent::Diagnostic`] and anything else as output.
fn read_stderr(pipe: ChildStderr, run: RunId, sink: EventSink) {
    for line in BufReader::new(pipe).lines().map_while(Result::ok) {
        let event = match Report::from_json(&line) {
            Some(report) => RunEvent::Diagnostic(report),
            None => RunEvent::Output(line),
        };
        sink(run, event);
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
        let id = self.next;
        let child = launcher.launch(player, project_dir, id, sink)?;
        self.next += 1;
        self.active = Some(Active { id, child });
        Ok(id)
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
        })
        .collect();
    problems.extend(report.scripts.iter().map(|error| RunDiagnostic {
        file: error.file.clone(),
        line: error.line,
        col: error.column,
        message: error.message.clone(),
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
    /// The IDE's own executable location could not be read.
    #[error("the IDE executable location could not be read: {0}")]
    Exe(std::io::Error),
    /// The IDE executable has no folder to look in.
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
    let exe = std::env::current_exe().map_err(PlayerError::Exe)?;
    let Some(dir) = exe.parent() else {
        return Err(PlayerError::NoExeDir);
    };
    let candidate = dir.join(format!("lazyrad-player{}", std::env::consts::EXE_SUFFIX));
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
