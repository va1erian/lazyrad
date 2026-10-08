#![forbid(unsafe_code)]

//! `Canvas`: a control a script draws on, with a frame loop and raw keyboard
//! and mouse input. It is the toolkit's basic game graphics API.
//!
//! # Drawing
//!
//! A canvas keeps a **retained display list**. Each drawing method a script
//! calls (`fill_rect`, `text`, …) appends one command and schedules a repaint;
//! the painter replays the list over the `background` colour, clipped to the
//! canvas. `clear(color)` empties the list and starts it with a full fill, so
//! a game redraws its scene each frame with `clear` followed by its shapes.
//!
//! Coordinates and sizes are **logical pixels** (DIPs) relative to the
//! canvas's top-left corner, as floats. The painter scales them by the
//! window's DPI, so a game looks the same at 100% and 150% scaling.
//!
//! # The frame loop
//!
//! Setting `fps` above zero starts a per-control timer
//! ([`Control::set_timer`]) that raises `Frame(dt)` about `fps` times a
//! second; `dt` is the time since the previous frame in seconds, measured
//! with [`Instant`] and clamped to [`MAX_FRAME_DT`] so a stall (a breakpoint,
//! a dragged window) does not teleport the game. Setting `fps` to zero stops
//! the timer, and dropping the control stops it too. A design-mode canvas
//! never starts one, and neither does a canvas whose `Frame` event is not
//! bound (there would be nobody to tell).
//!
//! # Input
//!
//! A mouse press focuses the canvas, so keys reach it; a script can also call
//! `focus()` (from `form_load`, typically). `KeyDown`/`KeyUp` carry a key
//! name ([`key_name`]); the keyboard's auto-repeat does not raise `KeyDown`
//! again, while `is_key_down(key)` stays true until the key is released.
//! Losing the focus forgets every held key, so none stays stuck down.
//! `MouseDown`/`MouseUp`/`MouseMove` carry the pointer position in logical
//! pixels; `mouse_x`/`mouse_y` hold the last one.
//!
//! # Re-entrancy
//!
//! Every event reaches the host as a message the toolkit queues, so a script
//! handler runs after the canvas's mapper or timer callback has returned. The
//! canvas never holds a borrow of its state while it calls out (focusing,
//! invalidating, raising an event), and the painter only reads the list, so a
//! `Frame` handler drawing on its own canvas is the ordinary path.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Instant;

use xui_core::app::Ui;
use xui_core::arrange::{self, Handle};
use xui_core::backend::{Canvas as Surface, Event, NodeKind, NodeSpec, TextStyle, TimerId};
use xui_core::geometry::{Point, Rect, Size};
use xui_core::layout::Constraints;
use xui_core::message::{Key, MouseButton};
use xui_core::units::Dip;
use xui_core::widget::{Control, Placeable};
use xui_core::{Color, WidgetId};

use crate::build::{BuildCx, CallError, EventHandler, Made, SetError, WidgetFactory};
use crate::doc::Node;
use crate::live::WidgetProps;
use crate::schema::{
    Access, ArgSpec, CATEGORY_APPEARANCE, CATEGORY_BEHAVIOR, CATEGORY_DATA, Children, MethodSpec,
    PropertySpec, WidgetSpec, arg, event, float_type, property, text_type, widget,
};
use crate::value::{Value, ValueType};

/// The canonical kind name.
pub const KIND: &str = "Canvas";

/// The event raised once per frame while `fps` is above zero.
pub const FRAME_EVENT: &str = "Frame";

/// The longest `dt` a `Frame` event reports, in seconds: a longer gap (a
/// breakpoint, a stalled window) is reported as this.
pub const MAX_FRAME_DT: f64 = 0.1;

/// The highest `fps` a canvas accepts.
pub const MAX_FPS: i64 = 240;

/// The most commands a display list holds. A script that draws every frame
/// without calling `clear` would otherwise grow it without bound.
pub const MAX_COMMANDS: usize = 100_000;

/// The largest text size `text` and `text_width` accept, in logical pixels.
const MAX_TEXT_SIZE: f32 = 1000.0;

/// The size of the control's name in the design-mode placeholder.
const DESIGN_LABEL_SIZE: Dip = Dip(12.0);

