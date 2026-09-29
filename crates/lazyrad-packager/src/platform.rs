#![forbid(unsafe_code)]

//! The packager's platform seam (PLAN.md §12, "Porting LazyRAD").
//!
//! The only host-specific step of an export is giving the new file its
//! "executable" marking. Patching a Windows stub's PE resources (see
//! [`crate::pe`]) is pure Rust and runs on every host, so it needs no seam. A
//! new platform provides [`make_executable`]; the portable fallback does
//! nothing, which is right wherever files carry no execute bit.

use std::fs::File;

/// Marks the freshly written export as executable.
///
/// Unix sets mode `0o755`; other platforms have no such bit and do nothing.
pub fn make_executable(file: &File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marking_a_fresh_file_succeeds() {
        let path = std::env::temp_dir().join(format!(
            "lazyrad-packager-platform-{}.bin",
            std::process::id()
        ));
        let file = File::create(&path).expect("file is created");
        make_executable(&file).expect("marking never fails on a fresh file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = file.metadata().expect("metadata").permissions().mode();
            assert_eq!(mode & 0o111, 0o111);
        }
        drop(file);
        let _ = std::fs::remove_file(&path);
    }
}
