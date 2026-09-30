#![forbid(unsafe_code)]

//! File and folder dialogs, through the installed platform.
//!
//! xui has none (PLAN.md §10, gap G4), so the IDE asks
//! [`lazyrad_runtime::platform::Platform::dialogs`]: on the desktop that is
//! `rfd` (see [`super::host`]), and with no platform installed every request is
//! answered as if the user cancelled.

use std::path::{Path, PathBuf};

use lazyrad_runtime::platform::{self, Filter};

/// The file name filter for a LazyRAD project file.
const PROJECT_FILTER: Filter<'static> = ("LazyRAD project", &["lrp"]);

/// Reports an error that stops the IDE before (or instead of) its window, in a
/// native message box. The IDE is a GUI-subsystem app on Windows, so there is
/// no console to print to.
pub fn show_fatal_error(message: &str) {
    platform::current().dialogs().show_error("LazyRAD", message);
}

/// Asks for an existing `.lrp` project file.
///
/// `None` when the user cancels (or no dialog is available).
pub fn open_project_file() -> Option<PathBuf> {
    platform::current()
        .dialogs()
        .open_file("Open Project", Some(PROJECT_FILTER))
}

/// Asks for a folder, for example the one a new project is created in.
///
/// `None` when the user cancels (or no dialog is available).
pub fn choose_folder(title: &str) -> Option<PathBuf> {
    platform::current().dialogs().choose_folder(title)
}

/// Asks where to save the project, suggesting `file_name` (`<name>.lrp`).
///
/// `None` when the user cancels (or no dialog is available).
pub fn save_project_file(file_name: &str) -> Option<PathBuf> {
    platform::current()
        .dialogs()
        .save_file("Save Project As", file_name, Some(PROJECT_FILTER))
}

/// Asks where to write the exported executable, suggesting `file_name`.
///
/// `None` when the user cancels (or no dialog is available).
pub fn save_exe_file(file_name: &str) -> Option<PathBuf> {
    let extension = std::env::consts::EXE_EXTENSION;
    let extensions = [extension];
    let filter = (!extension.is_empty()).then_some(("Program", &extensions[..]));
    platform::current()
        .dialogs()
        .save_file("Make Executable", file_name, filter)
}

/// The folder a chosen file lives in, or the file itself when it has no
/// parent (the filesystem root).
pub fn containing_folder(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => path.to_path_buf(),
    }
}

/// The project name suggested by a chosen `.lrp` path: its file stem, or
/// the whole path when there is none.
pub fn project_name_of(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn a_file_resolves_to_its_folder_and_stem() {
        let path = Path::new("/home/user/MyApp.lrp");
        assert_eq!(containing_folder(path), Path::new("/home/user"));
        assert_eq!(project_name_of(path), "MyApp");
    }

    #[test]
    fn a_bare_file_name_has_no_folder() {
        let path = Path::new("MyApp.lrp");
        assert_eq!(containing_folder(path), Path::new("MyApp.lrp"));
        assert_eq!(project_name_of(path), "MyApp");
    }

    #[test]
    fn without_an_installed_platform_every_request_is_a_cancel() {
        // No test installs a platform, so the portable fallback answers.
        assert_eq!(open_project_file(), None);
        assert_eq!(choose_folder("t"), None);
        assert_eq!(save_project_file("a.lrp"), None);
        assert_eq!(save_exe_file("a"), None);
    }
}
