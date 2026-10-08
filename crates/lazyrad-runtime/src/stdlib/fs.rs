#![forbid(unsafe_code)]

//! The `file_*` and `dir_*` standard-library functions.
//!
//! Every path a script names goes through the [`FsPolicy`] held by the
//! [`StdlibContext`](super::StdlibContext): the desktop default is
//! unrestricted, while LazyOS confines a produced app to its private data
//! directory (LazyOS plan D5). A refused path is an ordinary script error, so a
//! handler can show it.
//!
//! * `file_read_text(path)`, `file_write_text(path, text)`,
//!   `file_append_text(path, text)`;
//! * `file_read_bytes(path)`, `file_write_bytes(path, blob)`;
//! * `file_exists(path)`, `file_delete(path)`;
//! * `dir_list(path)` (names, sorted), `dir_create(path)`.
//!
//! A read of a project-relative path is served by the runtime's
//! [`ProjectFiles`](crate::files::ProjectFiles) before the filesystem, so a
//! script can read its project's scripts and assets both from a project folder
//! and from an exported app's in-memory payload. Writes always go to disk.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;

use rhai::{Array, Blob, Dynamic, Engine, EvalAltResult};

use super::script_error;
use crate::files::ProjectFiles;
use crate::fs_policy::{Access, FsPolicy};

/// A script-visible result.
type ScriptResult<T> = Result<T, Box<EvalAltResult>>;

/// Resolves `path` for `access`, turning a refusal into a script error.
fn resolve(policy: &FsPolicy, path: &str, access: Access) -> ScriptResult<PathBuf> {
    policy
        .resolve(path, access)
        .map_err(|error| script_error(error.to_string()))
}

/// An I/O failure as a script error naming the path the script used.
fn io_error(path: &str, error: std::io::Error) -> Box<EvalAltResult> {
    script_error(format!("`{path}`: {error}"))
}

/// The project file `path` names, if the project has it.
///
/// A provider already refuses paths that leave the project; an absolute path is
/// never a project-relative one, so it is left to the filesystem.
fn project_file(files: &Rc<dyn ProjectFiles>, path: &str) -> Option<Vec<u8>> {
    if std::path::Path::new(path).is_absolute() {
        return None;
    }
    files.read(path)
}

