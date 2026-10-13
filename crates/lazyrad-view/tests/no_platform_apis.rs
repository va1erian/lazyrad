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
