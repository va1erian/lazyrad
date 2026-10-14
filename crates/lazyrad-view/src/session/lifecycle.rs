//! The motion lifecycle: enter, exit, finish and the host's tick.

use std::collections::{BTreeMap, HashSet};

use super::Session;
use super::press::PRESSED;
use super::slots::{Props, rest_value};
use crate::animation::AnimationId;
use crate::easing::Easing;
use crate::motion::steps::{Phase, steps};
use crate::motion::{EXIT_MS, Preset};
use crate::patch::Patch;
use crate::planner::{Exit, ExitKind, Live, Origin, Outcome, Planner, Request};
use crate::value::Value;

/// The patch that puts `node.property` at its rest: the value last applied,
/// or `Unset` when nothing was.
fn rest_patch(node: &str, property: &str, props: &Props) -> Patch {
    match props.get(property).and_then(|prop| prop.applied.clone()) {
        Some(value) => Patch::Set {
            node: node.to_owned(),
            property: property.to_owned(),
            value,
        },
        None => Patch::Unset {
            node: node.to_owned(),
            property: property.to_owned(),
        },
    }
}

/// The patch that puts `node.property` at its rest as a `Set`: the value last
/// applied, or the property's identity when nothing was. What a property that
/// could not take the time to animate back gets, so it is never left where
/// the exit put it.
fn rest_set(node: &str, property: &str, props: &Props) -> Patch {
    let value = props
        .get(property)
        .and_then(|prop| prop.applied.clone())
        .unwrap_or_else(|| Value::Float(rest_value(props, property)));
    Patch::Set {
        node: node.to_owned(),
        property: property.to_owned(),
        value,
    }
}

/// Plans the exit of `name` and pushes its patches. Returns the properties
/// that are now animating (the ones whose end the node waits for, each with
/// the animation it waits for), or `None` when nothing takes time and the node
/// can leave at once.
pub(super) fn begin_exit(
    planner: &mut Planner,
    name: &str,
    preset: &Preset,
    reduced: bool,
    props: &Props,
    live: &Live<'_>,
    out: &mut Vec<Patch>,
) -> Option<BTreeMap<String, AnimationId>> {
    let mut animated = BTreeMap::new();
    for step in steps(preset, Phase::Exit, reduced) {
        let rest = rest_value(props, step.property);
        let outcome = planner.animate(
            &Request {
                node: name,
                property: step.property,
                origin: Origin::Belief(rest),
                to: step.away.relative_to(rest),
                duration_ms: step.duration_ms,
                delay_ms: 0.0,
                easing: step.easing,
            },
            live,
            out,
        );
        if let Outcome::Started(id) = outcome {
            animated.insert(step.property.to_owned(), id);
        }
    }
    (!animated.is_empty()).then_some(animated)
}

/// Plans the `enter` of `name`: each property starts at its preset's "away"
/// value (relative to where the node rests, for a slide) and ends at rest.
pub(super) fn begin_enter(
    planner: &mut Planner,
    name: &str,
    preset: &Preset,
    reduced: bool,
    props: &Props,
    live: &Live<'_>,
    out: &mut Vec<Patch>,
) {
    for step in steps(preset, Phase::Enter, reduced) {
        let rest = rest_value(props, step.property);
        planner.animate(
            &Request {
                node: name,
                property: step.property,
                origin: Origin::Instruct(step.away.relative_to(rest)),
                to: rest,
                duration_ms: step.duration_ms,
                delay_ms: 0.0,
                easing: step.easing,
            },
            live,
            out,
        );
    }
}

