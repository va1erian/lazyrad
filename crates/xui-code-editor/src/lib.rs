#![forbid(unsafe_code)]

//! A reusable code-editor widget for [xui](https://github.com/va1erian/xui).
//!
//! It is one `xui_core` `Custom` node with a painter and an event mapper, built
//! over pure modules: a `ropey`-backed text buffer with transactional undo, a
//! monospace-grid view, selection and scrolling, find/replace, diagnostics and
//! syntax highlighting. Nothing here needs a C++ toolchain or an OS clipboard,
//! so it also runs on small xui targets.
//!
//! Highlighting is pluggable: the editor holds a [`Highlighter`] behind a box,
//! so it is not generic over the language. [`PlainText`] is the default and
//! emits no tokens; the hand-written, line-incremental Rhai lexer is available
//! as `RhaiHighlighter` with the `rhai-syntax` feature. Bracket matching works
//! with any highlighter (a plain-text cache treats every bracket as code).
//!
//! # Embedding
//!
//! ```no_run
//! # use xui_core::app::Ui;
//! # use xui_core::geometry::Rect;
//! # use xui_code_editor::Editor;
//! # fn build<M: 'static>(ui: &Ui<M>) -> xui_core::backend::Result<()> {
//! let editor = Editor::new(ui, Rect::new(0, 0, 640, 400))?
//!     .on_change(|text| {
//!         // Return `Some(message)` to raise an app message, or `None`.
//!         let _ = text;
//!         None
//!     });
//! editor.set_text("fn main() {\n}\n");
//! # Ok(())
//! # }
//! ```
//!
//! # Features
//!
//! * `system-clipboard` (default): copy and paste through the OS clipboard with
//!   `arboard`. With `default-features = false` the editor falls back to an
//!   in-process clipboard, so it builds with no OS clipboard at all.
//! * `rhai-syntax`: the [`RhaiHighlighter`]. It is off by default, so a
//!   language-free editor carries no language rules.
//!
//! # Layout
//!
//! * [`buffer`] — the rope, line index and coalescing undo stack. No UI code.
//! * [`lexer`] — the [`Highlighter`] trait, the incremental cache and the
//!   optional Rhai lexer.
//! * [`view`] — the caret, selection and scroll state and its navigation rules.
//! * [`editor`] — the [`Editor`] widget that ties them to a `Custom` node.
//! * [`paint`] — the monospace grid painter.
//! * [`platform`] — the [`Clipboard`] seam.
//!
//! # Limitations
//!
//! * A monospace grid only: proportional fonts do not line up.
//! * No word wrap.
//! * No IME or composition events, so it cannot enter CJK text.
//! * Columns are char counts, so wide CJK characters and emoji do not line up
//!   with the grid; the host should use a monospace font with predictable
//!   advances.

pub mod buffer;
mod edit;
mod editor;
mod events;
pub mod find;
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
pub use find::Query;
pub use lexer::{HighlightCache, Highlighter, LineState, PlainText, Token, TokenClass};
pub use markers::{Marker, MarkerKind};
pub use options::{FontConfig, Options};
pub use platform::Clipboard;
pub use theme::EditorTheme;
pub use view::View;

#[cfg(feature = "rhai-syntax")]
pub use lexer::RhaiHighlighter;
