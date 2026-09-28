#![forbid(unsafe_code)]

//! The Iteration 1 standard library.
//!
//! This crate's whole public surface for scripts lives here. The functions use
//! VB spellings (`Left`, `Mid`, `MsgBox`, …) so a VB6 programmer recognises
//! them, and they are registered as native Rhai functions, globals and a
//! `App`/`Debug` object. PLAN.md §4.4 lists the intended eventual modules; this
//! is the small core Iteration 1 needs.
//!
//! # Doc comments
//!
//! Every function is documented twice: a Rust `///` comment for readers of this
//! source, and a Rhai doc-comment attached through [`rhai::FuncRegistration`].
//! The `metadata` feature turns the latter into the completion and signature
//! data the editor reads (PLAN.md §5), so completion has real help text from
//! the first milestone.
//!
//! # Non-blocking `MsgBox`
//!
//! [`MsgBox`] is asynchronous in Iteration 1. It records a
//! [`Msg::MsgBox`](crate::message::Msg::MsgBox) on the host's pending queue and
//! returns immediately; the application opens the in-window dialog and later
//! calls the optional callback with the VB result code. A blocking version
//! needs `Ui::open_modal`, which the canvas backend does not implement yet
//! (PLAN.md §10, G14; va1erian/xui#146).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use rhai::{Array, Dynamic, Engine, FnPtr, ImmutableString};
use time::OffsetDateTime;

use crate::engine::EngineHost;
use crate::message::{Msg, MsgBoxButtons, Pending, VB_CANCEL, VB_NO, VB_OK, VB_YES};

/// The host state the standard library needs from the running application.
///
/// It is assembled by [`crate::form::FormInstance`] when it builds a form, so
/// the script's `App` object and its message boxes speak for that form.
#[derive(Clone)]
pub struct StdlibContext {
    /// The form whose script is being run.
    pub form: String,
    /// The queue `MsgBox` and `App.quit` write to.
    pub pending: Pending,
    /// The value `App.title` reports.
    pub app_title: String,
    /// The value `App.path` reports.
    pub app_path: String,
}

impl StdlibContext {
    /// A context with nothing attached, for a syntax check or a unit test that
    /// does not run a real form.
    pub fn headless(form: impl Into<String>) -> StdlibContext {
        StdlibContext {
            form: form.into(),
            pending: Rc::new(RefCell::new(Vec::new())),
            app_title: "LazyRAD".to_owned(),
            app_path: String::new(),
        }
    }
}

/// The VB `MsgBox` button and result constants, exposed as script globals.
///
/// They keep a callback readable: `MsgBox("Save?", "", vbYesNo, |r| { ... })`
/// and `r == vbYes`.
pub const CONSTANTS: &[(&str, i64)] = &[
    ("vbOK", VB_OK),
    ("vbCancel", VB_CANCEL),
    ("vbYes", VB_YES),
    ("vbNo", VB_NO),
    ("vbOKOnly", 0),
    ("vbOKCancel", 1),
    ("vbYesNo", 4),
];

/// Registers the whole standard library on `host`.
///
/// This is the single place the stdlib is installed: [`EngineHost::new`] calls
/// it, so every engine — the player's, the designer preview's, the syntax
/// checker's — sees the same functions and globals.
pub fn register(host: &mut EngineHost, context: &StdlibContext) {
    let engine = host.engine_mut();
    register_strings(engine);
    register_math(engine);
    register_time(engine);
    register_debug(engine);
    register_app(engine);
    register_msg_box(engine, context);

    host.set_global("Debug", Dynamic::from(Debug));
    host.set_global("App", Dynamic::from(app_object(context)));
    for (name, value) in CONSTANTS {
        host.set_global(*name, Dynamic::from(*value));
    }
}

/// Registers a global native function with its Rhai parameter names and doc
/// comment, so `metadata` can describe it.
///
/// The Rust functions carry the same documentation; this macro is what makes it
/// visible to the engine. The last `params` entry would be the return type if
/// one were given, but the closures below return plain values and Rhai infers
/// those.
macro_rules! documented_fn {
    ($engine:expr, $name:literal, [$($param:literal),*], [$($comment:literal),*], $func:expr) => {{
        rhai::FuncRegistration::new($name)
            .with_params_info::<&str>([$($param),*])
            .with_comments::<&str>([$($comment),*])
            .register_into_engine($engine, $func);
    }};
}

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

