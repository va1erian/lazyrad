//! Offscreen tests of the `Canvas` control: its frame timer, input, display
//! list and DPI scaling, and that canvases never share state.
//!
//! The offscreen backend starts no timers, so [`TimerBackend`] wraps it: it
//! hands out timer ids, records which are live and at what interval, fires
//! them on request through the window's event sink, and counts repaint
//! requests. Everything else is forwarded unchanged.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use xui_canvas::OffscreenBackend;
use xui_core::app::{App, Ui, run_app};
use xui_core::arrange::{Handle, LayoutExt, absolute, panel};
use xui_core::backend::{
    Backend, Cursor, Event, FontSpec, ImplKind, NativeWindowHandle, NodeKind, NodeSpec, Painter,
    ParentRef, PlatformSpec, Result as BackendResult, TextLayout, TextMetrics, TextShaper,
    TextStyle, TimerId, Waker, WidgetId, WindowId,
};
use xui_core::image::Image;
use xui_core::message::{Key, Modifiers, MouseButton};
use xui_core::router::WidgetHost;
use xui_core::{Color, Dip, Rect, Theme};
use xui_form::{
    Binder, BuildOptions, CallError, Catalog, EventHandler, EventRef, Factories, FormDoc, LiveForm,
    Node, Value, build_with,
};

/// A backend that forwards to the offscreen one but runs real (manually
/// fired) timers and counts invalidations.
struct TimerBackend {
    inner: Rc<OffscreenBackend>,
    next_timer: Cell<usize>,
    /// Live timers: id to interval in milliseconds.
    timers: RefCell<BTreeMap<usize, u32>>,
    sinks: RefCell<HashMap<u64, Rc<dyn WidgetHost>>>,
    invalidations: RefCell<HashMap<WidgetId, usize>>,
}

impl TimerBackend {
    fn new(inner: OffscreenBackend) -> TimerBackend {
        TimerBackend {
            inner: Rc::new(inner),
            next_timer: Cell::new(1),
            timers: RefCell::new(BTreeMap::new()),
            sinks: RefCell::new(HashMap::new()),
            invalidations: RefCell::new(HashMap::new()),
        }
    }

    /// The intervals of the live timers, in id order.
    fn live(&self) -> Vec<u32> {
        self.timers.borrow().values().copied().collect()
    }

    /// Fires every live timer once in `window`.
    fn fire(&self, window: WindowId) {
        let ids: Vec<usize> = self.timers.borrow().keys().copied().collect();
        let sink = self.sinks.borrow().get(&window.raw()).cloned();
        if let Some(sink) = sink {
            for id in ids {
                sink.deliver(WidgetId::NONE, &Event::Timer { id: TimerId(id) });
            }
        }
    }

    /// How many repaints `id` asked for.
    fn invalidations(&self, id: WidgetId) -> usize {
        self.invalidations.borrow().get(&id).copied().unwrap_or(0)
    }
}

