//! `.lzp` packaging: round trips, malformed requests, limits and a soak loop.
//!
//! Every package built here is re-read with the independent verifier in
//! `common`, which enforces the LazyOS package rules.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use lazyrad_packager::lzp::{
    BuiltPackage, HostPermissions, IconSet, LzpError, PackageRequest, build_package, check_player,
    icons, write_package, zip::MAX_ENTRIES,
};

use common::verify;

/// A minimal valid x86-64 ELF header plus `extra` padding bytes.
fn fake_player(extra: usize) -> Vec<u8> {
    let mut elf = vec![0u8; 64 + extra];
    elf[..4].copy_from_slice(b"\x7fELF");
    elf[4] = 2; // 64-bit
    elf[5] = 1; // little-endian
    elf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes());
    elf
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lazyrad-lzp-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn example(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name);
    fs::read_dir(&dir)
        .expect("example exists")
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "lrp"))
        .expect("an .lrp")
}

fn request<'a>(project: &'a Path, player: &'a [u8]) -> PackageRequest<'a> {
    PackageRequest {
        project,
        player,
        author: "Ada Lovelace",
        system_name: None,
        description: None,
        icons: None,
        check: None,
        permissions: None,
    }
}

/// Writes a project of `modules` empty-ish module items and returns its `.lrp`.
fn synthetic_project(dir: &Path, name: &str, modules: usize) -> PathBuf {
    let mut lrp = format!("name = \"{name}\"\nversion = \"1.2.3\"\nstartup = \"m0\"\n");
    for n in 0..modules {
        lrp.push_str(&format!(
            "\n[[items]]\nkind = \"module\"\nname = \"m{n}\"\ncode = \"m{n}.rhai\"\n"
        ));
        fs::write(
            dir.join(format!("m{n}.rhai")),
            format!("fn f{n}() {{ {n} }}\n"),
        )
        .unwrap();
    }
    let path = dir.join(format!("{name}.lrp"));
    fs::write(&path, lrp).unwrap();
    path
}

#[test]
fn every_sample_packages_and_verifies() {
    let player = fake_player(10_000);
    for sample in ["hello", "calculator", "todo"] {
        let lrp = example(sample);
        let built = build_package(&request(&lrp, &player)).expect(sample);
        let package = verify(&built.bytes).unwrap_or_else(|e| panic!("{sample}: {e}"));
        assert_eq!(package.app("author"), "Ada Lovelace");
        assert!(package.app("system_name").starts_with("user.ada-lovelace."));
        assert_eq!(package.files["bin/lrplay.elf"], player);
        // The project files are byte-identical under resources/project/.
        let dir = lrp.parent().unwrap();
        let mut project_files = 0;
        for (name, data) in &package.files {
            if let Some(rel) = name.strip_prefix("resources/project/") {
                assert_eq!(&fs::read(dir.join(rel)).unwrap(), data, "{sample}/{rel}");
                project_files += 1;
            }
        }
        assert!(
            project_files >= 3,
            "{sample}: {project_files} project files"
        );
        assert_eq!(
            built.file_name(),
            format!("{}-{}.lzp", built.system_name, built.version)
        );
    }
}

