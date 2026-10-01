//! Integration tests for the filesystem access policy.
//!
//! Every test works in its own directory under [`std::env::temp_dir`], removes
//! it on drop, and never touches the user's real files. The symlink tests skip
//! themselves when the platform refuses to create a link (Windows without the
//! privilege), so they pass there instead of failing.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use lazyrad_runtime::fs_policy::{Access, FsError, FsPolicy, Sandbox};

/// A temporary directory that is removed when the test ends, even on a panic.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Scratch {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "lazyrad-fs-policy-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("scratch directory is created");
        Scratch { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    fn create_dir(&self, name: &str) -> PathBuf {
        let path = self.join(name);
        fs::create_dir_all(&path).expect("scratch subdirectory is created");
        path
    }

    fn write(&self, name: &str, contents: &[u8]) -> PathBuf {
        let path = self.join(name);
        fs::write(&path, contents).expect("scratch file is written");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// The canonical form of an existing path, matching what `resolve` returns.
fn canon(path: &Path) -> PathBuf {
    fs::canonicalize(path).expect("path exists")
}

/// A sandbox rooted at `scratch`.
fn sandbox(scratch: &Scratch) -> FsPolicy {
    FsPolicy::Sandboxed(Sandbox::new(scratch.path.clone()))
}

/// The string form of an absolute path, for handing to `resolve`.
fn text(path: &Path) -> &str {
    path.to_str().expect("temporary paths are valid UTF-8")
}

/// Creates a symbolic link, or returns an error on a platform that refuses
/// (Windows without the developer mode or privilege).
#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn make_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

#[cfg(not(any(unix, windows)))]
fn make_symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("this platform has no symbolic links"))
}

#[test]
fn a_regular_file_reads_and_writes_under_the_root() {
    let root = Scratch::new("normal");
    let file = root.write("hello.txt", b"hi");
    let policy = sandbox(&root);

    assert_eq!(policy.resolve("hello.txt", Access::Read), Ok(canon(&file)));
    assert_eq!(policy.resolve("hello.txt", Access::Write), Ok(canon(&file)));
}

#[test]
fn a_nested_new_file_resolves_without_being_created() {
    let root = Scratch::new("nested-new");
    let policy = sandbox(&root);

    let resolved = policy
        .resolve("a/b/c.txt", Access::Write)
        .expect("a new nested file is allowed");
    assert_eq!(
        resolved,
        canon(&root.path).join("a").join("b").join("c.txt")
    );
    // resolve must not create anything.
    assert!(!resolved.exists());
    assert!(!root.join("a").exists());
}

#[test]
fn a_parent_component_that_escapes_the_root_is_denied() {
    let root = Scratch::new("escape");
    let policy = sandbox(&root);

    assert!(matches!(
        policy.resolve("../outside.txt", Access::Read),
        Err(FsError::Denied { .. })
    ));
    assert!(matches!(
        policy.resolve("a/../../outside.txt", Access::Write),
        Err(FsError::Denied { .. })
    ));
}

#[test]
fn a_parent_component_that_stays_inside_is_allowed() {
    let root = Scratch::new("inside-dotdot");
    let policy = sandbox(&root);

    assert_eq!(
        policy.resolve("a/../b.txt", Access::Write),
        Ok(canon(&root.path).join("b.txt"))
    );
}

#[test]
fn an_absolute_path_outside_the_root_is_denied() {
    let root = Scratch::new("abs-root");
    let outside = Scratch::new("abs-outside");
    let secret = outside.write("secret.txt", b"x");
    let policy = sandbox(&root);

    assert!(matches!(
        policy.resolve(text(&secret), Access::Read),
        Err(FsError::Denied { .. })
    ));
}

#[test]
fn an_absolute_path_inside_the_root_is_allowed() {
    let root = Scratch::new("abs-inside");
    let file = root.write("f.txt", b"x");
    let policy = sandbox(&root);

    assert_eq!(policy.resolve(text(&file), Access::Read), Ok(canon(&file)));
}

#[test]
fn an_allowed_extra_file_is_readable_but_not_writable() {
    let root = Scratch::new("allow-read-root");
    let outside = Scratch::new("allow-read-picked");
    let picked = outside.write("picked.txt", b"data");
    let policy =
        FsPolicy::Sandboxed(Sandbox::new(root.path.clone()).allow(picked.clone(), Access::Read));

    assert_eq!(
        policy.resolve(text(&picked), Access::Read),
        Ok(canon(&picked))
    );
    assert!(matches!(
        policy.resolve(text(&picked), Access::Write),
        Err(FsError::Denied { .. })
    ));
}

