//! Resource limits: native stack, operations and memory.
//!
//! The limits in `bindings.rs` were chosen by measurement on a debug build (the
//! worst case for Rhai's native recursion); these tests keep them honest.

mod common;

use std::mem::size_of;

use common::{int, map, phone, text};
use lazyrad_view::bindings::{
    MAX_ARRAY_SIZE, MAX_CALL_LEVELS, MAX_MAP_SIZE, MAX_OPERATIONS, MAX_STRING_SIZE,
    PEAK_MEMORY_CEILING, VISIBLE_BUDGET,
};
use lazyrad_view::{BindError, BindingScope, Bindings, Value};
use rhai::{Dynamic, Map};

fn evaluate(source: &str, state: &Map) -> Result<Value, BindError> {
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(state, &Map::new(), &phone().to_map());
    bindings.evaluate(source, &mut scope, None)
}

fn limit_message(result: Result<Value, BindError>) -> String {
    match result {
        Err(BindError::Limit { message, .. }) => message,
        other => panic!("expected a limit error, got {other:?}"),
    }
}

/// Runs `body` on a thread with the stack of a default Windows main thread,
/// 1 MiB, in whatever profile the tests were built with (debug: the worst).
fn on_one_mebibyte_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(body)
        .expect("spawn")
        .join()
        .expect("the evaluation must end in a diagnostic, not a panic");
}

/// Nests `inner` in `depth` levels of one syntactic construct. Each construct
/// has a different native stack cost per level in Rhai's evaluator.
fn nest(kind: &str, depth: usize, inner: &str) -> String {
    let mut text = inner.to_owned();
    for _ in 0..depth {
        text = match kind {
            "arith" => format!("(0 + ({text}))"),
            "call" => format!("max(0, {text})"),
            "array" => format!("[{text}][0]"),
            "if" => format!("(if true {{ {text} }} else {{ 0 }})"),
            "interp" => format!("parse_int(`${{{text}}}`)"),
            "switch" => format!("(switch 1 {{ 1 => {text}, _ => 0 }})"),
            "map" => format!("#{{a: {text}}}.a"),
            "unary" => format!("(-(-({text})))"),
            "closure" => format!("(|a| {text}).call(1)"),
            "callback" => format!("[1].map(|x| {text})[0]"),
            "elseif" => format!("(if false {{ 0 }} else if false {{ 0 }} else {{ {text} }})"),
            other => panic!("unknown construct {other}"),
        };
    }
    text
}

const CONSTRUCTS: [&str; 11] = [
    "arith", "call", "array", "if", "interp", "switch", "map", "unary", "closure", "callback",
    "elseif",
];

#[test]
fn recursion_bombs_end_in_a_diagnostic_on_a_one_mebibyte_stack() {
    on_one_mebibyte_stack(|| {
        let state = map(&[("n", int(2))]);
        let bindings = Bindings::new();
        let mut scope = BindingScope::new(&state, &Map::new(), &phone().to_map());

        // The two shapes found by review, which aborted the process when the
        // call-depth limit was 16.
        let body = "|f, n| if n == 0 { 0 } else { f.call(f, n - 1) + 1 }";
        let bomb = format!("({body}).call({body}, 20)");
        let nested_bomb = format!("1 + (2 * (3 + ({body}).call({body}, 20)))");
        let result = bindings.evaluate(&bomb, &mut scope, None);
        assert!(
            matches!(result, Err(BindError::Limit { .. })),
            "{bomb}: {result:?}"
        );
        // Wrapped in arithmetic the bomb no longer fits the expression depth
        // limit, which is as good a refusal: a diagnostic from the parser.
        let result = bindings.evaluate(&nested_bomb, &mut scope, None);
        assert!(
            matches!(
                result,
                Err(BindError::Limit { .. } | BindError::Syntax { .. })
            ),
            "{nested_bomb}: {result:?}"
        );

        for kind in CONSTRUCTS {
            // The deepest nesting the parser accepts for this construct.
            let mut deepest = 0;
            for depth in 1..=64 {
                if bindings
                    .check_expression(&nest(kind, depth, "state.n"))
                    .is_err()
                {
                    break;
                }
                deepest = depth;
            }
            // One more level is refused with a diagnostic, not a stack overflow
            // in the parser.
            let too_deep = nest(kind, deepest + 1, "state.n");
            assert!(
                matches!(
                    bindings.check_expression(&too_deep),
                    Err(BindError::Syntax { .. })
                ),
                "{kind}: {deepest}"
            );
            // The deepest accepted nesting evaluates (or fails cleanly).
            let deepest_source = nest(kind, deepest, "state.n");
            let _ = bindings.evaluate(&deepest_source, &mut scope, None);

            // A recursion bomb whose body is that construct nested as deep as
            // the parser allows, so each call level costs as much stack as it
            // can.
            let mut body_depth = 0;
            for depth in 0..=64 {
                let body = format!(
                    "|f, n| if n == 0 {{ 0 }} else {{ {} }}",
                    nest(kind, depth, "f.call(f, n - 1)")
                );
                if bindings
                    .check_expression(&format!("({body}).call({body}, 1000)"))
                    .is_err()
                {
                    break;
                }
                body_depth = depth;
            }
            let body = format!(
                "|f, n| if n == 0 {{ 0 }} else {{ {} }}",
                nest(kind, body_depth, "f.call(f, n - 1)")
            );
            let result =
                bindings.evaluate(&format!("({body}).call({body}, 1000)"), &mut scope, None);
            assert!(result.is_err(), "{kind}: {result:?}");
        }
    });
}

