#![forbid(unsafe_code)]

//! Loading and saving `.lrp` project files and `.lfm` form documents.
//!
//! A form is an [`xui_form::FormDoc`]: this crate loads the TOML through the
//! crate's schema-guided decoder and saves it through its deterministic writer.
//! Saving calls [`write_if_changed`], so an unchanged file is left untouched and
//! the returned [`SaveReport`] records only the files actually rewritten.

use std::fs;
use std::path::{Path, PathBuf};

use xui_form::{Catalog, FormDoc};

use crate::error::{Diagnostic, DiagnosticKind, Error};
use crate::model::Project;

/// A form's on-disk extension.
pub const FORM_EXTENSION: &str = "lfm";
/// A project's on-disk extension.
pub const PROJECT_EXTENSION: &str = "lrp";
/// A code-behind's on-disk extension.
pub const CODE_EXTENSION: &str = "rhai";

/// Which files a save actually rewrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SaveReport {
    /// The paths that differed from disk and were written, in write order.
    pub written: Vec<PathBuf>,
}

impl SaveReport {
    /// Whether nothing needed writing.
    pub fn is_empty(&self) -> bool {
        self.written.is_empty()
    }

    /// How many files were written.
    pub fn len(&self) -> usize {
        self.written.len()
    }

    /// Whether `path` was written.
    pub fn contains(&self, path: impl AsRef<Path>) -> bool {
        self.written
            .iter()
            .any(|written| written.as_path() == path.as_ref())
    }
}

/// Writes `contents` to `path` only when the bytes differ.
pub fn write_if_changed(path: &Path, contents: &[u8]) -> Result<SaveReport, Error> {
    let mut report = SaveReport::default();
    if existing_bytes(path)?.as_deref() != Some(contents) {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|source| Error::io(parent, source))?;
        }
        fs::write(path, contents).map_err(|source| Error::io(path, source))?;
        report.written.push(path.to_path_buf());
    }
    Ok(report)
}

/// The bytes of `path`, or `None` when the file does not exist.
fn existing_bytes(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::io(path, source)),
    }
}

/// Reads `path` as UTF-8 text.
fn read_text(path: &Path) -> Result<String, Error> {
    fs::read_to_string(path).map_err(|source| Error::io(path, source))
}

/// Loads a form document from `path` against `catalog`.
pub fn load_form(path: &Path, catalog: &Catalog) -> Result<FormDoc, Error> {
    let text = read_text(path)?;
    FormDoc::from_toml(&text, catalog).map_err(|error| {
        let diagnostic = match error.line() {
            Some(line) => Diagnostic::at(DiagnosticKind::Syntax, path, line, error.message()),
            None => Diagnostic::new(DiagnosticKind::Syntax, path, error.message()),
        };
        Error::Diagnostic(diagnostic)
    })
}

/// Serialises a form document to `path`, writing it only if it changed.
pub fn save_form(path: &Path, doc: &FormDoc, catalog: &Catalog) -> Result<SaveReport, Error> {
    write_if_changed(path, doc.to_toml(catalog).as_bytes())
}

/// Serialises `project` to TOML, mapping the (rare) writer error to a diagnostic.
fn project_to_toml(path: &Path, project: &Project) -> Result<String, Error> {
    toml::to_string(project).map_err(|source| {
        Error::Diagnostic(Diagnostic::new(
            DiagnosticKind::Syntax,
            path.to_path_buf(),
            source.to_string(),
        ))
    })
}

impl Project {
    /// Loads the single `.lrp` file in `dir`.
    pub fn load(dir: &Path) -> Result<Self, Error> {
        let path = find_project_file(dir)?;
        let text = read_text(&path)?;
        let project: Self =
            toml::from_str(&text).map_err(|source| parse_error(&path, &text, source))?;
        let expected = project.file_name();
        if path.file_name().and_then(|name| name.to_str()) != Some(expected.as_str()) {
            return Err(Error::Diagnostic(Diagnostic::new(
                DiagnosticKind::ProjectFile,
                path.clone(),
                format!(
                    "project file is named `{}` but its `name` is `{}`; rename it to `{expected}`",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    project.name,
                ),
            )));
        }
        if let Some(bad) = project.unsafe_item_path() {
            return Err(Error::Diagnostic(Diagnostic::new(
                DiagnosticKind::ProjectFile,
                path.clone(),
                format!(
                    "item path `{}` must be a plain file name in the project folder",
                    bad.display()
                ),
            )));
        }
        Ok(project)
    }

