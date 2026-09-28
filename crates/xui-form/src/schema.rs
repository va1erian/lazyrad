#![forbid(unsafe_code)]

//! The form schema: widget specs, their properties and events, and the
//! [`Catalog`] that maps kinds to specs.
//!
//! The catalog is the single source of truth shared by the document decoder,
//! validation, the designer's property grid and the runtime builder. The
//! built-in catalog, [`Catalog::xui`], describes the portable `xui-core`
//! widgets; a consumer may register its own widgets or alias a kind, so it can
//! expose `CommandButton` as `Button` without copying the spec.
//!
//! [`Catalog`] is [`Serialize`], so a tool can export it as JSON for the
//! designer, completion and documentation.

use std::collections::BTreeMap;

use serde::{Serialize, Serializer};
use xui_core::layout::Anchor;
use xui_core::units::Dip;

use crate::value::{Value, ValueType};

/// The twelve anchor names, in the order the designer shows them.
pub const ANCHOR_NAMES: [&str; 12] = [
    "top_left",
    "top",
    "top_right",
    "left",
    "center",
    "right",
    "bottom_left",
    "bottom",
    "bottom_right",
    "stretch_horizontal",
    "stretch_vertical",
    "fill",
];

/// The property-grid section a property belongs to.
pub const CATEGORY_APPEARANCE: &str = "Appearance";
/// The property-grid section for geometry.
pub const CATEGORY_LAYOUT: &str = "Layout";
/// The property-grid section for behaviour.
pub const CATEGORY_BEHAVIOR: &str = "Behavior";
/// The property-grid section for data.
pub const CATEGORY_DATA: &str = "Data";

/// The [`Anchor`] a name denotes, if any.
pub fn anchor_from_name(name: &str) -> Option<Anchor> {
    Some(match name {
        "top_left" => Anchor::TopLeft,
        "top" => Anchor::Top,
        "top_right" => Anchor::TopRight,
        "left" => Anchor::Left,
        "center" => Anchor::Center,
        "right" => Anchor::Right,
        "bottom_left" => Anchor::BottomLeft,
        "bottom" => Anchor::Bottom,
        "bottom_right" => Anchor::BottomRight,
        "stretch_horizontal" => Anchor::StretchHorizontal,
        "stretch_vertical" => Anchor::StretchVertical,
        "fill" => Anchor::Fill,
        _ => return None,
    })
}

/// The name an [`Anchor`] is written as.
pub fn anchor_name(anchor: Anchor) -> &'static str {
    match anchor {
        Anchor::TopLeft => "top_left",
        Anchor::Top => "top",
        Anchor::TopRight => "top_right",
        Anchor::Left => "left",
        Anchor::Center => "center",
        Anchor::Right => "right",
        Anchor::BottomLeft => "bottom_left",
        Anchor::Bottom => "bottom",
        Anchor::BottomRight => "bottom_right",
        Anchor::StretchHorizontal => "stretch_horizontal",
        Anchor::StretchVertical => "stretch_vertical",
        Anchor::Fill => "fill",
    }
}

/// When a property may be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Readable and writable in the designer and at runtime.
    ReadWrite,
    /// Writable in the designer, read-only at runtime.
    DesignOnly,
    /// Read-only in the designer, writable at runtime.
    RuntimeOnly,
    /// Never writable.
    ReadOnly,
}

impl Access {
    /// Whether a value may be written in design mode.
    pub fn writable_in_design(self) -> bool {
        matches!(self, Access::ReadWrite | Access::DesignOnly)
    }

    /// Whether a value may be written at runtime.
    pub fn writable_at_runtime(self) -> bool {
        matches!(self, Access::ReadWrite | Access::RuntimeOnly)
    }
}

/// One property a widget kind accepts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PropertySpec {
    /// The property name, as written on disk.
    pub name: String,
    /// The value's type; the schema decodes the raw literal against it.
    pub ty: ValueType,
    /// The value used when the property is absent.
    pub default: Value,
    /// The property-grid grouping.
    pub category: String,
    /// Help text for the property grid and completion.
    pub description: String,
    /// When the property may be written.
    pub access: Access,
}

impl PropertySpec {
    /// Whether `value` is acceptable for this property.
    pub fn accepts(&self, value: &Value) -> bool {
        self.ty.accepts(value)
    }
}

