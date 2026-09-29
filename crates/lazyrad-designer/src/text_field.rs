#![forbid(unsafe_code)]

//! The pure model behind the property grid's inline text editor: text, caret,
//! selection and undo history, with the standard Edit chords (Ctrl+A, C, X, V,
//! Z, Y).
//!
//! xui's `Edit` has no hook for the grid's commit-on-Enter and commit-on-blur
//! rules, so the grid paints its own field. This model keeps that field's
//! editing testable without a window: it takes a key, the modifiers and a
//! [`Clipboard`], and reports whether it consumed the key.

use xui_code_editor::Clipboard;
use xui_core::message::{Key, Modifiers};

/// How many undo steps a field keeps.
const HISTORY: usize = 100;

/// One saved state, restored by undo and redo.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    chars: Vec<char>,
    caret: usize,
}

/// A single-line text field: text, caret, selection and undo history.
#[derive(Clone, Debug)]
pub(crate) struct TextField {
    chars: Vec<char>,
    caret: usize,
    /// The other end of the selection, when one exists.
    anchor: Option<usize>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Whether the last edit was a typed character, so a run of typing undoes
    /// as one step.
    typing: bool,
}

impl TextField {
    /// A field holding `initial`, with the caret at its end.
    pub(crate) fn new(initial: &str) -> TextField {
        let chars: Vec<char> = initial.chars().collect();
        TextField {
            caret: chars.len(),
            chars,
            anchor: None,
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
        }
    }

    /// The text.
    pub(crate) fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// The caret's position, in chars.
    pub(crate) fn caret(&self) -> usize {
        self.caret
    }

