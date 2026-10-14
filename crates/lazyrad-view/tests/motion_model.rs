//! The motion model: what a tree accepts and what it refuses.

mod common;

use lazyrad_view::limits::MAX_ANIMATION_MS;
use lazyrad_view::motion::{ANIMATABLE, identity, is_animatable};
use lazyrad_view::{Edge, Motion, MotionProblem, Node, Preset, Transition, Tree, TreeError, Value};

fn card(motion: Motion) -> Node {
    Node::new("card", "Panel").with_motion(motion)
}

#[test]
fn the_six_animatable_properties_are_accepted_and_have_an_identity() {
    assert_eq!(
        ANIMATABLE,
        [
            "opacity",
            "translate_x",
            "translate_y",
            "scale_x",
            "scale_y",
            "rotate"
        ]
    );
    let mut motion = Motion::new();
    for property in ANIMATABLE {
        assert!(is_animatable(property));
        motion = motion.with_change(property, Transition::new(200.0));
    }
    assert!(Tree::new(card(motion)).is_ok());
    assert_eq!(identity("opacity"), Some(1.0));
    assert_eq!(identity("scale_x"), Some(1.0));
    assert_eq!(identity("translate_y"), Some(0.0));
    assert_eq!(identity("rotate"), Some(0.0));
    assert_eq!(identity("width"), None);
}

#[test]
fn a_change_on_a_layout_property_is_refused_and_the_message_says_why() {
    for property in [
        "width",
        "height",
        "gap",
        "fill",
        "padding",
        "margin",
        "direction",
        "align",
        "max_width",
        "justify",
    ] {
        let motion = Motion::new().with_change(property, Transition::new(200.0));
        let error = Tree::new(card(motion)).unwrap_err();
        assert_eq!(
            error,
            TreeError::NotAnimatable {
                node: "card".into(),
                property: property.into(),
                layout: true
            },
            "{property}"
        );
        let message = error.to_string();
        assert!(message.contains(&format!("`{property}`")), "{message}");
        assert!(message.contains("layout property"), "{message}");
        assert!(message.contains("janky"), "{message}");
        assert!(message.contains("opacity"), "{message}");
    }
}

#[test]
fn any_other_property_is_refused_with_the_list_of_what_animates() {
    for property in ["text", "background", "visible", "no_such_thing", ""] {
        let motion = Motion::new().with_change(property, Transition::new(200.0));
        let error = Tree::new(card(motion)).unwrap_err();
        assert_eq!(
            error,
            TreeError::NotAnimatable {
                node: "card".into(),
                property: property.into(),
                layout: false
            }
        );
        let message = error.to_string();
        assert!(!message.contains("layout property"), "{message}");
        assert!(message.contains("only opacity, translate_x"), "{message}");
    }
}

#[test]
fn presets_belong_to_their_slots() {
    let press_in_enter = card(Motion::new().with_enter(Preset::PressScale));
    assert_eq!(
        Tree::new(press_in_enter).unwrap_err(),
        TreeError::WrongPreset {
            node: "card".into(),
            slot: "enter",
            preset: "PressScale"
        }
    );
    let press_in_exit = card(Motion::new().with_exit(Preset::PressScale));
    assert!(matches!(
        Tree::new(press_in_exit),
        Err(TreeError::WrongPreset { slot: "exit", .. })
    ));
    for preset in [Preset::Fade, Preset::slide(Edge::Left, 8.0)] {
        let error = Tree::new(card(Motion::new().with_press(preset))).unwrap_err();
        assert!(
            matches!(error, TreeError::WrongPreset { slot: "press", .. }),
            "{error}"
        );
    }
    assert_eq!(
        Tree::new(card(Motion::new().with_press(Preset::PressScale))).map(|_| ()),
        Ok(())
    );
}

#[test]
fn nan_negative_and_infinite_numbers_are_typed_errors() {
    let distance = |value: f64| Motion::new().with_enter(Preset::slide(Edge::Bottom, value));
    for (value, problem) in [
        (f64::NAN, MotionProblem::NotANumber),
        (-1.0, MotionProblem::Negative),
        (f64::NEG_INFINITY, MotionProblem::Negative),
        (f64::INFINITY, MotionProblem::NotFinite),
    ] {
        assert_eq!(
            Tree::new(card(distance(value))).unwrap_err(),
            TreeError::BadMotionValue {
                node: "card".into(),
                field: "enter.distance".into(),
                problem
            },
            "{value}"
        );
    }
    // The same on an exit, and a zero distance is a fade.
    let exit = Motion::new().with_exit(Preset::slide(Edge::Top, f64::NAN));
    assert!(matches!(
        Tree::new(card(exit)),
        Err(TreeError::BadMotionValue { ref field, .. }) if field == "exit.distance"
    ));
    assert!(Tree::new(card(distance(0.0))).is_ok());

    let duration = |value: f64| Motion::new().with_change("opacity", Transition::new(value));
    for (value, problem) in [
        (f64::NAN, MotionProblem::NotANumber),
        (-0.5, MotionProblem::Negative),
        (MAX_ANIMATION_MS + 1.0, MotionProblem::TooLong),
        (f64::INFINITY, MotionProblem::TooLong),
    ] {
        assert_eq!(
            Tree::new(card(duration(value))).unwrap_err(),
            TreeError::BadMotionValue {
                node: "card".into(),
                field: "change.opacity.duration_ms".into(),
                problem
            },
            "{value}"
        );
    }
    // Zero (an instant change) and the limit itself are fine.
    assert!(Tree::new(card(duration(0.0))).is_ok());
    assert!(Tree::new(card(duration(MAX_ANIMATION_MS))).is_ok());

    let message = Tree::new(card(duration(f64::NAN))).unwrap_err().to_string();
    assert_eq!(
        message,
        "`card.motion.change.opacity.duration_ms` is not a number"
    );
}

#[test]
fn the_first_offending_node_in_pre_order_is_reported() {
    let bad = Motion::new().with_change("gap", Transition::new(100.0));
    let root = Node::new("root", "Column")
        .with_child(Node::new("fine", "Label"))
        .with_child(
            Node::new("row", "Row")
                .with_child(Node::new("inner", "Panel").with_motion(bad.clone())),
        )
        .with_child(Node::new("later", "Panel").with_motion(bad));
    assert!(matches!(
        Tree::new(root),
        Err(TreeError::NotAnimatable { ref node, .. }) if node == "inner"
    ));
}

#[test]
fn an_empty_motion_is_the_default_and_changes_nothing() {
    let node = Node::new("n", "Label").with_prop("text", "hi");
    assert!(node.motion.is_empty());
    assert_eq!(node.motion, Motion::default());
    let with = node
        .clone()
        .with_motion(Motion::new().with_press(Preset::PressScale));
    assert_ne!(node, with);
    assert!(!with.motion.is_empty());
    assert_eq!(
        Tree::new(node).unwrap().root().props["text"],
        Value::Text("hi".into())
    );
}

#[test]
fn transitions_default_to_the_decelerating_curve() {
    let transition = Transition::new(250.0);
    assert_eq!(transition.duration_ms, 250.0);
    assert_eq!(transition.easing, lazyrad_view::Easing::expo_out());
    let linear = transition.with_easing(lazyrad_view::Easing::Linear);
    assert_eq!(linear.easing, lazyrad_view::Easing::Linear);
}
