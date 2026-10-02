#![forbid(unsafe_code)]

//! Wiring a form's `<control>_<event>` handlers to a Rhai script.
//!
//! The form's `.rhai` script is compiled once to learn the function names it
//! defines. [`ScriptBinder`] then answers, for each widget event, whether a
//! handler exists. It wires the event to a [`Msg::Event`] only when it does, so
//! a missing handler means the event is simply not wired and clicking does
//! nothing. [`ScriptForm::run`] runs the matching Rhai function through the
//! form's [`EngineHost`].
//!
//! A handler's name is the control name and `xui`'s event name in snake_case,
//! joined by `_` ([`handler_name`]): `hello_button_click`, `name_edit_change`,
//! `agree_check_toggle` (PLAN.md §1.1).
//!
//! # Window events
//!
//! `form_load` runs once the form is built and `form_close` runs when the
//! window is asked to close. [`ScriptForm::build`] runs `form_load`;
//! [`ScriptForm::close`] runs `form_close`.
//!
//! # Standard modules
//!
//! A form can `import "util" as util` or call a module's functions directly.
//! Where the modules come from is the host's business: [`ScriptForm::build`]
//! takes a `configure` callback that registers them, and the LazyRAD runtime
//! passes the project's standard modules there.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;

use rhai::{AST, Dynamic, FnPtr};
use xui_core::app::Ui;
use xui_form::{
    Binder, BuildError, BuildOptions, Catalog, EventHandler, EventRef, Factories, FormDoc,
    LiveForm, Value, build_with,
};

use crate::control::FormHost;
use crate::engine::{EngineHost, EngineSetup};
use crate::error::ScriptError;
use crate::message::Msg;

/// The control name window events are raised for: `form_load`, `form_close`.
pub const FORM: &str = "form";

/// The script function that handles `control`'s `event`: the control name and
/// the event name in snake_case, joined by `_` (PLAN.md §1.1). `xui`'s events
/// are PascalCase (`Click`, `Toggle`), so `hello_button` + `Click` becomes
/// `hello_button_click`.
pub fn handler_name(control: &str, event: &str) -> String {
    format!("{control}_{}", snake_case(event))
}

/// `PascalCase` or `camelCase` to `snake_case`.
fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, character) in name.chars().enumerate() {
        if character.is_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.extend(character.to_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

/// The function names `source` defines, or a located parse error.
pub fn script_functions(source: &str, file: &str) -> Result<BTreeSet<String>, ScriptError> {
    let engine = crate::new_engine();
    let ast = engine
        .compile(source)
        .map_err(|error| ScriptError::from_parse(file, &error))?;
    Ok(ast
        .iter_functions()
        .map(|function| function.name.to_owned())
        .collect())
}

/// The arguments to call a handler of `arity` parameters with: the event's
/// `args`, and `()` for every parameter the event does not supply.
///
/// Rhai matches a function by name *and* arity, so a handler declaring more
/// parameters than the event carries gets `()` for the rest.
pub fn event_arguments(arity: usize, args: &[Value]) -> Vec<Dynamic> {
    let mut arguments: Vec<Dynamic> = args
        .iter()
        .take(arity)
        .map(crate::value::to_dynamic)
        .collect();
    arguments.resize(arity, Dynamic::UNIT);
    arguments
}

/// Decides which widget events become [`Msg::Event`]s.
///
/// An event is wired only when the form's script defines the matching function,
/// so a missing handler is a silent no-op (the `xui-form` binder contract).
pub struct ScriptBinder {
    form: String,
    functions: Rc<BTreeSet<String>>,
}

impl ScriptBinder {
    /// A binder for the form named `form`, given the functions its script
    /// defines (see [`script_functions`]).
    pub fn new(form: impl Into<String>, functions: Rc<BTreeSet<String>>) -> ScriptBinder {
        ScriptBinder {
            form: form.into(),
            functions,
        }
    }

    /// Whether the script defines a handler for `node`'s `event`.
    pub fn handles(&self, node: &str, event: &str) -> bool {
        self.functions.contains(&handler_name(node, event))
    }
}

impl Binder<Msg> for ScriptBinder {
    fn bind(&self, event: EventRef<'_>) -> Option<EventHandler<Msg>> {
        if !self.handles(event.node, event.event) {
            return None;
        }
        let event_name = event.event.to_owned();

        let form = self.form.clone();
        let control = event.node.to_owned();
        Some(Rc::new(move |args| {
            Some(Msg::Event {
                form: form.clone(),
                control: control.clone(),
                event: event_name.clone(),
                args: args.to_vec(),
            })
        }))
    }
}

/// A failure while building and wiring a scripted form.
#[derive(Debug)]
pub enum FormError {
    /// The form's widgets failed to build.
    Build(BuildError),
    /// The form's script failed to compile or run.
    Script(ScriptError),
}

impl fmt::Display for FormError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormError::Build(error) => error.fmt(formatter),
            FormError::Script(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for FormError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FormError::Build(error) => Some(error),
            FormError::Script(error) => Some(error),
        }
    }
}

