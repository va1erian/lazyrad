//! A login form built entirely from embedded TOML.
//!
//! It shows that the crate has nothing to do with any particular application:
//! the form is schema-validated data, the events are mapped to a plain Rust
//! [`Msg`] enum by a [`Binder`], and the widgets come from the built-in
//! `xui-core` factories.
//!
//! Run it with `cargo run -p xui-form --example login`.

use std::cell::RefCell;
use std::rc::Rc;

use xui_canvas::WinitBackend;
use xui_core::app::{App, Ui, run_app};
use xui_core::backend::{Backend, PlatformSpec};
use xui_core::units::Dip;
use xui_form::{
    Binder, BuildOptions, Catalog, EventHandler, EventRef, Factories, FormDoc, LiveForm, Value,
    build_with,
};

/// The form, in the crate's canonical TOML.
const LOGIN: &str = "\
format = 1

[window]
name = \"frmLogin\"
height = 180
title = \"Sign in\"
width = 320

[[node]]
kind = \"Label\"
name = \"lblUser\"
left = 16
top = 20
width = 80
height = 20
text = \"User name\"

[[node]]
kind = \"Edit\"
name = \"txtUser\"
left = 100
top = 18
width = 190
height = 26
cue = \"your name\"

[[node]]
kind = \"Label\"
name = \"lblPass\"
left = 16
top = 56
width = 80
height = 20
text = \"Password\"

[[node]]
kind = \"Edit\"
name = \"txtPass\"
left = 100
top = 54
width = 190
height = 26
cue = \"********\"

[[node]]
kind = \"CheckBox\"
name = \"chkRemember\"
left = 100
top = 92
width = 160
height = 24
text = \"Remember me\"

[[node]]
kind = \"Button\"
name = \"cmdSignIn\"
left = 100
top = 128
width = 90
height = 30
text = \"Sign in\"

[[node]]
kind = \"Button\"
name = \"cmdCancel\"
left = 200
top = 128
width = 90
height = 30
text = \"Cancel\"
";

/// The application's messages; nothing here is application-specific.
#[derive(Clone, Debug)]
enum Msg {
    /// A change to the remembered state.
    Remember(bool),
    /// A sign-in attempt with the current user name.
    SignIn { user: String, remember: bool },
    /// The cancel button.
    Cancel,
}

/// The live field values the click handler reads.
#[derive(Default)]
struct LoginState {
    user: String,
    remember: bool,
}

/// Maps form events to [`Msg`].
struct LoginBinder {
    state: Rc<RefCell<LoginState>>,
}

impl Binder<Msg> for LoginBinder {
    fn bind(&self, event: EventRef<'_>) -> Option<EventHandler<Msg>> {
        match (event.node, event.event) {
            ("txtUser", "Change") => {
                let state = Rc::clone(&self.state);
                Some(Rc::new(move |args| {
                    if let Some(text) = args.first().and_then(Value::as_str) {
                        state.borrow_mut().user = text.to_owned();
                    }
                    None
                }))
            }
            ("chkRemember", "Toggle") => {
                let state = Rc::clone(&self.state);
                Some(Rc::new(move |args| {
                    let checked = args.first().and_then(Value::as_bool).unwrap_or(false);
                    state.borrow_mut().remember = checked;
                    Some(Msg::Remember(checked))
                }))
            }
            ("cmdSignIn", "Click") => {
                let state = Rc::clone(&self.state);
                Some(Rc::new(move |_| {
                    let state = state.borrow();
                    Some(Msg::SignIn {
                        user: state.user.clone(),
                        remember: state.remember,
                    })
                }))
            }
            ("cmdCancel", "Click") => Some(Rc::new(|_| Some(Msg::Cancel))),
            _ => None,
        }
    }
}

/// Owns the form (which owns every widget) and handles its messages.
struct LoginApp {
    /// Kept alive so the widgets are not destroyed.
    _form: LiveForm<Msg>,
}

impl App for LoginApp {
    type Msg = Msg;

    fn update(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
        match msg {
            Msg::Remember(on) => println!("remember me: {on}"),
            Msg::SignIn { user, remember } => {
                println!("sign in as `{user}` (remember = {remember})");
            }
            Msg::Cancel => ui.quit(),
        }
    }
}

fn main() {
    let catalog = Catalog::xui();
    let doc = FormDoc::from_toml(LOGIN, &catalog).expect("the login form loads");
    let title = doc
        .window
        .prop("title")
        .and_then(Value::as_str)
        .unwrap_or("Sign in")
        .to_owned();
    let width = doc
        .window
        .prop("width")
        .and_then(Value::as_int)
        .unwrap_or(320);
    let height = doc
        .window
        .prop("height")
        .and_then(Value::as_int)
        .unwrap_or(200);
    let spec = PlatformSpec::new(&title).size(Dip(width as f32), Dip(height as f32));
    let backend: Rc<dyn Backend> = Rc::new(WinitBackend::new());

    run_app(backend, spec, move |ui| {
        let factories: Factories<Msg> = Factories::xui();
        let state = Rc::new(RefCell::new(LoginState::default()));
        let binder = LoginBinder {
            state: Rc::clone(&state),
        };
        let form = build_with(
            ui,
            &doc,
            &catalog,
            &factories,
            &binder,
            BuildOptions::default(),
        )
        .expect("the form builds");
        LoginApp { _form: form }
    })
    .expect("the event loop runs");
}
