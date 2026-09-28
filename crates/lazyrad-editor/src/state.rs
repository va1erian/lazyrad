#![forbid(unsafe_code)]

//! The editor widget's shared mutable state.
//!
//! The painter, the event mapper and the public API all reach the same
//! [`EditorState`] through an `Rc<RefCell<..>>`. It is deliberately separate
//! from the generic [`Editor`](crate::Editor) so painting and the pure modules
//! do not carry the app's message type.

use std::time::{Duration, Instant};

use crate::buffer::Buffer;
use crate::lexer::LineLexer;
use crate::markers::Marker;
use crate::options::Options;
use crate::platform::Clipboard;
use crate::view::View;

/// How long two clicks may be apart to count as a double/triple click.
const MULTI_CLICK: Duration = Duration::from_millis(400);
/// How far apart two clicks may be, in pixels, to count as one gesture.
const CLICK_SLOP: i32 = 4;

/// Tracked click timing, for double- and triple-click selection.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Click {
    /// When the previous click landed.
    pub(crate) at: Option<Instant>,
    /// Where the previous click landed.
    pub(crate) position: (i32, i32),
    /// The run length, capped at three.
    pub(crate) count: u8,
}

impl Click {
    /// Records a click at `(x, y)` and returns the run length (1, 2 or 3).
    pub(crate) fn register(&mut self, x: i32, y: i32) -> u8 {
        let now = Instant::now();
        let near =
            (x - self.position.0).abs() <= CLICK_SLOP && (y - self.position.1).abs() <= CLICK_SLOP;
        let recent = self
            .at
            .is_some_and(|at| now.duration_since(at) <= MULTI_CLICK);
        self.count = if recent && near {
            (self.count + 1).min(3)
        } else {
            1
        };
        self.at = Some(now);
        self.position = (x, y);
        self.count
    }

    /// Forces the run to `count`, used when a backend reports a double click
    /// directly rather than as a second press.
    pub(crate) fn set_count(&mut self, count: u8, x: i32, y: i32) {
        self.count = count.clamp(1, 3);
        self.at = Some(Instant::now());
        self.position = (x, y);
    }
}

/// A scrollbar drag in progress.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Drag {
    /// The scroll offset when the drag began.
    pub(crate) start_offset: i32,
    /// The pointer coordinate when the drag began.
    pub(crate) start_pointer: i32,
}

/// A `Ui` call the event mapper wants made once the state borrow is released.
///
/// The canvas backend delivers `SetFocus`/`KillFocus` and `CaptureChanged`
/// synchronously from inside `Ui::focus` and `Ui::release_capture`, straight
/// back into this widget's mapper. Making those calls while the mapper holds
/// `borrow_mut()` on the state would re-enter and panic, so the mapper records
/// them here and the widget applies them after dropping the borrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// `Ui::focus` on the editor.
    Focus,
    /// `Ui::set_capture` on the editor.
    Capture,
    /// `Ui::release_capture`.
    ReleaseCapture,
}

/// Everything the editor reads on paint and writes on input.
pub(crate) struct EditorState {
    /// The text buffer.
    pub(crate) buffer: Buffer,
    /// The incremental line-highlight cache.
    pub(crate) highlight: LineLexer,
    /// The caret, selection and scroll position.
    pub(crate) view: View,
    /// The display options.
    pub(crate) options: Options,
    /// Diagnostic markers from issue #11.
    pub(crate) markers: Vec<Marker>,
    /// Whether the widget has keyboard focus.
    pub(crate) focused: bool,
    /// Whether a form editor has selected the widget, so its painter draws an
    /// outline (the designer's selection).
    pub(crate) selected: bool,
    /// Whether the caret is in the "on" phase of its blink.
    pub(crate) blink_on: bool,
    /// The clipboard the editor copies through.
    pub(crate) clipboard: Box<dyn Clipboard>,
    /// Click timing for double- and triple-click.
    pub(crate) click: Click,
    /// The vertical scrollbar drag, while dragging.
    pub(crate) v_drag: Option<Drag>,
    /// The horizontal scrollbar drag, while dragging.
    pub(crate) h_drag: Option<Drag>,
    /// Whether the last drag captured the pointer.
    pub(crate) captured: bool,
    /// `Ui` calls to make after the current event, outside the borrow.
    pub(crate) effects: Vec<Effect>,
}

impl EditorState {
    /// A state over `text` with `options` and a `clipboard`.
    pub(crate) fn new(text: &str, options: Options, clipboard: Box<dyn Clipboard>) -> EditorState {
        let buffer = Buffer::new(text);
        let highlight = LineLexer::new(&buffer);
        EditorState {
            buffer,
            highlight,
            view: View::new(),
            options,
            markers: Vec::new(),
            focused: false,
            selected: false,
            blink_on: true,
            clipboard,
            click: Click::default(),
            v_drag: None,
            h_drag: None,
            captured: false,
            effects: Vec::new(),
        }
    }

    /// Resets the caret blink to its visible phase.
    pub(crate) fn reset_blink(&mut self) {
        self.blink_on = true;
    }

    /// Toggles the caret blink.
    pub(crate) fn toggle_blink(&mut self) {
        self.blink_on = !self.blink_on;
    }

    /// Re-lexes from the earliest line an edit touched.
    ///
    /// The [`Buffer`] records the dirty char index, so typing in a large file
    /// only re-lexes the lines the change affects (usually one).
    pub(crate) fn sync_highlight(&mut self) {
        if let Some(range) = self.buffer.take_dirty() {
            let from = self.buffer.line_of_char(range.start);
            let through = self.buffer.line_of_char(range.end);
            self.highlight.relex(&self.buffer, from, through);
        }
    }

    /// Rebuilds the highlight cache from scratch, after replacing the text.
    pub(crate) fn reset_highlight(&mut self) {
        self.highlight.reset(&self.buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::InProcessClipboard;

    #[test]
    fn a_new_state_has_a_caret_at_the_start() {
        let state = EditorState::new("hello", Options::default(), Box::new(InProcessClipboard));
        assert_eq!(state.view.caret, 0);
        assert_eq!(state.buffer.text(), "hello");
        assert!(state.view.selection().is_none());
    }

    #[test]
    fn consecutive_clicks_are_counted_up_to_three() {
        let mut click = Click::default();
        assert_eq!(click.register(10, 10), 1);
        assert_eq!(click.register(10, 11), 2);
        assert_eq!(click.register(11, 10), 3);
        assert_eq!(click.register(11, 10), 3, "the run caps at three");
    }

    #[test]
    fn a_far_click_starts_a_new_run() {
        let mut click = Click::default();
        click.register(0, 0);
        assert_eq!(click.register(100, 100), 1);
    }

    #[test]
    fn a_reported_double_click_sets_the_run_to_two() {
        let mut click = Click::default();
        click.register(5, 5);
        click.set_count(2, 5, 5);
        assert_eq!(click.count, 2);
        assert_eq!(click.register(5, 6), 3);
    }
}
