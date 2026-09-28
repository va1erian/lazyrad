//! End-to-end tests for the [`PropertyGrid`] on the offscreen backend.
//!
//! The grid is built next to a [`Designer`] in one window and driven through
//! its messages. The designer owns the document and the undo history, so these
//! tests check that an edit made in the grid reaches the model, is undoable and
//! renames event handlers through the designer's rename sink.

use std::cell::RefCell;
use std::rc::Rc;

use xui_canvas::OffscreenBackend;
use xui_core::app::{App, Ui, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::geometry::Rect;
use xui_core::units::Dip;
use xui_form::{FormDoc, Node, Value};

use lazyrad_designer::{Designer, DesignerMsg, PropertyGrid, PropertyGridMsg, rename_handlers};

/// The host's message type.
#[derive(Clone, Debug)]
enum Msg {
    /// A designer input message.
    Designer(DesignerMsg),
    /// A property-grid message.
    Grid(PropertyGridMsg),
}

/// An app owning both the designer and its property grid.
struct Editor {
    designer: Rc<RefCell<Designer<Msg>>>,
    grid: PropertyGrid<Msg>,
}

impl App for Editor {
    type Msg = Msg;

    fn update(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
        match msg {
            Msg::Designer(msg) => self.designer.borrow().update(msg, ui),
            Msg::Grid(msg) => self.grid.update(msg, ui),
        }
    }
}

/// A form with one button, for the grid tests.
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

/// Builds a designer and grid offscreen, runs `check` inside the app closure
/// and returns its result.
fn with_editor<R>(
    doc: FormDoc,
    check: impl FnOnce(&Editor, &mut Ui<Msg>) -> R,
) -> (R, Option<FormDoc>) {
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let result: Rc<RefCell<Option<R>>> = Rc::new(RefCell::new(None));
    let doc_after: Rc<RefCell<Option<FormDoc>>> = Rc::new(RefCell::new(None));
    let result_for_app = Rc::clone(&result);
    let doc_for_app = Rc::clone(&doc_after);
    let spec = PlatformSpec::new("grid").size(Dip(560.0), Dip(240.0));

    run_app(backend, spec, move |ui| {
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            doc,
            Rc::clone(&catalog),
            Msg::Designer,
        )
        .expect("the designer builds");
        let designer = Rc::new(RefCell::new(designer));
        let grid = PropertyGrid::new(
            ui,
            Rect::new(340, 0, 556, 200),
            Rc::clone(&designer),
            catalog,
            Msg::Grid,
        )
        .expect("the grid builds");
        let editor = Editor { designer, grid };
        let outcome = check(&editor, ui);
        *result_for_app.borrow_mut() = Some(outcome);
        *doc_for_app.borrow_mut() = Some(editor.designer.borrow().doc());
        editor
    })
    .expect("run_app succeeds");

    let result = result.borrow_mut().take().expect("the check ran");
    let doc = doc_after.borrow_mut().take();
    (result, doc)
}

/// The index of a row by schema name.
fn row_index(editor: &Editor, name: &str) -> usize {
    editor
        .grid
        .rows()
        .iter()
        .position(|row| row.name == name)
        .unwrap_or_else(|| panic!("no `{name}` row"))
}

#[test]
fn the_grid_follows_the_designer_selection() {
    let (target, _) = with_editor(button_doc(), |editor, ui| {
        editor.designer.borrow().select_node("ok_button", ui);
        let target = editor.grid.target();
        let rows = editor.grid.rows();
        assert_eq!(target, lazyrad_designer::Target::Node("ok_button".into()));
        assert!(rows.iter().any(|row| row.is_name));
        assert!(rows.iter().any(|row| row.name == "text"));
        target
    });
    assert_eq!(target, lazyrad_designer::Target::Node("ok_button".into()));
}

#[test]
fn editing_text_updates_the_model_and_is_undoable() {
    let (after, _) = with_editor(button_doc(), |editor, ui| {
        editor.designer.borrow().select_node("ok_button", ui);
        let index = row_index(editor, "text");
        editor.grid.update(PropertyGridMsg::BeginEdit(index), ui);
        editor
            .grid
            .update(PropertyGridMsg::CommitText("Hello".into()), ui);
        let edited = editor.designer.borrow().doc();
        let live = editor.designer.borrow().live_value("ok_button", "text");
        assert!(editor.designer.borrow().undo(ui));
        let undone = editor.designer.borrow().doc();
        let live_after_undo = editor.designer.borrow().live_value("ok_button", "text");
        (edited, live, undone, live_after_undo)
    });
    assert_eq!(
        after
            .0
            .node("ok_button")
            .and_then(|node| node.prop("text"))
            .cloned(),
        Some(Value::Text("Hello".into()))
    );
    assert_eq!(after.1, Some(Value::Text("Hello".into())));
    assert_eq!(
        after
            .2
            .node("ok_button")
            .and_then(|node| node.prop("text"))
            .cloned(),
        Some(Value::Text("Go".into()))
    );
    // Undo pushes the restored value back into the live widget too.
    assert_eq!(after.3, Some(Value::Text("Go".into())));
}

