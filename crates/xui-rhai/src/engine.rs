#![forbid(unsafe_code)]

//! Building the Rhai [`Engine`] a script is evaluated with, and
//! the host that runs a form's script against a live form.
//!
//! The feature set is fixed by the workspace manifest (PLAN.md §1): `debugging`
//! for breakpoints and call stacks, `metadata` for completion, and `internals`
//! for the tokenizer, with `sync` deliberately absent so the engine keeps `Rc`.
//!
//! # How a script sees controls
//!
//! Rhai functions cannot see the enclosing scope, so `name_edit.text` inside
//! `fn hello_button_click()` would not resolve by itself. [`EngineHost`] installs an
//! [`Engine::on_var`] resolver that, for an unknown name, looks up the active
//! form's controls, then `form`, then the registered globals.
//!
//! The resolver *pushes the value into the scope* rather than returning it. A
//! value returned from `on_var` is marked read-only by Rhai, so a setter such as
//! `label.text = …` would fail with a "cannot modify property of constant"
//! error; a pushed variable is a normal mutable entry and the setter works.
//!
//! # The stdlib seam
//!
//! The engine built here is bare. A host plugs its own functions and globals in
//! through [`EngineSetup`]: LazyRAD passes its `StdlibContext`, another xui app
//! passes `()` and registers whatever it needs.
//!
//! # Limits and stopping
//!
//! [`new_engine`] applies operation and call-depth limits. [`EngineHost`] adds
//! an [`Engine::on_progress`] hook: a per-run operation budget stops a runaway
//! handler, and [`EngineHost::stop`] lets the host (a Ctrl+Break, a debugger
//! pause) terminate a running script at the next progress check.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use rhai::{AST, Dynamic, Engine, Scope};
use xui_form::Catalog;

use crate::control::{Form, FormHost, controls_by_name, register_control, register_form};
use crate::error::ScriptError;

/// The shipped operation limit; a script may not execute more operations than
/// this. It is a backstop above [`DEFAULT_OPERATION_BUDGET`].
pub const DEFAULT_MAX_OPERATIONS: u64 = 10_000_000;

/// The shipped call-depth limit.
pub const DEFAULT_MAX_CALL_LEVELS: usize = 128;

/// The shipped per-run operation budget checked by the progress hook.
pub const DEFAULT_OPERATION_BUDGET: u64 = 1_000_000;

/// A fresh Rhai engine with the shipped limits applied.
///
/// Each form gets its own engine because the resolver and the registered
/// control types are per-form state.
pub fn new_engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(DEFAULT_MAX_OPERATIONS);
    engine.set_max_call_levels(DEFAULT_MAX_CALL_LEVELS);
    engine.set_max_expr_depths(64, 32);
    engine
}

/// A hook that installs host-specific functions and globals on a fresh engine.
///
/// [`EngineHost::new`] calls it once, after the control, form and resolver
/// types are in place, so the host's standard library sees the same engine
/// every form does. `()` installs nothing, which is what a plain xui app that
/// needs no standard library passes.
pub trait EngineSetup {
    /// Installs the host's functions and globals on `host`.
    fn setup(self, host: &mut EngineHost);
}

impl EngineSetup for () {
    fn setup(self, _host: &mut EngineHost) {}
}

/// The progress state shared with the engine's `on_progress` hook.
struct Progress {
    stop: Cell<bool>,
    budget: Cell<u64>,
}

/// A Rhai engine wired to one form, with the compiled-script helpers around it.
///
/// The host owns the resolver, the form object (`form`) and the globals, so a
/// script sees the same form across events. Compile errors and runtime errors
/// are both returned as a located [`ScriptError`].
pub struct EngineHost {
    engine: Engine,
    file: String,
    progress: Rc<Progress>,
    globals: Rc<RefCell<BTreeMap<String, Dynamic>>>,
    /// Every module registered through [`EngineHost::register_module`], so
    /// `import "name" as …` can resolve them again after a later registration.
    modules: rhai::module_resolvers::StaticModuleResolver,
    /// Names bound by top-level `let`/`const` in the prepared script. Functions
    /// cannot see them, so an unknown-variable error for one gets a hint.
    top_level_vars: RefCell<BTreeSet<String>>,
}

