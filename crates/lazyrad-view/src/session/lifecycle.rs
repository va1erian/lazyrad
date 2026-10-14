//! The motion lifecycle: enter, exit, finish and press.

use std::collections::BTreeSet;

use super::Session;
use super::slots::{Props, rest_value};
use crate::easing::Easing;
use crate::motion::steps::{Phase, steps};
use crate::motion::{
    EXIT_MS, PRESS_IN_MS, PRESS_RELEASE_DELAY_MS, PRESS_RELEASE_MS, PRESS_SCALE, Preset,
};
use crate::patch::Patch;
use crate::planner::{Exit, ExitKind, Live, Origin, Outcome, Planner, Request};
use crate::value::Value;

/// The scale properties a press animates.
const PRESSED: [&str; 2] = ["scale_x", "scale_y"];

/// Plans the exit of `name` and pushes its patches. Returns the properties
/// that are now animating (the ones whose end the node waits for), or `None`
/// when nothing takes time and the node can leave at once.
pub(super) fn begin_exit(
    planner: &mut Planner,
    name: &str,
    preset: &Preset,
    reduced: bool,
    props: &Props,
    live: &Live<'_>,
    out: &mut Vec<Patch>,
) -> Option<BTreeSet<String>> {
    let mut animated = BTreeSet::new();
    for step in steps(preset, Phase::Exit, reduced) {
        let outcome = planner.animate(
            &Request {
                node: name,
                property: step.property,
                origin: Origin::Belief(rest_value(props, step.property)),
                to: step.away,
                duration_ms: step.duration_ms,
                delay_ms: 0.0,
                easing: step.easing,
            },
            live,
            out,
        );
        if outcome == Outcome::Started {
            animated.insert(step.property.to_owned());
        }
    }
    (!animated.is_empty()).then_some(animated)
}

/// Plans the `enter` of `name`: each property starts at its preset's "away"
/// value and ends at rest.
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
        planner.animate(
            &Request {
                node: name,
                property: step.property,
                origin: Origin::Instruct(step.away),
                to: rest_value(props, step.property),
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
/// wherever they are, and the node is diffed again.
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
        planner.animate(
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
    }
}

impl Session {
    /// The renderer reports that the animation of `node.property` ran to its
    /// end. Returns the patches that follow from it, in order.
    ///
    /// This is what lets an **exit** finish: the node an exit holds is
    /// removed (or hidden) once every property the exit animates has been
    /// reported, and the patches that do it are what this returns: `Remove`
    /// for a node that left the tree, `Set visible = false` (and the animated
    /// properties back at rest, so the node can be shown again) for one that
    /// was hidden. Apply them like any other patches.
    ///
    /// A report for an unknown node or property, a second report of the same
    /// animation and one for an animation that was cancelled all return
    /// nothing; none of them can remove a node twice. Reporting an animation
    /// that no exit waits for (an enter, a `change`) only lets the session
    /// forget it.
    #[must_use = "the patches that follow from a finished animation must be applied"]
    pub fn finished(&mut self, node: &str, property: &str) -> Vec<Patch> {
        self.planner.finish(node, property);
        let Some(exit) = self.planner.exits.get_mut(node) else {
            return Vec::new();
        };
        if !exit.pending.remove(property) || !exit.pending.is_empty() {
            return Vec::new();
        }
        let Some(exit) = self.planner.exits.remove(node) else {
            return Vec::new();
        };
        self.complete_exit(node, &exit)
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
                    let rest = props.get(property).and_then(|prop| prop.applied.clone());
                    patches.push(match rest {
                        Some(value) => Patch::Set {
                            node: node.to_owned(),
                            property: property.clone(),
                            value,
                        },
                        None => Patch::Unset {
                            node: node.to_owned(),
                            property: property.clone(),
                        },
                    });
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

    /// Feedback for a press of `node`: a node whose `press` is `PressScale`
    /// eases down to [`PRESS_SCALE`](crate::motion::PRESS_SCALE).
    ///
    /// Returns the patches (none for a node without a press preset, one that
    /// is leaving, one that is already pressed, an unknown name, a session
    /// that is not built, or when the environment asks for reduced motion).
    /// Call [`press_end`](Self::press_end) when the press is released or
    /// cancelled.
    #[must_use = "the patches of a press must be applied"]
    pub fn press_begin(&mut self, node: &str) -> Vec<Patch> {
        let Some(reduced) = self.last.as_ref().map(|last| last.env.reduce_motion) else {
            return Vec::new();
        };
        let Some(&slot) = self.index.get(node) else {
            return Vec::new();
        };
        let pressable = self
            .tree
            .get(node)
            .is_some_and(|found| matches!(found.motion.press, Some(Preset::PressScale)));
        if !pressable
            || reduced
            || self.planner.frozen.contains(node)
            || !self.planner.pressed.insert(node.to_owned())
        {
            return Vec::new();
        }
        let mut patches = Vec::new();
        for property in PRESSED {
            let rest = rest_value(&self.slots[slot], property);
            let to = rest * PRESS_SCALE;
            let outcome = self.planner.animate(
                &Request {
                    node,
                    property,
                    origin: Origin::Belief(rest),
                    to,
                    duration_ms: PRESS_IN_MS,
                    delay_ms: 0.0,
                    easing: Easing::expo_out(),
                },
                &Live::none(),
                &mut patches,
            );
            if outcome == Outcome::AtOnce {
                // The limit is reached: show the pressed state at once.
                patches.push(Patch::Set {
                    node: node.to_owned(),
                    property: property.to_owned(),
                    value: Value::Float(to),
                });
            }
        }
        patches
    }

    /// The press of `node` ended: it holds for
    /// [`PRESS_RELEASE_DELAY_MS`](crate::motion::PRESS_RELEASE_DELAY_MS), then
    /// eases back to rest over
    /// [`PRESS_RELEASE_MS`](crate::motion::PRESS_RELEASE_MS).
    ///
    /// Returns the patches; none unless [`press_begin`](Self::press_begin)
    /// started a press of `node`, so a release without a press, or a second
    /// one, is nothing.
    #[must_use = "the patches of a release must be applied"]
    pub fn press_end(&mut self, node: &str) -> Vec<Patch> {
        if !self.planner.pressed.remove(node) {
            return Vec::new();
        }
        let Some(&slot) = self.index.get(node) else {
            return Vec::new();
        };
        let mut patches = Vec::new();
        for property in PRESSED {
            let rest = rest_value(&self.slots[slot], property);
            let outcome = self.planner.animate(
                &Request {
                    node,
                    property,
                    origin: Origin::Belief(rest * PRESS_SCALE),
                    to: rest,
                    duration_ms: PRESS_RELEASE_MS,
                    delay_ms: PRESS_RELEASE_DELAY_MS,
                    easing: Easing::CubicOut,
                },
                &Live::none(),
                &mut patches,
            );
            if outcome == Outcome::AtOnce {
                patches.push(Patch::Set {
                    node: node.to_owned(),
                    property: property.to_owned(),
                    value: Value::Float(rest),
                });
            }
        }
        patches
    }
}
