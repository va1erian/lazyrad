#![forbid(unsafe_code)]

//! The runtime's platform seam (LazyOS plan P0).
//!
//! Everything LazyRAD needs from the operating system that xui cannot provide
//! is behind [`Platform`]: dialogs, the config directory, the default
//! monospace font, where the player executable lives and how to prepare its
//! process, and the file-access policy scripts run under. The desktop binaries
//! install a host implementation once at startup; LazyOS installs its own; a
//! process that installs nothing gets [`PortablePlatform`], whose answers are
//! the safe defaults (every dialog cancels, no config directory, scripts may
//! touch any file).
//!
//! The system clipboard is deliberately **not** here: xui routes it through the
//! window backend (`Ui::clipboard_text`), so `LazyOSBackend` supplies `clipboardd`
//! and the code editor needs nothing from LazyRAD.
//!
//! The per-crate seam modules of the IDE, packager and player (PLAN.md §12)
//! delegate to [`current`]; they keep only the compile-time choices (`cfg`
//! attributes for a host OS), so a new platform implements this trait instead of
//! editing them.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::fs_policy::FsPolicy;

/// `(description, extensions)` of a file name filter.
pub type Filter<'a> = (&'a str, &'a [&'a str]);

/// File, folder and message dialogs.
///
/// Every method answers "cancelled" (`None`) when no dialog can be shown, so a
/// caller never blocks on a dialog that cannot appear. On LazyOS the dialogs are
/// painted in-window, so an implementation may only be able to answer from a
/// previously stored result; see the LazyOS seam for how it is fed.
pub trait Dialogs: Send + Sync {
    /// Asks for an existing file.
    fn open_file(&self, title: &str, filter: Option<Filter<'_>>) -> Option<PathBuf>;
    /// Asks for a folder.
    fn choose_folder(&self, title: &str) -> Option<PathBuf>;
    /// Asks where to save, suggesting `file_name`.
    fn save_file(
        &self,
        title: &str,
        file_name: &str,
        filter: Option<Filter<'_>>,
    ) -> Option<PathBuf>;
    /// Shows an error that stops the program before or instead of its window.
    fn show_error(&self, title: &str, message: &str);
}

/// Dialogs for a platform with none: cancel everything, errors to stderr.
#[derive(Clone, Copy, Debug, Default)]
pub struct HeadlessDialogs;

impl Dialogs for HeadlessDialogs {
    fn open_file(&self, _title: &str, _filter: Option<Filter<'_>>) -> Option<PathBuf> {
        None
    }
    fn choose_folder(&self, _title: &str) -> Option<PathBuf> {
        None
    }
    fn save_file(&self, _title: &str, _name: &str, _filter: Option<Filter<'_>>) -> Option<PathBuf> {
        None
    }
    fn show_error(&self, title: &str, message: &str) {
        eprintln!("{title}: {message}");
    }
}

/// What a host operating system provides to LazyRAD.
pub trait Platform: Send + Sync {
    /// A short name for diagnostics (`"desktop"`, `"lazyos"`, `"portable"`).
    fn name(&self) -> &'static str;

    /// The dialogs this platform can show.
    fn dialogs(&self) -> &dyn Dialogs {
        &HeadlessDialogs
    }

    /// A filesystem to browse with xui's painted, in-window file dialog, for a
    /// platform with no blocking native dialogs (LazyOS). When `Some`, the IDE
    /// opens its own dialog on that filesystem and continues when the user
    /// answers, instead of calling [`Dialogs`] (which would have to block).
    fn file_system(&self) -> Option<std::rc::Rc<dyn xui_core::widget::FileSystem>> {
        None
    }

    /// Where the in-window file dialog starts, and where new projects go.
    fn projects_dir(&self) -> PathBuf {
        PathBuf::from("/")
    }

    /// The per-user directory for LazyRAD settings, if the platform has one.
    fn config_dir(&self) -> Option<PathBuf> {
        None
    }

    /// The monospace family the code editor uses by default.
    fn default_monospace_font(&self) -> &'static str {
        "DejaVu Sans Mono"
    }

    /// Whether the platform currently prefers a dark theme (the IDE's "System"
    /// theme choice). Light when the platform cannot tell.
    fn prefers_dark(&self) -> bool {
        false
    }

