//! The tree changing under a built session: nodes that leave through their
//! `exit`, nodes that arrive, and the identity an `enter` is gated on.

mod common;

use common::{built, map, phone, settle, tick, tree, visible_state};
use lazyrad_view::{
    Bindings, Edge, Motion, Node, Patch, Preset, RecordingRenderer, Renderer, Session, trace,
};
use rhai::Dynamic;

fn row(names: &[&str]) -> Node {
    let mut root = Node::new("root", "Column");
    for name in names {
        root = root.with_child(
            Node::new(*name, "Panel").with_motion(
                Motion::new()
                    .with_enter(Preset::Fade)
                    .with_exit(Preset::Fade),
            ),
        );
    }
    root
}

fn names(session: &Session) -> Vec<String> {
    session
        .tree()
        .iter()
        .map(|node| node.name.clone())
        .collect()
}

#[test]
fn a_node_removed_from_the_tree_is_held_then_removed_after_finished() {
    let (mut session, mut renderer) = built(row(&["a", "b", "c"]), &rhai::Map::new());
    settle(&mut session, &mut renderer);

    let turn = session.reconcile(tree(row(&["a", "c"])), &rhai::Map::new(), &phone());
    renderer.apply(&turn.patches);
    assert_eq!(
        renderer.take_trace(),
        "animate b.opacity from=1.0 to=0.0 duration=250.0 delay=0.0 easing=expo_in(6.0)\n"
    );
    // Held: still in the tree (and so in the renderer's child order), counted.
    assert_eq!(names(&session), ["root", "a", "b", "c"]);
    assert_eq!(session.motion_stats().held, 1);

    // A node added meanwhile is positioned among the renderer's children,
    // held one included, so it is in the right place once `b` is gone.
    let turn = session.reconcile(tree(row(&["a", "c", "d"])), &rhai::Map::new(), &phone());
    renderer.apply(&turn.patches);
    assert_eq!(
        renderer.take_trace(),
        "create d Panel parent=root index=3 {}\n\
         animate d.opacity from=0.0 to=1.0 duration=350.0 delay=0.0 easing=expo_out(6.0)\n"
    );
    assert_eq!(session.motion_stats().held, 1);

    tick(&mut session, &mut renderer, 250.0);
    assert_eq!(renderer.take_trace(), "remove b\n");
    assert_eq!(names(&session), ["root", "a", "c", "d"]);
    assert_eq!(session.motion_stats().held, 0);
    // A late duplicate removes nothing.
    assert!(session.finished("b", "opacity").is_empty());
    assert_eq!(session.value("b", "opacity"), None);
}

#[test]
fn a_node_added_back_while_it_exits_cancels_the_exit() {
    let (mut session, mut renderer) = built(row(&["a", "b"]), &rhai::Map::new());
    settle(&mut session, &mut renderer);
    let turn = session.reconcile(tree(row(&["a"])), &rhai::Map::new(), &phone());
    renderer.apply(&turn.patches);
    tick(&mut session, &mut renderer, 100.0);
    renderer.take_trace();

    let turn = session.reconcile(tree(row(&["a", "b"])), &rhai::Map::new(), &phone());
    renderer.apply(&turn.patches);
    // No create, no enter (it never left): it eases back from where it is.
    assert_eq!(
        renderer.take_trace(),
        "animate b.opacity from=current to=1.0 duration=250.0 delay=0.0 easing=expo_out(6.0)\n"
    );
    assert_eq!(session.motion_stats().held, 0);
    let frame = tick(&mut session, &mut renderer, 250.0);
    assert_eq!(frame.finished, [("b".to_owned(), "opacity".to_owned())]);
    // Its end must not remove it.
    assert_eq!(renderer.take_trace(), "");
    assert_eq!(names(&session), ["root", "a", "b"]);
}