/// The canvas spec for the catalog.
pub(crate) fn spec() -> WidgetSpec {
    let xy = || vec![arg("x", float_type()), arg("y", float_type())];
    let pointer = |name: &str, description: &str| {
        let mut args = xy();
        args.push(arg("button", text_type()));
        event(name, args, false, description)
    };
    WidgetSpec {
        properties: vec![
            property(
                "background",
                ValueType::Color,
                Value::Color(Color::rgb(0, 0, 0)),
                CATEGORY_APPEARANCE,
                "The colour painted under the drawing.",
            ),
            property(
                "fps",
                ValueType::Int {
                    min: Some(0),
                    max: Some(MAX_FPS),
                },
                Value::Int(0),
                CATEGORY_BEHAVIOR,
                "Frames per second of the Frame event; 0 stops the frame loop.",
            ),
            read_only(
                "mouse_x",
                "The pointer's last x position over the canvas, in logical pixels.",
            ),
            read_only(
                "mouse_y",
                "The pointer's last y position over the canvas, in logical pixels.",
            ),
        ],
        events: vec![
            event(
                FRAME_EVENT,
                vec![arg("dt", float_type())],
                true,
                "Raised fps times a second with the seconds since the previous frame.",
            ),
            event(
                "KeyDown",
                vec![arg("key", text_type())],
                false,
                "Raised when a key is pressed (not again while it auto-repeats).",
            ),
            event(
                "KeyUp",
                vec![arg("key", text_type())],
                false,
                "Raised when a key is released.",
            ),
            pointer(
                "MouseDown",
                "Raised when a mouse button is pressed over the canvas.",
            ),
            pointer("MouseUp", "Raised when a mouse button is released."),
            event(
                "MouseMove",
                xy(),
                false,
                "Raised when the pointer moves over the canvas.",
            ),
        ],
        methods: methods(),
        ..widget(
            KIND,
            "A drawing surface with a frame loop and keyboard and mouse input.",
            (320.0, 240.0),
            Children::None,
        )
    }
}

/// A read-only float property.
fn read_only(name: &str, description: &str) -> PropertySpec {
    PropertySpec {
        access: Access::ReadOnly,
        ..property(
            name,
            float_type(),
            Value::Float(0.0),
            CATEGORY_DATA,
            description,
        )
    }
}

/// Builds a method spec.
fn method(name: &str, args: Vec<ArgSpec>, returns: Option<ValueType>, text: &str) -> MethodSpec {
    MethodSpec {
        name: name.to_owned(),
        args,
        returns,
        description: text.to_owned(),
    }
}

/// The canvas's methods, in the order the docs list them.
fn methods() -> Vec<MethodSpec> {
    let float = |name: &str| arg(name, float_type());
    let color = || arg("color", ValueType::Color);
    vec![
        method(
            "clear",
            vec![color()],
            None,
            "Empties the drawing and fills the canvas with `color`.",
        ),
        method(
            "fill_rect",
            vec![float("x"), float("y"), float("w"), float("h"), color()],
            None,
            "Fills a rectangle.",
        ),
        method(
            "stroke_rect",
            vec![
                float("x"),
                float("y"),
                float("w"),
                float("h"),
                color(),
                float("width"),
            ],
            None,
            "Outlines a rectangle with a line `width` pixels wide.",
        ),
        method(
            "fill_round_rect",
            vec![
                float("x"),
                float("y"),
                float("w"),
                float("h"),
                float("radius"),
                color(),
            ],
            None,
            "Fills a rectangle with corners rounded by `radius`.",
        ),
        method(
            "fill_circle",
            vec![float("cx"), float("cy"), float("r"), color()],
            None,
            "Fills a circle centred on (cx, cy).",
        ),
        method(
            "stroke_circle",
            vec![
                float("cx"),
                float("cy"),
                float("r"),
                color(),
                float("width"),
            ],
            None,
            "Outlines a circle centred on (cx, cy).",
        ),
        method(
            "line",
            vec![
                float("x1"),
                float("y1"),
                float("x2"),
                float("y2"),
                color(),
                float("width"),
            ],
            None,
            "Draws a straight line.",
        ),
        method(
            "text",
            vec![
                float("x"),
                float("y"),
                arg("text", text_type()),
                color(),
                float("size"),
            ],
            None,
            "Draws text with its top-left corner at (x, y), `size` pixels high.",
        ),
        method(
            "text_width",
            vec![arg("text", text_type()), float("size")],
            Some(float_type()),
            "The width `text` takes when drawn at `size`, in logical pixels.",
        ),
        method(
            "is_key_down",
            vec![arg("key", text_type())],
            Some(ValueType::Bool),
            "Whether the named key is held down while the canvas has the focus.",
        ),
        method(
            "focus",
            Vec::new(),
            None,
            "Gives the canvas the keyboard focus.",
        ),
    ]
}

