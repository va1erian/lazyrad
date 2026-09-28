#![forbid(unsafe_code)]

//! The control schema registry.
//!
//! xui's [`Properties`] surface reports only a handful of names per widget
//! (PLAN.md §10, "Designer seam status"), so LazyRAD owns the full schema: for
//! every control type, the properties it accepts (with type, default, enum
//! values and category) and the events it raises (with arguments and which one
//! is the default). The runtime, the designer's property grid and the code
//! editor's completion all read this one source of truth.
//!
//! [`Properties`]: https://docs.rs/xui-core

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::model::PropValue;

/// The type a property's value must have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyType {
    /// A boolean.
    Bool,
    /// A whole number.
    Int,
    /// A real number; an integer is also accepted.
    Float,
    /// Free-form text.
    Text,
    /// One of a fixed set of strings.
    Enum,
}

/// One property a control type accepts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropertySchema {
    /// The property name, as written in the `.lfm`.
    pub name: String,
    /// The value type.
    pub ty: PropertyType,
    /// The value used when the property is absent.
    pub default: PropValue,
    /// The allowed values when `ty` is [`PropertyType::Enum`].
    pub enum_values: Vec<String>,
    /// The property grid's grouping, for example `Appearance`.
    pub category: String,
}

impl PropertySchema {
    /// The value type's display name.
    pub fn type_name(&self) -> &'static str {
        match self.ty {
            PropertyType::Bool => "bool",
            PropertyType::Int => "int",
            PropertyType::Float => "float",
            PropertyType::Text => "text",
            PropertyType::Enum => "enum",
        }
    }

    /// Whether `value` has a compatible type. Enum membership is checked
    /// separately by the validator, which needs to report the allowed set.
    pub fn accepts(&self, value: &PropValue) -> bool {
        match self.ty {
            PropertyType::Bool => matches!(value, PropValue::Bool(_)),
            PropertyType::Int => matches!(value, PropValue::Int(_)),
            PropertyType::Float => matches!(value, PropValue::Float(_) | PropValue::Int(_)),
            PropertyType::Text | PropertyType::Enum => {
                matches!(value, PropValue::Text(_) | PropValue::Enum(_))
            }
        }
    }
}

/// One event a control type raises.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSchema {
    /// The event name, appended to the control name (`cmdHello_Click`).
    pub name: String,
    /// The handler's argument names, in order.
    pub args: Vec<String>,
    /// Whether this is the event opened when the control is double-clicked.
    pub is_default: bool,
}

/// The properties and events of one control type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControlSchema {
    /// The type name, matching a control's `type`.
    pub type_name: String,
    /// The accepted properties, in a stable order.
    pub properties: Vec<PropertySchema>,
    /// The raised events, in a stable order.
    pub events: Vec<EventSchema>,
}

impl ControlSchema {
    /// The property named `name`, if declared.
    pub fn property(&self, name: &str) -> Option<&PropertySchema> {
        self.properties
            .iter()
            .find(|property| property.name == name)
    }

    /// The event named `name`, if declared.
    pub fn event(&self, name: &str) -> Option<&EventSchema> {
        self.events.iter().find(|event| event.name == name)
    }

    /// The default event, if one is declared.
    pub fn default_event(&self) -> Option<&EventSchema> {
        self.events.iter().find(|event| event.is_default)
    }
}

/// The control types LazyRAD knows, keyed by type name.
///
/// The built-in set is the Iteration 1 list (PLAN.md §4.4): `Label`,
/// `TextBox`, `CommandButton`, `CheckBox`, `OptionButton`, `Frame`, `ListBox`
/// and `ComboBox`. Construction is fallible-free and immutable once built; the
/// shared instance is [`SchemaRegistry::builtin`].
///
/// The form itself is not a control type, but it has properties and events
/// (`Form_Load`, `Form_Unload`, …) too; [`SchemaRegistry::form`] describes
/// them.
#[derive(Clone, Debug, Default)]
pub struct SchemaRegistry {
    controls: BTreeMap<String, ControlSchema>,
    form: Option<ControlSchema>,
}

/// The built-in registry, shared by every caller.
static BUILTIN: LazyLock<SchemaRegistry> = LazyLock::new(SchemaRegistry::build_builtin);

