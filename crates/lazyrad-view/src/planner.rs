//! The motion planner's bookkeeping: what is running, what is entering,
//! leaving or pressed, and the one place an animation is decided.
//!
//! A [`Session`](crate::Session) owns a [`Planner`] and consults it wherever a
//! change could animate. The planner never sees a clock: an animation counts
//! as running from its patch until the renderer reports it finished (or a
//! newer patch replaces it).

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::animation::{AnimFrom, clamp_ms};
use crate::easing::Easing;
use crate::limits::MAX_RUNNING_ANIMATIONS;
use crate::patch::Patch;
use crate::renderer::Renderer;
use crate::value::Value;

/// The number a [`Value`] holds, for the two numeric kinds.
pub(crate) fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(number) => Some(*number),
        // Animatable properties are small numbers; the widening is exact for
        // every integer one can sensibly hold.
        Value::Int(number) => Some(*number as f64),
        _ => None,
    }
}

/// What the renderer shows right now, when the turn has a renderer to ask.
#[derive(Clone, Copy)]
pub(crate) struct Live<'a>(pub Option<&'a dyn Renderer>);

impl Live<'_> {
    /// No renderer: nothing is known.
    pub fn none() -> Live<'static> {
        Live(None)
    }

    pub fn value(&self, node: &str, property: &str) -> Option<Value> {
        self.0?.live_value(node, property)
    }

    pub fn number(&self, node: &str, property: &str) -> Option<f64> {
        self.value(node, property).as_ref().and_then(number)
    }
}

/// How a node leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExitKind {
    /// It is out of the tree: `Remove` when the exit ends.
    Remove,
    /// Its `visible` became false: `Set visible = false` when the exit ends.
    Hide,
}

/// An exit in progress.
pub(crate) struct Exit {
    pub kind: ExitKind,
    /// The properties whose end the node still waits for.
    pub pending: BTreeSet<String>,
    /// Every property the exit animates (restored, or brought back, later).
    pub animated: BTreeSet<String>,
}

/// Where an animation starts, as the planner sees it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Origin {
    /// Where the core believes the property rests (its last applied value).
    /// The patch starts from `Current` instead when one is already running
    /// there, and nothing is started when the renderer already shows the
    /// target.
    Belief(f64),
    /// A start the preset dictates (an `enter` begins transparent): the node
    /// jumps there first, so the renderer showing the target proves nothing.
    Instruct(f64),
    /// Wherever the renderer is.
    Current,
}

/// One animation to plan.
#[derive(Clone, Copy)]
pub(crate) struct Request<'a> {
    pub node: &'a str,
    pub property: &'a str,
    pub origin: Origin,
    pub to: f64,
    pub duration_ms: f64,
    pub delay_ms: f64,
    pub easing: Easing,
}

/// What the planner decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// An `Animate` patch was pushed and the animation is running.
    Started,
    /// It cannot or must not take time (nothing to move between, no time to
    /// move in, the limit is reached): the caller applies the value at once.
    AtOnce,
    /// There is nothing to do: the property is already where it is going.
    Noop,
}

/// The planner's state.
#[derive(Default)]
pub(crate) struct Planner {
    /// Names that have entered. Survives `reset` (limited there to the names
    /// the tree has), so a rebuild or a hot reload does not replay an `enter`.
    /// A name leaves it when its node leaves the tree, so a node that comes
    /// back is new and enters again.
    pub entered: HashSet<String>,
    /// `(node, property)` of every animation in flight.
    running: BTreeSet<(String, String)>,
    /// Exits in progress, by node.
    pub exits: HashMap<String, Exit>,
    /// Nodes that are not diffed because they are leaving.
    pub frozen: HashSet<String>,
    /// Nodes between `press_begin` and `press_end`.
    pub pressed: HashSet<String>,
    /// Animations that did not start because of the limit.
    pub degraded: u64,
}

impl Planner {
    /// Forgets everything in flight: what the renderer had is gone with the
    /// tree it discarded. `entered` stays.
    pub fn reset(&mut self) {
        self.running.clear();
        self.exits.clear();
        self.frozen.clear();
        self.pressed.clear();
    }

