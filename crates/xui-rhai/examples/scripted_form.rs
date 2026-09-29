//! A scripted form with no LazyRAD project: an `.lfm` document plus a `.rhai`
//! script, built and run by `xui-rhai` alone.
//!
//! It shows the reusable path: [`FormDoc::from_toml`] loads the form,
//! [`ScriptForm::build`] builds its widgets, wires every `<control>_<event>`
//! handler the script defines and runs `form_load`, and the host routes widget
//! events back into the script.
//!
//! Run it with `cargo run -p xui-rhai --example scripted_form`.

use std::rc::Rc;

use xui_canvas::WinitBackend;
use xui_core::app::{App, Ui, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;
use xui_form::{Catalog, FormDoc, Value};
use xui_rhai::Msg;
use xui_rhai::form::{ScriptForm, ScriptSource};

/// The script, embedded so the example needs no working directory. The form's
/// `form_load` greets the initial name; `greet_button_click` greets the current
/// one.
const CODE: &str = include_str!("hello.rhai");

/// Owns the scripted form and routes its widget events into the script.
struct ScriptApp {
    form: ScriptForm,
}

impl App for ScriptApp {
    type Msg = Msg;

    fn update(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
        match msg {
            Msg::Event {
                control,
                event,
                args,
                ..
            } => {
                if let Err(error) = self.form.run(&control, &event, &args) {
                    eprintln!("xui-rhai: {error}");
                }
            }
            Msg::Quit => ui.quit(),
            _ => {}
        }
    }
}

/// The toolkit window spec for a form document.
fn window_spec(doc: &FormDoc) -> PlatformSpec {
    let title = doc
        .window
        .prop("title")
        .and_then(Value::as_str)
        .unwrap_or(&doc.window.name);
    let width = doc
        .window
        .prop("width")
        .and_then(Value::as_int)
        .unwrap_or(320) as f32;
    let height = doc
        .window
        .prop("height")
        .and_then(Value::as_int)
        .unwrap_or(200) as f32;
    PlatformSpec::new(title).size(Dip(width), Dip(height))
}

fn main() {
    let catalog = Catalog::xui();
    let doc = FormDoc::from_toml(include_str!("hello.lfm"), &catalog).expect("the form loads");
    let spec = window_spec(&doc);
    let backend: Rc<dyn Backend> = Rc::new(WinitBackend::new());

    run_app(backend, spec, move |ui| {
        let form = ScriptForm::build(
            ui,
            &doc,
            &catalog,
            ScriptSource {
                name: "main_form",
                code: CODE,
                file: "hello.rhai",
            },
            (),
            |_host| Ok(()),
        )
        .expect("the form builds");
        ScriptApp { form }
    })
    .expect("the event loop runs");
}
