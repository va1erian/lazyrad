//! Session behaviour: build, update, failure policy, invalidation.

mod common;

use std::rc::Rc;

use common::{int, map, phone, text, tree};
use lazyrad_view::{BindDiagnostic, Bindings, Environment, Node, Patch, Session, SizeClass, Value};
use rhai::{Dynamic, Map};

fn session(root: Node) -> Session {
    Session::new(tree(root), Bindings::new())
}

fn set(node: &str, property: &str, value: impl Into<Value>) -> Patch {
    Patch::Set {
        node: node.into(),
        property: property.into(),
        value: value.into(),
    }
}

fn greeting_tree() -> Node {
    Node::new("root", "Column")
        .with_child(Node::new("greeting", "Label").with_bind("text", "`Hello, ${state.name}!`"))
        .with_child(Node::new("count", "Label").with_bind("text", "`n=${state.n}`"))
}

#[test]
fn build_creates_parents_before_children_and_siblings_in_order() {
    let root = Node::new("root", "Column")
        .with_prop("gap", 8)
        .with_child(
            Node::new("row", "Row")
                .with_child(Node::new("a", "Label").with_prop("text", "A"))
                .with_child(Node::new("b", "Label").with_prop("text", "B")),
        )
        .with_child(Node::new("c", "Button"));
    let mut session = session(root);
    let result = session.build(&Map::new(), &phone());
    let order: Vec<(&str, Option<&str>, usize)> = result
        .patches
        .iter()
        .map(|patch| match patch {
            Patch::Create {
                node,
                parent,
                index,
                ..
            } => (node.as_str(), parent.as_deref(), *index),
            other => panic!("a build creates only: {other:?}"),
        })
        .collect();
    assert_eq!(
        order,
        [
            ("root", None, 0),
            ("row", Some("root"), 0),
            ("a", Some("row"), 0),
            ("b", Some("row"), 1),
            ("c", Some("root"), 1),
        ]
    );
    assert!(result.diagnostics.is_empty());
}

#[test]
fn update_emits_only_the_values_that_changed() {
    let mut session = session(greeting_tree());
    let state = map(&[("name", text("Ada")), ("n", int(0))]);
    session.build(&state, &phone());

    // Nothing changed: nothing to apply.
    assert!(session.update(&state, &phone()).patches.is_empty());

    // One input changed: one patch, on the node that reads it.
    let next = map(&[("name", text("Ada")), ("n", int(1))]);
    let result = session.update(&next, &phone());
    assert_eq!(result.patches, [set("count", "text", "n=1")]);
    assert_eq!(
        session.value("count", "text"),
        Some(&Value::Text("n=1".into()))
    );
}

#[test]
fn a_failing_binding_keeps_its_last_good_value_and_reports_once_per_turn() {
    let mut session = session(greeting_tree());
    let good = map(&[("name", text("Ada")), ("n", int(0))]);
    session.build(&good, &phone());

    // `name` disappears: the greeting fails, the counter still updates.
    let broken = map(&[("n", int(1))]);
    let result = session.update(&broken, &phone());
    assert_eq!(result.patches, [set("count", "text", "n=1")]);
    assert_eq!(result.diagnostics.len(), 1);
    let BindDiagnostic {
        node,
        property,
        message,
    } = &result.diagnostics[0];
    assert_eq!((node.as_str(), property.as_str()), ("greeting", "text"));
    assert!(message.contains("name"), "{message}");
    assert_eq!(
        session.value("greeting", "text"),
        Some(&Value::Text("Hello, Ada!".into()))
    );

    // Still failing next turn, even though nothing changed: reported again,
    // once, and the value is still the last good one.
    let again = session.update(&broken, &phone());
    assert!(again.patches.is_empty());
    assert_eq!(again.diagnostics.len(), 1);

    // It recovers as soon as the input is back.
    let fixed = map(&[("name", text("Grace")), ("n", int(1))]);
    let recovered = session.update(&fixed, &phone());
    assert_eq!(
        recovered.patches,
        [set("greeting", "text", "Hello, Grace!")]
    );
    assert!(recovered.diagnostics.is_empty());
}

