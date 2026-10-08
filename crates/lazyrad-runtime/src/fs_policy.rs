#![forbid(unsafe_code)]

//! Which filesystem paths a script may touch.
//!
//! A script is untrusted code, so the `file`/`dir`/`path` standard-library
//! modules will ask an [`FsPolicy`] to turn the path a script supplies into a
//! real path before they open it. On LazyOS that policy is
//! [`FsPolicy::Sandboxed`]; the desktop player uses [`FsPolicy::Unrestricted`]
//! (PLAN.md §4.4). This module is pure: it reads the filesystem only to
//! resolve links, never creates or changes anything, and pulls in no
//! dependency beyond `std`.
//!
//! # What "safe" means
//!
//! For a sandbox a path is safe when the location it really lands in — after
//! resolving every symbolic link in the part that already exists — is inside
//! the private root or an explicitly allowed entry that grants the requested
//! access. The check canonicalises the deepest existing ancestor and appends
//! the not-yet-existing tail, so `a/b/new.txt` under the root is allowed
//! before `a/b` exists, while a link inside the root that points outside is
//! refused. A write whose own final component is an existing link is always
//! refused, even if that link points back inside the root.
//!
//! # What is not handled
//!
//! Canonicalisation happens before the caller opens the path, so a
//! time-of-check/time-of-use race remains: another process could replace a
//! directory in the checked path with a link between [`FsPolicy::resolve`] and
//! the open. Callers should open the path [`FsPolicy::resolve`] returns, which
//! is the canonical location, to keep that window as small as `std` allows.
//! The check is also deliberately conservative on a case-insensitive
//! filesystem: a differently-cased path may be denied even when the OS would
//! open it, which fails safe.

use std::cell::RefCell;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

/// The longest script-supplied path a sandbox accepts, in bytes. A longer
/// path is refused before any filesystem call, so a hostile script cannot
/// hand the OS a path it is not built to handle.
const MAX_PATH_LEN: usize = 4096;

/// What a script wants to do with a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Open an existing path and read it.
    Read,
    /// Create, truncate or replace a path.
    Write,
}

impl Access {
    /// Whether a grant of `self` is enough for a request of `requested`.
    ///
    /// Write is the stronger grant: a path a script may write is also a path
    /// it may read. The reverse does not hold.
    fn allows(self, requested: Access) -> bool {
        match self {
            Access::Write => true,
            Access::Read => requested == Access::Read,
        }
    }
}

/// The policy that decides which filesystem paths a script may touch.
///
/// This is the value the IO standard library holds: the player installs
/// [`FsPolicy::Unrestricted`] and a sandboxed host installs
/// [`FsPolicy::Sandboxed`]. A policy is immutable; resolving never mutates it,
/// so it can be shared for the lifetime of a run.
#[derive(Clone, Debug)]
pub enum FsPolicy {
    /// Every path is allowed. Only the input checks (empty and NUL) apply, so
    /// the desktop player keeps its filesystem access (PLAN.md §4.4).
    Unrestricted,
    /// Only paths under the private root, or explicitly allowed, are
    /// reachable.
    Sandboxed(Sandbox),
}

/// A restricted view of the filesystem.
///
/// A sandbox grants read and write below its private root, plus read and/or
/// write to the extra files and directories the host lets the user pick. The
/// allowed list is shared and mutable through [`Sandbox::allow_runtime`], so a
/// file the user picks from a running program's `open_file_dialog` can be
/// granted after the sandbox was built; cloning a sandbox shares that list.
#[derive(Clone, Debug)]
pub struct Sandbox {
    /// The directory a relative request is resolved against and the only
    /// directory reachable by default.
    private_root: PathBuf,
    /// Extra user-picked files or directories and the access each grants.
    allowed: Rc<RefCell<Vec<Allowed>>>,
}

/// One entry the host added with [`Sandbox::allow`] or
/// [`Sandbox::allow_children`].
#[derive(Clone, Debug)]
struct Allowed {
    /// The user-picked file or directory.
    path: PathBuf,
    /// The access this entry grants.
    access: Access,
    /// How much of a directory entry the grant reaches.
    scope: Scope,
}

/// How much of a directory an [`Allowed`] entry covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    /// The directory and everything below it.
    Subtree,
    /// The directory itself (to list it) and the files directly in it; not
    /// its subdirectories or anything below them. A viewer that pages
    /// through a picked file's neighbours needs no more.
    Children,
}

/// Why a path was refused.
///
/// `path` is the script-supplied string exactly as it arrived, so the caller
/// can report it back to the user. `reason` is a short human-readable phrase
/// that says which rule refused it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsError {
    /// The path is well formed but the policy does not grant `access`.
    Denied { path: String, reason: String },
    /// The path itself is malformed: empty, containing a NUL byte, or (in a
    /// sandbox) longer than 4096 bytes.
    Invalid { path: String, reason: String },
}

