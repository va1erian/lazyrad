//! The random inputs and the minimal renderer of the model-based motion test.

use std::collections::HashMap;

use lazyrad_view::{Edge, Motion, Node, Patch, Preset, Session, Transition};
use rhai::Dynamic;

/// xorshift, so the suite needs no dependency and every failure replays.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

pub const LEAVES: [&str; 8] = ["a", "b", "c", "d", "e", "f", "g", "h"];
pub const GROUPS: [&str; 2] = ["g0", "g1"];

pub fn random_motion(rng: &mut Rng) -> Motion {
    let preset = |rng: &mut Rng| match rng.below(3) {
        0 => None,
        1 => Some(Preset::Fade),
        _ => Some(Preset::slide(Edge::Bottom, 24.0)),
    };
    let mut motion = Motion::new();
    motion.enter = preset(rng);
    motion.exit = preset(rng);
    if rng.chance(40) {
        motion = motion.with_change("opacity", Transition::new(120.0));
    }
    if rng.chance(30) {
        motion = motion.with_press(Preset::PressScale);
    }
    motion
}

pub fn leaf(name: &str, motions: &HashMap<&'static str, Motion>) -> Node {
    Node::new(name, "Panel")
        .with_bind("visible", format!("state.v_{name}"))
        .with_bind("opacity", "1.0 - state.dim * 0.5")
        .with_motion(motions[name].clone())
}

/// A random tree over a fixed alphabet. Three times out of four every node
/// keeps its parent and its place among its siblings and only its presence
/// changes (so `reconcile` can apply a difference); otherwise leaves move
/// between parents and reorder (so it has to rebuild).
pub fn random_tree(rng: &mut Rng, motions: &HashMap<&'static str, Motion>) -> Node {
    if rng.chance(75) {
        // root: a b g0[c d] g g1[e f] h
        let mut root = Node::new("root", "Column");
        for name in ["a", "b"] {
            if rng.chance(65) {
                root = root.with_child(leaf(name, motions));
            }
        }
        if rng.chance(75) {
            let mut group = Node::new("g0", "Row");
            for name in ["c", "d"] {
                if rng.chance(65) {
                    group = group.with_child(leaf(name, motions));
                }
            }
            root = root.with_child(group);
        }
        if rng.chance(65) {
            root = root.with_child(leaf("g", motions));
        }
        if rng.chance(75) {
            let mut group = Node::new("g1", "Row");
            for name in ["e", "f"] {
                if rng.chance(65) {
                    group = group.with_child(leaf(name, motions));
                }
            }
            root = root.with_child(group);
        }
        if rng.chance(65) {
            root = root.with_child(leaf("h", motions));
        }
        return root;
    }
    let mut groups: Vec<Node> = GROUPS.iter().map(|name| Node::new(*name, "Row")).collect();
    let mut root = Node::new("root", "Column");
    let mut leaves: Vec<&str> = LEAVES.to_vec();
    for i in (1..leaves.len()).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        leaves.swap(i, j);
    }
    for name in leaves {
        if !rng.chance(65) {
            continue;
        }
        let node = leaf(name, motions);
        match rng.below(3) {
            0 => groups[0] = std::mem::take(&mut groups[0]).with_child(node),
            1 => groups[1] = std::mem::take(&mut groups[1]).with_child(node),
            _ => root = root.with_child(node),
        }
    }
    for group in groups {
        if rng.chance(70) {
            root = root.with_child(group);
        }
    }
    root
}

pub fn random_state(rng: &mut Rng) -> rhai::Map {
    let mut state = rhai::Map::new();
    for name in LEAVES {
        state.insert(format!("v_{name}").into(), Dynamic::from(rng.chance(80)));
    }
    state.insert(
        "dim".into(),
        Dynamic::from_float(if rng.chance(50) { 0.0 } else { 1.0 }),
    );
    state
}

/// What a renderer that only understands `Create` and `Remove` holds.
#[derive(Default)]
pub struct Dom {
    root: Option<String>,
    children: HashMap<String, Vec<String>>,
    parent: HashMap<String, String>,
}

impl Dom {
    pub fn apply(&mut self, patches: &[Patch]) {
        for patch in patches {
            match patch {
                Patch::Create {
                    node, parent: None, ..
                } => {
                    *self = Self::default();
                    self.root = Some(node.clone());
                    self.children.insert(node.clone(), Vec::new());
                }
                Patch::Create {
                    node,
                    parent: Some(parent),
                    index,
                    ..
                } => {
                    let siblings = self
                        .children
                        .get_mut(parent)
                        .unwrap_or_else(|| panic!("create {node} under missing {parent}"));
                    assert!(
                        *index <= siblings.len(),
                        "create {node} at {index} among {siblings:?}"
                    );
                    siblings.insert(*index, node.clone());
                    self.parent.insert(node.clone(), parent.clone());
                    self.children.insert(node.clone(), Vec::new());
                }
                Patch::Remove { node } => {
                    let parent = self
                        .parent
                        .remove(node)
                        .unwrap_or_else(|| panic!("remove of unknown {node}"));
                    self.children
                        .get_mut(&parent)
                        .expect("a parent")
                        .retain(|name| name != node);
                    let mut pending = vec![node.clone()];
                    while let Some(name) = pending.pop() {
                        for child in self.children.remove(&name).unwrap_or_default() {
                            self.parent.remove(&child);
                            pending.push(child);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Names in pre-order with their parents.
    pub fn shape(&self) -> Vec<(String, Option<String>)> {
        let mut out = Vec::new();
        let mut stack: Vec<String> = self.root.iter().cloned().collect();
        while let Some(name) = stack.pop() {
            out.push((name.clone(), self.parent.get(&name).cloned()));
            stack.extend(self.children[&name].iter().rev().cloned());
        }
        out
    }
}

pub fn shape_of(node: &Node) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut stack: Vec<(&Node, Option<&str>)> = vec![(node, None)];
    while let Some((node, parent)) = stack.pop() {
        out.push((node.name.clone(), parent.map(str::to_owned)));
        for child in node.children.iter().rev() {
            stack.push((child, Some(node.name.as_str())));
        }
    }
    out
}

pub fn session_shape(session: &Session) -> Vec<(String, Option<String>)> {
    shape_of(session.tree().root())
}
