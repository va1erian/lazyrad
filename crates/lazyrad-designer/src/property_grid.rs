#![forbid(unsafe_code)]

//! The property grid: a two-column name/value list with per-type editors.
//!
//! xui has no property grid and no editable list cells (PLAN.md gaps G5 and G6),
//! so LazyRAD draws its own. [`PropertyGrid`] is a [`Custom`](xui_core::backend::NodeKind)
//! node that paints an object combo, the Alphabetic/Categorized tabs and the
//! property rows, and overlays a small editor on the value cell being edited.
//!
//! The rows come from the `xui-form` [`Catalog`], which is the single source of
//! truth for property types, defaults, categories and access rules. A
//! [`RuntimeOnly`](xui_form::Access::RuntimeOnly) property is hidden;
//! [`ReadOnly`](xui_form::Access::ReadOnly) properties are shown but cannot be
//! edited. The synthetic `(Name)` row at the top of a control renames it
//! through [`Surface::set_property`](crate::Surface::set_property), which
//! validates the identifier and its uniqueness.
//!
//! Editing commits through the designer: the grid calls
//! [`Designer::set_property`](crate::Designer::set_property), which records one
//! undo step on the [`Surface`](crate::Surface) and refreshes the live preview.
//! A text or numeric cell opens a small painted editor; Enter or losing focus
//! commits, and Escape reverts. A bool or enum cell opens a `CheckBox` or
//! `ComboBox`, whose own change is the commit.
//!
//! The host owns both widgets as `Rc<RefCell<Designer<_>>>` and forwards the
//! grid's messages from `App::update`. The grid subscribes to the designer's
//! selection, so the object combo and the rows follow the surface; the host can
//! still call [`PropertyGrid::sync`] after a change the grid did not make.
//!
//! # Example
//!
//! ```rust,no_run
//! # use std::cell::RefCell;
//! # use std::rc::Rc;
//! # use xui_core::app::{App, Ui};
//! # use xui_core::geometry::Rect;
//! # use xui_form::FormDoc;
//! # use lazyrad_designer::{Designer, DesignerMsg, PropertyGrid, PropertyGridMsg};
//! #[derive(Clone)]
//! enum Msg {
//!     Designer(DesignerMsg),
//!     Grid(PropertyGridMsg),
//! }
//!
//! struct Editor {
//!     designer: Rc<RefCell<Designer<Msg>>>,
//!     grid: PropertyGrid<Msg>,
//! }
//!
//! impl App for Editor {
//!     type Msg = Msg;
//!     fn update(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
//!         match msg {
//!             Msg::Designer(msg) => self.designer.borrow().update(msg, ui),
//!             Msg::Grid(msg) => self.grid.update(msg, ui),
//!         }
//!     }
//! }
//!
//! # fn build(ui: &mut Ui<Msg>) -> Editor {
//! let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
//! let doc = FormDoc::new("main_form");
//! let designer = Designer::new(ui, Rect::default(), doc, Rc::clone(&catalog), Msg::Designer)
//!     .expect("the designer builds");
//! let designer = Rc::new(RefCell::new(designer));
//! let grid = PropertyGrid::new(ui, Rect::new(0, 0, 240, 200), Rc::clone(&designer), catalog, Msg::Grid)
//!     .expect("the grid builds");
//! Editor { designer, grid }
//! # }
//! ```

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use xui_core::Theme;
use xui_core::app::Ui;
use xui_core::backend::{BackendError, Canvas, Event, NodeKind, NodeSpec, TextStyle, WidgetId};
use xui_core::geometry::{Point, Rect};
use xui_core::message::{Key, MouseButton};
use xui_core::units::Dip;
use xui_core::widget::{CheckBox, ComboBox, Control};
use xui_form::{Access, Catalog, FormDoc, Value, ValueType};

use crate::surface::Target;
use crate::widget::Designer;

/// The padding around the grid, in design units.
const PADDING: Dip = Dip(4.0);
/// The height of the object combo, in design units.
const COMBO_HEIGHT: Dip = Dip(24.0);
/// The height of a tab header, in design units.
const TAB_HEIGHT: Dip = Dip(22.0);
/// The height of a property row, in design units.
const ROW_HEIGHT: Dip = Dip(22.0);
/// The height of a category header, in design units.
const HEADER_HEIGHT: Dip = Dip(22.0);
/// The text size for labels and values, in design units.
const TEXT_SIZE: Dip = Dip(12.0);

/// Which view the grid shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    /// Every property, sorted by display name.
    #[default]
    Alphabetic,
    /// Properties grouped under their catalog category.
    Categorized,
}

/// One row of the property grid.
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyRow {
    /// The schema property name (`name` for the synthetic `(Name)` row).
    pub name: String,
    /// The display label.
    pub label: String,
    /// The property's type.
    pub ty: ValueType,
    /// The property's current value.
    pub value: Value,
    /// When the property may be written.
    pub access: Access,
    /// The catalog category the property belongs to.
    pub category: String,
    /// Whether this is the synthetic `(Name)` row.
    pub is_name: bool,
}

impl PropertyRow {
    /// Whether the row may be edited in the designer.
    pub fn editable(&self) -> bool {
        self.access.writable_in_design()
    }
}

/// A message the grid maps its input to, wrapped into the host's message type.
#[derive(Clone, Debug, PartialEq)]
pub enum PropertyGridMsg {
    /// The user chose an object from the combo.
    SelectObject(Target),
    /// The user switched the Alphabetic/Categorized tab.
    SetView(View),
    /// The user opened or closed the object combo.
    ToggleObjects,
    /// The user began editing the row at this index.
    BeginEdit(usize),
    /// A text or numeric editor committed `text`.
    CommitText(String),
    /// An enum editor committed a variant index.
    CommitChoice(usize),
    /// A bool editor committed a state.
    CommitBool(bool),
    /// The user pressed Escape: revert the active editor.
    Cancel,
}

