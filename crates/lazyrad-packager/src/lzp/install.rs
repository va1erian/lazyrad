#![forbid(unsafe_code)]

//! Installing a built package: the [`Installer`] seam.
//!
//! Installing and registering apps belongs to LazyOS's package system (`pkgd`
//! validates and unpacks into `/data/apps/<install_dir>`, `regd` registers the
//! app so it appears in the Start menu and in `mimed`). LazyRAD consumes that
//! service; it must not grow a second registry. Until `pkgd` exists, only the
//! [`DevInstaller`] fallback is usable.
//!
//! # The `pkgd` client (not implemented yet, on purpose)
//!
//! [`PkgdInstaller`] is the place for it, and it has to be a **MIDL-generated
//! client** (`idl/pkgd.midl`, `midlc`): the LazyOS rules forbid hand-written
//! Messenger method constants and TLV encoders (`AGENTS.md`, "Every interface
//! published on Messenger MUST be defined in a `.midl` file"). The generated
//! client lives in LazyOS's `libs/generated` and is reachable from the LazyOS
//! binaries (`lazyrad-os`), not from this portable crate, so the LazyOS
//! implementation of [`Installer`] is written in `lazyrad-os` when `pkgd`
//! lands and handed to the IDE; this crate only fixes the trait and the
//! result shape. The struct here always reports
//! [`InstallError::Unavailable`], so code written against the trait degrades to
//! the dev fallback instead of failing.

use std::path::PathBuf;

use crate::lzp::{BuiltPackage, LzpError, write_package};

/// Where the dev fallback keeps packages on LazyOS.
pub const DEV_PACKAGE_DIR: &str = "/data/packages";

/// One permission the installer will grant, with the words to show the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionNote {
    /// `interface`, `topic`, `file` or `network`.
    pub kind: String,
    /// The interface name, topic pattern, file rule or `outbound`.
    pub value: String,
    /// `low`, `medium` or `high`.
    pub risk: String,
    /// One friendly sentence.
    pub explanation: String,
}

/// What the installer says it would do with a package, for a consent screen.
///
/// It comes from the installer (`pkgd`'s `Inspect`), not from LazyRAD's own
/// reading of the manifest, so the words are the platform's and what is shown
/// is what will be enforced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageReview {
    /// The display name.
    pub name: String,
    /// The reverse-DNS id.
    pub system_name: String,
    /// The author as declared.
    pub author: String,
    /// The version.
    pub version: String,
    /// The permissions requested.
    pub permissions: Vec<PermissionNote>,
    /// Every reason the package cannot be installed; empty when it can.
    pub problems: Vec<String>,
}

/// What happened to the package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallState {
    /// Installed and registered: it is in the Start menu.
    Installed,
    /// Saved for a later install; nothing is registered.
    SavedNotInstalled,
}

/// The outcome of [`Installer::install`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledApp {
    /// The app's reverse-DNS id.
    pub system_name: String,
    /// Its version.
    pub version: String,
    /// Whether it is installed or only saved.
    pub state: InstallState,
    /// Where the package (or the installed app) is, when the installer knows.
    pub location: Option<PathBuf>,
}

impl InstalledApp {
    /// A line for the IDE's Output pane.
    pub fn summary(&self) -> String {
        match (self.state, &self.location) {
            (InstallState::Installed, _) => {
                format!("installed {} {}", self.system_name, self.version)
            }
            (InstallState::SavedNotInstalled, Some(path)) => format!(
                "saved, not installed: {} (the package installer is not available yet)",
                path.display()
            ),
            (InstallState::SavedNotInstalled, None) => {
                "saved, not installed (the package installer is not available yet)".to_owned()
            }
        }
    }
}

/// Why installing failed.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// There is no installer service on this system.
    #[error("the package installer (pkgd) is not available: {0}")]
    Unavailable(String),
    /// The installer refused the package; one line per reason.
    #[error("the installer refused the package:\n  {}", .0.join("\n  "))]
    Refused(Vec<String>),
    /// Saving the package failed.
    #[error(transparent)]
    Save(#[from] LzpError),
}

