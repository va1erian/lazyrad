//! Golden patch traces for the `profile_form` of docs/modern-ui-plan.md §4.2,
//! built by hand as the `.lfm` loader eventually will.
//!
//! The expected text is the contract a renderer is tested against: any
//! renderer driven by the same events must see exactly these patches.

mod common;

use common::{int, map, text, tree};
use lazyrad_view::{
    Bindings, Environment, Node, RecordingRenderer, Renderer, Session, SizeClass, Value,
};
use rhai::Map;

fn profile_form() -> Node {
    Node::new("profile_form", "Column")
        .with_prop("padding", 16)
        .with_prop("gap", 12)
        .with_child(Node::new("title_label", "Label").with_prop("text", "Profile"))
        .with_child(Node::new("name_edit", "Edit").with_prop("placeholder", "Your name"))
        .with_child(
            Node::new("greeting_label", "Label")
                .with_bind("text", "`Hello, ${state.name}!`")
                .with_bind("visible", "state.name != \"\""),
        )
        .with_child(
            Node::new("cards", "Flex")
                .with_prop("direction", Value::variant("Vertical"))
                .with_prop("gap", 12)
                .with_responsive(SizeClass::Medium, "direction", Value::variant("Horizontal"))
                .with_child(
                    Node::new("counter_card", "Panel")
                        .with_prop("card", true)
                        .with_responsive(SizeClass::Medium, "fill", 1)
                        .with_child(
                            Node::new("counter_content", "Column")
                                .with_prop("padding", 12)
                                .with_prop("gap", 8)
                                .with_child(
                                    Node::new("count_label", "Label")
                                        .with_bind("text", "`Clicked ${state.count} times`"),
                                )
                                .with_child(
                                    Node::new("add_button", "Button")
                                        .with_prop("text", "Add one")
                                        .with_prop("align", Value::variant("Start")),
                                ),
                        ),
                ),
        )
}

const PHONE_BUILD: &str = r#"create profile_form Column parent=- index=0 {gap=12, padding=16}
create title_label Label parent=profile_form index=0 {text="Profile"}
create name_edit Edit parent=profile_form index=1 {placeholder="Your name"}
create greeting_label Label parent=profile_form index=2 {text="Hello, !", visible=false}
create cards Flex parent=profile_form index=3 {direction=Vertical, gap=12}
create counter_card Panel parent=cards index=0 {card=true}
create counter_content Column parent=counter_card index=0 {gap=8, padding=12}
create count_label Label parent=counter_content index=0 {text="Clicked 0 times"}
create add_button Button parent=counter_content index=1 {align=Start, text="Add one"}
"#;

fn form_state(name: &str, count: i64) -> Map {
    map(&[("name", text(name)), ("count", int(count))])
}

#[test]
fn the_profile_form_turns_match_their_goldens() {
    let mut session = Session::new(tree(profile_form()), Bindings::new());
    let mut renderer = RecordingRenderer::new();
    let phone = Environment::new(360.0, 640.0);

    // form_load: state.name = ""; state.count = 0;
    let mut state = form_state("", 0);
    let built = session.build(&state, &phone);
    assert!(built.diagnostics.is_empty());
    renderer.apply(&built.patches);
    assert_eq!(renderer.take_trace(), PHONE_BUILD);

    // name_edit_change("Ada"): state.name = text;
    state.insert("name".into(), text("Ada"));
    let turn = session.update_with(&state, &phone, &mut renderer);
    assert!(turn.diagnostics.is_empty());
    assert_eq!(
        renderer.take_trace(),
        "set greeting_label.text = \"Hello, Ada!\"\n\
         set greeting_label.visible = true\n"
    );

    // add_button_click(): state.count += 1;
    state.insert("count".into(), int(1));
    session.update_with(&state, &phone, &mut renderer);
    assert_eq!(
        renderer.take_trace(),
        "set count_label.text = \"Clicked 1 times\"\n"
    );

    // The window grows to 700 dp: Medium. Exactly the two layout fields.
    let tablet = Environment::new(700.0, 640.0);
    let turn = session.update_with(&state, &tablet, &mut renderer);
    assert!(turn.diagnostics.is_empty());
    assert_eq!(
        renderer.take_trace(),
        "set cards.direction = Horizontal\n\
         set counter_card.fill = 1\n"
    );

    // 700 -> 800 stays in Medium: no patches at all.
    let wider = Environment::new(800.0, 640.0);
    let turn = session.update_with(&state, &wider, &mut renderer);
    assert!(turn.patches.is_empty());
    assert_eq!(renderer.take_trace(), "");

    // And back to a phone: the direction returns, `fill` (which only the
    // patch gave) is unset.
    session.update_with(&state, &phone, &mut renderer);
    assert_eq!(
        renderer.take_trace(),
        "set cards.direction = Vertical\n\
         unset counter_card.fill\n"
    );
}

#[test]
fn building_wide_resolves_the_cascade_into_the_creates() {
    let mut session = Session::new(tree(profile_form()), Bindings::new());
    let desktop = Environment::new(1000.0, 700.0);
    let built = session.build(&form_state("", 0), &desktop);
    assert!(built.diagnostics.is_empty());
    let trace = lazyrad_view::trace(&built.patches);
    // The only two lines that differ from the phone build.
    let expected = PHONE_BUILD
        .replace(
            "{direction=Vertical, gap=12}",
            "{direction=Horizontal, gap=12}",
        )
        .replace("{card=true}", "{card=true, fill=1}");
    assert_eq!(trace, expected);
}