#[test]
fn recursion_within_the_call_limit_still_works() {
    let body = "|f, n| if n == 0 { 0 } else { f.call(f, n - 1) + 1 }";
    let levels = MAX_CALL_LEVELS - 2;
    let source = format!("({body}).call({body}, {levels})");
    assert_eq!(
        evaluate(&source, &Map::new()),
        Ok(Value::Int(i64::try_from(levels).unwrap()))
    );
}

#[test]
fn a_runaway_expression_ends_at_a_limit_without_hanging() {
    // 1000 items at three closures each is about 15 000 operations, past the
    // budget, though the list itself is within the size limits.
    let items: Vec<Dynamic> = (0..1000).map(int).collect();
    let half = "x".repeat(MAX_STRING_SIZE / 2 + 1);
    let state = map(&[("items", Dynamic::from_array(items)), ("half", text(&half))]);
    let runaways = [
        "state.items.map(|x| x + 1).map(|x| x + 1).map(|x| x + 1).len()",
        "(|f| f.call(f)).call(|f| f.call(f))",
        "state.half + state.half",
    ];
    for source in runaways {
        let result = evaluate(source, &state);
        assert!(
            matches!(result, Err(BindError::Limit { .. })),
            "{source}: expected a limit error, got {result:?}"
        );
    }
    // And the engine is fine afterwards, with a fresh budget per evaluation.
    assert_eq!(
        evaluate("[1, 2, 3].map(|x| x + state.items.len()).len()", &state),
        Ok(Value::Int(3))
    );
}

#[test]
fn the_cheapest_closure_call_costs_three_operations() {
    // The memory bound below divides the budget by this number, so measure it
    // rather than trust it. A 1000-item literal costs about 1000 operations,
    // and each `map(|x| 0)` over it three per item: three maps fit the budget
    // (about 10 000), four do not (about 13 000). Fewer than three operations
    // per item would let four fit; more would refuse three.
    const { assert!(MAX_OPERATIONS == 12_000, "re-derive the counts below") };
    let literal = vec!["0"; 1000].join(",");
    let chain = |maps: usize| format!("[{literal}]{}.len()", ".map(|x| 0)".repeat(maps));
    assert_eq!(evaluate(&chain(3), &Map::new()), Ok(Value::Int(1000)));
    assert!(matches!(
        evaluate(&chain(4), &Map::new()),
        Err(BindError::Limit { .. })
    ));
}

#[test]
fn the_limits_bound_the_transient_memory_of_one_native_call() {
    // One native call iterates an array of at most MAX_ARRAY_SIZE elements
    // (literal: parser cap; built: size check at return; state: visible
    // budget), and keeps one result per iteration. A result copies at most
    // VISIBLE_BUDGET elements or entries, or MAX_STRING_SIZE bytes.
    let iterations = u64::try_from(MAX_ARRAY_SIZE).unwrap();
    let visible = u64::try_from(VISIBLE_BUDGET).unwrap();
    let array_bytes = iterations * visible * u64::try_from(size_of::<Dynamic>()).unwrap();
    // A map entry: a 24-byte key, a value and the tree's own overhead.
    let map_bytes = iterations * visible * 64;
    let string_bytes = iterations * u64::try_from(MAX_STRING_SIZE).unwrap();
    for (what, bytes) in [
        ("arrays", array_bytes),
        ("maps", map_bytes),
        ("strings", string_bytes),
    ] {
        assert!(
            bytes <= PEAK_MEMORY_CEILING,
            "{what}: {} MiB exceeds the {} MiB ceiling",
            bytes >> 20,
            PEAK_MEMORY_CEILING >> 20
        );
    }
    // The map limit is what a built map may hold; it must not exceed what a
    // copy of state may.
    const { assert!(MAX_MAP_SIZE <= VISIBLE_BUDGET) };
}

