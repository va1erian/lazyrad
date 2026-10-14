//! The animation cap: more simultaneous animations than the documented number
//! degrade to instant changes, never to a panic or an unbounded set.

mod common;

use common::{map, report, tree};
use lazyrad_view::limits::MAX_RUNNING_ANIMATIONS;
use lazyrad_view::{
    AnimFrom, Bindings, Edge, Environment, Motion, Node, Patch, Preset, Session, Transition,
};
use rhai::Dynamic;

fn phone() -> Environment {
    Environment::new(360.0, 640.0)
}

/// `count` panels, each fading in, with an `opacity` that follows
/// `state.dim` through an eased transition.
fn panels(count: usize) -> Node {
    let mut root = Node::new("root", "Column");
    for index in 0..count {
        root = root.with_child(
            Node::new(format!("item_{index}"), "Panel")
                .with_bind("opacity", "1.0 - state.dim * 0.5")
                .with_motion(
                    Motion::new()
                        .with_enter(Preset::Fade)
                        .with_exit(Preset::Fade)
                        .with_change("opacity", Transition::new(100.0)),
                ),
        );
    }
    root
}

/// The session's tree as a root without the nodes named in `gone`.
fn without(session: &Session, gone: &[&str]) -> Node {
    let mut root = Node::new("root", "Column");
    for node in session.tree().iter().skip(1) {
        if !gone.contains(&node.name.as_str()) {
            root = root.with_child(node.clone());
        }
    }
    root
}

fn dim(value: f64) -> rhai::Map {
    map(&[("dim", Dynamic::from_float(value))])
}

fn animated(patches: &[Patch]) -> Vec<&str> {
    patches
        .iter()
        .filter(|patch| matches!(patch, Patch::Animate { .. }))
        .map(Patch::node)
        .collect()
}

fn sets(patches: &[Patch]) -> Vec<&str> {
    patches
        .iter()
        .filter(|patch| matches!(patch, Patch::Set { .. }))
        .map(Patch::node)
        .collect()
}

#[test]
fn the_cap_is_the_documented_number_and_a_count_of_properties() {
    // Stated once, in `src/limits.rs`; `tests/limits_docs.rs` checks the table.
    assert_eq!(MAX_RUNNING_ANIMATIONS, 256);
}

#[test]
fn entrances_past_the_cap_show_at_once_and_are_counted() {
    let total = MAX_RUNNING_ANIMATIONS + 44;
    let mut session = Session::new(tree(panels(total)), Bindings::new());
    let built = session.build(&dim(0.0), &phone());

    // Every node is created; the first nodes in tree order animate, the rest
    // simply appear.
    assert_eq!(
        built
            .patches
            .iter()
            .filter(|patch| matches!(patch, Patch::Create { .. }))
            .count(),
        total + 1
    );
    let entering = animated(&built.patches);
    assert_eq!(entering.len(), MAX_RUNNING_ANIMATIONS);
    assert_eq!(entering[0], "item_0");
    assert_eq!(entering[MAX_RUNNING_ANIMATIONS - 1], "item_255");
    let stats = session.motion_stats();
    assert_eq!(stats.running, MAX_RUNNING_ANIMATIONS);
    assert_eq!(stats.degraded, 44);

    // Reporting them finished frees the room.
    for index in 0..MAX_RUNNING_ANIMATIONS {
        assert!(report(&mut session, &format!("item_{index}"), "opacity").is_empty());
    }
    assert_eq!(session.motion_stats().running, 0);
}

