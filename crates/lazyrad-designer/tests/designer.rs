//! End-to-end tests for the [`Designer`] widget on the offscreen backend.
//!
//! The only public way to obtain an xui `Ui` is through `run_app`, so the tests
//! build the designer inside its `make` closure. Pointer events are either
//! delivered through [`OffscreenBackend::inject`] (which exercises the overlay's
//! hit-testing and event mapper) or passed straight to [`Designer::update`] (for
//! the synchronous commands such as undo).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use xui_canvas::OffscreenBackend;
use xui_core::app::{App, Ui, run_app};
use xui_core::backend::{Backend, Event, PlatformSpec, WindowId};
use xui_core::geometry::Rect;
use xui_core::message::{Modifiers, MouseButton};
use xui_core::units::Dip;
use xui_form::{FormDoc, Node, Value};

use lazyrad_designer::{
    CONTROL_KINDS, Designer, DesignerMsg, Selection, Target, Toolbox, ToolboxMsg,
};

/// A handle the test keeps to the designer after `run_app` returns.
type DesignerSlot = Rc<RefCell<Option<Rc<RefCell<Designer<Msg>>>>>>;

/// A handle the [`Shell`] tests keep to their designer after `run_app` returns.
type ShellSlot = Rc<RefCell<Option<Rc<RefCell<Designer<Shell>>>>>>;

/// The host's message type: the designer routes its input through this.
#[derive(Clone, Debug)]
enum Msg {
    /// A designer input message.
    Designer(DesignerMsg),
}

/// An app that owns the designer and forwards designer messages to it.
struct Editor {
    designer: Rc<RefCell<Designer<Msg>>>,
}

impl App for Editor {
    type Msg = Msg;

    fn update(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
        match msg {
            Msg::Designer(msg) => self.designer.borrow().update(msg, ui),
        }
    }
}

/// A message type carrying both the designer's and the toolbox's input, for the
/// tests that host both widgets in one window.
#[derive(Clone, Debug)]
enum Shell {
    /// A designer input message.
    Designer(DesignerMsg),
    /// A toolbox message.
    Toolbox(ToolboxMsg),
}

/// An app owning both a designer and a toolbox, forwarding each message.
struct ShellApp {
    designer: Rc<RefCell<Designer<Shell>>>,
    /// Kept alive so the toolbox's node survives event dispatch.
    _toolbox: Toolbox<Shell>,
}

impl App for ShellApp {
    type Msg = Shell;

    fn update(&mut self, msg: Shell, ui: &mut Ui<Shell>) {
        match msg {
            Shell::Designer(msg) => self.designer.borrow().update(msg, ui),
            Shell::Toolbox(msg) => self.designer.borrow().handle_toolbox(msg, ui),
        }
    }
}

/// A form with one button, for the interaction tests.
fn button_doc() -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    let mut button = Node::new("Button", "ok_button");
    button.set_prop("left", Value::Int(16));
    button.set_prop("top", Value::Int(16));
    button.set_prop("width", Value::Int(80));
    button.set_prop("height", Value::Int(24));
    doc.insert(button);
    doc
}

/// Builds the designer offscreen, runs `check` against it, and returns its
/// result. The designer is available to `check` after `run_app` has drained the
/// messages.
fn with_designer<R>(
    doc: FormDoc,
    inject: impl FnOnce(&OffscreenBackend, WindowId),
    check: impl FnOnce(&Designer<Msg>) -> R,
) -> R {
    let backend = Rc::new(OffscreenBackend::new());
    let trait_backend: Rc<dyn Backend> = Rc::clone(&backend) as Rc<dyn Backend>;
    let designer_slot: DesignerSlot = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&designer_slot);
    let backend_for_inject = Rc::clone(&backend);
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let spec = PlatformSpec::new("designer").size(Dip(320.0), Dip(200.0));

    run_app(trait_backend, spec, move |ui| {
        let designer = Designer::new(ui, Rect::new(0, 0, 320, 200), doc, catalog, Msg::Designer)
            .expect("the designer builds");
        let designer = Rc::new(RefCell::new(designer));
        inject(&backend_for_inject, ui.window());
        *slot.borrow_mut() = Some(Rc::clone(&designer));
        Editor { designer }
    })
    .expect("run_app succeeds");

    let slot = designer_slot.borrow();
    let designer = slot.as_ref().expect("the designer was built");
    check(&designer.borrow())
}

