#![forbid(unsafe_code)]

//! The Iteration 1 standard library.
//!
//! It is deliberately small (PLAN.md §1.1, §4.4): Rhai already has the string,
//! maths and collection functions a script needs (`len`, `sub_string`,
//! `index_of`, `trim`, `to_upper`, `split`, `abs`, `floor`, `round`, `sqrt`,
//! `min`, `max`, …), so this module only adds what Rhai lacks. Everything is
//! snake_case:
//!
//! * `msg_box(text [, title [, buttons, callback]])`: an in-window message box;
//! * `open_file_dialog(title, filter, callback)`: the platform's open-file
//!   dialog, asynchronous like `msg_box`;
//! * `file_read_bytes(path)`, `file_write_bytes(path, blob)` (in [`fs`]);
//! * `app.title`, `app.path`, `app.quit()`;
//! * `now()`, `today()`;
//! * `random()`, `random_range(start, end)`, `seed_random(seed)`;
//! * `array.join(separator)`;
//! * for games on a `Canvas`: `rgb(r, g, b)` (a `0xRRGGBB` colour),
//!   `clamp(value, low, high)` and
//!   `rects_overlap(x1, y1, w1, h1, x2, y2, w2, h2)`;
//! * Rhai's own `print(value)` and `debug(value)`, routed to standard output so
//!   the IDE's Output pane shows them.
//!
//! # Doc comments
//!
//! Every function is registered through [`rhai::FuncRegistration`] with its
//! parameter names and a Rhai doc comment, so the `metadata` feature gives the
//! editor's completion real help text (PLAN.md §5).
//!
//! # Non-blocking `msg_box`
//!
//! `msg_box` is asynchronous in Iteration 1. It records a
//! [`Msg::MsgBox`] on the host's pending queue and
//! returns immediately; the application opens the in-window dialog and later
//! calls the optional callback with the button pressed (`"ok"`, `"cancel"`,
//! `"yes"` or `"no"`). A blocking version needs `Ui::open_modal`, which the
//! canvas backend does not implement yet (PLAN.md §10, G14; va1erian/xui#146).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use rhai::{Array, Dynamic, Engine, EvalAltResult, FnPtr, ImmutableString, Position};
use time::OffsetDateTime;

use crate::files::{NoProjectFiles, ProjectFiles};
use crate::fs_policy::FsPolicy;
use xui_rhai::EngineHost;
use xui_rhai::message::{Msg, MsgBoxButtons, Pending};

/// The host state the standard library needs from the running application.
///
/// It is assembled by [`crate::form::FormInstance`] when it builds a form, so
/// the script's `app` object and its message boxes speak for that form.
#[derive(Clone)]
pub struct StdlibContext {
    /// The form whose script is being run.
    pub form: String,
    /// The queue `msg_box` and `app.quit()` write to.
    pub pending: Pending,
    /// The value `app.title` reports.
    pub app_title: String,
    /// The value `app.path` reports.
    pub app_path: String,
    /// Which paths the `file_*` and `dir_*` functions may touch.
    pub fs: Rc<FsPolicy>,
    /// The project's own files, read before the filesystem.
    pub files: Rc<dyn ProjectFiles>,
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
            fs: Rc::new(FsPolicy::Unrestricted),
            files: Rc::new(NoProjectFiles),
        }
    }
}

/// Registers the whole standard library on `host`.
///
/// This is the single place the stdlib is installed: the [`EngineSetup`] impl
/// for [`StdlibContext`] calls it, so every engine (the player's, the designer
/// preview's, the syntax checker's) sees the same functions and globals,
/// followed by the host's [`crate::extensions`].
///
/// [`EngineSetup`]: xui_rhai::EngineSetup
pub fn register(host: &mut EngineHost, context: &StdlibContext) {
    let engine = host.engine_mut();
    register_output(engine);
    register_join(engine);
    register_games(engine);
    register_random(engine);
    register_time(engine);
    register_app(engine);
    register_msg_box(engine, context);
    register_open_file_dialog(engine, context);
    fs::register(engine, &context.fs, &context.files);
    crate::extensions::apply(
        engine,
        &crate::extensions::ExtensionScope {
            form: &context.form,
        },
    );
    host.set_global("app", Dynamic::from(app_object(context)));
}