impl SchemaRegistry {
    /// The registry of built-in control schemas.
    pub fn builtin() -> &'static SchemaRegistry {
        &BUILTIN
    }

    /// An empty registry, to which schemas can be added with [`Self::insert`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a control schema, replacing any schema with the same type name.
    pub fn insert(&mut self, schema: ControlSchema) {
        self.controls.insert(schema.type_name.clone(), schema);
    }

    /// The schema of the form itself: its properties and its `Form_*` events.
    ///
    /// Every registry has one; an empty registry falls back to the built-in
    /// form schema.
    pub fn form(&self) -> &ControlSchema {
        self.form
            .as_ref()
            .or(BUILTIN.form.as_ref())
            .expect("the built-in registry has a form schema")
    }

    /// Replaces the form schema.
    pub fn set_form(&mut self, schema: ControlSchema) {
        self.form = Some(schema);
    }

    /// The schema for `type_name`, if known.
    pub fn get(&self, type_name: &str) -> Option<&ControlSchema> {
        self.controls.get(type_name)
    }

    /// Whether `type_name` is known.
    pub fn contains(&self, type_name: &str) -> bool {
        self.controls.contains_key(type_name)
    }

    /// The known type names, in sorted order.
    pub fn type_names(&self) -> impl Iterator<Item = &str> {
        self.controls.keys().map(String::as_str)
    }

    /// The number of known control types.
    pub fn len(&self) -> usize {
        self.controls.len()
    }

    /// Whether the registry knows no control types.
    pub fn is_empty(&self) -> bool {
        self.controls.is_empty()
    }

    /// Builds the eight Iteration 1 control schemas and the form schema.
    fn build_builtin() -> Self {
        let mut registry = Self::new();
        for schema in [
            label_schema(),
            text_box_schema(),
            command_button_schema(),
            check_box_schema(),
            option_button_schema(),
            frame_schema(),
            list_box_schema(),
            combo_box_schema(),
        ] {
            registry.insert(schema);
        }
        registry.set_form(form_schema());
        registry
    }
}

// The schema builders below keep the data readable: each control is one list
// of properties and one list of events.

fn bool_property(name: &str, default: bool, category: &str) -> PropertySchema {
    PropertySchema {
        name: name.to_owned(),
        ty: PropertyType::Bool,
        default: PropValue::Bool(default),
        enum_values: Vec::new(),
        category: category.to_owned(),
    }
}

fn int_property(name: &str, default: i64, category: &str) -> PropertySchema {
    PropertySchema {
        name: name.to_owned(),
        ty: PropertyType::Int,
        default: PropValue::Int(default),
        enum_values: Vec::new(),
        category: category.to_owned(),
    }
}

fn text_property(name: &str, default: &str, category: &str) -> PropertySchema {
    PropertySchema {
        name: name.to_owned(),
        ty: PropertyType::Text,
        default: PropValue::Text(default.to_owned()),
        enum_values: Vec::new(),
        category: category.to_owned(),
    }
}

fn enum_property(name: &str, default: &str, values: &[&str], category: &str) -> PropertySchema {
    PropertySchema {
        name: name.to_owned(),
        ty: PropertyType::Enum,
        default: PropValue::Enum(default.to_owned()),
        enum_values: values.iter().map(|value| (*value).to_owned()).collect(),
        category: category.to_owned(),
    }
}

fn event(name: &str, is_default: bool) -> EventSchema {
    EventSchema {
        name: name.to_owned(),
        args: Vec::new(),
        is_default,
    }
}

fn event_with_args(name: &str, args: &[&str], is_default: bool) -> EventSchema {
    EventSchema {
        name: name.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        is_default,
    }
}

/// The properties the schema gives every visible control.
fn visibility_properties() -> Vec<PropertySchema> {
    vec![
        bool_property("enabled", true, "Behavior"),
        bool_property("visible", true, "Behavior"),
    ]
}

fn form_schema() -> ControlSchema {
    ControlSchema {
        type_name: "Form".to_owned(),
        properties: vec![
            text_property("caption", "", "Appearance"),
            int_property("width", crate::model::DEFAULT_FORM_WIDTH, "Position"),
            int_property("height", crate::model::DEFAULT_FORM_HEIGHT, "Position"),
        ],
        events: vec![
            event("Load", true),
            event("Unload", false),
            event("Click", false),
            event("Resize", false),
        ],
    }
}

fn label_schema() -> ControlSchema {
    let mut properties = vec![
        text_property("caption", "", "Appearance"),
        enum_property(
            "alignment",
            "left",
            &["left", "center", "right"],
            "Appearance",
        ),
    ];
    properties.extend(visibility_properties());
    ControlSchema {
        type_name: "Label".to_owned(),
        properties,
        events: vec![event("Click", true), event("DblClick", false)],
    }
}

fn text_box_schema() -> ControlSchema {
    let mut properties = vec![
        text_property("text", "", "Data"),
        bool_property("multiline", false, "Behavior"),
        bool_property("password", false, "Behavior"),
        bool_property("readonly", false, "Behavior"),
        int_property("max_length", 0, "Behavior"),
    ];
    properties.extend(visibility_properties());
    ControlSchema {
        type_name: "TextBox".to_owned(),
        properties,
        events: vec![
            event("Change", true),
            event_with_args("KeyPress", &["key"], false),
            event("GotFocus", false),
            event("LostFocus", false),
        ],
    }
}