/// Why a [`PropertyGrid`] could not be built.
#[derive(Debug, thiserror::Error)]
pub enum PropertyGridError {
    /// The backend could not create the grid's node or an editor node.
    #[error(transparent)]
    Backend(#[from] BackendError),
}

/// The catalog's category order for the Categorized view.
fn category_rank(category: &str) -> u8 {
    match category {
        "Misc" => 0,
        "Appearance" => 1,
        "Layout" => 2,
        "Behavior" => 3,
        "Data" => 4,
        _ => 5,
    }
}

/// A display label for a schema property name: `tab_index` becomes `Tab Index`
/// and `text` becomes `Text`.
fn display_label(name: &str) -> String {
    name.split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `value` rendered for the value cell and for a text editor.
fn format_value(value: &Value) -> String {
    match value {
        Value::Bool(value) => if *value { "True" } else { "False" }.to_owned(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::Text(value) | Value::Enum(value) => value.clone(),
        Value::Color(value) => format!("#{:02x}{:02x}{:02x}", value.r, value.g, value.b),
        Value::List(items) => items.join(", "),
        _ => String::new(),
    }
}

/// Parses `text` against `ty`, or `None` when it does not fit.
fn parse_value(ty: &ValueType, text: &str) -> Option<Value> {
    match ty {
        ValueType::Bool => match text.trim().to_ascii_lowercase().as_str() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        ValueType::Int { .. } => text.trim().parse::<i64>().ok().map(Value::Int),
        ValueType::Float { .. } => text.trim().parse::<f64>().ok().map(Value::Float),
        ValueType::Text { .. } => Some(Value::Text(text.to_owned())),
        ValueType::Enum { .. } => Some(Value::Enum(text.to_owned())),
        _ => None,
    }
}

/// Builds the rows for `target` from `doc` and the catalog.
///
/// A control's rows are its common properties and its widget-specific ones,
/// preceded by the synthetic `(Name)` row. The form's rows are its window
/// properties. [`RuntimeOnly`](Access::RuntimeOnly) properties are hidden.
/// The rows are ordered for `view`.
pub fn property_rows(
    catalog: &Catalog,
    doc: &FormDoc,
    target: &Target,
    view: View,
) -> Vec<PropertyRow> {
    let mut rows = match target {
        Target::Form => doc
            .window
            .props
            .keys()
            .chain(
                catalog
                    .window_spec()
                    .properties
                    .iter()
                    .map(|spec| &spec.name),
            )
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter_map(|name| row_for(catalog, "Window", &doc.window.props, name))
            .collect(),
        Target::Node(name) => match doc.node(name) {
            Some(node) => {
                let mut rows = vec![name_row(node)];
                let mut seen = std::collections::BTreeSet::new();
                let names = catalog
                    .common_properties()
                    .iter()
                    .map(|spec| spec.name.clone())
                    .chain(
                        catalog
                            .get(&node.kind)
                            .into_iter()
                            .flat_map(|spec| spec.properties.iter().map(|p| p.name.clone())),
                    );
                for name in names {
                    if !seen.insert(name.clone()) {
                        continue;
                    }
                    if let Some(row) = row_for(catalog, &node.kind, &node.props, &name) {
                        rows.push(row);
                    }
                }
                rows
            }
            None => Vec::new(),
        },
    };
    sort_rows(&mut rows, view);
    rows
}

/// The synthetic `(Name)` row for a node.
fn name_row(node: &xui_form::Node) -> PropertyRow {
    PropertyRow {
        name: "name".to_owned(),
        label: "(Name)".to_owned(),
        ty: ValueType::Text { multiline: false },
        value: Value::Text(node.name.clone()),
        access: Access::ReadWrite,
        category: "Misc".to_owned(),
        is_name: true,
    }
}

/// One schema property as a row, or `None` when it is hidden or unknown.
fn row_for(
    catalog: &Catalog,
    kind: &str,
    props: &std::collections::BTreeMap<String, Value>,
    name: &str,
) -> Option<PropertyRow> {
    let spec = if kind == "Window" {
        catalog.window_spec().property(name)?.clone()
    } else {
        catalog.property(kind, name)?
    };
    if spec.access == Access::RuntimeOnly {
        return None;
    }
    let value = props.get(name).cloned().unwrap_or(spec.default);
    Some(PropertyRow {
        name: spec.name.clone(),
        label: display_label(&spec.name),
        ty: spec.ty,
        value,
        access: spec.access,
        category: spec.category,
        is_name: false,
    })
}

/// Orders `rows` for `view`.
fn sort_rows(rows: &mut [PropertyRow], view: View) {
    match view {
        View::Alphabetic => rows.sort_by(|a, b| a.label.cmp(&b.label)),
        View::Categorized => rows.sort_by(|a, b| {
            category_rank(&a.category)
                .cmp(&category_rank(&b.category))
                .then_with(|| a.label.cmp(&b.label))
        }),
    }
}

/// A painted line: a category header, or a property row.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    /// A category header bearing the category's name.
    Header(String),
    /// The property row at this index.
    Row(usize),
}

/// The lines to paint for `rows` in `view`, with a header before each category
/// group in the Categorized view.
fn visual_lines(rows: &[PropertyRow], view: View) -> Vec<Line> {
    let mut lines = Vec::with_capacity(rows.len());
    let mut category: Option<&str> = None;
    for (index, row) in rows.iter().enumerate() {
        if view == View::Categorized && category != Some(row.category.as_str()) {
            category = Some(row.category.as_str());
            lines.push(Line::Header(row.category.clone()));
        }
        lines.push(Line::Row(index));
    }
    lines
}

/// The device-pixel layout of a grid of `area` size.
#[derive(Clone, Copy, Debug)]
struct Layout {
    width: i32,
    combo: Rect,
    tab_alpha: Rect,
    tab_cat: Rect,
    body_top: i32,
    body_bottom: i32,
    name_right: i32,
    row_h: i32,
    header_h: i32,
}

impl Layout {
    /// The layout for a grid whose local area has width `width` and height
    /// `height` at `dpi`.
    fn new(width: i32, height: i32, dpi: u32) -> Layout {
        let px = |dip: Dip| dip.to_px(dpi).value().max(1);
        let pad = PADDING.to_px(dpi).value().max(0);
        let combo_h = px(COMBO_HEIGHT);
        let tab_h = px(TAB_HEIGHT);
        let combo = Rect::new(pad, pad, (width - pad).max(pad), pad + combo_h);
        let tabs_top = combo.bottom + pad;
        let half = (width - 2 * pad) / 2;
        let tab_alpha = Rect::new(pad, tabs_top, pad + half, tabs_top + tab_h);
        let tab_cat = Rect::new(
            tab_alpha.right,
            tabs_top,
            (width - pad).max(pad),
            tabs_top + tab_h,
        );
        Layout {
            width,
            combo,
            tab_alpha,
            tab_cat,
            body_top: tabs_top + tab_h + pad,
            body_bottom: (height - pad).max(tabs_top + tab_h + pad),
            name_right: pad + ((width - 2 * pad) as f32 * 0.45) as i32,
            row_h: px(ROW_HEIGHT),
            header_h: px(HEADER_HEIGHT),
        }
    }

    /// The top of each visual line, starting at the body top less `scroll`.
    fn tops(&self, lines: &[Line], scroll: i32) -> Vec<i32> {
        let mut tops = Vec::with_capacity(lines.len());
        let mut y = self.body_top - scroll;
        for line in lines {
            tops.push(y);
            y += match line {
                Line::Header(_) => self.header_h,
                Line::Row(_) => self.row_h,
            };
        }
        tops
    }

    /// The value cell of `row_index`, if it is visible.
    fn value_cell<M: 'static>(&self, state: &GridState<M>, row_index: usize) -> Option<Rect> {
        let lines = visual_lines(&state.rows, state.view);
        let tops = self.tops(&lines, state.scroll);
        for (line, top) in lines.iter().zip(tops) {
            if let Line::Row(index) = line {
                if *index == row_index {
                    let bottom = top + self.row_h;
                    if bottom <= self.body_top || top >= self.body_bottom {
                        return None;
                    }
                    return Some(Rect::new(self.name_right, top, self.width, bottom));
                }
            }
        }
        None
    }

    /// The row under local `(x, y)`, if any.
    fn row_at<M: 'static>(&self, state: &GridState<M>, x: i32, y: i32) -> Option<usize> {
        if !(self.body_top..=self.body_bottom).contains(&y) {
            return None;
        }
        let lines = visual_lines(&state.rows, state.view);
        let tops = self.tops(&lines, state.scroll);
        for (line, top) in lines.iter().zip(tops) {
            let height = match line {
                Line::Header(_) => self.header_h,
                Line::Row(_) => self.row_h,
            };
            if (top..top + height).contains(&y) {
                let _ = x;
                return match line {
                    Line::Row(index) => Some(*index),
                    Line::Header(_) => None,
                };
            }
        }
        None
    }

