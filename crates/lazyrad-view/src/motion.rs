//! Motion is declarative data on a node: what it does when it appears, when it
//! leaves, when an animatable property changes and when it is pressed.
//!
//! ```text
//! Motion {
//!     enter:  Slide { from: Bottom, distance: 40 },   // when it is created or shown
//!     exit:   Fade,                                    // when it is removed or hidden
//!     change: { "opacity": 200 ms, "translate_y": 300 ms },
//!     press:  PressScale,
//! }
//! ```
//!
//! The core decides **what** animates and a renderer decides **how** (see
//! [`Patch::Animate`](crate::Patch::Animate)). This is the M8 vocabulary of
//! docs/modern-ui-plan.md §4.8: the 2D subset of Windows Phone 7's Metro
//! motion. Nothing here is 3D and nothing loops.
//!
//! # The rules, as the planner keeps them
//!
//! * **Transform and opacity only.** [`ANIMATABLE`] is the whole set. A
//!   `change` entry for any other property is a
//!   [`TreeError::NotAnimatable`](crate::TreeError::NotAnimatable): layout is
//!   immutable once mounted, and a reflow per frame is what makes an interface
//!   janky.
//! * **Fast in, slow out.** Entering uses the decelerating
//!   [`Easing::expo_out`], exiting the accelerating [`Easing::expo_in`].
//! * **Reduced motion** (`Environment::reduce_motion`): [`Preset::Slide`]
//!   resolves to [`Preset::Fade`], [`Preset::PressScale`] is dropped, a
//!   `change` transition is an instant `Set`, and nothing loops. The setting
//!   is read when an animation is planned, so flipping it does not touch one in
//!   flight: it runs on, unless the next change to that same property is then
//!   applied as a `Set`, which replaces it.
//! * **A slide is relative to where the node rests.** `Slide { from: Bottom,
//!   distance: 40 }` starts 40 below the node's own `translate_y`, not 40 from
//!   zero.
//!
//! # Example
//!
//! ```
//! use lazyrad_view::{
//!     Bindings, Edge, Environment, Motion, Node, Preset, Renderer, SamplingRenderer, Session,
//!     Tree,
//! };
//!
//! let card = Node::new("card", "Panel")
//!     .with_motion(Motion::new().with_enter(Preset::slide(Edge::Bottom, 40.0)));
//! let tree = Tree::new(Node::new("root", "Column").with_child(card)).unwrap();
//! let mut session = Session::new(tree, Bindings::new());
//! let mut renderer = SamplingRenderer::new();
//!
//! let env = Environment::new(360.0, 640.0);
//! renderer.apply(&session.build(&rhai::Map::new(), &env).patches);
//!
//! // The virtual clock: at t = 0 the card is 40 dp down and transparent...
//! let frame = renderer.advance(0.0);
//! assert_eq!(frame.value("card", "translate_y"), Some(40.0));
//! assert_eq!(frame.value("card", "opacity"), Some(0.0));
//! // ...and at rest once the entrance has run. The renderer reports what ended.
//! let frame = renderer.advance(350.0);
//! assert_eq!(frame.value("card", "translate_y"), Some(0.0));
//! for ended in &frame.finished {
//!     let patches = session.finished(&ended.node, &ended.property, ended.id);
//!     assert!(patches.is_empty());
//! }
//! ```

use std::collections::BTreeMap;

use crate::easing::Easing;

mod check;
pub(crate) mod steps;

pub use check::MotionProblem;

/// The properties that can be animated: the ones that do not change layout.
///
/// They are the common runtime properties of every widget node, `opacity: 1`
/// and the identity transform by default (see [`identity`]).
pub const ANIMATABLE: [&str; 6] = [
    "opacity",
    "translate_x",
    "translate_y",
    "scale_x",
    "scale_y",
    "rotate",
];

/// Layout properties: refused with a reason of their own by
/// [`TreeError::NotAnimatable`](crate::TreeError::NotAnimatable), because they
/// are the likeliest mistake.
const LAYOUT: [&str; 20] = [
    "width",
    "height",
    "min_width",
    "min_height",
    "max_width",
    "max_height",
    "fill",
    "gap",
    "padding",
    "margin",
    "align",
    "align_items",
    "justify",
    "direction",
    "span",
    "columns",
    "left",
    "top",
    "at",
    "anchor",
];

/// Whether `property` is one of [`ANIMATABLE`].
#[must_use]
pub fn is_animatable(property: &str) -> bool {
    ANIMATABLE.contains(&property)
}

/// The value an animatable property has when nothing sets it: `1` for
/// `opacity` and the scales, `0` for the translations and `rotate`. `None` for
/// a property that is not animatable.
#[must_use]
pub fn identity(property: &str) -> Option<f64> {
    match property {
        "opacity" | "scale_x" | "scale_y" => Some(1.0),
        "translate_x" | "translate_y" | "rotate" => Some(0.0),
        _ => None,
    }
}

