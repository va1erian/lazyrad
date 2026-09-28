#![forbid(unsafe_code)]

//! Loading a project's forms and wiring VB-style `Control_Event` handlers.
//!
//! This is the runtime side of PLAN.md §3 and §4: [`FormRuntime`] loads every
//! form and standard module named by a [`Project`], and [`FormApp`] builds one
//! form into a live [`xui`](xui_core) window. The mapping from a schema control
//! type to an `xui` widget is [`xui_form`]'s job (its factories), so nothing
//! here is written per widget kind; this module only connects the pieces.
//!
//! # Event wiring
//!
//! The form's `.rhai` script is compiled once to learn the function names it
//! defines. [`ScriptBinder`] then answers, for each widget event, whether a
//! handler exists. It wires the event to a [`Msg::Event`] only when it does, so
//! a missing handler means the event is simply not wired and clicking does
//! nothing. [`FormApp::update`] runs the matching Rhai function through the
//! form's [`EngineHost`].
//!
//! VB event names differ from `xui`'s: a list's double-click is `xui`'s
//! `Activate`, a check box's click is its `Toggle`, and so on. [`vb_event_names`]
//! maps each `xui` event to the handler suffixes to look for.
//!
//! # Window events
//!
//! `Form_Load` runs once the form is built and `Form_Unload` runs when the
//! window is asked to close (the window then closes for real). The window spec
//! itself comes from [`Catalog::window_spec`](xui_form::Catalog::window_spec).
//!
//! # Standard modules
//!
//! Every [`ProjectItem::Module`](lazyrad_project::ProjectItem) is compiled and
//! registered as a Rhai module on each form's engine, so a form can either
//! `import "modUtil" as util` or call the module's functions directly.
//!
//! # Multiple forms
//!
//! A script calls `frmOther.show()` (or `frmOther.unload()`) on a form object;
//! [`FormRef`] records the request and [`FormApp`] opens a non-modal secondary
//! window with [`Ui::open_window`]. Modal display is not supported in Iteration
//! 1 (PLAN.md §10, G14).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rhai::{AST, Dynamic};
use xui_canvas::WinitBackend;
use xui_core::app::{App, Ui, WindowHandle, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;
use xui_form::{
    Binder, BuildOptions, Catalog, EventHandler, EventRef, Factories, FormDoc, LiveForm, Value,
    build_with,
};

use lazyrad_project::{Project, lazyrad_catalog, load_form};

use crate::control::FormHost;
use crate::engine::EngineHost;
use crate::error::ScriptError;

/// The queue of messages a script's form methods (`show`, `unload`) leave for
/// the running [`FormApp`] to act on.
type Pending = Rc<RefCell<Vec<Msg>>>;

/// A message the runtime application handles.
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    /// A widget event that a `<control>_<event>` handler is wired to.
    Event {
        /// The form the event belongs to.
        form: String,
        /// The control that raised it.
        control: String,
        /// The VB-style event name (`Click`, `Change`, `DblClick`, …).
        event: String,
        /// The event's arguments, converted from the widget's typed values.
        args: Vec<Value>,
    },
    /// `frmOther.show()`: open a secondary window for the named form.
    ShowForm(String),
    /// `frmOther.unload()`: close the named form's secondary window.
    CloseForm(String),
}

/// One form's source: its document, its code-behind and where the code lives.
#[derive(Clone, Debug)]
pub struct FormSource {
    /// The form name (the project item name and `[window].name`).
    pub name: String,
    /// The live form document.
    pub doc: FormDoc,
    /// The `.rhai` code-behind.
    pub code: String,
    /// The code file, for error locations (for example `frmMain.rhai`).
    pub code_file: String,
}

impl FormSource {
    /// Builds a form source, deriving the code file name from `name`.
    pub fn new(name: impl Into<String>, doc: FormDoc, code: impl Into<String>) -> FormSource {
        let name = name.into();
        FormSource {
            code_file: format!("{name}.rhai"),
            name,
            doc,
            code: code.into(),
        }
    }
}

/// One standard module's source.
#[derive(Clone, Debug)]
pub struct ModuleSource {
    /// The module name, also the Rhai namespace it is registered under.
    pub name: String,
    /// The `.rhai` source.
    pub source: String,
    /// The source file, for error locations.
    pub file: String,
}

impl ModuleSource {
    /// Builds a module source, deriving the file name from `name`.
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> ModuleSource {
        let name = name.into();
        ModuleSource {
            file: format!("{name}.rhai"),
            name,
            source: source.into(),
        }
    }
}

