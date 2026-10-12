#![forbid(unsafe_code)]

//! The IDE's debugger state that needs no window: the breakpoint store, how
//! breakpoints follow edits, and the rows the debug panes show.
//!
//! # Conventions
//!
//! Every line number in this module is **one-based**, as in the debug
//! protocol, Rhai positions and diagnostics. The conversion to the code
//! editor's zero-based lines happens once, where markers are built
//! (`app/debug.rs`), and nowhere else. Breakpoints are keyed by *project item
//! name* (`main_form`), not by file name: the app maps an item to its code
//! file when it talks to the player.
//!
//! Breakpoints are IDE-session state, like VB6's: they are not written to the
//! project, so they never touch a file, the document's dirty flag or its undo
//! history.

use std::collections::{BTreeMap, BTreeSet};

use lazyrad_debug_proto::{Frame, Variable};

/// Where a debug run is, for what the commands and panes offer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DebugPhase {
    /// No debug run: either nothing runs or the program runs without the
    /// debugger.
    #[default]
    Off,
    /// The program runs under the debugger.
    Running,
    /// The program is stopped: at a breakpoint, after a step or on an error.
    Stopped,
}

/// The breakpoints of a project's items.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Breakpoints {
    by_item: BTreeMap<String, BTreeSet<usize>>,
}

impl Breakpoints {
    /// No breakpoints.
    pub fn new() -> Breakpoints {
        Breakpoints::default()
    }

    /// Whether any item has a breakpoint.
    pub fn is_empty(&self) -> bool {
        self.by_item.is_empty()
    }

    /// How many breakpoints there are in all.
    pub fn count(&self) -> usize {
        self.by_item.values().map(BTreeSet::len).sum()
    }

    /// The items that have breakpoints.
    pub fn items(&self) -> impl Iterator<Item = &str> {
        self.by_item.keys().map(String::as_str)
    }

