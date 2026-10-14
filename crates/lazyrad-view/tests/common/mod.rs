//! Helpers shared by the integration tests.
#![allow(dead_code)] // Each test file uses a different subset.

use lazyrad_view::{
    Bindings, Edge, Environment, Frame, Motion, Node, Preset, Renderer, SamplingRenderer, Session,
    Transition, Tree,
};
use rhai::{Dynamic, Map};

/// A state map from `(key, value)` pairs.
pub fn map(entries: &[(&str, Dynamic)]) -> Map {
    entries
        .iter()
        .map(|(key, value)| ((*key).into(), value.clone()))
        .collect()
}

pub fn text(value: &str) -> Dynamic {
    Dynamic::from(value.to_owned())
}

pub fn int(value: i64) -> Dynamic {
    Dynamic::from(value)
}

/// A phone-width environment (Compact).
pub fn phone() -> Environment {
    Environment::new(360.0, 640.0)
}

/// A validated tree, panicking on an invalid one.
pub fn tree(root: Node) -> Tree {
    Tree::new(root).expect("valid tree")
}

/// The motion of the plan's card (docs/modern-ui-plan.md section 4.8): it
/// slides up 40 dp and fades in, fades out, eases its opacity (200 ms) and its
/// `translate_y` (300 ms), and scales down while pressed.
pub fn card_motion() -> Motion {
    Motion::new()
        .with_enter(Preset::slide(Edge::Bottom, 40.0))
        .with_exit(Preset::Fade)
        .with_change("opacity", Transition::new(200.0))
        .with_change("translate_y", Transition::new(300.0))
        .with_press(Preset::PressScale)
}

/// The `profile_form` of section 4.2 reduced to what motion needs: a card
/// with `motion`, whose `translate_y` follows `state.lift` (0.0 or 1.0, eight
/// dp up) and whose `opacity` follows `state.dim`, and a label beside it.
pub fn profile_card(motion: Motion) -> Node {
    Node::new("profile_form", "Column")
        .with_prop("padding", 16)
        .with_child(Node::new("title_label", "Label").with_prop("text", "Profile"))
        .with_child(
            Node::new("counter_card", "Panel")
                .with_prop("card", true)
                .with_bind("translate_y", "0.0 - state.lift * 8.0")
                .with_bind("opacity", "1.0 - state.dim * 0.5")
                .with_motion(motion)
                .with_child(
                    Node::new("count_label", "Label")
                        .with_bind("text", "`Clicked ${state.count} times`"),
                )
                .with_child(Node::new("add_button", "Button").with_prop("text", "Add one")),
        )
}

/// State for [`profile_card`]: `lift` and `dim` are floats.
pub fn card_state(lift: f64, dim: f64, count: i64) -> rhai::Map {
    let mut state = rhai::Map::new();
    state.insert("lift".into(), Dynamic::from_float(lift));
    state.insert("dim".into(), Dynamic::from_float(dim));
    state.insert("count".into(), Dynamic::from(count));
    state
}

/// [`phone`] with reduced motion on.
pub fn reduced() -> Environment {
    let mut env = phone();
    env.reduce_motion = true;
    env
}

/// `t=100 node.property=value ...`, values to four decimals.
pub fn line(frame: &Frame) -> String {
    let mut text = format!("t={}", frame.time_ms);
    for sample in &frame.samples {
        text.push_str(&format!(
            " {}.{}={:.4}",
            sample.node, sample.property, sample.value
        ));
    }
    text
}

/// The profile card with [`card_motion`], built and applied, its entrance
/// still to play.
pub fn built_card(env: &Environment) -> (Session, SamplingRenderer) {
    let mut session = Session::new(tree(profile_card(card_motion())), Bindings::new());
    let mut renderer = SamplingRenderer::new();
    let turn = session.build(&card_state(0.0, 0.0, 0), env);
    assert!(turn.diagnostics.is_empty(), "{:?}", turn.diagnostics);
    renderer.apply(&turn.patches);
    (session, renderer)
}

/// [`built_card`] on a phone, its entrance played out and its trace taken.
pub fn settled_card() -> (Session, SamplingRenderer) {
    let (mut session, mut renderer) = built_card(&phone());
    tick(&mut session, &mut renderer, 350.0);
    renderer.take_trace();
    assert_eq!(session.motion_stats().running, 0);
    (session, renderer)
}

/// Moves a [`SamplingRenderer`]'s virtual clock by `dt_ms`, reports every
/// animation that ended to the session as a real renderer does, applies what
/// that returns, and gives back the frame.
pub fn tick(session: &mut Session, renderer: &mut SamplingRenderer, dt_ms: f64) -> Frame {
    let frame = renderer.advance(dt_ms);
    for (node, property) in &frame.finished {
        let patches = session.finished(node, property);
        renderer.apply(&patches);
    }
    frame
}

pub fn visible_state(visible: bool, text: &str) -> rhai::Map {
    map(&[
        ("show", Dynamic::from(visible)),
        ("text", Dynamic::from(text.to_owned())),
    ])
}

pub fn built(root: Node, state: &rhai::Map) -> (Session, SamplingRenderer) {
    let mut session = Session::new(tree(root), Bindings::new());
    let mut renderer = SamplingRenderer::new();
    renderer.apply(&session.build(state, &phone()).patches);
    (session, renderer)
}

/// Plays whatever is running to its end.
pub fn settle(session: &mut Session, renderer: &mut SamplingRenderer) {
    tick(session, renderer, 10_000.0);
    renderer.take_trace();
}
