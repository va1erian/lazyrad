#![forbid(unsafe_code)]

//! The designer's model and gesture engine, with no xui types.
//!
//! [`Surface`] owns an [`xui_form::FormDoc`] and turns design-unit input into
//! model edits. Every gesture is model-first: it edits the document through
//! [`Surface::pointer_down`], [`Surface::pointer_move`] and friends, and records
//! one snapshot in the [`History`] when the gesture ends. The xui widget in
//! [`crate::Designer`] owns a `Surface`, converts its pixel input to design
//! units, and re-applies the model to the live widgets. Keeping the pure logic
//! here means selection, snapping, undo/redo and clipboard behaviour are all
//! unit-tested without a window.

use std::collections::BTreeMap;
use std::rc::Rc;

use xui_form::{Catalog, FormDoc, Node, Value};

use crate::geometry::{DesignRect, Handle, handle_at, resize, resize_form, snap};
use crate::history::History;

/// The default grid spacing in design units.
pub const DEFAULT_GRID: i64 = 8;

/// How close (in design units) the pointer must be to a handle centre to grab
/// it.
pub const HANDLE_TOLERANCE: i64 = 4;

/// The smallest node a resize may leave behind, in design units.
const MIN_SIZE: i64 = 1;
/// The kinds whose `text` is a caption, which a new control fills with its own
/// name.
const CAPTIONED_KINDS: [&str; 4] = ["Button", "Label", "CheckBox", "GroupBox"];

/// A drag shorter than this in either axis is treated as a click, so the tool
/// drops the control at its schema default size instead of a degenerate one.
const MIN_DRAW: i64 = 2;

/// What the designer has selected: the form itself, or one or more controls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    /// The form/window is selected; its handles resize the client area.
    #[default]
    Form,
    /// Controls are selected, in the order they were added to the selection.
    Nodes(Vec<String>),
}

impl Selection {
    /// The selected node names, or an empty slice when the form is selected.
    pub fn nodes(&self) -> &[String] {
        match self {
            Selection::Form => &[],
            Selection::Nodes(names) => names,
        }
    }

    /// Whether the form itself is selected.
    pub fn is_form(&self) -> bool {
        matches!(self, Selection::Form)
    }

    /// Whether `name` is among the selected controls.
    pub fn contains(&self, name: &str) -> bool {
        self.nodes().iter().any(|selected| selected == name)
    }
}

/// A logical key the designer understands, mapped from xui's virtual keys by
/// [`crate::Designer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyInput {
    /// Nudge left.
    Left,
    /// Nudge right.
    Right,
    /// Nudge up.
    Up,
    /// Nudge down.
    Down,
    /// Delete the selection.
    Delete,
    /// Delete the selection (the keyboard's backspace).
    Backspace,
    /// Select the form.
    Escape,
    /// Copy the selection (Ctrl+C).
    Copy,
    /// Cut the selection: copy it, then delete it (Ctrl+X).
    Cut,
    /// Paste the clipboard (Ctrl+V).
    Paste,
    /// Duplicate the selection (Ctrl+D).
    Duplicate,
    /// Undo (Ctrl+Z).
    Undo,
    /// Redo (Ctrl+Y, or Ctrl+Shift+Z).
    Redo,
    /// Select every control (Ctrl+A).
    SelectAll,
}

/// A key press and the modifiers held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPress {
    /// The logical key.
    pub key: KeyInput,
    /// Whether Ctrl was held.
    pub ctrl: bool,
    /// Whether Shift was held.
    pub shift: bool,
}

impl KeyPress {
    /// A key press with no modifiers.
    pub const fn new(key: KeyInput) -> KeyPress {
        KeyPress {
            key,
            ctrl: false,
            shift: false,
        }
    }

    /// The same press with Ctrl held.
    pub const fn ctrl(key: KeyInput) -> KeyPress {
        KeyPress {
            key,
            ctrl: true,
            shift: false,
        }
    }
}

/// Which pointer the designer wants over its surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorHint {
    /// The platform default.
    #[default]
    Default,
    /// An east-west resize arrow.
    SizeHorizontal,
    /// A north-south resize arrow.
    SizeVertical,
}

/// What a design-unit edit changed, so [`crate::Designer`] knows how to refresh
/// the live widgets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Change {
    /// A node's position or size changed; re-apply geometry to the live widgets.
    pub geometry: bool,
    /// Nodes were added or removed; rebuild the live form.
    pub structure: bool,
    /// The selection changed.
    pub selection: bool,
    /// A non-geometry property changed; push the values into the live widgets.
    pub property: bool,
}

impl Change {
    /// No change at all.
    pub const NONE: Change = Change {
        geometry: false,
        structure: false,
        selection: false,
        property: false,
    };

    /// A geometry-only change.
    pub const GEOMETRY: Change = Change {
        geometry: true,
        ..Change::NONE
    };

    /// A non-geometry property change.
    pub const PROPERTY: Change = Change {
        property: true,
        ..Change::NONE
    };

    /// A selection-only change.
    pub const SELECTION: Change = Change {
        selection: true,
        ..Change::NONE
    };

    /// A structural change (which also refreshes the selection).
    pub const STRUCTURE: Change = Change {
        structure: true,
        selection: true,
        ..Change::NONE
    };

    /// Whether anything changed.
    pub const fn any(self) -> bool {
        self.geometry || self.structure || self.selection || self.property
    }
}

/// Which object a property command targets: the form itself, or a named node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The form/window.
    Form,
    /// The control named by the string.
    Node(String),
}

impl Target {
    /// The node name, or `None` for the form.
    pub fn node(&self) -> Option<&str> {
        match self {
            Target::Form => None,
            Target::Node(name) => Some(name),
        }
    }
}

/// Why a [`Surface::set_property`] command was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PropertyError {
    /// The target node does not exist.
    #[error("no such object")]
    UnknownObject,
    /// The object has no such property (or it is hidden in the designer).
    #[error("`{0}` is not an editable property")]
    UnknownProperty(String),
    /// The property is not writable in design mode.
    #[error("`{0}` is read-only in the designer")]
    ReadOnly(String),
    /// The value does not fit the property's schema.
    #[error("the value is not valid for `{0}`")]
    InvalidValue(String),
    /// A control name is not a valid identifier.
    #[error("`{0}` is not a valid control name")]
    InvalidName(String),
    /// A control name is already taken.
    #[error("`{0}` is already used by another control")]
    DuplicateName(String),
}

