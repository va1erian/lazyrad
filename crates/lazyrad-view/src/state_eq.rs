//! Deciding whether the form state is the same as last turn's.
//!
//! The answer must be **conservative**: `true` only when the two maps are
//! certainly equal. A wrong `false` costs one extra evaluation; a wrong `true`
//! would leave a bound property stale.

use rhai::{Array, Dynamic, Map};

/// Whether `a` and `b` hold exactly the same values.
///
/// Unknown types (custom types, function pointers) and shared values are never
/// equal, because their contents can change behind a clone's back.
pub(crate) fn same_map(a: &Map, b: &Map) -> bool {
    a.len() == b.len()
        && a.iter()
            .all(|(key, left)| b.get(key).is_some_and(|right| same_value(left, right)))
}

fn same_array(a: &Array, b: &Array) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(left, right)| same_value(left, right))
}

fn same_value(a: &Dynamic, b: &Dynamic) -> bool {
    if a.is_shared() || b.is_shared() {
        return false;
    }
    if a.is_unit() && b.is_unit() {
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
    if let (Some(x), Some(y)) = (a.read_lock::<Map>(), b.read_lock::<Map>()) {
        return same_map(&x, &y);
    }
    if let (Some(x), Some(y)) = (a.read_lock::<Array>(), b.read_lock::<Array>()) {
        return same_array(&x, &y);
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
