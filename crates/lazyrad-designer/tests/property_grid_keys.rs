//! Key-level tests for the property grid's inline text editor.
//!
//! Real `KeyDown` events go to the focused editor through the offscreen
//! backend. The clipboard is the in-process one, so no OS clipboard is touched.

use std::cell::RefCell;
use std::rc::Rc;

use xui_canvas::OffscreenBackend;
use xui_code_editor::Clipboard;
use xui_code_editor::platform::InProcessClipboard;
use xui_core::app::{App, Ui, run_app};
use xui_core::backend::{Backend, Event, PlatformSpec, WindowId};
use xui_core::geometry::Rect;
use xui_core::message::{Key, Modifiers};
use xui_core::units::Dip;
use xui_form::{FormDoc, Node, Value};

use lazyrad_designer::{Designer, DesignerMsg, PropertyGrid, PropertyGridMsg};

#[derive(Clone, Debug)]
enum Msg {
    Designer(DesignerMsg),
    Grid(PropertyGridMsg),
}

struct Host {
    designer: Rc<RefCell<Designer<Msg>>>,
    grid: PropertyGrid<Msg>,
}

impl App for Host {
    type Msg = Msg;

    fn update(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
        match msg {
            Msg::Designer(msg) => self.designer.borrow().update(msg, ui),
            Msg::Grid(msg) => self.grid.update(msg, ui),
        }
    }
}

/// The window and designer the app hands to the scenario.
type Slot = Rc<RefCell<Option<(WindowId, Rc<RefCell<Designer<Msg>>>)>>>;

fn button_doc() -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    let mut button = Node::new("Button", "ok_button");
    button.set_prop("left", Value::Int(16));
    button.set_prop("top", Value::Int(16));
    button.set_prop("width", Value::Int(80));
    button.set_prop("height", Value::Int(24));
    button.set_prop("text", Value::Text("Go".into()));
    doc.insert(button);
    doc
}

fn key(key: Key, ctrl: bool, shift: bool) -> Event {
    Event::KeyDown {
        key,
        modifiers: Modifiers {
            ctrl,
            shift,
            ..Modifiers::NONE
        },
        repeat: 1,
        system: false,
    }
}

/// Opens the `text` row's editor on a button reading "Go", then hands the
/// backend, window and designer to `scenario`, which runs once the app is live
/// so committed messages reach `update`.
fn with_open_editor(scenario: impl FnOnce(&OffscreenBackend, WindowId, &Designer<Msg>) + 'static) {
    let offscreen = Rc::new(OffscreenBackend::new());
    let backend: Rc<dyn Backend> = offscreen.clone();
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let slot: Slot = Rc::new(RefCell::new(None));
    let slot_for_app = Rc::clone(&slot);
    let hook_backend = Rc::clone(&offscreen);
    offscreen.set_run_hook(move || {
        let (window, designer) = slot.borrow_mut().take().expect("the app was built");
        scenario(&hook_backend, window, &designer.borrow());
    });
    let spec = PlatformSpec::new("grid keys").size(Dip(560.0), Dip(240.0));
    run_app(backend, spec, move |ui| {
        let designer = Rc::new(RefCell::new(
            Designer::new(
                ui,
                Rect::new(0, 0, 320, 200),
                button_doc(),
                Rc::clone(&catalog),
                Msg::Designer,
            )
            .expect("the designer builds"),
        ));
        let grid = PropertyGrid::new(
            ui,
            Rect::new(340, 0, 556, 200),
            Rc::clone(&designer),
            catalog,
            Msg::Grid,
        )
        .expect("the grid builds")
        .with_clipboard(InProcessClipboard);
        designer.borrow().select_node("ok_button", ui);
        let index = grid
            .rows()
            .iter()
            .position(|row| row.name == "text")
            .expect("a text row");
        grid.update(PropertyGridMsg::BeginEdit(index), ui);
        *slot_for_app.borrow_mut() = Some((ui.window(), Rc::clone(&designer)));
        Host { designer, grid }
    })
    .expect("run_app succeeds");
}

fn button_text(designer: &Designer<Msg>) -> Option<Value> {
    designer.doc().node("ok_button")?.prop("text").cloned()
}

#[test]
fn copy_cut_paste_undo_and_select_all_work_in_the_inline_editor() {
    with_open_editor(|offscreen, window, designer| {
        let clip = InProcessClipboard;
        clip.set_text("");

        // Ctrl+A then Ctrl+C: the clipboard gets the value, the text stays.
        offscreen.inject(window, key(Key::A, true, false));
        offscreen.inject(window, key(Key::C, true, false));
        assert_eq!(clip.text().as_deref(), Some("Go"));

        // Ctrl+X removes the selection; Ctrl+Z brings it back; Ctrl+Y redoes.
        offscreen.inject(window, key(Key::X, true, false));
        offscreen.inject(window, key(Key::Z, true, false));
        offscreen.inject(window, key(Key::Y, true, false));

        // Ctrl+V pastes over nothing, then Enter commits through the designer.
        clip.set_text("Stop");
        offscreen.inject(window, key(Key::V, true, false));
        offscreen.inject(window, key(Key::RETURN, false, false));
        offscreen.pump(window);
        assert_eq!(button_text(designer), Some(Value::Text("Stop".into())));
    });
}

#[test]
fn ctrl_z_restores_the_original_value_before_committing() {
    with_open_editor(|offscreen, window, designer| {
        offscreen.inject(window, key(Key::A, true, false));
        offscreen.inject(window, Event::Char('X'));
        offscreen.inject(window, key(Key::Z, true, false));
        offscreen.inject(window, key(Key::RETURN, false, false));
        offscreen.pump(window);
        assert_eq!(button_text(designer), Some(Value::Text("Go".into())));
    });
}

#[test]
fn pasting_an_empty_clipboard_leaves_the_value_alone() {
    with_open_editor(|offscreen, window, designer| {
        InProcessClipboard.set_text("");
        offscreen.inject(window, key(Key::A, true, false));
        offscreen.inject(window, key(Key::V, true, false));
        offscreen.inject(window, key(Key::RETURN, false, false));
        offscreen.pump(window);
        assert_eq!(button_text(designer), Some(Value::Text("Go".into())));
    });
}

#[test]
fn an_ide_chord_does_not_change_the_edited_text() {
    with_open_editor(|offscreen, window, designer| {
        // Ctrl+S is the IDE's; the field neither types it nor consumes it.
        offscreen.inject(window, key(Key::S, true, false));
        offscreen.inject(window, Event::Char('\u{13}'));
        offscreen.inject(window, key(Key::RETURN, false, false));
        offscreen.pump(window);
        assert_eq!(button_text(designer), Some(Value::Text("Go".into())));
    });
}
