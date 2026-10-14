//! A model-based test of the motion lifecycle: random trees, states, clock
//! steps, presses and (sometimes bogus) `finished` calls, with a minimal
//! renderer that only applies `Create` and `Remove`. After every step the tree
//! the renderer holds must be the tree the session holds, and once everything
//! has played out it must be the last tree the session was given.
//!
//! This is what catches an exit that leaks its node, a `Create` positioned
//! against the wrong children, and a held node that is never removed.

mod common;
mod model;

use std::collections::HashMap;

use common::tree;
use lazyrad_view::limits::MAX_RUNNING_ANIMATIONS;
use lazyrad_view::motion::{ANIMATABLE, identity};
use lazyrad_view::{
    Bindings, Environment, Motion, Patch, Renderer, SamplingRenderer, Session, Value,
};
use model::{Dom, LEAVES, Rng, random_motion, random_state, random_tree, session_shape, shape_of};

/// What a run exercised, so the test can fail if it stops reaching the
/// interesting paths.
#[derive(Default)]
struct Coverage {
    /// `reconcile` calls that applied a difference instead of rebuilding.
    incremental: u32,
    /// Nodes removed after their exit finished.
    removed_after_exit: u32,
    /// Nodes hidden after their exit finished.
    hidden_after_exit: u32,
    /// Exits cancelled because the node came back.
    cancelled: u32,
}