#[test]
fn a_node_without_an_exit_is_removed_at_once_and_enters_again_when_added() {
    let root = |with_c: bool| {
        let mut root = Node::new("root", "Column").with_child(Node::new("a", "Label"));
        if with_c {
            root = root.with_child(
                Node::new("c", "Panel").with_motion(Motion::new().with_enter(Preset::Fade)),
            );
        }
        root
    };
    let (mut session, mut renderer) = built(root(true), &rhai::Map::new());
    settle(&mut session, &mut renderer);

    let turn = session.reconcile(tree(root(false)), &rhai::Map::new(), &phone());
    renderer.apply(&turn.patches);
    assert_eq!(renderer.take_trace(), "remove c\n");
    assert_eq!(names(&session), ["root", "a"]);
    assert_eq!(session.motion_stats().held, 0);

    // A node that left and comes back is a new node: it enters.
    let turn = session.reconcile(tree(root(true)), &rhai::Map::new(), &phone());
    renderer.apply(&turn.patches);
    assert_eq!(
        renderer.take_trace(),
        "create c Panel parent=root index=1 {}\n\
         animate c.opacity from=0.0 to=1.0 duration=350.0 delay=0.0 easing=expo_out(6.0)\n"
    );
}

#[test]
fn a_hidden_node_that_is_removed_leaves_when_its_exit_ends() {
    let root = |with_card: bool| {
        let mut root = Node::new("root", "Column").with_child(Node::new("label", "Label"));
        if with_card {
            root = root.with_child(
                Node::new("card", "Panel")
                    .with_bind("visible", "state.show")
                    .with_motion(Motion::new().with_exit(Preset::Fade)),
            );
        }
        root
    };
    let (mut session, mut renderer) = built(root(true), &visible_state(true, "x"));
    settle(&mut session, &mut renderer);
    session.update_with(&visible_state(false, "x"), &phone(), &mut renderer);
    renderer.take_trace();
    // Removed while it fades out: the exit in flight carries on, and ends in
    // a `remove`, not a `set visible = false`.
    let turn = session.reconcile(tree(root(false)), &visible_state(false, "x"), &phone());
    assert!(turn.patches.is_empty());
    assert_eq!(session.motion_stats().held, 1);
    tick(&mut session, &mut renderer, 250.0);
    assert_eq!(renderer.take_trace(), "remove card\n");

    // A node that is already hidden has nothing to animate: gone at once.
    let (mut session, mut renderer) = built(root(true), &visible_state(false, "x"));
    renderer.take_trace();
    let turn = session.reconcile(tree(root(false)), &visible_state(false, "x"), &phone());
    renderer.apply(&turn.patches);
    assert_eq!(renderer.take_trace(), "remove card\n");
}

#[test]
fn a_slide_exit_waits_for_every_property_in_either_order() {
    let root = |with_b: bool| {
        let mut root = Node::new("root", "Column").with_child(Node::new("a", "Label"));
        if with_b {
            root = root.with_child(
                Node::new("b", "Panel")
                    .with_motion(Motion::new().with_exit(Preset::slide(Edge::Bottom, 40.0))),
            );
        }
        root
    };
    for order in [["opacity", "translate_y"], ["translate_y", "opacity"]] {
        let (mut session, mut renderer) = built(root(true), &rhai::Map::new());
        let turn = session.reconcile(tree(root(false)), &rhai::Map::new(), &phone());
        assert_eq!(
            trace(&turn.patches),
            "animate b.opacity from=1.0 to=0.0 duration=250.0 delay=0.0 easing=expo_in(6.0)\n\
             animate b.translate_y from=0.0 to=40.0 duration=250.0 delay=0.0 \
             easing=expo_in(6.0)\n"
        );
        renderer.apply(&turn.patches);
        assert!(session.finished("b", order[0]).is_empty());
        assert_eq!(session.motion_stats().held, 1);
        let last = session.finished("b", order[1]);
        assert_eq!(trace(&last), "remove b\n");
        // Both again: nothing, and nothing is removed twice.
        assert!(session.finished("b", order[0]).is_empty());
        assert!(session.finished("b", order[1]).is_empty());
    }
}