impl From<BuildError> for FormError {
    fn from(error: BuildError) -> FormError {
        FormError::Build(error)
    }
}

impl From<ScriptError> for FormError {
    fn from(error: ScriptError) -> FormError {
        FormError::Script(error)
    }
}

/// A form's script and the labels its errors are reported against.
///
/// [`ScriptForm::build`] takes one of these so the form name, the code and the
/// file name travel together.
pub struct ScriptSource<'a> {
    /// The form name handlers address.
    pub name: &'a str,
    /// The `.rhai` code.
    pub code: &'a str,
    /// The file errors are reported against (for example `main_form.rhai`).
    pub file: &'a str,
}

/// A form built and wired: the live widgets plus the script host.
///
/// It bundles what every consumer of a scripted form needs, so an xui app can
/// build a form from a document, run a `.rhai` script against it and route
/// widget events without knowing anything about LazyRAD.
pub struct ScriptForm {
    name: String,
    form: Rc<LiveForm<Msg>>,
    host: EngineHost,
    ast: AST,
    /// Every function the script defines, mapped to its parameter count.
    functions: BTreeMap<String, usize>,
}

impl ScriptForm {
    /// Builds `doc`'s widgets, wires its handlers and runs `form_load`.
    ///
    /// `source` names the form and carries its script; `setup` installs the
    /// host's standard library on the new engine, and `configure` runs after
    /// that so the host can register modules, extra globals or form references
    /// before the script is compiled and its top-level code runs.
    pub fn build<S: EngineSetup>(
        ui: &mut Ui<Msg>,
        doc: &FormDoc,
        catalog: &Catalog,
        source: ScriptSource<'_>,
        setup: S,
        configure: impl FnOnce(&mut EngineHost) -> Result<(), ScriptError>,
    ) -> Result<ScriptForm, FormError> {
        // Compile once to learn the handler names the binder must consult.
        let handler_names = script_functions(source.code, source.file)?;
        let binder = ScriptBinder::new(source.name, Rc::new(handler_names));

        let factories: Factories<Msg> = Factories::xui();
        let form = Rc::new(build_with(
            ui,
            doc,
            catalog,
            &factories,
            &binder,
            BuildOptions::default(),
        )?);

        let mut host = EngineHost::new(
            Rc::clone(&form) as Rc<dyn FormHost>,
            catalog,
            source.file,
            setup,
        );
        configure(&mut host)?;
        let ast = host.compile(source.code)?;
        let functions = ast
            .iter_functions()
            .map(|function| (function.name.to_owned(), function.params.len()))
            .collect();
        // Top-level code runs once, here; handlers run on the imports-only AST.
        let ast = host.prepare(&ast)?;

        let script = ScriptForm {
            name: source.name.to_owned(),
            form,
            host,
            ast,
            functions,
        };
        script.load()?;
        Ok(script)
    }

    /// The form's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The live widgets.
    pub fn live_form(&self) -> &Rc<LiveForm<Msg>> {
        &self.form
    }

    /// The script host, for metadata and stdlib calls.
    pub fn host(&self) -> &EngineHost {
        &self.host
    }

    /// Runs `form_load`, if the script defines it.
    pub fn load(&self) -> Result<(), ScriptError> {
        self.run(FORM, "Load", &[])
    }

