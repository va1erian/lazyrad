#![forbid(unsafe_code)]

//! The form document: a window, a flat list of nodes, and the TOML codec.
//!
//! A `.lfm`-style file is `format = 1`, a `[window]` table and one `[[node]]`
//! table per widget. Children name their container through `parent = "…"`, so
//! the list stays flat and diffs cleanly. Only non-default properties are
//! written, in a fixed key order, so a load/save cycle is byte-identical for a
//! canonical file; a value written explicitly equal to its default is dropped on
//! save.
//!
//! The document is plain data. The designer builds on [`FormDoc::node`],
//! [`FormDoc::insert`], [`FormDoc::remove`], [`FormDoc::rename`] and
//! [`FormDoc::reparent`]; the runtime feeds it to [`crate::build`].

use std::collections::BTreeMap;
use std::fmt;

use crate::FORMAT_VERSION;
use crate::schema::{Access, Catalog};
use crate::value::Value;

/// A form: the format version, the window and the nodes in creation order.
#[derive(Clone, Debug, PartialEq)]
pub struct FormDoc {
    /// The document format; loading a newer one is an error.
    pub format: u32,
    /// The window (`name` plus the window properties).
    pub window: WindowNode,
    /// The nodes, flat and in creation (z) order.
    pub nodes: Vec<Node>,
}

/// The window's name and properties; the window is not a widget node.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowNode {
    /// The form/window name.
    pub name: String,
    /// The window properties, only non-defaults after a canonical load.
    pub props: BTreeMap<String, Value>,
}

impl WindowNode {
    /// A window named `name` with no properties set.
    pub fn new(name: impl Into<String>) -> Self {
        WindowNode {
            name: name.into(),
            props: BTreeMap::new(),
        }
    }

    /// The property named `name`, if set.
    pub fn prop(&self, name: &str) -> Option<&Value> {
        self.props.get(name)
    }

    /// Sets a property, returning the previous value.
    pub fn set_prop(&mut self, name: impl Into<String>, value: Value) -> Option<Value> {
        self.props.insert(name.into(), value)
    }
}

/// One widget node in a form.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// The widget kind (a catalog kind or an alias).
    pub kind: String,
    /// The node name; unique within the form.
    pub name: String,
    /// The parent container's name, or `None` for the window.
    pub parent: Option<String>,
    /// The common and widget properties.
    pub props: BTreeMap<String, Value>,
}

impl Node {
    /// A node of `kind` named `name`, parented to the window.
    pub fn new(kind: impl Into<String>, name: impl Into<String>) -> Self {
        Node {
            kind: kind.into(),
            name: name.into(),
            parent: None,
            props: BTreeMap::new(),
        }
    }

    /// The property named `name`, if set.
    pub fn prop(&self, name: &str) -> Option<&Value> {
        self.props.get(name)
    }

    /// Sets a property, returning the previous value.
    pub fn set_prop(&mut self, name: impl Into<String>, value: Value) -> Option<Value> {
        self.props.insert(name.into(), value)
    }
}

impl FormDoc {
    /// An empty form named `name` with a 320x200 window.
    pub fn new(name: impl Into<String>) -> Self {
        let mut window = WindowNode::new(name);
        window.set_prop("width", Value::Int(320));
        window.set_prop("height", Value::Int(200));
        FormDoc {
            format: FORMAT_VERSION,
            window,
            nodes: Vec::new(),
        }
    }