#[test]
fn one_bad_binding_does_not_change_or_block_other_nodes() {
    let root = Node::new("root", "Column")
        .with_child(Node::new("bad", "Label").with_bind("text", "state.nope.deeper"))
        .with_child(Node::new("syntax", "Label").with_bind("text", "state.n +"))
        .with_child(Node::new("assign", "Label").with_bind("text", "state.n = 2"))
        .with_child(Node::new("ok", "Label").with_bind("text", "`n=${state.n}`"));
    let mut session = session(root);
    let state = map(&[("n", int(7))]);
    let built = session.build(&state, &phone());
    assert_eq!(built.diagnostics.len(), 3);
    let failed: Vec<&str> = built.diagnostics.iter().map(|d| d.node.as_str()).collect();
    assert_eq!(failed, ["bad", "syntax", "assign"]);
    assert_eq!(
        session.value("ok", "text"),
        Some(&Value::Text("n=7".into()))
    );
    // Failing bindings with no literal have no value at all, and the state the
    // statement binding tried to assign is untouched.
    assert_eq!(session.value("bad", "text"), None);
    assert_eq!(state["n"].as_int(), Ok(7));

    let next = session.update(&map(&[("n", int(8))]), &phone());
    assert_eq!(next.patches, [set("ok", "text", "n=8")]);
    assert_eq!(next.diagnostics.len(), 3);
}

#[test]
fn a_binding_that_has_not_succeeded_yet_shows_its_literal() {
    let root = Node::new("label", "Label")
        .with_prop("text", "loading...")
        .with_bind("text", "state.title");
    let mut session = session(root);
    let built = session.build(&Map::new(), &phone());
    assert_eq!(built.diagnostics.len(), 1);
    assert_eq!(
        session.value("label", "text"),
        Some(&Value::Text("loading...".into()))
    );
    let loaded = session.update(&map(&[("title", text("Done"))]), &phone());
    assert_eq!(loaded.patches, [set("label", "text", "Done")]);
}

#[test]
fn a_bound_value_wins_over_the_responsive_patch_which_wins_over_the_literal() {
    let root = Node::new("n", "Label")
        .with_prop("size", 10)
        .with_responsive(SizeClass::Medium, "size", 20)
        .with_bind("size", "state.size");
    let mut session = session(root);
    let medium = Environment::new(700.0, 500.0);
    // No state yet: the binding fails, so the responsive value shows.
    session.build(&Map::new(), &medium);
    assert_eq!(session.value("n", "size"), Some(&Value::Int(20)));
    // Once it produces a value, it beats both.
    let state = map(&[("size", int(99))]);
    assert_eq!(
        session.update(&state, &medium).patches,
        [set("n", "size", 99)]
    );
    // The responsive value no longer matters for a bound property.
    assert!(session.update(&state, &phone()).patches.is_empty());
    assert_eq!(session.value("n", "size"), Some(&Value::Int(99)));
}

#[test]
fn a_property_keeps_its_kind_and_integers_are_not_coerced() {
    let root = Node::new("card", "Flex")
        .with_prop("fill", 0)
        .with_bind("fill", "state.weight");
    let mut session = session(root);
    let float = map(&[("weight", Dynamic::from_float(1.0))]);
    let built = session.build(&float, &phone());
    assert_eq!(built.diagnostics.len(), 1);
    assert!(
        built.diagnostics[0].message.contains("int")
            && built.diagnostics[0].message.contains("float"),
        "{}",
        built.diagnostics[0].message
    );
    assert_eq!(session.value("card", "fill"), Some(&Value::Int(0)));
    // An integer is accepted.
    let result = session.update(&map(&[("weight", int(2))]), &phone());
    assert_eq!(result.patches, [set("card", "fill", 2)]);
    assert!(result.diagnostics.is_empty());
    // And a property with no literal is typed by its first good value.
    let untyped = Node::new("bar", "Slider").with_bind("value", "state.v");
    let mut session = Session::new(tree(untyped), Bindings::new());
    session.build(&map(&[("v", int(1))]), &phone());
    let drift = session.update(&map(&[("v", Dynamic::from_float(1.5))]), &phone());
    assert_eq!(drift.diagnostics.len(), 1);
    assert_eq!(session.value("bar", "value"), Some(&Value::Int(1)));
}

