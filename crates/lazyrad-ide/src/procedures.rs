#![forbid(unsafe_code)]

//! The two procedure combo boxes at the top of a form's code window (issue
//! #11), and the handler insertion they drive.
//!
//! The object combo lists the form itself and its controls; the procedure combo
//! lists the selected object's events. Both are read from the shared form
//! catalog ([`Catalog`]), so a control's events and their argument names match
//! exactly what the runtime binds. Choosing an event with no handler appends
//! `fn <control>_<event>(<args>) {\n}` (the event in snake_case, PLAN.md §1.1)
//! and places the caret inside it.
//!
//! Everything here is pure data and text, so it is unit-tested without a
//! `Ui`. The IDE supplies the source text and applies the returned snippet.

use lazyrad_project::{Catalog, FormDoc};
use xui_code_editor::{Buffer, HighlightCache, RhaiHighlighter, TokenClass};
use xui_form::{ArgSpec, EventSpec, ValueType};

/// The prefix the form's own handlers use: `form_load`, `form_close`.
pub const FORM_PREFIX: &str = "form";

/// The synthetic window event for `form_reload`, the hot-reload state hook
/// (issue #91). It is not raised by the toolkit, so it is not in the catalog;
/// the procedure combo lists it so a user can create the handler, and the
/// runtime calls it after `form_load` with the old `form.state`.
fn reload_event() -> EventSpec {
    EventSpec {
        name: "Reload".to_owned(),
        args: vec![ArgSpec {
            name: "old_state".to_owned(),
            ty: ValueType::Text { multiline: false },
        }],
        is_default: false,
        description: "Runs after form_load on a hot reload, with the old form.state.".to_owned(),
    }
}

/// One entry in the object combo: the form or a control, plus its events.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectEntry {
    /// The label shown in the object combo.
    pub label: String,
    /// The handler-name prefix (`form` for the window, the control name
    /// otherwise).
    pub prefix: String,
    /// The events the object raises, in catalog order.
    pub events: Vec<EventSpec>,
}

impl ObjectEntry {
    /// The procedure (event) names for the procedure combo, in the
    /// snake_case the handler uses (`click`, `load`).
    pub fn event_names(&self) -> Vec<String> {
        self.events
            .iter()
            .map(|event| snake_case(&event.name))
            .collect()
    }
}

/// Builds the object combo's entries for `form`: the window first, then each
/// control that declares at least one event.
pub fn objects(catalog: &Catalog, form: &FormDoc) -> Vec<ObjectEntry> {
    let mut form_events = catalog.window_spec().events.clone();
    // `form_reload` is a runtime hook, not a toolkit event, so it is added by
    // hand (issue #91).
    form_events.push(reload_event());
    let mut entries = vec![ObjectEntry {
        label: FORM_PREFIX.to_owned(),
        prefix: FORM_PREFIX.to_owned(),
        events: form_events,
    }];
    for node in &form.nodes {
        let Some(spec) = catalog.get(&node.kind) else {
            continue;
        };
        if spec.events.is_empty() {
            continue;
        }
        entries.push(ObjectEntry {
            label: node.name.clone(),
            prefix: node.name.clone(),
            events: spec.events.clone(),
        });
    }
    entries
}

/// The handler name for `prefix` and `event`: `hello_button` + `Click` is
/// `hello_button_click`, the name the runtime binds (PLAN.md §1.1).
pub fn signature(prefix: &str, event: &str) -> String {
    format!("{prefix}_{}", snake_case(event))
}

