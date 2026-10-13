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
    // Rhai counts one map entry per field, and the entry that holds the list
    // costs one more, so the most rows are (MAX_MAP_SIZE - 1) / fields, and
    // never more than MAX_ARRAY_SIZE.
    for fields in [1, 4, 5, 10, 20] {
        let fits = ((MAX_MAP_SIZE - 1) / fields).min(MAX_ARRAY_SIZE);
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
            shown.contains("state.rows (") && shown.contains("too large"),
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
    let note = format!("state.cfg.rows (more than {MAX_ARRAY_SIZE} array elements)");
    assert!(shown.contains(&note), "{shown}");
    assert_eq!(hidden, [note]);
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
    // Each fits alone, the two together do not.
    let (small, large) = (MAX_ARRAY_SIZE * 2 / 5, MAX_ARRAY_SIZE * 4 / 5);
    let state = map(&[("a", list(small)), ("b", list(large)), ("n", int(1))]);
    let (result, hidden) = run("state.a.len()", &state);
    assert_eq!(result, Ok(Value::Int(i64::try_from(small).unwrap())));
    let note = format!("state.b ({large} array elements)");
    assert_eq!(hidden, std::slice::from_ref(&note));
    let (result, _) = run("state.b.len()", &state);
    assert!(message(result).contains(&note));
}

/// The advertised number is the enforced one: a list of the limit's length is
/// visible and usable, one more element is not.
#[test]
fn the_advertised_array_limit_is_the_enforced_one() {
    let full = i64::try_from(MAX_ARRAY_SIZE).unwrap();
    let at = map(&[("items", list(MAX_ARRAY_SIZE))]);
    let (result, hidden) = run("state.items.len()", &at);
    assert_eq!(result, Ok(Value::Int(full)));
    assert!(hidden.is_empty(), "{hidden:?}");
    // Every method works on a full list, not just `len`.
    let (result, _) = run("state.items.map(|x| x + 1).len()", &at);
    assert_eq!(result, Ok(Value::Int(full)));
    let last_twelve = format!("state.items.filter(|x| x >= {}).len()", full - 12);
    let (result, _) = run(&last_twelve, &at);
    assert_eq!(result, Ok(Value::Int(12)));

    let over = map(&[("items", list(MAX_ARRAY_SIZE + 1))]);
    let (result, hidden) = run("state.items.len()", &over);
    let note = format!("state.items (more than {MAX_ARRAY_SIZE} array elements)");
    assert!(message(result).contains(&note));
    assert_eq!(hidden, [note]);
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
    // One entry fewer than the limit, plus the key that holds them, is the
    // limit.
    let inside = MAX_MAP_SIZE - 1;
    let at = map(&[("cfg", entries(inside))]);
    let (result, hidden) = run("state.cfg.len()", &at);
    assert_eq!(result, Ok(Value::Int(i64::try_from(inside).unwrap())));
    assert!(hidden.is_empty(), "{hidden:?}");
    let last = format!("state.cfg.c{:04}", inside - 1);
    assert_eq!(
        run(&last, &at).0,
        Ok(Value::Int(i64::try_from(inside).unwrap() - 1))
    );

    let over = map(&[("cfg", entries(MAX_MAP_SIZE))]);
    let (result, hidden) = run("state.cfg.len()", &over);
    let note = format!("state.cfg ({MAX_MAP_SIZE} map entries)");
    assert!(message(result).contains(&note));
    assert_eq!(hidden, [note]);
}

/// Every key of `state` costs one map entry, so a sibling can push a full list
/// over the limit: the boundary moves by one for each other key.
#[test]
fn a_sibling_key_moves_the_boundary_of_a_full_list_by_one() {
    let one_field = |n: usize| Dynamic::from_array((0..n).map(|_| record(1)).collect());
    // One row fewer than the limit is that many entries, and the key holding
    // them is the last one the limit allows: it fits alone.
    let most = MAX_MAP_SIZE - 1;
    let alone = map(&[("rows", one_field(most))]);
    let (result, hidden) = run("state.rows.len()", &alone);
    assert_eq!(result, Ok(Value::Int(i64::try_from(most).unwrap())));
    assert!(hidden.is_empty(), "{hidden:?}");
    // With one sibling key it is over: the list is hidden, and says why.
    let crowded = map(&[("rows", one_field(most)), ("n", int(1))]);
    let (result, hidden) = run("state.rows.len()", &crowded);
    let note = format!("state.rows ({most} array elements, {most} map entries)");
    assert!(message(result).contains(&note));
    assert_eq!(hidden, [note]);
    // One row fewer and the sibling fits beside it.
    let fits = map(&[("rows", one_field(most - 1)), ("n", int(1))]);
    let (result, hidden) = run("state.rows.len() + state.n", &fits);
    assert_eq!(result, Ok(Value::Int(i64::try_from(most).unwrap())));
    assert!(hidden.is_empty(), "{hidden:?}");
}
