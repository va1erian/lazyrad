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

use lazyrad_designer::{Designer, DesignerMsg, Selection};

/// A handle the test keeps to the designer after `run_app` returns.
type DesignerSlot = Rc<RefCell<Option<Rc<RefCell<Designer<Msg>>>>>>;

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

/// A form with one button, for the interaction tests.
fn button_doc() -> FormDoc {
    let mut doc = FormDoc::new("frmMain");
    let mut button = Node::new("Button", "cmdOk");
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
        doc.node("cmdOk").map(|node| node.kind.as_str()),
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
    assert_eq!(selection, Selection::Nodes(vec!["cmdOk".to_owned()]));
}

#[test]
fn dragging_a_control_moves_it_and_snaps_to_the_grid() {
    let moved = with_designer(
        button_doc(),
        |backend, window| drag(backend, window, (40, 20), (50, 30)),
        |designer| {
            let node = designer.doc();
            (
                node.node("cmdOk").and_then(|n| n.prop("left").cloned()),
                node.node("cmdOk").and_then(|n| n.prop("top").cloned()),
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
        Some(&Selection::Nodes(vec!["cmdOk".to_owned()]))
    );
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
                .node("cmdOk")
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
        let mut invalid = FormDoc::new("frmBroken");
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
