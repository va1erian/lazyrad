//! Shortcut behaviour driven straight through the app (issue #69): the Edit
//! menu's availability, the guards on window-level shortcuts, and xui's own
//! text box under the shortcut layer.
//!
//! Like `shortcut_tests`, no window is opened and every clipboard is an
//! in-process one.

use std::cell::RefCell;
use std::rc::Rc;

use lazyrad_designer::{DesignerMsg, Tool, ToolboxMsg};
use xui_canvas::OffscreenBackend;
use xui_code_editor::platform::InProcessClipboard;
use xui_core::app::{App, Ui, run_app};
use xui_core::arrange::{Handle, LayoutExt, absolute, edit};
use xui_core::backend::{Backend, Event, WindowId};
use xui_core::message::Key;
use xui_core::widget::{Edit, HasText};

use super::shortcut_tests::{Chord, unique_dir};
use super::{DocKind, IdeApp, Msg, default_platform_spec, shortcut_message};
use crate::command::Command;
use crate::edit_state::EditAvailability;
use crate::project::{DEFAULT_FORM, ProjectSession};
use crate::settings::Settings;
use crate::shortcut_backend::ShortcutBackend;

// ---- Availability and the shortcut guard, driven straight through the app ----

/// Runs `check` inside the app's build closure, with a project holding the
/// startup form, its code and designer open.
fn in_app(check: impl FnOnce(&mut IdeApp, &mut Ui<Msg>) + 'static) {
    let dir = unique_dir("app");
    let _ = std::fs::remove_dir_all(&dir);
    let cleanup = dir.clone();
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    run_app(backend, default_platform_spec(), move |ui| {
        let mut app = IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
        let session = ProjectSession::create("shortcuts", &dir).expect("create");
        app.session = Some(session);
        app.dispatcher.set_project_open(true);
        app.refresh_explorer(ui);
        check(&mut app, ui);
        app
    })
    .expect("the IDE runs to completion");
    let _ = std::fs::remove_dir_all(&cleanup);
}

#[test]
fn edit_availability_follows_the_active_document() {
    in_app(|app, ui| {
        // The Start Page is in front: no document to act on.
        assert_eq!(app.edit_availability(), EditAvailability::NONE);

        // A code tab: text to copy, nothing to undo, an empty clipboard.
        let form = DEFAULT_FORM.to_owned();
        app.open_document(&form, DocKind::Code).expect("code opens");
        let editor = app.code_editor(&form).expect("the editor");
        editor.set_clipboard(InProcessClipboard);
        xui_code_editor::Clipboard::set_text(&InProcessClipboard, "");
        let code = app.edit_availability();
        assert!(code.copy && code.cut && code.select_all);
        assert!(!code.undo && !code.redo && !code.paste && !code.delete);

        // An edit enables Undo; undoing it enables Redo and disables Undo.
        editor.insert_text("x");
        assert!(app.edit_availability().undo);
        editor.undo();
        let undone = app.edit_availability();
        assert!(!undone.undo && undone.redo);

        // A selection enables Delete; a filled clipboard enables Paste.
        editor.select_all();
        xui_code_editor::Clipboard::set_text(&InProcessClipboard, "text");
        let selected = app.edit_availability();
        assert!(selected.delete && selected.paste);

        // The designer tab answers for controls instead.
        app.open_document(&form, DocKind::Designer)
            .expect("designer");
        let empty = app.edit_availability();
        assert!(!empty.copy && !empty.cut && !empty.delete && !empty.select_all);
        assert!(!empty.undo && !empty.paste);
        app.update(
            Msg::Toolbox(ToolboxMsg::Select(Tool::control("Button"))),
            ui,
        );
        for msg in [
            DesignerMsg::PointerDown {
                x: 16,
                y: 16,
                ctrl: false,
            },
            DesignerMsg::PointerMove {
                x: 116,
                y: 40,
                ctrl: false,
            },
            DesignerMsg::PointerUp {
                x: 116,
                y: 40,
                ctrl: false,
            },
        ] {
            app.update(
                Msg::Designer {
                    document: form.clone(),
                    msg,
                },
                ui,
            );
        }
        let drawn = app.edit_availability();
        assert!(drawn.undo && drawn.select_all && drawn.copy && drawn.cut && drawn.delete);
        assert!(!drawn.paste, "nothing was copied yet");
        app.dispatch_edit(Command::Copy, ui);
        assert!(app.edit_availability().paste);

        // Going back to the code tab switches the answers back.
        app.open_document(&form, DocKind::Code).expect("code again");
        assert_eq!(app.edit_availability().cut, code.cut);
        assert!(!app.edit_availability().copy || app.edit_availability().select_all);
    });
}

