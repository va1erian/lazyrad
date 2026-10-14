//! Reduced motion: `Slide` resolves to `Fade`, `PressScale` is dropped and a
//! `change` is an instant `Set`.

mod common;

use common::{built_card, card_state, line, phone, reduced, settled_card, tick, tree};
use lazyrad_view::{Bindings, Node, RecordingRenderer, Renderer, Session, Value};

const BUILD: &str = r#"create profile_form Column parent=- index=0 {padding=16}
create title_label Label parent=profile_form index=0 {text="Profile"}
create counter_card Panel parent=profile_form index=1 {card=true, opacity=1.0, translate_y=0.0}
create count_label Label parent=counter_card index=0 {text="Clicked 0 times"}
create add_button Button parent=counter_card index=1 {text="Add one"}
"#;

#[test]
fn reduced_motion_makes_the_entrance_a_plain_fade() {
    let (mut session, mut renderer) = built_card(&reduced());
    // `Slide` is a `Fade`: no translation at all.
    assert_eq!(
        renderer.take_trace(),
        format!(
            "{BUILD}animate counter_card.opacity from=0.0 to=1.0 duration=350.0 delay=0.0 \
             easing=expo_out(6.0)\n"
        )
    );
    let frame = tick(&mut session, &mut renderer, 100.0);
    assert_eq!(line(&frame), "t=100 counter_card.opacity=0.8219");
}

#[test]
fn reduced_motion_makes_a_change_instant_and_drops_the_press() {
    let (mut session, mut renderer) = built_card(&reduced());
    tick(&mut session, &mut renderer, 350.0);
    renderer.take_trace();
    // The same lift that eases in `a_change_eases_...` is a `set` here.
    session.update_with(&card_state(1.0, 0.0, 0), &reduced(), &mut renderer);
    assert_eq!(
        renderer.take_trace(),
        "set counter_card.translate_y = -8.0\n"
    );
    assert_eq!(session.motion_stats().running, 0);
    // The press is dropped, in both directions.
    assert!(session.press_begin("counter_card").is_empty());
    assert!(session.press_end("counter_card").is_empty());
}

#[test]
fn reduced_motion_can_change_while_the_view_is_up() {
    let (mut session, mut renderer) = settled_card();
    // Calm: eased.
    session.update_with(&card_state(1.0, 0.0, 0), &phone(), &mut renderer);
    assert!(
        renderer
            .take_trace()
            .starts_with("animate counter_card.translate_y")
    );
    tick(&mut session, &mut renderer, 300.0);
    // The user asks for reduced motion: the next change is instant, and a
    // press is dropped.
    session.update_with(&card_state(0.0, 0.0, 0), &reduced(), &mut renderer);
    assert_eq!(
        renderer.take_trace(),
        "set counter_card.translate_y = 0.0\n"
    );
    assert!(session.press_begin("counter_card").is_empty());
    // And back: eased, and pressable, again.
    session.update_with(&card_state(1.0, 0.0, 0), &phone(), &mut renderer);
    assert!(
        renderer
            .take_trace()
            .starts_with("animate counter_card.translate_y")
    );
    assert!(!session.press_begin("counter_card").is_empty());
}

/// A binding reads `env.reduce_motion` and follows it, like any other part of
/// the environment.
#[test]
fn a_binding_can_follow_the_reduced_motion_setting() {
    let root = Node::new("root", "Column").with_child(Node::new("hint", "Label").with_bind(
        "text",
        "if env.reduce_motion { \"calm\" } else { \"lively\" }",
    ));
    let mut session = Session::new(tree(root), Bindings::new());
    let mut renderer = RecordingRenderer::new();
    let state = rhai::Map::new();
    renderer.apply(&session.build(&state, &phone()).patches);
    assert_eq!(
        renderer.live_value("hint", "text"),
        Some(Value::Text("lively".into()))
    );
    session.update_with(&state, &reduced(), &mut renderer);
    assert_eq!(
        renderer.take_trace().lines().last(),
        Some("set hint.text = \"calm\"")
    );
    // Nothing else changed: the same environment is not a change.
    let again = session.update_with(&state, &reduced(), &mut renderer);
    assert!(again.patches.is_empty());
}