    /// The dropdown rectangle while the object combo is open.
    fn dropdown(&self, count: usize, dpi: u32) -> Rect {
        let row = ROW_HEIGHT.to_px(dpi).value().max(1);
        Rect::new(
            self.combo.left,
            self.combo.bottom,
            self.combo.right,
            self.combo.bottom + row * count as i32,
        )
    }
}

/// The grid's live state.
struct GridState<M: 'static> {
    view: View,
    target: Target,
    rows: Vec<PropertyRow>,
    objects: Vec<(String, Target)>,
    selected_object: usize,
    objects_open: bool,
    scroll: i32,
    hover: Option<usize>,
    active_row: Option<usize>,
    error: Option<String>,
    editor: Option<Editor<M>>,
}

impl<M: 'static> GridState<M> {
    /// A grid showing the form, with no rows until the first sync.
    fn new() -> GridState<M> {
        GridState {
            view: View::default(),
            target: Target::Form,
            rows: Vec::new(),
            objects: Vec::new(),
            selected_object: 0,
            objects_open: false,
            scroll: 0,
            hover: None,
            active_row: None,
            error: None,
            editor: None,
        }
    }
}

/// The editor overlaid on the cell being edited.
enum Editor<M: 'static> {
    /// A painted single-line text/number editor.
    Text(TextEditor<M>),
    /// An enum drop-down.
    Choice(ComboBox<M>),
    /// A bool check box.
    Bool(CheckBox<M>),
}

impl<M: 'static> Editor<M> {
    /// The editor's node identity.
    fn id(&self) -> WidgetId {
        match self {
            Editor::Text(editor) => editor.id(),
            Editor::Choice(combo) => combo.id(),
            Editor::Bool(check) => check.id(),
        }
    }

    /// Gives the editor the keyboard focus, where its kind accepts one.
    fn focus(&self) {
        if let Editor::Text(editor) = self {
            editor.focus();
        }
    }
}

/// A painted single-line editor used for text and numeric cells.
///
/// xui's `Edit` has no public commit-on-Enter or focus-loss hook, and no way to
/// observe the keys it handles, so the grid paints its own field to get exact
/// Enter/Escape/blur semantics. (See the upstream note in the crate README.)
struct TextEditor<M: 'static> {
    control: Control<M>,
}

