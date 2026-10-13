//! The renderer seam: `update_with`, live values and callbacks.

mod common;

use std::rc::Rc;

use common::{int, map, phone, text, tree};
use lazyrad_view::{Bindings, Node, Patch, RecordingRenderer, Renderer, Session, Value};

fn set(node: &str, property: &str, value: impl Into<Value>) -> Patch {
    Patch::Set {
        node: node.into(),
        property: property.into(),
        value: value.into(),
    }
}

/// A renderer that calls back into the `Bindings` its session shares, from
/// both `live_value` and `apply`. The session must hold no borrow of that
/// shared state while it calls out.
struct Reentrant {
    bindings: Rc<Bindings>,
    inner: RecordingRenderer,
    calls: usize,
}

impl Renderer for Reentrant {
    fn apply(&mut self, patches: &[Patch]) {
        assert!(self.bindings.check_expression("state.n + 1").is_ok());
        assert!(self.bindings.check_expression("state.n +").is_err());
        let _ = self.bindings.stats();
        self.calls += 1;
        self.inner.apply(patches);
    }

    fn live_value(&self, node: &str, property: &str) -> Option<Value> {
        assert!(self.bindings.check_expression("env.width").is_ok());
        self.inner.live_value(node, property)
    }
}

#[test]
fn a_renderer_may_call_back_into_shared_state_during_apply() {
    let root = Node::new("root", "Column")
        .with_child(Node::new("count", "Label").with_bind("text", "`n=${state.n}`"));
    let bindings = Rc::new(Bindings::new());
    let mut session = Session::new(tree(root), Rc::clone(&bindings));
    let mut renderer = Reentrant {
        bindings: Rc::clone(&bindings),
        inner: RecordingRenderer::new(),
        calls: 0,
    };
    let state = map(&[("n", int(0))]);
    renderer.apply(&session.build(&state, &phone()).patches);
    let next = map(&[("n", int(1))]);
    session.update_with(&next, &phone(), &mut renderer);
    assert_eq!(renderer.calls, 2);
    // An update with nothing to apply does not call `apply` at all.
    session.update_with(&next, &phone(), &mut renderer);
    assert_eq!(renderer.calls, 2);
}

#[test]
fn update_with_skips_a_set_the_widget_already_shows() {
    let root = Node::new("name_edit", "Edit").with_bind("text", "state.name");
    let mut session = Session::new(tree(root), Bindings::new());
    let mut renderer = RecordingRenderer::new();
    renderer.apply(&session.build(&map(&[("name", text(""))]), &phone()).patches);
    renderer.take_trace();

    // The user types "Ad": the widget already shows it when the handler has
    // copied it into state and the binding sends it back.
    renderer.set_live("name_edit", "text", Value::Text("Ad".into()));
    let result = session.update_with(&map(&[("name", text("Ad"))]), &phone(), &mut renderer);
    assert!(result.patches.is_empty(), "{:?}", result.patches);
    assert_eq!(renderer.take_trace(), "");
    // The session counts the value as applied.
    assert_eq!(
        session.value("name_edit", "text"),
        Some(&Value::Text("Ad".into()))
    );

    // A state change the widget does not show yet is applied.
    let result = session.update_with(&map(&[("name", text("Ada"))]), &phone(), &mut renderer);
    assert_eq!(result.patches, [set("name_edit", "text", "Ada")]);
    assert_eq!(renderer.take_trace(), "set name_edit.text = \"Ada\"\n");
}

#[test]
fn a_renderer_that_cannot_tell_gets_every_set() {
    /// The default `live_value`: unknown.
    struct Blind(Vec<Patch>);
    impl Renderer for Blind {
        fn apply(&mut self, patches: &[Patch]) {
            self.0.extend_from_slice(patches);
        }
    }

    let root = Node::new("edit", "Edit").with_bind("text", "state.name");
    let mut session = Session::new(tree(root), Bindings::new());
    let mut renderer = Blind(Vec::new());
    session.build(&map(&[("name", text("a"))]), &phone());
    session.update_with(&map(&[("name", text("b"))]), &phone(), &mut renderer);
    assert_eq!(renderer.0, [set("edit", "text", "b")]);
}
