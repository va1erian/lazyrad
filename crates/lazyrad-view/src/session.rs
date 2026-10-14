//! A session: one view tree, the bindings that drive it and what has been
//! applied so far.
//!
//! # The turn
//!
//! ```text
//!   build(state, env)    initial `Create` patches, every node, parents first
//!   update(state, env)   re-evaluate, re-resolve responsive patches, `Set` only
//!                        the values that differ from the last applied ones
//! ```
//!
//! Each turn runs in three phases so it is atomic: bindings are evaluated
//! without touching the session, then the outcomes are committed, then the
//! patches are diffed. No binding can change another node's value.
//!
//! # Resolution
//!
//! A property's value is, lowest precedence first, its literal, the responsive
//! patch cascaded for the current size class, and its binding's last good
//! value. Values are typed: the kind of the literal or patch (or, failing
//! those, of the first good binding result) is fixed, and a result of another
//! kind is a diagnostic, never a coercion.
//!
//! # Failure
//!
//! A binding that fails keeps its last good value (or, before its first good
//! value, the literal or responsive value) and yields one
//! [`BindDiagnostic`](crate::BindDiagnostic) per turn. It is evaluated again every turn until it succeeds. Nothing
//! panics and no other binding is affected.
//!
//! # What is re-evaluated
//!
//! A binding that reads `env` runs again when the environment changed (any
//! field); one that reads `state` or `props` runs again when `state` changed,
//! as far as bindings can see it (a change inside an entry that is hidden from
//! them, see [`bindings`](crate::bindings), is not a change); one that reads
//! none of them runs only at build. Responsive patches are re-resolved on every
//! turn, and produce patches only when the size class changes the cascade.
//!
//! # Motion
//!
//! The same turn plans motion (see [`motion`](crate::motion)): a change to an
//! animatable property of a node with a `change` transition is a
//! [`Patch::Animate`] instead of a `Set`; a node that is created for the first
//! time, or shown, runs its `enter`; one that leaves the tree
//! ([`reconcile`](Session::reconcile)) or is hidden runs its `exit` and is held
//! until the renderer reports its animation [`finished`](Session::finished) by
//! id, or the host's [`advance`](Session::advance) tick finds it overdue. The
//! renderer owns the clock: the session reads none (its own is the one
//! `advance` moves), and a test drives a virtual one (see
//! [`SamplingRenderer`](crate::SamplingRenderer)).

mod lifecycle;
mod pass;
mod press;
mod slots;
mod structure;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use rhai::Map;

use crate::bindings::{BindingScope, Bindings};
use crate::environment::Environment;
#[cfg(doc)]
use crate::patch::Patch;
use crate::planner::{ExitKind, Live, Planner};
use crate::renderer::Renderer;
use crate::size_class::SizeClass;
use crate::tree::Tree;
use crate::turn::TurnResult;
use crate::value::Value;
use slots::{Last, Props, carry};

/// What the motion planner is holding, for tests and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MotionStats {
    /// Animations in flight: started and not yet reported finished or
    /// replaced. Bounded by
    /// [`MAX_RUNNING_ANIMATIONS`](crate::limits::MAX_RUNNING_ANIMATIONS).
    pub running: usize,
    /// Nodes held by an exit that has not finished.
    pub held: usize,
    /// Animations that were not started because the limit was reached; the
    /// change was applied at once. Counts for the life of the session.
    pub degraded: u64,
}

/// A view tree bound to state: it builds the tree once and then turns state
/// and environment changes into the smallest set of [`Patch`]es.
///
/// A session is single-threaded (it holds an [`Rc`]), like the engine under it.
pub struct Session {
    tree: Tree,
    bindings: Rc<Bindings>,
    /// Per node, in pre-order, indexed like [`Tree::placements`].
    slots: Vec<Props>,
    /// Node name to its slot.
    index: HashMap<String, usize>,
    /// `None` until the first turn: the session is not built yet.
    last: Option<Last>,
    /// What is running, entering, leaving or pressed.
    planner: Planner,
    /// Names the next turn must `Create`: nodes [`reconcile`](Self::reconcile)
    /// added.
    pending_create: HashSet<String>,
}

impl Session {
    /// A session over `tree`. Nothing is evaluated until [`build`](Self::build).
    ///
    /// `bindings` may be shared with other sessions through an [`Rc`] so each
    /// source is compiled once.
    #[must_use]
    pub fn new(tree: Tree, bindings: impl Into<Rc<Bindings>>) -> Self {
        let mut session = Self {
            tree,
            bindings: bindings.into(),
            slots: Vec::new(),
            index: HashMap::new(),
            last: None,
            planner: Planner::default(),
            pending_create: HashSet::new(),
        };
        session.reset();
        session
    }

    /// The tree: the one the session was given, or last
    /// [reconciled](Self::reconcile) to, plus the nodes that are still leaving
    /// (an exit holds its node in the tree until it finishes).
    #[must_use]
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// The value `property` of `node` was last given, if it has one. For an
    /// animated property this is the value it is going to.
    #[must_use]
    pub fn value(&self, node: &str, property: &str) -> Option<&Value> {
        let slot = self.slots.get(*self.index.get(node)?)?;
        slot.get(property)?.applied.as_ref()
    }

    /// The size class of the last turn's environment; `None` before the first.
    #[must_use]
    pub fn size_class(&self) -> Option<SizeClass> {
        self.last.as_ref().map(|last| last.env.size_class())
    }

