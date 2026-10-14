//! The validated node tree a session diffs.

use std::collections::HashSet;

use crate::motion::MotionProblem;
use crate::node::{MAX_TREE_DEPTH, Node};
use crate::value::{Value, ValueKind};

/// Why a [`Tree`] is invalid.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
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
    /// The tree nests deeper than [`MAX_TREE_DEPTH`].
    #[error("node `{name}` is nested {depth} levels deep; the limit is {max}")]
    TooDeep {
        /// The first node found beyond the limit.
        name: String,
        /// Its depth (the root is 1).
        depth: usize,
        /// The limit, [`MAX_TREE_DEPTH`].
        max: usize,
    },
    /// A `motion.change` entry names a property that is not animatable.
    ///
    /// Only `opacity`, the translations, the scales and `rotate` animate
    /// (see [`ANIMATABLE`](crate::motion::ANIMATABLE)); a layout property is
    /// the likeliest mistake, and the message says why it is refused.
    #[error(
        "`{node}` has a `change` transition for `{property}`, which cannot be animated: {}",
        crate::motion::why_not_animatable(*.layout)
    )]
    NotAnimatable {
        /// The node.
        node: String,
        /// The property named in `motion.change`.
        property: String,
        /// Whether it is a layout property (`width`, `gap`, `fill`, ...).
        layout: bool,
    },
    /// A preset is in a slot it does not belong to: `PressScale` is only a
    /// `press` preset, and a `press` preset must be `PressScale`.
    #[error("`{node}.motion.{slot}` is `{preset}`, which is not a {slot} preset")]
    WrongPreset {
        /// The node.
        node: String,
        /// `enter`, `exit` or `press`.
        slot: &'static str,
        /// The preset's name.
        preset: &'static str,
    },
    /// A number in a node's motion cannot be used: a NaN or negative duration
    /// or distance, an infinite distance, or a duration over the limit.
    #[error("`{node}.motion.{field}` {problem}")]
    BadMotionValue {
        /// The node.
        node: String,
        /// The field, such as `enter.distance` or
        /// `change.opacity.duration_ms`.
        field: String,
        /// What is wrong with it.
        problem: MotionProblem,
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

/// A validated view tree: node names are unique, every property keeps one
/// value kind and every node's motion is usable.
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
    /// [`TreeError::EmptyName`], [`TreeError::DuplicateName`],
    /// [`TreeError::MixedKinds`], [`TreeError::TooDeep`] or, for a node's
    /// [`motion`](Node::motion), [`TreeError::NotAnimatable`],
    /// [`TreeError::WrongPreset`] or [`TreeError::BadMotionValue`], for the
    /// first offending node in pre-order.
    pub fn new(root: Node) -> Result<Self, TreeError> {
        let mut names = HashSet::new();
        let mut stack = vec![(&root, 1_usize)];
        while let Some((node, depth)) = stack.pop() {
            if depth > MAX_TREE_DEPTH {
                return Err(TreeError::TooDeep {
                    name: node.name.clone(),
                    depth,
                    max: MAX_TREE_DEPTH,
                });
            }
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
            node.motion.validate(&node.name)?;
            stack.extend(node.children.iter().rev().map(|child| (child, depth + 1)));
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

    /// Removes the node called `name` and its subtree, and returns their names
    /// in pre-order. The root, and a name that is not in the tree, are not
    /// removed: the result is empty.
    pub(crate) fn remove_subtree(&mut self, name: &str) -> Vec<String> {
        if name == self.root.name || !self.names.contains(name) {
            return Vec::new();
        }
        // The child indexes from the root down to the node.
        let mut found: Option<Vec<usize>> = None;
        let mut stack: Vec<(&Node, Vec<usize>)> = vec![(&self.root, Vec::new())];
        while let Some((node, path)) = stack.pop() {
            if node.name == name {
                found = Some(path);
                break;
            }
            for (index, child) in node.children.iter().enumerate() {
                let mut child_path = path.clone();
                child_path.push(index);
                stack.push((child, child_path));
            }
        }
        let Some(path) = found else {
            return Vec::new();
        };
        let Some((last, parents)) = path.split_last() else {
            return Vec::new();
        };
        let mut parent = &mut self.root;
        for index in parents {
            parent = &mut parent.children[*index];
        }
        let removed = parent.children.remove(*last);
        let mut names = Vec::new();
        let mut pending = vec![&removed];
        while let Some(node) = pending.pop() {
            names.push(node.name.clone());
            pending.extend(node.children.iter().rev());
        }
        for removed_name in &names {
            self.names.remove(removed_name);
        }
        names
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
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if node.name == name {
            return Some(node);
        }
        stack.extend(node.children.iter());
    }
    None
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
    use crate::size_class::SizeClass;

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