impl<M: 'static> TextEditor<M> {
    /// Creates an editor showing `initial`, committed through `wrap`.
    fn new(
        ui: &Ui<M>,
        bounds: Rect,
        initial: &str,
        wrap: Rc<dyn Fn(PropertyGridMsg) -> M>,
    ) -> Result<TextEditor<M>, BackendError> {
        let control = Control::new(ui, &NodeSpec::new(NodeKind::Custom, bounds).tab_stop())?;
        ui.set_cursor(control.id(), xui_core::backend::Cursor::Text);
        let text = Rc::new(RefCell::new(initial.to_owned()));
        let caret = Rc::new(Cell::new(initial.chars().count()));
        let focused = Rc::new(Cell::new(true));
        let id = control.id();

        {
            let text = Rc::clone(&text);
            let caret = Rc::clone(&caret);
            let focused = Rc::clone(&focused);
            let theme = ui.theme_handle();
            let ui = ui.clone();
            control.set_painter(Rc::new(move |canvas| {
                let theme = theme.get();
                let bounds = canvas.bounds();
                canvas.clear(theme.input_background);
                canvas.stroke_rect(bounds, theme.border_focused, 1.0);
                let pad = PADDING.to_px(canvas.dpi()).value().max(0);
                let inner = bounds.shrink(pad);
                let style = TextStyle::new(theme.text, TEXT_SIZE).middle();
                let value = text.borrow();
                canvas.draw_text(&value, inner, &style);
                if focused.get() {
                    let prefix: String = value.chars().take(caret.get()).collect();
                    let advance = ui.measure_text(&prefix, &style, canvas.dpi()).width;
                    let x = (inner.left + advance).min(inner.right);
                    canvas.draw_line(
                        Point::new(x, inner.top),
                        Point::new(x, inner.bottom),
                        theme.text,
                        1.0,
                    );
                }
            }));
        }

        {
            let text = Rc::clone(&text);
            let caret = Rc::clone(&caret);
            let focused = Rc::clone(&focused);
            let ui = ui.clone();
            control.on_events(move |event| {
                match event {
                    Event::SetFocus => focused.set(true),
                    Event::KillFocus => {
                        focused.set(false);
                        return Some(wrap(PropertyGridMsg::CommitText(text.borrow().clone())));
                    }
                    Event::MouseDown {
                        button: MouseButton::Left,
                        ..
                    } => {
                        focused.set(true);
                        ui.focus(id);
                    }
                    Event::Char(character) if focused.get() && !character.is_control() => {
                        let mut chars: Vec<char> = text.borrow().chars().collect();
                        let at = caret.get().min(chars.len());
                        chars.insert(at, *character);
                        caret.set(at + 1);
                        *text.borrow_mut() = chars.into_iter().collect();
                        ui.invalidate(id);
                    }
                    Event::KeyDown {
                        key,
                        repeat,
                        system,
                        ..
                    } if focused.get() && *repeat <= 1 && !*system => match *key {
                        Key::RETURN => {
                            return Some(wrap(PropertyGridMsg::CommitText(text.borrow().clone())));
                        }
                        Key::ESCAPE => return Some(wrap(PropertyGridMsg::Cancel)),
                        Key::BACK => {
                            let mut chars: Vec<char> = text.borrow().chars().collect();
                            let at = caret.get().min(chars.len());
                            if at > 0 {
                                chars.remove(at - 1);
                                caret.set(at - 1);
                                *text.borrow_mut() = chars.into_iter().collect();
                                ui.invalidate(id);
                            }
                        }
                        Key::DELETE => {
                            let mut chars: Vec<char> = text.borrow().chars().collect();
                            let at = caret.get().min(chars.len());
                            if at < chars.len() {
                                chars.remove(at);
                                *text.borrow_mut() = chars.into_iter().collect();
                                ui.invalidate(id);
                            }
                        }
                        Key::LEFT => caret.set(caret.get().saturating_sub(1)),
                        Key::RIGHT => {
                            let len = text.borrow().chars().count();
                            caret.set((caret.get() + 1).min(len));
                        }
                        Key::HOME => caret.set(0),
                        Key::END => caret.set(text.borrow().chars().count()),
                        _ => {}
                    },
                    _ => {}
                }
                let _ = &focused;
                None
            });
        }

        Ok(TextEditor { control })
    }

    /// The editor's node identity.
    fn id(&self) -> WidgetId {
        self.control.id()
    }

    /// Gives the editor the keyboard focus.
    fn focus(&self) {
        self.control.focus();
    }
}

/// The property grid widget.
pub struct PropertyGrid<M: 'static> {
    control: Control<M>,
    designer: Rc<RefCell<Designer<M>>>,
    catalog: Rc<Catalog>,
    state: Rc<RefCell<GridState<M>>>,
    wrap: Rc<dyn Fn(PropertyGridMsg) -> M>,
}

