#![forbid(unsafe_code)]

//! A script error located in its source file.
//!
//! Rhai reports a [`Position`] as a line and column but does not
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
    /// The script functions the failure travelled through, outermost first.
    ///
    /// Rhai wraps an error raised inside a called function in
    /// [`EvalAltResult::ErrorInFunctionCall`], one wrapper per call level. This
    /// unwraps that chain so a host can show `handler → helper → …`; it is
    /// empty for a failure that never left the function it happened in.
    pub call_chain: Vec<String>,
}

impl ScriptError {
    /// Creates an error from a file and a position.
    pub fn new(file: impl Into<String>, position: Position, message: impl Into<String>) -> Self {
        ScriptError {
            file: file.into(),
            line: position.line().unwrap_or(0),
            column: position.position().unwrap_or(0),
            message: message.into(),
            call_chain: Vec::new(),
        }
    }

    /// Locates a compile error.
    pub fn from_parse(file: impl Into<String>, error: &ParseError) -> Self {
        ScriptError::new(file, error.position(), error.to_string())
    }

    /// Locates a runtime error.
    ///
    /// The message and position come from the innermost error Rhai wrapped: a
    /// failure inside a called function would otherwise repeat "in call to
    /// function …" once per level and, when Rhai gave the outer call no
    /// position, report `0:0`. The unwrapped function names become
    /// [`call_chain`](ScriptError::call_chain).
    ///
    /// A script terminated through the progress hook carries its termination
    /// value in [`EvalAltResult::ErrorTerminated`]; Rhai's `Display` hides it,
    /// so the value becomes the message.
    pub fn from_eval(file: impl Into<String>, error: &EvalAltResult) -> Self {
        let (position, call_chain, innermost) = unwrap_call_chain(error);
        let message = eval_message(innermost);
        let mut located = ScriptError::new(file, position, message);
        located.call_chain = call_chain;
        located
    }
}

/// The innermost error of Rhai's call wrappers, the nearest non-`NONE`
/// position on the way, and the names of the functions it passed through.
fn unwrap_call_chain(error: &EvalAltResult) -> (Position, Vec<String>, &EvalAltResult) {
    let mut chain = Vec::new();
    let mut position = Position::NONE;
    let mut inner = error;
    loop {
        // Each step is deeper, so the last non-`NONE` position seen is the
        // innermost one; a deeper `NONE` leaves the nearer outer position.
        let here = inner.position();
        if !here.is_none() {
            position = here;
        }
        match inner {
            EvalAltResult::ErrorInFunctionCall(name, _, next, _) => {
                chain.push(name.clone());
                inner = next;
            }
            _ => break,
        }
    }
    (position, chain, inner)
}

/// The message of the innermost error, without Rhai's call wrappers.
fn eval_message(error: &EvalAltResult) -> String {
    match error {
        EvalAltResult::ErrorTerminated(value, _) => value.to_string(),
        // A runtime binding's own message: the position is already in the
        // error's line and column, so Rhai's "Runtime error: ... (line ..)"
        // wrapper would only repeat it.
        EvalAltResult::ErrorRuntime(value, _) if value.is_string() => value.to_string(),
        _ => error.to_string(),
    }
}

/// Formats a call chain, collapsing a run of the same function (`mute ×200`).
fn format_call_chain(chain: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut index = 0;
    while index < chain.len() {
        let name = &chain[index];
        let mut end = index + 1;
        while end < chain.len() && chain[end] == *name {
            end += 1;
        }
        match end - index {
            1 => parts.push(name.clone()),
            count => parts.push(format!("{name} ×{count}")),
        }
        index = end;
    }
    parts.join(" → ")
}

impl fmt::Display for ScriptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}: {}",
            self.file, self.line, self.column, self.message
        )?;
        if !self.call_chain.is_empty() {
            write!(formatter, " (in {})", format_call_chain(&self.call_chain))?;
        }
        Ok(())
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
        assert!(located.call_chain.is_empty());
        assert_eq!(
            located.to_string(),
            format!("frmMain.rhai:2:{}: {}", located.column, located.message)
        );
    }

    /// Runs `source`'s `outer` handler and returns the located error.
    fn eval_error(source: &str, outer: &str) -> ScriptError {
        let engine = rhai::Engine::new();
        let ast = engine.compile(source).expect("the script compiles");
        let mut scope = rhai::Scope::new();
        let error = engine
            .call_fn::<rhai::Dynamic>(&mut scope, &ast, outer, ())
            .expect_err("the handler fails");
        ScriptError::from_eval("main_form.rhai", &error)
    }

    #[test]
    fn a_nested_error_reports_the_innermost_message_and_the_chain() {
        let error = eval_error(
            "fn outer() { helper(); }\nfn helper() { boom(); }\nfn boom() { let d = 0; 1 / d }",
            "outer",
        );
        assert!(
            error.message.starts_with("Division by zero"),
            "the innermost message: {}",
            error.message
        );
        assert!(error.line > 0, "the innermost position is used");
        // Rhai does not wrap the entry function, only the calls it makes; the
        // host adds the entry itself (`EngineHost::call_with`).
        assert_eq!(error.call_chain, ["helper", "boom"]);
        assert_eq!(
            error.to_string(),
            format!(
                "main_form.rhai:{}:{}: {} (in helper → boom)",
                error.line, error.column, error.message
            )
        );
    }

    #[test]
    fn a_repeated_call_chain_is_collapsed() {
        let error = eval_error(
            "fn outer() { mute(3); }\nfn mute(n) { if n > 0 { mute(n - 1); } else { let d = 0; 1 / d } }",
            "outer",
        );
        let rendered = error.to_string();
        assert!(
            rendered.contains("(in mute ×4)"),
            "the run of `mute` is collapsed: {rendered}"
        );
    }

    #[test]
    fn an_error_with_no_call_chain_keeps_its_plain_display() {
        let error = ScriptError::new("main_form.rhai", Position::new(3, 21), "Stack overflow");
        assert!(error.call_chain.is_empty());
        assert_eq!(error.to_string(), "main_form.rhai:3:21: Stack overflow");
    }

    #[test]
    fn a_direct_error_has_no_call_chain() {
        let error = eval_error("fn outer() { let d = 0; 1 / d }", "outer");
        assert!(
            error.message.starts_with("Division by zero"),
            "{}",
            error.message
        );
        assert!(error.call_chain.is_empty());
        assert!(!error.to_string().contains("(in"), "{error}");
    }
}
