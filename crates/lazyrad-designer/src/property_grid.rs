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
//! The keyboard drives a *current row*, drawn like the hover highlight: Up,
//! Down, Home, End, PageUp and PageDown move it, Tab and Shift+Tab too while no
//! edit is open, and Return or F2 edits it. While the object dropdown is open
//! the same keys move its highlight, Return picks it and Escape closes it. An
//! open inline text editor keeps every key but Escape. The pure movement and
//! dropdown layout logic is in `grid_nav`.
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

use xui_core::Lucide;
use xui_core::Theme;
use xui_core::app::Ui;
use xui_core::backend::{BackendError, Canvas, Event, NodeKind, NodeSpec, TextStyle, WidgetId};
use xui_core::geometry::{Point, Rect};
use xui_core::icon::draw_icon;
use xui_core::message::{Key, MouseButton};
use xui_core::units::Dip;
use xui_core::widget::{CheckBox, ComboBox, Control};
use xui_form::{Access, Catalog, FormDoc, Value, ValueType};

use crate::grid_nav::{ObjectDropdown, RowMove, move_row};
use crate::local_paint::paint_local;
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
/// How many rows one mouse-wheel notch scrolls.
const WHEEL_ROWS: i32 = 3;
/// The wheel delta of one notch. Both backends report `WHEEL_DELTA` units:
/// Win32 passes them through and the canvas backend scales winit's line
/// deltas by it (and passes a touchpad's pixel deltas as-is).
const WHEEL_NOTCH: i32 = 120;

/// The scroll offset change for a wheel `delta`: [`WHEEL_ROWS`] rows per
/// notch, proportionally for partial (touchpad) deltas. A positive delta is
/// "away from the user", which shows earlier rows, so it scrolls up.
fn wheel_scroll(delta: i16, row_h: i32) -> i32 {
    -(i32::from(delta) * WHEEL_ROWS * row_h / WHEEL_NOTCH)
}