/// `PascalCase` to `snake_case`; an already snake_case name is unchanged.
fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, character) in name.chars().enumerate() {
        if character.is_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.extend(character.to_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

/// The comma-separated argument list an [`EventSpec`] declares.
pub fn argument_list(event: &EventSpec) -> String {
    event
        .args
        .iter()
        .map(|arg| arg.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The handler source appended for a signature with `args`.
///
/// It has no leading separator and no trailing blank line beyond the single
/// newline after the closing brace, so the caller controls the spacing.
pub fn handler_snippet(signature: &str, args: &str) -> String {
    format!("fn {signature}({args}) {{\n}}\n")
}

/// The char offset just inside the body of the handler `signature` in `text`,
/// if a `fn <signature>(...) {` definition is present in the code.
///
/// Comments and strings are skipped (a `fn` in either is not a definition),
/// the parameter list must be balanced, and only the brace that directly
/// follows it counts. The caret lands just after that brace, ready for typing.
pub fn find_handler(text: &str, signature: &str) -> Option<usize> {
    let code = code_chars(text);
    let needle: Vec<char> = format!("fn {signature}(").chars().collect();
    let mut from = 0;
    while let Some(found) = code
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle.as_slice())
    {
        let start = from + found;
        from = start + 1;
        // `xfn name(` or `my_fn name(` is not a definition.
        if start > 0 && is_identifier_char(code[start - 1]) {
            continue;
        }
        if let Some(brace) = body_brace(&code, start + needle.len()) {
            return Some(brace + 1);
        }
    }
    None
}

/// `text`'s chars with every comment and string blanked to spaces, so a search
/// only sees code. Offsets are unchanged.
fn code_chars(text: &str) -> Vec<char> {
    let buffer = Buffer::new(text);
    let lexer = HighlightCache::new(&buffer, RhaiHighlighter);
    let mut code: Vec<char> = text.chars().collect();
    for line in 0..buffer.line_count() {
        let base = buffer.line_start(line);
        for token in lexer.tokens(line) {
            if matches!(
                token.class,
                TokenClass::Comment
                    | TokenClass::DocComment
                    | TokenClass::String
                    | TokenClass::Interpolation
            ) {
                for index in base + token.start..(base + token.end).min(code.len()) {
                    code[index] = ' ';
                }
            }
        }
    }
    code
}

/// The offset of the `{` opening a body, given the offset just past a
/// parameter list's `(`: the list must close, and only whitespace may sit
/// between its `)` and the brace.
fn body_brace(code: &[char], after_open: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut index = after_open;
    while depth > 0 {
        match code.get(index)? {
            '(' => depth += 1,
            ')' => depth -= 1,
            '{' | '}' | ';' => return None,
            _ => {}
        }
        index += 1;
    }
    while code.get(index)?.is_whitespace() {
        index += 1;
    }
    (code[index] == '{').then_some(index)
}

/// Whether `c` can be part of a Rhai identifier.
fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazyrad_project::{Node, lazyrad_catalog};

    fn form_with_controls() -> FormDoc {
        let mut form = FormDoc::new("form1");
        form.nodes.push(Node::new("Button", "go_button"));
        form.nodes.push(Node::new("Label", "result_label"));
        form.nodes.push(Node::new("Edit", "name_edit"));
        form
    }

    #[test]
    fn the_object_combo_starts_with_the_form_and_lists_eventful_controls() {
        let catalog = lazyrad_catalog();
        let form = form_with_controls();
        let objects = objects(&catalog, &form);

        assert_eq!(objects[0].label, "form");
        assert_eq!(objects[0].prefix, FORM_PREFIX);
        assert!(objects[0].events.iter().any(|event| event.name == "Load"));
        assert!(
            objects[0].event_names().contains(&"reload".to_owned()),
            "the form offers the form_reload hook"
        );

        let labels: Vec<&str> = objects.iter().map(|entry| entry.label.as_str()).collect();
        assert_eq!(
            labels,
            ["form", "go_button", "name_edit"],
            "a label has no events"
        );
        assert_eq!(objects[1].event_names(), ["click"]);
        assert_eq!(objects[2].event_names(), ["change"]);
    }

    #[test]
    fn an_event_argument_list_is_comma_separated_names() {
        let catalog = lazyrad_catalog();
        let window = catalog.window_spec();
        let resize = window.event("Resize").expect("the window resizes");
        assert_eq!(argument_list(resize), "width, height");
        let load = window.event("Load").expect("the window loads");
        assert_eq!(argument_list(load), "");
    }

    #[test]
    fn a_canvas_double_click_writes_its_frame_handler() {
        let catalog = lazyrad_catalog();
        let frame = catalog
            .get("Canvas")
            .and_then(|canvas| canvas.default_event())
            .expect("a canvas has a default event");
        let snippet = handler_snippet(&signature("canvas1", &frame.name), &argument_list(frame));
        assert_eq!(snippet, "fn canvas1_frame(dt) {\n}\n");
        let key = catalog
            .get("Canvas")
            .and_then(|canvas| canvas.event("KeyDown"))
            .expect("a canvas reports keys");
        assert_eq!(signature("canvas1", &key.name), "canvas1_key_down");
        assert_eq!(argument_list(key), "key");
    }

    #[test]
    fn a_handler_snippet_carries_the_signature_and_args() {
        assert_eq!(
            handler_snippet("go_button_click", ""),
            "fn go_button_click() {\n}\n"
        );
        assert_eq!(
            handler_snippet("form_resize", "width, height"),
            "fn form_resize(width, height) {\n}\n"
        );
    }

    #[test]
    fn an_existing_handler_is_found_at_its_body() {
        let text = "fn form_load() {\n    let x = 1;\n}\n";
        let offset = find_handler(text, "form_load").expect("the handler exists");
        assert_eq!(&text[..offset], "fn form_load() {");
        assert!(find_handler(text, "go_button_click").is_none());
    }

    #[test]
    fn find_handler_lands_inside_a_multi_arg_signature() {
        let text = "fn form_resize(width, height) {\n}\n";
        let offset = find_handler(text, "form_resize").expect("found");
        assert_eq!(offset, text.find('{').expect("there is a brace") + 1);
    }

    #[test]
    fn find_handler_ignores_comments_and_strings() {
        let text = "// fn go_button_click() {\nlet s = \"fn go_button_click() {\";\n";
        assert!(find_handler(text, "go_button_click").is_none());

        let text = "/* fn go_button_click() { */\nfn go_button_click() {\n}\n";
        let offset = find_handler(text, "go_button_click").expect("the real one");
        assert_eq!(offset, text.rfind('{').expect("a brace") + 1);
    }

    #[test]
    fn find_handler_needs_a_body_right_after_the_parameters() {
        // An unclosed parameter list is not a definition, so the later brace
        // (which belongs to `if`) is not taken.
        let text = "fn go_button_click(a;\nif x { }\n";
        assert!(find_handler(text, "go_button_click").is_none());
        let text = "fn go_button_click((a), b)\n{\n}\n";
        assert_eq!(
            find_handler(text, "go_button_click"),
            Some(text.find('{').expect("a brace") + 1)
        );
    }
}
