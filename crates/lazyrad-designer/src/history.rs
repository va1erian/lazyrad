#![forbid(unsafe_code)]

//! A snapshot undo/redo stack.
//!
//! Every designer command changes the whole [`FormDoc`](xui_form::FormDoc)
//! model, so the simplest correct history stores whole-document snapshots. A
//! form is small, snapshots are cheap clones, and every command (move, resize,
//! delete, paste, nudge) gets undo and redo for free without a bespoke inverse
//! for each one. A gesture records one snapshot when it ends, not on every
//! pointer move.

/// A linear undo/redo history over whole-value snapshots.
#[derive(Clone, Debug)]
pub struct History<T> {
    snapshots: Vec<T>,
    cursor: usize,
}

impl<T: Clone + PartialEq> History<T> {
    /// A history whose only snapshot is `initial`.
    pub fn new(initial: T) -> History<T> {
        History {
            snapshots: vec![initial],
            cursor: 0,
        }
    }

    /// Discards every snapshot and restarts from `initial`.
    pub fn reset(&mut self, initial: T) {
        self.snapshots = vec![initial];
        self.cursor = 0;
    }

    /// Records `value` as the new present, dropping any redo snapshots after it.
    ///
    /// Returns whether anything was recorded; recording a value equal to the
    /// current snapshot is a no-op, so a gesture that did not move anything
    /// leaves no undo step behind.
    pub fn record(&mut self, value: &T) -> bool {
        if self.snapshots[self.cursor] == *value {
            return false;
        }
        self.snapshots.truncate(self.cursor + 1);
        self.snapshots.push(value.clone());
        self.cursor = self.snapshots.len() - 1;
        true
    }

    /// Steps back one snapshot, returning the value that becomes the present.
    pub fn undo(&mut self) -> Option<&T> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        self.snapshots.get(self.cursor)
    }

    /// Steps forward one snapshot, returning the value that becomes the present.
    pub fn redo(&mut self) -> Option<&T> {
        if self.cursor + 1 >= self.snapshots.len() {
            return None;
        }
        self.cursor += 1;
        self.snapshots.get(self.cursor)
    }

    /// Whether [`History::undo`] would step back.
    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    /// Whether [`History::redo`] would step forward.
    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.snapshots.len()
    }

    /// How many snapshots are stored, including the present.
    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    /// Whether the history holds only the present snapshot.
    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_walk_the_snapshots() {
        let mut history = History::new(0);
        history.record(&1);
        history.record(&2);
        assert!(history.can_undo());
        assert_eq!(history.undo(), Some(&1));
        assert_eq!(history.undo(), Some(&0));
        assert!(!history.can_undo());
        assert_eq!(history.undo(), None);
        assert!(history.can_redo());
        assert_eq!(history.redo(), Some(&1));
        assert_eq!(history.redo(), Some(&2));
        assert_eq!(history.redo(), None);
    }

    #[test]
    fn recording_the_same_value_is_a_no_op() {
        let mut history = History::new(0);
        assert!(!history.record(&0));
        assert_eq!(history.len(), 1);
        assert!(!history.can_undo());
    }

    #[test]
    fn a_new_record_drops_the_redo_branch() {
        let mut history = History::new(0);
        history.record(&1);
        history.record(&2);
        history.undo();
        assert_eq!(history.undo(), Some(&0));
        history.record(&9);
        assert!(!history.can_redo());
        assert_eq!(history.redo(), None);
    }

    #[test]
    fn reset_forgets_everything() {
        let mut history = History::new(0);
        history.record(&1);
        history.reset(5);
        assert!(!history.can_undo());
        assert!(!history.can_redo());
        assert_eq!(history.len(), 1);
    }
}
