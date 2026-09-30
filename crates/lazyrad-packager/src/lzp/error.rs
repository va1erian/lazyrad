#![forbid(unsafe_code)]

//! Why a package could not be built or installed.

use std::path::PathBuf;

use crate::error::PayloadError;
use crate::lzp::zip::ZipError;

/// A failure building a `.lzp` package.
#[derive(Debug, thiserror::Error)]
pub enum LzpError {
    /// The project's files could not be gathered (missing, a link, a bad name,
    /// too large, or the `.lrp` itself is invalid).
    #[error(transparent)]
    Project(#[from] PayloadError),

    /// The project did not pass the pre-package check (compile errors). One
    /// line per problem.
    #[error("the project has problems:\n  {}", .0.join("\n  "))]
    Check(Vec<String>),

    /// The player binary is unusable.
    #[error("the player `{}` cannot be packaged: {reason}", path.display())]
    Player {
        /// The player file.
        path: PathBuf,
        /// Why.
        reason: String,
    },

    /// The manifest fields are invalid. One line per problem.
    #[error("the package manifest is invalid:\n  {}", .0.join("\n  "))]
    Manifest(Vec<String>),

    /// An icon is not a PNG.
    #[error("icon {name} is not a PNG file")]
    Icon {
        /// The icon entry name.
        name: String,
    },

    /// The archive rules refused an entry or the total.
    #[error(transparent)]
    Zip(#[from] ZipError),

    /// Reading or writing a file failed.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}
