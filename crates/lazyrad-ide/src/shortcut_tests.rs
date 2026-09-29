//! Keyboard shortcuts across the IDE (issue #69).
//!
//! Real `KeyDown` events, modifiers included, go through an offscreen backend
//! wrapped in the same [`ShortcutBackend`] the IDE runs with, so they reach the
//! focused node exactly as a window system would deliver them. The tests count
//! what each chord did: file chords must raise one [`Msg::Shortcut`] wherever
//! the focus is, and Edit chords must raise none and reach the focused widget
//! once. No window is opened, and every clipboard is an in-process one.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lazyrad_designer::{Designer, DesignerMsg, Tool, ToolboxMsg};
use xui_canvas::OffscreenBackend;
use xui_code_editor::Editor;
use xui_code_editor::platform::InProcessClipboard;
use xui_core::app::{App, Proxy, run_app};
use xui_core::backend::{Backend, Event, WidgetId, WindowId};
use xui_core::message::{Key, Modifiers};

use super::exit_criterion_tests::{RecordingLauncher, player_placeholder};
use super::{DocKind, IdeApp, Msg, default_platform_spec, shortcut_message};
use crate::command::Command;
use crate::project::{DEFAULT_FORM, ProjectSession};
use crate::settings::Settings;
use crate::shortcut_backend::ShortcutBackend;

/// A chord: which modifiers are held with the key.
#[derive(Clone, Copy)]
pub(super) struct Chord {
    pub(super) key: Key,
    ctrl: bool,
    shift: bool,
}

impl Chord {
    pub(super) const fn ctrl(key: Key) -> Chord {
        Chord {
            key,
            ctrl: true,
            shift: false,
        }
    }

    pub(super) const fn plain(key: Key) -> Chord {
        Chord {
            key,
            ctrl: false,
            shift: false,
        }
    }

    pub(super) const fn shift(key: Key) -> Chord {
        Chord {
            key,
            ctrl: false,
            shift: true,
        }
    }

    pub(super) const fn ctrl_shift(key: Key) -> Chord {
        Chord {
            key,
            ctrl: true,
            shift: true,
        }
    }

    pub(super) fn event(self) -> Event {
        Event::KeyDown {
            key: self.key,
            modifiers: Modifiers {
                ctrl: self.ctrl,
                shift: self.shift,
                ..Modifiers::NONE
            },
            repeat: 1,
            system: false,
        }
    }
}

/// The running IDE as the scenarios see it: the backend to inject into, the
/// widgets to focus and read, and what the shortcut layer intercepted.
struct Ide {
    offscreen: Rc<OffscreenBackend>,
    window: WindowId,
    proxy: Proxy<Msg>,
    /// Every command the shortcut layer turned into a message, in order.
    seen: Rc<RefCell<Vec<Command>>>,
    editor: Rc<Editor<Msg>>,
    designer: Rc<RefCell<Designer<Msg>>>,
    tree: WidgetId,
    launched: Rc<RecordingLauncher>,
    dir: PathBuf,
}

impl Ide {
    /// Sends a chord to the focused node and lets queued messages run.
    fn press(&self, chord: Chord) {
        self.offscreen.inject(self.window, chord.event());
        self.offscreen.pump(self.window);
    }

    /// Types `text` into the focused node.
    fn type_text(&self, text: &str) {
        for character in text.chars() {
            self.offscreen.inject(self.window, Event::Char(character));
        }
        self.offscreen.pump(self.window);
    }

    /// Moves the keyboard focus the way a window system does: the old node is
    /// told it lost it, then the new one that it gained it.
    fn focus(&self, id: WidgetId) {
        move_focus(&self.offscreen, self.window, id);
    }

    /// The saved `.rhai` on disk.
    fn saved_code(&self) -> String {
        std::fs::read_to_string(self.dir.join(format!("{DEFAULT_FORM}.rhai"))).unwrap_or_default()
    }

    fn node_count(&self) -> usize {
        self.designer.borrow().doc().nodes.len()
    }

    fn seen(&self) -> Vec<Command> {
        self.seen.borrow().clone()
    }
}

