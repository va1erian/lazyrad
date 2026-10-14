//! What a [`Motion`] must satisfy to be part of a [`Tree`](crate::Tree).

use super::{LAYOUT, Motion, Preset, is_animatable};
use crate::limits::MAX_ANIMATION_MS;
use crate::tree::TreeError;

impl Motion {
    /// Checks the motion of the node called `node`; see [`TreeError`] for what
    /// is refused.
    pub(crate) fn validate(&self, node: &str) -> Result<(), TreeError> {
        for (slot, preset) in [("enter", &self.enter), ("exit", &self.exit)] {
            if let Some(preset) = preset {
                if matches!(preset, Preset::PressScale) {
                    return Err(wrong_preset(node, slot, preset));
                }
                check_distance(node, slot, preset)?;
            }
        }
        for (property, transition) in &self.change {
            if !is_animatable(property) {
                return Err(TreeError::NotAnimatable {
                    node: node.to_owned(),
                    property: property.clone(),
                    layout: LAYOUT.contains(&property.as_str()),
                });
            }
            let duration = transition.duration_ms;
            let problem = if duration.is_nan() {
                Some(MotionProblem::NotANumber)
            } else if duration < 0.0 {
                Some(MotionProblem::Negative)
            } else if duration > MAX_ANIMATION_MS {
                Some(MotionProblem::TooLong)
            } else {
                None
            };
            if let Some(problem) = problem {
                return Err(TreeError::BadMotionValue {
                    node: node.to_owned(),
                    field: format!("change.{property}.duration_ms"),
                    problem,
                });
            }
        }
        if let Some(preset) = &self.press {
            if !matches!(preset, Preset::PressScale) {
                return Err(wrong_preset(node, "press", preset));
            }
        }
        Ok(())
    }
}

fn wrong_preset(node: &str, slot: &'static str, preset: &Preset) -> TreeError {
    TreeError::WrongPreset {
        node: node.to_owned(),
        slot,
        preset: preset.name(),
    }
}

fn check_distance(node: &str, slot: &str, preset: &Preset) -> Result<(), TreeError> {
    let Preset::Slide { distance, .. } = preset else {
        return Ok(());
    };
    let problem = if distance.is_nan() {
        MotionProblem::NotANumber
    } else if *distance < 0.0 {
        MotionProblem::Negative
    } else if distance.is_infinite() {
        MotionProblem::NotFinite
    } else {
        return Ok(());
    };
    Err(TreeError::BadMotionValue {
        node: node.to_owned(),
        field: format!("{slot}.distance"),
        problem,
    })
}

/// What is wrong with a number in a [`Motion`], for
/// [`TreeError::BadMotionValue`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MotionProblem {
    /// The value is NaN.
    #[error("is not a number")]
    NotANumber,
    /// The value is below zero.
    #[error("is negative")]
    Negative,
    /// The value is infinite.
    #[error("is infinite")]
    NotFinite,
    /// The duration is above [`MAX_ANIMATION_MS`].
    #[error("is longer than the limit (see `lazyrad_view::limits::MAX_ANIMATION_MS`)")]
    TooLong,
}
