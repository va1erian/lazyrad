#![forbid(unsafe_code)]

//! The LazyRAD runtime.
//!
//! This is the project, standard-library and player glue on top of
//! [`xui_rhai`], the reusable "script an `xui-form` with Rhai" crate. It loads
//! a `.lrp` project and its forms, registers the LazyRAD standard library
//! (`msg_box`, `app`, `random`, `now`/`today`, …) and manages multiple forms
//! and secondary windows. The player uses it to run programs and the IDE uses
//! it for the designer preview, completion metadata and syntax checks. It never
//! depends on an IDE crate. See PLAN.md §4.
//!
//! [`form`] loads a project's forms, builds them and wires `Control_Event`
//! handlers, [`stdlib`] is the Iteration 1 standard library, [`check`] collects
//! the problems that would stop a project from starting, and [`shell`] opens
//! the toolkit window the player and IDE share. [`extensions`] lets a host
//! add its own script functions to every engine (the LazyOS player's `msg`). The Rhai engine host, the
//! control and form Rhai types, value conversion and error locating live in
//! [`xui_rhai`] and are re-exported here so LazyRAD's callers see one crate.

pub mod check;
pub mod engine;
pub mod extensions;
pub mod form;
pub mod fs_policy;
pub mod platform;
pub mod shell;
pub mod stdlib;

pub use check::{CheckReport, check_project, check_runtime};
pub use engine::{EngineHost, new_engine};
pub use form::{
    FormApp, FormRuntime, FormSource, HandlerObserver, ModuleSource, Msg, RuntimeError,
    run_project_with, run_runtime_with,
};
#[cfg(feature = "desktop")]
pub use form::{run_project, run_runtime};
pub use fs_policy::{Access, FsError, FsPolicy, Sandbox};
pub use stdlib::StdlibContext;
pub use xui_rhai::message::{MsgBoxButtons, Pending};
pub use xui_rhai::{FormHost, ScriptError};
