//! The timing curves of motion.
//!
//! These are the same curves, with the same arithmetic, as `xui_core::motion`
//! (this crate does not depend on xui, so they are re-implemented and tested
//! against the same literals). Keep the two byte-compatible: the xui renderer
//! samples xui's, and golden traces assert this crate's.

use std::fmt;

/// The exponent the Windows Phone Toolkit's transitions give their
/// `ExponentialEase`: a sharp start that settles slowly.
pub const DEFAULT_EXPONENT: f32 = 6.0;

/// A timing curve: a pure map from normalised time `t` in `[0, 1]` to
/// normalised progress in `[0, 1]`.
///
/// Every curve starts at exactly `0.0` and ends at exactly `1.0` (see
/// [`ease`](Self::ease)), never leaves `[0, 1]` (no overshoot, no bounce) and
/// never goes backwards, so an animation stays between its endpoints.
///
/// *In* curves start slowly and finish fast; *out* curves start fast and
/// settle slowly. Metro motion mostly uses the exponential out curve: "fast in,
/// slow out".
///
/// An exponential is not a cubic Bezier, so the web renderer approximates
/// [`ExpoIn`](Self::ExpoIn) and [`ExpoOut`](Self::ExpoOut) with a CSS
/// `linear()` function sampled from [`ease`](Self::ease), and its fixtures carry
/// a tolerance. A native renderer evaluates [`ease`](Self::ease) directly.
///
/// Equality is exact, except that a NaN exponent equals a NaN exponent (like
/// [`Value`](crate::Value)), so a patch holding one is comparable.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Easing {
    /// `f(t) = t`: constant speed.
    Linear,
    /// `f(t) = t²`: starts slowly, accelerates.
    QuadIn,
    /// `f(t) = 1 - (1 - t)²`: starts fast, decelerates.
    QuadOut,
    /// [`QuadIn`](Self::QuadIn) for the first half, [`QuadOut`](Self::QuadOut)
    /// for the second, meeting at `f(0.5) = 0.5`.
    QuadInOut,
    /// `f(t) = t³`.
    CubicIn,
    /// `f(t) = 1 - (1 - t)³`.
    CubicOut,
    /// [`CubicIn`](Self::CubicIn) for the first half,
    /// [`CubicOut`](Self::CubicOut) for the second.
    CubicInOut,
    /// The Windows Phone Toolkit's `ExponentialEase` as an ease-in:
    /// `f(t) = (e^(k t) - 1) / (e^k - 1)` for the `exponent` `k`.
    ///
    /// An exponent of zero (or NaN) is [`Linear`](Self::Linear). A negative
    /// exponent is the ease-out of the same magnitude. An infinite exponent is
    /// a step: at the end for `ExpoIn`, at the start for `ExpoOut`.
    ExpoIn {
        /// The steepness `k`. [`DEFAULT_EXPONENT`] is `6`.
        exponent: f32,
    },
    /// The mirror of [`ExpoIn`](Self::ExpoIn): `f(t) = 1 - g(1 - t)` where `g`
    /// is the ease-in of the same `exponent`. This is the curve of Metro's
    /// transitions: over 90% of the way by `t = 0.4` at the default exponent.
    ExpoOut {
        /// The steepness `k`. [`DEFAULT_EXPONENT`] is `6`.
        exponent: f32,
    },
}

/// `a == b`, with NaN equal to NaN.
fn same_float(a: f32, b: f32) -> bool {
    a == b || (a.is_nan() && b.is_nan())
}

impl PartialEq for Easing {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ExpoIn { exponent: a }, Self::ExpoIn { exponent: b })
            | (Self::ExpoOut { exponent: a }, Self::ExpoOut { exponent: b }) => same_float(*a, *b),
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }
}

impl Eq for Easing {}

