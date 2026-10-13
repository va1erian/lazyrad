//! What one turn of a [`Session`](crate::Session) produces.

use std::fmt;

use crate::patch::Patch;

/// A binding that failed on a turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindDiagnostic {
    /// The node the binding belongs to.
    pub node: String,
    /// The bound property.
    pub property: String,
    /// What went wrong, with the position in the expression when known.
    pub message: String,
}

impl fmt::Display for BindDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}: {}", self.node, self.property, self.message)
    }
}

/// What one turn produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnResult {
    /// The changes to apply, in order.
    pub patches: Vec<Patch>,
    /// One entry per binding that failed this turn, in tree order.
    pub diagnostics: Vec<BindDiagnostic>,
}
