#![forbid(unsafe_code)]

//! The in-memory project model: [`Project`], [`ProjectItem`], [`Form`],
//! [`Control`] and [`PropValue`].
//!
//! These types are the shared vocabulary of the runtime, the designer and the
//! IDE. Their serde representation is the on-disk format (PLAN.md §3): the
//! project is one `.lrp` file, each form is one `.lfm` file. Every map is a
//! [`BTreeMap`] and every field is written in declaration order, so saving is
//! deterministic and diffs stay clean.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default form width, in device-independent pixels.
pub const DEFAULT_FORM_WIDTH: i64 = 320;
/// Default form height, in device-independent pixels.
pub const DEFAULT_FORM_HEIGHT: i64 = 200;

fn default_form_width() -> i64 {
    DEFAULT_FORM_WIDTH
}

fn default_form_height() -> i64 {
    DEFAULT_FORM_HEIGHT
}

/// A `.lrp` project: the file that ties the items together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// The project name, also the stem of the `.lrp` file.
    pub name: String,
    /// The project version, as a free-form string.
    pub version: String,
    /// The name of the item (form or module) run first.
    pub startup: String,
    /// The forms and modules that make up the project.
    pub items: Vec<ProjectItem>,
}

impl Project {
    /// A project with the given name, no items and itself as startup.
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            name: name.clone(),
            version: "0.1.0".to_owned(),
            startup: name,
            items: Vec::new(),
        }
    }

    /// The item named `name`, if any.
    pub fn item(&self, name: &str) -> Option<&ProjectItem> {
        self.items.iter().find(|item| item.name() == name)
    }

    /// The startup item, if it exists.
    pub fn startup_item(&self) -> Option<&ProjectItem> {
        self.item(&self.startup)
    }

    /// The file name of the `.lrp` on disk (`<name>.lrp`).
    pub fn file_name(&self) -> String {
        format!("{}.lrp", self.name)
    }
}

/// One form or standard module in a project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectItem {
    /// A form, with a layout file and a code-behind file.
    Form {
        /// The item name, matching `[form].name` in the layout.
        name: String,
        /// The `.lfm` layout, relative to the project directory.
        layout: PathBuf,
        /// The `.rhai` code-behind, relative to the project directory.
        code: PathBuf,
    },
    /// A standard module: code only.
    Module {
        /// The module name.
        name: String,
        /// The `.rhai` source, relative to the project directory.
        code: PathBuf,
    },
}

impl ProjectItem {
    /// The item's name.
    pub fn name(&self) -> &str {
        match self {
            Self::Form { name, .. } | Self::Module { name, .. } => name,
        }
    }

    /// The `.rhai` code path, relative to the project directory.
    pub fn code(&self) -> &Path {
        match self {
            Self::Form { code, .. } | Self::Module { code, .. } => code,
        }
    }

    /// The `.lfm` layout path for a form, or `None` for a module.
    pub fn layout(&self) -> Option<&Path> {
        match self {
            Self::Form { layout, .. } => Some(layout),
            Self::Module { .. } => None,
        }
    }

    /// Whether this item is a form.
    pub fn is_form(&self) -> bool {
        matches!(self, Self::Form { .. })
    }
}

/// A form layout (`.lfm`): a header and its controls.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    /// The form name.
    pub name: String,
    /// The caption shown in the title bar.
    pub caption: String,
    /// The width, in device-independent pixels.
    pub width: i64,
    /// The height, in device-independent pixels.
    pub height: i64,
    /// The controls on the form, in tab order.
    pub controls: Vec<Control>,
}

impl Form {
    /// A form with the given name and no controls.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            caption: String::new(),
            width: DEFAULT_FORM_WIDTH,
            height: DEFAULT_FORM_HEIGHT,
            controls: Vec::new(),
        }
    }

    /// The control named `name`, if any.
    pub fn control(&self, name: &str) -> Option<&Control> {
        self.controls.iter().find(|control| control.name == name)
    }
}

/// The on-disk shape of a `.lfm`: a `[form]` table and `[[control]]` entries.
#[derive(Serialize, Deserialize)]
pub(crate) struct FormFile {
    form: FormHeader,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    control: Vec<Control>,
}

/// The `[form]` part of a `.lfm`.
#[derive(Serialize, Deserialize)]
struct FormHeader {
    name: String,
    #[serde(default)]
    caption: String,
    #[serde(default = "default_form_width")]
    width: i64,
    #[serde(default = "default_form_height")]
    height: i64,
}

impl From<&Form> for FormFile {
    fn from(form: &Form) -> Self {
        Self {
            form: FormHeader {
                name: form.name.clone(),
                caption: form.caption.clone(),
                width: form.width,
                height: form.height,
            },
            control: form.controls.clone(),
        }
    }
}