impl Backend for TimerBackend {
    fn init(&self) {
        self.inner.init();
    }
    fn run(&self) -> i32 {
        self.inner.run()
    }
    fn run_with(&self, window: WindowId, on_ready: &mut dyn FnMut()) -> i32 {
        self.inner.run_with(window, on_ready)
    }
    fn quit(&self, code: i32) {
        self.inner.quit(code);
    }
    fn wake(&self, window: WindowId) {
        self.inner.wake(window);
    }
    fn waker(&self, window: WindowId) -> Waker {
        self.inner.waker(window)
    }
    fn set_event_sink(&self, window: WindowId, sink: Rc<dyn WidgetHost>) {
        self.sinks
            .borrow_mut()
            .insert(window.raw(), Rc::clone(&sink));
        self.inner.set_event_sink(window, sink);
    }
    fn open_window(&self, spec: &PlatformSpec) -> BackendResult<WindowId> {
        self.inner.open_window(spec)
    }
    fn close_window(&self, window: WindowId) {
        self.inner.close_window(window);
    }
    fn set_window_title(&self, window: WindowId, title: &str) {
        self.inner.set_window_title(window, title);
    }
    fn native_window(&self, window: WindowId) -> Option<NativeWindowHandle> {
        self.inner.native_window(window)
    }
    fn capture(&self, window: WindowId) -> BackendResult<Image> {
        self.inner.capture(window)
    }
    fn create(&self, parent: ParentRef, spec: &NodeSpec) -> BackendResult<WidgetId> {
        self.inner.create(parent, spec)
    }
    fn destroy(&self, id: WidgetId) {
        self.inner.destroy(id);
    }
    fn apply_moves(&self, window: WindowId, moves: &[(WidgetId, Rect)]) {
        self.inner.apply_moves(window, moves);
    }
    fn set_visible(&self, id: WidgetId, visible: bool) {
        self.inner.set_visible(id, visible);
    }
    fn set_enabled(&self, id: WidgetId, enabled: bool) {
        self.inner.set_enabled(id, enabled);
    }
    fn raise(&self, id: WidgetId) {
        self.inner.raise(id);
    }
    fn set_cursor(&self, id: WidgetId, cursor: Cursor) {
        self.inner.set_cursor(id, cursor);
    }
    fn set_clip(&self, id: WidgetId, rect: Option<Rect>) {
        self.inner.set_clip(id, rect);
    }
    fn set_capture(&self, id: WidgetId) {
        self.inner.set_capture(id);
    }
    fn release_capture(&self) {
        self.inner.release_capture();
    }
    fn focus(&self, id: WidgetId) {
        self.inner.focus(id);
    }
    fn set_text(&self, id: WidgetId, text: &str) {
        self.inner.set_text(id, text);
    }
    fn text(&self, id: WidgetId) -> String {
        self.inner.text(id)
    }
    fn bounds(&self, id: WidgetId) -> Rect {
        self.inner.bounds(id)
    }
    fn invalidate(&self, id: WidgetId) {
        *self.invalidations.borrow_mut().entry(id).or_default() += 1;
        self.inner.invalidate(id);
    }
    fn invalidate_rect(&self, id: WidgetId, rect: Rect) {
        self.inner.invalidate_rect(id, rect);
    }
    fn set_painter(&self, id: WidgetId, painter: Painter) {
        self.inner.set_painter(id, painter);
    }
    fn measure_text(&self, text: &str, style: &TextStyle, dpi: u32) -> TextMetrics {
        self.inner.measure_text(text, style, dpi)
    }
    fn text_shaper(&self) -> Box<dyn TextShaper> {
        self.inner.text_shaper()
    }
    fn layout_text(
        &self,
        text: &str,
        spec: &FontSpec,
        max_width: f32,
        dpi: u32,
    ) -> Box<dyn TextLayout> {
        self.inner.layout_text(text, spec, max_width, dpi)
    }
    fn dpi(&self, window: WindowId) -> u32 {
        self.inner.dpi(window)
    }
    fn client_rect(&self, window: WindowId) -> Rect {
        self.inner.client_rect(window)
    }
    fn set_theme(&self, window: WindowId, theme: &Theme) {
        self.inner.set_theme(window, theme);
    }
    fn set_timer(&self, _window: WindowId, millis: u32) -> TimerId {
        let id = self.next_timer.get();
        self.next_timer.set(id + 1);
        self.timers.borrow_mut().insert(id, millis);
        TimerId(id)
    }
    fn kill_timer(&self, _window: WindowId, id: TimerId) {
        self.timers.borrow_mut().remove(&id.0);
    }
    fn supports(&self, kind: NodeKind) -> ImplKind {
        self.inner.supports(kind)
    }
}

/// A canvas event the binder turned into a message.
#[derive(Clone, Debug, PartialEq)]
struct Raised {
    node: String,
    event: String,
    args: Vec<Value>,
}

/// Binds every canvas event, or (with `frame: false`) all but `Frame`.
struct CanvasBinder {
    frame: bool,
}

