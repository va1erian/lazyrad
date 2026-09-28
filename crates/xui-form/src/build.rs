#![forbid(unsafe_code)]

//! Building live `xui` widgets from a [`FormDoc`].
//!
//! The host supplies a [`Binder`] that decides what each event becomes, and a
//! [`Factories`] registry that knows how to create each kind. [`build`] walks
//! the flat node list, creates every widget through its factory and wires the
//! events the binder returns. The resulting [`LiveForm`] owns the widgets and
//! can read and write their properties, and re-anchor them when the window is
//! resized.
//!
//! A factory only ever sees the portable [`crate::schema`] and the
//! [`BuildCx`] helpers, so registering a custom widget needs no change here.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use xui_core::WidgetId;
use xui_core::app::Ui;
use xui_core::backend::{BackendError, Result as BackendResult};
use xui_core::geometry::{Rect, Size};
use xui_core::layout::{Anchor, anchored};
use xui_core::units::Dip;

use crate::doc::{FormDoc, Node};
use crate::schema::{Access, Catalog, EventSpec, WidgetSpec};
use crate::value::Value;

/// How a property write failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SetError {
    /// No widget of that name exists in the form.
    #[error("unknown widget")]
    UnknownWidget,
    /// No property of that name exists.
    #[error("unknown property")]
    UnknownProperty,
    /// The property exists but the value has the wrong type.
    #[error("the value has the wrong type")]
    TypeMismatch,
    /// The property cannot be written here (read-only, or design-only at
    /// runtime, or runtime-only in the designer).
    #[error("the property is read-only here")]
    ReadOnly,
}

/// How a build failed.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// A node names a kind the catalog does not know.
    #[error("unknown widget kind `{kind}` for node `{node}`")]
    UnknownKind {
        /// The unknown kind.
        kind: String,
        /// The node that used it.
        node: String,
    },
    /// A node names a kind no factory is registered for.
    #[error("no factory registered for kind `{kind}` (node `{node}`)")]
    UnknownFactory {
        /// The kind without a factory.
        kind: String,
        /// The node that used it.
        node: String,
    },
    /// A node names a parent that is missing or is not a container.
    #[error("parent `{parent}` of node `{node}` does not exist or is not a container")]
    UnknownParent {
        /// The node with the bad parent.
        node: String,
        /// The parent name.
        parent: String,
    },
    /// The backend could not create a node.
    #[error(transparent)]
    Backend(#[from] BackendError),
}

/// Options for [`build_with`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildOptions {
    /// Whether the form is being built for the designer. In design mode no
    /// binder is consulted, and design-only properties may be written.
    pub design_mode: bool,
}

/// A host event handler: it receives the event's typed arguments and returns
/// the host's message, or `None` to ignore the event.
pub type EventHandler<M> = Rc<dyn Fn(&[Value]) -> Option<M>>;

/// A reference to one event a document binds.
#[derive(Clone, Copy, Debug)]
pub struct EventRef<'a> {
    /// The node that raises the event.
    pub node: &'a str,
    /// The event's name.
    pub event: &'a str,
    /// The event's schema.
    pub spec: &'a EventSpec,
}

/// Decides what a document's event bindings become.
///
/// Returning `None` leaves the event unwired. The closure receives the event's
/// arguments as typed [`Value`]s and returns the host's message.
pub trait Binder<M> {
    /// Binds one event, or returns `None` to leave it unwired.
    fn bind(&self, event: EventRef<'_>) -> Option<EventHandler<M>>;
}

/// A live widget created from a [`Node`].
pub trait LiveWidget<M: 'static> {
    /// The widget's node identity.
    fn id(&self) -> WidgetId;

    /// The widget-specific property named `prop`, if it has one.
    fn get(&self, prop: &str) -> Option<Value>;

    /// Sets the widget-specific property named `prop`.
    fn set(&self, prop: &str, value: &Value) -> Result<(), SetError>;

