#![forbid(unsafe_code)]

//! IDE settings: theme choice, editor font size, recent projects and pane
//! sizes, persisted as TOML in the OS config directory (PLAN.md §9).
//!
//! The file lives under `directories`'s per-user config directory, so the IDE
//! remembers its layout and theme between runs. Loading is forgiving: a missing
//! or partial file yields defaults rather than an error, and a corrupt file is
//! reported to the caller, which then falls back to defaults.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The number of recent projects kept.
pub const RECENT_LIMIT: usize = 10;

/// The config directory's application name.
const APP: &str = "LazyRAD";

/// A temporary path next to `path` that no other save uses: two IDE instances
/// saving at once, or two saves in one process, never share a temporary file.
fn unique_temp_path(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "settings".to_owned());
    path.with_file_name(format!(
        ".{name}.{}.{nanos}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ))
}

/// Which theme the IDE should use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// The light palette.
    #[default]
    Light,
    /// The dark palette.
    Dark,
    /// Follow the operating system's light/dark preference.
    System,
}

impl ThemeChoice {
    /// Every choice, for a radio menu.
    pub const ALL: &'static [ThemeChoice] =
        &[ThemeChoice::Light, ThemeChoice::Dark, ThemeChoice::System];

    /// The choice's menu label.
    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
            ThemeChoice::System => "System",
        }
    }
}

/// The persisted sizes of the IDE's docked panes, in design units.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneSizes {
    /// The toolbox's width on the left.
    pub toolbox: f32,
    /// The width of the Project/Properties column on the right.
    pub right: f32,
    /// The Output pane's height at the bottom.
    pub output: f32,
    /// The Project pane's height within the right column.
    pub project: f32,
}

impl Default for PaneSizes {
    fn default() -> PaneSizes {
        PaneSizes {
            toolbox: 140.0,
            right: 240.0,
            output: 160.0,
            project: 220.0,
        }
    }
}

/// Everything the IDE persists.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The chosen theme.
    pub theme: ThemeChoice,
    /// The code editor's font size, in design points.
    pub editor_font_size: f32,
    /// The most recently opened projects, newest first.
    pub recent_projects: Vec<PathBuf>,
    /// The docked pane sizes.
    pub panes: PaneSizes,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            theme: ThemeChoice::Light,
            editor_font_size: 12.0,
            recent_projects: Vec::new(),
            panes: PaneSizes::default(),
        }
    }
}

impl Settings {
    /// The settings file's path: the per-user config directory plus
    /// `settings.toml`.
    ///
    /// `None` when the platform reports no config directory (an unusual
    /// environment); the IDE then runs with in-memory defaults.
    pub fn path() -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", APP)?;
        Some(dirs.config_dir().join("settings.toml"))
    }

    /// Reads the settings file, or returns the defaults when it is missing.
    ///
    /// A file that exists but cannot be read or parsed is an error, so the
    /// caller can tell the user their settings were ignored.
    pub fn load() -> Result<Settings, SettingsError> {
        match Settings::path() {
            Some(path) => Settings::load_from(&path),
            None => Ok(Settings::default()),
        }
    }

    /// Reads settings from `path`; a missing file yields the defaults.
    pub fn load_from(path: &Path) -> Result<Settings, SettingsError> {
        if !path.exists() {
            return Ok(Settings::default());
        }
        let text = std::fs::read_to_string(path).map_err(SettingsError::Io)?;
        Settings::from_toml(&text)
    }

    /// Writes the settings to the default path, creating the directory.
    pub fn save(&self) -> Result<(), SettingsError> {
        match Settings::path() {
            Some(path) => self.save_to(&path),
            None => Ok(()),
        }
    }

    /// Writes the settings to `path`, creating its parent directory.
    ///
    /// The write is atomic: the text goes to a temporary file next to `path`,
    /// which is then renamed over it, so a crash mid-write never leaves a
    /// truncated settings file behind.
    pub fn save_to(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(SettingsError::Io)?;
        }
        let text = self.to_toml()?;
        let temp = unique_temp_path(path);
        std::fs::write(&temp, text).map_err(|error| {
            let _ = std::fs::remove_file(&temp);
            SettingsError::Io(error)
        })?;
        std::fs::rename(&temp, path).map_err(|error| {
            let _ = std::fs::remove_file(&temp);
            SettingsError::Io(error)
        })
    }

    /// Serialises the settings to TOML.
    pub fn to_toml(&self) -> Result<String, SettingsError> {
        toml::to_string_pretty(self).map_err(SettingsError::Serialize)
    }

    /// Parses settings from TOML.
    pub fn from_toml(text: &str) -> Result<Settings, SettingsError> {
        toml::from_str(text).map_err(SettingsError::Parse)
    }

    /// Records `path` as the most recent project, moving it to the front and
    /// trimming the list to [`RECENT_LIMIT`].
    pub fn push_recent(&mut self, path: PathBuf) {
        self.recent_projects.retain(|existing| existing != &path);
        self.recent_projects.insert(0, path);
        self.recent_projects.truncate(RECENT_LIMIT);
    }
}

