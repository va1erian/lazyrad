#![forbid(unsafe_code)]

//! Where the IDE keeps per-user data, and the default monospace font, as the
//! installed platform reports them.

use std::path::{Path, PathBuf};

use lazyrad_runtime::platform;

/// The per-user configuration directory for the IDE.
///
/// `None` when the platform reports none (an unusual environment, or no
/// platform installed); the IDE then runs with in-memory settings.
pub fn config_dir() -> Option<PathBuf> {
    platform::current().config_dir()
}

/// The settings file inside `dir`, or `None` when there is no directory.
pub fn settings_file_in(dir: Option<&Path>) -> Option<PathBuf> {
    dir.map(|dir| dir.join("settings.toml"))
}

/// The IDE settings file's path (see [`config_dir`]).
pub fn settings_file() -> Option<PathBuf> {
    settings_file_in(config_dir().as_deref())
}

/// The monospace family the platform ships with (Consolas on Windows, Menlo on
/// macOS, DejaVu Sans Mono elsewhere and on any new platform).
pub fn default_monospace_font() -> &'static str {
    platform::current().default_monospace_font()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_config_directory_means_no_settings_file() {
        assert_eq!(settings_file_in(None), None);
    }

    #[test]
    fn the_settings_file_is_named_inside_the_directory() {
        let file = settings_file_in(Some(Path::new("cfg"))).expect("a directory gives a file");
        assert_eq!(file, Path::new("cfg").join("settings.toml"));
    }

    #[test]
    fn the_default_font_is_never_empty() {
        assert!(!default_monospace_font().is_empty());
    }
}
