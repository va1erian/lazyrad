//! What a binding can see of `state`: record lists, long strings, nested maps
//! and hidden entries, at their boundaries.

mod common;

use common::{int, map, phone, text};
use lazyrad_view::bindings::{MAX_ARRAY_SIZE, MAX_MAP_SIZE, MAX_STRING_SIZE};
use lazyrad_view::{BindError, BindingScope, Bindings, Value};
use rhai::{Dynamic, Map};

fn record(fields: usize) -> Dynamic {
    Dynamic::from_map(
        (0..fields)
            .map(|i| (format!("f{i}").into(), int(i as i64)))
            .collect::<Map>(),
    )
}

fn rows(count: usize, fields: usize) -> Dynamic {
    Dynamic::from_array((0..count).map(|_| record(fields)).collect())
}

fn list(len: usize) -> Dynamic {
    Dynamic::from_array((0..len as i64).map(int).collect())
}

/// Evaluates against `state` and also returns what the scope hid.
fn run(source: &str, state: &Map) -> (Result<Value, BindError>, Vec<String>) {
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(state, &Map::new(), &phone().to_map());
    let result = bindings.evaluate(source, &mut scope, None);
    (result, scope.hidden().to_vec())
}

fn message(result: Result<Value, BindError>) -> String {
    match result {
        Err(BindError::Runtime { message, .. } | BindError::Limit { message, .. }) => message,
        other => panic!("expected a diagnostic, got {other:?}"),
    }
}

#[test]
fn a_list_of_records_is_limited_by_its_fields_and_the_boundary_is_exact() {
    // (fields per row, most rows that fit). Rhai counts one map entry per
    // field, and the entry that holds the list costs one more, so the cap is
    // (MAX_MAP_SIZE - 1) / fields rows, and never more than
    // MAX_ARRAY_SIZE rows.
    for (fields, fits) in [(1, 1023), (4, 255), (5, 204), (10, 102), (20, 51)] {
        assert_eq!(
            fits,
            ((MAX_MAP_SIZE - 1) / fields).min(MAX_ARRAY_SIZE),
            "{fields} fields"
        );
        let ok = map(&[("rows", rows(fits, fields))]);
        let (result, hidden) = run("state.rows.len()", &ok);
        assert_eq!(
            result,
            Ok(Value::Int(i64::try_from(fits).unwrap())),
            "{fields} fields"
        );
        assert!(hidden.is_empty(), "{fields} fields: {hidden:?}");
        // The row field is reachable too.
        let (result, _) = run("state.rows[0].f0", &ok);
        assert_eq!(result, Ok(Value::Int(0)));

        // One more row and the whole list is hidden, with a diagnostic that
        // names it and says which limit it passed.
        let over = map(&[("rows", rows(fits + 1, fields))]);
        let (result, hidden) = run("state.rows.len()", &over);
        let shown = message(result);
        assert!(
            shown.contains("Property not found: rows")
                && shown.contains("state.rows (")
                && shown.contains("too large"),
            "{fields} fields: {shown}"
        );
        assert_eq!(hidden.len(), 1);
    }
}

#[test]
fn a_hidden_list_names_the_limit_it_passed() {
    let (_, hidden) = run("1", &map(&[("rows", rows(MAX_ARRAY_SIZE + 1, 1))]));
    assert_eq!(
        hidden,
        [format!(
            "state.rows (more than {MAX_ARRAY_SIZE} array elements)"
        )]
    );
    let (_, hidden) = run("1", &map(&[("rows", rows(MAX_MAP_SIZE / 5 + 1, 5))]));
    assert_eq!(
        hidden,
        [format!("state.rows (more than {MAX_MAP_SIZE} map entries)")]
    );
    let titles: Vec<Dynamic> = (0..100).map(|_| text(&"t".repeat(400))).collect();
    let (_, hidden) = run("1", &map(&[("titles", Dynamic::from_array(titles))]));
    assert_eq!(
        hidden,
        [format!(
            "state.titles (more than {MAX_STRING_SIZE} bytes of text)"
        )]
    );
}

#[test]
fn a_long_string_is_readable_and_using_it_names_the_entry() {
    let long = "x".repeat(MAX_STRING_SIZE + 8);
    let state = map(&[
        ("doc", text(&long)),
        (
            "note",
            Dynamic::from_map(
                [("body".into(), text(&long)), ("title".into(), text("T"))]
                    .into_iter()
                    .collect::<Map>(),
            ),
        ),
    ]);
    // Reading is fine, at the top level and nested.
    assert_eq!(run("state.doc", &state).0, Ok(Value::Text(long.clone())));
    assert_eq!(
        run("state.note.body", &state).0,
        Ok(Value::Text(long.clone()))
    );
    assert_eq!(
        run("state.note.title", &state).0,
        Ok(Value::Text("T".into()))
    );
    // Using it is a diagnostic that says which strings are too long.
    let shown = message(run("state.doc.len()", &state).0);
    assert!(
        shown.contains("Length of string exceeds the size limit")
            && shown.contains(&format!("state.doc ({} bytes)", long.len())),
        "{shown}"
    );
    let shown = message(run("state.note.body.to_upper()", &state).0);
    assert!(
        shown.contains(&format!("state.note.body ({} bytes)", long.len())),
        "{shown}"
    );
    // At the limit exactly, a method call works.
    let exact = map(&[("doc", text(&"x".repeat(MAX_STRING_SIZE)))]);
    assert_eq!(
        run("state.doc.len()", &exact).0,
        Ok(Value::Int(i64::try_from(MAX_STRING_SIZE).unwrap()))
    );
}

