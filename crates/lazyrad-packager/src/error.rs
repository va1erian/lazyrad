#![forbid(unsafe_code)]

//! The packager's errors: one for reading or building a payload, one for the
//! whole export. Every message says what is wrong in plain words; none of
//! these paths panics on hostile input.

use std::path::PathBuf;

/// A payload that cannot be built, read or trusted.
#[derive(Debug, thiserror::Error)]
pub enum PayloadError {
    /// The payload footer is present but names a format this build does not
    /// understand.
    #[error(
        "this executable's project data is format version {found}, but this player understands version {supported}"
    )]
    UnsupportedVersion {
        /// The version the footer declares.
        found: u32,
        /// The version this build reads and writes.
        supported: u32,
    },

    /// The file ends before the payload the footer describes does.
    #[error("the project data appended to this executable is truncated: {0}")]
    Truncated(String),

    /// The payload is present but does not check out (bad checksum, bad entry
    /// framing).
    #[error("the project data appended to this executable is corrupted: {0}")]
    Corrupt(String),

    /// The payload, or one entry, is larger than the format allows.
    #[error("the project is too large to pack: {0}")]
    TooLarge(String),

    /// The payload has more entries than the format allows.
    #[error("the project has too many files ({count}; at most {max})")]
    TooManyEntries {
        /// How many entries were found.
        count: usize,
        /// The most the format allows.
        max: usize,
    },

    /// An entry name is not a plain project file name.
    #[error("`{name}` is not a valid project file name: {reason}")]
    BadName {
        /// The offending name.
        name: String,
        /// Why it was refused.
        reason: &'static str,
    },

    /// Two entries have the same name (ignoring case).
    #[error("the project file `{0}` appears twice")]
    Duplicate(String),

    /// The payload must hold exactly one `.lrp`.
    #[error("the project data must contain exactly one .lrp file, found {0}")]
    ProjectFileCount(usize),

    /// A project file could not be read.
    #[error("cannot read `{}`: {source}", path.display())]
    Io {
        /// The file (or executable) involved.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        source: std::io::Error,
    },

    /// A project file is not a regular file (a link, a folder, a device).
    #[error("`{}` is not a regular file", .0.display())]
    NotRegular(PathBuf),

    /// The `.lrp` itself is not valid.
    #[error(transparent)]
    Project(#[from] lazyrad_project::Error),
}

impl PayloadError {
    /// An I/O failure on `path`.
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> PayloadError {
        PayloadError::Io {
            path: path.into(),
            source,
        }
    }
}

/// A failed export. The output file is never left half-written.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// The project could not be packed.
    #[error(transparent)]
    Payload(#[from] PayloadError),

    /// The stub (the player executable to copy) is unusable.
    #[error("the player stub `{}` cannot be used: {reason}", path.display())]
    Stub {
        /// The stub path.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },

    /// The output path is not one the export may write.
    #[error("cannot export to `{}`: {reason}", path.display())]
    Output {
        /// The output path.
        path: PathBuf,
        /// Why it was refused.
        reason: String,
    },

    /// The project's icon is not a usable `.ico`.
    #[error("the project icon `{}` cannot be used: {reason}", path.display())]
    Icon {
        /// The icon path.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },

    /// Setting the Windows resources or subsystem on the stub failed.
    #[error("cannot patch the Windows executable: {0}")]
    Patch(String),

    /// Writing the output failed.
    #[error("cannot write `{}`: {source}", path.display())]
    Write {
        /// The output path.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        source: std::io::Error,
    },
}
