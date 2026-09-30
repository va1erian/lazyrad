#![forbid(unsafe_code)]

//! TEMPORARY STUB of `fs_policy.rs`; REPLACE WITH THE REAL MODULE.
//!
//! The real `fs_policy` module is being written on the `oc-fs-policy` branch and
//! is merged later. This stub has the **same public signature** the plan fixes
//! (`Access`, `FsPolicy::{Unrestricted, Sandboxed(Sandbox)}`,
//! `Sandbox::new(root).allow(path, Access)`, `FsPolicy::resolve`) so the stdlib
//! and the platform seam can be written against it now. When the real module
//! lands: delete this file, rename the `mod` line in `lib.rs` to `fs_policy`,
//! and adapt `FsError` uses (only `Display` is relied on outside this file).
//!
//! It is deliberately small but not naive: paths are normalised lexically
//! (`.` and `..` resolved without touching the disk), absolute and relative
//! requests are both checked against the allow-list, and the deepest existing
//! ancestor is canonicalised so a symlink inside an allowed directory cannot
//! lead out of it.

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// What a script wants to do to a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Read or list.
    Read,
    /// Create, write, append or remove.
    Write,
}

/// Why a path was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum FsError {
    /// The path is outside every directory the sandbox allows for this access.
    Denied(String),
    /// The path is empty or contains a NUL byte.
    Invalid(String),
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::Denied(path) => write!(f, "access to `{path}` is not allowed"),
            FsError::Invalid(path) => write!(f, "`{path}` is not a valid path"),
        }
    }
}

impl std::error::Error for FsError {}

/// A root directory plus extra allowed directories.
#[derive(Clone, Debug)]
pub struct Sandbox {
    root: PathBuf,
    allowed: Vec<(PathBuf, Access)>,
}

impl Sandbox {
    /// A sandbox whose `root` is readable and writable and is where relative
    /// paths resolve.
    pub fn new(root: impl Into<PathBuf>) -> Sandbox {
        Sandbox {
            root: normalize(&root.into()),
            allowed: Vec::new(),
        }
    }

    /// Additionally allows `access` (and, for `Write`, reading too) under `path`.
    pub fn allow(mut self, path: impl Into<PathBuf>, access: Access) -> Sandbox {
        self.allowed.push((normalize(&path.into()), access));
        self
    }
}

/// How the file stdlib decides which paths a script may touch.
#[derive(Clone, Debug, Default)]
pub enum FsPolicy {
    /// The desktop default: anything the OS user can reach.
    #[default]
    Unrestricted,
    /// The LazyOS default: only the sandbox's directories.
    Sandboxed(Sandbox),
}

impl FsPolicy {
    /// The path a script's `requested` string names, or why it is refused.
    pub fn resolve(&self, requested: &str, access: Access) -> Result<PathBuf, FsError> {
        if requested.is_empty() || requested.contains('\0') {
            return Err(FsError::Invalid(requested.to_owned()));
        }
        let FsPolicy::Sandboxed(sandbox) = self else {
            return Ok(PathBuf::from(requested));
        };
        let wanted = Path::new(requested);
        let full = if wanted.is_absolute() {
            normalize(wanted)
        } else {
            normalize(&sandbox.root.join(wanted))
        };
        let real = canonical_prefix(&full);
        let permitted = std::iter::once(&(sandbox.root.clone(), Access::Write))
            .chain(sandbox.allowed.iter())
            .any(|(dir, granted)| {
                let covers = *granted == Access::Write || access == Access::Read;
                covers && full.starts_with(dir) && real.starts_with(canonical_prefix(dir))
            });
        if permitted {
            Ok(full)
        } else {
            Err(FsError::Denied(requested.to_owned()))
        }
    }
}

/// Resolves `.` and `..` without touching the disk. A `..` that would climb
/// above the root of the path is dropped (it cannot leave the filesystem).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    continue;
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path` with its deepest existing ancestor canonicalised (symlinks
/// resolved) and the not-yet-existing tail appended unchanged.
fn canonical_prefix(path: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut probe = path.to_path_buf();
    loop {
        if let Ok(real) = probe.canonicalize() {
            return tail.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (probe.file_name().map(|n| n.to_owned()), probe.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                probe = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lazyrad-fsp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn unrestricted_allows_anything_but_empty() {
        let policy = FsPolicy::Unrestricted;
        assert!(policy.resolve("/etc/passwd", Access::Read).is_ok());
        assert!(matches!(
            policy.resolve("", Access::Read),
            Err(FsError::Invalid(_))
        ));
    }

    #[test]
    fn relative_paths_resolve_under_the_root() {
        let root = scratch("rel");
        let policy = FsPolicy::Sandboxed(Sandbox::new(&root));
        let got = policy.resolve("a/b.txt", Access::Write).expect("inside");
        assert_eq!(got, normalize(&root.join("a/b.txt")));
    }

    #[test]
    fn traversal_and_absolute_escapes_are_denied() {
        let root = scratch("esc");
        let policy = FsPolicy::Sandboxed(Sandbox::new(&root));
        assert!(policy.resolve("../x", Access::Read).is_err());
        assert!(policy.resolve("a/../../x", Access::Read).is_err());
        assert!(policy.resolve("/etc/passwd", Access::Read).is_err());
        assert!(policy.resolve("a/../ok", Access::Read).is_ok());
    }

    #[test]
    fn extra_grants_respect_their_access() {
        let root = scratch("grant");
        let other = scratch("grant-other");
        let policy = FsPolicy::Sandboxed(Sandbox::new(&root).allow(&other, Access::Read));
        let file = other.join("f.txt");
        assert!(policy.resolve(file.to_str().unwrap(), Access::Read).is_ok());
        assert!(
            policy
                .resolve(file.to_str().unwrap(), Access::Write)
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_root_is_denied() {
        let root = scratch("link");
        let outside = scratch("link-outside");
        std::os::unix::fs::symlink(&outside, root.join("out")).expect("symlink");
        let policy = FsPolicy::Sandboxed(Sandbox::new(&root));
        assert!(policy.resolve("out/secret", Access::Read).is_err());
    }
}