    /// Parses a form from TOML, decoding each property against `catalog`.
    pub fn from_toml(text: &str, catalog: &Catalog) -> Result<FormDoc, LoadError> {
        let root: toml::Table = toml::from_str(text).map_err(|error| syntax_error(text, &error))?;

        let format = match root.get("format").and_then(toml::Value::as_integer) {
            Some(format) => {
                u32::try_from(format).map_err(|_| LoadError::plain("`format` must be positive"))?
            }
            None => {
                return Err(LoadError::at(
                    "missing or non-integer `format`",
                    find_key_line(text, Section::Root, "format"),
                ));
            }
        };
        if format != FORMAT_VERSION {
            return Err(LoadError::plain(format!(
                "unsupported form format {format}; this build understands format {FORMAT_VERSION}"
            )));
        }

        let window_table = root
            .get("window")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| LoadError::plain("missing `[window]` table"))?;
        let window_name = window_table
            .get("name")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| LoadError::plain("`[window]` must have a string `name`"))?
            .to_owned();
        let window = WindowNode {
            name: window_name,
            props: decode_window_props(window_table, catalog, text)?,
        };

        let mut nodes = Vec::new();
        if let Some(raw_nodes) = root.get("node") {
            let raw_nodes = raw_nodes
                .as_array()
                .ok_or_else(|| LoadError::plain("`node` must be an array of tables"))?;
            for (index, raw) in raw_nodes.iter().enumerate() {
                let table = raw
                    .as_table()
                    .ok_or_else(|| LoadError::plain("each `node` entry must be a table"))?;
                nodes.push(decode_node(table, catalog, text, index)?);
            }
        }

        Ok(FormDoc {
            format,
            window,
            nodes,
        })
    }

    /// Serialises the form to canonical TOML.
    pub fn to_toml(&self, catalog: &Catalog) -> String {
        let mut out = String::new();
        out.push_str(&format!("format = {}\n\n", self.format));

        out.push_str("[window]\n");
        out.push_str(&format!("name = {}\n", toml_string(&self.window.name)));
        write_props(&mut out, &self.window.props, |name| {
            catalog
                .window_spec()
                .property(name)
                .map(|spec| spec.default.clone())
        });

        for node in &self.nodes {
            out.push_str("\n[[node]]\n");
            out.push_str(&format!("kind = {}\n", toml_string(&node.kind)));
            out.push_str(&format!("name = {}\n", toml_string(&node.name)));
            if let Some(parent) = &node.parent {
                out.push_str(&format!("parent = {}\n", toml_string(parent)));
            }
            write_common_props(&mut out, node, catalog);
            write_widget_props(&mut out, node, catalog);
        }
        out
    }

    /// The node named `name`, if any.
    pub fn node(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.name == name)
    }

    /// The mutable node named `name`, if any.
    pub fn node_mut(&mut self, name: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|node| node.name == name)
    }

    /// The nodes whose parent is `parent`.
    pub fn children_of(&self, parent: &str) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|node| node.parent.as_deref() == Some(parent))
            .collect()
    }

    /// The nodes parented to the window.
    pub fn roots(&self) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|node| node.parent.is_none())
            .collect()
    }

    /// Appends `node` to the form.
    ///
    /// The caller owns uniqueness and parenting; [`FormDoc::validate`] reports a
    /// problem if they are wrong.
    pub fn insert(&mut self, node: Node) {
        self.nodes.push(node);
    }

    /// Removes the node named `name` and every descendant, returning whether a
    /// node was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let mut doomed = vec![name.to_owned()];
        let mut index = 0;
        while index < doomed.len() {
            let parent = doomed[index].clone();
            index += 1;
            for node in &self.nodes {
                if node.parent.as_deref() == Some(parent.as_str()) && !doomed.contains(&node.name) {
                    doomed.push(node.name.clone());
                }
            }
        }
        let before = self.nodes.len();
        self.nodes
            .retain(|node| !doomed.iter().any(|doomed| doomed == &node.name));
        self.nodes.len() != before
    }

    /// Renames the node named `old` to `new`, updating its children's `parent`
    /// references, and returns whether a node was renamed.
    pub fn rename(&mut self, old: &str, new: &str) -> bool {
        if self.node(new).is_some() {
            return false;
        }
        let mut renamed = false;
        for node in &mut self.nodes {
            if node.name == old {
                node.name = new.to_owned();
                renamed = true;
            }
        }
        if renamed {
            for node in &mut self.nodes {
                if node.parent.as_deref() == Some(old) {
                    node.parent = Some(new.to_owned());
                }
            }
        }
        renamed
    }

    /// Reparents the node named `name` to `parent` (`None` is the window),
    /// returning whether the node existed.
    ///
    /// Cycles are not prevented here; [`FormDoc::validate`] reports them.
    pub fn reparent(&mut self, name: &str, parent: Option<String>) -> bool {
        match self.node_mut(name) {
            Some(node) => {
                node.parent = parent;
                true
            }
            None => false,
        }
    }
}