#[test]
fn an_exit_with_nothing_to_move_removes_at_once() {
    // Already transparent: the fade has nowhere to go, so there is no animation
    // to wait for, and a hold would never end.
    let root = |with_ghost: bool| {
        let mut root = Node::new("root", "Column").with_child(Node::new("a", "Label"));
        if with_ghost {
            root = root.with_child(
                Node::new("ghost", "Panel")
                    .with_prop("opacity", 0.0)
                    .with_motion(Motion::new().with_exit(Preset::Fade)),
            );
        }
        root
    };
    let mut session = Session::new(tree(root(true)), Bindings::new());
    session.build(&rhai::Map::new(), &phone());
    let turn = session.reconcile(tree(root(false)), &rhai::Map::new(), &phone());
    assert_eq!(trace(&turn.patches), "remove ghost\n");
    assert_eq!(session.motion_stats().held, 0);
}

#[test]
fn reduced_motion_turns_a_slide_exit_into_a_fade() {
    let root = |with_b: bool| {
        let mut root = Node::new("root", "Column").with_child(Node::new("a", "Label"));
        if with_b {
            root = root.with_child(
                Node::new("b", "Panel")
                    .with_motion(Motion::new().with_exit(Preset::slide(Edge::Left, 24.0))),
            );
        }
        root
    };
    let mut reduced = phone();
    reduced.reduce_motion = true;
    let mut session = Session::new(tree(root(true)), Bindings::new());
    session.build(&rhai::Map::new(), &reduced);
    let turn = session.reconcile(tree(root(false)), &rhai::Map::new(), &reduced);
    assert_eq!(
        trace(&turn.patches),
        "animate b.opacity from=1.0 to=0.0 duration=250.0 delay=0.0 easing=expo_in(6.0)\n"
    );
    assert_eq!(trace(&session.finished("b", "opacity")), "remove b\n");
}

