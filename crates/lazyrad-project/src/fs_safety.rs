#![forbid(unsafe_code)]

//! The project crate's platform seam: file-safety primitives.
//!
//! Saving must never follow a symbolic link out of the project folder. How a
//! link is refused differs per platform (`O_NOFOLLOW` on Unix, reparse points
//! on Windows), so those details live here and nowhere else in the crate; no
//! other file uses `cfg(unix)` or `cfg(windows)` (PLAN.md §12, "Porting
//! LazyRAD"). A new platform provides [`write_no_follow`]. The portable
//! fallback, used on any platform that is neither Unix nor Windows, checks for
//! a link before opening, which is correct but leaves a small window between
//! the check and the open.

use std::fs;
use std::io::Write;
use std::path::Path;

/// Whether `path` itself is a symbolic link (without following it). A path
/// that cannot be inspected is not a link.
pub fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// The error for a write that would go through a symbolic link.
pub fn symlink_refused() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "refusing to write through a symbolic link",
    )
}

/// Writes `contents` to `path` without following a symbolic link at `path`,
/// creating the file when it does not exist.
///
/// The link check happens on the opened handle, not before the open, so a file
/// swapped for a link after the project was loaded cannot redirect the write
/// outside the project folder. The file is truncated only once the handle is
/// known to be the file itself.
pub fn write_no_follow(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Opening a link fails with `ELOOP`.
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Open the link itself rather than its target; it is refused below.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(not(any(unix, windows)))]
    if is_symlink(path) {
        return Err(symlink_refused());
    }
    let mut file = options.open(path)?;
    if file.metadata()?.file_type().is_symlink() {
        return Err(symlink_refused());
    }
    // Truncate only after the handle is known to be the file itself.
    file.set_len(0)?;
    file.write_all(contents)
}

/// Creates a symbolic link `link` to `target` for tests; an error where the
/// platform has no links or refuses to create one (Windows without the
/// privilege).
#[cfg(test)]
pub(crate) fn make_test_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    return std::os::windows::fs::symlink_file(target, link);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        Err(std::io::Error::other("no symbolic links on this platform"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "lazyrad-project-fs-safety-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("scratch directory is created");
        path
    }

    #[test]
    fn a_regular_file_is_written_and_truncated() {
        let dir = scratch("regular");
        let path = dir.join("file.txt");
        write_no_follow(&path, b"a long first version").expect("first write");
        write_no_follow(&path, b"short").expect("second write");
        assert_eq!(fs::read(&path).expect("readable"), b"short");
        assert!(!is_symlink(&path));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_path_is_not_a_symlink() {
        let dir = scratch("missing");
        assert!(!is_symlink(&dir.join("nothing")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unwritable_path_is_an_error() {
        let dir = scratch("unwritable");
        // A directory cannot be opened for writing, and a missing parent
        // directory is not created by the primitive.
        write_no_follow(&dir, b"x").expect_err("a directory is not a file");
        write_no_follow(&dir.join("no/such/dir/file"), b"x").expect_err("no parent directory");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_refusal_names_the_symlink() {
        let error = symlink_refused();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("symbolic link"));
    }

    #[test]
    fn a_symlink_is_refused_and_its_target_untouched() {
        let dir = scratch("symlink");
        let outside = dir.join("outside.txt");
        fs::write(&outside, b"keep").expect("target is written");
        let link = dir.join("item.rhai");
        if make_test_symlink(&outside, &link).is_err() {
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        assert!(is_symlink(&link));
        write_no_follow(&link, b"overwrite").expect_err("a link is refused");
        assert_eq!(fs::read(&outside).expect("target is readable"), b"keep");
        let _ = fs::remove_dir_all(&dir);
    }
}
