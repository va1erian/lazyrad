#![forbid(unsafe_code)]

//! Design-unit geometry and the eight resize handles.
//!
//! Everything the designer edits is stored in *design units* (DIPs, the unit of
//! [`FormDoc`](xui_form::FormDoc) properties): a node's `left`/`top` are
//! relative to its parent, and the window's `width`/`height` are its client
//! size. The overlay's device-pixel coordinates are converted to design units at
//! the event boundary, so all the hit-testing, snapping and editing here is
//! DPI-independent and unit-testable without an xui window.

/// A rectangle in design units, half-open on the right and bottom edges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DesignRect {
    /// The left edge.
    pub left: i64,
    /// The top edge.
    pub top: i64,
    /// The right (exclusive) edge.
    pub right: i64,
    /// The bottom (exclusive) edge.
    pub bottom: i64,
}

impl DesignRect {
    /// A rectangle from its edges.
    pub const fn new(left: i64, top: i64, right: i64, bottom: i64) -> DesignRect {
        DesignRect {
            left,
            top,
            right,
            bottom,
        }
    }

    /// A rectangle from an origin and a size.
    pub const fn from_size(left: i64, top: i64, width: i64, height: i64) -> DesignRect {
        DesignRect::new(left, top, left + width, top + height)
    }

    /// The rectangle spanned by two opposite corners, in any order.
    pub fn from_points(a: (i64, i64), b: (i64, i64)) -> DesignRect {
        DesignRect::new(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
    }

    /// The width in design units.
    pub const fn width(self) -> i64 {
        self.right - self.left
    }

    /// The height in design units.
    pub const fn height(self) -> i64 {
        self.bottom - self.top
    }

    /// Whether either dimension is non-positive.
    pub const fn is_empty(self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }

    /// Whether the point lies inside the half-open rectangle.
    pub const fn contains(self, x: i64, y: i64) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    /// Returns the rectangle moved by `(dx, dy)`.
    pub const fn offset(self, dx: i64, dy: i64) -> DesignRect {
        DesignRect::new(
            self.left + dx,
            self.top + dy,
            self.right + dx,
            self.bottom + dy,
        )
    }

    /// Whether the two half-open rectangles overlap with positive area.
    pub const fn intersects(self, other: DesignRect) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }

    /// The smallest rectangle containing both.
    pub fn union(self, other: DesignRect) -> DesignRect {
        DesignRect::new(
            self.left.min(other.left),
            self.top.min(other.top),
            self.right.max(other.right),
            self.bottom.max(other.bottom),
        )
    }

    /// The centre point, rounded towards zero.
    pub const fn center(self) -> (i64, i64) {
        (self.left + self.width() / 2, self.top + self.height() / 2)
    }
}

/// A resize handle around a selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// The top-left corner.
    NorthWest,
    /// The top edge.
    North,
    /// The top-right corner.
    NorthEast,
    /// The right edge.
    East,
    /// The bottom-right corner.
    SouthEast,
    /// The bottom edge.
    South,
    /// The bottom-left corner.
    SouthWest,
    /// The left edge.
    West,
}

impl Handle {
    /// The eight handles, corners first so a corner wins a tie in hit-testing.
    pub const ALL: [Handle; 8] = [
        Handle::NorthWest,
        Handle::NorthEast,
        Handle::SouthEast,
        Handle::SouthWest,
        Handle::North,
        Handle::East,
        Handle::South,
        Handle::West,
    ];

    /// The handle's centre on `rect`.
    pub const fn center(self, rect: DesignRect) -> (i64, i64) {
        let (cx, cy) = rect.center();
        match self {
            Handle::NorthWest => (rect.left, rect.top),
            Handle::North => (cx, rect.top),
            Handle::NorthEast => (rect.right, rect.top),
            Handle::East => (rect.right, cy),
            Handle::SouthEast => (rect.right, rect.bottom),
            Handle::South => (cx, rect.bottom),
            Handle::SouthWest => (rect.left, rect.bottom),
            Handle::West => (rect.left, cy),
        }
    }

    /// Whether dragging this handle moves the left edge.
    pub const fn moves_left(self) -> bool {
        matches!(self, Handle::NorthWest | Handle::SouthWest | Handle::West)
    }

    /// Whether dragging this handle moves the right edge.
    pub const fn moves_right(self) -> bool {
        matches!(self, Handle::NorthEast | Handle::SouthEast | Handle::East)
    }

    /// Whether dragging this handle moves the top edge.
    pub const fn moves_top(self) -> bool {
        matches!(self, Handle::NorthWest | Handle::NorthEast | Handle::North)
    }

    /// Whether dragging this handle moves the bottom edge.
    pub const fn moves_bottom(self) -> bool {
        matches!(self, Handle::SouthWest | Handle::SouthEast | Handle::South)
    }
}

/// The handle under `(x, y)`, if the point is within `tolerance` design units of
/// a handle centre.
pub fn handle_at(rect: DesignRect, x: i64, y: i64, tolerance: i64) -> Option<Handle> {
    Handle::ALL.into_iter().find(|handle| {
        let (cx, cy) = handle.center(rect);
        (x - cx).abs() <= tolerance && (y - cy).abs() <= tolerance
    })
}

/// Snaps `value` to the nearest multiple of `grid`; a `grid` of one (or less)
/// leaves the value alone.
pub fn snap(value: i64, grid: i64) -> i64 {
    if grid <= 1 {
        return value;
    }
    let grid = grid as f64;
    ((value as f64 / grid).round() * grid) as i64
}

