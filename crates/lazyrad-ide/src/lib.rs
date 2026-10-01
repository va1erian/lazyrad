#![forbid(unsafe_code)]

//! The LazyRAD IDE shell.
//!
//! This crate owns the main window: the VB6 menu bar, toolbar, toolbox, tabbed
//! document area, Project Explorer and Properties panes and the Output pane,
//! all wired to a central [`Command`] vocabulary. Menu items, toolbar items and
//! keyboard shortcuts all dispatch the same commands, and project management
//! (New/Open/Save, the explorer, dirty tracking and the save prompt) lives in
//! [`app`] on top of [`project`].
//!
//! [`run`] (feature `desktop`) opens the window on the portable `WinitBackend`,
//! [`run_with_backend`] on any other xui backend;
//! [`app`] holds the widget tree, [`command`] the dispatcher, [`settings`] the
//! persisted layout and theme, [`project`] the project rules, [`explorer`] the
//! Project Explorer's rows, [`dialog`] the three-way save prompt and
//! [`platform::dialogs`] the native file and folder dialogs xui lacks.

pub mod app;
pub mod command;
pub mod compile;
pub mod dialog;
pub mod edit_state;
pub mod explorer;
pub mod file_dialogs;
pub mod make_exe;
pub mod platform;
pub mod procedures;
pub mod project;
pub mod run;
pub mod settings;
pub mod shortcut_backend;
pub mod start_page;
pub mod theme;
pub mod tutorial;

use std::error::Error;
use std::rc::Rc;

use xui_core::app::run_app;
use xui_core::backend::Backend;

pub use app::{
    ContextAction, DocKind, IdeApp, IdeEvent, IdeObserver, Msg, PaneSlot, PromptKind, SaveChoice,
    default_platform_spec, shortcut_message,
};
pub use command::{Command, Dispatcher, Shortcut};
pub use edit_state::EditAvailability;
pub use explorer::Explorer;
pub use project::{ProjectSession, SessionError};
pub use settings::{PaneSizes, Settings, SettingsError, ThemeChoice};

/// Opens the IDE window on the desktop `winit` backend, with the desktop
/// [`platform::host::HostPlatform`] installed, and runs until it closes.
#[cfg(feature = "desktop")]
pub fn run() -> Result<(), Box<dyn Error>> {
    platform::host::install();
    run_with_backend(Rc::new(xui_canvas::WinitBackend::new()))
}

/// Runs the IDE on `inner`, a window backend the caller created, until it
/// closes. The caller installs its [`lazyrad_runtime::platform::Platform`]
/// first; LazyOS passes `LazyOSBackend::connect()` here.
pub fn run_with_backend(inner: Rc<dyn Backend>) -> Result<(), Box<dyn Error>> {
    run_with_options(
        inner,
        RunOptions {
            spec: default_platform_spec(),
            open: None,
            observer: None,
            launcher: None,
        },
    )
}

/// How an embedder starts the IDE.
pub struct RunOptions {
    /// The window. A compositor that bounds surfaces to the screen (LazyOS's
    /// `xuid` refuses a window taller than the display) needs a size that fits;
    /// [`default_platform_spec`] is sized for a desktop.
    pub spec: xui_core::backend::PlatformSpec,
    /// A project folder to open at start-up (LazyOS passes the folder a
    /// `.lrp` was opened from).
    pub open: Option<std::path::PathBuf>,
    /// Told about project and run milestones, or `None`.
    pub observer: Option<IdeObserver>,
    /// How the player is started, or `None` for the threaded default.
    pub launcher: Option<Rc<dyn run::Launcher>>,
}

/// Like [`run_with_backend`] with explicit [`RunOptions`].
pub fn run_with_options(inner: Rc<dyn Backend>, options: RunOptions) -> Result<(), Box<dyn Error>> {
    let RunOptions {
        spec,
        open,
        observer,
        launcher,
    } = options;
    let settings = Settings::load().unwrap_or_else(|error| {
        eprintln!("lazyrad-ide: {error}; using defaults");
        let defaults = Settings::default();
        match Settings::path() {
            Some(path) => defaults.stored_at(path),
            None => defaults,
        }
    });
    let recent = settings.recent_projects.clone();

    let shortcuts = Rc::new(shortcut_backend::ShortcutBackend::new(
        inner,
        shortcut_message,
    ));
    let proxy_cell = shortcuts.proxy_cell();
    let backend: Rc<dyn Backend> = shortcuts;
    let icon_backend = Rc::clone(&backend);

    run_app(backend, spec, move |ui| {
        *proxy_cell.borrow_mut() = Some(ui.proxy());
        start_page::install_window_icon(icon_backend.as_ref(), ui.window());
        let mut app = IdeApp::build(ui, settings, recent).expect("the IDE widgets build");
        if let Some(observer) = observer {
            app.set_observer(observer);
        }
        if let Some(launcher) = launcher {
            app.set_launcher(launcher);
        }
        if let Some(dir) = open {
            app.open_dir(dir, ui);
        }
        app
    })?;
    Ok(())
}
