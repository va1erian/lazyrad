#![forbid(unsafe_code)]

//! The player's platform seam (PLAN.md §12, "Porting LazyRAD").
//!
//! The one thing the player needs from the OS beyond xui is a way to tell the
//! user why an exported app did not start, since a GUI-subsystem app has no
//! console. That is [`show_error`]: a native message box through `rfd` behind
//! the default `native-dialogs` feature, and otherwise (or on a platform with
//! no dialog backend) a line on stderr. A new platform replaces this module.
//! The Windows icon resource is the only other platform piece, in `build.rs`.

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