impl<M: 'static> PropertyGrid<M> {
    /// Builds a property grid along `bounds`, editing the object the
    /// `designer` has selected. `wrap` turns a [`PropertyGridMsg`] into the
    /// host's message type.
    pub fn new(
        ui: &Ui<M>,
        bounds: Rect,
        designer: Rc<RefCell<Designer<M>>>,
        catalog: Rc<Catalog>,
        wrap: impl Fn(PropertyGridMsg) -> M + 'static,
    ) -> Result<PropertyGrid<M>, PropertyGridError> {
        let control = Control::new(ui, &NodeSpec::new(NodeKind::Custom, bounds).tab_stop())?;
        let state = Rc::new(RefCell::new(GridState::new()));
        let wrap: Rc<dyn Fn(PropertyGridMsg) -> M> = Rc::new(wrap);

        let grid = PropertyGrid {
            control,
            designer: Rc::clone(&designer),
            catalog: Rc::clone(&catalog),
            state: Rc::clone(&state),
            wrap: Rc::clone(&wrap),
        };
        grid.sync(ui);

        // Follow the designer's selection: refresh the rows and repaint.
        let weak_state: Weak<RefCell<GridState<M>>> = Rc::downgrade(&state);
        let weak_designer: Weak<RefCell<Designer<M>>> = Rc::downgrade(&designer);
        let catalog = Rc::clone(&catalog);
        let ui_sink = ui.clone();
        let grid_id = grid.id();
        designer.borrow().add_selection_sink(move |_| {
            if let (Some(state), Some(designer)) = (weak_state.upgrade(), weak_designer.upgrade()) {
                refresh_rows(&mut state.borrow_mut(), &designer.borrow(), &catalog);
                ui_sink.invalidate(grid_id);
            }
        });

        {
            let state = Rc::clone(&state);
            let theme = ui.theme_handle();
            grid.control.set_painter(Rc::new(move |canvas| {
                paint(canvas, &state.borrow(), &theme.get())
            }));
        }
        {
            let state = Rc::clone(&state);
            let ui_events = ui.clone();
            let wrap = Rc::clone(&wrap);
            let id = grid.id();
            grid.control
                .on_events(move |event| grid_message(&ui_events, id, event, &state, &wrap));
        }

        Ok(grid)
    }

    /// The grid's node identity.
    pub fn id(&self) -> WidgetId {
        self.control.id()
    }

    /// Moves/resizes the grid.
    pub fn set_bounds(&self, bounds: Rect) {
        self.control.set_bounds(bounds);
    }

    /// The view currently shown.
    pub fn view(&self) -> View {
        self.state.borrow().view
    }

    /// The property rows currently shown.
    pub fn rows(&self) -> Vec<PropertyRow> {
        self.state.borrow().rows.clone()
    }

    /// The object the grid is showing.
    pub fn target(&self) -> Target {
        self.state.borrow().target.clone()
    }

    /// Re-reads the designer's selection and document into the grid.
    pub fn sync(&self, ui: &Ui<M>) {
        refresh_rows(
            &mut self.state.borrow_mut(),
            &self.designer.borrow(),
            &self.catalog,
        );
        ui.invalidate(self.id());
    }

    /// Handles one grid message. The host forwards its wrapped message here.
    pub fn update(&self, msg: PropertyGridMsg, ui: &Ui<M>) {
        match msg {
            PropertyGridMsg::SelectObject(target) => {
                self.designer.borrow().select_object(&target, ui);
                let mut state = self.state.borrow_mut();
                state.objects_open = false;
                drop(state);
                self.sync(ui);
            }
            PropertyGridMsg::SetView(view) => {
                {
                    let mut state = self.state.borrow_mut();
                    if state.view == view {
                        return;
                    }
                    state.view = view;
                }
                self.sync(ui);
            }
            PropertyGridMsg::ToggleObjects => {
                let mut state = self.state.borrow_mut();
                state.objects_open = !state.objects_open && !state.objects.is_empty();
                drop(state);
                ui.invalidate(self.id());
            }
            PropertyGridMsg::BeginEdit(index) => self.begin_edit(index, ui),
            PropertyGridMsg::CommitText(text) => {
                let parsed = {
                    let state = self.state.borrow();
                    state
                        .active_row
                        .and_then(|index| state.rows.get(index))
                        .and_then(|row| parse_value(&row.ty, &text))
                };
                match parsed {
                    Some(value) => self.apply(value, ui),
                    None => {
                        let mut state = self.state.borrow_mut();
                        state.error = Some("the value is not valid for this property".to_owned());
                        drop(state);
                        ui.invalidate(self.id());
                    }
                }
            }
            PropertyGridMsg::CommitChoice(index) => {
                let value = {
                    let state = self.state.borrow();
                    state
                        .active_row
                        .and_then(|row| state.rows.get(row))
                        .and_then(|row| row.ty.variants().get(index).cloned())
                        .map(Value::Enum)
                };
                if let Some(value) = value {
                    self.apply(value, ui);
                }
            }
            PropertyGridMsg::CommitBool(checked) => self.apply(Value::Bool(checked), ui),
            PropertyGridMsg::Cancel => {
                {
                    let mut state = self.state.borrow_mut();
                    state.editor = None;
                    state.error = None;
                }
                ui.invalidate(self.id());
            }
        }
    }

    /// Creates the editor for row `index`, if the row can be edited.
    fn begin_edit(&self, index: usize, ui: &Ui<M>) {
        self.close_editor(ui);
        let (row, scroll) = {
            let state = self.state.borrow();
            let Some(row) = state.rows.get(index).cloned() else {
                return;
            };
            (row, state.scroll)
        };
        if !row.editable() {
            return;
        }
        let bounds = ui.bounds(self.id());
        let layout = Layout::new(bounds.width(), bounds.height(), ui.dpi());
        let cell = layout.value_cell(&self.state.borrow(), index);
        let Some(cell) = cell else {
            return;
        };
        let cell = Rect::new(
            bounds.left + cell.left,
            bounds.top + cell.top,
            bounds.left + cell.right,
            bounds.top + cell.bottom,
        );

        let editor: Result<Option<Editor<M>>, BackendError> = match &row.ty {
            ValueType::Bool => CheckBox::new(ui, cell, "")
                .map(|check| {
                    if let Value::Bool(checked) = row.value {
                        check.set_checked(checked);
                    }
                    let wrap = Rc::clone(&self.wrap);
                    check.on_toggle(move |checked| Some(wrap(PropertyGridMsg::CommitBool(checked))))
                })
                .map(Editor::Bool)
                .map(Some),
            ValueType::Enum { variants } => {
                let items: Vec<&str> = variants.iter().map(String::as_str).collect();
                let wrap = Rc::clone(&self.wrap);
                let current = match &row.value {
                    Value::Enum(current) => Some(current.clone()),
                    _ => None,
                };
                let variants = variants.clone();
                ComboBox::new(ui, cell, &items)
                    .map(|combo| {
                        if let Some(current) = current {
                            if let Some(index) =
                                variants.iter().position(|variant| variant == &current)
                            {
                                combo.select(index);
                            }
                        }
                        combo.on_select(move |index| {
                            Some(wrap(PropertyGridMsg::CommitChoice(index)))
                        })
                    })
                    .map(Editor::Choice)
                    .map(Some)
            }
            ValueType::Text { .. } | ValueType::Int { .. } | ValueType::Float { .. } => {
                let wrap = Rc::clone(&self.wrap);
                TextEditor::new(ui, cell, &format_value(&row.value), wrap)
                    .map(Editor::Text)
                    .map(Some)
            }
            _ => Ok(None),
        };

        match editor {
            Ok(Some(editor)) => {
                let editor_id = editor.id();
                {
                    let mut state = self.state.borrow_mut();
                    state.editor = Some(editor);
                    state.active_row = Some(index);
                    state.error = None;
                    state.scroll = scroll;
                }
                if let Some(editor) = self.state.borrow().editor.as_ref() {
                    editor.focus();
                }
                ui.invalidate(editor_id);
                ui.invalidate(self.id());
            }
            Ok(None) => {}
            Err(error) => {
                let mut state = self.state.borrow_mut();
                state.error = Some(error.to_string());
                drop(state);
                ui.invalidate(self.id());
            }
        }
    }

    /// Drops the active editor without committing.
    fn close_editor(&self, ui: &Ui<M>) {
        if self.state.borrow_mut().editor.take().is_some() {
            ui.invalidate(self.id());
        }
    }

    /// Commits `value` to the active row through the designer and resyncs.
    fn apply(&self, value: Value, ui: &Ui<M>) {
        let edit = {
            let state = self.state.borrow();
            state.active_row.and_then(|index| {
                state
                    .rows
                    .get(index)
                    .map(|row| (state.target.clone(), row.name.clone()))
            })
        };
        let Some((target, name)) = edit else {
            return;
        };
        let result = self
            .designer
            .borrow()
            .set_property(&target, &name, value, ui);
        {
            let mut state = self.state.borrow_mut();
            state.editor = None;
            state.error = result.err().map(|error| error.to_string());
        }
        self.sync(ui);
    }
}

