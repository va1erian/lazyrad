//! A renderer that misses a report must not hold a node, or a slot of the
//! animation limit, for the rest of the session: the host's `advance` tick
//! completes what is overdue, and `reset_motion` is the host's hard reset.

mod common;

use common::{map, phone, tree};
use lazyrad_view::limits::{ANIMATION_GRACE_MS, MAX_RUNNING_ANIMATIONS};
use lazyrad_view::motion::EXIT_MS;
use lazyrad_view::{
    AnimationId, Bindings, Motion, Node, Patch, Preset, RecordingRenderer, Renderer, Session,
    Transition, Value, trace,
};
use rhai::Dynamic;

/// `count` panels with a fade exit and an eased opacity that follows
/// `state.dim`.
fn panels(count: usize) -> Node {
    let mut root = Node::new("root", "Column");
    for index in 0..count {
        root = root.with_child(
            Node::new(format!("n{index}"), "Panel")
                .with_bind("opacity", "1.0 - state.dim * 0.5")
                .with_motion(
                    Motion::new()
                        .with_exit(Preset::Fade)
                        .with_change("opacity", Transition::new(100.0)),
                ),
        );
    }
    root
}

fn dim(value: f64) -> rhai::Map {
    map(&[("dim", Dynamic::from_float(value))])
}

fn count(patches: &[Patch], wanted: fn(&Patch) -> bool) -> usize {
    patches.iter().filter(|patch| wanted(patch)).count()
}

fn is_remove(patch: &Patch) -> bool {
    matches!(patch, Patch::Remove { .. })
}

fn is_animate(patch: &Patch) -> bool {
    matches!(patch, Patch::Animate { .. })
}

fn is_set(patch: &Patch) -> bool {
    matches!(patch, Patch::Set { .. })
}

/// The probe of the review: 300 nodes with an exit fade are removed against a
/// renderer that never reports.
#[test]
fn a_renderer_that_never_reports_holds_the_nodes_until_the_host_ticks() {
    let total = MAX_RUNNING_ANIMATIONS + 44;
    let mut session = Session::new(tree(panels(total)), Bindings::new());
    session.build(&dim(0.0), &phone());
    let turn = session.reconcile(tree(Node::new("root", "Column")), &dim(0.0), &phone());

    // The 256 that found a slot fade out and are held; 44 had none and went at
    // once.
    assert_eq!(count(&turn.patches, is_animate), MAX_RUNNING_ANIMATIONS);
    assert_eq!(count(&turn.patches, is_remove), 44);
    let stats = session.motion_stats();
    assert_eq!(stats.held, MAX_RUNNING_ANIMATIONS);
    assert_eq!(stats.running, MAX_RUNNING_ANIMATIONS);
    assert_eq!(stats.degraded, 44);
    assert_eq!(session.tree().len(), 1 + MAX_RUNNING_ANIMATIONS);

    // Without a tick nothing completes them: that is the contract, and the
    // reason a host must call `advance`.
    assert!(session.advance(0.0).is_empty());
    // Not before the exit and the grace have passed on the session's clock.
    let due = EXIT_MS + ANIMATION_GRACE_MS;
    assert!(session.advance(due - 1.0).is_empty());
    assert_eq!(session.motion_stats().held, MAX_RUNNING_ANIMATIONS);
    // Then every one is removed, exactly as `finished` would have.
    let done = session.advance(1.0);
    assert_eq!(count(&done, is_remove), MAX_RUNNING_ANIMATIONS);
    assert_eq!(done.len(), MAX_RUNNING_ANIMATIONS);
    let stats = session.motion_stats();
    assert_eq!((stats.held, stats.running), (0, 0));
    assert_eq!(session.tree().len(), 1);
    // And nothing is left to do.
    assert!(session.advance(10_000.0).is_empty());
}