/// Sends a left-drag from `(from)` to `(to)` in overlay-local device pixels.
fn drag(backend: &OffscreenBackend, window: WindowId, from: (i32, i32), to: (i32, i32)) {
    let modifiers = Modifiers::NONE;
    backend.inject(
        window,
        Event::MouseDown {
            x: from.0,
            y: from.1,
            button: MouseButton::Left,
            modifiers,
        },
    );
    backend.inject(
        window,
        Event::MouseMove {
            x: to.0,
            y: to.1,
            modifiers,
        },
    );
    backend.inject(
        window,
        Event::MouseUp {
            x: to.0,
            y: to.1,
            button: MouseButton::Left,
            modifiers,
        },
    );
}

#[test]
fn the_designer_builds_the_form_and_turns_on_design_mode() {
    let documents = |designer: &Designer<Msg>| (designer.design_mode(), designer.doc());
    let (design_mode, doc) = with_designer(button_doc(), |_, _| {}, documents);
    assert!(design_mode, "the designer runs the window in design mode");
    assert_eq!(doc.nodes.len(), 1);
    assert_eq!(
        doc.node("ok_button").map(|node| node.kind.as_str()),
        Some("Button")
    );
}

#[test]
fn clicking_a_control_selects_it_rather_than_firing_an_event() {
    let selection = with_designer(
        button_doc(),
        |backend, window| drag(backend, window, (40, 20), (40, 20)),
        |designer| designer.selection(),
    );
    assert_eq!(selection, Selection::Nodes(vec!["ok_button".to_owned()]));
}

#[test]
fn dragging_a_control_moves_it_and_snaps_to_the_grid() {
    let moved = with_designer(
        button_doc(),
        |backend, window| drag(backend, window, (40, 20), (50, 30)),
        |designer| {
            let node = designer.doc();
            (
                node.node("ok_button").and_then(|n| n.prop("left").cloned()),
                node.node("ok_button").and_then(|n| n.prop("top").cloned()),
            )
        },
    );
    // A (10, 10) drag snaps to (8, 8), so 16 -> 24 and 16 -> 24.
    assert_eq!(moved.0, Some(Value::Int(24)));
    assert_eq!(moved.1, Some(Value::Int(24)));
}

#[test]
fn the_selection_sink_is_notified() {
    let backend = Rc::new(OffscreenBackend::new());
    let trait_backend: Rc<dyn Backend> = Rc::clone(&backend) as Rc<dyn Backend>;
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let log: Rc<RefCell<Vec<Selection>>> = Rc::new(RefCell::new(Vec::new()));
    let log_for_sink = Rc::clone(&log);
    let spec = PlatformSpec::new("designer").size(Dip(320.0), Dip(200.0));

    run_app(trait_backend, spec, move |ui| {
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            button_doc(),
            catalog,
            Msg::Designer,
        )
        .expect("the designer builds");
        designer.set_selection_sink(move |selection| {
            log_for_sink.borrow_mut().push(selection.clone());
        });
        let designer = Rc::new(RefCell::new(designer));

        // Select the button synchronously so the sink fires immediately.
        designer.borrow().update(
            DesignerMsg::PointerDown {
                x: 40,
                y: 20,
                ctrl: false,
            },
            ui,
        );
        Editor { designer }
    })
    .expect("run_app succeeds");

    assert_eq!(
        log.borrow().last(),
        Some(&Selection::Nodes(vec!["ok_button".to_owned()]))
    );
}

