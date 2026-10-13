//! The sandbox surface: which functions a binding can and cannot call.
//!
//! A binding must be unable to block (`sleep`), end the evaluation (`exit`),
//! parse input (`parse_json`), read a clock or the debugger's call stack. Those
//! live in Rhai packages the engine deliberately does not register.

mod common;

use common::{int, map, phone};
use lazyrad_view::{BindError, BindingScope, Bindings, Value};
use rhai::Map;

/// The number of native functions the registered packages provide, as
/// `Bindings::registered_functions` reports it.
///
/// This is an allow-list by count: the names are not enumerable without Rhai's
/// `metadata` feature, which this crate does not enable. A change here means
/// either the package list in `bindings.rs` changed or the pinned Rhai did;
/// either way somebody must read what was added and decide it is safe.
const PINNED_FUNCTION_COUNT: usize = 703;

fn run(source: &str) -> Result<Value, BindError> {
    let state = map(&[("n", int(2))]);
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&state, &Map::new(), &phone().to_map());
    bindings.evaluate(source, &mut scope, None)
}

#[test]
fn sleep_exit_and_parse_json_are_not_callable() {
    for source in [
        // `sleep` blocks the thread: one call is one operation, so the
        // operation budget could never have stopped `sleep(3600)`.
        "sleep(1)",
        "sleep(0.5)",
        "sleep(3600)",
        "sleep(state.n)",
        // `exit` unwinds the whole evaluation with a value.
        "exit()",
        "exit(1)",
        // Parsing is a way to build maps out of text.
        "parse_json(\"{}\")",
        // Reflection and debugging.
        "get_fn_metadata_list()",
        "back_trace()",
        // A clock, a file, a value taken out of its owner.
        "timestamp()",
        "open(\"secret.txt\")",
        "take(state.n)",
        "(1).tag",
    ] {
        let result = run(source);
        match &result {
            Err(BindError::Runtime { message, .. }) => assert!(
                message.contains("not found") || message.contains("Unknown property"),
                "{source}: unexpected message {message}"
            ),
            Err(BindError::Syntax { .. }) => {}
            other => panic!("{source} must not be callable, got {other:?}"),
        }
    }
}

#[test]
fn every_function_a_binding_needs_is_still_there() {
    let cases: &[(&str, Value)] = &[
        // Strings and interpolation.
        ("\"abc\".to_upper()", Value::Text("ABC".into())),
        ("\"abc\".sub_string(1)", Value::Text("bc".into())),
        ("\"abc\".contains(\"b\")", Value::Bool(true)),
        ("`n=${state.n}`", Value::Text("n=2".into())),
        ("state.n.to_string()", Value::Text("2".into())),
        // Numbers.
        ("state.n.to_float() / 4.0", Value::Float(0.5)),
        ("max(state.n, 5)", Value::Int(5)),
        ("(7.5).floor()", Value::Float(7.0)),
        ("state.n - 5", Value::Int(-3)),
        ("(-5).abs()", Value::Int(5)),
        // Collections and closures.
        ("[1, 2, 3].map(|x| x * 2).len()", Value::Int(3)),
        ("[1, 2, 3].filter(|x| x > 1).len()", Value::Int(2)),
        ("[3, 1, 2].some(|x| x == 1)", Value::Bool(true)),
        ("#{a: 1, b: 2}.len()", Value::Int(2)),
        ("#{a: 1}.contains(\"a\")", Value::Bool(true)),
        // Function pointers and ranges.
        ("Fn(\"abs\").call(-4)", Value::Int(4)),
        ("(0..10).contains(3)", Value::Bool(true)),
        (
            "switch state.n { 2 => \"two\", _ => \"many\" }",
            Value::Text("two".into()),
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(run(source).as_ref(), Ok(expected), "{source}");
    }
}

#[test]
fn the_registered_function_set_is_pinned() {
    assert_eq!(
        Bindings::new().registered_functions(),
        PINNED_FUNCTION_COUNT,
        "the set of native functions a binding can call changed: review the \
         packages in bindings.rs (and the pinned Rhai), then update the pin"
    );
}
