//! One animation as data, and a virtual clock to run them against.
//!
//! The core never reads a real clock (the platform guard test fails the build
//! if it does). A renderer owns time: it samples an [`Animation`] with the
//! elapsed time it has, and tests do the same with a hand-written number, so
//! two samplers cannot disagree. [`AnimationSampler`] is that, packaged: a set
//! of running animations and a clock that moves only when told to.

use std::collections::BTreeMap;

use crate::easing::Easing;
use crate::limits::MAX_ANIMATION_MS;

/// Where an animation starts.
///
/// Equality is exact, except that a NaN equals a NaN, like
/// [`Value`](crate::Value).
#[derive(Clone, Copy, Debug)]
pub enum AnimFrom {
    /// An explicit start value.
    Value(f64),
    /// Wherever the renderer's widget is right now.
    ///
    /// The core knows only the *target* it last applied, not the interpolated
    /// value, so an animation that interrupts another one starts from where
    /// the renderer is. When the renderer cannot tell, it starts from `to`
    /// (there is nothing to animate).
    Current,
}

impl PartialEq for AnimFrom {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Value(a), Self::Value(b)) => a == b || (a.is_nan() && b.is_nan()),
            (Self::Current, Self::Current) => true,
            _ => false,
        }
    }
}

impl Eq for AnimFrom {}

/// A duration or delay, as every sampler and planner reads it: NaN and
/// negative values are `0`, and anything above
/// [`MAX_ANIMATION_MS`](crate::limits::MAX_ANIMATION_MS) is cut to it.
#[must_use]
pub fn clamp_ms(ms: f64) -> f64 {
    if ms.is_nan() || ms <= 0.0 {
        0.0
    } else {
        ms.min(MAX_ANIMATION_MS)
    }
}

/// A value that moves from `from` to `to` over `duration_ms`, after waiting
/// `delay_ms`, shaped by `easing`.
///
/// An animation is a pure function of the time elapsed since it started: it
/// holds no clock and no state.
///
/// ```text
/// elapsed:  0 ........ delay ........... delay + duration ......
/// value:    from       from  ~eased~     to                 to
/// ```
///
/// The value stays between `from` and `to` (the order does not matter, and
/// either may be negative): the curves never overshoot and the endpoints are
/// returned exactly as given, not recomputed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Animation {
    /// The value before the delay ends, and at its start.
    pub from: f64,
    /// The value once the animation is [finished](Self::is_finished).
    pub to: f64,
    /// How long the move takes, once the delay has passed, in milliseconds.
    pub duration_ms: f64,
    /// How long the animation holds `from` before it starts, in milliseconds.
    pub delay_ms: f64,
    /// The timing curve of the move.
    pub easing: Easing,
}

impl Animation {
    /// A linear animation from `from` to `to` over `duration_ms`, starting at
    /// once.
    #[must_use]
    pub const fn new(from: f64, to: f64, duration_ms: f64) -> Self {
        Self {
            from,
            to,
            duration_ms,
            delay_ms: 0.0,
            easing: Easing::Linear,
        }
    }

    /// This animation after `delay_ms`.
    #[must_use]
    pub const fn with_delay(mut self, delay_ms: f64) -> Self {
        self.delay_ms = delay_ms;
        self
    }

    /// This animation with the curve `easing`.
    #[must_use]
    pub const fn with_easing(mut self, easing: Easing) -> Self {
        self.easing = easing;
        self
    }

    /// The elapsed time at which the animation ends, in milliseconds: the
    /// delay plus the duration, each read through [`clamp_ms`].
    #[must_use]
    pub fn end_ms(&self) -> f64 {
        clamp_ms(self.delay_ms) + clamp_ms(self.duration_ms)
    }

    /// Whether the animation has reached `to` at `elapsed_ms`: true from the
    /// end on, and so, for a zero duration, from the delay on. A NaN elapsed
    /// time is the start.
    #[must_use]
    pub fn is_finished(&self, elapsed_ms: f64) -> bool {
        let elapsed = if elapsed_ms.is_nan() { 0.0 } else { elapsed_ms };
        elapsed - clamp_ms(self.delay_ms) >= clamp_ms(self.duration_ms)
    }

