#![forbid(unsafe_code)]

//! Validation of a project and its forms.
//!
//! Validation answers two kinds of question:
//!
//! 1. file-level: the startup item exists and every referenced file is present;
//! 2. form-level: each `.lfm` loads and passes [`xui_form::FormDoc::validate`]
//!    against the [`lazyrad_catalog`].
//!
//! Each problem becomes a [`Diagnostic`] naming the file and, when the text can
//! be located, the line.

use std::fs;
use std::path::Path;

use crate::error::{Diagnostic, DiagnosticKind};
use crate::io::load_form;
use crate::model::Project;
use crate::schema::lazyrad_catalog;

impl Project {
    /// Validates the whole project directory against the LazyRAD catalog.
    ///
    /// Never fails: unreadable or malformed files become diagnostics so the
    /// caller can show every problem at once.
    pub fn validate(&self, dir: &Path) -> Vec<Diagnostic> {
        let catalog = lazyrad_catalog();
        let project_path = dir.join(self.file_name());
        let project_text = fs::read_to_string(&project_path).unwrap_or_default();
        let mut diagnostics = Vec::new();

        if self.item(&self.startup).is_none() {
            let message = format!("startup item `{}` is not in the project", self.startup);
            diagnostics.push(located(
                DiagnosticKind::UnknownStartup,
                &project_path,
                &project_text,
                "startup",
                message,
            ));
        }

        for item in &self.items {
            if let Some(layout) = item.layout() {
                let path = dir.join(layout);
                if path.is_file() {
                    match load_form(&path, &catalog) {
                        Ok(doc) => diagnostics.extend(
                            doc.validate(&catalog)
                                .iter()
                                .map(|problem| Diagnostic::from_form(&path, problem)),
                        ),
                        Err(crate::error::Error::Diagnostic(mut diagnostic)) => {
                            diagnostic.kind = DiagnosticKind::InvalidForm;
                            diagnostics.push(diagnostic);
                        }
                        // The file exists but could not be read: it is as
                        // unusable as a missing one, not a syntax problem.
                        Err(error @ crate::error::Error::Io { .. }) => diagnostics.push(
                            Diagnostic::new(DiagnosticKind::MissingFile, &path, error.to_string()),
                        ),
                    }
                } else {
                    diagnostics.push(referenced_file_missing(
                        &project_path,
                        &project_text,
                        "layout",
                        layout.to_string_lossy().as_ref(),
                    ));
                }
            }

            let code = item.code();
            if !dir.join(code).is_file() {
                diagnostics.push(referenced_file_missing(
                    &project_path,
                    &project_text,
                    "code",
                    code.to_string_lossy().as_ref(),
                ));
            }
        }

        if let Some(icon) = &self.icon
            && !dir.join(icon).is_file()
        {
            diagnostics.push(referenced_file_missing(
                &project_path,
                &project_text,
                "icon",
                icon.to_string_lossy().as_ref(),
            ));
        }

        diagnostics
    }
}

/// Builds a "referenced file missing" diagnostic, located at the key that
/// references it.
fn referenced_file_missing(file: &Path, text: &str, key: &str, value: &str) -> Diagnostic {
    let message = format!("referenced {key} file `{value}` does not exist");
    match find_key_line(text, key, Some(value)) {
        Some(line) => Diagnostic::at(DiagnosticKind::MissingFile, file, line, message),
        None => Diagnostic::new(DiagnosticKind::MissingFile, file, message),
    }
}

/// Builds a diagnostic at the line of a top-level key, if locatable.
fn located(
    kind: DiagnosticKind,
    file: &Path,
    text: &str,
    key: &str,
    message: String,
) -> Diagnostic {
    match find_key_line(text, key, None) {
        Some(line) => Diagnostic::at(kind, file, line, message),
        None => Diagnostic::new(kind, file, message),
    }
}

/// Finds the one-based line of a `key = value` entry.
fn find_key_line(text: &str, key: &str, value: Option<&str>) -> Option<usize> {
    text.lines().enumerate().find_map(|(index, line)| {
        let trimmed = line.trim_start();
        let mut parts = trimmed.splitn(2, '=');
        let found_key = parts.next().map(str::trim)?;
        if found_key != key {
            return None;
        }
        match value {
            Some(value) if !trimmed.contains(value) => None,
            _ => Some(index + 1),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_lines_are_found() {
        let text = "name = \"A\"\nstartup = \"ghost\"\n";
        assert_eq!(find_key_line(text, "startup", None), Some(2));
        assert_eq!(find_key_line(text, "startup", Some("ghost")), Some(2));
        assert_eq!(find_key_line(text, "startup", Some("other")), None);
        assert_eq!(find_key_line(text, "missing", None), None);
    }
}
