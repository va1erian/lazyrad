#![forbid(unsafe_code)]

//! The [`Designer`] widget: the live preview panel and the transparent overlay.
//!
//! The designer renders the form's **real** `xui-core` widgets (through
//! [`xui_form::build_with`]) inside a [`Panel`], with
//! [`BuildOptions::design_mode`] on and the window in
//! [`Ui::set_design_mode(true)`](xui_core::app::Ui::set_design_mode). On top sits
//! a transparent [`Custom`](xui_core::backend::NodeKind::Custom) overlay that
//! receives all pointer and key input and paints the dot grid, the selection
//! outline and handles, the marquee and the drag preview.
//!
//! The overlay's event mapper only turns an input event into a [`DesignerMsg`]
//! and hands it to the host's message type through the `wrap` closure. The host
//! forwards it back with [`Designer::update`]. That keeps every model edit on
//! the host's `update` path, where it is safe to destroy and rebuild widgets,
//! and it is the same pattern the other xui widgets use.
//!
//! The overlay is **recreated** after every structural change rather than merely
//! raised. A backend without a z-order operation (the offscreen test backend)
//! picks the topmost node for a hit-test by creation order, so a rebuilt control
//! would otherwise land above the overlay and swallow input.
//!
//! **Design mode is per window.** `Ui::set_design_mode` is a flag on the window
//! (`Core`), not on a `Ui` handle or a container, so a `Designer` turns design
//! mode on for the whole window it is built in. A host that embeds a designer
//! next to live widgets in one window (the IDE) would need the designer in its
//! own child window, or an xui change to scope design mode to a subtree. See the
//! crate docs.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use xui_core::app::Ui;
use xui_core::backend::{
    BackendError, Canvas, Cursor, Dash, Event, NodeKind, NodeSpec, Rgba, Stroke, WidgetId,
};
use xui_core::geometry::{Point, Rect};
use xui_core::message::{Key, MouseButton};
use xui_core::units::{Dip, Px};
use xui_core::widget::{Control, Panel};
use xui_core::{Color, Theme};
use xui_form::{
    Binder, BuildError, BuildOptions, Catalog, EventHandler, EventRef, Factories, FormDoc,
    LiveForm, Value, build_with,
};

use crate::geometry::{DesignRect, Handle};
use crate::local_paint::paint_local;
use crate::surface::{
    Change, CursorHint, DEFAULT_GRID, KeyInput, KeyPress, Outcome, PropertyError, Selection,
    Surface, Target,
};
use crate::toolbox::ToolboxMsg;

/// An input event the overlay translated into design terms.
///
/// `x`/`y` are design units relative to the form's top-left, so the host can
/// forward the message to [`Designer::update`] without doing arithmetic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesignerMsg {
    /// The left button went down.
    PointerDown {
        /// The cursor x in design units.
        x: i64,
        /// The cursor y in design units.
        y: i64,
        /// Whether Ctrl was held.
        ctrl: bool,
    },
    /// The pointer moved.
    PointerMove {
        /// The cursor x in design units.
        x: i64,
        /// The cursor y in design units.
        y: i64,
        /// Whether Ctrl was held.
        ctrl: bool,
    },
    /// The left button was released.
    PointerUp {
        /// The cursor x in design units.
        x: i64,
        /// The cursor y in design units.
        y: i64,
        /// Whether Ctrl was held.
        ctrl: bool,
    },
    /// A relevant key was pressed.
    Key {
        /// The logical key.
        key: KeyInput,
        /// Whether Ctrl was held.
        ctrl: bool,
        /// Whether Shift was held.
        shift: bool,
    },
}