#[test]
fn a_large_state_list_is_hidden_so_a_closure_cannot_copy_it_over_and_over() {
    // The review's scenario: a literal of 400 items, each iteration returning
    // a 100 000-element state list. It used to allocate 640 MB before the size
    // check at the end of `map` refused the result.
    let items: Vec<Dynamic> = (0..100_000).map(int).collect();
    let state = map(&[("items", Dynamic::from_array(items)), ("n", int(7))]);
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&state, &Map::new(), &phone().to_map());
    assert_eq!(
        scope.hidden(),
        [format!("state.items (over {VISIBLE_BUDGET} elements)")]
    );

    // Nothing is copied, so the expression is cheap and completes; the
    // placeholder is what each iteration returns.
    let source = format!("[{}].map(|x| state.items).len()", vec!["0"; 400].join(","));
    assert_eq!(
        bindings.evaluate(&source, &mut scope, None),
        Ok(Value::Int(400))
    );

    // Any use of the hidden list is a diagnostic that names it.
    for source in [
        "state.items.len()",
        "state.items",
        "state.items[0]",
        "state.items.map(|x| x).len()",
    ] {
        let Err(BindError::Runtime { message, .. }) = bindings.evaluate(source, &mut scope, None)
        else {
            panic!("{source} must be refused");
        };
        assert!(
            message.contains("state.items") && message.contains("too large"),
            "{source}: {message}"
        );
    }
    // The rest of the state is unaffected.
    assert_eq!(
        bindings.evaluate("state.n * 2", &mut scope, None),
        Ok(Value::Int(14))
    );
}

#[test]
fn the_worst_visible_case_is_refused_after_at_most_the_ceiling() {
    // The worst case the limits permit: every iteration copies a list of
    // VISIBLE_BUDGET - 1 elements, 1000 times. About 16 MiB are alive when
    // `map` returns, and the size check on its result then refuses it.
    let size = VISIBLE_BUDGET - 1;
    let list: Vec<Dynamic> = (0..size as i64).map(int).collect();
    let state = map(&[("big", Dynamic::from_array(list))]);
    let source = format!("[{}].map(|x| state.big).len()", vec!["0"; 1000].join(","));
    let message = limit_message(evaluate(&source, &state));
    assert!(message.contains("array"), "{message}");
    // The same shape with a small list completes.
    let state = map(&[("small", Dynamic::from_array(vec![int(1), int(2)]))]);
    let source = format!("[{}].map(|x| state.small).len()", vec!["0"; 300].join(","));
    assert_eq!(evaluate(&source, &state), Ok(Value::Int(300)));
}

#[test]
fn two_lists_that_fit_alone_but_not_together_hide_the_larger_one() {
    let make = |n: i64| Dynamic::from_array((0..n).map(int).collect());
    let state = map(&[("a", make(600)), ("b", make(700)), ("n", int(1))]);
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&state, &Map::new(), &phone().to_map());
    assert_eq!(scope.hidden(), ["state.b (700 elements)"]);
    assert_eq!(
        bindings.evaluate("state.a.len()", &mut scope, None),
        Ok(Value::Int(600))
    );
    assert!(
        bindings
            .evaluate("state.b.len()", &mut scope, None)
            .is_err()
    );
}

#[test]
fn a_string_over_the_limit_is_readable_but_not_a_method_receiver() {
    // Strings are immutable and shared, so a long one in state costs nothing
    // to hand around; Rhai only refuses to call a method on it.
    let long = "x".repeat(MAX_STRING_SIZE + 1);
    let state = map(&[("doc", text(&long))]);
    assert_eq!(evaluate("state.doc", &state), Ok(Value::Text(long.clone())));
    let message = limit_message(evaluate("state.doc.len()", &state));
    assert!(message.contains("string"), "{message}");
}