impl FsError {
    /// A [`FsError::Denied`] for `path` with `reason`.
    fn denied(path: &str, reason: impl Into<String>) -> FsError {
        FsError::Denied {
            path: path.to_owned(),
            reason: reason.into(),
        }
    }

    /// A [`FsError::Invalid`] for `path` with `reason`.
    fn invalid(path: &str, reason: impl Into<String>) -> FsError {
        FsError::Invalid {
            path: path.to_owned(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::Denied { path, reason } => {
                write!(formatter, "access denied for {path:?}: {reason}")
            }
            FsError::Invalid { path, reason } => {
                write!(formatter, "invalid path {path:?}: {reason}")
            }
        }
    }
}

impl std::error::Error for FsError {}

impl FsPolicy {
    /// Resolve `requested` (script-supplied, relative paths are relative to the
    /// private root) to a path that is safe to open for `access`.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Invalid`] for an empty path, a path containing a NUL
    /// byte or — in a sandbox — a path longer than 4096 bytes, and
    /// [`FsError::Denied`] when the resolved location is not covered by the
    /// policy. Nothing is created or modified, whether the call succeeds or
    /// fails.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn resolve(&self, requested: &str, access: Access) -> Result<PathBuf, FsError> {
        validate_input(requested)?;
        match self {
            FsPolicy::Unrestricted => Ok(PathBuf::from(requested)),
            FsPolicy::Sandboxed(sandbox) => sandbox.resolve(requested, access),
        }
    }

    /// Grants `path` the given `access` at run time, so a file the user picked
    /// can be read afterwards.
    ///
    /// On [`FsPolicy::Unrestricted`] this does nothing: every path is already
    /// allowed. On a sandbox it adds an entry, limited to the exact file or
    /// directory, to the shared allowed list.
    pub fn allow_runtime(&self, path: PathBuf, access: Access) {
        if let FsPolicy::Sandboxed(sandbox) = self {
            sandbox.allow_runtime(path, access);
        }
    }

    /// Grants the directory `dir` and the files directly in it `access` at run
    /// time ([`Sandbox::allow_children`]); nothing on
    /// [`FsPolicy::Unrestricted`].
    pub fn allow_children_runtime(&self, dir: PathBuf, access: Access) {
        if let FsPolicy::Sandboxed(sandbox) = self {
            sandbox.allow_children_runtime(dir, access);
        }
    }
}

