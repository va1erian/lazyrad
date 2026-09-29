#![forbid(unsafe_code)]

//! Patching a Windows player stub: subsystem, icon and version resources.
//!
//! The IDE ships one player binary, built as a console program so the IDE can
//! read its output while running a project (PLAN.md §8). An exported app must
//! open without a console window, so export flips the copied stub's PE
//! subsystem from console to GUI. That is a two-byte header edit, needs no
//! second binary, and works on any host, so a Linux IDE can still export a
//! Windows executable from a Windows stub.

use editpe::constants::{IMAGE_SUBSYSTEM_WINDOWS_GUI, LANGUAGE_ID_EN_US};
use editpe::types::VersionU16;
use editpe::{Image, ResourceDirectory, VersionInfo, VersionStringTable};

use crate::error::ExportError;

/// What export writes into the executable's resources.
pub struct PeMetadata<'a> {
    /// The project name: product name, description and internal name.
    pub name: &'a str,
    /// The project version string.
    pub version: &'a str,
    /// The contents of an `.ico` file, or `None` to keep the stub's icon.
    pub icon: Option<&'a [u8]>,
}

/// Whether `bytes` look like a Windows executable (an `MZ` header).
pub fn is_pe(bytes: &[u8]) -> bool {
    bytes.starts_with(b"MZ")
}

/// Returns `stub` with the GUI subsystem, and the icon and version resources
/// from `metadata`.
pub fn patch(stub: &[u8], metadata: &PeMetadata<'_>) -> Result<Vec<u8>, ExportError> {
    let patch_error =
        |what: &str, error: &dyn std::fmt::Display| ExportError::Patch(format!("{what}: {error}"));
    let mut image =
        Image::parse(stub).map_err(|error| patch_error("not a valid PE file", &error))?;
    image.set_subsystem(IMAGE_SUBSYSTEM_WINDOWS_GUI);

    let mut resources = image
        .resource_directory()
        .cloned()
        .unwrap_or_else(ResourceDirectory::default);
    if let Some(icon) = metadata.icon {
        resources
            .set_main_icon(icon.to_vec())
            .map_err(|error| patch_error("the icon is not usable", &error))?;
    }
    let version = version_info(&resources, metadata);
    resources
        .set_version_info(&version)
        .map_err(|error| patch_error("the version information is not usable", &error))?;
    image
        .set_resource_directory(resources)
        .map_err(|error| patch_error("the resources do not fit", &error))?;
    Ok(image.data().to_vec())
}

/// The version resource: the stub's own, if it has one, with the project's
/// strings and numbers written over it.
fn version_info(resources: &ResourceDirectory, metadata: &PeMetadata<'_>) -> VersionInfo {
    let mut info = resources
        .get_version_info()
        .ok()
        .flatten()
        .unwrap_or_default();
    if info.strings.is_empty() {
        info.strings.push(VersionStringTable {
            key: "040904b0".to_owned(),
            strings: Default::default(),
        });
    }
    if info.vars.is_empty() {
        info.vars.push(VersionU16 {
            major: LANGUAGE_ID_EN_US,
            minor: 0x04b0,
        });
    }
    let file_name = format!("{}.exe", metadata.name);
    for table in &mut info.strings {
        let strings = [
            ("ProductName", metadata.name),
            ("FileDescription", metadata.name),
            ("InternalName", metadata.name),
            ("OriginalFilename", file_name.as_str()),
            ("FileVersion", metadata.version),
            ("ProductVersion", metadata.version),
        ];
        for (key, value) in strings {
            table.strings.insert(key.to_owned(), value.to_owned());
        }
    }
    if let Some((major, minor)) = numeric_version(metadata.version) {
        info.info.file_version.major = major;
        info.info.file_version.minor = minor;
        info.info.product_version.major = major;
        info.info.product_version.minor = minor;
    }
    info
}

/// `1.2.3.4` as the two 32-bit words a fixed file info holds (`1.2` and
/// `3.4`), or `None` when the version is not dotted numbers. Missing parts are
/// zero; a part beyond 65535 makes the whole version non-numeric.
fn numeric_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = [0u32; 4];
    for (index, part) in version.trim().split('.').enumerate() {
        let slot = parts.get_mut(index)?;
        let digits = part.split(['-', '+']).next().unwrap_or_default();
        *slot = digits.parse::<u16>().ok()?.into();
    }
    Some(((parts[0] << 16) | parts[1], (parts[2] << 16) | parts[3]))
}

#[cfg(test)]
mod tests {
    use super::numeric_version;

    #[test]
    fn dotted_versions_become_fixed_file_words() {
        assert_eq!(numeric_version("1.2.3.4"), Some((0x0001_0002, 0x0003_0004)));
        assert_eq!(numeric_version("0.1.0"), Some((0x0000_0001, 0x0000_0000)));
        assert_eq!(numeric_version("2"), Some((0x0002_0000, 0)));
        assert_eq!(numeric_version("1.0.0-beta"), Some((0x0001_0000, 0)));
    }

    #[test]
    fn a_free_form_version_has_no_numeric_form() {
        assert_eq!(numeric_version("banana"), None);
        assert_eq!(numeric_version("1.2.3.4.5"), None);
        assert_eq!(numeric_version("70000.1"), None);
        assert_eq!(numeric_version(""), None);
    }
}
