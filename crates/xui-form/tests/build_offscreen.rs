//! Offscreen build tests: clicking, reading and writing properties, design
//! mode, aliases and anchoring.

mod common;

use xui_core::geometry::{Rect, Size};
use xui_form::{BuildOptions, Catalog, FormDoc, Node, Value, ValueType};

use common::{Msg, aliased_catalog, click_node, with_form};

/// A minimal form with one clickable button.
fn click_doc() -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    let mut button = Node::new("Button", "cmdGo");
    button.set_prop("left", Value::Int(10));
    button.set_prop("top", Value::Int(10));
    button.set_prop("width", Value::Int(100));
    button.set_prop("height", Value::Int(30));
    button.set_prop("text", Value::Text("Go".to_owned()));
    doc.insert(button);
    doc
}

#[test]
fn clicking_a_button_delivers_the_binders_message() {
    let messages = click_node(
        &click_doc(),
        &Catalog::xui(),
        BuildOptions::default(),
        "cmdGo",
    );
    assert_eq!(messages, vec![Msg::Click("cmdGo".to_owned())]);
}

#[test]
fn design_mode_wires_no_events() {
    let messages = click_node(
        &click_doc(),
        &Catalog::xui(),
        BuildOptions { design_mode: true },
        "cmdGo",
    );
    assert!(messages.is_empty(), "the designer must not run event code");
}

#[test]
fn a_node_kind_may_use_an_alias() {
    let mut doc = FormDoc::new("main_form");
    let mut button = Node::new("CommandButton", "cmdGo");
    button.set_prop("left", Value::Int(0));
    button.set_prop("top", Value::Int(0));
    button.set_prop("width", Value::Int(80));
    button.set_prop("height", Value::Int(30));
    button.set_prop("text", Value::Text("Go".to_owned()));
    doc.insert(button);

    let messages = click_node(&doc, &aliased_catalog(), BuildOptions::default(), "cmdGo");
    assert_eq!(messages, vec![Msg::Click("cmdGo".to_owned())]);
}

/// Builds a form containing one node of every built-in kind.
fn all_kinds_doc() -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    let mut push = |kind: &str, name: &str, props: &[(&str, Value)]| {
        let mut node = Node::new(kind, name);
        node.set_prop("width", Value::Int(120));
        node.set_prop("height", Value::Int(30));
        for (property, value) in props {
            node.set_prop(*property, value.clone());
        }
        doc.insert(node);
    };

    push(
        "Label",
        "lblOne",
        &[("text", Value::Text("one".to_owned()))],
    );
    push(
        "Button",
        "cmdOne",
        &[("text", Value::Text("go".to_owned()))],
    );
    push(
        "CheckBox",
        "chkOne",
        &[
            ("text", Value::Text("check".to_owned())),
            ("checked", Value::Bool(false)),
        ],
    );
    push(
        "ToggleButton",
        "tglOne",
        &[
            ("text", Value::Text("toggle".to_owned())),
            ("checked", Value::Bool(false)),
        ],
    );
    push(
        "RadioGroup",
        "radOne",
        &[
            ("items", Value::List(vec!["a".to_owned(), "b".to_owned()])),
            ("selected", Value::Int(0)),
        ],
    );
    push(
        "Edit",
        "txtOne",
        &[
            ("text", Value::Text("edit".to_owned())),
            ("cue", Value::Text("type".to_owned())),
        ],
    );
    push(
        "MultilineEdit",
        "mmoOne",
        &[("text", Value::Text("multi".to_owned()))],
    );
    push(
        "NumberField",
        "numOne",
        &[
            ("value", Value::Float(5.0)),
            ("min", Value::Float(0.0)),
            ("max", Value::Float(10.0)),
            ("step", Value::Float(1.0)),
        ],
    );
    push(
        "Slider",
        "sldOne",
        &[
            ("value", Value::Float(5.0)),
            ("min", Value::Float(0.0)),
            ("max", Value::Float(10.0)),
        ],
    );
    push(
        "ProgressBar",
        "prgOne",
        &[("value", Value::Int(5)), ("max", Value::Int(10))],
    );
    push(
        "ComboBox",
        "cboOne",
        &[
            ("items", Value::List(vec!["x".to_owned(), "y".to_owned()])),
            ("selected", Value::Int(1)),
        ],
    );
    push(
        "ListView",
        "lstOne",
        &[
            ("items", Value::List(vec!["r1".to_owned(), "r2".to_owned()])),
            ("selected", Value::Int(1)),
            ("multi_select", Value::Bool(true)),
        ],
    );
    push(
        "GroupBox",
        "fraOne",
        &[("text", Value::Text("frame".to_owned()))],
    );
    push("Panel", "panOne", &[]);
    push(
        "Separator",
        "sepOne",
        &[("orientation", Value::Enum("vertical".to_owned()))],
    );
    push(
        "Hyperlink",
        "lnkOne",
        &[("text", Value::Text("link".to_owned()))],
    );
    doc
}

