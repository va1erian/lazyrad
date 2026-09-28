#![forbid(unsafe_code)]

//! Validation of a project and its forms.
//!
//! Validation answers five questions (issue #2):
//!
//! 1. every referenced file exists,
//! 2. the startup item exists,
//! 3. every control type is in the schema registry,
//! 4. every property is declared for its control's type and holds a value of
//!    the declared type (and, for enums, one of the allowed values),
//! 5. control names are unique identifiers within their form.
//!
//! Each problem becomes a [`Diagnostic`] naming the file and, when the text can
//! be located, the line. Syntax errors get their line from the TOML parser;
//! semantic errors are located by scanning the raw form text for the offending
//! control block and key.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::error::{Diagnostic, DiagnosticKind, Error};
use crate::model::{Form, Project};
use crate::schema::{PropertyType, SchemaRegistry};

impl Project {
    /// Validates the whole project directory against `registry`.
    ///
    /// Never fails: unreadable or malformed files become diagnostics so the
    /// caller can show every problem at once.
    pub fn validate(&self, dir: &Path, registry: &SchemaRegistry) -> Vec<Diagnostic> {
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
                    match fs::read_to_string(&path) {
                        Ok(text) => match Form::from_toml_with_schema_at(&text, &path, registry) {
                            Ok(form) => diagnostics.extend(validate_form(
                                &form,
                                registry,
                                &path,
                                Some(&text),
                            )),
                            Err(Error::Diagnostic(diagnostic)) => diagnostics.push(diagnostic),
                            Err(error) => diagnostics.push(Diagnostic::new(
                                DiagnosticKind::Syntax,
                                &path,
                                error.to_string(),
                            )),
                        },
                        Err(source) => diagnostics.push(Diagnostic::new(
                            DiagnosticKind::MissingFile,
                            &path,
                            format!("cannot read layout: {source}"),
                        )),
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

        diagnostics
    }
}

impl Form {
    /// Validates this form against `registry`, without line numbers.
    ///
    /// [`Project::validate`] is the located variant; this is for a form built
    /// in memory, for example by the designer.
    pub fn validate(&self, registry: &SchemaRegistry) -> Vec<Diagnostic> {
        validate_form(self, registry, Path::new("<form>"), None)
    }
}

/// Validates a form, using `text` to attach line numbers when it is available.
fn validate_form(
    form: &Form,
    registry: &SchemaRegistry,
    file: &Path,
    text: Option<&str>,
) -> Vec<Diagnostic> {
    let lines = text.map(FormLines::new);
    let mut diagnostics = Vec::new();
    let mut seen: BTreeMap<&str, ()> = BTreeMap::new();

    for (index, control) in form.controls.iter().enumerate() {
        if !is_identifier(&control.name) {
            diagnostics.push(control_located(
                DiagnosticKind::InvalidControlName,
                file,
                lines.as_ref(),
                index,
                "name",
                format!("`{}` is not a valid control name", control.name),
            ));
        }

        if seen.insert(control.name.as_str(), ()).is_some() {
            diagnostics.push(control_located(
                DiagnosticKind::DuplicateControlName,
                file,
                lines.as_ref(),
                index,
                "name",
                format!("control name `{}` is used more than once", control.name),
            ));
        }

        let Some(schema) = registry.get(&control.type_name) else {
            diagnostics.push(control_located(
                DiagnosticKind::UnknownControlType,
                file,
                lines.as_ref(),
                index,
                "type",
                format!("unknown control type `{}`", control.type_name),
            ));
            continue;
        };

        for (name, value) in &control.props {
            let Some(property) = schema.property(name) else {
                diagnostics.push(control_located(
                    DiagnosticKind::UnknownProperty,
                    file,
                    lines.as_ref(),
                    index,
                    name,
                    format!("`{}` is not a property of {}", name, control.type_name),
                ));
                continue;
            };

            if !property.accepts(value) {
                diagnostics.push(control_located(
                    DiagnosticKind::InvalidPropertyType,
                    file,
                    lines.as_ref(),
                    index,
                    name,
                    format!(
                        "`{name}` expects {}, found {}",
                        property.type_name(),
                        value.type_name()
                    ),
                ));
                continue;
            }

            if property.ty == PropertyType::Enum {
                let actual = value.as_str().unwrap_or_default();
                if !property.enum_values.iter().any(|allowed| allowed == actual) {
                    diagnostics.push(control_located(
                        DiagnosticKind::InvalidEnumValue,
                        file,
                        lines.as_ref(),
                        index,
                        name,
                        format!(
                            "`{name}` must be one of [{}], found `{actual}`",
                            property.enum_values.join(", ")
                        ),
                    ));
                }
            }
        }
    }

    diagnostics
}

/// Whether `name` is a valid identifier: a letter or `_`, then the same plus
/// digits.
fn is_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Builds a diagnostic at the line of a control's `key`, if locatable.
fn control_located(
    kind: DiagnosticKind,
    file: &Path,
    lines: Option<&FormLines>,
    index: usize,
    key: &str,
    message: String,
) -> Diagnostic {
    match lines.and_then(|lines| lines.key_line(index, key)) {
        Some(line) => Diagnostic::at(kind, file, line, message),
        None => Diagnostic::new(kind, file, message),
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

/// Builds a "referenced file missing" diagnostic, located at the key that
/// references it.
fn referenced_file_missing(file: &Path, text: &str, key: &str, value: &str) -> Diagnostic {
    let message = format!("referenced {key} file `{value}` does not exist");
    match find_key_line(text, key, Some(value)) {
        Some(line) => Diagnostic::at(DiagnosticKind::MissingFile, file, line, message),
        None => Diagnostic::new(DiagnosticKind::MissingFile, file, message),
    }
}

/// Finds the one-based line of a `key = value` entry.
///
/// With `value` set, the line must also contain that value; this disambiguates
/// several entries sharing a key.
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

/// Line numbers for each `[[control]]` block, used to locate a key.
struct FormLines {
    lines: Vec<String>,
    block_starts: Vec<usize>,
}

impl FormLines {
    fn new(text: &str) -> Self {
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let block_starts = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.trim_start().starts_with("[[control]]"))
            .map(|(index, _)| index)
            .collect();
        Self {
            lines,
            block_starts,
        }
    }

    /// The one-based line of `key` inside the control block at `index`.
    fn key_line(&self, index: usize, key: &str) -> Option<usize> {
        let start = *self.block_starts.get(index)?;
        let end = self
            .block_starts
            .get(index + 1)
            .copied()
            .unwrap_or(self.lines.len());
        self.lines[start..end]
            .iter()
            .enumerate()
            .find_map(|(offset, line)| {
                let trimmed = line.trim_start();
                let mut parts = trimmed.splitn(2, '=');
                let found_key = parts.next().map(str::trim)?;
                (found_key == key).then_some(start + offset + 1)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Control;

    fn control(type_name: &str, name: &str) -> Control {
        Control::new(type_name, name)
    }

    #[test]
    fn identifiers_are_checked() {
        assert!(is_identifier("cmdHello"));
        assert!(is_identifier("_x1"));
        assert!(!is_identifier(""));
        assert!(!is_identifier("1bad"));
        assert!(!is_identifier("has space"));
        assert!(!is_identifier("dot.name"));
    }

    #[test]
    fn duplicate_and_unknown_types_are_reported() {
        let mut form = Form::new("frmMain");
        form.controls.push(control("Label", "lblOne"));
        form.controls.push(control("Label", "lblOne"));
        form.controls.push(control("Nope", "bad"));

        let diagnostics = form.validate(SchemaRegistry::builtin());
        let kinds: Vec<DiagnosticKind> = diagnostics.iter().map(|d| d.kind).collect();
        assert!(kinds.contains(&DiagnosticKind::DuplicateControlName));
        assert!(kinds.contains(&DiagnosticKind::UnknownControlType));
    }

    #[test]
    fn unknown_and_mistyped_properties_are_reported() {
        let mut form = Form::new("frmMain");
        let mut button = control("CommandButton", "cmdGo");
        button.props.insert("caption".to_owned(), "Go".into());
        button.props.insert("nonsense".to_owned(), true.into());
        button.props.insert("enabled".to_owned(), "yes".into());
        form.controls.push(button);

        let diagnostics = form.validate(SchemaRegistry::builtin());
        let kinds: Vec<DiagnosticKind> = diagnostics.iter().map(|d| d.kind).collect();
        assert!(kinds.contains(&DiagnosticKind::UnknownProperty));
        assert!(kinds.contains(&DiagnosticKind::InvalidPropertyType));
    }

    #[test]
    fn enum_values_are_checked() {
        let mut form = Form::new("frmMain");
        let mut combo = control("ComboBox", "cboStyle");
        combo.props.insert("style".to_owned(), "diagonal".into());
        form.controls.push(combo);

        let diagnostics = form.validate(SchemaRegistry::builtin());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::InvalidEnumValue);
    }

    #[test]
    fn key_lines_are_found_for_semantic_errors() {
        let text = "\
[form]
name = \"frmMain\"

[[control]]
type = \"Label\"
name = \"lblOne\"

[[control]]
type = \"Label\"
name = \"lblOne\"
caption = \"dup\"
";
        let mut form = Form::new("frmMain");
        form.controls.push(control("Label", "lblOne"));
        let mut second = control("Label", "lblOne");
        second.props.insert("caption".to_owned(), "dup".into());
        form.controls.push(second);

        let diagnostics = validate_form(
            &form,
            SchemaRegistry::builtin(),
            Path::new("frmMain.lfm"),
            Some(text),
        );
        let duplicate = diagnostics
            .iter()
            .find(|d| d.kind == DiagnosticKind::DuplicateControlName)
            .expect("duplicate reported");
        assert_eq!(duplicate.line, Some(10));
    }
}
