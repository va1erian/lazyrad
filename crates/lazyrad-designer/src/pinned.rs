#![forbid(unsafe_code)]

//! [`Pinned`]: one built-in xui widget created at a fixed pixel rectangle,
//! outside any layout the host owns.
//!
//! xui creates its built-in widgets only through layout builders, so a widget
//! that sits over a painted surface (the property grid's cell editors) is
//! mounted on its own in an [`absolute`] layout at that rectangle. The layout
//! keeps it there; dropping the [`Pinned`] destroys it.

use std::rc::Rc;

use xui_core::app::Ui;
use xui_core::arrange::{Build, Handle, LayoutExt, Mounted, absolute};
use xui_core::backend::Result;
use xui_core::geometry::Rect;
use xui_core::units::Px;
use xui_core::widget::Placeable;

/// A widget mounted alone at a pixel rectangle; dropping it destroys the
/// widget.
pub(crate) struct Pinned<W: 'static, M: 'static> {
    widget: Rc<W>,
    _layout: Mounted<M>,
}

impl<W: Placeable<M> + 'static, M: 'static> Pinned<W, M> {
    /// Creates `build`'s widget at `rect`, in device pixels relative to the
    /// container `ui` is scoped to (or to the window).
    pub(crate) fn new(ui: &Ui<M>, rect: Rect, build: Build<W, M>) -> Result<Pinned<W, M>> {
        let dpi = ui.dpi();
        let dip = |px: i32| Px(px).to_dip(dpi);
        let handle = Handle::new();
        let layout = ui.mount(absolute().child(build.bind(&handle).at(
            dip(rect.left),
            dip(rect.top),
            dip(rect.width()),
            dip(rect.height()),
        )))?;
        Ok(Pinned {
            widget: handle.get(),
            _layout: layout,
        })
    }

    /// The widget.
    pub(crate) fn widget(&self) -> &W {
        &self.widget
    }
}
