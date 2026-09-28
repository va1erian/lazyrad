//! Validation through the public API.

use xui_form::{Catalog, FormDoc, Node, Severity, Value};

fn doc_with(node: Node) -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    doc.insert(node);
    doc
}

#[test]
fn a_valid_form_has_no_diagnostics() {
    let catalog = Catalog::xui();
    let mut doc = FormDoc::new("main_form");
    let mut button = Node::new("Button", "cmdGo");
    button.set_prop("text", Value::Text("Go".to_owned()));
    button.set_prop("width", Value::Int(100));
    button.set_prop("height", Value::Int(30));
    doc.insert(button);
    assert!(doc.validate(&catalog).is_empty());
}

#[test]
fn an_invalid_name_is_an_error() {
    let diagnostics = doc_with(Node::new("Button", "1bad")).validate(&Catalog::xui());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.is_error() && d.property.as_deref() == Some("name"))
    );
}

#[test]
fn duplicate_names_are_an_error() {
    let mut doc = FormDoc::new("main_form");
    doc.insert(Node::new("Button", "cmdGo"));
    doc.insert(Node::new("Button", "cmdGo"));
    let diagnostics = doc.validate(&Catalog::xui());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("more than once"))
    );
}

#[test]
fn an_unknown_kind_is_an_error() {
    let diagnostics = doc_with(Node::new("Nope", "bad")).validate(&Catalog::xui());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("unknown widget kind"))
    );
}

#[test]
fn an_unknown_property_is_an_error() {
    let mut node = Node::new("Button", "cmdGo");
    node.set_prop("nonsense", Value::Bool(true));
    let diagnostics = doc_with(node).validate(&Catalog::xui());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.property.as_deref() == Some("nonsense"))
    );
}

#[test]
fn a_mistyped_value_is_an_error() {
    let mut node = Node::new("Button", "cmdGo");
    node.set_prop("enabled", Value::Text("yes".to_owned()));
    let diagnostics = doc_with(node).validate(&Catalog::xui());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.property.as_deref() == Some("enabled"))
    );
}

#[test]
fn an_out_of_range_value_is_an_error() {
    let mut node = Node::new("Button", "cmdGo");
    node.set_prop("tab_index", Value::Int(-1));
    let diagnostics = doc_with(node).validate(&Catalog::xui());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.property.as_deref() == Some("tab_index"))
    );
}

#[test]
fn an_unknown_enum_variant_is_an_error() {
    let mut node = Node::new("Button", "cmdGo");
    node.set_prop("anchor", Value::Enum("sideways".to_owned()));
    let diagnostics = doc_with(node).validate(&Catalog::xui());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.property.as_deref() == Some("anchor"))
    );
}

#[test]
fn a_missing_parent_is_an_error() {
    let mut node = Node::new("Button", "cmdGo");
    node.parent = Some("ghost".to_owned());
    let diagnostics = doc_with(node).validate(&Catalog::xui());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("does not exist"))
    );
}

#[test]
fn a_cycle_is_an_error() {
    let mut doc = FormDoc::new("main_form");
    let mut a = Node::new("Panel", "panA");
    a.parent = Some("panB".to_owned());
    let mut b = Node::new("Panel", "panB");
    b.parent = Some("panA".to_owned());
    doc.insert(a);
    doc.insert(b);
    let diagnostics = doc.validate(&Catalog::xui());
    assert!(diagnostics.iter().any(|d| d.message.contains("cycle")));
}

#[test]
fn a_duplicate_tab_index_is_a_warning() {
    let mut doc = FormDoc::new("main_form");
    let mut one = Node::new("Button", "cmdOne");
    one.set_prop("tab_index", Value::Int(0));
    let mut two = Node::new("Button", "cmdTwo");
    two.set_prop("tab_index", Value::Int(0));
    doc.insert(one);
    doc.insert(two);
    let diagnostics = doc.validate(&Catalog::xui());
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].severity, Severity::Warning);
}
