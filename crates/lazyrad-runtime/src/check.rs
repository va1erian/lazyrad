#![forbid(unsafe_code)]

//! Checking a project before it runs.
//!
//! [`check_project`] is what the player runs before opening a window: it loads
//! the `.lrp`, validates the project and every form, and compiles every script.
//! It collects *all* the problems it finds rather than stopping at the first, so
//! a run fails once with a complete list (PLAN.md §9's error list uses the same
//! data).
//!
//! Loading the project itself can fail (no `.lrp`, a bad `.lrp`); that is the
//! only [`Err`]. Everything after that is reported in the [`CheckReport`], whose
//! [`diagnostics`](CheckReport::diagnostics) come from
//! [`Project::validate`](lazyrad_project::Project::validate) and whose
//! [`scripts`](CheckReport::scripts) are the scripts that did not parse.
//!
//! The check only *parses* scripts; it never runs them. The top-level
//! statements of a module or of the startup form's script run when that form is
//! built, so a runtime error there is not caught here: it surfaces as a fatal
//! runtime error (the player's exit code 2) rather than a check failure.
//! Evaluating scripts during the check would run their side effects (a
//! `msg_box`, say) before the program starts.
//!
//! Besides parse errors, the check runs [`lint`](crate::lint), a static pass
//! over the compiled AST for the mistakes that compile but fail at run time.
//! Its findings are warnings: they go in [`CheckReport::lints`] and never make
//! [`CheckReport::is_empty`] false. The one thing the lint evaluates is a
//! harmless probe call into a module to ask the engine whether a registered
//! extension defines it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use rhai::Engine;

use lazyrad_project::{Diagnostic, DiagnosticKind, lazyrad_catalog};

use xui_rhai::ScriptError;

use crate::form::{FormRuntime, RuntimeError, open_project};

/// Every problem a check found.
///
/// [`diagnostics`](CheckReport::diagnostics) and
/// [`scripts`](CheckReport::scripts) are errors that stop a project from
/// starting; [`lints`](CheckReport::lints) are warnings about scripts that
/// compile but are likely to fail at run time, and never make
/// [`is_empty`](CheckReport::is_empty) false.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckReport {
    /// File-level and form-level problems: missing files, a bad `.lfm`, an
    /// unknown startup item.
    pub diagnostics: Vec<Diagnostic>,
    /// Scripts that failed to compile, each located in its source file.
    pub scripts: Vec<ScriptError>,
    /// Scripts that compiled but a static lint flagged, located in their file.
    pub lints: Vec<ScriptError>,
}

impl CheckReport {
    /// Whether the project is clean and ready to run.
    ///
    /// Lint warnings do not count: a project with warnings still runs.
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty() && self.scripts.is_empty()
    }

    /// How many errors (not warnings) the report lists.
    pub fn len(&self) -> usize {
        self.diagnostics.len() + self.scripts.len()
    }

    /// The lint warnings, which do not stop the project from running.
    pub fn warnings(&self) -> &[ScriptError] {
        &self.lints
    }
}

/// The engine a check compiles with: the shipped one plus every registered
/// extension, so a `module::fn` lint knows the modules they define.
fn check_engine() -> Engine {
    let mut engine = crate::new_engine();
    crate::extensions::apply(&mut engine, &crate::extensions::ExtensionScope { form: "" });
    engine
}

/// The names a script sees without declaring them: `form`, `app` and every
/// control named here.
fn supplied(names: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    let mut supplied: BTreeSet<String> = names.into_iter().collect();
    supplied.insert("form".to_owned());
    supplied.insert("app".to_owned());
    supplied
}

/// Validates and compiles the project named by `path` without opening a window.
///
/// `path` may be a project directory or an `.lrp` file. Failing to load the
/// project is an [`Err`]; every problem found afterwards is collected into the
/// returned [`CheckReport`], so the caller can print the whole list at once.
pub fn check_project(path: impl AsRef<Path>) -> Result<CheckReport, RuntimeError> {
    let (dir, project) = open_project(path.as_ref())?;
    let mut report = CheckReport {
        diagnostics: project.validate(&dir),
        scripts: Vec::new(),
        lints: Vec::new(),
    };

    // `Project::validate` only checks that the startup *item* exists; a module
    // cannot be a startup form, so report that here as a project problem rather
    // than letting it surface as a runtime failure.
    if let Some(item) = project.startup_item()
        && !item.is_form()
    {
        report.diagnostics.push(Diagnostic::new(
            DiagnosticKind::UnknownStartup,
            dir.join(project.file_name()),
            format!("startup item `{}` is not a form", project.startup),
        ));
    }

    // One engine parses every script and resolves the extension modules the
    // `module::fn` lint checks against.
    let engine = check_engine();
    let project_modules = project_modules(&project);

    // A lint needs the control names a form supplies, so parse every form once.
    let catalog = lazyrad_catalog();
    let controls = form_controls(&project, &dir, &catalog);

    for item in &project.items {
        let code_path = dir.join(item.code());
        // A missing or unreadable script is already a validation diagnostic, so
        // there is nothing here to compile.
        let Ok(source) = fs::read_to_string(&code_path) else {
            continue;
        };
        let file = item.code().display().to_string();
        match engine.compile(&source) {
            Ok(ast) => {
                let supplied = supplied(controls_for(&controls, item.name()));
                report.lints.extend(crate::lint::script(
                    &ast,
                    &file,
                    &supplied,
                    &project_modules,
                    &engine,
                ));
            }
            Err(error) => report.scripts.push(ScriptError::from_parse(file, &error)),
        }
    }

    Ok(report)
}

