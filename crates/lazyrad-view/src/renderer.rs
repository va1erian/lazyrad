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
    ///
    /// An [`Animate`](Patch::Animate) is the renderer's to run, on its own
    /// clock. It must report each animation that runs to its end by calling
    /// [`Session::finished`](crate::Session::finished) with the patch's `id`
    /// (and apply the patches that returns), because a node that is leaving
    /// is held until it does. A renderer that cannot animate a property
    /// applies `to` at once and reports it finished. The host should also
    /// tick [`Session::advance`](crate::Session::advance) from its frame
    /// callback, so a report that is missed does not hold a node for ever.
    ///
    /// Animation ids are unique **per session**, not per process. A host that
    /// swaps or re-creates the [`Session`](crate::Session) over the same
    /// renderer must discard the renderer's pending reports from the old
    /// session: a stale id of the old session can equal one of the new
    /// session's (its first animation is `#1`), and would be taken for it.
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
/// [`set_live`](Self::set_live) to play the user editing a widget. A `Create`
/// starts a widget from its own properties, and a `Remove` takes the values of
/// the node and of everything created under it.
///
/// It has no clock, so it is a renderer that cannot animate: an
/// [`Animate`](Patch::Animate) is recorded and its `to` is applied at once. It
/// does not report [`Session::finished`](crate::Session::finished) either, so
/// a test that animates calls it itself, or uses
/// [`SamplingRenderer`](crate::SamplingRenderer), which has a virtual clock.
#[derive(Clone, Debug, Default)]
pub struct RecordingRenderer {
    patches: Vec<Patch>,
    live: BTreeMap<(String, String), Value>,
    /// Each node's parent, from the `Create`s, so a `Remove` takes the
    /// subtree's values with it.
    parents: BTreeMap<String, String>,
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

    /// `node` and every node created under it, in no particular order.
    pub(crate) fn subtree(&self, node: &str) -> Vec<String> {
        let mut names = vec![node.to_owned()];
        let mut next = 0;
        while next < names.len() {
            let parent = names[next].clone();
            names.extend(
                self.parents
                    .iter()
                    .filter(|(_, above)| **above == parent)
                    .map(|(name, _)| name.clone()),
            );
            next += 1;
        }
        names
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
                    match parent {
                        // A new root replaces the whole rendered tree.
                        None => {
                            self.live.clear();
                            self.parents.clear();
                        }
                        // A created widget starts from its own properties.
                        Some(parent) => {
                            self.live.retain(|(name, _), _| name != node);
                            self.parents.insert(node.clone(), parent.clone());
                        }
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
                Patch::Animate {
                    node, property, to, ..
                } => {
                    // No clock: the animation is over before it starts.
                    self.live
                        .insert((node.clone(), property.clone()), Value::Float(*to));
                }
                Patch::Remove { node } => {
                    // The node and everything under it.
                    for name in self.subtree(node) {
                        self.live.retain(|(owner, _), _| *owner != name);
                        self.parents.remove(&name);
                    }
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
    use crate::animation::{AnimFrom, AnimationId};
    use crate::easing::Easing;

    fn create(node: &str, parent: Option<&str>, value: i64) -> Patch {
        Patch::Create {
            node: node.into(),
            parent: parent.map(str::to_owned),
            index: 0,
            kind: "Panel".into(),
            props: BTreeMap::from([("n".to_owned(), Value::Int(value))]),
        }
    }

    #[test]
    fn a_remove_takes_the_subtree_and_a_create_starts_clean() {
        let mut renderer = RecordingRenderer::new();
        renderer.apply(&[
            create("root", None, 0),
            create("card", Some("root"), 1),
            create("label", Some("card"), 2),
            Patch::Set {
                node: "label".into(),
                property: "stale".into(),
                value: Value::Int(9),
            },
        ]);
        renderer.apply(&[Patch::Remove {
            node: "card".into(),
        }]);
        assert_eq!(renderer.live_value("card", "n"), None);
        assert_eq!(renderer.live_value("label", "n"), None);
        assert_eq!(renderer.live_value("root", "n"), Some(Value::Int(0)));
        // The same name created again is a new widget: nothing is left over.
        renderer.apply(&[
            create("card", Some("root"), 5),
            create("label", Some("card"), 6),
        ]);
        assert_eq!(renderer.live_value("label", "stale"), None);
        assert_eq!(renderer.live_value("label", "n"), Some(Value::Int(6)));
    }

    #[test]
    fn an_animation_is_applied_at_once() {
        let mut renderer = RecordingRenderer::new();
        renderer.apply(&[Patch::Animate {
            node: "card".into(),
            property: "opacity".into(),
            from: AnimFrom::Value(0.0),
            to: 1.0,
            duration_ms: 350.0,
            delay_ms: 0.0,
            easing: Easing::expo_out(),
            id: AnimationId(1),
        }]);
        assert_eq!(
            renderer.live_value("card", "opacity"),
            Some(Value::Float(1.0))
        );
        assert_eq!(
            renderer.take_trace(),
            "animate card.opacity from=0.0 to=1.0 duration=350.0 delay=0.0 easing=expo_out(6.0)
"
        );
    }

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
