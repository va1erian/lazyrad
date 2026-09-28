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

use std::collections::BTreeMap;
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

/// The UI-thread half of the debounce: each document's latest source and when
/// it is due.
///
/// Jobs and revisions are kept per document, so scheduling one document never
/// drops another's pending compile or makes its in-flight result look stale.
/// It is shared (`Rc<RefCell<..>>`) with the window's timer mapper, which calls
/// [`CompileScheduler::take_due`] on every tick.
#[derive(Debug, Default)]
pub struct CompileScheduler {
    /// A counter shared by every document, so a revision is never reused, even
    /// across a [`CompileScheduler::reset`].
    next_revision: u64,
    /// Each document's newest scheduled revision.
    latest: BTreeMap<String, u64>,
    /// Each document's pending job and the time it comes due.
    pending: BTreeMap<String, (Instant, Job)>,
}

impl CompileScheduler {
    /// An idle scheduler.
    pub fn new() -> CompileScheduler {
        CompileScheduler::default()
    }

    /// Records `source` for `name`, debouncing to [`COMPILE_DEBOUNCE`] from
    /// `now`. A later call for the same document replaces its pending job, so
    /// only a document's newest source is compiled; other documents are
    /// unaffected.
    pub fn schedule(&mut self, name: &str, source: &str, now: Instant) {
        self.next_revision += 1;
        let revision = self.next_revision;
        self.latest.insert(name.to_owned(), revision);
        self.pending.insert(
            name.to_owned(),
            (
                now + COMPILE_DEBOUNCE,
                Job {
                    name: name.to_owned(),
                    source: source.to_owned(),
                    revision,
                },
            ),
        );
    }

    /// Takes every job whose due time `now` has reached, clearing them.
    /// Returns an empty list while every debounce is still running or when
    /// idle.
    pub fn take_due(&mut self, now: Instant) -> Vec<Job> {
        let due: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, (at, _))| now >= *at)
            .map(|(name, _)| name.clone())
            .collect();
        due.into_iter()
            .filter_map(|name| self.pending.remove(&name).map(|(_, job)| job))
            .collect()
    }

    /// `name`'s newest scheduled revision, if it was ever scheduled.
    pub fn revision(&self, name: &str) -> Option<u64> {
        self.latest.get(name).copied()
    }

    /// Whether a result for `name` stamped `revision` is still the newest; an
    /// older one is stale and should be ignored.
    pub fn is_current(&self, name: &str, revision: u64) -> bool {
        self.revision(name) == Some(revision)
    }

    /// Whether any job is waiting for its debounce to elapse.
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Forgets every pending job and revision, so a compile still running for
    /// the previous project is stale when its result arrives.
    pub fn reset(&mut self) {
        self.latest.clear();
        self.pending.clear();
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
        scheduler.schedule("main_form", "let x = 1;", start);

        assert!(
            scheduler
                .take_due(start + Duration::from_millis(100))
                .is_empty(),
            "still inside the debounce"
        );
        assert!(scheduler.is_pending());

        let jobs = scheduler.take_due(start + COMPILE_DEBOUNCE);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "main_form");
        assert_eq!(jobs[0].source, "let x = 1;");
        assert!(!scheduler.is_pending());
        assert!(scheduler.take_due(start + COMPILE_DEBOUNCE * 2).is_empty());
    }

    #[test]
    fn a_newer_schedule_replaces_the_documents_pending_job() {
        let start = Instant::now();
        let mut scheduler = CompileScheduler::new();
        scheduler.schedule("main_form", "one", start);
        let first = scheduler.revision("main_form").expect("scheduled");
        scheduler.schedule("main_form", "two", start + Duration::from_millis(10));
        assert!(!scheduler.is_current("main_form", first));

        let jobs = scheduler.take_due(start + COMPILE_DEBOUNCE + Duration::from_millis(10));
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].source, "two", "only the newest source compiles");
        assert!(scheduler.is_current("main_form", jobs[0].revision));
    }

    #[test]
    fn documents_are_scheduled_and_judged_independently() {
        let start = Instant::now();
        let mut scheduler = CompileScheduler::new();
        scheduler.schedule("main_form", "a", start);
        scheduler.schedule("util", "b", start + Duration::from_millis(5));

        let jobs = scheduler.take_due(start + COMPILE_DEBOUNCE + Duration::from_millis(5));
        let names: Vec<&str> = jobs.iter().map(|job| job.name.as_str()).collect();
        assert_eq!(names, ["main_form", "util"], "neither job is lost");
        for job in &jobs {
            assert!(scheduler.is_current(&job.name, job.revision));
        }
    }

    #[test]
    fn a_reset_makes_in_flight_results_stale() {
        let start = Instant::now();
        let mut scheduler = CompileScheduler::new();
        scheduler.schedule("main_form", "a", start);
        let jobs = scheduler.take_due(start + COMPILE_DEBOUNCE);
        scheduler.reset();
        assert!(!scheduler.is_current("main_form", jobs[0].revision));

        scheduler.schedule("main_form", "b", start);
        assert!(!scheduler.is_current("main_form", jobs[0].revision));
    }
}
