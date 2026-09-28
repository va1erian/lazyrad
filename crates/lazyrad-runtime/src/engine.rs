#![forbid(unsafe_code)]

//! Building the Rhai [`Engine`](rhai::Engine) the runtime evaluates scripts
//! with, and the host that runs a form's script against a live form.
//!
//! The feature set is fixed by the workspace manifest (PLAN.md §1): `debugging`
//! for breakpoints and call stacks, `metadata` for completion, and `internals`
//! for the tokenizer, with `sync` deliberately absent so the engine keeps `Rc`.
//!
//! # How a script sees controls
//!
//! Rhai functions cannot see the enclosing scope, so `txtName.text` inside
//! `fn cmdHello_Click()` would not resolve by itself. [`EngineHost`] installs an
//! [`Engine::on_var`] resolver that, for an unknown name, looks up the active
//! form's controls, then `Me`, then the registered globals.
//!
//! The resolver *pushes the value into the scope* rather than returning it. A
//! value returned from `on_var` is marked read-only by Rhai, so a setter such as
//! `lbl.caption = …` would fail with a "cannot modify property of constant"
//! error; a pushed variable is a normal mutable entry and the setter works.
//!
//! # Limits and stopping
//!
//! [`new_engine`] applies operation and call-depth limits. [`EngineHost`] adds
//! an [`Engine::on_progress`] hook: a per-run operation budget stops a runaway
//! handler, and [`EngineHost::stop`] lets the host (a Ctrl+Break, a debugger
//! pause) terminate a running script at the next progress check.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
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

/// A fresh Rhai engine configured for LazyRAD.
///
/// Each form gets its own engine because the resolver and the registered
/// control types are per-form state. The shipped limits are applied here; use
/// [`EngineHost`] for a form wired to live widgets.
pub fn new_engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(DEFAULT_MAX_OPERATIONS);
    engine.set_max_call_levels(DEFAULT_MAX_CALL_LEVELS);
    engine.set_max_expr_depths(64, 32);
    engine
}

/// The progress state shared with the engine's `on_progress` hook.
struct Progress {
    stop: Cell<bool>,
    budget: Cell<u64>,
}

/// A Rhai engine wired to one form, with the compiled-script helpers around it.
///
/// The host owns the resolver, the form object (`Me`) and the globals, so a
/// script sees the same form across events. Compile errors and runtime errors
/// are both returned as a located [`ScriptError`].
pub struct EngineHost {
    engine: Engine,
    file: String,
    progress: Rc<Progress>,
    globals: Rc<RefCell<BTreeMap<String, Dynamic>>>,
}

impl EngineHost {
    /// Creates an engine whose unknown names resolve to `host`'s controls,
    /// `Me`, then the globals (which start empty).
    ///
    /// `catalog` supplies the property names each control accepts and the
    /// schema used to decode script values.
    #[allow(deprecated)] // `Engine::on_var` is flagged volatile but is the API this uses.
    pub fn new(host: Rc<dyn FormHost>, catalog: &Catalog, file: impl Into<String>) -> Self {
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
            } else if name == "Me" {
                context.scope_mut().push(name, form.clone());
            } else if let Some(value) = globals_resolver.borrow().get(name) {
                context.scope_mut().push(name, value.clone());
            }
            Ok(None)
        });

        EngineHost {
            engine,
            file: file.into(),
            progress,
            globals,
        }
    }

    /// The underlying engine, for stdlib registration and metadata.
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

    /// Adds or replaces a global, resolved after controls and `Me`.
    ///
    /// The stdlib's `App` and `Debug` objects are registered this way.
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

    /// Calls a script function defined in `ast`, locating any runtime error.
    ///
    /// A fresh scope is used for each call; the resolver re-populates it with
    /// the control and form handles the function reaches for.
    pub fn call(&self, ast: &AST, function: &str) -> Result<Dynamic, ScriptError> {
        let mut scope = Scope::new();
        self.engine
            .call_fn::<Dynamic>(&mut scope, ast, function, ())
            .map_err(|error| ScriptError::from_eval(&self.file, &error))
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
        engine.register_fn("lazyrad_probe", |x: i64| x + 1);
        let metadata = engine
            .gen_fn_metadata_to_json(true)
            .expect("metadata serialises to JSON");
        assert!(metadata.contains("lazyrad_probe"));
    }
}