/// Focuses `id`, delivering `KillFocus` to the previous holder and `SetFocus`
/// to the new one, which the offscreen backend's bare `focus` does not.
fn move_focus(offscreen: &OffscreenBackend, window: WindowId, id: WidgetId) {
    offscreen.inject(window, Event::KillFocus);
    offscreen.focus(id);
    offscreen.inject(window, Event::SetFocus);
}

/// A directory no other test shares, under the temp folder.
pub(super) fn unique_dir(purpose: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "lazyrad-shortcuts-{}-{purpose}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Which document tab is in front while the scenario runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Front {
    Code,
    Designer,
}

/// Builds the IDE with a project holding the startup form open as code and as
/// a designer (with one button drawn and selected), then runs `scenario` once
/// the app is live.
fn with_ide(front: Front, scenario: impl FnOnce(&Ide) + 'static) {
    let dir = unique_dir("ide");
    let _ = std::fs::remove_dir_all(&dir);
    let cleanup = dir.clone();

    let offscreen = Rc::new(OffscreenBackend::new());
    let seen: Rc<RefCell<Vec<Command>>> = Rc::new(RefCell::new(Vec::new()));
    let seen_for_map = Rc::clone(&seen);
    let shortcuts = Rc::new(ShortcutBackend::new(
        offscreen.clone() as Rc<dyn Backend>,
        move |event: &Event| {
            let message = shortcut_message(event);
            if let Some(Msg::Shortcut(command)) = &message {
                seen_for_map.borrow_mut().push(*command);
            }
            message
        },
    ));
    let proxy_cell = shortcuts.proxy_cell();
    let backend: Rc<dyn Backend> = shortcuts;

    let launcher = Rc::new(RecordingLauncher::default());
    let slot: Rc<RefCell<Option<Ide>>> = Rc::new(RefCell::new(None));
    let slot_for_app = Rc::clone(&slot);
    let hook_slot = Rc::clone(&slot);
    offscreen.set_run_hook(move || {
        let ide = hook_slot.borrow_mut().take().expect("the app was built");
        scenario(&ide);
    });
    let offscreen_for_app = Rc::clone(&offscreen);

    run_app(backend, default_platform_spec(), move |ui| {
        *proxy_cell.borrow_mut() = Some(ui.proxy());
        let mut app = IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
        let session = ProjectSession::create("shortcuts", &dir).expect("create");
        app.session = Some(session);
        app.dispatcher.set_project_open(true);
        app.refresh_explorer(ui);
        app.settings.player_path = Some(player_placeholder(&dir));
        app.launcher = Rc::clone(&launcher) as Rc<dyn crate::run::Launcher>;

        let form = DEFAULT_FORM.to_owned();
        app.open_document(&form, DocKind::Code).expect("code opens");
        app.open_document(&form, DocKind::Designer)
            .expect("the designer opens");
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
        let (_, designer, _) = app.active_designer().expect("the designer is in front");
        designer.borrow().select_node("button1", ui);
        let editor = app.code_editor(&form).expect("the code tab is open");
        editor.set_clipboard(InProcessClipboard);
        if front == Front::Code {
            app.open_document(&form, DocKind::Code).expect("selects");
        }
        *slot_for_app.borrow_mut() = Some(Ide {
            offscreen: offscreen_for_app,
            window: ui.window(),
            proxy: ui.proxy(),
            seen,
            editor,
            designer,
            tree: app.tree.id(),
            launched: Rc::clone(&launcher),
            dir: dir.clone(),
        });
        app
    })
    .expect("the IDE runs to completion");

    let _ = std::fs::remove_dir_all(&cleanup);
}

/// The focus targets a file chord must work from.
fn focus_targets(ide: &Ide) -> Vec<(&'static str, WidgetId)> {
    vec![
        ("code editor", ide.editor.id()),
        ("designer", ide.designer.borrow().id()),
        ("explorer", ide.tree),
    ]
}

