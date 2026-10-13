//! One node of a view tree and its builder.
//!
//! This is deliberately a small model of its own: the crate does not depend on
//! the form model, so a `.lfm` loader maps its document onto [`Node`]s.

use std::collections::{BTreeMap, BTreeSet};

use crate::size_class::SizeClass;
use crate::value::{Value, ValueKind};

/// The deepest tree [`Tree::new`](crate::Tree::new) accepts, counting the root
/// as depth 1.
///
/// Layouts nest a few dozen levels at most (a browser stops caring long before
/// this). The cap exists because `Node`'s derived `Clone`, `PartialEq` and
/// `Debug` recurse once per level; at this depth they stay a small fraction of
/// even the smallest main-thread stack in a debug build, so a validated tree is
/// always safe to copy and compare.
pub const MAX_TREE_DEPTH: usize = 128;

/// One node of a view tree.
///
/// A property resolves, lowest precedence first, from
///
/// 1. the literal in [`props`](Self::props),
/// 2. the [`responsive`](Self::responsive) patch cascaded for the current size
///    class,
/// 3. the [`bind`](Self::bind) expression, once it has produced a value.
///
/// # Dropping
///
/// A `Node` owns its children, and dropping a very deep chain recursively
/// would overflow the stack, so `Node` implements `Drop` to flatten the
/// subtree iteratively. The consequence for callers is that **fields cannot be
/// moved out of a `Node`** with a destructuring pattern or struct-update
/// syntax (`let Node { children, .. } = node` does not compile). Take them
/// through a `&mut` instead (`std::mem::take(&mut node.children)`) or clone.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Node {
    /// The unique name that addresses this node in patches.
    pub name: String,
    /// The node kind, for example `"Label"` or `"Column"`. Opaque to this crate.
    pub kind: String,
    /// Literal property values.
    pub props: BTreeMap<String, Value>,
    /// Property name to a Rhai expression over `state`, `props` and `env`.
    pub bind: BTreeMap<String, String>,
    /// Size class to a partial patch of property values, cascaded mobile-first
    /// (see [`Responsive`](crate::Responsive)).
    pub responsive: BTreeMap<SizeClass, BTreeMap<String, Value>>,
    /// Child nodes, in order.
    pub children: Vec<Node>,
}

impl Drop for Node {
    /// Drops the subtree without recursing, so a very deep chain (which
    /// [`Tree::new`](crate::Tree::new) rejects, and which is dropped on that
    /// error path) cannot overflow the stack.
    fn drop(&mut self) {
        let mut pending = std::mem::take(&mut self.children);
        while let Some(mut node) = pending.pop() {
            pending.append(&mut node.children);
        }
    }
}

impl Node {
    /// A node with no properties and no children.
    #[must_use]
    pub fn new(name: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
            props: BTreeMap::new(),
            bind: BTreeMap::new(),
            responsive: BTreeMap::new(),
            children: Vec::new(),
        }
    }

    /// Sets a literal property.
    #[must_use]
    pub fn with_prop(mut self, property: impl Into<String>, value: impl Into<Value>) -> Self {
        self.props.insert(property.into(), value.into());
        self
    }

    /// Binds a property to a Rhai expression.
    #[must_use]
    pub fn with_bind(mut self, property: impl Into<String>, expression: impl Into<String>) -> Self {
        self.bind.insert(property.into(), expression.into());
        self
    }

    /// Adds one property to the patch of a size class.
    #[must_use]
    pub fn with_responsive(
        mut self,
        class: SizeClass,
        property: impl Into<String>,
        value: impl Into<Value>,
    ) -> Self {
        self.responsive
            .entry(class)
            .or_default()
            .insert(property.into(), value.into());
        self
    }

    /// Appends a child.
    #[must_use]
    pub fn with_child(mut self, child: Node) -> Self {
        self.children.push(child);
        self
    }

    /// Every property this node sets or binds, sorted.
    pub(crate) fn property_names(&self) -> BTreeSet<&str> {
        let patched = self.responsive.values().flat_map(|patch| patch.keys());
        self.props
            .keys()
            .chain(patched)
            .chain(self.bind.keys())
            .map(String::as_str)
            .collect()
    }

    /// The kind a literal or patch fixes for `property`, if any sets it.
    pub(crate) fn literal_kind(&self, property: &str) -> Option<ValueKind> {
        self.props
            .get(property)
            .or_else(|| {
                self.responsive
                    .values()
                    .find_map(|patch| patch.get(property))
            })
            .map(Value::kind)
    }
}
