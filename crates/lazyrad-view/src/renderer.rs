//! The seam between the view core and whatever draws it.

use std::collections::BTreeMap;

use crate::patch::{Patch, trace};
use crate::value::Value;

/// Something that turns [`Patch`]es into widgets, DOM nodes or any other
/// representation of the tree.
pub trait Renderer {
    /// Applies `patches` in order.
    ///
    /// A build's patches describe the whole tree (the first one is the root's
    /// `Create`), so a renderer discards what it had before reading them.
    fn apply(&mut self, patches: &[Patch]);

    /// The value `property` of `node` has in the rendered widget right now, if
    /// the renderer can tell.
    ///
    /// The default is `None`, which means "unknown": every `Set` is applied.
    ///
    /// [`Session::update_with`](crate::Session::update_with) uses this to skip
    /// a `Set` whose value the widget already shows. The case that matters is a
    /// text field the user is typing in: its handler copies the text to state,
    /// a binding sends the same text back, and writing it into the widget would
    /// reset the caret, the selection and an IME composition. A renderer
    /// should answer for the properties a user can edit (`text`, `checked`,
    /// `value`) and may leave the rest unknown.
    fn live_value(&self, node: &str, property: &str) -> Option<Value> {
        let _ = (node, property);
        None
    }
}

/// A [`Renderer`] that records every patch and keeps the property values a
/// widget would show, for tests of this crate and of crates built on it.
///
/// Its [`live_value`](Renderer::live_value) answers from those values, so it
/// behaves like a renderer whose widgets report what they display. Use
/// [`set_live`](Self::set_live) to play the user editing a widget.
#[derive(Clone, Debug, Default)]
pub struct RecordingRenderer {
    patches: Vec<Patch>,
    live: BTreeMap<(String, String), Value>,
}

impl RecordingRenderer {
    /// An empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every patch applied since the last [`take_trace`](Self::take_trace).
    #[must_use]
    pub fn patches(&self) -> &[Patch] {
        &self.patches
    }

    /// The [trace](crate::trace) of [`patches`](Self::patches).
    #[must_use]
    pub fn trace(&self) -> String {
        trace(&self.patches)
    }

    /// Returns the trace and forgets the recorded patches, so each turn of a
    /// golden test reads only its own lines. The live values are kept.
    pub fn take_trace(&mut self) -> String {
        let text = self.trace();
        self.patches.clear();
        text
    }

    /// Plays the user changing a widget: the live value of `node.property`
    /// becomes `value` without a patch.
    pub fn set_live(&mut self, node: &str, property: &str, value: Value) {
        self.live
            .insert((node.to_owned(), property.to_owned()), value);
    }
}

impl Renderer for RecordingRenderer {
    fn apply(&mut self, patches: &[Patch]) {
        for patch in patches {
            match patch {
                Patch::Create {
                    node,
                    parent,
                    props,
                    ..
                } => {
                    if parent.is_none() {
                        // A new root replaces the whole rendered tree.
                        self.live.clear();
                    }
                    for (property, value) in props {
                        self.live
                            .insert((node.clone(), property.clone()), value.clone());
                    }
                }
                Patch::Set {
                    node,
                    property,
                    value,
                } => {
                    self.live
                        .insert((node.clone(), property.clone()), value.clone());
                }
                Patch::Unset { node, property } => {
                    self.live.remove(&(node.clone(), property.clone()));
                }
                Patch::Remove { node } => {
                    self.live.retain(|(name, _), _| name != node);
                }
                Patch::Move { .. } => {}
            }
            self.patches.push(patch.clone());
        }
    }

    fn live_value(&self, node: &str, property: &str) -> Option<Value> {
        self.live
            .get(&(node.to_owned(), property.to_owned()))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_traces_and_tracks_live_values() {
        let mut renderer = RecordingRenderer::new();
        renderer.apply(&[
            Patch::Create {
                node: "root".into(),
                parent: None,
                index: 0,
                kind: "Column".into(),
                props: BTreeMap::from([("gap".to_owned(), Value::Int(4))]),
            },
            Patch::Set {
                node: "root".into(),
                property: "gap".into(),
                value: Value::Int(8),
            },
        ]);
        assert_eq!(renderer.live_value("root", "gap"), Some(Value::Int(8)));
        assert_eq!(renderer.live_value("root", "nope"), None);
        assert_eq!(
            renderer.take_trace(),
            "create root Column parent=- index=0 {gap=4}\nset root.gap = 8\n"
        );
        assert_eq!(renderer.take_trace(), "");
        // The live values outlive the trace.
        assert_eq!(renderer.live_value("root", "gap"), Some(Value::Int(8)));
        renderer.apply(&[Patch::Unset {
            node: "root".into(),
            property: "gap".into(),
        }]);
        assert_eq!(renderer.live_value("root", "gap"), None);
    }

    #[test]
    fn a_new_root_replaces_the_live_tree() {
        let mut renderer = RecordingRenderer::new();
        renderer.set_live("gone", "text", Value::Text("x".into()));
        renderer.apply(&[Patch::Create {
            node: "root".into(),
            parent: None,
            index: 0,
            kind: "Column".into(),
            props: BTreeMap::new(),
        }]);
        assert_eq!(renderer.live_value("gone", "text"), None);
    }
}
