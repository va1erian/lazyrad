#![forbid(unsafe_code)]

//! The `lazyrad-player` command line.
//!
//! The player is the host that runs a LazyRAD program: it validates and compiles
//! the whole project, opens the startup form and runs the `xui` event loop
//! (PLAN.md §4). It is also the stub that exported executables are built from
//! (PLAN.md §8).
//!
//! # Diagnostics and exit codes
//!
//! Before anything is shown, the project is loaded, validated and every script
//! is compiled through [`lazyrad_runtime::check_project`]. All problems are
//! reported at once:
//!
//! * `stderr` carries one JSON object per problem, one per line, always;
//! * when `stderr` is a terminal, a human-readable line is printed too.
//!
//! The JSON shape is
//! `{"kind":"compile"|"runtime","file":…,"line":…,"col":…,"message":…}`. The
//! process then exits `0` normally, `1` for a load/validate/compile problem, or
//! `2` for a fatal runtime error. A runtime error inside an event handler is not
//! fatal: the runtime shows it in a `msg_box` and the program keeps running, so
//! it does not change the exit code.
//!
//! `Debug.print` (Rhai's `print`/`debug`) goes to stdout, one flushed line per
//! call, so an IDE reading the player's stdout sees each line as it happens.

use std::io::{IsTerminal, Write};
use std::path::Path;

use lazyrad_project::Diagnostic;
use lazyrad_runtime::{FormRuntime, RuntimeError, ScriptError, check_project, run_runtime};

/// The exit code for a normal run.
pub const EXIT_OK: i32 = 0;
/// The exit code for a project that does not load, validate or compile.
pub const EXIT_COMPILE: i32 = 1;
/// The exit code for a fatal runtime error.
pub const EXIT_RUNTIME: i32 = 2;

/// Reads the command line and runs the program, returning the process exit code.
///
/// `lazyrad-player <project dir | .lrp>` runs a project; with no argument the
/// player opens an empty window, which is the shell M0 used to prove the
/// windowed path works.
pub fn run_cli() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => run_empty_window(),
        [path] => run_project(Path::new(path)),
        _ => {
            eprintln!("usage: lazyrad-player <project dir | .lrp>");
            EXIT_COMPILE
        }
    }
}

/// Opens the empty-window shell (no project was named).
fn run_empty_window() -> i32 {
    let result =
        lazyrad_runtime::shell::run_empty_window("LazyRAD Player").map_err(RuntimeError::from);
    match result {
        Ok(()) => EXIT_OK,
        Err(error) => {
            emit(&[Report::from_runtime_error(&error)]);
            EXIT_RUNTIME
        }
    }
}

/// Checks, loads and runs the project named by `path`.
fn run_project(path: &Path) -> i32 {
    let report = match check_project(path) {
        Ok(report) => report,
        Err(error) => {
            emit(&[Report::from_load_error(&error)]);
            return EXIT_COMPILE;
        }
    };

    if !report.is_empty() {
        let mut reports: Vec<Report> = report
            .diagnostics
            .iter()
            .map(Report::from_diagnostic)
            .collect();
        reports.extend(report.scripts.iter().map(Report::from_compile_script));
        emit(&reports);
        return EXIT_COMPILE;
    }

    let runtime = match FormRuntime::load_path(path) {
        Ok(runtime) => runtime,
        Err(error) => {
            emit(&[Report::from_load_error(&error)]);
            return EXIT_COMPILE;
        }
    };

    match run_runtime(runtime) {
        Ok(()) => EXIT_OK,
        Err(error) => {
            emit(&[Report::from_runtime_error(&error)]);
            EXIT_RUNTIME
        }
    }
}

/// What kind of problem a [`Report`] describes. These strings are the JSON
/// `kind` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A load, validation or compilation problem found before the program ran.
    Compile,
    /// A fatal problem while the program was running.
    Runtime,
}

impl Kind {
    /// The name used in the machine-readable output.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Compile => "compile",
            Kind::Runtime => "runtime",
        }
    }
}

/// One problem, in the shape both output formats need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Whether this is a compile-phase or runtime problem.
    pub kind: Kind,
    /// The source file, or empty when the problem has none.
    pub file: String,
    /// The one-based line, or `0` when it is unknown.
    pub line: usize,
    /// The one-based column, or `0` when it is unknown.
    pub col: usize,
    /// The human-readable description.
    pub message: String,
}

impl Report {
    /// A compile-phase report for a validation diagnostic.
    pub fn from_diagnostic(diagnostic: &Diagnostic) -> Report {
        Report {
            kind: Kind::Compile,
            file: diagnostic.file.display().to_string(),
            line: diagnostic.line.unwrap_or(0),
            col: 0,
            message: diagnostic.message.clone(),
        }
    }