impl EngineHost {
    /// Creates an engine whose unknown names resolve to `host`'s controls,
    /// `form`, then the globals (which start empty).
    ///
    /// `catalog` supplies the property names each control accepts and the
    /// schema used to decode script values. `setup` installs the host's own
    /// functions and globals (see [`EngineSetup`]).
    #[allow(deprecated)] // `Engine::on_var` is flagged volatile but is the API this uses.
    pub fn new<S: EngineSetup>(
        host: Rc<dyn FormHost>,
        catalog: &Catalog,
        file: impl Into<String>,
        setup: S,
    ) -> Self {
        let mut engine = new_engine();
        register_control(&mut engine, catalog);
        register_form(&mut engine);

        let controls = controls_by_name(&host);
        let form = Form::new(Rc::clone(&host));
        let globals: Rc<RefCell<BTreeMap<String, Dynamic>>> =
            Rc::new(RefCell::new(BTreeMap::new()));
        let progress = Rc::new(Progress {
            stop: Cell::new(false),
            budget: Cell::new(DEFAULT_OPERATION_BUDGET),
        });

        let progress_hook = Rc::clone(&progress);
        engine.on_progress(move |operations| {
            if progress_hook.stop.get() {
                Some(Dynamic::from("script stopped"))
            } else {
                let budget = progress_hook.budget.get();
                (budget != 0 && operations > budget)
                    .then(|| Dynamic::from("operation budget exceeded"))
            }
        });

        let globals_resolver = Rc::clone(&globals);
        engine.on_var(move |name, _index, mut context| {
            if context.scope_mut().contains(name) {
                return Ok(None);
            }
            if let Some(control) = controls.get(name) {
                context.scope_mut().push(name, control.clone());
            } else if name == "form" {
                context.scope_mut().push(name, form.clone());
            } else if let Some(value) = globals_resolver.borrow().get(name) {
                context.scope_mut().push(name, value.clone());
            }
            Ok(None)
        });

        let mut host = EngineHost {
            engine,
            file: file.into(),
            progress,
            globals,
            modules: rhai::module_resolvers::StaticModuleResolver::new(),
            top_level_vars: RefCell::new(BTreeSet::new()),
        };
        setup.setup(&mut host);
        host
    }

    /// The underlying engine, for metadata and stdlib calls.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The underlying engine, mutably.
    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// The script file errors are reported against.
    pub fn file(&self) -> &str {
        &self.file
    }

    /// Adds or replaces a global, resolved after controls and `form`.
    ///
    /// The stdlib's `app` object is registered this way.
    pub fn set_global(&mut self, name: impl Into<String>, value: Dynamic) {
        self.globals.borrow_mut().insert(name.into(), value);
    }

    /// Requests that a running script stop at the next progress check.
    pub fn stop(&self) {
        self.progress.stop.set(true);
    }

    /// Clears a previous [`EngineHost::stop`].
    pub fn resume(&self) {
        self.progress.stop.set(false);
    }

    /// Whether a stop has been requested.
    pub fn is_stopped(&self) -> bool {
        self.progress.stop.get()
    }

    /// Sets the per-run operation budget; `0` disables the budget.
    pub fn set_operation_budget(&self, operations: u64) {
        self.progress.budget.set(operations);
    }

    /// Compiles `source`, locating a parse error against [`EngineHost::file`].
    pub fn compile(&self, source: &str) -> Result<AST, ScriptError> {
        self.engine
            .compile(source)
            .map_err(|error| ScriptError::from_parse(&self.file, &error))
    }

    /// Runs `ast`'s top-level statements once and returns the AST its handlers
    /// are called on.
    ///
    /// Rhai's `call_fn` evaluates an AST's statements before every call, so
    /// calling handlers on the full script would re-run its top-level code on
    /// every event. The returned AST keeps the functions and only the `import`
    /// statements, so `import "util" as util` aliases still resolve in handlers.
    pub fn prepare(&self, ast: &AST) -> Result<AST, ScriptError> {
        self.engine
            .run_ast_with_scope(&mut Scope::new(), ast)
            .map_err(|error| ScriptError::from_eval(&self.file, &error))?;
        // Replace, not extend: a host can prepare several scripts in turn, and
        // the hint must only name the current script's top-level `let`s.
        *self.top_level_vars.borrow_mut() = ast
            .statements()
            .iter()
            .filter_map(|statement| match statement {
                rhai::Stmt::Var(binding, ..) => Some(binding.0.name.to_string()),
                _ => None,
            })
            .collect();
        let imports = ast
            .statements()
            .iter()
            .filter(|statement| matches!(statement, rhai::Stmt::Import(..)))
            .cloned();
        Ok(AST::new(imports, ast.shared_lib().clone()))
    }