/// Installs the LazyRAD standard library when an [`EngineHost`] is built,
/// so the engine setup stays out of the reusable `xui-rhai` crate.
impl xui_rhai::EngineSetup for StdlibContext {
    fn setup(self, host: &mut EngineHost) {
        register(host, &self);
    }
}

/// Registers a global native function with its Rhai parameter names and doc
/// comment, so `metadata` can describe it. Without the `metadata` feature (the
/// LazyOS player) the names and comments are dropped and only the function is
/// registered, which keeps them out of the binary.
#[cfg(feature = "metadata")]
macro_rules! documented_fn {
    ($engine:expr, $name:literal, [$($param:literal),*], [$($comment:literal),*], $func:expr) => {{
        rhai::FuncRegistration::new($name)
            .with_params_info::<&str>([$($param),*])
            .with_comments::<&str>([$($comment),*])
            .register_into_engine($engine, $func);
    }};
}

/// The player flavour of [`documented_fn`]: register the function alone.
#[cfg(not(feature = "metadata"))]
macro_rules! documented_fn {
    ($engine:expr, $name:literal, [$($param:literal),*], [$($comment:literal),*], $func:expr) => {{
        rhai::FuncRegistration::new($name).register_into_engine($engine, $func);
    }};
}

mod fs;

/// A Rhai runtime error with no position; the VM fills it in.
fn script_error(message: impl Into<String>) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(message.into()),
        Position::NONE,
    ))
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Routes Rhai's built-in `print` and `debug` to standard output, one flushed
/// line per call, so the IDE's Output pane (reading the player's stdout) sees
/// each line as it happens.
fn register_output(engine: &mut Engine) {
    engine.on_print(|text| write_line(&mut std::io::stdout(), text));
    engine.on_debug(|text, _source, _position| write_line(&mut std::io::stdout(), text));
}

/// Writes `line` followed by a newline to `writer` and flushes it.
fn write_line(writer: &mut impl std::io::Write, line: &str) {
    let _ = writeln!(writer, "{line}");
    let _ = writer.flush();
}

// ---------------------------------------------------------------------------
// Arrays
// ---------------------------------------------------------------------------

/// Registers `array.join(separator)`, which Rhai's array package lacks.
fn register_join(engine: &mut Engine) {
    documented_fn!(
        engine,
        "join",
        ["items: Array", "separator: &str"],
        [
            "/// Joins the items' string forms with `separator`: `[1, 2].join(\", \")` is `\"1, 2\"`."
        ],
        |items: Array, separator: ImmutableString| join(&items, &separator)
    );
}

/// Joins `items` with `separator`, converting each value with its string form.
fn join(items: &[Dynamic], separator: &str) -> String {
    items
        .iter()
        .map(Dynamic::to_string)
        .collect::<Vec<_>>()
        .join(separator)
}

// ---------------------------------------------------------------------------
// Games
// ---------------------------------------------------------------------------

/// Registers `rgb`, `clamp` and `rects_overlap`, the small helpers a game on a
/// `Canvas` needs and Rhai lacks.
///
/// They take any mix of ints and floats, since game code computes with both.
fn register_games(engine: &mut Engine) {
    documented_fn!(
        engine,
        "rgb",
        ["r: int", "g: int", "b: int"],
        [
            "/// A colour as a `0xRRGGBB` int from red, green and blue in `0..=255` (each",
            "/// clamped). A canvas accepts it wherever it takes a colour."
        ],
        |r: Dynamic, g: Dynamic, b: Dynamic| -> Result<i64, Box<EvalAltResult>> {
            Ok(rgb(
                number("rgb", &r)?,
                number("rgb", &g)?,
                number("rgb", &b)?,
            ))
        }
    );

    documented_fn!(
        engine,
        "clamp",
        ["value: int", "low: int", "high: int"],
        ["/// `value` limited to `low..=high`."],
        |value: i64, low: i64, high: i64| -> Result<i64, Box<EvalAltResult>> {
            if low > high {
                return Err(script_error(format!(
                    "clamp needs low <= high, got {low} and {high}"
                )));
            }
            Ok(value.clamp(low, high))
        }
    );
    documented_fn!(
        engine,
        "clamp",
        ["value: float", "low: float", "high: float"],
        ["/// `value` limited to `low..=high`, as a float when any of them is one."],
        |value: Dynamic, low: Dynamic, high: Dynamic| -> Result<f64, Box<EvalAltResult>> {
            clamp(
                number("clamp", &value)?,
                number("clamp", &low)?,
                number("clamp", &high)?,
            )
        }
    );

    documented_fn!(
        engine,
        "rects_overlap",
        [
            "x1: float",
            "y1: float",
            "w1: float",
            "h1: float",
            "x2: float",
            "y2: float",
            "w2: float",
            "h2: float"
        ],
        [
            "/// Whether two rectangles, each given as x, y, width and height, overlap.",
            "/// Rectangles that only touch along an edge do not."
        ],
        |x1: Dynamic,
         y1: Dynamic,
         w1: Dynamic,
         h1: Dynamic,
         x2: Dynamic,
         y2: Dynamic,
         w2: Dynamic,
         h2: Dynamic|
         -> Result<bool, Box<EvalAltResult>> {
            let n = |value: &Dynamic| number("rects_overlap", value);
            Ok(rects_overlap(
                [n(&x1)?, n(&y1)?, n(&w1)?, n(&h1)?],
                [n(&x2)?, n(&y2)?, n(&w2)?, n(&h2)?],
            ))
        }
    );
}