#[test]
fn an_enum_property_is_bound_with_a_string() {
    let root = Node::new("cards", "Flex")
        .with_prop("direction", Value::variant("Vertical"))
        .with_bind(
            "direction",
            "if env.width >= 600 { \"Horizontal\" } else { \"Vertical\" }",
        );
    let mut session = session(root);
    session.build(&Map::new(), &phone());
    assert_eq!(
        session.value("cards", "direction"),
        Some(&Value::variant("Vertical"))
    );
    let wide = session.update(&Map::new(), &Environment::new(900.0, 600.0));
    assert_eq!(
        wide.patches,
        [set("cards", "direction", Value::variant("Horizontal"))]
    );
}

#[test]
fn a_binding_that_yields_unit_is_a_diagnostic() {
    let root = Node::new("l", "Label").with_bind("visible", "if state.show { true }");
    let mut session = session(root);
    let result = session.build(&map(&[("show", Dynamic::from(false))]), &phone());
    assert_eq!(result.diagnostics.len(), 1);
    assert!(result.diagnostics[0].message.contains("no value"));
    assert_eq!(session.value("l", "visible"), None);
    let shown = session.update(&map(&[("show", Dynamic::from(true))]), &phone());
    assert_eq!(shown.patches, [set("l", "visible", true)]);
}

#[test]
fn a_nan_result_does_not_repatch_every_turn() {
    let root = Node::new("l", "Label").with_bind("ratio", "0.0 / 0.0");
    let mut session = session(root);
    session.build(&Map::new(), &phone());
    assert!(session.update(&Map::new(), &phone()).patches.is_empty());
}

#[test]
fn rebuilding_forgets_what_was_applied() {
    let mut session = session(greeting_tree());
    let state = map(&[("name", text("Ada")), ("n", int(0))]);
    session.build(&state, &phone());
    session.update(&map(&[("name", text("Bob")), ("n", int(5))]), &phone());

    // A second build describes the whole tree again, from the current state.
    let rebuilt = session.build(&map(&[("name", text("Bob")), ("n", int(5))]), &phone());
    assert_eq!(rebuilt.patches.len(), 3);
    assert!(
        rebuilt
            .patches
            .iter()
            .all(|patch| matches!(patch, Patch::Create { .. }))
    );
    // And a failed binding has no "last good value" from before the rebuild.
    let empty = session.build(&Map::new(), &phone());
    assert_eq!(empty.diagnostics.len(), 2);
    assert_eq!(session.value("greeting", "text"), None);
    // The update after a rebuild is relative to the rebuild, not to history.
    let after = session.update(&map(&[("name", text("Bob")), ("n", int(5))]), &phone());
    assert_eq!(after.patches.len(), 2);
}

#[test]
fn a_node_missing_from_a_new_tree_is_not_diffed() {
    let mut session = session(greeting_tree());
    let state = map(&[("name", text("Ada")), ("n", int(0))]);
    session.build(&state, &phone());

    let reloaded = Node::new("root", "Column")
        .with_child(Node::new("greeting", "Label").with_bind("text", "`Hi, ${state.name}`"))
        .with_child(Node::new("extra", "Label").with_prop("text", "new"));
    session.replace_tree(tree(reloaded));
    assert!(session.tree().contains("extra") && !session.tree().contains("count"));

    let rebuilt = session.build(&state, &phone());
    let created: Vec<&str> = rebuilt.patches.iter().map(Patch::node).collect();
    assert_eq!(created, ["root", "greeting", "extra"]);
    assert_eq!(session.value("count", "text"), None);

    // Only the nodes of the new tree are diffed; `count` is gone for good.
    let update = session.update(&map(&[("name", text("Bob")), ("n", int(1))]), &phone());
    assert_eq!(update.patches, [set("greeting", "text", "Hi, Bob")]);
    assert!(update.diagnostics.is_empty());
}

