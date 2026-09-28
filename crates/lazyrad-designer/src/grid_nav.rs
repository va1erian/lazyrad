#![forbid(unsafe_code)]

//! Keyboard navigation for the property grid.
//!
//! Two pieces of pure logic live here so the grid's widget code stays about
//! painting and messages: [`move_row`], which turns a navigation key into the
//! next row or dropdown entry, and [`ObjectDropdown`], the bounded, scrollable
//! layout of the object combo's list. Painting and hit-testing both build their
//! [`ObjectDropdown`] from the same inputs, so what is drawn is what is hit.

use std::ops::Range;

use xui_core::geometry::{Point, Rect};

/// A keyboard movement of the current row (or of the open dropdown's highlight).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowMove {
    /// One row up (Up, Shift+Tab).
    Up,
    /// One row down (Down, Tab).
    Down,
    /// The first row.
    Home,
    /// The last row.
    End,
    /// One page up.
    PageUp,
    /// One page down.
    PageDown,
}

/// Where a `mv` from `current` lands among `count` entries, moving by `page`
/// entries for [`RowMove::PageUp`] and [`RowMove::PageDown`].
///
/// The movement stops at both ends. With no current entry, End selects the
/// last, PageDown goes one page from the top and every other movement selects
/// the first. A `current` past the end counts as the last entry. `None` when
/// there are no entries.
pub(crate) fn move_row(
    current: Option<usize>,
    count: usize,
    mv: RowMove,
    page: usize,
) -> Option<usize> {
    let last = count.checked_sub(1)?;
    let page = page.max(1);
    let Some(current) = current.map(|index| index.min(last)) else {
        return Some(match mv {
            RowMove::End => last,
            RowMove::PageDown => page.min(last),
            _ => 0,
        });
    };
    Some(match mv {
        RowMove::Up => current.saturating_sub(1),
        RowMove::Down => (current + 1).min(last),
        RowMove::Home => 0,
        RowMove::End => last,
        RowMove::PageUp => current.saturating_sub(page),
        RowMove::PageDown => (current + page).min(last),
    })
}

/// The object dropdown's layout: the entries that fit below the combo, and
/// which of them are showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ObjectDropdown {
    /// The visible list, a whole number of entries tall.
    pub rect: Rect,
    /// The height of one entry.
    row_h: i32,
    /// The index of the first visible entry.
    first: usize,
    /// How many entries are visible.
    visible: usize,
}

impl ObjectDropdown {
    /// The dropdown of `count` entries under `anchor`, no lower than `limit`
    /// (the grid's bottom), scrolled so entry `first` is at the top. `first`
    /// is clamped so the list never shows blank space after the last entry.
    pub fn new(anchor: Rect, limit: i32, row_h: i32, count: usize, first: usize) -> ObjectDropdown {
        let row_h = row_h.max(1);
        let room = ((limit - anchor.bottom).max(0) / row_h) as usize;
        let visible = count.min(room);
        // With no room nothing shows, whatever the requested scroll.
        let first = if visible == 0 {
            0
        } else {
            first.min(count - visible)
        };
        ObjectDropdown {
            rect: Rect::new(
                anchor.left,
                anchor.bottom,
                anchor.right,
                anchor.bottom + row_h * visible as i32,
            ),
            row_h,
            first,
            visible,
        }
    }

    /// The index of the first visible entry.
    pub fn first(&self) -> usize {
        self.first
    }

    /// How many entries are visible.
    pub fn visible(&self) -> usize {
        self.visible
    }

    /// The indices of the visible entries.
    pub fn range(&self) -> Range<usize> {
        self.first..self.first + self.visible
    }

    /// The rectangle of entry `index`, if it is visible.
    pub fn row_rect(&self, index: usize) -> Option<Rect> {
        if !self.range().contains(&index) {
            return None;
        }
        let top = self.rect.top + self.row_h * (index - self.first) as i32;
        Some(Rect::new(
            self.rect.left,
            top,
            self.rect.right,
            top + self.row_h,
        ))
    }

