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

use lazyrad_designer::{
    Designer, DesignerMsg, PropertyGrid, PropertyGridMsg, RowMove, Target, View, rename_handlers,
};

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
        // The renamed control stays selected, so the grid keeps showing it.
        assert_eq!(
            editor.designer.borrow().selection(),
            lazyrad_designer::Selection::Nodes(vec!["go_button".to_owned()])
        );
        assert!(!editor.grid.rows().is_empty(), "the grid is not blank");
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
    let (error, doc) = with_editor(button_doc(), |editor, ui| {
        editor.designer.borrow().select_node("ok_button", ui);
        let index = row_index(editor, "name");
        editor.grid.update(PropertyGridMsg::BeginEdit(index), ui);
        // An invalid identifier is rejected; the model keeps the original.
        editor
            .grid
            .update(PropertyGridMsg::CommitText("1bad".into()), ui);
        editor.grid.error()
    });
    assert!(doc.expect("a document").node("ok_button").is_some());
    let error = error.expect("the grid reports the rejected name");
    assert!(!error.is_empty());
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

/// Whether any pixel in `(x, y, w, h)` differs from `background`.
fn painted(
    image: &xui_canvas::RgbaImage,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    background: [u8; 4],
) -> bool {
    (y..y + h)
        .any(|row| (x..x + w).any(|col| image.pixel(col, row).is_some_and(|px| px != background)))
}

#[test]
fn the_tabs_and_category_headers_show_lucide_icons() {
    let backend = Rc::new(OffscreenBackend::new());
    let trait_backend: Rc<dyn Backend> = Rc::clone(&backend) as Rc<dyn Backend>;
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let spec = PlatformSpec::new("grid").size(Dip(560.0), Dip(240.0));

    run_app(trait_backend, spec, move |ui| {
        // The designer on the left, the grid beside it at `left` (`x` in image
        // pixels); the grid paints in its own coordinates.
        let (left, x) = (340, 340_u32);
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
            Rect::new(left, 0, left + 216, 200),
            Rc::clone(&designer),
            catalog,
            Msg::Grid,
        )
        .expect("the grid builds");
        designer.borrow().select_node("ok_button", ui);
        grid.update(PropertyGridMsg::SetView(View::Categorized), ui);
        let image = backend.render(ui.window()).expect("the window renders");

        // A grid surface pixel, clear of the combo and the tabs.
        let bg = image.pixel(x + 1, 1).expect("a background sample");
        // Tab icons: 16px, 4px in from each tab's left edge (tabs at y 32).
        assert!(
            painted(&image, x + 8, 35, 16, 16, bg),
            "the Alphabetic tab shows its icon"
        );
        assert!(
            painted(&image, x + 112, 35, 16, 16, bg),
            "the Categorized tab shows its icon"
        );
        // The first category header's expander chevron (body top 58).
        assert!(
            painted(&image, x + 4, 62, 14, 14, bg),
            "the category header shows its chevron"
        );

        Editor { designer, grid }
    })
    .expect("run_app succeeds");
}

#[test]
fn scrolling_is_clamped_to_the_rows() {
    let ((up, down), _) = with_editor(button_doc(), |editor, ui| {
        // A short grid cannot show every row.
        editor.grid.set_bounds(Rect::new(340, 0, 556, 110));
        editor.grid.sync(ui);
        editor.grid.update(PropertyGridMsg::Scroll(-500), ui);
        let up = editor.grid.scroll_offset();
        editor.grid.update(PropertyGridMsg::Scroll(100_000), ui);
        (up, editor.grid.scroll_offset())
    });
    assert_eq!(up, 0, "cannot scroll above the first row");
    assert!(
        down > 0,
        "a 200px grid cannot show every row, so it scrolls"
    );
    assert!(down < 100_000, "the offset stops at the last row");
}

#[test]
fn editing_a_row_below_the_view_scrolls_it_into_view() {
    let (scrolled, _) = with_editor(button_doc(), |editor, ui| {
        // A short grid cannot show every row, so the last one is below the view.
        editor.grid.set_bounds(Rect::new(340, 0, 556, 110));
        editor.grid.sync(ui);
        let last = editor.grid.rows().len() - 1;
        editor.grid.update(PropertyGridMsg::BeginEdit(last), ui);
        editor.grid.scroll_offset()
    });
    assert!(scrolled > 0, "the last row was brought into view");
}

