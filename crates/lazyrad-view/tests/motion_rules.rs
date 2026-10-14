//! Rules of the planner that a first review found loose: what a cancelled exit
//! does at the limit, where a slide starts, one animation per property in a
//! batch, the `rebuilt` flag and the interruption rules.

mod common;

use common::{map, phone, tree};
use lazyrad_view::limits::MAX_RUNNING_ANIMATIONS;
use lazyrad_view::{
    AnimFrom, Bindings, Edge, Environment, Motion, Node, Patch, Preset, RecordingRenderer,
    Renderer, SamplingRenderer, Session, Transition, trace,
};
use rhai::Dynamic;

fn state(show: bool, dim: f64) -> rhai::Map {
    map(&[
        ("show", Dynamic::from(show)),
        ("dim", Dynamic::from_float(dim)),
    ])
}

fn animations(patches: &[Patch]) -> Vec<&Patch> {
    patches
        .iter()
        .filter(|patch| matches!(patch, Patch::Animate { .. }))
        .collect()
}

/// The review's probe: a card whose slide-out has half finished, with the
/// limit reached, is shown again.
#[test]
fn a_cancelled_exit_that_cannot_animate_puts_the_property_at_rest() {
    let mut root = Node::new("root", "Column").with_child(
        Node::new("card", "Panel")
            .with_bind("visible", "state.show")
            .with_motion(Motion::new().with_exit(Preset::slide(Edge::Bottom, 40.0))),
    );
    for index in 0..MAX_RUNNING_ANIMATIONS + 44 {
        root = root.with_child(
            Node::new(format!("f{index}"), "Panel")
                .with_bind("opacity", "1.0 - state.dim * 0.5")
                .with_motion(Motion::new().with_change("opacity", Transition::new(100.0))),
        );
    }
    let mut session = Session::new(tree(root), Bindings::new());
    session.build(&state(true, 0.0), &phone());

    // The card starts to slide out; its fade (the opacity) is reported over.
    let turn = session.update(&state(false, 0.0), &phone());
    assert_eq!(animations(&turn.patches).len(), 2);
    let fade = session.running_animation("card", "opacity").unwrap();
    assert!(session.finished("card", "opacity", fade).is_empty());
    assert_eq!(session.running_animation("card", "opacity"), None);

    // Everything else fills the limit.
    session.update(&state(false, 1.0), &phone());
    assert_eq!(session.motion_stats().running, MAX_RUNNING_ANIMATIONS);

    // Shown again: the slide (still in flight) eases back, and the fade, which
    // finds no room, is put at rest at once instead of being left at zero.
    let turn = session.update(&state(true, 1.0), &phone());
    assert_eq!(
        trace(&turn.patches),
        "set card.opacity = 1.0\n\
         animate card.translate_y from=current to=0.0 duration=250.0 delay=0.0 \
         easing=expo_out(6.0)\n"
    );
    assert_eq!(session.motion_stats().held, 0);
    assert!(!session.is_leaving("card"));
}

#[test]
fn a_slide_is_relative_to_where_the_node_rests() {
    let root = |with_card: bool| {
        let mut root = Node::new("root", "Column").with_child(Node::new("a", "Label"));
        if with_card {
            root = root
                .with_child(
                    Node::new("card", "Panel")
                        .with_prop("translate_y", 100.0)
                        .with_motion(
                            Motion::new()
                                .with_enter(Preset::slide(Edge::Bottom, 40.0))
                                .with_exit(Preset::slide(Edge::Bottom, 40.0)),
                        ),
                )
                .with_child(
                    Node::new("side", "Panel")
                        .with_prop("translate_x", 10.0)
                        .with_motion(Motion::new().with_enter(Preset::slide(Edge::Left, 40.0))),
                );
        }
        root
    };
    let mut session = Session::new(tree(root(true)), Bindings::new());
    let built = session.build(&rhai::Map::new(), &phone());
    let enters: Vec<String> = animations(&built.patches)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        enters,
        [
            "animate card.opacity from=0.0 to=1.0 duration=350.0 delay=0.0 easing=expo_out(6.0)",
            // 40 below its place at 100, not 40 from zero.
            "animate card.translate_y from=140.0 to=100.0 duration=350.0 delay=0.0 \
             easing=expo_out(6.0)",
            "animate side.opacity from=0.0 to=1.0 duration=350.0 delay=0.0 easing=expo_out(6.0)",
            // And 40 to the left of its place at 10.
            "animate side.translate_x from=-30.0 to=10.0 duration=350.0 delay=0.0 \
             easing=expo_out(6.0)",
        ]
    );
    // The same on the way out: 40 below its place again (the entrances are
    // still in flight, so the exits start from `current`).
    let mut only_side = Node::new("root", "Column").with_child(Node::new("a", "Label"));
    only_side = only_side.with_child(
        Node::new("side", "Panel")
            .with_prop("translate_x", 10.0)
            .with_motion(Motion::new().with_enter(Preset::slide(Edge::Left, 40.0))),
    );
    let turn = session.reconcile(tree(only_side), &rhai::Map::new(), &phone());
    assert_eq!(
        trace(&turn.patches),
        "animate card.opacity from=current to=0.0 duration=250.0 delay=0.0 easing=expo_in(6.0)\n\
         animate card.translate_y from=current to=140.0 duration=250.0 delay=0.0 \
         easing=expo_in(6.0)\n"
    );
}