    /// The entry under `point`, if any.
    pub fn index_at(&self, point: Point) -> Option<usize> {
        if !self.rect.contains(point) {
            return None;
        }
        Some(self.first + ((point.y - self.rect.top) / self.row_h) as usize)
    }

    /// The first visible entry that shows `index`, moving as little as
    /// possible from the current one.
    pub fn first_showing(&self, index: usize) -> usize {
        if self.visible == 0 || index < self.first {
            index.min(self.first)
        } else if index >= self.first + self.visible {
            index + 1 - self.visible
        } else {
            self.first
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dropdown(limit: i32, count: usize, first: usize) -> ObjectDropdown {
        ObjectDropdown::new(Rect::new(4, 4, 100, 28), limit, 10, count, first)
    }

    #[test]
    fn moves_stop_at_both_ends() {
        assert_eq!(move_row(Some(0), 5, RowMove::Up, 3), Some(0));
        assert_eq!(move_row(Some(4), 5, RowMove::Down, 3), Some(4));
        assert_eq!(move_row(Some(1), 5, RowMove::PageDown, 3), Some(4));
        assert_eq!(move_row(Some(1), 5, RowMove::PageUp, 3), Some(0));
        assert_eq!(move_row(Some(2), 5, RowMove::Home, 3), Some(0));
        assert_eq!(move_row(Some(2), 5, RowMove::End, 3), Some(4));
    }

    #[test]
    fn a_missing_or_stale_current_entry_is_handled() {
        assert_eq!(move_row(None, 5, RowMove::Down, 3), Some(0));
        assert_eq!(move_row(None, 5, RowMove::Up, 3), Some(0));
        assert_eq!(move_row(None, 5, RowMove::End, 3), Some(4));
        assert_eq!(move_row(None, 5, RowMove::PageDown, 3), Some(3));
        assert_eq!(move_row(Some(99), 5, RowMove::Up, 3), Some(3));
        assert_eq!(move_row(Some(0), 0, RowMove::Down, 3), None);
    }

    #[test]
    fn the_dropdown_stops_at_the_grid_bottom() {
        // Room for (58 - 28) / 10 = 3 of 8 entries.
        let dd = dropdown(58, 8, 0);
        assert_eq!(dd.visible(), 3);
        assert_eq!(dd.rect.bottom, 58);
        // A small list keeps its own height.
        assert_eq!(dropdown(500, 2, 0).rect.bottom, 48);
    }

    #[test]
    fn the_dropdown_first_entry_is_clamped() {
        assert_eq!(dropdown(58, 8, 99).first(), 5);
        assert_eq!(dropdown(58, 2, 1).first(), 0, "everything fits");
    }

    #[test]
    fn a_dropdown_with_no_room_shows_nothing() {
        let dd = dropdown(20, 8, 3);
        assert_eq!(dd.visible(), 0);
        assert_eq!(dd.first(), 0);
        assert_eq!(dd.index_at(Point::new(10, 30)), None);
        assert_eq!(dd.first_showing(5), 0);
    }

    #[test]
    fn hits_match_the_drawn_rows() {
        let dd = dropdown(58, 8, 2);
        for index in dd.range() {
            let rect = dd.row_rect(index).expect("visible");
            assert_eq!(dd.index_at(Point::new(rect.left, rect.top)), Some(index));
            assert_eq!(
                dd.index_at(Point::new(rect.right - 1, rect.bottom - 1)),
                Some(index)
            );
        }
        assert_eq!(dd.row_rect(1), None);
        assert_eq!(dd.index_at(Point::new(10, 58)), None);
    }

    #[test]
    fn scrolling_keeps_an_entry_in_view() {
        let dd = dropdown(58, 8, 2);
        assert_eq!(dd.first_showing(3), 2);
        assert_eq!(dd.first_showing(0), 0);
        assert_eq!(dd.first_showing(7), 5);
    }
}