#[test]
fn update_before_build_is_a_build() {
    let mut session = session(greeting_tree());
    let result = session.update(&map(&[("name", text("Ada")), ("n", int(0))]), &phone());
    assert!(
        result
            .patches
            .iter()
            .all(|patch| matches!(patch, Patch::Create { .. }))
    );
    assert_eq!(result.patches.len(), 3);
}

#[test]
fn only_what_can_have_changed_is_re_evaluated() {
    let root = Node::new("root", "Column")
        .with_child(Node::new("w", "Label").with_bind("text", "`w=${env.width}`"))
        .with_child(Node::new("n", "Label").with_bind("text", "`n=${state.n}`"))
        .with_child(Node::new("k", "Label").with_bind("text", "`k=${1 + 1}`"));
    let bindings = Rc::new(Bindings::new());
    let mut session = Session::new(tree(root), Rc::clone(&bindings));
    let evaluations = || bindings.stats().evaluations;

    let state = map(&[("n", int(1))]);
    session.build(&state, &Environment::new(700.0, 500.0));
    assert_eq!(evaluations(), 3);

    // A resize with the same state: only the binding that reads `env` runs,
    // even though the size class is unchanged, because it reads the width.
    let resized = session.update(&state, &Environment::new(800.0, 500.0));
    assert_eq!(evaluations(), 4);
    assert_eq!(resized.patches, [set("w", "text", "w=800.0")]);

    // A state change in the same environment: only the state reader runs.
    let changed = session.update(&map(&[("n", int(2))]), &Environment::new(800.0, 500.0));
    assert_eq!(evaluations(), 5);
    assert_eq!(changed.patches, [set("n", "text", "n=2")]);

    // Nothing changed: nothing runs. The constant binding ran only at build.
    session.update(&map(&[("n", int(2))]), &Environment::new(800.0, 500.0));
    assert_eq!(evaluations(), 5);
}

#[test]
fn sessions_share_compiled_expressions() {
    let bindings = Rc::new(Bindings::new());
    let mut first = Session::new(tree(greeting_tree()), Rc::clone(&bindings));
    let mut second = Session::new(tree(greeting_tree()), Rc::clone(&bindings));
    let state = map(&[("name", text("Ada")), ("n", int(0))]);
    first.build(&state, &phone());
    second.build(&state, &phone());
    assert_eq!(bindings.stats().compilations, 2);
    assert_eq!(bindings.stats().evaluations, 4);
}

#[test]
fn shrinking_back_unsets_a_property_that_only_a_patch_gave() {
    let root = Node::new("card", "Panel")
        .with_responsive(SizeClass::Medium, "fill", 1)
        .with_responsive(SizeClass::Expanded, "gap", 4);
    let mut session = session(root);
    let none = Map::new();
    assert!(matches!(
        session.build(&none, &phone()).patches[0],
        Patch::Create { ref props, .. } if props.is_empty()
    ));
    assert_eq!(
        session
            .update(&none, &Environment::new(900.0, 600.0))
            .patches,
        [set("card", "fill", 1), set("card", "gap", 4)]
    );
    // Back to a phone: both go back to the default, in one turn.
    let back = session.update(&none, &phone());
    assert_eq!(
        back.patches,
        [
            Patch::Unset {
                node: "card".into(),
                property: "fill".into()
            },
            Patch::Unset {
                node: "card".into(),
                property: "gap".into()
            },
        ]
    );
    assert_eq!(session.value("card", "fill"), None);
}
