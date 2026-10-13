//! The patches a session produces and any renderer applies.

use std::collections::BTreeMap;
use std::fmt;

use crate::value::Value;

/// One change to the rendered tree.
///
/// A [`Session`](crate::Session) produces only [`Create`](Self::Create),
/// [`Set`](Self::Set) and [`Unset`](Self::Unset). [`Remove`](Self::Remove) and
/// [`Move`](Self::Move) are part of the contract so a renderer can be written
/// against the final vocabulary, but nothing produces them yet: they arrive
/// with conditionals, repeaters and components.
///
/// Nodes are addressed by their unique name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Patch {
    /// A node appears, with the properties it starts with.
    ///
    /// A build lists every node, parents before children and siblings in
    /// order, so a renderer can create them as it reads. `index` is the
    /// position among the parent's children.
    Create {
        /// The new node's name.
        node: String,
        /// The parent's name; `None` for the root.
        parent: Option<String>,
        /// The position among the parent's children.
        index: usize,
        /// The node kind.
        kind: String,
        /// The initial property values (only the properties that have one).
        props: BTreeMap<String, Value>,
    },
    /// A property takes a new value.
    Set {
        /// The node's name.
        node: String,
        /// The property.
        property: String,
        /// The new value.
        value: Value,
    },
    /// A property that had a value no longer has one and returns to the
    /// default it has when nothing sets it.
    ///
    /// This is what a responsive property that exists only above some size
    /// class becomes when the window shrinks back below it.
    Unset {
        /// The node's name.
        node: String,
        /// The property.
        property: String,
    },
    /// A node and its subtree disappear. **Reserved**: not produced yet.
    Remove {
        /// The node's name.
        node: String,
    },
    /// A node moves under a parent, at a position. **Reserved**: not produced
    /// yet.
    Move {
        /// The node's name.
        node: String,
        /// The new parent's name.
        parent: String,
        /// The position among the new parent's children.
        index: usize,
    },
}

impl Patch {
    /// The name of the node this patch changes.
    #[must_use]
    pub fn node(&self) -> &str {
        match self {
            Self::Create { node, .. }
            | Self::Set { node, .. }
            | Self::Unset { node, .. }
            | Self::Remove { node }
            | Self::Move { node, .. } => node,
        }
    }
}

/// One trace line, with no trailing newline. The format is stable (it is what
/// golden tests compare):
///
/// ```text
/// create <node> <kind> parent=<parent|-> index=<n> {<prop>=<value>, ...}
/// set <node>.<property> = <value>
/// unset <node>.<property>
/// remove <node>
/// move <node> parent=<parent> index=<n>
/// ```
///
/// Properties of a `create` are sorted by name; values are written as
/// [`Value`]'s `Display` does.
impl fmt::Display for Patch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Create {
                node,
                parent,
                index,
                kind,
                props,
            } => {
                let parent = parent.as_deref().unwrap_or("-");
                write!(f, "create {node} {kind} parent={parent} index={index} {{")?;
                for (position, (property, value)) in props.iter().enumerate() {
                    if position > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{property}={value}")?;
                }
                f.write_str("}")
            }
            Self::Set {
                node,
                property,
                value,
            } => write!(f, "set {node}.{property} = {value}"),
            Self::Unset { node, property } => write!(f, "unset {node}.{property}"),
            Self::Remove { node } => write!(f, "remove {node}"),
            Self::Move {
                node,
                parent,
                index,
            } => write!(f, "move {node} parent={parent} index={index}"),
        }
    }
}

/// The trace of `patches`: one line per patch, each ending in a newline, and
/// the empty string for no patches.
#[must_use]
pub fn trace(patches: &[Patch]) -> String {
    let mut text = String::new();
    for patch in patches {
        text.push_str(&patch.to_string());
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_has_a_stable_trace_line() {
        let create = Patch::Create {
            node: "title".into(),
            parent: None,
            index: 0,
            kind: "Label".into(),
            props: BTreeMap::from([
                ("text".to_owned(), Value::Text("Hi".into())),
                ("bold".to_owned(), Value::Bool(true)),
            ]),
        };
        assert_eq!(
            create.to_string(),
            r#"create title Label parent=- index=0 {bold=true, text="Hi"}"#
        );
        let set = Patch::Set {
            node: "cards".into(),
            property: "direction".into(),
            value: Value::variant("Horizontal"),
        };
        assert_eq!(set.to_string(), "set cards.direction = Horizontal");
        let unset = Patch::Unset {
            node: "card".into(),
            property: "fill".into(),
        };
        assert_eq!(unset.to_string(), "unset card.fill");
        assert_eq!(
            trace(&[set, unset]),
            "set cards.direction = Horizontal\nunset card.fill\n"
        );
        assert_eq!(trace(&[]), "");
    }

    /// `Remove` and `Move` are produced by no session yet, but the vocabulary
    /// must be comparable and printable so renderers and traces can rely on it.
    #[test]
    fn reserved_patches_compare_and_print() {
        let remove = Patch::Remove { node: "old".into() };
        let moved = Patch::Move {
            node: "item".into(),
            parent: "list".into(),
            index: 2,
        };
        assert_eq!(remove, Patch::Remove { node: "old".into() });
        assert_ne!(remove, Patch::Remove { node: "new".into() });
        assert_eq!(
            moved,
            Patch::Move {
                node: "item".into(),
                parent: "list".into(),
                index: 2
            }
        );
        assert_ne!(
            moved,
            Patch::Move {
                node: "item".into(),
                parent: "list".into(),
                index: 3
            }
        );
        assert_eq!(format!("{remove:?}"), r#"Remove { node: "old" }"#);
        assert_eq!(
            format!("{moved:?}"),
            r#"Move { node: "item", parent: "list", index: 2 }"#
        );
        assert_eq!(remove.to_string(), "remove old");
        assert_eq!(moved.to_string(), "move item parent=list index=2");
        assert_eq!(remove.node(), "old");
        assert_eq!(moved.node(), "item");
    }
}
