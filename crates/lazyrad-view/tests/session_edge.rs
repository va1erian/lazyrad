//! Session edge cases: odd environments, many bindings, exact diagnostics.

mod common;

use std::rc::Rc;

use common::{int, map, phone, text, tree};
use lazyrad_view::bindings::{MAX_ARRAY_SIZE, MAX_OPERATIONS};
use lazyrad_view::{
    BindDiagnostic, BindingScope, Bindings, Environment, Node, Patch, Session, SizeClass, Value,
};
use rhai::{Dynamic, Map};

fn set(node: &str, property: &str, value: impl Into<Value>) -> Patch {
    Patch::Set {
        node: node.into(),
        property: property.into(),
        value: value.into(),
    }
}

#[test]
fn nan_and_negative_widths_are_compact_and_do_not_re_evaluate() {
    let root = Node::new("root", "Flex")
        .with_prop("direction", Value::variant("Vertical"))
        .with_responsive(SizeClass::Medium, "direction", Value::variant("Horizontal"))
        .with_child(Node::new("w", "Label").with_bind("text", "`w=${env.width}`"));
    let bindings = Rc::new(Bindings::new());
    let mut session = Session::new(tree(root), Rc::clone(&bindings));
    let state = Map::new();

    let nan = Environment::new(f32::NAN, f32::NAN);
    session.build(&state, &nan);
    assert_eq!(session.size_class(), Some(SizeClass::Compact));
    assert_eq!(
        session.value("root", "direction"),
        Some(&Value::variant("Vertical"))
    );
    assert_eq!(
        session.value("w", "text"),
        Some(&Value::Text("w=NaN".into()))
    );
    let evaluated = bindings.stats().evaluations;

    // The same NaN environment is not a change, so nothing runs: it used to
    // re-evaluate every `env` binding on every turn.
    let again = session.update(&state, &nan);
    assert!(again.patches.is_empty() && again.diagnostics.is_empty());
    assert_eq!(bindings.stats().evaluations, evaluated);

    // A negative width is Compact too, and is a real change from NaN.
    let negative = Environment::new(-50.0, 640.0);
    let turn = session.update(&state, &negative);
    assert_eq!(turn.patches, [set("w", "text", "w=-50.0")]);
    assert_eq!(session.size_class(), Some(SizeClass::Compact));

    // Negative zero is zero.
    let zero = Environment::new(0.0, 640.0);
    let negative_zero = Environment::new(-0.0, 640.0);
    session.update(&state, &zero);
    let evaluated = bindings.stats().evaluations;
    assert!(session.update(&state, &negative_zero).patches.is_empty());
    assert_eq!(bindings.stats().evaluations, evaluated);
}

#[test]
fn many_distinct_bindings_are_compiled_once_each_across_turns() {
    // More sources than the cache's base capacity (4096): a session raises it
    // to fit, so turns do not recompile. This used to cost three compiles per
    // source in three turns.
    let count = 5000;
    let mut root = Node::new("root", "Column");
    for i in 0..count {
        root = root.with_child(
            Node::new(format!("n{i}"), "Label").with_bind("text", format!("`${{state.n + {i}}}`")),
        );
    }
    let bindings = Rc::new(Bindings::new());
    let mut session = Session::new(tree(root), Rc::clone(&bindings));
    for turn in 0..3_i64 {
        let result = session.update(&map(&[("n", int(turn))]), &phone());
        assert!(result.diagnostics.is_empty(), "turn {turn}");
    }
    let stats = bindings.stats();
    assert_eq!(stats.compilations, u64::try_from(count).unwrap());
    assert_eq!(stats.evaluations, 3 * u64::try_from(count).unwrap());
}

#[test]
fn the_cache_evicts_the_least_recently_used_and_never_everything() {
    let bindings = Bindings::new();
    let state = Map::new();
    let mut scope = BindingScope::new(&state, &Map::new(), &phone().to_map());
    let mut eval = |i: usize| {
        bindings
            .evaluate(&format!("{i} + 1"), &mut scope, None)
            .unwrap()
    };
    // Twice the base capacity, so eviction has run many times.
    for i in 0..9000 {
        assert_eq!(eval(i), Value::Int(i64::try_from(i).unwrap() + 1));
    }
    let after_fill = bindings.stats().compilations;
    assert_eq!(after_fill, 9000);
    // The most recent sources survived the evictions: no recompilation.
    let eval = |i: usize| {
        let mut scope = BindingScope::new(&Map::new(), &Map::new(), &phone().to_map());
        bindings
            .evaluate(&format!("{i} + 1"), &mut scope, None)
            .unwrap()
    };
    for i in 8900..9000 {
        eval(i);
    }
    assert_eq!(bindings.stats().compilations, after_fill);
    // The oldest were evicted, so one comes back as a recompilation.
    eval(0);
    assert_eq!(bindings.stats().compilations, after_fill + 1);
}

