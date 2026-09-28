#![forbid(unsafe_code)]

//! Typed property values and the schema-guided TOML codec.
//!
//! A form stores properties as [`Value`]s, but its dependencies are plain TOML.
//! The schema decides how a raw TOML literal decodes: a `String` may be a
//! [`Value::Text`] or a [`Value::Enum`] depending on the property's
//! [`ValueType`], and `"#ff0000"` is a [`Value::Color`] only where the schema
//! says so. This is what fixes the ambiguity of an untagged value on disk.
//!
//! An [`Value::Int`] is accepted wherever a [`ValueType::Float`] is expected, so
//! a hand-written `1` reads as `1.0`.

use serde::Serialize;
use xui_core::Color;

/// A typed property value.
///
/// The variant records how the schema decoded a raw TOML literal; it is not on
/// disk, which only stores the literal.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A boolean.
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// A real number; an [`Value::Int`] is also accepted where a float is
    /// expected.
    Float(f64),
    /// Free-form text.
    Text(String),
    /// One of a fixed set of strings.
    Enum(String),
    /// An opaque colour.
    Color(Color),
    /// A list of strings (list and combo items).
    List(Vec<String>),
}

impl Value {
    /// The variant's name, for diagnostics.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Text(_) => "text",
            Value::Enum(_) => "enum",
            Value::Color(_) => "color",
            Value::List(_) => "list",
        }
    }

    /// The boolean inside, if any.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The integer inside, if any.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// The real number inside, widening an [`Value::Int`].
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(value) => Some(*value),
            Value::Int(value) => Some(*value as f64),
            _ => None,
        }
    }

    /// The string inside, for both [`Value::Text`] and [`Value::Enum`].
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(value) | Value::Enum(value) => Some(value),
            _ => None,
        }
    }

    /// The colour inside, if any.
    pub fn as_color(&self) -> Option<Color> {
        match self {
            Value::Color(value) => Some(*value),
            _ => None,
        }
    }

    /// The string list inside, if any.
    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            Value::List(value) => Some(value),
            _ => None,
        }
    }

    /// Decodes a raw TOML literal according to `ty`.
    ///
    /// The decode is deliberately permissive about enum membership: an unknown
    /// variant still decodes to [`Value::Enum`] so [`crate::validate`] can
    /// report it against the document rather than failing the whole load.
    pub fn from_toml(raw: &toml::Value, ty: &ValueType) -> Result<Value, DecodeError> {
        match ty {
            ValueType::Bool => raw
                .as_bool()
                .map(Value::Bool)
                .ok_or_else(|| DecodeError::new(ty.type_name(), raw.type_name())),
            ValueType::Int { min, max } => {
                let value = raw
                    .as_integer()
                    .ok_or_else(|| DecodeError::new(ty.type_name(), raw.type_name()))?;
                check_int_range(value, *min, *max)?;
                Ok(Value::Int(value))
            }
            ValueType::Float { min, max } => {
                let value = match raw {
                    toml::Value::Integer(value) => *value as f64,
                    toml::Value::Float(value) => *value,
                    _ => return Err(DecodeError::new(ty.type_name(), raw.type_name())),
                };
                check_float_range(value, *min, *max)?;
                Ok(Value::Float(value))
            }
            ValueType::Text { .. } => raw
                .as_str()
                .map(|text| Value::Text(text.to_owned()))
                .ok_or_else(|| DecodeError::new(ty.type_name(), raw.type_name())),
            ValueType::Enum { .. } => raw
                .as_str()
                .map(|text| Value::Enum(text.to_owned()))
                .ok_or_else(|| DecodeError::new(ty.type_name(), raw.type_name())),
            ValueType::Color => {
                let text = raw
                    .as_str()
                    .ok_or_else(|| DecodeError::new(ty.type_name(), raw.type_name()))?;
                parse_color(text)
                    .map(Value::Color)
                    .ok_or_else(|| DecodeError::new("a `#rrggbb` colour", format!("`{text}`")))
            }
            ValueType::List => {
                let items = raw
                    .as_array()
                    .ok_or_else(|| DecodeError::new(ty.type_name(), raw.type_name()))?;
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    let text = item
                        .as_str()
                        .ok_or_else(|| DecodeError::new("a list of strings", item.type_name()))?;
                    out.push(text.to_owned());
                }
                Ok(Value::List(out))
            }
        }
    }

    /// Encodes the value as a raw TOML literal.
    pub fn to_toml(&self) -> toml::Value {
        match self {
            Value::Bool(value) => toml::Value::Boolean(*value),
            Value::Int(value) => toml::Value::Integer(*value),
            Value::Float(value) => toml::Value::Float(*value),
            Value::Text(value) | Value::Enum(value) => toml::Value::String(value.clone()),
            Value::Color(value) => toml::Value::String(format_color(*value)),
            Value::List(items) => {
                toml::Value::Array(items.iter().cloned().map(toml::Value::String).collect())
            }
        }
    }
}