impl Easing {
    /// [`ExpoIn`](Self::ExpoIn) with the Toolkit's [`DEFAULT_EXPONENT`]: the
    /// *accelerate* curve of exits.
    #[must_use]
    pub const fn expo_in() -> Self {
        Self::ExpoIn {
            exponent: DEFAULT_EXPONENT,
        }
    }

    /// [`ExpoOut`](Self::ExpoOut) with the Toolkit's [`DEFAULT_EXPONENT`]: the
    /// *decelerate* curve of enters and of `change` transitions.
    #[must_use]
    pub const fn expo_out() -> Self {
        Self::ExpoOut {
            exponent: DEFAULT_EXPONENT,
        }
    }

    /// Maps normalised time `t` to normalised progress.
    ///
    /// `t` is clamped to `[0, 1]`, so `ease(0.0) == 0.0` and `ease(1.0) == 1.0`
    /// hold exactly for every curve, and a NaN `t` is the start (`0.0`). The
    /// result is always within `[0, 1]`.
    #[must_use]
    pub fn ease(self, t: f32) -> f32 {
        if t.is_nan() || t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        let progress = match self {
            Self::Linear => t,
            Self::QuadIn => t * t,
            Self::QuadOut => 1.0 - (1.0 - t) * (1.0 - t),
            Self::QuadInOut if t < 0.5 => 2.0 * t * t,
            Self::QuadInOut => 1.0 - 2.0 * (1.0 - t) * (1.0 - t),
            Self::CubicIn => t * t * t,
            Self::CubicOut => 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t),
            Self::CubicInOut if t < 0.5 => 4.0 * t * t * t,
            Self::CubicInOut => 1.0 - 4.0 * (1.0 - t) * (1.0 - t) * (1.0 - t),
            Self::ExpoIn { exponent } => exponential(f64::from(exponent), f64::from(t)),
            // `1 - g(1 - t)` for `g` with exponent `k` is `g` with exponent
            // `-k`, the form that does not cancel near `t = 0`.
            Self::ExpoOut { exponent } => exponential(-f64::from(exponent), f64::from(t)),
        };
        // Rounding must never push a curve outside the range an animation
        // relies on.
        progress.clamp(0.0, 1.0)
    }
}

/// `(e^(k t) - 1) / (e^k - 1)` for `t` strictly inside `(0, 1)`, as an `f32`.
///
/// Evaluated with `exp_m1` so a small `k` keeps its precision, and rewritten
/// for `k > 0` as `e^(k (t - 1)) * (e^(-k t) - 1) / (e^(-k) - 1)` so that no
/// exponent is ever positive: the textbook form overflows to `inf / inf` for
/// `k` above about 709.
fn exponential(k: f64, t: f64) -> f32 {
    // The closed form is `0 / 0` at an exponent of zero, and its limit there
    // is `t`.
    if k == 0.0 || k.is_nan() {
        return t as f32;
    }
    let progress = if k > 0.0 {
        (k * (t - 1.0)).exp() * (-k * t).exp_m1() / (-k).exp_m1()
    } else {
        (k * t).exp_m1() / k.exp_m1()
    };
    progress as f32
}

/// The stable name used by patch traces: `linear`, `quad_in`, `quad_out`,
/// `quad_in_out`, `cubic_in`, `cubic_out`, `cubic_in_out`, `expo_in(<k>)` and
/// `expo_out(<k>)`, with `k` written as a Rust float (`6.0`).
impl fmt::Display for Easing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Linear => f.write_str("linear"),
            Self::QuadIn => f.write_str("quad_in"),
            Self::QuadOut => f.write_str("quad_out"),
            Self::QuadInOut => f.write_str("quad_in_out"),
            Self::CubicIn => f.write_str("cubic_in"),
            Self::CubicOut => f.write_str("cubic_out"),
            Self::CubicInOut => f.write_str("cubic_in_out"),
            Self::ExpoIn { exponent } => write!(f, "expo_in({exponent:?})"),
            Self::ExpoOut { exponent } => write!(f, "expo_out({exponent:?})"),
        }
    }
}
