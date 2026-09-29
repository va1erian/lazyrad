#![forbid(unsafe_code)]

//! Which Edit commands the focused document can act on right now.
//!
//! The Edit menu greys out an entry the active document cannot perform, such
//! as Paste with an empty clipboard or Undo with nothing to undo. The pinned
//! xui toolbar can only be enabled or disabled as a whole, so only the menu
//! follows this state.

use crate::command::Command;

/// Per-command availability of the Edit group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditAvailability {
    /// Undo has a step to undo.
    pub undo: bool,
    /// Redo has a step to redo.
    pub redo: bool,
    /// Cut has something to cut.
    pub cut: bool,
    /// Copy has something to copy.
    pub copy: bool,
    /// Paste has something to paste.
    pub paste: bool,
    /// Delete has something to delete.
    pub delete: bool,
    /// Select All has something to select.
    pub select_all: bool,
}

impl EditAvailability {
    /// Every Edit command available; the state before any document is asked.
    pub const ALL: EditAvailability = EditAvailability::uniform(true);

    /// No Edit command available: nothing has a document to act on.
    pub const NONE: EditAvailability = EditAvailability::uniform(false);

    const fn uniform(available: bool) -> EditAvailability {
        EditAvailability {
            undo: available,
            redo: available,
            cut: available,
            copy: available,
            paste: available,
            delete: available,
            select_all: available,
        }
    }

    /// Whether `command` is available. Commands outside the Edit group are
    /// always allowed here; the dispatcher decides those by project state.
    pub fn allows(self, command: Command) -> bool {
        match command {
            Command::Undo => self.undo,
            Command::Redo => self.redo,
            Command::Cut => self.cut,
            Command::Copy => self.copy,
            Command::Paste => self.paste,
            Command::Delete => self.delete,
            Command::SelectAll => self.select_all,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_field_gates_only_its_own_command() {
        let only_paste = EditAvailability {
            paste: true,
            ..EditAvailability::NONE
        };
        for command in Command::ALL.iter().copied().filter(|c| c.acts_on_focus()) {
            assert_eq!(only_paste.allows(command), command == Command::Paste);
        }
        assert!(EditAvailability::NONE.allows(Command::Save));
        assert!(EditAvailability::NONE.allows(Command::Find));
    }
}
