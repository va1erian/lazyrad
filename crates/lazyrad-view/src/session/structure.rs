//! Reconcile: swap the tree under a built session without a rebuild, so nodes
//! can leave (through their `exit`) and arrive (through their `enter`) while
//! everything else keeps what it has applied.

use std::collections::{HashMap, HashSet};

use rhai::Map;

use super::lifecycle::{begin_exit, cancel_exit};
use super::slots::{Props, carry, shows};
use super::{Environment, Session, TurnResult};
use crate::motion::Preset;
use crate::node::Node;
use crate::patch::Patch;
use crate::planner::{Exit, ExitKind, Live, keep_last_animates};
use crate::tree::Tree;

/// A node that is gone from the new tree and whose parent is not: where it
/// was, so an exit can keep it in place.
struct Departure {
    name: String,
    parent: String,
    /// The nearest earlier sibling that is still in the tree.
    after: Option<String>,
}

/// How the new tree differs from the one the renderer shows.
struct Plan {
    /// The top-most nodes that are gone, in the old pre-order.
    departures: Vec<Departure>,
    /// Every name that is gone, including the ones under a departure.
    gone: HashSet<String>,
    /// The names the new tree adds.
    added: HashSet<String>,
}

/// The children of each parent that `other` also has, in order.
fn kept_children<'a>(tree: &'a Tree, other: &Tree) -> HashMap<Option<&'a str>, Vec<&'a str>> {
    let mut kept: HashMap<Option<&str>, Vec<&str>> = HashMap::new();
    for placement in tree.placements() {
        if other.contains(&placement.node.name) {
            kept.entry(placement.parent)
                .or_default()
                .push(placement.node.name.as_str());
        }
    }
    kept
}

impl Plan {
    /// Compares the trees. `None` when a node that both have changed kind or
    /// parent, or kept siblings changed their relative order: a widget cannot
    /// be morphed or moved by this, so the caller rebuilds.
    fn between(old: &Tree, new: &Tree) -> Option<Self> {
        let (old_root, new_root) = (old.root(), new.root());
        if old_root.name != new_root.name || old_root.kind != new_root.kind {
            return None;
        }
        let new_places = new.placements();
        let new_at: HashMap<&str, (Option<&str>, &str)> = new_places
            .iter()
            .map(|p| (p.node.name.as_str(), (p.parent, p.node.kind.as_str())))
            .collect();
        let mut gone = HashSet::new();
        let mut departures: Vec<Departure> = Vec::new();
        let mut last_kept: HashMap<&str, &str> = HashMap::new();
        let old_places = old.placements();
        for placement in &old_places {
            let name = placement.node.name.as_str();
            match new_at.get(name) {
                Some((parent, kind)) => {
                    if *parent != placement.parent || *kind != placement.node.kind {
                        return None;
                    }
                    if let Some(parent) = placement.parent {
                        last_kept.insert(parent, name);
                    }
                }
                None => {
                    gone.insert(name.to_owned());
                    // The root is never gone, so a parent exists.
                    if let Some(parent) = placement.parent {
                        if !gone.contains(parent) {
                            departures.push(Departure {
                                name: name.to_owned(),
                                parent: parent.to_owned(),
                                after: last_kept.get(parent).map(|after| (*after).to_owned()),
                            });
                        }
                    }
                }
            }
        }
        if kept_children(old, new) != kept_children(new, old) {
            return None;
        }
        let added = new_places
            .iter()
            .filter(|p| !old.contains(&p.node.name))
            .map(|p| p.node.name.clone())
            .collect();
        Some(Self {
            departures,
            gone,
            added,
        })
    }
}

/// The names of `name` and everything under it, in pre-order.
fn subtree_names(tree: &Tree, name: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut pending: Vec<&Node> = tree.get(name).into_iter().collect();
    while let Some(node) = pending.pop() {
        names.push(node.name.clone());
        pending.extend(node.children.iter().rev());
    }
    names
}

/// A node held in the tree while it leaves, and where to put it.
type Held = HashMap<String, Vec<(Option<String>, Node)>>;

/// Puts each held node back among its parent's children, after the sibling it
/// followed.
fn attach(node: &mut Node, held: &Held) {
    if let Some(list) = held.get(&node.name) {
        let mut children = Vec::with_capacity(node.children.len() + list.len());
        let put = |after: Option<&str>, children: &mut Vec<Node>| {
            for (follows, held_node) in list {
                if follows.as_deref() == after {
                    children.push(held_node.clone());
                }
            }
        };
        put(None, &mut children);
        for child in std::mem::take(&mut node.children) {
            let name = child.name.clone();
            children.push(child);
            put(Some(&name), &mut children);
        }
        node.children = children;
    }
    for child in &mut node.children {
        attach(child, held);
    }
}

