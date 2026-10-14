//! The world of the model-based motion test: a session, a renderer that only
//! knows `Create` and `Remove`, a sampling renderer, and the random steps and
//! invariants that drive them.

use std::collections::HashMap;

use lazyrad_view::limits::MAX_RUNNING_ANIMATIONS;
use lazyrad_view::motion::{ANIMATABLE, identity};
use lazyrad_view::{
    AnimationId, Environment, Motion, Node, Patch, Renderer, SamplingRenderer, Session, Value,
};

use super::{Dom, Rng, random_state, random_tree, session_shape, shape_of};
use crate::common::tree;

/// What a run exercised, so the test can fail if it stops reaching the
/// interesting paths.
#[derive(Default)]
pub struct Coverage {
    /// `reconcile` calls that applied a difference instead of rebuilding.
    pub incremental: u32,
    /// Nodes removed after their exit finished.
    pub removed_after_exit: u32,
    /// Nodes hidden after their exit finished.
    pub hidden_after_exit: u32,
    /// Exits cancelled because the node came back.
    pub cancelled: u32,
    /// Stale, duplicate or invented reports that were ignored.
    pub ignored_reports: u32,
    /// Overdue animations the host's tick completed.
    pub completed_by_tick: u32,
    /// Runs that hit the animation limit.
    pub capped: u32,
    /// Hot reloads (`replace_tree`, then `build`).
    pub reloads: u32,
}

/// What reaches the session from the renderer, and from the host.
#[derive(Clone, Copy)]
pub struct Config {
    /// The share of finished animations the renderer reports.
    pub report_percent: u64,
    /// Whether the host calls `Session::advance` with the time that passed.
    pub tick: bool,
    /// A tree with more animating nodes than the limit.
    pub wide: bool,
}

pub struct World<'a> {
    pub seed: u64,
    pub config: Config,
    pub rng: Rng,
    pub motions: HashMap<&'static str, Motion>,
    pub session: Session,
    pub renderer: SamplingRenderer,
    pub dom: Dom,
    pub env: Environment,
    pub state: rhai::Map,
    pub wanted: Node,
    pub history: Vec<(String, String, AnimationId)>,
    pub coverage: &'a mut Coverage,
}