#[test]
fn the_menu_greys_out_what_the_document_cannot_do() {
    in_app(|app, ui| {
        let form = DEFAULT_FORM.to_owned();
        app.open_document(&form, DocKind::Code).expect("code opens");
        let editor = app.code_editor(&form).expect("the editor");
        editor.set_clipboard(InProcessClipboard);
        xui_code_editor::Clipboard::set_text(&InProcessClipboard, "");

        app.update(Msg::RefreshEdit, ui);
        let enabled = |app: &IdeApp, command: Command| {
            app.menu_commands
                .iter()
                .find(|(_, entry)| *entry == command)
                .is_some_and(|(id, _)| app.menu.is_enabled(*id))
        };
        assert!(!enabled(app, Command::Paste), "an empty clipboard");
        assert!(!enabled(app, Command::Undo), "nothing to undo");
        assert!(enabled(app, Command::Copy));
        assert!(
            enabled(app, Command::Save),
            "non-edit entries are untouched"
        );

        xui_code_editor::Clipboard::set_text(&InProcessClipboard, "x");
        editor.insert_text("y");
        app.update(Msg::RefreshEdit, ui);
        assert!(enabled(app, Command::Paste));
        assert!(enabled(app, Command::Undo));

        // Closing to the Start Page leaves nothing to act on.
        if let Some(docs) = &app.docs {
            docs.select(0);
        }
        app.update(Msg::TabChanged(0), ui);
        assert!(!enabled(app, Command::Copy));
        assert!(enabled(app, Command::Save));
    });
}

#[test]
fn a_shortcut_is_dropped_while_disabled_and_run_end_needs_a_run() {
    in_app(|app, ui| {
        let form = DEFAULT_FORM.to_owned();
        app.open_document(&form, DocKind::Code).expect("code opens");
        let editor = app.code_editor(&form).expect("the editor");
        editor.insert_text("// dirty\n");
        app.after_programmatic_edit(&form, ui);
        assert!(app.project_dirty());

        // Shift+F5 with nothing running is a disabled command: dropped.
        app.update(Msg::Shortcut(Command::RunEnd), ui);
        assert!(!app.is_running());

        // A shortcut with the project closed is dropped too.
        app.dispatcher.set_project_open(false);
        app.update(Msg::Shortcut(Command::Save), ui);
        assert!(app.project_dirty(), "Save was disabled");
        app.dispatcher.set_project_open(true);
        app.update(Msg::Shortcut(Command::Save), ui);
        assert!(!app.project_dirty(), "Save ran once it was enabled");
    });
}

#[test]
fn a_shortcut_is_dropped_while_a_prompt_is_open() {
    in_app(|app, ui| {
        let form = DEFAULT_FORM.to_owned();
        app.open_document(&form, DocKind::Code).expect("code opens");
        let editor = app.code_editor(&form).expect("the editor");
        editor.insert_text("// dirty\n");
        app.after_programmatic_edit(&form, ui);

        app.show_prompt(ui, super::PromptKind::Find);
        assert!(app.prompt.is_some(), "the prompt is open");
        app.update(Msg::Shortcut(Command::Save), ui);
        assert!(
            app.project_dirty(),
            "Ctrl+S typed into the prompt is ignored"
        );

        app.prompt = None;
        app.update(Msg::Shortcut(Command::Save), ui);
        assert!(!app.project_dirty());
    });
}

#[test]
fn edit_commands_leave_the_document_clean_unless_they_change_text() {
    in_app(|app, ui| {
        let form = DEFAULT_FORM.to_owned();
        app.open_document(&form, DocKind::Code).expect("code opens");
        let editor = app.code_editor(&form).expect("the editor");
        editor.set_clipboard(InProcessClipboard);
        xui_code_editor::Clipboard::set_text(&InProcessClipboard, "");
        for command in [
            Command::Undo,
            Command::Redo,
            Command::Paste,
            Command::Delete,
            Command::Copy,
            Command::SelectAll,
        ] {
            app.dispatch_edit(command, ui);
        }
        assert!(
            !app.documents.iter().any(|document| document.dirty),
            "commands that changed nothing must not dirty the tab"
        );

        // A real edit, then its undo, is a change each time.
        editor.insert_text("z");
        app.after_programmatic_edit(&form, ui);
        assert!(app.documents.iter().any(|document| document.dirty));
    });
}

// ---- xui's Edit: text boxes handle the chords themselves ----

/// The window and text box the app hands to the scenario.
type Built = (WindowId, Rc<Edit<Msg>>);

/// An app that only records the messages it is sent.
struct Recorder {
    seen: Rc<RefCell<Vec<Msg>>>,
}

impl App for Recorder {
    type Msg = Msg;

