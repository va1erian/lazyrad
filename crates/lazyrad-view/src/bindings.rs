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
//! Every limit ends an expression with a [`BindError::Limit`], never a panic or
//! an abort. They are sized for *view* expressions, not data processing:
//!
//! | Limit | Value | Why |
//! |---|---|---|
//! | operations | [`MAX_OPERATIONS`] | a label or a filtered count over a few thousand items |
//! | call depth | [`MAX_CALL_LEVELS`] | closures in closures; a level costs about 60 KiB of native stack in a debug build |
//! | expression depth | [`MAX_EXPR_DEPTHS`] | parsing and evaluating cost 7 to 25 KiB of native stack per level in a debug build |
//! | array / map / string size | [`MAX_ARRAY_SIZE`], [`MAX_MAP_SIZE`], [`MAX_STRING_SIZE`] | a value an expression builds; see below |
//! | visible state | [`VISIBLE_BUDGET`] | what an expression can read; see below |
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
//! 2. what a binding can read is bounded: [`BindingScope`] copies each of
//!    `state`, `props` and `env` with its largest entries replaced by a
//!    placeholder once the variable holds more than [`VISIBLE_BUDGET`] array
//!    elements and map entries in total (strings are immutable and shared, so
//!    they are never hidden). A binding that uses a hidden entry gets a
//!    diagnostic naming it, and the entry is never copied;
//! 3. a value an expression *builds* is size-checked at its native's return.
//!
//! One iteration therefore copies at most [`VISIBLE_BUDGET`] elements or
//! [`MAX_STRING_SIZE`] bytes, so one native call holds at most
//! [`MAX_ARRAY_SIZE`] of those: about 16 MiB of array elements, 64 MiB of map
//! entries, 16 MiB of text, all under [`PEAK_MEMORY_CEILING`].
//!
//! The limits apply to values already in `state` too: a method call on a
//! string longer than [`MAX_STRING_SIZE`], or on an array of more than
//! [`MAX_ARRAY_SIZE`] elements, is a `Limit` diagnostic. Keep derived counts
//! and summaries in `state`, and long lists in a control that owns its rows.
//!
//! # Closures that read `state`
//!
//! `state.items.map(|x| x * state.n)` mentions `state` as both the receiver
//! and a captured variable. Rhai makes a captured variable shared, and a
//! shared value cannot be read while it is borrowed as a receiver ("Data race
//! detected"). The evaluator therefore retries such a binding on a second
//! engine whose resolver hands out **private copies** of `state`, `props` and
//! `env` (at most [`VISIBLE_BUDGET`] elements each), and remembers that choice
//! for the source. Hoisting the read keeps the fast path:
//! `(|n| state.items.filter(|x| x > n)).call(state.n)`.
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
pub use crate::scope::BindingScope;
use crate::value::{Value, ValueKind};
pub use crate::visible::VISIBLE_BUDGET;
use crate::visible::{HIDDEN_TYPE_NAME, Hidden};

/// The most operations one evaluation may execute, **including** those run by
/// closures that array methods such as `map` call back into.
///
/// Rhai restarts its own counter inside such a callback, so the engine's
/// `max_operations` alone does not bound `array.map(...)`; a shared counter in
/// the progress hook does.
pub const MAX_OPERATIONS: u64 = 12_000;
/// The deepest call nesting an expression may reach (closures calling closures).
pub const MAX_CALL_LEVELS: usize = 4;
/// The deepest expression nesting the parser accepts in the expression and in a
/// function body.
pub const MAX_EXPR_DEPTHS: (usize, usize) = (16, 10);
/// The most bytes of string a value may hold (summed over a nested value).
pub const MAX_STRING_SIZE: usize = 16 * 1024;
/// The most elements an array value may hold (summed over a nested value).
pub const MAX_ARRAY_SIZE: usize = 1024;
/// The most entries a map value may hold (summed over a nested value).
pub const MAX_MAP_SIZE: usize = 256;
/// What the limits above keep one evaluation's transient allocation under,
/// in bytes. See the module docs.
pub const PEAK_MEMORY_CEILING: u64 = 64 * 1024 * 1024;
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
    engine.register_type_with_name::<Hidden>(HIDDEN_TYPE_NAME);
    if private_copies {
        // At the top level the resolver answers with a plain copy, which also
        // makes Rhai skip sharing the variable when a closure captures it.
        // Inside a function the name is the closure's own parameter, so the
        // normal lookup is left alone.
        #[allow(deprecated)] // `on_var` is flagged volatile, not deprecated.
        engine.on_var(|name, _index, context| {
            if context.call_level() > 0 || !matches!(name, "state" | "props" | "env") {
                return Ok(None);
            }
            Ok(context.scope().get(name).map(Dynamic::flatten_clone))
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
            if let BindError::Runtime { message, .. } = &mut error {
                if message.contains(HIDDEN_TYPE_NAME) && !scope.hidden.is_empty() {
                    *message = format!("{message}; {}", scope.too_large());
                }
            }
            error
        })?;
        if result.is::<Hidden>() {
            return Err(BindError::Runtime {
                message: scope.too_large(),
                position: None,
            });
        }
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