/// The script name of a key, or `None` for a key a canvas does not report.
///
/// Letters are `"a"`..`"z"` and digits `"0"`..`"9"` whatever the shift state;
/// the rest are `"left"`, `"right"`, `"up"`, `"down"`, `"space"`, `"enter"`,
/// `"escape"`, `"tab"`, `"backspace"`, `"delete"`, `"insert"`, `"home"`,
/// `"end"`, `"page_up"`, `"page_down"`, `"shift"`, `"control"`, `"alt"` and
/// `"f1"`..`"f12"`. Every name is lower-case ASCII.
pub fn key_name(key: Key) -> Option<&'static str> {
    const LETTERS: [&str; 26] = [
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r",
        "s", "t", "u", "v", "w", "x", "y", "z",
    ];
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    const FUNCTION: [&str; 12] = [
        "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12",
    ];
    let code = key.code();
    let offset = |base: Key| usize::from(code - base.code());
    Some(match key {
        Key::LEFT => "left",
        Key::RIGHT => "right",
        Key::UP => "up",
        Key::DOWN => "down",
        Key::SPACE => "space",
        Key::RETURN => "enter",
        Key::ESCAPE => "escape",
        Key::TAB => "tab",
        Key::BACK => "backspace",
        Key::DELETE => "delete",
        Key::INSERT => "insert",
        Key::HOME => "home",
        Key::END => "end",
        Key::PAGE_UP => "page_up",
        Key::PAGE_DOWN => "page_down",
        Key::SHIFT => "shift",
        Key::CONTROL => "control",
        Key::MENU => "alt",
        _ if (Key::A.code()..=Key::Z.code()).contains(&code) => LETTERS[offset(Key::A)],
        _ if (Key::DIGIT0.code()..=Key::DIGIT9.code()).contains(&code) => {
            DIGITS[offset(Key::DIGIT0)]
        }
        _ if (Key::F1.code()..=Key::F12.code()).contains(&code) => FUNCTION[offset(Key::F1)],
        _ => return None,
    })
}

/// The script name of a mouse button, or `None` for the extra buttons.
fn button_name(button: MouseButton) -> Option<&'static str> {
    match button {
        MouseButton::Left => Some("left"),
        MouseButton::Right => Some("right"),
        MouseButton::Middle => Some("middle"),
        _ => None,
    }
}

/// The timer interval for `fps` frames a second, in milliseconds (at least 1).
fn frame_interval(fps: i64) -> u32 {
    let fps = fps.clamp(1, MAX_FPS) as f64;
    ((1000.0 / fps).round() as u32).max(1)
}

/// The `dt` a frame reports: the seconds between `last` and `now`, or the
/// nominal interval for the first frame, clamped to `0..=MAX_FRAME_DT`.
fn frame_dt(last: Option<Instant>, now: Instant, interval_ms: u32) -> f64 {
    let seconds = match last {
        Some(last) => now.saturating_duration_since(last).as_secs_f64(),
        None => f64::from(interval_ms) / 1000.0,
    };
    seconds.clamp(0.0, MAX_FRAME_DT)
}

/// One retained drawing command, in logical pixels relative to the canvas.
#[derive(Clone, Debug, PartialEq)]
enum DrawCmd {
    /// Fill the whole canvas.
    Clear(Color),
    /// Fill a rectangle.
    FillRect(Area, Color),
    /// Outline a rectangle with a line width.
    StrokeRect(Area, Color, f32),
    /// Fill a rectangle with a corner radius.
    FillRoundRect(Area, f32, Color),
    /// Fill a circle: centre x, centre y, radius.
    FillCircle(f32, f32, f32, Color),
    /// Outline a circle: centre x, centre y, radius, then the line width.
    StrokeCircle(f32, f32, f32, Color, f32),
    /// A line between `[x1, y1, x2, y2]`, with a line width.
    Line([f32; 4], Color, f32),
    /// Text with its top-left corner at a point, then its size.
    Text(f32, f32, String, Color, f32),
}

/// A rectangle in logical pixels with a non-negative size.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Area {
    /// The area spanned from `(x, y)` by `w`×`h`; a negative size extends to
    /// the left or upwards.
    fn new(x: f32, y: f32, w: f32, h: f32) -> Area {
        Area {
            x: if w < 0.0 { x + w } else { x },
            y: if h < 0.0 { y + h } else { y },
            w: w.abs(),
            h: h.abs(),
        }
    }

    /// The device-pixel rectangle at `scale`.
    fn to_px(self, scale: f32) -> Rect {
        Rect::new(
            px(self.x, scale),
            px(self.y, scale),
            px(self.x + self.w, scale),
            px(self.y + self.h, scale),
        )
    }
}

/// A logical coordinate in device pixels (the cast saturates).
fn px(value: f32, scale: f32) -> i32 {
    (value * scale).round() as i32
}

/// Replays `commands` over `background` on `surface`, clipped to it.
fn paint_scene(surface: &mut dyn Surface, background: Color, commands: &[DrawCmd]) {
    let bounds = surface.bounds();
    let scale = surface.dpi() as f32 / 96.0;
    surface.push_clip(bounds);
    surface.fill_rect(bounds, background);
    for command in commands {
        match command {
            DrawCmd::Clear(color) => surface.fill_rect(bounds, *color),
            DrawCmd::FillRect(area, color) => surface.fill_rect(area.to_px(scale), *color),
            DrawCmd::StrokeRect(area, color, width) => {
                surface.stroke_rect(area.to_px(scale), *color, width * scale);
            }
            DrawCmd::FillRoundRect(area, radius, color) => {
                surface.fill_rounded_rect(area.to_px(scale), radius * scale, *color);
            }
            DrawCmd::FillCircle(cx, cy, r, color) => surface.fill_ellipse(
                Point::new(px(*cx, scale), px(*cy, scale)),
                r * scale,
                r * scale,
                *color,
            ),
            DrawCmd::StrokeCircle(cx, cy, r, color, width) => surface.stroke_ellipse(
                Point::new(px(*cx, scale), px(*cy, scale)),
                r * scale,
                r * scale,
                *color,
                width * scale,
            ),
            DrawCmd::Line([x1, y1, x2, y2], color, width) => surface.draw_line(
                Point::new(px(*x1, scale), px(*y1, scale)),
                Point::new(px(*x2, scale), px(*y2, scale)),
                *color,
                width * scale,
            ),
            DrawCmd::Text(x, y, text, color, size) => {
                let left = px(*x, scale);
                let top = px(*y, scale);
                let rect = Rect::new(
                    left,
                    top,
                    bounds.right.max(left + 1),
                    top + px(size * 2.0, scale).max(1),
                );
                surface.draw_text(text, rect, &TextStyle::new(*color, Dip(*size)));
            }
        }
    }
    surface.pop_clip();
}