#[test]
fn building_twice_or_replacing_the_tree_does_not_replay_an_enter() {
    let mut session = Session::new(tree(row(&["a", "b"])), Bindings::new());
    let first = session.build(&rhai::Map::new(), &phone());
    let animated = |turn: &lazyrad_view::TurnResult| {
        turn.patches
            .iter()
            .filter(|patch| matches!(patch, Patch::Animate { .. }))
            .map(|patch| patch.node().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(animated(&first), ["a", "b"]);

    // A rebuild creates everything again, silently.
    let second = session.build(&rhai::Map::new(), &phone());
    assert_eq!(second.patches.len(), 3);
    assert!(animated(&second).is_empty());

    // So does a hot reload that replaces the tree, and then builds.
    session.replace_tree(tree(row(&["a", "b"])));
    let third = session.build(&rhai::Map::new(), &phone());
    assert_eq!(third.patches.len(), 3);
    assert!(animated(&third).is_empty());

    // A genuinely new name enters, and only it.
    session.replace_tree(tree(row(&["a", "b", "extra"])));
    let fourth = session.build(&rhai::Map::new(), &phone());
    assert_eq!(fourth.patches.len(), 4 + 1);
    assert_eq!(animated(&fourth), ["extra"]);

    // A name that left the tree and returns is new again.
    session.replace_tree(tree(row(&["a", "b"])));
    session.build(&rhai::Map::new(), &phone());
    session.replace_tree(tree(row(&["a", "b", "extra"])));
    let fifth = session.build(&rhai::Map::new(), &phone());
    assert_eq!(animated(&fifth), ["extra"]);
}

#[test]
fn a_rebuild_forgets_what_was_running_and_holding() {
    let (mut session, mut renderer) = built(row(&["a", "b"]), &rhai::Map::new());
    assert_eq!(session.motion_stats().running, 2);
    let turn = session.reconcile(tree(row(&["a"])), &rhai::Map::new(), &phone());
    renderer.apply(&turn.patches);
    assert_eq!(session.motion_stats().held, 1);
    // The renderer discards its tree on a build; nothing is held for it, and
    // the node that was leaving is not part of what the build describes.
    let rebuilt = session.build(&rhai::Map::new(), &phone());
    assert!(
        rebuilt.patches.iter().all(|patch| patch.node() != "b"),
        "{}",
        trace(&rebuilt.patches)
    );
    assert_eq!(session.motion_stats().running, 0);
    assert_eq!(session.motion_stats().held, 0);
    assert!(session.finished("b", "opacity").is_empty());
    assert_eq!(names(&session), ["root", "a"]);
}

#[test]
fn reconciling_to_the_same_tree_changes_nothing() {
    let (mut session, mut renderer) = built(row(&["a", "b"]), &rhai::Map::new());
    settle(&mut session, &mut renderer);
    let turn = session.reconcile(tree(row(&["a", "b"])), &rhai::Map::new(), &phone());
    assert!(turn.patches.is_empty(), "{}", trace(&turn.patches));
    assert_eq!(names(&session), ["root", "a", "b"]);
    assert_eq!(session.motion_stats().running, 0);
}

#[test]
fn reconcile_keeps_what_is_applied_and_diffs_the_new_definition() {
    let v1 = Node::new("root", "Column")
        .with_child(Node::new("title", "Label").with_prop("text", "Hello"))
        .with_child(Node::new("count", "Label").with_bind("text", "`n=${state.n}`"));
    let v2 = Node::new("root", "Column")
        .with_child(Node::new("title", "Label").with_prop("text", "Hi"))
        .with_child(Node::new("count", "Label").with_bind("text", "`count: ${state.n}`"));
    let state = map(&[("n", Dynamic::from(3_i64))]);
    let mut session = Session::new(tree(v1), Bindings::new());
    let mut renderer = RecordingRenderer::new();
    renderer.apply(&session.build(&state, &phone()).patches);
    renderer.take_trace();

    // No rebuild: only what the new definition changes, the changed binding
    // evaluated this turn.
    let turn = session.reconcile(tree(v2), &state, &phone());
    assert!(turn.diagnostics.is_empty());
    assert_eq!(
        trace(&turn.patches),
        "set title.text = \"Hi\"\nset count.text = \"count: 3\"\n"
    );
}

#[test]
fn reconcile_rebuilds_when_a_kept_node_changes_kind_parent_or_root() {
    let before = Node::new("root", "Column")
        .with_child(Node::new("a", "Label"))
        .with_child(Node::new("row", "Row").with_child(Node::new("b", "Label")));
    let (mut session, _) = built(before, &rhai::Map::new());

    // `b` moves under another parent.
    let moved = Node::new("root", "Column")
        .with_child(Node::new("a", "Label").with_child(Node::new("b", "Label")))
        .with_child(Node::new("row", "Row"));
    let turn = session.reconcile(tree(moved), &rhai::Map::new(), &phone());
    assert_eq!(turn.patches.len(), 4);
    assert!(
        turn.patches
            .iter()
            .all(|p| matches!(p, Patch::Create { .. }))
    );

    // `a` changes kind.
    let kind = Node::new("root", "Column")
        .with_child(Node::new("a", "Edit").with_child(Node::new("b", "Label")))
        .with_child(Node::new("row", "Row"));
    let turn = session.reconcile(tree(kind), &rhai::Map::new(), &phone());
    assert_eq!(turn.patches.len(), 4);

    // Kept siblings change order.
    let swapped = Node::new("root", "Column")
        .with_child(Node::new("row", "Row"))
        .with_child(Node::new("a", "Edit").with_child(Node::new("b", "Label")));
    let turn = session.reconcile(tree(swapped), &rhai::Map::new(), &phone());
    assert_eq!(turn.patches.len(), 4);

    // The root is another node.
    let other = Node::new("page", "Column").with_child(Node::new("a", "Edit"));
    let turn = session.reconcile(tree(other), &rhai::Map::new(), &phone());
    assert!(matches!(
        turn.patches.first(),
        Some(Patch::Create { node, parent: None, .. }) if node == "page"
    ));

    // An unbuilt session builds.
    let mut unbuilt = Session::new(tree(Node::new("r", "Column")), Bindings::new());
    let turn = unbuilt.reconcile(tree(Node::new("r", "Column")), &rhai::Map::new(), &phone());
    assert_eq!(turn.patches.len(), 1);
}
