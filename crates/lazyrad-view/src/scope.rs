//! The variables one evaluation sees.

use rhai::{Map, Scope};

use crate::error::BindError;
use crate::limits::MAX_STRING_SIZE;
use crate::poison::{mentions_poison, rename_poison};
use crate::visible::{View, long_strings};

/// How many hidden entries a diagnostic lists before saying how many more.
const LISTED: usize = 5;

/// The first few of `hidden`, comma separated, and how many were left out.
fn summary(hidden: &[String]) -> String {
    let mut text = hidden
        .iter()
        .take(LISTED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if hidden.len() > LISTED {
        text.push_str(&format!(", and {} more", hidden.len() - LISTED));
    }
    text
}

/// The variables one evaluation sees: exactly `state`, `props` and `env`, as
/// constants. Build it once per turn and reuse it for every binding of the
/// turn, so the maps are copied once.
///
/// Each map is a **deep, bounded copy**: nothing in it is shared with the host
/// (a binding cannot write through to the host's state, however the host
/// stores it), and the entries that do not fit the size limits keep their key
/// but hold an opaque value. A binding that uses one gets a diagnostic that
/// names it and its size. See the [module docs](crate::bindings) for the
/// rules.
pub struct BindingScope {
    pub(crate) scope: Scope<'static>,
    /// The entries that were removed, as `variable.path (size or reason)`.
    pub(crate) hidden: Vec<String>,
}

impl BindingScope {
    /// Copies the three maps in as read-only variables.
    #[must_use]
    pub fn new(state: &Map, props: &Map, env: &Map) -> Self {
        Self::from_views(
            &View::of("state", state),
            &View::of("props", props),
            &View::of("env", env),
        )
    }

    /// A scope over views a caller already built (a [`Session`](crate::Session)
    /// builds the state's once per turn to compare it with the last turn's).
    pub(crate) fn from_views(state: &View, props: &View, env: &View) -> Self {
        let mut scope = Scope::new();
        let mut hidden = Vec::new();
        for (name, view) in [("state", state), ("props", props), ("env", env)] {
            scope.push_constant(name, view.map.clone());
            hidden.extend(view.hidden.iter().cloned());
        }
        Self { scope, hidden }
    }

    /// The entries removed from what bindings can see because they do not fit
    /// the size limits, as `variable.path (size or reason)`. Empty for any
    /// state that fits.
    #[must_use]
    pub fn hidden(&self) -> &[String] {
        &self.hidden
    }

    /// Adds to `error` what only the scope knows: which entries are hidden
    /// when Rhai's own message is about the type that stands in for one, and
    /// which strings are over the limit when Rhai refused one.
    pub(crate) fn explain(&self, error: &mut BindError) {
        match error {
            BindError::Runtime { message, .. } => {
                if mentions_poison(message) && !self.hidden.is_empty() {
                    *message = rename_poison(message);
                    message.push_str("; hidden from bindings because it is too large: ");
                    message.push_str(&summary(&self.hidden));
                }
            }
            BindError::Limit { message, .. } if message.contains("string") => {
                let mut found = Vec::new();
                for name in ["state", "props", "env"] {
                    let Some(value) = self.scope.get(name) else {
                        continue;
                    };
                    if let Some(map) = value.read_lock::<Map>() {
                        found.extend(long_strings(name, &map, 3));
                    }
                }
                if !found.is_empty() {
                    message.push_str(&format!(
                        "; strings over the {MAX_STRING_SIZE}-byte limit: {}",
                        found.join(", ")
                    ));
                }
            }
            _ => {}
        }
    }
}
