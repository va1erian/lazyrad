//! Errors a binding can produce, with source positions for lint use.

use std::fmt;

use rhai::{EvalAltResult, ParseError, Position};

use crate::bindings::{MAX_CALL_LEVELS, MAX_OPERATIONS};
use crate::value::ValueError;

/// A line and column in an expression's source, both starting at 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourcePosition {
    /// The line.
    pub line: usize,
    /// The column, when the parser or evaluator knows it.
    pub column: Option<usize>,
}

impl SourcePosition {
    fn from_rhai(position: Position) -> Option<Self> {
        position.line().map(|line| Self {
            line,
            column: position.position(),
        })
    }
}

impl fmt::Display for SourcePosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.column {
            Some(column) => write!(f, "line {}, column {column}", self.line),
            None => write!(f, "line {}", self.line),
        }
    }
}

/// The ` (line L, column C)` suffix of a message, or nothing.
fn at(position: &Option<SourcePosition>) -> String {
    position.map_or_else(String::new, |position| format!(" ({position})"))
}

/// Why a binding could not produce a value.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BindError {
    /// The source is not a single expression: a syntax error, a statement, an
    /// assignment, a loop, or a name that is not `state`, `props` or `env`.
    #[error("syntax error: {message}{}", at(.position))]
    Syntax {
        /// What is wrong.
        message: String,
        /// Where, in the expression.
        position: Option<SourcePosition>,
    },
    /// Evaluation failed: a missing key, a type error, a division by zero, ...
    #[error("{message}{}", at(.position))]
    Runtime {
        /// What went wrong.
        message: String,
        /// Where, in the expression.
        position: Option<SourcePosition>,
    },
    /// The expression hit a resource limit (operations, call depth or data
    /// size) and was stopped.
    #[error("{message}{}", at(.position))]
    Limit {
        /// Which limit.
        message: String,
        /// Where, in the expression.
        position: Option<SourcePosition>,
    },
    /// The expression produced something that is not a property value.
    #[error(transparent)]
    Value(#[from] ValueError),
}

impl BindError {
    pub(crate) fn from_parse(error: &ParseError) -> Self {
        Self::Syntax {
            message: error.err_type().to_string(),
            position: SourcePosition::from_rhai(error.position()),
        }
    }

    pub(crate) fn from_eval(error: &EvalAltResult) -> Self {
        let cause = error.unwrap_inner();
        let position = SourcePosition::from_rhai(cause.position())
            .or_else(|| SourcePosition::from_rhai(error.position()));
        match cause {
            // The shared budget's stop (the engine's own counter reports
            // `ErrorTooManyOperations`; both are the operation limit).
            EvalAltResult::ErrorTooManyOperations(_) | EvalAltResult::ErrorTerminated(..) => {
                Self::Limit {
                    message: format!(
                        "the expression exceeded the limit of {MAX_OPERATIONS} operations"
                    ),
                    position,
                }
            }
            EvalAltResult::ErrorStackOverflow(_) => Self::Limit {
                message: format!(
                    "the expression exceeded the call depth limit of {MAX_CALL_LEVELS}"
                ),
                position,
            },
            EvalAltResult::ErrorDataTooLarge(what, _) => Self::Limit {
                message: format!("{what} exceeds the size limit"),
                position,
            },
            // A message the crate itself raised (a refusal): Rhai would
            // prefix it with "Runtime error:".
            EvalAltResult::ErrorRuntime(value, _) => Self::Runtime {
                message: value.to_string(),
                position,
            },
            other => {
                // Rhai appends its own position; ours is structured.
                let mut message = other.to_string();
                let suffix = format!(" ({})", other.position());
                if message.ends_with(&suffix) {
                    message.truncate(message.len() - suffix.len());
                }
                Self::Runtime { message, position }
            }
        }
    }
}
