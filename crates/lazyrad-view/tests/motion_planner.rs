//! The motion planner on the plan's profile card: golden traces of enter,
//! change and the numbers a change cannot animate, with a virtual clock.
//!
//! The expected numbers come from the closed form of the Toolkit's
//! `ExponentialEase` (see `tests/easing.rs`), not from the code under test.

mod common;

use common::{built_card, card_state, line, phone, settled_card, tick, tree};
use lazyrad_view::{
    Bindings, Edge, Environment, Motion, Node, Preset, RecordingRenderer, Renderer,
    SamplingRenderer, Session, Transition, Value, trace,
};

const BUILD: &str = r#"create profile_form Column parent=- index=0 {padding=16}
create title_label Label parent=profile_form index=0 {text="Profile"}
create counter_card Panel parent=profile_form index=1 {card=true, opacity=1.0, translate_y=0.0}
create count_label Label parent=counter_card index=0 {text="Clicked 0 times"}
create add_button Button parent=counter_card index=1 {text="Add one"}
"#;

const ENTER: &str = "\
animate counter_card.opacity from=0.0 to=1.0 duration=350.0 delay=0.0 easing=expo_out(6.0)
animate counter_card.translate_y from=40.0 to=0.0 duration=350.0 delay=0.0 easing=expo_out(6.0)
";

#[test]
fn the_card_slides_up_and_fades_in_and_the_samples_match_the_golden() {
    let (mut session, mut renderer) = built_card(&phone());
    // The whole tree first, then the entrance, so a renderer has every node
    // before anything moves.
    assert_eq!(renderer.take_trace(), format!("{BUILD}{ENTER}"));

    let samples: Vec<String> = [0.0, 100.0, 150.0, 100.0]
        .iter()
        .map(|dt| line(&tick(&mut session, &mut renderer, *dt)))
        .collect();
    assert_eq!(
        samples,
        [
            "t=0 counter_card.opacity=0.0000 counter_card.translate_y=40.0000",
            "t=100 counter_card.opacity=0.8219 counter_card.translate_y=7.1222",
            "t=250 counter_card.opacity=0.9887 counter_card.translate_y=0.4525",
            "t=350 counter_card.opacity=1.0000 counter_card.translate_y=0.0000",
        ]
    );
    // Both ended at 350 ms and were reported; nothing is left running.
    assert_eq!(session.motion_stats().running, 0);
    assert_eq!(renderer.running(), 0);
    // The core's applied value is the rest, not a frame of the animation.
    assert_eq!(
        session.value("counter_card", "opacity"),
        Some(&Value::Float(1.0))
    );
}

#[test]
fn a_change_eases_from_the_last_applied_value_instead_of_setting() {
    let (mut session, mut renderer) = settled_card();
    let env = phone();
    // `count` is text, which is not animatable: a plain `set`, after the
    // `animate` of the lifted card (the patches follow the tree's order).
    let turn = session.update_with(&card_state(1.0, 0.0, 1), &env, &mut renderer);
    assert!(turn.diagnostics.is_empty());
    assert_eq!(
        renderer.take_trace(),
        "animate counter_card.translate_y from=0.0 to=-8.0 duration=300.0 delay=0.0 \
         easing=expo_out(6.0)\n\
         set count_label.text = \"Clicked 1 times\"\n"
    );
    // The session's value is the target; the widget is on its way.
    assert_eq!(
        session.value("counter_card", "translate_y"),
        Some(&Value::Float(-8.0))
    );
    assert_eq!(renderer.value("counter_card", "translate_y"), Some(0.0));
    let at_half = renderer.advance(150.0);
    assert_eq!(line(&at_half), "t=500 counter_card.translate_y=-7.6206");
    let at_end = tick(&mut session, &mut renderer, 150.0);
    assert_eq!(line(&at_end), "t=650 counter_card.translate_y=-8.0000");
    // No other turn patches it again: the diff is against the target.
    let quiet = session.update_with(&card_state(1.0, 0.0, 1), &env, &mut renderer);
    assert!(quiet.patches.is_empty());
}

