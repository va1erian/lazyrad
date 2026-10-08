#![forbid(unsafe_code)]

//! Hot reload: re-run a project's forms when its files change (issue #91).
//!
//! A player started with `--watch` polls the modification time and size of the
//! `.lrp` and of every `.lfm` / `.rhai` the project references, every
//! [`WATCH_INTERVAL_MS`] on the startup window's timer. When one changes,
//! [`FormRuntime::check_for_changes`] checks and loads a fresh runtime from
//! disk **before** touching the running forms (failure atomicity, checklist 3):
//!
//! * a project that does not check keeps the running forms and reports the
//!   diagnostics as a [`ReloadOutcome::Failed`], which the window shows in a
//!   non-blocking banner (never a dialog per keystroke);
//! * a clean project swaps its forms and modules into the running runtime and
//!   returns [`ReloadOutcome::Reloaded`], and every open window rebuilds its
//!   form through [`FormApp::reload_root`](crate::FormApp::reload_root).
//!
//! The polling needs no file-watch API (LazyOS has none) and no extra thread.
//! A half-written file simply fails once and is picked up again when it changes.
//!
//! # What a reload carries
//!
//! The new form's `form_load` runs as usual, and its event sources are
//! registered afresh. If the new script defines `fn form_reload(old_state)`, the
//! old form's [`form.state`](crate::FormRuntime) is passed to it after
//! `form_load`; a script that does not opt in starts with fresh state.
//!
//! A project that does not check keeps the running forms, but a reload that
//! checks and builds is committed: a runtime error in the new `form_load` is
//! reported in the banner, the new form stays, and `form_reload(old_state)`
//! still runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use crate::check::check_project;
use crate::form::{FormRuntime, RuntimeError, open_project};

/// How often the watch timer polls the project's files, in milliseconds.
pub const WATCH_INTERVAL_MS: u32 = 500;

/// What [`FormRuntime::check_for_changes`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// No watched file changed since the last check.
    Unchanged,
    /// The project reloaded; every open form window should rebuild its form.
    Reloaded,
    /// The changed project did not check. The running forms and the old sources
    /// are untouched, and these are the diagnostics to show in the banner.
    Failed(Vec<String>),
}

/// A file's modification time and length. `modified` is `None` when the
/// platform cannot report one; a file that does not exist reads as `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl Stamp {
    /// The stamp of the file at `path`, or `None` if it cannot be read.
    ///
    /// A file that vanished between listing and reading is `None`, never an
    /// error: a reload that races a deletion must not panic (checklist 6).
    fn read(path: &Path) -> Option<Stamp> {
        let metadata = fs::metadata(path).ok()?;
        Some(Stamp {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        })
    }
}

/// One watched file and the stamp last seen for it.
#[derive(Clone, Debug)]
struct WatchedFile {
    path: PathBuf,
    stamp: Option<Stamp>,
}

/// Polls the files a project references for changes.
pub(crate) struct Watcher {
    path: PathBuf,
    files: Vec<WatchedFile>,
}

impl Watcher {
    /// Watches the project named by `path` (a directory or an `.lrp`),
    /// snapshotting the files `runtime` was loaded from.
    pub(crate) fn new(path: PathBuf, runtime: &FormRuntime) -> Watcher {
        let mut watcher = Watcher {
            path,
            files: Vec::new(),
        };
        watcher.refresh(runtime);
        watcher
    }

    /// Re-derives the watched files from `runtime`'s current sources and takes
    /// a fresh snapshot. Called after a successful reload, when the project may
    /// reference a different set of files.
    pub(crate) fn refresh(&mut self, runtime: &FormRuntime) {
        let dir = runtime.path().to_path_buf();
        let mut paths: Vec<PathBuf> = Vec::new();
        {
            let sources = runtime.sources.borrow();
            paths.push(dir.join(sources.project.file_name()));
            for file in sources.project.referenced_files() {
                paths.push(dir.join(file));
            }
        }
        self.files = paths
            .into_iter()
            .map(|path| WatchedFile {
                stamp: Stamp::read(&path),
                path,
            })
            .collect();
    }

    /// Re-derives the watched files from the `.lrp` on disk, after a reload
    /// failed.
    ///
    /// The edited project may reference files the last good one did not (a new
    /// item whose files are not created yet). Watching them lets creating the
    /// file trigger the next attempt, instead of leaving the failure banner up
    /// until some other file changes. A file already watched keeps its stamp,
    /// so a file that did not change since the failed check does not retrigger;
    /// a newly referenced file is stamped as it is now (`None` when missing).
    /// A `.lrp` that does not parse leaves the set as it was: the `.lrp` itself
    /// is always watched, so fixing it triggers the next attempt.
    pub(crate) fn refresh_from_disk(&mut self) {
        let Ok((dir, project)) = open_project(&self.path) else {
            return;
        };
        let mut paths: Vec<PathBuf> = vec![dir.join(project.file_name())];
        paths.extend(project.referenced_files().map(|file| dir.join(file)));
        let known = std::mem::take(&mut self.files);
        self.files = paths
            .into_iter()
            .map(|path| {
                let stamp = match known.iter().find(|file| file.path == path) {
                    Some(file) => file.stamp,
                    None => Stamp::read(&path),
                };
                WatchedFile { path, stamp }
            })
            .collect();
    }

