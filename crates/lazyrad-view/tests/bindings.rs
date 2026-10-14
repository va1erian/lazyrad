//! The binding evaluator: what an expression can and cannot do.

mod common;

use common::{int, map, phone, text};
use lazyrad_view::{
    BindError, BindingScope, Bindings, Dependencies, Environment, Value, ValueError, ValueKind,
    check_expression,
};
use rhai::{Dynamic, Map};

/// Evaluates `source` against `state` on a phone, with empty props.
fn eval(source: &str, state: &Map) -> Result<Value, BindError> {
    eval_as(source, state, None)
}

fn eval_as(source: &str, state: &Map, kind: Option<ValueKind>) -> Result<Value, BindError> {
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(state, &Map::new(), &phone().to_map());
    bindings.evaluate(source, &mut scope, kind)
}

#[test]
fn interpolated_strings_work() {
    let state = map(&[("name", text("Ada")), ("count", int(3))]);
    assert_eq!(
        eval("`Hello, ${state.name}!`", &state),
        Ok(Value::Text("Hello, Ada!".into()))
    );
    // Non-string parts are converted.
    assert_eq!(
        eval("`Clicked ${state.count} times`", &state),
        Ok(Value::Text("Clicked 3 times".into()))
    );
}

#[test]
fn an_empty_state_is_fine_for_expressions_that_do_not_read_it() {
    let empty = Map::new();
    assert_eq!(eval("1 + 2", &empty), Ok(Value::Int(3)));
    assert_eq!(eval("\"a\" + \"b\"", &empty), Ok(Value::Text("ab".into())));
    assert_eq!(
        eval("env.size_class == \"compact\"", &empty),
        Ok(Value::Bool(true))
    );
}

#[test]
fn a_missing_key_is_a_clear_diagnostic_not_a_panic() {
    let empty = Map::new();
    for source in ["state.missing", "state.user.name", "state[\"missing\"]"] {
        let error = eval(source, &empty).unwrap_err();
        let BindError::Runtime { message, .. } = &error else {
            panic!("{source}: expected a runtime error, got {error:?}");
        };
        assert!(
            message.contains("missing") || message.contains("user"),
            "{message}"
        );
    }
    // Reading `props` outside a component is the same, not a crash.
    assert!(eval("props.title", &empty).is_err());
}

#[test]
fn an_expression_yielding_unit_is_an_error() {
    let state = map(&[("flag", Dynamic::from(false))]);
    // An `if` with no `else` is `()` when the condition is false.
    assert_eq!(
        eval("if state.flag { 1 }", &state),
        Err(BindError::Value(ValueError::Unit))
    );
    assert_eq!(
        eval("()", &Map::new()),
        Err(BindError::Value(ValueError::Unit))
    );
}

#[test]
fn unsupported_result_types_are_errors() {
    let state = map(&[("items", Dynamic::from_array(vec![int(1), int(2)]))]);
    for source in ["state.items", "state", "[1, 2]", "#{a: 1}", "'x'"] {
        assert!(
            matches!(
                eval(source, &state),
                Err(BindError::Value(ValueError::Unsupported { .. }))
            ),
            "{source}"
        );
    }
}

#[test]
fn integers_and_floats_are_not_silently_coerced() {
    let empty = Map::new();
    assert_eq!(eval("1", &empty), Ok(Value::Int(1)));
    assert_eq!(eval("1.0", &empty), Ok(Value::Float(1.0)));
    let mismatch = |expected, found| {
        Err(BindError::Value(ValueError::KindMismatch {
            expected,
            found,
        }))
    };
    assert_eq!(
        eval_as("1.0", &empty, Some(ValueKind::Int)),
        mismatch(ValueKind::Int, ValueKind::Float)
    );
    assert_eq!(
        eval_as("1", &empty, Some(ValueKind::Float)),
        mismatch(ValueKind::Float, ValueKind::Int)
    );
    // The explicit conversion is the way across.
    assert_eq!(
        eval_as("1.to_float()", &empty, Some(ValueKind::Float)),
        Ok(Value::Float(1.0))
    );
    // A string reads as an enum for an enum property, and for nothing else.
    assert_eq!(
        eval_as("\"Start\"", &empty, Some(ValueKind::Enum)),
        Ok(Value::variant("Start"))
    );
    assert_eq!(
        eval_as("\"Start\"", &empty, Some(ValueKind::Int)),
        mismatch(ValueKind::Int, ValueKind::Text)
    );
}

