//! The side-effect-free binding evaluator.
//!
//! A binding is a Rhai *expression* attached to a property. It sees exactly
//! three read-only variables and nothing else:
//!
//! | Variable | Type | Meaning                                                     |
//! |----------|------|-------------------------------------------------------------|
//! | `state`  | map  | the form's state                                            |
//! | `props`  | map  | the component's props (an empty map outside a component)    |
//! | `env`    | map  | the [`Environment`](crate::Environment), see [`Environment::to_map`](crate::Environment::to_map) |
//!
//! It cannot read a control, a script's top-level variable or anything else:
//! a name outside those three is a compile error, which keeps evaluation order
//! unobservable and rules out cycles.
//!
//! # The engine
//!
//! The engine is [`Engine::new_raw`] plus an explicit list of packages:
//! arithmetic, strings, iterators, function pointers, logic, math, arrays and
//! maps. **`CorePackage` is deliberately not used**: it contains
//! `LanguageCorePackage` (`sleep`, `exit`, `parse_json`, `take`, `tag`,
//! reflection), which a binding must not reach. There is no time package, no
//! I/O, no module resolver, and `print`, `debug` and `eval` are disabled
//! symbols. [`Bindings::registered_functions`] exposes the size of the
//! function set so a test can pin it.
//!
//! The source is compiled with [`Engine::compile_expression_with_scope`], so
//! statements, assignments and loops are compile errors, as is any name other
//! than the three variables. Optimisation is off: nothing is folded at compile
//! time (compilation always terminates) and the placeholder values the
//! variables are compiled against can never leak into a constant.
//!
//! # Limits
//!
//! Every limit ends an expression with a [`BindError::Limit`] (or, for
//! nesting, a syntax error), never a panic or an abort. They are sized for
//! *view* expressions, not data processing, and are documented at their
//! constants in [`crate::limits`]; in short:
//!
//! | What | Limit | In practice |
//! |---|---|---|
//! | operations in one evaluation | [`MAX_OPERATIONS`] = 32 768 | a `map`/`filter`/`reduce` step costs 3 to 5 per element, so about 12 steps over a full 512-element array (the 10-call chain cap comes first) |
//! | nested closure calls | [`MAX_CALL_LEVELS`] = 4 | |
//! | nesting depth | [`MAX_EXPR_DEPTHS`] = 16 and 10 | every operator in a flat chain is a level: at most 16 terms in `a + b + ...` or `a && b && ...`, 12 branches in an `else if` chain, 10 chained method calls |
//! | array elements in one value | [`MAX_ARRAY_SIZE`] = 512 | also the longest array literal, and the most iterations of one `map` |
//! | map entries in one value | [`MAX_MAP_SIZE`] = 512 | one per field, nested maps included: 102 rows of five fields, 51 of ten (the entry that holds the list costs one) |
//! | bytes of text in one value | [`MAX_STRING_SIZE`] = 32 KiB | texts inside arrays and maps are summed |
//!
//! The size limits apply to what a binding can *see* as well as to what it
//! builds, with Rhai's own counting (see below). Three consequences are worth
//! stating:
//!
//! * **A list of records is limited by its fields.** Rhai counts one map entry
//!   per field, so a list of five-field rows holds at most 102 rows, and a
//!   list over the limit is hidden from bindings. Keep counts and summaries in
//!   `state`, and long lists in a control that owns its rows.
//! * **A long string can be read but not used.** `bind text: state.doc` works
//!   at any length; `state.doc.len()` or `.to_upper()` on a string over
//!   [`MAX_STRING_SIZE`] is a `Limit` diagnostic that names the entry.
//! * **A hidden entry is absent.** Every variable is copied into the scope with
//!   the entries that do not fit removed (largest first; a map that does not
//!   fit is pruned inside instead of removed, so `cfg.name` survives a
//!   2000-row `cfg.rows`). Any read of a removed entry is a "Property not
//!   found" diagnostic that names it and its size; `keys()`, `len()` and
//!   `contains` on the enclosing map do not see it either.
//!
//! **Native stack.** Rhai evaluates by recursion, and a debug build's frames
//! are large. With the limits above the worst expressions found (every
//! construct nested to the cap, as a recursion bomb at the call limit) need
//! about 350 KiB (measured as the smallest thread that survives them); the
//! test suite runs them on a 1 MiB thread. A host that evaluates on a much
//! smaller stack must lower the constants.
//!
//! **Peak memory.** Rhai checks a value's size when a call *returns*, but a
//! native such as `array.map(...)` keeps every closure result until it does,
//! and a closure that returns a list from `state` copies it each time. Three
//! rules bound that:
//!
//! 1. every array a native can iterate has at most [`MAX_ARRAY_SIZE`] elements
//!    (a literal is capped by the parser, a built array by the size check at
//!    the native's return, and a `state` list by rule 2), so one call makes at
//!    most that many iterations;
//! 2. what a binding can read fits the same limits, counted the way Rhai
//!    counts, and is a deep copy that shares nothing with the host (see
//!    [`BindingScope`]);
//! 3. a value an expression *builds* is size-checked at its native's return.
//!
//! One iteration therefore copies at most [`MAX_ARRAY_SIZE`] array elements,
//! [`MAX_MAP_SIZE`] map entries and [`MAX_STRING_SIZE`] bytes of new text, and
//! one native call keeps at most [`MAX_ARRAY_SIZE`] of those.
//!
//! **The visible budget is 512.** In the worst shape (a list of about five
//! hundred one-field rows, returned once per element of a 511-element list) one
//! evaluation transiently allocates about 154 MiB in a debug build; typical
//! shapes are a third of that or less. The table, and what lowers it further,
//! are on [`PEAK_MEMORY_CEILING`]; the `limits` tests pin the estimate.
//!
//! # Closures that read `state`
//!
//! `state.items.map(|x| x * state.n)` mentions `state` as both the receiver
//! and a captured variable. Rhai makes a captured variable shared, and a
//! shared value cannot be read while it is borrowed as a receiver ("Data race
//! detected"). The evaluator therefore retries such a binding on a second
//! engine whose resolver hands out **private copies** of `state`, `props` and
//! `env`, and remembers that choice for the source. Hoisting the read keeps
//! the fast path: `(|n| state.items.filter(|x| x > n)).call(state.n)`.
//!
//! A closure created *inside another closure's body* that mentions `state`,
//! `props` or `env` is not rescued: the outer body's variable is the
//! closure's own parameter, which the inner closure would have to share, and
//! a copy per element is not affordable. It reports "Data race detected".

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use rhai::packages::{
    ArithmeticPackage, BasicArrayPackage, BasicFnPackage, BasicIteratorPackage, BasicMapPackage,
    BasicMathPackage, BasicStringPackage, LogicPackage, MoreStringPackage, Package,
};
use rhai::{AST, Dynamic, Engine, EvalAltResult, Map, OptimizationLevel, Scope};

