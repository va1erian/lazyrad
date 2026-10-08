#![forbid(unsafe_code)]

//! `PictureBox`: a control that shows a picture, with best fit, zoom,
//! rotation and panning, for picture viewers and image previews.
//!
//! # Loading
//!
//! The control never opens a file. A script reads the bytes through the
//! standard library (`file_read_bytes`, which applies the app's sandbox) and
//! hands them over: `picture1.load(file_read_bytes(path))`. PNG, JPEG, BMP and
//! GIF (its first frame) are recognised by their signature ([`decode`]); a file
//! that is not a picture, is damaged or is larger than [`decode::MAX_PIXELS`]
//! is a script error and leaves the shown picture unchanged.
//!
//! # Sizing
//!
//! With `fit` on (the default) a picture larger than the box is shrunk to fit
//! it and a smaller one is shown at its own size, centred; `stretch` also
//! enlarges a smaller one. Writing `zoom` (a percentage of the picture's own
//! pixels, where 100 is one picture pixel per screen pixel) turns `fit` off;
//! reading it gives the zoom being shown, so a status bar can display it while
//! fitted. `zoom_in()`/`zoom_out()` walk the usual viewer steps
//! ([`view::ZOOM_STEPS`]), `best_fit()` and `actual_size()` switch modes.
//!
//! # Rotation and panning
//!
//! `rotation` (0, 90, 180 or 270 degrees clockwise) turns the shown picture;
//! `rotate_cw()`/`rotate_ccw()` step it. `to_png()` returns the picture as
//! shown, rotation included, so a script can save it. A picture larger than
//! the box can be dragged with the left button; it never moves past its edges.
//!
//! # Events
//!
//! `KeyDown(key)` (the [`Canvas`](crate::canvas) key names; auto-repeat raises
//! it again, so a held arrow keeps paging), `Wheel(delta, ctrl)` with the wheel
//! notches (positive away from the user) and whether Ctrl was held,
//! `DoubleClick(x, y)` in logical pixels and `Click(x, y)` for a press and
//! release without a drag. A script maps them to its own commands (the
//! classic viewer pages with the wheel and zooms with Ctrl+wheel).

pub mod decode;
pub mod view;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use xui_core::app::Ui;
use xui_core::arrange::{self, Handle};
use xui_core::backend::{Canvas as Surface, Cursor, Event, NodeKind, NodeSpec, TextStyle};
use xui_core::geometry::{Rect, Size};
use xui_core::image::Image;
use xui_core::layout::Constraints;
use xui_core::message::MouseButton;
use xui_core::units::Dip;
use xui_core::widget::{Control, Placeable};
use xui_core::{Color, WidgetId};

use crate::build::{BuildCx, CallError, EventHandler, Made, SetError, WidgetFactory};
use crate::canvas::key_name;
use crate::doc::Node;
use crate::live::WidgetProps;
use crate::schema::{
    Access, ArgSpec, CATEGORY_APPEARANCE, CATEGORY_BEHAVIOR, CATEGORY_DATA, Children, MethodSpec,
    PropertySpec, WidgetSpec, arg, event, float_type, property, text_type, widget,
};
use crate::value::{Value, ValueType};

use view::Sizing;

/// The canonical kind name.
pub const KIND: &str = "PictureBox";

/// How far (in device pixels) the pointer may move between press and release
/// and still count as a click rather than a drag.
const CLICK_SLOP: i32 = 4;

/// The size of the placeholder text (the design-mode name, "No picture").
const PLACEHOLDER_SIZE: Dip = Dip(12.0);