    fn update(&mut self, msg: Msg, _ui: &mut Ui<Msg>) {
        self.seen.borrow_mut().push(msg);
    }
}

#[test]
fn a_text_box_keeps_the_edit_chords_and_the_window_keeps_the_file_chords() {
    let offscreen = Rc::new(OffscreenBackend::new());
    let shortcuts = Rc::new(ShortcutBackend::new(
        offscreen.clone() as Rc<dyn Backend>,
        shortcut_message,
    ));
    let proxy_cell = shortcuts.proxy_cell();
    let backend: Rc<dyn Backend> = shortcuts;
    let seen: Rc<RefCell<Vec<Msg>>> = Rc::new(RefCell::new(Vec::new()));
    let seen_for_app = Rc::clone(&seen);
    let seen_for_hook = Rc::clone(&seen);
    let hook_offscreen = Rc::clone(&offscreen);
    let slot: Rc<RefCell<Option<Built>>> = Rc::new(RefCell::new(None));
    let slot_for_app = Rc::clone(&slot);
    offscreen.set_run_hook(move || {
        let (window, edit) = slot.borrow_mut().take().expect("built");
        // The window system tells the focused box it has the focus.
        hook_offscreen.inject(window, Event::SetFocus);
        let press = |chord: Chord| {
            hook_offscreen.inject(window, chord.event());
            hook_offscreen.pump(window);
        };
        // Select all, copy, paste over the selection, then paste again: the
        // box holds the clipboard text once, i.e. each chord ran once.
        press(Chord::ctrl(Key::A));
        press(Chord::ctrl(Key::C));
        press(Chord::ctrl(Key::V));
        assert_eq!(edit.text(), "hello", "copy then paste over the selection");
        press(Chord::ctrl(Key::A));
        press(Chord::ctrl(Key::X));
        assert_eq!(edit.text(), "", "cut emptied the selected box");
        press(Chord::ctrl(Key::Z));
        assert_eq!(edit.text(), "hello", "undo restored the text");
        assert!(
            seen_for_hook.borrow().is_empty(),
            "no Edit chord became an IDE message"
        );

        // File chords are intercepted even while the text box has focus, and
        // the box does not see them.
        for chord in [
            Chord::ctrl(Key::S),
            Chord::ctrl_shift(Key::S),
            Chord::ctrl(Key::N),
            Chord::ctrl(Key::O),
            Chord::plain(Key::F5),
            Chord::shift(Key::F5),
        ] {
            press(chord);
        }
        assert_eq!(
            *seen_for_hook.borrow(),
            vec![
                Msg::Shortcut(Command::Save),
                Msg::Shortcut(Command::SaveAll),
                Msg::Shortcut(Command::NewProject),
                Msg::Shortcut(Command::OpenProject),
                Msg::Shortcut(Command::RunStart),
                Msg::Shortcut(Command::RunEnd),
            ]
        );
        assert_eq!(edit.text(), "hello", "the box ignored them");
    });
    run_app(backend, default_platform_spec(), move |ui| {
        *proxy_cell.borrow_mut() = Some(ui.proxy());
        let field = Handle::new();
        ui.root(absolute().child(edit().text("hello").bind(&field).at(0, 0, 200, 28)))
            .expect("the edit builds");
        let edit = field.get();
        edit.focus();
        *slot_for_app.borrow_mut() = Some((ui.window(), edit));
        Recorder { seen: seen_for_app }
    })
    .expect("runs");
}

// ---- Code completion sees the project ----

#[test]
fn completion_offers_a_control_drawn_after_the_code_window_opened() {
    in_app(|app, ui| {
        let form = DEFAULT_FORM.to_owned();
        app.open_document(&form, DocKind::Code).expect("code opens");
        let editor = app.code_editor(&form).expect("the editor");
        editor.set_text("fn f() {\n    button1.tex");
        editor.set_caret(editor.text().chars().count());
        assert!(
            !editor.trigger_completion(),
            "no button yet, so nothing to complete"
        );

        // Draw a button on the form; the message refreshes the context.
        app.open_document(&form, DocKind::Designer)
            .expect("designer");
        app.update(
            Msg::Toolbox(ToolboxMsg::Select(Tool::control("Button"))),
            ui,
        );
        for msg in [
            DesignerMsg::PointerDown {
                x: 16,
                y: 16,
                ctrl: false,
            },
            DesignerMsg::PointerUp {
                x: 116,
                y: 40,
                ctrl: false,
            },
        ] {
            app.update(
                Msg::Designer {
                    document: form.clone(),
                    msg,
                },
                ui,
            );
        }
        assert!(editor.trigger_completion(), "button1.text is offered");
        editor.close_completion();
        assert!(!editor.is_completing());
    });
}
