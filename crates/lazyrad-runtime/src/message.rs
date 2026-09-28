#![forbid(unsafe_code)]

//! The messages a form's application routes, and the queue a script writes to.
//!
//! A Rhai handler never touches the toolkit directly. When the standard library
//! needs the host to do something the script cannot do itself — open a message
//! box, close a window, quit — it pushes a [`Msg`] onto the shared pending
//! queue. [`crate::form::FormApp`] drains that queue after the handler returns
//! and performs the work, so a UI call never re-enters the Rhai engine.
//!
//! [`Pending`] is the queue itself: an [`Rc`] so the engine host, the standard
//! library and every [`crate::control::FormRef`] in a form share one queue.

use std::cell::RefCell;
use std::rc::Rc;

use rhai::FnPtr;
use xui_form::Value;

/// The queue of messages a script leaves for the running application.
pub type Pending = Rc<RefCell<Vec<Msg>>>;

/// A message the runtime application handles.
#[derive(Clone, Debug)]
pub enum Msg {
    /// A widget event that a `<control>_<event>` handler is wired to.
    Event {
        /// The form the event belongs to.
        form: String,
        /// The control that raised it.
        control: String,
        /// `xui`'s event name (`Click`, `Change`, `Toggle`, …); the handler is
        /// its snake_case form.
        event: String,
        /// The event's arguments, converted from the widget's typed values.
        args: Vec<Value>,
    },
    /// `other_form.show()`: open a secondary window for the named form.
    ShowForm(String),
    /// `other_form.unload()`: close the named form's secondary window.
    CloseForm(String),
    /// `msg_box(...)`: show an in-window dialog for the named form.
    MsgBox {
        /// The form whose script asked for the dialog.
        form: String,
        /// The dialog's message text.
        text: String,
        /// The dialog's title.
        title: String,
        /// The button set the dialog shows.
        buttons: MsgBoxButtons,
        /// The Rhai function pointer to call with the result, if any.
        callback: Option<FnPtr>,
    },
    /// A message box's callback, called by the application after the dialog
    /// closes with `result`.
    MsgBoxResult {
        /// The form the callback belongs to.
        form: String,
        /// The Rhai function pointer to call.
        callback: FnPtr,
        /// The button pressed: `"ok"`, `"cancel"`, `"yes"` or `"no"`.
        result: &'static str,
    },
    /// `app.quit()`: end the application.
    Quit,
}

/// The buttons a [`Msg`] message box shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgBoxButtons {
    /// One **OK** button.
    Ok,
    /// **OK** and **Cancel**.
    OkCancel,
    /// **Yes** and **No**.
    YesNo,
}

impl MsgBoxButtons {
    /// The button set a script names: `"ok"`, `"ok_cancel"` or `"yes_no"`.
    pub fn from_name(name: &str) -> Option<MsgBoxButtons> {
        match name {
            "ok" => Some(MsgBoxButtons::Ok),
            "ok_cancel" => Some(MsgBoxButtons::OkCancel),
            "yes_no" => Some(MsgBoxButtons::YesNo),
            _ => None,
        }
    }

    /// The name of the button that dismissed the box, as the script's callback
    /// receives it.
    ///
    /// `accepted` is true for the affirmative button (or Enter) and false for
    /// cancel (or Escape). An **OK**-only box always reports `"ok"`.
    pub fn result(self, accepted: bool) -> &'static str {
        match (self, accepted) {
            (MsgBoxButtons::Ok, _) | (MsgBoxButtons::OkCancel, true) => "ok",
            (MsgBoxButtons::OkCancel, false) => "cancel",
            (MsgBoxButtons::YesNo, true) => "yes",
            (MsgBoxButtons::YesNo, false) => "no",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_sets_are_named() {
        assert_eq!(MsgBoxButtons::from_name("ok"), Some(MsgBoxButtons::Ok));
        assert_eq!(
            MsgBoxButtons::from_name("ok_cancel"),
            Some(MsgBoxButtons::OkCancel)
        );
        assert_eq!(
            MsgBoxButtons::from_name("yes_no"),
            Some(MsgBoxButtons::YesNo)
        );
        assert_eq!(MsgBoxButtons::from_name("vbYesNo"), None);
    }

    #[test]
    fn results_name_the_button_pressed() {
        assert_eq!(MsgBoxButtons::Ok.result(true), "ok");
        assert_eq!(MsgBoxButtons::Ok.result(false), "ok");
        assert_eq!(MsgBoxButtons::OkCancel.result(true), "ok");
        assert_eq!(MsgBoxButtons::OkCancel.result(false), "cancel");
        assert_eq!(MsgBoxButtons::YesNo.result(true), "yes");
        assert_eq!(MsgBoxButtons::YesNo.result(false), "no");
    }
}