impl Binder<Raised> for CanvasBinder {
    fn bind(&self, event: EventRef<'_>) -> Option<EventHandler<Raised>> {
        if event.event == "Frame" && !self.frame {
            return None;
        }
        let node = event.node.to_owned();
        let name = event.event.to_owned();
        Some(Rc::new(move |args| {
            Some(Raised {
                node: node.clone(),
                event: name.clone(),
                args: args.to_vec(),
            })
        }))
    }
}

/// Records every message.
struct Recorder(Rc<RefCell<Vec<Raised>>>);

impl App for Recorder {
    type Msg = Raised;

    fn update(&mut self, msg: Raised, _ui: &mut Ui<Raised>) {
        self.0.borrow_mut().push(msg);
    }
}

/// A canvas node at `(left, top)`, 100x80, with extra properties.
fn canvas_node(name: &str, left: i64, top: i64, props: &[(&str, Value)]) -> Node {
    let mut node = Node::new("Canvas", name);
    for (prop, value) in [("left", left), ("top", top), ("width", 100), ("height", 80)] {
        node.set_prop(prop, Value::Int(value));
    }
    for (prop, value) in props {
        node.set_prop(*prop, value.clone());
    }
    node
}

/// A form holding `nodes`.
fn doc_with(nodes: Vec<Node>) -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    for node in nodes {
        doc.insert(node);
    }
    doc
}

/// Builds `doc` into `ui` (or into `container`).
fn build(
    ui: &Ui<Raised>,
    doc: &FormDoc,
    design_mode: bool,
    frame: bool,
    container: Option<WidgetId>,
) -> LiveForm<Raised> {
    let factories: Factories<Raised> = Factories::xui();
    build_with(
        ui,
        doc,
        &Catalog::xui(),
        &factories,
        &CanvasBinder { frame },
        BuildOptions {
            design_mode,
            container,
        },
    )
    .expect("the form builds")
}

/// Runs `check` inside a window on a [`TimerBackend`] at `dpi` and returns the
/// messages the app received afterwards.
fn session(dpi: u32, check: impl FnOnce(&Rc<TimerBackend>, &Ui<Raised>) + 'static) -> Vec<Raised> {
    let backend = Rc::new(TimerBackend::new(OffscreenBackend::with_dpi(dpi)));
    let messages = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::clone(&messages);
    let inner = Rc::clone(&backend);
    let spec = PlatformSpec::new("canvas tests").size(Dip(320.0), Dip(200.0));
    run_app(backend as Rc<dyn Backend>, spec, move |ui| {
        check(&inner, ui);
        Recorder(log)
    })
    .expect("run_app succeeds");
    messages.take()
}

/// A key press or release.
fn key(key: Key, down: bool, repeat: u16) -> Event {
    if down {
        Event::KeyDown {
            key,
            modifiers: Modifiers::NONE,
            repeat,
            system: false,
        }
    } else {
        Event::KeyUp {
            key,
            modifiers: Modifiers::NONE,
            system: false,
        }
    }
}

/// The RGB of a rendered pixel.
fn pixel(backend: &TimerBackend, window: WindowId, x: u32, y: u32) -> [u8; 3] {
    let image = backend.inner.render(window).expect("the window renders");
    let [r, g, b, _] = image.pixel(x, y).expect("inside the window");
    [r, g, b]
}

/// Draws a filled rectangle on `name` in `form`.
fn fill(form: &LiveForm<Raised>, name: &str, rect: [f64; 4], color: Color) {
    let mut args: Vec<Value> = rect.iter().map(|value| Value::Float(*value)).collect();
    args.push(Value::Color(color));
    form.call(name, "fill_rect", &args).expect("fill_rect");
}

const RED: Color = Color::rgb(255, 0, 0);
const BLUE: Color = Color::rgb(0, 0, 255);