impl From<FormFile> for Form {
    fn from(file: FormFile) -> Self {
        Self {
            name: file.form.name,
            caption: file.form.caption,
            width: file.form.width,
            height: file.form.height,
            controls: file.control,
        }
    }
}

/// A control placed on a form.
///
/// Geometry (`left`, `top`, `width`, `height`) and `tab_index` are first-class
/// because every control has them. Everything else lives in `props`, keyed by
/// the property names the schema registry declares. The map is flat on disk:
/// each property is an ordinary key in the `[[control]]` table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Control {
    /// The control type, for example `CommandButton`. Written as `type`.
    #[serde(rename = "type")]
    pub type_name: String,
    /// The control name; a unique identifier within the form.
    pub name: String,
    /// The x offset, in device-independent pixels.
    pub left: i64,
    /// The y offset, in device-independent pixels.
    pub top: i64,
    /// The width, in device-independent pixels.
    pub width: i64,
    /// The height, in device-independent pixels.
    pub height: i64,
    /// The position in the form's tab order.
    pub tab_index: i64,
    /// The remaining properties, sorted by name for a stable file.
    #[serde(flatten)]
    pub props: BTreeMap<String, PropValue>,
}

impl Control {
    /// A control of the given type and name at the origin, with default size.
    pub fn new(type_name: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            name: name.into(),
            left: 0,
            top: 0,
            width: 120,
            height: 24,
            tab_index: 0,
            props: BTreeMap::new(),
        }
    }

    /// Sets a property and returns `self`, for a fluent construction.
    #[must_use]
    pub fn with(mut self, name: impl Into<String>, value: impl Into<PropValue>) -> Self {
        self.props.insert(name.into(), value.into());
        self
    }

    /// The property named `name`, if set.
    pub fn prop(&self, name: &str) -> Option<&PropValue> {
        self.props.get(name)
    }
}

/// A typed property value.
///
/// On disk the first four variants are TOML scalars and [`PropValue::Enum`] is
/// a plain string; the schema tells the two string kinds apart, so a load
/// re-types enum properties from the registry (see [`crate::schema`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropValue {
    /// `true` or `false`.
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// A real number.
    Float(f64),
    /// A free-form string.
    Text(String),
    /// A string that is one of an enumerated set.
    Enum(String),
}

impl PropValue {
    /// The variant's name, for diagnostics.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Text(_) => "text",
            Self::Enum(_) => "enum",
        }
    }

    /// The boolean inside, if this is [`PropValue::Bool`].
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The integer inside, if this is [`PropValue::Int`].
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// The float inside, if this is [`PropValue::Float`].
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(value) => Some(*value),
            _ => None,
        }
    }

    /// The string inside, if this is [`PropValue::Text`] or [`PropValue::Enum`].
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(value) | Self::Enum(value) => Some(value),
            _ => None,
        }
    }
}

impl From<bool> for PropValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for PropValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<f64> for PropValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<&str> for PropValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for PropValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_item_accessors_cover_both_variants() {
        let form = ProjectItem::Form {
            name: "frmMain".to_owned(),
            layout: PathBuf::from("frmMain.lfm"),
            code: PathBuf::from("frmMain.rhai"),
        };
        let module = ProjectItem::Module {
            name: "modUtil".to_owned(),
            code: PathBuf::from("modUtil.rhai"),
        };
        assert_eq!(form.name(), "frmMain");
        assert_eq!(form.code(), Path::new("frmMain.rhai"));
        assert_eq!(form.layout(), Some(Path::new("frmMain.lfm")));
        assert!(form.is_form());
        assert_eq!(module.layout(), None);
        assert!(!module.is_form());
    }

    #[test]
    fn prop_value_accessors_are_variant_aware() {
        assert_eq!(PropValue::Bool(true).as_bool(), Some(true));
        assert_eq!(PropValue::Int(3).as_int(), Some(3));
        assert_eq!(PropValue::Float(1.5).as_float(), Some(1.5));
        assert_eq!(PropValue::Text("x".into()).as_str(), Some("x"));
        assert_eq!(PropValue::Enum("left".into()).as_str(), Some("left"));
        assert_eq!(PropValue::Int(3).as_str(), None);
    }

    #[test]
    fn startup_item_resolves() {
        let mut project = Project::new("MyApp");
        project.items.push(ProjectItem::Module {
            name: "modUtil".to_owned(),
            code: PathBuf::from("modUtil.rhai"),
        });
        assert!(project.startup_item().is_none());
        project.startup = "modUtil".to_owned();
        assert_eq!(
            project.startup_item().map(ProjectItem::name),
            Some("modUtil")
        );
        assert_eq!(project.file_name(), "MyApp.lrp");
    }
}
