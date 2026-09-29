#![forbid(unsafe_code)]

//! Platform seams for the editor. Today that is only the clipboard.
//!
//! xui has no portable clipboard: it lives in its Windows backend, not in
//! `xui-core`. The editor copies and pastes through this [`Clipboard`] trait,
//! which the OS clipboard ([`arboard`], behind the `system-clipboard` feature)
//! implements when it is available and an in-process buffer implements
//! everywhere else. A build with `default-features = false` never touches an OS
//! clipboard; an app on a platform without one implements [`Clipboard`] itself.

use std::cell::RefCell;

/// Text-only clipboard access.
pub trait Clipboard {
    /// The clipboard's current text, or `None` when it holds none or is
    /// unavailable.
    fn text(&self) -> Option<String>;

    /// Replaces the clipboard's text.
    fn set_text(&self, text: &str);
}

thread_local! {
    /// The in-process clipboard, shared by every editor on this thread.
    static IN_PROCESS: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The in-process clipboard: every editor on the thread shares one buffer.
#[derive(Clone, Copy, Debug, Default)]
pub struct InProcessClipboard;

impl Clipboard for InProcessClipboard {
    fn text(&self) -> Option<String> {
        IN_PROCESS.with(|value| {
            let value = value.borrow();
            (!value.is_empty()).then(|| value.clone())
        })
    }

    fn set_text(&self, text: &str) {
        IN_PROCESS.with(|value| *value.borrow_mut() = text.to_string());
    }
}

/// The system clipboard, backed by `arboard`.
#[cfg(feature = "system-clipboard")]
struct SystemClipboard {
    inner: RefCell<arboard::Clipboard>,
}

#[cfg(feature = "system-clipboard")]
impl SystemClipboard {
    /// Opens the OS clipboard, or returns why it cannot.
    fn new() -> Result<SystemClipboard, arboard::Error> {
        Ok(SystemClipboard {
            inner: RefCell::new(arboard::Clipboard::new()?),
        })
    }
}

#[cfg(feature = "system-clipboard")]
impl Clipboard for SystemClipboard {
    fn text(&self) -> Option<String> {
        self.inner.borrow_mut().get_text().ok()
    }

    fn set_text(&self, text: &str) {
        let _ = self.inner.borrow_mut().set_text(text.to_string());
    }
}

/// The clipboard the editor should use: the OS clipboard when the feature is
/// on and it can be opened, the in-process buffer otherwise.
pub fn clipboard() -> Box<dyn Clipboard> {
    #[cfg(feature = "system-clipboard")]
    if let Ok(system) = SystemClipboard::new() {
        return Box::new(system);
    }
    Box::new(InProcessClipboard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_in_process_clipboard_round_trips() {
        let clipboard = InProcessClipboard;
        clipboard.set_text("hello");
        assert_eq!(clipboard.text().as_deref(), Some("hello"));
    }

    #[test]
    fn two_in_process_handles_share_one_buffer() {
        let first = InProcessClipboard;
        let second = InProcessClipboard;
        first.set_text("shared");
        assert_eq!(second.text().as_deref(), Some("shared"));
    }
}
