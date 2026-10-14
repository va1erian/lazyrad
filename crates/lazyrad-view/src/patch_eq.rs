//! Equality of patches: exact, with NaN equal to NaN.

use crate::patch::Patch;

/// `a == b`, with NaN equal to NaN.
fn same_float(a: f64, b: f64) -> bool {
    a == b || (a.is_nan() && b.is_nan())
}

impl PartialEq for Patch {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Create {
                    node,
                    parent,
                    index,
                    kind,
                    props,
                },
                Self::Create {
                    node: node2,
                    parent: parent2,
                    index: index2,
                    kind: kind2,
                    props: props2,
                },
            ) => {
                node == node2
                    && parent == parent2
                    && index == index2
                    && kind == kind2
                    && props == props2
            }
            (
                Self::Set {
                    node,
                    property,
                    value,
                },
                Self::Set {
                    node: node2,
                    property: property2,
                    value: value2,
                },
            ) => node == node2 && property == property2 && value == value2,
            (
                Self::Unset { node, property },
                Self::Unset {
                    node: node2,
                    property: property2,
                },
            ) => node == node2 && property == property2,
            (
                Self::Animate {
                    node,
                    property,
                    from,
                    to,
                    duration_ms,
                    delay_ms,
                    easing,
                },
                Self::Animate {
                    node: node2,
                    property: property2,
                    from: from2,
                    to: to2,
                    duration_ms: duration_ms2,
                    delay_ms: delay_ms2,
                    easing: easing2,
                },
            ) => {
                node == node2
                    && property == property2
                    && from == from2
                    && same_float(*to, *to2)
                    && same_float(*duration_ms, *duration_ms2)
                    && same_float(*delay_ms, *delay_ms2)
                    && easing == easing2
            }
            (Self::Remove { node }, Self::Remove { node: node2 }) => node == node2,
            (
                Self::Move {
                    node,
                    parent,
                    index,
                },
                Self::Move {
                    node: node2,
                    parent: parent2,
                    index: index2,
                },
            ) => node == node2 && parent == parent2 && index == index2,
            _ => false,
        }
    }
}

impl Eq for Patch {}
