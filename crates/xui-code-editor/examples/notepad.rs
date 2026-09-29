//! A minimal plain-text notepad: one [`Editor`] filling a window on xui's
//! canvas backend.
//!
//! Run with:
//!
//! ```text
//! cargo run -p xui-code-editor --example notepad
//! ```
//!
//! `XUI_DEMO_AUTOCLOSE_MS` makes it quit itself, for headless smoke runs. The
//! editor defaults to the [`PlainText`](xui_code_editor::PlainText) highlighter;
//! swap in `RhaiHighlighter` (with the `rhai-syntax` feature) to colour Rhai.

use std::cell::Cell;
use std::rc::Rc;

use xui_code_editor::Editor;
use xui_core::Rect;
use xui_core::app::{App, Ui, run_app};
use xui_core::backend::PlatformSpec;
use xui_core::units::Dip;

/// What the notepad app reacts to.
enum Msg {
    /// The buffer changed; the new text drives the window title.
    Edited(String),
    /// Quit, raised by the smoke-test timer.
    Autoclose,
}

/// The notepad app: it only owns the editor, so the example stays minimal.
struct Notepad {
    _editor: Editor<Msg>,
}

impl App for Notepad {
    type Msg = Msg;

    fn update(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
        match msg {
            Msg::Edited(text) => {
                let lines = text.split('\n').count();
                ui.set_window_title(&format!("xui-code-editor — {lines} line(s)"));
            }
            Msg::Autoclose => ui.quit(),
        }
    }
}

fn main() {
    let _ = run_app(
        Rc::new(xui_canvas::WinitBackend::new()),
        PlatformSpec::new("xui-code-editor notepad").size(Dip(800.0), Dip(600.0)),
        |ui| {
            let dpi = ui.dpi();
            let p = move |value: f32| Dip(value).to_px(dpi).value();
            let bounds = Rect::new(p(0.0), p(0.0), p(800.0), p(600.0));
            let editor = Editor::new(ui, bounds)
                .expect("editor")
                .on_change(|text| Some(Msg::Edited(text.to_string())));
            editor.set_text("A minimal xui-code-editor window.\n\nType here.\n");

            let autoclose = Rc::new(Cell::new(None));
            if let Ok(millis) = std::env::var("XUI_DEMO_AUTOCLOSE_MS") {
                let _ = millis
                    .parse::<u32>()
                    .map(|ms| autoclose.set(Some(ui.set_timer(ms))));
            }
            ui.on_timer(move |fired| (autoclose.get() == Some(fired)).then_some(Msg::Autoclose));

            Notepad { _editor: editor }
        },
    );
}
