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
use std::sync::{Arc, Mutex, OnceLock};

use crate::fs_policy::FsPolicy;

/// `(description, extensions)` of a file name filter.
pub type Filter<'a> = (&'a str, &'a [&'a str]);

/// One filter group a file dialog offers: a display name and its patterns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFilter {
    /// The name shown in the dialog's filter list.
    pub name: String,
    /// The glob patterns (`*.mod`) the group matches.
    pub patterns: Vec<String>,
}

/// Parses a script's filter string into filter groups.
///
/// The syntax is `"name|pattern;name|pattern"`: groups are separated by `;`,
/// each group is a display name and its patterns after a `|`, and several
/// patterns in one group are separated by `,`. `"MOD files|*.mod;All files|*.*"`
/// is two groups. An empty or malformed group is skipped.
pub fn parse_filters(spec: &str) -> Vec<FileFilter> {
    spec.split(';')
        .filter_map(|group| {
            let group = group.trim();
            if group.is_empty() {
                return None;
            }
            let (name, patterns) = group.split_once('|').unwrap_or((group, "*"));
            let patterns: Vec<String> = patterns
                .split(',')
                .map(str::trim)
                .filter(|pattern| !pattern.is_empty())
                .map(str::to_owned)
                .collect();
            Some(FileFilter {
                name: name.trim().to_owned(),
                patterns,
            })
        })
        .collect()
}

/// The extension tokens a native dialog wants for one filter group's glob
/// patterns.
///
/// Scripts write filters as globs (`*.mod`, `*.*`), but native dialog toolkits
/// (`rfd`'s `add_filter`) take bare extensions (`mod`). Each `*.ext` becomes
/// `ext`; `*` and `*.*` mean "every file" and become the single token `*`,
/// which swallows the rest of the group. Anything that is not a plain extension
/// glob (`data*`, `*.{png,jpg}`, `sub/*.txt`, `*.`) cannot be expressed as an
/// extension and is skipped. Duplicates are dropped, order is kept.
pub fn extension_tokens(patterns: &[String]) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for pattern in patterns {
        let pattern = pattern.trim();
        if pattern == "*" || pattern == "*.*" {
            return vec!["*".to_owned()];
        }
        let Some(extension) = pattern.strip_prefix("*.") else {
            continue;
        };
        let plain = !extension.is_empty()
            && !extension.chars().any(|c| {
                c.is_whitespace() || matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '/' | '\\')
            });
        if plain && !tokens.iter().any(|token| token == extension) {
            tokens.push(extension.to_owned());
        }
    }
    tokens
}

/// The filter groups a native dialog should offer: each group's display name
/// with its [`extension_tokens`]. A group with no usable extension is left out
/// rather than offered as an empty filter that would hide every file.
pub fn dialog_filters(filters: &[FileFilter]) -> Vec<(String, Vec<String>)> {
    filters
        .iter()
        .filter_map(|group| {
            let tokens = extension_tokens(&group.patterns);
            (!tokens.is_empty()).then(|| (group.name.clone(), tokens))
        })
        .collect()
}

/// Called once with the file a dialog picked, or `None` when it was cancelled.
/// It may run on any thread.
pub type FileDone = Box<dyn FnOnce(Option<PathBuf>) + Send>;

/// Runs `pick` (a native dialog) on a worker thread and hands its answer to
/// `done`, so the window keeps running while the dialog is open.
///
/// If the thread cannot be started, `done` is answered with `None` at once,
/// so a script's request never stays pending.
pub fn pick_on_worker(pick: impl FnOnce() -> Option<PathBuf> + Send + 'static, done: FileDone) {
    pick_with(
        |work| {
            std::thread::Builder::new()
                .name("open-file-dialog".to_owned())
                .spawn(work)
                .map(drop)
        },
        pick,
        done,
    );
}

/// [`pick_on_worker`] with the thread spawner supplied, so the failure path
/// can be tested.
fn pick_with(
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
    pick: impl FnOnce() -> Option<PathBuf> + Send + 'static,
    done: FileDone,
) {
    // `done` is shared with the worker, so the spawn's failure path can still
    // answer when the worker never runs.
    let slot = Arc::new(Mutex::new(Some(done)));
    let worker_slot = Arc::clone(&slot);
    let take = |slot: &Mutex<Option<FileDone>>| {
        slot.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    };
    let spawned = spawn(Box::new(move || {
        let picked = pick();
        if let Some(done) = take(&worker_slot) {
            done(picked);
        }
    }));
    if let Err(error) = spawned {
        eprintln!("lazyrad: cannot start the file dialog: {error}");
        if let Some(done) = take(&slot) {
            done(None);
        }
    }
}

