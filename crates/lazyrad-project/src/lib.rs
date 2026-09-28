#![forbid(unsafe_code)]

//! The LazyRAD project and form file model.
//!
//! A project is a directory of plain-text files: an `.lrp` project file naming
//! the startup form or module, `.lfm` form layouts (TOML) and `.rhai`
//! code-behind. This crate owns their in-memory types, the `serde` load/save
//! round trip, the control schema registry and validation. See PLAN.md §3.
//!
//! The crate is deliberately dependency-light: it is the shared vocabulary
//! used by the runtime, the designer and the IDE, and never depends on any of
//! them.
//!
//! # Layout
//!
//! * [`model`] — [`Project`], [`ProjectItem`], [`Form`], [`Control`] and
//!   [`PropValue`].
//! * [`schema`] — the control schema registry the runtime, designer and
//!   completion share.
//! * [`io`] — [`Project::load`]/[`Project::save`] and [`Form::load`]/
//!   [`Form::save`], writing only files whose bytes changed.
//! * [`validate`] — [`Project::validate`], which reports every problem as a
//!   [`Diagnostic`] carrying its file and, where possible, a line.

pub mod error;
pub mod io;
pub mod model;
pub mod schema;
pub mod validate;

pub use error::{Diagnostic, DiagnosticKind, Error};
pub use io::{CODE_EXTENSION, FORM_EXTENSION, PROJECT_EXTENSION, SaveReport, write_if_changed};
pub use model::{Control, Form, Project, ProjectItem, PropValue};
pub use schema::{ControlSchema, EventSchema, PropertySchema, PropertyType, SchemaRegistry};