impl Serialize for Value {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_toml().serialize(serializer)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Text(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Text(value)
    }
}

impl From<Color> for Value {
    fn from(value: Color) -> Self {
        Value::Color(value)
    }
}

/// The type a property's value must have.
///
/// The bounds and variants are part of the schema, so a designer can build the
/// right editor and validation can check without a second source of truth.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValueType {
    /// A boolean.
    Bool,
    /// A whole number, optionally bounded (inclusive).
    Int {
        /// The smallest allowed value.
        min: Option<i64>,
        /// The largest allowed value.
        max: Option<i64>,
    },
    /// A real number, optionally bounded (inclusive); an integer is accepted.
    Float {
        /// The smallest allowed value.
        min: Option<f64>,
        /// The largest allowed value.
        max: Option<f64>,
    },
    /// Free-form text, possibly multi-line.
    Text {
        /// Whether the editor should allow line breaks.
        multiline: bool,
    },
    /// One of a fixed set of strings.
    Enum {
        /// The allowed strings, in display order.
        variants: Vec<String>,
    },
    /// An opaque `#rrggbb` colour.
    Color,
    /// A list of strings.
    List,
}

impl ValueType {
    /// The type's display name, for diagnostics.
    pub fn type_name(&self) -> &'static str {
        match self {
            ValueType::Bool => "bool",
            ValueType::Int { .. } => "int",
            ValueType::Float { .. } => "float",
            ValueType::Text { .. } => "text",
            ValueType::Enum { .. } => "enum",
            ValueType::Color => "color",
            ValueType::List => "list",
        }
    }

    /// The allowed enum variants, or an empty slice for other kinds.
    pub fn variants(&self) -> &[String] {
        match self {
            ValueType::Enum { variants } => variants,
            _ => &[],
        }
    }

    /// Whether `value` has a compatible type; enum membership and the numeric
    /// bounds are checked here too.
    pub fn accepts(&self, value: &Value) -> bool {
        match self {
            ValueType::Bool => matches!(value, Value::Bool(_)),
            ValueType::Int { min, max } => match value {
                Value::Int(value) => in_int_range(*value, *min, *max),
                _ => false,
            },
            ValueType::Float { min, max } => match value.as_float() {
                Some(value) => in_float_range(value, *min, *max),
                None => false,
            },
            ValueType::Text { .. } => matches!(value, Value::Text(_)),
            ValueType::Enum { variants } => match value {
                Value::Enum(value) => variants.iter().any(|variant| variant == value),
                _ => false,
            },
            ValueType::Color => matches!(value, Value::Color(_)),
            ValueType::List => matches!(value, Value::List(_)),
        }
    }
}

/// A failure to decode a raw TOML literal against a [`ValueType`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("expected {expected}, found {found}")]
pub struct DecodeError {
    /// A description of what the schema wanted.
    pub expected: String,
    /// A description of what was on disk.
    pub found: String,
}

