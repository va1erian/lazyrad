#![forbid(unsafe_code)]

//! The player's platform seam (PLAN.md §12, "Porting LazyRAD").
//!
//! The player needs two things from the OS beyond xui: a way to tell the user
//! why an exported app did not start, since a GUI-subsystem app has no console,
//! and the open-file dialog a script asks for with `open_file_dialog`. Both are
//! native dialogs through `rfd` behind the default `native-dialogs` feature,
//! and otherwise (or on a platform with no dialog backend) the error goes to
//! stderr and every file dialog cancels. A new platform replaces this module.
//! The Windows icon resource is the only other platform piece, in `build.rs`.

#[cfg(feature = "native-dialogs")]
use std::path::PathBuf;

use lazyrad_runtime::platform::{self, Dialogs, Platform};
#[cfg(feature = "native-dialogs")]
use lazyrad_runtime::platform::{FileDone, FileFilter, Filter, dialog_filters};

/// Installs [`PlayerPlatform`] as the process's platform.
///
/// Call once before running a program; a platform a host already installed (a
/// test, or LazyOS's own) is kept.
pub fn install() {
    let _ = platform::install(Box::new(PlayerPlatform));
}

/// The desktop player's platform: `rfd` dialogs, unrestricted files.
#[derive(Clone, Copy, Debug, Default)]
struct PlayerPlatform;

impl Platform for PlayerPlatform {
    fn name(&self) -> &'static str {
        "player"
    }

    fn dialogs(&self) -> &dyn Dialogs {
        #[cfg(feature = "native-dialogs")]
        {
            &NativeDialogs
        }
        #[cfg(not(feature = "native-dialogs"))]
        {
            &lazyrad_runtime::platform::HeadlessDialogs
        }
    }
}

/// The OS file dialogs, through `rfd`.
#[cfg(feature = "native-dialogs")]
struct NativeDialogs;

#[cfg(feature = "native-dialogs")]
impl Dialogs for NativeDialogs {
    fn open_file(&self, title: &str, filter: Option<Filter<'_>>) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        if let Some((name, extensions)) = filter {
            dialog = dialog.add_filter(name, extensions);
        }
        dialog.pick_file()
    }

    fn open_file_filtered(&self, title: &str, filters: &[FileFilter]) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        for (name, extensions) in dialog_filters(filters) {
            dialog = dialog.add_filter(name, &extensions);
        }
        dialog.pick_file()
    }

    /// Shows the dialog on a worker thread so the window keeps running.
    ///
    /// `rfd::AsyncFileDialog` is safe to start off the UI thread on every
    /// platform (macOS hops to the main thread itself); `pollster` runs its
    /// future to completion on the worker.
    fn open_file_async(&self, title: &str, filters: &[FileFilter], done: FileDone) {
        let mut dialog = rfd::AsyncFileDialog::new().set_title(title);
        for (name, extensions) in dialog_filters(filters) {
            dialog = dialog.add_filter(name, &extensions);
        }
        platform::pick_on_worker(
            move || pollster::block_on(dialog.pick_file()).map(|file| file.path().to_path_buf()),
            done,
        );
    }

    fn choose_folder(&self, _title: &str) -> Option<PathBuf> {
        None
    }

    fn save_file(
        &self,
        _title: &str,
        _file_name: &str,
        _filter: Option<Filter<'_>>,
    ) -> Option<PathBuf> {
        None
    }

    fn show_error(&self, title: &str, message: &str) {
        let _ = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(title)
            .set_description(message)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
}

/// Shows `text` as an error titled `title`.
pub fn show_error(title: &str, text: &str) {
    #[cfg(feature = "native-dialogs")]
    {
        let _ = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(title)
            .set_description(text)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
    #[cfg(not(feature = "native-dialogs"))]
    show_error_on_stderr(title, text);
}

/// The portable fallback: the message on stderr.
#[cfg_attr(feature = "native-dialogs", allow(dead_code))]
fn show_error_on_stderr(title: &str, text: &str) {
    eprintln!("{title}: {text}");
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_fallback_writes_to_stderr_without_blocking() {
        super::show_error_on_stderr("title", "text");
    }
}
