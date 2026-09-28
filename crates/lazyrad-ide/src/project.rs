#![forbid(unsafe_code)]

//! The IDE's project session: the on-disk project plus its in-memory forms and
//! code, and the operations the Project menu and Project Explorer apply to it
//! (issue #8).
//!
//! [`ProjectSession`] wraps [`lazyrad_project::Project`] with the loaded form
//! layouts and code-behind text, so the IDE can add, remove, rename and save
//! items without touching the file model directly. The "Standard EXE" template
//! lives here too: one `Form1` with a `.lfm` layout, a `.rhai` code-behind and
//! an `.lrp` project file naming `Form1` as the startup.
//!
//! Everything in this module is pure [`std`] plus [`lazyrad_project`]: it has
//! no xui types, so the project rules are unit-testable headlessly.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use lazyrad_project::{
    Catalog, Diagnostic, Error as ProjectError, FormDoc, Project, ProjectItem, SaveReport,
    is_plain_file_name, lazyrad_catalog, load_form, save_form, write_if_changed,
};

/// The form a "Standard EXE" project starts with.
pub const DEFAULT_FORM: &str = "Form1";

/// The name the New Project dialog offers.
pub const DEFAULT_PROJECT: &str = "MyApp";

/// Why a project operation failed.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The underlying project or form file could not be read, parsed or
    /// written.
    #[error(transparent)]
    Project(#[from] ProjectError),

    /// A plain filesystem failure outside the project file model, for example
    /// renaming an item's file.
    #[error("I/O error for {path}: {source}")]
    Io {
        /// The path that could not be accessed.
        path: PathBuf,
        /// The underlying operating-system error.
        #[source]
        source: std::io::Error,
    },

    /// A name that is not a valid identifier, or is already taken.
    #[error("{0}")]
    InvalidName(String),
}

/// A loaded project and the operations the IDE applies to it.
///
/// `forms` and `code` are keyed by item name so a form layout and its
/// code-behind travel together. A form always has both; a module has only
/// code.
pub struct ProjectSession {
    dir: PathBuf,
    project: Project,
    /// The catalog form layouts are read, written and validated against.
    catalog: Catalog,
    forms: BTreeMap<String, FormDoc>,
    code: BTreeMap<String, String>,
    dirty: bool,
    /// Files a rename made obsolete, relative to `dir`. They are deleted by
    /// the next [`ProjectSession::save`], after the new files are written, so a
    /// rename never touches the disk until the user saves.
    stale_files: Vec<PathBuf>,
}

impl ProjectSession {
    /// Creates a "Standard EXE" project named `name` in `dir` and writes it.
    ///
    /// The project contains one form, [`DEFAULT_FORM`], with `Form1.lfm` and
    /// `Form1.rhai`, and an `<name>.lrp` naming it as startup.
    pub fn create(name: &str, dir: &Path) -> Result<ProjectSession, SessionError> {
        if !is_identifier(name) {
            return Err(SessionError::InvalidName(format!(
                "`{name}` is not a valid project name"
            )));
        }

        // Never overwrite: refuse a folder that already holds a project or any
        // of the files the template writes.
        refuse_existing(
            dir,
            &[
                format!("{name}.lrp"),
                format!("{DEFAULT_FORM}.lfm"),
                format!("{DEFAULT_FORM}.rhai"),
            ],
        )?;

        let mut project = Project::new(name);
        project.startup = DEFAULT_FORM.to_owned();
        project.items.push(ProjectItem::Form {
            name: DEFAULT_FORM.to_owned(),
            layout: PathBuf::from(format!("{DEFAULT_FORM}.lfm")),
            code: PathBuf::from(format!("{DEFAULT_FORM}.rhai")),
        });

        let mut forms = BTreeMap::new();
        forms.insert(DEFAULT_FORM.to_owned(), FormDoc::new(DEFAULT_FORM));
        let mut code = BTreeMap::new();
        code.insert(DEFAULT_FORM.to_owned(), form_code(DEFAULT_FORM));

        let mut session = ProjectSession {
            dir: dir.to_path_buf(),
            project,
            catalog: lazyrad_catalog(),
            forms,
            code,
            dirty: true,
            stale_files: Vec::new(),
        };
        fs::create_dir_all(dir).map_err(|source| io_error(dir, source))?;
        session.save()?;
        Ok(session)
    }