/// A script number (an int or a float) as a float, or an error naming
/// `function`.
fn number(function: &str, value: &Dynamic) -> Result<f64, Box<EvalAltResult>> {
    if let Ok(value) = value.as_float() {
        Ok(value)
    } else if let Ok(value) = value.as_int() {
        Ok(value as f64)
    } else {
        Err(script_error(format!(
            "{function} expects numbers, got {}",
            value.type_name()
        )))
    }
}

/// `0xRRGGBB` from channels clamped to `0..=255` (a NaN channel is 0).
fn rgb(r: f64, g: f64, b: f64) -> i64 {
    let channel = |value: f64| value.clamp(0.0, 255.0) as i64;
    (channel(r) << 16) | (channel(g) << 8) | channel(b)
}

/// `value` limited to `low..=high`, refusing a reversed or NaN range.
fn clamp(value: f64, low: f64, high: f64) -> Result<f64, Box<EvalAltResult>> {
    if low.is_nan() || high.is_nan() || low > high {
        return Err(script_error(format!(
            "clamp needs low <= high, got {low} and {high}"
        )));
    }
    Ok(value.max(low).min(high))
}

/// Whether the rectangles `[x, y, w, h]` overlap with a positive area.
fn rects_overlap(a: [f64; 4], b: [f64; 4]) -> bool {
    let [ax, ay, aw, ah] = a;
    let [bx, by, bw, bh] = b;
    ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
}

// ---------------------------------------------------------------------------
// Random numbers
// ---------------------------------------------------------------------------

/// Registers `random`, `random_range` and `seed_random`, which Rhai lacks.
///
/// The generator is a small xorshift64* per engine: good enough for games and
/// demos, reproducible after `seed_random`, and free of a dependency.
fn register_random(engine: &mut Engine) {
    let state = Rc::new(Cell::new(seed()));

    let generator = Rc::clone(&state);
    documented_fn!(
        engine,
        "random",
        [],
        ["/// A random float in `[0.0, 1.0)`."],
        move || next_random(&generator)
    );

    let generator = Rc::clone(&state);
    documented_fn!(
        engine,
        "random_range",
        ["start: i64", "end: i64"],
        ["/// A random integer in `start..end` (the end is excluded)."],
        move |start: i64, end: i64| -> Result<i64, Box<EvalAltResult>> {
            if end <= start {
                return Err(script_error(format!(
                    "random_range needs start < end, got {start}..{end}"
                )));
            }
            // In i128, so even random_range(i64::MIN, i64::MAX) cannot
            // overflow; the offset is clamped below the (excluded) end.
            let span = end as i128 - start as i128;
            let offset = ((next_random(&generator) * span as f64) as i128).min(span - 1);
            Ok((start as i128 + offset) as i64)
        }
    );

    let generator = Rc::clone(&state);
    documented_fn!(
        engine,
        "seed_random",
        ["seed: i64"],
        ["/// Reseeds the generator, so the following random numbers repeat."],
        move |seed: i64| generator.set((seed as u64) | 1)
    );
}