/// Builds a designer with a double-click sink, delivers a full double-click
/// (down, up, double-click, up) at each local device-pixel point, and returns
/// the targets the sink recorded.
fn double_click_at(doc: FormDoc, points: Vec<(i32, i32)>) -> Vec<Target> {
    let backend = Rc::new(OffscreenBackend::new());
    let trait_backend: Rc<dyn Backend> = Rc::clone(&backend) as Rc<dyn Backend>;
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let log: Rc<RefCell<Vec<Target>>> = Rc::new(RefCell::new(Vec::new()));
    let log_for_sink = Rc::clone(&log);
    let spec = PlatformSpec::new("designer").size(Dip(320.0), Dip(200.0));

    run_app(trait_backend, spec, move |ui| {
        let designer = Designer::new(ui, Rect::new(0, 0, 320, 200), doc, catalog, Msg::Designer)
            .expect("the designer builds");
        designer.set_double_click_sink(move |target| {
            log_for_sink.borrow_mut().push(target.clone());
        });
        let designer = Rc::new(RefCell::new(designer));
        let window = ui.window();
        for (x, y) in &points {
            let modifiers = Modifiers::NONE;
            for event in [
                Event::MouseDown {
                    x: *x,
                    y: *y,
                    button: MouseButton::Left,
                    modifiers,
                },
                Event::MouseUp {
                    x: *x,
                    y: *y,
                    button: MouseButton::Left,
                    modifiers,
                },
                Event::MouseDoubleClick {
                    x: *x,
                    y: *y,
                    button: MouseButton::Left,
                    modifiers,
                },
                Event::MouseUp {
                    x: *x,
                    y: *y,
                    button: MouseButton::Left,
                    modifiers,
                },
            ] {
                backend.inject(window, event);
            }
        }
        Editor { designer }
    })
    .expect("run_app succeeds");

    log.borrow().clone()
}

#[test]
fn double_clicking_a_control_reports_that_control() {
    let targets = double_click_at(button_doc(), vec![(40, 20)]);
    assert_eq!(targets, vec![Target::Node("ok_button".to_owned())]);
}

#[test]
fn double_clicking_empty_form_area_reports_the_form() {
    let targets = double_click_at(button_doc(), vec![(200, 150)]);
    assert_eq!(targets, vec![Target::Form]);
}

#[test]
fn a_double_click_does_not_move_the_control() {
    // The first click of the pair starts a press/release on the overlay; it must
    // not leave a half-finished drag or a moved control behind.
    let moved = with_designer(
        button_doc(),
        |backend, window| {
            let modifiers = Modifiers::NONE;
            for event in [
                Event::MouseDown {
                    x: 40,
                    y: 20,
                    button: MouseButton::Left,
                    modifiers,
                },
                Event::MouseUp {
                    x: 40,
                    y: 20,
                    button: MouseButton::Left,
                    modifiers,
                },
                Event::MouseDoubleClick {
                    x: 40,
                    y: 20,
                    button: MouseButton::Left,
                    modifiers,
                },
                Event::MouseUp {
                    x: 40,
                    y: 20,
                    button: MouseButton::Left,
                    modifiers,
                },
            ] {
                backend.inject(window, event);
            }
        },
        |designer| {
            let doc = designer.doc();
            let node = doc.node("ok_button").expect("the button survives");
            (
                node.prop("left").and_then(Value::as_int),
                node.prop("top").and_then(Value::as_int),
            )
        },
    );
    assert_eq!(moved, (Some(16), Some(16)), "the control stayed put");
}

#[test]
fn the_overlay_paints_without_panicking() {
    let backend = Rc::new(OffscreenBackend::new());
    let trait_backend: Rc<dyn Backend> = Rc::clone(&backend) as Rc<dyn Backend>;
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let rendered = Rc::new(Cell::new(false));
    let rendered_for_check = Rc::clone(&rendered);
    let spec = PlatformSpec::new("designer").size(Dip(320.0), Dip(200.0));

    run_app(trait_backend, spec, move |ui| {
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            button_doc(),
            catalog,
            Msg::Designer,
        )
        .expect("the designer builds");
        // Select a control so the selection outline and handles paint too.
        designer.update(
            DesignerMsg::PointerDown {
                x: 40,
                y: 20,
                ctrl: false,
            },
            ui,
        );
        let image = backend.render(ui.window()).expect("the window renders");
        rendered_for_check.set(image.width == 320 && image.height == 200);
        Editor {
            designer: Rc::new(RefCell::new(designer)),
        }
    })
    .expect("run_app succeeds");

    assert!(rendered.get(), "the overlay painted into the window");
}