/// Why a [`Designer`] could not be built or refreshed.
#[derive(Debug, thiserror::Error)]
pub enum DesignerError {
    /// The backend could not create the panel or overlay node.
    #[error(transparent)]
    Backend(#[from] BackendError),
    /// The form document could not be built into live widgets.
    #[error(transparent)]
    Build(#[from] BuildError),
    /// The replacement document has errors; the designer kept its current one.
    #[error("the form document is invalid: {}", first_message(.0))]
    Invalid(Vec<xui_form::Diagnostic>),
}

/// The first diagnostic's message, for [`DesignerError::Invalid`]'s display.
fn first_message(diagnostics: &[xui_form::Diagnostic]) -> &str {
    diagnostics
        .first()
        .map_or("unknown error", |diagnostic| diagnostic.message.as_str())
}

/// A callback the host registers to observe selection changes.
type SelectionSink = Rc<dyn Fn(&Selection)>;

/// A callback the host registers to observe node renames, so it can rewrite the
/// form's `.rhai` handler names (see [`rename_handlers`](crate::rename_handlers)).
type RenameSink = Rc<dyn Fn(&str, &str)>;

/// The binder used for the design preview; design mode never consults it.
struct NoopBinder;

impl<M: 'static> Binder<M> for NoopBinder {
    fn bind(&self, _event: EventRef<'_>) -> Option<EventHandler<M>> {
        None
    }
}

/// The form-designer surface: a live preview panel plus an input/painting
/// overlay.
pub struct Designer<M: 'static> {
    /// The handle the designer was built with. It owns the design-mode scope
    /// and the parent the preview panel and overlay belong to, so it is used
    /// for every node the designer creates even when the host forwards an
    /// [`update`](Designer::update) with a different handle (the IDE edits a
    /// form from a property grid in another pane).
    ui: Ui<M>,
    panel: Panel<M>,
    panel_origin: Point,
    /// The overlay node, recreated after every rebuild. The `Control` destroys
    /// the node when it is replaced or dropped.
    overlay: RefCell<Option<Control<M>>>,
    overlay_id: Cell<WidgetId>,
    surface: Rc<RefCell<Surface>>,
    live: RefCell<Option<LiveForm<M>>>,
    factories: Factories<M>,
    binder: NoopBinder,
    catalog: Rc<Catalog>,
    wrap: Rc<dyn Fn(DesignerMsg) -> M>,
    on_selection: RefCell<Vec<SelectionSink>>,
    on_rename: RefCell<Option<RenameSink>>,
    design_mode: Cell<bool>,
}

