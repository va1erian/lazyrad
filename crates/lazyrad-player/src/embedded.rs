#![forbid(unsafe_code)]

//! Running an exported app: the player finds a project appended to its own
//! executable and runs it from memory.
//!
//! [`load_from_exe`] is the whole seam. It opens an executable, looks for the
//! payload footer ([`lazyrad_packager::Payload`]) and, if there is one, builds
//! the runtime from the payload's files without writing anything to disk. The
//! same checks the player makes on a project folder (form validation, script
//! compilation) run on the in-memory project, because a payload is input like
//! any other: it may be damaged or hand-made.
//!
//! # Windows GUI subsystem
//!
//! The shipped player is a console program, so the IDE can read a running
//! project's output. Export flips the *copy's* PE subsystem to GUI, so an
//! exported app has no console and nowhere to print. [`show_failure`] therefore
//! reports a startup failure in a native message box, as well as on stderr.

use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use lazyrad_packager::payload::ASSET_PREFIX;
use lazyrad_packager::{Payload, PayloadError};
use lazyrad_project::Project;
use lazyrad_runtime::{FormRuntime, ProjectFiles, RuntimeError, check_runtime};

use crate::Report;

/// The project's files as the executable's payload holds them: an item's plain
/// name, or an asset under the payload's `assets/` prefix.
///
/// It serves what [`lazyrad_runtime::DiskProject`] serves from a folder, so a
/// script reads the same files either way: the files the project's items
/// reference (by their exact names) and the assets (`assets/{relative}`). It
/// never serves the `.lrp` or any other payload entry, and the payload's own
/// `assets/` prefix is not visible to a script: asking for `assets/x` looks for
/// the project asset `assets/x`, not the raw entry for `x`.
struct PayloadFiles {
    payload: Payload,
    /// The item files the project references, by their project-relative names.
    referenced: BTreeSet<String>,
}

impl PayloadFiles {
    fn new(payload: Payload, project: &Project) -> PayloadFiles {
        PayloadFiles {
            payload,
            referenced: project
                .referenced_files()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .collect(),
        }
    }
}

impl ProjectFiles for PayloadFiles {
    fn read(&self, relative: &str) -> Option<Vec<u8>> {
        let entry = if self.referenced.contains(relative) {
            relative.to_owned()
        } else {
            format!("{ASSET_PREFIX}{relative}")
        };
        self.payload.get(&entry).map(<[u8]>::to_vec)
    }
}

/// The runtime for the project appended to `exe`, or `None` when `exe` carries
/// no payload (an ordinary player).
///
/// Every failure is returned as [`Report`]s: a damaged, truncated or
/// wrong-version payload, a project that does not validate, a script that does
/// not compile.
pub fn load_from_exe(exe: &Path) -> Result<Option<Rc<FormRuntime>>, Vec<Report>> {
    let mut file = match File::open(exe) {
        Ok(file) => file,
        // An executable that cannot be reopened is not evidence of a payload;
        // behave as a bare player rather than block it.
        Err(_) => return Ok(None),
    };
    let payload = match Payload::read_from(&mut file) {
        Ok(Some(payload)) => payload,
        Ok(None) => return Ok(None),
        Err(error) => return Err(vec![payload_report(exe, &error)]),
    };
    let dir = exe
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    runtime_from_payload(&payload, dir).map(Some)
}

/// Builds the runtime for `payload`; `dir` is what scripts see as `app.path`.
pub fn runtime_from_payload(
    payload: &Payload,
    dir: PathBuf,
) -> Result<Rc<FormRuntime>, Vec<Report>> {
    let project_entry = payload.project_entry();
    let text = std::str::from_utf8(&project_entry.data).map_err(|_| {
        vec![payload_report(
            &dir,
            &PayloadError::Corrupt(format!("`{}` is not valid UTF-8", project_entry.name)),
        )]
    })?;
    let project = Project::parse(Path::new(&project_entry.name), text)
        .map_err(|error| vec![Report::from_load_error(&RuntimeError::from(error))])?;

    let files: Rc<dyn ProjectFiles> = Rc::new(PayloadFiles::new(payload.clone(), &project));
    let runtime = FormRuntime::from_project(project, dir, files)
        .map_err(|error| vec![Report::from_load_error(&error)])?;

    let check = check_runtime(&runtime);
    if check.is_empty() {
        return Ok(runtime);
    }
    let mut reports: Vec<Report> = check
        .diagnostics
        .iter()
        .map(Report::from_diagnostic)
        .collect();
    reports.extend(check.scripts.iter().map(Report::from_compile_script));
    Err(reports)
}

/// A report for a payload problem in `exe`.
fn payload_report(exe: &Path, error: &PayloadError) -> Report {
    Report {
        kind: crate::Kind::Compile,
        file: exe.display().to_string(),
        line: 0,
        col: 0,
        message: error.to_string(),
    }
}

/// Shows `reports` in a native message box.
///
/// A GUI-subsystem exported app has no console, so this is the only place a
/// user sees why it did not start. It opens a real window, so tests never call
/// it.
pub fn show_failure(reports: &[Report]) {
    let mut text = String::new();
    for report in reports.iter().take(8) {
        text.push_str(&report.to_text());
        text.push('\n');
    }
    if reports.len() > 8 {
        text.push_str(&format!("... and {} more\n", reports.len() - 8));
    }
    crate::platform::show_error("This program cannot start", &text);
}

#[cfg(test)]
mod tests {
    use lazyrad_packager::Entry;
    use lazyrad_project::ProjectItem;

    use super::*;

    fn entry(name: &str, data: &[u8]) -> Entry {
        Entry {
            name: name.to_owned(),
            data: data.to_vec(),
        }
    }

    /// A payload for a one-module project that also ships `songs/song.mod`
    /// and `readme.txt` as assets.
    fn files() -> PayloadFiles {
        let mut project = Project::new("app");
        project.items.push(ProjectItem::Module {
            name: "m".to_owned(),
            code: "m.rhai".into(),
        });
        let payload = Payload::new(vec![
            entry("app.lrp", b"project"),
            entry("m.rhai", b"code"),
            entry("assets/songs/song.mod", b"MOD"),
            entry("assets/readme.txt", b"README"),
            entry("assets/m.rhai", b"asset copy"),
        ])
        .expect("a valid payload");
        PayloadFiles::new(payload, &project)
    }

    #[test]
    fn item_files_and_assets_are_served_by_their_project_paths() {
        let files = files();
        assert_eq!(files.read("m.rhai"), Some(b"code".to_vec()));
        assert_eq!(files.read("songs/song.mod"), Some(b"MOD".to_vec()));
        assert_eq!(files.read("readme.txt"), Some(b"README".to_vec()));
    }

    #[test]
    fn the_project_file_and_raw_entries_are_not_served() {
        let files = files();
        assert_eq!(
            files.read("app.lrp"),
            None,
            "the .lrp is not a project file"
        );
        // `assets/` is the payload's prefix, not part of a project path: the
        // raw entry for `readme.txt` is not reachable as `assets/readme.txt`.
        assert_eq!(files.read("assets/readme.txt"), None);
        assert_eq!(files.read("assets/songs/song.mod"), None);
        assert_eq!(files.read("missing.txt"), None);
        assert_eq!(files.read(""), None);
    }

    #[test]
    fn an_item_name_is_read_as_the_item_not_as_an_asset_of_the_same_name() {
        // `m.rhai` is both an item and (in this hand-made payload) an asset
        // entry; the item wins, as it does from a folder.
        assert_eq!(files().read("m.rhai"), Some(b"code".to_vec()));
    }
}
