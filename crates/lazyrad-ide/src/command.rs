#![forbid(unsafe_code)]

//! The central [`Command`] vocabulary and the [`Dispatcher`] that owns each
//! command's enabled state.
//!
//! Every IDE action is a [`Command`]. The menu bar, the toolbar and the
//! keyboard shortcut handler all produce the same values, so an action behaves
//! identically however it is invoked (PLAN.md §9). Until the feature behind a
//! command lands, dispatching it only logs (PLAN.md §9, issue #7).

use std::collections::HashMap;

use xui_core::message::{Key, Modifiers};

/// One IDE action.
///
/// The variants are grouped by the menu they belong to. `OpenRecent` carries
/// the index into the recent-projects list, so a whole submenu maps to one
/// command family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Command {
    /// File → New Project.
    NewProject,
    /// File → Open Project.
    OpenProject,
    /// File → Recent → the project at this index.
    OpenRecent(usize),
    /// File → Save.
    Save,
    /// File → Save As.
    SaveAs,
    /// File → Save All.
    SaveAll,
    /// File → Make `<Project>`.exe.
    MakeExe,
    /// File → Close Project.
    CloseProject,
    /// File → Exit.
    Exit,
    /// Edit → Undo.
    Undo,
    /// Edit → Redo.
    Redo,
    /// Edit → Cut.
    Cut,
    /// Edit → Copy.
    Copy,
    /// Edit → Paste.
    Paste,
    /// Edit → Delete.
    Delete,
    /// Edit → Select All.
    SelectAll,
    /// Edit → Find.
    Find,
    /// Edit → Find Next.
    FindNext,
    /// Edit → Replace.
    Replace,
    /// Edit → Go To Line.
    GoToLine,
    /// View → Code.
    ViewCode,
    /// View → Object.
    ViewObject,
    /// View → Project.
    ViewProject,
    /// View → Properties.
    ViewProperties,
    /// View → Toolbox.
    ViewToolbox,
    /// View → Output.
    ViewOutput,
    /// Project → Add Form.
    AddForm,
    /// Project → Add Module.
    AddModule,
    /// Project → Remove.
    Remove,
    /// Project → Properties.
    ProjectProperties,
    /// Run → Start.
    RunStart,
    /// Run → End.
    RunEnd,
    /// View → Theme → Light.
    ThemeLight,
    /// View → Theme → Dark.
    ThemeDark,
    /// View → Theme → System.
    ThemeSystem,
}

/// A modifier + key chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    /// Ctrl must be held.
    pub ctrl: bool,
    /// Shift must be held.
    pub shift: bool,
    /// Alt must be held.
    pub alt: bool,
    /// The virtual key.
    pub key: Key,
}

impl Shortcut {
    /// A shortcut with only the given modifiers set.
    const fn new(ctrl: bool, shift: bool, alt: bool, key: Key) -> Shortcut {
        Shortcut {
            ctrl,
            shift,
            alt,
            key,
        }
    }

    /// Whether a key-down event matches this chord.
    fn matches(self, key: Key, modifiers: Modifiers) -> bool {
        self.key == key
            && self.ctrl == modifiers.ctrl
            && self.shift == modifiers.shift
            && self.alt == modifiers.alt
    }

    /// The chord's display text, e.g. `Ctrl+Shift+S`.
    pub fn text(self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        parts.push(key_name(self.key));
        parts.join("+")
    }
}

/// A human-readable name for the keys the IDE binds.
fn key_name(key: Key) -> &'static str {
    match key {
        Key::F3 => "F3",
        Key::F5 => "F5",
        Key::F7 => "F7",
        Key::S => "S",
        Key::N => "N",
        Key::O => "O",
        Key::F => "F",
        Key::G => "G",
        Key::H => "H",
        _ => "?",
    }
}