    /// The selected char range, ordered, or `None` when nothing is selected.
    pub(crate) fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        (anchor != self.caret).then(|| (anchor.min(self.caret), anchor.max(self.caret)))
    }

    /// The selected text, or an empty string.
    pub(crate) fn selected_text(&self) -> String {
        self.selection()
            .map(|(start, end)| self.chars[start..end].iter().collect())
            .unwrap_or_default()
    }

    /// Whether an undo step exists.
    #[cfg(test)]
    pub(crate) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether a redo step exists.
    #[cfg(test)]
    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            chars: self.chars.clone(),
            caret: self.caret,
        }
    }

    /// Records the state before an edit. `typing` edits coalesce with a
    /// preceding run of typing.
    fn checkpoint(&mut self, typing: bool) {
        if !(typing && self.typing) {
            self.undo.push(self.snapshot());
            if self.undo.len() > HISTORY {
                self.undo.remove(0);
            }
        }
        self.typing = typing;
        self.redo.clear();
    }

    /// Removes the selection, leaving the caret at its start.
    fn remove_selection(&mut self) {
        if let Some((start, end)) = self.selection() {
            self.chars.drain(start..end);
            self.caret = start;
        }
        self.anchor = None;
    }

    /// Types `character`, replacing the selection. Control characters are
    /// ignored.
    pub(crate) fn insert_char(&mut self, character: char) -> bool {
        if character.is_control() {
            return false;
        }
        self.checkpoint(true);
        self.remove_selection();
        self.chars.insert(self.caret, character);
        self.caret += 1;
        true
    }

    /// Inserts `text` at the caret, replacing the selection. Line breaks and
    /// other control characters are dropped, since the field is one line.
    fn insert_text(&mut self, text: &str) -> bool {
        let clean: Vec<char> = text.chars().filter(|c| !c.is_control()).collect();
        if clean.is_empty() {
            return false;
        }
        self.checkpoint(false);
        self.remove_selection();
        let at = self.caret;
        self.chars.splice(at..at, clean.iter().copied());
        self.caret = at + clean.len();
        true
    }

    /// Selects the whole text.
    pub(crate) fn select_all(&mut self) {
        self.typing = false;
        self.anchor = Some(0);
        self.caret = self.chars.len();
    }

    /// Copies the selection to `clipboard`. Returns whether anything was
    /// copied; an empty selection leaves the clipboard alone.
    pub(crate) fn copy(&self, clipboard: &dyn Clipboard) -> bool {
        let selected = self.selected_text();
        if selected.is_empty() {
            return false;
        }
        clipboard.set_text(&selected);
        true
    }

    /// Cuts the selection to `clipboard`.
    pub(crate) fn cut(&mut self, clipboard: &dyn Clipboard) -> bool {
        if !self.copy(clipboard) {
            return false;
        }
        self.checkpoint(false);
        self.remove_selection();
        true
    }

    /// Pastes the clipboard's text. An empty or unavailable clipboard changes
    /// nothing.
    pub(crate) fn paste(&mut self, clipboard: &dyn Clipboard) -> bool {
        match clipboard.text() {
            Some(text) => self.insert_text(&text),
            None => false,
        }
    }

    /// Steps back one edit.
    pub(crate) fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.restore(previous);
        true
    }

    /// Re-applies the last undone edit.
    pub(crate) fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.restore(next);
        true
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.chars = snapshot.chars;
        self.caret = snapshot.caret.min(self.chars.len());
        self.anchor = None;
        self.typing = false;
    }

    /// Moves the caret to `to`, extending the selection when `extend`.
    fn move_to(&mut self, to: usize, extend: bool) {
        self.typing = false;
        if extend {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = to.min(self.chars.len());
    }

    /// Backspace (`forward == false`) or Delete: removes the selection, else
    /// the char before or after the caret. Records history only when
    /// something is removed.
    fn erase(&mut self, forward: bool) {
        let removable = self.selection().is_some()
            || if forward {
                self.caret < self.chars.len()
            } else {
                self.caret > 0
            };
        if !removable {
            self.anchor = None;
            return;
        }
        self.checkpoint(false);
        if self.selection().is_some() {
            self.remove_selection();
        } else if forward {
            self.chars.remove(self.caret);
        } else {
            self.caret -= 1;
            self.chars.remove(self.caret);
        }
    }

    /// Runs a key. Returns whether the field consumed it (and so must repaint);
    /// Enter, Escape and any key the field has no use for stay unconsumed so
    /// the caller can act on them.
    pub(crate) fn key(
        &mut self,
        key: Key,
        modifiers: Modifiers,
        clipboard: &dyn Clipboard,
    ) -> bool {
        if modifiers.alt || modifiers.win {
            return false;
        }
        if modifiers.ctrl {
            match key {
                Key::A => self.select_all(),
                Key::C => {
                    self.copy(clipboard);
                }
                Key::X => {
                    self.cut(clipboard);
                }
                Key::V => {
                    self.paste(clipboard);
                }
                Key::Z if modifiers.shift => {
                    self.redo();
                }
                Key::Z => {
                    self.undo();
                }
                Key::Y => {
                    self.redo();
                }
                _ => return false,
            }
            return true;
        }
        let len = self.chars.len();
        match key {
            Key::BACK => self.erase(false),
            Key::DELETE => self.erase(true),
            Key::LEFT => {
                let to = match self.selection() {
                    Some((start, _)) if !modifiers.shift => start,
                    _ => self.caret.saturating_sub(1),
                };
                self.move_to(to, modifiers.shift);
            }
            Key::RIGHT => {
                let to = match self.selection() {
                    Some((_, end)) if !modifiers.shift => end,
                    _ => self.caret + 1,
                };
                self.move_to(to, modifiers.shift);
            }
            Key::HOME => self.move_to(0, modifiers.shift),
            Key::END => self.move_to(len, modifiers.shift),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// A clipboard owned by the test, so no OS clipboard is touched.
    #[derive(Default)]
    struct Fake(RefCell<Option<String>>);

    impl Clipboard for Fake {
        fn text(&self) -> Option<String> {
            self.0.borrow().clone().filter(|text| !text.is_empty())
        }

        fn set_text(&self, text: &str) {
            *self.0.borrow_mut() = Some(text.to_owned());
        }
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        }
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        }
    }

    fn ctrl_shift() -> Modifiers {
        Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        }
    }

    #[test]
    fn select_all_then_copy_fills_the_clipboard() {
        let clip = Fake::default();
        let mut field = TextField::new("hello");
        assert!(field.key(Key::A, ctrl(), &clip));
        assert_eq!(field.selection(), Some((0, 5)));
        assert!(field.key(Key::C, ctrl(), &clip));
        assert_eq!(clip.text().as_deref(), Some("hello"));
        assert_eq!(field.text(), "hello", "copy leaves the text alone");
    }

    #[test]
    fn copy_without_a_selection_keeps_the_clipboard() {
        let clip = Fake::default();
        clip.set_text("keep");
        let mut field = TextField::new("abc");
        assert!(field.key(Key::C, ctrl(), &clip), "the chord is consumed");
        assert_eq!(clip.text().as_deref(), Some("keep"));
    }

    #[test]
    fn cut_removes_the_selection_and_undo_restores_it() {
        let clip = Fake::default();
        let mut field = TextField::new("hello");
        field.key(Key::A, ctrl(), &clip);
        field.key(Key::X, ctrl(), &clip);
        assert_eq!(field.text(), "");
        assert_eq!(clip.text().as_deref(), Some("hello"));
        field.key(Key::Z, ctrl(), &clip);
        assert_eq!(field.text(), "hello");
        field.key(Key::Y, ctrl(), &clip);
        assert_eq!(field.text(), "");
    }

    #[test]
    fn paste_replaces_the_selection_and_flattens_line_breaks() {
        let clip = Fake::default();
        clip.set_text("a\r\nb");
        let mut field = TextField::new("xyz");
        field.key(Key::A, ctrl(), &clip);
        field.key(Key::V, ctrl(), &clip);
        assert_eq!(field.text(), "ab");
        assert_eq!(field.caret(), 2);
    }

    #[test]
    fn paste_from_an_empty_clipboard_changes_nothing() {
        let clip = Fake::default();
        let mut field = TextField::new("abc");
        assert!(field.key(Key::V, ctrl(), &clip));
        assert_eq!(field.text(), "abc");
        assert!(!field.can_undo(), "no history for a no-op paste");
    }

    #[test]
    fn typing_coalesces_into_one_undo_step() {
        let clip = Fake::default();
        let mut field = TextField::new("");
        for c in "abc".chars() {
            field.insert_char(c);
        }
        assert_eq!(field.text(), "abc");
        field.key(Key::Z, ctrl(), &clip);
        assert_eq!(field.text(), "");
        assert!(!field.can_undo());
        assert!(field.can_redo());
        field.key(Key::Z, ctrl_shift(), &clip);
        assert_eq!(field.text(), "abc", "Ctrl+Shift+Z redoes");
    }

    #[test]
    fn a_new_edit_drops_the_redo_history() {
        let clip = Fake::default();
        let mut field = TextField::new("");
        field.insert_char('a');
        field.key(Key::Z, ctrl(), &clip);
        assert!(field.can_redo());
        field.insert_char('b');
        assert!(!field.can_redo());
    }

    #[test]
    fn undo_and_redo_with_no_history_do_nothing() {
        let clip = Fake::default();
        let mut field = TextField::new("abc");
        assert!(field.key(Key::Z, ctrl(), &clip));
        assert!(field.key(Key::Y, ctrl(), &clip));
        assert_eq!(field.text(), "abc");
    }

    #[test]
    fn backspace_and_delete_edit_around_the_caret_or_the_selection() {
        let clip = Fake::default();
        let mut field = TextField::new("abcd");
        field.key(Key::BACK, Modifiers::NONE, &clip);
        assert_eq!(field.text(), "abc");
        field.key(Key::HOME, Modifiers::NONE, &clip);
        field.key(Key::DELETE, Modifiers::NONE, &clip);
        assert_eq!(field.text(), "bc");
        field.key(Key::END, shift(), &clip);
        assert_eq!(field.selected_text(), "bc");
        field.key(Key::DELETE, Modifiers::NONE, &clip);
        assert_eq!(field.text(), "");
    }

    #[test]
    fn removal_at_the_edges_records_no_history() {
        let clip = Fake::default();
        let mut field = TextField::new("");
        field.key(Key::BACK, Modifiers::NONE, &clip);
        field.key(Key::DELETE, Modifiers::NONE, &clip);
        assert!(!field.can_undo());
    }

    #[test]
    fn shift_arrows_extend_and_plain_arrows_collapse() {
        let clip = Fake::default();
        let mut field = TextField::new("abc");
        field.key(Key::LEFT, shift(), &clip);
        field.key(Key::LEFT, shift(), &clip);
        assert_eq!(field.selected_text(), "bc");
        field.key(Key::LEFT, Modifiers::NONE, &clip);
        assert_eq!(field.selection(), None);
        assert_eq!(field.caret(), 1);
    }

    #[test]
    fn enter_escape_and_unbound_chords_are_left_to_the_caller() {
        let clip = Fake::default();
        let mut field = TextField::new("abc");
        assert!(!field.key(Key::RETURN, Modifiers::NONE, &clip));
        assert!(!field.key(Key::ESCAPE, Modifiers::NONE, &clip));
        assert!(!field.key(Key::S, ctrl(), &clip), "Ctrl+S is the IDE's");
        assert!(!field.key(Key::F5, Modifiers::NONE, &clip));
    }

    #[test]
    fn control_characters_are_not_typed() {
        let mut field = TextField::new("");
        assert!(!field.insert_char('\u{1}'));
        assert_eq!(field.text(), "");
        assert!(!field.can_undo());
    }
}
