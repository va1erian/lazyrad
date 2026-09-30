//! The `lazyrad-pack` command end to end: exit codes, output files, dev install.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn pack() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lazyrad-pack"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lazyrad-pack-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn player_file(dir: &Path) -> PathBuf {
    let mut elf = vec![0u8; 4096];
    elf[..4].copy_from_slice(b"\x7fELF");
    elf[4] = 2;
    elf[5] = 1;
    elf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes());
    let path = dir.join("lrplay.elf");
    fs::write(&path, elf).unwrap();
    path
}

fn sample(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn packaging_a_sample_writes_a_package_the_verifier_accepts() {
    let dir = scratch("ok");
    let out = pack()
        .arg(sample("hello"))
        .args(["--player"])
        .arg(player_file(&dir))
        .args(["--out"])
        .arg(dir.join("dist"))
        .args(["--author", "Ada"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let packages: Vec<_> = fs::read_dir(dir.join("dist")).unwrap().collect();
    assert_eq!(packages.len(), 1);
    let path = packages[0].as_ref().unwrap().path();
    assert_eq!(path.extension().unwrap(), "lzp");
    let package = common::verify(&fs::read(&path).unwrap()).expect("a valid package");
    assert_eq!(package.app("system_name"), "user.ada.hello");
    assert!(text(&out).contains("user.ada.hello"));
}

#[test]
fn install_dev_saves_the_package_and_says_not_installed() {
    let dir = scratch("dev");
    let out = pack()
        .arg(sample("calculator"))
        .arg("--player")
        .arg(player_file(&dir))
        .arg("--out")
        .arg(dir.join("dist"))
        .arg("--author")
        .arg("Ada")
        .arg("--install-dev")
        .arg(dir.join("saved"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("saved, not installed"),
        "{}",
        text(&out)
    );
    assert_eq!(fs::read_dir(dir.join("saved")).unwrap().count(), 1);
}

#[test]
fn usage_errors_exit_two() {
    let out = pack().output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = pack().arg("proj").arg("--bogus").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn failures_exit_one_and_write_nothing() {
    let dir = scratch("fail");
    // No player given and none in the environment.
    let out = pack()
        .env_remove("LAZYRAD_PLAYER")
        .arg(sample("hello"))
        .arg("--out")
        .arg(dir.join("dist"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("no player"));

    // A player that is not an ELF.
    let bogus = dir.join("bogus.elf");
    fs::write(&bogus, "hello").unwrap();
    let out = pack()
        .arg(sample("hello"))
        .arg("--player")
        .arg(&bogus)
        .arg("--out")
        .arg(dir.join("dist"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!dir.join("dist").exists(), "nothing was written");

    // A missing project.
    let out = pack()
        .arg(dir.join("nowhere"))
        .arg("--player")
        .arg(player_file(&dir))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn a_project_with_a_script_error_is_stopped_by_the_check() {
    let dir = scratch("check");
    let project = dir.join("proj");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("bad.lrp"),
        "name = \"bad\"\nversion = \"1.0.0\"\nstartup = \"m\"\n\n[[items]]\nkind = \"module\"\nname = \"m\"\ncode = \"m.rhai\"\n",
    )
    .unwrap();
    fs::write(project.join("m.rhai"), "fn broken( {\n").unwrap();
    let run = |extra: &[&str]| {
        pack()
            .arg(&project)
            .arg("--player")
            .arg(player_file(&dir))
            .arg("--out")
            .arg(dir.join("dist"))
            .args(extra)
            .output()
            .unwrap()
    };
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("m.rhai"), "{}", text(&out));
    assert!(!dir.join("dist").exists());
    // `--no-check` packs it anyway (the player would report the error).
    let out = run(&["--no-check"]);
    assert!(out.status.success(), "{}", text(&out));
}
