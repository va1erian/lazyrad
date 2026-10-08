#![forbid(unsafe_code)]

//! Collecting a project's `assets`: the extra files its globs match.
//!
//! A `.lrp` may list `assets = ["songs/*.mod", "icons/logo.png"]`; the packager
//! ships the files those patterns match. [`collect_assets`] walks the project
//! folder once, returns each matched file as a `/`-separated project-relative
//! path, and reports a warning for a pattern that matched nothing (a warning,
//! not an error). It never follows a symbolic link, so a link inside the
//! project cannot pull a file from outside into a package.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path};

use crate::error::Error;
use crate::glob;

/// The project-relative paths the `globs` match, and a warning per glob that
/// matched nothing.
///
/// Paths use `/` separators and are sorted, so a package is reproducible. The
/// walk reads only directory entries and file metadata; it never follows a
/// symbolic link and never reads file contents.
pub fn collect_assets(dir: &Path, globs: &[String]) -> Result<(Vec<String>, Vec<String>), Error> {
    let mut files = Vec::new();
    walk(dir, dir, &mut files)?;
    files.sort();
    files.dedup();

    let mut matched = BTreeSet::new();
    let mut warnings = Vec::new();
    for glob in globs {
        let hits: Vec<&String> = files
            .iter()
            .filter(|relative| glob::matches(glob, relative))
            .collect();
        if hits.is_empty() {
            warnings.push(format!("asset pattern `{glob}` matched no files"));
        }
        matched.extend(hits.into_iter().cloned());
    }
    Ok((matched.into_iter().collect(), warnings))
}

/// Collects every regular file under `dir`, as a path relative to `root`.
fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), Error> {
    let entries = fs::read_dir(dir).map_err(|source| Error::io(dir, source))?;
    for entry in entries {
        let entry = entry.map_err(|source| Error::io(dir, source))?;
        let path = entry.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|source| Error::io(path.clone(), source))?;
        // A link is skipped, whether it points at a file or a directory, so a
        // project can never pack something outside itself.
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            walk(root, &path, out)?;
        } else if metadata.is_file()
            && let Some(relative) = relative_of(root, &path)
        {
            out.push(relative);
        }
    }
    Ok(())
}

/// `path` relative to `root` as a `/`-separated string.
fn relative_of(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "lazyrad-project-assets-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("scratch directory is created");
        path
    }

    #[test]
    fn globs_collect_their_files_and_warn_when_empty() {
        let dir = scratch("collect");
        fs::create_dir_all(dir.join("songs")).expect("subdir");
        fs::write(dir.join("songs/song.mod"), b"data").expect("asset");
        fs::write(dir.join("notes.txt"), b"text").expect("other");

        let globs = vec!["songs/*.mod".to_owned(), "*.png".to_owned()];
        let (files, warnings) = collect_assets(&dir, &globs).expect("collects");
        assert_eq!(files, ["songs/song.mod"]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("*.png"), "{warnings:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn overlapping_globs_collect_each_file_once() {
        let dir = scratch("overlap");
        fs::write(dir.join("a.txt"), b"a").expect("asset");
        let globs = vec!["*.txt".to_owned(), "**/*.txt".to_owned()];
        let (files, warnings) = collect_assets(&dir, &globs).expect("collects");
        assert_eq!(files, ["a.txt"]);
        assert!(warnings.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_never_collected() {
        let dir = scratch("symlink");
        let outside = scratch("symlink-outside");
        fs::write(outside.join("secret.mod"), b"secret").expect("outside file");
        std::os::unix::fs::symlink(outside.join("secret.mod"), dir.join("link.mod"))
            .expect("symlink");
        let globs = vec!["*.mod".to_owned()];
        let (files, warnings) = collect_assets(&dir, &globs).expect("collects");
        assert!(files.is_empty(), "{files:?}");
        assert_eq!(warnings.len(), 1, "the pattern matched only a link");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }
}
