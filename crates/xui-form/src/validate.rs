#![forbid(unsafe_code)]

//! Validation of a [`FormDoc`] against a [`Catalog`].
//!
//! Validation is the designer's and the runtime's safety net: it reports every
//! problem at once as a [`Diagnostic`] rather than stopping at the first. The
//! decoder already rejects unknown kinds, unknown properties and mistyped
//! values at load time; validation re-checks them so a document built in memory
//! (by the designer) is held to the same rules, and adds the structural checks
//! the decoder cannot make: name uniqueness, parenting, cycles and the tab
//! order.

use std::collections::BTreeMap;

use crate::doc::FormDoc;
use crate::schema::Catalog;

/// How serious a [`Diagnostic`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The form cannot be built correctly.
    Error,
    /// The form is usable but something is suspicious.
    Warning,
}

/// One problem found in a form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// How serious the problem is.
    pub severity: Severity,
    /// The node the problem is in, if any.
    pub node: Option<String>,
    /// The property the problem is about, if any.
    pub property: Option<String>,
    /// A human-readable description.
    pub message: String,
    /// The one-based source line, when it is known.
    pub line: Option<usize>,
}

impl Diagnostic {
    /// An error against `node`.
    fn error(node: Option<&str>, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Error,
            node: node.map(str::to_owned),
            property: None,
            message: message.into(),
            line: None,
        }
    }

    /// An error against `node` and `property`.
    fn property_error(node: Option<&str>, property: &str, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Error,
            node: node.map(str::to_owned),
            property: Some(property.to_owned()),
            message: message.into(),
            line: None,
        }
    }

    /// A warning against `node` and `property`.
    fn warning(node: Option<&str>, property: &str, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            node: node.map(str::to_owned),
            property: Some(property.to_owned()),
            message: message.into(),
            line: None,
        }
    }

    /// Whether this is an error.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl FormDoc {
    /// Validates the form against `catalog`, returning every problem found.
    pub fn validate(&self, catalog: &Catalog) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        self.validate_window(catalog, &mut diagnostics);
        self.validate_names(&mut diagnostics);
        self.validate_properties(catalog, &mut diagnostics);
        self.validate_parenting(catalog, &mut diagnostics);
        self.validate_tab_order(&mut diagnostics);
        diagnostics
    }

    /// Checks the window name and its properties.
    fn validate_window(&self, catalog: &Catalog, diagnostics: &mut Vec<Diagnostic>) {
        if !is_identifier(&self.window.name) {
            diagnostics.push(Diagnostic {
                node: None,
                property: Some("name".to_owned()),
                ..Diagnostic::error(
                    None,
                    format!("`{}` is not a valid form name", self.window.name),
                )
            });
        }
        for (name, value) in &self.window.props {
            match catalog.window_spec().property(name) {
                Some(spec) if spec.accepts(value) => {}
                Some(spec) => diagnostics.push(Diagnostic::property_error(
                    None,
                    name,
                    format!(
                        "`{name}` expects {}, found {}",
                        spec.ty.type_name(),
                        value.type_name()
                    ),
                )),
                None => diagnostics.push(Diagnostic::property_error(
                    None,
                    name,
                    format!("`{name}` is not a window property"),
                )),
            }
        }
    }

    /// Checks that every node name is a unique identifier.
    fn validate_names(&self, diagnostics: &mut Vec<Diagnostic>) {
        let mut seen: BTreeMap<&str, ()> = BTreeMap::new();
        for node in &self.nodes {
            if !is_identifier(&node.name) {
                diagnostics.push(Diagnostic {
                    property: Some("name".to_owned()),
                    ..Diagnostic::error(
                        Some(&node.name),
                        format!("`{}` is not a valid node name", node.name),
                    )
                });
            }
            if seen.insert(node.name.as_str(), ()).is_some() {
                diagnostics.push(Diagnostic {
                    property: Some("name".to_owned()),
                    ..Diagnostic::error(
                        Some(&node.name),
                        format!("node name `{}` is used more than once", node.name),
                    )
                });
            }
        }
    }

    /// Checks that each kind is known and each property is known and well-typed.
    fn validate_properties(&self, catalog: &Catalog, diagnostics: &mut Vec<Diagnostic>) {
        for node in &self.nodes {
            let Some(spec) = catalog.get(&node.kind) else {
                diagnostics.push(Diagnostic::error(
                    Some(&node.name),
                    format!("unknown widget kind `{}`", node.kind),
                ));
                continue;
            };
            for (name, value) in &node.props {
                match catalog.property(&node.kind, name) {
                    Some(property) if property.accepts(value) => {}
                    Some(property) => diagnostics.push(Diagnostic::property_error(
                        Some(&node.name),
                        name,
                        format!(
                            "`{name}` expects {}, found {}",
                            property.ty.type_name(),
                            value.type_name()
                        ),
                    )),
                    None => diagnostics.push(Diagnostic::property_error(
                        Some(&node.name),
                        name,
                        format!("`{name}` is not a property of {}", spec.kind),
                    )),
                }
            }
        }
    }

    /// Checks parents exist, are containers that accept the child, and form no
    /// cycle.
    fn validate_parenting(&self, catalog: &Catalog, diagnostics: &mut Vec<Diagnostic>) {
        for node in &self.nodes {
            let Some(parent_name) = node.parent.as_deref() else {
                continue;
            };
            let Some(parent) = self.node(parent_name) else {
                diagnostics.push(Diagnostic::error(
                    Some(&node.name),
                    format!("parent `{parent_name}` does not exist"),
                ));
                continue;
            };
            let Some(parent_spec) = catalog.get(&parent.kind) else {
                continue;
            };
            if parent_spec.children.is_none() {
                diagnostics.push(Diagnostic::error(
                    Some(&node.name),
                    format!(
                        "{} `{parent_name}` cannot contain children",
                        parent_spec.kind
                    ),
                ));
                continue;
            }
            if catalog.contains(&node.kind) && !catalog.accepts_child(&parent.kind, &node.kind) {
                diagnostics.push(Diagnostic::error(
                    Some(&node.name),
                    format!(
                        "{} `{parent_name}` does not accept a {} child",
                        parent_spec.kind, node.kind
                    ),
                ));
            }
            if has_cycle(self, &node.name) {
                diagnostics.push(Diagnostic::error(
                    Some(&node.name),
                    format!("`{}` is part of a parent cycle", node.name),
                ));
            }
        }
    }

    /// Warns when two siblings share a `tab_index`.
    fn validate_tab_order(&self, diagnostics: &mut Vec<Diagnostic>) {
        let mut by_parent: BTreeMap<Option<&str>, BTreeMap<i64, &str>> = BTreeMap::new();
        for node in &self.nodes {
            let Some(value) = node.props.get("tab_index").and_then(|value| value.as_int()) else {
                continue;
            };
            let siblings = by_parent.entry(node.parent.as_deref()).or_default();
            if let Some(other) = siblings.insert(value, &node.name) {
                diagnostics.push(Diagnostic::warning(
                    Some(&node.name),
                    "tab_index",
                    format!("tab_index {value} is also used by `{other}`"),
                ));
            }
        }
    }
}

