#![forbid(unsafe_code)]

//! Building live `xui` widgets from a [`FormDoc`].
//!
//! The host supplies a [`Binder`] that decides what each event becomes, and a
//! [`Factories`] registry that knows how to describe each kind. [`build`]
//! asks every node's factory for a widget builder, places the builders in an
//! [`absolute`](xui_core::arrange::absolute) layout per level of the node tree
//! (each node at its design rectangle, moved by its anchor), and mounts them:
//! on the window, or in a container node the host chose. The resulting
//! [`LiveForm`] owns the widgets, reads and writes their properties, and keeps
//! them anchored as the window or container resizes.
//!
//! A factory only ever sees the portable [`crate::schema`] and the
//! [`BuildCx`] helpers, so registering a custom widget needs no change here.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use xui_core::WidgetId;
use xui_core::app::Ui;
use xui_core::arrange::{Entry, Handle, IntoEntry};
use xui_core::backend::BackendError;
use xui_core::geometry::Rect;
use xui_core::layout::Anchor;
use xui_core::units::Dip;
use xui_core::widget::Placeable;

use crate::doc::{FormDoc, Node};
use crate::live::{Common, Live, WidgetProps};
use crate::placement::{Geometry, Placement, Shared, Slot};
use crate::schema::{Catalog, EventSpec, MethodSpec, WidgetSpec};
use crate::value::{Value, ValueType};

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

/// How a method call failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CallError {
    /// No widget of that name exists in the form.
    #[error("unknown widget")]
    UnknownWidget,
    /// The widget's kind declares no method of that name.
    #[error("unknown method")]
    UnknownMethod,
    /// The arguments do not match the method's [`MethodSpec`]: the wrong
    /// count, or a value of the wrong type or out of range. The text says
    /// which.
    #[error("{0}")]
    WrongArgs(String),
    /// The method was called correctly but could not do its work (it is not
    /// available in design mode, say). The text says why.
    #[error("{0}")]
    Failed(String),
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
    /// The container node the form is placed in, filling it; `None` makes
    /// the form the window's content, filling its client area.
    pub container: Option<WidgetId>,
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
///
/// Its methods are called once the form is mounted.
pub trait LiveWidget<M: 'static> {
    /// The widget's node identity.
    fn id(&self) -> WidgetId;

    /// The widget-specific property named `prop`, if it has one.
    fn get(&self, prop: &str) -> Option<Value>;

    /// Sets the widget-specific property named `prop`.
    fn set(&self, prop: &str, value: &Value) -> Result<(), SetError>;

    /// Calls the method named `method` with `args`, returning its result, or
    /// `None` for a method that returns nothing. The default knows no
    /// methods.
    fn call(&self, method: &str, args: &[Value]) -> Result<Option<Value>, CallError> {
        let _ = (method, args);
        Err(CallError::UnknownMethod)
    }

    /// Every node the widget owns; the first is [`LiveWidget::id`].
    fn node_ids(&self) -> Vec<WidgetId> {
        vec![self.id()]
    }
}

/// What a factory makes for one node: the layout entry that creates the widget
/// when the form is mounted, and the live surface that reads and writes it
/// afterwards.
pub struct Made<M: 'static> {
    entry: Entry<M>,
    widget: Box<dyn LiveWidget<M>>,
    shared: Shared<M>,
}

impl<M: 'static> Made<M> {
    /// Pairs `entry`, a widget builder bound to `handle`, with `widget`, the
    /// live surface that reaches the widget through `handle` once it is
    /// created. The form places the entry at the node's design rectangle; the
    /// handle lets it place the same widget again when the node moves.
    pub fn new<W: Placeable<M> + 'static>(
        entry: impl IntoEntry<M>,
        handle: &Handle<W>,
        widget: Box<dyn LiveWidget<M>>,
    ) -> Made<M> {
        let handle = handle.clone();
        Made {
            entry: entry.into_entry(),
            widget,
            shared: Box::new(move || {
                handle
                    .try_get()
                    .map(|widget| widget as Rc<dyn Placeable<M>>)
            }),
        }
    }
}

