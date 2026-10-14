//! What the presets animate.

use super::{ENTER_MS, EXIT_MS, Edge, Preset};
use crate::easing::Easing;

/// Which way a preset runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Enter,
    Exit,
}

/// One property a preset animates: between its rest value and `away`, the value
/// at the node's "off" end (hidden, or offset).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Step {
    pub property: &'static str,
    pub away: f64,
    pub duration_ms: f64,
    pub easing: Easing,
}

/// What an `enter` or `exit` preset animates, in property order.
///
/// Under `reduced` a `Slide` is a `Fade`. `PressScale` has no enter or exit
/// steps (it is not valid there; a tree cannot hold it).
pub(crate) fn steps(preset: &Preset, phase: Phase, reduced: bool) -> Vec<Step> {
    let (duration_ms, easing) = match phase {
        Phase::Enter => (ENTER_MS, Easing::expo_out()),
        Phase::Exit => (EXIT_MS, Easing::expo_in()),
    };
    let fade = Step {
        property: "opacity",
        away: 0.0,
        duration_ms,
        easing,
    };
    match preset {
        Preset::Fade => vec![fade],
        Preset::Slide { .. } if reduced => vec![fade],
        Preset::Slide { from, distance } => {
            // Not finite, which a validated tree never holds, is no offset.
            let distance = if distance.is_finite() { *distance } else { 0.0 };
            let (property, away) = match from {
                // `0.0 - d`, not `-d`: a distance of zero is +0, not -0.
                Edge::Top => ("translate_y", 0.0 - distance),
                Edge::Bottom => ("translate_y", distance),
                Edge::Left => ("translate_x", 0.0 - distance),
                Edge::Right => ("translate_x", distance),
            };
            vec![
                fade,
                Step {
                    property,
                    away,
                    duration_ms,
                    easing,
                },
            ]
        }
        Preset::PressScale => Vec::new(),
    }
}
