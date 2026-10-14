//! Exits through `visible`: the node is held while it animates out and hidden
//! once the renderer reports the animation finished.

mod common;

use common::{built, ended, phone, report, settle, tick, visible_state};
use lazyrad_view::{AnimationId, Motion, Node, Preset, Renderer, Value, trace};

/// A card that fades in and out, whose `visible` and `text` follow the state,
/// beside a plain label.
fn toggled() -> Node {
    Node::new("root", "Column")
        .with_child(Node::new("label", "Label").with_bind("text", "state.text"))
        .with_child(
            Node::new("card", "Panel")
                .with_bind("visible", "state.show")
                .with_bind("title", "state.text")
                .with_motion(
                    Motion::new()
                        .with_enter(Preset::Fade)
                        .with_exit(Preset::Fade),
                ),
        )
}

#[test]
fn a_hidden_node_is_held_while_it_fades_then_hidden_after_finished() {
    let (mut session, mut renderer) = built(toggled(), &visible_state(true, "one"));
    settle(&mut session, &mut renderer);

    // `show` goes false and the label's text changes in the same turn.
    session.update_with(&visible_state(false, "two"), &phone(), &mut renderer);
    assert_eq!(
        renderer.take_trace(),
        "set label.text = \"two\"\n\
         animate card.opacity from=1.0 to=0.0 duration=250.0 delay=0.0 easing=expo_in(6.0)\n"
    );
    // The card is held: still shown as far as the session knows, not diffed
    // (its title does not follow the text), and counted.
    assert_eq!(session.value("card", "visible"), Some(&Value::Bool(true)));
    assert_eq!(
        session.value("card", "title"),
        Some(&Value::Text("one".into()))
    );
    assert_eq!(session.motion_stats().held, 1);
    let turn = session.update_with(&visible_state(false, "three"), &phone(), &mut renderer);
    assert_eq!(trace(&turn.patches), "set label.text = \"three\"\n");
    renderer.take_trace();

    // Nothing is removed or hidden until the renderer says the fade ended.
    let frame = tick(&mut session, &mut renderer, 200.0);
    assert!(frame.finished.is_empty());
    assert_eq!(session.motion_stats().held, 1);
    assert_eq!(renderer.take_trace(), "");
    let frame = tick(&mut session, &mut renderer, 50.0);
    assert_eq!(ended(&frame), [("card".to_owned(), "opacity".to_owned())]);
    // It is hidden, and what the exit moved is back at rest for the next show.
    assert_eq!(
        renderer.take_trace(),
        "set card.visible = false\nunset card.opacity\n"
    );
    assert_eq!(session.motion_stats().held, 0);
    assert_eq!(session.value("card", "visible"), Some(&Value::Bool(false)));

    // The diff resumes: what changed while it was held is caught up.
    session.update_with(&visible_state(false, "three"), &phone(), &mut renderer);
    assert_eq!(renderer.take_trace(), "set card.title = \"three\"\n");
}

#[test]
fn a_shown_node_enters_every_time_it_is_shown() {
    let (mut session, mut renderer) = built(toggled(), &visible_state(true, "x"));
    settle(&mut session, &mut renderer);
    session.update_with(&visible_state(false, "x"), &phone(), &mut renderer);
    settle(&mut session, &mut renderer);

    for _ in 0..2 {
        session.update_with(&visible_state(true, "x"), &phone(), &mut renderer);
        assert_eq!(
            renderer.take_trace(),
            "set card.visible = true\n\
             animate card.opacity from=0.0 to=1.0 duration=350.0 delay=0.0 easing=expo_out(6.0)\n"
        );
        settle(&mut session, &mut renderer);
        session.update_with(&visible_state(false, "x"), &phone(), &mut renderer);
        settle(&mut session, &mut renderer);
    }
}

#[test]
fn a_node_created_hidden_enters_when_it_is_first_shown() {
    let (mut session, mut renderer) = built(toggled(), &visible_state(false, "x"));
    // Created hidden: no entrance yet (and no exit either).
    assert!(!renderer.take_trace().contains("animate"));
    session.update_with(&visible_state(true, "x"), &phone(), &mut renderer);
    assert_eq!(
        renderer.take_trace(),
        "set card.visible = true\n\
         animate card.opacity from=0.0 to=1.0 duration=350.0 delay=0.0 easing=expo_out(6.0)\n"
    );
}

#[test]
fn showing_a_node_again_while_it_exits_cancels_the_exit() {
    let (mut session, mut renderer) = built(toggled(), &visible_state(true, "x"));
    settle(&mut session, &mut renderer);
    session.update_with(&visible_state(false, "x"), &phone(), &mut renderer);
    tick(&mut session, &mut renderer, 100.0);
    renderer.take_trace();

    let turn = session.update_with(&visible_state(true, "x"), &phone(), &mut renderer);
    // Back to rest from wherever the fade is; no `set visible`, it never left.
    assert_eq!(
        trace(&turn.patches),
        "animate card.opacity from=current to=1.0 duration=250.0 delay=0.0 easing=expo_out(6.0)\n"
    );
    assert_eq!(session.motion_stats().held, 0);
    renderer.take_trace();
    // Its end must not hide it.
    let frame = tick(&mut session, &mut renderer, 250.0);
    assert_eq!(ended(&frame), [("card".to_owned(), "opacity".to_owned())]);
    assert_eq!(renderer.take_trace(), "");
    assert_eq!(session.value("card", "visible"), Some(&Value::Bool(true)));
}

