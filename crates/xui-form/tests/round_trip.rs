//! Load/save round trips and load errors.

use xui_form::{Catalog, FormDoc, Value};

/// A canonical form: no default values, keys in the writer's order.
const SAMPLE: &str = "\
format = 1

[window]
name = \"frmMain\"
height = 240
resizable = false
title = \"Hello\"
width = 400

[[node]]
kind = \"Panel\"
name = \"panMain\"
top = 8
width = 380
height = 200

[[node]]
kind = \"Button\"
name = \"cmdGo\"
parent = \"panMain\"
left = 20
top = 20
width = 120
height = 30
text = \"Go\"

[[node]]
kind = \"ComboBox\"
name = \"cboPick\"
parent = \"panMain\"
left = 20
top = 60
width = 180
height = 26
anchor = \"top_right\"
items = [\"one\", \"two\"]
selected = 1
";

#[test]
fn a_canonical_form_round_trips_byte_for_byte() {
    let catalog = Catalog::xui();
    let doc = FormDoc::from_toml(SAMPLE, &catalog).expect("the sample loads");
    assert_eq!(doc.window.name, "frmMain");
    assert_eq!(
        doc.window.prop("title"),
        Some(&Value::Text("Hello".to_owned()))
    );
    assert_eq!(doc.nodes.len(), 3);
    assert_eq!(
        doc.node("cboPick").and_then(|node| node.prop("items")),
        Some(&Value::List(vec!["one".to_owned(), "two".to_owned()]))
    );
    assert_eq!(
        doc.node("cmdGo").and_then(|node| node.parent.as_deref()),
        Some("panMain")
    );
    assert_eq!(doc.to_toml(&catalog), SAMPLE);
}

#[test]
fn a_value_equal_to_its_default_is_dropped_on_save() {
    let catalog = Catalog::xui();
    let mut doc = FormDoc::new("frmMain");
    let mut button = xui_form::Node::new("Button", "cmdGo");
    // `width` defaults to the Button's default size, `visible` to true.
    button.set_prop("width", Value::Int(100));
    button.set_prop("visible", Value::Bool(true));
    button.set_prop("text", Value::Text("Go".to_owned()));
    doc.insert(button);

    let text = doc.to_toml(&catalog);
    assert!(
        !text.contains("visible"),
        "a default is not written: {text}"
    );
    assert!(
        !text.contains("width"),
        "a default size is not written: {text}"
    );
    assert!(text.contains("text = \"Go\""));

    // A hand-written default is likewise dropped, and that is documented.
    let explicit = "\
format = 1

[window]
name = \"frmMain\"

[[node]]
kind = \"Button\"
name = \"cmdGo\"
visible = true
text = \"Go\"
";
    let loaded = FormDoc::from_toml(explicit, &catalog).expect("loads");
    assert!(!loaded.to_toml(&catalog).contains("visible"));
}

#[test]
fn a_newer_format_is_rejected() {
    let catalog = Catalog::xui();
    let error = FormDoc::from_toml("format = 2\n\n[window]\nname = \"x\"\n", &catalog)
        .expect_err("format 2 is too new");
    assert!(error.to_string().contains("unsupported form format"));
}

#[test]
fn an_unknown_kind_is_reported_with_its_line() {
    let catalog = Catalog::xui();
    let text = "\
format = 1

[window]
name = \"frmMain\"

[[node]]
kind = \"Nope\"
name = \"bad\"
";
    let error = FormDoc::from_toml(text, &catalog).expect_err("unknown kind");
    assert_eq!(error.line(), Some(7));
    assert!(error.message().contains("Nope"));
}

#[test]
fn an_unknown_property_is_reported() {
    let catalog = Catalog::xui();
    let text = "\
format = 1

[window]
name = \"frmMain\"

[[node]]
kind = \"Button\"
name = \"cmdGo\"
nonsense = 1
";
    let error = FormDoc::from_toml(text, &catalog).expect_err("unknown property");
    assert_eq!(error.line(), Some(9));
    assert!(error.message().contains("nonsense"));
}

#[test]
fn a_mistyped_value_is_reported() {
    let catalog = Catalog::xui();
    let text = "\
format = 1

[window]
name = \"frmMain\"

[[node]]
kind = \"Button\"
name = \"cmdGo\"
width = \"wide\"
";
    let error = FormDoc::from_toml(text, &catalog).expect_err("mistyped value");
    assert_eq!(error.line(), Some(9));
    assert!(error.message().contains("invalid"));
}

#[test]
fn a_syntax_error_carries_its_line() {
    let catalog = Catalog::xui();
    let error =
        FormDoc::from_toml("format = 1\n\n[window\nname = ", &catalog).expect_err("syntax error");
    assert!(error.line().is_some());
}