    /// Runs `form_close`, if the script defines it.
    pub fn close(&self) -> Result<(), ScriptError> {
        self.run(FORM, "Close", &[])
    }

    /// Runs the handler for `control`'s `event`, if the script defines one.
    ///
    /// A missing handler is not an error: the event is simply ignored. Window
    /// events pass [`FORM`] as the control, so `Load` maps to `form_load`.
    pub fn run(&self, control: &str, event: &str, args: &[Value]) -> Result<(), ScriptError> {
        let function = handler_name(control, event);
        let Some(&arity) = self.functions.get(&function) else {
            return Ok(());
        };
        let arguments = event_arguments(arity, args);
        let _ = self.host.call_with(&self.ast, &function, arguments)?;
        Ok(())
    }

    /// Calls a Rhai function pointer the form's script handed to the runtime.
    ///
    /// This is how a non-blocking [`Msg::MsgBox`] still reports its result: the
    /// application stores the callback, then calls it here once the dialog
    /// closes. The callback is looked up in the form's compiled [`AST`], so a
    /// script-defined function or a closure both work.
    pub fn call_callback(&self, callback: &FnPtr, result: &str) -> Result<(), ScriptError> {
        callback
            .call::<Dynamic>(self.host.engine(), &self.ast, (result.to_owned(),))
            .map(|_| ())
            .map_err(|error| ScriptError::from_eval(self.host.file(), &error))
    }

    /// Calls a Rhai function pointer with `args` and returns its value, or the
    /// engine's own error so the caller can inspect what was thrown (a host
    /// turning a thrown value into a reply, for example). [`ScriptForm::locate`]
    /// turns that error into a [`ScriptError`] for display.
    pub fn call_fn(
        &self,
        callback: &FnPtr,
        args: Vec<Dynamic>,
    ) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
        callback.call::<Dynamic>(self.host.engine(), &self.ast, args)
    }

    /// A script error located in this form's code file.
    pub fn locate(&self, error: &rhai::EvalAltResult) -> ScriptError {
        ScriptError::from_eval(self.host.file(), error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_names_are_snake_case() {
        assert_eq!(handler_name("hello_button", "Click"), "hello_button_click");
        assert_eq!(handler_name("agree_check", "Toggle"), "agree_check_toggle");
        assert_eq!(
            handler_name("items_list", "Activate"),
            "items_list_activate"
        );
        assert_eq!(handler_name(FORM, "Load"), "form_load");
        assert_eq!(handler_name(FORM, "Close"), "form_close");
        assert_eq!(
            handler_name("grid", "SelectionChanged"),
            "grid_selection_changed"
        );
    }

    #[test]
    fn a_missing_handler_is_not_bound() {
        let binder = ScriptBinder::new(
            "main_form",
            Rc::new(BTreeSet::from(["go_button_click".to_owned()])),
        );
        let spec = Catalog::xui()
            .get("Button")
            .and_then(|widget| widget.event("Click"))
            .cloned()
            .expect("Button has a Click event");

        let bound = binder.bind(EventRef {
            node: "go_button",
            event: "Click",
            spec: &spec,
        });
        assert!(bound.is_some(), "go_button_click is defined");

        let missing = binder.bind(EventRef {
            node: "other_button",
            event: "Click",
            spec: &spec,
        });
        assert!(missing.is_none(), "other_button_click is not defined");
    }

    #[test]
    fn script_functions_collects_every_definition() {
        let names = script_functions(
            "fn form_load() {}\nfn go_button_click(x) {}\nfn helper() {}",
            "main_form.rhai",
        )
        .expect("the script compiles");
        assert_eq!(names.len(), 3);
        assert!(names.contains("form_load"));
        assert!(names.contains("go_button_click"));
    }

    #[test]
    fn event_arguments_pad_and_truncate_to_the_handler_arity() {
        let args = [Value::Int(1), Value::Int(2)];
        assert!(
            event_arguments(0, &args).is_empty(),
            "no parameters keeps none"
        );
        let padded = event_arguments(3, &args);
        assert_eq!(padded.len(), 3);
        assert_eq!(padded[0].as_int(), Ok(1));
        assert_eq!(padded[1].as_int(), Ok(2));
        assert!(padded[2].is_unit(), "the extra parameter is unit");
    }
}
