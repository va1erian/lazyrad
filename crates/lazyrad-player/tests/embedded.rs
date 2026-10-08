//! Exported-app tests: export a project with a fake stub, then run it from
//! memory through the player's loader, headlessly (the offscreen backend). No
//! real window is opened, no message box is shown and no user settings are
//! read or written.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use lazyrad_packager::payload::{FORMAT_VERSION, encode_unchecked};
use lazyrad_packager::{ExportRequest, Payload, export};
use lazyrad_player::Kind;
use lazyrad_player::embedded::{load_from_exe, runtime_from_payload};
use lazyrad_runtime::FormRuntime;
use xui_canvas::OffscreenBackend;
use xui_core::app::run_app;
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let path =
            std::env::temp_dir().join(format!("lazyrad-player-exe-{label}-{}", std::process::id()));
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

const STUB: &[u8] = b"stand-in for the player executable";

fn sample_dir(sample: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join(sample)
}

/// Exports `sample` with the fake stub and returns the exported file.
fn export_sample(scratch: &Scratch, sample: &str) -> PathBuf {
    let stub = scratch.path("stub.bin");
    fs::write(&stub, STUB).expect("stub writes");
    let output = scratch.path(&format!("{sample}.out"));
    export(&ExportRequest {
        stub: &stub,
        project: &sample_dir(sample).join(format!("{sample}.lrp")),
        output: &output,
    })
    .expect("export succeeds");
    output
}

/// Builds the startup form of `runtime` offscreen and returns the names of the
/// widgets it holds.
fn build_startup(runtime: Rc<FormRuntime>, widgets: &[&str]) {
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let startup = runtime.startup_name().expect("a startup form");
    let seen: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
    let seen_inner = Rc::clone(&seen);
    let names: Vec<String> = widgets.iter().map(|name| (*name).to_owned()).collect();
    let spec = PlatformSpec::new("exported app test").size(Dip(320.0), Dip(320.0));
    run_app(backend, spec, move |ui| {
        let app = runtime.build_app(ui, &startup).expect("the form builds");
        let form = app.root_form().expect("the form is live").clone();
        *seen_inner.borrow_mut() = names
            .iter()
            .map(|name| form.widget(name).is_some())
            .collect();
        app
    })
    .expect("the event loop runs");
    assert!(
        seen.borrow().iter().all(|found| *found),
        "every named widget exists: {widgets:?} -> {:?}",
        seen.borrow()
    );
}

#[test]
fn each_sample_runs_from_memory_like_from_disk() {
    let widgets: [(&str, &[&str]); 3] = [
        ("hello", &["name_edit"]),
        ("calculator", &[]),
        ("todo", &[]),
    ];
    for (sample, names) in widgets {
        let scratch = Scratch::new(&format!("sample-{sample}"));
        let exported = export_sample(&scratch, sample);

        let from_memory = load_from_exe(&exported)
            .unwrap_or_else(|reports| panic!("{sample}: {reports:?}"))
            .expect("the exported file carries a payload");
        let from_disk = FormRuntime::load(sample_dir(sample)).expect("the sample loads");

        assert_eq!(from_memory.project(), from_disk.project(), "{sample}");
        let memory_forms: Vec<String> = from_memory.form_names();
        let disk_forms: Vec<String> = from_disk.form_names();
        assert_eq!(memory_forms, disk_forms, "{sample}");
        for name in &memory_forms {
            assert_eq!(
                from_memory.form(name).expect("form").doc,
                from_disk.form(name).expect("form").doc,
                "{sample}/{name}"
            );
            assert_eq!(
                from_memory.form(name).expect("form").code,
                from_disk.form(name).expect("form").code,
                "{sample}/{name}"
            );
        }
        build_startup(from_memory, names);
    }
}

#[test]
fn a_player_without_a_payload_behaves_as_before() {
    let scratch = Scratch::new("bare");
    let bare = scratch.path("bare.bin");
    fs::write(&bare, STUB).expect("writes");
    assert!(matches!(load_from_exe(&bare), Ok(None)));
    // A missing executable is not a payload either.
    assert!(matches!(load_from_exe(&scratch.path("gone.bin")), Ok(None)));
}

#[test]
fn a_damaged_export_gives_a_clear_report() {
    let scratch = Scratch::new("damaged");
    let exported = export_sample(&scratch, "hello");
    let good = fs::read(&exported).expect("reads");

    // Corrupted.
    let mut corrupt = good.clone();
    corrupt[STUB.len() + 20] ^= 0x55;
    let path = scratch.path("corrupt.out");
    fs::write(&path, corrupt).expect("writes");
    let reports = load_from_exe(&path).err().expect("corruption fails");
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].kind, Kind::Compile);
    assert!(
        reports[0].message.contains("corrupted"),
        "{}",
        reports[0].message
    );

    // Truncated with the footer intact.
    let footer = good.len() - 40;
    let mut short = good[..footer - 5].to_vec();
    short.extend_from_slice(&good[footer..]);
    fs::write(&path, short).expect("writes");
    let reports = load_from_exe(&path).err().expect("truncation fails");
    assert!(
        reports[0].message.contains("truncated"),
        "{}",
        reports[0].message
    );

    // Version mismatch.
    let mismatch = encode_unchecked(&[("a.lrp", b"")], 0, FORMAT_VERSION + 1);
    fs::write(&path, mismatch).expect("writes");
    let reports = load_from_exe(&path).err().expect("a newer format fails");
    assert!(
        reports[0].message.contains("version"),
        "{}",
        reports[0].message
    );

    // Oversized: a footer claiming a huge payload.
    let mut huge = encode_unchecked(&[("a.lrp", b"")], 0, FORMAT_VERSION);
    let at = huge.len() - 40 + 24;
    huge[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    fs::write(&path, huge).expect("writes");
    let reports = load_from_exe(&path)
        .err()
        .expect("an oversized claim fails");
    assert!(
        reports[0].message.contains("too large"),
        "{}",
        reports[0].message
    );
}

