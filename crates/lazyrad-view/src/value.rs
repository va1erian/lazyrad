//! Property values and their conversion to and from Rhai.

use std::fmt;

use rhai::Dynamic;

/// A property value a node can hold and a binding can produce.
///
/// Equality is what the diff uses to decide whether a `Set` is needed. It is
/// exact: `Int(1)` is not `Float(1.0)`, and a `Float` NaN equals itself, so a
/// NaN result does not produce a patch every turn.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Value {
    /// A boolean.
    Bool(bool),
    /// A 64-bit integer.
    Int(i64),
    /// A 64-bit float.
    Float(f64),
    /// Free text.
    Text(String),
    /// The name of an enum variant, such as `Horizontal`.
    ///
    /// A Rhai script has no enum type, so a binding produces it as a string;
    /// the property's declared kind (see [`ValueKind`]) decides whether that
    /// string is [`Value::Text`] or [`Value::Enum`].
    Enum(String),
}

/// The type of a [`Value`], without the payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ValueKind {
    /// [`Value::Bool`].
    Bool,
    /// [`Value::Int`].
    Int,
    /// [`Value::Float`].
    Float,
    /// [`Value::Text`].
    Text,
    /// [`Value::Enum`].
    Enum,
}

impl fmt::Display for ValueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Float => "float",
            Self::Text => "text",
            Self::Enum => "enum",
        })
    }
}

/// Why a Rhai value is not a property [`Value`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ValueError {
    /// The expression produced `()`, usually a missing key or a branch with no
    /// `else`.
    #[error("the expression produced no value (`()`)")]
    Unit,
    /// The result is a type a property cannot hold (an array, a map, a char,
    /// a function pointer, ...).
    #[error("`{type_name}` is not a property value (bool, int, float or string)")]
    Unsupported {
        /// The Rhai type name of the result.
        type_name: String,
    },
    /// The result is a supported value of another kind than the property's.
    ///
    /// Values are never coerced: an integer is not silently widened to a float
    /// nor truncated the other way.
    #[error(
        "expected {expected}, got {found}; values are not coerced (convert explicitly, for example with `.to_float()`)"
    )]
    KindMismatch {
        /// The kind the property holds.
        expected: ValueKind,
        /// The kind the result has.
        found: ValueKind,
    },
}

impl Value {
    /// An [`Enum`](Value::Enum) value naming `variant`.
    #[must_use]
    pub fn variant(variant: impl Into<String>) -> Self {
        Self::Enum(variant.into())
    }

    /// The kind of this value.
    #[must_use]
    pub const fn kind(&self) -> ValueKind {
        match self {
            Self::Bool(_) => ValueKind::Bool,
            Self::Int(_) => ValueKind::Int,
            Self::Float(_) => ValueKind::Float,
            Self::Text(_) => ValueKind::Text,
            Self::Enum(_) => ValueKind::Enum,
        }
    }

    /// Converts a Rhai result.
    ///
    /// A string becomes [`Value::Text`]; use
    /// [`from_dynamic_as`](Self::from_dynamic_as) to read one as an enum.
    ///
    /// # Errors
    ///
    /// [`ValueError::Unit`] for `()` and [`ValueError::Unsupported`] for a type
    /// a property cannot hold. It never panics.
    pub fn from_dynamic(dynamic: &Dynamic) -> Result<Self, ValueError> {
        let value = dynamic.flatten_clone();
        if value.is_unit() {
            return Err(ValueError::Unit);
        }
        if let Ok(flag) = value.as_bool() {
            return Ok(Self::Bool(flag));
        }
        if let Ok(number) = value.as_int() {
            return Ok(Self::Int(number));
        }
        if let Ok(number) = value.as_float() {
            return Ok(Self::Float(number));
        }
        match value.into_string() {
            Ok(text) => Ok(Self::Text(text)),
            Err(type_name) => Err(ValueError::Unsupported {
                type_name: type_name.to_owned(),
            }),
        }
    }

    /// Converts a Rhai result for a property whose kind is `kind`.
    ///
    /// Only one widening exists: a string is accepted for an
    /// [`Enum`](ValueKind::Enum) property, because a script has no enum type.
    /// Every other kind must match exactly.
    ///
    /// # Errors
    ///
    /// As [`from_dynamic`](Self::from_dynamic), plus
    /// [`ValueError::KindMismatch`].
    pub fn from_dynamic_as(dynamic: &Dynamic, kind: ValueKind) -> Result<Self, ValueError> {
        match Self::from_dynamic(dynamic)? {
            Self::Text(text) if kind == ValueKind::Enum => Ok(Self::Enum(text)),
            value if value.kind() == kind => Ok(value),
            value => Err(ValueError::KindMismatch {
                expected: kind,
                found: value.kind(),
            }),
        }
    }

