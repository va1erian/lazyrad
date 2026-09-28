#![forbid(unsafe_code)]

//! The [`Editor`] widget: a single `NodeKind::Custom` xui node with a painter
//! and an event mapper (PLAN.md §5).
//!
//! The widget owns a [`Control`], the shared [`EditorState`] and the app's
//! `on_change` mapper. Everything the app configures goes through this type;
//! the pure buffer, view and edit rules live in their own modules.

use std::cell::RefCell;
use std::rc::Rc;

use xui_core::app::Ui;
use xui_core::backend::{Cursor, NodeKind, NodeSpec, Result, TimerId, WidgetId};
use xui_core::geometry::Rect;

use crate::buffer::Buffer;
use crate::events;
use crate::markers::Marker;
use crate::options::Options;
use crate::paint;
use crate::platform;
use crate::state::{EditorState, Effect};
use crate::theme::EditorTheme;
use crate::view::View;

/// How often the caret blinks, in milliseconds.
const BLINK_MS: u32 = 500;

/// Maps the new text to an optional app message.
type ChangeMapper<M> = Box<dyn Fn(&str) -> Option<M>>;

/// A code editor on a custom xui node.
pub struct Editor<M: 'static> {
    control: xui_core::widget::Control<M>,
    state: Rc<RefCell<EditorState>>,
    on_change: Rc<RefCell<Option<ChangeMapper<M>>>>,
    timer: TimerId,
}

impl<M: 'static> Editor<M> {
    /// Creates an editor at `bounds` with the default options.
    pub fn new(ui: &Ui<M>, bounds: Rect) -> Result<Editor<M>> {
        Editor::with_options(ui, bounds, Options::default())
    }

    /// Creates an editor at `bounds` with `options`.
    pub fn with_options(ui: &Ui<M>, bounds: Rect, options: Options) -> Result<Editor<M>> {
        let control = xui_core::widget::Control::new(
            ui,
            &NodeSpec::new(NodeKind::Custom, bounds).tab_stop(),
        )?;
        ui.set_cursor(control.id(), Cursor::Text);

        let state = Rc::new(RefCell::new(EditorState::new(
            "",
            options,
            platform::clipboard(),
        )));
        let on_change: Rc<RefCell<Option<ChangeMapper<M>>>> = Rc::new(RefCell::new(None));

        {
            let state = Rc::clone(&state);
            let theme = ui.theme_handle();
            control.set_painter(Rc::new(move |canvas| {
                let xui_theme = theme.get();
                let editor_theme = EditorTheme::from_theme(xui_theme);
                let state = state.borrow();
                paint::paint(canvas, &state, &editor_theme, &xui_theme);
            }));
        }
        {
            let state = Rc::clone(&state);
            let on_change = Rc::clone(&on_change);
            let ui = ui.clone();
            let id = control.id();
            control.on_events(move |event| {
                // In design mode the form editor handles input, not the widget.
                if ui.is_design_mode() && event.is_input() {
                    return None;
                }
                let (outcome, effects) = {
                    let mut state = state.borrow_mut();
                    let outcome = events::handle(&mut state, &ui, id, event);
                    (outcome, std::mem::take(&mut state.effects))
                };
                // Outside the borrow: these calls deliver events straight back
                // into this mapper (see `Effect`).
                for effect in effects {
                    match effect {
                        Effect::Focus => ui.focus(id),
                        Effect::Capture => ui.set_capture(id),
                        Effect::ReleaseCapture => ui.release_capture(),
                    }
                }
                let outcome = outcome?;
                ui.invalidate(id);
                if outcome.changed {
                    let text = state.borrow().buffer.text();
                    let mapper = on_change.borrow();
                    if let Some(mapper) = mapper.as_ref() {
                        return mapper(&text);
                    }
                }
                None
            });
        }

        let timer = ui.set_timer(BLINK_MS);
        Ok(Editor {
            control,
            state,
            on_change,
            timer,
        })
    }

    /// Maps a text change to the app's message. The closure returns `Some(msg)`
    /// to raise it, or `None` to ignore the change; it receives the new text.
    pub fn on_change(self, mapper: impl Fn(&str) -> Option<M> + 'static) -> Editor<M> {
        *self.on_change.borrow_mut() = Some(Box::new(mapper));
        self
    }

    /// The widget's node identity.
    pub fn id(&self) -> WidgetId {
        self.control.id()
    }

    /// Gives the editor the keyboard focus.
    pub fn focus(&self) {
        self.control.focus();
    }

    /// Marks the widget selected, so its painter draws a form-editor outline.
    pub fn set_selected(&self, selected: bool) {
        self.state.borrow_mut().selected = selected;
        self.control.set_selected(selected);
    }

    /// The whole text.
    pub fn text(&self) -> String {
        self.state.borrow().buffer.text()
    }

    /// Replaces the text, moving the caret to the start and clearing undo.
    pub fn set_text(&self, text: &str) {
        let mut state = self.state.borrow_mut();
        state.buffer = Buffer::new(text);
        state.view = View::new();
        state.reset_highlight();
        drop(state);
        self.control.invalidate();
    }

    /// Moves the caret to `line`/`col` (both zero-based, `col` in chars) and
    /// scrolls it into view.
    pub fn goto(&self, line: usize, col: usize) {
        let ui = self.control.ui().clone();
        {
            let mut state = self.state.borrow_mut();
            let line = line.min(state.buffer.line_count().saturating_sub(1));
            let start = state.buffer.line_start(line);
            let end = state.buffer.line_end(line);
            let position = start + col.min(end - start);
            state.view.caret = position;
            state.view.anchor = position;
            state.view.goal_col = None;
            events::ensure_visible(&mut state, &ui, self.control.id());
        }
        self.control.invalidate();
    }

    /// Replaces the diagnostic markers (squiggles, tints and breakpoints).
    pub fn set_markers(&self, markers: Vec<Marker>) {
        self.state.borrow_mut().markers = markers;
        self.control.invalidate();
    }

    /// Replaces the display options.
    pub fn set_options(&self, options: Options) {
        self.state.borrow_mut().options = options;
        self.control.invalidate();
    }

    /// The id of the caret-blink timer started at construction.
    ///
    /// xui exposes only one window timer mapping
    /// (`Ui::on_timer`) and keeps per-widget timer listeners `pub(crate)`, so
    /// the app forwards ticks to [`Editor::handle_timer`] from its own mapper.
    pub fn timer_id(&self) -> TimerId {
        self.timer
    }

    /// Toggles the caret blink when `id` is this editor's timer, returning
    /// whether the editor repainted.
    pub fn handle_timer(&self, id: TimerId) -> bool {
        if id != self.timer || id == TimerId(0) {
            return false;
        }
        let mut state = self.state.borrow_mut();
        if !state.focused {
            return false;
        }
        state.toggle_blink();
        drop(state);
        self.control.invalidate();
        true
    }

    /// The selected char range, ordered, or `None` when nothing is selected.
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.state.borrow().view.selection()
    }
}