#[test]
fn get_and_set_every_builtin_kind() {
    let doc = all_kinds_doc();
    let catalog = Catalog::xui();
    with_form(&doc, &catalog, BuildOptions::default(), |form| {
        assert_eq!(form.ids().len(), 16);

        // Text widgets round trip through their `text` property.
        for name in [
            "lblOne", "cmdOne", "chkOne", "tglOne", "txtOne", "mmoOne", "fraOne", "lnkOne",
        ] {
            form.set(name, "text", &Value::Text("changed".to_owned()))
                .expect("text is writable");
            assert_eq!(
                form.get(name, "text"),
                Some(Value::Text("changed".to_owned()))
            );
        }

        // Checked widgets.
        form.set("chkOne", "checked", &Value::Bool(true))
            .expect("checked is writable");
        assert_eq!(form.get("chkOne", "checked"), Some(Value::Bool(true)));
        form.set("tglOne", "checked", &Value::Bool(true))
            .expect("checked is writable");
        assert_eq!(form.get("tglOne", "checked"), Some(Value::Bool(true)));

        // Single-node integer selection.
        form.set("cboOne", "selected", &Value::Int(0))
            .expect("selected is writable");
        assert_eq!(form.get("cboOne", "selected"), Some(Value::Int(0)));

        // List selection, including clearing it.
        form.set("lstOne", "selected", &Value::Int(0))
            .expect("selected is writable");
        assert_eq!(form.get("lstOne", "selected"), Some(Value::Int(0)));
        form.set("lstOne", "selected", &Value::Int(-1))
            .expect("clearing is allowed");
        assert_eq!(form.get("lstOne", "selected"), Some(Value::Int(-1)));

        // Radio group selection.
        form.set("radOne", "selected", &Value::Int(1))
            .expect("selected is writable");
        assert_eq!(form.get("radOne", "selected"), Some(Value::Int(1)));

        // Numeric fields.
        form.set("numOne", "value", &Value::Float(3.0))
            .expect("value is writable");
        assert_eq!(form.get("numOne", "value"), Some(Value::Float(3.0)));
        form.set("numOne", "max", &Value::Float(20.0))
            .expect("max is writable");
        assert_eq!(form.get("numOne", "max"), Some(Value::Float(20.0)));
        form.set("sldOne", "value", &Value::Float(8.0))
            .expect("value is writable");
        assert_eq!(form.get("sldOne", "value"), Some(Value::Float(8.0)));

        // Progress bar.
        form.set("prgOne", "value", &Value::Int(7))
            .expect("value is writable");
        assert_eq!(form.get("prgOne", "value"), Some(Value::Int(7)));

        // Construction-only properties are read-only at runtime.
        assert!(form.set("radOne", "items", &Value::List(vec![])).is_err());
        assert!(
            form.set(
                "sepOne",
                "orientation",
                &Value::Enum("horizontal".to_owned())
            )
            .is_err()
        );
        assert!(
            form.set("txtOne", "cue", &Value::Text("x".to_owned()))
                .is_err()
        );

        // Common properties.
        form.set("cmdOne", "anchor", &Value::Enum("fill".to_owned()))
            .expect("anchor is writable");
        assert_eq!(
            form.get("cmdOne", "anchor"),
            Some(Value::Enum("fill".to_owned()))
        );
        form.set("cmdOne", "visible", &Value::Bool(false))
            .expect("visible is writable");
        assert_eq!(form.get("cmdOne", "visible"), Some(Value::Bool(false)));
        form.set("cmdOne", "tab_index", &Value::Int(3))
            .expect("tab_index is writable");
        assert_eq!(form.get("cmdOne", "tab_index"), Some(Value::Int(3)));

        // An unknown property is an error.
        assert!(form.set("cmdOne", "nope", &Value::Bool(true)).is_err());
    });
}

