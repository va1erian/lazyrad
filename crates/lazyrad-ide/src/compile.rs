#![forbid(unsafe_code)]

//! Background syntax checking for the code windows (issue #11).
//!
//! Typing should not block the UI on a compile, so a change only arms a
//! debounce timer. When the timer comes due the IDE moves the current source to
//! a worker thread, which builds its own [`Engine`] and calls
//! [`Engine::compile`]. The result travels back through a [`Proxy`], so the
//! same single-threaded message loop that drives the widgets delivers it.
//!
//! The Rhai engine is neither `Send` nor cheap to clone (it carries `Rc`
//! state), so it is created *inside* the worker rather than shared. Only plain
//! data — the source, a revision number and the reported errors — crosses the
//! thread boundary.
//!
//! [`Engine`]: rhai::Engine
//! [`Engine::compile`]: rhai::Engine::compile

use std::time::{Duration, Instant};

use lazyrad_runtime::engine::new_engine;

/// How long the editor waits after the last keystroke before compiling.
///
/// The issue asks that a syntax error surface within about half a second, so
/// the debounce plus the worker's compile must fit in that budget.
pub const COMPILE_DEBOUNCE: Duration = Duration::from_millis(300);

/// One parse error, in plain data, so it can cross a thread boundary and be
/// shown in the editor and the Error List.
///
/// `line` and `col` are one-based, exactly as Rhai reports them. The editor
/// converts them to its zero-based display coordinates when it builds markers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeDiagnostic {
    /// The one-based line of the error.
    pub line: usize,
    /// The one-based column of the error.
    pub col: usize,
    /// The human-readable message.
    pub message: String,
}

impl CodeDiagnostic {
    /// A diagnostic at `line`/`col` (one-based).
    pub fn new(line: usize, col: usize, message: impl Into<String>) -> CodeDiagnostic {
        CodeDiagnostic {
            line: line.max(1),
            col: col.max(1),
            message: message.into(),
        }
    }

    /// The diagnostic's message prefixed by its one-based position, for the
    /// Error List.
    pub fn label(&self) -> String {
        format!("({}:{}) {}", self.line, self.col, self.message)
    }
}

/// A compile job handed to a worker thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    /// The item whose source is being compiled.
    pub name: String,
    /// The source text.
    pub source: String,
    /// The revision the result belongs to; a newer revision makes it stale.
    pub revision: u64,
}

/// The UI-thread half of the debounce: the latest source and when it is due.
///
/// It is shared (`Rc<RefCell<..>>`) with the window's timer mapper, which calls
/// [`CompileScheduler::take_due`] on every tick.
#[derive(Debug, Default)]
pub struct CompileScheduler {
    revision: u64,
    due: Option<Instant>,
    job: Option<Job>,
}

impl CompileScheduler {
    /// An idle scheduler.
    pub fn new() -> CompileScheduler {
        CompileScheduler::default()
    }

    /// Records `source` for `name`, debouncing to [`COMPILE_DEBOUNCE`] from
    /// `now`. A later call replaces the pending job, so only the newest source
    /// is ever compiled.
    pub fn schedule(&mut self, name: &str, source: &str, now: Instant) {
        self.revision += 1;
        self.job = Some(Job {
            name: name.to_owned(),
            source: source.to_owned(),
            revision: self.revision,
        });
        self.due = Some(now + COMPILE_DEBOUNCE);
    }

    /// Takes the pending job when `now` has reached its due time, clearing it.
    /// Returns `None` while the debounce is still running or when idle.
    pub fn take_due(&mut self, now: Instant) -> Option<Job> {
        if self.due.is_some_and(|due| now >= due) {
            self.due = None;
            return self.job.take();
        }
        None
    }

    /// The newest scheduled revision. A result stamped older than this is
    /// stale and should be ignored.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether a job is waiting for its debounce to elapse.
    pub fn is_pending(&self) -> bool {
        self.job.is_some()
    }
}

/// Compiles `source` and returns the parse errors it reports.
///
/// This runs on a worker thread; it builds its own engine and never touches the
/// UI. A successful compile yields no diagnostics.
pub fn compile_source(source: &str) -> Vec<CodeDiagnostic> {
    let engine = new_engine();
    match engine.compile(source) {
        Ok(_) => Vec::new(),
        Err(error) => {
            let line = error.position().line().unwrap_or(1);
            let col = error.position().position().unwrap_or(1);
            // `err_type` drops the redundant "(line N, position M)" suffix; the
            // position is carried separately.
            vec![CodeDiagnostic::new(line, col, error.err_type().to_string())]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_script_has_no_diagnostics() {
        assert!(compile_source("fn form_load() {\n    let x = 1;\n}\n").is_empty());
    }

    #[test]
    fn a_syntax_error_reports_a_position() {
        // A missing closing paren: Rhai reports it at the offending spot.
        let errors = compile_source("fn form_load( {\n}\n");
        assert_eq!(errors.len(), 1, "one parse error: {errors:?}");
        assert!(errors[0].line >= 1);
        assert!(errors[0].col >= 1);
        assert!(!errors[0].message.is_empty());
    }

    #[test]
    fn a_diagnostic_label_carries_its_position() {
        let error = CodeDiagnostic::new(3, 7, "bad");
        assert_eq!(error.label(), "(3:7) bad");
    }

    #[test]
    fn the_scheduler_debounces_until_the_due_time() {
        let start = Instant::now();
        let mut scheduler = CompileScheduler::new();
        scheduler.schedule("Form1", "let x = 1;", start);

        assert!(
            scheduler
                .take_due(start + Duration::from_millis(100))
                .is_none(),
            "still inside the debounce"
        );
        assert!(scheduler.is_pending());

        let job = scheduler
            .take_due(start + COMPILE_DEBOUNCE)
            .expect("the job is due");
        assert_eq!(job.name, "Form1");
        assert_eq!(job.source, "let x = 1;");
        assert!(!scheduler.is_pending());
        assert!(scheduler.take_due(start + COMPILE_DEBOUNCE * 2).is_none());
    }

    #[test]
    fn a_newer_schedule_replaces_the_pending_job_and_bumps_the_revision() {
        let start = Instant::now();
        let mut scheduler = CompileScheduler::new();
        scheduler.schedule("Form1", "one", start);
        let first = scheduler.revision();
        scheduler.schedule("Form1", "two", start + Duration::from_millis(10));
        assert!(scheduler.revision() > first);

        let job = scheduler
            .take_due(start + COMPILE_DEBOUNCE + Duration::from_millis(10))
            .expect("due");
        assert_eq!(job.source, "two", "only the newest source compiles");
        assert_eq!(job.revision, scheduler.revision());
    }
}