    /// The scoped [`Ui`] children of this widget should be created through, for
    /// a container; `None` for a leaf.
    fn container_ui(&self) -> Option<&Ui<M>> {
        None
    }

    /// Where each of the widget's nodes goes when the widget is placed at
    /// `rect` (device pixels). A widget made of several nodes, such as a
    /// `RadioGroup` with one node per option, returns them all so a relayout
    /// moves the whole widget.
    fn placements(&self, rect: Rect) -> Vec<(WidgetId, Rect)> {
        vec![(self.id(), rect)]
    }

    /// Every node the widget owns; the first is [`LiveWidget::id`].
    fn node_ids(&self) -> Vec<WidgetId> {
        vec![self.id()]
    }
}

/// Creates the live widget for one kind.
pub trait WidgetFactory<M: 'static> {
    /// The canonical kind this factory builds.
    fn kind(&self) -> &str;

    /// Creates a widget for `node`, wiring its events through `cx`.
    fn create(&self, cx: &mut BuildCx<'_, M>, node: &Node)
    -> BackendResult<Box<dyn LiveWidget<M>>>;
}

/// A registry of widget factories, keyed by canonical kind.
pub struct Factories<M: 'static> {
    factories: BTreeMap<String, Box<dyn WidgetFactory<M>>>,
}

impl<M: 'static> Default for Factories<M> {
    fn default() -> Self {
        Factories::new()
    }
}

impl<M: 'static> Factories<M> {
    /// An empty registry.
    pub fn new() -> Self {
        Factories {
            factories: BTreeMap::new(),
        }
    }

    /// Registers a factory, replacing any with the same kind.
    pub fn register(&mut self, factory: impl WidgetFactory<M> + 'static) {
        self.factories
            .insert(factory.kind().to_owned(), Box::new(factory));
    }

    /// The factory for the canonical `kind`, if any.
    pub fn get(&self, kind: &str) -> Option<&dyn WidgetFactory<M>> {
        self.factories.get(kind).map(Box::as_ref)
    }

    /// The kinds with a registered factory, in sorted order.
    pub fn kinds(&self) -> impl Iterator<Item = &str> {
        self.factories.keys().map(String::as_str)
    }
}

/// What a factory receives when it creates a widget.
pub struct BuildCx<'a, M: 'static> {
    ui: &'a Ui<M>,
    node: &'a Node,
    spec: &'a WidgetSpec,
    rect: Rect,
    design: (i64, i64, i64, i64),
    binder: &'a dyn Binder<M>,
    design_mode: bool,
    catalog: Rc<Catalog>,
}