impl World<'_> {
    /// Hands `patches` to both renderers, and remembers the animations.
    pub fn apply(&mut self, patches: &[Patch]) {
        self.note(patches);
        self.renderer.apply(patches);
    }

    /// What `update_with` already gave the sampling renderer.
    fn note(&mut self, patches: &[Patch]) {
        self.dom.apply(patches);
        for patch in patches {
            if let Patch::Animate {
                node, property, id, ..
            } = patch
            {
                self.history.push((node.clone(), property.clone(), *id));
            }
        }
    }

    fn names(&self) -> Vec<String> {
        self.session
            .tree()
            .iter()
            .skip(1)
            .map(|node| node.name.clone())
            .collect()
    }

    fn count(&mut self, patches: &[Patch]) {
        for patch in patches {
            match patch {
                Patch::Remove { .. } => self.coverage.removed_after_exit += 1,
                Patch::Set {
                    value: Value::Bool(false),
                    property,
                    ..
                } if property == "visible" => self.coverage.hidden_after_exit += 1,
                _ => {}
            }
        }
    }

    pub fn step(&mut self) {
        match self.rng.below(100) {
            0..=19 => {
                self.wanted = random_tree(&mut self.rng, &self.motions, self.config.wide);
                let turn =
                    self.session
                        .reconcile(tree(self.wanted.clone()), &self.state, &self.env);
                if !turn.rebuilt {
                    self.coverage.incremental += 1;
                }
                self.coverage.cancelled += turn
                    .patches
                    .iter()
                    .filter(|patch| matches!(patch, Patch::Animate { from, .. } if *from == lazyrad_view::AnimFrom::Current))
                    .count() as u32;
                self.apply(&turn.patches);
            }
            20..=24 => {
                // A hot reload: the tree is replaced, then built.
                self.wanted = random_tree(&mut self.rng, &self.motions, self.config.wide);
                self.session.replace_tree(tree(self.wanted.clone()));
                let built = self.session.build(&self.state, &self.env);
                assert!(built.rebuilt);
                self.coverage.reloads += 1;
                self.apply(&built.patches);
            }
            25..=39 => {
                self.state = random_state(&mut self.rng);
                self.env.reduce_motion = self.rng.chance(25);
                let turn = self
                    .session
                    .update_with(&self.state, &self.env, &mut self.renderer);
                assert!(!turn.rebuilt);
                self.note(&turn.patches);
            }
            40..=64 => {
                let dt = [0.0, 40.0, 130.0, 400.0][self.rng.below(4) as usize];
                let frame = self.renderer.advance(dt);
                for ended in &frame.finished {
                    if self.rng.below(100) < self.config.report_percent {
                        let patches = self
                            .session
                            .finished(&ended.node, &ended.property, ended.id);
                        self.count(&patches);
                        self.apply(&patches);
                    }
                }
                if self.config.tick {
                    let before = self.session.motion_stats().running;
                    let patches = self.session.advance(dt);
                    let after = self.session.motion_stats().running;
                    self.coverage.completed_by_tick += before.saturating_sub(after) as u32;
                    self.count(&patches);
                    self.apply(&patches);
                }
            }
            65..=79 => self.stale_report(),
            80..=89 => {
                let names = self.names();
                if !names.is_empty() {
                    let node = names[self.rng.below(names.len() as u64) as usize].clone();
                    let patches = if self.rng.chance(60) {
                        self.session.press_begin(&node)
                    } else {
                        self.session.press_end(&node)
                    };
                    self.apply(&patches);
                }
            }
            90..=94 => {
                let built = self.session.build(&self.state, &self.env);
                self.apply(&built.patches);
            }
            95..=96 => {
                let patches = self.session.reset_motion();
                assert_eq!(self.session.motion_stats().running, 0);
                assert_eq!(self.session.motion_stats().held, 0);
                self.apply(&patches);
            }
            _ => {}
        }
    }

    /// A report that does not name the current animation of its node and
    /// property: an old one, a repeated one, or one that never existed.
    fn stale_report(&mut self) {
        let (node, property, id) = if !self.history.is_empty() && self.rng.chance(70) {
            self.history[self.rng.below(self.history.len() as u64) as usize].clone()
        } else {
            let names = self.names();
            let node = names
                .get(self.rng.below(names.len().max(1) as u64) as usize)
                .cloned()
                .unwrap_or_else(|| "ghost".to_owned());
            let property = ["opacity", "translate_y", "scale_x", "x"][self.rng.below(4) as usize];
            (node, property.to_owned(), AnimationId(self.rng.below(1000)))
        };
        // The current animation is a real report, and one the renderer has
        // not reached is a premature one: neither is what this checks.
        if self.session.running_animation(&node, &property) == Some(id) {
            return;
        }
        let before = self.session.motion_stats();
        let shape = session_shape(&self.session);
        let patches = self.session.finished(&node, &property, id);
        assert!(
            patches.is_empty(),
            "seed {}: a stale report of {node}.{property} {id} did something: {patches:?}",
            self.seed
        );
        assert_eq!(self.session.motion_stats(), before, "seed {}", self.seed);
        assert_eq!(session_shape(&self.session), shape, "seed {}", self.seed);
        self.coverage.ignored_reports += 1;
    }

    /// A rest value the session says `node.property` has.
    fn rest(&self, node: &str, property: &str) -> f64 {
        let fallback = identity(property).unwrap_or(0.0);
        match self.session.value(node, property) {
            Some(Value::Float(number)) => *number,
            Some(Value::Int(number)) => *number as f64,
            _ => fallback,
        }
    }

    pub fn check(&self, step: usize) {
        let seed = self.seed;
        assert_eq!(
            self.dom.shape(),
            session_shape(&self.session),
            "seed {seed}, step {step}: the renderer's tree is not the session's"
        );
        let stats = self.session.motion_stats();
        assert!(
            stats.running <= MAX_RUNNING_ANIMATIONS,
            "seed {seed}, step {step}: {stats:?}"
        );
        assert!(self.renderer.running() <= MAX_RUNNING_ANIMATIONS);
        // The scales are left out here: a node can be pressed, and a press
        // holds them until it is released.
        self.check_rest(step, false);
    }

    /// Every node the session shows, is not leaving and has nothing running
    /// on a property is at rest on it in the renderer.
    fn check_rest(&self, step: usize, with_scales: bool) {
        for node in self.session.tree().iter() {
            let name = &node.name;
            let shown = !matches!(
                self.session.value(name, "visible"),
                Some(Value::Bool(false))
            );
            if !shown || self.session.is_leaving(name) {
                continue;
            }
            for property in ANIMATABLE {
                if !with_scales && property.starts_with("scale") {
                    continue;
                }
                if self.session.running_animation(name, property).is_some() {
                    continue;
                }
                let expected = self.rest(name, property);
                let actual = self
                    .renderer
                    .value(name, property)
                    .unwrap_or_else(|| identity(property).unwrap_or(0.0));
                assert!(
                    (expected - actual).abs() < 1.0e-9,
                    "seed {}, step {step}: {name}.{property} rests at {actual} in the renderer, \
                     the session says {expected}",
                    self.seed
                );
            }
        }
    }

    /// Lets everything play out the way this renderer and host allow, then
    /// checks the end state.
    pub fn settle(&mut self) {
        let seed = self.seed;
        for name in self.names() {
            let patches = self.session.press_end(&name);
            self.apply(&patches);
        }
        for _ in 0..12 {
            let frame = self.renderer.advance(1000.0);
            if self.config.report_percent == 100 {
                for ended in &frame.finished {
                    let patches = self
                        .session
                        .finished(&ended.node, &ended.property, ended.id);
                    self.apply(&patches);
                }
            }
            if self.config.tick {
                let patches = self.session.advance(1000.0);
                self.apply(&patches);
            }
        }
        let stats = self.session.motion_stats();
        if !self.config.tick && self.config.report_percent < 100 {
            // Nothing ever completed these: only the host's reset does.
            if stats.running > 0 || stats.held > 0 {
                let patches = self.session.reset_motion();
                self.apply(&patches);
                self.renderer.advance(10_000.0);
            }
        }
        let stats = self.session.motion_stats();
        assert_eq!(stats.held, 0, "seed {seed}: {stats:?}");
        assert_eq!(stats.running, 0, "seed {seed}: {stats:?}");
        assert_eq!(self.renderer.running(), 0, "seed {seed}");
        if stats.degraded > 0 {
            self.coverage.capped += 1;
        }
        assert_eq!(
            self.dom.shape(),
            session_shape(&self.session),
            "seed {seed}"
        );
        assert_eq!(
            session_shape(&self.session),
            shape_of(&self.wanted),
            "seed {seed}: the session's tree is not the one it was given"
        );
        self.check_rest(usize::MAX, true);
        // And a hidden node is hidden.
        for node in self.session.tree().iter() {
            let visible = |value: Option<Value>| !matches!(value, Some(Value::Bool(false)));
            assert_eq!(
                visible(self.renderer.live_value(&node.name, "visible")),
                visible(self.session.value(&node.name, "visible").cloned()),
                "seed {seed}: {} is not as visible as the session says",
                node.name
            );
        }
    }
}
