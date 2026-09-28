#![forbid(unsafe_code)]

//! The LazyRAD custom code-editor widget.
//!
//! A `ropey`-backed [`Buffer`] with a monospace-grid view, a hand-written Rhai
//! lexer for highlighting, compile diagnostics and metadata-driven completion.
//! It is a single `xui_core` `Custom` node with a painter and an event mapper,
//! not a Scintilla port, so it stays pure Rust and needs no C++ toolchain on
//! LazyOS. See PLAN.md §5.
//!
//! The buffer, caret, selection, scrolling, editing, clipboard and undo are
//! here, along with the hand-written Rhai [`lexer`] and its incremental
//! highlighting. Completion comes in a later issue.
//!
//! # Layout
//!
//! * [`buffer`] — the rope, line index and coalescing undo stack. No UI code.
//! * [`lexer`] — the UI-free, line-incremental Rhai lexer and bracket matcher.
//! * [`view`] — the caret, selection and scroll state and its navigation rules.
//! * [`editor`] — the [`Editor`] widget that ties them to a `Custom` node.
//! * [`paint`] — the monospace grid painter.
//!
//! # Example
//!
//! ```no_run
//! # use xui_core::app::Ui;
//! # use xui_core::geometry::Rect;
//! # use lazyrad_editor::Editor;
//! # fn build<M: 'static>(ui: &Ui<M>) -> xui_core::backend::Result<()> {
//! let editor = Editor::new(ui, Rect::new(0, 0, 640, 400))?;
//! editor.set_text("fn main() {\n}\n");
//! editor.goto(1, 0);
//! # Ok(())
//! # }
//! ```

pub mod buffer;
mod edit;
mod editor;
mod events;
pub mod lexer;
pub mod markers;
mod metrics;
pub mod options;
mod paint;
pub mod platform;
mod scrollbar;
mod state;
mod text;
pub mod theme;
pub mod view;

pub use buffer::Buffer;
pub use editor::Editor;
pub use lexer::{LexState, LineLexer, Token, TokenClass};
pub use markers::{Marker, MarkerKind};
pub use options::{FontConfig, Options};
pub use platform::Clipboard;
pub use theme::EditorTheme;
pub use view::View;