#[test]
fn statements_and_assignments_are_rejected_at_compile_time() {
    let sources = [
        "let x = 1",
        "let x = 1; x",
        "state.count = 1",
        "state.count += 1",
        "state = #{}",
        "1; 2",
        "while true { }",
        "loop { break; }",
        "for i in 0..3 { }",
        "fn f() { 1 }",
        "import \"x\" as y; 1",
        "const C = 1; C",
    ];
    let bindings = Bindings::new();
    for source in sources {
        assert!(
            matches!(
                bindings.check_expression(source),
                Err(BindError::Syntax { .. })
            ),
            "{source:?} must not compile"
        );
        let mut scope = BindingScope::new(&Map::new(), &Map::new(), &phone().to_map());
        assert!(
            matches!(
                bindings.evaluate(source, &mut scope, None),
                Err(BindError::Syntax { .. })
            ),
            "{source:?} must not evaluate"
        );
    }
}

#[test]
fn a_syntax_error_carries_its_position() {
    let error = check_expression("state.count +* 2").unwrap_err();
    let BindError::Syntax { position, .. } = error else {
        panic!("expected a syntax error");
    };
    let position = position.expect("the parser reports a position");
    assert_eq!(position.line, 1);
    assert!(position.column.is_some());
    // The message names the place, for a lint or a diagnostic line.
    let shown = check_expression("(1 +").unwrap_err().to_string();
    assert!(shown.contains("line 1"), "{shown}");
}

#[test]
fn only_state_props_and_env_are_in_scope() {
    // A control, the form, a script's top-level variable: none exist here.
    for source in ["name_edit.text", "form.title", "score + 1", "stat.name"] {
        let error = check_expression(source).unwrap_err();
        assert!(
            matches!(error, BindError::Syntax { .. }),
            "{source}: {error:?}"
        );
        assert!(
            error.to_string().contains("not found") || error.to_string().contains("ndefined"),
            "{source}: {error}"
        );
    }
    assert!(check_expression("state.x + props.y + env.width").is_ok());
}

#[test]
fn eval_print_and_the_file_system_are_unreachable() {
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&Map::new(), &Map::new(), &phone().to_map());
    let mut run = |source: &str| bindings.evaluate(source, &mut scope, None);
    for source in [
        "eval(\"1 + 1\")",
        "print(\"hello\")",
        "debug(1)",
        // `type_of` would name the type that stands in for a hidden entry.
        "type_of(1)",
        "(1).type_of()",
        "Fn(\"type_of\").call(1)",
        "open(\"secret.txt\")",
        "read_file(\"secret.txt\")",
        "timestamp()",
        "import \"fs\" as fs; 1",
        "Fn(\"eval\").call(\"1 + 1\")",
    ] {
        let result = run(source);
        assert!(result.is_err(), "{source} must fail, got {result:?}");
    }
    // Calling `print` through a function pointer is not a way around the
    // disabled symbol either.
    assert!(run("Fn(\"print\").call(\"x\")").is_err());
    // What a binding legitimately has still works on the same engine.
    assert_eq!(run("\"a\".to_upper()"), Ok(Value::Text("A".into())));
}

