#![forbid(unsafe_code)]

//! Packaging a LazyRAD project as a LazyOS application package (`.lzp`).
//!
//! An app LazyRAD produces for LazyOS is a **standard `.lzp`**
//! (`docs/packages.md` in the LazyOS repo): LazyRAD does not invent a format, and
//! the installer, registry and Start menu treat it like any other package.
//!
//! ```text
//! manifest.toml               generated from the .lrp ([`manifest`])
//! bin/lrplay.elf              the LazyOS player: a copy of the stub
//! icons/app-{16,32,128}.png   the project's, or generated ([`icons`])
//! resources/project/**        the .lrp and every .lfm/.rhai it references
//! ```
//!
//! * [`zip`] is the writer, producing exactly the zip subset `lazypkg` reads;
//! * [`build_package`] lays the tree out and runs the optional pre-package check;
//! * [`install`] is the [`Installer`] seam: the LazyOS implementation will call
//!   `pkgd` through a MIDL-generated client, and until `pkgd` exists the
//!   [`DevInstaller`] saves the package for a later install.
//!
//! **Conformance.** Every test here re-reads what it built with an independent
//! verifier (`tests/common`) that mirrors the LazyOS rules; the LazyOS repo's
//! own tests re-open the same packages with `lazypkg::Package::open` and
//! `tools/pkg/build.py` (LazyRAD cannot depend on `lazypkg` directly: the LazyOS
//! repository is private and `lazypkg` is not published).

pub mod error;
pub mod icons;
pub mod install;
pub mod manifest;
pub mod zip;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use lazyrad_project::Project;

pub use error::LzpError;
pub use install::{
    DevInstaller, InstallError, InstallState, InstalledApp, Installer, PackageReview,
    PermissionNote, PkgdInstaller, install_with_fallback,
};

use crate::payload::Payload;
pub use manifest::HostPermissions;
use manifest::{Identity, PLAYER_ENTRY, PROJECT_DIR};
use zip::{MAX_ENTRY_UNCOMPRESSED, ZipWriter};

/// A pre-package check, for example the runtime's compile check: `Ok` to go
/// ahead, or one line per problem.
pub type Check<'a> = &'a dyn Fn(&Path) -> Result<(), Vec<String>>;

/// The three required icons as PNG bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconSet {
    /// 16 x 16.
    pub small: Vec<u8>,
    /// 32 x 32.
    pub medium: Vec<u8>,
    /// 128 x 128.
    pub large: Vec<u8>,
}

/// What to package.
pub struct PackageRequest<'a> {
    /// The project's `.lrp` file.
    pub project: &'a Path,
    /// The LazyOS player (`lrplay`, a static x86-64 ELF).
    pub player: &'a [u8],
    /// The author, shown when the package is installed. Unverified.
    pub author: &'a str,
    /// An explicit reverse-DNS id; `None` derives `user.<author>.<project>`.
    pub system_name: Option<&'a str>,
    /// A one-line description for the manifest.
    pub description: Option<&'a str>,
    /// The icons, or `None` for the generated defaults.
    pub icons: Option<&'a IconSet>,
    /// The check run before anything is packed (PLAN.md §8).
    pub check: Option<Check<'a>>,
    /// Asks the host platform which interfaces and topics the scripts use.
    /// `None` declares none beyond the packager's own.
    pub permissions: Option<DerivePermissions<'a>>,
}

/// Called with every `.rhai` source in the package; returns what the host
/// platform found they use ([`HostPermissions`]).
pub type DerivePermissions<'a> = &'a dyn Fn(&[&str]) -> HostPermissions;

/// A finished package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltPackage {
    /// The `.lzp` bytes.
    pub bytes: Vec<u8>,
    /// The app's reverse-DNS id.
    pub system_name: String,
    /// The normalised `MAJOR.MINOR.PATCH` version.
    pub version: String,
    /// The generated `manifest.toml`.
    pub manifest: String,
    /// The number of archive entries.
    pub entries: usize,
}

impl BuiltPackage {
    /// The conventional file name, `<system_name>-<version>.lzp`.
    pub fn file_name(&self) -> String {
        format!("{}-{}.lzp", self.system_name, self.version)
    }
}