impl<M: 'static> Designer<M> {
    /// Builds a designer showing `doc`, rooted at `bounds` in the host window.
    ///
    /// `wrap` turns a [`DesignerMsg`] into the host's message type, so the
    /// overlay can route input back to [`Designer::update`]. The designer turns
    /// design mode on for the window (see the module docs).
    pub fn new(
        ui: &Ui<M>,
        bounds: Rect,
        doc: FormDoc,
        catalog: Rc<Catalog>,
        wrap: impl Fn(DesignerMsg) -> M + 'static,
    ) -> Result<Designer<M>, DesignerError> {
        // Design mode is switched on for the build; a failed construction must
        // put the window back as it was, or the host's controls stop working.
        let previous_design_mode = ui.is_design_mode();
        ui.set_design_mode(true);
        let restore = |error: DesignerError| {
            ui.set_design_mode(previous_design_mode);
            error
        };

        let surface = Rc::new(RefCell::new(Surface::new(
            doc,
            Rc::clone(&catalog),
            DEFAULT_GRID,
        )));
        let dpi = ui.dpi();
        let form = surface.borrow().form_rect();
        let size = Rect::new(
            0,
            0,
            Dip(form.right as f32).to_px(dpi).value(),
            Dip(form.bottom as f32).to_px(dpi).value(),
        );
        let panel_bounds = Rect::new(
            bounds.left,
            bounds.top,
            bounds.left + size.width(),
            bounds.top + size.height(),
        );
        let panel = Panel::new(ui, panel_bounds).map_err(|error| restore(error.into()))?;

        let designer = Designer {
            ui: ui.clone(),
            panel,
            panel_origin: Point::new(bounds.left, bounds.top),
            overlay: RefCell::new(None),
            overlay_id: Cell::new(WidgetId::NONE),
            surface,
            live: RefCell::new(None),
            factories: Factories::xui(),
            binder: NoopBinder,
            catalog,
            wrap: Rc::new(wrap),
            on_selection: RefCell::new(Vec::new()),
            on_rename: RefCell::new(None),
            design_mode: Cell::new(true),
        };
        designer.rebuild(ui).map_err(restore)?;
        Ok(designer)
    }

    /// The overlay's node identity.
    pub fn id(&self) -> WidgetId {
        self.overlay_id.get()
    }

    /// The panel that hosts the live widgets.
    pub fn panel_id(&self) -> WidgetId {
        self.panel.id()
    }

    /// The message wrapper the overlay uses, for a host that needs to route a
    /// message itself.
    pub fn wrap(&self) -> Rc<dyn Fn(DesignerMsg) -> M> {
        Rc::clone(&self.wrap)
    }

    /// Registers a sink called whenever the selection changes (the property
    /// grid of issue #14 connects here). Replaces any previous sink.
    pub fn set_selection_sink(&self, sink: impl Fn(&Selection) + 'static) {
        let mut sinks = self.on_selection.borrow_mut();
        sinks.clear();
        sinks.push(Rc::new(sink));
    }

    /// Adds a selection sink without removing the ones already registered, so a
    /// property grid and a host can both observe the selection.
    pub fn add_selection_sink(&self, sink: impl Fn(&Selection) + 'static) {
        self.on_selection.borrow_mut().push(Rc::new(sink));
    }

    /// Removes every selection sink.
    pub fn clear_selection_sink(&self) {
        self.on_selection.borrow_mut().clear();
    }

    /// Registers a sink called when a control is renamed, with the old and new
    /// names. The host rewrites the form's `.rhai` handlers from this (see
    /// [`rename_handlers`](crate::rename_handlers)). Replaces any previous sink.
    pub fn set_rename_sink(&self, sink: impl Fn(&str, &str) + 'static) {
        *self.on_rename.borrow_mut() = Some(Rc::new(sink));
    }

    /// Removes the rename sink.
    pub fn clear_rename_sink(&self) {
        *self.on_rename.borrow_mut() = None;
    }

    /// Whether the window is in design mode, as the designer set it.
    pub fn design_mode(&self) -> bool {
        self.design_mode.get()
    }

    /// The current selection.
    pub fn selection(&self) -> Selection {
        self.surface.borrow().selection().clone()
    }

    /// A clone of the document being edited.
    pub fn doc(&self) -> FormDoc {
        self.surface.borrow().doc().clone()
    }

    /// The value of `prop` on the live widget named `name`, if the preview
    /// built and the widget reports it. Useful to confirm an edit reached the
    /// live form without a rebuild.
    pub fn live_value(&self, name: &str, prop: &str) -> Option<Value> {
        self.live.borrow().as_ref()?.get(name, prop)
    }

    /// The grid spacing in design units.
    pub fn grid(&self) -> i64 {
        self.surface.borrow().grid()
    }

    /// Sets the grid spacing and repaints the overlay.
    pub fn set_grid(&self, grid: i64, ui: &Ui<M>) {
        self.surface.borrow_mut().set_grid(grid);
        ui.invalidate(self.id());
    }

    /// Whether an undo step is available.
    pub fn can_undo(&self) -> bool {
        self.surface.borrow().can_undo()
    }

    /// Whether a redo step is available.
    pub fn can_redo(&self) -> bool {
        self.surface.borrow().can_redo()
    }

    /// Handles one designer message from the overlay.
    pub fn update(&self, msg: DesignerMsg, ui: &Ui<M>) {
        let overlay = self.id();
        let outcome = match msg {
            DesignerMsg::PointerDown { x, y, ctrl } => {
                ui.set_capture(overlay);
                ui.focus(overlay);
                self.surface.borrow_mut().pointer_down(x, y, ctrl)
            }
            DesignerMsg::PointerMove { x, y, ctrl } => {
                self.surface.borrow_mut().pointer_move(x, y, ctrl)
            }
            DesignerMsg::PointerUp { x, y, ctrl } => {
                self.surface.borrow_mut().pointer_up(x, y, ctrl)
            }
            DesignerMsg::Key { key, ctrl, shift } => {
                self.surface.borrow_mut().key(KeyPress { key, ctrl, shift })
            }
        };
        if matches!(msg, DesignerMsg::PointerUp { .. }) {
            ui.release_capture();
        }
        ui.set_cursor(overlay, cursor_for(outcome.cursor));
        self.refresh(ui, outcome.change);
    }

    /// Replaces the document and rebuilds the preview.
    ///
    /// The document is validated against the catalog first. If it has errors,
    /// [`DesignerError::Invalid`] is returned; if it validates but fails to
    /// build, the build error is returned. Either way the current document,
    /// preview and undo history are left exactly as they were.
    pub fn set_doc(&self, doc: FormDoc, ui: &Ui<M>) -> Result<(), DesignerError> {
        let errors: Vec<xui_form::Diagnostic> = doc
            .validate(&self.catalog)
            .into_iter()
            .filter(|diagnostic| diagnostic.severity == xui_form::Severity::Error)
            .collect();
        if !errors.is_empty() {
            return Err(DesignerError::Invalid(errors));
        }
        // A document can pass validation and still fail to build (a missing
        // factory, a backend error), so keep the whole previous state and put
        // it back, preview included, if the rebuild fails.
        let previous = self.surface.borrow().clone();
        self.surface.borrow_mut().set_doc(doc);
        if let Err(error) = self.rebuild(ui) {
            // `rebuild` leaves the old preview in place when it fails, so
            // restoring the surface is all that is needed.
            *self.surface.borrow_mut() = previous;
            ui.invalidate(self.id());
            return Err(error);
        }
        self.notify_selection();
        ui.invalidate(self.id());
        Ok(())
    }

    /// Undoes the last change, returning whether anything changed.
    pub fn undo(&self, ui: &Ui<M>) -> bool {
        self.history_step(ui, Surface::undo)
    }

    /// Redoes the last undone change, returning whether anything changed.
    pub fn redo(&self, ui: &Ui<M>) -> bool {
        self.history_step(ui, Surface::redo)
    }

    /// Applies an undo or redo step, then reports every control rename it
    /// reversed or re-applied through the rename sink, so the host rewrites the
    /// form's handlers to match (a rename is undoable like any other edit).
    fn history_step(&self, ui: &Ui<M>, step: impl FnOnce(&mut Surface) -> bool) -> bool {
        let before = self.doc();
        let changed = self.apply(ui, |surface| {
            if step(surface) {
                Outcome::changed(Change::STRUCTURE)
            } else {
                Outcome::none()
            }
        });
        if changed {
            for (old, new) in renamed_nodes(&before, &self.doc()) {
                self.notify_rename(&old, &new);
            }
        }
        changed
    }

    /// Copies the selection to the in-process clipboard, returning whether
    /// anything was copied. Takes no `Ui` because nothing on screen changes.
    pub fn copy(&self) -> bool {
        self.surface.borrow_mut().copy()
    }

    /// Pastes the clipboard, returning whether anything was pasted.
    pub fn paste(&self, ui: &Ui<M>) -> bool {
        self.apply(ui, |surface| {
            if surface.paste() {
                Outcome::changed(Change::STRUCTURE)
            } else {
                Outcome::none()
            }
        })
    }

    /// Arms the creation tool used by click-then-drag; `None` restores the
    /// pointer. An unknown kind is rejected.
    pub fn set_tool(&self, tool: Option<&str>, ui: &Ui<M>) {
        self.surface.borrow_mut().set_tool(tool);
        ui.invalidate(self.id());
    }

    /// The armed creation tool, or `None` for the pointer.
    pub fn tool(&self) -> Option<String> {
        self.surface.borrow().tool().map(str::to_owned)
    }

    /// Drops a control of `kind` at its catalog default size in the centre of
    /// the form, as an undoable command. Returns the new node's name.
    pub fn drop_control(&self, kind: &str, ui: &Ui<M>) -> Option<String> {
        let name = self.surface.borrow_mut().drop_control(kind);
        if name.is_some() {
            self.refresh(ui, Change::STRUCTURE);
        }
        name
    }

    /// Applies a [`ToolboxMsg`]: a click arms the tool, a double-click drops a
    /// control. The host forwards the toolbox's wrapped message here from its
    /// `update`.
    pub fn handle_toolbox(&self, msg: ToolboxMsg, ui: &Ui<M>) {
        match msg {
            ToolboxMsg::Select(tool) => self.set_tool(tool.kind(), ui),
            ToolboxMsg::Activate(tool) => {
                if let Some(kind) = tool.kind() {
                    self.drop_control(kind, ui);
                }
            }
        }
    }

    /// Duplicates the selection, returning whether anything was duplicated.
    pub fn duplicate(&self, ui: &Ui<M>) -> bool {
        self.apply(ui, |surface| {
            if surface.duplicate() {
                Outcome::changed(Change::STRUCTURE)
            } else {
                Outcome::none()
            }
        })
    }

    /// Deletes the selection, returning whether anything was deleted.
    pub fn delete_selection(&self, ui: &Ui<M>) -> bool {
        self.apply(ui, Surface::delete_selection)
    }

    /// Selects every control.
    pub fn select_all(&self, ui: &Ui<M>) {
        self.apply(ui, |surface| {
            surface.select_all();
            Outcome::changed(Change::SELECTION)
        });
    }

    /// Selects the form.
    pub fn select_form(&self, ui: &Ui<M>) {
        self.apply(ui, |surface| {
            surface.select_form();
            Outcome::changed(Change::SELECTION)
        });
    }

    /// Selects the node named `name`, if it exists, returning whether the
    /// selection changed.
    pub fn select_node(&self, name: &str, ui: &Ui<M>) -> bool {
        self.apply(ui, |surface| {
            if surface.select_node(name) {
                Outcome::changed(Change::SELECTION)
            } else {
                Outcome::none()
            }
        })
    }

    /// Selects a property-grid target (the form or a node), returning whether
    /// the selection changed.
    pub fn select_object(&self, target: &Target, ui: &Ui<M>) -> bool {
        match target {
            Target::Form => {
                let changed = !self.selection().is_form();
                self.apply(ui, |surface| {
                    surface.select_form();
                    Outcome::changed(Change::SELECTION)
                });
                changed
            }
            Target::Node(name) => self.select_node(name, ui),
        }
    }

    /// Applies one undoable property edit (the property grid's command) and
    /// refreshes the live preview. Renaming a node also notifies the rename
    /// sink. Returns whether anything changed.
    pub fn set_property(
        &self,
        target: &Target,
        name: &str,
        value: Value,
        ui: &Ui<M>,
    ) -> Result<bool, PropertyError> {
        let old_name = match (target, name) {
            (Target::Node(node), "name") => self
                .surface
                .borrow()
                .doc()
                .node(node)
                .map(|node| node.name.clone()),
            _ => None,
        };
        let change = self
            .surface
            .borrow_mut()
            .set_property(target, name, value.clone())?;
        // Only a real rename reaches the host (it rewrites the form's script).
        if let (Some(old), Value::Text(new)) = (old_name, &value)
            && change.any()
            && old != *new
        {
            self.notify_rename(&old, new);
        }
        self.refresh(ui, change);
        Ok(change.any())
    }

    /// The form's name followed by every control's name, in document order,
    /// paired with the property-grid target each identifies.
    pub fn objects(&self) -> Vec<(String, Target)> {
        let surface = self.surface.borrow();
        let doc = surface.doc();
        let mut objects = vec![(doc.window.name.clone(), Target::Form)];
        objects.extend(
            doc.nodes
                .iter()
                .map(|node| (node.name.clone(), Target::Node(node.name.clone()))),
        );
        objects
    }

    /// Runs a surface command and refreshes the live preview, returning whether
    /// anything changed.
    fn apply(&self, ui: &Ui<M>, command: impl FnOnce(&mut Surface) -> Outcome) -> bool {
        let change = command(&mut self.surface.borrow_mut()).change;
        self.refresh(ui, change);
        change.any()
    }

    /// Applies a change to the live widgets: rebuild after a structural change,
    /// otherwise re-apply geometry; then notify the selection sink and repaint.
    fn refresh(&self, ui: &Ui<M>, change: Change) {
        if change.structure {
            // A failed preview build keeps the overlay (so the user can undo
            // the edit that caused it); there is no caller to report to here.
            let _ = self.rebuild(ui);
            // The rebuild replaced the overlay node: give the new one focus, so
            // a keyboard command (Delete, then Ctrl+Z) keeps reaching it.
            ui.focus(self.id());
        } else if change.geometry {
            self.sync_geometry(ui);
        } else if change.property {
            self.sync_properties();
        }
        if change.selection {
            self.notify_selection();
        }
        ui.invalidate(self.id());
    }

    /// Destroys the live widgets and the overlay, rebuilds the widgets from the
    /// document, and recreates the overlay above them.
    ///
    /// The overlay is reinstalled even when the preview fails to build, so the
    /// designer keeps receiving input and the user can undo the edit that broke
    /// it; the build error is still returned.
    ///
    /// The replacement is built before anything is torn down: if the build
    /// fails, the current preview and overlay stay exactly as they were (any
    /// widgets the failed build created are dropped with the error), so the
    /// designer keeps working and the edit can be undone.
    fn rebuild(&self, ui: &Ui<M>) -> Result<(), DesignerError> {
        let doc = self.surface.borrow().doc().clone();
        let form = build_with(
            self.panel.ui(),
            &doc,
            &self.catalog,
            &self.factories,
            &self.binder,
            BuildOptions { design_mode: true },
        )?;
        // Create the new overlay before touching the current preview, so a
        // failure leaves the old preview and overlay in place.
        let overlay = self.create_overlay()?;
        *self.live.borrow_mut() = Some(form);
        self.resize_panel(ui);
        self.overlay_id.set(overlay.id());
        *self.overlay.borrow_mut() = Some(overlay);
        Ok(())
    }

    /// Creates the transparent overlay above the live widgets and wires its
    /// painter and event mapper; the caller installs it.
    ///
    /// The overlay is created through the handle the designer was built with,
    /// not the one an [`update`](Designer::update) arrived on, so a host that
    /// drives the model from elsewhere (the IDE's property grid) cannot
    /// re-parent the overlay out of its pane.
    fn create_overlay(&self) -> Result<Control<M>, BackendError> {
        let (width, height) = self.form_px(&self.ui);
        let origin = self.panel_origin;
        let bounds = Rect::new(origin.x, origin.y, origin.x + width, origin.y + height);
        let overlay = Control::new(&self.ui, &NodeSpec::new(NodeKind::Custom, bounds))?;

        let surface = Rc::clone(&self.surface);
        let theme = self.ui.theme_handle();
        overlay.set_painter(Rc::new(move |canvas| {
            paint_local(canvas, |canvas, _| {
                paint(canvas, &surface.borrow(), &theme.get());
            });
        }));

        let wrap = Rc::clone(&self.wrap);
        let ui_for_events = self.ui.clone();
        overlay
            .on_events(move |event| designer_message(event, &ui_for_events).map(|msg| wrap(msg)));
        Ok(overlay)
    }

    /// Pushes the document's geometry into the live widgets and resizes the
    /// panel/overlay when the form's client area changed.
    fn sync_geometry(&self, ui: &Ui<M>) {
        {
            let surface = self.surface.borrow();
            if let Some(live) = self.live.borrow().as_ref() {
                for node in &surface.doc().nodes {
                    let rect = surface.node_local_rect(node);
                    let _ = live.set(&node.name, "left", &Value::Int(rect.left));
                    let _ = live.set(&node.name, "top", &Value::Int(rect.top));
                    let _ = live.set(&node.name, "width", &Value::Int(rect.width()));
                    let _ = live.set(&node.name, "height", &Value::Int(rect.height()));
                }
            }
        }
        let bounds = self.resize_panel(ui);
        ui.apply_moves(&[(self.id(), bounds)]);
    }

    /// Pushes every node's stored property into the live widgets, so a
    /// non-geometry edit (`text`, `enabled`, `visible`, `checked`, …) shows
    /// immediately without a full rebuild. Values a widget does not report are
    /// ignored.
    fn sync_properties(&self) {
        {
            let surface = self.surface.borrow();
            if let Some(live) = self.live.borrow().as_ref() {
                for node in &surface.doc().nodes {
                    for (name, value) in &node.props {
                        let _ = live.set(&node.name, name, value);
                    }
                }
            }
        }
    }

    /// Sizes the preview panel to the document's client area and returns its
    /// bounds. Called on geometry edits and on every rebuild, so an undo, redo
    /// or `set_doc` that changes the form size keeps panel and overlay in step.
    fn resize_panel(&self, ui: &Ui<M>) -> Rect {
        let (width, height) = self.form_px(ui);
        let origin = self.panel_origin;
        let bounds = Rect::new(origin.x, origin.y, origin.x + width, origin.y + height);
        self.panel.set_bounds(bounds);
        bounds
    }

    /// The form's device-pixel size.
    fn form_px(&self, ui: &Ui<M>) -> (i32, i32) {
        let dpi = ui.dpi();
        let form = self.surface.borrow().form_rect();
        (
            Dip(form.right as f32).to_px(dpi).value(),
            Dip(form.bottom as f32).to_px(dpi).value(),
        )
    }

    /// Calls every selection sink, if any, with the current selection.
    fn notify_selection(&self) {
        let selection = self.surface.borrow().selection().clone();
        for sink in self.on_selection.borrow().iter() {
            sink(&selection);
        }
    }

    /// Calls the rename sink, if any, with the old and new control names.
    fn notify_rename(&self, old: &str, new: &str) {
        if let Some(sink) = self.on_rename.borrow().as_ref() {
            sink(old, new);
        }
    }
}