impl<'a, M: 'static> BuildCx<'a, M> {
    /// The [`Ui`] children should be parented to (the parent container, or the
    /// window).
    pub fn ui(&self) -> &Ui<M> {
        self.ui
    }

    /// The node being built.
    pub fn node(&self) -> &Node {
        self.node
    }

    /// The node's widget spec.
    pub fn spec(&self) -> &WidgetSpec {
        self.spec
    }

    /// The node's pixel rectangle.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Whether this is a design-mode build.
    pub fn design_mode(&self) -> bool {
        self.design_mode
    }

    /// The raw value of a property, if set.
    pub fn prop(&self, name: &str) -> Option<&Value> {
        self.node.props.get(name)
    }

    /// A text property, or the empty string.
    pub fn text(&self, name: &str) -> String {
        self.prop(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    /// A bool property, or `default`.
    pub fn bool(&self, name: &str, default: bool) -> bool {
        self.prop(name).and_then(Value::as_bool).unwrap_or(default)
    }

    /// An int property, or `default`.
    pub fn int(&self, name: &str, default: i64) -> i64 {
        self.prop(name).and_then(Value::as_int).unwrap_or(default)
    }

    /// A float property, or `default`.
    pub fn float(&self, name: &str, default: f64) -> f64 {
        self.prop(name).and_then(Value::as_float).unwrap_or(default)
    }

    /// A list property, or an empty vector.
    pub fn list(&self, name: &str) -> Vec<String> {
        self.prop(name)
            .and_then(Value::as_list)
            .map(<[String]>::to_vec)
            .unwrap_or_default()
    }

    /// The handler for `event`, if the binder supplied one.
    ///
    /// In design mode this is always `None`, so the designer's preview does not
    /// run the host's event code.
    pub fn handler(&self, event: &str) -> Option<EventHandler<M>> {
        if self.design_mode {
            return None;
        }
        let spec = self.spec.event(event)?;
        self.binder.bind(EventRef {
            node: &self.node.name,
            event,
            spec,
        })
    }

    /// Wraps a widget's own property surface with the common property handling.
    pub(crate) fn live<W: WidgetProps<M>>(&self, inner: W) -> Box<dyn LiveWidget<M>> {
        let common = Common::new(self.ui.clone(), self.design);
        Box::new(Live {
            common,
            inner,
            catalog: Rc::clone(&self.catalog),
            kind: self.spec.kind.clone(),
            design_mode: self.design_mode,
        })
    }
}

/// The widget-specific property surface a built-in widget implements.
///
/// The [`Live`] wrapper adds the common properties (geometry, `anchor`,
/// `visible`, `enabled`, `tab_index`) and the schema's access and type rules.
pub(crate) trait WidgetProps<M: 'static>: 'static {
    /// The widget's node identity.
    fn id(&self) -> WidgetId;
    /// Reads a widget-specific property.
    fn get_own(&self, prop: &str) -> Option<Value>;
    /// Writes a widget-specific property.
    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError>;
    /// The scoped [`Ui`] for a container.
    fn container_ui(&self) -> Option<&Ui<M>> {
        None
    }
    /// Notifies the widget that its `enabled` common property changed, so a
    /// widget with its own enabled state can dim itself. The default does
    /// nothing.
    fn set_enabled_hint(&self, _enabled: bool) {}

    /// Every node the widget owns; the first is [`WidgetProps::id`].
    fn node_ids(&self) -> Vec<WidgetId> {
        vec![self.id()]
    }

    /// Where each node goes when the widget is placed at `rect`.
    fn placements(&self, _ui: &Ui<M>, rect: Rect) -> Vec<(WidgetId, Rect)> {
        vec![(self.id(), rect)]
    }
}

/// One node's static layout metadata, kept for [`LiveForm::relayout`].
#[derive(Clone, Debug)]
struct NodeMeta {
    name: String,
    parent: Option<String>,
    id: WidgetId,
    design: (i64, i64, i64, i64),
    anchor: Anchor,
    is_container: bool,
}

/// A built form: every live widget plus the metadata needed to relayout it.
pub struct LiveForm<M: 'static> {
    ui: Ui<M>,
    #[allow(dead_code)]
    catalog: Rc<Catalog>,
    widgets: Vec<Box<dyn LiveWidget<M>>>,
    by_name: BTreeMap<String, usize>,
    nodes: Vec<NodeMeta>,
    design_size: (i64, i64),
}

