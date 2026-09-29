#![forbid(unsafe_code)]

//! The LazyRAD project file model.
//!
//! A project is a directory of plain-text files: an `.lrp` project file naming
//! the startup form or module, `.lfm` form layouts and `.rhai` code-behind. The
//! form layout itself is an [`xui_form::FormDoc`]; LazyRAD no longer has a form
//! model of its own. This crate owns the `.lrp` file, the load/save round trip
//! for it, the [`lazyrad_catalog`] (the portable catalog plus VB-style names)
//! and the project-level validation.
//!
//! The crate is deliberately dependency-light: it is the shared vocabulary used
//! by the runtime, the designer and the IDE, and never depends on any of them.

pub mod error;
pub mod io;
pub mod model;
pub mod schema;
pub mod validate;

pub use error::{Diagnostic, DiagnosticKind, Error};
pub use io::{
    CODE_EXTENSION, FORM_EXTENSION, PROJECT_EXTENSION, SaveReport, load_form, parse_form,
    save_form, write_if_changed,
};
pub use model::{Project, ProjectItem, is_plain_file_name};
pub use schema::lazyrad_catalog;
pub use xui_form::{Catalog, FormDoc, Node, Value};