/// Registers the file and directory functions over `policy` and `files`.
pub(super) fn register(engine: &mut Engine, policy: &Rc<FsPolicy>, files: &Rc<dyn ProjectFiles>) {
    let p = Rc::clone(policy);
    let f = Rc::clone(files);
    documented_fn!(
        engine,
        "file_read_text",
        ["path: &str"],
        ["/// Reads a whole UTF-8 text file."],
        move |path: &str| -> ScriptResult<String> {
            if let Some(bytes) = project_file(&f, path) {
                return String::from_utf8(bytes).map_err(|error| {
                    io_error(
                        path,
                        std::io::Error::new(std::io::ErrorKind::InvalidData, error),
                    )
                });
            }
            let real = resolve(&p, path, Access::Read)?;
            fs::read_to_string(real).map_err(|e| io_error(path, e))
        }
    );
    let p = Rc::clone(policy);
    documented_fn!(
        engine,
        "file_write_text",
        ["path: &str", "text: &str"],
        ["/// Creates or replaces a text file."],
        move |path: &str, text: &str| -> ScriptResult<()> {
            let real = resolve(&p, path, Access::Write)?;
            fs::write(real, text).map_err(|e| io_error(path, e))
        }
    );
    let p = Rc::clone(policy);
    let f = Rc::clone(files);
    documented_fn!(
        engine,
        "file_read_bytes",
        ["path: &str"],
        ["/// Reads a whole file as a `Blob` (a byte array)."],
        move |path: &str| -> ScriptResult<Blob> {
            if let Some(bytes) = project_file(&f, path) {
                return Ok(bytes);
            }
            let real = resolve(&p, path, Access::Read)?;
            fs::read(real).map_err(|e| io_error(path, e))
        }
    );
    let p = Rc::clone(policy);
    documented_fn!(
        engine,
        "file_write_bytes",
        ["path: &str", "data: Blob"],
        ["/// Creates or replaces a file with the bytes of `data`."],
        move |path: &str, data: Blob| -> ScriptResult<()> {
            let real = resolve(&p, path, Access::Write)?;
            fs::write(real, &data).map_err(|e| io_error(path, e))
        }
    );
    let p = Rc::clone(policy);
    documented_fn!(
        engine,
        "file_append_text",
        ["path: &str", "text: &str"],
        ["/// Appends text to a file, creating it when missing."],
        move |path: &str, text: &str| -> ScriptResult<()> {
            let real = resolve(&p, path, Access::Write)?;
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(real)
                .map_err(|e| io_error(path, e))?;
            file.write_all(text.as_bytes())
                .map_err(|e| io_error(path, e))
        }
    );
    let p = Rc::clone(policy);
    let f = Rc::clone(files);
    documented_fn!(
        engine,
        "file_exists",
        ["path: &str"],
        ["/// Whether a file or directory exists. A refused path is an error."],
        move |path: &str| -> ScriptResult<bool> {
            if project_file(&f, path).is_some() {
                return Ok(true);
            }
            Ok(resolve(&p, path, Access::Read)?.exists())
        }
    );
    let p = Rc::clone(policy);
    documented_fn!(
        engine,
        "file_delete",
        ["path: &str"],
        ["/// Deletes a file."],
        move |path: &str| -> ScriptResult<()> {
            let real = resolve(&p, path, Access::Write)?;
            fs::remove_file(real).map_err(|e| io_error(path, e))
        }
    );
    let p = Rc::clone(policy);
    documented_fn!(
        engine,
        "dir_list",
        ["path: &str"],
        ["/// The sorted names of the entries in a directory."],
        move |path: &str| -> ScriptResult<Array> {
            let real = resolve(&p, path, Access::Read)?;
            let mut names: Vec<String> = fs::read_dir(real)
                .map_err(|e| io_error(path, e))?
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            Ok(names.into_iter().map(Dynamic::from).collect())
        }
    );
    let p = Rc::clone(policy);
    documented_fn!(
        engine,
        "dir_create",
        ["path: &str"],
        ["/// Creates a directory and any missing parents."],
        move |path: &str| -> ScriptResult<()> {
            let real = resolve(&p, path, Access::Write)?;
            fs::create_dir_all(real).map_err(|e| io_error(path, e))
        }
    );
}

#[cfg(test)]
mod tests {
    use rhai::Engine;

    use super::*;
    use crate::fs_policy::Sandbox;

    fn engine(policy: FsPolicy) -> Engine {
        let files: Rc<dyn ProjectFiles> = Rc::new(crate::NoProjectFiles);
        let mut engine = Engine::new();
        register(&mut engine, &Rc::new(policy), &files);
        engine
    }

    /// An engine whose project files are `files`.
    fn engine_with_files(policy: FsPolicy, files: Rc<dyn ProjectFiles>) -> Engine {
        let mut engine = Engine::new();
        register(&mut engine, &Rc::new(policy), &files);
        engine
    }

    /// A provider with one in-memory file.
    struct OneFile(&'static str, &'static [u8]);