/// The picture box spec for the catalog.
pub(crate) fn spec() -> WidgetSpec {
    let xy = || vec![arg("x", float_type()), arg("y", float_type())];
    WidgetSpec {
        properties: vec![
            property(
                "background",
                ValueType::Color,
                Value::Color(Color::rgb(0xFF, 0xFF, 0xFF)),
                CATEGORY_APPEARANCE,
                "The colour around (and under transparent parts of) the picture.",
            ),
            property(
                "fit",
                ValueType::Bool,
                Value::Bool(true),
                CATEGORY_BEHAVIOR,
                "Best fit: shrink a picture larger than the box to fit it.",
            ),
            property(
                "stretch",
                ValueType::Bool,
                Value::Bool(false),
                CATEGORY_BEHAVIOR,
                "With fit, also enlarge a picture smaller than the box.",
            ),
            property(
                "zoom",
                ValueType::Float {
                    min: Some(view::MIN_ZOOM),
                    max: Some(view::MAX_ZOOM),
                },
                Value::Float(100.0),
                CATEGORY_BEHAVIOR,
                "The zoom in percent; setting it turns fit off. Reads the zoom shown.",
            ),
            property(
                "rotation",
                ValueType::Int {
                    min: Some(0),
                    max: Some(270),
                },
                Value::Int(0),
                CATEGORY_BEHAVIOR,
                "Clockwise rotation in degrees: 0, 90, 180 or 270.",
            ),
            read_only(
                "image_width",
                ValueType::Int {
                    min: None,
                    max: None,
                },
                Value::Int(0),
                "The picture's width in pixels, before rotation (0 when empty).",
            ),
            read_only(
                "image_height",
                ValueType::Int {
                    min: None,
                    max: None,
                },
                Value::Int(0),
                "The picture's height in pixels, before rotation (0 when empty).",
            ),
            read_only(
                "has_image",
                ValueType::Bool,
                Value::Bool(false),
                "Whether a picture is loaded.",
            ),
            read_only(
                "format",
                text_type(),
                Value::Text(String::new()),
                "The loaded picture's format: PNG, JPEG, BMP or GIF (empty when none).",
            ),
        ],
        events: vec![
            event(
                "KeyDown",
                vec![arg("key", text_type())],
                true,
                "Raised when a key is pressed while the picture box has the focus.",
            ),
            event(
                "Wheel",
                vec![arg("delta", float_type()), arg("ctrl", ValueType::Bool)],
                false,
                "Raised when the wheel turns: notches (positive away from the user) and Ctrl.",
            ),
            event(
                "Click",
                xy(),
                false,
                "Raised for a left click that did not drag.",
            ),
            event(
                "DoubleClick",
                xy(),
                false,
                "Raised for a left double-click.",
            ),
        ],
        methods: methods(),
        ..widget(
            KIND,
            "Shows a picture with best fit, zoom, rotation and panning.",
            (320.0, 240.0),
            Children::None,
        )
    }
}

