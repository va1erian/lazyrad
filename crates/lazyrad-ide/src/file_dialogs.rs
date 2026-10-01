#![forbid(unsafe_code)]

//! File dialogs drawn inside the IDE window.
//!
//! The desktop asks the OS (`platform::dialogs`, blocking). A platform that has
//! no blocking dialogs (LazyOS) returns a filesystem from
//! [`Platform::file_system`](lazyrad_runtime::platform::Platform::file_system),
//! and the IDE then shows xui's portable [`FileDialog`] on it: the answer does
//! not come back from the call, it arrives later as [`Msg::FileChosen`] (or
//! [`Msg::FileCancelled`]), and the IDE continues from there. [`FileRequest`]
//! names what the pending question was for, so the continuation is the same code
//! in both worlds.

use std::path::PathBuf;
use std::rc::Rc;

use xui_core::app::Ui;
use xui_core::backend::Result as UiResult;
use xui_core::widget::{FileDialog, FileSystem};

use crate::app::Msg;
use crate::platform::dialogs;

/// What a file question is for. The IDE remembers it while the dialog is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileRequest {
    /// Open an existing `.lrp`.
    OpenProject,
    /// Choose where the new project named `.0` goes: the answer is the new
    /// project's folder (created when missing).
    NewProject(String),
    /// Save the project's `.lrp` under another name; `.0` is the suggestion.
    SaveProjectAs(String),
    /// Write the exported program; `.0` is the suggested file name.
    MakeExe(String),
}

/// The answer to [`ask`]: either known now (a blocking OS dialog), or later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// The blocking dialog returned; `None` is "cancelled".
    Now(Option<PathBuf>),
    /// An in-window dialog opened; the answer arrives as a message.
    Later,
}

/// The two dialogs an IDE needs, built once on first use.
pub struct InWindowDialogs {
    open: FileDialog<Msg>,
    save: FileDialog<Msg>,
    start_dir: PathBuf,
}

impl InWindowDialogs {
    /// Builds the dialogs over `fs`, starting in `start_dir`.
    pub fn new(
        ui: &Ui<Msg>,
        fs: Rc<dyn FileSystem>,
        start_dir: PathBuf,
    ) -> UiResult<InWindowDialogs> {
        let open = FileDialog::open_file(ui, "Open Project")?
            .file_system(Rc::clone(&fs))
            .initial_dir(start_dir.clone())
            .filter("LazyRAD project", &["lrp"])
            .require_existing(true)
            .on_accept(|path| Some(Msg::FileChosen(path)))
            .on_cancel(|| Some(Msg::FileCancelled));
        let save = FileDialog::save_file(ui, "Save")?
            .file_system(fs)
            .initial_dir(start_dir.clone())
            .on_accept(|path| Some(Msg::FileChosen(path)))
            .on_cancel(|| Some(Msg::FileCancelled));
        Ok(InWindowDialogs {
            open,
            save,
            start_dir,
        })
    }

    /// Shows the dialog `request` needs.
    pub fn show(&self, request: &FileRequest) {
        match request {
            FileRequest::OpenProject => {
                self.open.set_initial_dir(self.start_dir.clone());
                self.open.open();
            }
            FileRequest::NewProject(name) => {
                // There is no folder picker: the user names the new folder.
                self.prepare_save(name);
                self.save.open();
            }
            FileRequest::SaveProjectAs(suggestion) | FileRequest::MakeExe(suggestion) => {
                self.prepare_save(suggestion);
                self.save.open();
            }
        }
    }

    fn prepare_save(&self, suggestion: &str) {
        self.save.set_initial_dir(self.start_dir.clone());
        self.save.set_suggested_name(suggestion);
    }

    /// Whether either dialog is showing (shortcuts must not act underneath).
    pub fn is_open(&self) -> bool {
        self.open.is_open() || self.save.is_open()
    }
}

/// Asks the blocking OS dialog that matches `request`.
pub fn ask_blocking(request: &FileRequest) -> Option<PathBuf> {
    match request {
        FileRequest::OpenProject => dialogs::open_project_file(),
        FileRequest::NewProject(_) => dialogs::choose_folder("Choose a folder for the project"),
        FileRequest::SaveProjectAs(name) => dialogs::save_project_file(name),
        FileRequest::MakeExe(name) => dialogs::save_exe_file(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_platform_the_blocking_dialogs_cancel() {
        // No platform is installed in unit tests, so every dialog cancels.
        for request in [
            FileRequest::OpenProject,
            FileRequest::NewProject("App".into()),
            FileRequest::SaveProjectAs("App.lrp".into()),
            FileRequest::MakeExe("App".into()),
        ] {
            assert_eq!(ask_blocking(&request), None, "{request:?}");
        }
    }
}