    /// Serialises the project to `<dir>/<name>.lrp`, writing it only if it
    /// changed.
    pub fn save(&self, dir: &Path) -> Result<SaveReport, Error> {
        let path = dir.join(self.file_name());
        let text = project_to_toml(&path, self)?;
        write_if_changed(&path, text.as_bytes())
    }

    /// Loads every form item's layout, keyed by item name.
    ///
    /// Modules are skipped: they have no layout.
    pub fn load_forms(
        &self,
        dir: &Path,
        catalog: &Catalog,
    ) -> Result<Vec<(String, FormDoc)>, Error> {
        let mut forms = Vec::new();
        for item in &self.items {
            if let Some(layout) = item.layout() {
                forms.push((
                    item.name().to_owned(),
                    load_form(&dir.join(layout), catalog)?,
                ));
            }
        }
        Ok(forms)
    }
}

/// Turns a `toml` parse failure into a located [`Diagnostic`].
fn parse_error(path: &Path, text: &str, source: toml::de::Error) -> Error {
    let line = source.span().map(|span| line_for_offset(text, span.start));
    let diagnostic = Diagnostic {
        kind: DiagnosticKind::Syntax,
        file: path.to_path_buf(),
        line,
        message: source.message().to_owned(),
    };
    Error::Diagnostic(diagnostic)
}

/// The one-based line an absolute byte offset falls on.
fn line_for_offset(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text.as_bytes()[..offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

/// Finds the single `.lrp` file in `dir`.
fn find_project_file(dir: &Path) -> Result<PathBuf, Error> {
    let entries = fs::read_dir(dir).map_err(|source| Error::io(dir, source))?;
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| Error::io(dir, source))?;
        let path = entry.path();
        if path.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some(PROJECT_EXTENSION)
        {
            found.push(path);
        }
    }
    found.sort();
    match found.len() {
        0 => Err(Error::Diagnostic(Diagnostic::new(
            DiagnosticKind::ProjectFile,
            dir.to_path_buf(),
            format!("no {PROJECT_EXTENSION} project file in this directory"),
        ))),
        1 => Ok(found.pop().expect("length checked")),
        _ => Err(Error::Diagnostic(Diagnostic::new(
            DiagnosticKind::ProjectFile,
            dir.to_path_buf(),
            format!(
                "expected one {PROJECT_EXTENSION} file, found {}",
                found.len()
            ),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("lazyrad-project-io-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("scratch directory is created");
        path
    }

    #[test]
    fn write_if_changed_writes_once_then_skips() {
        let dir = scratch("write-once");
        let path = dir.join("file.txt");

        let first = write_if_changed(&path, b"hello").expect("first write succeeds");
        assert_eq!(first.len(), 1);
        assert!(first.contains(&path));

        let second = write_if_changed(&path, b"hello").expect("second write succeeds");
        assert!(second.is_empty());

        let third = write_if_changed(&path, b"changed").expect("third write succeeds");
        assert!(third.contains(&path));
        assert_eq!(fs::read(&path).expect("file is readable"), b"changed");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_if_changed_creates_parent_directories() {
        let dir = scratch("nested");
        let path = dir.join("a/b/file.txt");
        let report = write_if_changed(&path, b"x").expect("write creates parents");
        assert_eq!(report.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_project_file_requires_exactly_one_lrp() {
        let dir = scratch("find");
        let empty = find_project_file(&dir).expect_err("no project file is an error");
        assert!(matches!(
            empty,
            Error::Diagnostic(Diagnostic {
                kind: DiagnosticKind::ProjectFile,
                ..
            })
        ));

        fs::write(
            dir.join("A.lrp"),
            "name = \"A\"\nversion = \"1\"\nstartup = \"x\"\n",
        )
        .expect("project is written");
        assert_eq!(
            find_project_file(&dir).expect("one project file is found"),
            dir.join("A.lrp")
        );

        fs::write(
            dir.join("B.lrp"),
            "name = \"B\"\nversion = \"1\"\nstartup = \"x\"\n",
        )
        .expect("second project is written");
        let many = find_project_file(&dir).expect_err("two project files is an error");
        assert!(matches!(many, Error::Diagnostic(_)));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn line_for_offset_counts_newlines() {
        let text = "a\nbb\nccc";
        assert_eq!(line_for_offset(text, 0), 1);
        assert_eq!(line_for_offset(text, 2), 2);
        assert_eq!(line_for_offset(text, 5), 3);
        assert_eq!(line_for_offset(text, text.len()), 3);
    }
}