/// The design-mode placeholder: the background, a border and the control's
/// name, so an empty canvas is visible in the designer.
fn paint_placeholder(surface: &mut dyn Surface, background: Color, name: &str) {
    let bounds = surface.bounds();
    surface.fill_rect(bounds, background);
    let luminance = 299 * u32::from(background.r)
        + 587 * u32::from(background.g)
        + 114 * u32::from(background.b);
    let (border, text) = if luminance < 128_000 {
        (Color::rgb(0x60, 0x60, 0x60), Color::rgb(0xd0, 0xd0, 0xd0))
    } else {
        (Color::rgb(0xa0, 0xa0, 0xa0), Color::rgb(0x30, 0x30, 0x30))
    };
    surface.stroke_rect(bounds, border, 1.0);
    let style = TextStyle::new(text, DESIGN_LABEL_SIZE).centered().middle();
    surface.draw_text(name, bounds, &style);
}

/// The state a canvas shares with its painter, event mapper and timer.
struct CanvasState {
    /// The retained display list.
    commands: RefCell<Vec<DrawCmd>>,
    /// The colour painted under the list.
    background: Cell<Color>,
    /// The frame rate; 0 means no frame loop.
    fps: Cell<i64>,
    /// The keys held down, by script name.
    keys: RefCell<BTreeSet<&'static str>>,
    /// The pointer's last position, in logical pixels.
    mouse: Cell<(f64, f64)>,
    /// When the previous frame was raised.
    last_frame: Cell<Option<Instant>>,
}

impl Default for CanvasState {
    /// An empty list over black, with no frame loop.
    fn default() -> CanvasState {
        CanvasState {
            commands: RefCell::new(Vec::new()),
            background: Cell::new(Color::rgb(0, 0, 0)),
            fps: Cell::new(0),
            keys: RefCell::new(BTreeSet::new()),
            mouse: Cell::new((0.0, 0.0)),
            last_frame: Cell::new(None),
        }
    }
}

impl CanvasState {
    /// Records a key press; `true` when the key was not already down, so the
    /// press is new rather than an auto-repeat.
    fn press(&self, key: &'static str) -> bool {
        self.keys.borrow_mut().insert(key)
    }

    /// Records a key release; `true` when the key was down.
    fn release(&self, key: &'static str) -> bool {
        self.keys.borrow_mut().remove(key)
    }

    /// Forgets every held key (the canvas lost the focus).
    fn release_all(&self) {
        self.keys.borrow_mut().clear();
    }

    /// Whether the key named `key` (in any case) is held.
    fn is_down(&self, key: &str) -> bool {
        let key = key.to_ascii_lowercase();
        self.keys.borrow().contains(key.as_str())
    }

    /// Appends a command, refusing it once the list is full.
    fn push(&self, command: DrawCmd) -> Result<(), CallError> {
        let mut commands = self.commands.borrow_mut();
        if commands.len() >= MAX_COMMANDS {
            return Err(CallError::Failed(format!(
                "the canvas already holds {MAX_COMMANDS} drawing commands; call clear() \
                 at the start of each frame"
            )));
        }
        commands.push(command);
        Ok(())
    }

    /// Empties the list and starts it with a full fill of `color`.
    fn clear(&self, color: Color) {
        let mut commands = self.commands.borrow_mut();
        commands.clear();
        commands.push(DrawCmd::Clear(color));
    }
}

/// The input event handlers a canvas raises, as the binder supplied them.
struct InputHandlers<M: 'static> {
    key_down: Option<EventHandler<M>>,
    key_up: Option<EventHandler<M>>,
    mouse_down: Option<EventHandler<M>>,
    mouse_up: Option<EventHandler<M>>,
    mouse_move: Option<EventHandler<M>>,
}

/// What the factory hands the widget's constructor.
struct Config<M: 'static> {
    name: String,
    background: Color,
    fps: i64,
    design_mode: bool,
    frame: Option<EventHandler<M>>,
    input: InputHandlers<M>,
}