    /// Calls a script function defined in `ast` with no arguments, locating any
    /// runtime error.
    ///
    /// A fresh scope is used for each call; the resolver re-populates it with
    /// the control and form handles the function reaches for.
    pub fn call(&self, ast: &AST, function: &str) -> Result<Dynamic, ScriptError> {
        self.call_with(ast, function, Vec::new())
    }

    /// Calls a script function defined in `ast`, passing `args`.
    ///
    /// [`Vec<Dynamic>`](Dynamic) implements Rhai's `FuncArgs`, so a handler with
    /// any number of parameters can be called from one place. The caller is
    /// responsible for matching the argument count to the function's signature;
    /// a mismatch is an ordinary Rhai runtime error.
    pub fn call_with(
        &self,
        ast: &AST,
        function: &str,
        args: Vec<Dynamic>,
    ) -> Result<Dynamic, ScriptError> {
        let mut scope = Scope::new();
        self.engine
            .call_fn::<Dynamic>(&mut scope, ast, function, args)
            .map_err(|error| self.locate(&error))
    }

    /// Locates a runtime error from a handler, adding a hint when the missing
    /// variable is one a top-level `let` declared (functions cannot see those).
    fn locate(&self, error: &rhai::EvalAltResult) -> ScriptError {
        let mut located = ScriptError::from_eval(&self.file, error);
        let mut inner = error;
        while let rhai::EvalAltResult::ErrorInFunctionCall(_, _, next, _) = inner {
            inner = next;
        }
        if let rhai::EvalAltResult::ErrorVariableNotFound(name, _) = inner {
            if self.top_level_vars.borrow().contains(name) {
                located.message.push_str(&format!(
                    "\n'{name}' is a top-level variable; functions can't see those. \
                     Keep values between events in form.state (e.g. form.state.{name})."
                ));
            }
        }
        located
    }

    /// Compiles `source` into a Rhai module named `name` and registers it.
    ///
    /// The module's functions are exposed both as a namespace (`import "name"`)
    /// and in the global namespace, so a standard module's helpers can be called
    /// directly (`Greeting("Ada")`) or qualified (`util::Greeting("Ada")`).
    /// `file` is the label a compile or evaluation error is reported against.
    pub fn register_module(
        &mut self,
        name: &str,
        file: &str,
        source: &str,
    ) -> Result<(), ScriptError> {
        let ast = self
            .engine
            .compile(source)
            .map_err(|error| ScriptError::from_parse(file, &error))?;
        let module = rhai::Module::eval_ast_as_new(Scope::new(), &ast, &self.engine)
            .map_err(|error| ScriptError::from_eval(file, &error))?;
        // A resolver entry backs `import "name" as x`; the global and static
        // registrations back `Name::fn()` and a bare `fn()` call respectively.
        self.modules.insert(name, module.clone());
        self.engine.set_module_resolver(self.modules.clone());
        let shared: rhai::Shared<rhai::Module> = module.into();
        self.engine.register_global_module(shared.clone());
        self.engine.register_static_module(name, shared);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::new_engine;

    #[test]
    fn the_engine_evaluates_rhai() {
        let engine = new_engine();
        let value: i64 = engine.eval("40 + 2").expect("a trivial script evaluates");
        assert_eq!(value, 42);
    }

    #[test]
    fn the_metadata_feature_reports_registered_functions() {
        // `gen_fn_metadata_to_json` only exists with the `metadata` feature,
        // so building at all proves the feature is on.
        let mut engine = new_engine();
        engine.register_fn("probe", |x: i64| x + 1);
        let metadata = engine
            .gen_fn_metadata_to_json(true)
            .expect("metadata serialises to JSON");
        assert!(metadata.contains("probe"));
    }
}