/// One argument of an event handler.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ArgSpec {
    /// The argument's name.
    pub name: String,
    /// The argument's type.
    pub ty: ValueType,
}

/// One event a widget kind raises.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EventSpec {
    /// The event name, given to [`crate::Binder::bind`].
    pub name: String,
    /// The handler's arguments, in order.
    pub args: Vec<ArgSpec>,
    /// Whether this is the event a double-click opens by default.
    pub is_default: bool,
    /// Help text for completion.
    pub description: String,
}

/// What a widget kind may contain.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Children {
    /// No children.
    None,
    /// Any registered kind.
    Any,
    /// Only these kinds (by canonical name).
    Only(Vec<String>),
}

impl Children {
    /// Whether `kind` (already resolved to a canonical name) is allowed.
    pub fn accepts(&self, kind: &str) -> bool {
        match self {
            Children::None => false,
            Children::Any => true,
            Children::Only(allowed) => allowed.iter().any(|candidate| candidate == kind),
        }
    }

    /// Whether this is the [`Children::None`] rule.
    pub fn is_none(&self) -> bool {
        matches!(self, Children::None)
    }
}

/// The properties, events and child rules of one widget kind.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WidgetSpec {
    /// The canonical kind name.
    pub kind: String,
    /// A one-line description, for the toolbox and docs.
    pub description: String,
    /// The widget-specific properties; the common ones are not repeated here.
    pub properties: Vec<PropertySpec>,
    /// The events the widget raises.
    pub events: Vec<EventSpec>,
    /// What the widget may contain.
    pub children: Children,
    /// The size a toolbox drop gives the widget, in design units.
    #[serde(serialize_with = "serialize_size")]
    pub default_size: (Dip, Dip),
}

impl WidgetSpec {
    /// The widget-specific property named `name`, if declared.
    pub fn property(&self, name: &str) -> Option<&PropertySpec> {
        self.properties.iter().find(|spec| spec.name == name)
    }

    /// The event named `name`, if declared.
    pub fn event(&self, name: &str) -> Option<&EventSpec> {
        self.events.iter().find(|spec| spec.name == name)
    }

    /// The default event, if one is declared.
    pub fn default_event(&self) -> Option<&EventSpec> {
        self.events.iter().find(|spec| spec.is_default)
    }
}

/// Serialises a design size as a two-element array.
fn serialize_size<S>((width, height): &(Dip, Dip), serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    use serde::ser::SerializeTuple;
    let mut tuple = serializer.serialize_tuple(2)?;
    tuple.serialize_element(&width.value())?;
    tuple.serialize_element(&height.value())?;
    tuple.end()
}

/// The catalog of widget kinds: the schema every tool reads.
#[derive(Clone, Debug, Serialize)]
pub struct Catalog {
    /// Canonical kind to spec.
    kinds: BTreeMap<String, WidgetSpec>,
    /// Alias to canonical kind.
    aliases: BTreeMap<String, String>,
    /// The properties every node has, defined once.
    common: Vec<PropertySpec>,
    /// The window's own spec (it is not a widget kind).
    window: WidgetSpec,
}

impl Default for Catalog {
    fn default() -> Catalog {
        Catalog::new()
    }
}

impl Catalog {
    /// A catalog with the common properties and the window spec, and no widget
    /// kinds.
    pub fn new() -> Catalog {
        Catalog {
            kinds: BTreeMap::new(),
            aliases: BTreeMap::new(),
            common: common_properties(),
            window: window_spec(),
        }
    }

    /// The built-in catalog of portable `xui-core` widgets.
    pub fn xui() -> Catalog {
        let mut catalog = Catalog::new();
        for spec in builtin_specs() {
            catalog.register(spec);
        }
        catalog
    }

    /// Adds a widget spec, replacing any spec with the same kind.
    pub fn register(&mut self, spec: WidgetSpec) {
        self.kinds.insert(spec.kind.clone(), spec);
    }

    /// Adds `alias` as another name for the canonical `kind`.
    ///
    /// The alias need not resolve yet; [`Catalog::get`] fails until it does.
    pub fn alias(&mut self, alias: impl Into<String>, kind: impl Into<String>) {
        self.aliases.insert(alias.into(), kind.into());
    }

