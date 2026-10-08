//! Export and payload tests. Everything happens in scratch folders under the
//! system temp directory, with a fake stub; no window is opened and no user
//! setting is touched.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use lazyrad_packager::payload::{
    FOOTER_LEN, FORMAT_VERSION, MAX_ENTRIES, MAX_ENTRY_BYTES, MAX_PAYLOAD_BYTES, encode_unchecked,
};
use lazyrad_packager::{
    Entry, ExportError, ExportRequest, Payload, PayloadError, atomic_write, export,
};

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let path =
            std::env::temp_dir().join(format!("lazyrad-packager-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("scratch directory is created");
        Scratch(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const STUB: &[u8] = b"not a real executable, just bytes standing in for the player";

fn sample_lrp(sample: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join(sample)
        .join(format!("{sample}.lrp"))
}

fn write_stub(scratch: &Scratch) -> PathBuf {
    let stub = scratch.path("player.bin");
    fs::write(&stub, STUB).expect("stub writes");
    stub
}

fn read_payload(bytes: &[u8]) -> Result<Option<Payload>, PayloadError> {
    Payload::read_from(&mut Cursor::new(bytes))
}

/// A small valid exported file, to corrupt in different ways.
fn exported_bytes(scratch: &Scratch) -> Vec<u8> {
    let stub = write_stub(scratch);
    let output = scratch.path("hello.out");
    export(&ExportRequest {
        stub: &stub,
        project: &sample_lrp("hello"),
        output: &output,
    })
    .expect("export succeeds");
    fs::read(output).expect("output reads")
}

#[test]
fn the_samples_round_trip_through_an_exported_file() {
    for sample in ["hello", "calculator", "todo", "brickbreaker"] {
        let scratch = Scratch::new(&format!("roundtrip-{sample}"));
        let stub = write_stub(&scratch);
        let output = scratch.path("app.out");
        let report = export(&ExportRequest {
            stub: &stub,
            project: &sample_lrp(sample),
            output: &output,
        })
        .expect("export succeeds");
        assert!(!report.patched_windows_resources, "a fake stub is not a PE");

        let bytes = fs::read(&output).expect("output reads");
        assert_eq!(report.bytes, bytes.len() as u64);
        assert!(bytes.starts_with(STUB), "the stub comes first");

        let read = read_payload(&bytes)
            .expect("the payload reads")
            .expect("the payload is there");
        let expected = Payload::from_project(&sample_lrp(sample)).expect("the sample packs");
        assert_eq!(read, expected, "{sample}");
        assert_eq!(
            read.project_entry().name,
            format!("{sample}.lrp"),
            "{sample}"
        );
        // Nothing but the output is left behind.
        let leftovers: Vec<_> = fs::read_dir(&scratch.0)
            .expect("scratch lists")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(leftovers.len(), 2, "stub and output only: {leftovers:?}");
    }
}

#[test]
fn a_file_without_a_footer_has_no_payload() {
    assert!(read_payload(STUB).expect("reads").is_none());
    assert!(read_payload(b"").expect("reads").is_none());
    assert!(read_payload(&[7u8; 39]).expect("reads").is_none());
}

#[test]
fn a_truncated_payload_is_an_error_not_a_crash() {
    let scratch = Scratch::new("truncated");
    let bytes = exported_bytes(&scratch);
    // Drop bytes from the middle so the footer is intact but the data is short.
    let footer_start = bytes.len() - FOOTER_LEN as usize;
    let mut cut = bytes[..footer_start - 10].to_vec();
    cut.extend_from_slice(&bytes[footer_start..]);
    let error = read_payload(&cut).expect_err("a short payload fails");
    assert!(matches!(error, PayloadError::Truncated(_)), "{error}");
    assert!(error.to_string().contains("truncated"));

    // A file cut off inside the payload has lost its footer, so it cannot be
    // told apart from a bare player: it reads as "no payload", not a crash.
    assert!(
        read_payload(&bytes[..bytes.len() - 5])
            .expect("reads")
            .is_none()
    );
}

#[test]
fn appended_junk_after_the_footer_reads_as_no_payload() {
    let scratch = Scratch::new("junk");
    let mut bytes = exported_bytes(&scratch);
    bytes.extend_from_slice(b"junk");
    // The footer is no longer at the end, so this reads as a bare player.
    assert!(read_payload(&bytes).expect("reads").is_none());
}

#[test]
fn a_corrupted_payload_fails_its_checksum() {
    let scratch = Scratch::new("corrupt");
    let mut bytes = exported_bytes(&scratch);
    let middle = STUB.len() + 30;
    bytes[middle] ^= 0xff;
    let error = read_payload(&bytes).expect_err("a flipped byte fails");
    assert!(matches!(error, PayloadError::Corrupt(_)), "{error}");
}

#[test]
fn a_version_mismatch_is_a_clear_error() {
    let raw = encode_unchecked(&[("a.lrp", b"")], 0, FORMAT_VERSION + 1);
    let error = read_payload(&raw).expect_err("a newer format fails");
    assert!(
        matches!(
            error,
            PayloadError::UnsupportedVersion { found, supported }
                if found == FORMAT_VERSION + 1 && supported == FORMAT_VERSION
        ),
        "{error}"
    );
    assert!(error.to_string().contains("version"));
}

#[test]
fn an_oversized_payload_is_refused_before_it_is_read() {
    let mut raw = encode_unchecked(&[("a.lrp", b"x")], 0, FORMAT_VERSION);
    // Overwrite `length` (footer bytes 16..24 counted from the footer start).
    let footer = raw.len() - FOOTER_LEN as usize;
    raw[footer + 24..footer + 32].copy_from_slice(&(MAX_PAYLOAD_BYTES + 1).to_le_bytes());
    let error = read_payload(&raw).expect_err("an oversized claim fails");
    assert!(matches!(error, PayloadError::TooLarge(_)), "{error}");
}

#[test]
fn an_oversized_entry_is_refused() {
    let big = vec![0u8; (MAX_ENTRY_BYTES + 1) as usize];
    let raw = encode_unchecked(&[("a.lrp", b""), ("big.rhai", &big)], 0, FORMAT_VERSION);
    let error = read_payload(&raw).expect_err("a huge entry fails");
    assert!(matches!(error, PayloadError::TooLarge(_)), "{error}");
}

#[test]
fn an_entry_claiming_more_data_than_exists_is_refused() {
    // One entry whose data_len says 1 GiB but is followed by nothing.
    let mut body = Vec::new();
    body.extend_from_slice(&5u16.to_le_bytes());
    body.extend_from_slice(b"a.lrp");
    body.extend_from_slice(&(MAX_ENTRY_BYTES).to_le_bytes());
    let mut raw = body.clone();
    // The helper cannot express a lying length, so the footer is built by hand.
    let mut footer = Vec::new();
    footer.extend_from_slice(b"LAZYRAD\0");
    footer.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    footer.extend_from_slice(&1u32.to_le_bytes());
    footer.extend_from_slice(&0u64.to_le_bytes());
    footer.extend_from_slice(&(body.len() as u64).to_le_bytes());
    footer.extend_from_slice(&fnv(&body).to_le_bytes());
    raw.extend_from_slice(&footer);
    let error = read_payload(&raw).expect_err("a lying length fails");
    assert!(matches!(error, PayloadError::Truncated(_)), "{error}");
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[test]
fn too_many_entries_are_refused() {
    let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("f{i}.rhai")).collect();
    let mut entries: Vec<(&str, &[u8])> = vec![("a.lrp", b"")];
    entries.extend(names.iter().map(|name| (name.as_str(), &b""[..])));
    let raw = encode_unchecked(&entries, 0, FORMAT_VERSION);
    let error = read_payload(&raw).expect_err("too many entries fail");
    assert!(
        matches!(error, PayloadError::TooManyEntries { .. }),
        "{error}"
    );
}

#[test]
fn hostile_entry_names_are_refused_like_item_paths() {
    let bad = [
        "../evil.rhai",
        "..\\evil.rhai",
        "/etc/passwd.rhai",
        "C:evil.rhai",
        "sub/main.rhai",
        "..",
        ".",
        "",
        "main.exe",
        "main.rhai.",
        "nul\0.rhai",
        "no_extension",
    ];
    for name in bad {
        let raw = encode_unchecked(&[("a.lrp", b""), (name, b"")], 0, FORMAT_VERSION);
        let error = read_payload(&raw).expect_err(name);
        assert!(
            matches!(error, PayloadError::BadName { .. }),
            "{name}: {error}"
        );
    }
}

#[test]
fn duplicate_names_and_missing_or_extra_projects_are_refused() {
    let raw = encode_unchecked(
        &[("a.lrp", b""), ("m.rhai", b""), ("M.RHAI", b"")],
        0,
        FORMAT_VERSION,
    );
    assert!(matches!(
        read_payload(&raw).expect_err("duplicate"),
        PayloadError::Duplicate(_)
    ));
    let raw = encode_unchecked(&[("m.rhai", b"")], 0, FORMAT_VERSION);
    assert!(matches!(
        read_payload(&raw).expect_err("no lrp"),
        PayloadError::ProjectFileCount(0)
    ));
    let raw = encode_unchecked(&[("a.lrp", b""), ("b.lrp", b"")], 0, FORMAT_VERSION);
    assert!(matches!(
        read_payload(&raw).expect_err("two lrp"),
        PayloadError::ProjectFileCount(2)
    ));
}

fn two_module_project(scratch: &Scratch, first: &str, second: &str) -> PathBuf {
    let dir = scratch.path("proj");
    fs::create_dir_all(&dir).expect("dir");
    fs::write(dir.join("Foo.rhai"), "").expect("code");
    fs::write(
        dir.join("app.lrp"),
        format!(
            "name = \"app\"\nversion = \"1\"\nstartup = \"a\"\n\n[[items]]\nkind = \"module\"\nname = \"a\"\ncode = \"{first}\"\n\n[[items]]\nkind = \"module\"\nname = \"b\"\ncode = \"{second}\"\n"
        ),
    )
    .expect("lrp");
    dir.join("app.lrp")
}

/// A project with an `assets` list, for the asset round-trip tests.
fn asset_project(scratch: &Scratch, assets: &str) -> PathBuf {
    let dir = scratch.path("assetproj");
    fs::create_dir_all(dir.join("songs")).expect("songs dir");
    fs::write(dir.join("m.rhai"), "").expect("code");
    fs::write(dir.join("songs/song.mod"), b"MOD").expect("asset");
    fs::write(dir.join("songs/other.mod"), b"OTHER").expect("asset");
    fs::write(dir.join("readme.txt"), b"readme").expect("other");
    fs::write(
        dir.join("app.lrp"),
        format!(
            "name = \"app\"\nversion = \"1\"\nstartup = \"m\"\nassets = [{assets}]\n\n[[items]]\nkind = \"module\"\nname = \"m\"\ncode = \"m.rhai\"\n"
        ),
    )
    .expect("lrp");
    dir.join("app.lrp")
}

#[test]
fn assets_are_packed_under_their_relative_paths() {
    let scratch = Scratch::new("assets");
    let stub = write_stub(&scratch);
    let lrp = asset_project(&scratch, "\"songs/*.mod\", \"*.png\"");
    let output = scratch.path("app.out");
    export(&ExportRequest {
        stub: &stub,
        project: &lrp,
        output: &output,
    })
    .expect("an empty glob does not fail the export");

    let payload = read_payload(&fs::read(&output).expect("reads"))
        .expect("reads")
        .expect("has a payload");
    assert_eq!(payload.get("assets/songs/song.mod"), Some(&b"MOD"[..]));
    assert_eq!(payload.get("assets/songs/other.mod"), Some(&b"OTHER"[..]));
    assert!(
        payload.get("assets/readme.txt").is_none(),
        "an unmatched file is not packed"
    );
    let names: Vec<&str> = payload.assets().map(|(name, _)| name).collect();
    assert_eq!(names, ["songs/other.mod", "songs/song.mod"]);
}

#[test]
fn hostile_asset_names_are_refused() {
    for name in [
        "assets/../evil",
        "assets/a/../../b",
        "assets//x",
        "assets/a\\b",
        "assets/./x",
        "assets/",
    ] {
        let raw = encode_unchecked(&[("a.lrp", b""), (name, b"")], 0, FORMAT_VERSION);
        let error = read_payload(&raw).expect_err(name);
        assert!(
            matches!(error, PayloadError::BadName { .. }),
            "{name}: {error}"
        );
    }
    // A well-formed asset path is accepted and does not count as the project.
    let raw = encode_unchecked(
        &[("a.lrp", b""), ("assets/songs/song.mod", b"MOD")],
        0,
        FORMAT_VERSION,
    );
    let payload = read_payload(&raw)
        .expect("a valid asset packs")
        .expect("present");
    assert_eq!(payload.get("assets/songs/song.mod"), Some(&b"MOD"[..]));
    assert_eq!(payload.project_entry().name, "a.lrp");
}

#[test]
fn references_differing_only_by_case_are_a_clear_error() {
    let entry = |name: &str| Entry {
        name: name.to_owned(),
        data: Vec::new(),
    };
    let error = Payload::new(vec![entry("a.lrp"), entry("Foo.rhai"), entry("foo.rhai")])
        .expect_err("a case collision fails");
    assert!(matches!(error, PayloadError::Duplicate(_)), "{error}");

    // Through from_project too, when the filesystem keeps the two names apart.
    let scratch = Scratch::new("case");
    let lrp = two_module_project(&scratch, "Foo.rhai", "foo.rhai");
    let dir = scratch.path("proj");
    fs::write(dir.join("foo.rhai"), "").expect("second file");
    let distinct = fs::read_dir(&dir)
        .expect("lists")
        .filter(|e| {
            e.as_ref()
                .is_ok_and(|e| e.file_name().to_string_lossy().ends_with(".rhai"))
        })
        .count()
        == 2;
    if distinct {
        let error = Payload::from_project(&lrp).expect_err("a case collision fails");
        assert!(matches!(error, PayloadError::Duplicate(_)), "{error}");
    }
}

#[test]
fn an_exact_duplicate_reference_packs_once() {
    let scratch = Scratch::new("exactdup");
    let lrp = two_module_project(&scratch, "Foo.rhai", "Foo.rhai");
    let payload = Payload::from_project(&lrp).expect("packs");
    assert_eq!(payload.entries().len(), 2, "app.lrp and Foo.rhai");
}

#[test]
fn an_export_replaces_an_existing_output() {
    let scratch = Scratch::new("replace");
    let stub = write_stub(&scratch);
    let output = scratch.path("app.out");
    fs::write(&output, b"old").expect("old output writes");
    export(&ExportRequest {
        stub: &stub,
        project: &sample_lrp("todo"),
        output: &output,
    })
    .expect("export succeeds");
    assert!(fs::read(&output).expect("reads").starts_with(STUB));
}

#[test]
fn the_output_may_not_be_the_stub_or_a_project_file() {
    let scratch = Scratch::new("selfwrite");
    let stub = write_stub(&scratch);
    let error = export(&ExportRequest {
        stub: &stub,
        project: &sample_lrp("todo"),
        output: &stub,
    })
    .expect_err("output equal to the stub is refused");
    assert!(matches!(error, ExportError::Output { .. }), "{error}");
    assert_eq!(fs::read(&stub).expect("stub reads"), STUB, "stub untouched");

    // A differently spelled path to the same file is still the stub.
    let dotted = scratch.0.join(".").join("player.bin");
    assert!(matches!(
        export(&ExportRequest {
            stub: &stub,
            project: &sample_lrp("todo"),
            output: &dotted,
        }),
        Err(ExportError::Output { .. })
    ));

    // Nor may it overwrite the project's own source.
    let project_dir = scratch.path("proj");
    fs::create_dir_all(&project_dir).expect("project dir");
    for name in ["todo.lrp", "main_form.lfm", "main_form.rhai"] {
        let source = sample_lrp("todo").with_file_name(name);
        fs::copy(source, project_dir.join(name)).expect("copies");
    }
    let source = project_dir.join("main_form.rhai");
    let before = fs::read(&source).expect("reads");
    let error = export(&ExportRequest {
        stub: &stub,
        project: &project_dir.join("todo.lrp"),
        output: &source,
    })
    .expect_err("overwriting source is refused");
    assert!(matches!(error, ExportError::Output { .. }), "{error}");
    assert_eq!(fs::read(&source).expect("reads"), before);
}

#[test]
fn a_directory_or_missing_folder_output_is_refused_and_leaves_nothing() {
    let scratch = Scratch::new("badout");
    let stub = write_stub(&scratch);
    let project = sample_lrp("todo");

    let folder = scratch.path("folder");
    fs::create_dir_all(&folder).expect("folder");
    assert!(matches!(
        export(&ExportRequest {
            stub: &stub,
            project: &project,
            output: &folder
        }),
        Err(ExportError::Output { .. })
    ));

    let missing = scratch.path("nope").join("app.out");
    assert!(matches!(
        export(&ExportRequest {
            stub: &stub,
            project: &project,
            output: &missing
        }),
        Err(ExportError::Write { .. })
    ));
    assert!(!scratch.path("nope").exists());
}

#[cfg(unix)]
#[test]
fn a_symlinked_output_is_refused() {
    let scratch = Scratch::new("symlink");
    let stub = write_stub(&scratch);
    let target = scratch.path("target.bin");
    fs::write(&target, b"precious").expect("target writes");
    let link = scratch.path("link.out");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");
    let error = export(&ExportRequest {
        stub: &stub,
        project: &sample_lrp("todo"),
        output: &link,
    })
    .expect_err("a symlink is refused");
    assert!(matches!(error, ExportError::Output { .. }), "{error}");
    assert_eq!(fs::read(&target).expect("reads"), b"precious");
}

#[test]
fn a_bad_stub_is_refused() {
    let scratch = Scratch::new("badstub");
    let project = sample_lrp("todo");
    let output = scratch.path("app.out");

    let missing = scratch.path("missing.bin");
    assert!(matches!(
        export(&ExportRequest {
            stub: &missing,
            project: &project,
            output: &output
        }),
        Err(ExportError::Stub { .. })
    ));
    assert!(matches!(
        export(&ExportRequest {
            stub: &scratch.0,
            project: &project,
            output: &output
        }),
        Err(ExportError::Stub { .. })
    ));

    // An already exported app is not a stub.
    let exported = scratch.path("exported.out");
    fs::write(&exported, exported_bytes(&scratch)).expect("writes");
    let error = export(&ExportRequest {
        stub: &exported,
        project: &project,
        output: &output,
    })
    .expect_err("an exported app is not a stub");
    assert!(matches!(error, ExportError::Stub { .. }), "{error}");
    assert!(!output.exists());
}

#[test]
fn a_bad_project_is_refused_before_anything_is_written() {
    let scratch = Scratch::new("badproject");
    let stub = write_stub(&scratch);
    let dir = scratch.path("proj");
    fs::create_dir_all(&dir).expect("dir");
    fs::write(
        dir.join("bad.lrp"),
        "name = \"bad\"\nversion = \"1\"\nstartup = \"m\"\n\n[[items]]\nkind = \"module\"\nname = \"m\"\ncode = \"../outside.rhai\"\n",
    )
    .expect("writes");
    let output = scratch.path("app.out");
    assert!(
        export(&ExportRequest {
            stub: &stub,
            project: &dir.join("bad.lrp"),
            output: &output,
        })
        .is_err()
    );
    assert!(!output.exists());

    // A referenced file that is missing.
    fs::write(
        dir.join("bad.lrp"),
        "name = \"bad\"\nversion = \"1\"\nstartup = \"m\"\n\n[[items]]\nkind = \"module\"\nname = \"m\"\ncode = \"m.rhai\"\n",
    )
    .expect("writes");
    assert!(matches!(
        export(&ExportRequest {
            stub: &stub,
            project: &dir.join("bad.lrp"),
            output: &output,
        }),
        Err(ExportError::Payload(PayloadError::Io { .. }))
    ));
}

#[test]
fn an_icon_path_must_be_a_plain_name_and_a_real_ico() {
    let scratch = Scratch::new("icon");
    let stub = write_stub(&scratch);
    let dir = scratch.path("proj");
    fs::create_dir_all(&dir).expect("dir");
    fs::write(dir.join("m.rhai"), "").expect("code");
    let write_lrp = |icon: &str| {
        fs::write(
            dir.join("app.lrp"),
            format!(
                "name = \"app\"\nversion = \"1.0.0\"\nstartup = \"m\"\nicon = \"{icon}\"\n\n[[items]]\nkind = \"module\"\nname = \"m\"\ncode = \"m.rhai\"\n"
            ),
        )
        .expect("lrp");
    };
    let output = scratch.path("app.out");
    let request = |output: &Path| {
        export(&ExportRequest {
            stub: &stub,
            project: &dir.join("app.lrp"),
            output,
        })
    };

    write_lrp("../evil.ico");
    assert!(matches!(
        request(&output),
        Err(ExportError::Payload(PayloadError::Project(_)))
    ));

    write_lrp("app.ico");
    assert!(
        matches!(request(&output), Err(ExportError::Icon { .. })),
        "missing icon"
    );
    fs::write(dir.join("app.ico"), b"this is not an icon at all, sorry").expect("icon");
    assert!(
        matches!(request(&output), Err(ExportError::Icon { .. })),
        "not an icon"
    );
    assert!(!output.exists());

    // The real one is fine; with a non-PE stub it is simply not applied.
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("assets")
            .join("rye-shades.ico"),
        dir.join("app.ico"),
    )
    .expect("icon copies");
    request(&output).expect("a real icon exports");
}

#[test]
fn a_failed_write_leaves_no_partial_output_and_keeps_the_old_one() {
    let scratch = Scratch::new("atomic");
    let output = scratch.path("app.out");

    // First write fails halfway: nothing at all remains.
    let error = atomic_write(&output, |file| {
        use std::io::Write;
        file.write_all(b"half")?;
        Err(std::io::Error::other("disk full"))
    })
    .expect_err("the write fails");
    assert_eq!(error.to_string(), "disk full");
    assert!(!output.exists());
    assert_eq!(
        fs::read_dir(&scratch.0).expect("lists").count(),
        0,
        "no temporary file is left"
    );

    // A failure over an existing file keeps the old bytes.
    fs::write(&output, b"old").expect("old writes");
    let _ = atomic_write(&output, |file| {
        use std::io::Write;
        file.write_all(b"new but broken")?;
        Err(std::io::Error::other("boom"))
    });
    assert_eq!(fs::read(&output).expect("reads"), b"old");
    assert_eq!(fs::read_dir(&scratch.0).expect("lists").count(), 1);

    // And success replaces it.
    atomic_write(&output, |file| {
        use std::io::Write;
        file.write_all(b"new")
    })
    .expect("writes");
    assert_eq!(fs::read(&output).expect("reads"), b"new");
}

/// Exports with the running test executable as the stub: on Windows that is a
/// real PE, so this exercises the subsystem, icon and version patching.
#[cfg(windows)]
#[test]
fn a_windows_stub_gets_the_gui_subsystem_icon_and_version() {
    use editpe::Image;

    let scratch = Scratch::new("pe");
    let stub = std::env::current_exe().expect("test exe path");
    let dir = scratch.path("proj");
    fs::create_dir_all(&dir).expect("dir");
    fs::write(dir.join("m.rhai"), "").expect("code");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("assets")
            .join("rye-shades.ico"),
        dir.join("app.ico"),
    )
    .expect("icon copies");
    fs::write(
        dir.join("MyApp.lrp"),
        "name = \"MyApp\"\nversion = \"2.3.4\"\nstartup = \"m\"\nicon = \"app.ico\"\n\n[[items]]\nkind = \"module\"\nname = \"m\"\ncode = \"m.rhai\"\n",
    )
    .expect("lrp");
    let output = scratch.path("MyApp.exe");
    let report = export(&ExportRequest {
        stub: &stub,
        project: &dir.join("MyApp.lrp"),
        output: &output,
    })
    .expect("export succeeds");
    assert!(report.patched_windows_resources);

    let bytes = fs::read(&output).expect("reads");
    assert!(
        read_payload(&bytes).expect("reads").is_some(),
        "payload survives"
    );
    let image = Image::parse(&bytes[..]).expect("output is still a PE");
    assert_eq!(image.subsystem(), 2, "GUI subsystem");
    let resources = image.resource_directory().expect("resources exist");
    assert!(resources.get_main_icon().expect("icon reads").is_some());
    let version = resources
        .get_version_info()
        .expect("version reads")
        .expect("version exists");
    let strings = &version.strings[0].strings;
    assert_eq!(
        strings.get("ProductName").map(String::as_str),
        Some("MyApp")
    );
    assert_eq!(
        strings.get("FileVersion").map(String::as_str),
        Some("2.3.4")
    );
}