#[test]
fn fps_starts_changes_and_stops_the_frame_timer() {
    session(96, |backend, ui| {
        let form = build(
            ui,
            &doc_with(vec![canvas_node("canvas1", 0, 0, &[])]),
            false,
            true,
            None,
        );
        assert!(backend.live().is_empty(), "fps 0 runs no loop");
        form.set("canvas1", "fps", &Value::Int(60)).expect("fps");
        assert_eq!(backend.live(), [17], "60 fps ticks every 17 ms");
        form.set("canvas1", "fps", &Value::Int(30)).expect("fps");
        assert_eq!(backend.live(), [33], "a new rate replaces the old timer");
        form.set("canvas1", "fps", &Value::Int(0)).expect("fps");
        assert!(backend.live().is_empty(), "fps 0 stops the loop");
        form.set("canvas1", "fps", &Value::Int(60)).expect("fps");
        assert_eq!(backend.live(), [17], "the loop restarts");
        assert_eq!(
            form.set("canvas1", "fps", &Value::Int(1000)),
            Err(xui_form::SetError::TypeMismatch),
            "fps is capped by the schema"
        );
        assert_eq!(form.get("canvas1", "fps"), Some(Value::Int(60)));
        drop(form);
        assert!(
            backend.live().is_empty(),
            "dropping the canvas kills its timer"
        );
    });
}

#[test]
fn a_designed_fps_starts_the_loop_and_frames_report_dt() {
    let messages = session(96, |backend, ui| {
        let form = build(
            ui,
            &doc_with(vec![canvas_node(
                "canvas1",
                0,
                0,
                &[("fps", Value::Int(50))],
            )]),
            false,
            true,
            None,
        );
        assert_eq!(backend.live(), [20]);
        for _ in 0..3 {
            backend.fire(ui.window());
        }
        drop(form);
    });
    let frames: Vec<&Raised> = messages.iter().filter(|msg| msg.event == "Frame").collect();
    assert_eq!(frames.len(), 3, "{messages:?}");
    for frame in frames {
        assert_eq!(frame.node, "canvas1");
        let dt = frame.args[0].as_float().expect("dt is a float");
        assert!((0.0..=0.1).contains(&dt), "{dt}");
    }
}

#[test]
fn design_mode_runs_no_loop_and_refuses_methods() {
    session(96, |backend, ui| {
        let form = build(
            ui,
            &doc_with(vec![canvas_node(
                "canvas1",
                0,
                0,
                &[("fps", Value::Int(60))],
            )]),
            true,
            true,
            None,
        );
        assert!(backend.live().is_empty(), "the designer never ticks");
        form.set("canvas1", "fps", &Value::Int(30))
            .expect("fps is a design property");
        assert!(backend.live().is_empty());
        assert!(matches!(
            form.call("canvas1", "clear", &[Value::Color(RED)]),
            Err(CallError::Failed(_))
        ));
    });
}

#[test]
fn an_unbound_frame_event_runs_no_loop() {
    session(96, |backend, ui| {
        let _form = build(
            ui,
            &doc_with(vec![canvas_node(
                "canvas1",
                0,
                0,
                &[("fps", Value::Int(60))],
            )]),
            false,
            false,
            None,
        );
        assert!(backend.live().is_empty(), "nobody listens to Frame");
    });
}

#[test]
fn a_drawing_call_invalidates_and_paints_at_the_window_dpi() {
    session(144, |backend, ui| {
        let form = build(
            ui,
            &doc_with(vec![canvas_node("canvas1", 0, 0, &[])]),
            false,
            true,
            None,
        );
        let id = form.widget("canvas1").expect("the canvas").id();
        let before = backend.invalidations(id);
        form.call("canvas1", "clear", &[Value::Color(BLUE)])
            .expect("clear");
        fill(&form, "canvas1", [10.0, 10.0, 20.0, 20.0], RED);
        assert!(
            backend.invalidations(id) >= before + 2,
            "each call repaints"
        );
        // At 150% a logical (10, 10, 20, 20) rectangle covers device pixels
        // 15..45.
        let window = ui.window();
        assert_eq!(pixel(backend, window, 16, 16), [255, 0, 0]);
        assert_eq!(pixel(backend, window, 44, 44), [255, 0, 0]);
        assert_eq!(pixel(backend, window, 50, 50), [0, 0, 255]);
        assert_eq!(pixel(backend, window, 12, 12), [0, 0, 255]);
    });
}

