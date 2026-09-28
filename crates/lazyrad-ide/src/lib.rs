#![forbid(unsafe_code)]

//! The LazyRAD IDE shell.
//!
//! This crate owns the main window: the VB6 menu bar, toolbar, toolbox, tabbed
//! document area, Project/Properties panes and Output pane, all wired to a
//! central [`Command`] vocabulary. Menu items, toolbar items and keyboard
//! shortcuts all dispatch the same commands, which are logged until the
//! features behind them land (PLAN.md §9).
//!
//! [`run`] opens the window on the portable [`WinitBackend`](xui_canvas::WinitBackend);
//! [`app`] holds the widget tree, [`command`] the dispatcher, [`settings`] the
//! persisted layout and theme, and [`theme`] the light/dark/system resolution.

pub mod app;
pub mod command;
pub mod settings;
pub mod shortcut_backend;
pub mod theme;

use std::error::Error;
use std::rc::Rc;

use xui_canvas::WinitBackend;
use xui_core::app::run_app;
use xui_core::backend::Backend;

pub use app::{IdeApp, Msg, PaneSlot, default_platform_spec, shortcut_message};
pub use command::{Command, Dispatcher, Shortcut};
pub use settings::{PaneSizes, Settings, SettingsError, ThemeChoice};

/// Opens the IDE window and runs until it closes.
pub fn run() -> Result<(), Box<dyn Error>> {
    let settings = Settings::load().unwrap_or_else(|error| {
        eprintln!("lazyrad-ide: {error}; using defaults");
        Settings::default()
    });
    let recent = settings.recent_projects.clone();

    let inner: Rc<dyn Backend> = Rc::new(WinitBackend::new());
    let backend = Rc::new(shortcut_backend::ShortcutBackend::new(
        inner,
        shortcut_message,
    ));
    let proxy_cell = backend.proxy_cell();

    run_app(backend, default_platform_spec(), move |ui| {
        *proxy_cell.borrow_mut() = Some(ui.proxy());
        IdeApp::build(ui, settings, recent).expect("the IDE widgets build")
    })?;
    Ok(())
}