    /// The canonical kind a name resolves to, following one alias.
    pub fn resolve<'a>(&'a self, name: &'a str) -> Option<&'a str> {
        if self.kinds.contains_key(name) {
            return Some(name);
        }
        self.aliases
            .get(name)
            .and_then(|kind| self.kinds.contains_key(kind).then_some(kind.as_str()))
    }

    /// The spec for `kind` (or an alias), if known.
    pub fn get(&self, kind: &str) -> Option<&WidgetSpec> {
        self.kinds.get(self.resolve(kind)?)
    }

    /// Whether `kind` (or an alias) is known.
    pub fn contains(&self, kind: &str) -> bool {
        self.resolve(kind).is_some()
    }

    /// The canonical kind names, in sorted order.
    pub fn kinds(&self) -> impl Iterator<Item = &str> {
        self.kinds.keys().map(String::as_str)
    }

    /// The aliases, as `(alias, canonical kind)` pairs in sorted order.
    pub fn aliases(&self) -> impl Iterator<Item = (&str, &str)> {
        self.aliases
            .iter()
            .map(|(alias, kind)| (alias.as_str(), kind.as_str()))
    }

    /// The window's spec.
    pub fn window_spec(&self) -> &WidgetSpec {
        &self.window
    }

    /// Replaces the window spec.
    pub fn set_window_spec(&mut self, spec: WidgetSpec) {
        self.window = spec;
    }

    /// The properties every node has, in save order.
    pub fn common_properties(&self) -> &[PropertySpec] {
        &self.common
    }

    /// The property spec for `name` on `kind`, checking the widget first and
    /// the common properties second.
    ///
    /// `width` and `height` take their default from the widget's
    /// [`WidgetSpec::default_size`], so a node that omits them is sized for its
    /// kind.
    pub fn property(&self, kind: &str, name: &str) -> Option<PropertySpec> {
        if let Some(spec) = self.get(kind) {
            if let Some(property) = spec.property(name) {
                return Some(property.clone());
            }
            if name == "width" || name == "height" {
                let common = self.common.iter().find(|property| property.name == name)?;
                let rounded = if name == "width" {
                    spec.default_size.0.value().round() as i64
                } else {
                    spec.default_size.1.value().round() as i64
                };
                let mut property = common.clone();
                property.default = Value::Int(rounded);
                return Some(property);
            }
        }
        self.common
            .iter()
            .find(|property| property.name == name)
            .cloned()
    }

    /// Whether `kind` (or an alias) is a container.
    pub fn is_container(&self, kind: &str) -> bool {
        self.get(kind).is_some_and(|spec| !spec.children.is_none())
    }

    /// Whether a node of `child_kind` may be placed inside `parent_kind`.
    pub fn accepts_child(&self, parent_kind: &str, child_kind: &str) -> bool {
        let (Some(parent), Some(child)) = (self.get(parent_kind), self.get(child_kind)) else {
            return false;
        };
        parent.children.accepts(&child.kind)
    }
}

/// Builds a property spec.
fn property(
    name: &str,
    ty: ValueType,
    default: Value,
    category: &str,
    description: &str,
) -> PropertySpec {
    PropertySpec {
        name: name.to_owned(),
        ty,
        default,
        category: category.to_owned(),
        description: description.to_owned(),
        access: Access::ReadWrite,
    }
}

/// Builds a design-only property spec.
fn design_property(
    name: &str,
    ty: ValueType,
    default: Value,
    category: &str,
    description: &str,
) -> PropertySpec {
    PropertySpec {
        access: Access::DesignOnly,
        ..property(name, ty, default, category, description)
    }
}