/// Describes the live widget for one kind.
pub trait WidgetFactory<M: 'static> {
    /// The canonical kind this factory builds.
    fn kind(&self) -> &str;

    /// Describes a widget for `node`, wiring its events through `cx`. The
    /// widget is created when the form is mounted.
    fn create(&self, cx: &mut BuildCx<'_, M>, node: &Node) -> Made<M>;
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

/// What a factory receives when it describes a widget.
pub struct BuildCx<'a, M: 'static> {
    ui: &'a Ui<M>,
    node: &'a Node,
    spec: &'a WidgetSpec,
    geometry: Rc<Geometry>,
    index: usize,
    binder: &'a dyn Binder<M>,
    design_mode: bool,
    catalog: Rc<Catalog>,
    placement: &'a Rc<Placement<M>>,
}

impl<'a, M: 'static> BuildCx<'a, M> {
    /// The node being built.
    pub fn node(&self) -> &Node {
        self.node
    }

    /// The node's widget spec.
    pub fn spec(&self) -> &WidgetSpec {
        self.spec
    }

    /// The node's design rectangle in device pixels, relative to its parent
    /// (the form or its container). The form places the widget there.
    pub fn rect(&self) -> Rect {
        px_rect(self.geometry.design.get(), self.ui.dpi())
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

    /// Pairs a built-in widget's builder, bound to `handle`, with its own
    /// property surface wrapped in the common property handling.
    pub(crate) fn made<W: Placeable<M> + 'static, P: WidgetProps<M>>(
        &self,
        entry: impl IntoEntry<M>,
        handle: &Handle<W>,
        inner: P,
    ) -> Made<M> {
        let common = Common {
            ui: self.ui.clone(),
            geometry: Rc::clone(&self.geometry),
            placement: Rc::downgrade(self.placement),
            index: self.index,
            visible: Cell::new(true),
            enabled: Cell::new(true),
            tab_index: Cell::new(0),
        };
        let live = Live {
            common,
            inner,
            catalog: Rc::clone(&self.catalog),
            kind: self.spec.kind.clone(),
            design_mode: self.design_mode,
        };
        Made::new(entry, handle, Box::new(live))
    }
}

/// One node's metadata, kept for name and kind lookups.
#[derive(Clone, Debug)]
struct NodeMeta {
    name: String,
    kind: String,
}

/// A built form: every live widget, mounted and kept anchored.
pub struct LiveForm<M: 'static> {
    ui: Ui<M>,
    catalog: Rc<Catalog>,
    widgets: Vec<Box<dyn LiveWidget<M>>>,
    by_name: BTreeMap<String, usize>,
    nodes: Vec<NodeMeta>,
    placement: Rc<Placement<M>>,
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

    /// Sets `prop` on the widget named `name`. A geometry or `anchor` edit
    /// moves the widget at once; inside [`LiveForm::batch`] the move waits for
    /// the batch to end.
    pub fn set(&self, name: &str, prop: &str, value: &Value) -> Result<(), SetError> {
        self.widget(name)
            .ok_or(SetError::UnknownWidget)?
            .set(prop, value)
    }

    /// Calls `method` on the widget named `name` with `args`, returning its
    /// result (`None` for a method that returns nothing).
    ///
    /// The arguments are checked against the method's [`MethodSpec`] first:
    /// a wrong count or type is [`CallError::WrongArgs`] and the widget is not
    /// touched. A design-mode form refuses every call with
    /// [`CallError::Failed`], since methods act on the running widget.
    pub fn call(
        &self,
        name: &str,
        method: &str,
        args: &[Value],
    ) -> Result<Option<Value>, CallError> {
        self.widget(name)
            .ok_or(CallError::UnknownWidget)?
            .call(method, args)
    }

    /// The method `method` on the node named `name`, if the node's kind
    /// declares it.
    pub fn method_spec(&self, name: &str, method: &str) -> Option<&MethodSpec> {
        self.catalog.method(self.kind(name)?, method)
    }

    /// Runs `edits` against the form and applies the moves its geometry edits
    /// cause once, when it returns, rather than after each edit: a designer
    /// pushing every node's rectangle after a drag.
    pub fn batch<R>(&self, edits: impl FnOnce(&Self) -> R) -> R {
        self.placement.batch(|| edits(self))
    }

    /// Changes the size the form's top-level nodes were designed at, in DIPs
    /// (the window's `width` and `height`): their anchors follow the
    /// difference between it and the size the form is laid out at. A
    /// designer calls it when the user resizes the form, so the nodes stay at
    /// their design positions.
    pub fn set_design_size(&self, width: i64, height: i64) {
        self.placement.set_design_size((width, height));
    }

    /// Every widget's node identity, in creation order.
    pub fn ids(&self) -> Vec<WidgetId> {
        self.widgets.iter().map(|widget| widget.id()).collect()
    }

    /// The pixel rectangle of the widget named `name`, if any.
    pub fn bounds(&self, name: &str) -> Option<Rect> {
        Some(self.ui.bounds(self.widget(name)?.id()))
    }

    /// The node names, in creation order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.nodes.iter().map(|node| node.name.as_str())
    }

    /// The catalog the form was built against.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The canonical widget kind of the node named `name`, if any.
    ///
    /// An alias resolves to the canonical kind, so a `CommandButton` node
    /// reports `Button`.
    pub fn kind(&self, name: &str) -> Option<&str> {
        self.nodes
            .iter()
            .find(|node| node.name == name)
            .map(|node| node.kind.as_str())
    }

    /// The schema type of `property` on the node named `name`, if the catalog
    /// declares that property for the node's kind.
    ///
    /// This is the schema lookup a runtime needs to decode a script value into
    /// the right [`Value`] variant (for example an `enum` property).
    pub fn property_type(&self, name: &str, property: &str) -> Option<ValueType> {
        let node = self.nodes.iter().find(|node| node.name == name)?;
        self.catalog
            .property(&node.kind, property)
            .map(|spec| spec.ty)
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

    /// Lays every widget out again now, at its anchored design rectangle.
    ///
    /// The form already re-anchors itself whenever the window (or the
    /// container it was built in) resizes, once the event being handled is
    /// done; this is for reading the new bounds within the same handler.
    pub fn relayout(&self) {
        self.placement.relayout();
    }
}

