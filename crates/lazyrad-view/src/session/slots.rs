//! What a session remembers about each property of each node.

use std::collections::BTreeMap;

use rhai::Map;

use crate::bindings::Dependencies;
use crate::environment::Environment;
use crate::motion::identity;
use crate::node::Node;
use crate::planner::number;
use crate::size_class::{Responsive, SizeClass};
use crate::value::{Value, ValueKind};

/// What the session remembers about one property of one node.
pub(super) struct PropState {
    /// The fixed value kind, once known.
    pub kind: Option<ValueKind>,
    /// The binding's source; `None` for an unbound property.
    pub source: Option<String>,
    /// What the binding reads; `None` for an unbound property.
    pub deps: Option<Dependencies>,
    /// The value the renderer was last given. For an animated property this
    /// is the animation's *target*.
    pub applied: Option<Value>,
    /// The last value the binding produced successfully.
    pub good: Option<Value>,
    /// Whether the binding failed on the last turn that ran it.
    pub failed: bool,
}

impl PropState {
    pub fn new(node: &Node, property: &str) -> Self {
        let source = node.bind.get(property).cloned();
        Self {
            kind: node.literal_kind(property),
            deps: source.as_deref().map(Dependencies::scan),
            source,
            applied: None,
            good: None,
            failed: false,
        }
    }

    /// This property's state under `node`'s new definition: unchanged if its
    /// binding and kind are, otherwise fresh but still knowing what the
    /// renderer shows.
    fn carried(self, node: &Node, property: &str) -> Self {
        let fresh = Self::new(node, property);
        if self.source == fresh.source && (fresh.kind.is_none() || fresh.kind == self.kind) {
            self
        } else {
            Self {
                applied: self.applied,
                ..fresh
            }
        }
    }
}

/// A node's properties, by name.
pub(super) type Props = BTreeMap<String, PropState>;

/// The state of `node` carried over from `previous` (its state under the old
/// tree, if it had one). A property the node no longer has but that the
/// renderer shows stays, with no binding, so the next diff unsets it.
pub(super) fn carry(node: &Node, previous: Option<Props>) -> Props {
    let mut previous = previous.unwrap_or_default();
    let mut props = Props::new();
    for property in node.property_names() {
        let state = match previous.remove(property) {
            Some(old) => old.carried(node, property),
            None => PropState::new(node, property),
        };
        props.insert(property.to_owned(), state);
    }
    for (property, old) in previous {
        if old.applied.is_some() {
            props.insert(
                property,
                PropState {
                    kind: None,
                    source: None,
                    deps: None,
                    applied: old.applied,
                    good: None,
                    failed: false,
                },
            );
        }
    }
    props
}

/// The inputs of the last turn, which the next one is compared with.
pub(super) struct Last {
    pub env: Environment,
    pub state: Map,
}

/// The value a property should show: its binding's last good value, else the
/// responsive cascade for `class`, else the literal.
pub(super) fn effective(
    node: &Node,
    property: &str,
    prop: &PropState,
    class: SizeClass,
) -> Option<Value> {
    if let Some(good) = &prop.good {
        return Some(good.clone());
    }
    Responsive::new(&node.responsive)
        .property(class, property)
        .or_else(|| node.props.get(property))
        .cloned()
}

/// The number an animatable property rests at: its last applied value, or its
/// identity when nothing was applied.
pub(super) fn rest_value(props: &Props, property: &str) -> f64 {
    props
        .get(property)
        .and_then(|prop| prop.applied.as_ref())
        .and_then(number)
        .or_else(|| identity(property))
        .unwrap_or(0.0)
}

/// Whether a `visible` value shows the node: anything but `false`.
pub(super) fn shows(value: Option<&Value>) -> bool {
    !matches!(value, Some(Value::Bool(false)))
}