/// The node renames between two versions of a form: `(old, new)` for each node
/// that kept its position and kind but changed its name. A rename edits a node
/// in place, so an undo or redo of one shows up exactly like this; a step that
/// adds or removes nodes renames nothing.
fn renamed_nodes(before: &FormDoc, after: &FormDoc) -> Vec<(String, String)> {
    if before.nodes.len() != after.nodes.len() {
        return Vec::new();
    }
    before
        .nodes
        .iter()
        .zip(&after.nodes)
        .filter(|(old, new)| old.name != new.name && old.kind == new.kind)
        .map(|(old, new)| (old.name.clone(), new.name.clone()))
        .collect()
}

/// Translates a local overlay event into a [`DesignerMsg`], dropping events the
/// designer does not use.
fn designer_message<M: 'static>(event: &Event, ui: &Ui<M>) -> Option<DesignerMsg> {
    let dpi = ui.dpi();
    let to = |value: i32| Px(value).to_dip(dpi).value().round() as i64;
    match event {
        Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            modifiers,
        } => Some(DesignerMsg::PointerDown {
            x: to(*x),
            y: to(*y),
            ctrl: modifiers.ctrl,
        }),
        Event::MouseMove { x, y, modifiers } => Some(DesignerMsg::PointerMove {
            x: to(*x),
            y: to(*y),
            ctrl: modifiers.ctrl,
        }),
        Event::MouseUp {
            x,
            y,
            button: MouseButton::Left,
            modifiers,
        } => Some(DesignerMsg::PointerUp {
            x: to(*x),
            y: to(*y),
            ctrl: modifiers.ctrl,
        }),
        Event::KeyDown {
            key,
            modifiers,
            system,
            ..
        } if !*system => key_input(*key, modifiers.ctrl).map(|key| DesignerMsg::Key {
            key,
            ctrl: modifiers.ctrl,
            shift: modifiers.shift,
        }),
        _ => None,
    }
}

