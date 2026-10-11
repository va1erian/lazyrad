#![forbid(unsafe_code)]

//! What a stop reports: the call stack, the locals and how a value is shown.
//!
//! Split from the session so that file holds the control flow (what stops and
//! when) and this one holds the data (what a stop looks like). Everything is
//! plain functions over Rhai's evaluation context.

use std::collections::BTreeMap;

use lazyrad_debug_proto::{Frame, Stop, StopReason, Variable, normalize_file};
use rhai::{Dynamic, Engine, EvalAltResult, EvalContext, Position};
use xui_rhai::control::is_host_handle;

use super::TOP_LEVEL;

/// The most locals one stop reports.
pub(super) const MAX_LOCALS: usize = 64;
/// The most stack frames one stop reports.
pub(super) const MAX_FRAMES: usize = 48;
/// The longest rendered value, in characters.
pub(super) const MAX_VALUE_CHARS: usize = 120;
/// The longest variable or function name reported, in characters.
pub(super) const MAX_NAME_CHARS: usize = 48;
/// The most elements or keys of a container shown in a rendered value.
pub(super) const MAX_PREVIEW_ITEMS: usize = 8;

/// One remembered local: scalars and shared handles are kept as the value,
/// containers as their length (cloning an array on every line would make a hot
/// loop quadratic).
#[derive(Debug)]
pub(super) enum HeldValue {
    Value(Dynamic),
    Array(usize),
    Map(usize),
}

/// The innermost function's locals as of the last line change.
///
/// Rhai rewinds a function's scope *before* it reports the function's exit, so
/// the locals an error stop wants are gone by then. The callback therefore
/// notes them once per line visit, reusing its buffers, and an exception stop
/// reports this note instead of the (by then empty) live scope.
#[derive(Debug, Default)]
pub(super) struct Held {
    pub(super) depth: usize,
    pub(super) items: Vec<(String, HeldValue)>,
}

impl Held {
    pub(super) fn remember(&mut self, scope: &rhai::Scope<'_>, depth: usize) {
        self.depth = depth;
        let mut count = 0;
        for (name, _constant, value) in scope.iter_raw() {
            if name == "app" || (!value.is_shared() && is_host_handle(value)) {
                continue;
            }
            if count >= MAX_LOCALS * 4 {
                break;
            }
            let held = if value.is_array() {
                HeldValue::Array(
                    value
                        .read_lock::<rhai::Array>()
                        .map_or(0, |array| array.len()),
                )
            } else if value.is_map() {
                HeldValue::Map(value.read_lock::<rhai::Map>().map_or(0, |map| map.len()))
            } else {
                HeldValue::Value(value.clone())
            };
            match self.items.get_mut(count) {
                Some(entry) => {
                    entry.0.clear();
                    entry.0.push_str(name);
                    entry.1 = held;
                }
                None => self.items.push((name.to_owned(), held)),
            }
            count += 1;
        }
        self.items.truncate(count);
    }
}

/// A local as the report will show it.
enum Shown<'a> {
    Live(&'a Dynamic),
    Array(usize),
    Map(usize),
}

