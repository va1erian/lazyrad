//! What a binding gets when an entry of `state` is too large to show it.
//!
//! The entry keeps its key and holds an opaque value. The shape of the map is
//! the real one; *using* the value is a diagnostic that names it, never a
//! silent wrong answer.

mod common;

use common::{int, map, phone, text};
use lazyrad_view::bindings::MAX_ARRAY_SIZE;
use lazyrad_view::{BindError, BindingScope, Bindings, Value};
use rhai::{Dynamic, Map};

/// A list over the array limit.
fn big_list() -> Dynamic {
    Dynamic::from_array((0..(MAX_ARRAY_SIZE as i64 * 2)).map(int).collect())
}

fn small_list(len: i64) -> Dynamic {
    Dynamic::from_array((0..len).map(int).collect())
}

fn run(source: &str, state: &Map) -> Result<Value, BindError> {
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(state, &Map::new(), &phone().to_map());
    bindings.evaluate(source, &mut scope, None)
}

/// The diagnostic `source` fails with, which must be a runtime one.
fn failure(source: &str, state: &Map) -> String {
    match run(source, state) {
        Err(BindError::Runtime { message, .. }) => message,
        other => panic!("{source} must fail with a diagnostic, got {other:?}"),
    }
}

/// A diagnostic that names a hidden entry and the limit it passed, in either of
/// the two forms the evaluator uses.
fn names(shown: &str, path: &str) -> bool {
    shown.contains(path)
        && shown.contains(&format!("more than {MAX_ARRAY_SIZE} array elements"))
        && shown.contains("too large")
}

#[test]
fn every_use_of_a_hidden_list_fails_and_names_it() {
    let state = map(&[("items", big_list()), ("items2", big_list()), ("n", int(1))]);
    // Each of these used to give a silent value (`false`, `true`, a type name
    // or the text of a type name). None may.
    for source in [
        "state.items == state.items2",
        "state.items != state.items2",
        "state.items == ()",
        "() == state.items",
        "state.items == 3",
        "state.items != 3",
        "state.items > 1",
        "1 < state.items",
        "state.items <= state.items2",
        "[state.items] == [state.items]",
        "[state.items].index_of(state.items)",
        "`${state.items}`",
        "`${state.items}`.len()",
        "(\"x\" + state.items).len()",
        "state.items.to_string().len()",
        "state.items.to_debug()",
        "if state.items { 1 } else { 2 }",
        "state.items && true",
        "!state.items",
        "state.items ?? 3",
        "state.get(\"items\")",
        "state.items",
        "state.items.len()",
        "state.items[0]",
        "state.items.map(|x| x).len()",
        "#{a: state.items}.a",
        "state.n + state.items.len()",
        "[state.items, state.items2].sort().len()",
        "state.to_json().len()",
        "1 in state.items",
    ] {
        let shown = failure(source, &state);
        assert!(names(&shown, "state.items"), "{source}: {shown}");
    }
    // `type_of` is not available at all, because it would name the type.
    assert!(matches!(
        run("type_of(state.items)", &state),
        Err(BindError::Syntax { .. })
    ));
    // What does not touch the hidden value is unaffected, and so is what has
    // a true answer: a list never equals a scalar, it is still in the map.
    assert_eq!(run("state.n", &state), Ok(Value::Int(1)));
    assert_eq!(run("[state.items].len()", &state), Ok(Value::Int(1)));
    assert_eq!(
        run(
            "switch state.items { 1 => \"one\", _ => \"other\" }",
            &state
        ),
        Ok(Value::Text("other".into()))
    );
}