/// A failure to load a form document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadError {
    /// A human-readable description.
    message: String,
    /// The one-based TOML line, when it could be located.
    line: Option<usize>,
}

impl LoadError {
    /// An error with no line.
    fn plain(message: impl Into<String>) -> Self {
        LoadError {
            message: message.into(),
            line: None,
        }
    }

    /// An error at a known line.
    fn at(message: impl Into<String>, line: Option<usize>) -> Self {
        LoadError {
            message: message.into(),
            line,
        }
    }

    /// The description.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The one-based TOML line, when located.
    pub fn line(&self) -> Option<usize> {
        self.line
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(formatter, "line {line}: {}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

impl std::error::Error for LoadError {}

/// Decodes the window properties (everything but `name`).
fn decode_window_props(
    table: &toml::Table,
    catalog: &Catalog,
    text: &str,
) -> Result<BTreeMap<String, Value>, LoadError> {
    let mut props = BTreeMap::new();
    for (key, raw) in table {
        if key == "name" {
            continue;
        }
        let spec = catalog
            .window_spec()
            .property(key)
            .ok_or_else(|| unknown_property("Window", key, None, text, Section::Window))?;
        props.insert(
            key.clone(),
            decode(raw, spec.ty.clone(), key, None, text, Section::Window)?,
        );
    }
    Ok(props)
}

/// Decodes one `[[node]]` table.
fn decode_node(
    table: &toml::Table,
    catalog: &Catalog,
    text: &str,
    index: usize,
) -> Result<Node, LoadError> {
    let section = Section::Node(index);
    let kind = table
        .get("kind")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| LoadError::plain("each node must have a string `kind`"))?
        .to_owned();
    let name = table
        .get("name")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| LoadError::plain("each node must have a string `name`"))?
        .to_owned();
    let parent = table
        .get("parent")
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| LoadError::plain("`parent` must be a string"))
        })
        .transpose()?;

    let _spec = catalog.get(&kind).ok_or_else(|| {
        LoadError::at(
            format!("`{kind}` is not a known widget kind"),
            find_key_line(text, section, "kind"),
        )
    })?;

    let mut props = BTreeMap::new();
    for (key, raw) in table {
        if matches!(key.as_str(), "kind" | "name" | "parent") {
            continue;
        }
        let spec = catalog
            .property(&kind, key)
            .ok_or_else(|| unknown_property(&kind, key, Some(&name), text, section))?;
        props.insert(
            key.clone(),
            decode(raw, spec.ty.clone(), key, Some(&name), text, section)?,
        );
    }

    Ok(Node {
        kind,
        name,
        parent,
        props,
    })
}

/// Decodes one raw literal against a type, mapping the error to a located
/// [`LoadError`].
fn decode(
    raw: &toml::Value,
    ty: crate::value::ValueType,
    key: &str,
    node: Option<&str>,
    text: &str,
    section: Section,
) -> Result<Value, LoadError> {
    Value::from_toml(raw, &ty).map_err(|source| {
        let subject = match node {
            Some(node) => format!("`{key}` on `{node}`"),
            None => format!("`{key}`"),
        };
        LoadError::at(
            format!("invalid {subject}: {source}"),
            find_key_line(text, section, key),
        )
    })
}

/// Builds an "unknown property" error.
fn unknown_property(
    kind: &str,
    key: &str,
    node: Option<&str>,
    text: &str,
    section: Section,
) -> LoadError {
    let where_ = match node {
        Some(node) => format!("`{key}` is not a property of {kind} (node `{node}`)"),
        None => format!("`{key}` is not a property of {kind}"),
    };
    LoadError::at(where_, find_key_line(text, section, key))
}

/// Turns a `toml` parse failure into a located [`LoadError`].
fn syntax_error(text: &str, error: &toml::de::Error) -> LoadError {
    let line = error
        .span()
        .map(|span| line_for_offset(text, span.start))
        .unwrap_or(1);
    LoadError::at(error.message().to_owned(), Some(line))
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

/// Finds the one-based line of a top-level `key = value` entry, if present.
/// The part of a form file a key is looked up in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    /// The keys before any table header (`format`).
    Root,
    /// The `[window]` table.
    Window,
    /// The `index`-th `[[node]]` table.
    Node(usize),
}

