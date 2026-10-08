//! Hot reload of a form that holds a `Canvas` (issue #91 with issue #104) and
//! a `Timer` (with issue #90).
//!
//! A `Canvas` runs its own per-control frame timer and a `Timer` control runs a
//! window timer routed by the form's `Timers`. A reload drops the old form,
//! which must kill both, and the new form starts its own: exactly one of each
//! stays live. The offscreen backend starts no timers, so
//! [`TimerBackend`] wraps it, hands out timer ids, records which are live and
//! fires them on request.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::rc::Rc;

use lazyrad_runtime::{FormRuntime, ReloadOutcome};
use xui_canvas::OffscreenBackend;
use xui_core::app::run_app;
use xui_core::backend::{
    Backend, Cursor, Event, FontSpec, ImplKind, NativeWindowHandle, NodeKind, NodeSpec, Painter,
    ParentRef, PlatformSpec, Result as BackendResult, TextLayout, TextMetrics, TextShaper,
    TextStyle, TimerId, Waker, WidgetId, WindowId,
};
use xui_core::image::Image;
use xui_core::router::WidgetHost;
use xui_core::{Dip, Rect, Theme};
use xui_form::Value;

/// A backend that forwards to the offscreen one but runs real (manually
/// fired) timers and counts invalidations.
struct TimerBackend {
    inner: Rc<OffscreenBackend>,
    next_timer: Cell<usize>,
    /// Live timers: id to interval in milliseconds.
    timers: RefCell<BTreeMap<usize, u32>>,
    sinks: RefCell<HashMap<u64, Rc<dyn WidgetHost>>>,
}