    /// Opens the `.lrp` project in `dir` and loads every form and code file.
    pub fn open(dir: &Path) -> Result<ProjectSession, SessionError> {
        let project = Project::load(dir)?;
        let catalog = lazyrad_catalog();
        let mut forms = BTreeMap::new();
        let mut code = BTreeMap::new();

        for item in &project.items {
            let name = item.name().to_owned();
            if let Some(layout) = item.layout() {
                forms.insert(name.clone(), load_form(&dir.join(layout), &catalog)?);
            }
            let path = dir.join(item.code());
            let source = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
            code.insert(name, source);
        }

        Ok(ProjectSession {
            dir: dir.to_path_buf(),
            project,
            catalog,
            forms,
            code,
            dirty: false,
            stale_files: Vec::new(),
        })
    }

    /// Writes the project, every form and every code file, returning the files
    /// that actually changed. Clears the session's dirty flag.
    pub fn save(&mut self) -> Result<SaveReport, SessionError> {
        fs::create_dir_all(&self.dir).map_err(|source| io_error(&self.dir, source))?;
        let mut report = SaveReport::default();

        for (name, form) in &self.forms {
            if let Some(path) = self.layout_path(name) {
                merge(&mut report, save_form(&path, form, &self.catalog)?);
            }
        }
        for (name, source) in &self.code {
            if let Some(path) = self.code_path(name) {
                merge(&mut report, write_if_changed(&path, source.as_bytes())?);
            }
        }
        merge(&mut report, self.project.save(&self.dir)?);

        // Only now, with every current file written and the `.lrp` pointing at
        // them, remove the files renamed items left behind.
        for stale in std::mem::take(&mut self.stale_files) {
            // Delete only plain names inside the project folder, and never a
            // file a current item still uses.
            if !is_plain_file_name(&stale) || self.references(&stale) {
                continue;
            }
            let path = self.dir.join(&stale);
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(&path, error)),
            }
        }

        self.dirty = false;
        Ok(report)
    }

    /// Saves the project as `file`, a chosen `.lrp` path.
    ///
    /// The project is renamed to the file's stem, matching the rule
    /// [`Project::load`] enforces, and written to that file's folder. Files
    /// already written under the old folder are not deleted; the new folder
    /// gets a complete copy.
    pub fn save_as(&mut self, file: &Path) -> Result<SaveReport, SessionError> {
        let dir = match file.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => self.dir.clone(),
        };
        let name = file
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.project.name.clone());
        if !is_identifier(&name) {
            return Err(SessionError::InvalidName(format!(
                "`{name}` is not a valid project name"
            )));
        }
        // Saving into another folder must not overwrite what is already there.
        if dir != self.dir {
            let mut files = vec![format!("{name}.lrp")];
            for item in &self.project.items {
                files.extend(
                    std::iter::once(item.code())
                        .chain(item.layout())
                        .map(|path| path.display().to_string()),
                );
            }
            refuse_existing(&dir, &files)?;
        }

        let old_name = std::mem::replace(&mut self.project.name, name);
        let old_dir = std::mem::replace(&mut self.dir, dir);
        let was_dirty = std::mem::replace(&mut self.dirty, true);
        let result = self.save();
        if result.is_err() {
            // A failed Save As leaves the session where it was.
            self.project.name = old_name;
            self.dir = old_dir;
            self.dirty = was_dirty;
        }
        result
    }

    /// Adds a new form from the form template, returning its name.
    ///
    /// The name is the first unused `Form<n>`.
    pub fn add_form(&mut self) -> String {
        let name = self.unique_name("Form");
        self.project.items.push(ProjectItem::Form {
            name: name.clone(),
            layout: PathBuf::from(format!("{name}.lfm")),
            code: PathBuf::from(format!("{name}.rhai")),
        });
        self.forms.insert(name.clone(), FormDoc::new(name.clone()));
        self.code.insert(name.clone(), form_code(&name));
        self.dirty = true;
        name
    }

    /// Adds a new standard module from the template, returning its name.
    ///
    /// The name is the first unused `Module<n>`.
    pub fn add_module(&mut self) -> String {
        let name = self.unique_name("Module");
        self.project.items.push(ProjectItem::Module {
            name: name.clone(),
            code: PathBuf::from(format!("{name}.rhai")),
        });
        self.code.insert(name.clone(), module_code(&name));
        self.dirty = true;
        name
    }

    /// Renames the item `old` to `new`, renaming its files on disk too.
    ///
    /// For a form, both the `.lfm` and the `.rhai` are renamed and the form's
    /// own `name` follows; the `.lrp` item is rewritten with the new paths. A
    /// newly added item whose files are not on disk yet is renamed in memory
    /// only.
    pub fn rename(&mut self, old: &str, new: &str) -> Result<(), SessionError> {
        if !is_identifier(new) {
            return Err(SessionError::InvalidName(format!(
                "`{new}` is not a valid item name"
            )));
        }
        if old == new {
            return Ok(());
        }
        if self.project.item(new).is_some() {
            return Err(SessionError::InvalidName(format!(
                "an item named `{new}` already exists"
            )));
        }
        let Some(index) = self
            .project
            .items
            .iter()
            .position(|item| item.name() == old)
        else {
            return Err(SessionError::InvalidName(format!(
                "there is no item named `{old}`"
            )));
        };

        let is_form = self.project.items[index].is_form();
        let old_code = self.project.items[index].code().to_path_buf();
        let old_layout = self.project.items[index].layout().map(Path::to_path_buf);
        let new_code = PathBuf::from(format!("{new}.rhai"));
        let new_layout = is_form.then(|| PathBuf::from(format!("{new}.lfm")));

        // Never let a rename overwrite a file that is already on disk, such as
        // one left behind by a removed item (Remove keeps its files).
        for path in std::iter::once(&new_code).chain(new_layout.as_ref()) {
            if self.dir.join(path).exists() && !self.stale_files.contains(path) {
                return Err(SessionError::InvalidName(format!(
                    "`{}` already exists in the project folder",
                    path.display()
                )));
            }
        }
        // The old files are removed by the next save, not now: until then the
        // `.lrp` on disk still names them.
        self.stale_files.push(old_code);
        self.stale_files.extend(old_layout);

        self.project.items[index] = match new_layout {
            Some(layout) => ProjectItem::Form {
                name: new.to_owned(),
                layout,
                code: new_code,
            },
            None => ProjectItem::Module {
                name: new.to_owned(),
                code: new_code,
            },
        };
        if self.project.startup == old {
            self.project.startup = new.to_owned();
        }

        if let Some(mut form) = self.forms.remove(old) {
            form.window.name = new.to_owned();
            self.forms.insert(new.to_owned(), form);
        }
        if let Some(source) = self.code.remove(old) {
            self.code.insert(new.to_owned(), source);
        }

        self.dirty = true;
        Ok(())
    }

    /// Removes `name` from the project. The item's files are left on disk, as
    /// VB does without the "delete files" option.
    ///
    /// If the removed item was the startup, startup falls to the first
    /// remaining item, or the empty string when none is left.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.project.items.len();
        self.project.items.retain(|item| item.name() != name);
        if self.project.items.len() == before {
            return false;
        }
        self.forms.remove(name);
        self.code.remove(name);
        if self.project.startup == name {
            self.project.startup = self
                .project
                .items
                .first()
                .map(|item| item.name().to_owned())
                .unwrap_or_default();
        }
        self.dirty = true;
        true
    }

    /// Marks `name` as the startup form or module.
    pub fn set_startup(&mut self, name: &str) -> bool {
        if self.project.item(name).is_none() || self.project.startup == name {
            return false;
        }
        self.project.startup = name.to_owned();
        self.dirty = true;
        true
    }

    /// Validates the project on disk, as issue #2 does.
    ///
    /// Meaningful once the project has been saved; a freshly added item is not
    /// on disk yet and reads as a missing-file diagnostic.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.project.validate(&self.dir)
    }

    /// The project's name.
    pub fn name(&self) -> &str {
        &self.project.name
    }

    /// The project's on-disk directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The project file's path, `<dir>/<name>.lrp`.
    pub fn project_file(&self) -> PathBuf {
        self.dir.join(self.project.file_name())
    }

    /// The startup item's name.
    pub fn startup(&self) -> &str {
        &self.project.startup
    }

    /// The underlying model, for validation and saving.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Whether a structural change (add, remove, rename or startup) has not
    /// been saved.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Forgets that a save was pending, after the caller has written the
    /// project.
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Whether any current item uses `path` (relative to the project folder).
    fn references(&self, path: &Path) -> bool {
        self.project
            .items
            .iter()
            .any(|item| item.code() == path || item.layout() == Some(path))
    }

    /// The names of the form items, in project order.
    pub fn form_names(&self) -> Vec<&str> {
        self.project
            .items
            .iter()
            .filter(|item| item.is_form())
            .map(ProjectItem::name)
            .collect()
    }

    /// The names of the module items, in project order.
    pub fn module_names(&self) -> Vec<&str> {
        self.project
            .items
            .iter()
            .filter(|item| !item.is_form())
            .map(ProjectItem::name)
            .collect()
    }

    /// Every item name, in project order.
    pub fn item_names(&self) -> Vec<&str> {
        self.project.items.iter().map(ProjectItem::name).collect()
    }

    /// The loaded form `name`, if it is a form in the project.
    pub fn form(&self, name: &str) -> Option<&FormDoc> {
        self.forms.get(name)
    }

    /// The code-behind or module source `name`, if the item exists.
    pub fn code(&self, name: &str) -> Option<&str> {
        self.code.get(name).map(String::as_str)
    }

    /// Replaces an item's code-behind or module source, marking the project
    /// dirty. Does nothing for a name that is not an item.
    pub fn set_code(&mut self, name: &str, source: String) -> bool {
        if self.project.item(name).is_none() {
            return false;
        }
        self.code.insert(name.to_owned(), source);
        self.dirty = true;
        true
    }

    /// The first unused `<prefix><n>` name, starting at 1.
    fn unique_name(&self, prefix: &str) -> String {
        (1..)
            .map(|number| format!("{prefix}{number}"))
            .find(|candidate| self.project.item(candidate).is_none())
            .expect("the range is unbounded")
    }

    /// The absolute path of a form's `.lfm`, if `name` is a form item.
    fn layout_path(&self, name: &str) -> Option<PathBuf> {
        self.project
            .item(name)
            .and_then(ProjectItem::layout)
            .map(|layout| self.dir.join(layout))
    }

    /// The absolute path of an item's `.rhai`, if `name` is an item.
    fn code_path(&self, name: &str) -> Option<PathBuf> {
        self.project
            .item(name)
            .map(|item| self.dir.join(item.code()))
    }
}