#[test]
fn every_use_of_a_pruned_nested_entry_fails_and_names_it() {
    let cfg = || {
        Dynamic::from_map(
            [
                ("name".into(), text("main")),
                ("rows".into(), big_list()),
                ("flag".into(), Dynamic::from(true)),
            ]
            .into_iter()
            .collect::<Map>(),
        )
    };
    let state = map(&[("cfg", cfg()), ("cfg2", cfg()), ("n", int(1))]);
    for source in [
        "state.cfg == state.cfg2",
        "state.cfg != state.cfg2",
        "state.cfg == #{name: \"main\", rows: [1], flag: true}",
        "`${state.cfg}`",
        "`${state.cfg}`.len()",
        "state.cfg.to_string().len()",
        "state.cfg.to_json()",
        "state.cfg.to_json().len()",
        "state.cfg.rows",
        "state.cfg[\"rows\"]",
        "state.cfg.rows.len()",
        "state.cfg.rows == state.cfg2.rows",
        "state.cfg.values()[2]",
        "state.cfg.values().to_string().len()",
        "state.cfg.values().filter(|v| v != 1).len()",
        "state.cfg.get(\"rows\")",
        "state.cfg.rows?.len",
    ] {
        let shown = failure(source, &state);
        assert!(
            names(&shown, "state.cfg.rows") || names(&shown, "state.cfg2.rows"),
            "{source}: {shown}"
        );
    }
    // The siblings, and the shape of the map, are the real ones.
    assert_eq!(
        run("state.cfg.name", &state),
        Ok(Value::Text("main".into()))
    );
    assert_eq!(run("state.cfg.flag", &state), Ok(Value::Bool(true)));
    assert_eq!(run("state.cfg.len()", &state), Ok(Value::Int(3)));
    assert_eq!(run("state.cfg.keys().len()", &state), Ok(Value::Int(3)));
    assert_eq!(
        run("state.cfg.contains(\"rows\")", &state),
        Ok(Value::Bool(true))
    );
    assert_eq!(run("\"rows\" in state.cfg", &state), Ok(Value::Bool(true)));
    // A map with a different shape is different, whatever the hidden value is.
    assert_eq!(
        run("state.cfg == #{name: \"main\"}", &state),
        Ok(Value::Bool(false))
    );
    assert_eq!(
        run("state.cfg != #{name: \"main\"}", &state),
        Ok(Value::Bool(true))
    );
}

#[test]
fn map_introspection_sees_the_real_shape_including_hidden_entries() {
    let state = map(&[("items", big_list()), ("n", int(1))]);
    assert_eq!(
        run("state.contains(\"items\")", &state),
        Ok(Value::Bool(true))
    );
    assert_eq!(run("\"items\" in state", &state), Ok(Value::Bool(true)));
    assert_eq!(
        run("state.contains(\"missing\")", &state),
        Ok(Value::Bool(false))
    );
    assert_eq!(run("state.keys().len()", &state), Ok(Value::Int(2)));
    assert_eq!(run("state.len()", &state), Ok(Value::Int(2)));
    assert_eq!(run("state.values().len()", &state), Ok(Value::Int(2)));
}

#[test]
fn a_conversion_that_does_not_run_is_not_refused_and_the_refusal_does_not_stick() {
    let state = map(&[
        ("items", big_list()),
        ("small", small_list(3)),
        ("title", text("<hidden entry")),
        ("n", int(2)),
    ]);
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&state, &Map::new(), &phone().to_map());
    let mut eval = |source: &str| bindings.evaluate(source, &mut scope, None);
    // The branch with the hidden entry is not taken.
    assert_eq!(
        eval("if state.n == 3 { `${state.items}` } else { \"b\" }"),
        Ok(Value::Text("b".into()))
    );
    // A failing evaluation does not poison the next one on the same scope.
    assert!(eval("`${state.items}`").is_err());
    assert_eq!(eval("state.n + 1"), Ok(Value::Int(3)));
    // Data that merely looks like the stand-in's text is data.
    assert_eq!(
        eval("state.title + \"!\""),
        Ok(Value::Text("<hidden entry!".into()))
    );
    // The same refusals happen in a closure that needs the private-copies
    // path (it reads `state` while iterating a list of it).
    for source in [
        "state.small.map(|x| `${state.items}`).len()",
        "state.small.map(|x| x * state.n + state.items.len()).len()",
        "state.small.map(|x| state.items == x).len()",
    ] {
        match eval(source) {
            Err(BindError::Runtime { message, .. }) => {
                assert!(names(&message, "state.items"), "{source}: {message}");
            }
            other => panic!("{source} must fail, got {other:?}"),
        }
    }
    // And the retry path still gives the right answer for the plain use.
    assert_eq!(
        eval("state.small.map(|x| x * state.n).len()"),
        Ok(Value::Int(3))
    );
}