/// Resizes `rect` by moving the edges `handle` names by `(dx, dy)`, keeping a
/// positive size and snapping the moved edges to `grid`.
pub fn resize(rect: DesignRect, handle: Handle, dx: i64, dy: i64, grid: i64) -> DesignRect {
    let mut left = rect.left;
    let mut top = rect.top;
    let mut right = rect.right;
    let mut bottom = rect.bottom;
    if handle.moves_left() {
        left = snap(rect.left + dx, grid);
    }
    if handle.moves_right() {
        right = snap(rect.right + dx, grid);
    }
    if handle.moves_top() {
        top = snap(rect.top + dy, grid);
    }
    if handle.moves_bottom() {
        bottom = snap(rect.bottom + dy, grid);
    }
    if left > right {
        std::mem::swap(&mut left, &mut right);
    }
    if top > bottom {
        std::mem::swap(&mut top, &mut bottom);
    }
    // Keep the edge the pointer is *not* moving pinned, so a too-small drag
    // shrinks against the opposite edge instead of crossing it.
    if right - left < 1 {
        if handle.moves_left() {
            left = right - 1;
        } else {
            right = left + 1;
        }
    }
    if bottom - top < 1 {
        if handle.moves_top() {
            top = bottom - 1;
        } else {
            bottom = top + 1;
        }
    }
    DesignRect::new(left, top, right, bottom)
}

/// Resizes the *form* rectangle (whose origin is fixed at `(0, 0)`) from the
/// pointer's absolute position `(x, y)`, snapping the moving edges to `grid`.
pub fn resize_form(rect: DesignRect, handle: Handle, x: i64, y: i64, grid: i64) -> DesignRect {
    let mut width = rect.width();
    let mut height = rect.height();
    let x = snap(x, grid);
    let y = snap(y, grid);
    if handle.moves_right() {
        width = x;
    } else if handle.moves_left() {
        width = rect.right - x;
    }
    if handle.moves_bottom() {
        height = y;
    } else if handle.moves_top() {
        height = rect.bottom - y;
    }
    DesignRect::new(0, 0, width.max(1), height.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_helpers_are_half_open() {
        let rect = DesignRect::new(0, 0, 10, 20);
        assert_eq!(rect.width(), 10);
        assert_eq!(rect.height(), 20);
        assert!(rect.contains(0, 0));
        assert!(rect.contains(9, 19));
        assert!(!rect.contains(10, 0));
        assert!(!rect.contains(0, 20));
        assert!(rect.offset(5, 5).contains(5, 5));
    }

    #[test]
    fn intersection_is_strict() {
        let a = DesignRect::new(0, 0, 10, 10);
        assert!(a.intersects(DesignRect::new(5, 5, 15, 15)));
        assert!(
            !a.intersects(DesignRect::new(10, 0, 20, 10)),
            "touching edges"
        );
        assert!(!a.intersects(DesignRect::new(11, 0, 20, 10)));
    }

    #[test]
    fn union_contains_both() {
        let a = DesignRect::new(0, 0, 10, 10);
        let b = DesignRect::new(20, 30, 25, 40);
        assert_eq!(a.union(b), DesignRect::new(0, 0, 25, 40));
    }

    #[test]
    fn corners_beat_edges_in_hit_testing() {
        let rect = DesignRect::new(0, 0, 100, 50);
        assert_eq!(handle_at(rect, 0, 0, 3), Some(Handle::NorthWest));
        assert_eq!(handle_at(rect, 100, 50, 3), Some(Handle::SouthEast));
        assert_eq!(handle_at(rect, 50, 0, 3), Some(Handle::North));
        assert_eq!(
            handle_at(rect, 50, 25, 3),
            None,
            "the middle is not a handle"
        );
    }

    #[test]
    fn snapping_rounds_to_the_grid() {
        assert_eq!(snap(3, 8), 0);
        assert_eq!(snap(5, 8), 8);
        assert_eq!(snap(11, 8), 8);
        assert_eq!(snap(13, 8), 16);
        assert_eq!(snap(-3, 8), 0);
        assert_eq!(snap(7, 0), 7, "grid 0 is disabled");
    }

    #[test]
    fn resizing_moves_only_the_named_edges() {
        let rect = DesignRect::new(0, 0, 100, 50);
        assert_eq!(
            resize(rect, Handle::SouthEast, 10, 10, 1),
            DesignRect::new(0, 0, 110, 60)
        );
        assert_eq!(
            resize(rect, Handle::West, 10, 0, 1),
            DesignRect::new(10, 0, 100, 50)
        );
        assert_eq!(
            resize(rect, Handle::North, 0, 5, 1),
            DesignRect::new(0, 5, 100, 50)
        );
    }

    #[test]
    fn a_resize_never_inverts_the_rectangle() {
        let rect = DesignRect::new(0, 0, 100, 50);
        let resized = resize(rect, Handle::East, -200, 0, 1);
        assert!(resized.width() >= 1);
        let form = DesignRect::new(0, 0, 100, 50);
        assert!(resize_form(form, Handle::SouthEast, -200, -200, 1).width() >= 1);
    }

    #[test]
    fn the_form_is_resized_from_its_fixed_origin() {
        let form = DesignRect::new(0, 0, 100, 50);
        assert_eq!(
            resize_form(form, Handle::SouthEast, 120, 80, 1),
            DesignRect::new(0, 0, 120, 80)
        );
        assert_eq!(
            resize_form(form, Handle::West, 20, 0, 1),
            DesignRect::new(0, 0, 80, 50)
        );
    }
}
