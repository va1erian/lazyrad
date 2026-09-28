#![forbid(unsafe_code)]

//! Converting between [`xui_form::Value`] and [`rhai::Dynamic`].
//!
//! This is the single place the two value systems meet. Reading a form value
//! ([`to_dynamic`]) is driven by the value's own variant; writing one
//! ([`to_value`]) is driven by the catalog's [`ValueType`], so a script string
//! becomes a [`Value::Text`] for a text property and a [`Value::Enum`] for an
//! `enum` one.

use rhai::{Array, Dynamic, ImmutableString};
use xui_core::Color;
use xui_form::{Value, ValueType};

/// Converts a form [`Value`] into a Rhai [`Dynamic`].
pub fn to_dynamic(value: &Value) -> Dynamic {
    match value {
        Value::Bool(value) => Dynamic::from(*value),
        Value::Int(value) => Dynamic::from(*value),
        Value::Float(value) => Dynamic::from(*value),
        Value::Text(value) | Value::Enum(value) => Dynamic::from(value.clone()),
        Value::Color(color) => Dynamic::from(format_color(*color)),
        Value::List(items) => Dynamic::from(
            items
                .iter()
                .map(|item| Dynamic::from(item.clone()))
                .collect::<Array>(),
        ),
        // `Value` is `#[non_exhaustive]`; an unknown future variant is exposed
        // as unit rather than failing the read.
        _ => Dynamic::UNIT,
    }
}

/// Converts a Rhai [`Dynamic`] into a form [`Value`] according to `ty`.
///
/// An integer is accepted where a float is expected, matching the document
/// decoder. A wrong type is an error message describing the mismatch.
pub fn to_value(dynamic: Dynamic, ty: &ValueType) -> Result<Value, String> {
    let found = dynamic.type_name();
    match ty {
        ValueType::Bool => dynamic
            .try_cast::<bool>()
            .map(Value::Bool)
            .ok_or_else(|| mismatch("bool", found)),
        ValueType::Int { .. } => dynamic
            .try_cast::<rhai::INT>()
            .map(Value::Int)
            .ok_or_else(|| mismatch("int", found)),
        ValueType::Float { .. } => {
            if let Some(value) = dynamic.clone().try_cast::<f64>() {
                Ok(Value::Float(value))
            } else if let Some(value) = dynamic.try_cast::<rhai::INT>() {
                Ok(Value::Float(value as f64))
            } else {
                Err(mismatch("float", found))
            }
        }
        ValueType::Text { .. } => dynamic
            .try_cast::<ImmutableString>()
            .map(|value| Value::Text(value.to_string()))
            .ok_or_else(|| mismatch("text", found)),
        ValueType::Enum { .. } => dynamic
            .try_cast::<ImmutableString>()
            .map(|value| Value::Enum(value.to_string()))
            .ok_or_else(|| mismatch("enum name", found)),
        ValueType::Color => {
            let text = dynamic
                .try_cast::<ImmutableString>()
                .ok_or_else(|| mismatch("a `#rrggbb` colour", found))?;
            parse_color(&text)
                .map(Value::Color)
                .ok_or_else(|| format!("`{text}` is not a `#rrggbb` colour"))
        }
        ValueType::List => {
            let items = dynamic
                .try_cast::<Array>()
                .ok_or_else(|| mismatch("a list", found))?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let item_found = item.type_name();
                let text = item
                    .try_cast::<ImmutableString>()
                    .ok_or_else(|| mismatch("a list of strings", item_found))?;
                out.push(text.to_string());
            }
            Ok(Value::List(out))
        }
        // `ValueType` is `#[non_exhaustive]`: refuse a type this build cannot
        // decode rather than guessing.
        _ => Err(format!("unsupported property type `{}`", ty.type_name())),
    }
}

/// Builds a type-mismatch message.
fn mismatch(expected: &str, found: &str) -> String {
    format!("expected {expected}, found {found}")
}

/// Formats an opaque colour as `#rrggbb`.
fn format_color(color: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

/// Parses `#rrggbb` or `#rrggbbaa` (the leading `#` is optional), dropping any
/// alpha channel because [`Color`] is opaque.
fn parse_color(text: &str) -> Option<Color> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(if hex.len() == 6 {
        Color::hex(value)
    } else {
        Color::hex(value >> 8)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_round_trip() {
        for value in [
            Value::Bool(true),
            Value::Int(-3),
            Value::Float(1.5),
            Value::Text("hi".to_owned()),
            Value::Enum("top_left".to_owned()),
        ] {
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
                _ => unreachable!(),
            };
            let dynamic = to_dynamic(&value);
            assert_eq!(to_value(dynamic, &ty), Ok(value));
        }
    }

    #[test]
    fn an_int_is_accepted_where_a_float_is_expected() {
        let dynamic = Dynamic::from(3_i64);
        assert_eq!(
            to_value(
                dynamic,
                &ValueType::Float {
                    min: None,
                    max: None
                }
            ),
            Ok(Value::Float(3.0))
        );
    }

    #[test]
    fn a_string_becomes_text_or_enum_by_schema() {
        let dynamic = Dynamic::from("fill".to_owned());
        assert_eq!(
            to_value(dynamic.clone(), &ValueType::Text { multiline: false }),
            Ok(Value::Text("fill".to_owned()))
        );
        assert_eq!(
            to_value(
                dynamic,
                &ValueType::Enum {
                    variants: Vec::new()
                }
            ),
            Ok(Value::Enum("fill".to_owned()))
        );
    }

    #[test]
    fn a_wrong_type_is_an_error() {
        let dynamic = Dynamic::from(true);
        let error = to_value(
            dynamic,
            &ValueType::Int {
                min: None,
                max: None,
            },
        )
        .expect_err("a bool is not an int");
        assert!(error.contains("expected int"));
    }

    #[test]
    fn lists_and_colours_convert() {
        let list = Value::List(vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(to_value(to_dynamic(&list), &ValueType::List), Ok(list));

        let colour = Value::Color(Color::rgb(0x12, 0x34, 0x56));
        assert_eq!(to_value(to_dynamic(&colour), &ValueType::Color), Ok(colour));
    }
}
