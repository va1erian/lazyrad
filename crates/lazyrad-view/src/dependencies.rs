//! Which variables a binding expression mentions.

/// Which of the three variables an expression mentions.
///
/// This is a *lexical* scan of the source for the identifiers, so it can
/// over-report (a property called `env`, or the word inside a string) but never
/// under-report: a variable can only be read by naming it. Over-reporting costs
/// an extra evaluation, never a stale value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dependencies {
    /// The expression may read `state`.
    pub state: bool,
    /// The expression may read `props`.
    pub props: bool,
    /// The expression may read `env`.
    pub env: bool,
}

impl Dependencies {
    /// Scans `source`.
    #[must_use]
    pub fn scan(source: &str) -> Self {
        let mut deps = Self::default();
        let identifiers = source
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|run| !run.is_empty());
        for identifier in identifiers {
            match identifier {
                "state" => deps.state = true,
                "props" => deps.props = true,
                "env" => deps.env = true,
                _ => {}
            }
        }
        deps
    }

    /// Whether the expression reads anything a handler turn can change.
    #[must_use]
    pub const fn reads_state_or_props(&self) -> bool {
        self.state || self.props
    }
}