#[test]
fn unreported_changes_stop_pinning_the_limit_once_the_host_ticks() {
    let total = MAX_RUNNING_ANIMATIONS + 44;
    let mut session = Session::new(tree(panels(total)), Bindings::new());
    session.build(&dim(0.0), &phone());
    let turn = session.update(&dim(1.0), &phone());
    assert_eq!(count(&turn.patches, is_animate), MAX_RUNNING_ANIMATIONS);
    assert_eq!(count(&turn.patches, is_set), 44);
    assert_eq!(session.motion_stats().running, MAX_RUNNING_ANIMATIONS);

    // The renderer never reports. Without a tick every later change of a node
    // that is not running is an instant `Set`...
    let turn = session.update(&dim(0.0), &phone());
    assert_eq!(count(&turn.patches, is_animate), MAX_RUNNING_ANIMATIONS);
    assert_eq!(session.motion_stats().running, MAX_RUNNING_ANIMATIONS);

    // ...until the host ticks: the changes are overdue, and the room is back.
    assert!(session.advance(100.0 + ANIMATION_GRACE_MS).is_empty());
    assert_eq!(session.motion_stats().running, 0);
    let turn = session.update(&dim(1.0), &phone());
    assert_eq!(count(&turn.patches, is_animate), MAX_RUNNING_ANIMATIONS);
    assert_eq!(count(&turn.patches, is_set), 44);
}

#[test]
fn a_late_report_after_the_tick_changes_nothing() {
    let mut session = Session::new(tree(panels(2)), Bindings::new());
    session.build(&dim(0.0), &phone());
    session.reconcile(
        tree(Node::new("root", "Column").with_child(Node::new("n1", "Panel"))),
        &dim(0.0),
        &phone(),
    );
    let exit = session.running_animation("n0", "opacity").expect("an exit");
    let done = session.advance(EXIT_MS + ANIMATION_GRACE_MS);
    assert_eq!(trace(&done), "remove n0\n");

    // The renderer's report arrives after the tick completed it.
    assert!(session.finished("n0", "opacity", exit).is_empty());
    assert_eq!(session.tree().len(), 2);
}

#[test]
fn the_tick_ignores_steps_that_are_not_time() {
    let mut session = Session::new(tree(panels(1)), Bindings::new());
    session.build(&dim(0.0), &phone());
    session.reconcile(tree(Node::new("root", "Column")), &dim(0.0), &phone());
    assert_eq!(session.motion_stats().held, 1);
    for step in [f64::NAN, -1.0e9, f64::INFINITY, f64::NEG_INFINITY, 0.0] {
        assert!(session.advance(step).is_empty(), "{step}");
        assert_eq!(session.motion_stats().held, 1, "{step}");
    }
    // A very long step is time, and completes it.
    assert_eq!(trace(&session.advance(1.0e9)), "remove n0\n");
}

/// A step so large that `now + delay + duration + grace` would equal `now` must
/// not make every later animation overdue at once.
#[test]
fn a_degenerate_step_does_not_poison_the_clock() {
    let mut session = Session::new(tree(panels(1)), Bindings::new());
    session.build(&dim(0.0), &phone());
    for step in [f64::MAX, f64::MAX, 1.0e300, 1.0e18] {
        assert!(session.advance(step).is_empty(), "{step}");
    }
    // An animation planned afterwards runs for its own time (a hundred
    // milliseconds, then the grace), not for none.
    let turn = session.update(&dim(1.0), &phone());
    assert_eq!(count(&turn.patches, is_animate), 1);
    assert_eq!(session.motion_stats().running, 1);
    assert!(session.advance(0.0).is_empty());
    assert!(session.advance(100.0 + ANIMATION_GRACE_MS - 1.0).is_empty());
    assert_eq!(session.motion_stats().running, 1);
    assert!(session.advance(1.0).is_empty());
    assert_eq!(session.motion_stats().running, 0);
    // The huge step itself completes what was already overdue by any measure.
    session.update(&dim(0.0), &phone());
    assert_eq!(session.motion_stats().running, 1);
    assert!(session.advance(f64::MAX).is_empty());
    assert_eq!(session.motion_stats().running, 0);
}