/// Whether `name` is a valid control identifier: a letter or `_` followed by
/// letters, digits or `_`.
pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether `c` may appear inside a Rhai identifier.
fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Rewrites event-handler definitions in a form's `.rhai` source so a control
/// rename keeps them bound: `fn <old>_<event>(` becomes `fn <new>_<event>(` for
/// each of the control's `events` (snake_case, see [`handler_events`]).
///
/// Only function declarations are touched, and only for the given events:
/// control names are snake_case and may contain `_`, so renaming `ok` must not
/// touch `fn ok_button_click()`, which belongs to a control named `ok_button`.
/// This is the textual rename the property grid's `(Name)` edit requests from
/// the host.
pub fn rename_handlers(source: &str, old: &str, new: &str, events: &[String]) -> String {
    if old.is_empty() || old == new {
        return source.to_owned();
    }
    let handlers: Vec<String> = events
        .iter()
        .map(|event| format!("{old}_{event}"))
        .collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < source.len() {
        let bytes = source.as_bytes();
        // Look for the `fn` keyword at a word boundary.
        let is_fn = bytes[i] == b'f'
            && bytes.get(i + 1) == Some(&b'n')
            && (i == 0 || !is_ident_char(bytes[i - 1] as char))
            && bytes.get(i + 2).is_none_or(|b| !is_ident_char(*b as char));
        if is_fn {
            let mut j = i + 2;
            while j < source.len() && (bytes[j] as char).is_whitespace() {
                j += 1;
            }
            let matched = handlers.iter().find(|handler| {
                source[j..].starts_with(handler.as_str())
                    && source[j + handler.len()..]
                        .chars()
                        .next()
                        .is_none_or(|next| !is_ident_char(next))
            });
            if let Some(handler) = matched {
                out.push_str(&source[i..j]);
                out.push_str(new);
                out.push_str(&handler[old.len()..]);
                i = j + handler.len();
                continue;
            }
        }
        let ch = source[i..].chars().next().expect("in bounds");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// The result of one gesture: what changed and which cursor to show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// What the edit changed.
    pub change: Change,
    /// The pointer to show over the surface.
    pub cursor: CursorHint,
}

impl Outcome {
    /// An outcome that changed nothing.
    pub const fn none() -> Outcome {
        Outcome {
            change: Change::NONE,
            cursor: CursorHint::Default,
        }
    }

    /// An outcome with `change` and the default cursor.
    pub const fn changed(change: Change) -> Outcome {
        Outcome {
            change,
            cursor: CursorHint::Default,
        }
    }

    /// The same outcome with `cursor`.
    pub const fn cursor(mut self, cursor: CursorHint) -> Outcome {
        self.cursor = cursor;
        self
    }
}

/// The gesture currently in progress.
#[derive(Clone, Debug)]
enum Drag {
    /// No gesture.
    None,
    /// Moving the selected nodes; `starts` holds each moved node's original
    /// `left`/`top`.
    Move {
        origin: (i64, i64),
        starts: Vec<(String, i64, i64)>,
    },
    /// Resizing a single node through `handle`.
    ResizeNode {
        name: String,
        handle: Handle,
        start_rect: DesignRect,
        origin: (i64, i64),
    },
    /// Resizing the form's client area.
    ResizeForm {
        handle: Handle,
        start_rect: DesignRect,
    },
    /// Rubber-band selection from `origin` to the live pointer position.
    Marquee { origin: (i64, i64) },
    /// Drawing a new control of `kind` from `origin` to the live pointer
    /// position (the click-then-drag creation gesture).
    Create { kind: String, origin: (i64, i64) },
}

/// The designer's document, selection, clipboard, history and live gesture.
#[derive(Clone)]
pub struct Surface {
    doc: FormDoc,
    catalog: Rc<Catalog>,
    selection: Selection,
    grid: i64,
    history: History<FormDoc>,
    clipboard: Vec<Node>,
    drag: Drag,
    preview: Option<DesignRect>,
    marquee: Option<DesignRect>,
    /// The active creation tool: a catalog kind, or `None` for the pointer.
    tool: Option<String>,
}

impl Surface {
    /// A surface editing `doc` with catalog `catalog` and a `grid` snap spacing.
    pub fn new(doc: FormDoc, catalog: Rc<Catalog>, grid: i64) -> Surface {
        let grid = grid.max(1);
        Surface {
            history: History::new(doc.clone()),
            doc,
            catalog,
            selection: Selection::Form,
            grid,
            clipboard: Vec::new(),
            drag: Drag::None,
            preview: None,
            marquee: None,
            tool: None,
        }
    }

    /// The document being edited.
    pub fn doc(&self) -> &FormDoc {
        &self.doc
    }

    /// Replaces the document and resets the history and selection.
    pub fn set_doc(&mut self, doc: FormDoc) {
        self.history.reset(doc.clone());
        self.doc = doc;
        self.selection = Selection::Form;
        self.drag = Drag::None;
        self.preview = None;
        self.marquee = None;
    }

    /// The current selection.
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// The grid spacing in design units.
    pub fn grid(&self) -> i64 {
        self.grid
    }

    /// Sets the grid spacing; a value below one is clamped to one.
    pub fn set_grid(&mut self, grid: i64) {
        self.grid = grid.max(1);
    }

    /// The active creation tool, or `None` for the pointer.
    pub fn tool(&self) -> Option<&str> {
        self.tool.as_deref()
    }

    /// Sets the active creation tool (`None` is the pointer). An unknown kind
    /// is rejected, so a stale toolbox entry cannot arm a broken tool.
    pub fn set_tool(&mut self, tool: Option<&str>) {
        self.tool = tool
            .filter(|kind| self.catalog.contains(kind))
            .map(str::to_owned);
    }

    /// Whether an undo step is available.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// Whether a redo step is available.
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// The number of copied nodes on the in-process clipboard.
    pub fn clipboard_len(&self) -> usize {
        self.clipboard.len()
    }

    /// The form's rectangle in design units, at the origin.
    pub fn form_rect(&self) -> DesignRect {
        let width = self
            .doc
            .window
            .prop("width")
            .and_then(Value::as_int)
            .unwrap_or(320);
        let height = self
            .doc
            .window
            .prop("height")
            .and_then(Value::as_int)
            .unwrap_or(200);
        DesignRect::new(0, 0, width, height)
    }

    /// The drag preview rectangle, while a move or resize is in progress.
    pub fn preview(&self) -> Option<DesignRect> {
        self.preview
    }

    /// The marquee rectangle, while a rubber-band selection is in progress.
    pub fn marquee(&self) -> Option<DesignRect> {
        self.marquee
    }

    /// Whether a gesture is in progress.
    pub fn is_dragging(&self) -> bool {
        !matches!(self.drag, Drag::None)
    }

    /// The local (parent-relative) rectangle a node occupies, using the
    /// catalog's default size for an omitted `width`/`height`.
    pub fn node_local_rect(&self, node: &Node) -> DesignRect {
        let default_width = self
            .catalog
            .get(&node.kind)
            .map(|spec| spec.default_size.0.value().round() as i64)
            .unwrap_or(0);
        let default_height = self
            .catalog
            .get(&node.kind)
            .map(|spec| spec.default_size.1.value().round() as i64)
            .unwrap_or(0);
        let left = int_prop(node, "left", 0);
        let top = int_prop(node, "top", 0);
        let width = int_prop(node, "width", default_width).max(0);
        let height = int_prop(node, "height", default_height).max(0);
        DesignRect::from_size(left, top, width, height)
    }

    /// The node's rectangle in form coordinates, accumulating its ancestors'
    /// origins.
    pub fn node_rect(&self, name: &str) -> Option<DesignRect> {
        let node = self.doc.node(name)?;
        let mut origin = (0, 0);
        let mut parent = node.parent.clone();
        // Bound the walk so a hand-written cycle cannot hang the designer.
        for _ in 0..=self.doc.nodes.len() {
            let Some(parent_name) = parent else {
                break;
            };
            let parent_node = self.doc.node(&parent_name)?;
            let rect = self.node_local_rect(parent_node);
            origin.0 += rect.left;
            origin.1 += rect.top;
            parent = parent_node.parent.clone();
        }
        Some(self.node_local_rect(node).offset(origin.0, origin.1))
    }

    /// The bounding rectangle of the current selection: the form rect for the
    /// form, or the union of the selected controls.
    pub fn selection_bounds(&self) -> DesignRect {
        match &self.selection {
            Selection::Form => self.form_rect(),
            Selection::Nodes(names) => {
                let mut bounds: Option<DesignRect> = None;
                for name in names {
                    if let Some(rect) = self.node_rect(name) {
                        bounds = Some(match bounds {
                            Some(bounds) => bounds.union(rect),
                            None => rect,
                        });
                    }
                }
                bounds.unwrap_or_else(|| self.form_rect())
            }
        }
    }

    /// The topmost node whose rectangle contains `(x, y)`, if any. Nodes later
    /// in creation order are on top, so they are searched first.
    pub fn hit_node(&self, x: i64, y: i64) -> Option<String> {
        self.doc
            .nodes
            .iter()
            .rev()
            .find(|node| {
                self.node_rect(&node.name)
                    .is_some_and(|rect| rect.contains(x, y))
            })
            .map(|node| node.name.clone())
    }

    /// The resize handle under `(x, y)`, if the selection exposes handles.
    pub fn handle_at(&self, x: i64, y: i64) -> Option<Handle> {
        match &self.selection {
            Selection::Form => handle_at(self.form_rect(), x, y, HANDLE_TOLERANCE),
            Selection::Nodes(names) if names.len() == 1 => self
                .node_rect(&names[0])
                .and_then(|rect| handle_at(rect, x, y, HANDLE_TOLERANCE)),
            _ => None,
        }
    }

    /// The cursor to show at `(x, y)`.
    pub fn cursor_at(&self, x: i64, y: i64) -> CursorHint {
        if self.tool.is_some() {
            return CursorHint::Default;
        }
        match self.handle_at(x, y) {
            Some(Handle::West | Handle::East) => CursorHint::SizeHorizontal,
            Some(Handle::North | Handle::South) => CursorHint::SizeVertical,
            _ => CursorHint::Default,
        }
    }

    /// Handles a left-button press at `(x, y)`.
    pub fn pointer_down(&mut self, x: i64, y: i64, ctrl: bool) -> Outcome {
        // An armed tool draws a new control; selection and handles are ignored
        // until the tool is set back to the pointer.
        if let Some(kind) = self.tool.clone() {
            self.drag = Drag::Create {
                kind,
                origin: (x, y),
            };
            self.preview = Some(DesignRect::new(x, y, x, y));
            return Outcome::none();
        }
        let cursor = self.cursor_at(x, y);
        if let Some(handle) = self.handle_at(x, y) {
            self.drag = match &self.selection {
                Selection::Form => Drag::ResizeForm {
                    handle,
                    start_rect: self.form_rect(),
                },
                Selection::Nodes(names) if names.len() == 1 => {
                    let name = names[0].clone();
                    Drag::ResizeNode {
                        start_rect: self.node_local_rect(self.doc.node(&name).expect("selected")),
                        name,
                        handle,
                        origin: (x, y),
                    }
                }
                _ => Drag::None,
            };
            if !matches!(self.drag, Drag::None) {
                return Outcome::none().cursor(cursor);
            }
        }

        match self.hit_node(x, y) {
            Some(name) => {
                if ctrl {
                    self.toggle_selection(&name);
                    self.drag = Drag::None;
                    Outcome::changed(Change::SELECTION).cursor(cursor)
                } else {
                    if !self.selection.contains(&name) {
                        self.selection = Selection::Nodes(vec![name.clone()]);
                    }
                    self.begin_move(x, y);
                    Outcome::changed(Change::SELECTION).cursor(cursor)
                }
            }
            None => {
                if !ctrl {
                    self.selection = Selection::Nodes(Vec::new());
                }
                self.drag = Drag::Marquee { origin: (x, y) };
                self.marquee = Some(DesignRect::new(x, y, x, y));
                Outcome::changed(Change::SELECTION).cursor(cursor)
            }
        }
    }

    /// Handles a pointer move to `(x, y)`.
    pub fn pointer_move(&mut self, x: i64, y: i64, _ctrl: bool) -> Outcome {
        let cursor = self.cursor_at(x, y);
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        let (drag, change) = match drag {
            Drag::None => (Drag::None, Change::NONE),
            Drag::Move { origin, starts } => {
                let dx = x - origin.0;
                let dy = y - origin.1;
                for (name, left, top) in &starts {
                    if let Some(node) = self.doc.node_mut(name) {
                        node.set_prop("left", Value::Int(snap(left + dx, self.grid)));
                        node.set_prop("top", Value::Int(snap(top + dy, self.grid)));
                    }
                }
                self.preview = Some(self.selection_bounds());
                (Drag::Move { origin, starts }, Change::GEOMETRY)
            }
            Drag::ResizeNode {
                name,
                handle,
                start_rect,
                origin,
            } => {
                let new = resize(start_rect, handle, x - origin.0, y - origin.1, self.grid);
                self.set_node_rect(&name, new);
                self.preview = self.node_rect(&name);
                (
                    Drag::ResizeNode {
                        name,
                        handle,
                        start_rect,
                        origin,
                    },
                    Change::GEOMETRY,
                )
            }
            Drag::ResizeForm { handle, start_rect } => {
                let new = resize_form(start_rect, handle, x, y, self.grid);
                self.set_window_size(new.right, new.bottom);
                self.preview = Some(new);
                (Drag::ResizeForm { handle, start_rect }, Change::GEOMETRY)
            }
            Drag::Marquee { origin } => {
                self.marquee = Some(DesignRect::from_points(origin, (x, y)));
                (Drag::Marquee { origin }, Change::NONE)
            }
            Drag::Create { kind, origin } => {
                self.preview = Some(DesignRect::from_points(origin, (x, y)));
                (Drag::Create { kind, origin }, Change::NONE)
            }
        };
        self.drag = drag;
        Outcome::changed(change).cursor(cursor)
    }

    /// Handles a left-button release at `(x, y)`.
    pub fn pointer_up(&mut self, x: i64, y: i64, _ctrl: bool) -> Outcome {
        let cursor = self.cursor_at(x, y);
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::None => Outcome::none().cursor(cursor),
            Drag::Move { .. } | Drag::ResizeNode { .. } | Drag::ResizeForm { .. } => {
                self.preview = None;
                self.history.record(&self.doc);
                Outcome::changed(Change::GEOMETRY).cursor(cursor)
            }
            Drag::Marquee { origin } => {
                self.marquee = None;
                let rect = DesignRect::from_points(origin, (x, y));
                let hits = self.nodes_in(rect);
                // A click or marquee that catches nothing selects the form, as in
                // VB6, so its handles and properties come back.
                self.selection = if hits.is_empty() {
                    Selection::Form
                } else {
                    Selection::Nodes(hits)
                };
                Outcome::changed(Change::SELECTION).cursor(cursor)
            }
            Drag::Create { kind, origin } => {
                self.preview = None;
                if self.finish_create(&kind, origin, (x, y)).is_some() {
                    Outcome::changed(Change::STRUCTURE).cursor(cursor)
                } else {
                    Outcome::none().cursor(cursor)
                }
            }
        }
    }

    /// Handles a key press.
    pub fn key(&mut self, press: KeyPress) -> Outcome {
        if press.ctrl {
            let shortcut = match press.key {
                KeyInput::Copy => Some(self.copy()),
                KeyInput::Cut => {
                    // Nothing on the clipboard changes unless something was
                    // copied, so an empty selection cuts nothing.
                    return if self.copy() {
                        self.delete_selection()
                    } else {
                        Outcome::none()
                    };
                }
                KeyInput::Paste => Some(self.paste()),
                KeyInput::Duplicate => Some(self.duplicate()),
                KeyInput::Undo if press.shift => Some(self.redo()),
                KeyInput::Undo => Some(self.undo()),
                KeyInput::Redo => Some(self.redo()),
                KeyInput::SelectAll => {
                    self.select_all();
                    return Outcome::changed(Change::SELECTION);
                }
                _ => None,
            };
            if let Some(true) = shortcut {
                let change = if matches!(press.key, KeyInput::Paste | KeyInput::Duplicate) {
                    Change::STRUCTURE
                } else if matches!(press.key, KeyInput::Copy) {
                    Change::NONE
                } else {
                    // Undo/redo can change the whole document; rebuild.
                    Change::STRUCTURE
                };
                return Outcome::changed(change);
            }
            if let Some(false) = shortcut {
                return Outcome::none();
            }
        }

        match press.key {
            KeyInput::Left => self.nudge(-1, 0, press.ctrl),
            KeyInput::Right => self.nudge(1, 0, press.ctrl),
            KeyInput::Up => self.nudge(0, -1, press.ctrl),
            KeyInput::Down => self.nudge(0, 1, press.ctrl),
            KeyInput::Delete | KeyInput::Backspace => self.delete_selection(),
            KeyInput::Escape => {
                self.selection = Selection::Form;
                Outcome::changed(Change::SELECTION)
            }
            _ => Outcome::none(),
        }
    }

    /// Selects the form.
    pub fn select_form(&mut self) {
        self.selection = Selection::Form;
    }

    /// Selects a single node, if it exists.
    pub fn select_node(&mut self, name: &str) -> bool {
        if self.doc.node(name).is_some() {
            self.selection = Selection::Nodes(vec![name.to_owned()]);
            true
        } else {
            false
        }
    }

    /// Adds `name` to the selection, or removes it if already selected.
    pub fn toggle_selection(&mut self, name: &str) {
        let mut names = self.selection.nodes().to_vec();
        if let Some(index) = names.iter().position(|selected| selected == name) {
            names.remove(index);
        } else if self.doc.node(name).is_some() {
            names.push(name.to_owned());
        }
        self.selection = if names.is_empty() {
            Selection::Form
        } else {
            Selection::Nodes(names)
        };
    }

    /// Selects every control.
    pub fn select_all(&mut self) {
        let names: Vec<String> = self
            .doc
            .nodes
            .iter()
            .map(|node| node.name.clone())
            .collect();
        self.selection = if names.is_empty() {
            Selection::Form
        } else {
            Selection::Nodes(names)
        };
    }

    /// Applies one undoable property edit to the form or a node.
    ///
    /// This is the property grid's command. The value is validated against the
    /// catalog schema and the schema's design-mode access rule; renaming a node
    /// (the synthetic `name` property) additionally validates the identifier
    /// and its uniqueness. A no-op edit returns [`Change::NONE`] and records no
    /// undo step. The returned [`Change`] tells [`crate::Designer`] how to
    /// refresh the live preview.
    pub fn set_property(
        &mut self,
        target: &Target,
        name: &str,
        value: Value,
    ) -> Result<Change, PropertyError> {
        match target {
            Target::Form => self.set_form_property(name, value),
            Target::Node(node) => self.set_node_property(node, name, value),
        }
    }

    /// Applies a property edit to the window.
    fn set_form_property(&mut self, name: &str, value: Value) -> Result<Change, PropertyError> {
        let Some(spec) = self.catalog.window_spec().property(name) else {
            return Err(PropertyError::UnknownProperty(name.to_owned()));
        };
        if !spec.access.writable_in_design() {
            return Err(PropertyError::ReadOnly(name.to_owned()));
        }
        if !spec.accepts(&value) {
            return Err(PropertyError::InvalidValue(name.to_owned()));
        }
        let current = self
            .doc
            .window
            .prop(name)
            .cloned()
            .unwrap_or_else(|| spec.default.clone());
        if current == value {
            return Ok(Change::NONE);
        }
        self.doc.window.set_prop(name, value);
        self.history.record(&self.doc);
        Ok(if matches!(name, "width" | "height") {
            Change::GEOMETRY
        } else {
            Change::PROPERTY
        })
    }

    /// Applies a property edit to a node, including the `name` rename.
    fn set_node_property(
        &mut self,
        node: &str,
        name: &str,
        value: Value,
    ) -> Result<Change, PropertyError> {
        let kind = self
            .doc
            .node(node)
            .map(|node| node.kind.clone())
            .ok_or(PropertyError::UnknownObject)?;

        if name == "name" {
            let Value::Text(new_name) = value else {
                return Err(PropertyError::InvalidValue(name.to_owned()));
            };
            if new_name == node {
                return Ok(Change::NONE);
            }
            if !is_valid_name(&new_name) {
                return Err(PropertyError::InvalidName(new_name));
            }
            if self.doc.node(&new_name).is_some() {
                return Err(PropertyError::DuplicateName(new_name));
            }
            self.doc.rename(node, &new_name);
            // Keep the renamed control selected under its new name, so the
            // property grid and the selection outline follow it.
            if let Selection::Nodes(names) = &mut self.selection {
                for selected in names.iter_mut().filter(|selected| *selected == node) {
                    *selected = new_name.clone();
                }
            }
            self.history.record(&self.doc);
            return Ok(Change::STRUCTURE);
        }

        let Some(spec) = self.catalog.property(&kind, name) else {
            return Err(PropertyError::UnknownProperty(name.to_owned()));
        };
        if !spec.access.writable_in_design() {
            return Err(PropertyError::ReadOnly(name.to_owned()));
        }
        if !spec.accepts(&value) {
            return Err(PropertyError::InvalidValue(name.to_owned()));
        }
        let current = self
            .doc
            .node(node)
            .and_then(|node| node.prop(name))
            .cloned()
            .unwrap_or_else(|| spec.default.clone());
        if current == value {
            return Ok(Change::NONE);
        }
        self.doc
            .node_mut(node)
            .expect("node checked above")
            .set_prop(name, value);
        self.history.record(&self.doc);
        Ok(if matches!(name, "left" | "top" | "width" | "height") {
            Change::GEOMETRY
        } else {
            Change::PROPERTY
        })
    }

    /// Deletes the selected controls (with their descendants).
    pub fn delete_selection(&mut self) -> Outcome {
        let names: Vec<String> = self.selection.nodes().to_vec();
        if names.is_empty() {
            return Outcome::none();
        }
        for name in &names {
            self.doc.remove(name);
        }
        self.selection = Selection::Form;
        self.history.record(&self.doc);
        Outcome::changed(Change::STRUCTURE)
    }

    /// Creates a control of `kind` filling `rect` (in form coordinates),
    /// parented to `parent` when that container accepts the child.
    ///
    /// The node gets the catalog's schema defaults, a unique snake_case name
    /// (`button1`, `edit1`, …) and the next free tab index among its siblings.
    /// It is inserted into the model and recorded as one undo step, and it
    /// becomes the selection. Returns the new node's name, or `None` when the
    /// kind is unknown or `parent` was rejected.
    pub fn create_control(
        &mut self,
        kind: &str,
        rect: DesignRect,
        parent: Option<String>,
    ) -> Option<String> {
        self.catalog.get(kind)?;
        let parent = parent.filter(|name| {
            self.doc
                .node(name)
                .is_some_and(|node| self.catalog.accepts_child(&node.kind, kind))
        });

        let (left, top) = match &parent {
            Some(parent) => {
                let origin = self.node_rect(parent)?;
                (rect.left - origin.left, rect.top - origin.top)
            }
            None => (rect.left, rect.top),
        };

        let name = self.auto_name(&control_base_name(kind));
        let mut node = Node::new(kind, name.clone());
        node.parent = parent.clone();
        for (property, value) in self.schema_defaults(kind) {
            node.set_prop(property, value);
        }
        // A control whose text is a caption starts out showing its name, as in
        // VB, so a new button is not an empty box. An Edit's or a combo's text
        // is the user's input, so it stays empty.
        if CAPTIONED_KINDS.contains(&kind) {
            node.set_prop("text", Value::Text(name.clone()));
        }
        node.set_prop("left", Value::Int(left));
        node.set_prop("top", Value::Int(top));
        node.set_prop("width", Value::Int(rect.width().max(MIN_SIZE)));
        node.set_prop("height", Value::Int(rect.height().max(MIN_SIZE)));
        node.set_prop(
            "tab_index",
            Value::Int(self.next_tab_index(parent.as_deref())),
        );

        self.doc.insert(node);
        self.selection = Selection::Nodes(vec![name.clone()]);
        self.history.record(&self.doc);
        Some(name)
    }

    /// Drops a control of `kind` at its catalog default size in the centre of
    /// the form (the toolbox's double-click action). Returns the new name.
    pub fn drop_control(&mut self, kind: &str) -> Option<String> {
        let (width, height) = self.default_size(kind);
        let form = self.form_rect();
        let left = ((form.width() - width) / 2).max(0);
        let top = ((form.height() - height) / 2).max(0);
        self.create_control(kind, DesignRect::from_size(left, top, width, height), None)
    }

    /// The topmost container whose rectangle contains `(x, y)`, so a control
    /// dropped on a `Frame` becomes its child.
    pub fn container_at(&self, x: i64, y: i64) -> Option<String> {
        self.doc
            .nodes
            .iter()
            .rev()
            .find(|node| {
                self.catalog.is_container(&node.kind)
                    && self
                        .node_rect(&node.name)
                        .is_some_and(|rect| rect.contains(x, y))
            })
            .map(|node| node.name.clone())
    }

    /// The catalog default size `(width, height)` of `kind`, or `(0, 0)`.
    fn default_size(&self, kind: &str) -> (i64, i64) {
        match self.catalog.get(kind) {
            Some(spec) => (
                spec.default_size.0.value().round() as i64,
                spec.default_size.1.value().round() as i64,
            ),
            None => (0, 0),
        }
    }

    /// Finishes a click-then-drag creation: snaps the drag to the grid, falls
    /// back to the default size for a click, finds the target container and
    /// creates the control.
    fn finish_create(&mut self, kind: &str, origin: (i64, i64), end: (i64, i64)) -> Option<String> {
        // Normalised, so a drag up or to the left creates what the preview
        // showed instead of falling back to the default size.
        let dragged = DesignRect::from_points(
            (snap(origin.0, self.grid), snap(origin.1, self.grid)),
            (snap(end.0, self.grid), snap(end.1, self.grid)),
        );
        let rect = if dragged.width() < MIN_DRAW || dragged.height() < MIN_DRAW {
            let (width, height) = self.default_size(kind);
            DesignRect::from_size(dragged.left, dragged.top, width, height)
        } else {
            dragged
        };
        let parent = self.container_at(rect.left, rect.top);
        self.create_control(kind, rect, parent)
    }

    /// The schema defaults for `kind`: the common properties, then its own.
    fn schema_defaults(&self, kind: &str) -> Vec<(String, Value)> {
        let mut defaults = Vec::new();
        for property in self.catalog.common_properties() {
            if let Some(spec) = self.catalog.property(kind, &property.name) {
                defaults.push((property.name.clone(), spec.default));
            }
        }
        if let Some(spec) = self.catalog.get(kind) {
            for property in &spec.properties {
                defaults.push((property.name.clone(), property.default.clone()));
            }
        }
        defaults
    }

    /// The next tab index among the children of `parent`: one past the highest
    /// in use, so a new control joins the end of the tab order.
    fn next_tab_index(&self, parent: Option<&str>) -> i64 {
        self.doc
            .nodes
            .iter()
            .filter(|node| node.parent.as_deref() == parent)
            .map(|node| int_prop(node, "tab_index", 0))
            .max()
            .map_or(0, |max| max + 1)
    }

    /// A fresh unique name derived from `base`, starting at one and skipping
    /// names already in use.
    fn auto_name(&self, base: &str) -> String {
        let mut index = 1;
        loop {
            let candidate = format!("{base}{index}");
            if self.doc.node(&candidate).is_none() {
                return candidate;
            }
            index += 1;
        }
    }

    /// Copies the selected controls (with their descendants) to the in-process
    /// clipboard, returning whether anything was copied.
    pub fn copy(&mut self) -> bool {
        let roots = self.selected_roots();
        if roots.is_empty() {
            return false;
        }
        let mut copied = Vec::new();
        for root in &roots {
            for name in self.subtree(root) {
                if let Some(node) = self.doc.node(&name) {
                    copied.push(node.clone());
                }
            }
        }
        self.clipboard = copied;
        true
    }

    /// Pastes the clipboard with fresh names, offset by one grid step, returning
    /// whether anything was pasted.
    pub fn paste(&mut self) -> bool {
        if self.clipboard.is_empty() {
            return false;
        }
        // Names chosen earlier in this paste count as taken, so two pasted
        // nodes can never end up with the same name.
        let mut renamed: BTreeMap<String, String> = BTreeMap::new();
        let mut taken = std::collections::BTreeSet::new();
        for node in &self.clipboard {
            let name = self.unique_name(&node.name, &taken);
            taken.insert(name.clone());
            renamed.insert(node.name.clone(), name);
        }

        let offset = self.grid;
        let mut new_roots = Vec::new();
        for node in &self.clipboard {
            let mut copy = node.clone();
            copy.name = renamed[&node.name].clone();
            let parent_copied = node
                .parent
                .as_deref()
                .and_then(|parent| renamed.get(parent).cloned());
            copy.parent = match &parent_copied {
                Some(parent) => Some(parent.clone()),
                None => node
                    .parent
                    .clone()
                    .filter(|parent| self.doc.node(parent).is_some()),
            };
            if parent_copied.is_none() {
                let left = int_prop(node, "left", 0) + offset;
                let top = int_prop(node, "top", 0) + offset;
                copy.set_prop("left", Value::Int(left));
                copy.set_prop("top", Value::Int(top));
                new_roots.push(copy.name.clone());
            }
            self.doc.insert(copy);
        }
        self.selection = Selection::Nodes(new_roots);
        self.history.record(&self.doc);
        true
    }

    /// Copies and pastes the selection in one step, returning whether anything
    /// was duplicated.
    pub fn duplicate(&mut self) -> bool {
        if !self.copy() {
            return false;
        }
        self.paste()
    }

    /// Undoes the last recorded change, returning whether anything changed.
    pub fn undo(&mut self) -> bool {
        let Some(snapshot) = self.history.undo().cloned() else {
            return false;
        };
        self.doc = snapshot;
        self.prune_selection();
        true
    }

    /// Redoes the last undone change, returning whether anything changed.
    pub fn redo(&mut self) -> bool {
        let Some(snapshot) = self.history.redo().cloned() else {
            return false;
        };
        self.doc = snapshot;
        self.prune_selection();
        true
    }

    /// Moves the selected controls by one step: `1` design unit with
    /// `fine` (Ctrl), or one grid step otherwise.
    pub fn nudge(&mut self, dx: i64, dy: i64, fine: bool) -> Outcome {
        let step = if fine { 1 } else { self.grid };
        let roots = self.selected_roots();
        if roots.is_empty() {
            return Outcome::none();
        }
        for name in &roots {
            let node = self.doc.node_mut(name).expect("selected root");
            let left = int_prop(node, "left", 0) + dx * step;
            let top = int_prop(node, "top", 0) + dy * step;
            node.set_prop("left", Value::Int(left));
            node.set_prop("top", Value::Int(top));
        }
        self.history.record(&self.doc);
        Outcome::changed(Change::GEOMETRY)
    }

    /// The selected node names whose own parent is not also selected, so a drag
    /// moves each subtree exactly once.
    fn selected_roots(&self) -> Vec<String> {
        self.selection
            .nodes()
            .iter()
            .filter(|name| {
                let mut parent = self.doc.node(name).and_then(|node| node.parent.clone());
                for _ in 0..=self.doc.nodes.len() {
                    let Some(parent_name) = parent else {
                        return true;
                    };
                    if self.selection.contains(&parent_name) {
                        return false;
                    }
                    parent = self
                        .doc
                        .node(&parent_name)
                        .and_then(|node| node.parent.clone());
                }
                true
            })
            .cloned()
            .collect()
    }

    /// Every node in `root`'s subtree, in document order (parents first).
    fn subtree(&self, root: &str) -> Vec<String> {
        let mut names = vec![root.to_owned()];
        let mut index = 0;
        while index < names.len() {
            let parent = names[index].clone();
            index += 1;
            for child in self.doc.children_of(&parent) {
                if !names.contains(&child.name) {
                    names.push(child.name.clone());
                }
            }
        }
        self.doc
            .nodes
            .iter()
            .filter(|node| names.contains(&node.name))
            .map(|node| node.name.clone())
            .collect()
    }

    /// The nodes whose rectangles intersect `rect`.
    fn nodes_in(&self, rect: DesignRect) -> Vec<String> {
        self.doc
            .nodes
            .iter()
            .filter(|node| {
                self.node_rect(&node.name)
                    .is_some_and(|node_rect| node_rect.intersects(rect))
            })
            .map(|node| node.name.clone())
            .collect()
    }

    /// Records the selected nodes' original positions and starts a move.
    fn begin_move(&mut self, x: i64, y: i64) {
        let starts = self
            .selected_roots()
            .into_iter()
            .map(|name| {
                let node = self.doc.node(&name).expect("selected root");
                (name, int_prop(node, "left", 0), int_prop(node, "top", 0))
            })
            .collect();
        self.drag = Drag::Move {
            origin: (x, y),
            starts,
        };
    }

    /// Writes a node's local rectangle back to its `left`/`top`/`width`/`height`
    /// properties.
    fn set_node_rect(&mut self, name: &str, rect: DesignRect) {
        if let Some(node) = self.doc.node_mut(name) {
            node.set_prop("left", Value::Int(rect.left));
            node.set_prop("top", Value::Int(rect.top));
            node.set_prop("width", Value::Int(rect.width().max(MIN_SIZE)));
            node.set_prop("height", Value::Int(rect.height().max(MIN_SIZE)));
        }
    }

    /// Writes the window's client size.
    fn set_window_size(&mut self, width: i64, height: i64) {
        self.doc
            .window
            .set_prop("width", Value::Int(width.max(MIN_SIZE)));
        self.doc
            .window
            .set_prop("height", Value::Int(height.max(MIN_SIZE)));
    }

    /// A name not already used by a node, derived from `base`.
    fn unique_name(&self, base: &str, taken: &std::collections::BTreeSet<String>) -> String {
        let free = |name: &str| self.doc.node(name).is_none() && !taken.contains(name);
        if free(base) {
            return base.to_owned();
        }
        let mut index = 1;
        loop {
            let candidate = format!("{base}{index}");
            if free(&candidate) {
                return candidate;
            }
            index += 1;
        }
    }

    /// Drops selected names that no longer exist after an undo or redo.
    fn prune_selection(&mut self) {
        if let Selection::Nodes(names) = &mut self.selection {
            names.retain(|name| self.doc.node(name).is_some());
            if names.is_empty() {
                self.selection = Selection::Form;
            }
        }
    }
}