/// Registers the VB string helpers.
fn register_strings(engine: &mut Engine) {
    documented_fn!(
        engine,
        "Left",
        ["text: &str", "count: i64"],
        ["Returns the first `count` characters of `text`."],
        |text: ImmutableString, count: i64| left(&text, count)
    );
    documented_fn!(
        engine,
        "Right",
        ["text: &str", "count: i64"],
        ["Returns the last `count` characters of `text`."],
        |text: ImmutableString, count: i64| right(&text, count)
    );
    documented_fn!(
        engine,
        "Mid",
        ["text: &str", "start: i64"],
        ["Returns `text` from the 1-based character `start` to its end."],
        |text: ImmutableString, start: i64| mid(&text, start, None)
    );
    documented_fn!(
        engine,
        "Mid",
        ["text: &str", "start: i64", "length: i64"],
        ["Returns `length` characters of `text` from the 1-based `start`."],
        |text: ImmutableString, start: i64, length: i64| mid(&text, start, Some(length))
    );
    documented_fn!(
        engine,
        "Len",
        ["text: &str"],
        ["Returns the number of characters in `text`."],
        |text: ImmutableString| char_len(&text)
    );
    documented_fn!(
        engine,
        "Trim",
        ["text: &str"],
        ["Removes leading and trailing whitespace."],
        |text: ImmutableString| text.trim().to_owned()
    );
    documented_fn!(
        engine,
        "LTrim",
        ["text: &str"],
        ["Removes leading whitespace."],
        |text: ImmutableString| text.trim_start().to_owned()
    );
    documented_fn!(
        engine,
        "RTrim",
        ["text: &str"],
        ["Removes trailing whitespace."],
        |text: ImmutableString| text.trim_end().to_owned()
    );
    documented_fn!(
        engine,
        "UCase",
        ["text: &str"],
        ["Returns `text` in upper case."],
        |text: ImmutableString| text.to_uppercase()
    );
    documented_fn!(
        engine,
        "LCase",
        ["text: &str"],
        ["Returns `text` in lower case."],
        |text: ImmutableString| text.to_lowercase()
    );
    documented_fn!(
        engine,
        "InStr",
        ["text: &str", "needle: &str"],
        ["Returns the 1-based position of `needle` in `text`, or 0."],
        |text: ImmutableString, needle: ImmutableString| in_str(&text, &needle, 1)
    );
    documented_fn!(
        engine,
        "InStr",
        ["start: i64", "text: &str", "needle: &str"],
        ["Returns the 1-based position of `needle` in `text` at or after `start`."],
        |start: i64, text: ImmutableString, needle: ImmutableString| in_str(&text, &needle, start)
    );
    documented_fn!(
        engine,
        "Replace",
        ["text: &str", "find: &str", "with: &str"],
        ["Replaces every occurrence of `find` in `text` with `with`."],
        |text: ImmutableString, find: ImmutableString, with: ImmutableString| text
            .replace(&*find, &with)
    );
    documented_fn!(
        engine,
        "Split",
        ["text: &str", "delimiter: &str"],
        ["Splits `text` on `delimiter` into an array of strings."],
        |text: ImmutableString, delimiter: ImmutableString| split(&text, &delimiter)
    );
    documented_fn!(
        engine,
        "Join",
        ["items: array", "delimiter: &str"],
        ["Joins an array of values into a string separated by `delimiter`."],
        |items: Array, delimiter: ImmutableString| join(items, &delimiter)
    );
    documented_fn!(
        engine,
        "Val",
        ["text: &str"],
        ["Parses the leading number in `text`; stops at the first other character."],
        |text: ImmutableString| val(&text)
    );
    documented_fn!(
        engine,
        "Str",
        ["number"],
        ["Converts a number to a string; a non-negative number keeps a leading space."],
        |value: Dynamic| str_number(&value)
    );
    documented_fn!(
        engine,
        "Format",
        ["number", "pattern: &str"],
        ["Formats a number with a pattern of `0`, `#`, `.`, `,` and `%`."],
        |value: Dynamic, pattern: ImmutableString| script_result(format_number(&value, &pattern))
    );
}

/// Turns a standard-library error message into a Rhai runtime error.
///
/// A native function returning `Result<T, String>` would otherwise be exposed
/// to the script as a `Result` value. Rhai recognises only
/// `Result<T, Box<EvalAltResult>>` as fallible, so every fallible helper goes
/// through here.
fn script_result<T>(result: Result<T, String>) -> Result<T, Box<rhai::EvalAltResult>> {
    result.map_err(crate::control::runtime_error)
}

/// The number of characters in `text`.
fn char_len(text: &str) -> i64 {
    text.chars().count() as i64
}

/// The first `count` characters of `text`. A negative count yields empty.
fn left(text: &str, count: i64) -> String {
    text.chars().take(count.max(0) as usize).collect()
}

/// The last `count` characters of `text`. A negative count yields empty.
fn right(text: &str, count: i64) -> String {
    let total = text.chars().count();
    let skip = total.saturating_sub(count.max(0) as usize);
    text.chars().skip(skip).collect()
}

/// `length` characters of `text` from the 1-based `start`, or to the end when
/// `length` is `None`. A start below 1 is clamped to 1.
fn mid(text: &str, start: i64, length: Option<i64>) -> String {
    let skip = (start.max(1) - 1) as usize;
    let chars: Vec<char> = text.chars().skip(skip).collect();
    let take = length.map_or(chars.len(), |length| length.max(0) as usize);
    chars.into_iter().take(take).collect()
}