    impl ProjectFiles for OneFile {
        fn read(&self, relative: &str) -> Option<Vec<u8>> {
            (relative == self.0).then(|| self.1.to_vec())
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lazyrad-fs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn a_sandboxed_script_round_trips_inside_its_root() {
        let root = scratch("rt");
        let engine = engine(FsPolicy::Sandboxed(Sandbox::new(root.clone())));
        let out: String = engine
            .eval(
                r#"
                dir_create("sub");
                file_write_text("sub/a.txt", "one");
                file_append_text("sub/a.txt", "two");
                let names = dir_list("sub");
                file_read_text("sub/a.txt") + ":" + names.len().to_string()
                    + ":" + file_exists("sub/a.txt").to_string()
                "#,
            )
            .expect("runs");
        assert_eq!(out, "onetwo:1:true");
        engine.run(r#"file_delete("sub/a.txt")"#).expect("deletes");
        assert!(!root.join("sub/a.txt").exists());
    }

    #[test]
    fn a_sandboxed_script_cannot_leave_its_root() {
        let root = scratch("esc");
        let engine = engine(FsPolicy::Sandboxed(Sandbox::new(root.clone())));
        for code in [
            r#"file_read_text("../x")"#,
            r#"file_write_text("/etc/lazyrad-test", "x")"#,
            r#"dir_list("a/../..")"#,
            r#"file_exists("")"#,
        ] {
            let error = engine.eval::<Dynamic>(code).expect_err(code);
            let text = error.to_string();
            assert!(
                text.contains("access denied") || text.contains("invalid"),
                "{code}: {text}"
            );
        }
    }

    #[test]
    fn binary_files_round_trip_inside_a_sandbox() {
        let root = scratch("bytes");
        let engine = engine(FsPolicy::Sandboxed(Sandbox::new(root.clone())));
        let bytes: Blob = engine
            .eval(
                r#"
                let data = blob(3);
                data[0] = 0; data[1] = 128; data[2] = 255;
                file_write_bytes("raw.bin", data);
                file_read_bytes("raw.bin")
                "#,
            )
            .expect("bytes round-trip");
        assert_eq!(bytes, vec![0u8, 128, 255]);
        assert_eq!(fs::read(root.join("raw.bin")).expect("on disk"), bytes);
    }

    #[test]
    fn a_sandboxed_binary_read_cannot_leave_its_root() {
        let root = scratch("bytes-esc");
        let engine = engine(FsPolicy::Sandboxed(Sandbox::new(root.clone())));
        let error = engine
            .eval::<Blob>(r#"file_read_bytes("../outside.bin")"#)
            .expect_err("the path escapes");
        assert!(error.to_string().contains("access denied"), "{error}");
        let error = engine
            .eval::<Dynamic>(r#"file_write_bytes("/tmp/lazyrad-test.bin", blob(1))"#)
            .expect_err("the path is absolute");
        assert!(error.to_string().contains("access denied"), "{error}");
    }

    #[test]
    fn project_files_are_read_through_the_normal_functions() {
        let files: Rc<dyn ProjectFiles> = Rc::new(OneFile("songs/song.mod", b"\x01\x02"));
        let engine = engine_with_files(FsPolicy::Unrestricted, files);
        let bytes = engine
            .eval::<Blob>(r#"file_read_bytes("songs/song.mod")"#)
            .expect("the asset reads");
        assert_eq!(bytes, vec![1, 2]);
        assert!(
            engine
                .eval::<bool>(r#"file_exists("songs/song.mod")"#)
                .expect("exists")
        );
        // The provider is consulted first even when the disk has no such file.
        assert!(
            engine
                .eval::<Blob>(r#"file_read_bytes("missing.bin")"#)
                .is_err()
        );
    }

    #[test]
    fn a_project_file_can_be_read_as_text() {
        let files: Rc<dyn ProjectFiles> = Rc::new(OneFile("note.txt", b"hello"));
        let engine = engine_with_files(FsPolicy::Unrestricted, files);
        let text = engine
            .eval::<String>(r#"file_read_text("note.txt")"#)
            .expect("the file reads");
        assert_eq!(text, "hello");
    }

    #[test]
    fn an_unrestricted_policy_uses_the_path_as_given() {
        let root = scratch("open");
        let engine = engine(FsPolicy::Unrestricted);
        let path = root.join("f.txt");
        let path = path.to_str().expect("utf-8 temp path").replace('\\', "/");
        let script = format!(r#"file_write_text("{path}", "hi"); file_read_text("{path}")"#);
        assert_eq!(engine.eval::<String>(&script).expect("runs"), "hi");
    }
}
