//! One turn: evaluate what can have changed, commit it, and diff and plan.

use std::collections::BTreeMap;

use rhai::Map;

use super::lifecycle::{begin_enter, begin_exit, cancel_exit};
use super::slots::{effective, shows};
use super::{BindingScope, Session};
use crate::environment::Environment;
use crate::motion::identity;
use crate::patch::Patch;
use crate::planner::{Exit, ExitKind, Live, Origin, Outcome, Request, number};
use crate::state_eq::same_map;
use crate::turn::{BindDiagnostic, TurnResult};
use crate::visible::View;

impl Session {
    pub(super) fn turn(&mut self, state: &Map, env: &Environment, live: &Live<'_>) -> TurnResult {
        let full = self.last.is_none();
        let env_changed = self.last.as_ref().is_none_or(|last| last.env != *env);
        // What bindings can see of the state, which is also what decides
        // whether a binding has anything new to read: a change inside an
        // entry that is hidden from them is not a change to them. Building it
        // costs at most `VISIBLE_BUDGET` steps per entry, however big the
        // state is.
        let state_view = View::of("state", state);
        let state_changed = self
            .last
            .as_ref()
            .is_none_or(|last| !same_map(&last.state, &state_view.map));
        let class = env.size_class();
        let reduced = env.reduce_motion;
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
                // A binding that has never produced a value is due: a node
                // that was just added by `reconcile` has none yet.
                let due = full
                    || prop.failed
                    || prop.good.is_none()
                    || (deps.reads_state_or_props() && state_changed)
                    || (deps.env && env_changed);
                if !due {
                    continue;
                }
                let scope = scope.get_or_insert_with(|| {
                    BindingScope::from_views(
                        &state_view,
                        &View::default(),
                        &View::of("env", &env.to_map()),
                    )
                });
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

        // Phase 3: diff against what was applied, and plan the motion.
        let mut patches = Vec::new();
        // The nodes whose `enter` runs, once every `Create` is out.
        let mut entering: Vec<usize> = Vec::new();
        for (slot, placement) in placements.iter().enumerate() {
            let node = placement.node;
            let name = node.name.as_str();
            if full || self.pending_create.contains(name) {
                let mut props = BTreeMap::new();
                for (property, prop) in &mut self.slots[slot] {
                    prop.applied = effective(node, property, prop, class);
                    if let Some(value) = &prop.applied {
                        props.insert(property.clone(), value.clone());
                    }
                }
                patches.push(Patch::Create {
                    node: name.to_owned(),
                    parent: placement.parent.map(str::to_owned),
                    index: placement.index,
                    kind: node.kind.clone(),
                    props,
                });
                // A name enters once: a rebuild creates it again silently. A
                // hidden node has not entered; it does when it is first shown.
                let visible = shows(
                    self.slots[slot]
                        .get("visible")
                        .and_then(|prop| prop.applied.as_ref()),
                );
                if visible && self.planner.entered.insert(name.to_owned()) {
                    entering.push(slot);
                }
                continue;
            }

            if self.planner.frozen.contains(name) {
                // Leaving: not diffed. One that is only hidden comes back when
                // its `visible` is wanted again.
                let hide = matches!(
                    self.planner.exits.get(name),
                    Some(Exit {
                        kind: ExitKind::Hide,
                        ..
                    })
                );
                let wanted = self.slots[slot]
                    .get("visible")
                    .map(|prop| shows(effective(node, "visible", prop, class).as_ref()));
                if hide && wanted == Some(true) {
                    cancel_exit(
                        &mut self.planner,
                        name,
                        &self.slots[slot],
                        live,
                        &mut patches,
                    );
                }
                continue;
            }

            // An exit through `visible`: animate out and hold the node.
            if let (Some(preset), Some(visible)) =
                (&node.motion.exit, self.slots[slot].get("visible"))
            {
                let hiding = shows(visible.applied.as_ref())
                    && !shows(effective(node, "visible", visible, class).as_ref());
                if hiding {
                    let animated = begin_exit(
                        &mut self.planner,
                        name,
                        preset,
                        reduced,
                        &self.slots[slot],
                        live,
                        &mut patches,
                    );
                    if let Some(animated) = animated {
                        self.planner.frozen.insert(name.to_owned());
                        self.planner.exits.insert(
                            name.to_owned(),
                            Exit {
                                kind: ExitKind::Hide,
                                pending: animated.clone(),
                                animated,
                            },
                        );
                        continue;
                    }
                }
            }

            let mut shown = false;
            for (property, prop) in &mut self.slots[slot] {
                let wanted = effective(node, property, prop, class);
                if wanted == prop.applied {
                    continue;
                }
                if property == "visible" && !shows(prop.applied.as_ref()) && shows(wanted.as_ref())
                {
                    shown = true;
                }
                match &wanted {
                    // An unset applies at once, whatever the property.
                    None => {
                        self.planner.stop(name, property);
                        patches.push(Patch::Unset {
                            node: name.to_owned(),
                            property: property.clone(),
                        });
                    }
                    Some(value) => {
                        // `change`: an animatable property with a transition
                        // eases from the last applied value, unless the user
                        // asked for reduced motion.
                        let transition = node
                            .motion
                            .change
                            .get(property.as_str())
                            .filter(|_| !reduced);
                        let rest = prop
                            .applied
                            .as_ref()
                            .and_then(number)
                            .or_else(|| identity(property));
                        let outcome = match (transition, number(value), rest) {
                            (Some(transition), Some(to), Some(rest)) => Some(self.planner.animate(
                                &Request {
                                    node: name,
                                    property,
                                    origin: Origin::Belief(rest),
                                    to,
                                    duration_ms: transition.duration_ms,
                                    delay_ms: 0.0,
                                    easing: transition.easing,
                                },
                                live,
                                &mut patches,
                            )),
                            _ => None,
                        };
                        if !matches!(outcome, Some(Outcome::Started | Outcome::Noop)) {
                            // An animation in flight is mid-way: what the widget
                            // shows then proves nothing, and the `Set` is what
                            // stops it.
                            let in_flight = self.planner.is_running(name, property);
                            self.planner.stop(name, property);
                            // The widget already shows it (the user typed it):
                            // writing it again would reset the caret.
                            if in_flight || live.value(name, property).as_ref() != Some(value) {
                                patches.push(Patch::Set {
                                    node: name.to_owned(),
                                    property: property.clone(),
                                    value: value.clone(),
                                });
                            }
                        }
                    }
                }
                prop.applied = wanted;
            }
            if shown {
                entering.push(slot);
            }
        }

        // Every `Create` and `Set` is out: now the nodes that appeared run
        // their `enter`, so a renderer sees the whole tree before anything
        // moves.
        for slot in entering {
            let node = placements[slot].node;
            if let Some(preset) = &node.motion.enter {
                begin_enter(
                    &mut self.planner,
                    &node.name,
                    preset,
                    reduced,
                    &self.slots[slot],
                    live,
                    &mut patches,
                );
            }
            self.planner.entered.insert(node.name.clone());
        }
        self.pending_create.clear();

        // The snapshot is only replaced when the visible state changed, so a
        // resize or a no-op turn costs a comparison, not a copy.
        if state_changed || self.last.is_none() {
            self.last = Some(super::slots::Last {
                env: *env,
                state: state_view.map,
            });
        } else if let Some(last) = self.last.as_mut() {
            last.env = *env;
        }
        TurnResult {
            patches,
            diagnostics,
        }
    }
}