impl<M: 'static> LiveForm<M> {
    /// The widget named `name`, if any.
    pub fn widget(&self, name: &str) -> Option<&dyn LiveWidget<M>> {
        self.by_name
            .get(name)
            .and_then(|index| self.widgets.get(*index))
            .map(Box::as_ref)
    }

    /// The value of `prop` on the widget named `name`, if any.
    pub fn get(&self, name: &str, prop: &str) -> Option<Value> {
        self.widget(name)?.get(prop)
    }

    /// Sets `prop` on the widget named `name`.
    pub fn set(&self, name: &str, prop: &str, value: &Value) -> Result<(), SetError> {
        self.widget(name)
            .ok_or(SetError::UnknownWidget)?
            .set(prop, value)
    }

    /// Every widget's node identity, in creation order.
    pub fn ids(&self) -> Vec<WidgetId> {
        self.nodes.iter().map(|node| node.id).collect()
    }

    /// The pixel rectangle of the widget named `name`, if any.
    pub fn bounds(&self, name: &str) -> Option<Rect> {
        let node = self.nodes.iter().find(|node| node.name == name)?;
        Some(self.ui.bounds(node.id))
    }

    /// The node names, in creation order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.nodes.iter().map(|node| node.name.as_str())
    }

    /// The current bounds of every node the widget named `name` owns: one for
    /// most widgets, one per option for a `RadioGroup`. A designer outlines
    /// the union of these.
    pub fn node_bounds(&self, name: &str) -> Vec<Rect> {
        self.widget(name)
            .map(|widget| {
                widget
                    .node_ids()
                    .into_iter()
                    .map(|id| self.ui.bounds(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Re-anchors every node from the design size to `new_client_size`.
    ///
    /// Each node is placed with [`anchored`] against its parent's design and
    /// new size, so a [`Anchor::Fill`] child grows with its container and a
    /// [`Anchor::BottomRight`] child keeps its offset from the corner. Every
    /// move is applied in one [`Ui::apply_moves`] call.
    pub fn relayout(&self, new_client_size: Size) {
        let dpi = self.ui.dpi();
        let design = px_size(self.design_size, dpi);
        let mut moves = Vec::new();
        self.anchor_children(None, design, new_client_size, dpi, &mut moves);
        self.ui.apply_moves(&moves);
    }

    /// Recursively computes the moves for the children of `parent`.
    fn anchor_children(
        &self,
        parent: Option<&str>,
        parent_design: Size,
        parent_new: Size,
        dpi: u32,
        moves: &mut Vec<(WidgetId, Rect)>,
    ) {
        for (index, node) in self.nodes.iter().enumerate() {
            if node.parent.as_deref() != parent {
                continue;
            }
            let (geometry, anchor) = self.current_placement(index, node);
            let design = px_rect(geometry, dpi);
            let placed = anchored(parent_design, parent_new, design, anchor);
            moves.extend(self.widgets[index].placements(placed));
            if node.is_container {
                self.anchor_children(Some(&node.name), design.size(), placed.size(), dpi, moves);
            }
        }
    }

    /// The design rectangle and anchor of the node at `index`, read from the
    /// live widget so edits made through [`LiveForm::set`] survive a relayout.
    /// A custom widget that does not report a common property falls back to
    /// the value the node was built with.
    fn current_placement(&self, index: usize, node: &NodeMeta) -> ((i64, i64, i64, i64), Anchor) {
        let widget = &self.widgets[index];
        let int = |prop: &str, fallback: i64| {
            widget
                .get(prop)
                .and_then(|value| value.as_int())
                .unwrap_or(fallback)
        };
        let (left, top, width, height) = node.design;
        let geometry = (
            int("left", left),
            int("top", top),
            int("width", width),
            int("height", height),
        );
        let anchor = widget
            .get("anchor")
            .as_ref()
            .and_then(Value::as_str)
            .and_then(crate::schema::anchor_from_name)
            .unwrap_or(node.anchor);
        (geometry, anchor)
    }
}

/// Builds every widget in `doc`, wiring events through `binder`.
pub fn build<M: 'static>(
    ui: &Ui<M>,
    doc: &FormDoc,
    catalog: &Catalog,
    factories: &Factories<M>,
    binder: &dyn Binder<M>,
) -> Result<LiveForm<M>, BuildError> {
    build_with(ui, doc, catalog, factories, binder, BuildOptions::default())
}

/// Builds every widget in `doc` with explicit [`BuildOptions`].
pub fn build_with<M: 'static>(
    ui: &Ui<M>,
    doc: &FormDoc,
    catalog: &Catalog,
    factories: &Factories<M>,
    binder: &dyn Binder<M>,
    options: BuildOptions,
) -> Result<LiveForm<M>, BuildError> {
    let catalog = Rc::new(catalog.clone());
    let dpi = ui.dpi();
    let mut widgets: Vec<Box<dyn LiveWidget<M>>> = Vec::new();
    let mut by_name = BTreeMap::new();
    let mut nodes = Vec::new();
    let mut container_uis: BTreeMap<String, Ui<M>> = BTreeMap::new();

    // A container must exist before its children, whatever order the document
    // lists them in (`reparent` does not move nodes, and files may be hand
    // written), so build parents first.
    for node in build_order(&doc.nodes)
        .into_iter()
        .map(|index| &doc.nodes[index])
    {
        let spec = catalog
            .get(&node.kind)
            .ok_or_else(|| BuildError::UnknownKind {
                kind: node.kind.clone(),
                node: node.name.clone(),
            })?;

        let parent_ui = match node.parent.as_deref() {
            None => ui.clone(),
            Some(parent) => {
                container_uis
                    .get(parent)
                    .cloned()
                    .ok_or_else(|| BuildError::UnknownParent {
                        node: node.name.clone(),
                        parent: parent.to_owned(),
                    })?
            }
        };

        let design = design_rect(node, spec);
        let rect = px_rect(design, dpi);
        let mut cx = BuildCx {
            ui: &parent_ui,
            node,
            spec,
            rect,
            design,
            binder,
            design_mode: options.design_mode,
            catalog: Rc::clone(&catalog),
        };

        let factory = factories
            .get(&spec.kind)
            .ok_or_else(|| BuildError::UnknownFactory {
                kind: spec.kind.clone(),
                node: node.name.clone(),
            })?;
        let widget = factory.create(&mut cx, node)?;
        let scoped = widget.container_ui().cloned();

        // Geometry is applied by the factory (it constructed the widget at
        // `cx.rect()`), so only the non-geometry common properties are pushed
        // here. This also keeps a multi-node widget such as `RadioGroup` from
        // being moved as if it were a single node.
        for name in ["anchor", "visible", "enabled", "tab_index"] {
            if let Some(value) = node.props.get(name) {
                let _ = widget.set(name, value);
            }
        }

        let id = widget.id();
        let index = widgets.len();
        widgets.push(widget);
        by_name.insert(node.name.clone(), index);
        nodes.push(NodeMeta {
            name: node.name.clone(),
            parent: node.parent.clone(),
            id,
            design,
            anchor: anchor_of(node),
            is_container: scoped.is_some(),
        });
        if let Some(scoped) = scoped {
            container_uis.insert(node.name.clone(), scoped);
        }
    }

    let design_size = (
        doc.window
            .prop("width")
            .and_then(Value::as_int)
            .unwrap_or(320),
        doc.window
            .prop("height")
            .and_then(Value::as_int)
            .unwrap_or(200),
    );

    Ok(LiveForm {
        ui: ui.clone(),
        catalog,
        widgets,
        by_name,
        nodes,
        design_size,
    })
}

/// The indices of `nodes` in an order where every parent comes before its
/// children, keeping the document order among nodes that are ready together.
///
/// Nodes whose parent never becomes available (a missing parent or a cycle)
/// are appended in document order, so the build reports them as
/// [`BuildError::UnknownParent`].
fn build_order(nodes: &[Node]) -> Vec<usize> {
    let mut order = Vec::with_capacity(nodes.len());
    let mut placed = std::collections::BTreeSet::new();
    let mut pending: Vec<usize> = (0..nodes.len()).collect();
    loop {
        let before = pending.len();
        pending.retain(|&index| {
            let ready = nodes[index]
                .parent
                .as_deref()
                .is_none_or(|parent| placed.contains(parent));
            if ready {
                order.push(index);
                placed.insert(nodes[index].name.as_str());
            }
            !ready
        });
        if pending.is_empty() || pending.len() == before {
            break;
        }
    }
    order.extend(pending);
    order
}

/// The design rectangle of a node, in DIPs, filling in the widget's default
/// size for an omitted `width`/`height`.
fn design_rect(node: &Node, spec: &WidgetSpec) -> (i64, i64, i64, i64) {
    let left = node.prop("left").and_then(Value::as_int).unwrap_or(0);
    let top = node.prop("top").and_then(Value::as_int).unwrap_or(0);
    let width = node
        .prop("width")
        .and_then(Value::as_int)
        .unwrap_or_else(|| spec.default_size.0.value().round() as i64);
    let height = node
        .prop("height")
        .and_then(Value::as_int)
        .unwrap_or_else(|| spec.default_size.1.value().round() as i64);
    (left, top, width, height)
}

/// The anchor a node declares, defaulting to `top_left`.
fn anchor_of(node: &Node) -> Anchor {
    node.prop("anchor")
        .and_then(Value::as_str)
        .and_then(crate::schema::anchor_from_name)
        .unwrap_or(Anchor::TopLeft)
}

/// Converts a DIP design rectangle to device pixels.
fn px_rect(design: (i64, i64, i64, i64), dpi: u32) -> Rect {
    let (left, top, width, height) = design;
    let to_px = |value: i64| Dip(value as f32).to_px(dpi).value();
    Rect::new(
        to_px(left),
        to_px(top),
        to_px(left) + to_px(width),
        to_px(top) + to_px(height),
    )
}

/// Converts a DIP size to device pixels.
fn px_size(size: (i64, i64), dpi: u32) -> Size {
    Size::new(
        Dip(size.0 as f32).to_px(dpi).value(),
        Dip(size.1 as f32).to_px(dpi).value(),
    )
}

/// The common property state every widget shares.
struct Common<M: 'static> {
    ui: Ui<M>,
    design: Cell<(i64, i64, i64, i64)>,
    anchor: Cell<Anchor>,
    visible: Cell<bool>,
    enabled: Cell<bool>,
    tab_index: Cell<i64>,
}

impl<M: 'static> Common<M> {
    /// Creates the common state for a freshly built widget.
    fn new(ui: Ui<M>, design: (i64, i64, i64, i64)) -> Common<M> {
        Common {
            ui,
            design: Cell::new(design),
            anchor: Cell::new(Anchor::TopLeft),
            visible: Cell::new(true),
            enabled: Cell::new(true),
            tab_index: Cell::new(0),
        }
    }

    /// Reads a common property.
    fn get(&self, prop: &str) -> Option<Value> {
        let (left, top, width, height) = self.design.get();
        match prop {
            "left" => Some(Value::Int(left)),
            "top" => Some(Value::Int(top)),
            "width" => Some(Value::Int(width)),
            "height" => Some(Value::Int(height)),
            "anchor" => Some(Value::Enum(
                crate::schema::anchor_name(self.anchor.get()).to_owned(),
            )),
            "visible" => Some(Value::Bool(self.visible.get())),
            "enabled" => Some(Value::Bool(self.enabled.get())),
            "tab_index" => Some(Value::Int(self.tab_index.get())),
            _ => None,
        }
    }

    /// Writes a common property. `None` means "not a common property".
    fn set(&self, prop: &str, value: &Value) -> Option<Result<(), SetError>> {
        match (prop, value) {
            ("left", Value::Int(value)) => self.set_geometry(|rect| rect.0 = *value),
            ("top", Value::Int(value)) => self.set_geometry(|rect| rect.1 = *value),
            ("width", Value::Int(value)) => self.set_geometry(|rect| rect.2 = *value),
            ("height", Value::Int(value)) => self.set_geometry(|rect| rect.3 = *value),
            ("left" | "top" | "width" | "height", _) => Some(Err(SetError::TypeMismatch)),
            ("anchor", Value::Enum(name)) => match crate::schema::anchor_from_name(name) {
                Some(anchor) => {
                    self.anchor.set(anchor);
                    Some(Ok(()))
                }
                None => Some(Err(SetError::TypeMismatch)),
            },
            ("anchor", _) => Some(Err(SetError::TypeMismatch)),
            ("visible", Value::Bool(visible)) => {
                self.visible.set(*visible);
                Some(Ok(()))
            }
            ("visible", _) => Some(Err(SetError::TypeMismatch)),
            ("enabled", Value::Bool(enabled)) => {
                self.enabled.set(*enabled);
                Some(Ok(()))
            }
            ("enabled", _) => Some(Err(SetError::TypeMismatch)),
            ("tab_index", Value::Int(index)) => {
                self.tab_index.set(*index);
                Some(Ok(()))
            }
            ("tab_index", _) => Some(Err(SetError::TypeMismatch)),
            _ => None,
        }
    }

    /// Mutates one geometry component. [`Live`] then moves every node.
    fn set_geometry(
        &self,
        mutate: impl FnOnce(&mut (i64, i64, i64, i64)),
    ) -> Option<Result<(), SetError>> {
        let mut design = self.design.get();
        mutate(&mut design);
        self.design.set(design);
        Some(Ok(()))
    }
}