#[test]
fn a_newer_change_on_the_same_property_starts_from_current() {
    let (mut session, mut renderer) = settled_card();
    let env = phone();
    session.update_with(&card_state(1.0, 0.0, 0), &env, &mut renderer);
    renderer.take_trace();
    let frame = tick(&mut session, &mut renderer, 100.0);
    assert_eq!(line(&frame), "t=450 counter_card.translate_y=-6.9345");

    // Back down while it is still moving: the core cannot know where the
    // widget is, so the patch starts from `current`.
    let turn = session.update_with(&card_state(0.0, 0.0, 0), &env, &mut renderer);
    assert_eq!(
        trace(&turn.patches),
        "animate counter_card.translate_y from=current to=0.0 duration=300.0 delay=0.0 \
         easing=expo_out(6.0)\n"
    );
    assert_eq!(session.motion_stats().running, 1);
    let samples: Vec<String> = [0.0, 75.0, 75.0, 150.0]
        .iter()
        .map(|dt| line(&tick(&mut session, &mut renderer, *dt)))
        .collect();
    assert_eq!(
        samples,
        [
            "t=450 counter_card.translate_y=-6.9345",
            "t=525 counter_card.translate_y=-1.5339",
            "t=600 counter_card.translate_y=-0.3289",
            "t=750 counter_card.translate_y=0.0000",
        ]
    );
    // After it ended, a change is from the applied value again.
    let turn = session.update_with(&card_state(1.0, 0.0, 0), &env, &mut renderer);
    assert!(trace(&turn.patches).contains("from=0.0 to=-8.0"));
}

#[test]
fn a_property_without_a_transition_or_a_non_numeric_value_is_a_plain_set() {
    let (mut session, mut renderer) = settled_card();
    let env = phone();
    // `opacity` has a transition (200 ms): animated.
    session.update_with(&card_state(0.0, 1.0, 0), &env, &mut renderer);
    assert_eq!(
        renderer.take_trace(),
        "animate counter_card.opacity from=1.0 to=0.5 duration=200.0 delay=0.0 \
         easing=expo_out(6.0)\n"
    );

    // A node with no `change` entry for the property sets it.
    let root = Node::new("root", "Column").with_child(
        Node::new("plain", "Panel")
            .with_bind("opacity", "1.0 - state.dim * 0.5")
            .with_motion(Motion::new().with_change("translate_x", Transition::new(100.0))),
    );
    let mut plain = Session::new(tree(root), Bindings::new());
    let mut sink = RecordingRenderer::new();
    sink.apply(&plain.build(&card_state(0.0, 0.0, 0), &env).patches);
    sink.take_trace();
    plain.update_with(&card_state(0.0, 1.0, 0), &env, &mut sink);
    assert_eq!(sink.take_trace(), "set plain.opacity = 0.5\n");
    assert_eq!(plain.motion_stats().running, 0);
}

#[test]
fn integers_animate_as_numbers_and_keep_their_kind() {
    let root = Node::new("root", "Column").with_child(
        Node::new("slider", "Panel")
            .with_prop("translate_x", 0)
            .with_bind("translate_x", "state.count")
            .with_motion(Motion::new().with_change("translate_x", Transition::new(100.0))),
    );
    let mut session = Session::new(tree(root), Bindings::new());
    let mut renderer = SamplingRenderer::new();
    let env = phone();
    renderer.apply(&session.build(&card_state(0.0, 0.0, 0), &env).patches);
    renderer.take_trace();
    session.update_with(&card_state(0.0, 0.0, 5), &env, &mut renderer);
    assert_eq!(
        renderer.take_trace(),
        "animate slider.translate_x from=0.0 to=5.0 duration=100.0 delay=0.0 \
         easing=expo_out(6.0)\n"
    );
    // What the session applied is still an integer.
    assert_eq!(session.value("slider", "translate_x"), Some(&Value::Int(5)));
}