#[test]
fn unknown_stale_and_duplicate_finished_calls_do_nothing() {
    let (mut session, mut renderer) = built(toggled(), &visible_state(true, "x"));
    settle(&mut session, &mut renderer);
    session.update_with(&visible_state(false, "x"), &phone(), &mut renderer);
    let exit = session
        .running_animation("card", "opacity")
        .expect("the exit's animation");

    // Not a node, not a property of it, not an exit's property, and not the
    // animation the exit waits for.
    assert!(session.finished("ghost", "opacity", exit).is_empty());
    assert!(session.finished("card", "translate_y", exit).is_empty());
    assert!(session.finished("label", "opacity", exit).is_empty());
    assert!(
        session
            .finished("card", "opacity", AnimationId(exit.0 + 100))
            .is_empty()
    );
    assert!(
        session
            .finished("card", "opacity", AnimationId(0))
            .is_empty()
    );
    assert_eq!(session.motion_stats().held, 1);
    assert_eq!(session.running_animation("card", "opacity"), Some(exit));

    let first = session.finished("card", "opacity", exit);
    assert_eq!(
        trace(&first),
        "set card.visible = false\nunset card.opacity\n"
    );
    // Again, and again: nothing, and the node is not hidden twice.
    assert!(session.finished("card", "opacity", exit).is_empty());
    assert!(session.finished("card", "opacity", exit).is_empty());
    assert_eq!(session.motion_stats().held, 0);
}

/// The usual event-queue race: the enter's end is delivered after the node
/// has started to leave. It names an animation that is no longer the current
/// one, so it must not complete the exit.
#[test]
fn a_late_report_of_the_enter_does_not_complete_the_exit() {
    let (mut session, mut renderer) = built(toggled(), &visible_state(true, "x"));
    let enter = session
        .running_animation("card", "opacity")
        .expect("the entrance");
    tick(&mut session, &mut renderer, 100.0);
    renderer.take_trace();

    // Hidden 100 ms into the entrance: the exit starts from where it is.
    let turn = session.update_with(&visible_state(false, "x"), &phone(), &mut renderer);
    assert_eq!(
        trace(&turn.patches),
        "animate card.opacity from=current to=0.0 duration=250.0 delay=0.0 easing=expo_in(6.0)\n"
    );
    let exit = session
        .running_animation("card", "opacity")
        .expect("the exit");
    assert_ne!(enter, exit);

    // The entrance's report arrives late: nothing happens.
    assert!(session.finished("card", "opacity", enter).is_empty());
    assert_eq!(session.motion_stats().held, 1);
    assert_eq!(session.value("card", "visible"), Some(&Value::Bool(true)));
    // Only the exit's own report ends it.
    let done = session.finished("card", "opacity", exit);
    assert_eq!(
        trace(&done),
        "set card.visible = false\nunset card.opacity\n"
    );
}

#[test]
fn a_quick_hide_show_hide_ignores_the_earlier_reports() {
    let (mut session, mut renderer) = built(toggled(), &visible_state(true, "x"));
    settle(&mut session, &mut renderer);

    session.update_with(&visible_state(false, "x"), &phone(), &mut renderer);
    let first_exit = session.running_animation("card", "opacity").unwrap();
    session.update_with(&visible_state(true, "x"), &phone(), &mut renderer);
    let comeback = session.running_animation("card", "opacity").unwrap();
    session.update_with(&visible_state(false, "x"), &phone(), &mut renderer);
    let second_exit = session.running_animation("card", "opacity").unwrap();
    assert!(first_exit < comeback && comeback < second_exit);

    // The first exit and the comeback end late, in either order: both stale.
    assert!(session.finished("card", "opacity", comeback).is_empty());
    assert!(session.finished("card", "opacity", first_exit).is_empty());
    assert_eq!(session.motion_stats().held, 1);
    assert_eq!(session.value("card", "visible"), Some(&Value::Bool(true)));
    // The second exit's own report hides it, once.
    assert_eq!(
        trace(&session.finished("card", "opacity", second_exit)),
        "set card.visible = false\nunset card.opacity\n"
    );
    assert!(session.finished("card", "opacity", second_exit).is_empty());
}

#[test]
fn the_ids_are_numbered_in_order_and_never_reused_across_a_rebuild() {
    let (mut session, mut renderer) = built(toggled(), &visible_state(true, "x"));
    let first = session.running_animation("card", "opacity").unwrap();
    assert_eq!(first, AnimationId(1));
    settle(&mut session, &mut renderer);
    assert_eq!(session.running_animation("card", "opacity"), None);
    // A rebuild forgets what was running, not what it numbered.
    session.update_with(&visible_state(false, "x"), &phone(), &mut renderer);
    let exit = session.running_animation("card", "opacity").unwrap();
    assert!(exit > first);
    renderer.apply(&session.build(&visible_state(true, "x"), &phone()).patches);
    assert_eq!(session.running_animation("card", "opacity"), None);
    // The report that was in flight across the rebuild is ignored.
    assert!(session.finished("card", "opacity", exit).is_empty());
    assert!(report(&mut session, "card", "opacity").is_empty());
}