#[test]
fn bindings_cannot_mutate_the_state_they_read() {
    let state = map(&[("items", Dynamic::from_array(vec![int(1), int(2), int(3)]))]);
    let bindings = Bindings::new();
    // One scope for the whole turn, as a session uses it.
    let mut scope = BindingScope::new(&state, &Map::new(), &phone().to_map());
    for source in [
        "state.items.push(4)",
        "state.items.clear()",
        "state.items.pop()",
    ] {
        let result = bindings.evaluate(source, &mut scope, None);
        assert!(result.is_err(), "{source} must not run, got {result:?}");
    }
    assert_eq!(
        bindings.evaluate("state.items.len()", &mut scope, None),
        Ok(Value::Int(3))
    );
    // The caller's map is untouched too.
    assert_eq!(state["items"].clone().into_array().unwrap().len(), 3);
}

#[test]
fn a_binding_reads_props_and_env() {
    let props = map(&[("title", text("Users"))]);
    let env = Environment {
        width: 700.0,
        height: 500.0,
        dark: true,
        text_scale: 1.25,
        reduce_motion: true,
    };
    let bindings = Bindings::new();
    let mut scope = BindingScope::new(&Map::new(), &props, &env.to_map());
    let mut run = |source: &str| bindings.evaluate(source, &mut scope, None);
    assert_eq!(run("props.title"), Ok(Value::Text("Users".into())));
    assert_eq!(run("env.width"), Ok(Value::Float(700.0)));
    assert_eq!(run("env.width >= 600"), Ok(Value::Bool(true)));
    assert_eq!(run("env.size_class"), Ok(Value::Text("medium".into())));
    assert_eq!(run("env.dark"), Ok(Value::Bool(true)));
    assert_eq!(run("env.text_scale"), Ok(Value::Float(1.25)));
    assert_eq!(run("env.reduce_motion"), Ok(Value::Bool(true)));
}

#[test]
fn a_source_is_compiled_once_and_a_compile_error_is_cached_too() {
    let bindings = Bindings::new();
    let state = map(&[("n", int(1))]);
    let mut scope = BindingScope::new(&state, &Map::new(), &phone().to_map());
    for _ in 0..5 {
        assert_eq!(
            bindings.evaluate("state.n + 1", &mut scope, None),
            Ok(Value::Int(2))
        );
    }
    let stats = bindings.stats();
    assert_eq!((stats.compilations, stats.evaluations), (1, 5));
    for _ in 0..3 {
        assert!(bindings.evaluate("state.n +", &mut scope, None).is_err());
    }
    let stats = bindings.stats();
    assert_eq!((stats.compilations, stats.evaluations), (2, 5));
}

#[test]
fn dependencies_are_scanned_conservatively() {
    let deps = |source| Dependencies::scan(source);
    assert_eq!(
        deps("state.count + 1"),
        Dependencies {
            state: true,
            props: false,
            env: false
        }
    );
    assert_eq!(
        deps("env.width > 600 && props.wide"),
        Dependencies {
            state: false,
            props: true,
            env: true
        }
    );
    assert_eq!(deps("1 + 2"), Dependencies::default());
    // A longer identifier is not a mention.
    assert_eq!(deps("environment + states"), Dependencies::default());
    // Interpolation and ranges still name the variable.
    assert!(deps("`w=${env.width}`").env);
    assert!(deps("0..env.width").env);
}

#[test]
fn hostile_sources_are_errors_not_overflows() {
    let bindings = Bindings::new();
    let nested = format!("{}1{}", "(".repeat(5_000), ")".repeat(5_000));
    let long_chain = vec!["1"; 20_000].join(" + ");
    let unbalanced = "(".repeat(1_000);
    let odd_characters = "\u{1F600}\u{0}\u{FFFF} state.n";
    for source in [&nested, &long_chain, &unbalanced, odd_characters] {
        // Whatever the verdict, it is a verdict and not a panic or a hang.
        let _ = bindings.check_expression(source);
        let mut scope = BindingScope::new(&Map::new(), &Map::new(), &phone().to_map());
        let _ = bindings.evaluate(source, &mut scope, None);
    }
    // Nesting beyond the parser's depth limit is a syntax error.
    assert!(matches!(
        bindings.check_expression(&nested),
        Err(BindError::Syntax { .. })
    ));
}