#[test]
fn file_chords_work_wherever_the_focus_is() {
    with_ide(Front::Code, |ide| {
        for (round, (target, id)) in focus_targets(ide).into_iter().enumerate() {
            // Dirty the document by typing, then move the focus to the target.
            ide.focus(ide.editor.id());
            ide.type_text(&format!("// {target}\n"));
            ide.focus(id);

            ide.press(Chord::ctrl(Key::S));
            assert!(
                ide.saved_code().contains(&format!("// {target}")),
                "Ctrl+S saved from the {target}"
            );

            ide.focus(ide.editor.id());
            ide.type_text(&format!("// all {target}\n"));
            ide.focus(id);
            ide.press(Chord::ctrl_shift(Key::S));
            assert!(
                ide.saved_code().contains(&format!("// all {target}")),
                "Ctrl+Shift+S saved from the {target}"
            );

            ide.press(Chord::plain(Key::F5));
            assert_eq!(
                ide.launched.launched.borrow().len(),
                round + 1,
                "F5 started one run from the {target}"
            );
            ide.press(Chord::shift(Key::F5));
            // The run ended, so the next round's F5 may start again.
        }
        let expected: Vec<Command> = std::iter::repeat_n(
            [
                Command::Save,
                Command::SaveAll,
                Command::RunStart,
                Command::RunEnd,
            ],
            3,
        )
        .flatten()
        .collect();
        assert_eq!(
            ide.seen(),
            expected,
            "each chord raised exactly one shortcut message"
        );
    });
}

#[test]
fn the_edit_chords_are_never_intercepted() {
    with_ide(Front::Code, |ide| {
        let chords = [
            Chord::ctrl(Key::Z),
            Chord::ctrl(Key::Y),
            Chord::ctrl_shift(Key::Z),
            Chord::ctrl(Key::X),
            Chord::ctrl(Key::C),
            Chord::ctrl(Key::V),
            Chord::ctrl(Key::A),
            Chord::plain(Key::DELETE),
        ];
        for (_, id) in focus_targets(ide) {
            ide.focus(id);
            for chord in chords {
                ide.press(chord);
            }
        }
        assert_eq!(ide.seen(), Vec::new(), "the focused widget owns them");
    });
}

#[test]
fn edit_chords_reach_the_code_editor_exactly_once() {
    with_ide(Front::Code, |ide| {
        let clipboard = InProcessClipboard;
        xui_code_editor::Clipboard::set_text(&clipboard, "");
        let editor = &ide.editor;
        ide.focus(editor.id());
        let original = editor.text();
        assert!(!original.is_empty(), "the form template has code");

        // Select all and copy: the clipboard holds the text, the text is kept.
        ide.press(Chord::ctrl(Key::A));
        ide.press(Chord::ctrl(Key::C));
        assert_eq!(
            xui_code_editor::Clipboard::text(&clipboard).as_deref(),
            Some(original.as_str())
        );
        assert_eq!(editor.text(), original);

        // Cut empties the buffer. A double dispatch would cut a second time
        // and leave the same empty buffer, so the undo history below counts.
        ide.press(Chord::ctrl(Key::X));
        assert_eq!(editor.text(), "");
        ide.type_text("q");
        assert_eq!(editor.text(), "q");

        // One Ctrl+Z undoes only the typing; a second undoes the cut.
        ide.press(Chord::ctrl(Key::Z));
        assert_eq!(editor.text(), "", "Ctrl+Z undid one step, not two");
        ide.press(Chord::ctrl(Key::Z));
        assert_eq!(editor.text(), original);
        // Ctrl+Y and Ctrl+Shift+Z each redo one step.
        ide.press(Chord::ctrl(Key::Y));
        assert_eq!(editor.text(), "");
        ide.press(Chord::ctrl(Key::Z));
        ide.press(Chord::ctrl_shift(Key::Z));
        assert_eq!(editor.text(), "");

        // Paste puts the clipboard back once.
        ide.press(Chord::ctrl(Key::V));
        assert_eq!(editor.text(), original);

        // Delete removes the selection only.
        ide.press(Chord::ctrl(Key::A));
        ide.press(Chord::plain(Key::DELETE));
        assert_eq!(editor.text(), "");
    });
}

#[test]
fn pasting_an_empty_clipboard_leaves_the_code_alone() {
    with_ide(Front::Code, |ide| {
        xui_code_editor::Clipboard::set_text(&InProcessClipboard, "");
        ide.focus(ide.editor.id());
        let original = ide.editor.text();
        ide.press(Chord::ctrl(Key::V));
        assert_eq!(ide.editor.text(), original);
        ide.press(Chord::ctrl(Key::Z));
        assert_eq!(ide.editor.text(), original, "no undo step was recorded");
    });
}