#[test]
fn an_allowed_extra_file_with_write_grants_read_too() {
    let root = Scratch::new("allow-write-root");
    let outside = Scratch::new("allow-write-picked");
    let picked = outside.write("picked.txt", b"data");
    let policy =
        FsPolicy::Sandboxed(Sandbox::new(root.path.clone()).allow(picked.clone(), Access::Write));

    assert_eq!(
        policy.resolve(text(&picked), Access::Write),
        Ok(canon(&picked))
    );
    assert_eq!(
        policy.resolve(text(&picked), Access::Read),
        Ok(canon(&picked))
    );
}

#[test]
fn an_allowed_directory_covers_its_contents_but_only_for_its_access() {
    let root = Scratch::new("allow-dir-root");
    let outside = Scratch::new("allow-dir-picked");
    let dir = outside.create_dir("picked");
    let inner = outside.write("picked/inner.txt", b"x");
    let policy =
        FsPolicy::Sandboxed(Sandbox::new(root.path.clone()).allow(dir.clone(), Access::Read));

    assert_eq!(
        policy.resolve(text(&inner), Access::Read),
        Ok(canon(&inner))
    );
    // A new file directly under the allowed directory is covered for reading.
    let new = dir.join("new.txt");
    assert_eq!(
        policy.resolve(text(&new), Access::Read),
        Ok(canon(&dir).join("new.txt"))
    );
    assert!(matches!(
        policy.resolve(text(&inner), Access::Write),
        Err(FsError::Denied { .. })
    ));
}

#[test]
fn a_file_grant_does_not_cover_a_sibling() {
    let root = Scratch::new("allow-sibling-root");
    let outside = Scratch::new("allow-sibling-picked");
    let picked = outside.write("picked.txt", b"data");
    let sibling = outside.write("picked.txt.bak", b"data");
    let policy =
        FsPolicy::Sandboxed(Sandbox::new(root.path.clone()).allow(picked.clone(), Access::Write));

    assert!(matches!(
        policy.resolve(text(&sibling), Access::Read),
        Err(FsError::Denied { .. })
    ));
}

#[test]
fn a_symlink_inside_the_root_pointing_outside_is_denied() {
    let root = Scratch::new("link-escape");
    let outside = Scratch::new("link-escape-outside");
    let secret = outside.write("secret.txt", b"x");
    let link = root.join("escape.txt");
    if make_symlink(&secret, &link).is_err() {
        return; // The platform cannot make links; nothing to test.
    }
    let policy = sandbox(&root);

    assert!(matches!(
        policy.resolve("escape.txt", Access::Read),
        Err(FsError::Denied { .. })
    ));
    assert!(matches!(
        policy.resolve("escape.txt", Access::Write),
        Err(FsError::Denied { .. })
    ));
}

#[test]
fn a_symlinked_directory_inside_the_root_cannot_escape() {
    let root = Scratch::new("dirlink-escape");
    let outside = Scratch::new("dirlink-escape-outside");
    let secret = outside.write("secret.txt", b"x");
    let link = root.join("escape-dir");
    if make_symlink(&outside.path, &link).is_err() {
        return;
    }
    let policy = sandbox(&root);

    assert!(matches!(
        policy.resolve("escape-dir/secret.txt", Access::Read),
        Err(FsError::Denied { .. })
    ));
    assert!(matches!(
        policy.resolve("escape-dir/new.txt", Access::Write),
        Err(FsError::Denied { .. })
    ));
    // Sanity: the target itself was reachable only through the link.
    assert_eq!(canon(&secret), canon(&outside.join("secret.txt")));
}

#[test]
fn a_write_to_a_final_symlink_is_denied_even_inside_the_root() {
    let root = Scratch::new("final-link");
    let real = root.write("real.txt", b"x");
    let link = root.join("link.txt");
    if make_symlink(&real, &link).is_err() {
        return;
    }
    let policy = sandbox(&root);

    assert!(matches!(
        policy.resolve("link.txt", Access::Write),
        Err(FsError::Denied { .. })
    ));
    // Reading through a link that stays inside the root is fine.
    assert_eq!(policy.resolve("link.txt", Access::Read), Ok(canon(&real)));
}

#[test]
fn the_root_prefix_does_not_match_a_longer_sibling_name() {
    let base = Scratch::new("prefix");
    let root = base.create_dir("root");
    base.create_dir("root-evil");
    let secret = base.write("root-evil/secret.txt", b"x");
    let policy = FsPolicy::Sandboxed(Sandbox::new(root.clone()));

    // Absolute form and the equivalent climb out of the root.
    assert!(matches!(
        policy.resolve(text(&secret), Access::Read),
        Err(FsError::Denied { .. })
    ));
    assert!(matches!(
        policy.resolve("../root-evil/secret.txt", Access::Read),
        Err(FsError::Denied { .. })
    ));
}

