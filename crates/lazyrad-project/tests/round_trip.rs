//! Integration tests for the load → save → load round trip and validation.
//!
//! The sample project lives in `examples/hello`. The tests copy it into a
//! temporary directory first so the repository files are never written to, and
//! they normalise CRLF to LF on the way in so the results are the same on
//! Windows and Linux checkouts.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lazyrad_project::{
    Diagnostic, DiagnosticKind, FORM_EXTENSION, Project, ProjectItem, lazyrad_catalog, load_form,
    save_form, write_if_changed,
};
use xui_form::Value;

/// The sample project shipped with the repository.
fn sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/hello")
}

/// A temporary directory removed when the test ends.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "lazyrad-project-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temporary directory is created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Copies a directory's files, normalising CRLF to LF.
fn copy_dir(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).expect("sample directory is readable") {
        let entry = entry.expect("directory entry is readable");
        let path = entry.path();
        if path.is_file() {
            let bytes = fs::read(&path).expect("sample file is readable");
            fs::write(to.join(entry.file_name()), normalize(&bytes)).expect("file is copied");
        }
    }
}

/// Normalises `\r\n` to `\n`.
fn normalize(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            index += 1;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

#[test]
fn sample_project_round_trips_byte_for_byte() {
    let temp = TempDir::new("round-trip");
    copy_dir(&sample_dir(), temp.path());
    let catalog = lazyrad_catalog();

    let project = Project::load(temp.path()).expect("sample project loads");
    assert_eq!(
        project.startup_item().map(|item| item.name()),
        Some("main_form")
    );
    let forms = project
        .load_forms(temp.path(), &catalog)
        .expect("sample forms load");
    assert_eq!(forms.len(), 1);

    let (name, form) = &forms[0];
    assert_eq!(name, "main_form");
    let button = form.node("hello_button").expect("the button exists");
    assert_eq!(button.kind, "Button");
    assert_eq!(
        button.prop("text"),
        Some(&Value::Text("Say hello".to_owned()))
    );

    // Saving every kind of file, twice, leaves every byte untouched.
    save_everything(temp.path(), &catalog);
    let first = snapshot(temp.path());
    save_everything(temp.path(), &catalog);
    let second = snapshot(temp.path());
    assert_eq!(first, second, "a save cycle changed the sample project");
    assert_eq!(
        first.keys().len(),
        4,
        "the sample project has four files: {first:?}"
    );
}

/// Loads, saves and checks the whole project once.
fn save_everything(dir: &Path, catalog: &lazyrad_project::Catalog) {
    let project = Project::load(dir).expect("project loads");
    assert!(project.save(dir).expect("project saves").is_empty());

    for (name, form) in project.load_forms(dir, catalog).expect("forms load").iter() {
        let path = dir.join(format!("{name}.{FORM_EXTENSION}"));
        assert!(
            save_form(&path, form, catalog)
                .expect("form saves")
                .is_empty()
        );
    }

    for item in &project.items {
        let path = dir.join(item.code());
        let bytes = fs::read(&path).expect("code file is readable");
        assert!(
            write_if_changed(&path, &bytes)
                .expect("code saves")
                .is_empty()
        );
    }
}

/// Reads every file in `dir` into a map, for a byte-for-byte comparison.
fn snapshot(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    for entry in fs::read_dir(dir).expect("directory is readable") {
        let entry = entry.expect("entry is readable");
        if entry.path().is_file() {
            files.insert(
                entry.path(),
                fs::read(entry.path()).expect("file is readable"),
            );
        }
    }
    files
}

#[test]
fn editing_one_control_writes_only_its_form() {
    let temp = TempDir::new("only-changed");
    copy_dir(&sample_dir(), temp.path());
    let catalog = lazyrad_catalog();

    let project = Project::load(temp.path()).expect("sample project loads");
    let form_path = temp.path().join("main_form.lfm");
    let mut form = load_form(&form_path, &catalog).expect("form loads");
    form.node_mut("hello_button")
        .expect("command button exists")
        .set_prop("left", Value::Int(24));

    let report = save_form(&form_path, &form, &catalog).expect("changed form saves");
    assert_eq!(report.len(), 1);
    assert!(report.contains(&form_path));

    // The project file did not change, so it is not rewritten.
    assert!(project.save(temp.path()).expect("project saves").is_empty());

    // Saving the edit again is a no-op.
    assert!(
        save_form(&form_path, &form, &catalog)
            .expect("form saves again")
            .is_empty()
    );
}

#[test]
fn the_sample_project_validates_cleanly() {
    let temp = TempDir::new("valid");
    copy_dir(&sample_dir(), temp.path());
    let project = Project::load(temp.path()).expect("sample project loads");
    let diagnostics = project.validate(temp.path());
    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics: {diagnostics:?}"
    );
}

#[test]
fn missing_startup_and_files_are_located_in_the_project_file() {
    let temp = TempDir::new("missing");
    let project = Project {
        name: "Broken".to_owned(),
        version: "0.1.0".to_owned(),
        startup: "ghost".to_owned(),
        items: vec![ProjectItem::Form {
            name: "frmBroken".to_owned(),
            layout: PathBuf::from("frmBroken.lfm"),
            code: PathBuf::from("frmBroken.rhai"),
        }],
    };
    project.save(temp.path()).expect("project saves");

    let diagnostics = project.validate(temp.path());
    let project_file = temp.path().join("Broken.lrp");

    let startup = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.kind == DiagnosticKind::UnknownStartup)
        .expect("startup is reported");
    assert_eq!(startup.file, project_file);
    assert!(startup.line.is_some(), "startup is located");

    let missing: Vec<&Diagnostic> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.kind == DiagnosticKind::MissingFile)
        .collect();
    assert_eq!(missing.len(), 2, "layout and code are both missing");
    assert!(missing.iter().all(|diagnostic| diagnostic.line.is_some()));
    assert!(
        missing
            .iter()
            .all(|diagnostic| diagnostic.file == project_file)
    );
}

