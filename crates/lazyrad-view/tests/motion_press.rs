//! `PressScale`: feedback while a node is pressed, on the plan's card.

mod common;

use common::{card_motion, line, profile_card, settled_card, tick, tree};
use lazyrad_view::{Bindings, Renderer, Session, trace};

#[test]
fn a_pressed_card_scales_to_096_and_returns_after_a_delay() {
    let (mut session, mut renderer) = settled_card();
    let begin = session.press_begin("counter_card");
    assert_eq!(
        trace(&begin),
        "animate counter_card.scale_x from=1.0 to=0.96 duration=100.0 delay=0.0 \
         easing=expo_out(6.0)\n\
         animate counter_card.scale_y from=1.0 to=0.96 duration=100.0 delay=0.0 \
         easing=expo_out(6.0)\n"
    );
    renderer.apply(&begin);
    let frame = tick(&mut session, &mut renderer, 50.0);
    assert_eq!(
        line(&frame),
        "t=400 counter_card.scale_x=0.9619 counter_card.scale_y=0.9619"
    );
    tick(&mut session, &mut renderer, 50.0);
    assert_eq!(session.motion_stats().running, 0);

    // The release holds for 200 ms, then eases back over 100 ms.
    let end = session.press_end("counter_card");
    assert_eq!(
        trace(&end),
        "animate counter_card.scale_x from=0.96 to=1.0 duration=100.0 delay=200.0 \
         easing=cubic_out\n\
         animate counter_card.scale_y from=0.96 to=1.0 duration=100.0 delay=200.0 \
         easing=cubic_out\n"
    );
    renderer.apply(&end);
    let samples: Vec<String> = [0.0, 200.0, 50.0, 50.0]
        .iter()
        .map(|dt| line(&tick(&mut session, &mut renderer, *dt)))
        .collect();
    assert_eq!(
        samples,
        [
            "t=450 counter_card.scale_x=0.9600 counter_card.scale_y=0.9600",
            "t=650 counter_card.scale_x=0.9600 counter_card.scale_y=0.9600",
            "t=700 counter_card.scale_x=0.9950 counter_card.scale_y=0.9950",
            "t=750 counter_card.scale_x=1.0000 counter_card.scale_y=1.0000",
        ]
    );
}

#[test]
fn a_release_during_the_press_in_starts_from_current_and_presses_are_idempotent() {
    let (mut session, mut renderer) = settled_card();
    renderer.apply(&session.press_begin("counter_card"));
    // A second press while pressed, and a release with no press, do nothing.
    assert!(session.press_begin("counter_card").is_empty());
    tick(&mut session, &mut renderer, 20.0);
    let end = session.press_end("counter_card");
    assert!(
        trace(&end).contains("from=current to=1.0"),
        "{}",
        trace(&end)
    );
    assert!(session.press_end("counter_card").is_empty());
    assert!(session.press_end("add_button").is_empty());
    // Not pressable: no press preset, an unknown name, or nothing built.
    assert!(session.press_begin("add_button").is_empty());
    assert!(session.press_begin("nope").is_empty());
    let mut unbuilt = Session::new(tree(profile_card(card_motion())), Bindings::new());
    assert!(unbuilt.press_begin("counter_card").is_empty());
}