/// A read-only data property.
fn read_only(name: &str, ty: ValueType, default: Value, description: &str) -> PropertySpec {
    PropertySpec {
        access: Access::ReadOnly,
        ..property(name, ty, default, CATEGORY_DATA, description)
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

/// The picture box's methods, in the order the docs list them.
fn methods() -> Vec<MethodSpec> {
    let none = Vec::new;
    vec![
        method(
            "load",
            vec![arg("data", ValueType::Bytes)],
            None,
            "Shows the picture in `data` (a blob from file_read_bytes); an error if it is not one.",
        ),
        method("clear", none(), None, "Removes the picture."),
        method(
            "best_fit",
            none(),
            None,
            "Turns fit on and centres the picture.",
        ),
        method("actual_size", none(), None, "Shows the picture at 100%."),
        method("zoom_in", none(), None, "Zooms in one step."),
        method("zoom_out", none(), None, "Zooms out one step."),
        method(
            "rotate_cw",
            none(),
            None,
            "Turns the picture 90 degrees clockwise.",
        ),
        method(
            "rotate_ccw",
            none(),
            None,
            "Turns the picture 90 degrees anticlockwise.",
        ),
        method(
            "to_png",
            none(),
            Some(ValueType::Bytes),
            "The picture as shown (rotation included) encoded as PNG, for file_write_bytes.",
        ),
        method(
            "focus",
            none(),
            None,
            "Gives the picture box the keyboard focus.",
        ),
    ]
}

/// The picture a box shows, decoded once and rotated on demand.
struct Picture {
    /// The decoded picture, unrotated.
    original: Image,
    /// `original` turned by the current rotation (the same image at 0).
    shown: Image,
    format: decode::Format,
}

/// A left-button drag: where the press was (device pixels) and the pan then.
type Drag = ((i32, i32), (f64, f64));

/// The state a picture box shares with its painter and event mapper.
struct PictureState {
    picture: RefCell<Option<Picture>>,
    background: Cell<Color>,
    fit: Cell<bool>,
    stretch: Cell<bool>,
    /// The zoom used while `fit` is off, in percent.
    zoom: Cell<f64>,
    /// Quarter turns clockwise, 0..=3.
    quarters: Cell<u8>,
    /// The pan offset from centred, in device pixels.
    pan: Cell<(f64, f64)>,
    /// The size of the box at the last paint, in device pixels.
    view: Cell<(f64, f64)>,
    /// A left-button drag in progress.
    drag: Cell<Option<Drag>>,
    /// Whether the current press moved far enough to be a drag.
    dragged: Cell<bool>,
}

impl Default for PictureState {
    fn default() -> PictureState {
        PictureState {
            picture: RefCell::new(None),
            background: Cell::new(Color::rgb(0xFF, 0xFF, 0xFF)),
            fit: Cell::new(true),
            stretch: Cell::new(false),
            zoom: Cell::new(100.0),
            quarters: Cell::new(0),
            pan: Cell::new((0.0, 0.0)),
            view: Cell::new((0.0, 0.0)),
            drag: Cell::new(None),
            dragged: Cell::new(false),
        }
    }
}

impl PictureState {
    /// The shown picture's size (after rotation), if any.
    fn shown_size(&self) -> Option<(u32, u32)> {
        self.picture.borrow().as_ref().map(|p| p.shown.size())
    }

    /// How the picture is sized now.
    fn sizing(&self) -> Sizing {
        if self.fit.get() {
            Sizing::Fit {
                enlarge: self.stretch.get(),
            }
        } else {
            Sizing::Zoom(self.zoom.get())
        }
    }

    /// The zoom shown, in percent (100 with no picture or no size yet).
    fn effective_zoom(&self) -> f64 {
        let Some((w, h)) = self.shown_size() else {
            return if self.fit.get() {
                100.0
            } else {
                self.zoom.get()
            };
        };
        view::scale(self.sizing(), self.view.get(), (f64::from(w), f64::from(h))) * 100.0
    }

    /// Switches to a fixed zoom, keeping the same point of the picture in the
    /// centre of the box.
    fn set_zoom(&self, percent: f64) {
        let percent = percent.clamp(view::MIN_ZOOM, view::MAX_ZOOM);
        let before = self.effective_zoom();
        let (x, y) = self.pan.get();
        let ratio = if before > 0.0 { percent / before } else { 1.0 };
        self.pan.set((x * ratio, y * ratio));
        self.zoom.set(percent);
        self.fit.set(false);
    }

    /// Sets the rotation in quarter turns, re-rotating the cached picture.
    fn set_quarters(&self, quarters: u8) {
        let quarters = quarters % 4;
        self.quarters.set(quarters);
        self.pan.set((0.0, 0.0));
        if let Some(picture) = self.picture.borrow_mut().as_mut() {
            picture.shown = view::rotate(&picture.original, quarters);
        }
    }

    /// Replaces the picture; the rotation and pan start afresh.
    fn show(&self, original: Image, format: decode::Format) {
        self.quarters.set(0);
        self.pan.set((0.0, 0.0));
        *self.picture.borrow_mut() = Some(Picture {
            shown: original.clone(),
            original,
            format,
        });
    }
}

/// The event handlers a picture box raises.
struct Handlers<M: 'static> {
    key_down: Option<EventHandler<M>>,
    wheel: Option<EventHandler<M>>,
    click: Option<EventHandler<M>>,
    double_click: Option<EventHandler<M>>,
}

/// What the factory hands the widget's constructor.
struct Config<M: 'static> {
    name: String,
    design_mode: bool,
    background: Color,
    fit: bool,
    stretch: bool,
    zoom: f64,
    rotation: i64,
    handlers: Handlers<M>,
}