/// The one-based line of `key` inside `section`, so an error in the third
/// node points at that node rather than at the first `key` in the file.
fn find_key_line(text: &str, section: Section, key: &str) -> Option<usize> {
    let mut current = Some(Section::Root);
    let mut nodes_seen = 0;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            current = match trimmed {
                "[[node]]" => {
                    nodes_seen += 1;
                    Some(Section::Node(nodes_seen - 1))
                }
                "[window]" => Some(Section::Window),
                _ => None,
            };
            continue;
        }
        if current != Some(section) {
            continue;
        }
        let found = trimmed.split('=').next().map(str::trim);
        if found == Some(key) {
            return Some(index + 1);
        }
    }
    None
}

/// Writes the window's non-default properties, in map order.
fn write_props(
    out: &mut String,
    props: &BTreeMap<String, Value>,
    default: impl Fn(&str) -> Option<Value>,
) {
    for (name, value) in props {
        if default(name).as_ref() == Some(value) {
            continue;
        }
        out.push_str(&format!("{name} = {}\n", value.to_toml()));
    }
}

/// Writes a node's non-default common properties, in the documented order.
fn write_common_props(out: &mut String, node: &Node, catalog: &Catalog) {
    for spec in catalog.common_properties() {
        let Some(value) = node.props.get(&spec.name) else {
            continue;
        };
        let default = catalog
            .property(&node.kind, &spec.name)
            .map(|property| property.default);
        if default.as_ref() == Some(value) {
            continue;
        }
        out.push_str(&format!("{} = {}\n", spec.name, value.to_toml()));
    }
}

/// Writes a node's remaining non-default properties, alphabetically.
fn write_widget_props(out: &mut String, node: &Node, catalog: &Catalog) {
    let common: Vec<&str> = catalog
        .common_properties()
        .iter()
        .map(|spec| spec.name.as_str())
        .collect();
    for (name, value) in &node.props {
        if common.contains(&name.as_str()) {
            continue;
        }
        let default = catalog
            .property(&node.kind, name)
            .map(|property| property.default);
        if default.as_ref() == Some(value) {
            continue;
        }
        out.push_str(&format!("{name} = {}\n", value.to_toml()));
    }
}

/// A TOML basic-string literal for `value`.
fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_owned()).to_string()
}

/// The access rule for `property` on `kind`, if known (used by tests and the
/// designer).
pub fn property_access(catalog: &Catalog, kind: &str, property: &str) -> Option<Access> {
    if kind == "Window" {
        return catalog
            .window_spec()
            .property(property)
            .map(|spec| spec.access);
    }
    catalog.property(kind, property).map(|spec| spec.access)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_and_cycle_edits_are_plain_data() {
        let mut doc = FormDoc::new("main_form");
        let mut a = Node::new("Panel", "panA");
        a.set_prop("width", Value::Int(200));
        a.set_prop("height", Value::Int(120));
        let mut b = Node::new("Panel", "panB");
        b.parent = Some("panA".to_owned());
        doc.insert(a);
        doc.insert(b);

        assert_eq!(doc.children_of("panA").len(), 1);
        assert!(doc.rename("panA", "panRoot"));
        assert_eq!(
            doc.node("panB").and_then(|n| n.parent.as_deref()),
            Some("panRoot")
        );
        assert!(!doc.rename("panB", "panRoot"), "the new name is taken");
        assert!(doc.reparent("panB", None));
        assert!(doc.node("panB").is_some_and(|n| n.parent.is_none()));
        assert_eq!(doc.roots().len(), 2);
    }

    #[test]
    fn remove_cascades_to_descendants() {
        let mut doc = FormDoc::new("main_form");
        let mut panel = Node::new("Panel", "panA");
        panel.set_prop("width", Value::Int(10));
        doc.insert(panel);
        let mut child = Node::new("Button", "cmdGo");
        child.parent = Some("panA".to_owned());
        child.set_prop("width", Value::Int(10));
        doc.insert(child);
        let mut grandchild = Node::new("Button", "cmdDeep");
        grandchild.parent = Some("cmdGo".to_owned());
        doc.insert(grandchild);

        assert!(doc.remove("panA"));
        assert!(doc.nodes.is_empty());
        assert!(!doc.remove("panA"));
    }
}