/// Why a property cannot be animated, for [`TreeError::NotAnimatable`].
pub(crate) fn why_not_animatable(layout: bool) -> &'static str {
    if layout {
        "it is a layout property; layout is fixed once a form is mounted and a reflow per frame \
         is what makes an interface janky, so only opacity, translate_x, translate_y, scale_x, \
         scale_y and rotate animate"
    } else {
        "only opacity, translate_x, translate_y, scale_x, scale_y and rotate animate"
    }
}

/// The duration of an `enter` preset, in milliseconds (a Toolkit transition is
/// 250 to 350 ms; these are tokens, tuned by eye).
pub const ENTER_MS: f64 = 350.0;
/// The duration of an `exit` preset: shorter than an enter, so leaving is
/// quick.
pub const EXIT_MS: f64 = 250.0;
/// How far [`Preset::PressScale`] scales a pressed node (a LazyRAD token, not a
/// Windows Phone value: there is no tilt).
pub const PRESS_SCALE: f64 = 0.96;
/// How long a pressed node takes to reach [`PRESS_SCALE`].
pub const PRESS_IN_MS: f64 = 100.0;
/// How long a released node holds before it returns (the Toolkit's tilt waits
/// 200 ms).
pub const PRESS_RELEASE_DELAY_MS: f64 = 200.0;
/// How long the return takes (the Toolkit's tilt releases over 100 ms).
pub const PRESS_RELEASE_MS: f64 = 100.0;

/// An edge of a node's parent a preset moves it from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Edge {
    /// The top edge: the node starts above its place.
    Top,
    /// The bottom edge: the node starts below its place.
    Bottom,
    /// The left edge.
    Left,
    /// The right edge.
    Right,
}

/// A named piece of motion. The vocabulary grows with the milestones (M9 adds
/// `Roll`, `Rotate`, a flat `Turnstile`, `Feather`, ...), so a `match` needs a
/// wildcard arm.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Preset {
    /// Opacity from 0 to 1 on `enter`, and back on `exit`.
    Fade,
    /// A translation from `distance` dp towards the `from` edge, with a fade.
    ///
    /// On `exit` the node moves *back towards* that edge: it leaves the way it
    /// came. (The plan does not say which way an exit goes; this is the
    /// reading that needs no second parameter.)
    Slide {
        /// The edge the node comes from, and goes back to.
        from: Edge,
        /// The offset in dp. Finite and not negative (a
        /// [`TreeError`](crate::TreeError) otherwise).
        distance: f64,
    },
    /// A pressed node eases down to [`PRESS_SCALE`] and back. Only valid as a
    /// `press` preset.
    PressScale,
}

impl Preset {
    /// [`Preset::Slide`] from `from` by `distance` dp.
    #[must_use]
    pub const fn slide(from: Edge, distance: f64) -> Self {
        Self::Slide { from, distance }
    }

    /// The preset's name, as the document and messages spell it.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Fade => "Fade",
            Self::Slide { .. } => "Slide",
            Self::PressScale => "PressScale",
        }
    }
}

/// The timing of a `change` transition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    /// How long the change takes, in milliseconds.
    pub duration_ms: f64,
    /// The timing curve.
    pub easing: Easing,
}

impl Transition {
    /// A transition of `duration_ms` with the decelerating default curve,
    /// [`Easing::expo_out`].
    #[must_use]
    pub const fn new(duration_ms: f64) -> Self {
        Self {
            duration_ms,
            easing: Easing::expo_out(),
        }
    }

    /// This transition with the curve `easing`.
    #[must_use]
    pub const fn with_easing(mut self, easing: Easing) -> Self {
        self.easing = easing;
        self
    }
}

/// A node's motion. The default is empty, which is a node that does not
/// animate, so a tree without motion behaves exactly as it did before.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Motion {
    /// Runs when the node is created (the first time a name appears, and
    /// again after the node has left the tree) and when its `visible`
    /// becomes true.
    pub enter: Option<Preset>,
    /// Runs when the node is removed from the tree and when its `visible`
    /// becomes false. The node is held, and its room kept, until it ends.
    pub exit: Option<Preset>,
    /// Per animatable property, how a change of its value is eased instead of
    /// applied at once. Only [`ANIMATABLE`] names are valid.
    pub change: BTreeMap<String, Transition>,
    /// Feedback while the node is pressed (see
    /// [`Session::press_begin`](crate::Session::press_begin)).
    pub press: Option<Preset>,
}

impl Motion {
    /// A motion that does nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// This motion with an `enter` preset.
    #[must_use]
    pub fn with_enter(mut self, preset: Preset) -> Self {
        self.enter = Some(preset);
        self
    }

    /// This motion with an `exit` preset.
    #[must_use]
    pub fn with_exit(mut self, preset: Preset) -> Self {
        self.exit = Some(preset);
        self
    }

    /// This motion with a `change` transition for `property`.
    #[must_use]
    pub fn with_change(mut self, property: impl Into<String>, transition: Transition) -> Self {
        self.change.insert(property.into(), transition);
        self
    }

    /// This motion with a `press` preset.
    #[must_use]
    pub fn with_press(mut self, preset: Preset) -> Self {
        self.press = Some(preset);
        self
    }

    /// Whether the motion declares nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.enter.is_none()
            && self.exit.is_none()
            && self.change.is_empty()
            && self.press.is_none()
    }
}