/// The live canvas: one custom node with a painter, an event mapper and an
/// optional frame timer.
pub(crate) struct CanvasWidget<M: 'static> {
    control: Control<M>,
    state: Rc<CanvasState>,
    frame: Option<EventHandler<M>>,
    timer: Cell<Option<TimerId>>,
    design_mode: bool,
}

impl<M: 'static> CanvasWidget<M> {
    /// Creates the canvas's node in `ui` and starts its frame loop if
    /// `config.fps` asks for one.
    fn new(ui: &Ui<M>, config: Config<M>) -> xui_core::backend::Result<CanvasWidget<M>> {
        let control = Control::new(
            ui,
            &NodeSpec::new(NodeKind::Custom, Rect::default()).tab_stop(),
        )?;
        let state = Rc::new(CanvasState::default());
        state.background.set(config.background);
        state.fps.set(config.fps.clamp(0, MAX_FPS));
        let design_mode = config.design_mode;

        {
            let state = Rc::clone(&state);
            let name = config.name;
            control.set_painter(Rc::new(move |surface: &mut dyn Surface| {
                let background = state.background.get();
                if design_mode {
                    paint_placeholder(surface, background, &name);
                    return;
                }
                // The list is only mutated in short scopes that never call
                // out, but a painter must not panic on a borrow regardless.
                match state.commands.try_borrow() {
                    Ok(commands) => paint_scene(surface, background, &commands),
                    Err(_) => paint_scene(surface, background, &[]),
                }
            }));
        }

        // A design-mode canvas raises nothing: the designer handles its input.
        if !design_mode {
            let state = Rc::clone(&state);
            let input = config.input;
            let ui = ui.clone();
            let id = control.id();
            control.on_events(move |event| {
                // A designer scope switched on after the build owns the input.
                if ui.is_design_mode() && event.is_input() {
                    return None;
                }
                map_event(&ui, id, &state, &input, event)
            });
        }

        let widget = CanvasWidget {
            control,
            state,
            frame: if design_mode { None } else { config.frame },
            timer: Cell::new(None),
            design_mode,
        };
        widget.restart_timer();
        Ok(widget)
    }

    /// The node identity.
    fn id(&self) -> WidgetId {
        self.control.id()
    }

    /// Stops any running frame timer and, when `fps` is above zero outside
    /// design mode and a `Frame` handler is bound, starts a new one.
    fn restart_timer(&self) {
        if let Some(timer) = self.timer.take() {
            self.control.kill_timer(timer);
        }
        let fps = self.state.fps.get();
        if self.design_mode || fps <= 0 {
            return;
        }
        let Some(frame) = self.frame.clone() else {
            return;
        };
        let interval = frame_interval(fps);
        self.state.last_frame.set(None);
        let state = Rc::clone(&self.state);
        let timer = self.control.set_timer(interval, move || {
            let now = Instant::now();
            let dt = frame_dt(state.last_frame.replace(Some(now)), now, interval);
            frame(&[Value::Float(dt)])
        });
        self.timer.set(timer);
    }

    /// Sets the frame rate and restarts the loop for it.
    fn set_fps(&self, fps: i64) {
        self.state.fps.set(fps.clamp(0, MAX_FPS));
        self.restart_timer();
    }

    /// Runs a drawing call that changes the list, then schedules a repaint.
    /// The list's borrow ends before the invalidation.
    fn draw(&self, change: impl FnOnce(&CanvasState) -> Result<(), CallError>) -> CallResult {
        change(&self.state)?;
        self.control.invalidate();
        Ok(None)
    }

    /// The width of `text` drawn at `size`, in logical pixels, measured by the
    /// backend's text shaper.
    fn text_width(&self, text: &str, size: f32) -> f64 {
        if text.is_empty() {
            return 0.0;
        }
        let dpi = self.control.dpi().max(1);
        let style = TextStyle::new(Color::rgb(0, 0, 0), Dip(size));
        let metrics = self.control.ui().measure_text(text, &style, dpi);
        f64::from(metrics.width) * 96.0 / f64::from(dpi)
    }

    /// Calls a method. The live form checked the arguments against the spec;
    /// each is still validated (finite numbers, sensible sizes) before the
    /// list changes, so a bad call leaves the drawing untouched.
    fn call(&self, method: &str, args: &[Value]) -> CallResult {
        let a = Args { method, args };
        match method {
            "clear" => {
                let color = a.color(0)?;
                self.draw(|state| {
                    state.clear(color);
                    Ok(())
                })
            }
            "fill_rect" => {
                let command = DrawCmd::FillRect(a.area(0)?, a.color(4)?);
                self.draw(|state| state.push(command))
            }
            "stroke_rect" => {
                let command = DrawCmd::StrokeRect(a.area(0)?, a.color(4)?, a.width(5)?);
                self.draw(|state| state.push(command))
            }
            "fill_round_rect" => {
                let command = DrawCmd::FillRoundRect(a.area(0)?, a.width(4)?, a.color(5)?);
                self.draw(|state| state.push(command))
            }
            "fill_circle" => {
                let command =
                    DrawCmd::FillCircle(a.number(0)?, a.number(1)?, a.width(2)?, a.color(3)?);
                self.draw(|state| state.push(command))
            }
            "stroke_circle" => {
                let command = DrawCmd::StrokeCircle(
                    a.number(0)?,
                    a.number(1)?,
                    a.width(2)?,
                    a.color(3)?,
                    a.width(4)?,
                );
                self.draw(|state| state.push(command))
            }
            "line" => {
                let points = [a.number(0)?, a.number(1)?, a.number(2)?, a.number(3)?];
                let command = DrawCmd::Line(points, a.color(4)?, a.width(5)?);
                self.draw(|state| state.push(command))
            }
            "text" => {
                let command = DrawCmd::Text(
                    a.number(0)?,
                    a.number(1)?,
                    a.text(2)?.to_owned(),
                    a.color(3)?,
                    a.text_size(4)?,
                );
                self.draw(|state| state.push(command))
            }
            "text_width" => {
                let width = self.text_width(a.text(0)?, a.text_size(1)?);
                Ok(Some(Value::Float(width)))
            }
            "is_key_down" => Ok(Some(Value::Bool(self.state.is_down(a.text(0)?)))),
            "focus" => {
                self.control.focus();
                Ok(None)
            }
            _ => Err(CallError::UnknownMethod),
        }
    }
}