    /// The value `elapsed_ms` after the animation started.
    ///
    /// * Before the delay has passed it is `from`.
    /// * At the end and after it, it is exactly `to`.
    /// * In between it is `from` moved towards `to` by the curve.
    /// * A duration or delay that is NaN or negative is `0`, and one above the
    ///   [limit](crate::limits::MAX_ANIMATION_MS) is cut to it. A **zero
    ///   duration** has no in-between: `from` until the delay, `to` from then.
    /// * An `elapsed_ms` of NaN is the start, and a negative one is before it.
    /// * If `from` or `to` is not finite there is nothing to interpolate, so
    ///   the animation holds `from` until it ends and then returns `to`, rather
    ///   than producing NaN from a finite pair.
    #[must_use]
    pub fn sample(&self, elapsed_ms: f64) -> f64 {
        let elapsed = if elapsed_ms.is_nan() { 0.0 } else { elapsed_ms };
        let running = elapsed - clamp_ms(self.delay_ms);
        let duration = clamp_ms(self.duration_ms);
        if running < 0.0 {
            return self.from;
        }
        if running >= duration {
            return self.to;
        }
        if !(self.from.is_finite() && self.to.is_finite()) {
            return self.from;
        }
        // `0 <= running < duration`, so the duration is not zero and `t` is in
        // `[0, 1)`. The curve is evaluated in `f32`, as xui's is.
        let eased = f64::from(self.easing.ease((running / duration) as f32));
        // A weighted sum rather than `from + (to - from) * eased`: the
        // difference overflows for endpoints of opposite sign near
        // `f64::MAX`, and this form is exact at `eased` of 0 and 1.
        self.from * (1.0 - eased) + self.to * eased
    }
}

/// One animated value at the time of a [`Frame`].
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// The node.
    pub node: String,
    /// The animated property.
    pub property: String,
    /// Its value at the frame's time.
    pub value: f64,
}

/// What [`AnimationSampler::advance`] returns: the animated values at the new
/// time and which animations ended there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    /// The sampler's clock after the step, in milliseconds.
    pub time_ms: f64,
    /// Every animation that was running during the step, in node then property
    /// order. One that ended at this step is included with its final value.
    pub samples: Vec<Sample>,
    /// The animations that ended at this step, in the same order. These are
    /// what a renderer reports to
    /// [`Session::finished`](crate::Session::finished).
    pub finished: Vec<(String, String)>,
}

impl Frame {
    /// The value of `node.property` in this frame, if it was animating.
    #[must_use]
    pub fn value(&self, node: &str, property: &str) -> Option<f64> {
        self.samples
            .iter()
            .find(|sample| sample.node == node && sample.property == property)
            .map(|sample| sample.value)
    }
}

struct Running {
    animation: Animation,
    started_ms: f64,
}

/// A virtual clock and the animations running against it.
///
/// Time moves only through [`advance`](Self::advance), so a test asserts values
/// at fixed times (`t = 0`, `100`, `250` ms) and a renderer's driver injects the
/// time it has. It holds at most one animation per node and property: starting
/// another replaces the first, from where it is.
#[derive(Default)]
pub struct AnimationSampler {
    now_ms: f64,
    running: BTreeMap<(String, String), Running>,
}

impl AnimationSampler {
    /// A sampler at time zero with nothing running.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The clock, in milliseconds since the sampler was made.
    #[must_use]
    pub fn now_ms(&self) -> f64 {
        self.now_ms
    }

    /// How many animations are running.
    #[must_use]
    pub fn len(&self) -> usize {
        self.running.len()
    }

    /// Whether nothing is running.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    /// Starts `animation` on `node.property` at the current time, replacing
    /// any animation that was running there.
    pub fn start(&mut self, node: &str, property: &str, animation: Animation) {
        self.running.insert(
            (node.to_owned(), property.to_owned()),
            Running {
                animation,
                started_ms: self.now_ms,
            },
        );
    }

    /// Stops the animation on `node.property` without finishing it. Returns
    /// whether one was running.
    pub fn cancel(&mut self, node: &str, property: &str) -> bool {
        self.running
            .remove(&(node.to_owned(), property.to_owned()))
            .is_some()
    }

    /// Stops every animation of `node`.
    pub fn cancel_node(&mut self, node: &str) {
        self.running.retain(|(name, _), _| name != node);
    }

    /// Stops everything.
    pub fn clear(&mut self) {
        self.running.clear();
    }

    /// The value of `node.property` at the current time, if it is animating.
    #[must_use]
    pub fn value(&self, node: &str, property: &str) -> Option<f64> {
        let running = self.running.get(&(node.to_owned(), property.to_owned()))?;
        Some(running.animation.sample(self.now_ms - running.started_ms))
    }

    /// Moves the clock forward by `dt_ms` (a NaN or negative step is zero: the
    /// clock never goes backwards), samples every running animation at the new
    /// time and removes the ones that ended.
    pub fn advance(&mut self, dt_ms: f64) -> Frame {
        if dt_ms > 0.0 {
            self.now_ms += dt_ms;
        }
        let now = self.now_ms;
        let mut frame = Frame {
            time_ms: now,
            ..Frame::default()
        };
        for ((node, property), running) in &self.running {
            let elapsed = now - running.started_ms;
            frame.samples.push(Sample {
                node: node.clone(),
                property: property.clone(),
                value: running.animation.sample(elapsed),
            });
            if running.animation.is_finished(elapsed) {
                frame.finished.push((node.clone(), property.clone()));
            }
        }
        for key in &frame.finished {
            self.running.remove(key);
        }
        frame
    }
}