#[test]
fn undo_and_redo_round_trip_a_move() {
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let observed: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
    let observed_for_check = Rc::clone(&observed);
    let spec = PlatformSpec::new("designer").size(Dip(320.0), Dip(200.0));

    run_app(backend, spec, move |ui| {
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            button_doc(),
            catalog,
            Msg::Designer,
        )
        .expect("the designer builds");
        designer.update(
            DesignerMsg::PointerDown {
                x: 40,
                y: 20,
                ctrl: false,
            },
            ui,
        );
        designer.update(
            DesignerMsg::PointerMove {
                x: 50,
                y: 30,
                ctrl: false,
            },
            ui,
        );
        designer.update(
            DesignerMsg::PointerUp {
                x: 50,
                y: 30,
                ctrl: false,
            },
            ui,
        );
        let left = |designer: &Designer<Msg>| {
            designer
                .doc()
                .node("ok_button")
                .and_then(|node| node.prop("left").and_then(Value::as_int))
                .unwrap_or_default()
        };
        let after_move = left(&designer);
        designer.undo(ui);
        let after_undo = left(&designer);
        designer.redo(ui);
        let after_redo = left(&designer);
        *observed_for_check.borrow_mut() = vec![after_move, after_undo, after_redo];
        Editor {
            designer: Rc::new(RefCell::new(designer)),
        }
    })
    .expect("run_app succeeds");

    assert_eq!(*observed.borrow(), vec![24, 16, 24]);
}

/// Builds a designer *and* a toolbox in one window, injects `input`, then runs
/// `check` against the designer. The toolbox sits to the right of the form, at
/// local `x = 340`, so it is never covered by the designer's overlay.
fn with_toolbox<R>(
    input: impl FnOnce(&OffscreenBackend, WindowId),
    check: impl FnOnce(&Designer<Shell>) -> R,
) -> R {
    let backend = Rc::new(OffscreenBackend::new());
    let trait_backend: Rc<dyn Backend> = Rc::clone(&backend) as Rc<dyn Backend>;
    let designer_slot: ShellSlot = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&designer_slot);
    let backend_for_inject = Rc::clone(&backend);
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let spec = PlatformSpec::new("designer").size(Dip(460.0), Dip(200.0));

    run_app(trait_backend, spec, move |ui| {
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            FormDoc::new("main_form"),
            catalog,
            Shell::Designer,
        )
        .expect("the designer builds");
        let designer = Rc::new(RefCell::new(designer));
        let toolbox = Toolbox::new(ui, Rect::new(340, 0, 436, 200), Shell::Toolbox)
            .expect("the toolbox builds");
        input(&backend_for_inject, ui.window());
        *slot.borrow_mut() = Some(Rc::clone(&designer));
        ShellApp {
            designer,
            _toolbox: toolbox,
        }
    })
    .expect("run_app succeeds");

    let slot = designer_slot.borrow();
    check(&slot.as_ref().expect("the designer was built").borrow())
}

/// Injects a left-button press (and release) at client `(x, y)`.
fn click(backend: &OffscreenBackend, window: WindowId, x: i32, y: i32) {
    let modifiers = Modifiers::NONE;
    backend.inject(
        window,
        Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            modifiers,
        },
    );
    backend.inject(
        window,
        Event::MouseUp {
            x,
            y,
            button: MouseButton::Left,
            modifiers,
        },
    );
}

/// Injects a left double-click at client `(x, y)`.
fn double_click(backend: &OffscreenBackend, window: WindowId, x: i32, y: i32) {
    backend.inject(
        window,
        Event::MouseDoubleClick {
            x,
            y,
            button: MouseButton::Left,
            modifiers: Modifiers::NONE,
        },
    );
}