/// A short grid (110px), so neither its rows nor its dropdown can all show.
fn shorten(editor: &Editor, ui: &mut Ui<Msg>) {
    editor.grid.set_bounds(Rect::new(340, 0, 556, 110));
    editor.grid.sync(ui);
}

/// A form with `count` buttons named `button_00`, `button_01`, ...
fn many_buttons_doc(count: usize) -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    for index in 0..count {
        doc.insert(Node::new("Button", format!("button_{index:02}")));
    }
    doc
}

/// Sends `mv` to the grid.
fn press(editor: &Editor, ui: &mut Ui<Msg>, mv: RowMove) {
    editor.grid.update(PropertyGridMsg::MoveRow(mv), ui);
}

/// The number of dropdown entries showing, from its height in 22px rows.
fn shown_entries(editor: &Editor, ui: &Ui<Msg>) -> usize {
    (editor.grid.dropdown_rect(ui).expect("open").height() / 22) as usize
}

#[test]
fn down_from_the_last_row_stays_put() {
    let ((first, at_end, last, again, up), _) = with_editor(button_doc(), |editor, ui| {
        shorten(editor, ui);
        let last = editor.grid.rows().len() - 1;
        press(editor, ui, RowMove::Down);
        let first = editor.grid.current_row();
        press(editor, ui, RowMove::End);
        let at_end = editor.grid.current_row();
        press(editor, ui, RowMove::Down);
        let again = editor.grid.current_row();
        press(editor, ui, RowMove::Home);
        press(editor, ui, RowMove::Up);
        (first, at_end, last, again, editor.grid.current_row())
    });
    assert_eq!(first, Some(0), "Down with no current row selects the first");
    assert_eq!(at_end, Some(last));
    assert_eq!(again, at_end, "Down from the last row stays put");
    assert_eq!(up, Some(0), "Up from the first row stays put");
}

#[test]
fn page_down_and_end_scroll_the_view() {
    let ((after_page, after_end, after_home), _) = with_editor(button_doc(), |editor, ui| {
        shorten(editor, ui);
        press(editor, ui, RowMove::PageDown);
        let page = (editor.grid.current_row(), editor.grid.scroll_offset());
        press(editor, ui, RowMove::End);
        let end = (editor.grid.current_row(), editor.grid.scroll_offset());
        press(editor, ui, RowMove::Home);
        let home = (editor.grid.current_row(), editor.grid.scroll_offset());
        (page, end, home)
    });
    assert!(after_page.0 > Some(0), "PageDown moved past the first row");
    assert!(after_end.1 > 0, "End scrolled the last row into view");
    assert!(after_end.1 >= after_page.1);
    assert_eq!(after_home, (Some(0), 0), "Home shows the first row again");
}

#[test]
fn navigating_the_categorized_view_skips_headers() {
    let ((visited, rows, home_scroll), _) = with_editor(button_doc(), |editor, ui| {
        shorten(editor, ui);
        editor
            .grid
            .update(PropertyGridMsg::SetView(View::Categorized), ui);
        let mut visited = Vec::new();
        for _ in 0..editor.grid.rows().len() + 3 {
            press(editor, ui, RowMove::Down);
            let current = editor.grid.current_row().expect("a current row");
            if visited.last() != Some(&current) {
                visited.push(current);
            }
        }
        press(editor, ui, RowMove::Home);
        let rows = editor.grid.rows().len();
        (visited, rows, editor.grid.scroll_offset())
    });
    let expected: Vec<usize> = (0..rows).collect();
    assert_eq!(
        visited, expected,
        "every row once, in order, no header stop"
    );
    assert_eq!(
        home_scroll, 0,
        "Home also reveals the first category header"
    );
}

#[test]
fn the_current_row_survives_a_sync_and_a_view_switch() {
    let ((kept, switched, other_object), _) = with_editor(button_doc(), |editor, ui| {
        editor.designer.borrow().select_node("ok_button", ui);
        shorten(editor, ui);
        let text = row_index(editor, "text");
        editor.grid.update(PropertyGridMsg::BeginEdit(text), ui);
        editor.grid.sync(ui);
        let kept = editor.grid.current_row() == Some(row_index(editor, "text"));
        editor
            .grid
            .update(PropertyGridMsg::SetView(View::Categorized), ui);
        let switched = editor.grid.current_row() == Some(row_index(editor, "text"));
        // Another object starts from a clean slate.
        editor.designer.borrow().select_form(ui);
        (kept, switched, editor.grid.current_row())
    });
    assert!(kept, "the same property is still current after a sync");
    assert!(
        switched,
        "the same property is still current in the new order"
    );
    assert_eq!(other_object, None, "a new target has no current row");
}