#[test]
fn edit_chords_act_on_the_designers_controls_exactly_once() {
    with_ide(Front::Designer, |ide| {
        ide.focus(ide.designer.borrow().id());
        let start = ide.node_count();
        assert!(start >= 1, "the button was drawn");

        // Copy then paste adds exactly one control.
        ide.press(Chord::ctrl(Key::C));
        assert_eq!(ide.node_count(), start, "copy adds nothing");
        ide.press(Chord::ctrl(Key::V));
        assert_eq!(ide.node_count(), start + 1, "paste added one, not two");

        // The pasted control is selected: cut removes it, and it pastes back.
        ide.press(Chord::ctrl(Key::X));
        assert_eq!(ide.node_count(), start, "cut removed the selection");
        ide.press(Chord::ctrl(Key::V));
        assert_eq!(ide.node_count(), start + 1);

        // Delete removes the selection; Ctrl+Z restores it; Ctrl+Y removes it.
        ide.press(Chord::plain(Key::DELETE));
        assert_eq!(ide.node_count(), start);
        ide.press(Chord::ctrl(Key::Z));
        assert_eq!(ide.node_count(), start + 1, "undo restored one control");
        ide.press(Chord::ctrl(Key::Y));
        assert_eq!(ide.node_count(), start, "redo removed it again");
        ide.press(Chord::ctrl(Key::Z));

        // Select All selects every control, so Delete empties the form.
        ide.press(Chord::ctrl(Key::A));
        ide.press(Chord::plain(Key::DELETE));
        assert_eq!(ide.node_count(), 0);
        assert_eq!(ide.seen(), Vec::new(), "no chord became an IDE command");
    });
}

#[test]
fn cut_and_delete_with_nothing_selected_change_nothing() {
    with_ide(Front::Designer, |ide| {
        let designer = Rc::clone(&ide.designer);
        ide.focus(designer.borrow().id());
        let start = ide.node_count();
        // Escape selects the form itself, which is not a control.
        ide.press(Chord::plain(Key::ESCAPE));
        ide.press(Chord::ctrl(Key::X));
        ide.press(Chord::plain(Key::DELETE));
        assert_eq!(ide.node_count(), start);
    });
}

#[test]
fn the_explorer_ignores_edit_chords_and_keeps_the_ide_shortcuts() {
    with_ide(Front::Code, |ide| {
        let code = ide.editor.text();
        let nodes = ide.node_count();
        ide.focus(ide.tree);
        for chord in [
            Chord::ctrl(Key::C),
            Chord::ctrl(Key::X),
            Chord::ctrl(Key::V),
            Chord::ctrl(Key::A),
            Chord::ctrl(Key::Z),
            Chord::ctrl(Key::Y),
        ] {
            ide.press(chord);
        }
        assert_eq!(ide.editor.text(), code, "the code was not touched");
        assert_eq!(ide.node_count(), nodes, "the form was not touched");
        assert_eq!(ide.seen(), Vec::new());
        ide.press(Chord::ctrl(Key::S));
        assert_eq!(ide.seen(), vec![Command::Save], "Ctrl+S still fires");
    });
}

#[test]
fn a_menu_edit_command_hands_the_focus_back_to_the_document() {
    with_ide(Front::Code, |ide| {
        ide.focus(ide.tree);
        let original = ide.editor.text();
        // The menu or toolbar dispatches the command; Copy leaves the text.
        assert!(ide.proxy.send(Msg::Command(Command::Copy)).is_ok());
        ide.offscreen.pump(ide.window);
        // A window system tells the new focus holder; if the focus had stayed
        // on the explorer, the typing below would go there instead.
        ide.offscreen.inject(ide.window, Event::SetFocus);
        // Typing now lands in the editor, so the focus came back to it.
        ide.type_text("!");
        assert_eq!(
            ide.editor.text().chars().count(),
            original.chars().count() + 1
        );
        assert!(ide.editor.text().contains('!'));
    });
}