/// Recomputes the rows, object list and targets from the designer.
fn refresh_rows<M: 'static>(state: &mut GridState<M>, designer: &Designer<M>, catalog: &Catalog) {
    let target = target_for(&designer.selection());
    let doc = designer.doc();
    state.rows = property_rows(catalog, &doc, &target, state.view);
    state.objects = objects_of(&doc);
    state.selected_object = state
        .objects
        .iter()
        .position(|(_, object)| object == &target)
        .unwrap_or(0);
    state.target = target;
    state.objects_open = false;
    state.hover = None;
    state.active_row = None;
    state.error = None;
    state.editor = None;
}

/// The target a designer selection means.
fn target_for(selection: &crate::Selection) -> Target {
    match selection {
        crate::Selection::Form => Target::Form,
        crate::Selection::Nodes(names) => names
            .first()
            .map(|name| Target::Node(name.clone()))
            .unwrap_or(Target::Form),
    }
}

/// The form and its controls, in document order.
fn objects_of(doc: &FormDoc) -> Vec<(String, Target)> {
    let mut objects = vec![(doc.window.name.clone(), Target::Form)];
    objects.extend(
        doc.nodes
            .iter()
            .map(|node| (node.name.clone(), Target::Node(node.name.clone()))),
    );
    objects
}

/// Paints the combo, tabs, rows and any open dropdown.
fn paint<M: 'static>(canvas: &mut dyn Canvas, state: &GridState<M>, theme: &Theme) {
    let dpi = canvas.dpi();
    let bounds = canvas.bounds();
    let width = bounds.width();
    let height = bounds.height();
    canvas.clear(theme.surface);
    let layout = Layout::new(width, height, dpi);
    let body = Rect::new(0, layout.body_top, width, layout.body_bottom);

    paint_object_combo(canvas, state, theme, &layout);
    paint_tabs(canvas, state, theme, &layout);

    canvas.push_clip(body);
    paint_rows(canvas, state, theme, &layout);
    canvas.pop_clip();

    if state.objects_open {
        paint_dropdown(canvas, state, theme, &layout, dpi);
    }

    if let Some(error) = &state.error {
        let pad = PADDING.to_px(dpi).value().max(0);
        let rect = Rect::new(pad, layout.body_bottom, width - pad, height);
        let style = TextStyle::new(theme.accent, TEXT_SIZE).middle();
        canvas.draw_text(error, rect, &style);
    }
}

/// Paints the object combo, showing the selected object.
fn paint_object_combo<M: 'static>(
    canvas: &mut dyn Canvas,
    state: &GridState<M>,
    theme: &Theme,
    layout: &Layout,
) {
    let combo = layout.combo;
    canvas.fill_rect(combo, theme.input_background);
    canvas.stroke_rect(combo, theme.input_border, 1.0);
    let dpi = canvas.dpi();
    let pad = PADDING.to_px(dpi).value().max(0);
    let arrow = Dip(8.0).to_px(dpi).value().max(4);
    let label = state
        .objects
        .get(state.selected_object)
        .map(|(name, _)| name.as_str())
        .unwrap_or("(no selection)");
    let text = Rect::new(
        combo.left + pad,
        combo.top,
        combo.right - pad - arrow,
        combo.bottom,
    );
    let style = TextStyle::new(theme.text, TEXT_SIZE).middle();
    canvas.draw_text(label, text, &style);

    let (cx, cy) = (
        combo.right - pad - arrow / 2,
        combo.top + combo.height() / 2,
    );
    let half = (arrow / 2).max(2);
    let color = theme.text;
    canvas.draw_line(
        Point::new(cx - half, cy - half / 2),
        Point::new(cx, cy + half / 2),
        color,
        1.5,
    );
    canvas.draw_line(
        Point::new(cx, cy + half / 2),
        Point::new(cx + half, cy - half / 2),
        color,
        1.5,
    );
}

