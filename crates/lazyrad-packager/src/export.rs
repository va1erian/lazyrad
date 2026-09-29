#![forbid(unsafe_code)]

//! Exporting a project: stub + payload, written atomically.
//!
//! [`export`] never leaves a half-written executable. It builds the whole
//! output in a `create_new` temporary file next to the target and renames it
//! into place only when every byte is written and synced; on any error the
//! temporary file is removed and an existing target is untouched.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use lazyrad_project::Project;

use crate::error::{ExportError, PayloadError};
use crate::payload::{Payload, has_footer, read_regular};
use crate::pe::{PeMetadata, is_pe, patch};

/// The largest player stub export will copy, in bytes.
pub const MAX_STUB_BYTES: u64 = 512 * 1024 * 1024;
/// The largest icon file export will read, in bytes.
pub const MAX_ICON_BYTES: u64 = 4 * 1024 * 1024;

/// What to export and where.
pub struct ExportRequest<'a> {
    /// The player executable to copy.
    pub stub: &'a Path,
    /// The project's `.lrp` file.
    pub project: &'a Path,
    /// The executable to write.
    pub output: &'a Path,
}

/// What an export did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportReport {
    /// The executable that was written.
    pub output: PathBuf,
    /// Its size in bytes.
    pub bytes: u64,
    /// Whether the stub was a Windows executable and had its subsystem,
    /// icon and version resources patched.
    pub patched_windows_resources: bool,
}

/// Exports the project as a self-contained executable.
pub fn export(request: &ExportRequest<'_>) -> Result<ExportReport, ExportError> {
    let project = Project::load_file(request.project).map_err(PayloadError::from)?;
    let payload = Payload::from_project(request.project)?;
    let project_dir = request
        .project
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));

    let stub = read_stub(request.stub)?;
    check_output(request, &project, project_dir)?;

    let icon = match &project.icon {
        Some(name) => Some(read_icon(&project_dir.join(name))?),
        None => None,
    };
    let patched = is_pe(&stub);
    let stub = if patched {
        patch(
            &stub,
            &PeMetadata {
                name: &project.name,
                version: &project.version,
                icon: icon.as_deref(),
            },
        )?
    } else {
        stub
    };

    atomic_write(request.output, |file| {
        file.write_all(&stub)?;
        payload.write_to(file, stub.len() as u64)
    })
    .map_err(|source| ExportError::Write {
        path: request.output.to_path_buf(),
        source,
    })?;

    let bytes = fs::metadata(request.output).map_or(0, |metadata| metadata.len());
    Ok(ExportReport {
        output: request.output.to_path_buf(),
        bytes,
        patched_windows_resources: patched,
    })
}

/// Reads the stub, refusing one that is missing, not a regular file, too big,
/// or already an exported program.
fn read_stub(path: &Path) -> Result<Vec<u8>, ExportError> {
    let stub_error = |reason: String| ExportError::Stub {
        path: path.to_path_buf(),
        reason,
    };
    let metadata = fs::metadata(path).map_err(|error| stub_error(error.to_string()))?;
    if !metadata.is_file() {
        return Err(stub_error("it is not a file".to_owned()));
    }
    if metadata.len() > MAX_STUB_BYTES {
        return Err(stub_error(format!(
            "it is {} bytes; the limit is {MAX_STUB_BYTES}",
            metadata.len()
        )));
    }
    let mut data = Vec::with_capacity(metadata.len() as usize);
    fs::File::open(path)
        .and_then(|file| file.take(MAX_STUB_BYTES + 1).read_to_end(&mut data))
        .map_err(|error| stub_error(error.to_string()))?;
    if data.len() as u64 > MAX_STUB_BYTES {
        return Err(stub_error("it grew past the size limit".to_owned()));
    }
    if has_footer(&data) {
        return Err(stub_error(
            "it already carries project data; export from the plain player, not an exported app"
                .to_owned(),
        ));
    }
    Ok(data)
}

/// Reads the project's icon file.
fn read_icon(path: &Path) -> Result<Vec<u8>, ExportError> {
    let icon_error = |reason: String| ExportError::Icon {
        path: path.to_path_buf(),
        reason,
    };
    let data = read_regular(path, MAX_ICON_BYTES).map_err(|error| icon_error(error.to_string()))?;
    // An .ico starts with reserved 0, type 1.
    if data.len() < 22 || data[..4] != [0, 0, 1, 0] {
        return Err(icon_error("it is not an .ico file".to_owned()));
    }
    Ok(data)
}

/// Refuses an output that is a link, a folder, the stub itself, or one of the
/// project's own files.
fn check_output(
    request: &ExportRequest<'_>,
    project: &Project,
    project_dir: &Path,
) -> Result<(), ExportError> {
    let refuse = |reason: &str| ExportError::Output {
        path: request.output.to_path_buf(),
        reason: reason.to_owned(),
    };
    if request.output.file_name().is_none() {
        return Err(refuse("it has no file name"));
    }
    match fs::symlink_metadata(request.output) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(refuse("it is a symbolic link"));
        }
        Ok(metadata) if !metadata.is_file() => return Err(refuse("it is not a regular file")),
        Ok(_) => {
            // The output exists: it must not be the stub or a project file,
            // however differently the paths are spelled.
            let output = fs::canonicalize(request.output).map_err(|error| ExportError::Output {
                path: request.output.to_path_buf(),
                reason: error.to_string(),
            })?;
            let same = |other: &Path| fs::canonicalize(other).is_ok_and(|other| other == output);
            if same(request.stub) {
                return Err(refuse("it is the player stub itself"));
            }
            if same(request.project)
                || project
                    .referenced_files()
                    .any(|relative| same(&project_dir.join(relative)))
            {
                return Err(refuse("it is one of the project's own files"));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(refuse(&error.to_string())),
    }
    Ok(())
}

/// Writes `output` through a temporary file beside it: `write` fills the file,
/// which is then synced and renamed over `output`. The temporary file is
/// removed on any failure, and `output` is untouched unless the rename lands.
pub fn atomic_write(
    output: &Path,
    write: impl FnOnce(&mut fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let (temp_path, mut file) = create_temp(output)?;
    let guard = TempGuard(Some(temp_path.clone()));
    write(&mut file)?;
    file.sync_all()?;
    make_executable(&file)?;
    drop(file);
    fs::rename(&temp_path, output)?;
    guard.commit();
    Ok(())
}

/// Removes the temporary file unless the write was committed.
struct TempGuard(Option<PathBuf>);

impl TempGuard {
    fn commit(mut self) {
        self.0 = None;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

/// Creates a fresh temporary file next to `output`, never reusing a name.
fn create_temp(output: &Path) -> std::io::Result<(PathBuf, fs::File)> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = output.file_name().unwrap_or_default().to_string_lossy();
    for _ in 0..16 {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = dir.join(format!(".{name}.{}.{unique}.tmp", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no unused temporary file name",
    ))
}

/// Makes the new file executable on Unix; other platforms have no such bit.
fn make_executable(_file: &fs::File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        _file.set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}
