//! Deciding whether the form state is the same as last turn's.
//!
//! The answer must be **conservative**: `true` only when the two maps are
//! certainly equal. A wrong `false` costs one extra evaluation; a wrong `true`
//! would leave a bound property stale.

use rhai::{Array, Dynamic, Map};

use crate::poison::Poison;

/// The deepest nesting of maps and arrays compared. Anything deeper is
/// treated as changed, so a hostile or accidental 100 000-level state cannot
/// overflow the stack here. Real form state nests a handful of levels.
const MAX_DEPTH: usize = 64;

/// Whether `a` and `b` hold exactly the same values.
///
/// Unknown types (custom types, function pointers), shared values and values
/// nested deeper than [`MAX_DEPTH`] are never equal, because their contents
/// can change behind a clone's back or cannot be compared safely.
pub(crate) fn same_map(a: &Map, b: &Map) -> bool {
    same_map_at(a, b, 0)
}

fn same_map_at(a: &Map, b: &Map, depth: usize) -> bool {
    a.len() == b.len()
        && a.iter().all(|(key, left)| {
            b.get(key)
                .is_some_and(|right| same_value(left, right, depth))
        })
}

fn same_array_at(a: &Array, b: &Array, depth: usize) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(left, right)| same_value(left, right, depth))
}

fn same_value(a: &Dynamic, b: &Dynamic, depth: usize) -> bool {
    if a.is_shared() || b.is_shared() {
        return false;
    }
    if a.is_unit() && b.is_unit() {
        return true;
    }
    // Two hidden entries at one place are the same to a binding: it cannot
    // read either.
    if a.is::<Poison>() && b.is::<Poison>() {
        return true;
    }
    if let (Ok(x), Ok(y)) = (a.as_bool(), b.as_bool()) {
        return x == y;
    }
    if let (Ok(x), Ok(y)) = (a.as_int(), b.as_int()) {
        return x == y;
    }
    if let (Ok(x), Ok(y)) = (a.as_float(), b.as_float()) {
        // Bit equality: NaN equals itself, and `0.0` is not `-0.0`.
        return x.to_bits() == y.to_bits();
    }
    if let (Ok(x), Ok(y)) = (a.as_char(), b.as_char()) {
        return x == y;
    }
    if let (Ok(x), Ok(y)) = (a.as_immutable_string_ref(), b.as_immutable_string_ref()) {
        return x.as_str() == y.as_str();
    }
    if depth >= MAX_DEPTH {
        return false;
    }
    if let (Some(x), Some(y)) = (a.read_lock::<Map>(), b.read_lock::<Map>()) {
        return same_map_at(&x, &y, depth + 1);
    }
    if let (Some(x), Some(y)) = (a.read_lock::<Array>(), b.read_lock::<Array>()) {
        return same_array_at(&x, &y, depth + 1);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: &[(&str, Dynamic)]) -> Map {
        entries
            .iter()
            .map(|(key, value)| ((*key).into(), value.clone()))
            .collect()
    }

    #[test]
    fn equal_scalars_nested_maps_and_arrays_are_the_same() {
        let make = || {
            map(&[
                ("count", Dynamic::from(3_i64)),
                ("name", Dynamic::from("Ada".to_owned())),
                ("ratio", Dynamic::from_float(0.5)),
                ("on", Dynamic::from(true)),
                ("nothing", Dynamic::UNIT),
                (
                    "user",
                    Dynamic::from_map(map(&[("id", Dynamic::from(1_i64))])),
                ),
                (
                    "tags",
                    Dynamic::from_array(vec![Dynamic::from("a".to_owned())]),
                ),
            ])
        };
        assert!(same_map(&make(), &make()));
    }

    #[test]
    fn any_difference_is_a_change() {
        let base = map(&[("count", Dynamic::from(1_i64))]);
        assert!(!same_map(&base, &map(&[("count", Dynamic::from(2_i64))])));
        // The integer 1 and the float 1.0 are different values.
        assert!(!same_map(
            &base,
            &map(&[("count", Dynamic::from_float(1.0))])
        ));
        assert!(!same_map(&base, &map(&[("other", Dynamic::from(1_i64))])));
        assert!(!same_map(&base, &Map::new()));
        let nested = |n: i64| {
            map(&[(
                "items",
                Dynamic::from_array(vec![Dynamic::from_map(map(&[("n", Dynamic::from(n))]))]),
            )])
        };
        assert!(same_map(&nested(1), &nested(1)));
        assert!(!same_map(&nested(1), &nested(2)));
    }

    #[test]
    fn very_deep_nesting_is_treated_as_changed_instead_of_recursing() {
        let nested = |levels: usize| {
            let mut value = Dynamic::from(1_i64);
            for _ in 0..levels {
                value = Dynamic::from_array(vec![value]);
            }
            map(&[("deep", value)])
        };
        assert!(same_map(&nested(10), &nested(10)));
        // Past MAX_DEPTH the answer is the conservative one, even for equal
        // values, and it is reached without walking the whole chain.
        assert!(!same_map(&nested(MAX_DEPTH + 40), &nested(MAX_DEPTH + 40)));
    }

    #[test]
    fn shared_and_unknown_values_are_never_the_same() {
        let mut shared = Dynamic::from(1_i64);
        shared = shared.into_shared();
        let a = map(&[("v", shared)]);
        assert!(!same_map(&a, &a.clone()));
        #[derive(Clone)]
        struct Opaque;
        let b = map(&[("v", Dynamic::from(Opaque))]);
        assert!(!same_map(&b, &b.clone()));
    }
}