/// The 1-based character position of `needle` in `text` at or after the 1-based
/// `start`, or 0 when it is absent.
fn in_str(text: &str, needle: &str, start: i64) -> i64 {
    let skip = (start.max(1) - 1) as usize;
    let offset = text
        .char_indices()
        .nth(skip)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    match text[offset..].find(needle) {
        Some(byte) => (text[..offset + byte].chars().count() + 1) as i64,
        None => 0,
    }
}

/// Splits `text` into a Rhai array of strings. As in VB, an empty delimiter
/// does not split: the result is the whole string as a single element.
fn split(text: &str, delimiter: &str) -> Array {
    if delimiter.is_empty() {
        return vec![Dynamic::from(text.to_string())];
    }
    text.split(delimiter)
        .map(|part| Dynamic::from(part.to_string()))
        .collect()
}

/// Joins an array of values with `delimiter`, converting each value with its
/// Rhai string form.
fn join(items: Array, delimiter: &str) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(delimiter)
}

/// Parses the leading decimal number in `text`, the way VB `Val` does.
///
/// Leading whitespace and an optional sign are accepted; parsing stops at the
/// first character that cannot continue the number. A string with no leading
/// digit is 0.
fn val(text: &str) -> f64 {
    let trimmed = text.trim_start();
    let (sign, rest) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let mut dot = false;
    let mut end = 0;
    for (index, character) in rest.char_indices() {
        if character.is_ascii_digit() {
            end = index + character.len_utf8();
        } else if character == '.' && !dot {
            dot = true;
            end = index + character.len_utf8();
        } else {
            break;
        }
    }
    rest[..end].parse::<f64>().map_or(0.0, |value| value * sign)
}

/// VB `Str`: a non-negative number gets a leading space to reserve the sign
/// column; a negative number keeps its `-`.
fn str_number(value: &Dynamic) -> String {
    if let Some(integer) = value.clone().try_cast::<i64>() {
        return if integer < 0 {
            integer.to_string()
        } else {
            format!(" {integer}")
        };
    }
    if let Some(float) = value.clone().try_cast::<f64>() {
        return if float.is_sign_negative() {
            float.to_string()
        } else {
            format!(" {float}")
        };
    }
    value.to_string()
}

/// Formats a number with a VB-style numeric pattern.
///
/// Only numbers are handled (the date patterns are deferred with the
/// `DateTime` type). The supported pattern characters are `0` (a required
/// digit), `#` (an optional digit), `.` (the decimal point), `,` (thousands
/// grouping) and `%` (multiply by 100 and append a percent sign); other
/// characters are ignored. The number of digits after `.` sets the rounding,
/// and the number of `0`s before it sets the minimum integer width.
fn format_number(value: &Dynamic, pattern: &str) -> Result<String, String> {
    let number = if let Some(integer) = value.clone().try_cast::<i64>() {
        integer as f64
    } else if let Some(float) = value.clone().try_cast::<f64>() {
        float
    } else {
        return Err("Format expects a number".to_owned());
    };

    let (body, percent) = match pattern.strip_suffix('%') {
        Some(body) => (body, true),
        None => (pattern, false),
    };
    let decimals = match body.find('.') {
        Some(dot) => body[dot + 1..]
            .chars()
            .filter(|character| *character == '0' || *character == '#')
            .count(),
        None => 0,
    };
    let grouping = body.contains(',');
    let min_integer = body
        .split('.')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|character| *character == '0')
        .count();

    let scaled = if percent { number * 100.0 } else { number };
    let mut text = format!("{scaled:.decimals$}");
    let negative = text.starts_with('-');
    if negative {
        text.remove(0);
    }
    let (mut integer, fraction) = match text.split_once('.') {
        Some((integer, fraction)) => (integer.to_owned(), Some(fraction.to_owned())),
        None => (text, None),
    };
    while integer.len() < min_integer {
        integer.insert(0, '0');
    }
    if grouping {
        integer = group_digits(&integer);
    }

    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.push_str(&integer);
    if let Some(fraction) = fraction {
        out.push('.');
        out.push_str(&fraction);
    }
    if percent {
        out.push('%');
    }
    Ok(out)
}

/// Inserts a comma every three digits from the right.
fn group_digits(digits: &str) -> String {
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    let length = digits.len();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (length - index) % 3 == 0 {
            out.push(',');
        }
        out.push(character);
    }
    out
}

// ---------------------------------------------------------------------------
// Math
// ---------------------------------------------------------------------------