/// The common properties, in save order.
fn common_properties() -> Vec<PropertySpec> {
    vec![
        property(
            "left",
            ValueType::Int {
                min: None,
                max: None,
            },
            Value::Int(0),
            CATEGORY_LAYOUT,
            "The x offset from the parent's left edge, in design units.",
        ),
        property(
            "top",
            ValueType::Int {
                min: None,
                max: None,
            },
            Value::Int(0),
            CATEGORY_LAYOUT,
            "The y offset from the parent's top edge, in design units.",
        ),
        property(
            "width",
            ValueType::Int {
                min: None,
                max: None,
            },
            Value::Int(0),
            CATEGORY_LAYOUT,
            "The width, in design units.",
        ),
        property(
            "height",
            ValueType::Int {
                min: None,
                max: None,
            },
            Value::Int(0),
            CATEGORY_LAYOUT,
            "The height, in design units.",
        ),
        property(
            "anchor",
            ValueType::Enum {
                variants: ANCHOR_NAMES.iter().map(|name| (*name).to_owned()).collect(),
            },
            Value::Enum("top_left".to_owned()),
            CATEGORY_LAYOUT,
            "How the node follows its parent when it is resized.",
        ),
        property(
            "visible",
            ValueType::Bool,
            Value::Bool(true),
            CATEGORY_BEHAVIOR,
            "Whether the node is shown.",
        ),
        property(
            "enabled",
            ValueType::Bool,
            Value::Bool(true),
            CATEGORY_BEHAVIOR,
            "Whether the node accepts input.",
        ),
        property(
            "tab_index",
            ValueType::Int {
                min: Some(0),
                max: None,
            },
            Value::Int(0),
            CATEGORY_BEHAVIOR,
            "The node's place in the tab order among its siblings.",
        ),
    ]
}

/// The window's spec; the window is not a widget kind.
fn window_spec() -> WidgetSpec {
    WidgetSpec {
        kind: "Window".to_owned(),
        description: "The top-level window that hosts the form.".to_owned(),
        properties: vec![
            property(
                "title",
                ValueType::Text { multiline: false },
                Value::Text(String::new()),
                CATEGORY_APPEARANCE,
                "The title shown in the window's title bar.",
            ),
            property(
                "width",
                ValueType::Int {
                    min: Some(1),
                    max: None,
                },
                Value::Int(320),
                CATEGORY_LAYOUT,
                "The client width, in design units.",
            ),
            property(
                "height",
                ValueType::Int {
                    min: Some(1),
                    max: None,
                },
                Value::Int(200),
                CATEGORY_LAYOUT,
                "The client height, in design units.",
            ),
            property(
                "resizable",
                ValueType::Bool,
                Value::Bool(true),
                CATEGORY_BEHAVIOR,
                "Whether the window can be resized.",
            ),
        ],
        events: vec![
            event("Load", Vec::new(), true, "Raised once the window is built."),
            event(
                "Close",
                vec![arg("cancel", ValueType::Bool)],
                false,
                "Raised when the window is asked to close; `cancel` stops it.",
            ),
            event(
                "Resize",
                vec![arg("width", int_type()), arg("height", int_type())],
                false,
                "Raised when the client area changes size.",
            ),
            event(
                "Activate",
                Vec::new(),
                false,
                "Raised when the window is focused.",
            ),
        ],
        children: Children::None,
        default_size: (Dip(320.0), Dip(200.0)),
    }
}

/// An unbounded int type.
fn int_type() -> ValueType {
    ValueType::Int {
        min: None,
        max: None,
    }
}

/// An unbounded float type.
fn float_type() -> ValueType {
    ValueType::Float {
        min: None,
        max: None,
    }
}

/// A single-line text type.
fn text_type() -> ValueType {
    ValueType::Text { multiline: false }
}

/// An enum type from variant names.
fn enum_type(variants: &[&str]) -> ValueType {
    ValueType::Enum {
        variants: variants.iter().map(|name| (*name).to_owned()).collect(),
    }
}

/// Builds an event spec.
fn event(name: &str, args: Vec<ArgSpec>, is_default: bool, description: &str) -> EventSpec {
    EventSpec {
        name: name.to_owned(),
        args,
        is_default,
        description: description.to_owned(),
    }
}

/// Builds an event argument spec.
fn arg(name: &str, ty: ValueType) -> ArgSpec {
    ArgSpec {
        name: name.to_owned(),
        ty,
    }
}

/// Builds a widget spec with no properties or events.
fn widget(kind: &str, description: &str, size: (f32, f32), children: Children) -> WidgetSpec {
    WidgetSpec {
        kind: kind.to_owned(),
        description: description.to_owned(),
        properties: Vec::new(),
        events: Vec::new(),
        children,
        default_size: (Dip(size.0), Dip(size.1)),
    }
}

