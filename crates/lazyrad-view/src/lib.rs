#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! The renderer-neutral core of LazyRAD's responsive, state-bound UI layer.
//!
//! A view is a tree of named [`Node`]s. Their properties come from literals,
//! from per-[`SizeClass`] responsive patches and from Rhai expressions over the
//! form's `state` and the [`Environment`]. A [`Session`] evaluates all three
//! and turns every change into [`Patch`]es that any [`Renderer`] (a widget
//! toolkit, the DOM, a test recorder) applies.
//!
//! ```text
//!   Tree (nodes, bind, responsive)      state: Map     Environment
//!              └───────────────┬───────────────┴────────────┘
//!                          Session::build / update
//!                              │  Bindings: Rhai expressions, no side effects
//!                              ▼
//!                     Vec<Patch> + Vec<BindDiagnostic>
//!                              │
//!                      Renderer::apply
//! ```
//!
//! # Design rules
//!
//! * **No platform.** The crate depends on `rhai` and `thiserror` only: no
//!   toolkit, no form model, no file, thread, clock or network access (a test
//!   scans the sources for them). It builds for `wasm32-unknown-unknown`, and
//!   for the static musl target LazyOS uses.
//! * **Pure, sealed, bounded expressions.** A binding sees `state`, `props`
//!   and `env` and nothing else; it cannot assign, loop, sleep, exit, print or
//!   evaluate text; and its operations, nesting, memory and the amount of state
//!   it can see are limited, each ending in a diagnostic and never an abort
//!   (see [`bindings`]).
//! * **Failure is local.** A bad binding keeps its last good value and reports
//!   a [`BindDiagnostic`]; nothing panics and the rest of the tree is updated.
//! * **Responsive means mobile-first**, like CSS `min-width` queries
//!   (see [`Responsive`]).
//! * **Typed values, no coercion.** `1` and `1.0` are different values, and a
//!   property keeps one kind.
//!
//! # Example
//!
//! ```
//! use std::rc::Rc;
//! use lazyrad_view::{
//!     Bindings, Environment, Node, RecordingRenderer, Renderer, Session, SizeClass, Tree, Value,
//! };
//!
//! let tree = Tree::new(
//!     Node::new("root", "Flex")
//!         .with_prop("direction", Value::variant("Vertical"))
//!         .with_responsive(SizeClass::Medium, "direction", Value::variant("Horizontal"))
//!         .with_child(Node::new("count_label", "Label").with_bind("text", "`Clicked ${state.count}`")),
//! )
//! .unwrap();
//! let mut session = Session::new(tree, Bindings::new());
//! let mut renderer = RecordingRenderer::new();
//!
//! let mut state = rhai::Map::new();
//! state.insert("count".into(), 0_i64.into());
//! let phone = Environment::new(360.0, 640.0);
//! renderer.apply(&session.build(&state, &phone).patches);
//!
//! state.insert("count".into(), 1_i64.into());
//! session.update_with(&state, &Environment::new(700.0, 640.0), &mut renderer);
//! assert_eq!(
//!     renderer.trace(),
//!     "create root Flex parent=- index=0 {direction=Vertical}\n\
//!      create count_label Label parent=root index=0 {text=\"Clicked 0\"}\n\
//!      set root.direction = Horizontal\n\
//!      set count_label.text = \"Clicked 1\"\n",
//! );
//! ```

pub mod bindings;
mod cache;
mod dependencies;
mod environment;
mod error;
pub mod limits;
mod node;
mod patch;
mod renderer;
mod scope;
mod session;
mod size_class;
mod state_eq;
mod tree;
mod turn;
mod value;
mod visible;

pub use bindings::{BindingScope, BindingStats, Bindings, Dependencies, check_expression};
pub use environment::Environment;
pub use error::{BindError, SourcePosition};
pub use node::{MAX_TREE_DEPTH, Node};
pub use patch::{Patch, trace};
pub use renderer::{RecordingRenderer, Renderer};
pub use session::Session;
pub use size_class::{Responsive, SizeClass};
pub use tree::{Tree, TreeError};
pub use turn::{BindDiagnostic, TurnResult};
pub use value::{Value, ValueError, ValueKind};