/// Registers the VB math helpers and the random-number functions.
fn register_math(engine: &mut Engine) {
    documented_fn!(
        engine,
        "Abs",
        ["number"],
        ["Returns the absolute value of a number."],
        |value: Dynamic| script_result(abs_value(value))
    );
    documented_fn!(
        engine,
        "Int",
        ["number"],
        ["Returns the largest integer less than or equal to a number."],
        |value: Dynamic| script_result(int_value(value))
    );
    documented_fn!(
        engine,
        "Round",
        ["number"],
        ["Rounds a number to the nearest whole number, halves to even."],
        |value: Dynamic| script_result(round_value(value, 0))
    );
    documented_fn!(
        engine,
        "Round",
        ["number", "digits: i64"],
        ["Rounds a number to `digits` decimal places, halves to even."],
        |value: Dynamic, digits: i64| script_result(round_value(value, digits))
    );
    documented_fn!(
        engine,
        "Sqr",
        ["number"],
        ["Returns the square root of a non-negative number."],
        |value: Dynamic| script_result(sqr_value(value))
    );
    documented_fn!(
        engine,
        "Min",
        ["a", "b"],
        ["Returns the smaller of two numbers."],
        |a: Dynamic, b: Dynamic| script_result(min_value(a, b))
    );
    documented_fn!(
        engine,
        "Max",
        ["a", "b"],
        ["Returns the larger of two numbers."],
        |a: Dynamic, b: Dynamic| script_result(max_value(a, b))
    );

    let state = Rc::new(Cell::new(seed()));
    let state_for_rnd = Rc::clone(&state);
    documented_fn!(
        engine,
        "Rnd",
        [],
        ["Returns a pseudo-random number in the half-open range 0.0 to 1.0."],
        move || next_random(&state_for_rnd)
    );
    let state_for_seed = Rc::clone(&state);
    documented_fn!(
        engine,
        "Randomize",
        [],
        ["Re-seeds the random-number generator from the clock."],
        move || state_for_seed.set(seed())
    );
    documented_fn!(
        engine,
        "Randomize",
        ["seed: i64"],
        ["Re-seeds the random-number generator with `seed`."],
        move |value: i64| state.set(value as u64 | 1)
    );
}

/// The absolute value of an integer or float, keeping its type.
fn abs_value(value: Dynamic) -> Result<Dynamic, String> {
    if let Some(integer) = value.clone().try_cast::<i64>() {
        return Ok(Dynamic::from(integer.saturating_abs()));
    }
    if let Some(float) = value.try_cast::<f64>() {
        return Ok(Dynamic::from(float.abs()));
    }
    Err("Abs expects a number".to_owned())
}

/// VB `Int`: the floor of a number, as an integer.
fn int_value(value: Dynamic) -> Result<Dynamic, String> {
    if let Some(integer) = value.clone().try_cast::<i64>() {
        return Ok(Dynamic::from(integer));
    }
    if let Some(float) = value.try_cast::<f64>() {
        return Ok(Dynamic::from(float.floor() as i64));
    }
    Err("Int expects a number".to_owned())
}

/// VB `Round`: half-to-even rounding to `digits` decimal places.
///
/// An integer is returned unchanged when `digits` is non-negative, so
/// `Round(3)` stays an integer.
fn round_value(value: Dynamic, digits: i64) -> Result<Dynamic, String> {
    if let Some(integer) = value.clone().try_cast::<i64>() {
        if digits >= 0 {
            return Ok(Dynamic::from(integer));
        }
    }
    let number = if let Some(integer) = value.clone().try_cast::<i64>() {
        integer as f64
    } else if let Some(float) = value.try_cast::<f64>() {
        float
    } else {
        return Err("Round expects a number".to_owned());
    };
    let factor = 10f64.powi(digits.clamp(-308, 308) as i32);
    Ok(Dynamic::from(banker_round(number * factor) / factor))
}

/// Rounds to the nearest whole number, with halves going to the even neighbour.
fn banker_round(number: f64) -> f64 {
    let floor = number.floor();
    let fraction = number - floor;
    if fraction < 0.5 {
        floor
    } else if fraction > 0.5 {
        floor + 1.0
    } else if (floor as i64) % 2 == 0 {
        floor
    } else {
        floor + 1.0
    }
}

/// The square root of a number; a negative argument is an error.
fn sqr_value(value: Dynamic) -> Result<Dynamic, String> {
    let number = as_number(&value).ok_or_else(|| "Sqr expects a number".to_owned())?;
    if number < 0.0 {
        return Err("Sqr expects a non-negative number".to_owned());
    }
    Ok(Dynamic::from(number.sqrt()))
}

/// The smaller of two numbers, integer when both are integers.
fn min_value(a: Dynamic, b: Dynamic) -> Result<Dynamic, String> {
    compare(a, b, |x, y| x < y, "Min")
}

/// The larger of two numbers, integer when both are integers.
fn max_value(a: Dynamic, b: Dynamic) -> Result<Dynamic, String> {
    compare(a, b, |x, y| x > y, "Max")
}

/// Picks one of two numbers with `wins`, preserving integer type when both
/// arguments are integers.
fn compare(
    a: Dynamic,
    b: Dynamic,
    wins: impl Fn(f64, f64) -> bool,
    name: &str,
) -> Result<Dynamic, String> {
    if let (Some(x), Some(y)) = (a.clone().try_cast::<i64>(), b.clone().try_cast::<i64>()) {
        return Ok(Dynamic::from(if wins(x as f64, y as f64) { x } else { y }));
    }
    let x = as_number(&a).ok_or_else(|| format!("{name} expects numbers"))?;
    let y = as_number(&b).ok_or_else(|| format!("{name} expects numbers"))?;
    Ok(Dynamic::from(if wins(x, y) { x } else { y }))
}