/// Merges `other`'s written paths into `report`.
fn merge(report: &mut SaveReport, other: SaveReport) {
    report.written.extend(other.written);
}

/// Wraps an I/O error with the path that failed.
fn io_error(path: &Path, source: std::io::Error) -> SessionError {
    SessionError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Fails when `dir` already holds a `.lrp` project or any of `files`.
fn refuse_existing(dir: &Path, files: &[String]) -> Result<(), SessionError> {
    if let Some(taken) = files.iter().find(|file| dir.join(file).exists()) {
        return Err(SessionError::InvalidName(format!(
            "`{taken}` already exists in {}",
            dir.display()
        )));
    }
    let has_project = fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "lrp"))
    });
    if has_project {
        return Err(SessionError::InvalidName(format!(
            "{} already contains a project",
            dir.display()
        )));
    }
    Ok(())
}

/// Whether `name` is a valid Rhai-style identifier.
fn is_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// The code-behind template for a new form.
fn form_code(name: &str) -> String {
    format!("// {name} — event handlers for the form.\n\nfn Form_Load() {{\n}}\n")
}

/// The source template for a new standard module.
fn module_code(name: &str) -> String {
    format!("// {name} — shared functions.\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "lazyrad-ide-project-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn create_writes_a_standard_exe_that_loads_back() {
        let dir = scratch("create");
        let session = ProjectSession::create("MyApp", &dir).expect("create succeeds");

        assert_eq!(session.name(), "MyApp");
        assert_eq!(session.startup(), DEFAULT_FORM);
        assert!(dir.join("MyApp.lrp").is_file());
        assert!(dir.join("Form1.lfm").is_file());
        assert!(dir.join("Form1.rhai").is_file());
        assert!(!session.is_dirty(), "create writes the files");

        let reopened = ProjectSession::open(&dir).expect("the project opens");
        assert_eq!(reopened.name(), "MyApp");
        assert_eq!(reopened.startup(), DEFAULT_FORM);
        assert_eq!(
            reopened.form(DEFAULT_FORM).map(|f| f.window.name.as_str()),
            Some("Form1")
        );
        assert!(reopened.code(DEFAULT_FORM).is_some());

        assert!(
            reopened.diagnostics().is_empty(),
            "a fresh Standard EXE validates: {:?}",
            reopened.diagnostics()
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_standard_exe_rejects_an_invalid_name() {
        let dir = scratch("bad-name");
        assert!(matches!(
            ProjectSession::create("1 bad", &dir),
            Err(SessionError::InvalidName(_))
        ));
    }

    #[test]
    fn adding_forms_and_modules_uses_unique_names() {
        let dir = scratch("add");
        let mut session = ProjectSession::create("MyApp", &dir).expect("create succeeds");

        assert_eq!(session.add_form(), "Form2");
        assert_eq!(session.add_module(), "Module1");
        assert_eq!(session.add_module(), "Module2");
        assert!(session.is_dirty());
        assert_eq!(session.form_names(), ["Form1", "Form2"]);
        assert_eq!(session.module_names(), ["Module1", "Module2"]);

        let report = session.save().expect("save succeeds");
        assert!(!report.is_empty());
        assert!(!session.is_dirty());
        assert!(dir.join("Form2.lfm").is_file());
        assert!(dir.join("Form2.rhai").is_file());
        assert!(dir.join("Module1.rhai").is_file());
        assert!(
            session.diagnostics().is_empty(),
            "new items validate: {:?}",
            session.diagnostics()
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn renaming_a_form_renames_its_files_and_keeps_the_project_valid() {
        let dir = scratch("rename");
        let mut session = ProjectSession::create("MyApp", &dir).expect("create succeeds");
        session.save().expect("initial save");

        session
            .rename(DEFAULT_FORM, "frmMain")
            .expect("rename succeeds");

        // Nothing moves on disk until the project is saved.
        assert!(dir.join("Form1.lfm").is_file());
        assert!(!dir.join("frmMain.lfm").exists());
        assert_eq!(session.startup(), "frmMain", "startup follows the rename");
        assert_eq!(
            session
                .form("frmMain")
                .map(|form| form.window.name.as_str()),
            Some("frmMain"),
            "the form's own name follows"
        );

        session.save().expect("save after rename");
        assert!(!dir.join("Form1.lfm").exists());
        assert!(!dir.join("Form1.rhai").exists());
        assert!(dir.join("frmMain.lfm").is_file());
        assert!(dir.join("frmMain.rhai").is_file());
        assert!(
            session.diagnostics().is_empty(),
            "the renamed project validates: {:?}",
            session.diagnostics()
        );

        let reopened = ProjectSession::open(&dir).expect("reopen");
        assert_eq!(reopened.startup(), "frmMain");
        assert!(reopened.form("frmMain").is_some());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rename_that_is_never_saved_leaves_the_project_loadable() {
        let dir = scratch("rename-discard");
        let mut session = ProjectSession::create("MyApp", &dir).expect("create succeeds");
        session
            .rename(DEFAULT_FORM, "frmMain")
            .expect("rename succeeds");
        drop(session); // Discard: the IDE closes without saving.

        let reopened = ProjectSession::open(&dir).expect("the saved project still opens");
        assert!(reopened.form(DEFAULT_FORM).is_some());
        assert!(reopened.diagnostics().is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_refuses_a_folder_that_already_holds_a_project() {
        let dir = scratch("create-existing");
        ProjectSession::create("MyApp", &dir).expect("first create succeeds");
        fs::write(dir.join(format!("{DEFAULT_FORM}.rhai")), "// mine").expect("edit code");

        assert!(matches!(
            ProjectSession::create("Other", &dir),
            Err(SessionError::InvalidName(_))
        ));
        assert_eq!(
            fs::read_to_string(dir.join(format!("{DEFAULT_FORM}.rhai"))).expect("still there"),
            "// mine",
            "the existing code is untouched"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_save_as_keeps_the_session_where_it_was() {
        let dir = scratch("save-as-bad");
        let target = scratch("save-as-bad-target");
        let mut session = ProjectSession::create("MyApp", &dir).expect("create succeeds");

        assert!(matches!(
            session.save_as(&target.join("not valid.lrp")),
            Err(SessionError::InvalidName(_))
        ));
        assert_eq!(session.name(), "MyApp");
        assert_eq!(session.dir(), dir.as_path());

        fs::create_dir_all(&target).expect("target folder");
        fs::write(target.join(format!("{DEFAULT_FORM}.rhai")), "// theirs").expect("write");
        assert!(matches!(
            session.save_as(&target.join("Copy.lrp")),
            Err(SessionError::InvalidName(_))
        ));
        assert_eq!(session.dir(), dir.as_path(), "the session did not move");
        assert_eq!(
            fs::read_to_string(target.join(format!("{DEFAULT_FORM}.rhai"))).expect("still there"),
            "// theirs"
        );

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn a_rename_never_overwrites_a_removed_items_files() {
        let dir = scratch("rename-overwrite");
        let mut session = ProjectSession::create("MyApp", &dir).expect("create succeeds");
        let second = session.add_form();
        session.save().expect("save both forms");
        fs::write(dir.join(format!("{second}.rhai")), "// keep me").expect("write code");
        assert!(session.remove(&second), "Remove keeps the files on disk");

        assert!(matches!(
            session.rename(DEFAULT_FORM, &second),
            Err(SessionError::InvalidName(_))
        ));
        assert_eq!(
            fs::read_to_string(dir.join(format!("{second}.rhai"))).expect("still there"),
            "// keep me"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_rejects_taken_and_invalid_names() {
        let dir = scratch("rename-bad");
        let mut session = ProjectSession::create("MyApp", &dir).expect("create succeeds");
        session.add_form();

        assert!(matches!(
            session.rename("Form1", "Form2"),
            Err(SessionError::InvalidName(_))
        ));
        assert!(matches!(
            session.rename("Form1", "2bad"),
            Err(SessionError::InvalidName(_))
        ));
        assert!(matches!(
            session.rename("Nope", "Fine"),
            Err(SessionError::InvalidName(_))
        ));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_the_startup_falls_back_to_the_first_item() {
        let dir = scratch("remove");
        let mut session = ProjectSession::create("MyApp", &dir).expect("create succeeds");
        let module = session.add_module();
        session.set_startup(&module);
        assert_eq!(session.startup(), "Module1");

        assert!(session.remove("Module1"));
        assert_eq!(session.startup(), "Form1", "startup falls back to Form1");
        assert!(!session.remove("Module1"), "a second remove is a no-op");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_as_moves_the_project_to_the_chosen_folder() {
        let dir = scratch("save-as-src");
        let target = scratch("save-as-dst");
        let mut session = ProjectSession::create("MyApp", &dir).expect("create succeeds");

        session
            .save_as(&target.join("Renamed.lrp"))
            .expect("save as succeeds");

        assert_eq!(session.name(), "Renamed");
        assert_eq!(session.dir(), target.as_path());
        assert!(target.join("Renamed.lrp").is_file());
        assert!(target.join("Form1.lfm").is_file());

        let reopened = ProjectSession::open(&target).expect("reopen");
        assert_eq!(reopened.name(), "Renamed");

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn the_done_when_flow_round_trips() {
        let dir = scratch("done-when");
        let mut session = ProjectSession::create("MyApp", &dir).expect("create succeeds");
        assert_eq!(session.add_form(), "Form2");
        assert_eq!(session.add_module(), "Module1");
        session
            .rename("Form2", "frmSecond")
            .expect("rename succeeds");
        session.save().expect("save succeeds");

        let reopened = ProjectSession::open(&dir).expect("reopen succeeds");
        assert_eq!(reopened.form_names(), ["Form1", "frmSecond"]);
        assert_eq!(reopened.module_names(), ["Module1"]);
        assert!(
            reopened.diagnostics().is_empty(),
            "the round trip validates: {:?}",
            reopened.diagnostics()
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_leaves_unchanged_files_alone() {
        let dir = scratch("save-once");
        let mut session = ProjectSession::create("MyApp", &dir).expect("create succeeds");
        session.add_module();
        session.save().expect("first save");

        let second = session.save().expect("second save");
        assert!(second.is_empty(), "nothing changed since the first save");

        let _ = fs::remove_dir_all(&dir);
    }
}