fn run(seed: u64, coverage: &mut Coverage) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let motions: HashMap<&'static str, Motion> = LEAVES
        .iter()
        .map(|name| (*name, random_motion(&mut rng)))
        .collect();
    let mut wanted = random_tree(&mut rng, &motions);
    let mut session = Session::new(tree(wanted.clone()), Bindings::new());
    let mut renderer = SamplingRenderer::new();
    let mut dom = Dom::default();
    let mut env = Environment::new(360.0, 640.0);
    let mut state = random_state(&mut rng);

    let built = session.build(&state, &env);
    dom.apply(&built.patches);
    renderer.apply(&built.patches);

    let verbose = std::env::var("MOTION_SEED").is_ok_and(|value| value == seed.to_string());
    for step in 0..60 {
        // What the step did to the session, to be applied to both renderers.
        let mut handed: Vec<Patch> = Vec::new();
        let mut applied_by_session = false;
        match rng.below(100) {
            0..=24 => {
                wanted = random_tree(&mut rng, &motions);
                let turn = session.reconcile(tree(wanted.clone()), &state, &env);
                let rebuilt = matches!(
                    turn.patches.first(),
                    Some(Patch::Create { parent: None, .. })
                );
                if !rebuilt {
                    coverage.incremental += 1;
                }
                coverage.cancelled += turn
                    .patches
                    .iter()
                    .filter(|patch| {
                        matches!(
                            patch,
                            Patch::Animate {
                                from: lazyrad_view::AnimFrom::Current,
                                ..
                            }
                        )
                    })
                    .count() as u32;
                handed = turn.patches;
            }
            25..=44 => {
                state = random_state(&mut rng);
                env.reduce_motion = rng.chance(25);
                let turn = session.update_with(&state, &env, &mut renderer);
                if verbose {
                    eprintln!(
                        "step {step} update (reduce {}, dim {:?}):
{}",
                        env.reduce_motion,
                        state["dim"],
                        lazyrad_view::trace(&turn.patches)
                    );
                }
                dom.apply(&turn.patches);
                applied_by_session = true;
            }
            45..=74 => {
                let dt = [0.0, 40.0, 130.0, 400.0][rng.below(4) as usize];
                let frame = renderer.advance(dt);
                for (node, property) in &frame.finished {
                    let patches = session.finished(node, property);
                    if verbose {
                        eprintln!(
                            "step {step} finished {node}.{property}:
{}",
                            lazyrad_view::trace(&patches)
                        );
                    }
                    for patch in &patches {
                        match patch {
                            Patch::Remove { .. } => coverage.removed_after_exit += 1,
                            Patch::Set {
                                value: lazyrad_view::Value::Bool(false),
                                ..
                            } => coverage.hidden_after_exit += 1,
                            _ => {}
                        }
                    }
                    dom.apply(&patches);
                    renderer.apply(&patches);
                }
            }
            75..=84 => {
                // A report that names a node or property nobody animates (a
                // renderer reports only what ran to its end, so one that is
                // still running is never named).
                let node = ["a", "b", "g0", "ghost", "root"][rng.below(5) as usize];
                let property = ["opacity", "translate_y", "scale_x", "x"][rng.below(4) as usize];
                if !renderer.is_running(node, property) {
                    handed = session.finished(node, property);
                }
            }
            85..=94 => {
                let node = LEAVES[rng.below(8) as usize];
                handed = if rng.chance(60) {
                    session.press_begin(node)
                } else {
                    session.press_end(node)
                };
            }
            _ => {
                handed = session.build(&state, &env).patches;
            }
        }
        if verbose && !applied_by_session {
            eprintln!(
                "step {step} handed:
{}",
                lazyrad_view::trace(&handed)
            );
        }
        if !applied_by_session {
            dom.apply(&handed);
            renderer.apply(&handed);
        }
        assert_eq!(
            dom.shape(),
            session_shape(&session),
            "seed {seed}, step {step}: the renderer's tree is not the session's"
        );
        let stats = session.motion_stats();
        assert!(
            stats.running <= MAX_RUNNING_ANIMATIONS,
            "seed {seed}, step {step}: {stats:?}"
        );
    }

    // Release whatever is still pressed, then let everything play out,
    // reporting each end. Nothing may stay held.
    for name in LEAVES {
        let patches = session.press_end(name);
        dom.apply(&patches);
        renderer.apply(&patches);
    }
    for _ in 0..20 {
        let frame = renderer.advance(1000.0);
        for (node, property) in &frame.finished {
            let patches = session.finished(node, property);
            dom.apply(&patches);
            renderer.apply(&patches);
        }
        if renderer.running() == 0 {
            break;
        }
    }
    assert_eq!(renderer.running(), 0, "seed {seed}");
    assert_eq!(session.motion_stats().held, 0, "seed {seed}");
    assert_eq!(session.motion_stats().running, 0, "seed {seed}");
    // After a settle, the renderer holds what the session holds, which is the
    // last tree the session was given.
    assert_eq!(dom.shape(), session_shape(&session), "seed {seed}");
    assert_eq!(
        session_shape(&session),
        shape_of(&wanted),
        "seed {seed}: the session's tree is not the one it was given"
    );
    // And every widget rests where the session says it does: no exit left a
    // node transparent or offset, no press left it small, and a hidden node is
    // hidden.
    for node in session.tree().iter() {
        let visible = |value: Option<Value>| match value {
            Some(Value::Bool(flag)) => flag,
            _ => true,
        };
        assert_eq!(
            visible(renderer.live_value(&node.name, "visible")),
            visible(session.value(&node.name, "visible").cloned()),
            "seed {seed}: {} is not as visible as the session says",
            node.name
        );
        for property in ANIMATABLE {
            let rest = identity(property).unwrap_or(0.0);
            let expected = match session.value(&node.name, property) {
                Some(Value::Float(number)) => *number,
                Some(Value::Int(number)) => *number as f64,
                _ => rest,
            };
            let actual = renderer.value(&node.name, property).unwrap_or(rest);
            assert!(
                (expected - actual).abs() < 1.0e-9,
                "seed {seed}: {}.{property} rests at {actual}, the session says {expected}",
                node.name
            );
        }
    }
}

#[test]
fn the_renderers_tree_is_the_sessions_tree_through_random_lifecycles() {
    // `MOTION_SEEDS=5000 cargo test ...` runs a longer soak.
    let seeds: u64 = std::env::var("MOTION_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(300);
    let mut coverage = Coverage::default();
    for seed in 1..=seeds {
        run(seed, &mut coverage);
    }
    // The run must keep reaching the paths it exists for.
    assert!(coverage.incremental >= 1000, "{}", coverage.incremental);
    assert!(
        coverage.removed_after_exit >= 100,
        "{}",
        coverage.removed_after_exit
    );
    assert!(
        coverage.hidden_after_exit >= 100,
        "{}",
        coverage.hidden_after_exit
    );
    assert!(coverage.cancelled >= 20, "{}", coverage.cancelled);
}
