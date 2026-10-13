//! Helpers shared by the integration tests.
#![allow(dead_code)] // Each test file uses a different subset.

use lazyrad_view::{Environment, Node, Tree};
use rhai::{Dynamic, Map};

/// A state map from `(key, value)` pairs.
pub fn map(entries: &[(&str, Dynamic)]) -> Map {
    entries
        .iter()
        .map(|(key, value)| ((*key).into(), value.clone()))
        .collect()
}

pub fn text(value: &str) -> Dynamic {
    Dynamic::from(value.to_owned())
}

pub fn int(value: i64) -> Dynamic {
    Dynamic::from(value)
}

/// A phone-width environment (Compact).
pub fn phone() -> Environment {
    Environment::new(360.0, 640.0)
}

/// A validated tree, panicking on an invalid one.
pub fn tree(root: Node) -> Tree {
    Tree::new(root).expect("valid tree")
}
