#![forbid(unsafe_code)]

//! Window-level keyboard shortcuts for xui, which has no accelerator table
//! (gap G10 in PLAN.md §10).
//!
//! xui delivers `KeyDown` to whichever widget holds focus, not to the window,
//! and core exposes no way to register a window-level key handler. This module
//! wraps a [`Backend`] and intercepts the event [`WidgetHost`] the runtime
//! installs: every decoded event passes through [`ShortcutSink`], which maps a
//! matching key chord to the app's message and pushes it through a [`Proxy`].
//! The command therefore fires regardless of which pane has focus, and the
//! wrapped backend behaves exactly like the inner one for everything else.
//!
//! The workaround can go away once xui grows `Ui::set_accelerators`.

use std::cell::RefCell;
use std::rc::Rc;

use xui_core::app::Proxy;
use xui_core::backend::{
    Backend, Cursor, Event, FontSpec, ImplKind, NativeWindowHandle, NodeKind, NodeSpec, Painter,
    ParentRef, PlatformSpec, Result, TextLayout, TextMetrics, TextShaper, TextStyle, TimerId,
    Waker, WidgetId, WindowId,
};
use xui_core::image::Image;
use xui_core::router::WidgetHost;
use xui_core::{Dip, Rect, Theme};

/// Maps an event to the message a bound shortcut raises.
type ShortcutMap<M> = Rc<dyn Fn(&Event) -> Option<M>>;

/// A `Backend` that forwards to another and turns matching key chords into app
/// messages.
pub struct ShortcutBackend<M: 'static> {
    inner: Rc<dyn Backend>,
    map: ShortcutMap<M>,
    proxy: Rc<RefCell<Option<Proxy<M>>>>,
}

impl<M: 'static> ShortcutBackend<M> {
    /// Wraps `inner`; `map` returns the message for an event that is a bound
    /// shortcut, or `None` to let it through untouched.
    pub fn new(inner: Rc<dyn Backend>, map: impl Fn(&Event) -> Option<M> + 'static) -> Self {
        ShortcutBackend {
            inner,
            map: Rc::new(map),
            proxy: Rc::new(RefCell::new(None)),
        }
    }

    /// The shared cell the app fills with its [`Proxy`] once the window exists.
    ///
    /// Until it is set, a matched shortcut is forwarded to the inner sink
    /// instead of being turned into a message, so no event is silently lost.
    pub fn proxy_cell(&self) -> Rc<RefCell<Option<Proxy<M>>>> {
        Rc::clone(&self.proxy)
    }
}

/// Hands a matched shortcut's message to the app, reporting whether it was
/// delivered.
type SendShortcut<M> = Rc<dyn Fn(M) -> bool>;

/// The event sink that recognises shortcuts before the widget layer sees them.
struct ShortcutSink<M: 'static> {
    inner: Rc<dyn WidgetHost>,
    map: ShortcutMap<M>,
    send: SendShortcut<M>,
}

impl<M: 'static> WidgetHost for ShortcutSink<M> {
    fn deliver(&self, target: WidgetId, event: &Event) -> bool {
        if let Some(msg) = (self.map)(event)
            && (self.send)(msg)
        {
            return true;
        }
        self.inner.deliver(target, event)
    }
}

