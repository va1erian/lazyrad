//! The patches a session produces and any renderer applies.

use std::collections::BTreeMap;
use std::fmt;

use crate::animation::{AnimFrom, Animation, AnimationId};
use crate::easing::Easing;
use crate::value::Value;

/// One change to the rendered tree.
///
/// A [`Session`](crate::Session) produces [`Create`](Self::Create),
/// [`Set`](Self::Set), [`Unset`](Self::Unset), [`Animate`](Self::Animate) and,
/// when a node leaves the tree, [`Remove`](Self::Remove).
/// [`Move`](Self::Move) is part of the contract so a renderer can be written
/// against the final vocabulary, but nothing produces it yet: it arrives with
/// repeaters and components.
///
/// Nodes are addressed by their unique name.
///
/// Equality is exact, except that a NaN equals a NaN (like
/// [`Value`](crate::Value)), so a patch holding one is comparable.
///
/// The vocabulary grows (repeaters and components bring more), so a `match`
/// needs a wildcard arm.
#[derive(Clone, Debug)]
#[non_exhaustive]
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
    /// class becomes when the window shrinks back below it. It applies at
    /// once, **except** for an animatable property of a node with a `change`
    /// transition for it: that one is an [`Animate`](Self::Animate) to the
    /// property's default (see [`identity`](crate::motion::identity)), like
    /// the `Set` that gave it a value was.
    Unset {
        /// The node's name.
        node: String,
        /// The property.
        property: String,
    },
    /// A numeric property moves to a value over time instead of jumping.
    ///
    /// Only the [animatable](crate::motion::ANIMATABLE) properties are
    /// animated. The planner emits this instead of a `Set` for a property that
    /// has a `change` transition, and for the presets of `enter`, `exit` and
    /// `press` (see [`motion`](crate::motion)).
    ///
    /// What a renderer does with it:
    ///
    /// * Hold `from` for `delay_ms`, then move to `to` over `duration_ms`
    ///   shaped by `easing`, and be at exactly `to` afterwards. The same curves
    ///   are available as [`Animation`] and [`Easing::ease`].
    /// * Start `from: Current` from the value the widget shows now. Any
    ///   animation already running on the same node and property is replaced,
    ///   whatever `from` is: the widget jumps to an explicit `from` first.
    /// * Apply `to` at once when it cannot animate that property.
    /// * Report each animation that ran **to its end** by calling
    ///   [`Session::finished`](crate::Session::finished) with the node, the
    ///   property and this patch's `id`: it is how an `exit` learns it may
    ///   remove its node. A renderer that applied `to` at once reports it too.
    ///   An animation that was replaced or cancelled is not reported, and a
    ///   report that carries an id that is no longer the current one of that
    ///   node and property is ignored, so a late event cannot complete an exit
    ///   that has only just started.
    /// * A renderer that misses a report is not stuck for ever when the host
    ///   also ticks [`Session::advance`](crate::Session::advance): the session
    ///   then completes an animation that is overdue by itself.
    ///
    /// A batch that creates a node and animates it is applied before the next
    /// frame is drawn, so a node that enters is never seen at its rest value
    /// first. A batch holds at most one `Animate` per node and property (the
    /// last one planned), so a renderer needs no ordering rule for them.
    ///
    /// The [trace](crate::trace) line leaves out the `id`: it is bookkeeping,
    /// numbered in order, and not part of what moves.
    Animate {
        /// The node's name.
        node: String,
        /// The animated property, one of [`ANIMATABLE`](crate::motion::ANIMATABLE).
        property: String,
        /// Where the animation starts.
        from: AnimFrom,
        /// The value it ends at.
        to: f64,
        /// How long the move takes, in milliseconds.
        duration_ms: f64,
        /// How long it holds `from` before it starts, in milliseconds.
        delay_ms: f64,
        /// The timing curve.
        easing: Easing,
        /// This animation's number, which the renderer hands back in
        /// [`Session::finished`](crate::Session::finished).
        id: AnimationId,
    },
    /// A node and its subtree disappear.
    ///
    /// Produced when a node leaves the tree (see
    /// [`Session::reconcile`](crate::Session::reconcile)), straight away or,
    /// for a node with an `exit` preset, once the exit has finished.
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
            | Self::Animate { node, .. }
            | Self::Remove { node }
            | Self::Move { node, .. } => node,
        }
    }
}

impl Patch {
    /// The [`Animation`] an [`Animate`](Self::Animate) patch describes, for a
    /// renderer or a test that samples it. `current` is the value the widget
    /// shows now, which a `from: Current` starts from; without one it starts
    /// from `to` (there is nothing to animate). `None` for any other patch.
    #[must_use]
    pub fn animation(&self, current: Option<f64>) -> Option<Animation> {
        let Self::Animate {
            from,
            to,
            duration_ms,
            delay_ms,
            easing,
            ..
        } = self
        else {
            return None;
        };
        let from = match from {
            AnimFrom::Value(value) => *value,
            AnimFrom::Current => current.unwrap_or(*to),
        };
        Some(Animation {
            from,
            to: *to,
            duration_ms: *duration_ms,
            delay_ms: *delay_ms,
            easing: *easing,
        })
    }
}

/// One trace line, with no trailing newline. The format is stable (it is what
/// golden tests compare):
///
/// ```text
/// create <node> <kind> parent=<parent|-> index=<n> {<prop>=<value>, ...}
/// set <node>.<property> = <value>
/// unset <node>.<property>
/// animate <node>.<property> from=<current|n> to=<n> duration=<ms> delay=<ms> easing=<curve>
/// remove <node>
/// move <node> parent=<parent> index=<n>
/// ```
///
/// Properties of a `create` are sorted by name; values are written as
/// [`Value`]'s `Display` does. The numbers of an `animate` are written as Rust
/// floats (`40.0`, `0.5`) and the curve as [`Easing`]'s `Display` does
/// (`expo_out(6.0)`). The `id` of an `animate` is not written.
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
            Self::Animate {
                node,
                property,
                from,
                to,
                duration_ms,
                delay_ms,
                easing,
                id: _,
            } => {
                write!(f, "animate {node}.{property} from=")?;
                match from {
                    AnimFrom::Value(value) => write!(f, "{value:?}")?,
                    AnimFrom::Current => f.write_str("current")?,
                }
                write!(
                    f,
                    " to={to:?} duration={duration_ms:?} delay={delay_ms:?} easing={easing}"
                )
            }
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