#[test]
fn keys_are_reported_once_and_released_on_focus_loss() {
    let messages = session(96, |backend, ui| {
        let form = build(
            ui,
            &doc_with(vec![canvas_node("canvas1", 0, 0, &[])]),
            false,
            true,
            None,
        );
        form.call("canvas1", "focus", &[]).expect("focus");
        let window = ui.window();
        let inner = &backend.inner;
        assert!(inner.inject(window, key(Key::LEFT, true, 1)));
        // The auto-repeat raises nothing, but the key stays down.
        inner.inject(window, key(Key::LEFT, true, 2));
        inner.inject(window, key(Key::LEFT, true, 3));
        let down = |name: &str| {
            form.call("canvas1", "is_key_down", &[Value::Text(name.to_owned())])
                .expect("is_key_down")
        };
        assert_eq!(down("left"), Some(Value::Bool(true)));
        assert_eq!(down("right"), Some(Value::Bool(false)));
        inner.inject(window, key(Key::A, true, 1));
        inner.inject(window, key(Key::A, false, 1));
        assert_eq!(down("a"), Some(Value::Bool(false)));
        // Losing the focus while Left is held forgets it.
        inner.inject(window, Event::KillFocus);
        assert_eq!(down("left"), Some(Value::Bool(false)), "no stuck key");
    });
    let keys: Vec<(String, Value)> = messages
        .iter()
        .filter(|msg| msg.event.starts_with("Key"))
        .map(|msg| (msg.event.clone(), msg.args[0].clone()))
        .collect();
    assert_eq!(
        keys,
        [
            ("KeyDown".to_owned(), Value::Text("left".to_owned())),
            ("KeyDown".to_owned(), Value::Text("a".to_owned())),
            ("KeyUp".to_owned(), Value::Text("a".to_owned())),
        ]
    );
}

#[test]
fn a_click_focuses_the_canvas_and_reports_logical_coordinates() {
    let messages = session(192, |backend, ui| {
        let form = build(
            ui,
            &doc_with(vec![canvas_node("canvas1", 10, 10, &[])]),
            false,
            true,
            None,
        );
        let window = ui.window();
        let inner = &backend.inner;
        // At 200% the canvas starts at device (20, 20); device (60, 80) is
        // logical (20, 30) inside it.
        let (x, y) = (60, 80);
        inner.inject(
            window,
            Event::MouseDown {
                x,
                y,
                button: MouseButton::Left,
                modifiers: Modifiers::NONE,
            },
        );
        inner.inject(
            window,
            Event::MouseUp {
                x,
                y,
                button: MouseButton::Left,
                modifiers: Modifiers::NONE,
            },
        );
        assert_eq!(form.get("canvas1", "mouse_x"), Some(Value::Float(20.0)));
        assert_eq!(form.get("canvas1", "mouse_y"), Some(Value::Float(30.0)));
        // The press focused the canvas, so a key now reaches it.
        assert!(inner.inject(window, key(Key::SPACE, true, 1)));
    });
    let events: Vec<&str> = messages.iter().map(|msg| msg.event.as_str()).collect();
    assert_eq!(events, ["MouseDown", "MouseUp", "KeyDown"]);
    assert_eq!(
        messages[0].args,
        [
            Value::Float(20.0),
            Value::Float(30.0),
            Value::Text("left".to_owned())
        ]
    );
}

