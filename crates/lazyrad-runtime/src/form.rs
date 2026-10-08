#![forbid(unsafe_code)]

//! Loading a project's forms and running them.
//!
//! This is the project side of PLAN.md §3 and §4: [`FormRuntime`] loads every
//! form and standard module named by a [`Project`], and [`FormApp`] builds one
//! form into a live [`xui`](xui_core) window and routes its messages. The
//! reusable "script an `xui-form` with Rhai" machinery — the Rhai engine host,
//! the `<control>_<event>` binding, the scripted form and its window events —
//! lives in [`xui_rhai`] and is re-exported through [`crate::engine`] and this
//! module.
//!
//! # Event wiring
//!
//! A handler's name is the control name and `xui`'s event name in snake_case,
//! joined by `_` ([`handler_name`]): `hello_button_click`, `name_edit_change`,
//! `agree_check_toggle` (PLAN.md §1.1). An event is wired only when the script
//! defines the matching function, so clicking a control with no handler does
//! nothing. [`FormInstance::run`] runs the matching Rhai function through the
//! form's engine host.
//!
//! # Window events
//!
//! `form_load` runs once the form is built and `form_close` runs when the
//! window is asked to close (the window then closes for real). On a hot reload
//! (issue #91) `form_load` runs again on the rebuilt form and, when the script
//! defines `fn form_reload(old_state)`, it is called afterwards with the old
//! form's `form.state`. The window spec itself comes from
//! [`Catalog::window_spec`](xui_form::Catalog::window_spec).
//!
//! # Standard modules
//!
//! Every [`ProjectItem::Module`](lazyrad_project::ProjectItem) is compiled and
//! registered as a Rhai module on each form's engine, so a form can either
//! `import "util" as util` or call the module's functions directly.
//!
//! # Multiple forms
//!
//! A script calls `other_form.show()` (or `other_form.unload()`) on a form object;
//! [`FormRef`] records the request and [`FormApp`] opens a non-modal secondary
//! window with [`Ui::open_window`]. Modal display is not supported in Iteration
//! 1 (PLAN.md §10, G14).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rhai::{Dynamic, FnPtr};
#[cfg(feature = "desktop")]
use xui_canvas::WinitBackend;
use xui_core::app::{App, Ui, WindowHandle, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;
use xui_core::{Dialog, DialogAction};
use xui_form::{
    Binder, BuildOptions, Catalog, EventHandler, EventRef, Factories, FormDoc, LiveForm, Value,
    build_with,
};

use lazyrad_project::{Node, Project, lazyrad_catalog, parse_form};

use crate::events::Poller;
use crate::fs_policy::FsPolicy;
use crate::platform;
use crate::reload::{ReloadOutcome, WATCH_INTERVAL_MS};
use crate::stdlib::StdlibContext;
use xui_rhai::form::{FormError, ScriptForm, ScriptSource};
use xui_rhai::message::Pending;
use xui_rhai::{EngineHost, ScriptError};

pub use xui_rhai::form::{FORM, handler_name};
pub use xui_rhai::message::{Msg, MsgBoxButtons};

/// One form's source: its document, its code-behind and where the code lives.
#[derive(Clone, Debug)]
pub struct FormSource {
    /// The form name (the project item name and `[window].name`).
    pub name: String,
    /// The live form document.
    pub doc: FormDoc,
    /// The `.rhai` code-behind.
    pub code: String,
    /// The code file, for error locations (for example `main_form.rhai`).
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

/// The project's reloadable content: its `.lrp` model, its forms and its
/// modules.
///
/// A hot reload (issue #91) swaps this whole set for a freshly loaded one, so
/// every accessor returns an owned clone rather than borrowing the map.
#[derive(Clone, Debug)]
pub(crate) struct Sources {
    pub(crate) project: Project,
    pub(crate) forms: BTreeMap<String, FormSource>,
    pub(crate) modules: Vec<ModuleSource>,
}

/// Every form and module a project declares, plus what a running window needs.
///
/// The runtime is shared (`Rc`) by every open window: a form's script may ask a
/// different form to open, and the request travels through this type.
pub struct FormRuntime {
    pub(crate) sources: RefCell<Sources>,
    catalog: Catalog,
    pending: Pending,
    /// The project directory, shown to scripts as `App.path`.
    path: PathBuf,
    opened: RefCell<BTreeSet<String>>,
    windows: RefCell<BTreeMap<String, WindowHandle<Msg>>>,
    /// Each built form's window, so a message addressed to a form (a message
    /// box and its result) reaches that window whichever window flushes it.
    inboxes: RefCell<BTreeMap<String, Ui<Msg>>>,
    /// Which paths scripts may touch, fixed when the runtime is built from
    /// the installed platform's policy.
    fs: Rc<FsPolicy>,
    /// Told after every event handler that ran without error.
    observer: RefCell<Option<HandlerObserver>>,
    /// Told after every handler, callback or poll that failed.
    error_observer: RefCell<Option<ErrorObserver>>,
    /// Told whenever the application opens a message box.
    msg_box_observer: RefCell<Option<MsgBoxObserver>>,
    /// The hot-reload watcher, when the player was started with `--watch`.
    pub(crate) watch: RefCell<Option<crate::reload::Watcher>>,
}

/// A callback told `(form, control, event)` after an event handler ran without
/// error. The LazyOS player uses it to print its `LRPLAY:EVENT:PASS` serial
/// marker; any host can use it for telemetry or tests.
pub type HandlerObserver = Rc<dyn Fn(&str, &str, &str)>;

/// A callback told `(form, control, event, error)` whenever a handler, a
/// message-box callback or a poll fails. The LazyOS player uses it to print its
/// `LRPLAY:SCRIPTERR` serial marker; a host's test kit uses it to count
/// failures.
///
/// The event name is `"callback"` for a message-box callback and `"poll"` for
/// an event source; both pass `""` as the control.
pub type ErrorObserver = Rc<dyn Fn(&str, &str, &str, &ScriptError)>;

/// A callback told `(form, text, title)` whenever a message box is opened,
/// whether by a script's `msg_box` or by a failed handler. A host's test kit
/// records them instead of blocking.
pub type MsgBoxObserver = Rc<dyn Fn(&str, &str, &str)>;

impl FormRuntime {
    /// Installs the observer told after each successful event handler,
    /// replacing any earlier one.
    pub fn set_handler_observer(&self, observer: HandlerObserver) {
        *self.observer.borrow_mut() = Some(observer);
    }

    /// Installs the observer told after each failed handler, callback or poll,
    /// replacing any earlier one.
    pub fn set_error_observer(&self, observer: ErrorObserver) {
        *self.error_observer.borrow_mut() = Some(observer);
    }

    /// Installs the observer told whenever a message box is opened, replacing
    /// any earlier one.
    pub fn set_msg_box_observer(&self, observer: MsgBoxObserver) {
        *self.msg_box_observer.borrow_mut() = Some(observer);
    }

    /// Tells the observer, if any, that a handler ran.
    fn notify_handler(&self, form: &str, control: &str, event: &str) {
        // Cloned out so an observer that re-enters the runtime cannot hit a
        // held borrow.
        let observer = self.observer.borrow().clone();
        if let Some(observer) = observer {
            observer(form, control, event);
        }
    }

    /// Tells the observer, if any, that a handler, callback or poll failed.
    fn notify_error(&self, form: &str, control: &str, event: &str, error: &ScriptError) {
        // Cloned out for the same re-entrancy reason as `notify_handler`.
        let observer = self.error_observer.borrow().clone();
        if let Some(observer) = observer {
            observer(form, control, event, error);
        }
    }

    /// Tells the observer, if any, that a message box was opened.
    fn notify_msg_box(&self, form: &str, text: &str, title: &str) {
        let observer = self.msg_box_observer.borrow().clone();
        if let Some(observer) = observer {
            observer(form, text, title);
        }
    }

    /// Loads the project in `dir`: its `.lrp`, every form's `.lfm` and `.rhai`,
    /// and every standard module.
    pub fn load(dir: impl AsRef<Path>) -> Result<Rc<FormRuntime>, RuntimeError> {
        Self::load_path(dir)
    }

    /// Loads the project named by `path`, which may be a project directory or an
    /// `.lrp` file.
    ///
    /// A directory case loads its single `.lrp` ([`Project::load`]); an `.lrp`
    /// file is loaded directly and its parent becomes the project directory
    /// (the value scripts see as `app.path`).
    pub fn load_path(path: impl AsRef<Path>) -> Result<Rc<FormRuntime>, RuntimeError> {
        let (dir, project) = open_project(path.as_ref())?;
        let root = dir.clone();
        Self::from_project(project, dir, |relative| {
            fs::read_to_string(root.join(relative))
        })
    }

    /// Builds a runtime from a project and a way to read its files, without
    /// assuming they live on disk.
    ///
    /// `read` returns the text of a file named by an item path (`main.lfm`,
    /// `main.rhai`); `dir` is only what scripts see as `app.path`. The player
    /// uses this to run an exported executable's payload from memory, and
    /// [`FormRuntime::load_path`] uses it to read from the project folder, so
    /// both take exactly the same route.
    pub fn from_project(
        project: Project,
        dir: PathBuf,
        read: impl Fn(&Path) -> std::io::Result<String>,
    ) -> Result<Rc<FormRuntime>, RuntimeError> {
        let catalog = lazyrad_catalog();
        let mut forms = BTreeMap::new();
        let mut modules = Vec::new();

        for item in &project.items {
            let code = read(item.code()).map_err(|source| RuntimeError::Io {
                path: dir.join(item.code()),
                source,
            })?;
            match item.layout() {
                Some(layout) => {
                    let text = read(layout).map_err(|source| RuntimeError::Io {
                        path: dir.join(layout),
                        source,
                    })?;
                    let doc = parse_form(&dir.join(layout), &text, &catalog)?;
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
            sources: RefCell::new(Sources {
                project,
                forms,
                modules,
            }),
            catalog,
            pending: Rc::new(RefCell::new(Vec::new())),
            path: dir,
            opened: RefCell::new(BTreeSet::new()),
            windows: RefCell::new(BTreeMap::new()),
            inboxes: RefCell::new(BTreeMap::new()),
            fs: Rc::new(platform::current().fs_policy()),
            observer: RefCell::new(None),
            error_observer: RefCell::new(None),
            msg_box_observer: RefCell::new(None),
            watch: RefCell::new(None),
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
            sources: RefCell::new(Sources {
                project: Project::new("runtime"),
                forms: by_name,
                modules,
            }),
            catalog: lazyrad_catalog(),
            pending: Rc::new(RefCell::new(Vec::new())),
            path: PathBuf::from("."),
            opened: RefCell::new(BTreeSet::new()),
            windows: RefCell::new(BTreeMap::new()),
            inboxes: RefCell::new(BTreeMap::new()),
            fs: Rc::new(platform::current().fs_policy()),
            observer: RefCell::new(None),
            error_observer: RefCell::new(None),
            msg_box_observer: RefCell::new(None),
            watch: RefCell::new(None),
        })
    }

    /// The project this runtime was loaded from.
    pub fn project(&self) -> Project {
        self.sources.borrow().project.clone()
    }

    /// The catalog the forms are built against.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The project directory, exposed to scripts as `App.path`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The form names, in sorted order.
    pub fn form_names(&self) -> Vec<String> {
        self.sources.borrow().forms.keys().cloned().collect()
    }

    /// The form named `name`, if the project has it.
    pub fn form(&self, name: &str) -> Option<FormSource> {
        self.sources.borrow().forms.get(name).cloned()
    }

    /// The project's startup form name.
    pub fn startup_name(&self) -> Result<String, RuntimeError> {
        let sources = self.sources.borrow();
        let name = &sources.project.startup;
        if sources.forms.contains_key(name) {
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

    /// Sends `msg` to every open form window, so a reload rebuilds each one.
    pub(crate) fn broadcast(&self, msg: Msg) {
        for ui in self.inboxes.borrow().values() {
            ui.emit(msg.clone());
        }
    }

    /// Builds `form` into `ui` and returns the application that drives it.
    pub fn build_app(
        self: &Rc<Self>,
        ui: &mut Ui<Msg>,
        form: &str,
    ) -> Result<FormApp, RuntimeError> {
        let source = self
            .form(form)
            .ok_or_else(|| RuntimeError::UnknownForm(form.to_owned()))?;
        let root = Rc::new(FormInstance::build(ui, &source, self)?);

        // `form_close` runs in the close mapper and then the runtime performs
        // its normal close: a primary window quits the loop, a secondary one
        // does not. Intercepting the close with a message would skip that quit.
        install_close_handler(&root, ui);

        self.inboxes
            .borrow_mut()
            .insert(form.to_owned(), ui.clone());
        // Only the startup window drives the project-wide watch; the others are
        // rebuilt when it broadcasts `Msg::Reload`.
        let watching = self.watch_enabled() && self.startup_name().ok().as_deref() == Some(form);
        if watching {
            ui.every(WATCH_INTERVAL_MS, Msg::WatchTick);
        }
        let mut app = FormApp {
            form: form.to_owned(),
            root: Some(Rc::clone(&root)),
            runtime: Rc::clone(self),
            dialogs: Vec::new(),
            poller: Poller::new(ui, form),
            banner: None,
            generation: 0,
            watching,
        };
        app.flush(ui);
        // `form_load` may already have subscribed to something.
        app.poller.sync(ui);
        Ok(app)
    }

    /// Records that a secondary window for `name` was opened.
    fn mark_open(&self, name: &str) {
        self.opened.borrow_mut().insert(name.to_owned());
    }

    /// Forgets a secondary window for `name`.
    fn mark_closed(&self, name: &str) {
        self.opened.borrow_mut().remove(name);
        self.inboxes.borrow_mut().remove(name);
    }
}

/// A form built and wired: the live widgets plus the script host.
///
/// It is a thin wrapper over [`xui_rhai::form::ScriptForm`] that registers the
/// project's standard modules and its form references before the script is
/// compiled. All of its methods borrow it, so a [`FormApp`] shares one through
/// an [`Rc`]-and-calls it from its `update`.
pub struct FormInstance {
    script: ScriptForm,
}

impl FormInstance {
    /// Builds `source`'s widgets, wires its handlers and runs `form_load`.
    fn build(
        ui: &mut Ui<Msg>,
        source: &FormSource,
        runtime: &Rc<FormRuntime>,
    ) -> Result<FormInstance, RuntimeError> {
        let instance = FormInstance::build_deferred(ui, source, runtime)?;
        instance.load()?;
        Ok(instance)
    }

    /// Builds `source`'s widgets and compiles its script, but does not run
    /// `form_load` yet.
    ///
    /// A hot reload (issue #91) builds the new form this way first, so a build
    /// failure leaves the running form untouched; it then releases the old
    /// form's event sources and calls [`FormInstance::load`].
    fn build_deferred(
        ui: &mut Ui<Msg>,
        source: &FormSource,
        runtime: &Rc<FormRuntime>,
    ) -> Result<FormInstance, RuntimeError> {
        let stdlib = StdlibContext {
            form: source.name.clone(),
            pending: Rc::clone(&runtime.pending),
            app_title: runtime.project().name.clone(),
            app_path: runtime.path().display().to_string(),
            fs: Rc::clone(&runtime.fs),
        };
        let runtime_for_setup = Rc::clone(runtime);
        let script = ScriptForm::build_deferred(
            ui,
            &source.doc,
            &runtime.catalog,
            ScriptSource {
                name: &source.name,
                code: &source.code,
                file: &source.code_file,
            },
            stdlib,
            move |host| {
                // Cloned out so a module's top-level code cannot hit a held
                // borrow of the runtime's sources.
                let modules = runtime_for_setup.sources.borrow().modules.clone();
                for module in &modules {
                    host.register_module(&module.name, &module.file, &module.source)?;
                }
                register_form_refs(host, &runtime_for_setup);
                Ok(())
            },
        )
        .map_err(|error| match error {
            FormError::Build(error) => RuntimeError::Build(error),
            FormError::Script(error) => RuntimeError::Script(error),
        })?;

        Ok(FormInstance { script })
    }

    /// Runs the form's `form_load` handler.
    fn load(&self) -> Result<(), ScriptError> {
        self.script.load()
    }

    /// The form's `form.state` map, read before a reload replaces the form.
    fn state(&self) -> rhai::Map {
        self.script.state()
    }

    /// Runs `form_reload(old_state)` after a reload, if the script defines it.
    fn reload(&self, old_state: rhai::Map) -> Result<(), ScriptError> {
        self.script.reload(old_state)
    }

    /// Calls a function pointer the script handed to a host extension (a
    /// Messenger event handler, say) with `args`, returning the engine's own
    /// error so the extension can see what was thrown.
    pub fn call_fn(
        &self,
        callback: &FnPtr,
        args: Vec<Dynamic>,
    ) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
        self.script.call_fn(callback, args)
    }

    /// An engine error located in this form's code, for display.
    pub fn locate(&self, error: &rhai::EvalAltResult) -> ScriptError {
        self.script.locate(error)
    }

    /// The form's name.
    pub fn name(&self) -> &str {
        self.script.name()
    }

    /// The live widgets.
    pub fn live_form(&self) -> &Rc<LiveForm<Msg>> {
        self.script.live_form()
    }

    /// Runs the handler for `control`'s `event`, if the script defines one.
    ///
    /// A missing handler is not an error: the event is simply ignored. Window
    /// events pass [`FORM`] as the control, so `Load` maps to `form_load`.
    pub fn run(&self, control: &str, event: &str, args: &[Value]) -> Result<(), ScriptError> {
        self.script.run(control, event, args)
    }

    /// Runs the `form_close` handler, if the script defines one.
    pub fn close(&self) -> Result<(), ScriptError> {
        self.script.close()
    }

    /// Calls a Rhai function pointer the form's script handed to the runtime.
    ///
    /// This is how a non-blocking [`Msg::MsgBox`] still reports its result: the
    /// application stores the callback, then calls it here once the dialog
    /// closes. The callback is looked up in the form's compiled AST, so a
    /// script-defined function or a closure both work.
    pub fn call_callback(&self, callback: &FnPtr, result: &str) -> Result<(), ScriptError> {
        self.script.call_callback(callback, result)
    }
}

/// The application that owns one window's form and routes its messages.
pub struct FormApp {
    /// The form this window drives, kept even when [`FormApp::root`] is `None`.
    form: String,
    root: Option<Rc<FormInstance>>,
    runtime: Rc<FormRuntime>,
    /// Every open message box. A [`Dialog`] destroys its nodes when dropped, so
    /// the application keeps each one alive until it closes.
    dialogs: Vec<Dialog<Msg>>,
    /// Polls the host's event sources while the form has work pending, and
    /// releases what the form registered when the window goes away.
    poller: Poller,
    /// The hot-reload error banner strip, shown while a reload failed.
    banner: Option<Banner>,
    /// Bumped on every reload, so a message box result for the old form is
    /// dropped rather than run against the new script (checklist 1).
    generation: u64,
    /// Whether this window drives the project's hot-reload polling (the
    /// startup window does; secondary windows are rebuilt on `Msg::Reload`).
    watching: bool,
}

/// The label strip a failed reload shows, kept alive so its nodes live.
struct Banner {
    strip: LiveForm<Msg>,
    text: String,
}

/// The name of the node inside the banner's form document.
const BANNER_NODE: &str = "reload_banner";

impl FormApp {
    /// A placeholder app for a window whose form could not be built: it owns no
    /// form, so updating it quits the loop.
    pub(crate) fn failed(runtime: Rc<FormRuntime>) -> FormApp {
        FormApp::empty(runtime, String::new())
    }

    /// The live form this window drives, when it built successfully.
    pub fn root_form(&self) -> Option<&Rc<LiveForm<Msg>>> {
        self.root.as_ref().map(|instance| instance.live_form())
    }

    /// The form this window drives, even when its build failed.
    pub fn form_name(&self) -> &str {
        &self.form
    }

    /// An application for a window whose form could not be built: it holds the
    /// form name so a reload can try again, but drives no form.
    fn empty(runtime: Rc<FormRuntime>, form: String) -> FormApp {
        FormApp {
            form,
            root: None,
            runtime,
            dialogs: Vec::new(),
            poller: Poller::idle(),
            banner: None,
            generation: 0,
            watching: false,
        }
    }

    /// The hot-reload diagnostics currently shown in this window's banner, if
    /// any. Cleared by the next successful reload.
    pub fn banner_text(&self) -> Option<&str> {
        self.banner.as_ref().map(|banner| banner.text.as_str())
    }

    /// Whether the window is polling the host's event sources for its form.
    pub fn is_polling(&self) -> bool {
        self.poller.is_polling()
    }

    /// Rebuilds this window's form from the runtime's current sources (issue
    /// #91).
    ///
    /// The new form is built **before** the old one is touched, so a build
    /// failure keeps the running form and shows the error in the banner
    /// (checklist 3). On success the old form's event sources are released
    /// through the existing [`EventSource::release`](crate::extensions::EventSource)
    /// path, its timer is stopped, its open message boxes are dropped and its
    /// pending callbacks are invalidated; then the new `form_load` and, when
    /// the script defines it, `form_reload(old_state)` run.
    ///
    /// A form the project no longer declares closes a secondary window and
    /// leaves the startup form untouched. Returns whether the form was rebuilt.
    pub fn reload_root(&mut self, ui: &mut Ui<Msg>) -> bool {
        let Some(source) = self.runtime.form(&self.form) else {
            // The project no longer declares this form. A secondary window
            // closes; the startup window (the one that drives the watch) keeps
            // its last good build.
            if !self.watching {
                ui.close();
            }
            return false;
        };
        let old_state = self
            .root
            .as_ref()
            .map_or_else(rhai::Map::new, |root| root.state());
        let new_root = match FormInstance::build_deferred(ui, &source, &self.runtime) {
            Ok(instance) => Rc::new(instance),
            Err(error) => {
                self.show_banner(ui, &[error.to_string()]);
                return false;
            }
        };
        // Release the old subscriptions and stop its timer before the new
        // `form_load` runs: `release` is keyed by form name, so doing it after
        // would drop the new subscriptions too.
        let old_poller = std::mem::replace(&mut self.poller, Poller::idle());
        old_poller.release(ui);
        // A box opened for the old instance must not call the new script.
        self.generation += 1;
        self.dialogs.clear();
        self.root = Some(Rc::clone(&new_root));
        self.clear_banner();
        // The window's close handler must run the new form's `form_close`, not
        // the dropped old instance's (checklist 1).
        install_close_handler(&new_root, ui);
        if let Err(error) = new_root.load() {
            self.show_banner(ui, &[error.to_string()]);
        } else if let Err(error) = new_root.reload(old_state) {
            self.show_banner(ui, &[error.to_string()]);
        }
        self.poller = Poller::new(ui, &self.form);
        self.poller.sync(ui);
        true
    }

    /// Shows (or updates) the reload banner with `diagnostics`.
    fn show_banner(&mut self, ui: &Ui<Msg>, diagnostics: &[String]) {
        let text = format_diagnostics(diagnostics);
        if let Some(banner) = &mut self.banner {
            let _ = banner
                .strip
                .set(BANNER_NODE, "text", &Value::Text(text.clone()));
            banner.text = text;
            return;
        }
        if let Some(strip) = build_banner(ui, self.runtime.catalog(), &text) {
            self.banner = Some(Banner { strip, text });
        }
    }

    /// Removes the reload banner, if one is shown.
    fn clear_banner(&mut self) {
        self.banner = None;
    }

    /// Moves every message a script left pending into the right window's queue.
    ///
    /// The queue is shared by every window, so a message addressed to a form
    /// (a message box, its result) goes to that form's window; the rest are
    /// handled here.
    fn flush(&self, ui: &mut Ui<Msg>) {
        let pending: Vec<Msg> = self.runtime.pending.borrow_mut().drain(..).collect();
        let own = self.root.as_ref().map(|root| root.name().to_owned());
        for msg in pending {
            match addressee(&msg) {
                Some(form) if Some(form) != own.as_deref() => {
                    match self.runtime.inboxes.borrow().get(form) {
                        Some(target) => target.emit(msg),
                        None => eprintln!("lazyrad: form `{form}` is not open; message dropped"),
                    }
                }
                _ => ui.emit(msg),
            }
        }
    }

    /// Opens a non-modal secondary window for `name`, if it is not already open.
    fn show_form(&self, name: &str, ui: &mut Ui<Msg>) {
        if self.runtime.is_open(name) {
            return;
        }
        let Some(source) = self.runtime.form(name) else {
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
                    FormApp::empty(Rc::clone(&runtime), form_name)
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

    /// Reports a handler's runtime error without stopping the program.
    ///
    /// A widget handler's failure is not fatal: the error is shown in a
    /// message box (the script's own `msg_box`, non-blocking like any other)
    /// and the application keeps running, so the player's exit code stays 0.
    /// Only a failure that prevents the window from opening is fatal. A
    /// `Canvas`'s failing `Frame` handler also stops that canvas's frame loop
    /// (its `fps` becomes 0), so the error is shown once rather than every
    /// frame; the script can set `fps` again to restart it.
    ///
    /// `control` and `event` name what failed, for the
    /// [`ErrorObserver`](FormRuntime::set_error_observer): a widget event uses
    /// the real control and event, a message-box callback uses `""` and
    /// `"callback"`, and an event source poll uses `""` and `"poll"`.
    fn report_handler_error(
        &mut self,
        ui: &mut Ui<Msg>,
        form: &str,
        control: &str,
        event: &str,
        error: ScriptError,
    ) {
        self.runtime.notify_error(form, control, event, &error);
        self.open_msg_box(
            ui,
            form,
            &error.to_string(),
            "LazyRAD",
            MsgBoxButtons::Ok,
            None,
        );
    }

    /// Shows a non-blocking message box in the window.
    ///
    /// The dialog's shape follows `buttons`. Its action is turned into a
    /// [`Msg::MsgBoxResult`] naming the button pressed and, when the script
    /// supplied one, the callback to run; the application routes that message
    /// back through [`FormApp::update`] rather than calling the script here.
    ///
    /// The dialog is kept in [`FormApp::dialogs`] so it lives until it closes.
    ///
    /// A blocking `msg_box` would need `Ui::open_modal`, which the canvas backend
    /// does not implement (PLAN.md §10, G14; va1erian/xui#146), so Iteration 1
    /// is deliberately asynchronous.
    fn open_msg_box(
        &mut self,
        ui: &mut Ui<Msg>,
        form: &str,
        text: &str,
        title: &str,
        buttons: MsgBoxButtons,
        callback: Option<FnPtr>,
    ) {
        self.runtime.notify_msg_box(form, text, title);
        let dialog = match buttons {
            MsgBoxButtons::Ok => Dialog::message(ui, title, text),
            MsgBoxButtons::OkCancel => Dialog::confirm(ui, title, text),
            MsgBoxButtons::YesNo => Dialog::confirm(ui, title, text)
                .map(|dialog| dialog.accept_label("Yes").cancel_label("No")),
        };
        let dialog = match dialog {
            Ok(dialog) => dialog,
            Err(error) => {
                eprintln!("lazyrad: cannot show msg_box: {error}");
                return;
            }
        };
        let form = form.to_owned();
        let generation = self.generation;
        let dialog = dialog.on_action(move |action| {
            let accepted = matches!(action, DialogAction::Accept(_));
            let result = buttons.result(accepted);
            callback.clone().map(|callback| Msg::MsgBoxResult {
                form: form.clone(),
                callback,
                result,
                generation,
            })
        });
        dialog.open();
        self.dialogs.push(dialog);
    }
}

impl App for FormApp {
    type Msg = Msg;

    fn update(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
        match msg {
            Msg::WatchTick => {
                if self.watching {
                    match self.runtime.check_for_changes() {
                        ReloadOutcome::Unchanged => {}
                        ReloadOutcome::Reloaded => {
                            self.clear_banner();
                            // Every open window, this one included, rebuilds on
                            // the message so the reload path is one code path.
                            self.runtime.broadcast(Msg::Reload);
                        }
                        ReloadOutcome::Failed(diagnostics) => {
                            self.show_banner(ui, &diagnostics);
                        }
                    }
                }
            }
            Msg::Reload => {
                self.reload_root(ui);
            }
            Msg::Quit => {
                ui.quit();
                return;
            }
            other => self.handle_form_message(other, ui),
        }
        // A closed dialog no longer needs its nodes kept alive.
        self.dialogs.retain(Dialog::is_open);
        // A handler may have subscribed to something, or closed the last
        // subscription: run the polling timer only while there is work.
        self.poller.sync(ui);
    }
}

impl FormApp {
    /// Handles the messages that need a live form, leaving the window-wide ones
    /// ([`Msg::WatchTick`], [`Msg::Reload`], [`Msg::Quit`]) to [`FormApp::update`].
    fn handle_form_message(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
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
                if form.as_str() == root.name() {
                    let frame = is_frame_event(root.live_form(), &control, &event);
                    // A frame queued before its canvas's loop stopped (by a
                    // failed frame, or the script setting `fps = 0`) is stale.
                    if !(frame && frame_loop_stopped(root.live_form(), &control)) {
                        match root.run(&control, &event, &args) {
                            Ok(()) => self.runtime.notify_handler(root.name(), &control, &event),
                            Err(error) => {
                                // A failing frame handler would fail again on
                                // the next frame: stop that canvas's loop so
                                // the error is reported once, not 60 times a
                                // second.
                                if frame {
                                    stop_frame_loop(root.live_form(), &control);
                                }
                                self.report_handler_error(ui, root.name(), &control, &event, error);
                            }
                        }
                    }
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
            Msg::MsgBox {
                form,
                text,
                title,
                buttons,
                callback,
            } => {
                self.open_msg_box(ui, &form, &text, &title, buttons, callback);
                self.flush(ui);
            }
            Msg::MsgBoxResult {
                form,
                callback,
                result,
                generation,
            } => {
                // A result for the previous instance (the form reloaded while
                // its box was open) must not run the old callback against the
                // new script (checklist 1).
                if generation == self.generation
                    && form.as_str() == root.name()
                    && let Err(error) = root.call_callback(&callback, result)
                {
                    self.report_handler_error(ui, root.name(), "", "callback", error);
                }
                self.flush(ui);
            }
            Msg::Poll => {
                for error in self.poller.poll(&root) {
                    self.report_handler_error(ui, root.name(), "", "poll", error);
                }
                self.flush(ui);
            }
            // Handled by `update` before this is reached.
            Msg::WatchTick | Msg::Reload | Msg::Quit => {}
        }
    }
}

/// Whether `event` on `control` is a `Canvas`'s per-frame event.
fn is_frame_event(form: &LiveForm<Msg>, control: &str, event: &str) -> bool {
    event == xui_form::canvas::FRAME_EVENT && form.kind(control) == Some(xui_form::canvas::KIND)
}

/// Whether the `Canvas` named `control` has no frame loop running (`fps` is 0).
fn frame_loop_stopped(form: &LiveForm<Msg>, control: &str) -> bool {
    form.get(control, "fps") == Some(Value::Int(0))
}

/// Stops the frame loop of the `Canvas` named `control` by setting its `fps`
/// to 0, as the script itself would.
fn stop_frame_loop(form: &LiveForm<Msg>, control: &str) {
    if let Err(error) = form.set(control, "fps", &Value::Int(0)) {
        eprintln!("lazyrad: cannot stop `{control}`'s frame loop: {error}");
    }
}

/// The form a message must be handled by, when it belongs to one window.
/// Installs the window's close handler so it runs `root`'s `form_close`.
///
/// A reload replaces the root, so the handler is installed again with the new
/// instance; the old one is dropped with the old root.
fn install_close_handler(root: &Rc<FormInstance>, ui: &Ui<Msg>) {
    let root = Rc::clone(root);
    ui.on_close(move || {
        if let Err(error) = root.close() {
            eprintln!("lazyrad: {error}");
        }
        None
    });
}

/// The form a message must be handled by, when it belongs to one window.
fn addressee(msg: &Msg) -> Option<&str> {
    match msg {
        Msg::MsgBox { form, .. } | Msg::MsgBoxResult { form, .. } => Some(form),
        _ => None,
    }
}

/// The `other_form` object a script calls `show`/`unload` on.
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
    for name in runtime
        .sources
        .borrow()
        .forms
        .keys()
        .cloned()
        .collect::<Vec<_>>()
    {
        host.set_global(
            name.clone(),
            Dynamic::from(FormRef::new(name.clone(), Rc::clone(&runtime.pending))),
        );
    }
}

/// The text the reload banner shows for `diagnostics`: the first problem, plus
/// a count when there are more.
fn format_diagnostics(diagnostics: &[String]) -> String {
    match diagnostics {
        [] => "reload failed".to_owned(),
        [only] => format!("reload failed: {only}"),
        [first, rest @ ..] => format!("reload failed: {first} (+{} more)", rest.len()),
    }
}

/// Builds the banner strip (a full-width label at the top of the window).
///
/// The strip is a one-node form mounted on the window above the form's own
/// widgets, so it never blocks input. Dropping the returned [`LiveForm`]
/// destroys the strip.
fn build_banner(ui: &Ui<Msg>, catalog: &Catalog, text: &str) -> Option<LiveForm<Msg>> {
    let mut doc = FormDoc::new(BANNER_NODE);
    let mut label = Node::new("Label", BANNER_NODE);
    label.set_prop("left", Value::Int(0));
    label.set_prop("top", Value::Int(0));
    label.set_prop("width", Value::Int(640));
    label.set_prop("height", Value::Int(28));
    label.set_prop("anchor", Value::Enum("stretch_horizontal".to_owned()));
    label.set_prop("text", Value::Text(text.to_owned()));
    doc.insert(label);
    let factories: Factories<Msg> = Factories::xui();
    build_with(
        ui,
        &doc,
        catalog,
        &factories,
        &NoEvents,
        BuildOptions::default(),
    )
    .ok()
}

/// A binder that wires no events: the banner strip is inert.
struct NoEvents;

impl Binder<Msg> for NoEvents {
    fn bind(&self, _event: EventRef<'_>) -> Option<EventHandler<Msg>> {
        None
    }
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

/// Resolves `path` to the project directory and `.lrp` project it names.
///
/// A directory is searched for its single `.lrp`; an `.lrp` file is loaded
/// directly and its parent directory becomes the project directory. Any other
/// path is treated as a file and fails with an I/O error.
pub(crate) fn open_project(path: &Path) -> Result<(PathBuf, Project), RuntimeError> {
    if path.is_dir() {
        Ok((path.to_path_buf(), Project::load(path)?))
    } else {
        let dir = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
        Ok((dir, Project::load_file(path)?))
    }
}

/// Runs the project in `dir` on the portable `winit` backend.
#[cfg(feature = "desktop")]
pub fn run_project(dir: impl AsRef<Path>) -> Result<(), RuntimeError> {
    let backend: Rc<dyn Backend> = Rc::new(WinitBackend::new());
    run_project_with(backend, dir)
}

/// Loads the project in `dir` and runs it on `backend`.
///
/// The backend is passed in so the same path runs on `winit` and, in tests, on
/// the offscreen backend.
pub fn run_project_with(
    backend: Rc<dyn Backend>,
    dir: impl AsRef<Path>,
) -> Result<(), RuntimeError> {
    let runtime = FormRuntime::load(dir)?;
    run_runtime_with(backend, runtime)
}

/// Runs an already-loaded project on the portable `winit` backend.
#[cfg(feature = "desktop")]
pub fn run_runtime(runtime: Rc<FormRuntime>) -> Result<(), RuntimeError> {
    let backend: Rc<dyn Backend> = Rc::new(WinitBackend::new());
    run_runtime_with(backend, runtime)
}

/// Runs an already-loaded runtime's startup form on `backend`.
///
/// A failure that prevents the startup window from opening (the form cannot be
/// built, or its `form_load` handler fails) is returned when the event loop
/// ends. An event handler's runtime error does not come back here: it is shown
/// in a message box and the program keeps running.
pub fn run_runtime_with(
    backend: Rc<dyn Backend>,
    runtime: Rc<FormRuntime>,
) -> Result<(), RuntimeError> {
    let startup = runtime.startup_name()?;
    let doc = runtime
        .form(&startup)
        .map(|source| source.doc.clone())
        .ok_or_else(|| RuntimeError::UnknownForm(startup.clone()))?;
    let spec = window_spec(&doc);

    // `run_app` reports nothing about the app it built, so a fatal build error
    // is parked here and returned once the loop has ended.
    let failure: Rc<RefCell<Option<RuntimeError>>> = Rc::new(RefCell::new(None));
    let runtime_for_app = Rc::clone(&runtime);
    let startup_for_app = startup.clone();
    let failure_for_app = Rc::clone(&failure);
    run_app(backend, spec, move |ui| {
        match runtime_for_app.build_app(ui, &startup_for_app) {
            Ok(app) => {
                runtime_for_app.mark_open(&startup_for_app);
                app
            }
            Err(error) => {
                *failure_for_app.borrow_mut() = Some(error);
                ui.quit_with(1);
                FormApp::empty(Rc::clone(&runtime_for_app), startup_for_app.clone())
            }
        }
    })?;
    match failure.borrow_mut().take() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazyrad_project::Node;
    use xui_canvas::OffscreenBackend;

    /// The offscreen window spec the tests use.
    fn spec() -> PlatformSpec {
        PlatformSpec::new("lazyrad-runtime form tests").size(Dip(320.0), Dip(200.0))
    }

    #[test]
    fn a_message_box_callback_runs_against_the_form() {
        let mut doc = FormDoc::new("main_form");
        let mut label = Node::new("Label", "result_label");
        label.set_prop("left", Value::Int(10));
        label.set_prop("top", Value::Int(10));
        label.set_prop("width", Value::Int(160));
        doc.insert(label);

        let runtime = FormRuntime::from_sources(
            vec![FormSource::new(
                "main_form",
                doc,
                "fn report(result) { result_label.text = `${result}`; }",
            )],
            Vec::new(),
        );

        let backend = Rc::new(OffscreenBackend::new());
        let capture: Rc<RefCell<Option<Rc<LiveForm<Msg>>>>> = Rc::new(RefCell::new(None));
        let slot = Rc::clone(&capture);
        let callback = FnPtr::new("report").expect("a valid function name");
        run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
            let app = runtime
                .build_app(ui, "main_form")
                .expect("main_form builds");
            let root = app.root.clone().expect("the form is live");
            root.call_callback(&callback, "ok")
                .expect("the callback runs");
            *slot.borrow_mut() = Some(root.live_form().clone());
            app
        })
        .expect("the event loop runs");

        let form = capture.borrow_mut().take().expect("the form was captured");
        assert_eq!(
            form.get("result_label", "text"),
            Some(Value::Text("ok".to_owned()))
        );
    }

    #[test]
    fn opening_a_message_box_keeps_a_dialog_alive() {
        let runtime = FormRuntime::from_sources(
            vec![FormSource::new("main_form", FormDoc::new("main_form"), "")],
            Vec::new(),
        );
        let backend = Rc::new(OffscreenBackend::new());
        let capture: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
        let slot = Rc::clone(&capture);

        run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
            let mut app = runtime
                .build_app(ui, "main_form")
                .expect("main_form builds");
            app.open_msg_box(ui, "main_form", "Hello", "Title", MsgBoxButtons::Ok, None);
            *slot.borrow_mut() = Some(app.dialogs.last().is_some_and(Dialog::is_open));
            app
        })
        .expect("the event loop runs");

        assert_eq!(capture.borrow().as_ref(), Some(&true));
    }

    #[test]
    fn a_handler_error_shows_a_message_box() {
        let runtime = FormRuntime::from_sources(
            vec![FormSource::new("main_form", FormDoc::new("main_form"), "")],
            Vec::new(),
        );
        let backend = Rc::new(OffscreenBackend::new());
        let capture: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
        let slot = Rc::clone(&capture);

        run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
            let mut app = runtime
                .build_app(ui, "main_form")
                .expect("main_form builds");
            let error = ScriptError::new(
                "main_form.rhai",
                rhai::Position::new(1, 1),
                "division by zero",
            );
            app.report_handler_error(ui, "main_form", "go_button", "Click", error);
            *slot.borrow_mut() = Some(app.dialogs.last().is_some_and(Dialog::is_open));
            app
        })
        .expect("the event loop runs");

        assert_eq!(
            capture.borrow().as_ref(),
            Some(&true),
            "a handler error opens a non-blocking message box"
        );
    }

    /// What the frame test records after each step: the open dialogs and the
    /// two canvases' `fps`.
    type Observation = (usize, Option<Value>, Option<Value>);

    /// A `Frame` (or other) event for `control` on `main_form`.
    fn canvas_event(control: &str, event: &str, args: Vec<Value>) -> Msg {
        Msg::Event {
            form: "main_form".to_owned(),
            control: control.to_owned(),
            event: event.to_owned(),
            args,
        }
    }

    #[test]
    fn a_failing_frame_handler_stops_its_canvas_and_is_reported_once() {
        let mut doc = FormDoc::new("main_form");
        for (name, left, fps) in [("canvas1", 0, 60), ("canvas2", 160, 30)] {
            let mut canvas = Node::new("Canvas", name);
            canvas.set_prop("left", Value::Int(left));
            canvas.set_prop("width", Value::Int(150));
            canvas.set_prop("height", Value::Int(100));
            canvas.set_prop("fps", Value::Int(fps));
            doc.insert(canvas);
        }
        // canvas2's handler draws on its own canvas and sets its own fps from
        // inside the frame: the ordinary, re-entrant path.
        let code = r##"
            fn canvas1_frame(dt) { canvas1.clear("#000000"); let x = 1 / 0; }
            fn canvas1_key_down(key) { canvas1.fps = 60; }
            fn canvas2_frame(dt) {
                canvas2.clear(0x000000);
                canvas2.fill_rect(0, 0, 10, 10, "#ff0000");
                canvas2.fps = 30;
            }
        "##;
        let runtime =
            FormRuntime::from_sources(vec![FormSource::new("main_form", doc, code)], Vec::new());
        let backend = Rc::new(OffscreenBackend::new());
        let capture: Rc<RefCell<Vec<Observation>>> = Rc::new(RefCell::new(Vec::new()));
        let slot = Rc::clone(&capture);
        run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
            let mut app = runtime
                .build_app(ui, "main_form")
                .expect("main_form builds");
            let form = app.root_form().expect("the form is live").clone();
            let record = |app: &FormApp| {
                slot.borrow_mut().push((
                    app.dialogs.len(),
                    form.get("canvas1", "fps"),
                    form.get("canvas2", "fps"),
                ));
            };
            let frame = |control: &str| canvas_event(control, "Frame", vec![Value::Float(0.016)]);
            // The failing frame is reported and stops canvas1's loop.
            app.update(frame("canvas1"), ui);
            record(&app);
            // A frame queued before the stop is dropped, not reported again.
            app.update(frame("canvas1"), ui);
            record(&app);
            // canvas2 keeps running.
            app.update(frame("canvas2"), ui);
            record(&app);
            // The script restarts canvas1; its next failure is reported again.
            app.update(
                canvas_event("canvas1", "KeyDown", vec![Value::Text("space".to_owned())]),
                ui,
            );
            app.update(frame("canvas1"), ui);
            record(&app);
            app
        })
        .expect("the event loop runs");

        let stopped = Some(Value::Int(0));
        let thirty = Some(Value::Int(30));
        assert_eq!(
            *capture.borrow(),
            vec![
                (1, stopped.clone(), thirty.clone()),
                (1, stopped.clone(), thirty.clone()),
                (1, stopped.clone(), thirty.clone()),
                (2, stopped, thirty),
            ]
        );
    }

    #[test]
    fn a_fatal_build_error_is_returned_by_runtime_with() {
        // A `form_load` that fails prevents the window from opening, so the
        // error must come back as a fatal runtime error rather than a dialog.
        // `from_sources` names its project "runtime" and uses that as startup,
        // so the failing startup form is named for it.
        let runtime = FormRuntime::from_sources(
            vec![FormSource::new(
                "runtime",
                FormDoc::new("runtime"),
                "fn form_load() { let x = 1 / 0; }",
            )],
            Vec::new(),
        );
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        let error = run_runtime_with(backend, runtime).expect_err("form_load fails");
        let RuntimeError::Script(script) = error else {
            panic!("a located script error is expected, got {error:?}");
        };
        assert_eq!(script.file, "runtime.rhai");
    }

    /// A one-button form whose `go_button_click` is `code`.
    fn button_form(code: &str) -> Rc<FormRuntime> {
        let mut doc = FormDoc::new("main_form");
        let mut button = Node::new("Button", "go_button");
        button.set_prop("left", Value::Int(10));
        button.set_prop("top", Value::Int(10));
        button.set_prop("width", Value::Int(100));
        button.set_prop("height", Value::Int(28));
        doc.insert(button);
        FormRuntime::from_sources(vec![FormSource::new("main_form", doc, code)], Vec::new())
    }

    /// Emits one `go_button` click and lets the loop drain it.
    fn click_go(runtime: Rc<FormRuntime>) {
        let backend = Rc::new(OffscreenBackend::new());
        run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
            let app = runtime
                .build_app(ui, "main_form")
                .expect("main_form builds");
            ui.emit(Msg::Event {
                form: "main_form".to_owned(),
                control: "go_button".to_owned(),
                event: "Click".to_owned(),
                args: Vec::new(),
            });
            app
        })
        .expect("the event loop runs");
    }

    #[test]
    fn an_error_observer_hears_a_failed_event() {
        let runtime = button_form("fn go_button_click() { let d = 0; 1 / d }");
        let heard: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&heard);
        runtime.set_error_observer(Rc::new(move |form, control, event, error| {
            sink.borrow_mut()
                .push(format!("{form}.{control}.{event}: {error}"));
        }));

        click_go(runtime);

        let heard = heard.borrow();
        assert_eq!(heard.len(), 1, "heard: {heard:?}");
        assert!(
            heard[0].starts_with("main_form.go_button.Click: "),
            "heard: {heard:?}"
        );
        assert!(heard[0].contains("Division by zero"), "heard: {heard:?}");
    }

    #[test]
    fn an_error_observer_hears_a_failed_callback() {
        let runtime = button_form("fn bad_callback(result) { let d = 0; 1 / d }");
        let heard: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&heard);
        runtime.set_error_observer(Rc::new(move |form, control, event, _error| {
            sink.borrow_mut().push(format!("{form}.{control}.{event}"));
        }));
        let callback = FnPtr::new("bad_callback").expect("a valid function name");

        let backend = Rc::new(OffscreenBackend::new());
        run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
            let app = runtime
                .build_app(ui, "main_form")
                .expect("main_form builds");
            ui.emit(Msg::MsgBoxResult {
                form: "main_form".to_owned(),
                callback,
                result: "ok",
                // The first build of a window is generation 0.
                generation: 0,
            });
            app
        })
        .expect("the event loop runs");

        assert_eq!(heard.borrow().as_slice(), ["main_form..callback"]);
    }

    #[test]
    fn an_error_observer_may_re_enter_the_runtime() {
        // The observer is cloned out of its `RefCell` before it runs, so one
        // that installs a new observer (or otherwise touches the runtime) must
        // not hit a held borrow.
        let runtime = button_form("fn go_button_click() { let d = 0; 1 / d }");
        let runtime_for_observer = Rc::clone(&runtime);
        runtime.set_error_observer(Rc::new(move |_form, _control, _event, _error| {
            let _ = runtime_for_observer.form_names().len();
            runtime_for_observer.set_error_observer(Rc::new(|_, _, _, _| {}));
        }));

        click_go(runtime);
    }

    #[test]
    fn a_stack_overflow_reaches_the_error_observer() {
        // Deep Rhai recursion needs more than a test thread's default stack in
        // a debug build, so the whole run goes on a large one.
        //
        // Rhai 1.26 treats `ErrorStackOverflow` as a system exception: it is
        // not wrapped in `ErrorInFunctionCall` and its position is overwritten
        // to `NONE` on the way out, so the overflow reports no chain and `0:0`.
        // The point of this test is that the failure is a first-class one a
        // host can see at all (the MOD sample only showed it in a dialog).
        let error = crate::testing::run_on_large_stack(|| {
            let runtime =
                button_form("fn go_button_click() { recurse(); }\nfn recurse() { recurse(); }");
            let captured: Rc<RefCell<Option<ScriptError>>> = Rc::new(RefCell::new(None));
            let sink = Rc::clone(&captured);
            runtime.set_error_observer(Rc::new(move |_form, _control, _event, error| {
                *sink.borrow_mut() = Some(error.clone());
            }));
            click_go(runtime);
            captured
                .borrow_mut()
                .take()
                .expect("the error was reported")
        });

        assert_eq!(error.message, "Stack overflow");
        assert_eq!(error.file, "main_form.rhai");
        assert_eq!(error.call_chain, ["go_button_click"]);
        assert_eq!(
            error.to_string(),
            "main_form.rhai:0:0: Stack overflow (in go_button_click)"
        );
    }

    #[test]
    fn a_handler_calling_itself_counts_every_level() {
        // Rhai wraps the two recursive calls but not the entry call, so the
        // chain is the entry plus both: three levels, not two.
        let error = crate::testing::run_on_large_stack(|| {
            let runtime = button_form(
                "fn go_button_click() {\n\
                     if !(\"n\" in form.state) { form.state.n = 0; }\n\
                     form.state.n += 1;\n\
                     if form.state.n < 3 { go_button_click(); } else { let d = 0; 1 / d }\n\
                 }",
            );
            let captured: Rc<RefCell<Option<ScriptError>>> = Rc::new(RefCell::new(None));
            let sink = Rc::clone(&captured);
            runtime.set_error_observer(Rc::new(move |_form, _control, _event, error| {
                *sink.borrow_mut() = Some(error.clone());
            }));
            click_go(runtime);
            captured
                .borrow_mut()
                .take()
                .expect("the error was reported")
        });

        assert_eq!(
            error.call_chain,
            ["go_button_click", "go_button_click", "go_button_click"],
            "{error}"
        );
    }

    #[test]
    fn a_deeply_nested_error_reports_a_position_and_chain() {
        // A catchable error keeps Rhai's call wrappers, so the innermost
        // position and the whole `handler → helper → …` chain survive.
        let error = crate::testing::run_on_large_stack(|| {
            let runtime = button_form(
                "fn go_button_click() { first(); }\n\
                 fn first() { second(); }\n\
                 fn second() { let d = 0; 1 / d }",
            );
            let captured: Rc<RefCell<Option<ScriptError>>> = Rc::new(RefCell::new(None));
            let sink = Rc::clone(&captured);
            runtime.set_error_observer(Rc::new(move |_form, _control, _event, error| {
                *sink.borrow_mut() = Some(error.clone());
            }));
            click_go(runtime);
            captured
                .borrow_mut()
                .take()
                .expect("the error was reported")
        });

        assert!(error.line > 0, "a non-zero position: {error}");
        assert!(error.message.starts_with("Division by zero"), "{error}");
        assert_eq!(error.call_chain, ["go_button_click", "first", "second"]);
        assert!(
            error
                .to_string()
                .contains("(in go_button_click → first → second)"),
            "{error}"
        );
    }

    #[test]
    fn only_form_messages_have_an_addressee() {
        let msg_box = Msg::MsgBox {
            form: "other_form".to_owned(),
            text: String::new(),
            title: String::new(),
            buttons: MsgBoxButtons::Ok,
            callback: None,
        };
        assert_eq!(addressee(&msg_box), Some("other_form"));
        assert_eq!(addressee(&Msg::ShowForm("other_form".to_owned())), None);
        assert_eq!(addressee(&Msg::Quit), None);
    }

    #[test]
    fn every_built_form_registers_its_window_until_closed() {
        let runtime = FormRuntime::from_sources(
            vec![
                FormSource::new("main_form", FormDoc::new("main_form"), ""),
                FormSource::new("other_form", FormDoc::new("other_form"), ""),
            ],
            Vec::new(),
        );
        let backend = Rc::new(OffscreenBackend::new());
        let capture: Rc<RefCell<Vec<Vec<String>>>> = Rc::new(RefCell::new(Vec::new()));
        let slot = Rc::clone(&capture);
        let runtime_in_loop = Rc::clone(&runtime);

        run_app(backend as Rc<dyn Backend>, spec(), move |ui| {
            let app = runtime_in_loop
                .build_app(ui, "main_form")
                .expect("main_form builds");
            let names = |runtime: &FormRuntime| runtime.inboxes.borrow().keys().cloned().collect();
            app.show_form("other_form", ui);
            slot.borrow_mut().push(names(&runtime_in_loop));
            app.close_form("other_form");
            slot.borrow_mut().push(names(&runtime_in_loop));
            app
        })
        .expect("the event loop runs");

        assert_eq!(
            *capture.borrow(),
            vec![
                vec!["main_form".to_owned(), "other_form".to_owned()],
                vec!["main_form".to_owned()],
            ]
        );
    }
}