/// Reads the player binary, refusing anything that is not a plain x86-64 ELF
/// within the per-entry limit.
pub fn read_player(path: &Path) -> Result<Vec<u8>, LzpError> {
    let refuse = |reason: String| LzpError::Player {
        path: path.to_path_buf(),
        reason,
    };
    let metadata = fs::metadata(path).map_err(|error| refuse(error.to_string()))?;
    if !metadata.is_file() {
        return Err(refuse("it is not a file".to_owned()));
    }
    let mut data = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_ENTRY_UNCOMPRESSED + 1).read_to_end(&mut data))
        .map_err(|error| refuse(error.to_string()))?;
    check_player(&data).map_err(|reason| refuse(reason.to_owned()))?;
    Ok(data)
}

/// Whether `data` is an acceptable player: ELF, 64-bit, little-endian, x86-64,
/// and small enough for one package entry.
pub fn check_player(data: &[u8]) -> Result<(), &'static str> {
    if data.len() as u64 > MAX_ENTRY_UNCOMPRESSED {
        return Err("it is larger than 16 MiB, the limit for one package file");
    }
    if data.len() < 20 || data[..4] != *b"\x7fELF" {
        return Err("it is not an ELF executable (build it with tools/lazyrad/build.py)");
    }
    if data[4] != 2 || data[5] != 1 || u16::from_le_bytes([data[18], data[19]]) != 0x3E {
        return Err("it is not a 64-bit little-endian x86-64 ELF");
    }
    Ok(())
}

/// Builds the package for `request`.
pub fn build_package(request: &PackageRequest<'_>) -> Result<BuiltPackage, LzpError> {
    let project = Project::load_file(request.project).map_err(crate::error::PayloadError::from)?;
    if let Some(check) = request.check {
        check(request.project).map_err(LzpError::Check)?;
    }
    let payload = Payload::from_project(request.project)?;
    check_player(request.player).map_err(|reason| LzpError::Player {
        path: PathBuf::from("<player>"),
        reason: reason.to_owned(),
    })?;

    let scripts: Vec<&str> = payload
        .entries()
        .iter()
        .filter(|entry| entry.name.ends_with(".rhai"))
        .filter_map(|entry| std::str::from_utf8(&entry.data).ok())
        .collect();
    let host = request
        .permissions
        .map(|derive| derive(&scripts))
        .unwrap_or_default();
    let built = manifest::build_with(
        &Identity {
            name: &project.name,
            author: request.author,
            version: &project.version,
            system_name: request.system_name,
            description: request.description,
        },
        scripts.iter().copied(),
        &host,
    )?;

    let mut zip = ZipWriter::new();
    zip.add("manifest.toml", built.text.as_bytes())?;
    zip.add(PLAYER_ENTRY, request.player)?;
    add_icons(&mut zip, &project.name, request.icons)?;
    for entry in payload.entries() {
        // An asset is stored under the payload's `assets/` prefix; on disk it
        // keeps its project-relative path, so a script reads it through the
        // same path in a folder and in an exported app.
        let relative = entry
            .name
            .strip_prefix(crate::payload::ASSET_PREFIX)
            .unwrap_or(&entry.name);
        zip.add(&format!("{PROJECT_DIR}/{relative}"), &entry.data)?;
    }
    let entries = zip.len();
    Ok(BuiltPackage {
        bytes: zip.finish(),
        system_name: built.system_name,
        version: built.version,
        manifest: built.text,
        entries,
    })
}

/// Adds the three icons: the caller's (validated as PNG) or the defaults.
fn add_icons(zip: &mut ZipWriter, name: &str, icons: Option<&IconSet>) -> Result<(), LzpError> {
    for size in icons::SIZES {
        let entry = icons::entry_name(size);
        let data = match icons {
            Some(set) => {
                let data = match size {
                    16 => &set.small,
                    32 => &set.medium,
                    _ => &set.large,
                };
                if !icons::is_png(data) {
                    return Err(LzpError::Icon { name: entry });
                }
                data.clone()
            }
            None => icons::default_icon(name, size),
        };
        zip.add(&entry, &data)?;
    }
    Ok(())
}

/// Writes `package` into `dir` as `<system_name>-<version>.lzp` atomically and
/// returns the path.
pub fn write_package(dir: &Path, package: &BuiltPackage) -> Result<PathBuf, LzpError> {
    let io = |path: &Path, source| LzpError::Io {
        path: path.to_path_buf(),
        source,
    };
    fs::create_dir_all(dir).map_err(|error| io(dir, error))?;
    let target = dir.join(package.file_name());
    let temp = dir.join(format!(
        ".{}.{}.tmp",
        package.file_name(),
        std::process::id()
    ));
    let result = (|| {
        let mut file = fs::File::create(&temp)?;
        std::io::Write::write_all(&mut file, &package.bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, &target)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(io(&target, error));
    }
    Ok(target)
}
