//! The bounded view of `state`, `props` and `env` that bindings evaluate on.
//!
//! Rhai checks the size of a value when a call returns, not when a closure
//! hands one back to a native such as `array.map(...)`, which keeps every
//! result until it returns. A closure that returns a 100 000-element list from
//! `state` therefore copies it once per iteration. The only way to bound that
//! is to bound what a binding can *see*, with the numbers Rhai itself enforces
//! ([`MAX_ARRAY_SIZE`], [`MAX_MAP_SIZE`], [`MAX_STRING_SIZE`], counted the way
//! Rhai counts: array elements and map entries summed over everything nested,
//! so a list of five-field rows costs six per row).
//!
//! Every variable is copied into the evaluation scope, deeply (no shared cell
//! of the host survives, so a binding cannot write through to the host's
//! state), and the copy is pruned until it fits:
//!
//! * a **map entry** that does not fit on its own is *descended into* if it is
//!   a map, so `cfg.name` survives a 2000-row `cfg.rows`, and removed if it is
//!   an array;
//! * if the whole variable still does not fit, the largest container entries
//!   are removed, largest first;
//! * a **string** is never removed, and a string that is a map entry does not
//!   count toward the variable: reading `state.doc` always works, and calling
//!   a method on one longer than [`MAX_STRING_SIZE`] is a diagnostic that
//!   names it.
//!
//! A removed entry is simply absent, so *any* use of it fails with "Property
//! not found", which [`View::hidden`] lets the evaluator extend with the reason
//! and the size.

use rhai::{Array, Blob, Dynamic, Map};

use crate::limits::{MAX_ARRAY_SIZE, MAX_MAP_SIZE, MAX_STRING_SIZE};

/// The deepest nesting walked. Anything deeper is treated as not fitting.
const MAX_WALK_DEPTH: usize = 32;

/// What Rhai counts in a value: array elements, map entries and string bytes,
/// each summed over everything nested.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Sizes {
    arrays: usize,
    maps: usize,
    strings: usize,
}

impl Sizes {
    fn within(self) -> bool {
        self.arrays <= MAX_ARRAY_SIZE
            && self.maps <= MAX_MAP_SIZE
            && self.strings <= MAX_STRING_SIZE
    }

    fn plus(self, other: Self) -> Self {
        Self {
            arrays: self.arrays.saturating_add(other.arrays),
            maps: self.maps.saturating_add(other.maps),
            strings: self.strings.saturating_add(other.strings),
        }
    }

    /// How much of the limits this uses, as a number to sort by.
    fn weight(self) -> u128 {
        let (a, m, s) = (
            MAX_ARRAY_SIZE as u128,
            MAX_MAP_SIZE as u128,
            MAX_STRING_SIZE as u128,
        );
        self.arrays as u128 * m * s + self.maps as u128 * a * s + self.strings as u128 * a * m
    }

    /// `512 array elements, 40 map entries`, leaving out what is zero.
    fn describe(self) -> String {
        let mut parts = Vec::new();
        if self.arrays > 0 {
            parts.push(format!("{} array elements", self.arrays));
        }
        if self.maps > 0 {
            parts.push(format!("{} map entries", self.maps));
        }
        if self.strings > 0 {
            parts.push(format!("{} bytes of text", self.strings));
        }
        parts.join(", ")
    }
}

/// Why a value does not fit on its own.
fn reason(sizes: Sizes) -> String {
    if sizes.arrays > MAX_ARRAY_SIZE {
        format!("more than {MAX_ARRAY_SIZE} array elements")
    } else if sizes.maps > MAX_MAP_SIZE {
        format!("more than {MAX_MAP_SIZE} map entries")
    } else if sizes.strings > MAX_STRING_SIZE {
        format!("more than {MAX_STRING_SIZE} bytes of text")
    } else {
        "nested too deeply".to_owned()
    }
}