fn command_button_schema() -> ControlSchema {
    let mut properties = vec![
        text_property("caption", "", "Appearance"),
        bool_property("default", false, "Behavior"),
        bool_property("cancel", false, "Behavior"),
    ];
    properties.extend(visibility_properties());
    ControlSchema {
        type_name: "CommandButton".to_owned(),
        properties,
        events: vec![event("Click", true), event("DblClick", false)],
    }
}

fn check_box_schema() -> ControlSchema {
    let mut properties = vec![
        text_property("caption", "", "Appearance"),
        bool_property("checked", false, "Data"),
    ];
    properties.extend(visibility_properties());
    ControlSchema {
        type_name: "CheckBox".to_owned(),
        properties,
        events: vec![event("Click", true), event("Change", false)],
    }
}

fn option_button_schema() -> ControlSchema {
    let mut properties = vec![
        text_property("caption", "", "Appearance"),
        bool_property("checked", false, "Data"),
    ];
    properties.extend(visibility_properties());
    ControlSchema {
        type_name: "OptionButton".to_owned(),
        properties,
        events: vec![event("Click", true), event("Change", false)],
    }
}

fn frame_schema() -> ControlSchema {
    let mut properties = vec![text_property("caption", "", "Appearance")];
    properties.extend(visibility_properties());
    ControlSchema {
        type_name: "Frame".to_owned(),
        properties,
        events: vec![event("Click", true), event("DblClick", false)],
    }
}

fn list_box_schema() -> ControlSchema {
    let mut properties = vec![
        int_property("selected_index", 0, "Data"),
        bool_property("multi_select", false, "Behavior"),
    ];
    properties.extend(visibility_properties());
    ControlSchema {
        type_name: "ListBox".to_owned(),
        properties,
        events: vec![
            event("Click", true),
            event("DblClick", false),
            event("Change", false),
        ],
    }
}

fn combo_box_schema() -> ControlSchema {
    let mut properties = vec![
        text_property("text", "", "Data"),
        enum_property("style", "dropdown", &["dropdown", "simple"], "Appearance"),
        bool_property("sorted", false, "Behavior"),
    ];
    properties.extend(visibility_properties());
    ControlSchema {
        type_name: "ComboBox".to_owned(),
        properties,
        events: vec![
            event("Change", true),
            event("Click", false),
            event("DblClick", false),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_has_the_iteration_one_controls() {
        let registry = SchemaRegistry::builtin();
        let names: Vec<&str> = registry.type_names().collect();
        assert_eq!(
            names,
            [
                "CheckBox",
                "ComboBox",
                "CommandButton",
                "Frame",
                "Label",
                "ListBox",
                "OptionButton",
                "TextBox",
            ]
        );
        assert_eq!(registry.len(), 8);
    }

    #[test]
    fn every_control_declares_a_default_event() {
        for name in SchemaRegistry::builtin().type_names() {
            let schema = SchemaRegistry::builtin()
                .get(name)
                .expect("named type exists");
            let default = schema.default_event();
            assert!(default.is_some(), "{name} has no default event");
            assert_eq!(
                schema
                    .events
                    .iter()
                    .filter(|event| event.is_default)
                    .count(),
                1,
                "{name} has more than one default event"
            );
        }
    }

    #[test]
    fn the_form_schema_is_separate_from_control_types() {
        let registry = SchemaRegistry::builtin();
        let form = registry.form();
        assert_eq!(form.type_name, "Form");
        assert_eq!(form.default_event().map(|e| e.name.as_str()), Some("Load"));
        assert!(form.event("Unload").is_some());
        assert!(!registry.contains("Form"));
        assert_eq!(SchemaRegistry::new().form().type_name, "Form");
    }

    #[test]
    fn enum_properties_carry_their_values_and_default() {
        let schema = SchemaRegistry::builtin()
            .get("ComboBox")
            .expect("ComboBox exists");
        let style = schema.property("style").expect("style is declared");
        assert_eq!(style.ty, PropertyType::Enum);
        assert_eq!(style.default, PropValue::Enum("dropdown".to_owned()));
        assert_eq!(style.enum_values, ["dropdown", "simple"]);
        assert!(style.accepts(&PropValue::Text("simple".to_owned())));
        assert!(!style.accepts(&PropValue::Int(1)));
    }

    #[test]
    fn float_properties_accept_integers() {
        let float = PropertySchema {
            name: "ratio".to_owned(),
            ty: PropertyType::Float,
            default: PropValue::Float(0.0),
            enum_values: Vec::new(),
            category: "Layout".to_owned(),
        };
        assert!(float.accepts(&PropValue::Float(1.5)));
        assert!(float.accepts(&PropValue::Int(2)));
        assert!(!float.accepts(&PropValue::Text("x".to_owned())));
    }
}