#[test]
fn values_that_cannot_be_animated_are_set_at_once() {
    let root = Node::new("root", "Column").with_child(
        Node::new("card", "Panel")
            .with_bind("opacity", "state.v")
            .with_bind("translate_y", "state.w")
            .with_motion(
                Motion::new()
                    .with_change("opacity", Transition::new(100.0))
                    // A zero duration has nothing to move through.
                    .with_change("translate_y", Transition::new(0.0)),
            ),
    );
    let mut session = Session::new(tree(root), Bindings::new());
    let mut renderer = RecordingRenderer::new();
    let env = phone();
    let state = |v: f64, w: f64| {
        let mut state = rhai::Map::new();
        state.insert("v".into(), rhai::Dynamic::from_float(v));
        state.insert("w".into(), rhai::Dynamic::from_float(w));
        state
    };
    renderer.apply(&session.build(&state(0.5, 0.0), &env).patches);
    renderer.take_trace();
    session.update_with(&state(f64::NAN, 4.0), &env, &mut renderer);
    // To NaN, and a zero duration: both at once.
    assert_eq!(
        renderer.take_trace(),
        "set card.opacity = NaN\nset card.translate_y = 4.0\n"
    );
    // From NaN: also at once.
    session.update_with(&state(0.25, 4.0), &env, &mut renderer);
    assert_eq!(renderer.take_trace(), "set card.opacity = 0.25\n");
    assert_eq!(session.motion_stats().running, 0);
}

/// A node whose `translate_y` exists only from Medium up.
fn medium_only(motion: Motion) -> Session {
    let root = Node::new("root", "Column").with_child(
        Node::new("card", "Panel")
            .with_responsive(lazyrad_view::SizeClass::Medium, "translate_y", 12.0)
            .with_motion(motion),
    );
    Session::new(tree(root), Bindings::new())
}

#[test]
fn an_unset_eases_back_to_the_default_when_the_property_has_a_transition() {
    let mut session = medium_only(Motion::new().with_change("translate_y", Transition::new(300.0)));
    let mut renderer = SamplingRenderer::new();
    let state = rhai::Map::new();
    renderer.apply(&session.build(&state, &phone()).patches);
    renderer.take_trace();

    // Growing gives it a value, from the identity, by animation...
    let tablet = Environment::new(700.0, 640.0);
    session.update_with(&state, &tablet, &mut renderer);
    assert_eq!(
        renderer.take_trace(),
        "animate card.translate_y from=0.0 to=12.0 duration=300.0 delay=0.0 \
         easing=expo_out(6.0)\n"
    );
    tick(&mut session, &mut renderer, 300.0);
    // ...and shrinking takes it away the same way: back to the default, not a
    // jump.
    session.update_with(&state, &phone(), &mut renderer);
    assert_eq!(
        renderer.take_trace(),
        "animate card.translate_y from=12.0 to=0.0 duration=300.0 delay=0.0 \
         easing=expo_out(6.0)\n"
    );
    // The session's model is that the property has no value.
    assert_eq!(session.value("card", "translate_y"), None);
    assert_eq!(session.motion_stats().running, 1);
    // Interrupted by growing again: from where it is.
    session.update_with(&state, &tablet, &mut renderer);
    assert!(renderer.take_trace().contains("from=current to=12.0"));
}

#[test]
fn an_unset_applies_at_once_without_a_transition_or_under_reduced_motion() {
    // No `change` entry for the property: growing and shrinking are plain.
    let mut session = medium_only(Motion::new().with_change("opacity", Transition::new(300.0)));
    let mut renderer = RecordingRenderer::new();
    let state = rhai::Map::new();
    renderer.apply(&session.build(&state, &phone()).patches);
    renderer.take_trace();
    let tablet = Environment::new(700.0, 640.0);
    session.update_with(&state, &tablet, &mut renderer);
    assert_eq!(renderer.take_trace(), "set card.translate_y = 12.0\n");
    session.update_with(&state, &phone(), &mut renderer);
    assert_eq!(renderer.take_trace(), "unset card.translate_y\n");

    // With a transition, but the user asked for reduced motion.
    let mut session = medium_only(Motion::new().with_change("translate_y", Transition::new(300.0)));
    let mut renderer = RecordingRenderer::new();
    let calm_tablet = Environment {
        reduce_motion: true,
        ..tablet
    };
    let calm_phone = Environment {
        reduce_motion: true,
        ..phone()
    };
    renderer.apply(&session.build(&state, &calm_phone).patches);
    renderer.take_trace();
    session.update_with(&state, &calm_tablet, &mut renderer);
    assert_eq!(renderer.take_trace(), "set card.translate_y = 12.0\n");
    session.update_with(&state, &calm_phone, &mut renderer);
    assert_eq!(renderer.take_trace(), "unset card.translate_y\n");
    assert_eq!(session.motion_stats().running, 0);
}