/// What a method returns.
type CallResult = Result<Option<Value>, CallError>;

/// A method's arguments, with readers that validate each one.
struct Args<'a> {
    method: &'a str,
    args: &'a [Value],
}

impl Args<'_> {
    /// The error for argument `index` (zero-based; reported one-based).
    fn wrong(&self, index: usize, why: &str) -> CallError {
        CallError::WrongArgs(format!("{}: argument {} {why}", self.method, index + 1))
    }

    /// A finite number.
    fn number(&self, index: usize) -> Result<f32, CallError> {
        let value = self
            .args
            .get(index)
            .and_then(Value::as_float)
            .ok_or_else(|| self.wrong(index, "must be a number"))? as f32;
        if value.is_finite() {
            Ok(value)
        } else {
            Err(self.wrong(index, "must be a finite number"))
        }
    }

    /// A finite, non-negative number (a radius, a line width).
    fn width(&self, index: usize) -> Result<f32, CallError> {
        let value = self.number(index)?;
        if value < 0.0 {
            return Err(self.wrong(index, "must not be negative"));
        }
        Ok(value)
    }

    /// A text size: above zero and at most [`MAX_TEXT_SIZE`].
    fn text_size(&self, index: usize) -> Result<f32, CallError> {
        let value = self.number(index)?;
        if value <= 0.0 || value > MAX_TEXT_SIZE {
            return Err(self.wrong(index, "must be a size above 0 and up to 1000"));
        }
        Ok(value)
    }

    /// Four numbers starting at `index`: x, y, width, height.
    fn area(&self, index: usize) -> Result<Area, CallError> {
        Ok(Area::new(
            self.number(index)?,
            self.number(index + 1)?,
            self.number(index + 2)?,
            self.number(index + 3)?,
        ))
    }

    /// A colour.
    fn color(&self, index: usize) -> Result<Color, CallError> {
        self.args
            .get(index)
            .and_then(Value::as_color)
            .ok_or_else(|| self.wrong(index, "must be a colour"))
    }

    /// A string.
    fn text(&self, index: usize) -> Result<&str, CallError> {
        self.args
            .get(index)
            .and_then(Value::as_str)
            .ok_or_else(|| self.wrong(index, "must be text"))
    }
}

/// Turns one input event into the host's message, updating the key and
/// pointer state first. No borrow of `state` is held across a call into `ui`,
/// which can deliver focus events straight back into the mapper.
fn map_event<M: 'static>(
    ui: &Ui<M>,
    id: WidgetId,
    state: &CanvasState,
    input: &InputHandlers<M>,
    event: &Event,
) -> Option<M> {
    let dpi = f64::from(ui.dpi().max(1));
    let logical = |x: i32, y: i32| (f64::from(x) * 96.0 / dpi, f64::from(y) * 96.0 / dpi);
    let pointer = |handler: &Option<EventHandler<M>>, x: f64, y: f64, button: &str| {
        handler.as_ref().and_then(|handler| {
            handler(&[
                Value::Float(x),
                Value::Float(y),
                Value::Text(button.to_owned()),
            ])
        })
    };
    match *event {
        // A quick second click arrives as a double-click: a game wants it as
        // another press.
        Event::MouseDown { x, y, button, .. } | Event::MouseDoubleClick { x, y, button, .. } => {
            let (x, y) = logical(x, y);
            state.mouse.set((x, y));
            ui.focus(id);
            ui.set_capture(id);
            pointer(&input.mouse_down, x, y, button_name(button)?)
        }
        Event::MouseUp { x, y, button, .. } => {
            let (x, y) = logical(x, y);
            state.mouse.set((x, y));
            ui.release_capture();
            pointer(&input.mouse_up, x, y, button_name(button)?)
        }
        Event::MouseMove { x, y, .. } => {
            let (x, y) = logical(x, y);
            state.mouse.set((x, y));
            let handler = input.mouse_move.as_ref()?;
            handler(&[Value::Float(x), Value::Float(y)])
        }
        Event::KeyDown { key, .. } => {
            let name = key_name(key)?;
            // An auto-repeat finds the key already down and raises nothing.
            if !state.press(name) {
                return None;
            }
            let handler = input.key_down.as_ref()?;
            handler(&[Value::Text(name.to_owned())])
        }
        Event::KeyUp { key, .. } => {
            let name = key_name(key)?;
            state.release(name);
            let handler = input.key_up.as_ref()?;
            handler(&[Value::Text(name.to_owned())])
        }
        Event::KillFocus => {
            state.release_all();
            None
        }
        _ => None,
    }
}