#[test]
fn a_hidden_node_is_hidden_by_the_tick_and_restored_like_finished_would() {
    let root = Node::new("root", "Column").with_child(
        Node::new("card", "Panel")
            .with_bind("visible", "state.show")
            .with_prop("opacity", 0.8)
            .with_motion(Motion::new().with_exit(Preset::Fade)),
    );
    let mut session = Session::new(tree(root), Bindings::new());
    let state = |show: bool| map(&[("show", Dynamic::from(show))]);
    session.build(&state(true), &phone());
    let turn = session.update(&state(false), &phone());
    assert_eq!(count(&turn.patches, is_animate), 1);
    assert_eq!(
        trace(&session.advance(EXIT_MS + ANIMATION_GRACE_MS)),
        "set card.visible = false\nset card.opacity = 0.8\n"
    );
    assert_eq!(session.motion_stats().held, 0);
    assert!(!session.is_leaving("card"));
}

#[test]
fn reset_motion_drops_everything_and_puts_it_at_rest() {
    let root = Node::new("root", "Column")
        .with_child(
            Node::new("leaving", "Panel").with_motion(Motion::new().with_exit(Preset::Fade)),
        )
        .with_child(
            Node::new("hiding", "Panel")
                .with_bind("visible", "state.show")
                .with_prop("opacity", 0.8)
                .with_motion(Motion::new().with_exit(Preset::Fade)),
        )
        .with_child(
            Node::new("easing", "Panel")
                .with_bind("translate_y", "state.lift")
                .with_motion(Motion::new().with_change("translate_y", Transition::new(300.0))),
        )
        .with_child(
            Node::new("pressed", "Button")
                .with_motion(Motion::new().with_press(Preset::PressScale)),
        );
    let state = |show: bool, lift: f64| {
        map(&[
            ("show", Dynamic::from(show)),
            ("lift", Dynamic::from_float(lift)),
        ])
    };
    let mut session = Session::new(tree(root.clone()), Bindings::new());
    let mut renderer = RecordingRenderer::new();
    renderer.apply(&session.build(&state(true, 0.0), &phone()).patches);

    // One node leaves, one hides, one eases, one is pressed.
    let mut without_leaving = root.clone();
    without_leaving.children.remove(0);
    renderer.apply(
        &session
            .reconcile(tree(without_leaving), &state(false, 8.0), &phone())
            .patches,
    );
    renderer.apply(&session.press_begin("pressed"));
    assert!(session.is_leaving("leaving"));
    assert!(session.is_leaving("hiding"));
    let before = session.motion_stats();
    assert_eq!(before.held, 2);
    assert!(before.running >= 4);

    let old = session
        .running_animation("easing", "translate_y")
        .expect("a change");
    let patches = session.reset_motion();
    renderer.apply(&patches);
    assert_eq!(
        trace(&patches),
        "set hiding.visible = false\n\
         set hiding.opacity = 0.8\n\
         remove leaving\n\
         set easing.translate_y = 8.0\n\
         unset pressed.scale_x\n\
         unset pressed.scale_y\n"
    );
    let after = session.motion_stats();
    assert_eq!((after.running, after.held), (0, 0));
    assert!(!session.is_leaving("hiding"));
    assert_eq!(session.running_animation("easing", "translate_y"), None);
    assert_eq!(session.tree().len(), 4);
    // What was in flight is forgotten: its report is stale, and a release of
    // the press has nothing to release.
    assert!(session.finished("easing", "translate_y", old).is_empty());
    assert!(session.press_end("pressed").is_empty());
    assert_eq!(
        session.value("hiding", "visible"),
        Some(&Value::Bool(false))
    );
    // A second reset has nothing to do.
    assert!(session.reset_motion().is_empty());
    // And the ids go on: nothing is reused.
    let next = session.update(&state(false, 0.0), &phone());
    let Some(Patch::Animate { id, .. }) = next.patches.first() else {
        panic!("{}", trace(&next.patches));
    };
    assert!(id.0 > old.0);
    assert_ne!(*id, AnimationId(0));
}