#[test]
fn a_current_row_that_disappears_is_dropped() {
    // The last row of a control has no counterpart on the form, so no stale
    // index survives the switch even after a sync.
    let (current, _) = with_editor(button_doc(), |editor, ui| {
        editor.designer.borrow().select_node("ok_button", ui);
        let last = editor.grid.rows().len() - 1;
        editor.grid.update(PropertyGridMsg::BeginEdit(last), ui);
        editor.designer.borrow().select_form(ui);
        editor.grid.sync(ui);
        editor.grid.current_row()
    });
    assert_eq!(current, None);
}

#[test]
fn clicking_a_row_makes_it_current() {
    let (current, _) = with_editor(button_doc(), |editor, ui| {
        editor.designer.borrow().select_node("ok_button", ui);
        let text = row_index(editor, "text");
        editor.grid.update(PropertyGridMsg::BeginEdit(text), ui);
        editor.grid.current_row() == Some(text)
    });
    assert!(current);
}

#[test]
fn activate_edits_the_current_row_and_ignores_no_row() {
    let (idle, doc) = with_editor(button_doc(), |editor, ui| {
        editor.designer.borrow().select_node("ok_button", ui);
        // No current row: nothing opens, so a stray commit is dropped.
        editor.grid.update(PropertyGridMsg::Activate, ui);
        editor
            .grid
            .update(PropertyGridMsg::CommitText("ignored".into()), ui);
        let idle = editor.grid.error().is_none();
        let text = row_index(editor, "text");
        while editor.grid.current_row() != Some(text) {
            press(editor, ui, RowMove::Down);
        }
        editor.grid.update(PropertyGridMsg::Activate, ui);
        editor
            .grid
            .update(PropertyGridMsg::CommitText("Open".into()), ui);
        idle
    });
    assert!(idle);
    let node = doc.expect("doc").node("ok_button").cloned().expect("node");
    assert_eq!(node.props.get("text"), Some(&Value::Text("Open".into())));
}

#[test]
fn moving_the_current_row_closes_an_open_editor() {
    let (applied, doc) = with_editor(button_doc(), |editor, ui| {
        editor.designer.borrow().select_node("ok_button", ui);
        let text = row_index(editor, "text");
        editor.grid.update(PropertyGridMsg::BeginEdit(text), ui);
        press(editor, ui, RowMove::Down);
        // A late commit from the closed editor must not be applied.
        editor
            .grid
            .update(PropertyGridMsg::CommitText("late".into()), ui);
        editor.grid.error().is_some()
    });
    assert!(!applied);
    let node = doc.expect("doc").node("ok_button").cloned().expect("node");
    assert_eq!(node.props.get("text"), Some(&Value::Text("Go".into())));
}

#[test]
fn the_dropdown_stays_inside_the_grid_and_scrolls() {
    let ((rect, first, rows, last_first, picked), _) =
        with_editor(many_buttons_doc(20), |editor, ui| {
            shorten(editor, ui);
            editor.grid.update(PropertyGridMsg::ToggleObjects, ui);
            let rect = editor.grid.dropdown_rect(ui).expect("open");
            let first = editor.grid.dropdown_first(ui);
            let rows = shown_entries(editor, ui);
            editor.grid.update(PropertyGridMsg::ScrollObjects(1000), ui);
            let last_first = editor.grid.dropdown_first(ui);
            // The drawn top entry is the one a click there picks.
            let picked = editor.grid.object_at(ui, rect.left + 2, rect.top + 2);
            (rect, first, rows, last_first, picked)
        });
    assert!(
        rect.bottom <= 110,
        "the dropdown ends inside the 110px grid"
    );
    assert!(rows > 0 && rows < 21, "some but not all of 21 objects show");
    assert_eq!(first, 0);
    assert_eq!(last_first, 21 - rows, "scrolled to the end, not past it");
    assert_eq!(
        picked,
        Some(Target::Node(format!("button_{:02}", last_first - 1))),
        "the entry drawn at the top after scrolling is the one hit"
    );
}

