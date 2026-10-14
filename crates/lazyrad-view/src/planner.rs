//! The motion planner's bookkeeping: what is running, what is entering,
//! leaving or pressed, and the one place an animation is decided.
//!
//! A [`Session`](crate::Session) owns a [`Planner`] and consults it wherever a
//! change could animate. The planner reads no real clock. It numbers every
//! animation it starts ([`AnimationId`]), and an animation counts as running
//! from its patch until the renderer reports that id finished, a newer patch
//! on the same node and property replaces it, or the host's
//! [`Session::advance`](crate::Session::advance) tick finds it overdue.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::animation::{AnimFrom, AnimationId, clamp_ms};
use crate::easing::Easing;
use crate::limits::{ANIMATION_GRACE_MS, MAX_ANIMATION_MS, MAX_RUNNING_ANIMATIONS};
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
    /// The properties whose end the node still waits for, with the animation
    /// each one waits for: a report of any other id does not count.
    pub pending: BTreeMap<String, AnimationId>,
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
    /// jumps there first, so the renderer showing the target proves nothing,
    /// and an animation in flight is replaced rather than continued.
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
    /// An `Animate` patch was pushed and the animation, numbered so, is
    /// running.
    Started(AnimationId),
    /// It cannot or must not take time (nothing to move between, no time to
    /// move in, the limit is reached): the caller applies the value at once.
    AtOnce,
    /// There is nothing to do: the property is already where it is going.
    Noop,
}

impl Outcome {
    /// Whether nothing more is needed: an animation runs, or there is nothing
    /// to move.
    pub fn handled(self) -> bool {
        matches!(self, Self::Started(_) | Self::Noop)
    }
}

/// An animation in flight, as the planner knows it.
struct Running {
    id: AnimationId,
    /// The session clock at which the animation is overdue: its end plus the
    /// grace for a renderer to report it.
    due_ms: f64,
}

/// The planner's state.
#[derive(Default)]
pub(crate) struct Planner {
    /// Names that have entered. Survives `reset` (limited there to the names
    /// the tree has), so a rebuild or a hot reload does not replay an `enter`.
    /// A name leaves it when its node leaves the tree, so a node that comes
    /// back is new and enters again.
    pub entered: HashSet<String>,
    /// Every animation in flight, by node and property.
    running: BTreeMap<(String, String), Running>,
    /// Exits in progress, by node.
    pub exits: HashMap<String, Exit>,
    /// Nodes that are not diffed because they are leaving.
    pub frozen: HashSet<String>,
    /// Nodes between `press_begin` and `press_end`.
    pub pressed: HashSet<String>,
    /// Animations that did not start because of the limit.
    pub degraded: u64,
    /// The last id given. Never reset: ids are not reused in a session.
    last_id: u64,
    /// The session clock, which only [`tick`](Self::tick) moves.
    now_ms: f64,
}

impl Planner {
    /// Forgets everything in flight: what the renderer had is gone with the
    /// tree it discarded. `entered`, the ids and the clock stay.
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
        self.id(node, property).is_some()
    }

    /// The animation in flight on `node.property`.
    pub fn id(&self, node: &str, property: &str) -> Option<AnimationId> {
        self.running
            .get(&(node.to_owned(), property.to_owned()))
            .map(|running| running.id)
    }

    /// Every `(node, property)` in flight, in order.
    pub fn running_keys(&self) -> Vec<(String, String)> {
        self.running.keys().cloned().collect()
    }

    /// An instant change supersedes an animation of the property.
    pub fn stop(&mut self, node: &str, property: &str) {
        self.running.remove(&(node.to_owned(), property.to_owned()));
    }

    /// The renderer (or the host's tick) reports animation `id` of
    /// `node.property` finished. True, and the animation is no longer running,
    /// when `id` is the one in flight; false for anything else (a late,
    /// repeated or unknown report), which changes nothing.
    pub fn finish(&mut self, node: &str, property: &str, id: AnimationId) -> bool {
        let key = (node.to_owned(), property.to_owned());
        if self
            .running
            .get(&key)
            .is_some_and(|running| running.id == id)
        {
            self.running.remove(&key);
            true
        } else {
            false
        }
    }

    /// Moves the session clock forward (a NaN, negative or infinite step is
    /// zero: the clock never goes backwards and never reaches infinity).
    ///
    /// A step is cut to the longest an animation can be from its start to its
    /// overdue point (the longest delay and duration plus the grace): a longer
    /// one completes everything anyway, and a huge finite one (`f64::MAX`)
    /// would make `now + delay + duration + grace` equal `now`, so that every
    /// animation planned afterwards was overdue at once.
    pub fn tick(&mut self, dt_ms: f64) {
        if dt_ms.is_finite() && dt_ms > 0.0 {
            self.now_ms += dt_ms.min(2.0 * MAX_ANIMATION_MS + ANIMATION_GRACE_MS);
        }
    }

    /// The animations that are overdue at the session clock, in order.
    pub fn overdue(&self) -> Vec<(String, String, AnimationId)> {
        self.running
            .iter()
            .filter(|(_, running)| running.due_ms <= self.now_ms)
            .map(|((node, property), running)| (node.clone(), property.clone(), running.id))
            .collect()
    }

    /// Forgets a node that is gone.
    pub fn forget(&mut self, node: &str) {
        self.entered.remove(node);
        self.running.retain(|(name, _), _| name != node);
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
    /// 4. Otherwise the patch is pushed with the next id, from `Current` if a
    ///    `change`, `exit` or `press` found one in flight, and the property is
    ///    running. An exit that waits for this property now waits for this id.
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
        let in_flight = self.running.contains_key(&key);
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
        self.last_id += 1;
        let id = AnimationId(self.last_id);
        out.push(Patch::Animate {
            node: request.node.to_owned(),
            property: request.property.to_owned(),
            from,
            to: request.to,
            duration_ms: duration,
            delay_ms: delay,
            easing: request.easing,
            id,
        });
        self.running.insert(
            key,
            Running {
                id,
                due_ms: self.now_ms + delay + duration + ANIMATION_GRACE_MS,
            },
        );
        // An exit that waited for the animation this one replaced waits for
        // this one: it is the current animation of the property now.
        if let Some(waiting) = self
            .exits
            .get_mut(request.node)
            .and_then(|exit| exit.pending.get_mut(request.property))
        {
            *waiting = id;
        }
        Outcome::Started(id)
    }
}

/// Keeps only the last `Animate` for each node and property, so a batch tells
/// a renderer one thing about each: an `enter` after a `change`, or a cancel
/// followed by an exit, would otherwise arrive as two.
pub(crate) fn keep_last_animates(patches: &mut Vec<Patch>) {
    let mut last: HashMap<(&str, &str), usize> = HashMap::new();
    for (index, patch) in patches.iter().enumerate() {
        if let Patch::Animate { node, property, .. } = patch {
            last.insert((node.as_str(), property.as_str()), index);
        }
    }
    let keep: Vec<bool> = patches
        .iter()
        .enumerate()
        .map(|(index, patch)| match patch {
            Patch::Animate { node, property, .. } => {
                last.get(&(node.as_str(), property.as_str())) == Some(&index)
            }
            _ => true,
        })
        .collect();
    if keep.iter().all(|kept| *kept) {
        return;
    }
    let mut index = 0;
    patches.retain(|_| {
        let kept = keep[index];
        index += 1;
        kept
    });
}