/// A dynamic value as an `f64`, if it is a number.
fn as_number(value: &Dynamic) -> Option<f64> {
    if let Some(integer) = value.clone().try_cast::<i64>() {
        return Some(integer as f64);
    }
    value.clone().try_cast::<f64>()
}

/// A seed for the random generator, from the clock.
fn seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1
}

/// The next value of an xorshift64* generator, in `[0, 1)`.
fn next_random(state: &Cell<u64>) -> f64 {
    let mut value = state.get();
    value ^= value >> 12;
    value ^= value << 25;
    value ^= value >> 27;
    state.set(value);
    let result = value.wrapping_mul(0x2545_F491_4F6C_DD1D);
    (result >> 11) as f64 / (1u64 << 53) as f64
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// Registers `Now`, `Date` and `Time`.
fn register_time(engine: &mut Engine) {
    documented_fn!(
        engine,
        "Now",
        [],
        ["Returns the current local date and time as an ISO-8601 string."],
        || format_now(local_now())
    );
    documented_fn!(
        engine,
        "Date",
        [],
        ["Returns the current local date as `YYYY-MM-DD`."],
        || format_date(local_now())
    );
    documented_fn!(
        engine,
        "Time",
        [],
        ["Returns the current local time as `HH:MM:SS`."],
        || format_time(local_now())
    );
}

/// The current time in the local zone, falling back to UTC when the offset is
/// not available (a sandbox or a platform without a time zone database).
fn local_now() -> OffsetDateTime {
    OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc())
}

/// Formats a date and time as `YYYY-MM-DD HH:MM:SS`.
fn format_now(value: OffsetDateTime) -> String {
    format!("{} {}", format_date(value), format_time(value))
}

/// Formats a date as `YYYY-MM-DD`.
fn format_date(value: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        value.year(),
        u8::from(value.month()),
        value.day()
    )
}

/// Formats a time as `HH:MM:SS`.
fn format_time(value: OffsetDateTime) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        value.hour(),
        value.minute(),
        value.second()
    )
}

// ---------------------------------------------------------------------------
// Debug
// ---------------------------------------------------------------------------

/// VB `Debug`: the script's output channel.
///
/// `Debug.print` writes one line to standard output and flushes it, so the
/// IDE's output pane (reading the player's stdout) sees each call immediately.
/// A drawn output pane and a structured protocol are later milestones, but the
/// line format is already what they consume.
#[derive(Clone)]
pub struct Debug;

impl Debug {
    /// Writes `value` as one line to standard output.
    fn print(value: &Dynamic) {
        write_line(&mut std::io::stdout(), &value.to_string());
    }
}

/// Writes `line` followed by a newline to `writer` and flushes it.
fn write_line(writer: &mut impl std::io::Write, line: &str) {
    let _ = writeln!(writer, "{line}");
    let _ = writer.flush();
}