impl<M: 'static> Drop for Editor<M> {
    fn drop(&mut self) {
        // The blink timer belongs to the window; stop it with the widget.
        if self.timer != TimerId(0) {
            self.control.ui().kill_timer(self.timer);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::markers::{Marker, MarkerKind};
    use crate::options::Options;
    use crate::state::EditorState;

    #[test]
    fn default_options_are_monospace_friendly() {
        let options = Options::default();
        assert_eq!(options.tab_width, 4);
        assert!(options.show_gutter);
    }

    #[test]
    fn a_marker_can_be_attached_to_a_diagnostic_span() {
        let marker = Marker::new(4, 2, 9, MarkerKind::Error);
        assert!(marker.has_span());
    }

    #[test]
    fn the_state_exposes_selection_over_the_buffer() {
        let state = EditorState::new("abc\ndef", Options::default(), Box::new(SafeClipboard));
        assert_eq!(state.buffer.line_count(), 2);
    }

    #[test]
    fn the_widget_builds_and_paints_through_an_offscreen_backend() {
        use std::cell::RefCell;
        use std::rc::Rc;

        use xui_canvas::OffscreenBackend;
        use xui_core::backend::PlatformSpec;
        use xui_core::geometry::Rect;
        use xui_core::units::Dip;
        use xui_core::{App, Image, run_app};

        struct Empty;

        impl App for Empty {
            type Msg = ();
            fn update(&mut self, _msg: (), _ui: &mut xui_core::Ui<()>) {}
        }

        let backend = Rc::new(OffscreenBackend::new());
        let shot: Rc<RefCell<Option<Image>>> = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&shot);
        run_app(
            backend,
            PlatformSpec::new("editor").size(Dip(200.0), Dip(120.0)),
            move |ui| {
                let editor = crate::Editor::new(ui, Rect::new(0, 0, 200, 120)).expect("editor");
                editor.set_text("hello\nworld");
                *sink.borrow_mut() = ui.capture().ok();
                Empty
            },
        )
        .expect("run_app");

        let captured = shot.borrow();
        let image = captured.as_ref().expect("the editor painted something");
        assert!(image.width() > 0 && image.height() > 0);
    }

    use crate::platform::Clipboard;

    struct SafeClipboard;

    impl Clipboard for SafeClipboard {
        fn text(&self) -> Option<String> {
            None
        }
        fn set_text(&self, _text: &str) {}
    }
}
