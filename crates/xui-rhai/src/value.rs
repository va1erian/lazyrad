#![forbid(unsafe_code)]

//! Converting between [`xui_form::Value`] and [`rhai::Dynamic`].
//!
//! This is the single place the two value systems meet. Reading a form value
//! ([`to_dynamic`]) is driven by the value's own variant; writing one
//! ([`to_value`]) is driven by the catalog's [`ValueType`], so a script string
//! becomes a [`Value::Text`] for a text property and a [`Value::Enum`] for an
//! `enum` one.
//!
//! A colour is written either as a `"#rrggbb"` string or as a `0xRRGGBB`
//! integer (what the stdlib's `rgb(r, g, b)` returns). A method argument
//! ([`to_arg_value`]) is also lenient about numbers, because games compute
//! with floats: a float passed for an int parameter is truncated, and an int
//! passed for a float parameter is widened.

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
        Value::Bytes(bytes) => Dynamic::from_blob(bytes.to_vec()),
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
            if let Some(rgb) = dynamic.clone().try_cast::<rhai::INT>() {
                return u32::try_from(rgb)
                    .ok()
                    .filter(|rgb| *rgb <= 0xFF_FFFF)
                    .map(|rgb| Value::Color(Color::hex(rgb)))
                    .ok_or_else(|| format!("{rgb} is not a 0xRRGGBB colour"));
            }
            let text = dynamic
                .try_cast::<ImmutableString>()
                .ok_or_else(|| mismatch("a `#rrggbb` string or 0xRRGGBB colour", found))?;
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
        ValueType::Bytes => dynamic
            .try_cast::<rhai::Blob>()
            .map(|blob| Value::Bytes(blob.into()))
            .ok_or_else(|| mismatch("a blob (bytes)", found)),
        // `ValueType` is `#[non_exhaustive]`: refuse a type this build cannot
        // decode rather than guessing.
        _ => Err(format!("unsupported property type `{}`", ty.type_name())),
    }
}

/// Converts a Rhai [`Dynamic`] into a method argument of type `ty`.
///
/// This is [`to_value`] plus numeric coercion: a float passed where an int is
/// expected is truncated towards zero (a non-finite float or one out of the
/// int range is an error), and an int passed where a float is expected is
/// widened.
pub fn to_arg_value(dynamic: Dynamic, ty: &ValueType) -> Result<Value, String> {
    if let ValueType::Int { .. } = ty
        && let Some(value) = dynamic.clone().try_cast::<f64>()
    {
        // `i64::MIN as f64` is exact; `i64::MAX as f64` rounds up to 2^63,
        // so the upper bound is exclusive.
        if !value.is_finite() || value < i64::MIN as f64 || value >= i64::MAX as f64 {
            return Err(format!("{value} is not a whole number in range"));
        }
        return Ok(Value::Int(value.trunc() as i64));
    }
    to_value(dynamic, ty)
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
        let bytes = Value::Bytes(vec![1, 2, 3].into());
        assert_eq!(to_value(to_dynamic(&bytes), &ValueType::Bytes), Ok(bytes));
        assert!(to_value(Dynamic::from(1_i64), &ValueType::Bytes).is_err());
        let list = Value::List(vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(to_value(to_dynamic(&list), &ValueType::List), Ok(list));

        let colour = Value::Color(Color::rgb(0x12, 0x34, 0x56));
        assert_eq!(to_value(to_dynamic(&colour), &ValueType::Color), Ok(colour));
    }

    #[test]
    fn a_colour_may_be_an_integer_or_a_string() {
        let expected = Ok(Value::Color(Color::rgb(0x12, 0x34, 0x56)));
        assert_eq!(
            to_value(Dynamic::from(0x12_3456_i64), &ValueType::Color),
            expected
        );
        assert_eq!(
            to_value(Dynamic::from("#123456".to_owned()), &ValueType::Color),
            expected
        );
        assert!(to_value(Dynamic::from(-1_i64), &ValueType::Color).is_err());
        assert!(to_value(Dynamic::from(0x100_0000_i64), &ValueType::Color).is_err());
        assert!(to_value(Dynamic::from(true), &ValueType::Color).is_err());
    }

    #[test]
    fn method_arguments_coerce_between_int_and_float() {
        let int = ValueType::Int {
            min: None,
            max: None,
        };
        let float = ValueType::Float {
            min: None,
            max: None,
        };
        assert_eq!(
            to_arg_value(Dynamic::from(2.9_f64), &int),
            Ok(Value::Int(2))
        );
        assert_eq!(
            to_arg_value(Dynamic::from(-2.9_f64), &int),
            Ok(Value::Int(-2))
        );
        assert!(to_arg_value(Dynamic::from(f64::NAN), &int).is_err());
        assert!(to_arg_value(Dynamic::from(1e30_f64), &int).is_err());
        assert_eq!(
            to_arg_value(Dynamic::from(3_i64), &float),
            Ok(Value::Float(3.0))
        );
        assert_eq!(
            to_arg_value(Dynamic::from(1.5_f64), &float),
            Ok(Value::Float(1.5))
        );
        // A property write stays strict about ints.
        assert!(to_value(Dynamic::from(2.5_f64), &int).is_err());
    }
}