/// The canvas fills whatever slot the form gives it; it has no natural size.
impl<M: 'static> Placeable<M> for CanvasWidget<M> {
    fn id(&self) -> WidgetId {
        CanvasWidget::id(self)
    }

    fn measure(&self, _ui: &Ui<M>, _constraints: Constraints) -> Size {
        Size::new(0, 0)
    }
}

/// `Canvas`: a drawing surface with a frame loop and raw input.
pub(crate) struct CanvasFactory;

impl<M: 'static> WidgetFactory<M> for CanvasFactory {
    fn kind(&self) -> &str {
        KIND
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, node: &Node) -> Made<M> {
        let canvas = Handle::new();
        let config = Config {
            name: node.name.clone(),
            background: cx
                .prop("background")
                .and_then(Value::as_color)
                .unwrap_or(Color::rgb(0, 0, 0)),
            fps: cx.int("fps", 0),
            design_mode: cx.design_mode(),
            frame: cx.handler(FRAME_EVENT),
            input: InputHandlers {
                key_down: cx.handler("KeyDown"),
                key_up: cx.handler("KeyUp"),
                mouse_down: cx.handler("MouseDown"),
                mouse_up: cx.handler("MouseUp"),
                mouse_move: cx.handler("MouseMove"),
            },
        };
        let build = arrange::build(move |ui: &Ui<M>| CanvasWidget::new(ui, config)).bind(&canvas);
        cx.made(build, &canvas.clone(), CanvasProps { canvas })
    }
}

/// The canvas's own property and method surface.
struct CanvasProps<M: 'static> {
    canvas: Handle<CanvasWidget<M>>,
}

