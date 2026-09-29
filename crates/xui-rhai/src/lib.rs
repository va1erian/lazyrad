#![forbid(unsafe_code)]

//! Script [`xui-form`](xui_form) forms with [Rhai](https://rhai.rs).
//!
//! This crate is the reusable half of LazyRAD's runtime: everything needed to
//! load a form document, build it into live widgets, run a `.rhai` script
//! against it and route widget events to `fn <control>_<event>` handlers. It
//! knows nothing about LazyRAD projects, so any xui app can use it.
//!
//! * [`value`] is the one place form values and Rhai values are converted;
//! * [`control`] holds the Rhai control and form types (`name_edit.text`,
//!   `form.title`, `form.state`, `form.show`/`hide`);
//! * [`engine`] builds the Rhai [`Engine`](rhai::Engine) and [`EngineHost`],
//!   including the `on_var` resolver, the operation budget and module
//!   registration;
//! * [`form`] wires `<control>_<event>` handlers to widget events through
//!   [`ScriptBinder`] and runs a scripted form through [`ScriptForm`];
//! * [`error`] locates a script failure in its source file;
//! * [`message`] is the vocabulary a script's UI calls leave for the host.
//!
//! # How a script sees controls
//!
//! Rhai functions cannot see the enclosing scope, so `name_edit.text` inside a
//! handler would not resolve by itself. [`EngineHost`] installs an
//! [`Engine::on_var`](rhai::Engine::on_var) resolver that *pushes* the control
//! into the scope rather than returning it: a value returned from `on_var` is
//! read-only, so `name_edit.text = …` would fail, while a pushed variable is a
//! normal mutable entry.
//!
//! # Handler naming
//!
//! A handler is `fn <control>_<event>(args…)` with xui's event name in
//! snake_case: `hello_button_click`, `name_edit_change`, `form_load`,
//! `form_close`. [`handler_name`](form::handler_name) builds the name, and
//! [`ScriptBinder`] wires an event only when the compiled
//! script defines it.

pub mod control;
pub mod engine;
pub mod error;
pub mod form;
pub mod message;
pub mod value;

pub use control::FormHost;
pub use engine::{EngineHost, EngineSetup, new_engine};
pub use error::ScriptError;
pub use form::{FORM, ScriptBinder, ScriptForm, ScriptSource};
pub use message::{Msg, MsgBoxButtons, Pending};
