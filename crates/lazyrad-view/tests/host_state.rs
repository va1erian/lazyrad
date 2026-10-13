//! The host's `state` is never written through by a binding.
//!
//! A host may keep part of its state in shared cells (`Dynamic::into_shared`).
//! Cloning such a value clones the handle, so a script that could mutate it
//! would change the host's own data. Every variable is therefore deep-copied
//! into the scope, and the retry path copies again.

mod common;

use common::{int, map, phone};
use lazyrad_view::{BindError, BindingScope, Bindings, Node, Session, Value};
use rhai::{Array, Dynamic, Map};

/// A host state whose list and nested map live in shared cells.
fn host() -> (Map, Dynamic, Dynamic) {
    let items = Dynamic::from_array((1..=5).map(int).collect()).into_shared();
    let cfg = Dynamic::from_map(
        [(
            "rows".into(),
            Dynamic::from_array(vec![int(1)]).into_shared(),
        )]
        .into_iter()
        .collect::<Map>(),
    )
    .into_shared();
    let state = map(&[
        ("items", items.clone()),
        ("cfg", cfg.clone()),
        ("n", int(2)),
    ]);
    (state, items, cfg)
}

fn len(shared: &Dynamic) -> usize {
    shared
        .read_lock::<Array>()
        .expect("the host's list is still an array")
        .len()
}

fn evaluate(
    bindings: &Bindings,
    scope: &mut BindingScope,
    source: &str,
) -> Result<Value, BindError> {
    bindings.evaluate(source, scope, None)
}

#[test]
fn a_binding_cannot_change_a_shared_host_list_on_the_first_path() {
    let (state, items, _) = host();
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&state, &Map::new(), &phone().to_map());
    for source in [
        "state.items.push(9)",
        "state.items.clear()",
        "state.items.pop()",
    ] {
        let result = evaluate(&bindings, &mut scope, source);
        assert!(result.is_err(), "{source}: {result:?}");
        assert_eq!(len(&items), 5, "{source} changed the host's list");
    }
    // What the binding read before and after is the same.
    assert_eq!(
        evaluate(&bindings, &mut scope, "state.items.len()"),
        Ok(Value::Int(5))
    );
}

#[test]
fn a_binding_cannot_change_a_shared_host_list_on_the_retry_path() {
    let (state, items, _) = host();
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&state, &Map::new(), &phone().to_map());
    // The closure makes the first attempt race and the source is retried on
    // private copies. The trailing `pop` is the write that used to reach the
    // host's list through the shallow copy.
    let sources = [
        "state.items.map(|x| x * state.n).len() + state.items.pop()",
        "state.items.filter(|x| x > state.n).len() + state.items.pop()",
    ];
    for source in sources {
        let result = evaluate(&bindings, &mut scope, source);
        assert!(result.is_err(), "{source}: {result:?}");
        assert_eq!(len(&items), 5, "{source} changed the host's list");
    }
    // A plain read on the retry path still works and sees the host's values.
    assert_eq!(
        evaluate(
            &bindings,
            &mut scope,
            "state.items.map(|x| x * state.n).len()"
        ),
        Ok(Value::Int(5))
    );
    assert_eq!(len(&items), 5);
}

#[test]
fn shared_values_nested_in_state_do_not_panic_rhais_size_check() {
    // Rhai's own accounting is `unreachable!` on a shared value found inside
    // a map, so a method call on the whole map used to be a panic risk.
    let (state, _, cfg) = host();
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&state, &Map::new(), &phone().to_map());
    assert_eq!(
        evaluate(&bindings, &mut scope, "state.keys().len()"),
        Ok(Value::Int(3))
    );
    assert_eq!(
        evaluate(&bindings, &mut scope, "state.cfg.keys().len()"),
        Ok(Value::Int(1))
    );
    assert_eq!(
        evaluate(&bindings, &mut scope, "state.cfg.rows.len()"),
        Ok(Value::Int(1))
    );
    // The host's nested cell is untouched.
    assert!(cfg.read_lock::<Map>().is_some());
}

#[test]
fn a_session_turn_leaves_the_host_state_alone() {
    let (state, items, cfg) = host();
    let root = Node::new("root", "Column")
        .with_child(Node::new("popper", "Label").with_bind("text", "`${state.items.pop()}`"))
        .with_child(Node::new("count", "Label").with_bind("text", "`${state.items.len()}`"))
        .with_child(Node::new("clearer", "Label").with_bind("text", "`${state.cfg.rows.clear()}`"));
    let mut session = Session::new(common::tree(root), Bindings::new());
    let built = session.build(&state, &phone());
    assert_eq!(built.diagnostics.len(), 2, "{:?}", built.diagnostics);
    assert_eq!(
        session.value("count", "text"),
        Some(&Value::Text("5".into()))
    );
    session.update(&state, &phone());
    assert_eq!(len(&items), 5);
    let rows = cfg.read_lock::<Map>().unwrap();
    assert_eq!(rows["rows"].read_lock::<Array>().unwrap().len(), 1);
}
