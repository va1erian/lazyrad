//! End-to-end tests for the player command line.
//!
//! Only the failure paths are exercised here: a project that compiles would
//! open a window and block. The tests run the real binary and read the JSON
//! diagnostics from its stderr.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The player binary built for this test run.
fn player() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lazyrad-player"))
}

/// A scratch directory for one test, emptied first.
fn scratch(label: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("lazyrad-player-cli-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("scratch directory is created");
    path
}

/// Writes a minimal one-form project named `check` into `dir`.
fn write_project(dir: &Path, code: &str) {
    fs::write(
        dir.join("check.lrp"),
        "name = \"check\"\nversion = \"0.1.0\"\nstartup = \"main_form\"\n\n\
         [[items]]\nkind = \"form\"\nname = \"main_form\"\n\
         layout = \"main_form.lfm\"\ncode = \"main_form.rhai\"\n",
    )
    .expect("project writes");
    fs::write(
        dir.join("main_form.lfm"),
        "format = 1\n\n[window]\nname = \"main_form\"\ntitle = \"Check\"\n",
    )
    .expect("form writes");
    fs::write(dir.join("main_form.rhai"), code).expect("code writes");
}

/// The JSON objects the player wrote to stderr, in order.
fn json_reports(output: &Output) -> Vec<serde_json::Value> {
    let stderr = String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8");
    stderr
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).expect("each JSON line parses"))
        .collect()
}

#[test]
fn a_syntax_error_exits_1_with_a_json_diagnostic() {
    let dir = scratch("syntax");
    write_project(&dir, "fn broken() {\n    let x = ;\n}\n");

    let output = player().arg(&dir).output().expect("the player runs");
    assert_eq!(output.status.code(), Some(1), "a compile error exits 1");

    let reports = json_reports(&output);
    assert_eq!(reports.len(), 1, "one problem: {reports:?}");
    assert_eq!(reports[0]["kind"], "compile");
    assert_eq!(reports[0]["file"], "main_form.rhai");
    assert_eq!(reports[0]["line"], 2);
    assert!(reports[0]["col"].as_u64().unwrap_or(0) > 0);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn an_lrp_path_is_accepted_and_checked() {
    let dir = scratch("lrp");
    write_project(&dir, "fn broken() { let x = ; }");

    let output = player()
        .arg(dir.join("check.lrp"))
        .output()
        .expect("the player runs");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json_reports(&output).len(), 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_ide_can_parse_the_json_the_player_emits() {
    let dir = scratch("parse");
    write_project(&dir, "fn broken() {\n    let x = ;\n}\n");

    let output = player().arg(&dir).output().expect("the player runs");
    assert_eq!(output.status.code(), Some(1));

    // The IDE reads each stderr line through the player's own parser, so the
    // two halves cannot drift apart.
    let stderr = String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8");
    let reports: Vec<lazyrad_player::Report> = stderr
        .lines()
        .filter_map(lazyrad_player::Report::from_json)
        .collect();
    assert_eq!(reports.len(), 1, "one diagnostic: {stderr:?}");
    assert_eq!(reports[0].kind, lazyrad_player::Kind::Compile);
    assert_eq!(reports[0].file, "main_form.rhai");
    assert_eq!(reports[0].line, 2);
    assert!(reports[0].col > 0);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_directory_without_a_project_exits_1() {
    let dir = scratch("empty");
    let output = player().arg(&dir).output().expect("the player runs");
    assert_eq!(output.status.code(), Some(1));
    let reports = json_reports(&output);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["kind"], "compile");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn extra_arguments_are_rejected() {
    let output = player()
        .args(["one", "two"])
        .output()
        .expect("the player runs");
    assert_eq!(output.status.code(), Some(1));
}
