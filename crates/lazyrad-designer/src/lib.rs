#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! The LazyRAD form-designer surface.
//!
//! It renders the form's real xui widgets in design mode and lays a transparent
//! `Custom` overlay on top for the dot grid, selection handles and mouse
//! editing. The model lives in [`lazyrad_project`] and the property grid sits
//! alongside the surface. See PLAN.md §6.
//!
//! # Layout
//!
//! * [`Surface`] is the pure, xui-free engine: it owns the [`FormDoc`], the
//!   selection, the in-process clipboard, the undo/redo [`History`] and the live
//!   gesture. Everything is in design units, so it is unit-tested without a
//!   window.
//! * [`Designer`] is the xui widget: a [`Panel`](xui_core::widget::Panel)
//!   holding the live widgets and a transparent overlay node that receives input
//!   and paints.
//! * [`Toolbox`] is a [`Custom`](xui_core::backend::NodeKind) icon grid of the
//!   control kinds. It maps a click to [`ToolboxMsg::Select`] and a
//!   double-click to [`ToolboxMsg::Activate`]; the host forwards those to
//!   [`Designer::handle_toolbox`], which arms the click-then-drag tool or drops
//!   a control in the centre of the form. Every creation is one undoable
//!   [`Surface`] command.
//! * [`PropertyGrid`] is a [`Custom`](xui_core::backend::NodeKind) two-column
//!   name/value list driven by the [`Catalog`](xui_form::Catalog) schema. It
//!   follows the designer's selection, overlays a typed editor on the cell
//!   being edited, and commits each edit as an undoable
//!   [`Surface::set_property`] command. A rename raises the host's rename sink
//!   with the old and new names, and [`rename_handlers`] rewrites the form's
//!   `.rhai` handlers.
//!
//! # Wiring a designer into a host
//!
//! The overlay's event mapper only turns input into a [`DesignerMsg`] and wraps
//! it with the closure given to [`Designer::new`]. The host adds a variant for
//! that message to its own message enum and forwards it in `App::update`:
//!
//! ```rust,no_run
//! # use std::rc::Rc;
//! # use xui_core::app::{App, Ui};
//! # use xui_core::Rect;
//! # use xui_form::FormDoc;
//! # use lazyrad_designer::{Designer, DesignerMsg};
//! #[derive(Clone)]
//! enum Msg {
//!     Designer(DesignerMsg),
//! }
//!
//! struct Editor {
//!     designer: Designer<Msg>,
//! }
//!
//! impl App for Editor {
//!     type Msg = Msg;
//!     fn update(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
//!         match msg {
//!             Msg::Designer(msg) => self.designer.update(msg, ui),
//!         }
//!     }
//! }
//!
//! # fn build(ui: &mut Ui<Msg>) -> Editor {
//! let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
//! let doc = FormDoc::new("main_form");
//! let designer = Designer::new(ui, Rect::default(), doc, catalog, Msg::Designer)
//!     .expect("the designer builds");
//! Editor { designer }
//! # }
//! ```
//!
//! # Design mode is per window
//!
//! [`Ui::set_design_mode`](xui_core::app::Ui::set_design_mode) is a flag on the
//! window, not on a container or a `Ui` handle, so a [`Designer`] puts the whole
//! window into design mode. A host that wants a designer *and* live widgets in
//! one window (the IDE) needs the designer in its own child window, or an xui
//! change that scopes design mode to a subtree. The designer itself is
//! unaffected: the overlay receives input either way, and
//! [`BuildOptions::design_mode`](xui_form::BuildOptions) already leaves the
//! preview unwired from the host's events.

pub mod geometry;
pub mod history;
pub mod property_grid;
pub mod surface;
pub mod toolbox;
pub mod widget;

pub use geometry::{DesignRect, Handle, handle_at, resize, resize_form, snap};
pub use history::History;
pub use property_grid::{
    PropertyGrid, PropertyGridError, PropertyGridMsg, PropertyRow, View, property_rows,
};
pub use surface::{
    Change, CursorHint, DEFAULT_GRID, HANDLE_TOLERANCE, KeyInput, KeyPress, Outcome, PropertyError,
    Selection, Surface, Target, control_base_name, handler_events, is_valid_name, rename_handlers,
    snake_case,
};
pub use toolbox::{CONTROL_KINDS, Tool, Toolbox, ToolboxMsg, tools};
pub use widget::{Designer, DesignerError, DesignerMsg};