/// A seed from the clock; never zero, which would stall xorshift.
fn seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0x9E37_79B9_7F4A_7C15, |elapsed| elapsed.as_nanos() as u64);
    nanos | 1
}

/// The next float in `[0, 1)` from the xorshift64* generator in `state`.
fn next_random(state: &Cell<u64>) -> f64 {
    let mut x = state.get();
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    state.set(x);
    let scrambled = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
    (scrambled >> 11) as f64 / (1u64 << 53) as f64
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// Registers `now` and `today`.
fn register_time(engine: &mut Engine) {
    documented_fn!(
        engine,
        "now",
        [],
        ["/// The current local date and time as `YYYY-MM-DD HH:MM:SS`."],
        || format_date_time(local_now())
    );
    documented_fn!(
        engine,
        "today",
        [],
        ["/// The current local date as `YYYY-MM-DD`."],
        || format_date(local_now())
    );
}

/// The current time in the local zone, falling back to UTC when the offset is
/// not available (a sandbox or a platform without a time zone database).
///
/// Without the `desktop` feature (LazyOS: no zone database, and `time`'s
/// `local-offset` is not compiled in) the clock is always UTC.
#[cfg(feature = "desktop")]
fn local_now() -> OffsetDateTime {
    OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc())
}

/// The portable clock: UTC.
#[cfg(not(feature = "desktop"))]
fn local_now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// Formats a date and time as `YYYY-MM-DD HH:MM:SS`.
fn format_date_time(value: OffsetDateTime) -> String {
    format!(
        "{} {:02}:{:02}:{:02}",
        format_date(value),
        value.hour(),
        value.minute(),
        value.second()
    )
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

// ---------------------------------------------------------------------------
// app
// ---------------------------------------------------------------------------

/// The `app` object: facts about the running program.
///
/// `title` and `path` are plain getters. `quit()` is asynchronous like
/// `msg_box`: it records a [`Msg::Quit`] for the application to act on once
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

/// The `app` object a [`StdlibContext`] describes.
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
// msg_box
// ---------------------------------------------------------------------------

/// Registers the three `msg_box` overloads.
fn register_msg_box(engine: &mut Engine, context: &StdlibContext) {
    let pending = Rc::clone(&context.pending);
    let form = context.form.clone();
    let title = context.app_title.clone();
    documented_fn!(
        engine,
        "msg_box",
        ["text: &str"],
        ["/// Shows a non-blocking message box with one OK button."],
        move |text: ImmutableString| queue_msg_box(
            &pending,
            &form,
            &text,
            &title,
            MsgBoxButtons::Ok,
            None,
        )
    );

    let pending = Rc::clone(&context.pending);
    let form = context.form.clone();
    documented_fn!(
        engine,
        "msg_box",
        ["text: &str", "title: &str"],
        ["/// Shows a non-blocking message box with a title and one OK button."],
        move |text: ImmutableString, title: ImmutableString| queue_msg_box(
            &pending,
            &form,
            &text,
            &title,
            MsgBoxButtons::Ok,
            None,
        )
    );

    let pending = Rc::clone(&context.pending);
    let form = context.form.clone();
    documented_fn!(
        engine,
        "msg_box",
        ["text: &str", "title: &str", "buttons: &str", "callback: Fn"],
        [
            "/// Shows a non-blocking message box and calls `callback` with the button pressed.",
            "///",
            "/// `buttons` is `\"ok\"`, `\"ok_cancel\"` or `\"yes_no\"`; the callback",
            "/// receives `\"ok\"`, `\"cancel\"`, `\"yes\"` or `\"no\"`."
        ],
        move |text: ImmutableString,
              title: ImmutableString,
              buttons: ImmutableString,
              callback: FnPtr|
              -> Result<(), Box<EvalAltResult>> {
            let buttons = MsgBoxButtons::from_name(&buttons).ok_or_else(|| {
                script_error(format!(
                    "msg_box buttons must be \"ok\", \"ok_cancel\" or \"yes_no\", got \"{buttons}\""
                ))
            })?;
            queue_msg_box(&pending, &form, &text, &title, buttons, Some(callback));
            Ok(())
        }
    );
}

/// Registers `open_file_dialog(title, filter, callback)`.
///
/// Like `msg_box`, it is asynchronous: it records a [`Msg::OpenFileDialog`] and
/// returns, and the application shows the platform's dialog and calls the
/// callback afterwards. The callback receives the picked path as a string, or
/// `()` when the user cancels; a picked path is granted read access to the
/// script's sandbox, limited to that file.
fn register_open_file_dialog(engine: &mut Engine, context: &StdlibContext) {
    let pending = Rc::clone(&context.pending);
    let form = context.form.clone();
    documented_fn!(
        engine,
        "open_file_dialog",
        ["title: &str", "filter: &str", "callback: Fn"],
        [
            "/// Asks the platform for a file to open, without blocking.",
            "///",
            "/// `filter` is `\"name|pattern;name|pattern\"`: groups separated by `;`,",
            "/// each a display name and its `,`-separated patterns after a `|`.",
            "/// The callback runs with the picked path, or `()` when the user",
            "/// cancels. A picked file is granted read access to the sandbox."
        ],
        move |title: ImmutableString, filter: ImmutableString, callback: FnPtr| {
            pending.borrow_mut().push(Msg::OpenFileDialog {
                form: form.clone(),
                title: title.to_string(),
                filter: filter.to_string(),
                callback,
            });
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

    /// An engine with the stdlib registered on `context`.
    fn engine_with(context: &StdlibContext) -> Engine {
        let mut engine = crate::new_engine();
        register_output(&mut engine);
        register_join(&mut engine);
        register_games(&mut engine);
        register_random(&mut engine);
        register_time(&mut engine);
        register_app(&mut engine);
        register_msg_box(&mut engine, context);
        register_open_file_dialog(&mut engine, context);
        engine
    }

    #[test]
    fn join_uses_each_items_string_form() {
        let engine = engine_with(&StdlibContext::headless("main_form"));
        let joined: String = engine
            .eval(r#"[1, "two", 3.5].join(", ")"#)
            .expect("join evaluates");
        assert_eq!(joined, "1, two, 3.5");
        let empty: String = engine.eval(r#"[].join("-")"#).expect("empty join");
        assert_eq!(empty, "");
    }

    #[test]
    fn a_seeded_generator_repeats_and_stays_in_range() {
        let engine = engine_with(&StdlibContext::headless("main_form"));
        let script = r#"
            seed_random(42);
            let first = [random(), random_range(0, 10)];
            seed_random(42);
            let second = [random(), random_range(0, 10)];
            [first, second]
        "#;
        let runs: Array = engine.eval(script).expect("random evaluates");
        assert_eq!(
            runs[0].to_string(),
            runs[1].to_string(),
            "seeded runs repeat"
        );
        let first = runs[0].clone().cast::<Array>();
        let unit = first[0].as_float().expect("random is a float");
        assert!((0.0..1.0).contains(&unit));
        let integer = first[1].as_int().expect("random_range is an int");
        assert!((0..10).contains(&integer));
    }

    #[test]
    fn random_range_handles_the_widest_range() {
        let engine = engine_with(&StdlibContext::headless("main_form"));
        for _ in 0..100 {
            let value: i64 = engine
                .eval("random_range(-9223372036854775807 - 1, 9223372036854775807)")
                .expect("the widest range works");
            assert!(value < i64::MAX, "the end is excluded");
        }
    }

    #[test]
    fn random_range_rejects_an_empty_range() {
        let engine = engine_with(&StdlibContext::headless("main_form"));
        assert!(engine.eval::<i64>("random_range(5, 5)").is_err());
    }

    #[test]
    fn time_functions_use_iso_formats() {
        let engine = engine_with(&StdlibContext::headless("main_form"));
        let date: String = engine.eval("today()").expect("today evaluates");
        assert_eq!(date.len(), 10);
        assert_eq!(&date[4..5], "-");
        assert_eq!(&date[7..8], "-");
        let stamp: String = engine.eval("now()").expect("now evaluates");
        assert_eq!(stamp.len(), 19);
        assert_eq!(&stamp[10..11], " ");
        assert_eq!(&stamp[13..14], ":");
    }

    #[test]
    fn msg_box_queues_a_dialog_and_rejects_unknown_buttons() {
        let context = StdlibContext::headless("main_form");
        let engine = engine_with(&context);
        engine.run(r#"msg_box("Hello");"#).expect("msg_box runs");
        engine
            .run(r#"msg_box("Save?", "Editor", "yes_no", |answer| answer);"#)
            .expect("msg_box with a callback runs");
        {
            let queued = context.pending.borrow();
            assert!(matches!(
                &queued[0],
                Msg::MsgBox { text, buttons: MsgBoxButtons::Ok, callback: None, .. } if text == "Hello"
            ));
            assert!(matches!(
                &queued[1],
                Msg::MsgBox {
                    buttons: MsgBoxButtons::YesNo,
                    callback: Some(_),
                    ..
                }
            ));
        }
        assert!(engine.run(r#"msg_box("?", "t", "maybe", |a| a);"#).is_err());
    }

    #[test]
    fn app_reports_its_facts_and_queues_quit() {
        let context = StdlibContext {
            app_title: "Hello".to_owned(),
            app_path: "/projects/hello".to_owned(),
            ..StdlibContext::headless("main_form")
        };
        let mut engine = engine_with(&context);
        let app = app_object(&context);
        engine.register_fn("the_app", move || app.clone());
        let title: String = engine.eval("the_app().title").expect("title");
        assert_eq!(title, "Hello");
        let path: String = engine.eval("the_app().path").expect("path");
        assert_eq!(path, "/projects/hello");
        engine.run("the_app().quit();").expect("quit runs");
        assert!(matches!(context.pending.borrow().last(), Some(Msg::Quit)));
    }

    #[cfg(feature = "metadata")]
    #[test]
    fn every_function_carries_a_doc_comment_for_completion() {
        let engine = engine_with(&StdlibContext::headless("main_form"));
        let metadata = engine
            .gen_fn_metadata_to_json(false)
            .expect("metadata serialises");
        for name in [
            "msg_box",
            "open_file_dialog",
            "join",
            "random",
            "random_range",
            "seed_random",
            "now",
            "today",
            "rgb",
            "clamp",
            "rects_overlap",
        ] {
            assert!(
                metadata.contains(&format!("\"{name}\"")),
                "{name} is listed"
            );
        }
        assert!(metadata.contains("non-blocking message box"));
    }

    #[test]
    fn rgb_packs_and_clamps_the_channels() {
        let engine = engine_with(&StdlibContext::headless("main_form"));
        let packed: i64 = engine.eval("rgb(0x12, 0x34, 0x56)").expect("rgb");
        assert_eq!(packed, 0x12_3456);
        let clamped: i64 = engine.eval("rgb(300, -5, 127.9)").expect("rgb");
        assert_eq!(clamped, 0xFF_007F);
        assert!(engine.eval::<i64>(r#"rgb("a", 0, 0)"#).is_err());
    }

    #[test]
    fn clamp_limits_ints_and_floats() {
        let engine = engine_with(&StdlibContext::headless("main_form"));
        assert_eq!(engine.eval::<i64>("clamp(15, 0, 10)").expect("int"), 10);
        assert_eq!(engine.eval::<i64>("clamp(-3, 0, 10)").expect("int"), 0);
        assert_eq!(
            engine.eval::<f64>("clamp(2.5, 0, 1)").expect("mixed"),
            1.0,
            "a float value with int bounds"
        );
        assert_eq!(
            engine.eval::<f64>("clamp(0.25, 0.0, 1.0)").expect("f"),
            0.25
        );
        assert!(engine.eval::<i64>("clamp(1, 10, 0)").is_err());
        assert!(engine.eval::<f64>("clamp(1.0, 10.0, 0.0)").is_err());
    }

    #[test]
    fn rects_overlap_needs_a_shared_area() {
        let engine = engine_with(&StdlibContext::headless("main_form"));
        let overlap = |script: &str| engine.eval::<bool>(script).expect("evaluates");
        assert!(overlap("rects_overlap(0, 0, 10, 10, 5, 5, 10, 10)"));
        assert!(overlap(
            "rects_overlap(0.0, 0.0, 10.0, 10.0, 9.5, 9.5, 1, 1)"
        ));
        assert!(
            !overlap("rects_overlap(0, 0, 10, 10, 10, 0, 5, 5)"),
            "touching edges do not overlap"
        );
        assert!(!overlap("rects_overlap(0, 0, 10, 10, 20, 20, 5, 5)"));
    }

    #[test]
    fn write_line_writes_one_flushed_line() {
        let mut buffer = Vec::new();
        write_line(&mut buffer, "hello");
        assert_eq!(buffer, b"hello\n");
    }
}
