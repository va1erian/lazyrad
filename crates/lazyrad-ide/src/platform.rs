#![forbid(unsafe_code)]

//! Platform integration the IDE needs but xui cannot provide.
//!
//! The only thing here today is native file and folder dialogs. xui has none
//! (PLAN.md §10, gap G4), so on the desktop they are [`rfd`]'s OS dialogs.
//! Keeping them behind [`dialogs`] means LazyOS can drop in a painted xui
//! dialog later without the IDE knowing the difference.
//!
//! [`rfd`]: https://docs.rs/rfd

/// Reports an error that stops the IDE before (or instead of) its window, in a
/// native message box. The IDE is a GUI-subsystem app on Windows, so there is
/// no console to print to.
pub fn show_fatal_error(message: &str) {
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("LazyRAD")
        .set_description(message)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// Native file and folder dialogs.
pub mod dialogs {
    use std::path::{Path, PathBuf};

    use rfd::FileDialog;

    /// The file name filter for a LazyRAD project file.
    const PROJECT_FILTER: (&str, &[&str]) = ("LazyRAD project", &["lrp"]);

    /// Asks for an existing `.lrp` project file.
    ///
    /// `None` when the user cancels.
    pub fn open_project_file() -> Option<PathBuf> {
        FileDialog::new()
            .set_title("Open Project")
            .add_filter(PROJECT_FILTER.0, PROJECT_FILTER.1)
            .pick_file()
    }

    /// Asks for a folder, for example the one a new project is created in.
    ///
    /// `None` when the user cancels.
    pub fn choose_folder(title: &str) -> Option<PathBuf> {
        FileDialog::new().set_title(title).pick_folder()
    }

    /// Asks where to save the project, suggesting `file_name` (`<name>.lrp`).
    ///
    /// `None` when the user cancels.
    pub fn save_project_file(file_name: &str) -> Option<PathBuf> {
        FileDialog::new()
            .set_title("Save Project As")
            .add_filter(PROJECT_FILTER.0, PROJECT_FILTER.1)
            .set_file_name(file_name)
            .save_file()
    }

    /// Asks where to write the exported executable, suggesting `file_name`.
    ///
    /// `None` when the user cancels.
    pub fn save_exe_file(file_name: &str) -> Option<PathBuf> {
        let mut dialog = FileDialog::new()
            .set_title("Make Executable")
            .set_file_name(file_name);
        if !std::env::consts::EXE_EXTENSION.is_empty() {
            dialog = dialog.add_filter("Program", &[std::env::consts::EXE_EXTENSION]);
        }
        dialog.save_file()
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
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::dialogs::{containing_folder, project_name_of};

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
}