/// Cancels the exit of `name`: the properties it animated go back to rest from
/// wherever they are, and the node is diffed again. A property that cannot
/// take the time (the limit is reached, and its exit animation had ended) is
/// put at rest at once, so the node is never left transparent or offset.
pub(super) fn cancel_exit(
    planner: &mut Planner,
    name: &str,
    props: &Props,
    live: &Live<'_>,
    out: &mut Vec<Patch>,
) {
    planner.frozen.remove(name);
    let Some(exit) = planner.exits.remove(name) else {
        return;
    };
    for property in &exit.animated {
        let outcome = planner.animate(
            &Request {
                node: name,
                property,
                origin: Origin::Current,
                to: rest_value(props, property),
                duration_ms: EXIT_MS,
                delay_ms: 0.0,
                easing: Easing::expo_out(),
            },
            live,
            out,
        );
        if outcome == Outcome::AtOnce {
            out.push(rest_set(name, property, props));
        }
    }
}

impl Session {
    /// The renderer reports that animation `id` of `node.property` ran to its
    /// end. Returns the patches that follow from it, in order.
    ///
    /// This is what lets an **exit** finish: the node an exit holds is
    /// removed (or hidden) once every property the exit animates has been
    /// reported, and the patches that do it are what this returns: `Remove`
    /// for a node that left the tree, `Set visible = false` (and the animated
    /// properties back at rest, so the node can be shown again) for one that
    /// was hidden. Apply them like any other patches.
    ///
    /// `id` is the one the [`Animate`](Patch::Animate) patch carried. A report
    /// counts only if it names the **current** animation of that node and
    /// property, so these all return nothing and change nothing: an unknown
    /// node or property, a second report of the same animation, a report of
    /// an animation that a newer one replaced (an `enter` that ends after the
    /// node has started to leave, a quick hide, show and hide), and one that
    /// arrives after [`advance`](Self::advance) completed the animation by
    /// itself. None of them can remove a node, or hide one, early or twice.
    /// Reporting an animation that no exit waits for (an enter, a `change`)
    /// only lets the session forget it.
    ///
    /// Only an animation that ran to its end is reported: not one a newer
    /// patch replaced, and not one a `Set` or `Remove` cancelled.
    #[must_use = "the patches that follow from a finished animation must be applied"]
    pub fn finished(&mut self, node: &str, property: &str, id: AnimationId) -> Vec<Patch> {
        if !self.planner.finish(node, property, id) {
            return Vec::new();
        }
        let Some(exit) = self.planner.exits.get_mut(node) else {
            return Vec::new();
        };
        if exit.pending.get(property) != Some(&id) {
            return Vec::new();
        }
        exit.pending.remove(property);
        if !exit.pending.is_empty() {
            return Vec::new();
        }
        let Some(exit) = self.planner.exits.remove(node) else {
            return Vec::new();
        };
        self.complete_exit(node, &exit)
    }

    /// The host's frame tick: moves the session's own clock forward by
    /// `dt_ms` (a NaN or negative step is zero) and completes every animation
    /// that is overdue, exactly as [`finished`](Self::finished) would have, so
    /// a renderer that misses a report cannot hold a node, or a slot of
    /// [`MAX_RUNNING_ANIMATIONS`](crate::limits::MAX_RUNNING_ANIMATIONS), for
    /// the rest of the session. Returns the patches that follow (a `Remove`,
    /// or a `Set visible = false` and the properties back at rest); apply them
    /// like any other.
    ///
    /// An animation is overdue
    /// [`ANIMATION_GRACE_MS`](crate::limits::ANIMATION_GRACE_MS) after its
    /// delay and duration have passed on this clock. The clock reads no real
    /// time and moves only here, so **a host must call this** from its frame
    /// callback (the DOM renderer's `requestAnimationFrame`, the xui
    /// `MotionDriver`'s tick) with the time that passed. Without it only
    /// [`finished`](Self::finished) completes animations: that is what a test
    /// or a renderer that always reports can do, and a renderer that
    /// sometimes does not will leak.
    ///
    /// A renderer's late report of an animation completed here carries an id
    /// that is no longer current, and is ignored. (A step that is NaN,
    /// negative or infinite moves the clock by nothing.)
    #[must_use = "the patches that follow from an overdue animation must be applied"]
    pub fn advance(&mut self, dt_ms: f64) -> Vec<Patch> {
        self.planner.tick(dt_ms);
        let mut patches = Vec::new();
        for (node, property, id) in self.planner.overdue() {
            patches.extend(self.finished(&node, &property, id));
        }
        patches
    }