/// An enter that follows a change in the same batch: the renderer is told one
/// thing about the property.
#[test]
fn one_animate_per_property_in_a_batch_the_last_one_planned() {
    let root = Node::new("root", "Column").with_child(
        Node::new("card", "Panel")
            .with_bind("visible", "state.show")
            .with_bind("opacity", "1.0 - state.dim * 0.5")
            .with_motion(
                Motion::new()
                    .with_enter(Preset::Fade)
                    .with_change("opacity", Transition::new(200.0)),
            ),
    );
    let mut session = Session::new(tree(root), Bindings::new());
    session.build(&state(false, 0.0), &phone());

    // Shown and dimmed in one turn: the `change` of the opacity and the
    // entrance both want it.
    let turn = session.update(&state(true, 1.0), &phone());
    assert_eq!(
        trace(&turn.patches),
        "set card.visible = true\n\
         animate card.opacity from=0.0 to=0.5 duration=350.0 delay=0.0 easing=expo_out(6.0)\n"
    );
    let Some(Patch::Animate { id, .. }) = turn.patches.last() else {
        panic!("{}", trace(&turn.patches));
    };
    assert_eq!(session.running_animation("card", "opacity"), Some(*id));
    assert_eq!(session.motion_stats().running, 1);
}

#[test]
fn a_cancel_followed_by_a_new_exit_in_one_reconcile_is_one_animate() {
    let tree_of = |with_b: bool| {
        let mut root = Node::new("root", "Column").with_child(Node::new("a", "Label"));
        if with_b {
            root = root.with_child(
                Node::new("b", "Panel")
                    .with_bind("visible", "state.show")
                    .with_motion(Motion::new().with_exit(Preset::Fade)),
            );
        }
        root
    };
    let mut session = Session::new(tree(tree_of(true)), Bindings::new());
    session.build(&state(true, 0.0), &phone());
    session.reconcile(tree(tree_of(false)), &state(true, 0.0), &phone());
    assert_eq!(session.motion_stats().held, 1);

    // `b` comes back in the same call that hides it: the cancel (to rest) and
    // the new exit (to nothing) are both planned, and only the exit is sent.
    let turn = session.reconcile(tree(tree_of(true)), &state(false, 0.0), &phone());
    assert_eq!(
        trace(&turn.patches),
        "animate b.opacity from=current to=0.0 duration=250.0 delay=0.0 easing=expo_in(6.0)\n"
    );
    let Some(Patch::Animate { id, .. }) = turn.patches.first() else {
        panic!("one animate");
    };
    assert_eq!(session.running_animation("b", "opacity"), Some(*id));
    assert_eq!(session.motion_stats().held, 1);
    // Its own report is what hides it.
    assert_eq!(
        trace(&session.finished("b", "opacity", *id)),
        "set b.visible = false\nunset b.opacity\n"
    );
}

#[test]
fn the_rebuilt_flag_says_when_the_patches_are_the_whole_tree() {
    let root = |extra: bool| {
        let mut root = Node::new("root", "Column").with_child(Node::new("a", "Label"));
        if extra {
            root = root.with_child(Node::new("b", "Label"));
        }
        root
    };
    let mut session = Session::new(tree(root(false)), Bindings::new());
    let env = phone();
    let none = rhai::Map::new();
    // An update of a session that is not built builds.
    assert!(session.update(&none, &env).rebuilt);
    assert!(session.build(&none, &env).rebuilt);
    assert!(!session.update(&none, &env).rebuilt);
    // A reconcile that applies a difference does not rebuild...
    let turn = session.reconcile(tree(root(true)), &none, &env);
    assert!(!turn.rebuilt);
    assert!(matches!(turn.patches[0], Patch::Create { .. }));
    // ...and one that cannot, does, whatever its patches start with.
    let moved = Node::new("root", "Column")
        .with_child(Node::new("b", "Label").with_child(Node::new("a", "Label")));
    let turn = session.reconcile(tree(moved), &none, &env);
    assert!(turn.rebuilt);
    // `replace_tree` leaves the session unbuilt: the next turn builds.
    session.replace_tree(tree(root(false)));
    assert!(session.update(&none, &env).rebuilt);
}