/// Paints the Alphabetic/Categorized tabs.
fn paint_tabs<M: 'static>(
    canvas: &mut dyn Canvas,
    state: &GridState<M>,
    theme: &Theme,
    layout: &Layout,
) {
    for (view, rect, label) in [
        (View::Alphabetic, layout.tab_alpha, "Alphabetic"),
        (View::Categorized, layout.tab_cat, "Categorized"),
    ] {
        let active = state.view == view;
        let color = if active {
            canvas.fill_rect(rect, theme.selection);
            theme.text
        } else {
            theme.text_disabled
        };
        let style = TextStyle::new(color, TEXT_SIZE).middle().centered();
        canvas.draw_text(label, rect, &style);
    }
}

/// Paints every property row, with category headers in the Categorized view.
fn paint_rows<M: 'static>(
    canvas: &mut dyn Canvas,
    state: &GridState<M>,
    theme: &Theme,
    layout: &Layout,
) {
    let dpi = canvas.dpi();
    let pad = PADDING.to_px(dpi).value().max(0);
    let lines = visual_lines(&state.rows, state.view);
    let tops = layout.tops(&lines, state.scroll);
    for (line, top) in lines.iter().zip(tops) {
        match line {
            Line::Header(category) => {
                let rect = Rect::new(0, top, layout.width, top + layout.header_h);
                canvas.fill_rect(rect, theme.background);
                let style = TextStyle::new(theme.text, TEXT_SIZE).middle().bold();
                canvas.draw_text(category, rect.shrink(pad), &style);
            }
            Line::Row(index) => {
                let Some(row) = state.rows.get(*index) else {
                    continue;
                };
                let rect = Rect::new(0, top, layout.width, top + layout.row_h);
                if state.active_row == Some(*index) {
                    canvas.fill_rect(rect, theme.selection);
                } else if state.hover == Some(*index) {
                    canvas.fill_rect(rect, theme.hover);
                }
                let name_rect = Rect::new(pad, top, layout.name_right - pad, rect.bottom);
                let value_rect = Rect::new(
                    layout.name_right + pad,
                    top,
                    layout.width - pad,
                    rect.bottom,
                );
                let color = if row.editable() {
                    theme.text
                } else {
                    theme.text_disabled
                };
                let style = TextStyle::new(color, TEXT_SIZE).middle();
                canvas.draw_text(&row.label, name_rect, &style);
                canvas.draw_text(&format_value(&row.value), value_rect, &style);
            }
        }
    }
}

/// Paints the open object dropdown.
fn paint_dropdown<M: 'static>(
    canvas: &mut dyn Canvas,
    state: &GridState<M>,
    theme: &Theme,
    layout: &Layout,
    dpi: u32,
) {
    let rect = layout.dropdown(state.objects.len(), dpi);
    canvas.push_clip(Rect::new(
        0,
        layout.combo.bottom,
        layout.width,
        layout.body_bottom,
    ));
    canvas.fill_rect(rect, theme.surface);
    canvas.stroke_rect(rect, theme.input_border, 1.0);
    let pad = PADDING.to_px(dpi).value().max(0);
    let row = ROW_HEIGHT.to_px(dpi).value().max(1);
    for (index, (name, _)) in state.objects.iter().enumerate() {
        let row_rect = Rect::new(
            rect.left,
            rect.top + row * index as i32,
            rect.right,
            rect.top + row * (index + 1) as i32,
        );
        if index == state.selected_object {
            canvas.fill_rect(row_rect, theme.selection);
        }
        let style = TextStyle::new(theme.text, TEXT_SIZE).middle();
        canvas.draw_text(name, row_rect.shrink(pad), &style);
    }
    canvas.pop_clip();
}