#[test]
fn form_problems_are_located_in_the_form_file() {
    let temp = TempDir::new("form-problems");
    let project = Project {
        name: "Broken".to_owned(),
        version: "0.1.0".to_owned(),
        startup: "frmBroken".to_owned(),
        items: vec![ProjectItem::Form {
            name: "frmBroken".to_owned(),
            layout: PathBuf::from("frmBroken.lfm"),
            code: PathBuf::from("frmBroken.rhai"),
        }],
    };
    project.save(temp.path()).expect("project saves");
    fs::write(temp.path().join("frmBroken.rhai"), "// code\n").expect("code is written");
    fs::write(
        temp.path().join("frmBroken.lfm"),
        "\
format = 1

[window]
name = \"frmBroken\"

[[node]]
kind = \"NoSuchWidget\"
name = \"bad\"
",
    )
    .expect("form is written");

    let diagnostics = project.validate(temp.path());
    let invalid = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.kind == DiagnosticKind::InvalidForm)
        .unwrap_or_else(|| panic!("invalid form is reported: {diagnostics:?}"));
    assert_eq!(invalid.file, temp.path().join("frmBroken.lfm"));
    assert_eq!(invalid.line, Some(7), "the `kind` line is located");
}

#[test]
fn a_project_file_named_differently_from_its_project_is_rejected() {
    let temp = TempDir::new("name-mismatch");
    fs::write(
        temp.path().join("App.lrp"),
        "name = \"Hello\"\nversion = \"0.1.0\"\nstartup = \"Hello\"\nitems = []\n",
    )
    .expect("project file is written");

    let error = Project::load(temp.path()).expect_err("a mismatched name is rejected");
    let lazyrad_project::Error::Diagnostic(diagnostic) = error else {
        panic!("expected a diagnostic, got {error:?}");
    };
    assert_eq!(diagnostic.kind, DiagnosticKind::ProjectFile);
    assert_eq!(diagnostic.file, temp.path().join("App.lrp"));
    assert!(
        diagnostic.message.contains("Hello.lrp"),
        "{}",
        diagnostic.message
    );
}

#[test]
fn item_paths_outside_the_project_folder_are_rejected() {
    for bad in ["../evil.rhai", "sub/frmMain.rhai", "/etc/evil.rhai"] {
        let temp = TempDir::new("unsafe-path");
        fs::write(
            temp.path().join("App.lrp"),
            format!(
                "name = \"App\"\nversion = \"0.1.0\"\nstartup = \"modX\"\n\n[[items]]\nkind = \"module\"\nname = \"modX\"\ncode = {bad:?}\n"
            ),
        )
        .expect("project file is written");
        let error = Project::load(temp.path()).expect_err("an unsafe item path is rejected");
        let lazyrad_project::Error::Diagnostic(diagnostic) = error else {
            panic!("expected a diagnostic for {bad}, got {error:?}");
        };
        assert_eq!(diagnostic.kind, DiagnosticKind::ProjectFile, "{bad}");
    }
}

#[test]
fn a_plain_file_name_is_a_single_normal_component() {
    use lazyrad_project::is_plain_file_name;
    assert!(is_plain_file_name(Path::new("frmMain.lfm")));
    assert!(!is_plain_file_name(Path::new("../frmMain.lfm")));
    assert!(!is_plain_file_name(Path::new("forms/frmMain.lfm")));
    assert!(!is_plain_file_name(Path::new("/frmMain.lfm")));
    assert!(!is_plain_file_name(Path::new("")));
    assert!(!is_plain_file_name(Path::new(".")));
}

/// Symlinks need no privilege on Unix; on Windows creating one usually does, so
/// the check is exercised on Linux CI.
#[cfg(unix)]
#[test]
fn a_symlinked_item_file_is_rejected() {
    let temp = TempDir::new("symlink-item");
    let outside = TempDir::new("symlink-target");
    fs::write(outside.path().join("secret.rhai"), "// outside").expect("target");
    std::os::unix::fs::symlink(
        outside.path().join("secret.rhai"),
        temp.path().join("util.rhai"),
    )
    .expect("symlink");
    fs::write(
        temp.path().join("app.lrp"),
        "name = \"app\"\nversion = \"0.1.0\"\nstartup = \"util\"\n\n[[items]]\nkind = \"module\"\nname = \"util\"\ncode = \"util.rhai\"\n",
    )
    .expect("project file");

    let error = Project::load(temp.path()).expect_err("a symlinked item is rejected");
    let lazyrad_project::Error::Diagnostic(diagnostic) = error else {
        panic!("expected a diagnostic, got {error:?}");
    };
    assert_eq!(diagnostic.kind, DiagnosticKind::ProjectFile);
    assert!(
        diagnostic.message.contains("symbolic link"),
        "{}",
        diagnostic.message
    );
}
