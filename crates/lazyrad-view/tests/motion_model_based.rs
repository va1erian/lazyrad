//! A model-based test of the motion lifecycle: random trees, states, clock
//! steps, presses, stale and bogus `finished` reports, rebuilds and hot
//! reloads, against a minimal renderer that only applies `Create` and
//! `Remove` (plus a sampling renderer for the values).
//!
//! Each seed runs against one kind of renderer: one that reports every
//! animation that ends, one that reports only some, and one that reports none,
//! each with and without the host's `Session::advance` tick. After every step
//!
//! * the tree the renderer holds is the tree the session holds,
//! * at most the documented number of animations is running,
//! * a report that does not name the current animation changes nothing, and
//! * a node the session shows and is not leaving, with nothing running on a
//!   property, has that property at rest in the renderer.
//!
//! Once everything has played out (by reports, by `advance`, or, for the
//! renderer that never gets either, by `reset_motion`) nothing is held or
//! running, and the renderer holds the last tree the session was given, every
//! widget at its rest.

mod common;
mod model;

use std::collections::HashMap;

use common::tree;
use lazyrad_view::{Bindings, Environment, Motion, SamplingRenderer, Session};
use model::world::{Config, Coverage, World};
use model::{Dom, LEAVES, Rng, random_motion, random_state, random_tree};

fn run(seed: u64, coverage: &mut Coverage) {
    // Five kinds of renderer and host, round and round.
    let (report_percent, tick) =
        [(100, false), (100, true), (0, true), (0, false), (50, true)][(seed % 5) as usize];
    let config = Config {
        report_percent,
        tick,
        wide: seed % 7 == 0,
    };
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let motions: HashMap<&'static str, Motion> = LEAVES
        .iter()
        .map(|name| (*name, random_motion(&mut rng)))
        .collect();
    let wanted = random_tree(&mut rng, &motions, config.wide);
    let state = random_state(&mut rng);
    let mut world = World {
        seed,
        config,
        session: Session::new(tree(wanted.clone()), Bindings::new()),
        renderer: SamplingRenderer::new(),
        dom: Dom::default(),
        env: Environment::new(360.0, 640.0),
        state,
        wanted,
        rng,
        motions,
        history: Vec::new(),
        coverage,
    };
    let built = world.session.build(&world.state, &world.env);
    world.apply(&built.patches);
    for step in 0..60 {
        world.step();
        world.check(step);
    }
    world.settle();
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
    // The run must keep reaching the paths it exists for (these are for the
    // default 300 seeds; a longer soak only reaches more).
    assert!(coverage.incremental >= 800, "{}", coverage.incremental);
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
    assert!(
        coverage.ignored_reports >= 500,
        "{}",
        coverage.ignored_reports
    );
    assert!(
        coverage.completed_by_tick >= 50,
        "{}",
        coverage.completed_by_tick
    );
    assert!(coverage.capped >= 10, "{}", coverage.capped);
    assert!(coverage.reloads >= 100, "{}", coverage.reloads);
}