    /// The project path to check and reload from.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Whether any watched file changed since the last check, updating every
    /// stamp so a change is reported once.
    pub(crate) fn changed(&mut self) -> bool {
        let mut changed = false;
        for file in &mut self.files {
            let stamp = Stamp::read(&file.path);
            if stamp != file.stamp {
                file.stamp = stamp;
                changed = true;
            }
        }
        changed
    }
}

impl FormRuntime {
    /// Enables hot reload, watching the project at `path`.
    ///
    /// `path` is the same directory or `.lrp` the runtime was loaded from, so a
    /// change is checked and reloaded the same way.
    pub fn enable_watch(&self, path: impl Into<PathBuf>) {
        let watcher = Watcher::new(path.into(), self);
        *self.watch.borrow_mut() = Some(watcher);
    }

    /// Whether hot reload is enabled on this runtime.
    pub fn watch_enabled(&self) -> bool {
        self.watch.borrow().is_some()
    }

    /// Checks the watched project for changes and, when one is found, checks
    /// and loads it afresh.
    ///
    /// A clean project's forms and modules are swapped into this runtime
    /// (checklist 3: the fresh runtime is built first, so a failure leaves the
    /// old sources intact). A project that does not check changes nothing and
    /// returns its diagnostics.
    pub fn check_for_changes(&self) -> ReloadOutcome {
        let path = {
            let mut watch = self.watch.borrow_mut();
            let Some(watcher) = watch.as_mut() else {
                return ReloadOutcome::Unchanged;
            };
            if !watcher.changed() {
                return ReloadOutcome::Unchanged;
            }
            watcher.path().to_path_buf()
        };
        match reload_project(&path) {
            Ok(fresh) => {
                *self.sources.borrow_mut() = fresh.sources.borrow().clone();
                if let Some(watcher) = self.watch.borrow_mut().as_mut() {
                    watcher.refresh(self);
                }
                ReloadOutcome::Reloaded
            }
            Err(diagnostics) => {
                // Watch the files the edited `.lrp` references, so creating a
                // missing one triggers the next attempt.
                if let Some(watcher) = self.watch.borrow_mut().as_mut() {
                    watcher.refresh_from_disk();
                }
                ReloadOutcome::Failed(diagnostics)
            }
        }
    }
}