#[test]
fn changes_past_the_cap_are_sets_and_an_animation_in_flight_is_replaced_not_added() {
    let total = MAX_RUNNING_ANIMATIONS + 44;
    let mut session = Session::new(tree(panels(total)), Bindings::new());
    session.build(&dim(0.0), &phone());
    for index in 0..MAX_RUNNING_ANIMATIONS {
        assert!(report(&mut session, &format!("item_{index}"), "opacity").is_empty());
    }

    // The first change: room for 256 of the 300.
    let turn = session.update(&dim(1.0), &phone());
    assert_eq!(animated(&turn.patches).len(), MAX_RUNNING_ANIMATIONS);
    let at_once = sets(&turn.patches);
    assert_eq!(at_once.len(), 44);
    assert_eq!(at_once[0], "item_256");
    assert_eq!(session.motion_stats().running, MAX_RUNNING_ANIMATIONS);
    assert_eq!(session.motion_stats().degraded, 88);

    // Changing back: the 256 in flight are replaced (from `current`, no new
    // slot is needed), and the 44 still find no room.
    let turn = session.update(&dim(0.0), &phone());
    let replaced: Vec<&Patch> = turn
        .patches
        .iter()
        .filter(|patch| matches!(patch, Patch::Animate { .. }))
        .collect();
    assert_eq!(replaced.len(), MAX_RUNNING_ANIMATIONS);
    assert!(replaced.iter().all(|patch| matches!(
        patch,
        Patch::Animate {
            from: AnimFrom::Current,
            ..
        }
    )));
    assert_eq!(sets(&turn.patches).len(), 44);
    assert_eq!(session.motion_stats().running, MAX_RUNNING_ANIMATIONS);
    assert_eq!(session.motion_stats().degraded, 132);

    // One finishes: a node that had no room now has some.
    assert!(report(&mut session, "item_0", "opacity").is_empty());
    assert_eq!(session.motion_stats().running, MAX_RUNNING_ANIMATIONS - 1);
    let turn = session.update(&dim(1.0), &phone());
    assert_eq!(animated(&turn.patches).len(), MAX_RUNNING_ANIMATIONS);
    assert_eq!(session.motion_stats().running, MAX_RUNNING_ANIMATIONS);
}

#[test]
fn an_exit_that_finds_no_room_removes_the_node_at_once() {
    let total = MAX_RUNNING_ANIMATIONS + 2;
    let mut session = Session::new(tree(panels(total)), Bindings::new());
    session.build(&dim(0.0), &phone());
    // The entrances of the first 256 hold every slot; the last two nodes
    // just appeared.
    assert_eq!(session.motion_stats().running, MAX_RUNNING_ANIMATIONS);
    // Two nodes with nothing running leave: there is no room to fade them
    // out, so they go at once instead of being held.
    let smaller = without(&session, &["item_256", "item_257"]);
    let turn = session.reconcile(tree(smaller), &dim(0.0), &phone());
    let removed: Vec<&str> = turn
        .patches
        .iter()
        .filter(|patch| matches!(patch, Patch::Remove { .. }))
        .map(Patch::node)
        .collect();
    assert_eq!(removed, ["item_256", "item_257"]);
    assert_eq!(session.motion_stats().held, 0);
    assert_eq!(session.tree().len(), MAX_RUNNING_ANIMATIONS + 1);

    // One that is mid-entrance leaves from where it is: its slot is already
    // its own, so the exit is held.
    let smaller = without(&session, &["item_0"]);
    let turn = session.reconcile(tree(smaller), &dim(0.0), &phone());
    assert_eq!(animated(&turn.patches), ["item_0"]);
    assert_eq!(session.motion_stats().held, 1);
    assert_eq!(session.motion_stats().running, MAX_RUNNING_ANIMATIONS);
}

#[test]
fn a_big_tree_of_entrances_is_one_bounded_pass() {
    // Far past the cap, with a translating preset (two properties per node).
    let mut root = Node::new("root", "Column");
    for index in 0..1000 {
        root = root.with_child(
            Node::new(format!("n{index}"), "Panel")
                .with_motion(Motion::new().with_enter(Preset::slide(Edge::Left, 12.0))),
        );
    }
    let mut session = Session::new(tree(root), Bindings::new());
    let built = session.build(&rhai::Map::new(), &phone());
    assert_eq!(
        animated(&built.patches).len(),
        MAX_RUNNING_ANIMATIONS,
        "one `Animate` per running slot, two per node"
    );
    assert_eq!(session.motion_stats().running, MAX_RUNNING_ANIMATIONS);
    // 1000 nodes, two properties each, minus the 256 that found room.
    assert_eq!(session.motion_stats().degraded, 2000 - 256);
}