/// Whether following `name`'s parent chain returns to `name`.
fn has_cycle(doc: &FormDoc, name: &str) -> bool {
    let mut current = name;
    for _ in 0..=doc.nodes.len() {
        let Some(node) = doc.node(current) else {
            return false;
        };
        match node.parent.as_deref() {
            None => return false,
            Some(parent) if parent == name => return true,
            Some(parent) => current = parent,
        }
    }
    true
}

/// Whether `name` is a valid identifier: a letter or `_`, then the same plus
/// digits.
pub(crate) fn is_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;

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
    fn duplicate_names_and_unknown_kinds_are_reported() {
        let mut doc = FormDoc::new("frmMain");
        doc.insert(crate::doc::Node::new("Label", "lblOne"));
        doc.insert(crate::doc::Node::new("Label", "lblOne"));
        doc.insert(crate::doc::Node::new("Nope", "bad"));

        let diagnostics = doc.validate(&Catalog::xui());
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("more than once"))
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("unknown widget kind"))
        );
    }

    #[test]
    fn unknown_and_mistyped_properties_are_reported() {
        let mut doc = FormDoc::new("frmMain");
        let mut button = crate::doc::Node::new("Button", "cmdGo");
        button.set_prop("text", Value::Text("Go".to_owned()));
        button.set_prop("nonsense", Value::Bool(true));
        button.set_prop("enabled", Value::Text("yes".to_owned()));
        doc.insert(button);

        let diagnostics = doc.validate(&Catalog::xui());
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.property.as_deref() == Some("nonsense"))
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.property.as_deref() == Some("enabled"))
        );
    }

    #[test]
    fn enum_membership_and_ranges_are_reported() {
        let mut doc = FormDoc::new("frmMain");
        let mut button = crate::doc::Node::new("Button", "cmdGo");
        button.set_prop("anchor", Value::Enum("sideways".to_owned()));
        button.set_prop("tab_index", Value::Int(-1));
        doc.insert(button);

        let diagnostics = doc.validate(&Catalog::xui());
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.property.as_deref() == Some("anchor"))
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.property.as_deref() == Some("tab_index"))
        );
    }

    #[test]
    fn a_missing_parent_and_cycles_are_reported() {
        let mut doc = FormDoc::new("frmMain");
        let mut orphan = crate::doc::Node::new("Button", "cmdOrphan");
        orphan.parent = Some("ghost".to_owned());
        doc.insert(orphan);

        let mut a = crate::doc::Node::new("Panel", "panA");
        a.parent = Some("panB".to_owned());
        let mut b = crate::doc::Node::new("Panel", "panB");
        b.parent = Some("panA".to_owned());
        doc.insert(a);
        doc.insert(b);

        let diagnostics = doc.validate(&Catalog::xui());
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("does not exist"))
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("cycle"))
        );
    }

    #[test]
    fn parenting_checks_the_container_rule() {
        let mut doc = FormDoc::new("frmMain");
        let panel = crate::doc::Node::new("Panel", "panA");
        doc.insert(panel);
        let mut child = crate::doc::Node::new("Button", "cmdGo");
        child.parent = Some("panA".to_owned());
        doc.insert(child);
        assert!(doc.validate(&Catalog::xui()).is_empty());

        let mut on_button = crate::doc::Node::new("Label", "lblBad");
        on_button.parent = Some("cmdGo".to_owned());
        doc.insert(on_button);
        let diagnostics = doc.validate(&Catalog::xui());
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("cannot contain children"))
        );
    }

    #[test]
    fn duplicate_tab_index_is_a_warning() {
        let mut doc = FormDoc::new("frmMain");
        let mut one = crate::doc::Node::new("Button", "cmdOne");
        one.set_prop("tab_index", Value::Int(0));
        let mut two = crate::doc::Node::new("Button", "cmdTwo");
        two.set_prop("tab_index", Value::Int(0));
        doc.insert(one);
        doc.insert(two);

        let diagnostics = doc.validate(&Catalog::xui());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Severity::Warning);
    }
}
