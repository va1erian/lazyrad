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

use std::fs::File;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use lazyrad_packager::{Payload, PayloadError};
use lazyrad_project::Project;
use lazyrad_runtime::{FormRuntime, RuntimeError, check_runtime};

use crate::Report;

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

    let runtime = FormRuntime::from_project(project, dir, |relative| {
        let name = relative.to_string_lossy();
        let data = payload.get(&name).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "the file is not in the executable's project data",
            )
        })?;
        String::from_utf8(data.to_vec())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
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
    reports.extend(check.lints.iter().map(Report::from_lint));
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