    /// The breakpoint lines of `item`, ascending.
    pub fn lines(&self, item: &str) -> Vec<usize> {
        self.by_item
            .get(item)
            .map(|lines| lines.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Whether `item` has a breakpoint on `line`.
    pub fn has(&self, item: &str, line: usize) -> bool {
        self.by_item
            .get(item)
            .is_some_and(|lines| lines.contains(&line))
    }

    /// Sets or clears the breakpoint on `line` of `item` and returns whether
    /// it is set afterwards. Line `0` is not a line and is refused.
    pub fn toggle(&mut self, item: &str, line: usize) -> bool {
        if line == 0 {
            return false;
        }
        let lines = self.by_item.entry(item.to_owned()).or_default();
        let set = if lines.remove(&line) {
            false
        } else {
            lines.insert(line);
            true
        };
        if lines.is_empty() {
            self.by_item.remove(item);
        }
        set
    }

    /// Drops every breakpoint of `item` that lies past a document of
    /// `line_count` lines.
    pub fn clamp(&mut self, item: &str, line_count: usize) {
        if let Some(lines) = self.by_item.get_mut(item) {
            lines.retain(|line| *line <= line_count);
            if lines.is_empty() {
                self.by_item.remove(item);
            }
        }
    }

    /// Moves `item`'s breakpoints to follow an edit of its text from `old` to
    /// `new`, and drops any that no longer have a line. See [`shift_lines`].
    pub fn follow_edit(&mut self, item: &str, old: &str, new: &str) {
        let Some(lines) = self.by_item.get(item) else {
            return;
        };
        let moved: BTreeSet<usize> = shift_lines(old, new, lines.iter().copied())
            .into_iter()
            .collect();
        if moved.is_empty() {
            self.by_item.remove(item);
        } else {
            self.by_item.insert(item.to_owned(), moved);
        }
    }

    /// Moves `old`'s breakpoints to `new` (an item was renamed), replacing any
    /// `new` had.
    pub fn rename(&mut self, old: &str, new: &str) {
        if let Some(lines) = self.by_item.remove(old) {
            self.by_item.insert(new.to_owned(), lines);
        }
    }

    /// Forgets `item`'s breakpoints (the item was removed).
    pub fn remove(&mut self, item: &str) {
        self.by_item.remove(item);
    }

    /// Forgets everything (the project was closed).
    pub fn clear(&mut self) {
        self.by_item.clear();
    }
}

/// The number of lines of `text`: one more than its newlines, so an empty
/// document and a trailing newline both count the last (empty) line, as the
/// editor does.
pub fn line_count(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count() + 1
}

/// Where the one-based `lines` of `old` are in `new`.
///
/// The two texts are compared line by line (a trailing `\r` is ignored, so a
/// CRLF document diffs like an LF one). The common leading lines and the
/// common trailing lines are unchanged; what lies between is the edit:
///
/// * a line before the edit stays;
/// * a line after it moves by the edit's net change in line count, so inserting
///   or removing lines above a breakpoint carries it along;
/// * a line *inside* the edit keeps its line number while that line still
///   exists in the edited region (typing on a line, splitting it, or
///   rewriting a block), and is dropped when the edit deleted it.
///
/// A result past the end of `new` is dropped, and the result is ascending
/// without duplicates.
pub fn shift_lines(old: &str, new: &str, lines: impl IntoIterator<Item = usize>) -> Vec<usize> {
    let old_lines: Vec<&str> = old.split('\n').map(trim_cr).collect();
    let new_lines: Vec<&str> = new.split('\n').map(trim_cr).collect();

    let mut prefix = 0;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old_lines.len() - prefix
        && suffix < new_lines.len() - prefix
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    // The edit replaces old[prefix..old_end] with new[prefix..new_end].
    let old_end = old_lines.len() - suffix;
    let new_end = new_lines.len() - suffix;
    let kept_in_edit = new_end - prefix;

    let mut moved = BTreeSet::new();
    for line in lines {
        if line == 0 {
            continue;
        }
        let index = line - 1;
        let target = if index < prefix {
            Some(index)
        } else if index >= old_end {
            // After the edit: shifted by the net change.
            Some(index - old_end + new_end)
        } else if index - prefix < kept_in_edit {
            // Inside the edit and the edited region still has that line.
            Some(index)
        } else {
            None
        };
        if let Some(target) = target
            && target < new_lines.len()
        {
            moved.insert(target + 1);
        }
    }
    moved.into_iter().collect()
}

/// `line` without a trailing carriage return.
fn trim_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// How a pane row shows a local: `name = value  (type)`.
pub fn local_row(variable: &Variable) -> String {
    if variable.ty.is_empty() {
        format!("{} = {}", variable.name, variable.value)
    } else {
        format!("{} = {}   ({})", variable.name, variable.value, variable.ty)
    }
}

/// How a Call Stack row shows a frame: `#0 function  file:line`.
pub fn frame_row(index: usize, frame: &Frame) -> String {
    let place = match (frame.file.is_empty(), frame.line) {
        (true, _) => String::new(),
        (false, 0) => format!("   {}", frame.file),
        (false, line) => format!("   {}:{line}", frame.file),
    };
    format!("#{index} {}{place}", frame.function)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> String {
        text.to_owned()
    }

    #[test]
    fn toggling_sets_and_clears_and_refuses_line_zero() {
        let mut points = Breakpoints::new();
        assert!(points.is_empty());
        assert!(points.toggle("main_form", 3));
        assert!(points.has("main_form", 3));
        assert!(points.toggle("main_form", 1));
        assert_eq!(points.lines("main_form"), [1, 3]);
        assert_eq!(points.count(), 2);
        assert!(!points.toggle("main_form", 3), "second toggle clears");
        assert!(!points.toggle("main_form", 1));
        assert!(points.is_empty(), "an item without lines is forgotten");
        assert!(!points.toggle("main_form", 0));
        assert!(points.is_empty());
    }

    #[test]
    fn breakpoints_of_other_items_are_independent() {
        let mut points = Breakpoints::new();
        points.toggle("a", 2);
        points.toggle("b", 2);
        points.toggle("a", 2);
        assert_eq!(points.lines("b"), [2]);
        assert_eq!(points.items().collect::<Vec<_>>(), ["b"]);
    }

    #[test]
    fn lines_past_the_end_are_dropped() {
        let mut points = Breakpoints::new();
        points.toggle("a", 2);
        points.toggle("a", 9);
        points.clamp("a", 5);
        assert_eq!(points.lines("a"), [2]);
        points.clamp("a", 1);
        assert!(points.is_empty());
        points.clamp("missing", 3);
    }

    #[test]
    fn counting_lines_matches_the_editor() {
        assert_eq!(line_count(""), 1);
        assert_eq!(line_count("a"), 1);
        assert_eq!(line_count("a\n"), 2);
        assert_eq!(line_count("a\r\nb\r\n"), 3);
    }

    #[test]
    fn inserting_lines_above_carries_a_breakpoint_down() {
        let old = lines("one\ntwo\nthree\nfour");
        let new = lines("one\nNEW\nNEW2\ntwo\nthree\nfour");
        assert_eq!(shift_lines(&old, &new, [3, 4]), [5, 6]);
        // Before the edit nothing moves.
        assert_eq!(shift_lines(&old, &new, [1]), [1]);
    }

    #[test]
    fn removing_lines_above_carries_a_breakpoint_up_and_drops_deleted_ones() {
        let old = lines("one\ntwo\nthree\nfour\nfive");
        let new = lines("one\nfour\nfive");
        // `three` (3) was deleted with `two` (2); `four` and `five` moved up.
        assert_eq!(shift_lines(&old, &new, [2, 3, 4, 5]), [2, 3]);
    }

    #[test]
    fn typing_on_a_line_keeps_its_breakpoint() {
        let old = lines("a\nb\nc");
        let new = lines("a\nbbbb\nc");
        assert_eq!(shift_lines(&old, &new, [2]), [2]);
    }

    #[test]
    fn pressing_enter_at_the_start_of_a_line_moves_the_line_down() {
        // The cursor was at column 0 of `b`: a blank line appears above it.
        let old = lines("a\nb\nc");
        let new = lines("a\n\nb\nc");
        assert_eq!(shift_lines(&old, &new, [2]), [3]);
    }

    #[test]
    fn pressing_enter_inside_a_line_keeps_the_breakpoint_on_its_first_half() {
        let old = lines("a\nbcd\nc");
        let new = lines("a\nb\ncd\nc");
        assert_eq!(shift_lines(&old, &new, [2]), [2]);
        assert_eq!(shift_lines(&old, &new, [3]), [4], "the line below moved");
    }

    #[test]
    fn deleting_the_breakpoint_line_drops_it() {
        let old = lines("a\nb\nc");
        let new = lines("a\nc");
        assert_eq!(shift_lines(&old, &new, [2]), Vec::<usize>::new());
        assert_eq!(shift_lines(&old, &new, [3]), [2]);
    }

    #[test]
    fn replacing_a_block_keeps_breakpoints_inside_its_new_extent_only() {
        let old = lines("a\nx1\nx2\nx3\nz");
        let new = lines("a\ny1\nz");
        // The 3-line block became one line.
        assert_eq!(shift_lines(&old, &new, [2, 3, 4, 5]), [2, 3]);
    }

    #[test]
    fn crlf_documents_diff_like_lf_ones() {
        let old = "a\r\nb\r\nc\r\n";
        let new = "a\r\ninserted\r\nb\r\nc\r\n";
        assert_eq!(shift_lines(old, new, [2, 3]), [3, 4]);
        // Mixed endings: only the content is compared.
        assert_eq!(shift_lines("a\r\nb", "a\nb", [2]), [2]);
    }

    #[test]
    fn an_empty_or_identical_edit_changes_nothing() {
        let text = "a\nb\nc";
        assert_eq!(shift_lines(text, text, [1, 2, 3]), [1, 2, 3]);
        assert_eq!(shift_lines("", "", [1]), [1]);
        assert_eq!(shift_lines(text, text, []), Vec::<usize>::new());
    }

    #[test]
    fn clearing_the_document_drops_everything_but_the_first_line() {
        assert_eq!(shift_lines("a\nb\nc", "", [1, 2, 3]), [1]);
        assert_eq!(shift_lines("", "a\nb", [1]), [1]);
    }

    #[test]
    fn a_zero_line_is_ignored() {
        assert_eq!(shift_lines("a", "a\nb", [0, 1]), [1]);
    }

    #[test]
    fn follow_edit_updates_the_store_and_forgets_empty_items() {
        let mut points = Breakpoints::new();
        points.toggle("a", 3);
        points.follow_edit("a", "x\ny\nz", "x\ny\nQ\nz");
        assert_eq!(points.lines("a"), [4]);
        points.follow_edit("a", "x\ny\nQ\nz", "x\ny\nQ");
        assert!(points.is_empty(), "the line it was on is gone");
        points.follow_edit("missing", "a", "b");
    }

    #[test]
    fn rename_and_remove_follow_the_item() {
        let mut points = Breakpoints::new();
        points.toggle("old", 4);
        points.toggle("taken", 1);
        points.rename("old", "taken");
        assert_eq!(points.lines("taken"), [4], "a rename replaces the target");
        assert!(points.lines("old").is_empty());
        points.remove("taken");
        assert!(points.is_empty());
        points.toggle("x", 1);
        points.clear();
        assert!(points.is_empty());
    }

    #[test]
    fn pane_rows_read_plainly() {
        let variable = Variable {
            name: "total".to_owned(),
            ty: "i64".to_owned(),
            value: "6".to_owned(),
        };
        assert_eq!(local_row(&variable), "total = 6   (i64)");
        let untyped = Variable {
            ty: String::new(),
            ..variable
        };
        assert_eq!(local_row(&untyped), "total = 6");
        let frame = Frame {
            function: "go_button_click".to_owned(),
            file: "main_form.rhai".to_owned(),
            line: 12,
        };
        assert_eq!(
            frame_row(0, &frame),
            "#0 go_button_click   main_form.rhai:12"
        );
        let unknown = Frame {
            function: "<top level>".to_owned(),
            file: String::new(),
            line: 0,
        };
        assert_eq!(frame_row(2, &unknown), "#2 <top level>");
    }
}
