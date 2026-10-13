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
//! value, the literal or responsive value) and yields one [`BindDiagnostic`]
//! per turn. It is evaluated again every turn until it succeeds. Nothing
//! panics and no other binding is affected.
//!
//! # What is re-evaluated
//!
//! A binding that reads `env` runs again when the environment changed (any
//! field); one that reads `state` or `props` runs again when `state` changed;
//! one that reads none of them runs only at build. Responsive patches are
//! re-resolved on every turn, and produce patches only when the size class
//! changes the cascade.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::rc::Rc;

use rhai::Map;

use crate::bindings::{BindingScope, Bindings, Dependencies};
use crate::environment::Environment;
use crate::patch::Patch;
use crate::renderer::Renderer;
use crate::size_class::{Responsive, SizeClass};
use crate::state_eq::same_map;
use crate::tree::{Node, Tree};
use crate::value::{Value, ValueKind};

/// A binding that failed on a turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindDiagnostic {
    /// The node the binding belongs to.
    pub node: String,
    /// The bound property.
    pub property: String,
    /// What went wrong, with the position in the expression when known.
    pub message: String,
}

impl fmt::Display for BindDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}: {}", self.node, self.property, self.message)
    }
}

/// What one turn produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnResult {
    /// The changes to apply, in order.
    pub patches: Vec<Patch>,
    /// One entry per binding that failed this turn, in tree order.
    pub diagnostics: Vec<BindDiagnostic>,
}

/// What the session remembers about one property of one node.
struct PropState {
    /// The fixed value kind, once known.
    kind: Option<ValueKind>,
    /// What the binding reads; `None` for an unbound property.
    deps: Option<Dependencies>,
    /// The value the renderer was last given.
    applied: Option<Value>,
    /// The last value the binding produced successfully.
    good: Option<Value>,
    /// Whether the binding failed on the last turn that ran it.
    failed: bool,
}

impl PropState {
    fn new(node: &Node, property: &str) -> Self {
        Self {
            kind: node.literal_kind(property),
            deps: node
                .bind
                .get(property)
                .map(|source| Dependencies::scan(source)),
            applied: None,
            good: None,
            failed: false,
        }
    }
}

/// The inputs of the last turn, which the next one is compared with.
struct Last {
    env: Environment,
    state: Map,
}

