#![forbid(unsafe_code)]

//! The open-file dialogs scripts asked for, answered off the UI thread.
//!
//! `open_file_dialog` is asynchronous: the platform shows its dialog on a worker
//! (see [`Dialogs::open_file_async`](crate::platform::Dialogs::open_file_async))
//! and the window keeps handling events meanwhile. The worker hands its answer
//! back through this module's mailbox, which is thread-safe. The script's
//! callback (a Rhai function pointer, not `Send`) never leaves the UI thread:
//! it waits in a table here, keyed by request id, and the window that owns it
//! runs it when it collects the answer.
//!
//! A window collects answers from its poll timer while it has requests waiting
//! (`Timers::sync_dialogs`), because the toolkit's thread-safe `Proxy` needs a
//! `Send` message type and [`Msg`](crate::form::Msg) carries Rhai function
//! pointers. Each window is an *owner*; closing it cancels its requests, so an
//! answer that arrives afterwards is dropped (no callback, no sandbox grant).

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rhai::FnPtr;

use crate::platform::FileDone;

/// A request's id.
type RequestId = u64;

/// One answered request: its id and the picked path, `None` when cancelled.
type Reply = (RequestId, Option<PathBuf>);

/// A request whose answer has not been collected yet.
struct Waiting {
    /// The window that asked.
    owner: u64,
    /// The script function to call with the answer.
    callback: FnPtr,
}

/// Every open-file request of one runtime, shared by its windows.
pub(crate) struct FileDialogs {
    next_request: Cell<RequestId>,
    next_owner: Cell<u64>,
    waiting: RefCell<BTreeMap<RequestId, Waiting>>,
    /// Answers workers have delivered but no window has collected yet.
    replies: Arc<Mutex<Vec<Reply>>>,
}

impl FileDialogs {
    pub(crate) fn new() -> FileDialogs {
        FileDialogs {
            next_request: Cell::new(1),
            next_owner: Cell::new(1),
            waiting: RefCell::new(BTreeMap::new()),
            replies: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A fresh owner id for a window.
    pub(crate) fn new_owner(&self) -> u64 {
        let owner = self.next_owner.get();
        self.next_owner.set(owner + 1);
        owner
    }

    /// Registers a request for `owner` and starts it with `start`, which is
    /// given the function to call (from any thread) with the answer.
    ///
    /// `start` may answer before it returns; the answer is then collected by the
    /// next [`FileDialogs::take_replies`].
    pub(crate) fn request(&self, owner: u64, callback: FnPtr, start: impl FnOnce(FileDone)) {
        let id = self.next_request.get();
        self.next_request.set(id + 1);
        self.waiting
            .borrow_mut()
            .insert(id, Waiting { owner, callback });
        let replies = Arc::clone(&self.replies);
        start(Box::new(move |picked| {
            replies
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push((id, picked));
        }));
    }

    /// Whether `owner` has a request that is not answered and collected yet.
    pub(crate) fn has_waiting(&self, owner: u64) -> bool {
        self.waiting
            .borrow()
            .values()
            .any(|waiting| waiting.owner == owner)
    }

    /// Takes the answers to `owner`'s requests: each callback with the path
    /// picked, or `None` for a cancellation.
    ///
    /// Another window's answers stay for it; an answer to a cancelled request
    /// is discarded.
    pub(crate) fn take_replies(&self, owner: u64) -> Vec<(FnPtr, Option<PathBuf>)> {
        let mut replies = self
            .replies
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if replies.is_empty() {
            return Vec::new();
        }
        let mut waiting = self.waiting.borrow_mut();
        let mut ready = Vec::new();
        replies.retain(|(id, picked)| match waiting.get(id) {
            // Nobody is waiting: the owner closed. Drop it.
            None => false,
            Some(request) if request.owner == owner => {
                if let Some(request) = waiting.remove(id) {
                    ready.push((request.callback, picked.clone()));
                }
                false
            }
            // Another window's answer: leave it for that window.
            Some(_) => true,
        });
        ready
    }

    /// Cancels every request `owner` made, because its window is gone.
    pub(crate) fn cancel(&self, owner: u64) {
        self.waiting
            .borrow_mut()
            .retain(|_, waiting| waiting.owner != owner);
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    fn callback(name: &str) -> FnPtr {
        FnPtr::new(name).expect("a valid function name")
    }

    /// Starts a request that keeps its `done` for the test to release.
    fn held(dialogs: &FileDialogs, owner: u64, name: &str, slot: &Rc<RefCell<Option<FileDone>>>) {
        let slot = Rc::clone(slot);
        dialogs.request(owner, callback(name), move |done| {
            *slot.borrow_mut() = Some(done);
        });
    }

    #[test]
    fn an_answer_is_collected_by_its_owner() {
        let dialogs = FileDialogs::new();
        let owner = dialogs.new_owner();
        let slot = Rc::new(RefCell::new(None));
        held(&dialogs, owner, "picked", &slot);
        assert!(dialogs.has_waiting(owner));
        assert!(dialogs.take_replies(owner).is_empty(), "not answered yet");

        let done = slot.borrow_mut().take().expect("started");
        done(Some(PathBuf::from("song.mod")));
        let ready = dialogs.take_replies(owner);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].0.fn_name(), "picked");
        assert_eq!(
            ready[0].1.as_deref(),
            Some(std::path::Path::new("song.mod"))
        );
        assert!(!dialogs.has_waiting(owner));
        assert!(dialogs.take_replies(owner).is_empty(), "delivered once");
    }

    #[test]
    fn an_answer_may_arrive_from_another_thread() {
        let dialogs = FileDialogs::new();
        let owner = dialogs.new_owner();
        let slot = Rc::new(RefCell::new(None));
        held(&dialogs, owner, "picked", &slot);
        let done = slot.borrow_mut().take().expect("started");
        std::thread::spawn(move || done(None))
            .join()
            .expect("the worker finishes");
        let ready = dialogs.take_replies(owner);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].1, None, "a cancellation");
    }

    #[test]
    fn a_closed_owner_drops_its_late_answer() {
        let dialogs = FileDialogs::new();
        let owner = dialogs.new_owner();
        let slot = Rc::new(RefCell::new(None));
        held(&dialogs, owner, "picked", &slot);
        dialogs.cancel(owner);
        assert!(!dialogs.has_waiting(owner));

        let done = slot.borrow_mut().take().expect("started");
        done(Some(PathBuf::from("late.mod")));
        assert!(dialogs.take_replies(owner).is_empty());
        // The late answer was discarded, not kept for a later window.
        let reopened = dialogs.new_owner();
        assert!(dialogs.take_replies(reopened).is_empty());
    }

    #[test]
    fn an_answer_waits_for_the_window_that_asked() {
        let dialogs = FileDialogs::new();
        let first = dialogs.new_owner();
        let second = dialogs.new_owner();
        let slot = Rc::new(RefCell::new(None));
        held(&dialogs, first, "picked", &slot);
        let done = slot.borrow_mut().take().expect("started");
        done(Some(PathBuf::from("a.mod")));

        assert!(dialogs.take_replies(second).is_empty());
        assert_eq!(
            dialogs.take_replies(first).len(),
            1,
            "still there for the asker"
        );
    }

    #[test]
    fn a_dialog_that_answers_immediately_is_collected_at_once() {
        let dialogs = FileDialogs::new();
        let owner = dialogs.new_owner();
        dialogs.request(owner, callback("picked"), |done| done(None));
        assert_eq!(dialogs.take_replies(owner).len(), 1);
    }
}