/// The built-in widget specs, grounded in the `xui-core` widget APIs.
fn builtin_specs() -> Vec<WidgetSpec> {
    vec![
        WidgetSpec {
            properties: vec![property(
                "text",
                text_type(),
                Value::Text(String::new()),
                CATEGORY_APPEARANCE,
                "The label's text.",
            )],
            ..widget(
                "Label",
                "A static text label.",
                (120.0, 20.0),
                Children::None,
            )
        },
        WidgetSpec {
            properties: vec![property(
                "text",
                text_type(),
                Value::Text(String::new()),
                CATEGORY_APPEARANCE,
                "The button's caption.",
            )],
            events: vec![event(
                "Click",
                Vec::new(),
                true,
                "Raised when the button is pressed.",
            )],
            ..widget("Button", "A push button.", (100.0, 28.0), Children::None)
        },
        WidgetSpec {
            properties: vec![
                property(
                    "text",
                    text_type(),
                    Value::Text(String::new()),
                    CATEGORY_APPEARANCE,
                    "The box's label.",
                ),
                property(
                    "checked",
                    ValueType::Bool,
                    Value::Bool(false),
                    CATEGORY_DATA,
                    "Whether the box is checked.",
                ),
            ],
            events: vec![event(
                "Toggle",
                vec![arg("checked", ValueType::Bool)],
                true,
                "Raised with the new checked state.",
            )],
            ..widget(
                "CheckBox",
                "A labelled check box.",
                (120.0, 24.0),
                Children::None,
            )
        },
        WidgetSpec {
            properties: vec![
                property(
                    "text",
                    text_type(),
                    Value::Text(String::new()),
                    CATEGORY_APPEARANCE,
                    "The button's label.",
                ),
                property(
                    "checked",
                    ValueType::Bool,
                    Value::Bool(false),
                    CATEGORY_DATA,
                    "Whether the button is latched down.",
                ),
            ],
            events: vec![event(
                "Toggle",
                vec![arg("checked", ValueType::Bool)],
                true,
                "Raised with the new checked state.",
            )],
            ..widget(
                "ToggleButton",
                "A button that latches a checked state.",
                (100.0, 28.0),
                Children::None,
            )
        },
        WidgetSpec {
            properties: vec![
                design_property(
                    "items",
                    ValueType::List,
                    Value::List(Vec::new()),
                    CATEGORY_DATA,
                    "The option labels, top to bottom.",
                ),
                property(
                    "selected",
                    ValueType::Int {
                        min: Some(0),
                        max: None,
                    },
                    Value::Int(0),
                    CATEGORY_DATA,
                    "The selected option index.",
                ),
            ],
            events: vec![event(
                "Select",
                vec![arg("index", int_type())],
                true,
                "Raised with the newly selected index.",
            )],
            ..widget(
                "RadioGroup",
                "A vertical set of radio options.",
                (160.0, 96.0),
                Children::None,
            )
        },
        WidgetSpec {
            properties: vec![
                property(
                    "text",
                    text_type(),
                    Value::Text(String::new()),
                    CATEGORY_DATA,
                    "The field's text.",
                ),
                property(
                    "cue",
                    text_type(),
                    Value::Text(String::new()),
                    CATEGORY_APPEARANCE,
                    "A placeholder shown while the field is empty.",
                ),
            ],
            events: vec![event(
                "Change",
                vec![arg("text", text_type())],
                true,
                "Raised with the new text as the user types.",
            )],
            ..widget(
                "Edit",
                "A single-line text field.",
                (160.0, 24.0),
                Children::None,
            )
        },
        WidgetSpec {
            properties: vec![property(
                "text",
                ValueType::Text { multiline: true },
                Value::Text(String::new()),
                CATEGORY_DATA,
                "The text area's text.",
            )],
            events: vec![event(
                "Change",
                vec![arg("text", ValueType::Text { multiline: true })],
                true,
                "Raised with the new text as the user types.",
            )],
            ..widget(
                "MultilineEdit",
                "A multi-line text area.",
                (200.0, 100.0),
                Children::None,
            )
        },
        WidgetSpec {
            properties: vec![
                property(
                    "value",
                    float_type(),
                    Value::Float(0.0),
                    CATEGORY_DATA,
                    "The current value.",
                ),
                property(
                    "min",
                    float_type(),
                    Value::Float(0.0),
                    CATEGORY_DATA,
                    "The smallest allowed value.",
                ),
                property(
                    "max",
                    float_type(),
                    Value::Float(100.0),
                    CATEGORY_DATA,
                    "The largest allowed value.",
                ),
                design_property(
                    "step",
                    float_type(),
                    Value::Float(1.0),
                    CATEGORY_BEHAVIOR,
                    "The step used by the steppers and arrow keys.",
                ),
            ],
            events: vec![
                event(
                    "Change",
                    vec![arg("value", float_type())],
                    true,
                    "Raised with the new value as it changes.",
                ),
                event(
                    "Commit",
                    vec![arg("value", float_type())],
                    false,
                    "Raised with the value when it is committed.",
                ),
            ],
            ..widget(
                "NumberField",
                "A numeric field with steppers.",
                (120.0, 28.0),
                Children::None,
            )
        },
        WidgetSpec {
            properties: vec![
                property(
                    "value",
                    float_type(),
                    Value::Float(0.0),
                    CATEGORY_DATA,
                    "The current value.",
                ),
                property(
                    "min",
                    float_type(),
                    Value::Float(0.0),
                    CATEGORY_DATA,
                    "The smallest allowed value.",
                ),
                property(
                    "max",
                    float_type(),
                    Value::Float(100.0),
                    CATEGORY_DATA,
                    "The largest allowed value.",
                ),
            ],
            events: vec![
                event(
                    "Change",
                    vec![arg("value", float_type())],
                    true,
                    "Raised with the new value while dragging.",
                ),
                event(
                    "Commit",
                    vec![arg("value", float_type())],
                    false,
                    "Raised with the value when the drag ends.",
                ),
            ],
            ..widget(
                "Slider",
                "A horizontal range control.",
                (160.0, 24.0),
                Children::None,
            )
        },
        WidgetSpec {
            properties: vec![
                property(
                    "value",
                    int_type(),
                    Value::Int(0),
                    CATEGORY_DATA,
                    "The current value.",
                ),
                property(
                    "max",
                    ValueType::Int {
                        min: Some(1),
                        max: None,
                    },
                    Value::Int(100),
                    CATEGORY_DATA,
                    "The upper bound of the range `0..=max`.",
                ),
            ],
            ..widget(
                "ProgressBar",
                "A read-only progress indicator.",
                (160.0, 12.0),
                Children::None,
            )
        },
        WidgetSpec {
            properties: vec![
                design_property(
                    "items",
                    ValueType::List,
                    Value::List(Vec::new()),
                    CATEGORY_DATA,
                    "The choices in the drop-down.",
                ),
                property(
                    "selected",
                    ValueType::Int {
                        min: Some(0),
                        max: None,
                    },
                    Value::Int(0),
                    CATEGORY_DATA,
                    "The selected index.",
                ),
            ],
            events: vec![event(
                "Select",
                vec![arg("index", int_type())],
                true,
                "Raised with the chosen index.",
            )],
            ..widget(
                "ComboBox",
                "A drop-down list of choices.",
                (160.0, 24.0),
                Children::None,
            )
        },
        WidgetSpec {
            properties: vec![
                design_property(
                    "items",
                    ValueType::List,
                    Value::List(Vec::new()),
                    CATEGORY_DATA,
                    "The rows, one per line.",
                ),
                property(
                    "selected",
                    int_type(),
                    Value::Int(-1),
                    CATEGORY_DATA,
                    "The selected row, or -1 for none.",
                ),
                design_property(
                    "multi_select",
                    ValueType::Bool,
                    Value::Bool(false),
                    CATEGORY_BEHAVIOR,
                    "Whether more than one row may be selected.",
                ),
            ],
            events: vec![
                event(
                    "Select",
                    vec![arg("index", int_type())],
                    true,
                    "Raised with the primary selected row.",
                ),
                event(
                    "Activate",
                    vec![arg("index", int_type())],
                    false,
                    "Raised when a row is activated (Return or a double-click).",
                ),
            ],
            ..widget(
                "ListView",
                "A list of rows with a selection.",
                (240.0, 140.0),
                Children::None,
            )
        },
        WidgetSpec {
            properties: vec![property(
                "text",
                text_type(),
                Value::Text(String::new()),
                CATEGORY_APPEARANCE,
                "The frame's title.",
            )],
            ..widget(
                "GroupBox",
                "A titled frame that groups widgets.",
                (200.0, 120.0),
                Children::Any,
            )
        },
        WidgetSpec {
            ..widget(
                "Panel",
                "A container that owns child widgets.",
                (200.0, 120.0),
                Children::Any,
            )
        },
        WidgetSpec {
            properties: vec![design_property(
                "orientation",
                enum_type(&["horizontal", "vertical"]),
                Value::Enum("horizontal".to_owned()),
                CATEGORY_APPEARANCE,
                "The direction the divider runs.",
            )],
            ..widget("Separator", "A divider line.", (120.0, 8.0), Children::None)
        },
        WidgetSpec {
            properties: vec![property(
                "text",
                text_type(),
                Value::Text(String::new()),
                CATEGORY_APPEARANCE,
                "The link's text.",
            )],
            events: vec![event(
                "Click",
                Vec::new(),
                true,
                "Raised when the link is clicked.",
            )],
            ..widget(
                "Hyperlink",
                "A clickable link label.",
                (120.0, 20.0),
                Children::None,
            )
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_catalog_has_the_documented_kinds() {
        let catalog = Catalog::xui();
        let kinds: Vec<&str> = catalog.kinds().collect();
        assert_eq!(
            kinds,
            [
                "Button",
                "CheckBox",
                "ComboBox",
                "Edit",
                "GroupBox",
                "Hyperlink",
                "Label",
                "ListView",
                "MultilineEdit",
                "NumberField",
                "Panel",
                "ProgressBar",
                "RadioGroup",
                "Separator",
                "Slider",
                "ToggleButton",
            ]
        );
    }

    #[test]
    fn aliases_resolve_to_the_canonical_kind() {
        let mut catalog = Catalog::xui();
        catalog.alias("CommandButton", "Button");
        assert_eq!(catalog.resolve("CommandButton"), Some("Button"));
        assert_eq!(
            catalog.get("CommandButton").map(|spec| spec.kind.as_str()),
            Some("Button")
        );
        assert!(catalog.contains("CommandButton"));
        assert_eq!(catalog.resolve("Nope"), None);
    }

    #[test]
    fn common_properties_apply_to_every_kind() {
        let catalog = Catalog::xui();
        for kind in catalog.kinds() {
            let spec = catalog.get(kind).expect("kind exists");
            assert!(catalog.property(kind, "left").is_some());
            assert!(catalog.property(kind, "anchor").is_some());
            // The spec itself holds only widget-specific properties.
            assert!(spec.property("left").is_none());
        }
    }

    #[test]
    fn width_defaults_to_the_widget_size() {
        let catalog = Catalog::xui();
        let width = catalog
            .property("Button", "width")
            .expect("width is common");
        assert_eq!(width.default, Value::Int(100));
        let height = catalog
            .property("Panel", "height")
            .expect("height is common");
        assert_eq!(height.default, Value::Int(120));
    }

    #[test]
    fn only_named_widget_supports_its_events() {
        let catalog = Catalog::xui();
        let button = catalog.get("Button").expect("Button exists");
        assert!(button.event("Click").is_some());
        assert_eq!(
            button.default_event().map(|event| event.name.as_str()),
            Some("Click")
        );
        let label = catalog.get("Label").expect("Label exists");
        assert!(label.events.is_empty());
    }

    #[test]
    fn container_rules_accept_children() {
        let catalog = Catalog::xui();
        assert!(catalog.is_container("Panel"));
        assert!(catalog.accepts_child("GroupBox", "Button"));
        assert!(!catalog.accepts_child("Button", "Label"));
        assert!(!catalog.is_container("Label"));
    }

    #[test]
    fn the_catalog_serialises_to_json() {
        let catalog = Catalog::xui();
        let json = serde_json::to_string(&catalog).expect("catalog serialises");
        assert!(json.contains("\"Button\""));
        assert!(json.contains("stretch_horizontal"));
    }
}