    /// What the motion planner holds right now.
    #[must_use]
    pub fn motion_stats(&self) -> MotionStats {
        MotionStats {
            running: self.planner.running(),
            held: self.planner.exits.len(),
            degraded: self.planner.degraded,
        }
    }

    /// Replaces the tree (a hot reload). Everything applied is forgotten, so
    /// the session is unbuilt: call [`build`](Self::build) next. Use
    /// [`reconcile`](Self::reconcile) to keep what is applied.
    ///
    /// The nodes that have already entered stay known by name, so the rebuild
    /// does not play their `enter` again. A name the new tree does not have is
    /// forgotten: a node that comes back later is a new node and enters.
    pub fn replace_tree(&mut self, tree: Tree) {
        self.tree = tree;
        self.reset();
    }

    /// Builds the whole tree: one [`Patch::Create`] per node, parents before
    /// children and siblings in order, carrying the resolved properties.
    ///
    /// Calling it again is a rebuild: every applied value, last good value
    /// and failure is forgotten and the result again describes the whole tree.
    /// The `enter` of a node that has entered before does not play again (see
    /// [`Motion`](crate::Motion)); animations in flight are forgotten, because
    /// the renderer discards the tree it had.
    ///
    /// The `Animate` patches of the nodes that do enter follow the last
    /// `Create`, so a renderer has the whole tree before anything moves.
    pub fn build(&mut self, state: &Map, env: &Environment) -> TurnResult {
        // A node that is still leaving is not part of what was asked for: the
        // renderer discards its tree, and the rebuild must not bring it back.
        let leaving: Vec<String> = self
            .planner
            .exits
            .iter()
            .filter(|(_, exit)| exit.kind == ExitKind::Remove)
            .map(|(name, _)| name.clone())
            .collect();
        for name in leaving {
            self.tree.remove_subtree(&name);
        }
        self.reset();
        self.turn(state, env, &Live::none())
    }

    /// Re-evaluates what can have changed and returns a [`Patch::Set`] (or
    /// [`Patch::Unset`], or a [`Patch::Animate`] for a property with a `change`
    /// transition) for each property whose value differs from the last
    /// *applied* one.
    ///
    /// On a session that is not built yet this is a [`build`](Self::build).
    pub fn update(&mut self, state: &Map, env: &Environment) -> TurnResult {
        self.turn(state, env, &Live::none())
    }

    /// [`update`](Self::update), then hands the patches to `renderer`.
    ///
    /// A `Set` whose value equals the renderer's
    /// [`live_value`](Renderer::live_value) is dropped: the widget already
    /// shows it, and writing it again would reset the caret of a text field
    /// being typed in. The session counts such a value as applied. So is an
    /// `Animate` whose target the renderer already shows. Neither is dropped
    /// while an animation of that property is in flight: the live value is
    /// then mid-way, and the new patch is what replaces the animation. The
    /// preset animations of an `enter` are never dropped either: they start
    /// from a value the preset chooses, not from what the widget shows. The
    /// returned [`TurnResult`] holds the patches that were handed over, and
    /// `apply` is not called when none are left.
    ///
    /// The diff is against the last *applied* value, never against the widget:
    /// if the user changed a bound widget and the handler refused the change,
    /// no patch is produced and the widget stays ahead of the session until the
    /// bound value next changes (or the view is rebuilt).
    ///
    /// The session finishes its own bookkeeping before the renderer is called,
    /// and holds no borrow of its own across the call.
    pub fn update_with(
        &mut self,
        state: &Map,
        env: &Environment,
        renderer: &mut dyn Renderer,
    ) -> TurnResult {
        let result = {
            let live = Live(Some(&*renderer));
            self.turn(state, env, &live)
        };
        if !result.patches.is_empty() {
            renderer.apply(&result.patches);
        }
        result
    }

    /// Forgets everything applied and re-derives the per-node tables.
    ///
    /// What survives is the set of names that have entered (pruned to the
    /// nodes the tree has), which is the identity an `enter` is gated on.
    fn reset(&mut self) {
        self.reserve_sources();
        self.reindex();
        self.slots = self
            .tree
            .placements()
            .iter()
            .map(|placement| carry(placement.node, None))
            .collect();
        self.last = None;
        self.pending_create.clear();
        self.planner.reset();
        let tree = &self.tree;
        self.planner.entered.retain(|name| tree.contains(name));
    }

    /// Room in the compile cache for every distinct source of the tree, so it
    /// never thrashes on a form with many bindings.
    fn reserve_sources(&self) {
        let sources: HashSet<&str> = self
            .tree
            .iter()
            .flat_map(|node| node.bind.values().map(String::as_str))
            .collect();
        self.bindings.reserve(sources.len());
    }

    /// Node name to slot, from the tree's pre-order.
    fn reindex(&mut self) {
        self.index = self
            .tree
            .placements()
            .iter()
            .enumerate()
            .map(|(slot, placement)| (placement.node.name.clone(), slot))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::Node;
    use crate::patch::Patch;

    #[test]
    fn a_session_over_an_empty_motion_makes_no_motion_patches() {
        let tree = Tree::new(Node::new("root", "Column").with_prop("gap", 4)).unwrap();
        let mut session = Session::new(tree, Bindings::new());
        let turn = session.build(&Map::new(), &Environment::new(360.0, 640.0));
        assert!(
            turn.patches
                .iter()
                .all(|patch| matches!(patch, Patch::Create { .. }))
        );
        assert_eq!(session.motion_stats(), MotionStats::default());
    }
}