const LRP: &str = "name = \"a\"\nversion = \"1\"\nstartup = \"main_form\"\n\n[[items]]\nkind = \"form\"\nname = \"main_form\"\nlayout = \"main_form.lfm\"\ncode = \"main_form.rhai\"\n";
const LFM: &str = "format = 1\n\n[window]\nname = \"main_form\"\ntitle = \"A\"\n";

fn payload_of(entries: &[(&str, &str)]) -> Payload {
    let raw = encode_unchecked(
        &entries
            .iter()
            .map(|(name, text)| (*name, text.as_bytes()))
            .collect::<Vec<_>>(),
        0,
        FORMAT_VERSION,
    );
    Payload::read_from(&mut std::io::Cursor::new(raw))
        .expect("reads")
        .expect("has a payload")
}

#[test]
fn a_payload_that_does_not_compile_is_reported_not_run() {
    let payload = payload_of(&[
        ("a.lrp", LRP),
        ("main_form.lfm", LFM),
        ("main_form.rhai", "fn broken() { let x = ; }"),
    ]);
    let reports = runtime_from_payload(&payload, PathBuf::from("."))
        .err()
        .expect("a syntax error is reported");
    assert!(reports.iter().any(|report| report.file == "main_form.rhai"));
}

#[test]
fn a_payload_missing_a_referenced_file_is_reported() {
    let payload = payload_of(&[("a.lrp", LRP), ("main_form.lfm", LFM)]);
    let reports = runtime_from_payload(&payload, PathBuf::from("."))
        .err()
        .expect("a missing script is reported");
    assert!(
        reports[0].message.contains("not in the executable"),
        "{reports:?}"
    );
}

#[test]
fn a_payload_with_an_unsafe_item_path_is_refused() {
    let lrp = LRP.replace("main_form.rhai", "../main_form.rhai");
    // The entry name is what the payload validates; the item path inside the
    // .lrp is what the project parser validates.
    let payload = payload_of(&[("a.lrp", &lrp), ("main_form.lfm", LFM)]);
    assert!(runtime_from_payload(&payload, PathBuf::from(".")).is_err());
}

#[test]
fn a_payload_whose_lrp_name_disagrees_with_its_project_is_refused() {
    let payload = payload_of(&[
        ("other.lrp", LRP),
        ("main_form.lfm", LFM),
        ("main_form.rhai", ""),
    ]);
    assert!(runtime_from_payload(&payload, PathBuf::from(".")).is_err());
}

/// Export with the real player binary as the stub, and a project icon. The
/// exported file is only inspected and loaded, never launched (that would open
/// a window).
#[test]
fn the_real_player_is_a_valid_stub() {
    let scratch = Scratch::new("real-stub");
    let stub = PathBuf::from(env!("CARGO_BIN_EXE_lazyrad-player"));
    let dir = scratch.path("proj");
    fs::create_dir_all(&dir).expect("project dir");
    for name in ["hello.lrp", "main_form.lfm", "main_form.rhai", "util.rhai"] {
        fs::copy(sample_dir("hello").join(name), dir.join(name)).expect("sample copies");
    }
    let icon = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("rye-shades.ico");
    fs::copy(&icon, dir.join("hello.ico")).expect("icon copies");
    let lrp = fs::read_to_string(dir.join("hello.lrp")).expect("lrp reads");
    fs::write(
        dir.join("hello.lrp"),
        lrp.replace("startup =", "icon = \"hello.ico\"\nstartup ="),
    )
    .expect("lrp writes");

    let output = scratch.path("hello.exe");
    let report = export(&ExportRequest {
        stub: &stub,
        project: &dir.join("hello.lrp"),
        output: &output,
    })
    .expect("export succeeds");
    assert!(load_from_exe(&output).expect("loads").is_some());

    #[cfg(windows)]
    {
        assert!(report.patched_windows_resources);
        let bytes = fs::read(&output).expect("reads");
        let image = editpe::Image::parse(&bytes[..]).expect("still a PE");
        assert_eq!(image.subsystem(), 2, "the GUI subsystem: no console window");
        let resources = image.resource_directory().expect("resources");
        assert!(resources.get_main_icon().expect("icon").is_some());
        // The unpatched player stays a console program for the IDE's pipes.
        let original = fs::read(&stub).expect("stub reads");
        let original = editpe::Image::parse(&original[..]).expect("player is a PE");
        assert_eq!(original.subsystem(), 3, "the shipped player is console");
        // The bare player has its default icon embedded at build time.
        assert!(
            original
                .resource_directory()
                .expect("resources")
                .get_main_icon()
                .expect("icon")
                .is_some()
        );
    }
    #[cfg(not(windows))]
    assert!(!report.patched_windows_resources);
}
