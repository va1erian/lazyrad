#![forbid(unsafe_code)]

//! The desktop implementation of [`lazyrad_runtime::platform::Platform`].
//!
//! This is the only place the IDE names a desktop OS crate: `rfd` (dialogs,
//! behind `native-dialogs`), `directories` (the config directory) and
//! `dark-light` (the theme, Windows and macOS only). Without the `desktop`
//! feature the module is not compiled and a LazyOS (or other) build installs its
//! own platform instead.

use std::path::PathBuf;
use std::process::Command;

use lazyrad_runtime::platform::{self, Dialogs, Platform};
#[cfg(feature = "native-dialogs")]
use lazyrad_runtime::platform::{FileFilter, Filter};

use super::process;

/// The config directory's application name.
const APP: &str = "LazyRAD";

/// The desktop host: native dialogs, the user config directory, the OS theme.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostPlatform;

/// Installs [`HostPlatform`] as the process's platform. Call first in `main`.
pub fn install() {
    // A second install means something already chose a platform (a test or a
    // different front end); keeping that choice is right, so ignore the refusal.
    let _ = platform::install(Box::new(HostPlatform));
}

impl Platform for HostPlatform {
    fn name(&self) -> &'static str {
        "desktop"
    }

    fn dialogs(&self) -> &dyn Dialogs {
        #[cfg(feature = "native-dialogs")]
        {
            &NativeDialogs
        }
        #[cfg(not(feature = "native-dialogs"))]
        {
            &lazyrad_runtime::platform::HeadlessDialogs
        }
    }

    fn config_dir(&self) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", APP)?;
        Some(dirs.config_dir().to_path_buf())
    }

    fn default_monospace_font(&self) -> &'static str {
        if cfg!(windows) {
            "Consolas"
        } else if cfg!(target_os = "macos") {
            "Menlo"
        } else {
            "DejaVu Sans Mono"
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn prefers_dark(&self) -> bool {
        matches!(dark_light::detect(), Ok(dark_light::Mode::Dark))
    }

    fn prepare_player_command(&self, command: &mut Command) {
        process::hide_console_window(command);
    }
}

/// The OS dialogs, through `rfd`.
#[cfg(feature = "native-dialogs")]
struct NativeDialogs;

#[cfg(feature = "native-dialogs")]
impl Dialogs for NativeDialogs {
    fn open_file(&self, title: &str, filter: Option<Filter<'_>>) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        if let Some((name, extensions)) = filter {
            dialog = dialog.add_filter(name, extensions);
        }
        dialog.pick_file()
    }

    fn open_file_filtered(&self, title: &str, filters: &[FileFilter]) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        for group in filters {
            let patterns: Vec<&str> = group.patterns.iter().map(String::as_str).collect();
            dialog = dialog.add_filter(&group.name, &patterns);
        }
        dialog.pick_file()
    }

    fn choose_folder(&self, title: &str) -> Option<PathBuf> {
        rfd::FileDialog::new().set_title(title).pick_folder()
    }

    fn save_file(
        &self,
        title: &str,
        file_name: &str,
        filter: Option<Filter<'_>>,
    ) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new()
            .set_title(title)
            .set_file_name(file_name);
        if let Some((name, extensions)) = filter {
            dialog = dialog.add_filter(name, extensions);
        }
        dialog.save_file()
    }

    fn show_error(&self, title: &str, message: &str) {
        let _ = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(title)
            .set_description(message)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_reports_its_name_and_a_font() {
        let host = HostPlatform;
        assert_eq!(host.name(), "desktop");
        assert!(!host.default_monospace_font().is_empty());
    }

    #[test]
    fn the_config_directory_is_named_after_the_app() {
        if let Some(dir) = HostPlatform.config_dir() {
            // `directories` lowercases the name on Linux (`~/.config/lazyrad`).
            let name = dir.to_string_lossy().to_lowercase();
            assert!(name.contains(&APP.to_lowercase()));
        }
    }
}