impl Session {
    /// Swaps in `tree` **without a rebuild**: what the renderer has stays,
    /// and only the difference is patched. This is the live form of
    /// [`replace_tree`](Self::replace_tree) followed by
    /// [`build`](Self::build), and the way a node leaves the tree.
    ///
    /// * A node the new tree adds is created (a [`Patch::Create`] in this
    ///   turn's patches) and runs its `enter` unless it has entered before.
    /// * A node the new tree lacks is removed. With an `exit` preset it is
    ///   animated out first and **held**: it stays in [`tree`](Self::tree),
    ///   is not diffed, and its `Remove` is what
    ///   [`finished`](Self::finished) returns once the exit ends. Without one,
    ///   or when it is already hidden, the `Remove` is in this turn's
    ///   patches. Only the top-most node that is gone runs an exit; what is
    ///   under it leaves with it.
    /// * A node that returns while it is exiting cancels the exit: what the
    ///   exit moved goes back to rest from wherever it is.
    /// * Every node both trees have is diffed against its new definition (its
    ///   literals, responsive patches, bindings and motion), keeping the
    ///   values it has applied.
    ///
    /// When a node that both trees have changes **kind or parent**, when kept
    /// siblings change their order, when the root changes, or when the session
    /// is not built, there is no cheap difference to apply and this is
    /// `replace_tree` and `build`: the patches describe the whole tree, and
    /// [`TurnResult::rebuilt`](crate::TurnResult::rebuilt) is true. A renderer
    /// that treats a rebuild differently (it discards its tree) reads that
    /// flag, not the patches.
    pub fn reconcile(&mut self, tree: Tree, state: &Map, env: &Environment) -> TurnResult {
        let plan = if self.last.is_some() {
            Plan::between(&self.tree, &tree)
        } else {
            None
        };
        let Some(plan) = plan else {
            self.replace_tree(tree);
            return self.turn(state, env, &Live::none());
        };
        let live = Live::none();
        let reduced = env.reduce_motion;
        let mut patches = Vec::new();

        // Which departures an exit holds: the ones already exiting, and the
        // visible ones that have a preset. The rest go at once.
        let mut held = Held::new();
        let mut holding: Vec<&Departure> = Vec::new();
        let mut at_once: Vec<&Departure> = Vec::new();
        for departure in &plan.departures {
            let name = departure.name.as_str();
            let visible = self.index.get(name).is_some_and(|&slot| {
                shows(
                    self.slots[slot]
                        .get("visible")
                        .and_then(|prop| prop.applied.as_ref()),
                )
            });
            let has_exit = self
                .tree
                .get(name)
                .is_some_and(|node| node.motion.exit.is_some());
            if self.planner.exits.contains_key(name) || (visible && has_exit) {
                holding.push(departure);
                if let Some(node) = self.tree.get(name) {
                    held.entry(departure.parent.clone())
                        .or_default()
                        .push((departure.after.clone(), node.clone()));
                }
            } else {
                at_once.push(departure);
            }
        }

        let mut root = tree.root().clone();
        attach(&mut root, &held);
        let Ok(merged) = Tree::new(root) else {
            // Cannot happen (both trees are validated and the names are
            // disjoint), but never panic on the way: rebuild.
            self.replace_tree(tree);
            return self.turn(state, env, &live);
        };

        for departure in &at_once {
            patches.push(Patch::Remove {
                node: departure.name.clone(),
            });
            for gone in subtree_names(&self.tree, &departure.name) {
                self.planner.forget(&gone);
            }
        }

        // Carry what is applied by name, then install the merged tree.
        let old_names: Vec<String> = self
            .tree
            .placements()
            .iter()
            .map(|placement| placement.node.name.clone())
            .collect();
        let mut taken: HashMap<String, Props> = old_names
            .into_iter()
            .zip(std::mem::take(&mut self.slots))
            .collect();
        let slots: Vec<Props> = merged
            .placements()
            .iter()
            .map(|placement| carry(placement.node, taken.remove(&placement.node.name)))
            .collect();
        self.tree = merged;
        self.slots = slots;
        self.reindex();
        self.reserve_sources();
        self.pending_create = plan.added;

        // A node that is back: its exit is over.
        let returned: Vec<String> = self
            .planner
            .exits
            .iter()
            .filter(|(name, exit)| exit.kind == ExitKind::Remove && !plan.gone.contains(*name))
            .map(|(name, _)| name.clone())
            .collect();
        for name in returned {
            for member in subtree_names(&self.tree, &name) {
                self.planner.frozen.remove(&member);
            }
            if let Some(&slot) = self.index.get(name.as_str()) {
                cancel_exit(
                    &mut self.planner,
                    &name,
                    &self.slots[slot],
                    &live,
                    &mut patches,
                );
            }
        }

        // The departures an exit holds: start theirs, or keep holding.
        for departure in holding {
            let name = departure.name.as_str();
            if let Some(exit) = self.planner.exits.get_mut(name) {
                // Hidden and now removed: it leaves when its exit ends.
                exit.kind = ExitKind::Remove;
            } else {
                let preset: Option<Preset> = self.tree.get(name).and_then(|n| n.motion.exit);
                let animated = match (preset, self.index.get(name)) {
                    (Some(preset), Some(&slot)) => begin_exit(
                        &mut self.planner,
                        name,
                        &preset,
                        reduced,
                        &self.slots[slot],
                        &live,
                        &mut patches,
                    ),
                    _ => None,
                };
                match animated {
                    Some(pending) => {
                        self.planner.exits.insert(
                            name.to_owned(),
                            Exit {
                                kind: ExitKind::Remove,
                                animated: pending.keys().cloned().collect(),
                                pending,
                            },
                        );
                    }
                    None => {
                        // Nothing takes time: it goes now.
                        patches.push(Patch::Remove {
                            node: name.to_owned(),
                        });
                        self.drop_subtree(name);
                        continue;
                    }
                }
            }
            for member in subtree_names(&self.tree, name) {
                self.planner.frozen.insert(member);
            }
        }

        let mut result = self.turn(state, env, &live);
        patches.append(&mut result.patches);
        // One `Animate` per node and property across the whole batch: a cancel
        // and a `change`, say, would otherwise tell the renderer two things.
        keep_last_animates(&mut patches);
        result.patches = patches;
        result
    }
}