/// Installs a built package.
///
/// It takes the [`BuiltPackage`] (bytes plus metadata) rather than bare bytes so
/// the dev fallback can name its file without parsing the archive; a `pkgd`
/// client sends `package.bytes` and nothing else.
pub trait Installer {
    /// Asks the installer what it would grant, without changing anything.
    /// `Ok(None)` means this installer has no consent step (the dev fallback).
    fn review(&self, _package: &BuiltPackage) -> Result<Option<PackageReview>, InstallError> {
        Ok(None)
    }

    /// Starts an installed app by its `system_name`.
    fn launch(&self, _system_name: &str) -> Result<(), InstallError> {
        Err(InstallError::Unavailable(
            "this installer cannot start apps".to_owned(),
        ))
    }

    /// Installs `package`, or saves it when installing is not possible.
    fn install(&self, package: &BuiltPackage) -> Result<InstalledApp, InstallError>;
}

/// The fallback for a platform with no package manager: writes the `.lzp` into a directory
/// and reports "saved, not installed".
pub struct DevInstaller {
    dir: PathBuf,
}

impl DevInstaller {
    /// Saves packages into `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> DevInstaller {
        DevInstaller { dir: dir.into() }
    }

    /// Saves packages into [`DEV_PACKAGE_DIR`].
    pub fn on_lazyos() -> DevInstaller {
        DevInstaller::new(DEV_PACKAGE_DIR)
    }
}

impl Installer for DevInstaller {
    fn install(&self, package: &BuiltPackage) -> Result<InstalledApp, InstallError> {
        let location = write_package(&self.dir, package)?;
        Ok(InstalledApp {
            system_name: package.system_name.clone(),
            version: package.version.clone(),
            state: InstallState::SavedNotInstalled,
            location: Some(location),
        })
    }
}

/// The placeholder for the `pkgd` client; see the module documentation.
#[derive(Clone, Copy, Debug, Default)]
pub struct PkgdInstaller;

impl Installer for PkgdInstaller {
    fn install(&self, _package: &BuiltPackage) -> Result<InstalledApp, InstallError> {
        Err(InstallError::Unavailable(
            "no pkgd client is linked into this build".to_owned(),
        ))
    }
}

/// Installs with `primary`, and on [`InstallError::Unavailable`] saves with
/// `fallback` instead. Any other error is returned unchanged: a refusal is not
/// a reason to write the package somewhere else.
pub fn install_with_fallback(
    primary: &dyn Installer,
    fallback: &dyn Installer,
    package: &BuiltPackage,
) -> Result<InstalledApp, InstallError> {
    match primary.install(package) {
        Err(InstallError::Unavailable(_)) => fallback.install(package),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package() -> BuiltPackage {
        BuiltPackage {
            bytes: b"PK".to_vec(),
            system_name: "user.ada.todo".to_owned(),
            version: "1.0.0".to_owned(),
            manifest: String::new(),
            entries: 0,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lazyrad-inst-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_dev_installer_saves_and_says_so() {
        let dir = scratch("dev");
        let app = DevInstaller::new(&dir).install(&package()).unwrap();
        assert_eq!(app.state, InstallState::SavedNotInstalled);
        let path = app.location.clone().unwrap();
        assert_eq!(path, dir.join("user.ada.todo-1.0.0.lzp"));
        assert_eq!(std::fs::read(&path).unwrap(), b"PK");
        assert!(app.summary().starts_with("saved, not installed"));
        // No temporary file is left behind.
        let leftovers = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(leftovers, 1);
    }

    #[test]
    fn the_pkgd_placeholder_is_unavailable_and_falls_back() {
        assert!(matches!(
            PkgdInstaller.install(&package()),
            Err(InstallError::Unavailable(_))
        ));
        let dir = scratch("fallback");
        let app =
            install_with_fallback(&PkgdInstaller, &DevInstaller::new(&dir), &package()).unwrap();
        assert_eq!(app.state, InstallState::SavedNotInstalled);
    }

    #[test]
    fn a_refusal_does_not_fall_back() {
        struct Refuses;
        impl Installer for Refuses {
            fn install(&self, _: &BuiltPackage) -> Result<InstalledApp, InstallError> {
                Err(InstallError::Refused(vec!["bad manifest".to_owned()]))
            }
        }
        let dir = scratch("refused");
        let result = install_with_fallback(&Refuses, &DevInstaller::new(&dir), &package());
        assert!(matches!(result, Err(InstallError::Refused(_))));
        assert!(!dir.exists(), "nothing was written");
    }
}