use crate::cache::Cache;
pub use crate::dependencies::Dependencies;
use crate::error::BindError;
pub use crate::limits::{
    MAX_ARRAY_SIZE, MAX_CALL_LEVELS, MAX_EXPR_DEPTHS, MAX_MAP_SIZE, MAX_OPERATIONS,
    MAX_STRING_SIZE, PEAK_MEMORY_CEILING,
};
pub use crate::scope::BindingScope;
use crate::value::{Value, ValueKind};
use crate::visible::deep_copy;

/// Compiled expressions kept at least, before the least recently used are
/// evicted. A session raises it to twice its own number of distinct sources.
const BASE_CACHE_CAPACITY: usize = 4096;

/// How much work a [`Bindings`] has done, for tests and profiling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BindingStats {
    /// Sources compiled. A source is compiled once and cached, unless it was
    /// evicted to make room.
    pub compilations: u64,
    /// Expressions evaluated.
    pub evaluations: u64,
}

/// A compiled expression and what evaluating it has taught us.
struct Compiled {
    ast: AST,
    /// Set once the fast path hit a data race: the source is evaluated on
    /// private copies from then on.
    private_copies: Cell<bool>,
}

type Cached = Result<Rc<Compiled>, BindError>;

/// A compiled-expression cache and the restricted engine that runs it.
///
/// Share one between sessions with an [`Rc`]: sources are compiled once, by
/// text, however many nodes or forms use them. It has no setting; the limits
/// are the constants of this module.
pub struct Bindings {
    /// The fast engine: `state`, `props` and `env` are scope constants.
    engine: Engine,
    /// The engine for sources that hit a data race, built on first need.
    copying: OnceCell<Engine>,
    /// Operations spent by the evaluation in progress; the progress hooks of
    /// both engines count into it and `evaluate` resets it.
    budget: Rc<Cell<u64>>,
    functions: usize,
    cache: RefCell<Cache<Cached>>,
    compilations: Cell<u64>,
    evaluations: Cell<u64>,
}

impl Default for Bindings {
    fn default() -> Self {
        Self::new()
    }
}

/// Builds the restricted engine. `private_copies` adds the resolver that hands
/// closures their own copies of the three variables.
fn build_engine(budget: &Rc<Cell<u64>>, private_copies: bool) -> (Engine, usize) {
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
    engine.set_optimization_level(OptimizationLevel::None);
    engine.set_strict_variables(true);
    engine.set_fail_on_invalid_map_property(true);
    engine.set_max_operations(MAX_OPERATIONS);
    let spent = Rc::clone(budget);
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
    if private_copies {
        // At the top level the resolver answers with a deep copy, which also
        // makes Rhai skip sharing the variable when a closure captures it.
        // Inside a function the name is the closure's own parameter, so the
        // normal lookup is left alone.
        #[allow(deprecated)] // `on_var` is flagged volatile, not deprecated.
        engine.on_var(|name, _index, context| {
            if context.call_level() > 0 || !matches!(name, "state" | "props" | "env") {
                return Ok(None);
            }
            Ok(context.scope().get(name).map(|value| deep_copy(value, 0)))
        });
    }
    (engine, functions)
}