/// Why loading or saving settings failed.
#[derive(Debug)]
pub enum SettingsError {
    /// The file could not be read or written.
    Io(std::io::Error),
    /// The file was not valid TOML.
    Parse(toml::de::Error),
    /// The settings could not be serialised to TOML.
    Serialize(toml::ser::Error),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsError::Io(error) => write!(f, "settings file: {error}"),
            SettingsError::Parse(error) => write!(f, "settings file: {error}"),
            SettingsError::Serialize(error) => {
                write!(f, "settings could not be written: {error}")
            }
        }
    }
}

impl std::error::Error for SettingsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let settings = Settings::default();
        let text = settings.to_toml().expect("settings serialise");
        let parsed = Settings::from_toml(&text).expect("settings parse");
        assert_eq!(parsed, settings);
    }

    #[test]
    fn a_partial_file_fills_in_defaults() {
        let parsed = Settings::from_toml("theme = \"dark\"\n").expect("a partial file parses");
        assert_eq!(parsed.theme, ThemeChoice::Dark);
        assert_eq!(
            parsed.editor_font_size,
            Settings::default().editor_font_size
        );
        assert_eq!(parsed.panes, PaneSizes::default());
    }

    #[test]
    fn recent_projects_are_deduplicated_newest_first_and_capped() {
        let mut settings = Settings::default();
        for index in 0..RECENT_LIMIT + 5 {
            settings.push_recent(PathBuf::from(format!("project-{index}")));
        }
        assert_eq!(settings.recent_projects.len(), RECENT_LIMIT);
        assert_eq!(
            settings.recent_projects[0],
            PathBuf::from(format!("project-{}", RECENT_LIMIT + 4))
        );

        settings.push_recent(PathBuf::from("project-5"));
        assert_eq!(settings.recent_projects.len(), RECENT_LIMIT);
        assert_eq!(settings.recent_projects[0], PathBuf::from("project-5"));
        assert_eq!(
            settings
                .recent_projects
                .iter()
                .filter(|path| path.ends_with("project-5"))
                .count(),
            1,
            "the path appears once"
        );
    }

    #[test]
    fn saving_and_loading_a_file_round_trips() {
        let dir =
            std::env::temp_dir().join(format!("lazyrad-settings-test-{}", std::process::id()));
        let path = dir.join("settings.toml");
        let mut settings = Settings {
            theme: ThemeChoice::System,
            ..Settings::default()
        };
        settings.push_recent(PathBuf::from("hello.lrp"));
        settings.save_to(&path).expect("settings save");

        let loaded = Settings::load_from(&path).expect("settings load");
        assert_eq!(loaded, settings);

        // A second save replaces the file and leaves no temporary behind.
        settings.theme = ThemeChoice::Dark;
        settings
            .save_to(&path)
            .expect("settings save over an existing file");
        assert_eq!(Settings::load_from(&path).expect("settings load"), settings);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("settings directory is readable")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temporary files left: {leftovers:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_save_gets_its_own_temporary_file() {
        let path = Path::new("config").join("settings.toml");
        let first = unique_temp_path(&path);
        let second = unique_temp_path(&path);
        assert_ne!(first, second);
        assert_eq!(first.parent(), path.parent(), "renames stay on one volume");
    }

    #[test]
    fn a_missing_file_loads_as_defaults() {
        let path = std::env::temp_dir().join("lazyrad-settings-does-not-exist.toml");
        assert_eq!(
            Settings::load_from(&path).expect("a missing file is not an error"),
            Settings::default()
        );
    }
}
