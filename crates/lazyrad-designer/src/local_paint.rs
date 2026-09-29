#![forbid(unsafe_code)]

//! Painting in a node's own coordinates.
//!
//! xui's [`Canvas`] documents node-local coordinates, but its software
//! compositor (offscreen and windowed alike) hands a painter window
//! coordinates: [`Canvas::bounds`] is the node's absolute rectangle, with no
//! translation or clip applied. [`paint_local`] translates by that origin and
//! clips to the node, which is a no-op on a canvas that honours the contract,
//! since its bounds already sit at the origin.

use xui_core::backend::Canvas;
use xui_core::geometry::Rect;

/// Runs `paint` with the canvas origin at the node's top-left corner and its
/// drawing clipped to the node, passing the node's area at the origin.
///
/// Inside `paint`, draw within the given area; [`Canvas::bounds`] and
/// [`Canvas::clear`] still use the untranslated rectangle.
pub(crate) fn paint_local(canvas: &mut dyn Canvas, paint: impl FnOnce(&mut dyn Canvas, Rect)) {
    let bounds = canvas.bounds();
    let area = Rect::from_size(bounds.size());
    canvas.save();
    canvas.set_translation(bounds.left as f32, bounds.top as f32);
    canvas.push_clip(area);
    paint(canvas, area);
    canvas.pop_clip();
    canvas.restore();
}