impl Command {
    /// Every command without a payload, in menu order.
    ///
    /// Used to seed the dispatcher's enabled table and to look up shortcuts.
    /// `OpenRecent` is omitted because its index is data, not a distinct
    /// action; [`Command::OpenRecent`] is enabled with the rest of the recent
    /// list.
    pub const ALL: &'static [Command] = &[
        Command::NewProject,
        Command::OpenProject,
        Command::Save,
        Command::SaveAs,
        Command::SaveAll,
        Command::MakeExe,
        Command::CloseProject,
        Command::Exit,
        Command::Undo,
        Command::Redo,
        Command::Cut,
        Command::Copy,
        Command::Paste,
        Command::Delete,
        Command::SelectAll,
        Command::Find,
        Command::FindNext,
        Command::Replace,
        Command::GoToLine,
        Command::ViewCode,
        Command::ViewObject,
        Command::ViewProject,
        Command::ViewProperties,
        Command::ViewToolbox,
        Command::ViewOutput,
        Command::AddForm,
        Command::AddModule,
        Command::Remove,
        Command::ProjectProperties,
        Command::RunStart,
        Command::RunEnd,
        Command::ThemeLight,
        Command::ThemeDark,
        Command::ThemeSystem,
    ];

    /// The command's menu label, without an access key marker.
    pub fn label(self) -> String {
        match self {
            Command::NewProject => "New Project".to_string(),
            Command::OpenProject => "Open Project".to_string(),
            Command::OpenRecent(index) => format!("Recent {index}"),
            Command::Save => "Save".to_string(),
            Command::SaveAs => "Save As".to_string(),
            Command::SaveAll => "Save All".to_string(),
            Command::MakeExe => "Make EXE".to_string(),
            Command::CloseProject => "Close Project".to_string(),
            Command::Exit => "Exit".to_string(),
            Command::Undo => "Undo".to_string(),
            Command::Redo => "Redo".to_string(),
            Command::Cut => "Cut".to_string(),
            Command::Copy => "Copy".to_string(),
            Command::Paste => "Paste".to_string(),
            Command::Delete => "Delete".to_string(),
            Command::SelectAll => "Select All".to_string(),
            Command::Find => "Find".to_string(),
            Command::FindNext => "Find Next".to_string(),
            Command::Replace => "Replace".to_string(),
            Command::GoToLine => "Go To Line".to_string(),
            Command::ViewCode => "Code".to_string(),
            Command::ViewObject => "Object".to_string(),
            Command::ViewProject => "Project".to_string(),
            Command::ViewProperties => "Properties".to_string(),
            Command::ViewToolbox => "Toolbox".to_string(),
            Command::ViewOutput => "Output".to_string(),
            Command::AddForm => "Add Form".to_string(),
            Command::AddModule => "Add Module".to_string(),
            Command::Remove => "Remove".to_string(),
            Command::ProjectProperties => "Properties".to_string(),
            Command::RunStart => "Start".to_string(),
            Command::RunEnd => "End".to_string(),
            Command::ThemeLight => "Light".to_string(),
            Command::ThemeDark => "Dark".to_string(),
            Command::ThemeSystem => "System".to_string(),
        }
    }

    /// The command's keyboard shortcut, if it has one.
    pub fn shortcut(self) -> Option<Shortcut> {
        match self {
            Command::Save => Some(Shortcut::new(true, false, false, Key::S)),
            Command::SaveAll => Some(Shortcut::new(true, true, false, Key::S)),
            Command::NewProject => Some(Shortcut::new(true, false, false, Key::N)),
            Command::OpenProject => Some(Shortcut::new(true, false, false, Key::O)),
            Command::RunStart => Some(Shortcut::new(false, false, false, Key::F5)),
            Command::ViewCode => Some(Shortcut::new(false, false, false, Key::F7)),
            Command::ViewObject => Some(Shortcut::new(false, true, false, Key::F7)),
            Command::Find => Some(Shortcut::new(true, false, false, Key::F)),
            Command::FindNext => Some(Shortcut::new(false, false, false, Key::F3)),
            Command::Replace => Some(Shortcut::new(true, false, false, Key::H)),
            Command::GoToLine => Some(Shortcut::new(true, false, false, Key::G)),
            _ => None,
        }
    }

    /// The shortcut's display text, e.g. `Ctrl+S`.
    ///
    /// xui menus cannot render this yet (gap G11), but the dispatcher and the
    /// eventual command palette use it.
    pub fn shortcut_text(self) -> Option<String> {
        self.shortcut().map(Shortcut::text)
    }

    /// Whether the command needs an open project to make sense.
    ///
    /// The dispatcher disables these until a project is opened, so the menu
    /// and toolbar reflect what is actually available.
    pub fn requires_project(self) -> bool {
        matches!(
            self,
            Command::Save
                | Command::SaveAs
                | Command::SaveAll
                | Command::MakeExe
                | Command::CloseProject
                | Command::Undo
                | Command::Redo
                | Command::Cut
                | Command::Copy
                | Command::Paste
                | Command::Delete
                | Command::SelectAll
                | Command::Find
                | Command::FindNext
                | Command::Replace
                | Command::GoToLine
                | Command::ViewCode
                | Command::ViewObject
                | Command::AddForm
                | Command::AddModule
                | Command::Remove
                | Command::ProjectProperties
                | Command::RunStart
                | Command::RunEnd
        )
    }

    /// Whether the command changes design-time code or the project, so it must
    /// be refused while a program is running (issue #16).
    pub fn is_editing(self) -> bool {
        matches!(
            self,
            Command::Undo
                | Command::Redo
                | Command::Cut
                | Command::Paste
                | Command::Delete
                | Command::Replace
                | Command::AddForm
                | Command::AddModule
                | Command::Remove
                | Command::ProjectProperties
        )
    }

    /// The command a key-down event maps to, if any.
    ///
    /// Only plain Ctrl/Shift chords are bound; Alt combinations are left to the
    /// menu's own mnemonic handling, and a Windows-key chord is the OS's.
    pub fn from_keydown(key: Key, modifiers: Modifiers) -> Option<Command> {
        if modifiers.alt || modifiers.win {
            return None;
        }
        Command::ALL.iter().copied().find(|command| {
            command
                .shortcut()
                .is_some_and(|shortcut| shortcut.matches(key, modifiers))
        })
    }
}