#[test]
fn two_canvases_on_one_form_are_independent() {
    let messages = session(96, |backend, ui| {
        let form = build(
            ui,
            &doc_with(vec![
                canvas_node("canvas1", 0, 0, &[("fps", Value::Int(60))]),
                canvas_node("canvas2", 150, 0, &[]),
            ]),
            false,
            true,
            None,
        );
        assert_eq!(backend.live(), [17], "only canvas1 runs a loop");
        fill(&form, "canvas1", [0.0, 0.0, 100.0, 80.0], RED);
        form.call("canvas2", "clear", &[Value::Color(BLUE)])
            .expect("clear");
        let window = ui.window();
        assert_eq!(pixel(backend, window, 50, 40), [255, 0, 0]);
        assert_eq!(pixel(backend, window, 200, 40), [0, 0, 255]);
        // Clearing canvas2 again leaves canvas1's drawing alone.
        form.call("canvas2", "clear", &[Value::Color(Color::rgb(0, 255, 0))])
            .expect("clear");
        assert_eq!(pixel(backend, window, 50, 40), [255, 0, 0]);

        form.call("canvas1", "focus", &[]).expect("focus");
        backend.inner.inject(window, key(Key::UP, true, 1));
        let up = |name: &str| form.call(name, "is_key_down", &[Value::Text("up".to_owned())]);
        assert_eq!(up("canvas1"), Ok(Some(Value::Bool(true))));
        assert_eq!(up("canvas2"), Ok(Some(Value::Bool(false))));
        backend.fire(window);
    });
    let frames: Vec<&str> = messages
        .iter()
        .filter(|msg| msg.event == "Frame")
        .map(|msg| msg.node.as_str())
        .collect();
    assert_eq!(frames, ["canvas1"]);
}

#[test]
fn canvases_on_two_forms_share_nothing() {
    session(96, |backend, ui| {
        let left = Handle::new();
        let right = Handle::new();
        let _mounted = ui
            .mount(
                absolute()
                    .child(panel(absolute()).plain().bind(&left).at(0, 0, 150, 100))
                    .child(panel(absolute()).plain().bind(&right).at(160, 0, 150, 100)),
            )
            .expect("the containers mount");
        // Both forms name their canvas `canvas1`.
        let doc = doc_with(vec![canvas_node("canvas1", 0, 0, &[])]);
        let first = build(ui, &doc, false, true, Some(left.get().id()));
        let second = build(ui, &doc, false, true, Some(right.get().id()));
        first
            .call("canvas1", "clear", &[Value::Color(RED)])
            .expect("clear");
        second
            .call("canvas1", "clear", &[Value::Color(BLUE)])
            .expect("clear");
        let window = ui.window();
        assert_eq!(pixel(backend, window, 50, 40), [255, 0, 0]);
        assert_eq!(pixel(backend, window, 210, 40), [0, 0, 255]);
        first.set("canvas1", "fps", &Value::Int(60)).expect("fps");
        assert_eq!(second.get("canvas1", "fps"), Some(Value::Int(0)));
        assert_eq!(backend.live().len(), 1);
        second.set("canvas1", "fps", &Value::Int(30)).expect("fps");
        assert_eq!(backend.live(), [17, 33], "each form has its own timer");
        drop(first);
        assert_eq!(
            backend.live(),
            [33],
            "closing one form keeps the other's loop"
        );
    });
}

#[test]
fn text_far_off_the_canvas_paints_without_overflowing() {
    session(96, |backend, ui| {
        let form = build(
            ui,
            &doc_with(vec![canvas_node("canvas1", 0, 0, &[])]),
            false,
            true,
            None,
        );
        for (x, y) in [(1e10, 0.0), (0.0, 1e10), (-1e10, -1e10)] {
            form.call(
                "canvas1",
                "text",
                &[
                    Value::Float(x),
                    Value::Float(y),
                    Value::Text("x".to_owned()),
                    Value::Color(RED),
                    Value::Float(1000.0),
                ],
            )
            .expect("a finite coordinate is accepted");
        }
        fill(&form, "canvas1", [-1e10, -1e10, 2e10, 2e10], BLUE);
        // Painting clamps instead of overflowing (a panic in debug builds).
        pixel(backend, ui.window(), 1, 1);
        drop(form);
    });
}
