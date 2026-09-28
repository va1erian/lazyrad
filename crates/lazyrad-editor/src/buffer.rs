#![forbid(unsafe_code)]

//! The editor's text buffer: a [`ropey::Rope`], a line index and a coalescing
//! undo/redo stack.
//!
//! The buffer is deliberately free of UI code. Every operation works in *char*
//! indices (not bytes), so a multi-byte character is one caret step, and the
//! whole module is unit-tested without a backend (PLAN.md §5).
//!
//! The line index is rebuilt lazily after an edit and cached, so a burst of
//! edits (a typing run) pays for it once, on the next read, rather than on every
//! keystroke. Reads still take `&self` because the index sits behind a
//! `RefCell`.
//!
//! Undo is stored as a stack of *groups*; a group is a list of splices applied
//! as one user action. Typing runs coalesce into a single group (the
//! [`Buffer::insert`] `coalesce` flag), while an explicit
//! [`Buffer::begin_edit`]/[`Buffer::end_edit`] pair forces one group for a
//! compound change such as indenting a block of lines.

use std::cell::{RefCell, RefMut};
use std::ops::Range;

use ropey::Rope;

/// One reversible splice: `before` at `anchor` was replaced by `after`.
///
/// Applying it forward removes `before` and inserts `after`; applying it
/// backward does the reverse. All positions are char indices.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Edit {
    anchor: usize,
    before: String,
    after: String,
}

impl Edit {
    /// The length of `before` in chars.
    fn before_len(&self) -> usize {
        self.before.chars().count()
    }

    /// The length of `after` in chars.
    fn after_len(&self) -> usize {
        self.after.chars().count()
    }

    /// Whether this edit is a pure insertion (nothing removed).
    fn is_insertion(&self) -> bool {
        self.before.is_empty()
    }

    /// Whether this edit is a pure deletion (nothing inserted).
    fn is_deletion(&self) -> bool {
        self.after.is_empty()
    }

    /// Tries to fold `next` into this edit, returning whether it did. Only
    /// adjacent pure insertions or pure deletions coalesce, which is exactly
    /// what a run of typing or a run of backspaces produces.
    fn try_merge(&mut self, next: &Edit) -> bool {
        if self.is_insertion()
            && next.is_insertion()
            && next.anchor == self.anchor + self.after_len()
        {
            self.after.push_str(&next.after);
            return true;
        }
        if self.is_deletion() && next.is_deletion() {
            // Backspace: the new removal sits immediately before this one.
            if next.anchor + next.before_len() == self.anchor {
                let mut merged = next.before.clone();
                merged.push_str(&self.before);
                self.before = merged;
                self.anchor = next.anchor;
                return true;
            }
            // Forward delete: the new removal sits at the same anchor, since
            // the following chars shifted left after the first removal.
            if next.anchor == self.anchor {
                self.before.push_str(&next.before);
                return true;
            }
        }
        false
    }
}

/// A list of edits applied as one undoable action.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Group {
    edits: Vec<Edit>,
    /// Whether a further adjacent insertion/deletion may coalesce into this
    /// group. Set for typing runs, off for everything else.
    coalesce: bool,
}

/// The char index where each line starts, plus the longest line's length.
///
/// Rebuilt lazily: [`Buffer`] marks it stale after an edit and the next read
/// rebuilds it in one linear pass.
#[derive(Debug, Default)]
struct LineIndex {
    starts: Vec<usize>,
    max_line_chars: usize,
    stale: bool,
}

impl LineIndex {
    /// Rebuilds the index from `rope` if it is stale.
    fn ensure(&mut self, rope: &Rope) {
        if !self.stale {
            return;
        }
        self.rebuild(rope);
    }

    /// Rebuilds the index from `rope`.
    fn rebuild(&mut self, rope: &Rope) {
        self.starts.clear();
        self.max_line_chars = 0;
        let lines = rope.len_lines().max(1);
        self.starts.reserve(lines);
        for line in 0..lines {
            self.starts.push(rope.line_to_char(line));
            let len = line_content_len(&rope.line(line));
            self.max_line_chars = self.max_line_chars.max(len);
        }
        self.stale = false;
    }

    /// The line holding `char_idx`, found by binary search over the starts.
    fn line_of(&self, char_idx: usize) -> usize {
        match self.starts.binary_search(&char_idx) {
            Ok(line) => line,
            Err(next) => next.saturating_sub(1),
        }
    }
}

