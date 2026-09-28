#![forbid(unsafe_code)]

//! The LazyRAD Rhai runtime.
//!
//! This is the core of the system. It sets up the [`rhai::Engine`] with the
//! `debugging`, `metadata` and `internals` features (but not `sync`), registers
//! the LazyRAD stdlib, instantiates forms from the [`lazyrad_project`] model
//! and binds `Control_Event` functions to the xui widgets. The player uses it
//! to run programs and the IDE uses it for the designer preview, completion
//! metadata and syntax checks. It never depends on an IDE crate. See PLAN.md
//! §4.
//!
//! [`engine`] builds the scripting engine and [`shell`] opens the toolkit
//! window the player and IDE share.

pub mod engine;
pub mod shell;
