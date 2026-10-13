//! The variables one evaluation sees.

use rhai::{Map, Scope};

use crate::visible::{VISIBLE_BUDGET, View};

/// The variables one evaluation sees: exactly `state`, `props` and `env`, as
/// constants. Build it once per turn and reuse it for every binding of the
/// turn, so the maps are copied once.
///
/// Each map is copied as a bounded view: a variable holding more than
/// [`VISIBLE_BUDGET`] array elements and map entries in total has its largest
/// entries replaced by a placeholder, so no closure can copy a large list over
/// and over, and a million-row list is never copied at all. A binding that uses
/// a hidden entry gets a diagnostic naming it.
pub struct BindingScope {
    pub(crate) scope: Scope<'static>,
    /// The entries that were replaced, as `variable.key (size)`.
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

    /// The entries hidden from bindings because they were too large, as
    /// `variable.key (size)`. Empty for any state that fits.
    #[must_use]
    pub fn hidden(&self) -> &[String] {
        &self.hidden
    }

    /// The diagnostic for a binding that used a hidden entry.
    pub(crate) fn too_large(&self) -> String {
        format!(
            "a value in the state is too large for a binding to use \
             (limit {VISIBLE_BUDGET} elements and entries per variable): {}",
            self.hidden.join(", ")
        )
    }
}