    /// The platform's own widget palette for the IDE's "System" theme, when
    /// it has one (LazyOS: the desktop's theme and accent). `None` picks xui's
    /// light or dark palette from [`Platform::prefers_dark`].
    fn system_theme(&self) -> Option<xui_core::Theme> {
        None
    }

    /// The file-access policy scripts run under (LazyOS plan D5). The desktop
    /// default is unrestricted; LazyOS returns a sandbox rooted at the app's
    /// private data directory.
    fn fs_policy(&self) -> FsPolicy {
        FsPolicy::Unrestricted
    }

    /// Where the player executable is expected, next to the running program by
    /// default. `None` when the platform has no fixed place (the IDE then relies
    /// on its `player_path` setting).
    fn player_executable(&self) -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let name = format!("lazyrad-player{}", std::env::consts::EXE_SUFFIX);
        Some(exe.parent()?.join(name))
    }

    /// Adjusts the player's [`Command`] before it is spawned with piped stdio
    /// (the Windows desktop hides the console window here).
    fn prepare_player_command(&self, _command: &mut Command) {}

    /// The system services and topics `scripts` use, for a packaged app's
    /// manifest. Only the platform knows its services (LazyOS answers from
    /// the calls to its generated `sys::*` modules); the default is none.
    fn script_permissions(&self, _scripts: &[&str]) -> ScriptPermissions {
        ScriptPermissions::default()
    }
}

/// What [`Platform::script_permissions`] found: entries in the package
/// manifest's grammar (`os.lazy.confd.v1`, `subscribe:system/confd/changed/#`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptPermissions {
    /// Interfaces the scripts call.
    pub interfaces: Vec<String>,
    /// `publish:`/`subscribe:` topic rules.
    pub topics: Vec<String>,
}

/// The defaults: no dialogs, no config directory, unrestricted files.
#[derive(Clone, Copy, Debug, Default)]
pub struct PortablePlatform;

impl Platform for PortablePlatform {
    fn name(&self) -> &'static str {
        "portable"
    }
}

static CURRENT: OnceLock<Box<dyn Platform>> = OnceLock::new();
static PORTABLE: PortablePlatform = PortablePlatform;

/// Installs the process's platform. Call once, first thing in `main`; a second
/// call (or one after [`current`] was used) is refused and returns the rejected
/// platform so the caller can report the ordering bug.
pub fn install(platform: Box<dyn Platform>) -> Result<(), Box<dyn Platform>> {
    CURRENT.set(platform)
}

/// The installed platform, or [`PortablePlatform`] when none was installed.
pub fn current() -> &'static dyn Platform {
    match CURRENT.get() {
        Some(platform) => platform.as_ref(),
        None => &PORTABLE,
    }
}

/// `dir` joined with the settings file name, or `None` without a directory.
pub fn settings_file_in(dir: Option<&Path>) -> Option<PathBuf> {
    dir.map(|dir| dir.join("settings.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_portable_defaults_are_safe() {
        let platform = PortablePlatform;
        assert_eq!(platform.name(), "portable");
        assert_eq!(platform.dialogs().open_file("t", None), None);
        assert_eq!(platform.dialogs().choose_folder("t"), None);
        assert_eq!(platform.dialogs().save_file("t", "a", None), None);
        assert_eq!(platform.config_dir(), None);
        assert!(!platform.default_monospace_font().is_empty());
        assert!(!platform.prefers_dark());
        assert!(
            platform.file_system().is_none(),
            "no in-window dialogs by default"
        );
        assert!(matches!(platform.fs_policy(), FsPolicy::Unrestricted));
    }

    #[test]
    fn the_default_player_sits_next_to_the_running_program() {
        let player = PortablePlatform.player_executable().expect("has a path");
        let file = player.file_name().unwrap().to_string_lossy().into_owned();
        assert!(file.starts_with("lazyrad-player"));
    }

    #[test]
    fn current_falls_back_to_portable() {
        // No test installs a platform into the global, so this is the fallback.
        assert_eq!(current().name(), "portable");
    }

    #[test]
    fn a_settings_file_needs_a_directory() {
        assert_eq!(settings_file_in(None), None);
        assert_eq!(
            settings_file_in(Some(Path::new("cfg"))),
            Some(Path::new("cfg").join("settings.toml"))
        );
    }
}
