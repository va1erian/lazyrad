#![forbid(unsafe_code)]

//! The LazyRAD custom code-editor widget.
//!
//! A `ropey`-backed buffer with a monospace-grid view, a hand-written Rhai
//! lexer for highlighting, compile diagnostics and metadata-driven completion.
//! It is a single `xui_core` `Custom` node with a painter and an event mapper,
//! not a Scintilla port, so it stays pure Rust and needs no C++ toolchain on
//! LazyOS. See PLAN.md §5.