#[test]
fn a_big_nested_entry_is_removed_and_its_siblings_stay_usable() {
    let cfg = Dynamic::from_map(
        [
            ("name".into(), text("main")),
            ("rows".into(), list(2_000)),
            ("flag".into(), Dynamic::from(true)),
        ]
        .into_iter()
        .collect::<Map>(),
    );
    let state = map(&[("cfg", cfg), ("n", int(1))]);
    assert_eq!(
        run("state.cfg.name", &state).0,
        Ok(Value::Text("main".into()))
    );
    assert_eq!(run("state.cfg.flag", &state).0, Ok(Value::Bool(true)));
    let (result, hidden) = run("state.cfg.rows.len()", &state);
    let shown = message(result);
    assert!(
        shown.contains("state.cfg.rows (more than 1024 array elements)"),
        "{shown}"
    );
    assert_eq!(hidden.len(), 1);
}

#[test]
fn every_use_of_a_hidden_entry_fails_and_names_it() {
    let state = map(&[("items", list(1100)), ("items2", list(1100)), ("n", int(1))]);
    let uses = [
        // The silent ones: each used to yield a value.
        "type_of(state.items)",
        "`${state.items}`",
        "state.items == state.items2",
        "state.items != state.items2",
        "if state.items { 1 } else { 2 }",
        "!state.items",
        // The loud ones, for completeness.
        "state.items",
        "state.items.len()",
        "state.items[0]",
        "state.items.map(|x| x).len()",
        "[state.items].len()",
        "#{a: state.items}.len()",
        "state.n + state.items.len()",
    ];
    for source in uses {
        let (result, _) = run(source, &state);
        let shown = match result {
            Err(BindError::Runtime { message, .. }) => message,
            other => panic!("{source} must fail, got {other:?}"),
        };
        assert!(
            shown.contains("Property not found")
                && shown.contains("state.items (more than 1024 array elements)"),
            "{source}: {shown}"
        );
    }
    // The ones that are not hidden are unaffected.
    assert_eq!(run("state.n", &state).0, Ok(Value::Int(1)));
}

#[test]
fn map_introspection_does_not_see_a_hidden_entry_which_is_documented() {
    // A removed entry is absent from the map a binding sees, so asking the
    // *map* about it is not an error. This pins the documented behaviour.
    let state = map(&[("items", list(1100)), ("n", int(1))]);
    assert_eq!(
        run("state.contains(\"items\")", &state).0,
        Ok(Value::Bool(false))
    );
    assert_eq!(run("state.keys().len()", &state).0, Ok(Value::Int(1)));
    assert_eq!(run("state.len()", &state).0, Ok(Value::Int(1)));
}

#[test]
fn many_plain_entries_over_the_map_limit_are_left_alone() {
    // 1500 scalars cannot be pruned (they are not containers), so they all
    // stay readable; asking the whole map is then over Rhai's limit.
    let state: Map = (0..1500)
        .map(|i| (format!("k{i:04}").into(), int(i)))
        .collect();
    let (result, hidden) = run("state.k0007 + state.k1499", &state);
    assert_eq!(result, Ok(Value::Int(1506)));
    assert!(hidden.is_empty());
    let shown = message(run("state.len()", &state).0);
    assert!(shown.contains("Size of object map"), "{shown}");
}

#[test]
fn two_lists_that_fit_alone_but_not_together_hide_the_larger_one() {
    let state = map(&[("a", list(600)), ("b", list(700)), ("n", int(1))]);
    let (result, hidden) = run("state.a.len()", &state);
    assert_eq!(result, Ok(Value::Int(600)));
    assert_eq!(hidden, ["state.b (700 array elements)"]);
    let (result, _) = run("state.b.len()", &state);
    assert!(message(result).contains("state.b (700 array elements)"));
}

/// The advertised number is the enforced one: 1024 elements are visible and
/// usable, 1025 are not.
#[test]
fn the_advertised_1024_elements_are_all_usable() {
    assert_eq!(MAX_ARRAY_SIZE, 1024);
    assert_eq!(MAX_MAP_SIZE, 1024);
    let at = map(&[("items", list(1024))]);
    let (result, hidden) = run("state.items.len()", &at);
    assert_eq!(result, Ok(Value::Int(1024)));
    assert!(hidden.is_empty(), "{hidden:?}");
    // Every method works on a full list, not just `len`.
    let (result, _) = run("state.items.map(|x| x + 1).len()", &at);
    assert_eq!(result, Ok(Value::Int(1024)));
    let (result, _) = run("state.items.filter(|x| x >= 1000).len()", &at);
    assert_eq!(result, Ok(Value::Int(24)));

    let over = map(&[("items", list(1025))]);
    let (result, hidden) = run("state.items.len()", &over);
    assert!(message(result).contains("state.items (more than 1024 array elements)"));
    assert_eq!(hidden.len(), 1);
}

#[test]
fn a_nested_map_at_the_entry_limit_is_usable_and_one_more_is_not() {
    let entries = |n: usize| {
        Dynamic::from_map(
            (0..n)
                .map(|i| (format!("c{i:04}").into(), int(i as i64)))
                .collect::<Map>(),
        )
    };
    // 1023 entries plus the key that holds them is 1024 map entries.
    let at = map(&[("cfg", entries(1023))]);
    let (result, hidden) = run("state.cfg.len()", &at);
    assert_eq!(result, Ok(Value::Int(1023)));
    assert!(hidden.is_empty(), "{hidden:?}");
    assert_eq!(run("state.cfg.c1022", &at).0, Ok(Value::Int(1022)));

    let over = map(&[("cfg", entries(1024))]);
    let (result, hidden) = run("state.cfg.len()", &over);
    assert!(message(result).contains("state.cfg (1024 map entries)"));
    assert_eq!(hidden, ["state.cfg (1024 map entries)"]);
}