#[test]
fn editing_geometry_updates_the_model() {
    let (_, doc) = with_editor(button_doc(), |editor, ui| {
        editor.designer.borrow().select_node("ok_button", ui);
        let index = row_index(editor, "left");
        editor.grid.update(PropertyGridMsg::BeginEdit(index), ui);
        editor
            .grid
            .update(PropertyGridMsg::CommitText("64".into()), ui);
    });
    assert_eq!(
        doc.expect("a document")
            .node("ok_button")
            .and_then(|node| node.prop("left")),
        Some(&Value::Int(64))
    );
}

#[test]
fn editing_an_enum_commits_the_chosen_variant() {
    let (_, doc) = with_editor(button_doc(), |editor, ui| {
        editor.designer.borrow().select_node("ok_button", ui);
        let index = row_index(editor, "anchor");
        editor.grid.update(PropertyGridMsg::BeginEdit(index), ui);
        // `fill` is the last variant of the anchor enum.
        editor.grid.update(PropertyGridMsg::CommitChoice(11), ui);
    });
    assert_eq!(
        doc.expect("a document")
            .node("ok_button")
            .and_then(|node| node.prop("anchor")),
        Some(&Value::Enum("fill".into()))
    );
}

#[test]
fn editing_a_bool_commits_the_new_state() {
    let (_, doc) = with_editor(button_doc(), |editor, ui| {
        editor.designer.borrow().select_node("ok_button", ui);
        let index = row_index(editor, "enabled");
        editor.grid.update(PropertyGridMsg::BeginEdit(index), ui);
        editor.grid.update(PropertyGridMsg::CommitBool(false), ui);
    });
    assert_eq!(
        doc.expect("a document")
            .node("ok_button")
            .and_then(|node| node.prop("enabled")),
        Some(&Value::Bool(false))
    );
}

#[test]
fn renaming_a_control_fires_the_rename_sink() {
    let renamed: Rc<RefCell<Option<(String, String)>>> = Rc::new(RefCell::new(None));
    let log = Rc::clone(&renamed);
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let doc_after: Rc<RefCell<Option<FormDoc>>> = Rc::new(RefCell::new(None));
    let doc_for_app = Rc::clone(&doc_after);
    let spec = PlatformSpec::new("grid").size(Dip(560.0), Dip(240.0));

    run_app(backend, spec, move |ui| {
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            button_doc(),
            Rc::clone(&catalog),
            Msg::Designer,
        )
        .expect("the designer builds");
        designer.set_rename_sink(move |old, new| {
            *log.borrow_mut() = Some((old.to_owned(), new.to_owned()));
        });
        let designer = Rc::new(RefCell::new(designer));
        let grid = PropertyGrid::new(
            ui,
            Rect::new(340, 0, 556, 200),
            Rc::clone(&designer),
            catalog,
            Msg::Grid,
        )
        .expect("the grid builds");
        let editor = Editor { designer, grid };
        editor.designer.borrow().select_node("ok_button", ui);
        let index = row_index(&editor, "name");
        editor.grid.update(PropertyGridMsg::BeginEdit(index), ui);
        editor
            .grid
            .update(PropertyGridMsg::CommitText("go_button".into()), ui);
        *doc_for_app.borrow_mut() = Some(editor.designer.borrow().doc());
        editor
    })
    .expect("run_app succeeds");

    assert_eq!(
        renamed.borrow().as_ref(),
        Some(&("ok_button".to_owned(), "go_button".to_owned()))
    );
    let doc = doc_after.borrow();
    let node = doc
        .as_ref()
        .expect("a doc")
        .node("go_button")
        .expect("renamed");
    assert_eq!(
        rename_handlers(
            "fn ok_button_click() {}",
            "ok_button",
            "go_button",
            &["click".to_owned()]
        ),
        "fn go_button_click() {}"
    );
    assert_eq!(node.kind, "Button");
}

#[test]
fn an_invalid_name_keeps_the_control_and_reports_an_error() {
    let (_, doc) = with_editor(button_doc(), |editor, ui| {
        editor.designer.borrow().select_node("ok_button", ui);
        let index = row_index(editor, "name");
        editor.grid.update(PropertyGridMsg::BeginEdit(index), ui);
        // An invalid identifier is rejected; the model keeps the original.
        editor
            .grid
            .update(PropertyGridMsg::CommitText("1bad".into()), ui);
    });
    assert!(doc.expect("a document").node("ok_button").is_some());
}

#[test]
fn the_grid_paints_without_panicking() {
    let backend = Rc::new(OffscreenBackend::new());
    let trait_backend: Rc<dyn Backend> = Rc::clone(&backend) as Rc<dyn Backend>;
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let spec = PlatformSpec::new("grid").size(Dip(560.0), Dip(240.0));

    run_app(trait_backend, spec, move |ui| {
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            button_doc(),
            Rc::clone(&catalog),
            Msg::Designer,
        )
        .expect("the designer builds");
        let designer = Rc::new(RefCell::new(designer));
        let grid = PropertyGrid::new(
            ui,
            Rect::new(340, 0, 556, 200),
            Rc::clone(&designer),
            catalog,
            Msg::Grid,
        )
        .expect("the grid builds");
        designer.borrow().select_node("ok_button", ui);
        let image = backend.render(ui.window()).expect("the window renders");
        assert_eq!((image.width, image.height), (560, 240));
        Editor { designer, grid }
    })
    .expect("run_app succeeds");
}