#[test]
fn failing_bindings_report_these_exact_diagnostics() {
    let root = Node::new("root", "Column")
        .with_child(Node::new("missing", "Label").with_bind("text", "state.title"))
        .with_child(Node::new("nested", "Label").with_bind("text", "state.user.name"))
        .with_child(Node::new("syntax", "Label").with_bind("text", "state.n +"))
        .with_child(Node::new("assign", "Label").with_bind("text", "state.n = 1"))
        .with_child(Node::new("unit", "Label").with_bind("visible", "if state.n > 5 { true }"))
        .with_child(
            Node::new("kind", "Flex")
                .with_prop("fill", 0)
                .with_bind("fill", "1.5"),
        )
        .with_child(Node::new("unknown", "Label").with_bind("text", "controls.name_edit.text"))
        .with_child(Node::new("zero", "Label").with_bind("text", "state.n / 0"))
        .with_child(Node::new("call", "Label").with_bind("text", "sleep(1)"))
        .with_child(
            Node::new("hog", "Label").with_bind("text", "(|f| f.call(f)).call(|f| f.call(f))"),
        );
    let mut session = Session::new(tree(root), Bindings::new());
    let result = session.build(&map(&[("n", int(1))]), &phone());
    let lines: Vec<String> = result.diagnostics.iter().map(ToString::to_string).collect();
    assert_eq!(
        lines,
        [
            "missing.text: Property not found: title (line 1, column 7)",
            "nested.text: Property not found: user (line 1, column 7)",
            "syntax.text: syntax error: Script is incomplete (line 1, column 10)",
            "assign.text: syntax error: Unexpected '=' (line 1, column 9)",
            "unit.visible: the expression produced no value (`()`)",
            "kind.fill: expected int, got float; values are not coerced \
             (convert explicitly, for example with `.to_float()`)",
            "unknown.text: syntax error: Undefined variable: controls (line 1, column 1)",
            "zero.text: Division by zero: 1 / 0",
            "call.text: Function not found: sleep (i64) (line 1, column 1)",
            "hog.text: the expression exceeded the call depth limit of 4 (line 1, column 17)",
        ]
    );
    // The structured form carries the same three parts.
    assert_eq!(
        result.diagnostics[0],
        BindDiagnostic {
            node: "missing".into(),
            property: "text".into(),
            message: "Property not found: title (line 1, column 7)".into(),
        }
    );
}

#[test]
fn the_operation_limit_diagnostic_names_the_limit() {
    // 250 iterations over 250 inner ones: about 250 000 operations.
    let lit = vec!["0"; 250].join(",");
    let root = Node::new("hog", "Label").with_bind(
        "text",
        format!("[{lit}].map(|x| [{lit}].map(|y| y).len()).len()"),
    );
    let mut session = Session::new(tree(root), Bindings::new());
    let result = session.build(&Map::new(), &phone());
    assert_eq!(result.diagnostics.len(), 1);
    assert!(
        result.diagnostics[0].message.starts_with(&format!(
            "the expression exceeded the limit of {MAX_OPERATIONS} operations"
        )),
        "{}",
        result.diagnostics[0].message
    );
}

#[test]
fn a_change_inside_a_hidden_list_is_not_a_change_to_any_binding() {
    // Bindings cannot see an entry that is over the visible budget, so a turn
    // that only changes it has nothing to re-evaluate (and does not even look
    // at its 200 000 rows beyond the budget).
    let rows = |first: i64| {
        let mut rows: Vec<Dynamic> = (0..200_000).map(int).collect();
        rows[0] = int(first);
        Dynamic::from_array(rows)
    };
    let root = Node::new("title", "Label").with_bind("text", "`n=${state.n}`");
    let bindings = Rc::new(Bindings::new());
    let mut session = Session::new(tree(root), Rc::clone(&bindings));
    let state = |first: i64, n: i64| map(&[("rows", rows(first)), ("n", int(n))]);

    session.build(&state(0, 1), &phone());
    assert_eq!(bindings.stats().evaluations, 1);
    let same = session.update(&state(99, 1), &phone());
    assert!(same.patches.is_empty());
    assert_eq!(bindings.stats().evaluations, 1);
    let changed = session.update(&state(99, 2), &phone());
    assert_eq!(changed.patches, [set("title", "text", "n=2")]);
    assert_eq!(bindings.stats().evaluations, 2);
}

#[test]
fn a_hidden_state_entry_is_named_in_the_diagnostic() {
    let rows: Vec<Dynamic> = (0..(MAX_ARRAY_SIZE as i64 * 2)).map(int).collect();
    let state = map(&[("rows", Dynamic::from_array(rows)), ("title", text("Hi"))]);
    let root = Node::new("root", "Column")
        .with_child(Node::new("count", "Label").with_bind("text", "state.rows.len()"))
        .with_child(Node::new("title", "Label").with_bind("text", "state.title"));
    let mut session = Session::new(tree(root), Bindings::new());
    let result = session.build(&state, &phone());
    assert_eq!(result.diagnostics.len(), 1);
    let line = result.diagnostics[0].to_string();
    assert!(
        line.starts_with("count.text: ")
            && line.contains(&format!(
                "state.rows (more than {MAX_ARRAY_SIZE} array elements)"
            ))
            && line.contains("too large"),
        "{line}"
    );
    assert_eq!(
        session.value("title", "text"),
        Some(&Value::Text("Hi".into()))
    );
}

/// A binding that branches on a hidden entry's presence sees the real shape,
/// and one that uses its value fails with a diagnostic naming it, instead of
/// showing a silent wrong branch.
#[test]
fn a_binding_that_asks_whether_a_hidden_entry_exists_gets_the_true_answer() {
    let rows: Vec<Dynamic> = (0..(MAX_ARRAY_SIZE as i64 * 2)).map(int).collect();
    let state = map(&[("rows", Dynamic::from_array(rows))]);
    let root = Node::new("root", "Column")
        .with_child(Node::new("has", "Label").with_bind(
            "text",
            "if state.contains(\"rows\") { \"present\" } else { \"none\" }",
        ))
        .with_child(Node::new("count", "Label").with_bind("text", "`${state.rows}`"));
    let mut session = Session::new(tree(root), Bindings::new());
    let result = session.build(&state, &phone());
    assert_eq!(
        session.value("has", "text"),
        Some(&Value::Text("present".into()))
    );
    assert_eq!(result.diagnostics.len(), 1);
    let line = result.diagnostics[0].to_string();
    assert!(
        line.starts_with("count.text: ") && line.contains("`state.rows` is hidden"),
        "{line}"
    );
}