/// A [`LiveWidget`] that adds the common properties and the schema's access and
/// type rules to a widget's own surface.
struct Live<M: 'static, W: WidgetProps<M>> {
    common: Common<M>,
    inner: W,
    catalog: Rc<Catalog>,
    kind: String,
    design_mode: bool,
}

impl<M: 'static, W: WidgetProps<M>> LiveWidget<M> for Live<M, W> {
    fn id(&self) -> WidgetId {
        self.inner.id()
    }

    fn get(&self, prop: &str) -> Option<Value> {
        self.common.get(prop).or_else(|| self.inner.get_own(prop))
    }

    fn set(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        let Some(spec) = self.catalog.property(&self.kind, prop) else {
            return Err(SetError::UnknownProperty);
        };
        let writable = if self.design_mode {
            matches!(spec.access, Access::ReadWrite | Access::DesignOnly)
        } else {
            matches!(spec.access, Access::ReadWrite | Access::RuntimeOnly)
        };
        if !writable {
            return Err(SetError::ReadOnly);
        }
        if !spec.accepts(value) {
            return Err(SetError::TypeMismatch);
        }
        if let Some(result) = self.common.set(prop, value) {
            if result.is_ok() {
                self.apply_common(prop, value);
            }
            return result;
        }
        self.inner.set_own(prop, value)
    }

    fn container_ui(&self) -> Option<&Ui<M>> {
        self.inner.container_ui()
    }

    fn placements(&self, rect: Rect) -> Vec<(WidgetId, Rect)> {
        self.inner.placements(&self.common.ui, rect)
    }

    fn node_ids(&self) -> Vec<WidgetId> {
        self.inner.node_ids()
    }
}

impl<M: 'static, W: WidgetProps<M>> Live<M, W> {
    /// Applies a common property that was just recorded to every node the
    /// widget owns, so a multi-node widget moves, hides and disables as one.
    fn apply_common(&self, prop: &str, value: &Value) {
        let ui = &self.common.ui;
        match (prop, value) {
            ("left" | "top" | "width" | "height", _) => {
                let rect = px_rect(self.common.design.get(), ui.dpi());
                ui.apply_moves(&self.inner.placements(ui, rect));
            }
            ("visible", Value::Bool(visible)) => {
                for id in self.inner.node_ids() {
                    ui.set_visible(id, *visible);
                }
            }
            ("enabled", Value::Bool(enabled)) => {
                for id in self.inner.node_ids() {
                    ui.set_enabled(id, *enabled);
                }
                self.inner.set_enabled_hint(*enabled);
            }
            _ => {}
        }
    }
}