/// Turns one input event into a [`PropertyGridMsg`], wrapped into the host's
/// message type.
fn grid_message<M: 'static>(
    ui: &Ui<M>,
    id: WidgetId,
    event: &Event,
    shared: &Rc<RefCell<GridState<M>>>,
    wrap: &Rc<dyn Fn(PropertyGridMsg) -> M>,
) -> Option<M> {
    let bounds = ui.bounds(id);
    let layout = Layout::new(bounds.width(), bounds.height(), ui.dpi());
    match event {
        Event::MouseMove { x, y, .. } => {
            let row = layout.row_at(&shared.borrow(), *x, *y);
            let mut state = shared.borrow_mut();
            if state.hover != row {
                state.hover = row;
                drop(state);
                ui.invalidate(id);
            }
            None
        }
        Event::MouseLeave | Event::CaptureChanged => {
            let mut state = shared.borrow_mut();
            if state.hover.take().is_some() {
                drop(state);
                ui.invalidate(id);
            }
            None
        }
        Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            ..
        } => {
            ui.focus(id);
            // An open dropdown takes the click first.
            let dropdown_target = {
                let state = shared.borrow();
                if state.objects_open {
                    Some((
                        layout.dropdown(state.objects.len(), ui.dpi()),
                        state.objects.clone(),
                    ))
                } else {
                    None
                }
            };
            if let Some((dropdown, objects)) = dropdown_target {
                if dropdown.contains(Point::new(*x, *y)) {
                    let row = ROW_HEIGHT.to_px(ui.dpi()).value().max(1);
                    let index = ((*y - dropdown.top) / row).max(0) as usize;
                    if let Some((_, target)) = objects.get(index).cloned() {
                        return Some(wrap(PropertyGridMsg::SelectObject(target)));
                    }
                }
                shared.borrow_mut().objects_open = false;
                ui.invalidate(id);
                return None;
            }
            let point = Point::new(*x, *y);
            if layout.combo.contains(point) {
                return Some(wrap(PropertyGridMsg::ToggleObjects));
            }
            if layout.tab_alpha.contains(point) {
                return Some(wrap(PropertyGridMsg::SetView(View::Alphabetic)));
            }
            if layout.tab_cat.contains(point) {
                return Some(wrap(PropertyGridMsg::SetView(View::Categorized)));
            }
            let row = layout.row_at(&shared.borrow(), *x, *y)?;
            Some(wrap(PropertyGridMsg::BeginEdit(row)))
        }
        Event::KeyDown {
            key,
            repeat,
            system,
            ..
        } if *repeat <= 1 && !*system => {
            let active = shared.borrow().active_row;
            match *key {
                Key::ESCAPE => Some(wrap(PropertyGridMsg::Cancel)),
                Key::RETURN => active.map(|index| wrap(PropertyGridMsg::BeginEdit(index))),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xui_form::Node;

    fn catalog() -> Catalog {
        lazyrad_project::lazyrad_catalog()
    }

    fn doc() -> FormDoc {
        let mut doc = FormDoc::new("main_form");
        let mut button = Node::new("Button", "ok_button");
        button.set_prop("text", Value::Text("Go".into()));
        button.set_prop("enabled", Value::Bool(false));
        button.set_prop("left", Value::Int(16));
        doc.insert(button);
        doc
    }

    #[test]
    fn labels_are_title_cased_words() {
        assert_eq!(display_label("text"), "Text");
        assert_eq!(display_label("tab_index"), "Tab Index");
        assert_eq!(display_label("name"), "Name");
    }

    #[test]
    fn values_format_for_display() {
        assert_eq!(format_value(&Value::Bool(true)), "True");
        assert_eq!(format_value(&Value::Int(-3)), "-3");
        assert_eq!(format_value(&Value::Text("hi".into())), "hi");
        assert_eq!(
            format_value(&Value::List(vec!["a".into(), "b".into()])),
            "a, b"
        );
    }

    #[test]
    fn text_parses_against_the_schema() {
        let ty = ValueType::Text { multiline: false };
        assert_eq!(parse_value(&ty, "hello"), Some(Value::Text("hello".into())));
        let int = ValueType::Int {
            min: None,
            max: None,
        };
        assert_eq!(parse_value(&int, " 42 "), Some(Value::Int(42)));
        assert_eq!(parse_value(&int, "nope"), None);
        assert_eq!(
            parse_value(&ValueType::Bool, "true"),
            Some(Value::Bool(true))
        );
        assert_eq!(parse_value(&ValueType::Bool, "maybe"), None);
    }

    #[test]
    fn a_node_has_a_name_row_and_its_schema_properties() {
        let rows = property_rows(
            &catalog(),
            &doc(),
            &Target::Node("ok_button".into()),
            View::Alphabetic,
        );
        assert_eq!(rows.first().map(|row| row.label.as_str()), Some("(Name)"));
        assert!(rows.iter().any(|row| row.name == "text"));
        assert!(rows.iter().any(|row| row.name == "enabled"));
        assert!(
            rows.iter().all(|row| row.name != "items"),
            "a Button has no ComboBox properties"
        );
    }

    #[test]
    fn alphabetic_rows_are_sorted_by_label() {
        let rows = property_rows(
            &catalog(),
            &doc(),
            &Target::Node("ok_button".into()),
            View::Alphabetic,
        );
        let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        assert_eq!(labels, sorted);
    }

    #[test]
    fn categorized_rows_group_by_category() {
        let rows = property_rows(
            &catalog(),
            &doc(),
            &Target::Node("ok_button".into()),
            View::Categorized,
        );
        let ranks: Vec<u8> = rows
            .iter()
            .map(|row| category_rank(&row.category))
            .collect();
        let mut sorted = ranks.clone();
        sorted.sort_unstable();
        assert_eq!(ranks, sorted);
    }

    #[test]
    fn the_form_shows_its_window_properties() {
        let rows = property_rows(&catalog(), &doc(), &Target::Form, View::Alphabetic);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert!(names.contains(&"title"));
        assert!(names.contains(&"width"));
        assert!(names.contains(&"resizable"));
    }

    #[test]
    fn the_object_list_is_the_form_then_its_controls() {
        let objects = objects_of(&doc());
        assert_eq!(objects[0].0, "main_form");
        assert_eq!(objects[0].1, Target::Form);
        assert_eq!(objects[1].0, "ok_button");
        assert_eq!(objects[1].1, Target::Node("ok_button".into()));
    }

    #[test]
    fn a_selection_maps_to_a_target() {
        assert_eq!(target_for(&crate::Selection::Form), Target::Form);
        assert_eq!(
            target_for(&crate::Selection::Nodes(vec!["ok_button".into()])),
            Target::Node("ok_button".into())
        );
    }

    #[test]
    fn the_categorized_view_inserts_headers() {
        let rows = property_rows(
            &catalog(),
            &doc(),
            &Target::Node("ok_button".into()),
            View::Categorized,
        );
        let lines = visual_lines(&rows, View::Categorized);
        let headers = lines
            .iter()
            .filter(|line| matches!(line, Line::Header(_)))
            .count();
        let categories: std::collections::BTreeSet<&str> =
            rows.iter().map(|row| row.category.as_str()).collect();
        assert_eq!(headers, categories.len());
    }

    #[test]
    fn a_read_only_property_is_shown_but_not_editable() {
        let mut catalog = catalog();
        let mut spec = catalog.get("Button").expect("Button").clone();
        spec.properties.push(xui_form::PropertySpec {
            name: "shown".into(),
            ty: ValueType::Bool,
            default: Value::Bool(false),
            category: "Behavior".into(),
            description: String::new(),
            access: Access::ReadOnly,
        });
        catalog.register(spec);
        let rows = property_rows(
            &catalog,
            &doc(),
            &Target::Node("ok_button".into()),
            View::Alphabetic,
        );
        let row = rows.iter().find(|row| row.name == "shown").expect("row");
        assert!(!row.editable());
    }
}
