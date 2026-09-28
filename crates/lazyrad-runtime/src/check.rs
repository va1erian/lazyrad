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
//! The check only *parses* scripts; it never runs them. A module's top-level
//! statements run when the startup form is built, so a runtime error there is
//! not caught here: it surfaces as a fatal runtime error (the player's exit code
//! 2) rather than a check failure. Evaluating modules during the check would run
//! their side effects (a `msg_box`, say) before the program starts.

use std::fs;
use std::path::Path;

use lazyrad_project::{Diagnostic, DiagnosticKind};

use crate::error::ScriptError;
use crate::form::{RuntimeError, open_project};

/// Every problem that would stop a project from starting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckReport {
    /// File-level and form-level problems: missing files, a bad `.lfm`, an
    /// unknown startup item.
    pub diagnostics: Vec<Diagnostic>,
    /// Scripts that failed to compile, each located in its source file.
    pub scripts: Vec<ScriptError>,
}

impl CheckReport {
    /// Whether the project is clean and ready to run.
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty() && self.scripts.is_empty()
    }

    /// How many problems the report lists.
    pub fn len(&self) -> usize {
        self.diagnostics.len() + self.scripts.len()
    }
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

    // One engine parses every script; compilation only needs the grammar, not
    // the form's controls or globals, so the shared setup is enough.
    let engine = crate::new_engine();
    for item in &project.items {
        let code_path = dir.join(item.code());
        // A missing or unreadable script is already a validation diagnostic, so
        // there is nothing here to compile.
        let Ok(source) = fs::read_to_string(&code_path) else {
            continue;
        };
        if let Err(error) = engine.compile(&source) {
            report.scripts.push(ScriptError::from_parse(
                item.code().display().to_string(),
                &error,
            ));
        }
    }

    Ok(report)
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
