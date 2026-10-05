//! Shared harness for the integration tests.
//!
//! The only public way to obtain an `xui` [`Ui`] is through [`run_app`], so the
//! tests build the form from inside its `make` closure and run their assertions
//! there. The [`OffscreenBackend`] renders headlessly, so this works on CI.

use std::cell::RefCell;
use std::rc::Rc;

use xui_canvas::OffscreenBackend;
use xui_core::app::{App, run_app};
use xui_core::arrange::{Handle, LayoutExt, absolute, panel};
use xui_core::backend::{Backend, Event, PlatformSpec};
use xui_core::geometry::{Rect, Size};
use xui_core::message::{Modifiers, MouseButton};
use xui_core::units::Dip;
use xui_form::{
    Binder, BuildOptions, Catalog, EventHandler, EventRef, Factories, FormDoc, LiveForm, Value,
    build_with,
};

/// A message a test binder maps an event to.
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    /// A click on the named node.
    Click(String),
    /// A toggle with its new state.
    Toggle(bool),
    /// A selection with its index.
    Select(i64),
    /// A text change with its new text.
    Change(String),
}

/// A binder that maps every built-in event to a [`Msg`].
pub struct TestBinder;

impl Binder<Msg> for TestBinder {
    fn bind(&self, event: EventRef<'_>) -> Option<EventHandler<Msg>> {
        let node = event.node.to_owned();
        match event.event {
            "Click" => Some(Rc::new(move |_| Some(Msg::Click(node.clone())))),
            "Toggle" => Some(Rc::new(move |args| {
                args.first().and_then(Value::as_bool).map(Msg::Toggle)
            })),
            "Select" | "Activate" => Some(Rc::new(move |args| {
                args.first().and_then(Value::as_int).map(Msg::Select)
            })),
            "Change" | "Commit" => Some(Rc::new(move |args| {
                args.first()
                    .and_then(Value::as_str)
                    .map(|text| Msg::Change(text.to_owned()))
            })),
            _ => None,
        }
    }
}

/// An app that records the messages it receives.
pub struct TestApp {
    /// Every message, in order.
    pub messages: Rc<RefCell<Vec<Msg>>>,
}

impl App for TestApp {
    type Msg = Msg;

    fn update(&mut self, msg: Msg, _ui: &mut xui_core::Ui<Msg>) {
        self.messages.borrow_mut().push(msg);
    }
}

/// Builds `doc` offscreen and runs `check` against the live form, returning its
/// result.
pub fn with_form<R>(
    doc: &FormDoc,
    catalog: &Catalog,
    options: BuildOptions,
    check: impl FnOnce(&LiveForm<Msg>) -> R,
) -> R {
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let factories: Factories<Msg> = Factories::xui();
    let binder = TestBinder;
    let messages = Rc::new(RefCell::new(Vec::new()));
    let slot: Rc<RefCell<Option<R>>> = Rc::new(RefCell::new(None));
    let slot_inner = Rc::clone(&slot);
    let log = Rc::clone(&messages);
    let spec = PlatformSpec::new("xui-form test").size(Dip(320.0), Dip(200.0));
    run_app(backend, spec, move |ui| {
        let form = build_with(ui, doc, catalog, &factories, &binder, options).expect("form builds");
        *slot_inner.borrow_mut() = Some(check(&form));
        TestApp {
            messages: Rc::clone(&log),
        }
    })
    .expect("run_app succeeds");
    slot.borrow_mut().take().expect("the closure ran")
}

/// Builds `doc` offscreen inside a 320x200 container node and runs `check`
/// against the live form, with a function that resizes the container (the
/// form re-anchors to it), returning `check`'s result.
pub fn with_form_in_container<R>(
    doc: &FormDoc,
    catalog: &Catalog,
    options: BuildOptions,
    check: impl FnOnce(&LiveForm<Msg>, &dyn Fn(Size)) -> R,
) -> R {
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let factories: Factories<Msg> = Factories::xui();
    let binder = TestBinder;
    let slot: Rc<RefCell<Option<R>>> = Rc::new(RefCell::new(None));
    let slot_inner = Rc::clone(&slot);
    let spec = PlatformSpec::new("xui-form test").size(Dip(400.0), Dip(300.0));
    run_app(backend, spec, move |ui| {
        let host = Handle::new();
        let mounted = ui
            .mount(absolute().child(panel(absolute()).plain().bind(&host).at(0, 0, 320, 200)))
            .expect("the container mounts");
        let container = host.get().id();
        let options = BuildOptions {
            container: Some(container),
            ..options
        };
        let form = build_with(ui, doc, catalog, &factories, &binder, options).expect("form builds");
        let resize = |size: Size| ui.apply_moves(&[(container, Rect::from_size(size))]);
        *slot_inner.borrow_mut() = Some(check(&form, &resize));
        drop(form);
        drop(mounted);
        TestApp {
            messages: Rc::new(RefCell::new(Vec::new())),
        }
    })
    .expect("run_app succeeds");
    slot.borrow_mut().take().expect("the closure ran")
}

/// Builds `doc` offscreen, simulates a left click on `node_name`, and returns
/// the messages the app received.
pub fn click_node(
    doc: &FormDoc,
    catalog: &Catalog,
    options: BuildOptions,
    node_name: &str,
) -> Vec<Msg> {
    let backend = Rc::new(OffscreenBackend::new());
    let factories: Factories<Msg> = Factories::xui();
    let binder = TestBinder;
    let messages = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::clone(&messages);
    let backend_for_click = Rc::clone(&backend);
    let node_name = node_name.to_owned();
    let spec = PlatformSpec::new("xui-form click").size(Dip(320.0), Dip(200.0));
    run_app(backend as Rc<dyn Backend>, spec, move |ui| {
        let form = build_with(ui, doc, catalog, &factories, &binder, options).expect("form builds");
        if let Some(id) = form.widget(&node_name).map(|widget| widget.id()) {
            let bounds = ui.bounds(id);
            let window = ui.window();
            let modifiers = Modifiers::NONE;
            let x = bounds.left + bounds.width() / 2;
            let y = bounds.top + bounds.height() / 2;
            let _ = backend_for_click.inject(
                window,
                Event::MouseDown {
                    x,
                    y,
                    button: MouseButton::Left,
                    modifiers,
                },
            );
            let _ = backend_for_click.inject(
                window,
                Event::MouseMove {
                    x: x + 1,
                    y,
                    modifiers,
                },
            );
            let _ = backend_for_click.inject(
                window,
                Event::MouseUp {
                    x,
                    y,
                    button: MouseButton::Left,
                    modifiers,
                },
            );
        }
        TestApp {
            messages: Rc::clone(&log),
        }
    })
    .expect("run_app succeeds");
    messages.borrow().clone()
}

/// A catalog with some aliases, for the alias tests.
pub fn aliased_catalog() -> Catalog {
    let mut catalog = Catalog::xui();
    catalog.alias("CommandButton", "Button");
    catalog.alias("TextBox", "Edit");
    catalog.alias("Frame", "GroupBox");
    catalog.alias("ListBox", "ListView");
    catalog.alias("OptionButton", "RadioGroup");
    catalog
}