#[test]
fn construction_only_properties_are_readable_in_design_mode() {
    let doc = all_kinds_doc();
    let catalog = Catalog::xui();
    with_form(&doc, &catalog, BuildOptions { design_mode: true }, |form| {
        assert_eq!(
            form.get("sepOne", "orientation"),
            Some(Value::Enum("vertical".to_owned()))
        );
        assert_eq!(
            form.get("txtOne", "cue"),
            Some(Value::Text("type".to_owned()))
        );
        assert_eq!(
            form.get("radOne", "items"),
            Some(Value::List(vec!["a".to_owned(), "b".to_owned()]))
        );
    });
}

/// A form with a filling panel and a bottom-right button inside it.
fn anchor_doc() -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    let mut panel = Node::new("Panel", "panMain");
    panel.set_prop("left", Value::Int(0));
    panel.set_prop("top", Value::Int(0));
    panel.set_prop("width", Value::Int(320));
    panel.set_prop("height", Value::Int(200));
    panel.set_prop("anchor", Value::Enum("fill".to_owned()));
    doc.insert(panel);

    let mut button = Node::new("Button", "cmdGo");
    button.parent = Some("panMain".to_owned());
    button.set_prop("left", Value::Int(200));
    button.set_prop("top", Value::Int(150));
    button.set_prop("width", Value::Int(50));
    button.set_prop("height", Value::Int(40));
    button.set_prop("anchor", Value::Enum("bottom_right".to_owned()));
    doc.insert(button);
    doc
}

#[test]
fn relayout_fills_and_anchors_bottom_right() {
    let doc = anchor_doc();
    let catalog = Catalog::xui();
    let (panel, button) = with_form(&doc, &catalog, BuildOptions::default(), |form| {
        assert_eq!(form.bounds("panMain"), Some(Rect::new(0, 0, 320, 200)));
        assert_eq!(form.bounds("cmdGo"), Some(Rect::new(200, 150, 250, 190)));

        form.relayout(Size::new(500, 400));
        (form.bounds("panMain"), form.bounds("cmdGo"))
    });
    assert_eq!(panel, Some(Rect::new(0, 0, 500, 400)));
    assert_eq!(button, Some(Rect::new(380, 350, 430, 390)));
}

#[test]
fn a_form_reports_a_node_kind_and_property_type() {
    let doc = click_doc();
    with_form(&doc, &Catalog::xui(), BuildOptions::default(), |form| {
        assert_eq!(form.kind("cmdGo"), Some("Button"));
        assert_eq!(form.kind("ghost"), None);
        assert_eq!(
            form.property_type("cmdGo", "text"),
            Some(ValueType::Text { multiline: false })
        );
        assert_eq!(
            form.property_type("cmdGo", "enabled"),
            Some(ValueType::Bool)
        );
        assert_eq!(form.property_type("cmdGo", "nope"), None);
    });
}

#[test]
fn a_form_with_a_bad_parent_fails_to_build() {
    let mut doc = FormDoc::new("main_form");
    let mut child = Node::new("Button", "cmdGo");
    child.parent = Some("ghost".to_owned());
    doc.insert(child);

    let backend: std::rc::Rc<dyn xui_core::backend::Backend> =
        std::rc::Rc::new(xui_canvas::OffscreenBackend::new());
    let catalog = Catalog::xui();
    let factories: xui_form::Factories<Msg> = xui_form::Factories::xui();
    let binder = common::TestBinder;
    let spec = xui_core::backend::PlatformSpec::new("bad parent")
        .size(xui_core::units::Dip(320.0), xui_core::units::Dip(200.0));
    let result = std::cell::RefCell::new(None);
    let result_ref = &result;
    xui_core::app::run_app(backend, spec, move |ui| {
        *result_ref.borrow_mut() = Some(xui_form::build(ui, &doc, &catalog, &factories, &binder));
        common::TestApp {
            messages: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
        }
    })
    .expect("run_app succeeds");
    let error = match result.borrow_mut().take().expect("build ran") {
        Err(error) => error,
        Ok(_) => panic!("a missing parent must fail the build"),
    };
    assert!(matches!(error, xui_form::BuildError::UnknownParent { .. }));
}

#[test]
fn relayout_keeps_geometry_and_anchor_edits_made_through_set() {
    let doc = anchor_doc();
    let catalog = Catalog::xui();
    let button = with_form(&doc, &catalog, BuildOptions::default(), |form| {
        // Move the button and pin it top-left instead of bottom-right.
        form.set("cmdGo", "left", &Value::Int(10))
            .expect("left is settable");
        form.set("cmdGo", "top", &Value::Int(20))
            .expect("top is settable");
        form.set("cmdGo", "anchor", &Value::Enum("top_left".to_owned()))
            .expect("anchor is settable");
        form.relayout(Size::new(500, 400));
        form.bounds("cmdGo")
    });
    assert_eq!(button, Some(Rect::new(10, 20, 60, 60)));
}