/// Owns every command's enabled state.
///
/// A disabled command is greyed out in the menu, and [`Dispatcher::dispatch`]
/// refuses to run it. Every command starts enabled: at M0 there is no project
/// loader yet, so gating by project is exercised through
/// [`Dispatcher::set_project_open`] rather than applied at startup.
pub struct Dispatcher {
    enabled: HashMap<Command, bool>,
    project_open: bool,
    running: bool,
}

impl Dispatcher {
    /// A dispatcher with every command enabled.
    pub fn new() -> Dispatcher {
        let mut dispatcher = Dispatcher {
            enabled: HashMap::new(),
            project_open: true,
            running: false,
        };
        for &command in Command::ALL {
            dispatcher.enabled.insert(command, true);
        }
        dispatcher.recompute();
        dispatcher
    }

    /// Whether a command may run.
    ///
    /// A recent-project entry is enabled whenever the recent list is non-empty;
    /// the menu builder controls that by only adding entries it has, so the
    /// dispatcher treats every `OpenRecent` as available.
    pub fn is_enabled(&self, command: Command) -> bool {
        if matches!(command, Command::OpenRecent(_)) {
            return true;
        }
        self.enabled.get(&command).copied().unwrap_or(false)
    }

    /// Sets a command's enabled state explicitly.
    pub fn set_enabled(&mut self, command: Command, enabled: bool) {
        self.enabled.insert(command, enabled);
    }

    /// Whether a project is open.
    pub fn project_open(&self) -> bool {
        self.project_open
    }

    /// Whether a program is currently running (issue #16).
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Records whether a project is open; project-scoped commands follow.
    pub fn set_project_open(&mut self, open: bool) {
        self.project_open = open;
        self.recompute();
    }

    /// Records whether a program is running: Start is offered only while idle
    /// and End only while running (issue #16).
    pub fn set_running(&mut self, running: bool) {
        self.running = running;
        self.recompute();
    }

    /// Re-derives every command's enabled state from the project and run state.
    ///
    /// Run's two commands are special: a project-scoped command follows the
    /// project, while Start/End also follow whether a program is running.
    fn recompute(&mut self) {
        for &command in Command::ALL {
            if command.requires_project() {
                self.enabled.insert(command, self.project_open);
            }
        }
        self.enabled
            .insert(Command::RunStart, self.project_open && !self.running);
        self.enabled
            .insert(Command::RunEnd, self.project_open && self.running);
    }

    /// Runs `command`'s M0 behaviour: log it. The commands that already have
    /// an implementation are handled by the caller, which owns the `Ui`.
    ///
    /// Returns the line the caller should append to the output pane, or `None`
    /// when the command is disabled.
    pub fn dispatch(&self, command: Command) -> Option<String> {
        if !self.is_enabled(command) {
            return None;
        }
        let text = match command.shortcut_text() {
            Some(shortcut) => format!("{} ({shortcut})", command.label()),
            None => command.label(),
        };
        Some(format!("Command: {text}"))
    }
}