/// Adds the sizes of what is inside `value` to `acc`, as Rhai's own accounting
/// does (`eval/data_check.rs`), and stops as soon as a limit is passed or the
/// nesting is too deep. `false` means it does not fit.
///
/// One deliberate difference: a string that is a direct entry of a map, with no
/// array above it, is **free**. It is never removed (reading it always works),
/// and a method call on it names it when Rhai refuses; counting it would only
/// hide the sibling entries next to a long text. A string that is an array
/// element, or sits in a map inside an array, counts, because nothing could
/// name it then.
fn measure(value: &Dynamic, acc: &mut Sizes, depth: usize, in_array: bool) -> bool {
    if depth > MAX_WALK_DEPTH {
        return false;
    }
    if let Some(array) = value.read_lock::<Array>() {
        for element in array.iter() {
            acc.arrays += 1;
            if !acc.within() || !measure(element, acc, depth + 1, true) {
                return false;
            }
        }
        return true;
    }
    if let Some(map) = value.read_lock::<Map>() {
        for entry in map.values() {
            acc.maps += 1;
            if !acc.within() || !measure(entry, acc, depth + 1, in_array) {
                return false;
            }
        }
        return true;
    }
    if let Some(blob) = value.read_lock::<Blob>() {
        acc.arrays += 1 + blob.len();
        return acc.within();
    }
    if let Ok(text) = value.as_immutable_string_ref() {
        if in_array {
            acc.strings += text.len();
        }
        return acc.within();
    }
    true
}

/// A deep copy of `value` in which no shared cell survives.
///
/// A host may hand over a `Dynamic` that is shared (`into_shared`); cloning it
/// would clone the *handle*, and a script could then write through it to the
/// host's own state. Rebuilding every array and map breaks that.
pub(crate) fn deep_copy(value: &Dynamic, depth: usize) -> Dynamic {
    if depth <= 2 * MAX_WALK_DEPTH {
        if let Some(array) = value.read_lock::<Array>() {
            return Dynamic::from_array(array.iter().map(|v| deep_copy(v, depth + 1)).collect());
        }
        if let Some(map) = value.read_lock::<Map>() {
            return Dynamic::from_map(
                map.iter()
                    .map(|(key, v)| (key.clone(), deep_copy(v, depth + 1)))
                    .collect(),
            );
        }
    }
    value.flatten_clone()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A number, a bool, a string, `()`: free, never removed.
    Plain,
    Array,
    Map,
}

fn kind_of(value: &Dynamic) -> Kind {
    if value.read_lock::<Map>().is_some() {
        Kind::Map
    } else if value.read_lock::<Array>().is_some() || value.read_lock::<Blob>().is_some() {
        Kind::Array
    } else {
        Kind::Plain
    }
}

enum Fate {
    Keep,
    Remove,
    /// A map that did not fit, replaced by its pruned self.
    Pruned(Map),
}

struct Entry<'a> {
    key: &'a str,
    value: &'a Dynamic,
    kind: Kind,
    /// What it holds; `None` if it does not fit on its own (and why).
    sizes: Result<Sizes, Sizes>,
    fate: Fate,
}

impl Entry<'_> {
    fn kept(&self) -> bool {
        !matches!(self.fate, Fate::Remove)
    }
}

