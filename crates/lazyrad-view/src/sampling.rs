//! A renderer with a virtual clock, for golden tests of motion.

use crate::animation::{AnimationId, AnimationSampler, Frame};
use crate::patch::Patch;
use crate::renderer::{RecordingRenderer, Renderer};
use crate::value::Value;

/// A [`RecordingRenderer`] that also runs animations against a **virtual
/// clock**, so a test asserts animated values at fixed times (`t = 0`, `100`,
/// `250` ms) without any real time passing.
///
/// * [`Patch::Animate`] starts an [`Animation`](crate::Animation) at the
///   current virtual time, remembering the patch's id. `from: Current` starts
///   from the value [`value`](Self::value) reports; a newer animation on the
///   same node and property replaces the running one; a `Set`, an `Unset` or a
///   `Remove` of it cancels it.
/// * [`advance`](Self::advance) moves the clock and returns the sampled
///   [`Frame`], with the animations that ended in `finished` (each with its
///   id): report them to [`Session::finished`](crate::Session::finished), as a
///   real renderer does. A test that wants a renderer that **misses** a report
///   simply does not.
/// * [`live_value`](Renderer::live_value) answers with the animated value
///   while one runs, so the session sees what the widget shows.
///
/// It reads no real clock: time moves only through `advance`.
#[derive(Default)]
pub struct SamplingRenderer {
    recording: RecordingRenderer,
    sampler: AnimationSampler,
}

/// The number a [`Value`] holds, for the two numeric kinds.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(number) => Some(*number),
        // The animatable properties are small numbers; the widening is exact
        // for every integer a property can sensibly hold.
        Value::Int(number) => Some(*number as f64),
        _ => None,
    }
}

impl SamplingRenderer {
    /// A renderer at virtual time zero with nothing running.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every patch applied since the last [`take_trace`](Self::take_trace).
    #[must_use]
    pub fn patches(&self) -> &[Patch] {
        self.recording.patches()
    }

    /// The [trace](crate::trace) of [`patches`](Self::patches).
    #[must_use]
    pub fn trace(&self) -> String {
        self.recording.trace()
    }

    /// Returns the trace and forgets the recorded patches. Live values and
    /// running animations are kept.
    pub fn take_trace(&mut self) -> String {
        self.recording.take_trace()
    }

    /// Plays the user changing a widget: the live value of `node.property`
    /// becomes `value` without a patch, and an animation of it stops.
    pub fn set_live(&mut self, node: &str, property: &str, value: Value) {
        self.sampler.cancel(node, property);
        self.recording.set_live(node, property, value);
    }

    /// The virtual clock, in milliseconds.
    #[must_use]
    pub fn now_ms(&self) -> f64 {
        self.sampler.now_ms()
    }

    /// How many animations are running.
    #[must_use]
    pub fn running(&self) -> usize {
        self.sampler.len()
    }

    /// Whether an animation of `node.property` is running.
    #[must_use]
    pub fn is_running(&self, node: &str, property: &str) -> bool {
        self.sampler.value(node, property).is_some()
    }

    /// The id of the animation running on `node.property`, as the patch that
    /// started it numbered it.
    #[must_use]
    pub fn running_id(&self, node: &str, property: &str) -> Option<AnimationId> {
        self.sampler.id(node, property)
    }

    /// The number `node.property` shows now: the animated value while one
    /// runs, else the live value if it is numeric.
    #[must_use]
    pub fn value(&self, node: &str, property: &str) -> Option<f64> {
        self.sampler.value(node, property).or_else(|| {
            self.recording
                .live_value(node, property)
                .as_ref()
                .and_then(number)
        })
    }

    /// Moves the virtual clock forward by `dt_ms` and returns the animated
    /// values at the new time. The animations that ended are in
    /// [`Frame::finished`]; their final value is now the live one.
    pub fn advance(&mut self, dt_ms: f64) -> Frame {
        self.sampler.advance(dt_ms)
    }
}

impl Renderer for SamplingRenderer {
    fn apply(&mut self, patches: &[Patch]) {
        for patch in patches {
            match patch {
                Patch::Create { parent: None, .. } => self.sampler.clear(),
                // A widget that is created again starts from its properties.
                Patch::Create { node, .. } => self.sampler.cancel_node(node),
                Patch::Set { node, property, .. } | Patch::Unset { node, property } => {
                    self.sampler.cancel(node, property);
                }
                Patch::Remove { node } => {
                    for name in self.recording.subtree(node) {
                        self.sampler.cancel_node(&name);
                    }
                }
                Patch::Animate {
                    node, property, id, ..
                } => {
                    // Read before the recording applies `to`.
                    let current = self.value(node, property);
                    if let Some(animation) = patch.animation(current) {
                        self.sampler.start(node, property, *id, animation);
                    }
                }
                Patch::Move { .. } => {}
            }
            self.recording.apply(std::slice::from_ref(patch));
        }
    }

    fn live_value(&self, node: &str, property: &str) -> Option<Value> {
        match self.sampler.value(node, property) {
            Some(animated) => Some(Value::Float(animated)),
            None => self.recording.live_value(node, property),
        }
    }
}