#[test]
fn empty_nul_and_overlong_input_are_invalid_in_a_sandbox() {
    let root = Scratch::new("invalid");
    let policy = sandbox(&root);

    assert!(matches!(
        policy.resolve("", Access::Read),
        Err(FsError::Invalid { .. })
    ));
    assert!(matches!(
        policy.resolve("a\0b", Access::Read),
        Err(FsError::Invalid { .. })
    ));
    let long = "a".repeat(5000);
    assert!(matches!(
        policy.resolve(&long, Access::Write),
        Err(FsError::Invalid { .. })
    ));
}

#[test]
fn unrestricted_returns_the_path_unchanged() {
    let policy = FsPolicy::Unrestricted;

    assert_eq!(
        policy.resolve("relative/x.txt", Access::Read),
        Ok(PathBuf::from("relative/x.txt"))
    );
    assert_eq!(
        policy.resolve("../still/relative", Access::Write),
        Ok(PathBuf::from("../still/relative"))
    );
    let absolute = if cfg!(windows) {
        "C:\\Windows\\notepad.exe"
    } else {
        "/etc/hosts"
    };
    assert_eq!(
        policy.resolve(absolute, Access::Write),
        Ok(PathBuf::from(absolute))
    );
}

#[test]
fn unrestricted_still_rejects_empty_and_nul() {
    let policy = FsPolicy::Unrestricted;

    assert!(matches!(
        policy.resolve("", Access::Read),
        Err(FsError::Invalid { .. })
    ));
    assert!(matches!(
        policy.resolve("a\0b", Access::Read),
        Err(FsError::Invalid { .. })
    ));
}

#[test]
fn unrestricted_does_not_check_length() {
    // The desktop player is deliberately unrestricted; only empty and NUL are
    // refused, so a long legitimate path passes through (the sandbox is the
    // policy that caps length).
    let policy = FsPolicy::Unrestricted;
    let long = "a".repeat(5000);

    assert_eq!(
        policy.resolve(&long, Access::Read),
        Ok(PathBuf::from(&long))
    );
}

#[test]
fn resolve_never_creates_a_file() {
    let root = Scratch::new("no-create");
    let policy = sandbox(&root);

    let resolved = policy
        .resolve("does/not/exist.txt", Access::Write)
        .expect("the path is inside the root");
    assert!(!resolved.exists());
    assert!(!root.join("does").exists());
}

#[test]
fn sandboxes_do_not_share_allowed_entries() {
    let root = Scratch::new("independent-root");
    let outside = Scratch::new("independent-picked");
    let picked = outside.write("picked.txt", b"data");
    let permissive =
        FsPolicy::Sandboxed(Sandbox::new(root.path.clone()).allow(picked.clone(), Access::Read));
    let strict = sandbox(&root);

    assert!(permissive.resolve(text(&picked), Access::Read).is_ok());
    assert!(matches!(
        strict.resolve(text(&picked), Access::Read),
        Err(FsError::Denied { .. })
    ));
}

#[test]
fn a_denied_error_names_the_path_and_reason() {
    let root = Scratch::new("error-shape");
    let policy = sandbox(&root);

    let error = policy
        .resolve("../outside.txt", Access::Read)
        .expect_err("the path escapes");
    match &error {
        FsError::Denied { path, reason } => {
            assert_eq!(path, "../outside.txt");
            assert!(!reason.is_empty());
        }
        other => panic!("expected Denied, got {other:?}"),
    }
    assert!(error.to_string().contains("access denied"));
    let _: &dyn std::error::Error = &error;
}

#[test]
fn a_dangling_symlink_in_the_path_is_denied() {
    use lazyrad_runtime::fs_policy::{Access, FsPolicy, Sandbox};
    let base = std::env::temp_dir().join(format!("lazyrad_fsp_dangling_{}", std::process::id()));
    let root = base.join("root");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    // `evil` points at a directory that does not exist yet.
    let link = root.join("evil");
    let made = {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.join("missing"), &link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(outside.join("missing"), &link).is_ok()
        }
    };
    if made {
        let policy = FsPolicy::Sandboxed(Sandbox::new(root.clone()));
        assert!(policy.resolve("evil/file.txt", Access::Write).is_err());
    }
    let _ = std::fs::remove_dir_all(&base);
}