/// The live picture box: one custom node with a painter and event mapper.
pub(crate) struct PictureWidget<M: 'static> {
    control: Control<M>,
    state: Rc<PictureState>,
}

impl<M: 'static> PictureWidget<M> {
    fn new(ui: &Ui<M>, config: Config<M>) -> xui_core::backend::Result<PictureWidget<M>> {
        let control = Control::new(
            ui,
            &NodeSpec::new(NodeKind::Custom, Rect::default()).tab_stop(),
        )?;
        let state = Rc::new(PictureState::default());
        state.background.set(config.background);
        state.fit.set(config.fit);
        state.stretch.set(config.stretch);
        state
            .zoom
            .set(config.zoom.clamp(view::MIN_ZOOM, view::MAX_ZOOM));
        state
            .quarters
            .set(quarters_of(config.rotation).unwrap_or(0));
        let design_mode = config.design_mode;
        {
            let state = Rc::clone(&state);
            let name = config.name;
            control.set_painter(Rc::new(move |surface: &mut dyn Surface| {
                paint(surface, &state, design_mode.then_some(name.as_str()));
            }));
        }
        if !design_mode {
            let state = Rc::clone(&state);
            let handlers = config.handlers;
            let ui = ui.clone();
            let id = control.id();
            control.on_events(move |event| {
                if ui.is_design_mode() && event.is_input() {
                    return None;
                }
                map_event(&ui, id, &state, &handlers, event)
            });
        }
        Ok(PictureWidget { control, state })
    }

    fn id(&self) -> WidgetId {
        self.control.id()
    }

    /// Records the box's current size, so a zoom read or step before the
    /// next paint (or before the first) uses the real view.
    fn sync_view(&self) {
        let bounds = self.control.bounds();
        if bounds.width() > 0 && bounds.height() > 0 {
            self.state
                .view
                .set((f64::from(bounds.width()), f64::from(bounds.height())));
        }
    }

    /// Runs a change to the view, then repaints.
    fn change(&self, change: impl FnOnce(&PictureState)) -> CallResult {
        change(&self.state);
        self.control.invalidate();
        Ok(None)
    }

    fn call(&self, method: &str, args: &[Value]) -> CallResult {
        self.sync_view();
        let state = &self.state;
        match method {
            "load" => {
                let bytes = args.first().and_then(Value::as_bytes).ok_or_else(|| {
                    CallError::WrongArgs("load: argument 1 must be a blob".into())
                })?;
                let (image, format) = decode::decode(bytes)
                    .map_err(|error| CallError::Failed(format!("load: {error}")))?;
                self.change(|state| state.show(image, format))
            }
            "clear" => self.change(|state| {
                state.picture.borrow_mut().take();
            }),
            "best_fit" => self.change(|state| {
                state.fit.set(true);
                state.pan.set((0.0, 0.0));
            }),
            "actual_size" => self.change(|state| state.set_zoom(100.0)),
            "zoom_in" => {
                let next = view::step_zoom(state.effective_zoom(), true);
                self.change(|state| state.set_zoom(next))
            }
            "zoom_out" => {
                let next = view::step_zoom(state.effective_zoom(), false);
                self.change(|state| state.set_zoom(next))
            }
            "rotate_cw" => self.change(|state| state.set_quarters(state.quarters.get() + 1)),
            "rotate_ccw" => self.change(|state| state.set_quarters(state.quarters.get() + 3)),
            "to_png" => {
                let picture = state.picture.borrow();
                let picture = picture
                    .as_ref()
                    .ok_or_else(|| CallError::Failed("to_png: no picture is loaded".into()))?;
                let bytes = picture
                    .shown
                    .encode_png()
                    .map_err(|error| CallError::Failed(format!("to_png: {error}")))?;
                Ok(Some(Value::Bytes(bytes.into())))
            }
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

/// The quarter turns for a rotation in degrees, if it is a multiple of 90.
fn quarters_of(degrees: i64) -> Option<u8> {
    (degrees % 90 == 0).then(|| (degrees.rem_euclid(360) / 90) as u8)
}

/// Paints the box: the background, then the picture (or a placeholder).
fn paint(surface: &mut dyn Surface, state: &PictureState, design_name: Option<&str>) {
    let bounds = surface.bounds();
    let background = state.background.get();
    surface.push_clip(bounds);
    surface.fill_rect(bounds, background);
    state
        .view
        .set((f64::from(bounds.width()), f64::from(bounds.height())));
    let picture = state.picture.try_borrow();
    match (design_name, picture.as_ref().ok().and_then(|p| p.as_ref())) {
        (Some(name), _) => placeholder(surface, background, name),
        (None, Some(picture)) => {
            let size = picture.shown.size();
            let scale = view::scale(
                state.sizing(),
                state.view.get(),
                (f64::from(size.0), f64::from(size.1)),
            );
            let rect = view::placement(bounds, scale, size, state.pan.get());
            surface.draw_image(&picture.shown, rect);
        }
        (None, None) => {}
    }
    surface.pop_clip();
}

/// The design-mode look: a border and the control's name.
fn placeholder(surface: &mut dyn Surface, background: Color, text: &str) {
    let bounds = surface.bounds();
    let luminance = 299 * u32::from(background.r)
        + 587 * u32::from(background.g)
        + 114 * u32::from(background.b);
    let ink = if luminance < 128_000 {
        Color::rgb(0xd0, 0xd0, 0xd0)
    } else {
        Color::rgb(0x50, 0x50, 0x50)
    };
    surface.stroke_rect(bounds, ink, 1.0);
    let style = TextStyle::new(ink, PLACEHOLDER_SIZE).centered().middle();
    surface.draw_text(text, bounds, &style);
}

/// Turns one input event into the host's message, panning on a drag.
fn map_event<M: 'static>(
    ui: &Ui<M>,
    id: WidgetId,
    state: &PictureState,
    handlers: &Handlers<M>,
    event: &Event,
) -> Option<M> {
    let dpi = f64::from(ui.dpi().max(1));
    let logical = |v: i32| f64::from(v) * 96.0 / dpi;
    let raise = |handler: &Option<EventHandler<M>>, args: &[Value]| {
        handler.as_ref().and_then(|handler| handler(args))
    };
    match *event {
        Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            ..
        } => {
            ui.focus(id);
            ui.set_capture(id);
            state.drag.set(Some(((x, y), state.pan.get())));
            state.dragged.set(false);
            None
        }
        Event::MouseMove { x, y, .. } => {
            let ((x0, y0), (px, py)) = state.drag.get()?;
            if (x - x0).abs() > CLICK_SLOP || (y - y0).abs() > CLICK_SLOP {
                state.dragged.set(true);
            }
            if state.dragged.get() {
                let size = state.shown_size()?;
                let scale = view::scale(
                    state.sizing(),
                    state.view.get(),
                    (f64::from(size.0), f64::from(size.1)),
                );
                let drawn = (f64::from(size.0) * scale, f64::from(size.1) * scale);
                let wanted = (px + f64::from(x - x0), py + f64::from(y - y0));
                state
                    .pan
                    .set(view::clamp_pan(wanted, state.view.get(), drawn));
                ui.set_cursor(id, Cursor::Hand);
                ui.invalidate(id);
            }
            None
        }
        Event::MouseUp {
            x,
            y,
            button: MouseButton::Left,
            ..
        } => {
            // Read the drag first: releasing the capture delivers
            // `CaptureChanged`, which forgets it.
            let was_click = state.drag.take().is_some() && !state.dragged.get();
            ui.release_capture();
            ui.set_cursor(id, Cursor::Default);
            if was_click {
                raise(
                    &handlers.click,
                    &[Value::Float(logical(x)), Value::Float(logical(y))],
                )
            } else {
                None
            }
        }
        Event::MouseDoubleClick {
            x,
            y,
            button: MouseButton::Left,
            ..
        } => raise(
            &handlers.double_click,
            &[Value::Float(logical(x)), Value::Float(logical(y))],
        ),
        Event::MouseWheel {
            delta,
            horizontal: false,
            modifiers,
            ..
        } => raise(
            &handlers.wheel,
            &[Value::Float(f64::from(delta)), Value::Bool(modifiers.ctrl)],
        ),
        Event::KeyDown { key, .. } => {
            let name = key_name(key)?;
            raise(&handlers.key_down, &[Value::Text(name.to_owned())])
        }
        Event::CaptureChanged => {
            state.drag.set(None);
            None
        }
        _ => None,
    }
}