impl TimerBackend {
    fn new(inner: OffscreenBackend) -> TimerBackend {
        TimerBackend {
            inner: Rc::new(inner),
            next_timer: Cell::new(1),
            timers: RefCell::new(BTreeMap::new()),
            sinks: RefCell::new(HashMap::new()),
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

/// `timers` in ascending order of interval.
fn sorted(mut timers: Vec<u32>) -> Vec<u32> {
    timers.sort_unstable();
    timers
}

/// The project: a label, a 60 fps canvas and a (disabled) 40 ms timer.
fn write_project(dir: &Path, code: &str) {
    fs::write(
        dir.join("check.lrp"),
        "name = \"check\"\nversion = \"0.1.0\"\nstartup = \"main_form\"\n\n\
         [[items]]\nkind = \"form\"\nname = \"main_form\"\n\
         layout = \"main_form.lfm\"\ncode = \"main_form.rhai\"\n",
    )
    .expect("project writes");
    fs::write(
        dir.join("main_form.lfm"),
        "format = 1\n\n[window]\nname = \"main_form\"\ntitle = \"Check\"\n\n\
         [[node]]\nkind = \"Label\"\nname = \"result_label\"\n\
         left = 10\ntop = 10\nwidth = 200\nheight = 20\ntext = \"before\"\n\n\
         [[node]]\nkind = \"Canvas\"\nname = \"canvas1\"\n\
         left = 10\ntop = 40\nwidth = 100\nheight = 80\nfps = 60\n\n\
         [[node]]\nkind = \"Timer\"\nname = \"timer1\"\ninterval = 40\n",
    )
    .expect("form writes");
    fs::write(dir.join("main_form.rhai"), code).expect("code writes");
}

#[test]
fn a_reload_rebuilds_the_canvas_with_exactly_one_frame_timer() {
    let dir = std::env::temp_dir().join(format!(
        "lazyrad-runtime-reload-canvas-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory is created");
    write_project(
        &dir,
        "fn canvas1_frame(dt) { result_label.text = \"old frame\"; }",
    );
    let runtime = FormRuntime::load(&dir).expect("the project loads");
    runtime.enable_watch(&dir);

    let backend = Rc::new(TimerBackend::new(OffscreenBackend::new()));
    let seen = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&seen);
    let inner = Rc::clone(&backend);
    let dir_for_run = dir.clone();
    let runtime_for_run = Rc::clone(&runtime);
    let spec = PlatformSpec::new("reload canvas tests").size(Dip(320.0), Dip(200.0));
    run_app(backend as Rc<dyn Backend>, spec, move |ui| {
        let mut app = runtime_for_run
            .build_app(ui, "main_form")
            .expect("the form builds");
        assert_eq!(
            sorted(inner.live()),
            [17, 500],
            "the canvas frame loop and the watch tick"
        );

        fs::write(
            dir_for_run.join("main_form.rhai"),
            "fn canvas1_frame(dt) { result_label.text = \"new frame\"; }",
        )
        .expect("the script is rewritten");
        assert_eq!(runtime_for_run.check_for_changes(), ReloadOutcome::Reloaded);
        assert!(app.reload_root(ui), "the form rebuilds");

        // The old canvas's timer died with it; the new canvas started its own.
        assert_eq!(
            sorted(inner.live()),
            [17, 500],
            "one frame timer and one watch tick, none leaked"
        );
        assert_eq!(
            app.root_form().expect("live").get("canvas1", "fps"),
            Some(Value::Int(60))
        );
        // A tick reaches the new script's handler, through the message queue
        // drained after this closure.
        inner.fire(ui.window());
        *slot.borrow_mut() = Some(Rc::clone(app.root_form().expect("live")));
        app
    })
    .expect("the event loop runs");

    let form = seen.borrow_mut().take().expect("the form was captured");
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("new frame".to_owned())),
        "the new script's canvas1_frame ran"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_reload_restarts_a_timer_control_without_leaking_the_old_one() {
    let dir = std::env::temp_dir().join(format!(
        "lazyrad-runtime-reload-timer-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory is created");
    // No `canvas1_frame`, so the canvas runs no frame loop: only the Timer
    // control's window timer and the watch tick are live.
    write_project(
        &dir,
        "fn form_load() { timer1.enabled = true; }\n\
         fn timer1_tick() { result_label.text = \"old tick\"; }",
    );
    let runtime = FormRuntime::load(&dir).expect("the project loads");
    runtime.enable_watch(&dir);

    let backend = Rc::new(TimerBackend::new(OffscreenBackend::new()));
    let seen = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&seen);
    let inner = Rc::clone(&backend);
    let dir_for_run = dir.clone();
    let runtime_for_run = Rc::clone(&runtime);
    let spec = PlatformSpec::new("reload timer tests").size(Dip(320.0), Dip(200.0));
    run_app(backend as Rc<dyn Backend>, spec, move |ui| {
        let mut app = runtime_for_run
            .build_app(ui, "main_form")
            .expect("the form builds");
        assert_eq!(sorted(inner.live()), [40, 500], "timer1 and the watch tick");

        fs::write(
            dir_for_run.join("main_form.rhai"),
            "fn form_load() { timer1.interval = 70; timer1.enabled = true; }\n\
             fn timer1_tick() { result_label.text = \"new tick\"; }",
        )
        .expect("the script is rewritten");
        assert_eq!(runtime_for_run.check_for_changes(), ReloadOutcome::Reloaded);
        assert!(app.reload_root(ui), "the form rebuilds");

        // The old 40 ms timer died with the old form; the new one runs at 70.
        assert_eq!(
            sorted(inner.live()),
            [70, 500],
            "one timer at the new interval and one watch tick, none leaked"
        );
        assert!(app.is_timer_running("timer1"));
        // A tick reaches the new script's handler, through the message queue
        // drained after this closure.
        inner.fire(ui.window());
        *slot.borrow_mut() = Some(Rc::clone(app.root_form().expect("live")));
        app
    })
    .expect("the event loop runs");

    let form = seen.borrow_mut().take().expect("the form was captured");
    assert_eq!(
        form.get("result_label", "text"),
        Some(Value::Text("new tick".to_owned())),
        "the new script's timer1_tick ran"
    );
    let _ = fs::remove_dir_all(&dir);
}
