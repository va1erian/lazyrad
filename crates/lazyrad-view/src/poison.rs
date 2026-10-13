//! The value that stands in for an entry too large to show a binding.

use rhai::{Array, Dynamic, Map};

/// The deepest nesting searched for a [`Poison`]. Deeper values are never
/// produced by the view (it poisons anything nested too deeply).
const MAX_SEARCH_DEPTH: usize = 64;

/// The name a [`Poison`] has in Rhai's own messages ("Function not found:
/// len (<hidden entry>)").
pub(crate) const HIDDEN_TYPE_NAME: &str = "<hidden entry>";

/// The value that stands in for an entry too large to show a binding.
///
/// It is not data: it has no methods, properties or operators, so using it
/// fails, and the evaluator reports that with the entry's path and the reason
/// it was hidden. It counts as one entry toward the limits (its key's), and is
/// not shared, so Rhai's size check handles it.
#[derive(Clone, Debug)]
pub(crate) struct Poison {
    /// `state.cfg.rows`.
    pub path: String,
    /// Why: the limit it passed, in words.
    pub why: String,
}

impl Poison {
    /// The text a conversion returns. Rhai swallows an error raised by a
    /// conversion, so the evaluator does not rely on this text: the conversion
    /// also records a refusal, which fails the evaluation.
    pub(crate) fn text(&self) -> String {
        format!("<hidden entry {}>", self.path)
    }

    /// The diagnostic for using it for `what`.
    pub(crate) fn refusal(&self, what: &str) -> String {
        format!(
            "`{}` is hidden from bindings because it is too large ({}); it cannot be {what}",
            self.path, self.why
        )
    }
}

/// Whether a Rhai message talks about a [`Poison`], by the name Rhai gave it:
/// the registered one, or (in a few messages that skip the registry) the Rust
/// type's.
pub(crate) fn mentions_poison(message: &str) -> bool {
    message.contains(HIDDEN_TYPE_NAME) || message.contains(std::any::type_name::<Poison>())
}

/// `message` with the Rust name of [`Poison`] replaced by the registered one.
pub(crate) fn rename_poison(message: &str) -> String {
    message.replace(std::any::type_name::<Poison>(), HIDDEN_TYPE_NAME)
}

/// The first [`Poison`] in `value`, looking inside arrays and maps.
pub(crate) fn find_poison(value: &Dynamic, depth: usize) -> Option<Poison> {
    if let Some(poison) = value.read_lock::<Poison>() {
        return Some(poison.clone());
    }
    if depth > MAX_SEARCH_DEPTH {
        return None;
    }
    if let Some(map) = value.read_lock::<Map>() {
        return find_poison_in(&map, depth + 1);
    }
    if let Some(array) = value.read_lock::<Array>() {
        return array
            .iter()
            .find_map(|element| find_poison(element, depth + 1));
    }
    None
}

/// The first [`Poison`] among the values of `map`, looking deeper.
pub(crate) fn find_poison_in(map: &Map, depth: usize) -> Option<Poison> {
    map.values().find_map(|entry| find_poison(entry, depth))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poison(path: &str) -> Dynamic {
        Dynamic::from(Poison {
            path: path.into(),
            why: "more than a limit".into(),
        })
    }

    #[test]
    fn a_poison_is_found_inside_maps_and_lists() {
        let nested = Dynamic::from_map(
            [(
                "inner".into(),
                Dynamic::from_array(vec![Dynamic::from(1_i64), poison("state.x")]),
            )]
            .into_iter()
            .collect(),
        );
        assert_eq!(find_poison(&nested, 0).unwrap().path, "state.x");
        assert!(find_poison(&Dynamic::from(1_i64), 0).is_none());
        let plain = Dynamic::from_array(vec![Dynamic::from(1_i64)]);
        assert!(find_poison(&plain, 0).is_none());
    }

    #[test]
    fn both_of_rhais_spellings_are_recognised_and_normalised() {
        assert!(mentions_poison("Function not found: len (<hidden entry>)"));
        let rust = format!(
            "Data type incorrect: {} (expecting bool)",
            std::any::type_name::<Poison>()
        );
        assert!(mentions_poison(&rust));
        assert!(rename_poison(&rust).contains(HIDDEN_TYPE_NAME));
        assert!(!mentions_poison("Property not found: x"));
    }

    #[test]
    fn the_text_and_the_refusal_name_the_entry() {
        let poison = Poison {
            path: "state.cfg.rows".into(),
            why: "too many".into(),
        };
        assert_eq!(poison.text(), "<hidden entry state.cfg.rows>");
        assert_eq!(
            poison.refusal("compared"),
            "`state.cfg.rows` is hidden from bindings because it is too large (too many); it cannot be compared"
        );
    }
}