#[test]
fn an_animate_towards_the_value_the_widget_already_shows_is_dropped() {
    let (mut session, mut renderer) = settled_card();
    let env = phone();
    // Something else (a script's `animate`, the user) left the widget at 0.5.
    renderer.set_live("counter_card", "opacity", Value::Float(0.5));
    let turn = session.update_with(&card_state(0.0, 1.0, 0), &env, &mut renderer);
    // The binding now wants 0.5: nothing to animate, nothing handed over, and
    // the session counts it applied so it is not retried every turn.
    assert!(turn.patches.is_empty(), "{}", trace(&turn.patches));
    assert_eq!(renderer.take_trace(), "");
    assert_eq!(
        session.value("counter_card", "opacity"),
        Some(&Value::Float(0.5))
    );
    assert_eq!(session.motion_stats().running, 0);

    // A target the widget does not show is animated, from what was applied.
    let turn = session.update_with(&card_state(0.0, 0.0, 0), &env, &mut renderer);
    assert_eq!(
        trace(&turn.patches),
        "animate counter_card.opacity from=0.5 to=1.0 duration=200.0 delay=0.0 \
         easing=expo_out(6.0)\n"
    );
}

#[test]
fn only_a_session_with_a_renderer_drops_the_animate() {
    // `update` has no renderer to ask, so the same change is an `animate`.
    let (mut session, _renderer) = settled_card();
    let turn = session.update(&card_state(0.0, 1.0, 0), &phone());
    assert_eq!(turn.patches.len(), 1);
    assert!(matches!(
        turn.patches[0],
        lazyrad_view::Patch::Animate { .. }
    ));
}

#[test]
fn the_default_preset_vocabulary_is_what_the_planner_expands() {
    // A `Fade` enter, no translation; an `Edge::Top` slide goes up.
    let root = Node::new("root", "Column")
        .with_child(Node::new("fader", "Panel").with_motion(Motion::new().with_enter(Preset::Fade)))
        .with_child(
            Node::new("dropper", "Panel")
                .with_motion(Motion::new().with_enter(Preset::slide(Edge::Top, 24.0))),
        )
        .with_child(
            Node::new("comer", "Panel")
                .with_motion(Motion::new().with_enter(Preset::slide(Edge::Right, 16.0))),
        );
    let mut session = Session::new(tree(root), Bindings::new());
    let built = session.build(&rhai::Map::new(), &phone());
    let animates: Vec<String> = built
        .patches
        .iter()
        .filter(|patch| matches!(patch, lazyrad_view::Patch::Animate { .. }))
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        animates,
        [
            "animate fader.opacity from=0.0 to=1.0 duration=350.0 delay=0.0 easing=expo_out(6.0)",
            "animate dropper.opacity from=0.0 to=1.0 duration=350.0 delay=0.0 easing=expo_out(6.0)",
            "animate dropper.translate_y from=-24.0 to=0.0 duration=350.0 delay=0.0 \
             easing=expo_out(6.0)",
            "animate comer.opacity from=0.0 to=1.0 duration=350.0 delay=0.0 easing=expo_out(6.0)",
            "animate comer.translate_x from=16.0 to=0.0 duration=350.0 delay=0.0 \
             easing=expo_out(6.0)",
        ]
    );
}