/// The report for a stop: location, call stack and locals. `held` replaces the
/// live scope with the remembered locals (an error stop, see [`Held`]).
pub(super) fn snapshot(
    context: &EvalContext<'_, '_, '_, '_, '_, '_>,
    reason: StopReason,
    file: &str,
    line: usize,
    message: Option<String>,
    held: Option<&Held>,
) -> Stop {
    let engine = context.engine();
    let file = normalize_file(file);
    let mut truncated = false;

    let mut stack = Vec::new();
    match context.debugger() {
        Some(debugger) if !debugger.call_stack().is_empty() => {
            let frames = debugger.call_stack();
            let count = frames.len();
            // Innermost: the function at the stop position.
            stack.push(Frame {
                function: clip(frames[count - 1].fn_name.as_str(), MAX_NAME_CHARS),
                file: file.clone(),
                line: to_line(line),
            });
            // Each caller is stopped where it called the frame above it: that
            // call's position, in the file the caller is defined in.
            for index in (0..count - 1).rev() {
                stack.push(Frame {
                    function: clip(frames[index].fn_name.as_str(), MAX_NAME_CHARS),
                    file: frames[index]
                        .source
                        .as_deref()
                        .map(normalize_file)
                        .unwrap_or_default(),
                    line: frames[index + 1].pos.line().map_or(0, to_line),
                });
                if stack.len() >= MAX_FRAMES {
                    truncated = index > 0;
                    break;
                }
            }
        }
        _ => stack.push(Frame {
            function: TOP_LEVEL.to_owned(),
            file: file.clone(),
            line: to_line(line),
        }),
    }

    // Rhai lists a scope newest first. The *newest* declaration of a name is the
    // visible one (it shadows the older), and the pane reads in declaration
    // order, so keep the first sighting and reverse at the end.
    let mut order: Vec<&str> = Vec::new();
    let mut by_name: BTreeMap<&str, Shown<'_>> = BTreeMap::new();
    match held {
        Some(held) => {
            for (name, value) in &held.items {
                if by_name.contains_key(name.as_str()) {
                    continue;
                }
                order.push(name);
                by_name.insert(
                    name,
                    match value {
                        HeldValue::Value(value) => Shown::Live(value),
                        HeldValue::Array(len) => Shown::Array(*len),
                        HeldValue::Map(len) => Shown::Map(*len),
                    },
                );
            }
        }
        None => {
            for (name, _constant, value) in context.scope().iter_raw().take(MAX_LOCALS * 8) {
                if by_name.contains_key(name) {
                    continue;
                }
                order.push(name);
                by_name.insert(name, Shown::Live(value));
            }
        }
    }
    order.reverse();
    let mut locals = Vec::new();
    for name in order {
        let Some(shown) = by_name.get(name) else {
            continue;
        };
        let (ty, text) = match shown {
            Shown::Live(value) => {
                let value = if value.is_shared() {
                    value.flatten_clone()
                } else {
                    (*value).clone()
                };
                if is_host_handle(&value) || name == "app" {
                    continue;
                }
                describe(engine, &value)
            }
            Shown::Array(len) => ("array".to_owned(), format!("[…{len} items]")),
            Shown::Map(len) => ("map".to_owned(), format!("#{{…{len} keys}}")),
        };
        if locals.len() >= MAX_LOCALS {
            truncated = true;
            break;
        }
        locals.push(Variable {
            name: clip(name, MAX_NAME_CHARS),
            ty,
            value: text,
        });
    }

    Stop {
        reason,
        file,
        line: to_line(line),
        message,
        stack,
        locals,
        truncated,
    }
}

/// The position of the error at the bottom of Rhai's call wrappers.
pub(super) fn innermost_position(error: &EvalAltResult) -> Option<Position> {
    let mut current = error;
    while let EvalAltResult::ErrorInFunctionCall(_, _, inner, _) = current {
        current = inner;
    }
    let position = current.position();
    (!position.is_none()).then_some(position)
}

/// A one-based line as the protocol's `u32`.
pub(super) fn to_line(line: usize) -> u32 {
    u32::try_from(line).unwrap_or(u32::MAX)
}

/// `text` cut to `max` characters, with an ellipsis when it was cut.
pub(super) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

/// A value's type name and a short rendering of it.
pub(super) fn describe(engine: &Engine, value: &Dynamic) -> (String, String) {
    let ty = engine.map_type_name(value.type_name()).to_owned();
    (ty, clip(&render(value, 0), MAX_VALUE_CHARS))
}

/// A value as text: strings quoted, containers summarised.
fn render(value: &Dynamic, depth: usize) -> String {
    if value.is_unit() {
        return "()".to_owned();
    }
    if value.is_string() {
        let shown = value.clone().flatten().into_immutable_string();
        return match shown {
            Ok(text) => format!("{:?}", text.as_str()),
            Err(_) => value.to_string(),
        };
    }
    if let Some(array) = value.read_lock::<rhai::Array>() {
        if depth > 0 {
            return format!("[…{}]", array.len());
        }
        let mut parts: Vec<String> = array
            .iter()
            .take(MAX_PREVIEW_ITEMS)
            .map(|item| render(item, depth + 1))
            .collect();
        if array.len() > MAX_PREVIEW_ITEMS {
            parts.push(format!("… {} items", array.len()));
        }
        return format!("[{}]", parts.join(", "));
    }
    if let Some(map) = value.read_lock::<rhai::Map>() {
        if depth > 0 {
            return format!("#{{…{}}}", map.len());
        }
        let mut parts: Vec<String> = map
            .iter()
            .take(MAX_PREVIEW_ITEMS)
            .map(|(key, item)| format!("{key}: {}", render(item, depth + 1)))
            .collect();
        if map.len() > MAX_PREVIEW_ITEMS {
            parts.push(format!("… {} keys", map.len()));
        }
        return format!("#{{{}}}", parts.join(", "));
    }
    value.to_string()
}