/// The box fills whatever slot the form gives it.
impl<M: 'static> Placeable<M> for PictureWidget<M> {
    fn id(&self) -> WidgetId {
        PictureWidget::id(self)
    }

    fn measure(&self, _ui: &Ui<M>, _constraints: Constraints) -> Size {
        Size::new(0, 0)
    }
}

/// `PictureBox`: shows a picture.
pub(crate) struct PictureFactory;

impl<M: 'static> WidgetFactory<M> for PictureFactory {
    fn kind(&self) -> &str {
        KIND
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, node: &Node) -> Made<M> {
        let picture = Handle::new();
        let config = Config {
            name: node.name.clone(),
            design_mode: cx.design_mode(),
            background: cx
                .prop("background")
                .and_then(Value::as_color)
                .unwrap_or(Color::rgb(0xFF, 0xFF, 0xFF)),
            fit: cx.bool("fit", true),
            stretch: cx.bool("stretch", false),
            zoom: cx.float("zoom", 100.0),
            rotation: cx.int("rotation", 0),
            handlers: Handlers {
                key_down: cx.handler("KeyDown"),
                wheel: cx.handler("Wheel"),
                click: cx.handler("Click"),
                double_click: cx.handler("DoubleClick"),
            },
        };
        let build = arrange::build(move |ui: &Ui<M>| PictureWidget::new(ui, config)).bind(&picture);
        cx.made(build, &picture.clone(), PictureProps { picture })
    }
}