#[test]
fn a_child_listed_before_its_container_still_builds() {
    let ordered = anchor_doc();
    let mut doc = FormDoc::new("main_form");
    // The button (child) first, then its panel.
    doc.nodes = vec![ordered.nodes[1].clone(), ordered.nodes[0].clone()];
    let catalog = Catalog::xui();
    let bounds = with_form(&doc, &catalog, BuildOptions::default(), |form| {
        form.bounds("cmdGo")
    });
    assert_eq!(bounds, Some(Rect::new(200, 150, 250, 190)));
}

#[test]
fn float_properties_accept_integers_at_runtime() {
    let mut doc = FormDoc::new("main_form");
    doc.insert(Node::new("NumberField", "numOne"));
    doc.insert(Node::new("Slider", "sldOne"));
    let catalog = Catalog::xui();
    with_form(&doc, &catalog, BuildOptions::default(), |form| {
        form.set("numOne", "value", &Value::Int(3))
            .expect("an int sets a NumberField value");
        assert_eq!(form.get("numOne", "value"), Some(Value::Float(3.0)));
        form.set("sldOne", "value", &Value::Int(0))
            .expect("an int sets a Slider value");
    });
}

#[test]
fn a_list_view_without_selected_has_no_selection() {
    let mut doc = FormDoc::new("main_form");
    let mut list = Node::new("ListView", "items_list");
    list.set_prop("items", Value::List(vec!["a".to_owned(), "b".to_owned()]));
    doc.insert(list);
    let catalog = Catalog::xui();
    let selected = with_form(&doc, &catalog, BuildOptions::default(), |form| {
        form.get("items_list", "selected")
    });
    assert_eq!(selected, Some(Value::Int(-1)));
}

#[test]
fn a_list_view_accepts_items_at_runtime() {
    let mut doc = FormDoc::new("main_form");
    let mut list = Node::new("ListView", "items_list");
    list.set_prop("items", Value::List(vec!["a".to_owned()]));
    doc.insert(list);
    let catalog = Catalog::xui();
    let (before, after) = with_form(&doc, &catalog, BuildOptions::default(), |form| {
        let before = form.get("items_list", "items");
        form.set(
            "items_list",
            "items",
            &Value::List(vec!["a".to_owned(), "b".to_owned()]),
        )
        .expect("items is writable at runtime");
        (before, form.get("items_list", "items"))
    });
    assert_eq!(before, Some(Value::List(vec!["a".to_owned()])));
    assert_eq!(
        after,
        Some(Value::List(vec!["a".to_owned(), "b".to_owned()]))
    );
}

/// A form with a three-option radio group pinned to the bottom-right corner.
fn radio_doc() -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    let mut group = Node::new("RadioGroup", "optSize");
    group.set_prop(
        "items",
        Value::List(vec!["S".to_owned(), "M".to_owned(), "L".to_owned()]),
    );
    group.set_prop("left", Value::Int(200));
    group.set_prop("top", Value::Int(100));
    group.set_prop("width", Value::Int(100));
    group.set_prop("anchor", Value::Enum("bottom_right".to_owned()));
    doc.insert(group);
    doc
}

#[test]
fn relayout_and_edits_move_every_radio_option() {
    let doc = radio_doc();
    let catalog = Catalog::xui();
    let (before, after_resize, after_edit) =
        with_form(&doc, &catalog, BuildOptions::default(), |form| {
            let before = form.node_bounds("optSize");
            form.relayout(Size::new(420, 300));
            let after_resize = form.node_bounds("optSize");
            form.set("optSize", "left", &Value::Int(10))
                .expect("left is settable");
            (before, after_resize, form.node_bounds("optSize"))
        });
    assert_eq!(before.len(), 3, "one node per option");
    for (old, new) in before.iter().zip(&after_resize) {
        // The window grew by (100, 100): every option follows the corner.
        assert_eq!((new.left, new.top), (old.left + 100, old.top + 100));
    }
    for (option, moved) in after_resize.iter().zip(&after_edit) {
        assert_eq!(moved.left, 10, "an edited left moves every option");
        assert_eq!(moved.height(), option.height());
    }
}
