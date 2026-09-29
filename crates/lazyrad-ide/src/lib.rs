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
//! [`run`] opens the window on the portable [`WinitBackend`](xui_canvas::WinitBackend);
//! [`app`] holds the widget tree, [`command`] the dispatcher, [`settings`] the
//! persisted layout and theme, [`project`] the project rules, [`explorer`] the
//! Project Explorer's rows, [`dialog`] the three-way save prompt and
//! [`platform::dialogs`] the native file and folder dialogs xui lacks.

pub mod app;
pub mod command;
pub mod compile;
pub mod dialog;
pub mod explorer;
pub mod platform;
pub mod procedures;
pub mod project;
pub mod run;
pub mod settings;
pub mod shortcut_backend;
pub mod theme;

use std::error::Error;
use std::rc::Rc;

use xui_canvas::WinitBackend;
use xui_core::app::run_app;
use xui_core::backend::Backend;

pub use app::{
    ContextAction, DocKind, IdeApp, Msg, PaneSlot, PromptKind, SaveChoice, default_platform_spec,
    shortcut_message,
};
pub use command::{Command, Dispatcher, Shortcut};
pub use explorer::Explorer;
pub use project::{ProjectSession, SessionError};
pub use settings::{PaneSizes, Settings, SettingsError, ThemeChoice};

/// Opens the IDE window and runs until it closes.
pub fn run() -> Result<(), Box<dyn Error>> {
    let settings = Settings::load().unwrap_or_else(|error| {
        eprintln!("lazyrad-ide: {error}; using defaults");
        let defaults = Settings::default();
        match Settings::path() {
            Some(path) => defaults.stored_at(path),
            None => defaults,
        }
    });
    let recent = settings.recent_projects.clone();

    let inner: Rc<dyn Backend> = Rc::new(WinitBackend::new());
    let shortcuts = Rc::new(shortcut_backend::ShortcutBackend::new(
        inner,
        shortcut_message,
    ));
    let proxy_cell = shortcuts.proxy_cell();
    let backend: Rc<dyn Backend> = shortcuts;

    run_app(backend, default_platform_spec(), move |ui| {
        *proxy_cell.borrow_mut() = Some(ui.proxy());
        IdeApp::build(ui, settings, recent).expect("the IDE widgets build")
    })?;
    Ok(())
}
