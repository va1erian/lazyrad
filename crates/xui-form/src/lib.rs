#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Declarative forms for [xui](https://github.com/va1erian/xui).
//!
//! This crate adds a form layer on top of the portable `xui-core` widgets:
//!
//! * a **schema** ([`Catalog`], [`WidgetSpec`]) that describes each widget kind,
//!   its typed properties and the events it raises;
//! * a **document** ([`FormDoc`]) that stores a form as plain TOML and round
//!   trips byte-identically;
//! * **validation** ([`FormDoc::validate`]) that reports every problem as a
//!   [`Diagnostic`];
//! * a **builder** ([`build`]) that turns a document into live `xui` widgets,
//!   placed in an `absolute()` layout by their design rectangles and anchors,
//!   mapping events through a host-supplied [`Binder`];
//! * **methods** ([`MethodSpec`], [`LiveForm::call`]) a script calls on a
//!   widget, such as the [`canvas`] control's drawing calls or the
//!   [`picture`] box's zoom and rotation.
//!
//! # Upstream intent
//!
//! The crate is designed to be moved into the xui repository unchanged. It
//! depends only on `xui-core`, `serde` and `toml`, and knows nothing about any
//! particular application. A consumer registers its own widget kinds or aliases
//! a built-in one (see [`Catalog::alias`]) to expose application-specific
//! names.
//!
//! # Example
//!
//! ```
//! use xui_form::{Catalog, FormDoc, Value};
//!
//! let catalog = Catalog::xui();
//! let doc = FormDoc::from_toml(
//!     r#"
//! format = 1
//!
//! [window]
//! name = "main_form"
//! title = "Hello"
//!
//! [[node]]
//! kind = "Button"
//! name = "cmdGo"
//! text = "Go"
//! "#,
//!     &catalog,
//! )
//! .expect("the form loads");
//!
//! assert_eq!(doc.node("cmdGo").and_then(|n| n.prop("text")), Some(&Value::Text("Go".to_owned())));
//! assert_eq!(doc.to_toml(&catalog).contains("title = \"Hello\""), true);
//! ```

pub mod build;
pub mod canvas;
pub mod doc;
pub mod picture;
pub mod schema;
pub mod validate;
pub mod value;

mod live;
mod lucide_names;
mod placement;
mod xui_factories;

/// The current document format; loading anything newer is an error.
pub const FORMAT_VERSION: u32 = 1;

pub use build::{
    Binder, BuildCx, BuildError, BuildOptions, CallError, EventHandler, EventRef, Factories,
    LiveForm, LiveWidget, Made, SetError, WidgetFactory, build, build_with,
};
pub use doc::{FormDoc, LoadError, Node, WindowNode};
pub use schema::{
    Access, ArgSpec, Catalog, Children, EventSpec, MethodSpec, PropertySpec, WidgetSpec,
};
pub use validate::{Diagnostic, Severity};
pub use value::{DecodeError, Value, ValueType};