/// Registers `Debug` and its `print`.
///
/// `print` is reserved by Rhai, so `Debug.print(value)` cannot be parsed as an
/// ordinary method call: the parser rejects a reserved word after `.`. It is
/// registered as a custom syntax instead, and because that turns `print` into a
/// custom keyword, a matching `print(value)` syntax is registered too so the
/// built-in spelling keeps working.
fn register_debug(engine: &mut Engine) {
    engine.register_type_with_name::<Debug>("Debug");
    let _ = engine.register_custom_syntax(
        ["Debug", ".", "print", "(", "$expr$", ")"],
        false,
        |context, inputs| {
            let value = inputs[0].eval_with_context(context)?;
            Debug::print(&value);
            Ok(Dynamic::UNIT)
        },
    );
    let _ =
        engine.register_custom_syntax(["print", "(", "$expr$", ")"], false, |context, inputs| {
            let value = inputs[0].eval_with_context(context)?;
            Debug::print(&value);
            Ok(Dynamic::UNIT)
        });
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/// VB `App`: facts about the running program.
///
/// `title` and `path` are plain getters. `quit()` is asynchronous like
/// [`MsgBox`]: it records a [`Msg::Quit`] for the application to act on once
/// the handler returns.
#[derive(Clone)]
pub struct App {
    title: String,
    path: String,
    pending: Pending,
}

impl App {
    /// Requests that the application end.
    fn quit(&self) {
        self.pending.borrow_mut().push(Msg::Quit);
    }
}

/// The `App` object a [`StdlibContext`] describes.
fn app_object(context: &StdlibContext) -> App {
    App {
        title: context.app_title.clone(),
        path: context.app_path.clone(),
        pending: Rc::clone(&context.pending),
    }
}

/// Registers the `App` type, its getters and `quit`.
fn register_app(engine: &mut Engine) {
    engine.register_type_with_name::<App>("App");
    engine.register_get("title", |app: &mut App| app.title.clone());
    engine.register_get("path", |app: &mut App| app.path.clone());
    engine.register_fn("quit", |app: &mut App| app.quit());
}

// ---------------------------------------------------------------------------
// MsgBox
// ---------------------------------------------------------------------------

/// Registers the three `MsgBox` overloads.
fn register_msg_box(engine: &mut Engine, context: &StdlibContext) {
    let pending = Rc::clone(&context.pending);
    let form = context.form.clone();
    let default_title = context.app_title.clone();

    let title = default_title.clone();
    documented_fn!(
        engine,
        "MsgBox",
        ["text: &str"],
        ["Shows a non-blocking message box with one OK button."],
        move |text: ImmutableString| queue_msg_box(
            &pending,
            &form,
            &text,
            &title,
            MsgBoxButtons::OkOnly,
            None,
        )
    );

    let pending = Rc::clone(&context.pending);
    let form = context.form.clone();
    documented_fn!(
        engine,
        "MsgBox",
        ["text: &str", "title: &str"],
        ["Shows a non-blocking message box with the given title and one OK button."],
        move |text: ImmutableString, title: ImmutableString| queue_msg_box(
            &pending,
            &form,
            &text,
            &title,
            MsgBoxButtons::OkOnly,
            None,
        )
    );

    let pending = Rc::clone(&context.pending);
    let form = context.form.clone();
    documented_fn!(
        engine,
        "MsgBox",
        ["text: &str", "title: &str", "buttons: i64", "callback: Fn"],
        ["Shows a non-blocking message box and calls `callback` with the result code."],
        move |text: ImmutableString, title: ImmutableString, buttons: i64, callback: FnPtr| {
            queue_msg_box(
                &pending,
                &form,
                &text,
                &title,
                MsgBoxButtons::from_vb(buttons),
                Some(callback),
            )
        }
    );
}

/// Records a message box for the application to show after the handler returns.
fn queue_msg_box(
    pending: &Pending,
    form: &str,
    text: &str,
    title: &str,
    buttons: MsgBoxButtons,
    callback: Option<FnPtr>,
) {
    pending.borrow_mut().push(Msg::MsgBox {
        form: form.to_owned(),
        text: text.to_owned(),
        title: title.to_owned(),
        buttons,
        callback,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazyrad_project::lazyrad_catalog;
    use xui_form::{SetError, Value, ValueType};

    /// A form host with no controls, enough to build an engine for tests.
    struct NoControls;

    impl crate::control::FormHost for NoControls {
        fn get(&self, _control: &str, _property: &str) -> Option<Value> {
            None
        }

        fn set(&self, _control: &str, _property: &str, _value: &Value) -> Result<(), SetError> {
            Ok(())
        }

        fn property_type(&self, _control: &str, _property: &str) -> Option<ValueType> {
            None
        }

        fn names(&self) -> Vec<String> {
            Vec::new()
        }
    }

    /// Builds a stdlib host whose pending queue the caller can inspect.
    fn test_host(context: &StdlibContext) -> EngineHost {
        EngineHost::new(
            Rc::new(NoControls),
            &lazyrad_catalog(),
            "frmMain.rhai",
            context.clone(),
        )
    }

    #[test]
    fn functions_call_through_the_engine() {
        let mut host = test_host(&StdlibContext::headless("frmMain"));
        let engine = host.engine_mut();
        let left: String = engine.eval("Left(\"hello\", 3)").unwrap();
        assert_eq!(left, "hel");
        let joined: String = engine.eval("Join(Split(\"a,b\", \",\"), \"-\")").unwrap();
        assert_eq!(joined, "a-b");
        let formatted: String = engine.eval("Format(1234.5, \"#,##0.00\")").unwrap();
        assert_eq!(formatted, "1,234.50");
        let root: f64 = engine.eval("Sqr(16)").unwrap();
        assert_eq!(root, 4.0);
        let text: String = engine.eval("Str(-5)").unwrap();
        assert_eq!(text, "-5");
    }

    #[test]
    fn now_date_and_time_return_iso_strings() {
        let mut host = test_host(&StdlibContext::headless("frmMain"));
        let engine = host.engine_mut();
        let date: String = engine.eval("Date()").unwrap();
        assert_eq!(date.len(), 10);
        assert_eq!(date.matches('-').count(), 2);
        let time: String = engine.eval("Time()").unwrap();
        assert_eq!(time.matches(':').count(), 2);
        let now: String = engine.eval("Now()").unwrap();
        assert!(now.contains(' '));
    }

    #[test]
    fn app_reports_the_context_and_can_quit() {
        let pending: Pending = Rc::new(RefCell::new(Vec::new()));
        let context = StdlibContext {
            form: "frmMain".to_owned(),
            pending: Rc::clone(&pending),
            app_title: "Demo".to_owned(),
            app_path: "C:/demo".to_owned(),
        };
        let mut host = test_host(&context);
        let engine = host.engine_mut();
        let title: String = engine.eval("App.title").unwrap();
        let path: String = engine.eval("App.path").unwrap();
        assert_eq!(title, "Demo");
        assert_eq!(path, "C:/demo");
        engine.eval::<()>("App.quit()").unwrap();
        assert!(matches!(pending.borrow().last(), Some(Msg::Quit)));
    }

    #[test]
    fn msg_box_records_the_callback_and_buttons() {
        let pending: Pending = Rc::new(RefCell::new(Vec::new()));
        let context = StdlibContext {
            form: "frmMain".to_owned(),
            pending: Rc::clone(&pending),
            app_title: "Demo".to_owned(),
            app_path: String::new(),
        };
        let mut host = test_host(&context);
        host.engine_mut()
            .eval::<()>("MsgBox(\"Save?\", \"Confirm\", vbYesNo, |r| r)")
            .unwrap();

        let queued = pending.borrow();
        assert_eq!(queued.len(), 1);
        match &queued[0] {
            Msg::MsgBox {
                form,
                text,
                title,
                buttons,
                callback,
            } => {
                assert_eq!(form, "frmMain");
                assert_eq!(text, "Save?");
                assert_eq!(title, "Confirm");
                assert_eq!(*buttons, MsgBoxButtons::YesNo);
                assert!(callback.is_some());
            }
            other => panic!("expected a MsgBox message, got {other:?}"),
        }
    }

    #[test]
    fn msg_box_defaults_its_title_and_call_back_is_optional() {
        let pending: Pending = Rc::new(RefCell::new(Vec::new()));
        let context = StdlibContext {
            form: "frmMain".to_owned(),
            pending: Rc::clone(&pending),
            app_title: "Demo".to_owned(),
            app_path: String::new(),
        };
        let mut host = test_host(&context);
        host.engine_mut().eval::<()>("MsgBox(\"hello\")").unwrap();
        match &pending.borrow()[0] {
            Msg::MsgBox {
                title,
                buttons,
                callback,
                ..
            } => {
                assert_eq!(title, "Demo");
                assert_eq!(*buttons, MsgBoxButtons::OkOnly);
                assert!(callback.is_none());
            }
            other => panic!("expected a MsgBox message, got {other:?}"),
        }
    }

    #[test]
    fn debug_print_is_callable() {
        let mut host = test_host(&StdlibContext::headless("frmMain"));
        host.engine_mut()
            .eval::<()>("Debug.print(\"a line\")")
            .expect("Debug.print runs");
        // Registering the `Debug.print` syntax turns `print` into a custom
        // keyword; the plain spelling must keep working.
        host.engine_mut()
            .eval::<()>("print(\"another line\")")
            .expect("print still runs");
    }

    #[test]
    fn stdlib_functions_carry_doc_comments_in_metadata() {
        let mut host = test_host(&StdlibContext::headless("frmMain"));
        let metadata = host
            .engine_mut()
            .gen_fn_metadata_to_json(false)
            .expect("metadata serialises");
        assert!(metadata.contains("Returns the first `count` characters"));
        assert!(metadata.contains("Returns a pseudo-random number"));
    }

    #[test]
    fn left_right_and_mid_count_characters() {
        assert_eq!(left("hello", 3), "hel");
        assert_eq!(left("hello", 99), "hello");
        assert_eq!(left("hello", -1), "");
        assert_eq!(right("hello", 3), "llo");
        assert_eq!(right("hello", 99), "hello");
        assert_eq!(mid("hello", 2, Some(3)), "ell");
        assert_eq!(mid("hello", 2, None), "ello");
        assert_eq!(mid("hello", 0, Some(2)), "he");
        assert_eq!(mid("héllo", 2, Some(2)), "él");
    }

    #[test]
    fn len_counts_characters_not_bytes() {
        assert_eq!(char_len("héllo"), 5);
    }

    #[test]
    fn trim_case_and_replace_helpers_work() {
        let mut host = test_host(&StdlibContext::headless("frmMain"));
        let engine = host.engine_mut();
        let trim: String = engine.eval(r#"Trim("  hi  ")"#).unwrap();
        assert_eq!(trim, "hi");
        let ltrim: String = engine.eval(r#"LTrim("  hi  ")"#).unwrap();
        assert_eq!(ltrim, "hi  ");
        let rtrim: String = engine.eval(r#"RTrim("  hi  ")"#).unwrap();
        assert_eq!(rtrim, "  hi");
        let upper: String = engine.eval(r#"UCase("aBc")"#).unwrap();
        assert_eq!(upper, "ABC");
        let lower: String = engine.eval(r#"LCase("aBc")"#).unwrap();
        assert_eq!(lower, "abc");
        let replaced: String = engine.eval(r#"Replace("a-b-c", "-", "+")"#).unwrap();
        assert_eq!(replaced, "a+b+c");
        let untouched: String = engine.eval(r#"Replace("abc", "z", "+")"#).unwrap();
        assert_eq!(untouched, "abc");
    }

    #[test]
    fn randomize_seeds_the_generator() {
        let mut first = test_host(&StdlibContext::headless("frmMain"));
        let mut second = test_host(&StdlibContext::headless("frmMain"));
        let a: f64 = first.engine_mut().eval("Randomize(7); Rnd()").unwrap();
        let b: f64 = second.engine_mut().eval("Randomize(7); Rnd()").unwrap();
        assert_eq!(a, b, "the same seed produces the same first value");
        assert!((0.0..1.0).contains(&a));
    }

    #[test]
    fn in_str_is_one_based_and_zero_when_absent() {
        assert_eq!(in_str("hello", "ll", 1), 3);
        assert_eq!(in_str("hello", "z", 1), 0);
        assert_eq!(in_str("hello", "l", 4), 4);
        assert_eq!(in_str("héllo", "llo", 1), 3);
    }

    #[test]
    fn split_and_join_round_trip() {
        let parts = split("a,b,c", ",");
        assert_eq!(join(parts, "-"), "a-b-c");
        assert_eq!(
            split("abc", "").len(),
            1,
            "VB: an empty delimiter does not split"
        );
    }

    #[test]
    fn val_parses_the_leading_number() {
        assert_eq!(val("12.5abc"), 12.5);
        assert_eq!(val("  -3"), -3.0);
        assert_eq!(val("abc"), 0.0);
        assert_eq!(val("+7"), 7.0);
        assert_eq!(val(".5"), 0.5);
        assert_eq!(val("1.2.3"), 1.2);
    }

    #[test]
    fn str_reserves_a_leading_space_for_non_negatives() {
        assert_eq!(str_number(&Dynamic::from(5_i64)), " 5");
        assert_eq!(str_number(&Dynamic::from(-5_i64)), "-5");
        assert_eq!(str_number(&Dynamic::from(1.5_f64)), " 1.5");
        assert_eq!(str_number(&Dynamic::from(-1.5_f64)), "-1.5");
    }

    #[test]
    fn format_supports_decimals_grouping_and_percent() {
        assert_eq!(
            format_number(&Dynamic::from(1234.5_f64), "0.00").unwrap(),
            "1234.50"
        );
        assert_eq!(
            format_number(&Dynamic::from(1234.5_f64), "#,##0.00").unwrap(),
            "1,234.50"
        );
        assert_eq!(format_number(&Dynamic::from(12_i64), "000").unwrap(), "012");
        assert_eq!(
            format_number(&Dynamic::from(0.123_f64), "0.0%").unwrap(),
            "12.3%"
        );
        assert!(format_number(&Dynamic::from("x".to_owned()), "0").is_err());
    }

    #[test]
    fn abs_int_and_round_keep_their_types() {
        assert_eq!(abs_value(Dynamic::from(-3_i64)).unwrap().to_string(), "3");
        assert_eq!(
            abs_value(Dynamic::from(-3.5_f64)).unwrap().to_string(),
            "3.5"
        );
        assert_eq!(
            int_value(Dynamic::from(-1.5_f64)).unwrap().to_string(),
            "-2"
        );
        assert_eq!(
            round_value(Dynamic::from(3_i64), 0).unwrap().to_string(),
            "3"
        );
        assert_eq!(
            round_value(Dynamic::from(0.5_f64), 0).unwrap().to_string(),
            "0.0"
        );
        assert_eq!(
            round_value(Dynamic::from(1.5_f64), 0).unwrap().to_string(),
            "2.0"
        );
        assert_eq!(
            round_value(Dynamic::from(2.345_f64), 2)
                .unwrap()
                .to_string(),
            "2.35"
        );
    }

    #[test]
    fn sqr_min_and_max_work() {
        assert_eq!(sqr_value(Dynamic::from(9_i64)).unwrap().to_string(), "3.0");
        assert!(sqr_value(Dynamic::from(-1_i64)).is_err());
        assert_eq!(
            min_value(Dynamic::from(3_i64), Dynamic::from(7_i64))
                .unwrap()
                .to_string(),
            "3"
        );
        assert_eq!(
            max_value(Dynamic::from(3_i64), Dynamic::from(7.5_f64))
                .unwrap()
                .to_string(),
            "7.5"
        );
    }

    #[test]
    fn the_random_generator_stays_in_range() {
        let state = Cell::new(42);
        for _ in 0..1000 {
            let value = next_random(&state);
            assert!((0.0..1.0).contains(&value));
        }
    }

    #[test]
    fn time_formatting_is_iso_8601() {
        let value = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("a valid timestamp");
        assert_eq!(format_date(value), "2023-11-14");
        assert_eq!(format_time(value), "22:13:20");
        assert_eq!(format_now(value), "2023-11-14 22:13:20");
    }

    #[test]
    fn write_line_adds_a_newline_and_flushes() {
        let mut buffer: Vec<u8> = Vec::new();
        write_line(&mut buffer, "hello");
        assert_eq!(buffer, b"hello\n");
    }
}
