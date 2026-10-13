//! Closures that read `state` while a `state` list is their receiver.
//!
//! Rhai shares a captured variable, and a shared value cannot be read while it
//! is borrowed as a receiver ("Data race detected"). The evaluator retries such
//! a source on private copies of `state`, `props` and `env`.

mod common;

use common::{int, map, phone, text};
use lazyrad_view::{BindError, BindingScope, Bindings, Value};
use rhai::{Dynamic, Map};

fn state() -> Map {
    map(&[
        ("n", int(3)),
        (
            "items",
            Dynamic::from_array(vec![int(1), int(2), int(3), int(4), int(5)]),
        ),
        ("name", text("Ada")),
    ])
}

fn evaluate(
    bindings: &Bindings,
    scope: &mut BindingScope,
    source: &str,
) -> Result<Value, BindError> {
    bindings.evaluate(source, scope, None)
}

#[test]
fn a_closure_may_read_state_while_iterating_a_state_list() {
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&state(), &Map::new(), &phone().to_map());
    let cases: &[(&str, Value)] = &[
        // The three forms the reviewer reported.
        ("state.items.map(|x| x * state.n).len()", Value::Int(5)),
        ("state.items.map(|x| x * state.n)[1]", Value::Int(6)),
        ("state.items.filter(|x| x > state.n).len()", Value::Int(2)),
        ("state.items.some(|x| x == state.n)", Value::Bool(true)),
        (
            "state.items.some(|x| x == state.n + 10)",
            Value::Bool(false),
        ),
        // The other variables, alone and mixed.
        (
            "state.items.filter(|x| x > env.width / 100).len()",
            Value::Int(2),
        ),
        (
            "state.items.map(|x| x + state.n + env.width.to_int()).len()",
            Value::Int(5),
        ),
        // Several closures in one expression.
        (
            "state.items.filter(|x| x > state.n).map(|x| x * state.n).len()",
            Value::Int(2),
        ),
        // A closure that never touches the receiver's variable keeps working.
        ("[1, 2, 3].map(|x| x + state.n).len()", Value::Int(3)),
        // The hoisted spelling is equivalent and stays on the fast path.
        (
            "(|n| state.items.filter(|x| x > n).len()).call(state.n)",
            Value::Int(2),
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(
            evaluate(&bindings, &mut scope, source).as_ref(),
            Ok(expected),
            "{source}"
        );
    }
    // The shared scope is intact for the rest of the turn.
    assert_eq!(
        evaluate(&bindings, &mut scope, "state.n"),
        Ok(Value::Int(3))
    );
    assert_eq!(
        evaluate(&bindings, &mut scope, "state.items.len()"),
        Ok(Value::Int(5))
    );
}

#[test]
fn the_retry_is_remembered_per_source() {
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&state(), &Map::new(), &phone().to_map());
    let source = "state.items.filter(|x| x > state.n).len()";
    for _ in 0..4 {
        assert_eq!(evaluate(&bindings, &mut scope, source), Ok(Value::Int(2)));
    }
    let stats = bindings.stats();
    assert_eq!((stats.compilations, stats.evaluations), (1, 4));
}

#[test]
fn private_copies_cannot_leak_a_change_back_into_the_state() {
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&state(), &Map::new(), &phone().to_map());
    // A mutating method on the copy is still refused, and the original is
    // untouched afterwards.
    let result = evaluate(
        &bindings,
        &mut scope,
        "state.items.map(|x| x + state.n).push(1)",
    );
    assert!(result.is_err(), "{result:?}");
    assert_eq!(
        evaluate(&bindings, &mut scope, "state.items.len()"),
        Ok(Value::Int(5))
    );
}

#[test]
fn a_closure_inside_a_closure_is_a_pinned_limitation() {
    // The inner closure captures the outer closure's own `state` parameter,
    // which the outer body also uses as a receiver. Rescuing that would need a
    // copy of the state per element, so it reports the race instead.
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&state(), &Map::new(), &phone().to_map());
    let nested = "state.items.map(|x| state.items.filter(|y| y > state.n).len()).len()";
    let result = evaluate(&bindings, &mut scope, nested);
    match result {
        Err(BindError::Runtime { message, .. }) => {
            assert_eq!(message, "Data race detected", "{message}");
        }
        other => panic!("expected the pinned data race, got {other:?}"),
    }
    // It is the inner closure that matters, not what the outer one iterates.
    let inner_only = "[10, 20].map(|x| state.items.filter(|y| y > state.n).len() + x).len()";
    assert!(matches!(
        evaluate(&bindings, &mut scope, inner_only),
        Err(BindError::Runtime { ref message, .. }) if message == "Data race detected"
    ));
    // Reading `state` inside the inner closure without using it as a receiver
    // in the outer body is fine.
    let plain = "[10, 20].map(|x| [1, 2, 3].filter(|y| y > state.n).len() + x).len()";
    assert_eq!(evaluate(&bindings, &mut scope, plain), Ok(Value::Int(2)));
}
