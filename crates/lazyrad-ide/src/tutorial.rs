#![forbid(unsafe_code)]

//! The "Getting started" tutorial the Start Page shows under its welcome text.
//!
//! The content is plain data ([`BLOCKS`]) so tests can check it against the
//! runtime: every code sample must compile as a Rhai script, every stdlib
//! function it documents ([`STDLIB_FUNCTIONS`]) must be registered, and every
//! control property and event it documents ([`CONTROL_PROPERTIES`],
//! [`CONTROL_EVENTS`]) must exist in the catalog. The page (`start_page.rs`)
//! only paints these blocks.
//!
//! Names follow PLAN.md §1.1: xui property names in snake_case, handlers
//! `fn <control>_<event>()`, `form_load` and `form_close`, the current form as
//! `form`, and a snake_case stdlib.

/// One piece of the tutorial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    /// A section heading.
    Heading(&'static str),
    /// A paragraph, wrapped to the column.
    Para(&'static str),
    /// A paragraph set apart from the rest: the one thing not to miss.
    Callout(&'static str),
    /// A Rhai code sample, drawn in the editor's monospace font and colours.
    Code(&'static str),
}

/// The counter handlers from "Remember values between events". The runtime's
/// `form_state_keeps_a_counter_between_two_clicks` test runs this exact text.
pub const COUNTER_SAMPLE: &str = "fn form_load() { form.state.count = 0; }
fn button1_click() {
    form.state.count += 1;
    edit1.text = `Clicked ${form.state.count} times`;
}";

/// The tutorial, top to bottom.
pub const BLOCKS: &[Block] = &[
    Block::Heading("Getting started"),
    Block::Para("1. Draw controls from the Toolbox onto the form."),
    Block::Para("2. Double-click a control to create its handler in the code editor."),
    Block::Para("3. Press F5 to run the program."),
    Block::Heading("Handlers"),
    Block::Para(
        "A handler is a function named after the control and the event: the control's \
         name, an underscore, and the event in lower case. The form has two of its own: \
         form_load runs once when the form opens, form_close when it is closed. \
         Other events are edit1_change, check1_toggle, list1_select and list1_activate.",
    ),
    Block::Code(
        "fn form_load() {
    label1.text = \"Ready\";
}

fn button1_click() {
    msg_box(\"Hello!\");
}

fn form_close() {
    print(\"bye\");
}",
    ),
    Block::Heading("Controls and their properties"),
    Block::Para(
        "Every control is a variable named after it. Properties are lower case with \
         underscores, exactly as the Properties grid shows them: edit1.text is right, \
         edit1.Text is an error. Common to all controls: left, top, width, height, \
         anchor, visible, enabled and tab_index. Others: text (Button, Edit, Label, \
         CheckBox, ...), checked (CheckBox, ToggleButton), items and selected (ListView, \
         ComboBox, RadioGroup), value (NumberField, Slider, ProgressBar).",
    ),
    Block::Code(
        "fn button1_click() {
    label1.text = edit1.text;
    check1.checked = true;
    edit1.enabled = false;

    // items is a copy: change it, then assign it back.
    let items = list1.items;
    items.push(\"one more\");
    list1.items = items;

    // selected is the chosen row's index; check it before using it.
    if list1.selected >= 0 {
        label1.text = items[list1.selected];
    }
}",
    ),
    Block::Heading("Remember values between events"),
    Block::Callout(
        "Functions cannot see variables declared with let at the top of the script, so a \
         counter kept there does not work. Keep values between events in form.state, an \
         object map that lives as long as the form.",
    ),
    Block::Code(COUNTER_SAMPLE),
    Block::Para(
        "form is the current form. form.state holds your data; form.title is a string \
         you can read and set.",
    ),
    Block::Heading("The standard library"),
    Block::Code(
        "msg_box(\"Saved\");                     // a message box
msg_box(\"Saved\", \"My App\");          // with a title
msg_box(\"Delete it?\", \"Confirm\", \"yes_no\", Fn(\"answered\"));
fn answered(button) {                 // \"ok\", \"cancel\", \"yes\" or \"no\"
    if button == \"yes\" { list1.items = []; }
}

open_file_dialog(\"Open a song\", \"MOD files|*.mod;All files|*.*\", |path| {
    print(path);                      // the picked path, or () when cancelled
});

let data = file_read_bytes(\"songs/song.mod\");   // a Blob of bytes
file_write_bytes(\"copy.mod\", data);
let text = file_read_text(\"notes.txt\");          // project-relative paths work

app.quit();                           // close the program
let title = app.title;                // also app.path
let stamp = now();                    // \"2026-01-31 14:05:09\"
let day = today();                    // \"2026-01-31\"
let x = random();                     // a float from 0.0 up to 1.0
let die = random_range(1, 7);         // 1 to 6: the end is excluded
seed_random(42);                      // make random() repeatable
let line = [\"a\", \"b\"].join(\", \");    // \"a, b\"
print(line);                          // to the Output pane",
    ),
    Block::Para(
        "msg_box does not wait: the code after it runs at once, and the callback is \
         called later with the button pressed. Rhai's own string, math and array \
         functions (len, trim, to_upper, split, abs, min, max, ...) are available too.",
    ),
    Block::Heading("Games: the Canvas"),
    Block::Para(
        "A Canvas is a control you draw on. Set its fps property and canvas1_frame(dt) \
         runs that many times a second, dt being the seconds since the last frame. Each \
         frame, clear the canvas and draw the scene again. Positions are in pixels from \
         the canvas's top-left corner; a colour is \"#rrggbb\" or rgb(r, g, b). Clicking \
         the canvas gives it the keyboard; is_key_down tells whether a key is held, and \
         canvas1_key_down(key), canvas1_key_up(key), canvas1_mouse_down(x, y, button), \
         canvas1_mouse_up and canvas1_mouse_move(x, y) report input as it happens.",
    ),
    Block::Code(
        "fn form_load() {
    form.state.x = 0.0;
    canvas1.fps = 60;
    canvas1.focus();                      // keys go to the canvas
}

fn canvas1_frame(dt) {
    if canvas1.is_key_down(\"right\") { form.state.x += 200.0 * dt; }
    if canvas1.is_key_down(\"left\") { form.state.x -= 200.0 * dt; }
    form.state.x = clamp(form.state.x, 0.0, canvas1.width - 20);

    canvas1.clear(rgb(20, 20, 40));
    canvas1.fill_rect(form.state.x, 100, 20, 20, \"#ffcc00\");
    canvas1.fill_circle(160, 60, 10, 0x40c0ff);
    canvas1.text(8, 8, `x = ${form.state.x.to_int()}`, \"#ffffff\", 14);
    if rects_overlap(form.state.x, 100, 20, 20, 150, 50, 20, 20) {
        print(\"hit\");
    }
}

fn canvas1_key_down(key) {
    if key == \"space\" { print(\"jump\"); }
}",
    ),
    Block::Para(
        "Other drawing methods: stroke_rect, fill_round_rect, stroke_circle, line and \
         text_width. Do not give your own functions a canvas method's name and number \
         of parameters (a fn clear(color) of your own): Rhai would call yours for \
         canvas1.clear(color) too.",
    ),
    Block::Heading("Modules"),
    Block::Para(
        "Project > Add Module adds a plain script file. Its functions can be called \
         directly or through the module name.",
    ),
    Block::Code(
        "// util.rhai
fn greeting(name) { `Hello, ${name}!` }

// main_form.rhai
fn form_load() {
    label1.text = greeting(\"Ada\");        // direct
    label1.text = util::greeting(\"Ada\");  // or qualified
}",
    ),
    Block::Heading("Rhai in a minute"),
    Block::Code(
        "let name = \"Ada\";                    // variables
let total = 0;
for x in 0..10 { total += x; }        // 0 to 9
if total > 40 {
    print(\"big\");
} else {
    print(\"small\");
}
let message = `Hello, ${name}!`;      // interpolated string
let list = [1, 2, 3];                 // array
list.push(4);
let person = #{ name: \"Ada\", age: 36 };  // map
person.age += 1;
fn double(x) { x * 2 }                // the last value is returned",
    ),
    Block::Heading("A complete example"),
    Block::Para(
        "Put a Label named label1, an Edit named edit1 and a Button named button1 on \
         the form, then write this and press F5.",
    ),
    Block::Code(
        "fn button1_click() {
    label1.text = `Hello, ${edit1.text}!`;
}",
    ),
];

/// The stdlib functions the tutorial documents. A test checks each is registered
/// on the engine and appears in the tutorial's code.
pub const STDLIB_FUNCTIONS: &[&str] = &[
    "msg_box",
    "open_file_dialog",
    "file_read_bytes",
    "file_write_bytes",
    "file_read_text",
    "now",
    "today",
    "random",
    "random_range",
    "seed_random",
    "join",
    "print",
    "rgb",
    "clamp",
    "rects_overlap",
];

/// The `(widget kind, property)` pairs the tutorial documents. A test checks
/// each exists in the catalog.
pub const CONTROL_PROPERTIES: &[(&str, &str)] = &[
    ("Edit", "text"),
    ("Label", "text"),
    ("Button", "text"),
    ("CheckBox", "checked"),
    ("ToggleButton", "checked"),
    ("ListView", "items"),
    ("ListView", "selected"),
    ("ComboBox", "items"),
    ("ComboBox", "selected"),
    ("RadioGroup", "items"),
    ("RadioGroup", "selected"),
    ("NumberField", "value"),
    ("Slider", "value"),
    ("ProgressBar", "value"),
    ("Edit", "enabled"),
    ("Edit", "left"),
    ("Edit", "top"),
    ("Edit", "width"),
    ("Edit", "height"),
    ("Edit", "anchor"),
    ("Edit", "visible"),
    ("Edit", "tab_index"),
    ("Canvas", "fps"),
    ("Canvas", "width"),
];

/// The `(widget kind, control name, event)` triples whose handler names the
/// tutorial mentions.
pub const CONTROL_EVENTS: &[(&str, &str, &str)] = &[
    ("Button", "button1", "Click"),
    ("Edit", "edit1", "Change"),
    ("CheckBox", "check1", "Toggle"),
    ("ListView", "list1", "Select"),
    ("ListView", "list1", "Activate"),
    ("Canvas", "canvas1", "Frame"),
    ("Canvas", "canvas1", "KeyDown"),
    ("Canvas", "canvas1", "KeyUp"),
    ("Canvas", "canvas1", "MouseDown"),
    ("Canvas", "canvas1", "MouseUp"),
    ("Canvas", "canvas1", "MouseMove"),
];

/// The tutorial's code samples, in order.
pub fn code_samples() -> impl Iterator<Item = &'static str> {
    BLOCKS.iter().filter_map(|block| match block {
        Block::Code(code) => Some(*code),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::rc::Rc;

    use lazyrad_runtime::{EngineHost, FormHost, StdlibContext};
    use xui_form::{SetError, Value, ValueType};

    use super::*;

    /// A form with no controls: enough to build the engine and its stdlib.
    struct NoForm;

    impl FormHost for NoForm {
        fn get(&self, _control: &str, _property: &str) -> Option<Value> {
            None
        }
        fn set(&self, _control: &str, _property: &str, _value: &Value) -> Result<(), SetError> {
            Err(SetError::UnknownWidget)
        }
        fn property_type(&self, _control: &str, _property: &str) -> Option<ValueType> {
            None
        }
        fn names(&self) -> Vec<String> {
            Vec::new()
        }
        fn kind(&self, _control: &str) -> Option<String> {
            None
        }
        fn property_names(&self, _control: &str) -> Vec<String> {
            Vec::new()
        }
    }

    /// The runtime's engine with the stdlib installed, and the names of every
    /// function registered on it (Rhai's built-ins included).
    fn engine() -> (EngineHost, BTreeSet<String>) {
        let host = EngineHost::new(
            Rc::new(NoForm),
            &lazyrad_project::lazyrad_catalog(),
            "tutorial.rhai",
            StdlibContext::headless("main_form"),
        );
        let metadata = host
            .engine()
            .gen_fn_metadata_to_json(true)
            .expect("metadata serialises");
        let json: serde_json::Value = serde_json::from_str(&metadata).expect("metadata is JSON");
        let names = json["functions"]
            .as_array()
            .expect("a function list")
            .iter()
            .filter_map(|function| function["name"].as_str().map(str::to_owned))
            .collect();
        (host, names)
    }

    /// `code` with `//` comments and string contents removed, so a scan sees
    /// only code.
    fn strip(code: &str) -> String {
        let mut out = String::new();
        for line in code.lines() {
            let mut in_string: Option<char> = None;
            let mut chars = line.chars().peekable();
            while let Some(c) = chars.next() {
                match in_string {
                    Some(quote) if c == quote => {
                        in_string = None;
                        out.push(c);
                    }
                    Some(_) => {}
                    None if c == '/' && chars.peek() == Some(&'/') => break,
                    None => {
                        if c == '"' || c == '`' {
                            in_string = Some(c);
                        }
                        out.push(c);
                    }
                }
            }
            out.push('\n');
        }
        out
    }

    /// One identifier in some code, with the character before and after it.
    struct Word {
        text: String,
        before: char,
        after: char,
    }

    /// Every identifier in `code`.
    fn words(code: &str) -> Vec<Word> {
        let chars: Vec<char> = code.chars().collect();
        let mut words = Vec::new();
        let mut index = 0;
        while index < chars.len() {
            if chars[index].is_alphabetic() || chars[index] == '_' {
                let start = index;
                while index < chars.len() && (chars[index].is_alphanumeric() || chars[index] == '_')
                {
                    index += 1;
                }
                words.push(Word {
                    text: chars[start..index].iter().collect(),
                    before: if start == 0 { ' ' } else { chars[start - 1] },
                    after: chars.get(index).copied().unwrap_or(' '),
                });
            } else {
                index += 1;
            }
        }
        words
    }

    /// Every block's text, joined.
    fn all_text() -> String {
        BLOCKS
            .iter()
            .map(|block| match block {
                Block::Heading(text)
                | Block::Para(text)
                | Block::Callout(text)
                | Block::Code(text) => *text,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_code_sample_compiles() {
        let (host, _) = engine();
        for sample in code_samples() {
            host.compile(sample)
                .unwrap_or_else(|error| panic!("{error}\n{sample}"));
        }
    }

    #[test]
    fn the_counter_sample_is_the_one_the_runtime_test_runs() {
        assert!(code_samples().any(|sample| sample == COUNTER_SAMPLE));
    }

    #[test]
    fn every_documented_stdlib_function_is_registered_and_shown() {
        let (_, registered) = engine();
        let code: String = code_samples().collect::<Vec<_>>().join("\n");
        for name in STDLIB_FUNCTIONS {
            assert!(
                registered.contains(*name),
                "`{name}` is documented but not registered"
            );
            assert!(code.contains(name), "`{name}` is not in a code sample");
        }
    }

    #[test]
    fn every_function_a_sample_calls_exists() {
        let (_, registered) = engine();
        let mut defined: BTreeSet<String> = BTreeSet::new();
        for sample in code_samples() {
            let words = words(&strip(sample));
            for pair in words.windows(2) {
                if pair[0].text == "fn" {
                    defined.insert(pair[1].text.clone());
                }
            }
        }
        for sample in code_samples() {
            for word in words(&strip(sample)) {
                let keyword = word.text == "fn" || word.text == "Fn";
                if word.after == '(' && !keyword {
                    assert!(
                        registered.contains(&word.text) || defined.contains(&word.text),
                        "`{}(` in a sample is neither registered nor defined",
                        word.text
                    );
                }
            }
        }
    }

    #[test]
    fn every_documented_property_and_event_is_in_the_catalog() {
        let catalog = lazyrad_project::lazyrad_catalog();
        for (kind, property) in CONTROL_PROPERTIES {
            assert!(
                catalog.property(kind, property).is_some(),
                "{kind} has no `{property}` property"
            );
        }
        let text = all_text();
        for (kind, control, event) in CONTROL_EVENTS {
            let spec = catalog.get(kind).expect("a known kind");
            assert!(
                spec.events.iter().any(|spec| spec.name == *event),
                "{kind} has no {event} event"
            );
            let handler = lazyrad_runtime::form::handler_name(control, event);
            assert!(text.contains(&handler), "`{handler}` is not mentioned");
        }
    }

    #[test]
    fn samples_use_real_properties_on_controls_form_and_app() {
        let catalog = lazyrad_project::lazyrad_catalog();
        let kinds = [
            ("button1", "Button"),
            ("edit1", "Edit"),
            ("label1", "Label"),
            ("check1", "CheckBox"),
            ("list1", "ListView"),
            ("canvas1", "Canvas"),
        ];
        let mut checked = 0;
        for sample in code_samples() {
            let words = words(&strip(sample));
            for pair in words.windows(2) {
                let (owner, member) = (&pair[0], &pair[1]);
                if member.before != '.' || owner.after != '.' {
                    continue;
                }
                checked += 1;
                if let Some((_, kind)) = kinds.iter().find(|(name, _)| *name == owner.text) {
                    let is_method = member.after == '(';
                    assert!(
                        if is_method {
                            catalog.method(kind, &member.text).is_some()
                        } else {
                            catalog.property(kind, &member.text).is_some()
                        },
                        "{}.{} is not a {kind} property or method",
                        owner.text,
                        member.text
                    );
                } else if owner.text == "form" {
                    assert!(["title", "state"].contains(&member.text.as_str()));
                } else if owner.text == "app" {
                    assert!(["title", "path", "quit"].contains(&member.text.as_str()));
                }
            }
        }
        assert!(checked > 10, "the scan found {checked} member accesses");
    }
}
