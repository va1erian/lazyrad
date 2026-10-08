#![forbid(unsafe_code)]

//! Reading a project's own files, whether it lives in a folder or is packed
//! into an exported executable.
//!
//! A script reads its project's `.lfm`, `.rhai` and asset files through the
//! ordinary `file_*` standard-library functions, by a project-relative path.
//! [`ProjectFiles`] is what the runtime consults before it falls back to the
//! filesystem, so the same script works both when the project is a folder on
//! disk ([`DiskProject`]) and when it is the in-memory payload of an exported
//! app (the player's own provider).
//!
//! A provider never follows a symbolic link and never leaves the project: a
//! path with a parent component, an absolute path or a drive prefix is refused.
//! Providers serve reads only; writes always go to the filesystem through the
//! [`FsPolicy`](crate::fs_policy::FsPolicy).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use lazyrad_project::Project;

/// A read-only source of a project's files, keyed by project-relative path.
pub trait ProjectFiles: 'static {
    /// The bytes of the project-relative file `relative`, or `None` when the
    /// project has no such file. The path uses `/` separators.
    fn read(&self, relative: &str) -> Option<Vec<u8>>;
}

/// A provider with no files, for a runtime built from in-memory sources.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoProjectFiles;

impl ProjectFiles for NoProjectFiles {
    fn read(&self, _relative: &str) -> Option<Vec<u8>> {
        None
    }
}

/// A provider that reads a project folder from disk.
///
/// It serves exactly what an export would ship: the files the project's items
/// reference and the files its `assets` globs match. Any other file in the
/// folder is left to the filesystem, so a script reads the same files from a
/// folder as from an exported app, and a data file it writes next to the
/// project is read back from where it was written.
pub struct DiskProject {
    root: PathBuf,
    /// The `/`-separated paths of the files the items reference.
    referenced: BTreeSet<String>,
    /// The project's asset globs.
    assets: Vec<String>,
    /// The `.lrp` itself, which is never served even when a glob matches it.
    project_file: String,
}

impl DiskProject {
    /// A provider for `project`, whose folder is `root`.
    pub fn new(root: impl Into<PathBuf>, project: &Project) -> DiskProject {
        DiskProject {
            root: root.into(),
            referenced: project
                .referenced_files()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .collect(),
            assets: project.assets.clone(),
            project_file: project.file_name(),
        }
    }

    /// Whether the project ships the project-relative file `relative`.
    fn ships(&self, relative: &str) -> bool {
        if relative == self.project_file {
            return false;
        }
        self.referenced.contains(relative)
            || self
                .assets
                .iter()
                .any(|pattern| lazyrad_project::glob::matches(pattern, relative))
    }
}

impl ProjectFiles for DiskProject {
    fn read(&self, relative: &str) -> Option<Vec<u8>> {
        if !self.ships(relative) {
            return None;
        }
        let path = safe_join(&self.root, relative)?;
        // Never read through a link, in the file itself or in any folder on
        // the way to it, so a link inside the project cannot reach outside.
        let mut walked = self.root.clone();
        for component in Path::new(relative).components() {
            walked.push(component);
            if fs::symlink_metadata(&walked).ok()?.file_type().is_symlink() {
                return None;
            }
        }
        if !fs::symlink_metadata(&path).ok()?.file_type().is_file() {
            return None;
        }
        fs::read(path).ok()
    }
}

/// Joins `relative` to `root`, refusing anything that could leave it.
///
/// Only normal path components are allowed, so `..`, an absolute path and a
/// Windows drive prefix are all refused before the filesystem is touched.
fn safe_join(root: &Path, relative: &str) -> Option<PathBuf> {
    // Project paths use `/`. A backslash is a separator on Windows but an
    // ordinary character elsewhere, so the same name would mean different
    // files on different hosts: refuse it everywhere.
    if relative.contains('\\') {
        return None;
    }
    let relative = Path::new(relative);
    if relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
        && !relative.as_os_str().is_empty()
    {
        Some(root.join(relative))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_with_a_backslash_is_refused_on_every_host() {
        let root = Path::new("project");
        assert!(safe_join(root, "songs\\song.mod").is_none());
        assert!(safe_join(root, "..\\outside.txt").is_none());
        assert!(safe_join(root, "a\\..\\..\\b").is_none());
        assert_eq!(
            safe_join(root, "songs/song.mod"),
            Some(root.join("songs/song.mod"))
        );
    }

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "lazyrad-runtime-files-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("scratch directory is created");
        path
    }

    /// A project whose one form is `main.lfm` / `main.rhai`, shipping `assets`.
    fn project(assets: &[&str]) -> Project {
        let mut project = Project::new("demo");
        project.items.push(lazyrad_project::ProjectItem::Form {
            name: "main".to_owned(),
            layout: "main.lfm".into(),
            code: "main.rhai".into(),
        });
        project.assets = assets.iter().map(|glob| (*glob).to_owned()).collect();
        project
    }

    #[test]
    fn a_disk_project_reads_its_own_files() {
        let dir = scratch("read");
        fs::create_dir_all(dir.join("songs")).expect("subdir");
        fs::write(dir.join("songs/song.mod"), b"data").expect("asset");
        fs::write(dir.join("main.rhai"), b"code").expect("script");
        let files = DiskProject::new(dir.clone(), &project(&["songs/*.mod"]));

        assert_eq!(files.read("songs/song.mod"), Some(b"data".to_vec()));
        assert_eq!(files.read("main.rhai"), Some(b"code".to_vec()));
        assert_eq!(files.read("songs/missing.mod"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_disk_project_serves_only_what_an_export_ships() {
        let dir = scratch("ships");
        fs::write(dir.join("notes.txt"), b"user data").expect("data file");
        let files = DiskProject::new(dir.clone(), &project(&["songs/*.mod"]));

        // A data file the script wrote is left to the filesystem, so a read
        // goes where the write went.
        assert_eq!(files.read("notes.txt"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_disk_project_never_serves_its_project_file() {
        let dir = scratch("lrp");
        fs::write(dir.join("demo.lrp"), b"project").expect("project file");
        fs::write(dir.join("main.rhai"), b"code").expect("script");
        let files = DiskProject::new(dir.clone(), &project(&["**"]));

        assert_eq!(files.read("demo.lrp"), None);
        assert_eq!(files.read("main.rhai"), Some(b"code".to_vec()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_disk_project_refuses_paths_that_leave_it() {
        let dir = scratch("escape");
        fs::write(dir.join("inside.txt"), b"x").expect("file");
        let files = DiskProject::new(dir.clone(), &project(&["**"]));

        assert_eq!(files.read("../outside.txt"), None);
        assert_eq!(files.read("/etc/hosts"), None);
        assert_eq!(files.read("a/../b"), None);
        assert_eq!(files.read(""), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_disk_project_never_reads_through_a_linked_folder() {
        let dir = scratch("linked");
        let outside = scratch("linked-outside");
        fs::write(outside.join("secret.mod"), b"secret").expect("outside file");
        std::os::unix::fs::symlink(&outside, dir.join("songs")).expect("link");
        let files = DiskProject::new(dir.clone(), &project(&["songs/*.mod"]));

        assert_eq!(files.read("songs/secret.mod"), None);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn an_empty_provider_has_nothing() {
        assert_eq!(NoProjectFiles.read("main_form.rhai"), None);
    }
}