/// The picture box's own property and method surface.
struct PictureProps<M: 'static> {
    picture: Handle<PictureWidget<M>>,
}

impl<M: 'static> WidgetProps<M> for PictureProps<M> {
    fn id(&self) -> WidgetId {
        self.picture.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        let widget = self.picture.get();
        widget.sync_view();
        let state = &widget.state;
        let picture = state.picture.borrow();
        let size = picture.as_ref().map_or((0, 0), |p| p.original.size());
        match prop {
            "background" => Some(Value::Color(state.background.get())),
            "fit" => Some(Value::Bool(state.fit.get())),
            "stretch" => Some(Value::Bool(state.stretch.get())),
            "zoom" => {
                drop(picture);
                // Two decimals: enough for a status bar, stable in tests.
                Some(Value::Float(
                    (state.effective_zoom() * 100.0).round() / 100.0,
                ))
            }
            "rotation" => Some(Value::Int(i64::from(state.quarters.get()) * 90)),
            "image_width" => Some(Value::Int(i64::from(size.0))),
            "image_height" => Some(Value::Int(i64::from(size.1))),
            "has_image" => Some(Value::Bool(picture.is_some())),
            "format" => Some(Value::Text(
                picture.as_ref().map_or("", |p| p.format.name()).to_owned(),
            )),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        let widget = self.picture.get();
        widget.sync_view();
        let state = &widget.state;
        match (prop, value) {
            ("background", Value::Color(color)) => state.background.set(*color),
            ("fit", Value::Bool(fit)) => {
                state.fit.set(*fit);
                state.pan.set((0.0, 0.0));
            }
            ("stretch", Value::Bool(stretch)) => state.stretch.set(*stretch),
            ("zoom", value) if value.as_float().is_some_and(f64::is_finite) => {
                state.set_zoom(value.as_float().unwrap_or(100.0));
            }
            ("rotation", Value::Int(degrees)) => {
                state.set_quarters(quarters_of(*degrees).ok_or(SetError::TypeMismatch)?);
            }
            ("image_width" | "image_height" | "has_image" | "format", _) => {
                return Err(SetError::ReadOnly);
            }
            ("background" | "fit" | "stretch" | "zoom" | "rotation", _) => {
                return Err(SetError::TypeMismatch);
            }
            _ => return Err(SetError::UnknownProperty),
        }
        widget.control.invalidate();
        Ok(())
    }

    fn call_own(&self, method: &str, args: &[Value]) -> CallResult {
        self.picture.get().call(method, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32) -> Image {
        Image::from_rgba(w, h, vec![0x80; (w * h * 4) as usize]).unwrap()
    }

    #[test]
    fn rotation_accepts_only_quarter_turns() {
        assert_eq!(quarters_of(0), Some(0));
        assert_eq!(quarters_of(90), Some(1));
        assert_eq!(quarters_of(270), Some(3));
        assert_eq!(quarters_of(360), Some(0));
        assert_eq!(quarters_of(-90), Some(3));
        assert_eq!(quarters_of(45), None);
    }

    #[test]
    fn the_shown_zoom_follows_fit_and_the_view() {
        let state = PictureState::default();
        state.view.set((400.0, 300.0));
        state.show(image(800, 600), decode::Format::Png);
        assert_eq!(state.effective_zoom(), 50.0, "fitted");
        state.set_zoom(200.0);
        assert!(!state.fit.get(), "a zoom turns fit off");
        assert_eq!(state.effective_zoom(), 200.0);
    }

    #[test]
    fn zooming_scales_the_pan_and_rotating_resets_it() {
        let state = PictureState::default();
        state.view.set((100.0, 100.0));
        state.show(image(1000, 1000), decode::Format::Png);
        state.set_zoom(100.0);
        state.pan.set((40.0, -20.0));
        state.set_zoom(200.0);
        assert_eq!(state.pan.get(), (80.0, -40.0));
        state.set_quarters(1);
        assert_eq!(state.pan.get(), (0.0, 0.0));
    }

    #[test]
    fn rotating_turns_the_shown_picture_only() {
        let state = PictureState::default();
        state.show(image(4, 2), decode::Format::Bmp);
        state.set_quarters(state.quarters.get() + 1);
        assert_eq!(state.shown_size(), Some((2, 4)));
        state.set_quarters(state.quarters.get() + 3);
        assert_eq!(state.shown_size(), Some((4, 2)));
        let picture = state.picture.borrow();
        assert_eq!(picture.as_ref().map(|p| p.original.size()), Some((4, 2)));
    }

    #[test]
    fn a_new_picture_starts_unrotated() {
        let state = PictureState::default();
        state.show(image(4, 2), decode::Format::Png);
        state.set_quarters(1);
        state.show(image(3, 3), decode::Format::Gif);
        assert_eq!(state.quarters.get(), 0);
    }

    #[test]
    fn the_spec_lists_the_methods_and_key_down_is_the_default_event() {
        let spec = spec();
        assert_eq!(
            spec.default_event().map(|event| event.name.as_str()),
            Some("KeyDown")
        );
        for name in [
            "load",
            "clear",
            "best_fit",
            "actual_size",
            "zoom_in",
            "zoom_out",
            "rotate_cw",
            "rotate_ccw",
            "to_png",
            "focus",
        ] {
            assert!(spec.method(name).is_some(), "{name}");
        }
        assert_eq!(
            spec.method("load").map(|m| m.args[0].ty.clone()),
            Some(ValueType::Bytes)
        );
    }
}