#[test]
fn the_manifest_points_the_player_at_the_packaged_project() {
    let built = build_package(&request(&example("hello"), &fake_player(0))).unwrap();
    let package = verify(&built.bytes).unwrap();
    let entry = package.manifest["entry"].as_table().unwrap();
    assert_eq!(entry["binary"].as_str(), Some("bin/lrplay.elf"));
    let args: Vec<&str> = entry["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(args, ["--project", "resources/project"]);
    assert!(
        package
            .files
            .keys()
            .any(|n| n == "resources/project/hello.lrp")
    );
}

#[test]
fn permissions_follow_what_the_scripts_use() {
    let dir = scratch("perms");
    let lrp = synthetic_project(&dir, "plain", 1);
    let plain = verify(
        &build_package(&request(&lrp, &fake_player(0)))
            .unwrap()
            .bytes,
    )
    .unwrap();
    assert!(plain.permission("files").is_empty());
    assert!(plain.permission("network").is_empty());

    fs::write(
        dir.join("m0.rhai"),
        "fn save() { file_write_text(\"n.txt\", \"x\"); }\n",
    )
    .unwrap();
    let uses = verify(
        &build_package(&request(&lrp, &fake_player(0)))
            .unwrap()
            .bytes,
    )
    .unwrap();
    let id = uses.app("system_name").to_owned();
    assert_eq!(
        uses.permission("files"),
        [
            format!("read:/data/apps/{id}/data"),
            format!("write:/data/apps/{id}/data")
        ]
    );
}

#[test]
fn the_host_declares_the_services_the_scripts_use() {
    let dir = scratch("host-perms");
    let lrp = synthetic_project(&dir, "messenger", 1);
    fs::write(
        dir.join("m0.rhai"),
        "fn f() { sys::confd::get(\"sys/x\"); }\n",
    )
    .unwrap();
    let seen = std::cell::RefCell::new(Vec::new());
    let derive = |scripts: &[&str]| {
        seen.borrow_mut()
            .extend(scripts.iter().map(|s| s.to_string()));
        HostPermissions {
            interfaces: vec![
                "os.lazy.confd.v1".to_owned(),
                "os.lazy.display.v1".to_owned(),
                "os.lazy.confd.v1".to_owned(),
            ],
            topics: vec!["subscribe:system/confd/changed/#".to_owned()],
        }
    };
    let player = fake_player(0);
    let mut req = request(&lrp, &player);
    req.permissions = Some(&derive);
    let package = verify(&build_package(&req).unwrap().bytes).unwrap();
    assert!(seen.borrow().iter().any(|s| s.contains("sys::confd::get")));
    assert_eq!(
        package.permission("interfaces"),
        ["os.lazy.display.v1", "os.lazy.confd.v1"]
    );
    assert_eq!(
        package.permission("topics"),
        ["subscribe:system/confd/changed/#"]
    );
}

#[test]
fn an_explicit_system_name_is_used_and_a_bad_one_is_refused() {
    let dir = scratch("sysname");
    let lrp = synthetic_project(&dir, "app", 1);
    let player = fake_player(0);
    let mut req = request(&lrp, &player);
    req.system_name = Some("org.example.app");
    assert_eq!(build_package(&req).unwrap().system_name, "org.example.app");
    for bad in [
        "Org.example.app",
        "a.b",
        "org.example..app",
        "org.example.app!",
        "",
    ] {
        req.system_name = Some(bad);
        assert!(
            matches!(build_package(&req), Err(LzpError::Manifest(_))),
            "{bad:?} must be refused"
        );
    }
}

#[test]
fn a_bad_version_or_author_is_reported_with_every_problem() {
    let dir = scratch("badmeta");
    let lrp = synthetic_project(&dir, "app", 1);
    let text = fs::read_to_string(&lrp)
        .unwrap()
        .replace("1.2.3", "v2-beta");
    fs::write(&lrp, text).unwrap();
    let player = fake_player(0);
    let mut req = request(&lrp, &player);
    req.author = "";
    let Err(LzpError::Manifest(problems)) = build_package(&req) else {
        panic!("expected manifest problems");
    };
    assert!(problems.len() >= 2, "{problems:?}");
}

#[test]
fn the_player_is_checked() {
    assert!(check_player(&fake_player(0)).is_ok());
    assert!(check_player(b"MZ not an elf at all, just text").is_err());
    let mut arm = fake_player(0);
    arm[18] = 0xB7; // aarch64
    assert!(check_player(&arm).is_err());
    let mut elf32 = fake_player(0);
    elf32[4] = 1;
    assert!(check_player(&elf32).is_err());
    assert!(check_player(&[]).is_err());
    // Over the per-entry limit: 16 MiB + 1.
    assert!(check_player(&fake_player(16 * 1024 * 1024 - 64 + 1)).is_err());
    assert!(check_player(&fake_player(16 * 1024 * 1024 - 64)).is_ok());

    let dir = scratch("player");
    let lrp = synthetic_project(&dir, "app", 1);
    let junk = b"hello".to_vec();
    assert!(matches!(
        build_package(&request(&lrp, &junk)),
        Err(LzpError::Player { .. })
    ));
}

#[test]
fn a_project_file_over_the_limit_is_refused() {
    let dir = scratch("huge");
    let lrp = synthetic_project(&dir, "app", 1);
    fs::write(dir.join("m0.rhai"), vec![b'a'; 9 * 1024 * 1024]).unwrap();
    assert!(matches!(
        build_package(&request(&lrp, &fake_player(0))),
        Err(LzpError::Project(_))
    ));
}

#[test]
fn the_package_total_limit_is_enforced() {
    // Seven near-limit scripts (56 MiB, under the project's own 64 MiB payload
    // cap) plus a 16 MiB player overflow the package's 64 MiB total.
    let dir = scratch("total");
    let lrp = synthetic_project(&dir, "app", 7);
    for n in 0..7 {
        fs::write(
            dir.join(format!("m{n}.rhai")),
            vec![b'a'; 8 * 1024 * 1024 - 1],
        )
        .unwrap();
    }
    let player = fake_player(16 * 1024 * 1024 - 64);
    let result = build_package(&request(&lrp, &player));
    assert!(
        matches!(result, Err(LzpError::Zip(_))),
        "7 x 8 MiB + 16 MiB must exceed the 64 MiB total: {result:?}"
    );
}

#[test]
fn names_differing_only_in_case_collide() {
    let dir = scratch("case");
    let mut lrp = String::from("name = \"app\"\nversion = \"1.0.0\"\nstartup = \"a\"\n");
    for (name, file) in [("a", "Util.rhai"), ("b", "util.rhai")] {
        lrp.push_str(&format!(
            "\n[[items]]\nkind = \"module\"\nname = \"{name}\"\ncode = \"{file}\"\n"
        ));
        // On a case-insensitive host the second write replaces the first, which
        // still leaves two references to one file: refused the same way.
        fs::write(dir.join(file), "fn f() {}").unwrap();
    }
    let path = dir.join("app.lrp");
    fs::write(&path, lrp).unwrap();
    assert!(build_package(&request(&path, &fake_player(0))).is_err());
}

#[test]
fn traversal_in_project_item_paths_is_refused() {
    let dir = scratch("traversal");
    for bad in ["../evil.rhai", "sub/evil.rhai", "/abs.rhai", "a\\\\b.rhai"] {
        let lrp = format!(
            "name = \"app\"\nversion = \"1.0.0\"\nstartup = \"m\"\n\n[[items]]\nkind = \"module\"\nname = \"m\"\ncode = \"{bad}\"\n"
        );
        let path = dir.join("app.lrp");
        fs::write(&path, lrp).unwrap();
        assert!(
            build_package(&request(&path, &fake_player(0))).is_err(),
            "{bad} must be refused"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_project_file_is_refused() {
    let dir = scratch("symlink");
    let lrp = synthetic_project(&dir, "app", 1);
    let outside = scratch("symlink-outside").join("secret.txt");
    fs::write(&outside, "secret").unwrap();
    fs::remove_file(dir.join("m0.rhai")).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("m0.rhai")).unwrap();
    assert!(build_package(&request(&lrp, &fake_player(0))).is_err());
}

#[test]
fn the_entry_limit_holds_to_the_exact_entry() {
    // manifest + player + 3 icons + .lrp = 6 fixed entries, plus one per module.
    let fixed = 6;
    let player = fake_player(0);
    let dir = scratch("edge");
    let ok = synthetic_project(&dir, "edge", MAX_ENTRIES - fixed);
    let built = build_package(&request(&ok, &player)).expect("exactly MAX_ENTRIES entries");
    assert_eq!(built.entries, MAX_ENTRIES);
    verify(&built.bytes).expect("a package at the limit verifies");

    let over = scratch("edge-over");
    let too_many = synthetic_project(&over, "edge", MAX_ENTRIES - fixed + 1);
    assert!(
        matches!(
            build_package(&request(&too_many, &player)),
            Err(LzpError::Zip(_))
        ),
        "one more entry is refused by the archive limit"
    );
}

#[test]
fn the_pre_package_check_can_veto() {
    let dir = scratch("check");
    let lrp = synthetic_project(&dir, "app", 1);
    let player = fake_player(0);
    let veto = |_: &Path| Err(vec!["main.rhai:1:1: boom".to_owned()]);
    let mut req = request(&lrp, &player);
    req.check = Some(&veto);
    let Err(LzpError::Check(lines)) = build_package(&req) else {
        panic!("the check must stop the build");
    };
    assert_eq!(lines, ["main.rhai:1:1: boom"]);
    let pass = |_: &Path| Ok(());
    req.check = Some(&pass);
    assert!(build_package(&req).is_ok());
}

#[test]
fn custom_icons_must_be_pngs_and_generated_ones_always_are() {
    let dir = scratch("icons");
    let lrp = synthetic_project(&dir, "app", 1);
    let player = fake_player(0);
    let good = IconSet {
        small: icons::default_icon("x", 16),
        medium: icons::default_icon("x", 32),
        large: icons::default_icon("x", 128),
    };
    let mut req = request(&lrp, &player);
    req.icons = Some(&good);
    let built = build_package(&req).unwrap();
    let package = verify(&built.bytes).unwrap();
    assert_eq!(package.files["icons/app-32.png"], good.medium);

    let bad = IconSet {
        small: b"GIF89a".to_vec(),
        ..good.clone()
    };
    req.icons = Some(&bad);
    assert!(matches!(build_package(&req), Err(LzpError::Icon { .. })));
}

#[test]
fn packages_are_reproducible() {
    let lrp = example("calculator");
    let player = fake_player(1000);
    let a = build_package(&request(&lrp, &player)).unwrap();
    let b = build_package(&request(&lrp, &player)).unwrap();
    assert_eq!(a.bytes, b.bytes);
}

#[test]
fn a_written_package_is_the_built_one() {
    let out = scratch("write");
    let built = build_package(&request(&example("hello"), &fake_player(0))).unwrap();
    let path = write_package(&out, &built).unwrap();
    assert_eq!(
        path.file_name().unwrap().to_string_lossy(),
        built.file_name()
    );
    assert_eq!(fs::read(&path).unwrap(), built.bytes);
    assert_eq!(
        fs::read_dir(&out).unwrap().count(),
        1,
        "no temporary file remains"
    );
}

/// Builds and re-verifies hundreds of packages with varying names, sizes and
/// contents, checking the writer stays correct and deterministic under load.
#[test]
fn soak_many_packages_round_trip() {
    let dir = scratch("soak");
    let mut seen_names = std::collections::BTreeSet::new();
    let mut bytes_total = 0usize;
    for round in 0..300usize {
        let project_dir = dir.join(format!("p{round}"));
        fs::create_dir_all(&project_dir).unwrap();
        let name = format!("App {round} {}", "x".repeat(round % 20));
        let lrp = synthetic_project(&project_dir, &format!("app{round}"), 1 + round % 25);
        // Vary the content so compression ratios and stored/deflate choices differ.
        let body = if round % 3 == 0 {
            "a".repeat(round * 37)
        } else {
            format!("{:x}", round * 7919).repeat(round % 50 + 1)
        };
        fs::write(
            project_dir.join("m0.rhai"),
            format!("// {name}\nfn f() {{ \"{body}\" }}\n"),
        )
        .unwrap();
        let player = fake_player(round * 131 % 70_000);
        let mut req = request(&lrp, &player);
        req.author = if round % 2 == 0 {
            "Ada"
        } else {
            "Grace Hopper"
        };
        let built: BuiltPackage =
            build_package(&req).unwrap_or_else(|e| panic!("round {round}: {e}"));
        let package = verify(&built.bytes).unwrap_or_else(|e| panic!("round {round}: {e}"));
        assert_eq!(package.files["bin/lrplay.elf"], player, "round {round}");
        let again = build_package(&req).unwrap();
        assert_eq!(
            again.bytes, built.bytes,
            "round {round} is not reproducible"
        );
        seen_names.insert(built.system_name.clone());
        bytes_total += built.bytes.len();
        fs::remove_dir_all(&project_dir).unwrap();
    }
    assert_eq!(
        seen_names.len(),
        300,
        "every round produced a distinct app id"
    );
    assert!(bytes_total > 300 * 500);
}