impl Sandbox {
    /// A sandbox that grants read and write below `private_root`.
    ///
    /// `private_root` should be absolute; a relative root is resolved against
    /// the process working directory when a path is checked. Creating the
    /// sandbox does not touch the filesystem, so the root may not exist yet.
    pub fn new(private_root: PathBuf) -> Sandbox {
        Sandbox {
            private_root,
            allowed: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Allow `path`, granting it `access`, and return the sandbox.
    ///
    /// `path` is a file or a directory the user picked. A directory grant
    /// covers its contents; a file grant covers that path only. If `path` does
    /// not exist it is treated as a file, so a to-be-created file can be
    /// allowed but not the not-yet-existing children of a missing directory.
    #[must_use]
    pub fn allow(self, path: PathBuf, access: Access) -> Sandbox {
        self.allow_runtime(path, access);
        self
    }

    /// Grants `path` the given `access` on a sandbox that is already built.
    ///
    /// This is how a path the user picks in a running program is added to the
    /// sandbox after the fact. It is shared with every clone of the sandbox, so
    /// all of a runtime's forms see the grant.
    pub fn allow_runtime(&self, path: PathBuf, access: Access) {
        self.allowed.borrow_mut().push(Allowed {
            path,
            access,
            scope: Scope::Subtree,
        });
    }

    /// Allow the directory `dir` itself and the files directly in it, granting
    /// them `access`, and return the sandbox. Subdirectories and anything
    /// below them stay outside, and a filesystem root (`/`) is never granted
    /// this way.
    #[must_use]
    pub fn allow_children(self, dir: PathBuf, access: Access) -> Sandbox {
        self.allow_children_runtime(dir, access);
        self
    }

    /// [`Sandbox::allow_children`] on a sandbox that is already built, shared
    /// with every clone like [`Sandbox::allow_runtime`].
    pub fn allow_children_runtime(&self, dir: PathBuf, access: Access) {
        self.allowed.borrow_mut().push(Allowed {
            path: dir,
            access,
            scope: Scope::Children,
        });
    }

    /// The sandboxed half of [`FsPolicy::resolve`].
    fn resolve(&self, requested: &str, access: Access) -> Result<PathBuf, FsError> {
        if requested.len() > MAX_PATH_LEN {
            return Err(FsError::invalid(
                requested,
                "path is longer than 4096 bytes",
            ));
        }

        let candidate = self.candidate(requested);
        if access == Access::Write && is_symlink(&candidate) {
            return Err(FsError::denied(
                requested,
                "refusing to write through a symbolic link",
            ));
        }

        let Some(resolved) = resolve_real(&candidate) else {
            return Err(FsError::denied(
                requested,
                "no existing ancestor to resolve",
            ));
        };
        if self.grants(&resolved, access) {
            Ok(resolved)
        } else {
            Err(FsError::denied(requested, "path is outside the sandbox"))
        }
    }

    /// Join a relative `requested` to the root and normalise the result
    /// lexically, without touching the filesystem. An absolute `requested` is
    /// normalised as-is; the containment check decides whether it is allowed.
    fn candidate(&self, requested: &str) -> PathBuf {
        let requested_path = Path::new(requested);
        let joined = if requested_path.is_absolute() {
            requested_path.to_path_buf()
        } else {
            self.private_root.join(requested_path)
        };
        normalize_lexically(&joined)
    }

    /// Whether `resolved` is inside the private root or an allowed entry with
    /// enough access. `resolved` and the roots are canonical, so the
    /// comparison is component-wise and `/root` never matches `/root-evil`.
    fn grants(&self, resolved: &Path, access: Access) -> bool {
        if resolve_real(&self.private_root).is_some_and(|root| resolved.starts_with(&root)) {
            return true;
        }
        self.allowed
            .borrow()
            .iter()
            .any(|entry| entry.grants(resolved, access))
    }
}

impl Allowed {
    /// Whether this entry covers `resolved` for `access`.
    fn grants(&self, resolved: &Path, access: Access) -> bool {
        if !self.access.allows(access) {
            return false;
        }
        let Some(entry) = resolve_real(&self.path) else {
            return false;
        };
        if !is_existing_dir(&entry) {
            // A file entry matches only itself.
            return resolved == entry;
        }
        match self.scope {
            // `starts_with` compares whole components, so `/a/b` does not
            // cover `/a/bc`.
            Scope::Subtree => resolved.starts_with(&entry),
            // The listing and the files in it. A root has no parent: granting
            // its children would be most of the filesystem, so it grants
            // nothing. `resolved` is canonical, so a link in the directory
            // that points elsewhere has another parent and is refused.
            Scope::Children => {
                entry.parent().is_some()
                    && (resolved == entry
                        || (resolved.parent() == Some(entry.as_path())
                            && !is_existing_dir(resolved)))
            }
        }
    }
}

/// Reject the malformed inputs both policies share.
fn validate_input(requested: &str) -> Result<(), FsError> {
    if requested.is_empty() {
        return Err(FsError::invalid(requested, "path is empty"));
    }
    if requested.contains('\0') {
        return Err(FsError::invalid(requested, "path contains a NUL byte"));
    }
    Ok(())
}

/// Normalise a path without touching the filesystem: drop `.`, and let `..`
/// remove the preceding component. A leading `..` on an absolute path cannot
/// remove the root, so an escaping request collapses to a path outside the
/// root and is denied by the containment check rather than traversed. Because
/// this is lexical, a `..` never follows a symbolic link, which is what makes
/// it safe to combine with the later canonicalisation.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

/// The real location `path` would open, or `None` when no ancestor exists.
///
/// The deepest existing ancestor is canonicalised (which resolves its links)
/// and the missing tail is appended as literal names, so a new file under an
/// existing directory resolves without the file existing. A relative path is
/// resolved against the working directory by [`fs::canonicalize`].
fn resolve_real(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    let mut tail: Vec<OsString> = Vec::new();
    loop {
        if let Ok(real) = fs::canonicalize(&current) {
            let mut resolved = real;
            for part in tail.iter().rev() {
                resolved.push(part);
            }
            return Some(resolved);
        }
        // A link that exists but does not resolve (dangling) must not be
        // mistaken for a missing name: creating through it would land wherever
        // it points, possibly outside the sandbox.
        if is_symlink(&current) {
            return None;
        }
        let name = current.file_name()?.to_os_string();
        let parent = current.parent()?;
        if parent.as_os_str().is_empty() {
            return None;
        }
        tail.push(name);
        current = parent.to_path_buf();
    }
}

/// Whether `path` itself is a symbolic link, without following it.
fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// Whether `path` names an existing directory, following links.
fn is_existing_dir(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}
