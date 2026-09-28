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
        /// The VB-style event name (`Click`, `Change`, `DblClick`, …).
        event: String,
        /// The event's arguments, converted from the widget's typed values.
        args: Vec<Value>,
    },
    /// `frmOther.show()`: open a secondary window for the named form.
    ShowForm(String),
    /// `frmOther.unload()`: close the named form's secondary window.
    CloseForm(String),
    /// `MsgBox(...)`: show an in-window dialog for the named form.
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
        /// The VB-style result code (`vbOK`, `vbCancel`, `vbYes`, `vbNo`).
        result: i64,
    },
    /// `App.quit()`: end the application.
    Quit,
}

/// The buttons a [`Msg`] message box shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgBoxButtons {
    /// One **OK** button.
    OkOnly,
    /// **OK** and **Cancel**.
    OkCancel,
    /// **Yes** and **No**.
    YesNo,
}

impl MsgBoxButtons {
    /// Maps a VB `MsgBox` button constant onto the dialog shapes Iteration 1
    /// supports.
    ///
    /// The full VB set is not implemented: `vbYesNoCancel` and the icon flags
    /// collapse to one of these three shapes, since the in-window dialog has at
    /// most two buttons. An unknown value is treated as [`MsgBoxButtons::OkOnly`].
    pub fn from_vb(value: i64) -> MsgBoxButtons {
        match value {
            1 => MsgBoxButtons::OkCancel,
            4 => MsgBoxButtons::YesNo,
            _ => MsgBoxButtons::OkOnly,
        }
    }

    /// The VB result code for a dismissal.
    ///
    /// `accepted` is true for the affirmative button (or Enter) and false for
    /// cancel (or Escape). An **OK**-only dialog always reports `vbOK`, matching
    /// VB, which has no cancel result when there is only an **OK** button.
    pub fn result(self, accepted: bool) -> i64 {
        match (self, accepted) {
            (MsgBoxButtons::OkOnly, _) => VB_OK,
            (MsgBoxButtons::OkCancel, true) => VB_OK,
            (MsgBoxButtons::OkCancel, false) => VB_CANCEL,
            (MsgBoxButtons::YesNo, true) => VB_YES,
            (MsgBoxButtons::YesNo, false) => VB_NO,
        }
    }
}

/// VB `vbOK` result code.
pub const VB_OK: i64 = 1;
/// VB `vbCancel` result code.
pub const VB_CANCEL: i64 = 2;
/// VB `vbYes` result code.
pub const VB_YES: i64 = 6;
/// VB `vbNo` result code.
pub const VB_NO: i64 = 7;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vb_button_constants_map_to_the_supported_shapes() {
        assert_eq!(MsgBoxButtons::from_vb(0), MsgBoxButtons::OkOnly);
        assert_eq!(MsgBoxButtons::from_vb(1), MsgBoxButtons::OkCancel);
        assert_eq!(MsgBoxButtons::from_vb(4), MsgBoxButtons::YesNo);
        assert_eq!(MsgBoxButtons::from_vb(3), MsgBoxButtons::OkOnly);
        assert_eq!(MsgBoxButtons::from_vb(99), MsgBoxButtons::OkOnly);
    }

    #[test]
    fn result_codes_follow_vb() {
        assert_eq!(MsgBoxButtons::OkOnly.result(true), VB_OK);
        assert_eq!(MsgBoxButtons::OkOnly.result(false), VB_OK);
        assert_eq!(MsgBoxButtons::OkCancel.result(true), VB_OK);
        assert_eq!(MsgBoxButtons::OkCancel.result(false), VB_CANCEL);
        assert_eq!(MsgBoxButtons::YesNo.result(true), VB_YES);
        assert_eq!(MsgBoxButtons::YesNo.result(false), VB_NO);
    }
}