/// Reads an int property, or `default` when absent or the wrong type.
fn int_prop(node: &Node, name: &str, default: i64) -> i64 {
    node.prop(name).and_then(Value::as_int).unwrap_or(default)
}

/// The base name a control kind is auto-named from: the kind in snake_case
/// (PLAN.md §1.1), so the first ones become `button1`, `edit1`, `check_box1`
/// and so on.
pub fn control_base_name(kind: &str) -> String {
    snake_case(kind)
}

/// The snake_case event names a control of `kind` can have handlers for, from
/// the catalog: a `Button`'s `Click` is `click`, so its handler is
/// `<name>_click`.
pub fn handler_events(catalog: &Catalog, kind: &str) -> Vec<String> {
    catalog
        .get(kind)
        .map(|spec| {
            spec.events
                .iter()
                .map(|event| snake_case(&event.name))
                .collect()
        })
        .unwrap_or_default()
}

/// `PascalCase` to `snake_case`: `CheckBox` → `check_box`, `Click` → `click`.
pub fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, character) in name.chars().enumerate() {
        if character.is_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.extend(character.to_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A form with two buttons and a panel holding a label.
    fn sample() -> Surface {
        let mut doc = FormDoc::new("main_form");
        let mut button = Node::new("Button", "ok_button");
        button.set_prop("left", Value::Int(16));
        button.set_prop("top", Value::Int(16));
        button.set_prop("width", Value::Int(80));
        button.set_prop("height", Value::Int(24));
        doc.insert(button);

        let mut other = Node::new("Button", "cancel_button");
        other.set_prop("left", Value::Int(120));
        other.set_prop("top", Value::Int(16));
        other.set_prop("width", Value::Int(80));
        other.set_prop("height", Value::Int(24));
        doc.insert(other);

        let mut panel = Node::new("Panel", "main_panel");
        panel.set_prop("left", Value::Int(50));
        panel.set_prop("top", Value::Int(80));
        panel.set_prop("width", Value::Int(160));
        panel.set_prop("height", Value::Int(100));
        doc.insert(panel);

        let mut label = Node::new("Label", "inner_label");
        label.parent = Some("main_panel".to_owned());
        label.set_prop("left", Value::Int(10));
        label.set_prop("top", Value::Int(10));
        label.set_prop("width", Value::Int(60));
        label.set_prop("height", Value::Int(20));
        doc.insert(label);

        Surface::new(
            doc,
            Rc::new(lazyrad_project::lazyrad_catalog()),
            DEFAULT_GRID,
        )
    }

    #[test]
    fn a_paste_never_gives_two_nodes_the_same_name() {
        let mut doc = FormDoc::new("main_form");
        doc.insert(Node::new("Button", "btn"));
        doc.insert(Node::new("Button", "btn1"));
        let mut surface = Surface::new(
            doc,
            Rc::new(lazyrad_project::lazyrad_catalog()),
            DEFAULT_GRID,
        );
        surface.select_all();
        assert!(surface.copy());
        // With `btn1` gone, `btn` would naively become `btn1` and `btn1` stay
        // `btn1`.
        assert!(surface.select_node("btn1"));
        surface.delete_selection();
        assert!(surface.paste());

        let names: Vec<&str> = surface
            .doc()
            .nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(names.len(), unique.len(), "duplicate names in {names:?}");
        assert_eq!(names.len(), 3);
    }

    #[test]
    fn a_child_rectangle_is_relative_to_its_parent() {
        let surface = sample();
        assert_eq!(
            surface.node_rect("inner_label"),
            Some(DesignRect::new(60, 90, 120, 110))
        );
    }

    #[test]
    fn clicking_selects_the_topmost_node() {
        let mut surface = sample();
        let outcome = surface.pointer_down(20, 20, false);
        assert!(outcome.change.selection);
        assert_eq!(
            surface.selection(),
            &Selection::Nodes(vec!["ok_button".into()])
        );
        surface.pointer_up(20, 20, false);
    }

    #[test]
    fn ctrl_click_adds_then_removes() {
        let mut surface = sample();
        surface.pointer_down(20, 20, false);
        surface.pointer_up(20, 20, false);
        surface.pointer_down(130, 20, true);
        assert_eq!(
            surface.selection(),
            &Selection::Nodes(vec!["ok_button".into(), "cancel_button".into()])
        );
        surface.pointer_up(130, 20, true);
        surface.pointer_down(130, 20, true);
        assert_eq!(
            surface.selection(),
            &Selection::Nodes(vec!["ok_button".into()])
        );
    }

    #[test]
    fn escape_selects_the_form() {
        let mut surface = sample();
        surface.select_node("ok_button");
        let outcome = surface.key(KeyPress::new(KeyInput::Escape));
        assert!(outcome.change.selection);
        assert!(surface.selection().is_form());
    }

    #[test]
    fn dragging_moves_and_snaps_to_the_grid() {
        let mut surface = sample();
        surface.pointer_down(20, 20, false);
        surface.pointer_move(30, 30, false);
        surface.pointer_up(30, 30, false);
        let node = surface.doc.node("ok_button").expect("node");
        assert_eq!(node.prop("left"), Some(&Value::Int(24)));
        assert_eq!(node.prop("top"), Some(&Value::Int(24)));
    }

    #[test]
    fn a_handle_resizes_a_single_node() {
        let mut surface = sample();
        surface.select_node("ok_button");
        // Grab the south-east handle at (96, 40) and drag to (120, 64).
        surface.pointer_down(96, 40, false);
        surface.pointer_move(120, 64, false);
        surface.pointer_up(120, 64, false);
        let node = surface.doc.node("ok_button").expect("node");
        assert_eq!(node.prop("width"), Some(&Value::Int(104)));
        assert_eq!(node.prop("height"), Some(&Value::Int(48)));
    }

    #[test]
    fn the_form_handle_resizes_the_client_area() {
        let mut surface = sample();
        assert!(surface.selection().is_form());
        let form = surface.form_rect();
        surface.pointer_down(form.right, form.bottom, false);
        surface.pointer_move(form.right + 40, form.bottom + 40, false);
        surface.pointer_up(form.right + 40, form.bottom + 40, false);
        assert_eq!(surface.form_rect(), DesignRect::new(0, 0, 360, 240));
    }

    #[test]
    fn marquee_selects_the_intersecting_nodes() {
        let mut surface = sample();
        // Start below the form's handles and drag up across both buttons.
        surface.pointer_down(0, 50, false);
        surface.pointer_move(130, 20, false);
        surface.pointer_up(130, 20, false);
        assert_eq!(
            surface.selection(),
            &Selection::Nodes(vec!["ok_button".into(), "cancel_button".into()])
        );
    }

    #[test]
    fn arrow_keys_nudge_by_the_grid_and_one_unit_with_ctrl() {
        let mut surface = sample();
        surface.select_node("ok_button");
        surface.key(KeyPress::new(KeyInput::Right));
        assert_eq!(
            surface.doc.node("ok_button").and_then(|n| n.prop("left")),
            Some(&Value::Int(24))
        );
        surface.key(KeyPress::ctrl(KeyInput::Right));
        assert_eq!(
            surface.doc.node("ok_button").and_then(|n| n.prop("left")),
            Some(&Value::Int(25))
        );
    }

    #[test]
    fn the_grid_is_configurable() {
        let mut surface = sample();
        surface.set_grid(4);
        surface.key(KeyPress::new(KeyInput::Down));
        assert_eq!(
            surface.doc.node("ok_button").and_then(|n| n.prop("top")),
            Some(&Value::Int(16))
        );
        assert_eq!(surface.grid(), 4);
    }

    #[test]
    fn delete_removes_the_selection_and_cascades() {
        let mut surface = sample();
        surface.select_node("main_panel");
        let outcome = surface.delete_selection();
        assert!(outcome.change.structure);
        assert!(surface.doc.node("main_panel").is_none());
        assert!(surface.doc.node("inner_label").is_none());
        assert!(surface.selection().is_form());
    }

    #[test]
    fn copy_paste_gives_a_unique_offset_copy() {
        let mut surface = sample();
        surface.select_node("ok_button");
        assert!(surface.copy());
        assert!(surface.paste());
        assert_eq!(
            surface.selection(),
            &Selection::Nodes(vec!["ok_button1".into()])
        );
        let node = surface.doc.node("ok_button1").expect("pasted");
        assert_eq!(node.prop("left"), Some(&Value::Int(24)));
        assert_eq!(node.prop("top"), Some(&Value::Int(24)));
    }

    #[test]
    fn ctrl_x_copies_then_deletes_the_selected_controls() {
        let mut surface = sample();
        surface.select_node("ok_button");
        let outcome = surface.key(KeyPress::ctrl(KeyInput::Cut));
        assert!(outcome.change.structure);
        assert!(surface.doc.node("ok_button").is_none());
        assert_eq!(surface.clipboard_len(), 1);
        assert!(surface.paste(), "the cut control pastes back");
        assert!(surface.doc.node("ok_button").is_some());
        // One undo reverts the paste, another the cut.
        assert!(surface.undo());
        assert!(surface.undo());
        assert!(surface.doc.node("ok_button").is_some());
    }

    #[test]
    fn ctrl_x_with_only_the_form_selected_cuts_nothing() {
        let mut surface = sample();
        surface.select_form();
        let outcome = surface.key(KeyPress::ctrl(KeyInput::Cut));
        assert!(!outcome.change.structure);
        assert_eq!(surface.clipboard_len(), 0);
        assert!(!surface.can_undo(), "no history entry for a no-op cut");
    }

    #[test]
    fn pasting_a_container_remaps_its_children() {
        let mut surface = sample();
        surface.select_node("main_panel");
        assert!(surface.duplicate());
        let copy = surface.doc.node("main_panel1").expect("copied panel");
        assert_eq!(copy.parent, None);
        let child = surface.doc.node("inner_label1").expect("copied child");
        assert_eq!(child.parent.as_deref(), Some("main_panel1"));
        assert_eq!(
            surface.node_rect("inner_label1"),
            Some(DesignRect::new(68, 98, 128, 118))
        );
    }

    #[test]
    fn undo_and_redo_restore_every_edit() {
        let mut surface = sample();
        surface.select_node("ok_button");
        surface.key(KeyPress::new(KeyInput::Right));
        assert_eq!(
            surface.doc.node("ok_button").and_then(|n| n.prop("left")),
            Some(&Value::Int(24))
        );
        assert!(surface.can_undo());
        assert!(surface.undo());
        assert_eq!(
            surface.doc.node("ok_button").and_then(|n| n.prop("left")),
            Some(&Value::Int(16))
        );
        assert!(surface.redo());
        assert_eq!(
            surface.doc.node("ok_button").and_then(|n| n.prop("left")),
            Some(&Value::Int(24))
        );
    }

    #[test]
    fn undoing_a_delete_restores_the_node() {
        let mut surface = sample();
        surface.select_node("ok_button");
        surface.delete_selection();
        assert!(surface.undo());
        assert!(surface.doc.node("ok_button").is_some());
    }

    #[test]
    fn edits_round_trip_through_toml() {
        let mut surface = sample();
        surface.select_node("ok_button");
        surface.key(KeyPress::new(KeyInput::Right));
        surface.duplicate();
        surface.select_node("main_panel");
        surface.key(KeyPress::new(KeyInput::Escape));

        let catalog = lazyrad_project::lazyrad_catalog();
        let text = surface.doc().to_toml(&catalog);
        let round_tripped = FormDoc::from_toml(&text, &catalog).expect("the form reloads");
        // Only non-default values are written, so the canonical text is what
        // must round-trip byte-for-byte, not the in-memory defaults.
        assert_eq!(round_tripped.to_toml(&catalog), text);
    }

    /// An empty form surface for the creation tests.
    fn empty() -> Surface {
        Surface::new(
            FormDoc::new("main_form"),
            Rc::new(lazyrad_project::lazyrad_catalog()),
            DEFAULT_GRID,
        )
    }

    #[test]
    fn an_unknown_tool_is_rejected() {
        let mut surface = empty();
        surface.set_tool(Some("Button"));
        assert_eq!(surface.tool(), Some("Button"));
        surface.set_tool(Some("Nope"));
        assert_eq!(surface.tool(), None);
        surface.set_tool(None);
        assert_eq!(surface.tool(), None);
    }

    #[test]
    fn a_tool_drag_creates_a_control_at_the_dragged_size() {
        let mut surface = empty();
        surface.set_tool(Some("Button"));
        surface.pointer_down(10, 10, false);
        surface.pointer_move(90, 50, false);
        let outcome = surface.pointer_up(90, 50, false);
        assert!(outcome.change.structure);
        let node = surface.doc.node("button1").expect("created");
        assert_eq!(node.kind, "Button");
        assert_eq!(node.prop("left"), Some(&Value::Int(8)));
        assert_eq!(node.prop("top"), Some(&Value::Int(8)));
        assert_eq!(node.prop("width"), Some(&Value::Int(80)));
        assert_eq!(node.prop("height"), Some(&Value::Int(40)));
        assert!(surface.selection().contains("button1"));
    }

    #[test]
    fn a_reverse_tool_drag_creates_the_same_control() {
        let mut surface = empty();
        surface.set_tool(Some("Button"));
        surface.pointer_down(90, 50, false);
        surface.pointer_move(10, 10, false);
        surface.pointer_up(10, 10, false);
        let node = surface.doc.node("button1").expect("created");
        assert_eq!(node.prop("left"), Some(&Value::Int(8)));
        assert_eq!(node.prop("top"), Some(&Value::Int(8)));
        assert_eq!(node.prop("width"), Some(&Value::Int(80)));
        assert_eq!(node.prop("height"), Some(&Value::Int(40)));
    }

    #[test]
    fn a_tool_click_uses_the_schema_default_size() {
        let mut surface = empty();
        surface.set_tool(Some("Button"));
        surface.pointer_down(40, 40, false);
        surface.pointer_up(40, 40, false);
        let node = surface.doc.node("button1").expect("created");
        assert_eq!(node.prop("width"), Some(&Value::Int(100)));
        assert_eq!(node.prop("height"), Some(&Value::Int(28)));
        assert_eq!(node.prop("left"), Some(&Value::Int(40)));
        assert_eq!(node.prop("top"), Some(&Value::Int(40)));
    }

    #[test]
    fn creation_names_follow_vb_and_skip_taken_names() {
        let mut surface = empty();
        assert_eq!(
            surface.create_control("Button", DesignRect::default(), None),
            Some("button1".into())
        );
        assert_eq!(
            surface.create_control("Button", DesignRect::default(), None),
            Some("button2".into())
        );
        surface.select_node("button2");
        surface.key(KeyPress::new(KeyInput::Escape));
        assert_eq!(
            surface.create_control("Button", DesignRect::default(), None),
            Some("button3".into())
        );
        assert_eq!(
            surface.create_control("Label", DesignRect::default(), None),
            Some("label1".into())
        );
        assert_eq!(
            surface.create_control("Edit", DesignRect::default(), None),
            Some("edit1".into())
        );
    }

    #[test]
    fn a_new_control_gets_schema_defaults_and_the_next_tab_index() {
        let mut surface = empty();
        surface.create_control("Button", DesignRect::new(0, 0, 100, 28), None);
        surface.create_control("Label", DesignRect::new(0, 40, 120, 20), None);
        let button = surface.doc.node("button1").expect("button");
        assert_eq!(button.prop("text"), Some(&Value::Text("button1".into())));
        assert_eq!(button.prop("enabled"), Some(&Value::Bool(true)));
        assert_eq!(button.prop("tab_index"), Some(&Value::Int(0)));
        let label = surface.doc.node("label1").expect("label");
        assert_eq!(label.prop("tab_index"), Some(&Value::Int(1)));
    }

    #[test]
    fn a_control_dropped_on_a_frame_becomes_its_child() {
        let mut doc = FormDoc::new("main_form");
        let mut frame = Node::new("GroupBox", "group_box1");
        frame.set_prop("left", Value::Int(40));
        frame.set_prop("top", Value::Int(40));
        frame.set_prop("width", Value::Int(200));
        frame.set_prop("height", Value::Int(120));
        doc.insert(frame);
        let mut surface = Surface::new(
            doc,
            Rc::new(lazyrad_project::lazyrad_catalog()),
            DEFAULT_GRID,
        );

        surface.set_tool(Some("Label"));
        surface.pointer_down(48, 48, false);
        surface.pointer_move(96, 80, false);
        surface.pointer_up(96, 80, false);

        let node = surface.doc.node("label1").expect("created");
        assert_eq!(node.parent.as_deref(), Some("group_box1"));
        assert_eq!(node.prop("left"), Some(&Value::Int(8)));
        assert_eq!(node.prop("top"), Some(&Value::Int(8)));
        assert_eq!(
            surface.node_rect("label1"),
            Some(DesignRect::new(48, 48, 96, 80))
        );
    }

    #[test]
    fn a_new_captioned_control_shows_its_name_and_an_edit_stays_empty() {
        let mut surface = empty();
        for kind in ["Button", "Label", "CheckBox", "Edit"] {
            surface.drop_control(kind);
        }
        let text = |name: &str| {
            surface
                .doc
                .node(name)
                .and_then(|node| node.prop("text"))
                .cloned()
        };
        assert_eq!(text("button1"), Some(Value::Text("button1".into())));
        assert_eq!(text("label1"), Some(Value::Text("label1".into())));
        assert_eq!(text("check_box1"), Some(Value::Text("check_box1".into())));
        assert_ne!(
            text("edit1"),
            Some(Value::Text("edit1".into())),
            "an Edit's text is the user's input, not a caption"
        );
    }

    #[test]
    fn creating_a_control_is_undoable() {
        let mut surface = empty();
        surface.set_tool(Some("CheckBox"));
        surface.pointer_down(8, 8, false);
        surface.pointer_up(8, 8, false);
        assert!(surface.doc.node("check_box1").is_some());
        assert!(surface.undo());
        assert!(surface.doc.node("check_box1").is_none());
        assert!(surface.redo());
        assert!(surface.doc.node("check_box1").is_some());
    }

    #[test]
    fn drop_control_centres_a_default_sized_control() {
        let mut surface = empty();
        assert_eq!(surface.drop_control("Button"), Some("button1".into()));
        let node = surface.doc.node("button1").expect("created");
        assert_eq!(node.parent, None);
        assert_eq!(node.prop("left"), Some(&Value::Int(110)));
        assert_eq!(node.prop("top"), Some(&Value::Int(86)));
        assert_eq!(node.prop("width"), Some(&Value::Int(100)));
        assert_eq!(node.prop("height"), Some(&Value::Int(28)));
    }

    #[test]
    fn a_click_on_empty_space_selects_the_form() {
        let mut surface = sample();
        surface.select_node("ok_button");
        surface.pointer_down(300, 190, false);
        assert_eq!(surface.selection(), &Selection::Nodes(Vec::new()));
        surface.pointer_up(300, 190, false);
        assert_eq!(surface.selection(), &Selection::Form);
    }

    #[test]
    fn setting_a_property_is_undoable_and_typed() {
        let mut surface = sample();
        let change = surface
            .set_property(
                &Target::Node("ok_button".into()),
                "text",
                Value::Text("Go".into()),
            )
            .expect("text is editable");
        assert_eq!(change, Change::PROPERTY);
        assert_eq!(
            surface.doc.node("ok_button").and_then(|n| n.prop("text")),
            Some(&Value::Text("Go".into()))
        );
        assert!(surface.undo());
        assert_eq!(
            surface.doc.node("ok_button").and_then(|n| n.prop("text")),
            None
        );
        assert!(surface.redo());
    }

    #[test]
    fn geometry_edits_report_geometry() {
        let mut surface = sample();
        let change = surface
            .set_property(&Target::Node("ok_button".into()), "left", Value::Int(32))
            .expect("left is editable");
        assert_eq!(change, Change::GEOMETRY);
        assert_eq!(
            surface.doc.node("ok_button").and_then(|n| n.prop("left")),
            Some(&Value::Int(32))
        );
    }

    #[test]
    fn form_properties_are_editable_but_unknown_ones_are_not() {
        let mut surface = sample();
        assert_eq!(
            surface
                .set_property(&Target::Form, "title", Value::Text("Hello".into()))
                .expect("title is editable"),
            Change::PROPERTY
        );
        assert_eq!(
            surface.set_property(&Target::Form, "nope", Value::Int(1)),
            Err(PropertyError::UnknownProperty("nope".into()))
        );
    }

    #[test]
    fn a_typed_mismatch_is_rejected() {
        let mut surface = sample();
        assert_eq!(
            surface.set_property(&Target::Node("ok_button".into()), "text", Value::Int(3)),
            Err(PropertyError::InvalidValue("text".into()))
        );
        assert_eq!(
            surface.set_property(
                &Target::Node("go_button".into()),
                "text",
                Value::Text("x".into())
            ),
            Err(PropertyError::UnknownObject)
        );
    }

    #[test]
    fn renaming_validates_the_identifier_and_uniqueness() {
        let mut surface = sample();
        assert_eq!(
            surface.set_property(
                &Target::Node("ok_button".into()),
                "name",
                Value::Text("1bad".into())
            ),
            Err(PropertyError::InvalidName("1bad".into()))
        );
        assert_eq!(
            surface.set_property(
                &Target::Node("ok_button".into()),
                "name",
                Value::Text("cancel_button".into())
            ),
            Err(PropertyError::DuplicateName("cancel_button".into()))
        );
        let change = surface
            .set_property(
                &Target::Node("ok_button".into()),
                "name",
                Value::Text("go_button".into()),
            )
            .expect("a fresh identifier is accepted");
        assert_eq!(change, Change::STRUCTURE);
        assert!(surface.doc.node("go_button").is_some());
        assert!(surface.undo());
        assert!(surface.doc.node("ok_button").is_some());
    }

    #[test]
    fn a_no_op_edit_records_nothing() {
        let mut surface = sample();
        let change = surface
            .set_property(
                &Target::Node("ok_button".into()),
                "text",
                Value::Text(String::new()),
            )
            .expect("the default text");
        assert_eq!(change, Change::NONE);
        assert!(!surface.can_undo());
    }

    #[test]
    fn rename_handlers_rewrites_only_the_controls_own_handlers() {
        let events = vec!["click".to_owned()];
        let source = "\
fn ok_click() {
    ok_click();
}
fn ok_button_click() {}
fn ok_clicked() {}
fn other_click() {}
";
        let renamed = rename_handlers(source, "ok", "go", &events);
        assert!(renamed.contains("fn go_click() {"));
        // A call is not a declaration, so it is left for the developer.
        assert!(renamed.contains("    ok_click();"));
        // `ok_button` is a different control, and `clicked` is not an event.
        assert!(renamed.contains("fn ok_button_click() {}"));
        assert!(renamed.contains("fn ok_clicked() {}"));
        assert!(renamed.contains("fn other_click() {}"));
        assert_eq!(rename_handlers(source, "ok", "ok", &events), source);
    }

    #[test]
    fn handler_events_are_the_catalogs_events_in_snake_case() {
        let catalog = lazyrad_project::lazyrad_catalog();
        assert_eq!(handler_events(&catalog, "Button"), vec!["click".to_owned()]);
        assert!(handler_events(&catalog, "CheckBox").contains(&"toggle".to_owned()));
        assert_eq!(snake_case("CheckBox"), "check_box");
    }
}
