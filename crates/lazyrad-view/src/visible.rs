//! The bounded view of `state`, `props` and `env` that bindings evaluate on.
//!
//! Rhai checks the size of a value when a call returns, not when a closure
//! hands one back to a native such as `array.map(...)`, which keeps every
//! result until it returns. A closure that returns a 100 000-element list from
//! `state` therefore copies it once per iteration. The only way to bound that
//! is to bound what a binding can *see*: every variable is copied into the
//! evaluation scope with its largest container entries replaced by a
//! placeholder once the whole variable would exceed [`VISIBLE_BUDGET`]
//! elements and entries. Scalars and strings are never hidden (strings are
//! immutable and shared, so copying one is free).

use rhai::{Array, Dynamic, Map};

/// The most array elements and map entries, summed over a whole variable and
/// everything nested in it, that a binding can see. A form's `state` is view
/// state, not a data set: a list longer than this belongs in a control that
/// owns its rows, with a count or a summary in `state`.
pub const VISIBLE_BUDGET: usize = 1024;

/// The deepest nesting walked. Anything deeper counts as over budget.
const MAX_WALK_DEPTH: usize = 64;

/// The name the placeholder has in Rhai's error messages.
pub(crate) const HIDDEN_TYPE_NAME: &str = "too-large value";

/// What a hidden entry is replaced with. It has no methods and no properties,
/// so any use of it is an error that names [`HIDDEN_TYPE_NAME`].
#[derive(Clone, Debug)]
pub(crate) struct Hidden;

/// Spends `left` on the elements and entries below `value`; `false` once it
/// runs out or the nesting is too deep.
fn within(value: &Dynamic, left: &mut usize, depth: usize) -> bool {
    if let Some(array) = value.read_lock::<Array>() {
        return all_within(array.iter(), left, depth);
    }
    if let Some(map) = value.read_lock::<Map>() {
        return all_within(map.values(), left, depth);
    }
    true
}

fn all_within<'a>(
    items: impl Iterator<Item = &'a Dynamic>,
    left: &mut usize,
    depth: usize,
) -> bool {
    if depth >= MAX_WALK_DEPTH {
        return false;
    }
    for item in items {
        if *left == 0 {
            return false;
        }
        *left -= 1;
        if !within(item, left, depth + 1) {
            return false;
        }
    }
    true
}

/// What bindings can see of one variable: the bounded copy, and the entries
/// that were hidden to make it fit.
#[derive(Clone, Debug, Default)]
pub(crate) struct View {
    pub map: Map,
    /// `label.key (size)` for each hidden entry.
    pub hidden: Vec<String>,
}

impl View {
    /// The bounded view of `source`, called `label` in diagnostics.
    pub(crate) fn of(label: &str, source: &Map) -> Self {
        let (map, hidden) = visible_copy(label, source);
        Self { map, hidden }
    }
}

/// A copy of `source` that fits [`VISIBLE_BUDGET`], and the entries that were
/// replaced to make it fit, as `label.key` with their size.
///
/// The largest entries are hidden first. The count of the walk is capped at
/// the budget, so a million-row list costs a thousand steps to refuse and is
/// never copied.
fn visible_copy(label: &str, source: &Map) -> (Map, Vec<String>) {
    // `Some(weight)` when an entry fits the budget on its own; `None` when it
    // is already past it, and is certainly one of the entries to hide.
    let weights: Vec<(&str, Option<usize>)> = source
        .iter()
        .map(|(key, value)| {
            let mut left = VISIBLE_BUDGET;
            let fits = within(value, &mut left, 0);
            (key.as_str(), fits.then_some(VISIBLE_BUDGET - left))
        })
        .collect();

    let mut hide: Vec<&str> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // An entry that does not fit on its own is always hidden.
    let mut total = source.len();
    let mut known: Vec<(&str, usize)> = Vec::new();
    for &(key, weight) in &weights {
        match weight {
            Some(weight) => {
                total += weight;
                known.push((key, weight));
            }
            None => {
                hide.push(key);
                notes.push(format!("{label}.{key} (over {VISIBLE_BUDGET} elements)"));
            }
        }
    }

    // Then the largest of the rest, until the variable fits.
    known.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    for (key, weight) in known {
        if total <= VISIBLE_BUDGET || weight == 0 {
            break;
        }
        total -= weight;
        hide.push(key);
        notes.push(format!("{label}.{key} ({weight} elements)"));
    }

    let copy = source
        .iter()
        .map(|(key, value)| {
            let hidden = hide.contains(&key.as_str());
            let value = if hidden {
                Dynamic::from(Hidden)
            } else {
                value.clone()
            };
            (key.clone(), value)
        })
        .collect();
    (copy, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(len: usize) -> Dynamic {
        Dynamic::from_array((0..len as i64).map(Dynamic::from).collect())
    }

    fn state(entries: &[(&str, Dynamic)]) -> Map {
        entries
            .iter()
            .map(|(key, value)| ((*key).into(), value.clone()))
            .collect()
    }

    #[test]
    fn a_small_state_is_copied_whole() {
        let source = state(&[("n", Dynamic::from(1_i64)), ("rows", list(100))]);
        let (copy, hidden) = visible_copy("state", &source);
        assert!(hidden.is_empty());
        assert_eq!(copy.len(), 2);
        assert!(copy["rows"].is::<Array>());
    }

    #[test]
    fn the_largest_entries_are_hidden_first_and_scalars_never() {
        let source = state(&[
            ("n", Dynamic::from(1_i64)),
            ("name", Dynamic::from("Ada".to_owned())),
            ("small", list(10)),
            ("big", list(800)),
            ("huge", list(5_000)),
        ]);
        let (copy, hidden) = visible_copy("state", &source);
        assert!(copy["huge"].is::<Hidden>());
        // 5 entries + 10 + 800 = 815 fits once `huge` is gone.
        assert!(copy["big"].is::<Array>() && copy["small"].is::<Array>());
        assert!(copy["n"].is::<i64>() && copy["name"].is_string());
        assert_eq!(hidden.len(), 1);
        assert!(hidden[0].starts_with("state.huge"), "{hidden:?}");
    }

    #[test]
    fn two_lists_that_fit_alone_but_not_together_hide_the_larger() {
        let source = state(&[("a", list(600)), ("b", list(700))]);
        let (copy, hidden) = visible_copy("state", &source);
        assert!(copy["a"].is::<Array>());
        assert!(copy["b"].is::<Hidden>());
        assert_eq!(hidden, ["state.b (700 elements)"]);
    }

    #[test]
    fn nesting_counts_and_a_very_deep_value_is_hidden() {
        // 40 maps of 30 entries each is 1200 entries nested in one value.
        let rows: Vec<Dynamic> = (0..40)
            .map(|_| {
                Dynamic::from_map(
                    (0..30)
                        .map(|i| (format!("k{i}").into(), Dynamic::from(i as i64)))
                        .collect(),
                )
            })
            .collect();
        let source = state(&[("table", Dynamic::from_array(rows))]);
        let (copy, hidden) = visible_copy("props", &source);
        assert!(copy["table"].is::<Hidden>());
        assert_eq!(hidden.len(), 1);

        let mut deep = Dynamic::from(1_i64);
        for _ in 0..(MAX_WALK_DEPTH + 10) {
            deep = Dynamic::from_array(vec![deep]);
        }
        let (copy, _) = visible_copy("state", &state(&[("deep", deep)]));
        assert!(copy["deep"].is::<Hidden>());
    }
}