#[test]
fn a_tool_drag_on_the_designer_creates_a_control() {
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let created: Rc<RefCell<Option<FormDoc>>> = Rc::new(RefCell::new(None));
    let created_for_check = Rc::clone(&created);
    let spec = PlatformSpec::new("designer").size(Dip(320.0), Dip(200.0));

    run_app(backend, spec, move |ui| {
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            FormDoc::new("main_form"),
            catalog,
            Msg::Designer,
        )
        .expect("the designer builds");
        designer.set_tool(Some("Button"), ui);
        assert_eq!(designer.tool().as_deref(), Some("Button"));
        designer.update(
            DesignerMsg::PointerDown {
                x: 10,
                y: 10,
                ctrl: false,
            },
            ui,
        );
        designer.update(
            DesignerMsg::PointerMove {
                x: 90,
                y: 50,
                ctrl: false,
            },
            ui,
        );
        designer.update(
            DesignerMsg::PointerUp {
                x: 90,
                y: 50,
                ctrl: false,
            },
            ui,
        );
        *created_for_check.borrow_mut() = Some(designer.doc());
        Editor {
            designer: Rc::new(RefCell::new(designer)),
        }
    })
    .expect("run_app succeeds");

    let created = created.borrow();
    let node = created
        .as_ref()
        .and_then(|doc| doc.node("button1"))
        .expect("the control was created");
    assert_eq!(node.kind, "Button");
    assert_eq!(node.prop("width"), Some(&Value::Int(80)));
    assert_eq!(node.prop("height"), Some(&Value::Int(40)));
}

#[test]
fn dropping_every_toolbox_kind_creates_it_and_undo_removes_it() {
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let created: Rc<RefCell<Option<FormDoc>>> = Rc::new(RefCell::new(None));
    let after_undo: Rc<RefCell<Option<FormDoc>>> = Rc::new(RefCell::new(None));
    let created_for_check = Rc::clone(&created);
    let after_undo_for_check = Rc::clone(&after_undo);
    let spec = PlatformSpec::new("designer").size(Dip(320.0), Dip(200.0));

    run_app(backend, spec, move |ui| {
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            FormDoc::new("main_form"),
            catalog,
            Msg::Designer,
        )
        .expect("the designer builds");
        for kind in CONTROL_KINDS {
            assert!(designer.drop_control(kind, ui).is_some(), "{kind} drops");
        }
        assert_eq!(designer.doc().nodes.len(), CONTROL_KINDS.len());
        *created_for_check.borrow_mut() = Some(designer.doc());
        assert!(designer.undo(ui));
        *after_undo_for_check.borrow_mut() = Some(designer.doc());
        Editor {
            designer: Rc::new(RefCell::new(designer)),
        }
    })
    .expect("run_app succeeds");

    let created = created.borrow();
    let names: Vec<&str> = created
        .as_ref()
        .expect("a doc")
        .nodes
        .iter()
        .map(|node| node.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "button1",
            "edit1",
            "label1",
            "check_box1",
            "radio_group1",
            "group_box1",
            "list_view1",
            "combo_box1",
            "canvas1"
        ]
    );
    let after_undo = after_undo.borrow();
    assert_eq!(
        after_undo.as_ref().expect("a doc").nodes.len(),
        CONTROL_KINDS.len() - 1,
        "undo removed the last control"
    );
}

#[test]
fn clicking_a_toolbox_tile_arms_the_tool() {
    let armed = with_toolbox(
        |backend, window| click(backend, window, 350, 40),
        |designer| designer.tool(),
    );
    assert_eq!(armed.as_deref(), Some("Button"));
}

#[test]
fn double_clicking_a_toolbox_tile_drops_a_control() {
    let doc = with_toolbox(
        |backend, window| double_click(backend, window, 350, 40),
        |designer| designer.doc(),
    );
    let node = doc.node("button1").expect("the control was dropped");
    // The Button's 100x28 default centred in a 320x200 form.
    assert_eq!(node.prop("left"), Some(&Value::Int(110)));
    assert_eq!(node.prop("top"), Some(&Value::Int(86)));
}

