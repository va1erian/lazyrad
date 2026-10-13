//! The node tree a session diffs.
//!
//! This is deliberately a small model of its own: the crate does not depend on
//! the form model, so a `.lfm` loader maps its document onto [`Node`]s.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::size_class::SizeClass;
use crate::value::{Value, ValueKind};

/// One node of a view tree.
///
/// A property resolves, lowest precedence first, from
///
/// 1. the literal in [`props`](Self::props),
/// 2. the [`responsive`](Self::responsive) patch cascaded for the current size
///    class,
/// 3. the [`bind`](Self::bind) expression, once it has produced a value.
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

impl Node {
    /// A node with no properties and no children.
    #[must_use]
    pub fn new(name: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
            ..Self::default()
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

/// Why a [`Tree`] is invalid.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TreeError {
    /// A node has an empty name, which cannot address it in a patch.
    #[error("a `{kind}` node has an empty name")]
    EmptyName {
        /// The kind of the unnamed node.
        kind: String,
    },
    /// Two nodes share a name.
    #[error("node name `{name}` is used more than once")]
    DuplicateName {
        /// The repeated name.
        name: String,
    },
    /// A property is given values of different kinds by its literal and its
    /// responsive patches, so a size-class change would change its type.
    #[error("`{node}.{property}` is a {first} in one place and a {second} in another")]
    MixedKinds {
        /// The node.
        node: String,
        /// The property.
        property: String,
        /// The kind of the first value found (literal first, then patches in
        /// class order).
        first: ValueKind,
        /// The kind that disagrees.
        second: ValueKind,
    },
}

/// A node with its position, as [`Tree::placements`] lists them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placement<'a> {
    pub node: &'a Node,
    /// The parent's name; `None` for the root.
    pub parent: Option<&'a str>,
    /// The position among the parent's children.
    pub index: usize,
}

/// A validated view tree: node names are unique and every property keeps one
/// value kind.
#[derive(Clone, Debug, PartialEq)]
pub struct Tree {
    root: Node,
    names: HashSet<String>,
}

impl Tree {
    /// Validates `root` and wraps it.
    ///
    /// # Errors
    ///
    /// [`TreeError::EmptyName`], [`TreeError::DuplicateName`] or
    /// [`TreeError::MixedKinds`], for the first offending node in pre-order.
    pub fn new(root: Node) -> Result<Self, TreeError> {
        let mut names = HashSet::new();
        let mut stack = vec![&root];
        while let Some(node) = stack.pop() {
            if node.name.is_empty() {
                return Err(TreeError::EmptyName {
                    kind: node.kind.clone(),
                });
            }
            if !names.insert(node.name.clone()) {
                return Err(TreeError::DuplicateName {
                    name: node.name.clone(),
                });
            }
            check_kinds(node)?;
            stack.extend(node.children.iter().rev());
        }
        Ok(Self { root, names })
    }

    /// The root node.
    #[must_use]
    pub fn root(&self) -> &Node {
        &self.root
    }

    /// The number of nodes.
    #[must_use]
    #[allow(clippy::len_without_is_empty)] // A tree always has a root.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// The node called `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Node> {
        if self.names.contains(name) {
            find(&self.root, name)
        } else {
            None
        }
    }

    /// Every node in pre-order: parents before children, siblings in order.
    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.placements()
            .into_iter()
            .map(|placement| placement.node)
    }

    /// Whether a node called `name` exists.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// Every node in pre-order with its parent and sibling index. The position
    /// in the returned list is the node's pre-order index.
    pub(crate) fn placements(&self) -> Vec<Placement<'_>> {
        let mut placements = Vec::with_capacity(self.names.len());
        let mut stack = vec![Placement {
            node: &self.root,
            parent: None,
            index: 0,
        }];
        while let Some(placement) = stack.pop() {
            for (index, child) in placement.node.children.iter().enumerate().rev() {
                stack.push(Placement {
                    node: child,
                    parent: Some(placement.node.name.as_str()),
                    index,
                });
            }
            placements.push(placement);
        }
        placements
    }
}

/// The node called `name` at or below `node`.
fn find<'a>(node: &'a Node, name: &str) -> Option<&'a Node> {
    if node.name == name {
        return Some(node);
    }
    node.children.iter().find_map(|child| find(child, name))
}

/// Every literal and patch value of a property must have the same kind.
fn check_kinds(node: &Node) -> Result<(), TreeError> {
    for property in node.property_names() {
        let literal = node.props.get(property);
        let patched = node
            .responsive
            .values()
            .filter_map(|patch| patch.get(property));
        let mut values = literal.into_iter().chain(patched).map(Value::kind);
        if let Some(first) = values.next() {
            if let Some(second) = values.find(|kind| *kind != first) {
                return Err(TreeError::MixedKinds {
                    node: node.name.clone(),
                    property: property.to_owned(),
                    first,
                    second,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Node {
        Node::new("root", "Column")
            .with_child(
                Node::new("row", "Row")
                    .with_child(Node::new("a", "Label"))
                    .with_child(Node::new("b", "Label")),
            )
            .with_child(Node::new("c", "Label"))
    }

    #[test]
    fn walks_in_pre_order_with_parents_and_indexes() {
        let tree = Tree::new(sample()).unwrap();
        let order: Vec<_> = tree
            .placements()
            .iter()
            .map(|p| (p.node.name.as_str(), p.parent, p.index))
            .collect();
        assert_eq!(
            order,
            [
                ("root", None, 0),
                ("row", Some("root"), 0),
                ("a", Some("row"), 0),
                ("b", Some("row"), 1),
                ("c", Some("root"), 1),
            ]
        );
        assert_eq!(tree.len(), 5);
        assert!(tree.contains("b") && !tree.contains("x"));
        assert_eq!(tree.get("c").map(|n| n.kind.as_str()), Some("Label"));
    }

    #[test]
    fn duplicate_and_empty_names_are_typed_errors() {
        let duplicate = Node::new("root", "Column")
            .with_child(Node::new("same", "Label"))
            .with_child(Node::new("row", "Row").with_child(Node::new("same", "Edit")));
        assert_eq!(
            Tree::new(duplicate),
            Err(TreeError::DuplicateName {
                name: "same".into()
            })
        );
        let root_twice = Node::new("root", "Column").with_child(Node::new("root", "Label"));
        assert!(matches!(
            Tree::new(root_twice),
            Err(TreeError::DuplicateName { .. })
        ));
        let unnamed = Node::new("root", "Column").with_child(Node::new("", "Label"));
        assert_eq!(
            Tree::new(unnamed),
            Err(TreeError::EmptyName {
                kind: "Label".into()
            })
        );
    }

    #[test]
    fn a_property_cannot_change_kind_between_literal_and_patch() {
        let node = Node::new("n", "Flex").with_prop("fill", 0).with_responsive(
            SizeClass::Medium,
            "fill",
            1.5,
        );
        let err = Tree::new(node).unwrap_err();
        assert_eq!(
            err,
            TreeError::MixedKinds {
                node: "n".into(),
                property: "fill".into(),
                first: ValueKind::Int,
                second: ValueKind::Float
            }
        );
        // The same kind in every place is fine, and so is a patch-only property.
        let ok = Node::new("n", "Flex")
            .with_prop("fill", 0)
            .with_responsive(SizeClass::Medium, "fill", 1)
            .with_responsive(SizeClass::Large, "gap", 4);
        assert!(Tree::new(ok).is_ok());
    }
}