/// A view tree bound to state: it builds the tree once and then turns state
/// and environment changes into the smallest set of [`Patch`]es.
///
/// A session is single-threaded (it holds an [`Rc`]), like the engine under it.
pub struct Session {
    tree: Tree,
    bindings: Rc<Bindings>,
    /// Per node, in pre-order, indexed like [`Tree::placements`].
    slots: Vec<BTreeMap<String, PropState>>,
    /// Node name to its slot.
    index: HashMap<String, usize>,
    /// `None` until the first turn: the session is not built yet.
    last: Option<Last>,
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
        };
        session.reset();
        session
    }

    /// The tree.
    #[must_use]
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// The value `property` of `node` was last given, if it has one.
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

    /// Replaces the tree (a hot reload). Everything applied is forgotten, so
    /// the session is unbuilt: call [`build`](Self::build) next.
    pub fn replace_tree(&mut self, tree: Tree) {
        self.tree = tree;
        self.reset();
    }

    /// Builds the whole tree: one [`Patch::Create`] per node, parents before
    /// children and siblings in order, carrying the resolved properties.
    ///
    /// Calling it again is a rebuild: every applied value, last good value
    /// and failure is forgotten and the result again describes the whole tree.
    pub fn build(&mut self, state: &Map, env: &Environment) -> TurnResult {
        self.reset();
        self.turn(state, env)
    }

    /// Re-evaluates what can have changed and returns a [`Patch::Set`] (or
    /// [`Patch::Unset`]) for each property whose value differs from the last
    /// *applied* one.
    ///
    /// On a session that is not built yet this is a [`build`](Self::build).
    pub fn update(&mut self, state: &Map, env: &Environment) -> TurnResult {
        self.turn(state, env)
    }

    /// [`update`](Self::update), then hands the patches to `renderer`.
    ///
    /// A `Set` whose value equals the renderer's
    /// [`live_value`](Renderer::live_value) is dropped first: the widget
    /// already shows it, and writing it again would reset the caret of a text
    /// field being typed in. The session counts such a value as applied. The
    /// returned [`TurnResult`] holds the patches that were handed over, and
    /// `apply` is not called when none are left.
    ///
    /// The session finishes its own bookkeeping before the renderer is called,
    /// and holds no borrow of its own across the call.
    pub fn update_with(
        &mut self,
        state: &Map,
        env: &Environment,
        renderer: &mut dyn Renderer,
    ) -> TurnResult {
        let mut result = self.turn(state, env);
        result.patches.retain(|patch| match patch {
            Patch::Set {
                node,
                property,
                value,
            } => renderer.live_value(node, property).as_ref() != Some(value),
            _ => true,
        });
        if !result.patches.is_empty() {
            renderer.apply(&result.patches);
        }
        result
    }

    /// Forgets everything applied and re-derives the per-node tables.
    fn reset(&mut self) {
        let placements = self.tree.placements();
        self.index = placements
            .iter()
            .enumerate()
            .map(|(slot, placement)| (placement.node.name.clone(), slot))
            .collect();
        self.slots = placements
            .iter()
            .map(|placement| {
                placement
                    .node
                    .property_names()
                    .into_iter()
                    .map(|property| {
                        (
                            property.to_owned(),
                            PropState::new(placement.node, property),
                        )
                    })
                    .collect()
            })
            .collect();
        self.last = None;
    }

    fn turn(&mut self, state: &Map, env: &Environment) -> TurnResult {
        let full = self.last.is_none();
        let env_changed = self.last.as_ref().is_none_or(|last| last.env != *env);
        let state_changed = self
            .last
            .as_ref()
            .is_none_or(|last| !same_map(&last.state, state));
        let class = env.size_class();
        let placements = self.tree.placements();

        // Phase 1: evaluate. Reads the session, changes nothing.
        let mut scope: Option<BindingScope> = None;
        let mut outcomes = Vec::new();
        for (slot, placement) in placements.iter().enumerate() {
            for (property, source) in &placement.node.bind {
                let Some(prop) = self.slots[slot].get(property) else {
                    continue;
                };
                let deps = prop.deps.unwrap_or_default();
                let due = full
                    || prop.failed
                    || (deps.reads_state_or_props() && state_changed)
                    || (deps.env && env_changed);
                if !due {
                    continue;
                }
                let scope = scope
                    .get_or_insert_with(|| BindingScope::new(state, &Map::new(), &env.to_map()));
                let result = self.bindings.evaluate(source, scope, prop.kind);
                outcomes.push((slot, property, result));
            }
        }

        // Phase 2: commit the outcomes.
        let mut diagnostics = Vec::new();
        for (slot, property, result) in outcomes {
            let Some(prop) = self.slots[slot].get_mut(property) else {
                continue;
            };
            match result {
                Ok(value) => {
                    prop.kind.get_or_insert(value.kind());
                    prop.good = Some(value);
                    prop.failed = false;
                }
                Err(error) => {
                    prop.failed = true;
                    diagnostics.push(BindDiagnostic {
                        node: placements[slot].node.name.clone(),
                        property: property.clone(),
                        message: error.to_string(),
                    });
                }
            }
        }

        // Phase 3: diff against what was applied.
        let mut patches = Vec::new();
        for (slot, placement) in placements.iter().enumerate() {
            let node = placement.node;
            if full {
                let mut props = BTreeMap::new();
                for (property, prop) in &mut self.slots[slot] {
                    prop.applied = effective(node, property, prop, class);
                    if let Some(value) = &prop.applied {
                        props.insert(property.clone(), value.clone());
                    }
                }
                patches.push(Patch::Create {
                    node: node.name.clone(),
                    parent: placement.parent.map(str::to_owned),
                    index: placement.index,
                    kind: node.kind.clone(),
                    props,
                });
                continue;
            }
            for (property, prop) in &mut self.slots[slot] {
                let wanted = effective(node, property, prop, class);
                if wanted == prop.applied {
                    continue;
                }
                patches.push(match &wanted {
                    Some(value) => Patch::Set {
                        node: node.name.clone(),
                        property: property.clone(),
                        value: value.clone(),
                    },
                    None => Patch::Unset {
                        node: node.name.clone(),
                        property: property.clone(),
                    },
                });
                prop.applied = wanted;
            }
        }

        self.last = Some(Last {
            env: *env,
            state: state.clone(),
        });
        TurnResult {
            patches,
            diagnostics,
        }
    }
}

/// The value a property should show: its binding's last good value, else the
/// responsive cascade for `class`, else the literal.
fn effective(node: &Node, property: &str, prop: &PropState, class: SizeClass) -> Option<Value> {
    if let Some(good) = &prop.good {
        return Some(good.clone());
    }
    Responsive::new(&node.responsive)
        .property(class, property)
        .or_else(|| node.props.get(property))
        .cloned()
}