/// Maps an xui virtual key to a designer action, requiring Ctrl for the letter
/// shortcuts so ordinary typing is not swallowed.
fn key_input(key: Key, ctrl: bool) -> Option<KeyInput> {
    Some(match key {
        Key::LEFT => KeyInput::Left,
        Key::RIGHT => KeyInput::Right,
        Key::UP => KeyInput::Up,
        Key::DOWN => KeyInput::Down,
        Key::DELETE => KeyInput::Delete,
        Key::BACK => KeyInput::Backspace,
        Key::ESCAPE => KeyInput::Escape,
        Key::C if ctrl => KeyInput::Copy,
        Key::V if ctrl => KeyInput::Paste,
        Key::D if ctrl => KeyInput::Duplicate,
        Key::Z if ctrl => KeyInput::Undo,
        Key::Y if ctrl => KeyInput::Redo,
        Key::A if ctrl => KeyInput::SelectAll,
        _ => return None,
    })
}

/// Maps a design-unit cursor hint to an xui cursor.
fn cursor_for(hint: CursorHint) -> Cursor {
    match hint {
        CursorHint::Default => Cursor::Default,
        CursorHint::SizeHorizontal => Cursor::SizeHorizontal,
        CursorHint::SizeVertical => Cursor::SizeVertical,
    }
}