/// `source`, pruned until it fits, and a note for each entry removed.
fn fit_map(path: &str, source: &Map, depth: usize, notes: &mut Vec<String>) -> Map {
    let mut entries: Vec<Entry<'_>> = source
        .iter()
        .map(|(key, value)| {
            let kind = kind_of(value);
            let mut sizes = Sizes::default();
            let fits = kind == Kind::Plain || measure(value, &mut sizes, depth + 1, false);
            Entry {
                key: key.as_str(),
                value,
                kind,
                sizes: if fits { Ok(sizes) } else { Err(sizes) },
                fate: Fate::Keep,
            }
        })
        .collect();

    // Pass 1: whatever does not fit alone. A map is descended into; an array
    // cannot be pruned (its indexes mean something), so it goes.
    for entry in &mut entries {
        if entry.sizes.is_ok() {
            continue;
        }
        let entry_path = format!("{path}.{}", entry.key);
        let pruned = match (entry.kind, entry.value.read_lock::<Map>()) {
            (Kind::Map, Some(map)) if depth < MAX_WALK_DEPTH => {
                Some(fit_map(&entry_path, &map, depth + 1, notes))
            }
            _ => None,
        };
        let Some(pruned) = pruned else {
            notes.push(format!(
                "{entry_path} ({})",
                reason(entry.sizes.unwrap_or_else(|sizes| sizes))
            ));
            entry.fate = Fate::Remove;
            continue;
        };
        let mut sizes = Sizes::default();
        let as_value = Dynamic::from_map(pruned);
        if measure(&as_value, &mut sizes, depth + 1, false) {
            let Some(pruned) = as_value.try_cast::<Map>() else {
                continue;
            };
            entry.sizes = Ok(sizes);
            entry.fate = Fate::Pruned(pruned);
        } else {
            notes.push(format!("{entry_path} ({})", reason(sizes)));
            entry.fate = Fate::Remove;
        }
    }

    // Pass 2: the whole map. Largest container entries first.
    let total = |entries: &[Entry<'_>]| {
        entries
            .iter()
            .filter(|entry| entry.kept())
            .fold(Sizes::default(), |acc, entry| {
                let own = Sizes {
                    maps: 1,
                    ..Sizes::default()
                };
                acc.plus(own).plus(entry.sizes.unwrap_or_else(|s| s))
            })
    };
    // Strings that are direct entries are free; only the containers' text and
    // the entry and element counts decide. A map that holds more scalar
    // entries than the limit cannot be fixed by removing containers, so the
    // entry count is only a reason to remove while removing can help.
    let mut used = total(&entries);
    let map_limit = MAX_MAP_SIZE.max(entries.iter().filter(|e| e.kept()).count());
    let fits = |used: Sizes| {
        used.arrays <= MAX_ARRAY_SIZE && used.maps <= map_limit && used.strings <= MAX_STRING_SIZE
    };
    if !fits(used) {
        let mut order: Vec<usize> = (0..entries.len())
            .filter(|&index| entries[index].kept() && entries[index].kind != Kind::Plain)
            .collect();
        // Largest first; among equals the later key goes, so the earliest
        // keys of a long list of entries survive.
        order.sort_by(|&a, &b| {
            let weight = |index: usize| entries[index].sizes.unwrap_or_else(|s| s).weight();
            weight(b)
                .cmp(&weight(a))
                .then_with(|| entries[b].key.cmp(entries[a].key))
        });
        for index in order {
            if fits(used) {
                break;
            }
            let sizes = entries[index].sizes.unwrap_or_else(|s| s);
            used = Sizes {
                arrays: used.arrays.saturating_sub(sizes.arrays),
                maps: used.maps.saturating_sub(sizes.maps + 1),
                strings: used.strings.saturating_sub(sizes.strings),
            };
            notes.push(format!(
                "{path}.{} ({})",
                entries[index].key,
                sizes.describe()
            ));
            entries[index].fate = Fate::Remove;
        }
    }

    entries
        .into_iter()
        .filter_map(|entry| {
            let key = source.get_key_value(entry.key)?.0.clone();
            match entry.fate {
                Fate::Keep => Some((key, deep_copy(entry.value, depth + 1))),
                Fate::Pruned(map) => Some((key, Dynamic::from_map(map))),
                Fate::Remove => None,
            }
        })
        .collect()
}

/// What bindings can see of one variable: the pruned copy, and a note for each
/// entry that was removed from it, as `variable.key (reason or size)`.
#[derive(Clone, Debug, Default)]
pub(crate) struct View {
    pub map: Map,
    pub hidden: Vec<String>,
}

impl View {
    /// The pruned, unshared view of `source`, called `label` in diagnostics.
    pub(crate) fn of(label: &str, source: &Map) -> Self {
        let mut hidden = Vec::new();
        let map = fit_map(label, source, 0, &mut hidden);
        Self { map, hidden }
    }
}

/// Up to `limit` strings in `map` longer than [`MAX_STRING_SIZE`], as
/// `path (N bytes)`. Only walked to explain an error.
pub(crate) fn long_strings(label: &str, map: &Map, limit: usize) -> Vec<String> {
    fn walk(value: &Dynamic, path: &str, depth: usize, found: &mut Vec<String>, limit: usize) {
        if found.len() >= limit || depth > MAX_WALK_DEPTH {
            return;
        }
        if let Some(map) = value.read_lock::<Map>() {
            for (key, entry) in map.iter() {
                walk(entry, &format!("{path}.{key}"), depth + 1, found, limit);
            }
        } else if let Some(array) = value.read_lock::<Array>() {
            for (index, element) in array.iter().enumerate() {
                walk(
                    element,
                    &format!("{path}[{index}]"),
                    depth + 1,
                    found,
                    limit,
                );
            }
        } else if let Ok(text) = value.as_immutable_string_ref() {
            if text.len() > MAX_STRING_SIZE {
                found.push(format!("{path} ({} bytes)", text.len()));
            }
        }
    }
    let mut found = Vec::new();
    walk(&Dynamic::from_map(map.clone()), label, 0, &mut found, limit);
    found
}

#[cfg(test)]
#[path = "visible_tests.rs"]
mod tests;
