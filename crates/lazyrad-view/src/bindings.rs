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
//! I/O, no module resolver, and `print`, `debug`, `eval` and `type_of` are
//! disabled symbols. [`Bindings::registered_functions`] exposes the size of the
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
//! Every limit is a constant in [`crate::limits`], which is the one place that
//! states the numbers and says why; this page and the README refer there, and
//! a test fails if either repeats one. Every limit ends an expression with a
//! [`BindError::Limit`] (or, for nesting, a syntax error), never a panic or an
//! abort. Three consequences are worth knowing:
//!
//! * **A list of records is limited by its fields.** Rhai counts one map entry
//!   per field, so the rows a list can hold are the map limit divided by the
//!   fields in a row. A list over a limit is hidden from bindings.
//! * **A long string can be read but not used.** `bind text: state.doc` works
//!   at any length; a method call on a string over the string limit is a
//!   `Limit` diagnostic that names the entry.
//! * **A hidden entry keeps its key and loses its value.** Every variable is
//!   copied into the scope with the entries that do not fit replaced by an
//!   opaque value (largest first; a map that does not fit is pruned inside, so
//!   `cfg.name` survives a very long `cfg.rows`). `contains`, `keys`, `len`
//!   and a comparison of two maps see the real shape; *using* the value (a
//!   read, a comparison, a method call, a truthiness test, a text conversion
//!   including `to_json`) is a diagnostic that names the entry and why it is
//!   hidden, however the result is used afterwards. See [`crate::limits`] for
//!   the memory this buys.
//!
//! # Closures that read `state`
//!
//! `state.items.map(|x| x * state.n)` mentions `state` as both the receiver
//! and a captured variable. Rhai makes a captured variable shared, and a
//! shared value cannot be read while it is borrowed as a receiver ("Data race
//! detected"). The evaluator therefore retries such a binding on a second
//! engine whose resolver hands out **private, read-only, shared copies** of
//! `state`, `props` and `env`: a closure curries a handle to one copy, so it
//! costs a handle clone per call, not a copy of the state. The choice is
//! remembered for the source, and [`BindingStats::state_copies`] counts the
//! copies. Hoisting the read keeps the fast path:
//! `(|n| state.items.filter(|x| x > n)).call(state.n)`.
//!
//! A closure created *inside another closure's body* that mentions `state`,
//! `props` or `env` is not rescued: the outer body's variable is the
//! closure's own parameter, which the inner closure would have to share, and
//! a copy per element is not affordable. It reports "Data race detected".

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use rhai::{AST, Dynamic, Engine, EvalAltResult, Map, Scope};

use crate::cache::Cache;
pub use crate::dependencies::Dependencies;
use crate::engine::{Counters, build_engine};
use crate::error::BindError;
pub use crate::limits::{
    MAX_ARRAY_SIZE, MAX_CALL_LEVELS, MAX_EXPR_DEPTHS, MAX_MAP_SIZE, MAX_OPERATIONS,
    MAX_STRING_SIZE, PEAK_MEMORY_CEILING,
};
use crate::poison::Poison;
pub use crate::scope::BindingScope;
use crate::value::{Value, ValueKind};

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
    /// Private copies of `state`, `props` or `env` made for closures. A source
    /// that never needs the retry makes none; one that does makes a few per
    /// evaluation, however long the lists it iterates.
    pub state_copies: u64,
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
/// are the constants of [`crate::limits`].
pub struct Bindings {
    /// The fast engine: `state`, `props` and `env` are scope constants.
    engine: Engine,
    /// The engine for sources that hit a data race, built on first need.
    copying: OnceCell<Engine>,
    counters: Counters,
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

impl Bindings {
    /// Builds the restricted engine (see the [module docs](self)).
    #[must_use]
    pub fn new() -> Self {
        let counters = Counters::default();
        let (engine, functions) = build_engine(&counters, false);
        Self {
            engine,
            copying: OnceCell::new(),
            counters,
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
    /// `scope` was built from. A result that is, or contains in a text, an
    /// entry hidden from the binding is an error naming it.
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
        // A hidden entry that was turned into text fails the evaluation, even
        // when Rhai went on with its fallback text and the result looks fine.
        if let Some(message) = self.counters.refused.borrow_mut().take() {
            return Err(BindError::Runtime {
                message,
                position: None,
            });
        }
        let result = outcome.map_err(|error| {
            let mut error = BindError::from_eval(&error);
            scope.explain(&mut error);
            error
        })?;
        if let Some(poison) = result.read_lock::<Poison>() {
            return Err(BindError::Runtime {
                message: poison.refusal("the result of a binding"),
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
            state_copies: self.counters.copies.get(),
        }
    }

    fn run(
        &self,
        engine: &Engine,
        ast: &AST,
        scope: &mut BindingScope,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        self.counters.budget.set(0);
        self.counters.refused.borrow_mut().take();
        engine.eval_ast_with_scope::<Dynamic>(&mut scope.scope, ast)
    }

    fn copying_engine(&self) -> &Engine {
        self.copying
            .get_or_init(|| build_engine(&self.counters, true).0)
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