    /// The Rhai value for this property value. An enum becomes its name as a
    /// string.
    pub fn to_dynamic(&self) -> Dynamic {
        match self {
            Self::Bool(flag) => Dynamic::from(*flag),
            Self::Int(number) => Dynamic::from(*number),
            Self::Float(number) => Dynamic::from_float(*number),
            Self::Text(text) | Self::Enum(text) => Dynamic::from(text.clone()),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a == b || (a.is_nan() && b.is_nan()),
            (Self::Text(a), Self::Text(b)) | (Self::Enum(a), Self::Enum(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

/// The stable text form used by patch traces: booleans and numbers as written
/// in Rust (a float always shows a decimal point or exponent), text quoted
/// with `\"`, `\\`, `\n`, `\r`, `\t` and `\u{..}` escapes, and an enum as its
/// bare name.
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(flag) => write!(f, "{flag}"),
            Self::Int(number) => write!(f, "{number}"),
            Self::Float(number) => write!(f, "{number:?}"),
            Self::Enum(name) => f.write_str(name),
            Self::Text(text) => {
                f.write_str("\"")?;
                for ch in text.chars() {
                    match ch {
                        '"' => f.write_str("\\\"")?,
                        '\\' => f.write_str("\\\\")?,
                        '\n' => f.write_str("\\n")?,
                        '\r' => f.write_str("\\r")?,
                        '\t' => f.write_str("\\t")?,
                        c if c.is_control() => write!(f, "\\u{{{:x}}}", u32::from(c))?,
                        c => write!(f, "{c}")?,
                    }
                }
                f.write_str("\"")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_and_floats_are_not_coerced() {
        let int = Dynamic::from(1_i64);
        let float = Dynamic::from_float(1.0);
        assert_eq!(Value::from_dynamic(&int), Ok(Value::Int(1)));
        assert_eq!(Value::from_dynamic(&float), Ok(Value::Float(1.0)));
        assert_ne!(Value::Int(1), Value::Float(1.0));
        assert_eq!(
            Value::from_dynamic_as(&float, ValueKind::Int),
            Err(ValueError::KindMismatch {
                expected: ValueKind::Int,
                found: ValueKind::Float
            })
        );
        assert_eq!(
            Value::from_dynamic_as(&int, ValueKind::Float),
            Err(ValueError::KindMismatch {
                expected: ValueKind::Float,
                found: ValueKind::Int
            })
        );
    }

    #[test]
    fn unit_and_unsupported_types_are_errors() {
        assert_eq!(Value::from_dynamic(&Dynamic::UNIT), Err(ValueError::Unit));
        let array = Dynamic::from_array(vec![Dynamic::from(1_i64)]);
        assert!(matches!(
            Value::from_dynamic(&array),
            Err(ValueError::Unsupported { .. })
        ));
        let character = Dynamic::from('x');
        assert!(matches!(
            Value::from_dynamic(&character),
            Err(ValueError::Unsupported { .. })
        ));
        let map = Dynamic::from_map(rhai::Map::new());
        assert!(matches!(
            Value::from_dynamic(&map),
            Err(ValueError::Unsupported { .. })
        ));
    }

    #[test]
    fn a_string_is_text_unless_the_property_is_an_enum() {
        let text = Dynamic::from("Horizontal".to_owned());
        assert_eq!(
            Value::from_dynamic(&text),
            Ok(Value::Text("Horizontal".into()))
        );
        assert_eq!(
            Value::from_dynamic_as(&text, ValueKind::Enum),
            Ok(Value::variant("Horizontal"))
        );
        // An enum property is not a bool property.
        assert!(Value::from_dynamic_as(&Dynamic::from(true), ValueKind::Enum).is_err());
    }

    #[test]
    fn values_round_trip_through_rhai() {
        let values = [
            Value::Bool(true),
            Value::Int(-7),
            Value::Float(2.5),
            Value::Text("hi".into()),
        ];
        for value in values {
            assert_eq!(Value::from_dynamic(&value.to_dynamic()), Ok(value));
        }
        let variant = Value::variant("Start");
        assert_eq!(
            Value::from_dynamic_as(&variant.to_dynamic(), ValueKind::Enum),
            Ok(variant)
        );
    }

    #[test]
    fn nan_equals_itself_so_it_does_not_repatch() {
        assert_eq!(Value::Float(f64::NAN), Value::Float(f64::NAN));
        assert_ne!(Value::Float(f64::NAN), Value::Float(0.0));
    }

    #[test]
    fn display_is_the_trace_form() {
        assert_eq!(Value::Bool(false).to_string(), "false");
        assert_eq!(Value::Int(3).to_string(), "3");
        assert_eq!(Value::Float(1.0).to_string(), "1.0");
        assert_eq!(Value::variant("Start").to_string(), "Start");
        assert_eq!(
            Value::Text("a \"b\"\n\\".into()).to_string(),
            r#""a \"b\"\n\\""#
        );
        assert_eq!(Value::Text("\u{1}".into()).to_string(), r#""\u{1}""#);
    }
}