#[test]
fn the_toolbox_paints_without_panicking() {
    let backend = Rc::new(OffscreenBackend::new());
    let trait_backend: Rc<dyn Backend> = Rc::clone(&backend) as Rc<dyn Backend>;
    let rendered = Rc::new(Cell::new(false));
    let rendered_for_check = Rc::clone(&rendered);
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let spec = PlatformSpec::new("designer").size(Dip(460.0), Dip(200.0));

    run_app(trait_backend, spec, move |ui| {
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            FormDoc::new("main_form"),
            catalog,
            Shell::Designer,
        )
        .expect("the designer builds");
        let toolbox = Toolbox::new(ui, Rect::new(340, 0, 436, 200), Shell::Toolbox)
            .expect("the toolbox builds");
        let image = backend.render(ui.window()).expect("the window renders");
        rendered_for_check.set(image.width == 460 && image.height == 200);
        ShellApp {
            designer: Rc::new(RefCell::new(designer)),
            _toolbox: toolbox,
        }
    })
    .expect("run_app succeeds");

    assert!(rendered.get(), "the toolbox painted into the window");
}

#[test]
fn set_doc_keeps_the_current_form_when_the_new_one_is_invalid_and_resizes_the_panel() {
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let results: Rc<RefCell<Option<(bool, String, i32)>>> = Rc::new(RefCell::new(None));
    let results_for_check = Rc::clone(&results);
    let spec = PlatformSpec::new("designer").size(Dip(320.0), Dip(200.0));

    run_app(backend, spec, move |ui| {
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            button_doc(),
            catalog,
            Msg::Designer,
        )
        .expect("the designer builds");

        // An unknown kind is rejected, and the current form survives.
        let mut invalid = FormDoc::new("broken_form");
        invalid.insert(Node::new("NoSuchKind", "ghost"));
        let rejected = designer.set_doc(invalid, ui).is_err();
        let kept = designer.doc().window.name.clone();

        // A valid, wider form resizes the preview panel with the rebuild.
        let mut wider = button_doc();
        wider.window.set_prop("width", Value::Int(400));
        designer.set_doc(wider, ui).expect("a valid form loads");
        let panel_width = ui.bounds(designer.panel_id()).width();

        *results_for_check.borrow_mut() = Some((rejected, kept, panel_width));
        Editor {
            designer: Rc::new(RefCell::new(designer)),
        }
    })
    .expect("run_app succeeds");

    let (rejected, kept, panel_width) = results.borrow_mut().take().expect("the check ran");
    assert!(rejected, "an invalid document is refused");
    assert_eq!(
        kept,
        button_doc().window.name,
        "the current document is kept"
    );
    assert_eq!(panel_width, 400, "the panel follows the new form size");
}

#[test]
fn set_doc_rolls_back_when_a_valid_form_fails_to_build() {
    // A kind the catalog knows but no factory builds: it validates, then the
    // preview build fails with an unknown factory.
    let mut catalog = lazyrad_project::lazyrad_catalog();
    let mut fancy = catalog.get("Button").expect("Button spec").clone();
    fancy.kind = "FancyButton".to_owned();
    catalog.register(fancy);
    let catalog = Rc::new(catalog);

    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let results: Rc<RefCell<Option<(bool, String, bool)>>> = Rc::new(RefCell::new(None));
    let results_for_check = Rc::clone(&results);
    let spec = PlatformSpec::new("designer").size(Dip(320.0), Dip(200.0));

    run_app(backend, spec, move |ui| {
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            button_doc(),
            catalog,
            Msg::Designer,
        )
        .expect("the designer builds");
        // Make an undoable edit, so a reset history would show.
        designer.update(
            DesignerMsg::PointerDown {
                x: 40,
                y: 20,
                ctrl: false,
            },
            ui,
        );
        designer.update(
            DesignerMsg::PointerMove {
                x: 50,
                y: 30,
                ctrl: false,
            },
            ui,
        );
        designer.update(
            DesignerMsg::PointerUp {
                x: 50,
                y: 30,
                ctrl: false,
            },
            ui,
        );

        let mut unbuildable = FormDoc::new("fancy_form");
        unbuildable.insert(Node::new("FancyButton", "fancy1"));
        let failed = designer.set_doc(unbuildable, ui).is_err();
        let kept = designer.doc().window.name.clone();
        let can_undo = designer.can_undo();

        *results_for_check.borrow_mut() = Some((failed, kept, can_undo));
        Editor {
            designer: Rc::new(RefCell::new(designer)),
        }
    })
    .expect("run_app succeeds");

    let (failed, kept, can_undo) = results.borrow_mut().take().expect("the check ran");
    assert!(failed, "the build failure is reported");
    assert_eq!(
        kept,
        button_doc().window.name,
        "the current document is kept"
    );
    assert!(can_undo, "the undo history survives");
}