/// A failure to load or run a project.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// The project or one of its forms failed to load.
    #[error(transparent)]
    Project(#[from] lazyrad_project::Error),

    /// A form's widgets failed to build.
    #[error(transparent)]
    Build(#[from] xui_form::BuildError),

    /// The toolkit backend failed.
    #[error(transparent)]
    Backend(#[from] xui_core::backend::BackendError),

    /// A script failed to compile or run.
    #[error(transparent)]
    Script(#[from] ScriptError),

    /// A source file could not be read.
    #[error("cannot read `{}`: {source}", path.display())]
    Io {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying operating-system error.
        #[source]
        source: std::io::Error,
    },

    /// The startup item is not one of the project's forms.
    #[error("the startup item `{0}` is not a form in this project")]
    MissingStartup(String),

    /// A form name has no source in the runtime.
    #[error("the project has no form `{0}`")]
    UnknownForm(String),
}

/// Every form and module a project declares, plus what a running window needs.
///
/// The runtime is shared (`Rc`) by every open window: a form's script may ask a
/// different form to open, and the request travels through this type.
pub struct FormRuntime {
    project: Project,
    forms: BTreeMap<String, FormSource>,
    modules: Vec<ModuleSource>,
    catalog: Catalog,
    pending: Pending,
    opened: RefCell<BTreeSet<String>>,
    windows: RefCell<BTreeMap<String, WindowHandle<Msg>>>,
}

impl FormRuntime {
    /// Loads the project in `dir`: its `.lrp`, every form's `.lfm` and `.rhai`,
    /// and every standard module.
    pub fn load(dir: impl AsRef<Path>) -> Result<Rc<FormRuntime>, RuntimeError> {
        let dir = dir.as_ref();
        let project = Project::load(dir)?;
        let catalog = lazyrad_catalog();
        let mut forms = BTreeMap::new();
        let mut modules = Vec::new();

        for item in &project.items {
            let code_path = dir.join(item.code());
            let code = fs::read_to_string(&code_path).map_err(|source| RuntimeError::Io {
                path: code_path.clone(),
                source,
            })?;
            match item.layout() {
                Some(layout) => {
                    let doc = load_form(&dir.join(layout), &catalog)?;
                    forms.insert(
                        item.name().to_owned(),
                        FormSource {
                            name: item.name().to_owned(),
                            doc,
                            code,
                            code_file: item.code().display().to_string(),
                        },
                    );
                }
                None => modules.push(ModuleSource {
                    name: item.name().to_owned(),
                    source: code,
                    file: item.code().display().to_string(),
                }),
            }
        }

        Ok(Rc::new(FormRuntime {
            project,
            forms,
            modules,
            catalog,
            pending: Rc::new(RefCell::new(Vec::new())),
            opened: RefCell::new(BTreeSet::new()),
            windows: RefCell::new(BTreeMap::new()),
        }))
    }

    /// Builds a runtime from already-loaded sources, without touching the disk.
    ///
    /// This is the seam the IDE uses for a designer preview and the tests use
    /// for hand-written forms.
    pub fn from_sources(forms: Vec<FormSource>, modules: Vec<ModuleSource>) -> Rc<FormRuntime> {
        let mut by_name = BTreeMap::new();
        for form in forms {
            by_name.insert(form.name.clone(), form);
        }
        Rc::new(FormRuntime {
            project: Project::new("runtime"),
            forms: by_name,
            modules,
            catalog: lazyrad_catalog(),
            pending: Rc::new(RefCell::new(Vec::new())),
            opened: RefCell::new(BTreeSet::new()),
            windows: RefCell::new(BTreeMap::new()),
        })
    }

    /// The project this runtime was loaded from.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The catalog the forms are built against.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The form names, in sorted order.
    pub fn form_names(&self) -> impl Iterator<Item = &str> {
        self.forms.keys().map(String::as_str)
    }

    /// The form named `name`, if the project has it.
    pub fn form(&self, name: &str) -> Option<&FormSource> {
        self.forms.get(name)
    }

    /// The project's startup form name.
    pub fn startup_name(&self) -> Result<String, RuntimeError> {
        let name = &self.project.startup;
        if self.forms.contains_key(name) {
            Ok(name.clone())
        } else {
            Err(RuntimeError::MissingStartup(name.clone()))
        }
    }

    /// Whether a window for `name` is currently open.
    ///
    /// The startup window has no handle and is open for the process's life; a
    /// secondary window the user closed reports closed, so it can be reopened.
    pub fn is_open(&self, name: &str) -> bool {
        if !self.opened.borrow().contains(name) {
            return false;
        }
        self.windows
            .borrow()
            .get(name)
            .is_none_or(|handle| handle.is_open())
    }

    /// Builds `form` into `ui` and returns the application that drives it.
    pub fn build_app(
        self: &Rc<Self>,
        ui: &mut Ui<Msg>,
        form: &str,
    ) -> Result<FormApp, RuntimeError> {
        let source = self
            .forms
            .get(form)
            .ok_or_else(|| RuntimeError::UnknownForm(form.to_owned()))?;
        let root = Rc::new(FormInstance::build(ui, source, self)?);

        // `Form_Unload` runs in the close mapper and then the runtime performs
        // its normal close: a primary window quits the loop, a secondary one
        // does not. Intercepting the close with a message would skip that quit.
        let root_for_close = Rc::clone(&root);
        ui.on_close(move || {
            if let Err(error) = root_for_close.run("Form", "Unload", &[]) {
                eprintln!("lazyrad: {error}");
            }
            None
        });

        let app = FormApp {
            root: Some(Rc::clone(&root)),
            runtime: Rc::clone(self),
        };
        app.flush(ui);
        Ok(app)
    }

    /// Records that a secondary window for `name` was opened.
    fn mark_open(&self, name: &str) {
        self.opened.borrow_mut().insert(name.to_owned());
    }

    /// Forgets a secondary window for `name`.
    fn mark_closed(&self, name: &str) {
        self.opened.borrow_mut().remove(name);
    }
}

/// A form built and wired: the live widgets plus the script host.
///
/// All of its methods borrow it, so a [`FormApp`] shares one through an
/// [`Rc`]-and-calls it from its `update`.
pub struct FormInstance {
    name: String,
    form: Rc<LiveForm<Msg>>,
    host: EngineHost,
    ast: AST,
    /// Every function the script defines, mapped to its parameter count.
    functions: BTreeMap<String, usize>,
}

impl FormInstance {
    /// Builds `source`'s widgets, wires its handlers and runs `Form_Load`.
    fn build(
        ui: &mut Ui<Msg>,
        source: &FormSource,
        runtime: &Rc<FormRuntime>,
    ) -> Result<FormInstance, RuntimeError> {
        // Compile once to learn the handler names the binder must consult.
        let handler_names = script_functions(&source.code, &source.code_file)?;
        let binder = ScriptBinder {
            form: source.name.clone(),
            functions: Rc::new(handler_names),
        };

        let factories: Factories<Msg> = Factories::xui();
        let live = Rc::new(build_with(
            ui,
            &source.doc,
            &runtime.catalog,
            &factories,
            &binder,
            BuildOptions::default(),
        )?);

        let mut host = EngineHost::new(
            Rc::clone(&live) as Rc<dyn FormHost>,
            &runtime.catalog,
            &source.code_file,
        );
        for module in &runtime.modules {
            host.register_module(&module.name, &module.file, &module.source)?;
        }
        let ast = host.compile(&source.code)?;
        let functions = ast
            .iter_functions()
            .map(|function| (function.name.to_owned(), function.params.len()))
            .collect();
        register_form_refs(&mut host, runtime);

        let instance = FormInstance {
            name: source.name.clone(),
            form: live,
            host,
            ast,
            functions,
        };
        instance.run("Form", "Load", &[])?;
        Ok(instance)
    }

    /// The form's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The live widgets.
    pub fn live_form(&self) -> &Rc<LiveForm<Msg>> {
        &self.form
    }

    /// Runs the handler for `control`'s `event`, if the script defines one.
    ///
    /// A missing handler is not an error: the event is simply ignored. Window
    /// events pass `control = "Form"`, so `Load` maps to `Form_Load`.
    pub fn run(&self, control: &str, event: &str, args: &[Value]) -> Result<(), ScriptError> {
        let function = if control == "Form" {
            format!("Form_{event}")
        } else {
            format!("{control}_{event}")
        };
        let Some(&arity) = self.functions.get(&function) else {
            return Ok(());
        };
        let arguments: Vec<Dynamic> = args
            .iter()
            .take(arity)
            .map(crate::value::to_dynamic)
            .collect();
        let _ = self.host.call_with(&self.ast, &function, arguments)?;
        Ok(())
    }
}

/// The application that owns one window's form and routes its messages.
pub struct FormApp {
    root: Option<Rc<FormInstance>>,
    runtime: Rc<FormRuntime>,
}

impl FormApp {
    /// The live form this window drives, when it built successfully.
    pub fn root_form(&self) -> Option<&Rc<LiveForm<Msg>>> {
        self.root.as_ref().map(|instance| instance.live_form())
    }

    /// Moves every message a script left pending into the window's queue.
    fn flush(&self, ui: &mut Ui<Msg>) {
        let pending: Vec<Msg> = self.runtime.pending.borrow_mut().drain(..).collect();
        for msg in pending {
            ui.emit(msg);
        }
    }

    /// Opens a non-modal secondary window for `name`, if it is not already open.
    fn show_form(&self, name: &str, ui: &mut Ui<Msg>) {
        if self.runtime.is_open(name) {
            return;
        }
        let Some(source) = self.runtime.form(name).cloned() else {
            eprintln!("lazyrad: the project has no form `{name}`");
            return;
        };
        let spec = window_spec(&source.doc);
        let runtime = Rc::clone(&self.runtime);
        let form_name = name.to_owned();
        let result =
            ui.open_window::<FormApp, _>(spec, move |ui| match runtime.build_app(ui, &form_name) {
                Ok(app) => app,
                Err(error) => {
                    eprintln!("lazyrad: cannot open `{form_name}`: {error}");
                    FormApp {
                        root: None,
                        runtime: Rc::clone(&runtime),
                    }
                }
            });
        match result {
            Ok(handle) => {
                self.runtime.mark_open(name);
                self.runtime
                    .windows
                    .borrow_mut()
                    .insert(name.to_owned(), handle);
            }
            Err(error) => eprintln!("lazyrad: cannot open `{name}`: {error}"),
        }
    }

    /// Closes a secondary window for `name`.
    fn close_form(&self, name: &str) {
        if let Some(handle) = self.runtime.windows.borrow_mut().remove(name) {
            handle.close();
        }
        self.runtime.mark_closed(name);
    }
}

impl App for FormApp {
    type Msg = Msg;

    fn update(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
        let Some(root) = self.root.clone() else {
            ui.quit();
            return;
        };
        match msg {
            Msg::Event {
                form,
                control,
                event,
                args,
            } => {
                if form.as_str() == root.name()
                    && let Err(error) = root.run(&control, &event, &args)
                {
                    eprintln!("lazyrad: {error}");
                }
                self.flush(ui);
            }
            Msg::ShowForm(name) => {
                self.show_form(&name, ui);
                self.flush(ui);
            }
            Msg::CloseForm(name) => {
                self.close_form(&name);
                self.flush(ui);
            }
        }
    }
}

/// The `frmOther` object a script calls `show`/`unload` on.
#[derive(Clone)]
pub struct FormRef {
    name: String,
    pending: Pending,
}

impl FormRef {
    /// A reference to the form named `name`.
    fn new(name: String, pending: Pending) -> FormRef {
        FormRef { name, pending }
    }

    /// Requests that the form's window be opened.
    fn show(&mut self) {
        self.pending
            .borrow_mut()
            .push(Msg::ShowForm(self.name.clone()));
    }

    /// Requests that the form's window be closed.
    fn unload(&mut self) {
        self.pending
            .borrow_mut()
            .push(Msg::CloseForm(self.name.clone()));
    }
}

/// Registers the [`FormRef`] type and exposes every form name as a global.
fn register_form_refs(host: &mut EngineHost, runtime: &Rc<FormRuntime>) {
    host.engine_mut()
        .register_type_with_name::<FormRef>("FormRef");
    host.engine_mut()
        .register_fn("show", |reference: &mut FormRef| reference.show());
    host.engine_mut()
        .register_fn("unload", |reference: &mut FormRef| reference.unload());
    for name in runtime.forms.keys() {
        host.set_global(
            name.clone(),
            Dynamic::from(FormRef::new(name.clone(), Rc::clone(&runtime.pending))),
        );
    }
}

/// Decides which widget events become [`Msg::Event`]s.
///
/// An event is wired only when the form's script defines the matching function,
/// so a missing handler is a silent no-op (the `xui-form` binder contract).
struct ScriptBinder {
    form: String,
    functions: Rc<BTreeSet<String>>,
}

impl Binder<Msg> for ScriptBinder {
    fn bind(&self, event: EventRef<'_>) -> Option<EventHandler<Msg>> {
        let candidates = vb_event_names(event.event);
        let event_name = candidates
            .iter()
            .find(|candidate| {
                self.functions
                    .contains(&format!("{}_{candidate}", event.node))
            })
            .map(|candidate| (*candidate).to_owned())?;

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

/// The VB-style handler suffixes to look for, given an `xui` event name.
///
/// The first match wins, so `Click` is preferred where a VB control's click is
/// the common spelling and an explicit `Toggle`/`Activate` name is a fallback.
fn vb_event_names(event: &str) -> Vec<&str> {
    match event {
        "Click" => vec!["Click"],
        "Change" => vec!["Change"],
        "Commit" => vec!["Change", "Commit"],
        "Toggle" => vec!["Click", "Toggle"],
        "Select" => vec!["Click", "Select"],
        "Activate" => vec!["DblClick", "Activate"],
        other => vec![other],
    }
}

/// The function names `source` defines, or a located parse error.
fn script_functions(source: &str, file: &str) -> Result<BTreeSet<String>, ScriptError> {
    let engine = crate::new_engine();
    let ast = engine
        .compile(source)
        .map_err(|error| ScriptError::from_parse(file, &error))?;
    Ok(ast
        .iter_functions()
        .map(|function| function.name.to_owned())
        .collect())
}

/// The toolkit window spec for a form document.
fn window_spec(doc: &FormDoc) -> PlatformSpec {
    let title = doc
        .window
        .prop("title")
        .and_then(Value::as_str)
        .unwrap_or(&doc.window.name);
    let width = doc
        .window
        .prop("width")
        .and_then(Value::as_int)
        .unwrap_or(320) as f32;
    let height = doc
        .window
        .prop("height")
        .and_then(Value::as_int)
        .unwrap_or(200) as f32;
    PlatformSpec::new(title).size(Dip(width), Dip(height))
}

/// Runs the project in `dir` on the portable `winit` backend.
pub fn run_project(dir: impl AsRef<Path>) -> Result<(), RuntimeError> {
    let backend: Rc<dyn Backend> = Rc::new(WinitBackend::new());
    run_project_with(backend, dir)
}

/// Runs the project in `dir` on `backend`, opening its startup form.
///
/// The backend is passed in so the same path runs on `winit` and, in tests, on
/// the offscreen backend.
pub fn run_project_with(
    backend: Rc<dyn Backend>,
    dir: impl AsRef<Path>,
) -> Result<(), RuntimeError> {
    let runtime = FormRuntime::load(dir)?;
    let startup = runtime.startup_name()?;
    let doc = runtime
        .form(&startup)
        .map(|source| source.doc.clone())
        .ok_or_else(|| RuntimeError::UnknownForm(startup.clone()))?;
    let spec = window_spec(&doc);

    let runtime_for_app = Rc::clone(&runtime);
    run_app(backend, spec, move |ui| {
        match runtime_for_app.build_app(ui, &startup) {
            Ok(app) => {
                runtime_for_app.mark_open(&startup);
                app
            }
            Err(error) => {
                eprintln!("lazyrad: cannot open `{startup}`: {error}");
                ui.quit_with(1);
                FormApp {
                    root: None,
                    runtime: Rc::clone(&runtime_for_app),
                }
            }
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vb_event_names_map_to_vb_spellings() {
        assert_eq!(vb_event_names("Click"), vec!["Click"]);
        assert_eq!(vb_event_names("Change"), vec!["Change"]);
        assert_eq!(vb_event_names("Toggle"), vec!["Click", "Toggle"]);
        assert_eq!(vb_event_names("Activate"), vec!["DblClick", "Activate"]);
        assert_eq!(vb_event_names("Select"), vec!["Click", "Select"]);
        assert_eq!(vb_event_names("Custom"), vec!["Custom"]);
    }

    #[test]
    fn a_missing_handler_is_not_bound() {
        let binder = ScriptBinder {
            form: "frmMain".to_owned(),
            functions: Rc::new(BTreeSet::from(["cmdGo_Click".to_owned()])),
        };
        let spec = Catalog::xui()
            .get("Button")
            .and_then(|widget| widget.event("Click"))
            .cloned()
            .expect("Button has a Click event");

        let bound = binder.bind(EventRef {
            node: "cmdGo",
            event: "Click",
            spec: &spec,
        });
        assert!(bound.is_some(), "cmdGo_Click is defined");

        let missing = binder.bind(EventRef {
            node: "cmdOther",
            event: "Click",
            spec: &spec,
        });
        assert!(missing.is_none(), "cmdOther_Click is not defined");
    }

    #[test]
    fn script_functions_collects_every_definition() {
        let names = script_functions(
            "fn Form_Load() {}\nfn cmdGo_Click(x) {}\nfn helper() {}",
            "frmMain.rhai",
        )
        .expect("the script compiles");
        assert_eq!(names.len(), 3);
        assert!(names.contains("Form_Load"));
        assert!(names.contains("cmdGo_Click"));
    }
}
