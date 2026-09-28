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
//! window the player and IDE share. [`form`] loads a project's forms, builds
//! them and wires `Control_Event` handlers, [`control`] holds the Rhai control
//! and form types, [`value`] is the one place form values and Rhai values are
//! converted, [`message`] carries the messages a script leaves for the host,
//! [`stdlib`] is the Iteration 1 standard library, and [`error`] locates a
//! script failure.

pub mod control;
pub mod engine;
pub mod error;
pub mod form;
pub mod message;
pub mod shell;
pub mod stdlib;
pub mod value;

pub use control::FormHost;
pub use engine::{EngineHost, new_engine};
pub use error::ScriptError;
pub use form::{
    FormApp, FormRuntime, FormSource, ModuleSource, Msg, RuntimeError, run_project,
    run_project_with,
};
pub use message::{MsgBoxButtons, Pending};
pub use stdlib::StdlibContext;
