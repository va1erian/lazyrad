#![forbid(unsafe_code)]

//! Native file and folder dialogs.
//!
//! xui has none (PLAN.md §10, gap G4), so on the desktop they are `rfd`'s OS
//! dialogs, behind the default `native-dialogs` feature. Without it every
//! request is answered as if the user cancelled, which is the seam a painted
//! in-app dialog (LazyOS) replaces.

use std::path::{Path, PathBuf};

/// The file name filter for a LazyRAD project file.
const PROJECT_FILTER: Filter<'static> = ("LazyRAD project", &["lrp"]);

/// Reports an error that stops the IDE before (or instead of) its window, in a
/// native message box. The IDE is a GUI-subsystem app on Windows, so there is
/// no console to print to.
pub fn show_fatal_error(message: &str) {
    backend::show_error("LazyRAD", message);
}

/// Asks for an existing `.lrp` project file.
///
/// `None` when the user cancels (or no dialog is available).
pub fn open_project_file() -> Option<PathBuf> {
    backend::open_file("Open Project", Some(PROJECT_FILTER))
}

/// Asks for a folder, for example the one a new project is created in.
///
/// `None` when the user cancels (or no dialog is available).
pub fn choose_folder(title: &str) -> Option<PathBuf> {
    backend::choose_folder(title)
}

/// Asks where to save the project, suggesting `file_name` (`<name>.lrp`).
///
/// `None` when the user cancels (or no dialog is available).
pub fn save_project_file(file_name: &str) -> Option<PathBuf> {
    backend::save_file("Save Project As", file_name, Some(PROJECT_FILTER))
}

/// Asks where to write the exported executable, suggesting `file_name`.
///
/// `None` when the user cancels (or no dialog is available).
pub fn save_exe_file(file_name: &str) -> Option<PathBuf> {
    let extension = std::env::consts::EXE_EXTENSION;
    let extensions = [extension];
    let filter = (!extension.is_empty()).then_some(("Program", &extensions[..]));
    backend::save_file("Make Executable", file_name, filter)
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

#[cfg(not(feature = "native-dialogs"))]
use headless as backend;
#[cfg(feature = "native-dialogs")]
use native as backend;

/// `(description, extensions)` of a file name filter.
type Filter<'a> = (&'a str, &'a [&'a str]);

/// The OS dialogs, through `rfd`.
#[cfg(feature = "native-dialogs")]
mod native {
    use std::path::PathBuf;

    use rfd::FileDialog;

    use super::Filter;

    pub fn show_error(title: &str, message: &str) {
        let _ = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(title)
            .set_description(message)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }

    pub fn open_file(title: &str, filter: Option<Filter<'_>>) -> Option<PathBuf> {
        let mut dialog = FileDialog::new().set_title(title);
        if let Some((name, extensions)) = filter {
            dialog = dialog.add_filter(name, extensions);
        }
        dialog.pick_file()
    }

    pub fn choose_folder(title: &str) -> Option<PathBuf> {
        FileDialog::new().set_title(title).pick_folder()
    }

    pub fn save_file(title: &str, file_name: &str, filter: Option<Filter<'_>>) -> Option<PathBuf> {
        let mut dialog = FileDialog::new().set_title(title).set_file_name(file_name);
        if let Some((name, extensions)) = filter {
            dialog = dialog.add_filter(name, extensions);
        }
        dialog.save_file()
    }
}

/// The portable fallback: no dialog can be shown, so every request is a
/// cancel and errors go to stderr. Always compiled so it is tested everywhere.
#[cfg_attr(feature = "native-dialogs", allow(dead_code))]
mod headless {
    use std::path::PathBuf;

    use super::Filter;

    pub fn show_error(title: &str, message: &str) {
        eprintln!("{title}: {message}");
    }

    pub fn open_file(_title: &str, _filter: Option<Filter<'_>>) -> Option<PathBuf> {
        None
    }

    pub fn choose_folder(_title: &str) -> Option<PathBuf> {
        None
    }

    pub fn save_file(
        _title: &str,
        _file_name: &str,
        _filter: Option<Filter<'_>>,
    ) -> Option<PathBuf> {
        None
    }
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
    fn the_headless_fallback_answers_every_request_as_a_cancel() {
        assert_eq!(headless::open_file("t", Some(PROJECT_FILTER)), None);
        assert_eq!(headless::choose_folder("t"), None);
        assert_eq!(headless::save_file("t", "a.lrp", None), None);
        headless::show_error("t", "message goes to stderr");
    }
}