/// Builds every widget in `doc`, wiring events through `binder`, as the
/// window's content.
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
    let placement = Rc::new(Placement::new(ui.clone(), options.container, design_size));
    let mut widgets: Vec<Box<dyn LiveWidget<M>>> = Vec::new();
    let mut entries = Vec::new();
    let mut slots: Vec<Slot<M>> = Vec::new();
    let mut by_name = BTreeMap::new();
    let mut nodes = Vec::new();

    // A container must exist before its children, whatever order the document
    // lists them in (`reparent` does not move nodes, and files may be hand
    // written), so describe parents first.
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

        let parent = match node.parent.as_deref() {
            None => None,
            Some(parent) => Some(
                by_name
                    .get(parent)
                    .copied()
                    .filter(|index: &usize| slots[*index].is_container)
                    .ok_or_else(|| BuildError::UnknownParent {
                        node: node.name.clone(),
                        parent: parent.to_owned(),
                    })?,
            ),
        };

        let factory = factories
            .get(&spec.kind)
            .ok_or_else(|| BuildError::UnknownFactory {
                kind: spec.kind.clone(),
                node: node.name.clone(),
            })?;
        let geometry = Rc::new(Geometry {
            design: Cell::new(design_rect(node, spec)),
            anchor: Cell::new(anchor_of(node)),
        });
        let index = widgets.len();
        let mut cx = BuildCx {
            ui,
            node,
            spec,
            geometry: Rc::clone(&geometry),
            index,
            binder,
            design_mode: options.design_mode,
            catalog: Rc::clone(&catalog),
            placement: &placement,
        };
        let made = factory.create(&mut cx, node);

        widgets.push(made.widget);
        entries.push(made.entry);
        slots.push(Slot {
            parent,
            geometry,
            shared: made.shared,
            is_container: catalog.is_container(&spec.kind),
        });
        by_name.insert(node.name.clone(), index);
        nodes.push(NodeMeta {
            name: node.name.clone(),
            kind: spec.kind.clone(),
        });
    }

    placement.mount(slots, entries)?;

    // Geometry and anchor are the layout's; the other common properties are
    // pushed once the widgets exist.
    for (widget, meta) in widgets.iter().zip(&nodes) {
        let Some(node) = doc.node(&meta.name) else {
            continue;
        };
        for name in ["visible", "enabled", "tab_index"] {
            if let Some(value) = node.props.get(name) {
                let _ = widget.set(name, value);
            }
        }
    }

    Ok(LiveForm {
        ui: ui.clone(),
        catalog,
        widgets,
        by_name,
        nodes,
        placement,
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