/// The number of chars in a line slice, excluding its terminator.
fn line_content_len(slice: &ropey::RopeSlice<'_>) -> usize {
    let len = slice.len_chars();
    if len == 0 {
        return 0;
    }
    let mut end = len;
    if slice.char(len - 1) == '\n' {
        end -= 1;
    }
    if end > 0 && slice.char(end - 1) == '\r' {
        end -= 1;
    }
    end
}

/// The text buffer.
pub struct Buffer {
    rope: Rope,
    index: RefCell<LineIndex>,
    undo: Vec<Group>,
    redo: Vec<Group>,
    /// The group an explicit [`Buffer::begin_edit`] is accumulating into.
    pending: Option<Group>,
}

impl Buffer {
    /// A buffer holding `text`.
    pub fn new(text: &str) -> Buffer {
        let rope = Rope::from_str(text);
        let mut index = LineIndex::default();
        index.rebuild(&rope);
        Buffer {
            rope,
            index: RefCell::new(index),
            undo: Vec::new(),
            redo: Vec::new(),
            pending: None,
        }
    }

    /// The whole text.
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// The number of chars.
    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// The line index, rebuilt first if an edit made it stale.
    fn index(&self) -> RefMut<'_, LineIndex> {
        let mut index = self.index.borrow_mut();
        index.ensure(&self.rope);
        index
    }

    /// Marks the cached line index stale after an edit.
    fn invalidate_index(&self) {
        self.index.borrow_mut().stale = true;
    }

    /// The number of lines. A trailing newline yields a final empty line, as a
    /// text editor shows one.
    pub fn line_count(&self) -> usize {
        self.index().starts.len().max(1)
    }

    /// The number of chars in the longest line, for the horizontal extent.
    pub fn max_line_chars(&self) -> usize {
        self.index().max_line_chars
    }

    /// The zero-based line holding `char_idx`.
    pub fn line_of_char(&self, char_idx: usize) -> usize {
        self.index().line_of(char_idx.min(self.rope.len_chars()))
    }

    /// The char index where `line` starts.
    pub fn line_start(&self, line: usize) -> usize {
        self.index()
            .starts
            .get(line)
            .copied()
            .unwrap_or(self.rope.len_chars())
    }

    /// The char index just past `line`'s content, before any terminator.
    pub fn line_end(&self, line: usize) -> usize {
        let start = self.line_start(line);
        let stop = if line + 1 < self.line_count() {
            self.line_start(line + 1)
        } else {
            self.rope.len_chars()
        };
        let slice = self.rope.slice(start..stop);
        start + line_content_len(&slice)
    }

    /// `line`'s text without its line terminator.
    pub fn line_string(&self, line: usize) -> String {
        let start = self.line_start(line);
        let end = self.line_end(line);
        self.rope.slice(start..end).to_string()
    }

    /// The char at `char_idx`.
    pub fn char_at(&self, char_idx: usize) -> Option<char> {
        self.rope.get_char(char_idx)
    }

    /// The text in `range`, clamped to the buffer.
    pub fn slice(&self, range: Range<usize>) -> String {
        let start = range.start.min(self.rope.len_chars());
        let end = range.end.min(self.rope.len_chars()).max(start);
        self.rope.slice(start..end).to_string()
    }

    /// Inserts `text` at `at`. When `coalesce` is set and the previous edit was
    /// an adjacent typing run, the two share one undo group.
    pub fn insert(&mut self, at: usize, text: &str, coalesce: bool) {
        let at = at.min(self.rope.len_chars());
        let edit = Edit {
            anchor: at,
            before: String::new(),
            after: text.to_string(),
        };
        self.rope.insert(at, text);
        self.record(edit, coalesce);
    }

    /// Removes `range`.
    pub fn remove(&mut self, range: Range<usize>, coalesce: bool) {
        let start = range.start.min(self.rope.len_chars());
        let end = range.end.min(self.rope.len_chars()).max(start);
        if start == end {
            return;
        }
        let before = self.rope.slice(start..end).to_string();
        let edit = Edit {
            anchor: start,
            before,
            after: String::new(),
        };
        self.rope.remove(start..end);
        self.record(edit, coalesce);
    }

    /// Replaces `range` with `text`.
    pub fn replace(&mut self, range: Range<usize>, text: &str, coalesce: bool) {
        let start = range.start.min(self.rope.len_chars());
        let end = range.end.min(self.rope.len_chars()).max(start);
        let before = self.rope.slice(start..end).to_string();
        let edit = Edit {
            anchor: start,
            before,
            after: text.to_string(),
        };
        self.rope.remove(start..end);
        self.rope.insert(start, text);
        self.record(edit, coalesce);
    }

    /// Starts an explicit undo group: every edit until [`Buffer::end_edit`]
    /// undoes as one action.
    pub fn begin_edit(&mut self) {
        self.pending = Some(Group {
            edits: Vec::new(),
            coalesce: false,
        });
    }

    /// Stops the current typing/deletion run from coalescing with later edits.
    ///
    /// The editor calls this when the caret moves for a reason other than
    /// typing, so a backspace after navigation starts a fresh undo group.
    pub fn break_coalescing(&mut self) {
        if let Some(group) = self.undo.last_mut() {
            group.coalesce = false;
        }
    }

    /// Ends the group started by [`Buffer::begin_edit`].
    pub fn end_edit(&mut self) {
        if let Some(group) = self.pending.take() {
            self.invalidate_index();
            if !group.edits.is_empty() {
                self.redo.clear();
                self.undo.push(group);
            }
        }
    }

    /// Whether every change has been undone.
    pub fn is_clean(&self) -> bool {
        self.undo.is_empty()
    }

    /// Undoes the last group, returning the caret char index it should move to.
    pub fn undo(&mut self) -> Option<usize> {
        let group = self.undo.pop()?;
        for edit in group.edits.iter().rev() {
            self.apply_backward(edit);
        }
        let caret = group.edits.first().map(|edit| edit.anchor);
        self.invalidate_index();
        self.redo.push(group);
        caret
    }

    /// Redoes the last undone group, returning the caret char index.
    pub fn redo(&mut self) -> Option<usize> {
        let group = self.redo.pop()?;
        for edit in group.edits.iter() {
            self.apply_forward(edit);
        }
        let caret = group
            .edits
            .last()
            .map(|edit| edit.anchor + edit.after_len());
        self.invalidate_index();
        self.undo.push(group);
        caret
    }

    /// Applies `edit` forward.
    fn apply_forward(&mut self, edit: &Edit) {
        let start = edit.anchor.min(self.rope.len_chars());
        let end = (start + edit.before_len()).min(self.rope.len_chars());
        self.rope.remove(start..end);
        self.rope.insert(start, &edit.after);
    }

    /// Applies `edit` backward.
    fn apply_backward(&mut self, edit: &Edit) {
        let start = edit.anchor.min(self.rope.len_chars());
        let end = (start + edit.after_len()).min(self.rope.len_chars());
        self.rope.remove(start..end);
        self.rope.insert(start, &edit.before);
    }

    /// Records `edit`, either into the pending explicit group, into the last
    /// coalescing group, or as a new group.
    fn record(&mut self, edit: Edit, coalesce: bool) {
        if let Some(group) = self.pending.as_mut() {
            group.edits.push(edit);
            return;
        }
        self.invalidate_index();
        self.redo.clear();
        if coalesce
            && let Some(group) = self.undo.last_mut()
            && group.coalesce
            && let Some(last) = group.edits.last_mut()
            && last.try_merge(&edit)
        {
            return;
        }
        self.undo.push(Group {
            edits: vec![edit],
            coalesce,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::Buffer;

    #[test]
    fn line_index_tracks_lines_including_a_trailing_empty_one() {
        let buffer = Buffer::new("one\ntwo\nthree");
        assert_eq!(buffer.line_count(), 3);
        assert_eq!(buffer.line_string(0), "one");
        assert_eq!(buffer.line_string(2), "three");
        assert_eq!(buffer.line_of_char(0), 0);
        assert_eq!(buffer.line_of_char(4), 1);
        assert_eq!(buffer.line_of_char(8), 2);

        let buffer = Buffer::new("a\n");
        assert_eq!(buffer.line_count(), 2);
        assert_eq!(buffer.line_string(1), "");
    }

    #[test]
    fn crlf_terminators_are_not_part_of_the_line() {
        let buffer = Buffer::new("one\r\ntwo");
        assert_eq!(buffer.line_string(0), "one");
        assert_eq!(buffer.line_end(0), 3);
        assert_eq!(buffer.line_string(1), "two");
    }

    #[test]
    fn insert_and_remove_update_the_text() {
        let mut buffer = Buffer::new("hello world");
        buffer.insert(5, ",", true);
        assert_eq!(buffer.text(), "hello, world");
        buffer.remove(5..6, true);
        assert_eq!(buffer.text(), "hello world");
    }

    #[test]
    fn a_typing_run_coalesces_into_one_undo_group() {
        let mut buffer = Buffer::new("");
        for (at, ch) in "abc".chars().enumerate() {
            buffer.insert(at, &ch.to_string(), true);
        }
        assert_eq!(buffer.text(), "abc");
        assert_eq!(buffer.undo(), Some(0));
        assert_eq!(buffer.text(), "");
        assert_eq!(buffer.redo(), Some(3));
        assert_eq!(buffer.text(), "abc");
    }

    #[test]
    fn non_adjacent_insertions_do_not_coalesce() {
        let mut buffer = Buffer::new("ab");
        buffer.insert(2, "c", true);
        buffer.insert(0, "x", true);
        assert_eq!(buffer.text(), "xabc");
        buffer.undo();
        assert_eq!(buffer.text(), "abc");
        buffer.undo();
        assert_eq!(buffer.text(), "ab");
    }

    #[test]
    fn a_backspace_run_coalesces_into_one_group() {
        let mut buffer = Buffer::new("abcd");
        buffer.remove(3..4, true);
        buffer.remove(2..3, true);
        assert_eq!(buffer.text(), "ab");
        assert_eq!(buffer.undo(), Some(2));
        assert_eq!(buffer.text(), "abcd");
    }

    #[test]
    fn a_forward_delete_run_coalesces_into_one_group() {
        let mut buffer = Buffer::new("abcd");
        buffer.remove(1..2, true);
        buffer.remove(1..2, true);
        assert_eq!(buffer.text(), "ad");
        assert_eq!(buffer.undo(), Some(1));
        assert_eq!(buffer.text(), "abcd");
    }

    #[test]
    fn an_explicit_edit_is_one_undo_step() {
        let mut buffer = Buffer::new("a\nb\nc");
        buffer.begin_edit();
        buffer.insert(0, "  ", false);
        buffer.insert(4, "  ", false);
        buffer.insert(8, "  ", false);
        buffer.end_edit();
        assert_eq!(buffer.text(), "  a\n  b\n  c");
        buffer.undo();
        assert_eq!(buffer.text(), "a\nb\nc");
    }

    #[test]
    fn a_new_edit_clears_the_redo_stack() {
        let mut buffer = Buffer::new("a");
        buffer.insert(1, "b", true);
        buffer.undo();
        assert_eq!(buffer.text(), "a");
        buffer.insert(0, "z", true);
        assert_eq!(buffer.redo(), None);
        assert_eq!(buffer.text(), "za");
    }

    #[test]
    fn multi_byte_chars_move_as_one_step() {
        let mut buffer = Buffer::new("héllo");
        assert_eq!(buffer.len_chars(), 5);
        buffer.remove(1..2, true);
        assert_eq!(buffer.text(), "hllo");
        buffer.insert(1, "é", true);
        assert_eq!(buffer.text(), "héllo");
        assert_eq!(buffer.line_of_char(buffer.len_chars()), 0);
    }

    #[test]
    fn undo_reports_the_caret_it_restores() {
        let mut buffer = Buffer::new("hello");
        buffer.insert(5, " world", true);
        assert_eq!(buffer.undo(), Some(5));
        assert_eq!(buffer.redo(), Some(11));
    }

    #[test]
    fn the_longest_line_is_tracked() {
        let mut buffer = Buffer::new("a\nlonger\nbb");
        assert_eq!(buffer.max_line_chars(), 6);
        buffer.insert(0, "xxxxxxx\n", false);
        assert_eq!(buffer.max_line_chars(), 7);
    }

    #[test]
    fn a_five_thousand_line_file_stays_responsive_to_edits() {
        // 5,000 lines of Rhai-ish text, then 1,000 single-character inserts at
        // the end. If any step were quadratic this would take minutes; the
        // generous bound only catches that, not normal machine variance.
        let mut text = String::new();
        for line in 0..5_000 {
            text.push_str("fn handler_");
            text.push_str(&line.to_string());
            text.push_str("() { let x = 1; }\n");
        }
        let mut buffer = Buffer::new(&text);
        assert_eq!(buffer.line_count(), 5_001);

        let started = std::time::Instant::now();
        let base = buffer.len_chars();
        for caret in base..base + 1_000 {
            buffer.insert(caret, "x", true);
        }
        assert_eq!(buffer.len_chars(), text.chars().count() + 1_000);
        buffer.undo();
        assert_eq!(buffer.len_chars(), text.chars().count());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(15),
            "1,000 edits on a 5,000-line file took {:?}",
            started.elapsed()
        );
    }
}
