#![forbid(unsafe_code)]

//! File → Make `<Project>`.exe… (issue #70).
//!
//! The IDE exports a project as a self-contained executable: a copy of the
//! player with the project appended (see [`lazyrad_packager`]). The steps the
//! IDE drives, in order, are: save the project, run the same whole-project
//! check as Run → Start (and stop on any problem), locate the player stub,
//! ask where to write the file, then export.
//!
//! This module holds the parts that do not need a window: the menu label, the
//! suggested file name and the export call itself, so they can be tested
//! without opening a dialog.
//!
//! # Windows: no console window
//!
//! The player is built as a console program so the IDE can read a running
//! project's output. The export patches the *copy's* PE subsystem to GUI, so
//! one player binary serves both purposes. The exported app shows startup
//! failures in a native message box.

use std::path::Path;

use lazyrad_packager::{ExportError, ExportReport, ExportRequest, export};

/// The File menu label: `Make MyApp.exe…`, or a generic one with no project.
///
/// The exported file's suffix follows the host (`.exe` on Windows); the label
/// always says `.exe`, as the command is named after VB6's *Make Project1.exe*.
pub fn menu_label(project_name: Option<&str>) -> String {
    match project_name {
        Some(name) => format!("Ma&ke {name}.exe…"),
        None => "Ma&ke .exe…".to_owned(),
    }
}

/// The file name suggested in the save dialog: the project name plus the
/// platform's executable suffix.
pub fn suggested_file_name(project_name: &str) -> String {
    format!("{project_name}{}", std::env::consts::EXE_SUFFIX)
}

/// Exports the project whose `.lrp` is `project_file` to `output`, using
/// `stub` as the player to copy.
///
/// The caller has already saved the project and checked it; this is the
/// packaging only. Nothing is written unless the whole export succeeds.
pub fn export_project(
    project_file: &Path,
    stub: &Path,
    output: &Path,
) -> Result<ExportReport, ExportError> {
    export(&ExportRequest {
        stub,
        project: project_file,
        output,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{export_project, menu_label, suggested_file_name};

    #[test]
    fn the_label_names_the_project() {
        assert_eq!(menu_label(Some("MyApp")), "Ma&ke MyApp.exe…");
        assert_eq!(menu_label(None), "Ma&ke .exe…");
    }

    #[test]
    fn the_suggested_name_uses_the_platform_suffix() {
        assert_eq!(
            suggested_file_name("MyApp"),
            format!("MyApp{}", std::env::consts::EXE_SUFFIX)
        );
    }

    #[test]
    fn a_project_exports_and_a_failed_export_writes_nothing() {
        let dir: PathBuf =
            std::env::temp_dir().join(format!("lazyrad-ide-make-exe-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch directory");
        let project = crate::project::ProjectSession::create("MyApp", &dir).expect("creates");
        let lrp = project.project_file();
        let stub = dir.join("player.bin");
        fs::write(&stub, b"stand-in for the player").expect("stub writes");

        let output = dir.join("MyApp.out");
        let report = export_project(&lrp, &stub, &output).expect("export succeeds");
        assert_eq!(report.output, output);
        assert!(output.is_file());

        // A missing stub fails cleanly and leaves no new file.
        let other = dir.join("Other.out");
        assert!(export_project(&lrp, &dir.join("missing.bin"), &other).is_err());
        assert!(!other.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