/// File, folder and message dialogs.
///
/// Every method answers "cancelled" (`None`) when no dialog can be shown, so a
/// caller never blocks on a dialog that cannot appear. On LazyOS the dialogs are
/// painted in-window, so an implementation may only be able to answer from a
/// previously stored result; see the LazyOS seam for how it is fed.
pub trait Dialogs: Send + Sync {
    /// Asks for an existing file.
    fn open_file(&self, title: &str, filter: Option<Filter<'_>>) -> Option<PathBuf>;
    /// Asks for an existing file, offering several filter groups.
    ///
    /// The default offers only the first group, through [`Dialogs::open_file`],
    /// so an implementation that supports one filter needs nothing more.
    fn open_file_filtered(&self, title: &str, filters: &[FileFilter]) -> Option<PathBuf> {
        match dialog_filters(filters).first() {
            Some((name, tokens)) => {
                let tokens: Vec<&str> = tokens.iter().map(String::as_str).collect();
                self.open_file(title, Some((name, &tokens)))
            }
            None => self.open_file(title, None),
        }
    }
    /// Asks for an existing file without blocking the caller: `done` receives
    /// the answer later, from any thread.
    ///
    /// A script's `open_file_dialog` uses this, so the window keeps running while
    /// the dialog is up. An implementation must return promptly and call `done`
    /// exactly once. The default answers at once through
    /// [`Dialogs::open_file_filtered`], which is right for a platform whose
    /// dialogs never block (the headless ones cancel immediately); a desktop
    /// platform overrides it to show the dialog on a worker thread.
    fn open_file_async(&self, title: &str, filters: &[FileFilter], done: FileDone) {
        done(self.open_file_filtered(title, filters));
    }
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

    /// The files the program was started to open (a picture double-clicked in
    /// the file manager), which scripts read as `app.documents`. Empty by
    /// default. A sandboxing platform's [`Platform::fs_policy`] must grant
    /// read access to each of them (LazyOS also grants its folder, so a
    /// viewer can page through the neighbours).
    fn documents(&self) -> Vec<PathBuf> {
        Vec::new()
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

    /// A `done` that records what it was answered with.
    fn recording() -> (Arc<Mutex<Vec<Option<PathBuf>>>>, FileDone) {
        let answers: Arc<Mutex<Vec<Option<PathBuf>>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&answers);
        let done: FileDone = Box::new(move |picked| sink.lock().unwrap().push(picked));
        (answers, done)
    }

    #[test]
    fn a_worker_answers_with_the_picked_path() {
        let (answers, done) = recording();
        pick_with(
            |work| {
                work();
                Ok(())
            },
            || Some(PathBuf::from("song.mod")),
            done,
        );
        assert_eq!(*answers.lock().unwrap(), [Some(PathBuf::from("song.mod"))]);
    }

    #[test]
    fn a_worker_that_cannot_start_answers_none_at_once() {
        let (answers, done) = recording();
        pick_with(
            |_work| Err(std::io::Error::other("no threads left")),
            || panic!("the dialog never opens"),
            done,
        );
        assert_eq!(*answers.lock().unwrap(), [None]);
    }

    #[test]
    fn a_real_worker_thread_answers() {
        let (answers, done) = recording();
        pick_on_worker(|| Some(PathBuf::from("a.txt")), done);
        for _ in 0..200 {
            if !answers.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(*answers.lock().unwrap(), [Some(PathBuf::from("a.txt"))]);
    }
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
    fn filter_groups_parse_from_the_script_syntax() {
        let filters = parse_filters("MOD files|*.mod;All files|*.*");
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].name, "MOD files");
        assert_eq!(filters[0].patterns, ["*.mod"]);
        assert_eq!(filters[1].name, "All files");
        assert_eq!(filters[1].patterns, ["*.*"]);

        // Several patterns in one group, and a bare name gets a catch-all.
        let filters = parse_filters("Images|*.png,*.jpg;Everything");
        assert_eq!(filters[0].patterns, ["*.png", "*.jpg"]);
        assert_eq!(filters[1].patterns, ["*"]);
        assert!(parse_filters("").is_empty());
        assert!(parse_filters(" ; ;").is_empty());
    }

    #[test]
    fn glob_patterns_become_extension_tokens() {
        let tokens = |patterns: &[&str]| {
            let patterns: Vec<String> = patterns.iter().map(|p| (*p).to_owned()).collect();
            extension_tokens(&patterns)
        };
        assert_eq!(tokens(&["*.mod"]), ["mod"]);
        assert_eq!(tokens(&["*.png", " *.jpg ", "*.png"]), ["png", "jpg"]);
        assert_eq!(tokens(&["*.tar.gz"]), ["tar.gz"]);
        // "Every file" in either spelling wins over the rest of the group.
        assert_eq!(tokens(&["*"]), ["*"]);
        assert_eq!(tokens(&["*.*"]), ["*"]);
        assert_eq!(tokens(&["*.mod", "*.*"]), ["*"]);
        // Not a plain extension: skipped.
        assert!(tokens(&["data*", "*.", "*.{png,jpg}", "sub/*.txt", "*.t?t", ""]).is_empty());
        assert_eq!(tokens(&["data*", "*.mod"]), ["mod"]);
    }

    #[test]
    fn dialog_filters_drop_groups_without_extensions() {
        let filters = parse_filters("MOD files|*.mod;Odd|data*;All files|*.*");
        assert_eq!(
            dialog_filters(&filters),
            [
                ("MOD files".to_owned(), vec!["mod".to_owned()]),
                ("All files".to_owned(), vec!["*".to_owned()]),
            ]
        );
    }

    #[test]
    fn the_default_async_dialog_answers_at_once() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let filters = parse_filters("MOD files|*.mod");
        HeadlessDialogs.open_file_async(
            "Open",
            &filters,
            Box::new(move |picked| {
                let _ = sender.send(picked);
            }),
        );
        assert_eq!(
            receiver.try_recv(),
            Ok(None),
            "cancelled, and already answered"
        );
    }

    #[test]
    fn the_default_filtered_dialog_offers_the_first_group() {
        // `HeadlessDialogs` cancels, so this only checks the default does not
        // panic with several groups.
        let filters = parse_filters("MOD files|*.mod;All files|*.*");
        assert_eq!(HeadlessDialogs.open_file_filtered("Open", &filters), None);
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