/// Converts a design-unit rectangle to device pixels.
fn px_rect(rect: DesignRect, dpi: u32) -> Rect {
    let to = |value: i64| Dip(value as f32).to_px(dpi).value();
    Rect::new(to(rect.left), to(rect.top), to(rect.right), to(rect.bottom))
}

/// Draws the dot grid, the form and selection outlines, the handles, and the
/// marquee and drag previews into the overlay's own pixels.
fn paint(canvas: &mut dyn Canvas, surface: &Surface, theme: &Theme) {
    let dpi = canvas.dpi();
    let form = surface.form_rect();

    let step = surface.grid().max(1);
    // A pathological grid (1 unit over a large form) would draw millions of
    // dots, so cap the work and let the grid disappear instead.
    if (form.width() / step) * (form.height() / step) <= 250_000 {
        let dot = Dip(1.0).to_px(dpi).value().max(1);
        let grid = Rgba::with_alpha(theme.border.r, theme.border.g, theme.border.b, 0x40);
        let mut x = 0;
        while x <= form.right {
            let px = Dip(x as f32).to_px(dpi).value();
            let mut y = 0;
            while y <= form.bottom {
                let py = Dip(y as f32).to_px(dpi).value();
                canvas.fill_rect_rgba(Rect::new(px, py, px + dot, py + dot), grid);
                y += step;
            }
            x += step;
        }
    }

    canvas.stroke_rect(px_rect(form, dpi), theme.border, 1.0);

    let bounds = surface.selection_bounds();
    canvas.stroke_rect(px_rect(bounds, dpi), theme.accent, 2.0);

    let handle_size = Dip(7.0).to_px(dpi).value().max(3);
    // Handles only where a drag on them resizes: the form or one node. With
    // several nodes (or none) selected, a drag there would move or marquee.
    let resizable = match surface.selection() {
        Selection::Form => true,
        Selection::Nodes(names) => names.len() == 1,
    };
    for handle in Handle::ALL.into_iter().filter(|_| resizable) {
        let (cx, cy) = handle.center(bounds);
        let center = Point::new(
            Dip(cx as f32).to_px(dpi).value(),
            Dip(cy as f32).to_px(dpi).value(),
        );
        let rect = Rect::new(
            center.x - handle_size / 2,
            center.y - handle_size / 2,
            center.x + handle_size / 2,
            center.y + handle_size / 2,
        );
        canvas.fill_rect(rect, theme.background);
        canvas.stroke_rect(rect, theme.accent, 2.0);
    }

    if let Some(marquee) = surface.marquee() {
        dashed_rect(canvas, px_rect(marquee, dpi), theme.accent);
    }
    if let Some(preview) = surface.preview() {
        dashed_rect(canvas, px_rect(preview, dpi), theme.accent);
    }
}

/// Strokes a dashed rectangle, used for the marquee and the drag preview.
fn dashed_rect(canvas: &mut dyn Canvas, rect: Rect, color: Color) {
    let color: Rgba = color.into();
    let stroke = Stroke::new(1.0).dash(Dash::Dashed);
    let top_left = Point::new(rect.left, rect.top);
    let top_right = Point::new(rect.right, rect.top);
    let bottom_right = Point::new(rect.right, rect.bottom);
    let bottom_left = Point::new(rect.left, rect.bottom);
    canvas.draw_line_stroked(top_left, top_right, color, &stroke);
    canvas.draw_line_stroked(top_right, bottom_right, color, &stroke);
    canvas.draw_line_stroked(bottom_right, bottom_left, color, &stroke);
    canvas.draw_line_stroked(bottom_left, top_left, color, &stroke);
}
