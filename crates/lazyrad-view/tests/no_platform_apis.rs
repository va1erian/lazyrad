//! `lazyrad-view` must stay buildable for `wasm32-unknown-unknown` and the
//! static musl LazyOS target, where there is no file system, thread, process,
//! environment, network or monotonic clock to lean on. No wasm target is
//! installed in CI, so this test is the guard: it scans the crate's sources for
//! the APIs that would break that, and for `unsafe` and stdout printing.

use std::fs;
use std::path::{Path, PathBuf};

/// Tokens that must not appear in `src/`, matched as whole words.
const FORBIDDEN: &[&str] = &[
    "std::fs",
    "std::thread",
    "std::process",
    "std::env",
    "std::net",
    "Instant",
    "SystemTime",
    "unsafe",
    "println!",
    "eprintln!",
    "print!",
    "eprint!",
    "dbg!",
];

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The forbidden tokens `source` contains, each as a whole word.
fn violations(source: &str) -> Vec<&'static str> {
    FORBIDDEN
        .iter()
        .copied()
        .filter(|token| {
            source.match_indices(token).any(|(start, _)| {
                let before = source[..start].chars().next_back();
                let after = source[start + token.len()..].chars().next();
                !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char)
            })
        })
        .collect()
}

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("readable source directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn the_sources_use_no_platform_api() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(
        files.len() >= 8,
        "expected to scan the crate's modules, found {files:?}"
    );

    let mut offences = Vec::new();
    for file in &files {
        let source = fs::read_to_string(file).expect("readable source file");
        for token in violations(&source) {
            offences.push(format!("{}: `{token}`", file.display()));
        }
    }
    assert!(
        offences.is_empty(),
        "platform APIs in a wasm-clean crate:\n{}",
        offences.join("\n")
    );
}

/// The names (and raw values) a manifest section declares, ignoring comments.
fn section_entries(manifest: &str, section: &str) -> Vec<(String, String)> {
    let mut current = String::new();
    let mut entries = Vec::new();
    for line in manifest.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            current = header.trim_end_matches(']').trim().to_owned();
        } else if current == section {
            if let Some((key, value)) = line.split_once('=') {
                entries.push((key.trim().to_owned(), value.trim().to_owned()));
            }
        }
    }
    entries
}

fn manifest_sections(manifest: &str) -> Vec<String> {
    manifest
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| line.starts_with('['))
        .map(|line| line.trim_matches(|c| c == '[' || c == ']').to_owned())
        .collect()
}

/// The crate must depend on `rhai` and `thiserror` and on nothing else, and
/// must not add features to the workspace's Rhai pin: a feature such as
/// `unchecked` (no limits), `sync` or `no_*` would change the sandbox without
/// touching this crate's source.
#[test]
fn the_manifest_depends_on_exactly_rhai_and_thiserror_with_no_extra_features() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("the crate manifest");
    let dependencies = section_entries(&manifest, "dependencies");
    let names: Vec<&str> = dependencies.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["rhai.workspace", "thiserror.workspace"],
        "{dependencies:?}"
    );
    for (name, value) in &dependencies {
        assert_eq!(value, "true", "{name} must come from the workspace as is");
    }
    // No other way to bring a crate or a feature in.
    for section in manifest_sections(&manifest) {
        assert!(
            ["package", "lints", "dependencies"].contains(&section.as_str()),
            "unexpected manifest section [{section}]"
        );
    }

    // The workspace pin itself: exact version, no default features, and only
    // the two features the player needs.
    let workspace = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("Cargo.toml"),
    )
    .expect("the workspace manifest");
    let pins = section_entries(&workspace, "workspace.dependencies");
    let rhai = pins
        .iter()
        .find(|(name, _)| name == "rhai")
        .map(|(_, value)| value.as_str())
        .expect("a rhai pin");
    assert_eq!(
        rhai,
        r#"{ version = "=1.26.1", default-features = false, features = ["std", "debugging"] }"#,
        "the workspace's Rhai pin changed: re-check the sandbox in lazyrad-view"
    );
}

/// The matcher itself: every forbidden token is caught, and the look-alikes the
/// crate legitimately contains are not.
#[test]
fn the_scanner_catches_each_token_and_not_look_alikes() {
    for token in FORBIDDEN {
        let source = format!("fn f() {{ let _ = {token}; }}");
        assert_eq!(violations(&source), vec![*token], "{token}");
    }
    // The workspace lint attribute and ordinary words are fine.
    assert!(violations("#![forbid(unsafe_code)]").is_empty());
    assert!(violations("let instantly = 1; let unsafely = 2;").is_empty());
    assert!(violations("use std::fsx;").is_empty());
    // `eprintln!` is caught by its own entry (the shorter `println!` is not
    // a whole word inside it).
    assert_eq!(violations("eprintln!(\"x\")"), vec!["eprintln!"]);
}