    /// How many animations are in flight.
    pub fn running(&self) -> usize {
        self.running.len()
    }

    /// Whether an animation of `node.property` is in flight.
    pub fn is_running(&self, node: &str, property: &str) -> bool {
        self.running
            .contains(&(node.to_owned(), property.to_owned()))
    }

    /// An instant change supersedes an animation of the property.
    pub fn stop(&mut self, node: &str, property: &str) {
        self.running.remove(&(node.to_owned(), property.to_owned()));
    }

    /// The renderer reported the animation finished.
    pub fn finish(&mut self, node: &str, property: &str) {
        self.stop(node, property);
    }

    /// Forgets a node that is gone.
    pub fn forget(&mut self, node: &str) {
        self.entered.remove(node);
        self.running.retain(|(name, _)| name != node);
        self.exits.remove(node);
        self.frozen.remove(node);
        self.pressed.remove(node);
    }

    /// Decides one animation and, when it runs, pushes its patch.
    ///
    /// 1. A start or target that is not finite, or a zero duration with no
    ///    delay, has nothing to move through: [`Outcome::AtOnce`].
    /// 2. With no animation in flight on the property, and a start that is a
    ///    *belief* or the renderer's own, a renderer that already shows the
    ///    target makes it [`Outcome::Noop`]; so does a start equal to the
    ///    target. (An animation in flight is mid-way, so what it shows proves
    ///    nothing, and a newer animation must replace it.)
    /// 3. A new animation past [`MAX_RUNNING_ANIMATIONS`] is
    ///    [`Outcome::AtOnce`], and counted in `degraded`.
    /// 4. Otherwise the patch is pushed, from `Current` if one was in flight,
    ///    and the property is running.
    pub fn animate(
        &mut self,
        request: &Request<'_>,
        live: &Live<'_>,
        out: &mut Vec<Patch>,
    ) -> Outcome {
        // `-0.0` and `0.0` are the same number; a trace should say one.
        let plain = |value: f64| if value == 0.0 { 0.0 } else { value };
        let duration = clamp_ms(request.duration_ms);
        let delay = clamp_ms(request.delay_ms);
        let key = (request.node.to_owned(), request.property.to_owned());
        let origin = match request.origin {
            Origin::Belief(value) => Origin::Belief(plain(value)),
            Origin::Instruct(value) => Origin::Instruct(plain(value)),
            Origin::Current => Origin::Current,
        };
        let request = &Request {
            to: plain(request.to),
            origin,
            ..*request
        };
        let from = match request.origin {
            Origin::Belief(value) | Origin::Instruct(value) => Some(value),
            Origin::Current => None,
        };
        if !request.to.is_finite()
            || from.is_some_and(|value| !value.is_finite())
            || (duration == 0.0 && delay == 0.0)
        {
            self.running.remove(&key);
            return Outcome::AtOnce;
        }
        let in_flight = self.running.contains(&key);
        if !in_flight {
            let at_target = live
                .number(request.node, request.property)
                .is_some_and(|shown| shown == request.to);
            let moves_nowhere = from.is_some_and(|value| value == request.to);
            let instructed = matches!(request.origin, Origin::Instruct(_));
            if moves_nowhere || (at_target && !instructed) {
                return Outcome::Noop;
            }
            if self.running.len() >= MAX_RUNNING_ANIMATIONS {
                self.degraded += 1;
                return Outcome::AtOnce;
            }
        }
        let from = match (request.origin, in_flight) {
            (Origin::Current, _) | (Origin::Belief(_), true) => AnimFrom::Current,
            (Origin::Belief(value) | Origin::Instruct(value), _) => AnimFrom::Value(value),
        };
        out.push(Patch::Animate {
            node: request.node.to_owned(),
            property: request.property.to_owned(),
            from,
            to: request.to,
            duration_ms: duration,
            delay_ms: delay,
            easing: request.easing,
        });
        self.running.insert(key);
        Outcome::Started
    }
}
