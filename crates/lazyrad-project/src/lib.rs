#![forbid(unsafe_code)]

//! The LazyRAD project and form file model.
//!
//! A project is a directory of plain-text files: an `.lrp` project file naming
//! the startup form or module, `.lfm` form layouts (TOML) and `.rhai`
//! code-behind. This crate owns their in-memory types, the `serde`
//! load/save round trip and validation. See PLAN.md §3.
//!
//! The crate is deliberately dependency-light: it is the shared vocabulary
//! used by the runtime, the designer and the IDE, and never depends on any of
//! them.
