#![forbid(unsafe_code)]

//! Exporting a LazyRAD project as a self-contained executable (PLAN.md §8).
//!
//! An exported app is a copy of the player (the *stub*) with the project's
//! `.lrp`, `.lfm` and `.rhai` files appended as a [`Payload`] and a fixed
//! footer. The player finds the footer in its own executable at startup and
//! runs the project from memory; nothing is extracted to disk.
//!
//! * [`payload`] is the archive format, its limits and its reader. It is the
//!   only place that interprets bytes from an executable, and it treats them as
//!   untrusted: a truncated, corrupted, oversized or wrong-version payload is
//!   an error value, never a panic.
//! * [`export`] builds the output and writes it atomically.
//! * [`pe`] patches a Windows stub: the GUI subsystem (so no console window
//!   appears), the project's icon and its version resources.
//!
//! Payload entry names are plain file names, checked with the same rule as
//! `.lrp` item paths ([`lazyrad_project::is_plain_file_name`]), so a crafted
//! payload cannot name a file outside the project. Code signing is out of
//! scope: appending a payload invalidates any signature the stub had.

pub mod error;
pub mod export;
pub mod payload;
pub mod pe;
pub mod platform;

pub use error::{ExportError, PayloadError};
pub use export::{ExportReport, ExportRequest, atomic_write, export};
pub use payload::{Entry, FORMAT_VERSION, Payload};