impl DecodeError {
    /// Creates a decode error from the expected and found descriptions.
    pub fn new(expected: impl Into<String>, found: impl Into<String>) -> Self {
        DecodeError {
            expected: expected.into(),
            found: found.into(),
        }
    }
}

/// Checks an integer against optional inclusive bounds.
fn check_int_range(value: i64, min: Option<i64>, max: Option<i64>) -> Result<(), DecodeError> {
    if in_int_range(value, min, max) {
        Ok(())
    } else {
        Err(DecodeError::new(
            format!("an int in {}", describe_int_bounds(min, max)),
            format!("`{value}`"),
        ))
    }
}

/// Checks a float against optional inclusive bounds.
fn check_float_range(value: f64, min: Option<f64>, max: Option<f64>) -> Result<(), DecodeError> {
    if in_float_range(value, min, max) {
        Ok(())
    } else {
        Err(DecodeError::new(
            format!("a float in {}", describe_float_bounds(min, max)),
            format!("`{value}`"),
        ))
    }
}

/// Whether `value` lies within the inclusive integer bounds.
fn in_int_range(value: i64, min: Option<i64>, max: Option<i64>) -> bool {
    min.is_none_or(|min| value >= min) && max.is_none_or(|max| value <= max)
}

/// Whether `value` lies within the inclusive float bounds.
fn in_float_range(value: f64, min: Option<f64>, max: Option<f64>) -> bool {
    min.is_none_or(|min| value >= min) && max.is_none_or(|max| value <= max)
}

/// Renders integer bounds for a diagnostic.
fn describe_int_bounds(min: Option<i64>, max: Option<i64>) -> String {
    match (min, max) {
        (Some(min), Some(max)) => format!("{min}..={max}"),
        (Some(min), None) => format!("at least {min}"),
        (None, Some(max)) => format!("at most {max}"),
        (None, None) => "range".to_owned(),
    }
}

/// Renders float bounds for a diagnostic.
fn describe_float_bounds(min: Option<f64>, max: Option<f64>) -> String {
    match (min, max) {
        (Some(min), Some(max)) => format!("{min}..={max}"),
        (Some(min), None) => format!("at least {min}"),
        (None, Some(max)) => format!("at most {max}"),
        (None, None) => "range".to_owned(),
    }
}

/// Parses `#rrggbb` or `#rrggbbaa` (the leading `#` is optional), ignoring any
/// alpha channel because [`Color`] is opaque.
pub(crate) fn parse_color(text: &str) -> Option<Color> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    // `#rrggbb` is already in `Color::hex` order; `#rrggbbaa` drops the alpha.
    Some(if hex.len() == 6 {
        Color::hex(value)
    } else {
        Color::hex(value >> 8)
    })
}

