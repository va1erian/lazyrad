//! Press feedback: `PressScale` down, and back after a delay.

use super::Session;
use super::slots::rest_value;
use crate::easing::Easing;
use crate::motion::{PRESS_IN_MS, PRESS_RELEASE_DELAY_MS, PRESS_RELEASE_MS, PRESS_SCALE, Preset};
use crate::patch::Patch;
use crate::planner::{Live, Origin, Outcome, Request};
use crate::value::Value;

/// The scale properties a press animates.
pub(super) const PRESSED: [&str; 2] = ["scale_x", "scale_y"];

impl Session {
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