    /// Drops every running animation and every held node, putting what they
    /// moved at rest: the host's way to recover when it knows its renderer
    /// lost track (a page that was in the background, a restarted renderer).
    ///
    /// A node held by an exit leaves as if the exit had ended (`Remove`, or
    /// `Set visible = false`); every other animation in flight is replaced by
    /// a `Set` (or `Unset`) of the value the session last applied, and a node
    /// that is pressed is released at once. Nothing is running or held
    /// afterwards. The names that have entered are kept, so no `enter` plays
    /// again.
    #[must_use = "the patches that put the nodes at rest must be applied"]
    pub fn reset_motion(&mut self) -> Vec<Patch> {
        let mut patches = Vec::new();
        let mut leaving: Vec<String> = self.planner.exits.keys().cloned().collect();
        leaving.sort();
        for name in leaving {
            // A node under another one that left is already gone.
            if let Some(exit) = self.planner.exits.remove(&name) {
                patches.extend(self.complete_exit(&name, &exit));
            }
        }
        let mut done: HashSet<(String, String)> = patches
            .iter()
            .filter_map(|patch| match patch {
                Patch::Set { node, property, .. } | Patch::Unset { node, property } => {
                    Some((node.clone(), property.clone()))
                }
                _ => None,
            })
            .collect();
        let mut keys = self.planner.running_keys();
        let mut pressed: Vec<String> = self.planner.pressed.iter().cloned().collect();
        pressed.sort();
        for name in pressed {
            keys.extend(
                PRESSED
                    .iter()
                    .map(|property| (name.clone(), (*property).to_owned())),
            );
        }
        for (node, property) in keys {
            let Some(&slot) = self.index.get(node.as_str()) else {
                continue;
            };
            if done.insert((node.clone(), property.clone())) {
                patches.push(rest_patch(&node, &property, &self.slots[slot]));
            }
        }
        self.planner.reset();
        patches
    }

    /// The animation in flight on `node.property`, if there is one: the id the
    /// renderer reports it with.
    #[must_use]
    pub fn running_animation(&self, node: &str, property: &str) -> Option<AnimationId> {
        self.planner.id(node, property)
    }

    /// Whether `node` is held by an exit: leaving the tree, or fading out
    /// before it is hidden. A renderer can take such a node out of hit
    /// testing and focus while it finishes.
    #[must_use]
    pub fn is_leaving(&self, node: &str) -> bool {
        self.planner.frozen.contains(node)
    }

    /// An exit ran out: take the node away, or hide it.
    fn complete_exit(&mut self, node: &str, exit: &Exit) -> Vec<Patch> {
        match exit.kind {
            ExitKind::Remove => {
                self.drop_subtree(node);
                vec![Patch::Remove {
                    node: node.to_owned(),
                }]
            }
            ExitKind::Hide => {
                self.planner.frozen.remove(node);
                let mut patches = vec![Patch::Set {
                    node: node.to_owned(),
                    property: "visible".to_owned(),
                    value: Value::Bool(false),
                }];
                let Some(&slot) = self.index.get(node) else {
                    return patches;
                };
                let props = &mut self.slots[slot];
                if let Some(visible) = props.get_mut("visible") {
                    visible.applied = Some(Value::Bool(false));
                }
                // The node is hidden: put what the exit moved back at rest, so
                // showing it again starts from where it belongs.
                for property in &exit.animated {
                    self.planner.stop(node, property);
                    patches.push(rest_patch(node, property, props));
                }
                patches
            }
        }
    }

    /// Removes `name` and its subtree from the tree and the session.
    pub(super) fn drop_subtree(&mut self, name: &str) {
        let Some(&first) = self.index.get(name) else {
            return;
        };
        let removed = self.tree.remove_subtree(name);
        if removed.is_empty() {
            return;
        }
        self.slots.drain(first..first + removed.len());
        self.reindex();
        for gone in &removed {
            self.planner.forget(gone);
        }
    }
}