/// Checks and loads the project at `path`, returning the fresh runtime or the
/// diagnostics that stopped it.
fn reload_project(path: &Path) -> Result<Rc<FormRuntime>, Vec<String>> {
    match check_project(path) {
        Ok(report) if report.is_empty() => {}
        Ok(report) => {
            let mut diagnostics: Vec<String> =
                report.diagnostics.iter().map(ToString::to_string).collect();
            diagnostics.extend(report.scripts.iter().map(ToString::to_string));
            return Err(diagnostics);
        }
        Err(error) => return Err(vec![error.to_string()]),
    }
    FormRuntime::load_path(path).map_err(|error: RuntimeError| vec![error.to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A scratch project directory for one test, emptied first.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "lazyrad-runtime-reload-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("scratch directory is created");
        path
    }

    /// Writes a minimal one-form project named `check` into `dir`.
    fn write_project(dir: &Path, code: &str) {
        fs::write(
            dir.join("check.lrp"),
            "name = \"check\"\nversion = \"0.1.0\"\nstartup = \"main_form\"\n\n\
             [[items]]\nkind = \"form\"\nname = \"main_form\"\n\
             layout = \"main_form.lfm\"\ncode = \"main_form.rhai\"\n",
        )
        .expect("project writes");
        fs::write(
            dir.join("main_form.lfm"),
            "format = 1\n\n[window]\nname = \"main_form\"\ntitle = \"Check\"\n",
        )
        .expect("form writes");
        fs::write(dir.join("main_form.rhai"), code).expect("code writes");
    }

    #[test]
    fn an_unchanged_project_does_not_reload() {
        let dir = scratch("unchanged");
        write_project(&dir, "fn form_load() {}");
        let runtime = FormRuntime::load(&dir).expect("the project loads");
        runtime.enable_watch(&dir);

        assert_eq!(runtime.check_for_changes(), ReloadOutcome::Unchanged);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edited_script_swaps_the_runtime_sources() {
        let dir = scratch("edited");
        write_project(&dir, "fn form_load() {}");
        let runtime = FormRuntime::load(&dir).expect("the project loads");
        runtime.enable_watch(&dir);

        fs::write(dir.join("main_form.rhai"), "fn form_load() { let x = 1; }")
            .expect("code writes");
        assert_eq!(runtime.check_for_changes(), ReloadOutcome::Reloaded);
        assert!(
            runtime
                .form("main_form")
                .expect("the form is there")
                .code
                .contains("let x = 1"),
            "the new source is live"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_script_keeps_the_old_sources_and_reports_diagnostics() {
        let dir = scratch("broken");
        write_project(&dir, "fn form_load() {}");
        let runtime = FormRuntime::load(&dir).expect("the project loads");
        runtime.enable_watch(&dir);

        fs::write(dir.join("main_form.rhai"), "fn broken() { let x = ; }").expect("code writes");
        let outcome = runtime.check_for_changes();
        let ReloadOutcome::Failed(diagnostics) = outcome else {
            panic!("a broken script must fail, got {outcome:?}");
        };
        assert!(!diagnostics.is_empty(), "the diagnostics name the problem");
        assert!(
            runtime
                .form("main_form")
                .expect("the form is there")
                .code
                .contains("fn form_load"),
            "the old source is intact"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deleted_script_does_not_panic_and_reports_diagnostics() {
        let dir = scratch("deleted");
        write_project(&dir, "fn form_load() {}");
        let runtime = FormRuntime::load(&dir).expect("the project loads");
        runtime.enable_watch(&dir);

        fs::remove_file(dir.join("main_form.rhai")).expect("the script is removed");
        let outcome = runtime.check_for_changes();
        assert!(
            matches!(outcome, ReloadOutcome::Failed(_)),
            "a missing referenced file is a diagnostic, got {outcome:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_removed_reference_is_dropped_from_the_watched_set_after_a_reload() {
        let dir = scratch("refresh");
        fs::write(
            dir.join("check.lrp"),
            "name = \"check\"\nversion = \"0.1.0\"\nstartup = \"main_form\"\n\n\
             [[items]]\nkind = \"form\"\nname = \"main_form\"\n\
             layout = \"main_form.lfm\"\ncode = \"main_form.rhai\"\n\n\
             [[items]]\nkind = \"module\"\nname = \"util\"\ncode = \"util.rhai\"\n",
        )
        .expect("project writes");
        fs::write(
            dir.join("main_form.lfm"),
            "format = 1\n\n[window]\nname = \"main_form\"\n",
        )
        .expect("form writes");
        fs::write(dir.join("main_form.rhai"), "fn form_load() {}").expect("code writes");
        fs::write(dir.join("util.rhai"), "fn helper() { 1 }").expect("module writes");
        let runtime = FormRuntime::load(&dir).expect("the project loads");
        runtime.enable_watch(&dir);

        // Drop the module item and its file: a clean one-form project again.
        fs::write(
            dir.join("check.lrp"),
            "name = \"check\"\nversion = \"0.1.0\"\nstartup = \"main_form\"\n\n\
             [[items]]\nkind = \"form\"\nname = \"main_form\"\n\
             layout = \"main_form.lfm\"\ncode = \"main_form.rhai\"\n",
        )
        .expect("project writes");
        fs::remove_file(dir.join("util.rhai")).expect("the module is removed");
        assert_eq!(runtime.check_for_changes(), ReloadOutcome::Reloaded);

        // The removed file is no longer watched, so recreating it is a no-op.
        fs::write(dir.join("util.rhai"), "fn helper() { 2 }").expect("module returns");
        assert_eq!(
            runtime.check_for_changes(),
            ReloadOutcome::Unchanged,
            "a file the project no longer references is not watched"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_created_after_a_failed_reload_is_watched_and_reloads() {
        let dir = scratch("late-file");
        write_project(&dir, "fn form_load() {}");
        let runtime = FormRuntime::load(&dir).expect("the project loads");
        runtime.enable_watch(&dir);

        // A new form item, before its files exist: the reload fails.
        let mut lrp = fs::read_to_string(dir.join("check.lrp")).expect("project reads");
        lrp.push_str(
            "\n[[items]]\nkind = \"form\"\nname = \"second_form\"\n\
             layout = \"second_form.lfm\"\ncode = \"second_form.rhai\"\n",
        );
        fs::write(dir.join("check.lrp"), lrp).expect("project writes");
        assert!(
            matches!(runtime.check_for_changes(), ReloadOutcome::Failed(_)),
            "the missing files fail the reload"
        );
        assert_eq!(
            runtime.check_for_changes(),
            ReloadOutcome::Unchanged,
            "nothing changed since the failed check"
        );

        // Creating the files is what the next check must notice.
        fs::write(
            dir.join("second_form.lfm"),
            "format = 1\n\n[window]\nname = \"second_form\"\n",
        )
        .expect("layout writes");
        fs::write(dir.join("second_form.rhai"), "fn form_load() {}").expect("code writes");
        assert_eq!(runtime.check_for_changes(), ReloadOutcome::Reloaded);
        assert!(
            runtime.form("second_form").is_some(),
            "the new form is live"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn watching_is_off_by_default() {
        let dir = scratch("off");
        write_project(&dir, "fn form_load() {}");
        let runtime = FormRuntime::load(&dir).expect("the project loads");
        assert!(!runtime.watch_enabled());
        assert_eq!(runtime.check_for_changes(), ReloadOutcome::Unchanged);
        let _ = fs::remove_dir_all(&dir);
    }
}