/// The project's own module names.
fn project_modules(project: &lazyrad_project::Project) -> BTreeSet<String> {
    project
        .items
        .iter()
        .filter(|item| item.layout().is_none())
        .map(|item| item.name().to_owned())
        .collect()
}

/// The control names of every form, keyed by form name.
fn form_controls(
    project: &lazyrad_project::Project,
    dir: &Path,
    catalog: &lazyrad_project::Catalog,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut controls = BTreeMap::new();
    for item in &project.items {
        let Some(layout) = item.layout() else {
            continue;
        };
        let path = dir.join(layout);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(doc) = lazyrad_project::parse_form(&path, &text, catalog) {
            controls.insert(
                item.name().to_owned(),
                doc.nodes.iter().map(|node| node.name.clone()).collect(),
            );
        }
    }
    controls
}

/// The controls a script may reference: its own form's, or every form's for a
/// module (which has no form of its own).
fn controls_for(controls: &BTreeMap<String, BTreeSet<String>>, name: &str) -> BTreeSet<String> {
    match controls.get(name) {
        Some(own) => own.clone(),
        None => controls.values().flatten().cloned().collect(),
    }
}

/// Checks a runtime that is already loaded, without reading any files.
///
/// This is [`check_project`] for a project held in memory (an exported
/// executable's payload): the startup item must be a form, every form must
/// validate against the catalog and every script must compile. Files are named
/// by their project-relative names, since there is no folder.
pub fn check_runtime(runtime: &FormRuntime) -> CheckReport {
    let project = runtime.project();
    let mut report = CheckReport::default();
    let project_file = project.file_name();

    match project.startup_item() {
        None => report.diagnostics.push(Diagnostic::new(
            DiagnosticKind::UnknownStartup,
            &project_file,
            format!("startup item `{}` is not in the project", project.startup),
        )),
        Some(item) if !item.is_form() => report.diagnostics.push(Diagnostic::new(
            DiagnosticKind::UnknownStartup,
            &project_file,
            format!("startup item `{}` is not a form", project.startup),
        )),
        Some(_) => {}
    }

    for item in &project.items {
        if let (Some(layout), Some(form)) = (item.layout(), runtime.forms.get(item.name())) {
            report.diagnostics.extend(
                form.doc
                    .validate(runtime.catalog())
                    .iter()
                    .map(|problem| Diagnostic::from_form(layout, problem)),
            );
        }
    }

    let engine = check_engine();
    let project_modules: BTreeSet<String> = runtime
        .modules
        .iter()
        .map(|module| module.name.clone())
        .collect();
    let controls: BTreeMap<String, BTreeSet<String>> = runtime
        .forms
        .iter()
        .map(|(name, form)| {
            (
                name.clone(),
                form.doc
                    .nodes
                    .iter()
                    .map(|node| node.name.clone())
                    .collect(),
            )
        })
        .collect();

    let sources = runtime
        .forms
        .values()
        .map(|form| {
            (
                form.code_file.as_str(),
                form.code.as_str(),
                form.name.as_str(),
            )
        })
        .chain(runtime.modules.iter().map(|module| {
            (
                module.file.as_str(),
                module.source.as_str(),
                module.name.as_str(),
            )
        }));
    for (file, source, name) in sources {
        match engine.compile(source) {
            Ok(ast) => {
                let supplied = supplied(controls_for(&controls, name));
                report.lints.extend(crate::lint::script(
                    &ast,
                    file,
                    &supplied,
                    &project_modules,
                    &engine,
                ));
            }
            Err(error) => report
                .scripts
                .push(ScriptError::from_parse(file.to_owned(), &error)),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use lazyrad_project::DiagnosticKind;

    use super::check_project;

    /// A scratch directory for one test, emptied first.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "lazyrad-runtime-check-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("scratch directory is created");
        path
    }

    /// Writes a minimal one-form project named `check` into `dir`.
    fn write_project(dir: &std::path::Path, code: &str) {
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

    #[test]
    fn the_shipped_sample_is_clean() {
        let sample = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/hello");
        let report = check_project(&sample).expect("the sample loads");
        assert!(report.is_empty(), "the sample is clean: {report:?}");
    }

    #[test]
    fn a_syntax_error_is_located_in_its_file() {
        let dir = scratch("syntax");
        write_project(&dir, "fn broken() {\n    let x = ;\n}\n");

        let report = check_project(&dir).expect("the project loads");
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        assert_eq!(report.scripts.len(), 1);
        let error = &report.scripts[0];
        assert_eq!(error.file, "main_form.rhai");
        assert_eq!(error.line, 2);
        assert!(error.column > 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_broken_script_is_reported() {
        let dir = scratch("every");
        fs::write(
            dir.join("check.lrp"),
            "name = \"check\"\nversion = \"0.1.0\"\nstartup = \"main_form\"\n\n\
             [[items]]\nkind = \"form\"\nname = \"main_form\"\n\
             layout = \"main_form.lfm\"\ncode = \"main_form.rhai\"\n\n\
             [[items]]\nkind = \"module\"\nname = \"util\"\ncode = \"util.rhai\"\n",
        )
        .expect("project writes");
        fs::write(
            dir.join("main_form.lfm"),
            "format = 1\n\n[window]\nname = \"main_form\"\n",
        )
        .expect("form writes");
        fs::write(dir.join("main_form.rhai"), "fn a() { let x = ; }").expect("code writes");
        fs::write(dir.join("util.rhai"), "fn b() { let y = ; }").expect("module writes");

        let report = check_project(&dir).expect("the project loads");
        let mut files: Vec<_> = report
            .scripts
            .iter()
            .map(|error| error.file.as_str())
            .collect();
        files.sort_unstable();
        assert_eq!(files, ["main_form.rhai", "util.rhai"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_form_becomes_a_validation_diagnostic() {
        let dir = scratch("badform");
        write_project(&dir, "fn ok() {}");
        fs::write(
            dir.join("main_form.lfm"),
            "format = 1\n\n[window]\nname = \"main_form\"\nnope = 1\n",
        )
        .expect("form writes");

        let report = check_project(&dir).expect("the project loads");
        assert!(
            !report.diagnostics.is_empty(),
            "an unknown window property is a diagnostic"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_lrp_path_loads_its_project() {
        let dir = scratch("lrp");
        write_project(&dir, "fn ok() {}");

        let report = check_project(dir.join("check.lrp")).expect("the .lrp loads");
        assert!(report.is_empty(), "{report:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_without_an_lrp_is_an_error() {
        let dir = scratch("empty");
        assert!(check_project(&dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_top_level_const_used_in_a_function_is_a_lint() {
        let dir = scratch("lint-const");
        write_project(
            &dir,
            "const GREETING = \"hi\";\nfn hello() { greeting_label.text = GREETING; }\n",
        );

        let report = check_project(&dir).expect("the project loads");
        assert!(
            report.is_empty(),
            "a lint does not stop the project running"
        );
        assert_eq!(report.warnings().len(), 1, "{:?}", report.warnings());
        let lint = &report.warnings()[0];
        assert_eq!(lint.file, "main_form.rhai");
        assert_eq!(lint.line, 2);
        assert!(lint.message.contains("GREETING"), "{}", lint.message);
        assert!(lint.message.starts_with("lint:"), "{}", lint.message);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_global_qualified_const_is_not_a_lint() {
        let dir = scratch("lint-global");
        write_project(
            &dir,
            "const GREETING = \"hi\";\nfn hello() { let x = global::GREETING; }\n",
        );
        let report = check_project(&dir).expect("the project loads");
        assert!(report.warnings().is_empty(), "{:?}", report.warnings());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_parameter_shadowing_a_const_is_not_a_lint() {
        let dir = scratch("lint-param");
        write_project(
            &dir,
            "const GREETING = \"hi\";\nfn hello(GREETING) { let x = GREETING; }\n",
        );
        let report = check_project(&dir).expect("the project loads");
        assert!(report.warnings().is_empty(), "{:?}", report.warnings());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_const_name_in_a_string_or_comment_is_not_a_lint() {
        let dir = scratch("lint-text");
        write_project(
            &dir,
            "const GREETING = \"hi\";\n\
             fn hello() {\n\
                 let x = \"GREETING\"; // GREETING is not a variable here\n\
             }\n",
        );
        let report = check_project(&dir).expect("the project loads");
        assert!(report.warnings().is_empty(), "{:?}", report.warnings());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_const_shadowed_by_a_control_is_not_a_lint() {
        let dir = scratch("lint-control");
        write_project(
            &dir,
            "const save_button = \"x\";\nfn run_it() { let x = save_button; }\n",
        );
        fs::write(
            dir.join("main_form.lfm"),
            "format = 1\n\n[window]\nname = \"main_form\"\n\n\
             [[node]]\nkind = \"Button\"\nname = \"save_button\"\n",
        )
        .expect("form writes");
        let report = check_project(&dir).expect("the project loads");
        assert!(report.scripts.is_empty(), "{:?}", report.scripts);
        assert!(report.warnings().is_empty(), "{:?}", report.warnings());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_function_that_calls_a_method_of_its_own_name_is_a_lint() {
        let dir = scratch("lint-recurse");
        write_project(&dir, "fn mute(deck, on) { deck.mute(!on); }\n");
        let report = check_project(&dir).expect("the project loads");
        assert_eq!(report.warnings().len(), 1, "{:?}", report.warnings());
        assert!(
            report.warnings()[0].message.contains("mute"),
            "{}",
            report.warnings()[0].message
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_method_call_with_a_different_name_is_not_a_lint() {
        let dir = scratch("lint-method-ok");
        write_project(&dir, "fn mute(deck, on) { deck.unmute(!on); }\n");
        let report = check_project(&dir).expect("the project loads");
        assert!(report.warnings().is_empty(), "{:?}", report.warnings());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_module_call_is_a_lint() {
        let dir = scratch("lint-module");
        write_project(&dir, "fn run_it() { sys::confd::get(\"sys/ui/theme\"); }\n");
        let report = check_project(&dir).expect("the project loads");
        assert!(report.scripts.is_empty(), "{:?}", report.scripts);
        assert_eq!(report.warnings().len(), 1, "{:?}", report.warnings());
        assert!(
            report.warnings()[0].message.contains("sys"),
            "{}",
            report.warnings()[0].message
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_module_call_is_not_a_lint() {
        let dir = scratch("lint-module-ok");
        fs::write(
            dir.join("check.lrp"),
            "name = \"check\"\nversion = \"0.1.0\"\nstartup = \"main_form\"\n\n\
             [[items]]\nkind = \"form\"\nname = \"main_form\"\n\
             layout = \"main_form.lfm\"\ncode = \"main_form.rhai\"\n\n\
             [[items]]\nkind = \"module\"\nname = \"util\"\ncode = \"util.rhai\"\n",
        )
        .expect("project writes");
        fs::write(
            dir.join("main_form.lfm"),
            "format = 1\n\n[window]\nname = \"main_form\"\n",
        )
        .expect("form writes");
        fs::write(
            dir.join("main_form.rhai"),
            "fn run_it() { util::greeting(); }",
        )
        .expect("code writes");
        fs::write(dir.join("util.rhai"), "fn greeting() { \"hi\" }\n").expect("module writes");

        let report = check_project(&dir).expect("the project loads");
        assert!(report.scripts.is_empty(), "{:?}", report.scripts);
        assert!(report.warnings().is_empty(), "{:?}", report.warnings());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_extension_module_call_is_not_a_lint() {
        crate::extensions::clear();
        crate::extensions::add(|engine| {
            let module = rhai::Module::new();
            engine.register_static_module("sys", module.into());
        });
        let dir = scratch("lint-module-ext");
        write_project(&dir, "fn run_it() { sys::confd::get(\"sys/ui/theme\"); }\n");

        let report = check_project(&dir).expect("the project loads");
        crate::extensions::clear();

        assert!(report.scripts.is_empty(), "{:?}", report.scripts);
        assert!(report.warnings().is_empty(), "{:?}", report.warnings());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_runtime_knows_the_runtimes_modules() {
        use crate::form::{FormRuntime, FormSource, ModuleSource};
        use lazyrad_project::FormDoc;

        let runtime = FormRuntime::from_sources(
            vec![FormSource::new(
                "runtime",
                FormDoc::new("runtime"),
                "fn run_it() { util::greeting(); }",
            )],
            vec![ModuleSource::new("util", "fn greeting() { \"hi\" }")],
        );
        let report = super::check_runtime(&runtime);
        assert!(report.scripts.is_empty(), "{:?}", report.scripts);
        assert!(report.warnings().is_empty(), "{:?}", report.warnings());
    }

    #[test]
    fn a_module_startup_is_a_diagnostic() {
        let dir = scratch("module-startup");
        fs::write(
            dir.join("check.lrp"),
            "name = \"check\"\nversion = \"0.1.0\"\nstartup = \"util\"\n\n\
             [[items]]\nkind = \"module\"\nname = \"util\"\ncode = \"util.rhai\"\n",
        )
        .expect("project writes");
        fs::write(dir.join("util.rhai"), "fn greeting() { \"hi\" }\n").expect("module writes");

        let report = check_project(&dir).expect("the project loads");
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == DiagnosticKind::UnknownStartup),
            "a module startup is reported: {report:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
