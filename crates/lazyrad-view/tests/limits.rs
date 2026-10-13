//! Resource limits: native stack, operations and memory.
//!
//! The limits in `bindings.rs` were chosen by measurement on a debug build (the
//! worst case for Rhai's native recursion); these tests keep them honest.

mod common;

use std::mem::size_of;

use common::{int, map, phone, text};
use lazyrad_view::bindings::{
    MAX_ARRAY_SIZE, MAX_CALL_LEVELS, MAX_MAP_SIZE, MAX_OPERATIONS, MAX_STRING_SIZE,
    PEAK_MEMORY_CEILING,
};
use lazyrad_view::{BindError, BindingScope, Bindings, Value};
use rhai::{Dynamic, Map};

fn evaluate(source: &str, state: &Map) -> Result<Value, BindError> {
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(state, &Map::new(), &phone().to_map());
    bindings.evaluate(source, &mut scope, None)
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
    let half = "x".repeat(MAX_STRING_SIZE / 2 + 1);
    let state = map(&[("half", text(&half))]);
    // 250 outer iterations over 250 inner ones is about 250 000 operations.
    let lit = vec!["0"; 250].join(",");
    let nested = format!("[{lit}].map(|x| [{lit}].map(|y| y).len()).len()");
    let runaways = [
        nested.as_str(),
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
        evaluate("[1, 2, 3].map(|x| x + state.half.len()).len()", &state),
        Ok(Value::Int(3))
    );
}

#[test]
fn the_operation_budget_is_what_the_docs_say() {
    // The docs say: a literal element costs one operation and a `map(|y| 0)`
    // step three per element. Nested, n outer iterations of an n-element
    // inner step cost about n + n * (n + 3n) = n + 4n^2.
    const { assert!(MAX_OPERATIONS == 32_768, "re-derive n below") };
    let nested = |n: usize| {
        let lit = vec!["0"; n].join(",");
        format!("[{lit}].map(|x| [{lit}].map(|y| 0).len()).len()")
    };
    // 85 -> about 28 985 operations, 95 -> about 36 195.
    assert_eq!(evaluate(&nested(85), &Map::new()), Ok(Value::Int(85)));
    assert!(matches!(
        evaluate(&nested(95), &Map::new()),
        Err(BindError::Limit { .. })
    ));
    // A pipeline of eight five-operation steps over a 500-element list fits:
    // 500 + 8 * 5 * 500 = 20 500 operations.
    let lit = vec!["0"; 500].join(",");
    let pipeline = format!("[{lit}]{}.len()", ".map(|x| x + 1)".repeat(8));
    assert_eq!(evaluate(&pipeline, &Map::new()), Ok(Value::Int(500)));
}

#[test]
fn the_documented_nesting_caps_are_the_real_ones() {
    let bindings = Bindings::new();
    let check = |source: String| bindings.check_expression(&source).is_ok();
    let terms = |n: usize| {
        (0..n)
            .map(|i| format!("state.a{i}"))
            .collect::<Vec<_>>()
            .join(" + ")
    };
    assert!(
        check(terms(16)) && !check(terms(17)),
        "16 terms in a flat chain"
    );
    let ands = |n: usize| vec!["true"; n].join(" && ");
    assert!(check(ands(16)) && !check(ands(17)), "16 terms joined by &&");
    let else_ifs = |n: usize| {
        let mut source = String::from("if state.n == 0 { 0 }");
        for i in 1..n {
            source.push_str(&format!(" else if state.n == {i} {{ {i} }}"));
        }
        source.push_str(" else { 99 }");
        source
    };
    assert!(
        check(else_ifs(12)) && !check(else_ifs(13)),
        "12 branches of else if"
    );
    let calls = |n: usize| format!("[1, 2, 3]{}", ".map(|x| x)".repeat(n));
    assert!(
        check(calls(10)) && !check(calls(11)),
        "10 chained method calls"
    );
}

#[test]
fn the_limits_bound_the_transient_memory_of_one_native_call() {
    // One native call iterates an array of at most MAX_ARRAY_SIZE elements and
    // keeps one result per iteration. The costliest result is many small maps:
    // a map with one entry allocates a node of about 540 bytes (measured), and
    // at most this many fit the limits at once: one per array element (a list
    // of one-field rows) plus one per two entries of the rest (a map of
    // one-field maps).
    let nested_maps = u64::try_from(MAX_ARRAY_SIZE + (MAX_MAP_SIZE - MAX_ARRAY_SIZE) / 2).unwrap();
    let map_bytes = nested_maps * 540;
    let array_bytes = u64::try_from(MAX_ARRAY_SIZE * size_of::<Dynamic>()).unwrap();
    let text_bytes = u64::try_from(MAX_STRING_SIZE).unwrap();
    let one_copy = map_bytes + array_bytes + text_bytes;
    let call = one_copy * u64::try_from(MAX_ARRAY_SIZE).unwrap();
    assert!(
        call <= PEAK_MEMORY_CEILING,
        "{} MiB exceeds the {} MiB ceiling",
        call >> 20,
        PEAK_MEMORY_CEILING >> 20
    );
    // Measured on the debug build with the worst shape (510 one-field rows
    // returned on each of 511 iterations, plus a 31 KiB text built on each):
    // 154 MiB peak working set, 146 MiB over an idle process. The estimate
    // above is the same shape with the budget filled exactly, so it sits just
    // above the measurement.
    const MEASURED_PEAK: u64 = 154 * 1024 * 1024;
    assert!(
        (MEASURED_PEAK - 8 * 1024 * 1024..=MEASURED_PEAK + 16 * 1024 * 1024).contains(&call),
        "the estimate tracks the measurement ({} MiB)",
        call >> 20
    );
    const { assert!(MEASURED_PEAK <= PEAK_MEMORY_CEILING) };
}