#[test]
fn dropdown_clicks_outside_its_entries_hit_nothing() {
    let ((below, closed), _) = with_editor(many_buttons_doc(20), |editor, ui| {
        shorten(editor, ui);
        let closed = editor.grid.object_at(ui, 10, 40);
        editor.grid.update(PropertyGridMsg::ToggleObjects, ui);
        let rect = editor.grid.dropdown_rect(ui).expect("open");
        let below = editor.grid.object_at(ui, rect.left + 2, rect.bottom + 1);
        (below, closed)
    });
    assert_eq!(below, None, "below the clamped list is not an entry");
    assert_eq!(closed, None, "a closed dropdown has no entries");
}

#[test]
fn dropdown_keys_move_the_highlight_and_keep_it_in_view() {
    let ((end, past, rows, selected), _) = with_editor(many_buttons_doc(20), |editor, ui| {
        shorten(editor, ui);
        editor.grid.update(PropertyGridMsg::ToggleObjects, ui);
        let rows = shown_entries(editor, ui);
        press(editor, ui, RowMove::End);
        let end = (
            editor.grid.dropdown_highlight(),
            editor.grid.dropdown_first(ui),
        );
        press(editor, ui, RowMove::Down);
        let past = editor.grid.dropdown_highlight();
        press(editor, ui, RowMove::Up);
        editor.grid.update(PropertyGridMsg::Activate, ui);
        (end, past, rows, editor.designer.borrow().selection())
    });
    assert_eq!(end.0, 20);
    assert_eq!(past, 20, "Down from the last object stays put");
    assert_eq!(end.1, 21 - rows, "the highlighted last object is visible");
    assert_eq!(
        selected,
        lazyrad_designer::Selection::Nodes(vec!["button_18".into()]),
        "Return picked the highlighted object"
    );
}

#[test]
fn escape_closes_the_dropdown_without_moving_the_rows() {
    let ((open, closed, current), _) = with_editor(many_buttons_doc(3), |editor, ui| {
        editor.designer.borrow().select_node("button_01", ui);
        press(editor, ui, RowMove::Down);
        editor.grid.update(PropertyGridMsg::ToggleObjects, ui);
        let open = editor.grid.objects_open();
        press(editor, ui, RowMove::Down);
        editor.grid.update(PropertyGridMsg::Cancel, ui);
        (open, !editor.grid.objects_open(), editor.grid.current_row())
    });
    assert!(open);
    assert!(closed);
    assert_eq!(current, Some(0), "dropdown keys did not move the rows");
}

#[test]
fn the_dropdown_reopens_on_the_selected_object() {
    let ((highlight, first, rows), _) = with_editor(many_buttons_doc(20), |editor, ui| {
        shorten(editor, ui);
        editor.designer.borrow().select_node("button_17", ui);
        editor.grid.update(PropertyGridMsg::ToggleObjects, ui);
        let rows = shown_entries(editor, ui);
        (
            editor.grid.dropdown_highlight(),
            editor.grid.dropdown_first(ui),
            rows,
        )
    });
    assert_eq!(highlight, 18, "the form is entry 0, so button_17 is 18");
    assert!(
        (first..first + rows).contains(&highlight),
        "the selected object is scrolled into view"
    );
}

#[test]
fn a_sync_closes_the_dropdown_and_resets_its_highlight() {
    let ((open, highlight), _) = with_editor(many_buttons_doc(20), |editor, ui| {
        shorten(editor, ui);
        editor.grid.update(PropertyGridMsg::ToggleObjects, ui);
        press(editor, ui, RowMove::End);
        editor.grid.sync(ui);
        (editor.grid.objects_open(), editor.grid.dropdown_highlight())
    });
    assert!(!open);
    assert_eq!(highlight, 0, "the form is still the selected object");
}

#[test]
fn a_tiny_grid_gives_the_dropdown_no_entries_without_panicking() {
    let ((rect, picked, highlight), _) = with_editor(many_buttons_doc(5), |editor, ui| {
        editor.grid.set_bounds(Rect::new(340, 0, 556, 30));
        editor.grid.sync(ui);
        editor.grid.update(PropertyGridMsg::ToggleObjects, ui);
        press(editor, ui, RowMove::PageDown);
        editor.grid.update(PropertyGridMsg::ScrollObjects(-5), ui);
        let rect = editor.grid.dropdown_rect(ui).expect("open");
        let picked = editor.grid.object_at(ui, rect.left + 2, rect.top + 2);
        (rect, picked, editor.grid.dropdown_highlight())
    });
    assert_eq!(rect.height(), 0);
    assert_eq!(picked, None);
    assert!(highlight <= 5, "the highlight stays a valid object index");
}
