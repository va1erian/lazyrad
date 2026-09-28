#![forbid(unsafe_code)]

//! Errors and validation diagnostics.
//!
//! A project has two layers of problems: file-level ones (a missing form, a bad
//! `.lrp`, a TOML syntax error) and form-level ones (a bad control), which come
//! from [`xui_form`]. Both funnel through [`Diagnostic`], which names the file
//! and, when the text can be located, the line. [`Error`] wraps a diagnostic
//! together with plain I/O failures.

use std::fmt;
use std::path::PathBuf;

/// What kind of problem a [`Diagnostic`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticKind {
    /// The project or a form could not be parsed.
    Syntax,
    /// A referenced file does not exist on disk.
    MissingFile,
    /// A project directory has no `.lrp`, or more than one.
    ProjectFile,
    /// The project's `startup` names an item that is not in `items`.
    UnknownStartup,
    /// A form failed validation.
    InvalidForm,
}

/// One problem found in a project file.
///
/// `file` is absolute or project-relative depending on how the caller passed it
/// in; `line` is one-based and present only when the offending text was located.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// The kind of problem.
    pub kind: DiagnosticKind,
    /// The file the problem is in.
    pub file: PathBuf,
    /// The one-based line, when it is known.
    pub line: Option<usize>,
    /// A human-readable description.
    pub message: String,
}

impl Diagnostic {
    /// Builds a diagnostic with no line number.
    pub fn new(kind: DiagnosticKind, file: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            kind,
            file: file.into(),
            line: None,
            message: message.into(),
        }
    }

    /// Builds a diagnostic at a known one-based line.
    pub fn at(
        kind: DiagnosticKind,
        file: impl Into<PathBuf>,
        line: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            file: file.into(),
            line: Some(line),
            message: message.into(),
        }
    }

    /// Returns the same diagnostic with a line number attached.
    #[must_use]
    pub fn with_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }

    /// Converts a `xui-form` form diagnostic into a project diagnostic against
    /// `file`.
    pub fn from_form(file: impl Into<PathBuf>, diagnostic: &xui_form::Diagnostic) -> Self {
        let prefix = match (&diagnostic.node, &diagnostic.property) {
            (Some(node), Some(property)) => format!("{node}.{property}: "),
            (Some(node), None) => format!("{node}: "),
            (None, Some(property)) => format!("{property}: "),
            (None, None) => String::new(),
        };
        Self {
            kind: DiagnosticKind::InvalidForm,
            file: file.into(),
            line: diagnostic.line,
            message: format!("{prefix}{}", diagnostic.message),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(
                formatter,
                "{}:{}: {}",
                self.file.display(),
                line,
                self.message
            ),
            None => write!(formatter, "{}: {}", self.file.display(), self.message),
        }
    }
}

impl std::error::Error for Diagnostic {}

/// A failure to load or save a project.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A problem with the content of a file, carrying its own location.
    #[error(transparent)]
    Diagnostic(#[from] Diagnostic),

    /// The file could not be read or written.
    #[error("I/O error for {path}: {source}")]
    Io {
        /// The file that could not be accessed.
        path: PathBuf,
        /// The underlying operating-system error.
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    /// Wraps an I/O error, remembering which path failed.
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diagnostic_display_includes_the_line_when_known() {
        let located = Diagnostic::at(
            DiagnosticKind::Syntax,
            "frmMain.lfm",
            7,
            "`foo` is not a property",
        );
        assert_eq!(
            located.to_string(),
            "frmMain.lfm:7: `foo` is not a property"
        );

        let loose = Diagnostic::new(DiagnosticKind::Syntax, "frmMain.lfm", "unexpected token");
        assert_eq!(loose.to_string(), "frmMain.lfm: unexpected token");
    }

    #[test]
    fn with_line_attaches_a_line() {
        let diagnostic =
            Diagnostic::new(DiagnosticKind::MissingFile, "Hello.lrp", "gone").with_line(3);
        assert_eq!(diagnostic.line, Some(3));
    }

    #[test]
    fn an_io_error_names_the_path() {
        let error = Error::io(
            "Missing.lrp",
            std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert_eq!(
            error.to_string(),
            "I/O error for Missing.lrp: entity not found"
        );
    }
}
