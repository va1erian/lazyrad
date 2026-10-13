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
//! The engine is built from [`Engine::new_raw`] plus the packages an
//! expression needs (core, logic, math, strings, arrays, maps), so it has no
//! time source and no I/O. `print`, `debug` and `eval` are disabled symbols
//! and no module resolver is installed. Statements, assignments and loops are
//! rejected at compile time because the source is compiled with
//! [`Engine::compile_expression_with_scope`]. A runaway expression ends at the
//! operation, call-depth, expression-depth or data-size limits below, with a
//! [`BindError::Limit`].
//!
//! Optimisation is off, so nothing is folded at compile time (compilation
//! always terminates) and the placeholder values the variables are compiled
//! against can never leak into a constant.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rhai::packages::{
    BasicArrayPackage, BasicMapPackage, BasicMathPackage, CorePackage, LogicPackage,
    MoreStringPackage, Package,
};
use rhai::{AST, Dynamic, Engine, Map, OptimizationLevel, Scope};

use crate::error::BindError;
use crate::value::{Value, ValueKind};

/// The most operations one evaluation may execute, **including** those run by
/// closures that array methods such as `map` call back into.
///
/// Rhai restarts its own counter inside such a callback, so the engine's
/// `max_operations` alone does not bound `array.map(...)`; a shared counter in
/// the progress hook does.
pub const MAX_OPERATIONS: u64 = 100_000;
/// The deepest call nesting an expression may reach (closures calling closures).
pub const MAX_CALL_LEVELS: usize = 16;
/// The deepest expression nesting the parser accepts, and the same limit
/// inside a function body.
pub const MAX_EXPR_DEPTHS: (usize, usize) = (64, 32);
/// The longest string a value may have, in bytes.
///
/// The limit also applies to a string that is already in `state`: calling a
/// method on one longer than this is reported as a [`BindError::Limit`].
pub const MAX_STRING_SIZE: usize = 1 << 24;
/// The largest array a value may have (see [`MAX_STRING_SIZE`]).
pub const MAX_ARRAY_SIZE: usize = 1_000_000;
/// The largest map a value may have (see [`MAX_STRING_SIZE`]).
pub const MAX_MAP_SIZE: usize = 100_000;
/// Compiled expressions kept before the cache is emptied. Trees are finite,
/// so this only bounds an editor that lints many different sources.
const MAX_CACHED_EXPRESSIONS: usize = 4096;

/// Which of the three variables an expression mentions.
///
/// This is a *lexical* scan of the source for the identifiers, so it can
/// over-report (a property called `env`, or the word inside a string) but never
/// under-report: a variable can only be read by naming it. Over-reporting costs
/// an extra evaluation, never a stale value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dependencies {
    /// The expression may read `state`.
    pub state: bool,
    /// The expression may read `props`.
    pub props: bool,
    /// The expression may read `env`.
    pub env: bool,
}

impl Dependencies {
    /// Scans `source`.
    #[must_use]
    pub fn scan(source: &str) -> Self {
        let mut deps = Self::default();
        let identifiers = source
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|run| !run.is_empty());
        for identifier in identifiers {
            match identifier {
                "state" => deps.state = true,
                "props" => deps.props = true,
                "env" => deps.env = true,
                _ => {}
            }
        }
        deps
    }

    /// Whether the expression reads anything a handler turn can change.
    #[must_use]
    pub const fn reads_state_or_props(&self) -> bool {
        self.state || self.props
    }
}

/// The variables one evaluation sees: exactly `state`, `props` and `env`, as
/// constants. Build it once per turn and reuse it for every binding of the
/// turn, so the maps are copied once.
pub struct BindingScope(Scope<'static>);

impl BindingScope {
    /// Copies the three maps in as read-only variables.
    #[must_use]
    pub fn new(state: &Map, props: &Map, env: &Map) -> Self {
        let mut scope = Scope::new();
        scope.push_constant("state", state.clone());
        scope.push_constant("props", props.clone());
        scope.push_constant("env", env.clone());
        Self(scope)
    }
}

/// How much work a [`Bindings`] has done, for tests and profiling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BindingStats {
    /// Distinct sources compiled. A source is compiled once and cached.
    pub compilations: u64,
    /// Expressions evaluated.
    pub evaluations: u64,
}

type Cached = Result<Rc<AST>, BindError>;

/// A compiled-expression cache and the restricted engine that runs it.
///
/// Share one between sessions with an [`Rc`]: sources are compiled once, by
/// text, however many nodes or forms use them. It has no setting; the limits
/// are the constants of this module.
pub struct Bindings {
    engine: Engine,
    /// Operations spent by the evaluation in progress; the progress hook of
    /// the engine counts into it and `evaluate` resets it.
    budget: Rc<Cell<u64>>,
    cache: RefCell<HashMap<String, Cached>>,
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
        let mut engine = Engine::new_raw();
        engine.register_global_module(CorePackage::new().as_shared_module());
        engine.register_global_module(LogicPackage::new().as_shared_module());
        engine.register_global_module(BasicMathPackage::new().as_shared_module());
        engine.register_global_module(MoreStringPackage::new().as_shared_module());
        engine.register_global_module(BasicArrayPackage::new().as_shared_module());
        engine.register_global_module(BasicMapPackage::new().as_shared_module());
        engine.disable_symbol("eval");
        engine.disable_symbol("print");
        engine.disable_symbol("debug");
        engine.set_optimization_level(OptimizationLevel::None);
        engine.set_strict_variables(true);
        engine.set_fail_on_invalid_map_property(true);
        engine.set_max_operations(MAX_OPERATIONS);
        let budget = Rc::new(Cell::new(0_u64));
        let spent = Rc::clone(&budget);
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
        Self {
            engine,
            budget,
            cache: RefCell::new(HashMap::new()),
            compilations: Cell::new(0),
            evaluations: Cell::new(0),
        }
    }

    /// Compiles `source` without running or caching it.
    ///
    /// # Errors
    ///
    /// [`BindError::Syntax`] with the position, for anything that is not a
    /// single expression over `state`, `props` and `env`.
    pub fn check_expression(&self, source: &str) -> Result<(), BindError> {
        if let Some(cached) = self.cache.borrow().get(source) {
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
        let ast = self.compiled(source)?;
        self.evaluations.set(self.evaluations.get() + 1);
        self.budget.set(0);
        let result = self
            .engine
            .eval_ast_with_scope::<Dynamic>(&mut scope.0, &ast)
            .map_err(|error| BindError::from_eval(&error))?;
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

    /// The compiled AST for `source`, from the cache or freshly compiled.
    ///
    /// No borrow of the cache is held while compiling or evaluating.
    fn compiled(&self, source: &str) -> Cached {
        if let Some(cached) = self.cache.borrow().get(source) {
            return cached.clone();
        }
        let entry = self.compile(source).map(Rc::new);
        let mut cache = self.cache.borrow_mut();
        if cache.len() >= MAX_CACHED_EXPRESSIONS {
            cache.clear();
        }
        cache.insert(source.to_owned(), entry.clone());
        entry
    }

    fn compile(&self, source: &str) -> Result<AST, BindError> {
        self.compilations.set(self.compilations.get() + 1);
        let mut names = Scope::new();
        names.push("state", Map::new());
        names.push("props", Map::new());
        names.push("env", Map::new());
        self.engine
            .compile_expression_with_scope(&names, source)
            .map_err(|error| BindError::from_parse(&error))
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