impl<M: Send + 'static> Backend for ShortcutBackend<M> {
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
        let proxy = Rc::clone(&self.proxy);
        let send: SendShortcut<M> = Rc::new(move |msg| {
            proxy
                .borrow()
                .as_ref()
                .is_some_and(|proxy| proxy.send(msg).is_ok())
        });
        let wrapped: Rc<dyn WidgetHost> = Rc::new(ShortcutSink {
            inner: sink,
            map: Rc::clone(&self.map),
            send,
        });
        self.inner.set_event_sink(window, wrapped);
    }

    fn open_window(&self, spec: &PlatformSpec) -> Result<WindowId> {
        self.inner.open_window(spec)
    }

    fn close_window(&self, window: WindowId) {
        self.inner.close_window(window);
    }

    fn set_window_title(&self, window: WindowId, title: &str) {
        self.inner.set_window_title(window, title);
    }

    fn set_window_enabled(&self, window: WindowId, enabled: bool) {
        self.inner.set_window_enabled(window, enabled);
    }

    fn native_window(&self, window: WindowId) -> Option<NativeWindowHandle> {
        self.inner.native_window(window)
    }

    fn capture(&self, window: WindowId) -> Result<Image> {
        self.inner.capture(window)
    }

    fn run_modal(&self, window: WindowId) -> Result<()> {
        self.inner.run_modal(window)
    }

    fn minimize(&self, window: WindowId) {
        self.inner.minimize(window);
    }

    fn toggle_maximize(&self, window: WindowId) {
        self.inner.toggle_maximize(window);
    }

    fn is_maximized(&self, window: WindowId) -> bool {
        self.inner.is_maximized(window)
    }

    fn caption_inset(&self, window: WindowId) -> Dip {
        self.inner.caption_inset(window)
    }

    fn create(&self, parent: ParentRef, spec: &NodeSpec) -> Result<WidgetId> {
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

    fn set_drag_region(&self, id: WidgetId, drag: bool) {
        self.inner.set_drag_region(id, drag);
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

    fn set_cue(&self, id: WidgetId, cue: &str) {
        self.inner.set_cue(id, cue);
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

    fn set_timer(&self, window: WindowId, millis: u32) -> TimerId {
        self.inner.set_timer(window, millis)
    }

    fn kill_timer(&self, window: WindowId, id: TimerId) {
        self.inner.kill_timer(window, id);
    }

    fn supports(&self, kind: NodeKind) -> ImplKind {
        self.inner.supports(kind)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use xui_core::Key;

    use super::*;

    /// A host that records the events it was handed.
    struct Recorder {
        seen: Rc<RefCell<Vec<Event>>>,
    }

    impl WidgetHost for Recorder {
        fn deliver(&self, _target: WidgetId, event: &Event) -> bool {
            self.seen.borrow_mut().push(*event);
            true
        }
    }

    /// A sink whose matching decision and send result the test controls.
    struct Probe {
        sink: ShortcutSink<u32>,
        seen: Rc<RefCell<Vec<Event>>>,
        sent: Rc<Cell<u32>>,
    }

    fn sink(map: u32, send_ok: bool) -> Probe {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sent = Rc::new(Cell::new(0));
        let recorder: Rc<dyn WidgetHost> = Rc::new(Recorder {
            seen: Rc::clone(&seen),
        });
        let sent_for_send = Rc::clone(&sent);
        let sink = ShortcutSink {
            inner: recorder,
            map: Rc::new(move |event: &Event| {
                matches!(event, Event::KeyDown { .. }).then_some(map)
            }),
            send: Rc::new(move |msg| {
                sent_for_send.set(msg);
                send_ok
            }),
        };
        Probe { sink, seen, sent }
    }

    fn key(key: Key) -> Event {
        Event::KeyDown {
            key,
            modifiers: xui_core::Modifiers::NONE,
            repeat: 1,
            system: false,
        }
    }

    #[test]
    fn a_matched_chord_is_sent_and_consumed() {
        let probe = sink(7, true);
        assert!(probe.sink.deliver(WidgetId::NONE, &key(Key::S)));
        assert_eq!(probe.sent.get(), 7);
        assert!(
            probe.seen.borrow().is_empty(),
            "the inner sink never saw it"
        );
    }

    #[test]
    fn an_unmatched_event_reaches_the_wrapped_sink() {
        let probe = sink(0, true);
        let close = Event::Close;
        assert!(probe.sink.deliver(WidgetId::NONE, &close));
        assert_eq!(*probe.seen.borrow(), vec![close]);
    }

    #[test]
    fn a_failed_send_falls_through_to_the_wrapped_sink() {
        let probe = sink(7, false);
        let down = key(Key::F5);
        assert!(probe.sink.deliver(WidgetId::NONE, &down));
        assert_eq!(*probe.seen.borrow(), vec![down]);
    }

    #[test]
    fn the_wrapper_delegates_plain_backend_calls() {
        let inner: Rc<dyn Backend> = Rc::new(xui_canvas::OffscreenBackend::new());
        let backend = ShortcutBackend::<u32>::new(Rc::clone(&inner), |_| None);
        let window = backend
            .open_window(&PlatformSpec::new("shortcuts"))
            .expect("the offscreen window opens");
        assert_eq!(backend.dpi(window), inner.dpi(window));
        assert_eq!(backend.client_rect(window), inner.client_rect(window));
    }
}