/// The number of whole list entries a wheel `delta` scrolls: [`WHEEL_ROWS`]
/// per notch, and at least one for any non-zero (touchpad) delta, since a
/// list cannot scroll part of an entry.
fn wheel_entries(delta: i16) -> i32 {
    let entries = -(i32::from(delta) * WHEEL_ROWS / WHEEL_NOTCH);
    if entries == 0 {
        -i32::from(delta.signum())
    } else {
        entries
    }
}
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
    /// The user pressed Escape: revert the active editor, or close the open
    /// object dropdown.
    Cancel,
    /// The user pressed a navigation key: move the current row, or the
    /// highlighted object while the dropdown is open.
    MoveRow(RowMove),
    /// The user pressed Return or F2: begin editing the current row, or pick
    /// the highlighted object while the dropdown is open.
    Activate,
    /// The user scrolled the open object dropdown by this many entries
    /// (positive shows later objects).
    ScrollObjects(i32),
    /// The user scrolled the rows by this many pixels (positive moves the
    /// content up, showing later rows).
    Scroll(i32),
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
    height: i32,
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
            height,
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

    /// The height of a visual line.
    fn line_height(&self, line: &Line) -> i32 {
        match line {
            Line::Header(_) => self.header_h,
            Line::Row(_) => self.row_h,
        }
    }

    /// The largest useful scroll offset: the content's overflow below the
    /// body, or zero when everything fits.
    fn max_scroll(&self, lines: &[Line]) -> i32 {
        let content: i32 = lines.iter().map(|line| self.line_height(line)).sum();
        (content - (self.body_bottom - self.body_top)).max(0)
    }

    /// `scroll` clamped to `0..=max_scroll`.
    fn clamp_scroll(&self, lines: &[Line], scroll: i32) -> i32 {
        scroll.clamp(0, self.max_scroll(lines))
    }

    /// The scroll offset that brings `row_index` fully into view, moving as
    /// little as possible from `scroll`. A row that opens a category also brings
    /// its header into view when scrolling up.
    fn scroll_to_row(&self, lines: &[Line], scroll: i32, row_index: usize) -> i32 {
        let mut top = 0;
        let mut header_top = None;
        for line in lines {
            let height = self.line_height(line);
            if matches!(line, Line::Row(index) if *index == row_index) {
                let body = self.body_bottom - self.body_top;
                // The header only when header and row fit the body together.
                let reveal = header_top
                    .filter(|header| top + height - header <= body)
                    .unwrap_or(top);
                let scroll = if reveal < scroll {
                    reveal
                } else if top + height > scroll + body {
                    top + height - body
                } else {
                    scroll
                };
                return self.clamp_scroll(lines, scroll);
            }
            header_top = matches!(line, Line::Header(_)).then_some(top);
            top += height;
        }
        self.clamp_scroll(lines, scroll)
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

    /// The rows a page key moves by: the whole rows that fit in the body, less
    /// one so consecutive pages overlap.
    fn page_rows(&self) -> usize {
        let fit = (self.body_bottom - self.body_top) / self.row_h;
        (fit - 1).max(1) as usize
    }

    /// The object dropdown while the combo is open, bounded by the grid's
    /// height and scrolled to entry `first`. Painting and hit-testing both use
    /// this, so they agree.
    fn dropdown(&self, count: usize, first: usize) -> ObjectDropdown {
        ObjectDropdown::new(self.combo, self.height, self.row_h, count, first)
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
    /// The row the keyboard is on, kept apart from the mouse hover. Only
    /// property rows (never headers) can be current.
    current_row: Option<usize>,
    /// The highlighted object while the dropdown is open.
    dropdown_highlight: usize,
    /// The first object the open dropdown shows.
    dropdown_first: usize,
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
            current_row: None,
            dropdown_highlight: 0,
            dropdown_first: 0,
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
                let stale_editor =
                    refresh_rows(&mut state.borrow_mut(), &designer.borrow(), &catalog);
                drop(stale_editor);
                ui_sink.invalidate(grid_id);
            }
        });

        {
            let state = Rc::clone(&state);
            let theme = ui.theme_handle();
            grid.control.set_painter(Rc::new(move |canvas| {
                paint_local(canvas, |canvas, area| {
                    paint(canvas, area, &state.borrow(), &theme.get());
                });
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

    /// The message of the last rejected edit, if it is still shown.
    pub fn error(&self) -> Option<String> {
        self.state.borrow().error.clone()
    }

    /// Re-reads the designer's selection and document into the grid.
    pub fn sync(&self, ui: &Ui<M>) {
        let stale_editor = refresh_rows(
            &mut self.state.borrow_mut(),
            &self.designer.borrow(),
            &self.catalog,
        );
        // Dropped only now, with the state released: it can deliver events.
        drop(stale_editor);
        self.clamp_scroll(ui);
        ui.invalidate(self.id());
    }

    /// The rows' current scroll offset in pixels (0 is the top).
    pub fn scroll_offset(&self) -> i32 {
        self.state.borrow().scroll
    }

    /// The current row (the one the keyboard is on), as an index into
    /// [`rows`](PropertyGrid::rows).
    pub fn current_row(&self) -> Option<usize> {
        self.state.borrow().current_row
    }

    /// Whether the object dropdown is open.
    pub fn objects_open(&self) -> bool {
        self.state.borrow().objects_open
    }

    /// The highlighted object's index while the dropdown is open.
    pub fn dropdown_highlight(&self) -> usize {
        self.state.borrow().dropdown_highlight
    }

    /// The index of the first object the open dropdown shows, at the grid's
    /// current size.
    pub fn dropdown_first(&self, ui: &Ui<M>) -> usize {
        let layout = self.layout(ui);
        let state = self.state.borrow();
        layout
            .dropdown(state.objects.len(), state.dropdown_first)
            .first()
    }

    /// The open dropdown's rectangle in the grid's local pixels, or `None`
    /// while it is closed. It never extends below the grid.
    pub fn dropdown_rect(&self, ui: &Ui<M>) -> Option<Rect> {
        let layout = self.layout(ui);
        let state = self.state.borrow();
        state.objects_open.then(|| {
            layout
                .dropdown(state.objects.len(), state.dropdown_first)
                .rect
        })
    }

    /// The object the open dropdown draws under the grid-local point
    /// `(x, y)`, or `None` when it is closed or the point misses its entries.
    /// A click uses exactly this test.
    pub fn object_at(&self, ui: &Ui<M>, x: i32, y: i32) -> Option<Target> {
        let layout = self.layout(ui);
        let state = self.state.borrow();
        if !state.objects_open {
            return None;
        }
        dropdown_pick(&state, &layout, Point::new(x, y))
    }

    /// The grid's layout at its current bounds.
    fn layout(&self, ui: &Ui<M>) -> Layout {
        let bounds = ui.bounds(self.id());
        Layout::new(bounds.width(), bounds.height(), ui.dpi())
    }

    /// Keeps the scroll offset inside the content after the rows changed.
    fn clamp_scroll(&self, ui: &Ui<M>) {
        let layout = self.layout(ui);
        let mut state = self.state.borrow_mut();
        let lines = visual_lines(&state.rows, state.view);
        state.scroll = layout.clamp_scroll(&lines, state.scroll);
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
                // The rows reorder; the sync keeps the current row by name and
                // it is brought back into view.
                self.sync(ui);
                self.reveal_current(ui);
            }
            PropertyGridMsg::ToggleObjects => {
                let layout = self.layout(ui);
                let mut state = self.state.borrow_mut();
                state.objects_open = !state.objects_open && !state.objects.is_empty();
                if state.objects_open {
                    // Open on the selected object, scrolled into view.
                    let count = state.objects.len();
                    state.dropdown_highlight = state.selected_object.min(count - 1);
                    state.dropdown_first = layout
                        .dropdown(count, 0)
                        .first_showing(state.dropdown_highlight);
                }
                drop(state);
                ui.invalidate(self.id());
            }
            PropertyGridMsg::MoveRow(mv) => self.move_row(mv, ui),
            PropertyGridMsg::Activate => self.activate(ui),
            PropertyGridMsg::ScrollObjects(delta) => {
                let layout = self.layout(ui);
                let mut state = self.state.borrow_mut();
                if state.objects_open {
                    let count = state.objects.len();
                    let first = state.dropdown_first as i64 + i64::from(delta);
                    let first = first.clamp(0, count as i64) as usize;
                    state.dropdown_first = layout.dropdown(count, first).first();
                }
                drop(state);
                ui.invalidate(self.id());
            }
            PropertyGridMsg::BeginEdit(index) => self.begin_edit(index, ui),
            PropertyGridMsg::Scroll(delta) => {
                // An open editor sits at its row's old position; close it
                // rather than leave it floating over another row.
                self.close_editor(ui);
                let layout = self.layout(ui);
                let mut state = self.state.borrow_mut();
                let lines = visual_lines(&state.rows, state.view);
                state.scroll = layout.clamp_scroll(&lines, state.scroll + delta);
                drop(state);
                ui.invalidate(self.id());
            }
            PropertyGridMsg::CommitText(text) => {
                // A commit can arrive after its editor closed (Enter, then the
                // focus loss the close causes); only a live text editor with an
                // active row commits.
                let live = {
                    let state = self.state.borrow();
                    matches!(state.editor, Some(Editor::Text(_))) && state.active_row.is_some()
                };
                if !live {
                    return;
                }
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
                // Take the editor out first: dropping it can deliver events
                // that reach the grid's state.
                let editor = {
                    let mut state = self.state.borrow_mut();
                    state.objects_open = false;
                    state.error = None;
                    state.active_row = None;
                    state.editor.take()
                };
                let had_editor = editor.is_some();
                drop(editor);
                if had_editor {
                    // The editor held the focus; hand it back for the keyboard.
                    ui.focus(self.id());
                }
                ui.invalidate(self.id());
            }
        }
    }

    /// Moves the current row by `mv`, or the highlighted object while the
    /// dropdown is open, scrolling the target into view.
    fn move_row(&self, mv: RowMove, ui: &Ui<M>) {
        let layout = self.layout(ui);
        let dropdown_open = self.state.borrow().objects_open;
        if dropdown_open {
            let mut state = self.state.borrow_mut();
            let count = state.objects.len();
            let view = layout.dropdown(count, state.dropdown_first);
            if let Some(next) = move_row(
                Some(state.dropdown_highlight),
                count,
                mv,
                view.visible().max(1),
            ) {
                state.dropdown_highlight = next;
                state.dropdown_first = layout.dropdown(count, view.first_showing(next)).first();
            }
            drop(state);
            ui.invalidate(self.id());
            return;
        }
        // An open editor sits at its row's position; close it, as scrolling does.
        self.close_editor(ui);
        let mut state = self.state.borrow_mut();
        state.active_row = None;
        let next = move_row(state.current_row, state.rows.len(), mv, layout.page_rows());
        state.current_row = next;
        if let Some(next) = next {
            let lines = visual_lines(&state.rows, state.view);
            state.scroll = layout.scroll_to_row(&lines, state.scroll, next);
        }
        drop(state);
        ui.invalidate(self.id());
    }

    /// Scrolls the current row into view, if there is one.
    fn reveal_current(&self, ui: &Ui<M>) {
        let layout = self.layout(ui);
        let mut state = self.state.borrow_mut();
        if let Some(current) = state.current_row {
            let lines = visual_lines(&state.rows, state.view);
            state.scroll = layout.scroll_to_row(&lines, state.scroll, current);
        }
        drop(state);
        ui.invalidate(self.id());
    }

    /// Begins editing the current row, or picks the highlighted object while
    /// the dropdown is open.
    fn activate(&self, ui: &Ui<M>) {
        let (open, object, current) = {
            let state = self.state.borrow();
            (
                state.objects_open,
                state
                    .objects
                    .get(state.dropdown_highlight)
                    .map(|(_, target)| target.clone()),
                state.current_row,
            )
        };
        if open {
            match object {
                Some(target) => self.update(PropertyGridMsg::SelectObject(target), ui),
                None => self.update(PropertyGridMsg::Cancel, ui),
            }
        } else if let Some(index) = current {
            self.begin_edit(index, ui);
        }
    }

    /// Creates the editor for row `index`, if the row can be edited.
    fn begin_edit(&self, index: usize, ui: &Ui<M>) {
        self.close_editor(ui);
        {
            // The editor goes over the row's value cell, which must be visible.
            let layout = self.layout(ui);
            let mut state = self.state.borrow_mut();
            let lines = visual_lines(&state.rows, state.view);
            state.scroll = layout.scroll_to_row(&lines, state.scroll, index);
        }
        let (row, scroll) = {
            let mut state = self.state.borrow_mut();
            let Some(row) = state.rows.get(index).cloned() else {
                return;
            };
            // Clicking or activating a row makes it the current one, editable
            // or not.
            state.current_row = Some(index);
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
        // Take the editor out first: dropping it can deliver events that reach
        // the grid's state, which must not be borrowed then.
        let editor = self.state.borrow_mut().editor.take();
        if editor.is_some() {
            drop(editor);
            // The editor usually held the keyboard focus, and neither backend
            // hands it back when its node goes: take it so navigation keys keep
            // reaching the grid. (Every caller is a grid interaction.)
            ui.focus(self.id());
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
        // Drop the editor outside the borrow: dropping it can deliver events.
        let editor = self.state.borrow_mut().editor.take();
        drop(editor);
        // The editor held the focus; hand it back so the keyboard keeps working.
        ui.focus(self.id());
        // Refresh first: `sync` rebuilds the rows, which clears the error, so a
        // rejected commit's message is set afterwards and stays visible.
        self.sync(ui);
        if let Err(error) = result {
            self.state.borrow_mut().error = Some(error.to_string());
            ui.invalidate(self.id());
        }
    }
}

/// Recomputes the rows, object list and targets from the designer.
/// Returns the editor the refresh displaced, for the caller to drop once the
/// state is no longer borrowed.
fn refresh_rows<M: 'static>(
    state: &mut GridState<M>,
    designer: &Designer<M>,
    catalog: &Catalog,
) -> Option<Editor<M>> {
    let target = target_for(&designer.selection());
    // The current row survives by property name, unless the object changed.
    let current = if target == state.target {
        state
            .current_row
            .and_then(|index| state.rows.get(index))
            .map(|row| row.name.clone())
    } else {
        // Another object's rows start from the top.
        state.scroll = 0;
        None
    };
    let doc = designer.doc();
    state.rows = property_rows(catalog, &doc, &target, state.view);
    state.objects = objects_of(&doc);
    state.selected_object = state
        .objects
        .iter()
        .position(|(_, object)| object == &target)
        .unwrap_or(0);
    state.target = target;
    state.current_row = current.and_then(|name| state.rows.iter().position(|row| row.name == name));
    state.objects_open = false;
    state.dropdown_highlight = state.selected_object;
    state.dropdown_first = 0;
    state.hover = None;
    state.active_row = None;
    state.error = None;
    state.editor.take()
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

/// Paints the combo, tabs, rows and any open dropdown, over the grid's `area`
/// in its own coordinates.
fn paint<M: 'static>(canvas: &mut dyn Canvas, area: Rect, state: &GridState<M>, theme: &Theme) {
    let dpi = canvas.dpi();
    let width = area.width();
    let height = area.height();
    canvas.fill_rect(area, theme.surface);
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

    let side = arrow.max(4);
    let icon_rect = Rect::new(
        combo.right - pad - side,
        combo.top + (combo.height() - side) / 2,
        combo.right - pad,
        combo.top + (combo.height() + side) / 2,
    );
    draw_icon(canvas, Lucide::ChevronDown, icon_rect, theme.text, dpi);
}

/// Paints the Alphabetic/Categorized tabs, each with its Lucide icon.
fn paint_tabs<M: 'static>(
    canvas: &mut dyn Canvas,
    state: &GridState<M>,
    theme: &Theme,
    layout: &Layout,
) {
    let dpi = canvas.dpi();
    let pad = PADDING.to_px(dpi).value().max(0);
    let gap = Dip(4.0).to_px(dpi).value().max(1);
    let icon = Dip(16.0).to_px(dpi).value().max(1);
    for (view, rect, label, glyph) in [
        (
            View::Alphabetic,
            layout.tab_alpha,
            "Alphabetic",
            Lucide::ArrowDownAZ,
        ),
        (
            View::Categorized,
            layout.tab_cat,
            "Categorized",
            Lucide::ListTree,
        ),
    ] {
        let active = state.view == view;
        let color = if active {
            canvas.fill_rect(rect, theme.selection);
            theme.text
        } else {
            theme.text_disabled
        };
        let icon_rect = Rect::new(
            rect.left + pad,
            rect.top + (rect.height() - icon) / 2,
            rect.left + pad + icon,
            rect.top + (rect.height() + icon) / 2,
        );
        draw_icon(canvas, glyph, icon_rect, color, dpi);
        let text = Rect::new(icon_rect.right + gap, rect.top, rect.right, rect.bottom);
        let style = TextStyle::new(color, TEXT_SIZE).middle().centered();
        canvas.draw_text(label, text, &style);
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
                // The category is always expanded; the chevron points down.
                let icon = Dip(14.0).to_px(dpi).value().max(1);
                let icon_rect = Rect::new(
                    pad,
                    rect.top + (rect.height() - icon) / 2,
                    pad + icon,
                    rect.top + (rect.height() + icon) / 2,
                );
                draw_icon(canvas, Lucide::ChevronDown, icon_rect, theme.text, dpi);
                let text = Rect::new(
                    icon_rect.right + pad,
                    rect.top,
                    rect.right - pad,
                    rect.bottom,
                );
                let style = TextStyle::new(theme.text, TEXT_SIZE).middle().bold();
                canvas.draw_text(category, text, &style);
            }
            Line::Row(index) => {
                let Some(row) = state.rows.get(*index) else {
                    continue;
                };
                let rect = Rect::new(0, top, layout.width, top + layout.row_h);
                if state.active_row == Some(*index) {
                    canvas.fill_rect(rect, theme.selection);
                } else if state.current_row == Some(*index) || state.hover == Some(*index) {
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
    let dropdown = layout.dropdown(state.objects.len(), state.dropdown_first);
    let rect = dropdown.rect;
    canvas.push_clip(rect);
    canvas.fill_rect(rect, theme.surface);
    canvas.stroke_rect(rect, theme.input_border, 1.0);
    let pad = PADDING.to_px(dpi).value().max(0);
    for index in dropdown.range() {
        let Some((name, _)) = state.objects.get(index) else {
            continue;
        };
        let Some(row_rect) = dropdown.row_rect(index) else {
            continue;
        };
        // The keyboard highlight, and the object the grid is showing.
        if index == state.dropdown_highlight {
            canvas.fill_rect(row_rect, theme.selection);
        } else if index == state.selected_object {
            canvas.fill_rect(row_rect, theme.hover);
        }
        let style = TextStyle::new(theme.text, TEXT_SIZE).middle();
        canvas.draw_text(name, row_rect.shrink(pad), &style);
    }
    canvas.pop_clip();
}

/// The object the open dropdown shows under local `point`, if any. Hit-testing
/// goes through the same clamped layout [`paint_dropdown`] draws.
fn dropdown_pick<M: 'static>(
    state: &GridState<M>,
    layout: &Layout,
    point: Point,
) -> Option<Target> {
    let index = layout
        .dropdown(state.objects.len(), state.dropdown_first)
        .index_at(point)?;
    state.objects.get(index).map(|(_, target)| target.clone())
}

/// Which part of the grid owns the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyOwner {
    /// The rows: navigation keys move the current row.
    Grid,
    /// The open object dropdown: navigation keys move its highlight.
    Dropdown,
    /// An inline text editor, which has its own use for the navigation keys.
    TextEditor,
}

/// The message a key press means, or `None` to leave it unconsumed.
///
/// `editing` is whether any inline editor is open, `current` the current row
/// and `rows` how many rows there are. A held key repeats the movement keys,
/// but Return, F2 and Escape act once per press. While a text editor owns the
/// keyboard, only Escape (which reverts the edit either way) is the grid's;
/// Tab moves rows only when no edit is open, and lets focus move on at the ends.
fn key_message(
    key: Key,
    shift: bool,
    repeat: u16,
    owner: KeyOwner,
    editing: bool,
    current: Option<usize>,
    rows: usize,
) -> Option<PropertyGridMsg> {
    let once = repeat <= 1;
    match key {
        Key::ESCAPE if once => return Some(PropertyGridMsg::Cancel),
        _ if owner == KeyOwner::TextEditor => return None,
        Key::RETURN | Key::F2 if once => return Some(PropertyGridMsg::Activate),
        _ => {}
    }
    let mv = match key {
        Key::UP => RowMove::Up,
        Key::DOWN => RowMove::Down,
        Key::HOME => RowMove::Home,
        Key::END => RowMove::End,
        Key::PAGE_UP => RowMove::PageUp,
        Key::PAGE_DOWN => RowMove::PageDown,
        Key::TAB if owner == KeyOwner::Grid && !editing => {
            let mv = if shift { RowMove::Up } else { RowMove::Down };
            // At either end, let Tab move the focus out of the grid.
            if move_row(current, rows, mv, 1) == current {
                return None;
            }
            mv
        }
        _ => return None,
    };
    Some(PropertyGridMsg::MoveRow(mv))
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
            let dropdown_click = {
                let state = shared.borrow();
                state
                    .objects_open
                    .then(|| dropdown_pick(&state, &layout, Point::new(*x, *y)))
            };
            if let Some(picked) = dropdown_click {
                if let Some(target) = picked {
                    return Some(wrap(PropertyGridMsg::SelectObject(target)));
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
        Event::MouseWheel {
            delta,
            x,
            y,
            horizontal: false,
            ..
        } => {
            // The object dropdown overlays the rows: the wheel over it scrolls
            // its list, and elsewhere does nothing while it is open.
            let dropdown = {
                let state = shared.borrow();
                state
                    .objects_open
                    .then(|| layout.dropdown(state.objects.len(), state.dropdown_first))
            };
            if let Some(dropdown) = dropdown {
                return dropdown
                    .rect
                    .contains(Point::new(*x, *y))
                    .then(|| wrap(PropertyGridMsg::ScrollObjects(wheel_entries(*delta))));
            }
            Some(wrap(PropertyGridMsg::Scroll(wheel_scroll(
                *delta,
                layout.row_h,
            ))))
        }
        Event::KeyDown {
            key,
            modifiers,
            repeat,
            system,
        } if !*system => {
            let msg = {
                let state = shared.borrow();
                let owner = if state.objects_open {
                    KeyOwner::Dropdown
                } else if matches!(state.editor, Some(Editor::Text(_))) {
                    KeyOwner::TextEditor
                } else {
                    KeyOwner::Grid
                };
                key_message(
                    *key,
                    modifiers.shift,
                    *repeat,
                    owner,
                    state.editor.is_some(),
                    state.current_row,
                    state.rows.len(),
                )
            };
            msg.map(|msg| wrap(msg))
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

    fn press(key: Key, owner: KeyOwner) -> Option<PropertyGridMsg> {
        key_message(key, false, 1, owner, false, Some(1), 5)
    }

    #[test]
    fn navigation_keys_map_to_row_moves() {
        for (key, mv) in [
            (Key::UP, RowMove::Up),
            (Key::DOWN, RowMove::Down),
            (Key::HOME, RowMove::Home),
            (Key::END, RowMove::End),
            (Key::PAGE_UP, RowMove::PageUp),
            (Key::PAGE_DOWN, RowMove::PageDown),
            (Key::TAB, RowMove::Down),
        ] {
            assert_eq!(
                press(key, KeyOwner::Grid),
                Some(PropertyGridMsg::MoveRow(mv))
            );
        }
        assert_eq!(
            key_message(Key::TAB, true, 1, KeyOwner::Grid, false, Some(1), 5),
            Some(PropertyGridMsg::MoveRow(RowMove::Up))
        );
        // The open dropdown takes the same movement keys.
        assert_eq!(
            press(Key::DOWN, KeyOwner::Dropdown),
            Some(PropertyGridMsg::MoveRow(RowMove::Down))
        );
    }

    #[test]
    fn return_and_f2_activate_once_per_press() {
        assert_eq!(
            press(Key::RETURN, KeyOwner::Grid),
            Some(PropertyGridMsg::Activate)
        );
        assert_eq!(
            press(Key::F2, KeyOwner::Dropdown),
            Some(PropertyGridMsg::Activate)
        );
        assert_eq!(
            key_message(Key::RETURN, false, 2, KeyOwner::Grid, false, Some(1), 5),
            None
        );
        // Held movement keys repeat.
        assert_eq!(
            key_message(Key::DOWN, false, 5, KeyOwner::Grid, false, Some(1), 5),
            Some(PropertyGridMsg::MoveRow(RowMove::Down))
        );
    }

    #[test]
    fn a_text_editor_keeps_its_keys() {
        for key in [
            Key::UP,
            Key::DOWN,
            Key::HOME,
            Key::END,
            Key::PAGE_UP,
            Key::PAGE_DOWN,
            Key::TAB,
            Key::RETURN,
            Key::F2,
        ] {
            assert_eq!(press(key, KeyOwner::TextEditor), None, "{key:?}");
        }
        // Escape reverts the edit whoever sees it.
        assert_eq!(
            press(Key::ESCAPE, KeyOwner::TextEditor),
            Some(PropertyGridMsg::Cancel)
        );
    }

    #[test]
    fn tab_only_moves_rows_when_idle_and_lets_focus_leave_at_the_ends() {
        let tab = |shift, owner, editing, current| {
            key_message(Key::TAB, shift, 1, owner, editing, current, 5)
        };
        assert_eq!(tab(false, KeyOwner::Grid, true, Some(1)), None, "editing");
        assert_eq!(tab(false, KeyOwner::Dropdown, false, Some(1)), None);
        assert_eq!(tab(false, KeyOwner::Grid, false, Some(4)), None, "last row");
        assert_eq!(tab(true, KeyOwner::Grid, false, Some(0)), None, "first row");
        assert!(tab(false, KeyOwner::Grid, false, None).is_some());
        assert_eq!(
            key_message(Key::TAB, false, 1, KeyOwner::Grid, false, None, 0),
            None,
            "no rows"
        );
    }

    #[test]
    fn a_dropdown_click_picks_the_entry_that_is_drawn() {
        let mut state = GridState::<()>::new();
        state.objects = (0..10)
            .map(|index| {
                (
                    format!("object_{index}"),
                    Target::Node(format!("object_{index}")),
                )
            })
            .collect();
        state.objects_open = true;
        let layout = Layout::new(200, 110, 96);
        for first in [0, 3, 99] {
            state.dropdown_first = first;
            let dropdown = layout.dropdown(state.objects.len(), state.dropdown_first);
            assert!(dropdown.rect.bottom <= 110);
            for index in dropdown.range() {
                let cell = dropdown.row_rect(index).expect("drawn");
                let hit = dropdown_pick(&state, &layout, Point::new(cell.left + 1, cell.top + 1));
                assert_eq!(hit, Some(state.objects[index].1.clone()));
            }
            let below = Point::new(dropdown.rect.left + 1, dropdown.rect.bottom + 1);
            assert_eq!(dropdown_pick(&state, &layout, below), None);
        }
    }
}

#[cfg(test)]
mod wheel_tests {
    use super::wheel_scroll;

    #[test]
    fn one_notch_scrolls_three_rows() {
        assert_eq!(
            wheel_scroll(-120, 22),
            66,
            "towards the user shows later rows"
        );
        assert_eq!(wheel_scroll(120, 22), -66);
        assert_eq!(wheel_scroll(-240, 22), 132, "two notches");
    }

    #[test]
    fn a_notch_scrolls_three_list_entries_and_a_nudge_scrolls_one() {
        use super::wheel_entries;
        assert_eq!(wheel_entries(-120), 3);
        assert_eq!(wheel_entries(120), -3);
        assert_eq!(wheel_entries(-10), 1, "a small touchpad delta still moves");
        assert_eq!(wheel_entries(10), -1);
        assert_eq!(wheel_entries(0), 0);
    }

    #[test]
    fn a_partial_delta_scrolls_proportionally() {
        // A touchpad reports pixel-sized deltas; a third of a notch is a row.
        assert_eq!(wheel_scroll(-40, 22), 22);
        assert_eq!(wheel_scroll(0, 22), 0);
    }
}