/// Formats an opaque colour as `#rrggbb`.
pub(crate) fn format_color(color: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

/// The TOML literal for a value, as it would appear on disk (used by tests).
#[cfg(test)]
pub(crate) fn toml_literal(value: &Value) -> String {
    value.to_toml().to_string()
}

/// The TOML kind name of a raw value (for error messages).
trait TomlKind {
    fn type_name(&self) -> &'static str;
}

impl TomlKind for toml::Value {
    fn type_name(&self) -> &'static str {
        match self {
            toml::Value::String(_) => "a string",
            toml::Value::Integer(_) => "an int",
            toml::Value::Float(_) => "a float",
            toml::Value::Boolean(_) => "a bool",
            toml::Value::Datetime(_) => "a datetime",
            toml::Value::Array(_) => "an array",
            toml::Value::Table(_) => "a table",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ints_decode_where_floats_are_expected() {
        let ty = ValueType::Float {
            min: None,
            max: None,
        };
        assert_eq!(
            Value::from_toml(&toml::Value::Integer(3), &ty),
            Ok(Value::Float(3.0))
        );
        assert_eq!(
            Value::from_toml(&toml::Value::Float(1.5), &ty),
            Ok(Value::Float(1.5))
        );
    }

    #[test]
    fn the_schema_decides_text_versus_enum() {
        let text = ValueType::Text { multiline: false };
        let enumeration = ValueType::Enum {
            variants: vec!["left".to_owned(), "right".to_owned()],
        };
        let raw = toml::Value::String("left".to_owned());
        assert_eq!(
            Value::from_toml(&raw, &text),
            Ok(Value::Text("left".to_owned()))
        );
        assert_eq!(
            Value::from_toml(&raw, &enumeration),
            Ok(Value::Enum("left".to_owned()))
        );
    }

    #[test]
    fn colours_parse_with_and_without_alpha() {
        let ty = ValueType::Color;
        let red = toml::Value::String("#ff0000".to_owned());
        let red_alpha = toml::Value::String("#ff000080".to_owned());
        assert_eq!(
            Value::from_toml(&red, &ty),
            Ok(Value::Color(Color::rgb(255, 0, 0)))
        );
        assert_eq!(
            Value::from_toml(&red_alpha, &ty),
            Ok(Value::Color(Color::rgb(255, 0, 0)))
        );
        assert_eq!(
            Value::from_toml(&toml::Value::String("nope".to_owned()), &ty),
            Err(DecodeError::new("a `#rrggbb` colour", "`nope`"))
        );
    }

    #[test]
    fn a_colour_encodes_as_lowercase_hex() {
        let value = Value::Color(Color::rgb(0x12, 0x34, 0x56));
        assert_eq!(toml_literal(&value), "\"#123456\"");
    }

    #[test]
    fn ranges_are_checked() {
        let ty = ValueType::Int {
            min: Some(0),
            max: Some(10),
        };
        assert!(Value::from_toml(&toml::Value::Integer(5), &ty).is_ok());
        assert!(Value::from_toml(&toml::Value::Integer(11), &ty).is_err());
    }

    #[test]
    fn enums_accept_unknown_members_for_validation() {
        let ty = ValueType::Enum {
            variants: vec!["a".to_owned()],
        };
        assert_eq!(
            Value::from_toml(&toml::Value::String("b".to_owned()), &ty),
            Ok(Value::Enum("b".to_owned()))
        );
        assert!(!ty.accepts(&Value::Enum("b".to_owned())));
    }

    #[test]
    fn lists_require_strings() {
        let ty = ValueType::List;
        let raw = toml::Value::Array(vec![
            toml::Value::String("one".to_owned()),
            toml::Value::String("two".to_owned()),
        ]);
        assert_eq!(
            Value::from_toml(&raw, &ty),
            Ok(Value::List(vec!["one".to_owned(), "two".to_owned()]))
        );
        let bad = toml::Value::Array(vec![toml::Value::Integer(1)]);
        assert!(Value::from_toml(&bad, &ty).is_err());
    }

    #[test]
    fn values_round_trip_through_toml() {
        for value in [
            Value::Bool(true),
            Value::Int(-4),
            Value::Float(1.5),
            Value::Text("hello \"world\"".to_owned()),
            Value::Enum("left".to_owned()),
            Value::Color(Color::rgb(1, 2, 3)),
            Value::List(vec!["a".to_owned(), "b".to_owned()]),
        ] {
            let raw = value.to_toml();
            let ty = match &value {
                Value::Bool(_) => ValueType::Bool,
                Value::Int(_) => ValueType::Int {
                    min: None,
                    max: None,
                },
                Value::Float(_) => ValueType::Float {
                    min: None,
                    max: None,
                },
                Value::Text(_) => ValueType::Text { multiline: false },
                Value::Enum(_) => ValueType::Enum {
                    variants: Vec::new(),
                },
                Value::Color(_) => ValueType::Color,
                Value::List(_) => ValueType::List,
            };
            assert_eq!(Value::from_toml(&raw, &ty), Ok(value));
        }
    }
}