#[test]
fn an_enter_replaces_what_is_in_flight_instead_of_continuing_it() {
    let root = Node::new("root", "Column").with_child(
        Node::new("card", "Panel")
            .with_bind("visible", "state.show")
            .with_bind("opacity", "1.0 - state.dim * 0.5")
            .with_motion(
                Motion::new()
                    .with_enter(Preset::Fade)
                    .with_change("opacity", Transition::new(200.0)),
            ),
    );
    let mut session = Session::new(tree(root), Bindings::new());
    let mut renderer = SamplingRenderer::new();
    renderer.apply(&session.build(&state(true, 0.0), &phone()).patches);
    common::settle(&mut session, &mut renderer);

    // A change is in flight when the node is hidden (no exit) and shown.
    session.update_with(&state(true, 1.0), &phone(), &mut renderer);
    assert_eq!(session.motion_stats().running, 1);
    renderer.take_trace();
    session.update_with(&state(false, 1.0), &phone(), &mut renderer);
    assert_eq!(renderer.take_trace(), "set card.visible = false\n");
    session.update_with(&state(true, 1.0), &phone(), &mut renderer);
    // The entrance starts from its own value, not from `current`.
    assert_eq!(
        renderer.take_trace(),
        "set card.visible = true\n\
         animate card.opacity from=0.0 to=0.5 duration=350.0 delay=0.0 easing=expo_out(6.0)\n"
    );
    assert_eq!(session.motion_stats().running, 1);
}

#[test]
fn flipping_reduced_motion_mid_flight_leaves_the_animation_until_a_set_replaces_it() {
    let root = Node::new("root", "Column").with_child(
        Node::new("card", "Panel")
            .with_bind("opacity", "1.0 - state.dim * 0.5")
            .with_motion(Motion::new().with_change("opacity", Transition::new(200.0))),
    );
    let mut session = Session::new(tree(root), Bindings::new());
    let mut renderer = SamplingRenderer::new();
    renderer.apply(&session.build(&state(true, 0.0), &phone()).patches);
    renderer.take_trace();
    session.update_with(&state(true, 1.0), &phone(), &mut renderer);
    assert_eq!(session.motion_stats().running, 1);
    let calm = Environment {
        reduce_motion: true,
        ..phone()
    };
    // The flip alone touches nothing in flight.
    let turn = session.update_with(&state(true, 1.0), &calm, &mut renderer);
    assert!(turn.patches.is_empty());
    assert_eq!(session.motion_stats().running, 1);
    assert_eq!(renderer.running(), 1);
    // The next change of that property is a `Set`, which replaces it.
    session.update_with(&state(true, 0.0), &calm, &mut renderer);
    assert_eq!(
        renderer.take_trace().lines().last(),
        Some("set card.opacity = 1.0")
    );
    assert_eq!(session.motion_stats().running, 0);
    assert_eq!(renderer.running(), 0);
}

#[test]
fn the_planner_never_hands_a_renderer_a_from_it_cannot_resolve() {
    // A `change` with nothing in flight is `from=<value>`; one in flight is
    // `current`. Never both, never neither.
    let root = Node::new("root", "Column").with_child(
        Node::new("card", "Panel")
            .with_bind("opacity", "1.0 - state.dim * 0.5")
            .with_motion(Motion::new().with_change("opacity", Transition::new(200.0))),
    );
    let mut session = Session::new(tree(root), Bindings::new());
    let mut renderer = RecordingRenderer::new();
    renderer.apply(&session.build(&state(true, 0.0), &phone()).patches);
    let first = session.update(&state(true, 1.0), &phone());
    let second = session.update(&state(true, 0.0), &phone());
    let from = |turn: &lazyrad_view::TurnResult| match &turn.patches[0] {
        Patch::Animate { from, .. } => *from,
        other => panic!("{other}"),
    };
    assert_eq!(from(&first), AnimFrom::Value(1.0));
    assert_eq!(from(&second), AnimFrom::Current);
}