    /// A compile-phase report for a script that failed to parse.
    pub fn from_compile_script(error: &ScriptError) -> Report {
        Report {
            kind: Kind::Compile,
            file: error.file.clone(),
            line: error.line,
            col: error.column,
            message: error.message.clone(),
        }
    }

    /// A load-phase report: the project itself could not be opened.
    pub fn from_load_error(error: &RuntimeError) -> Report {
        Report::from_error(Kind::Compile, error)
    }

    /// A fatal runtime report.
    pub fn from_runtime_error(error: &RuntimeError) -> Report {
        Report::from_error(Kind::Runtime, error)
    }

    /// Describes `error`, keeping its location when it has one.
    fn from_error(kind: Kind, error: &RuntimeError) -> Report {
        match error {
            RuntimeError::Script(script) => Report {
                kind,
                file: script.file.clone(),
                line: script.line,
                col: script.column,
                message: script.message.clone(),
            },
            RuntimeError::Project(lazyrad_project::Error::Diagnostic(diagnostic)) => {
                let mut report = Report::from_diagnostic(diagnostic);
                report.kind = kind;
                report
            }
            RuntimeError::Project(lazyrad_project::Error::Io { path, source }) => Report {
                kind,
                file: path.display().to_string(),
                line: 0,
                col: 0,
                message: source.to_string(),
            },
            RuntimeError::Io { path, source } => Report {
                kind,
                file: path.display().to_string(),
                line: 0,
                col: 0,
                message: source.to_string(),
            },
            other => Report {
                kind,
                file: String::new(),
                line: 0,
                col: 0,
                message: other.to_string(),
            },
        }
    }

    /// The machine-readable JSON object for this report.
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "kind": self.kind.as_str(),
            "file": self.file,
            "line": self.line,
            "col": self.col,
            "message": self.message,
        })
        .to_string()
    }

    /// The human-readable single line for this report.
    pub fn to_text(&self) -> String {
        if self.line > 0 && self.col > 0 {
            format!("{}:{}:{}: {}", self.file, self.line, self.col, self.message)
        } else if self.line > 0 {
            format!("{}:{}: {}", self.file, self.line, self.message)
        } else if self.file.is_empty() {
            format!("lazyrad-player: {}", self.message)
        } else {
            format!("{}: {}", self.file, self.message)
        }
    }
}

/// Writes `reports` to stderr: JSON always, and a readable line on a terminal.
fn emit(reports: &[Report]) {
    let stderr = std::io::stderr();
    let human = stderr.is_terminal();
    let mut out = stderr.lock();
    for report in reports {
        if human {
            let _ = writeln!(out, "{}", report.to_text());
        }
        let _ = writeln!(out, "{}", report.to_json());
    }
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Kind, Report};

    #[test]
    fn a_diagnostic_reports_its_line_and_no_column() {
        let diagnostic = lazyrad_project::Diagnostic::at(
            lazyrad_project::DiagnosticKind::Syntax,
            "main_form.lfm",
            7,
            "`nope` is not a property",
        );
        let report = Report::from_diagnostic(&diagnostic);
        assert_eq!(report.kind, Kind::Compile);
        assert_eq!(report.file, "main_form.lfm");
        assert_eq!(report.line, 7);
        assert_eq!(report.col, 0);
        assert_eq!(
            report.to_text(),
            "main_form.lfm:7: `nope` is not a property"
        );
    }

    #[test]
    fn json_is_one_object_with_the_expected_keys() {
        let report = Report {
            kind: Kind::Runtime,
            file: "main_form.rhai".to_owned(),
            line: 3,
            col: 12,
            message: "boom \"quoted\"".to_owned(),
        };
        let json = report.to_json();
        assert!(json.starts_with('{') && json.ends_with('}'));
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(value["kind"], "runtime");
        assert_eq!(value["file"], "main_form.rhai");
        assert_eq!(value["line"], 3);
        assert_eq!(value["col"], 12);
        assert_eq!(value["message"], "boom \"quoted\"");
        assert_eq!(json.lines().count(), 1, "one object per line");
    }

    #[test]
    fn text_falls_back_when_there_is_no_location() {
        let report = Report {
            kind: Kind::Runtime,
            file: String::new(),
            line: 0,
            col: 0,
            message: "the backend is gone".to_owned(),
        };
        assert_eq!(report.to_text(), "lazyrad-player: the backend is gone");

        let named = Report {
            file: "main_form.lfm".to_owned(),
            ..report
        };
        assert_eq!(named.to_text(), "main_form.lfm: the backend is gone");
    }

    #[test]
    fn a_missing_project_is_a_compile_error() {
        let dir =
            std::env::temp_dir().join(format!("lazyrad-player-no-project-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory is created");
        let error =
            lazyrad_runtime::check_project(&dir).expect_err("a directory with no .lrp fails");
        let report = Report::from_load_error(&error);
        assert_eq!(report.kind, Kind::Compile);
        let _ = std::fs::remove_dir_all(PathBuf::from(&dir));
    }
}
