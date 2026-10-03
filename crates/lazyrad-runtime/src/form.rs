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
//! window is asked to close (the window then closes for real). The window spec
//! itself comes from [`Catalog::window_spec`](xui_form::Catalog::window_spec).
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
use xui_form::{Catalog, FormDoc, LiveForm, Value};

use lazyrad_project::{Project, lazyrad_catalog, parse_form};

use crate::events::Poller;
use crate::fs_policy::FsPolicy;
use crate::platform;
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

/// Every form and module a project declares, plus what a running window needs.
///
/// The runtime is shared (`Rc`) by every open window: a form's script may ask a
/// different form to open, and the request travels through this type.
pub struct FormRuntime {
    project: Project,
    pub(crate) forms: BTreeMap<String, FormSource>,
    pub(crate) modules: Vec<ModuleSource>,
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
}

/// A callback told `(form, control, event)` after an event handler ran without
/// error. The LazyOS player uses it to print its `LRPLAY:EVENT:PASS` serial
/// marker; any host can use it for telemetry or tests.
pub type HandlerObserver = Rc<dyn Fn(&str, &str, &str)>;

impl FormRuntime {
    /// Installs the observer told after each successful event handler,
    /// replacing any earlier one.
    pub fn set_handler_observer(&self, observer: HandlerObserver) {
        *self.observer.borrow_mut() = Some(observer);
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
            project,
            forms,
            modules,
            catalog,
            pending: Rc::new(RefCell::new(Vec::new())),
            path: dir,
            opened: RefCell::new(BTreeSet::new()),
            windows: RefCell::new(BTreeMap::new()),
            inboxes: RefCell::new(BTreeMap::new()),
            fs: Rc::new(platform::current().fs_policy()),
            observer: RefCell::new(None),
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
            path: PathBuf::from("."),
            opened: RefCell::new(BTreeSet::new()),
            windows: RefCell::new(BTreeMap::new()),
            inboxes: RefCell::new(BTreeMap::new()),
            fs: Rc::new(platform::current().fs_policy()),
            observer: RefCell::new(None),
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

    /// The project directory, exposed to scripts as `App.path`.
    pub fn path(&self) -> &Path {
        &self.path
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

        // `form_close` runs in the close mapper and then the runtime performs
        // its normal close: a primary window quits the loop, a secondary one
        // does not. Intercepting the close with a message would skip that quit.
        let root_for_close = Rc::clone(&root);
        ui.on_close(move || {
            if let Err(error) = root_for_close.close() {
                eprintln!("lazyrad: {error}");
            }
            None
        });

        self.inboxes
            .borrow_mut()
            .insert(form.to_owned(), ui.clone());
        let mut app = FormApp {
            root: Some(Rc::clone(&root)),
            runtime: Rc::clone(self),
            dialogs: Vec::new(),
            poller: Poller::new(ui, form),
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
        let stdlib = StdlibContext {
            form: source.name.clone(),
            pending: Rc::clone(&runtime.pending),
            app_title: runtime.project().name.clone(),
            app_path: runtime.path().display().to_string(),
            fs: Rc::clone(&runtime.fs),
        };
        let runtime_for_setup = Rc::clone(runtime);
        let script = ScriptForm::build(
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
                for module in &runtime_for_setup.modules {
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
    root: Option<Rc<FormInstance>>,
    runtime: Rc<FormRuntime>,
    /// Every open message box. A [`Dialog`] destroys its nodes when dropped, so
    /// the application keeps each one alive until it closes.
    dialogs: Vec<Dialog<Msg>>,
    /// Polls the host's event sources while the form has work pending, and
    /// releases what the form registered when the window goes away.
    poller: Poller,
}

impl FormApp {
    /// The live form this window drives, when it built successfully.
    pub fn root_form(&self) -> Option<&Rc<LiveForm<Msg>>> {
        self.root.as_ref().map(|instance| instance.live_form())
    }

    /// Whether the window is polling the host's event sources for its form.
    pub fn is_polling(&self) -> bool {
        self.poller.is_polling()
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
                        dialogs: Vec::new(),
                        poller: Poller::idle(),
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

    /// Reports a handler's runtime error without stopping the program.
    ///
    /// A widget handler's failure is not fatal: the error is shown in a
    /// message box (the script's own `msg_box`, non-blocking like any other)
    /// and the application keeps running, so the player's exit code stays 0.
    /// Only a failure that prevents the window from opening is fatal.
    fn report_handler_error(&mut self, ui: &mut Ui<Msg>, form: &str, error: ScriptError) {
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
        let dialog = dialog.on_action(move |action| {
            let accepted = matches!(action, DialogAction::Accept(_));
            let result = buttons.result(accepted);
            callback.clone().map(|callback| Msg::MsgBoxResult {
                form: form.clone(),
                callback,
                result,
            })
        });
        dialog.open();
        self.dialogs.push(dialog);
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
                if form.as_str() == root.name() {
                    match root.run(&control, &event, &args) {
                        Ok(()) => self.runtime.notify_handler(root.name(), &control, &event),
                        Err(error) => self.report_handler_error(ui, root.name(), error),
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
            } => {
                if form.as_str() == root.name()
                    && let Err(error) = root.call_callback(&callback, result)
                {
                    self.report_handler_error(ui, root.name(), error);
                }
                self.flush(ui);
            }
            Msg::Poll => {
                for error in self.poller.poll(&root) {
                    self.report_handler_error(ui, root.name(), error);
                }
                self.flush(ui);
            }
            Msg::Quit => ui.quit(),
        }
        // A closed dialog no longer needs its nodes kept alive.
        self.dialogs.retain(Dialog::is_open);
        // A handler may have subscribed to something, or closed the last
        // subscription: run the polling timer only while there is work.
        self.poller.sync(ui);
    }
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
    for name in runtime.forms.keys() {
        host.set_global(
            name.clone(),
            Dynamic::from(FormRef::new(name.clone(), Rc::clone(&runtime.pending))),
        );
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
                FormApp {
                    root: None,
                    runtime: Rc::clone(&runtime_for_app),
                    dialogs: Vec::new(),
                    poller: Poller::idle(),
                }
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
            app.report_handler_error(ui, "main_form", error);
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