impl Default for Dispatcher {
    fn default() -> Self {
        Dispatcher::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        }
    }

    #[test]
    fn a_key_chord_maps_to_its_command() {
        assert_eq!(Command::from_keydown(Key::S, ctrl()), Some(Command::Save));
        assert_eq!(
            Command::from_keydown(
                Key::S,
                Modifiers {
                    ctrl: true,
                    shift: true,
                    ..Modifiers::NONE
                }
            ),
            Some(Command::SaveAll)
        );
        assert_eq!(
            Command::from_keydown(Key::N, ctrl()),
            Some(Command::NewProject)
        );
        assert_eq!(
            Command::from_keydown(Key::O, ctrl()),
            Some(Command::OpenProject)
        );
        assert_eq!(
            Command::from_keydown(Key::F5, Modifiers::NONE),
            Some(Command::RunStart)
        );
        assert_eq!(
            Command::from_keydown(Key::F7, Modifiers::NONE),
            Some(Command::ViewCode)
        );
        assert_eq!(
            Command::from_keydown(
                Key::F7,
                Modifiers {
                    shift: true,
                    ..Modifiers::NONE
                }
            ),
            Some(Command::ViewObject)
        );
        assert_eq!(Command::from_keydown(Key::F, ctrl()), Some(Command::Find));
    }

    #[test]
    fn the_editing_find_shortcuts_map_to_their_commands() {
        assert_eq!(
            Command::from_keydown(Key::F3, Modifiers::NONE),
            Some(Command::FindNext)
        );
        assert_eq!(
            Command::from_keydown(Key::G, ctrl()),
            Some(Command::GoToLine)
        );
        assert_eq!(
            Command::from_keydown(Key::H, ctrl()),
            Some(Command::Replace)
        );
        assert_eq!(Command::GoToLine.shortcut_text().as_deref(), Some("Ctrl+G"));
        assert_eq!(Command::FindNext.shortcut_text().as_deref(), Some("F3"));
    }

    #[test]
    fn an_unbound_chord_maps_to_nothing() {
        assert_eq!(Command::from_keydown(Key::S, Modifiers::NONE), None);
        assert_eq!(Command::from_keydown(Key::F5, ctrl()), None);
        assert_eq!(
            Command::from_keydown(
                Key::F5,
                Modifiers {
                    alt: true,
                    ..Modifiers::NONE
                }
            ),
            None,
            "Alt chords are left to mnemonic handling"
        );
    }

    #[test]
    fn a_shortcut_reads_as_its_text() {
        assert_eq!(Command::Save.shortcut_text().as_deref(), Some("Ctrl+S"));
        assert_eq!(
            Command::SaveAll.shortcut_text().as_deref(),
            Some("Ctrl+Shift+S")
        );
        assert_eq!(
            Command::ViewObject.shortcut_text().as_deref(),
            Some("Shift+F7")
        );
        assert_eq!(Command::Copy.shortcut_text(), None);
    }

    #[test]
    fn project_commands_follow_the_project_state() {
        let mut dispatcher = Dispatcher::new();
        assert!(dispatcher.is_enabled(Command::NewProject));
        assert!(dispatcher.is_enabled(Command::Save));
        assert_eq!(
            dispatcher.dispatch(Command::Save).as_deref(),
            Some("Command: Save (Ctrl+S)")
        );

        dispatcher.set_project_open(false);
        assert!(!dispatcher.is_enabled(Command::Save));
        assert!(!dispatcher.is_enabled(Command::RunStart));
        assert_eq!(dispatcher.dispatch(Command::Save), None);
        assert!(dispatcher.is_enabled(Command::NewProject));

        dispatcher.set_project_open(true);
        assert!(dispatcher.is_enabled(Command::Save));
    }

    #[test]
    fn run_start_and_end_follow_the_running_state() {
        let mut dispatcher = Dispatcher::new();
        assert!(dispatcher.is_enabled(Command::RunStart));
        assert!(
            !dispatcher.is_enabled(Command::RunEnd),
            "End is for a running program only"
        );

        dispatcher.set_running(true);
        assert!(!dispatcher.is_enabled(Command::RunStart));
        assert!(dispatcher.is_enabled(Command::RunEnd));

        dispatcher.set_running(false);
        assert!(dispatcher.is_enabled(Command::RunStart));
        assert!(!dispatcher.is_enabled(Command::RunEnd));

        dispatcher.set_running(true);
        dispatcher.set_project_open(false);
        assert!(
            !dispatcher.is_enabled(Command::RunEnd),
            "closing the project ends the run"
        );
    }

    #[test]
    fn mutating_commands_are_refused_while_running() {
        assert!(Command::AddForm.is_editing());
        assert!(Command::Remove.is_editing());
        assert!(Command::Paste.is_editing());
        assert!(!Command::RunEnd.is_editing());
        assert!(!Command::Save.is_editing());
        assert!(!Command::ViewCode.is_editing());
    }

    #[test]
    fn every_shortcut_is_unique() {
        let mut seen: Vec<(Key, bool, bool, bool)> = Vec::new();
        for &command in Command::ALL {
            if let Some(shortcut) = command.shortcut() {
                let chord = (shortcut.key, shortcut.ctrl, shortcut.shift, shortcut.alt);
                assert!(
                    !seen.contains(&chord),
                    "two commands share the chord {chord:?}"
                );
                seen.push(chord);
            }
        }
    }
}
