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
    DiagnosticKind, FORM_EXTENSION, Form, Project, PropValue, SchemaRegistry, write_if_changed,
};

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
            let normalized = normalize(&bytes);
            fs::write(to.join(entry.file_name()), normalized).expect("file is copied");
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

/// The `SaveReport` is asserted to be empty in the round-trip test.
#[test]
fn sample_project_round_trips_byte_for_byte() {
    let temp = TempDir::new("round-trip");
    copy_dir(&sample_dir(), temp.path());
    let registry = SchemaRegistry::builtin();

    let project = Project::load(temp.path()).expect("sample project loads");
    assert_eq!(
        project.startup_item().map(|item| item.name()),
        Some("frmMain")
    );
    let forms = project
        .load_forms(temp.path(), registry)
        .expect("sample forms load");
    assert_eq!(forms.len(), 1);

    let (name, form) = &forms[0];
    assert_eq!(name, "frmMain");
    let combo = form.control("cboGreeting").expect("combo exists");
    assert_eq!(
        combo.prop("style"),
        Some(&PropValue::Enum("dropdown".to_owned()))
    );

    // Saving every kind of file, twice, leaves every byte untouched.
    save_everything(temp.path(), registry);
    let first = snapshot(temp.path());
    save_everything(temp.path(), registry);
    let second = snapshot(temp.path());
    assert_eq!(first, second, "a save cycle changed the sample project");
    assert_eq!(
        first.keys().len(),
        4,
        "the sample project has four files: {first:?}"
    );
}

/// Loads, saves and checks the whole project once.
///
/// Every save must be a no-op: the sample is already in the serialiser's
/// canonical form, and code files are written only when they differ.
fn save_everything(dir: &Path, registry: &SchemaRegistry) {
    let project = Project::load(dir).expect("project loads");
    assert!(project.save(dir).expect("project saves").is_empty());

    for (name, form) in project
        .load_forms(dir, registry)
        .expect("forms load")
        .iter()
    {
        let path = dir.join(format!("{name}.{FORM_EXTENSION}"));
        assert!(form.save(&path).expect("form saves").is_empty());
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
    let registry = SchemaRegistry::builtin();

    let project = Project::load(temp.path()).expect("sample project loads");
    let mut form = Form::load(&temp.path().join("frmMain.lfm"), registry).expect("form loads");
    form.controls[0].left += 8;

    let form_path = temp.path().join("frmMain.lfm");
    let report = form.save(&form_path).expect("changed form saves");
    assert_eq!(report.len(), 1);
    assert!(report.contains(&form_path));

    // The project file did not change, so it is not rewritten.
    assert!(project.save(temp.path()).expect("project saves").is_empty());

    // Saving the edit again is a no-op.
    assert!(form.save(&form_path).expect("form saves again").is_empty());
}

#[test]
fn the_sample_project_validates_cleanly() {
    let temp = TempDir::new("valid");
    copy_dir(&sample_dir(), temp.path());
    let project = Project::load(temp.path()).expect("sample project loads");
    let diagnostics = project.validate(temp.path(), SchemaRegistry::builtin());
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
        items: vec![lazyrad_project::ProjectItem::Form {
            name: "frmBroken".to_owned(),
            layout: PathBuf::from("frmBroken.lfm"),
            code: PathBuf::from("frmBroken.rhai"),
        }],
    };
    project.save(temp.path()).expect("project saves");

    let diagnostics = project.validate(temp.path(), SchemaRegistry::builtin());
    let project_file = temp.path().join("Broken.lrp");

    let startup = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.kind == DiagnosticKind::UnknownStartup)
        .expect("startup is reported");
    assert_eq!(startup.file, project_file);
    assert!(startup.line.is_some(), "startup is located");

    let missing: Vec<&lazyrad_project::Diagnostic> = diagnostics
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
        items: vec![lazyrad_project::ProjectItem::Form {
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
[form]
name = \"frmBroken\"

[[control]]
type = \"NoSuchWidget\"
name = \"bad\"
left = 0
top = 0
width = 10
height = 10
tab_index = 0
",
    )
    .expect("form is written");

    let diagnostics = project.validate(temp.path(), SchemaRegistry::builtin());
    let unknown = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.kind == DiagnosticKind::UnknownControlType)
        .unwrap_or_else(|| panic!("unknown type is reported: {diagnostics:?}"));
    assert_eq!(unknown.file, temp.path().join("frmBroken.lfm"));
    assert_eq!(unknown.line, Some(5), "the `type` line is located");
}
