#![forbid(unsafe_code)]

//! A script error located in its source file.
//!
//! Rhai reports a [`Position`](rhai::Position) as a line and column but does not
//! know which file the script came from, because an [`AST`](rhai::AST) carries
//! no file name. The runtime pairs the position with the file it compiled, so
//! the IDE can jump straight to the failing line.

use std::fmt;

use rhai::{EvalAltResult, ParseError, Position};

/// A script failure with the file and one-based position it happened at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptError {
    /// The script file the error is reported against.
    pub file: String,
    /// The one-based line, or `0` when Rhai had no position.
    pub line: usize,
    /// The one-based column, or `0` when Rhai had no position.
    pub column: usize,
    /// The human-readable message.
    pub message: String,
}

impl ScriptError {
    /// Creates an error from a file and a position.
    pub fn new(file: impl Into<String>, position: Position, message: impl Into<String>) -> Self {
        ScriptError {
            file: file.into(),
            line: position.line().unwrap_or(0),
            column: position.position().unwrap_or(0),
            message: message.into(),
        }
    }

    /// Locates a compile error.
    pub fn from_parse(file: impl Into<String>, error: &ParseError) -> Self {
        ScriptError::new(file, error.position(), error.to_string())
    }

    /// Locates a runtime error.
    ///
    /// A script terminated through the progress hook carries its termination
    /// value in [`EvalAltResult::ErrorTerminated`]; Rhai's `Display` hides it,
    /// so the value becomes the message.
    pub fn from_eval(file: impl Into<String>, error: &EvalAltResult) -> Self {
        let message = match error {
            EvalAltResult::ErrorTerminated(value, _) => value.to_string(),
            _ => error.to_string(),
        };
        ScriptError::new(file, error.position(), message)
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}: {}",
            self.file, self.line, self.column, self.message
        )
    }
}

impl std::error::Error for ScriptError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_carries_file_line_and_column() {
        let engine = rhai::Engine::new();
        let error = engine
            .compile("fn button_click() {\n    let x = ;\n}")
            .expect_err("the script does not compile");
        let located = ScriptError::from_parse("frmMain.rhai", &error);
        assert_eq!(located.file, "frmMain.rhai");
        assert_eq!(located.line, 2);
        assert!(located.column > 0);
        assert_eq!(
            located.to_string(),
            format!("frmMain.rhai:2:{}: {}", located.column, located.message)
        );
    }
}