#[test]
fn a_failed_construction_restores_the_windows_design_mode() {
    let mut catalog = lazyrad_project::lazyrad_catalog();
    let mut fancy = catalog.get("Button").expect("Button spec").clone();
    fancy.kind = "FancyButton".to_owned();
    catalog.register(fancy);
    let catalog = Rc::new(catalog);

    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let results: Rc<Cell<Option<(bool, bool)>>> = Rc::new(Cell::new(None));
    let results_for_check = Rc::clone(&results);
    let spec = PlatformSpec::new("designer").size(Dip(320.0), Dip(200.0));

    run_app(backend, spec, move |ui| {
        let mut unbuildable = FormDoc::new("fancy_form");
        unbuildable.insert(Node::new("FancyButton", "fancy1"));
        let failed = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            unbuildable,
            Rc::clone(&catalog),
            Msg::Designer,
        )
        .is_err();
        results_for_check.set(Some((failed, ui.is_design_mode())));
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            button_doc(),
            catalog,
            Msg::Designer,
        )
        .expect("a valid form builds");
        Editor {
            designer: Rc::new(RefCell::new(designer)),
        }
    })
    .expect("run_app succeeds");

    let (failed, design_mode) = results.get().expect("the check ran");
    assert!(failed, "the unbuildable form is reported");
    assert!(!design_mode, "the window left design mode again");
}

#[test]
fn undoing_and_redoing_a_rename_reports_it_through_the_rename_sink() {
    let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
    let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
    let renames: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&renames);
    let spec = PlatformSpec::new("designer").size(Dip(320.0), Dip(200.0));

    run_app(backend, spec, move |ui| {
        let designer = Designer::new(
            ui,
            Rect::new(0, 0, 320, 200),
            button_doc(),
            catalog,
            Msg::Designer,
        )
        .expect("the designer builds");
        let sink = Rc::clone(&observed);
        designer.set_rename_sink(move |old, new| {
            sink.borrow_mut().push((old.to_owned(), new.to_owned()));
        });
        designer
            .set_property(
                &lazyrad_designer::Target::Node("ok_button".to_owned()),
                "name",
                Value::Text("go_button".into()),
                ui,
            )
            .expect("the rename is valid");
        assert!(designer.undo(ui), "the rename is undone");
        assert!(designer.redo(ui), "and redone");
        // A move is undone without reporting any rename.
        designer.update(
            DesignerMsg::PointerDown {
                x: 40,
                y: 20,
                ctrl: false,
            },
            ui,
        );
        designer.update(
            DesignerMsg::PointerMove {
                x: 60,
                y: 40,
                ctrl: false,
            },
            ui,
        );
        designer.update(
            DesignerMsg::PointerUp {
                x: 60,
                y: 40,
                ctrl: false,
            },
            ui,
        );
        assert!(designer.undo(ui), "the move is undone");
        Editor {
            designer: Rc::new(RefCell::new(designer)),
        }
    })
    .expect("run_app succeeds");

    let pair = |old: &str, new: &str| (old.to_owned(), new.to_owned());
    assert_eq!(
        *renames.borrow(),
        vec![
            pair("ok_button", "go_button"),
            pair("go_button", "ok_button"),
            pair("ok_button", "go_button"),
        ],
        "the rename, its undo and its redo each reach the host; the move's undo does not"
    );
}
