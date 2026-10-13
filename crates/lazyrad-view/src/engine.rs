//! Building the restricted Rhai engines a [`Bindings`](crate::Bindings) owns.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rhai::packages::{
    ArithmeticPackage, BasicArrayPackage, BasicFnPackage, BasicIteratorPackage, BasicMapPackage,
    BasicMathPackage, BasicStringPackage, LogicPackage, MoreStringPackage, Package,
};
use rhai::{Dynamic, Engine, EvalAltResult, Map, OptimizationLevel, Position};

use crate::limits::{
    MAX_ARRAY_SIZE, MAX_CALL_LEVELS, MAX_EXPR_DEPTHS, MAX_MAP_SIZE, MAX_OPERATIONS, MAX_STRING_SIZE,
};
use crate::poison::{HIDDEN_TYPE_NAME, Poison, find_poison_in};
use crate::visible::deep_copy;

/// What the evaluator and its engines count into.
#[derive(Clone, Default)]
pub(crate) struct Counters {
    /// Operations spent by the evaluation in progress; the progress hook of
    /// each engine counts into it and the evaluator resets it.
    pub budget: Rc<Cell<u64>>,
    /// Deep copies of `state`, `props` or `env` handed to closures by the
    /// private-copies resolver.
    pub copies: Rc<Cell<u64>>,
    /// The refusal for the first hidden entry the evaluation in progress
    /// turned into text. Rhai swallows an error raised by a text conversion
    /// (and uses the type's name instead), so a conversion records it here and
    /// the evaluator fails the evaluation with it afterwards, however the
    /// text was used.
    pub refused: Rc<RefCell<Option<String>>>,
}

/// An error that carries `message` to the diagnostic unchanged.
fn refusal(message: String) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(Dynamic::from(message), Position::NONE).into()
}

/// Gives a [`Poison`] the operators and conversions Rhai would otherwise
/// answer silently (`==` is `false` between unrelated types; a text
/// conversion falls back to the type name).
fn register_poison(engine: &mut Engine, refused: &Rc<RefCell<Option<String>>>) {
    engine.register_type_with_name::<Poison>(HIDDEN_TYPE_NAME);
    // Rhai answers a comparison between unrelated types with a constant
    // (`false`, or `true` for `!=`) instead of an error.
    for operator in ["==", "!=", "<", "<=", ">", ">="] {
        engine.register_fn(
            operator,
            |p: Poison, _: Dynamic| -> Result<bool, Box<EvalAltResult>> {
                Err(refusal(p.refusal("compared")))
            },
        );
        engine.register_fn(
            operator,
            |_: Dynamic, p: Poison| -> Result<bool, Box<EvalAltResult>> {
                Err(refusal(p.refusal("compared")))
            },
        );
    }
    // Every text conversion (`to_string`, `to_debug`, `${..}`, `+` with a
    // text, a list or map that holds one) goes through these two.
    for name in ["to_string", "to_debug"] {
        let refused = Rc::clone(refused);
        engine.register_fn(name, move |p: Poison| {
            refused
                .borrow_mut()
                .get_or_insert_with(|| p.refusal("converted to text"));
            p.text()
        });
    }
    // `to_json` formats values itself and would print the Rust type's name.
    // It takes the map by value, so it is callable on a constant, like the
    // function it replaces.
    engine.register_fn(
        "to_json",
        |map: Map| -> Result<String, Box<EvalAltResult>> {
            match find_poison_in(&map, 0) {
                Some(p) => Err(refusal(p.refusal("converted to text"))),
                None => Ok(rhai::format_map_as_json(&map)),
            }
        },
    );
}

/// Builds one restricted engine and returns it with the number of native
/// functions its packages registered.
///
/// `private_copies` adds the resolver that answers a top-level `state`,
/// `props` or `env` with a private, read-only, shared copy, which is what lets
/// a closure capture one without the receiver's borrow racing it.
pub(crate) fn build_engine(counters: &Counters, private_copies: bool) -> (Engine, usize) {
    let mut engine = Engine::new_raw();
    let mut functions = 0_usize;
    // The whole sandbox surface, package by package. Adding a name here is a
    // security decision; `registered_functions` pins the result in a test.
    macro_rules! install {
        ($($package:ty),+ $(,)?) => {$(
            let module = <$package>::new().as_shared_module();
            functions += module.count().1;
            engine.register_global_module(module);
        )+};
    }
    install!(
        ArithmeticPackage,
        BasicStringPackage,
        BasicIteratorPackage,
        BasicFnPackage,
        LogicPackage,
        BasicMathPackage,
        MoreStringPackage,
        BasicArrayPackage,
        BasicMapPackage,
    );
    engine.disable_symbol("eval");
    engine.disable_symbol("print");
    engine.disable_symbol("debug");
    // `type_of` would name a hidden entry's type as an ordinary value.
    engine.disable_symbol("type_of");
    engine.set_optimization_level(OptimizationLevel::None);
    engine.set_strict_variables(true);
    engine.set_fail_on_invalid_map_property(true);
    engine.set_max_operations(MAX_OPERATIONS);
    let spent = Rc::clone(&counters.budget);
    engine.on_progress(move |_| {
        let used = spent.get().saturating_add(1);
        spent.set(used);
        (used > MAX_OPERATIONS).then(|| Dynamic::from("operation limit"))
    });
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_expr_depths(MAX_EXPR_DEPTHS.0, MAX_EXPR_DEPTHS.1);
    engine.set_max_string_size(MAX_STRING_SIZE);
    engine.set_max_array_size(MAX_ARRAY_SIZE);
    engine.set_max_map_size(MAX_MAP_SIZE);
    register_poison(&mut engine, &counters.refused);
    if private_copies {
        // At the top level the resolver answers with a private copy; Rhai then
        // does not share the variable itself when a closure captures it, and
        // the closure curries the copy. The copy is a *shared* value, so
        // handing it to a closure is a handle clone, not a copy of the state
        // per call, and it is read-only, so a method that would mutate it is
        // refused. Each reference resolves to a copy of its own, so the
        // receiver of `state.items.map(...)` and the `state` the closure
        // captured are different cells and cannot race. Inside a function the
        // name is the closure's own parameter, so the normal lookup is left
        // alone.
        let copies = Rc::clone(&counters.copies);
        #[allow(deprecated)] // `on_var` is flagged volatile, not deprecated.
        engine.on_var(move |name, _index, context| {
            if context.call_level() > 0 || !matches!(name, "state" | "props" | "env") {
                return Ok(None);
            }
            Ok(context.scope().get(name).map(|value| {
                copies.set(copies.get() + 1);
                deep_copy(value, 0).into_read_only().into_shared()
            }))
        });
    }
    (engine, functions)
}