impl<M: 'static> WidgetProps<M> for CanvasProps<M> {
    fn id(&self) -> WidgetId {
        self.canvas.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        let canvas = self.canvas.get();
        let state = &canvas.state;
        match prop {
            "background" => Some(Value::Color(state.background.get())),
            "fps" => Some(Value::Int(state.fps.get())),
            "mouse_x" => Some(Value::Float(state.mouse.get().0)),
            "mouse_y" => Some(Value::Float(state.mouse.get().1)),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        let canvas = self.canvas.get();
        match (prop, value) {
            ("background", Value::Color(color)) => {
                canvas.state.background.set(*color);
                canvas.control.invalidate();
                Ok(())
            }
            ("fps", Value::Int(fps)) => {
                canvas.set_fps(*fps);
                Ok(())
            }
            ("mouse_x" | "mouse_y", _) => Err(SetError::ReadOnly),
            ("background" | "fps", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn call_own(&self, method: &str, args: &[Value]) -> CallResult {
        self.canvas.get().call(method, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn keys_have_script_names() {
        for (key, name) in [
            (Key::LEFT, "left"),
            (Key::RIGHT, "right"),
            (Key::UP, "up"),
            (Key::DOWN, "down"),
            (Key::SPACE, "space"),
            (Key::RETURN, "enter"),
            (Key::ESCAPE, "escape"),
            (Key::A, "a"),
            (Key::P, "p"),
            (Key::Z, "z"),
            (Key::DIGIT0, "0"),
            (Key::DIGIT9, "9"),
            (Key::F1, "f1"),
            (Key::F12, "f12"),
            (Key::PAGE_DOWN, "page_down"),
            (Key::MENU, "alt"),
        ] {
            assert_eq!(key_name(key), Some(name), "{key:?}");
        }
        // A key a canvas does not report (VK_LWIN).
        assert_eq!(key_name(Key::from_code(0x5B)), None);
        // Every name is lower-case ASCII, so `is_key_down` can fold case.
        for code in 0..=0xFF {
            if let Some(name) = key_name(Key::from_code(code)) {
                assert!(name.is_ascii(), "{code:#x}");
                assert_eq!(name, name.to_ascii_lowercase(), "{code:#x}");
            }
        }
    }

    #[test]
    fn mouse_buttons_have_script_names() {
        assert_eq!(button_name(MouseButton::Left), Some("left"));
        assert_eq!(button_name(MouseButton::Right), Some("right"));
        assert_eq!(button_name(MouseButton::Middle), Some("middle"));
        assert_eq!(button_name(MouseButton::X1), None);
    }

    #[test]
    fn a_repeat_does_not_press_again_and_focus_loss_releases_every_key() {
        let state = CanvasState::default();
        assert!(state.press("left"), "the first press is new");
        assert!(!state.press("left"), "an auto-repeat is not a new press");
        assert!(state.is_down("left"));
        assert!(state.is_down("LEFT"), "key names fold case");
        assert!(state.press("space"));
        state.release_all();
        assert!(!state.is_down("left"), "focus loss forgets held keys");
        assert!(!state.is_down("space"));
        assert!(
            state.press("left"),
            "a key pressed after focus returns is new"
        );
        assert!(state.release("left"));
        assert!(!state.release("left"));
    }

    #[test]
    fn dt_is_measured_and_clamped() {
        let start = Instant::now();
        assert_eq!(
            frame_dt(None, start, 16),
            0.016,
            "the first frame is nominal"
        );
        let later = start + Duration::from_millis(20);
        assert!((frame_dt(Some(start), later, 16) - 0.020).abs() < 1e-9);
        let stalled = start + Duration::from_secs(3);
        assert_eq!(frame_dt(Some(start), stalled, 16), MAX_FRAME_DT);
        // A clock that went backwards reports zero, not a negative dt.
        assert_eq!(frame_dt(Some(later), start, 16), 0.0);
    }

    #[test]
    fn the_interval_follows_fps() {
        assert_eq!(frame_interval(60), 17);
        assert_eq!(frame_interval(30), 33);
        assert_eq!(frame_interval(1), 1000);
        assert_eq!(frame_interval(240), 4);
        assert_eq!(frame_interval(10_000), 4, "fps is capped");
    }

    #[test]
    fn clear_resets_the_list_and_push_appends() {
        let state = CanvasState::default();
        let red = Color::rgb(255, 0, 0);
        state
            .push(DrawCmd::FillRect(Area::new(0.0, 0.0, 1.0, 1.0), red))
            .expect("room");
        state
            .push(DrawCmd::FillCircle(1.0, 1.0, 1.0, red))
            .expect("room");
        assert_eq!(state.commands.borrow().len(), 2);
        state.clear(Color::rgb(0, 0, 0));
        assert_eq!(
            *state.commands.borrow(),
            vec![DrawCmd::Clear(Color::rgb(0, 0, 0))]
        );
    }

    #[test]
    fn a_full_list_refuses_more_commands() {
        let state = CanvasState::default();
        let red = Color::rgb(255, 0, 0);
        state
            .commands
            .borrow_mut()
            .resize(MAX_COMMANDS, DrawCmd::Clear(red));
        let error = state
            .push(DrawCmd::Clear(red))
            .expect_err("the list is full");
        assert!(matches!(error, CallError::Failed(_)));
        assert_eq!(state.commands.borrow().len(), MAX_COMMANDS);
    }

    #[test]
    fn a_negative_size_extends_the_other_way() {
        let area = Area::new(10.0, 10.0, -4.0, -6.0);
        assert_eq!(
            area,
            Area {
                x: 6.0,
                y: 4.0,
                w: 4.0,
                h: 6.0
            }
        );
        assert_eq!(area.to_px(1.5), Rect::new(9, 6, 15, 15));
    }

    #[test]
    fn bad_arguments_are_rejected_before_drawing() {
        let red = Value::Color(Color::rgb(255, 0, 0));
        let args = [
            Value::Float(f64::NAN),
            Value::Int(0),
            Value::Int(1),
            Value::Int(1),
            red,
        ];
        let reader = Args {
            method: "fill_rect",
            args: &args,
        };
        assert!(matches!(reader.area(0), Err(CallError::WrongArgs(_))));
        let args = [Value::Float(-1.0)];
        let reader = Args {
            method: "fill_circle",
            args: &args,
        };
        assert!(reader.width(0).is_err(), "a negative radius is refused");
        let args = [Value::Float(0.0)];
        let reader = Args {
            method: "text",
            args: &args,
        };
        assert!(reader.text_size(0).is_err(), "a zero text size is refused");
    }

    #[test]
    fn the_spec_lists_the_methods_and_frame_is_the_default_event() {
        let spec = spec();
        assert_eq!(
            spec.default_event().map(|event| event.name.as_str()),
            Some(FRAME_EVENT)
        );
        for name in [
            "clear",
            "fill_rect",
            "stroke_rect",
            "fill_round_rect",
            "fill_circle",
            "stroke_circle",
            "line",
            "text",
            "text_width",
            "is_key_down",
            "focus",
        ] {
            assert!(spec.method(name).is_some(), "{name}");
        }
        assert_eq!(
            spec.method("text_width").and_then(|m| m.returns.clone()),
            Some(float_type())
        );
        assert_eq!(spec.default_size, (Dip(320.0), Dip(240.0)));
    }
}
