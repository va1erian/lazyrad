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

/// The number of private copies of `state` one evaluation of `source` makes,
/// over a list of `len` integers. Counting copies is timing-insensitive: a
/// retry that cloned the state for every closure call would make this grow
/// with `len`.
fn copies_per_evaluation(source: &str, len: usize) -> u64 {
    let items = Dynamic::from_array((0..len as i64).map(int).collect());
    let state = map(&[("n", int(2)), ("items", items)]);
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&state, &Map::new(), &phone().to_map());
    // The first evaluation finds the race and retries; later ones go straight
    // to the copying engine. Count the second and third.
    assert!(evaluate(&bindings, &mut scope, source).is_ok());
    let before = bindings.stats().state_copies;
    for _ in 0..2 {
        assert!(evaluate(&bindings, &mut scope, source).is_ok());
    }
    let spent = bindings.stats().state_copies - before;
    assert_eq!(
        spent % 2,
        0,
        "each evaluation copies the same number of times"
    );
    spent / 2
}

#[test]
fn the_retry_copies_the_state_a_fixed_number_of_times_not_once_per_call() {
    for source in [
        "state.items.map(|x| x * state.n).len()",
        "state.items.filter(|x| x > state.n).len()",
        "state.items.some(|x| x == state.n)",
    ] {
        let few = copies_per_evaluation(source, 4);
        let many = copies_per_evaluation(source, 500);
        assert!(few > 0, "{source}: the retry path must be the one measured");
        assert_eq!(few, many, "{source}: copies must not grow with the list");
        assert!(many <= 4, "{source}: {many} copies");
    }
    // A source that does not need the retry makes none.
    assert_eq!(
        copies_per_evaluation("state.items.map(|x| x + 1).len()", 500),
        0
    );
}

/// Rhai's size accounting cannot look inside a shared value that is nested in
/// data (it would panic), so the copies the retry hands to closures must be
/// unshared wherever a closure puts them.
#[test]
fn closures_that_return_or_embed_the_state_work_on_the_retry_path() {
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&state(), &Map::new(), &phone().to_map());
    let cases: &[(&str, i64)] = &[
        ("state.items.map(|x| state).len()", 5),
        ("state.items.map(|x| [state]).len()", 5),
        ("state.items.map(|x| #{a: state}).len()", 5),
        ("state.items.map(|x| state.items).len()", 5),
        ("state.items.map(|x| [state, state.n * x]).len()", 5),
        ("[state, state.items.map(|x| x * state.n)].len()", 2),
        ("state.items.map(|x| #{s: state, n: state.n})[0].n", 3),
        ("state.items.map(|x| state)[0].items.len()", 5),
    ];
    for (source, expected) in cases {
        assert_eq!(
            evaluate(&bindings, &mut scope, source),
            Ok(Value::Int(*expected)),
            "{source}"
        );
    }
    assert!(bindings.stats().state_copies > 0, "the retry path ran");
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