impl Bindings {
    /// Builds the restricted engine (see the [module docs](self)).
    #[must_use]
    pub fn new() -> Self {
        let budget = Rc::new(Cell::new(0_u64));
        let (engine, functions) = build_engine(&budget, false);
        Self {
            engine,
            copying: OnceCell::new(),
            budget,
            functions,
            cache: RefCell::new(Cache::new(BASE_CACHE_CAPACITY)),
            compilations: Cell::new(0),
            evaluations: Cell::new(0),
        }
    }

    /// How many native functions the engine has registered.
    ///
    /// The set is chosen package by package (see the [module docs](self)); a
    /// test pins this number so a Rhai upgrade or a package change cannot add a
    /// function to the sandbox unnoticed.
    #[must_use]
    pub fn registered_functions(&self) -> usize {
        self.functions
    }

    /// Makes room for `sources` distinct expressions at once, so a tree with
    /// more bindings than the base capacity does not recompile every turn.
    ///
    /// A [`Session`](crate::Session) calls this with its own count.
    pub fn reserve(&self, sources: usize) {
        self.cache.borrow_mut().reserve(sources.saturating_mul(2));
    }

    /// Compiles `source` without running or caching it.
    ///
    /// # Errors
    ///
    /// [`BindError::Syntax`] with the position, for anything that is not a
    /// single expression over `state`, `props` and `env`.
    pub fn check_expression(&self, source: &str) -> Result<(), BindError> {
        if let Some(cached) = self.cache.borrow().peek(source) {
            return cached.as_ref().map(|_| ()).map_err(Clone::clone);
        }
        self.compile(source).map(|_| ())
    }

    /// Evaluates `source` against `scope` and converts the result.
    ///
    /// `kind` is the property's declared kind: with it, only a result of that
    /// kind is accepted (see [`Value::from_dynamic_as`]); without it any
    /// supported kind is.
    ///
    /// The source is compiled on first use and the compile result, including a
    /// compile error, is cached by source text.
    ///
    /// # Errors
    ///
    /// A [`BindError`]. Nothing panics, and no expression can change the maps
    /// `scope` was built from.
    pub fn evaluate(
        &self,
        source: &str,
        scope: &mut BindingScope,
        kind: Option<ValueKind>,
    ) -> Result<Value, BindError> {
        let compiled = self.compiled(source)?;
        self.evaluations.set(self.evaluations.get() + 1);
        let mut outcome = if compiled.private_copies.get() {
            self.run(self.copying_engine(), &compiled.ast, scope)
        } else {
            self.run(&self.engine, &compiled.ast, scope)
        };
        if let Err(error) = &outcome {
            let race = matches!(error.unwrap_inner(), EvalAltResult::ErrorDataRace(..));
            if race && !compiled.private_copies.get() {
                compiled.private_copies.set(true);
                outcome = self.run(self.copying_engine(), &compiled.ast, scope);
            }
        }
        let result = outcome.map_err(|error| {
            let mut error = BindError::from_eval(&error);
            scope.explain(&mut error);
            error
        })?;
        match kind {
            Some(kind) => Value::from_dynamic_as(&result, kind),
            None => Value::from_dynamic(&result),
        }
        .map_err(BindError::from)
    }

    /// The work done so far.
    #[must_use]
    pub fn stats(&self) -> BindingStats {
        BindingStats {
            compilations: self.compilations.get(),
            evaluations: self.evaluations.get(),
        }
    }

    fn run(
        &self,
        engine: &Engine,
        ast: &AST,
        scope: &mut BindingScope,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        self.budget.set(0);
        engine.eval_ast_with_scope::<Dynamic>(&mut scope.scope, ast)
    }

    fn copying_engine(&self) -> &Engine {
        self.copying
            .get_or_init(|| build_engine(&self.budget, true).0)
    }

    /// The compiled expression for `source`, from the cache or freshly
    /// compiled. No borrow of the cache is held while compiling or evaluating.
    fn compiled(&self, source: &str) -> Cached {
        let hit = self.cache.borrow_mut().get(source);
        if let Some(cached) = hit {
            return cached;
        }
        let entry = self.compile(source).map(Rc::new);
        self.cache.borrow_mut().insert(source, entry.clone());
        entry
    }

    fn compile(&self, source: &str) -> Result<Compiled, BindError> {
        self.compilations.set(self.compilations.get() + 1);
        let mut names = Scope::new();
        names.push("state", Map::new());
        names.push("props", Map::new());
        names.push("env", Map::new());
        let ast = self
            .engine
            .compile_expression_with_scope(&names, source)
            .map_err(|error| BindError::from_parse(&error))?;
        Ok(Compiled {
            ast,
            private_copies: Cell::new(false),
        })
    }
}

/// Checks that `source` is a valid binding expression, for lint use.
///
/// This builds a throwaway [`Bindings`]; to check many sources, build one and
/// call [`Bindings::check_expression`].
///
/// # Errors
///
/// [`BindError::Syntax`], which carries the position in the source.
pub fn check_expression(source: &str) -> Result<(), BindError> {
    Bindings::new().check_expression(source)
}
