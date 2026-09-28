#![forbid(unsafe_code)]
//! A tiny scrollbar: track/thumb geometry for the vertical and horizontal
//! bars.
//!
//! xui's scrollbar widget and its painter are private (PLAN.md §10, gap G1:
//! `xui_core::widget::scrollbar` and `widget::painter` are `pub(crate)`), so
//! the editor carries this reimplementation until xui exports `ScrollBar` as a
//! widget. The upstream request is tracked at
//! <https://github.com/va1erian/xui/issues> (see the `ScrollBar`/`Popup`
//! export proposal). The geometry mirrors xui's own so the two look alike.

use xui_core::backend::Canvas;
use xui_core::geometry::Rect;
use xui_core::theme::Theme;
use xui_core::units::Dip;

/// The shortest a thumb may shrink to.
const MIN_THUMB: Dip = Dip(24.0);

/// Which way a bar runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    /// A vertical bar on the trailing edge.
    Vertical,
    /// A horizontal bar on the bottom edge.
    Horizontal,
}

/// The scroll state a bar draws and drags, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scroll {
    /// The visible extent.
    pub viewport: i32,
    /// The content's total extent.
    pub content: i32,
    /// The current offset.
    pub offset: i32,
}

impl Scroll {
    /// The largest useful offset.
    pub fn max_offset(self) -> i32 {
        (self.content - self.viewport).max(0)
    }

    /// Whether the content overflows the viewport.
    pub fn overflows(self) -> bool {
        self.content > self.viewport
    }
}

/// The thumb's rectangle within `track`, or `None` when nothing scrolls.
pub fn thumb(track: Rect, scroll: Scroll, orientation: Orientation, dpi: u32) -> Option<Rect> {
    let length = match orientation {
        Orientation::Vertical => track.height(),
        Orientation::Horizontal => track.width(),
    };
    if !scroll.overflows() || length <= 0 {
        return None;
    }
    let min = MIN_THUMB.to_px(dpi).value().min(length);
    let proportional =
        (i64::from(length) * i64::from(scroll.viewport.max(0)) / i64::from(scroll.content)) as i32;
    let thumb_len = proportional.clamp(min, length);
    let travel = length - thumb_len;
    let max = scroll.max_offset();
    let pos = if max > 0 {
        travel * scroll.offset.clamp(0, max) / max
    } else {
        0
    };
    Some(match orientation {
        Orientation::Vertical => Rect::new(
            track.left + 2,
            track.top + pos,
            track.right - 2,
            track.top + pos + thumb_len,
        ),
        Orientation::Horizontal => Rect::new(
            track.left + pos,
            track.top + 2,
            track.left + pos + thumb_len,
            track.bottom - 2,
        ),
    })
}

/// The offset a thumb drag to `pointer` reaches, given where the drag began.
pub fn offset_from_drag(
    track: Rect,
    scroll: Scroll,
    orientation: Orientation,
    start_offset: i32,
    start_pointer: i32,
    pointer: i32,
    dpi: u32,
) -> i32 {
    let Some(thumb) = thumb(track, scroll, orientation, dpi) else {
        return 0;
    };
    let length = match orientation {
        Orientation::Vertical => track.height(),
        Orientation::Horizontal => track.width(),
    };
    let thumb_len = match orientation {
        Orientation::Vertical => thumb.height(),
        Orientation::Horizontal => thumb.width(),
    };
    let travel = (length - thumb_len).max(1);
    let max = scroll.max_offset();
    (start_offset + (pointer - start_pointer) * max / travel).clamp(0, max)
}

/// Paints a bar's track and thumb, into `track`.
pub fn paint(
    canvas: &mut dyn Canvas,
    track: Rect,
    scroll: Scroll,
    orientation: Orientation,
    theme: Theme,
) {
    canvas.fill_rect(track, theme.scrollbar_track);
    if let Some(thumb) = thumb(track, scroll, orientation, canvas.dpi()) {
        canvas.fill_rounded_rect(thumb, 2.0, theme.scrollbar);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_content_has_no_thumb() {
        let track = Rect::new(0, 0, 12, 100);
        let fit = Scroll {
            viewport: 100,
            content: 100,
            offset: 0,
        };
        assert!(thumb(track, fit, Orientation::Vertical, 96).is_none());
    }

    #[test]
    fn the_thumb_shrinks_and_tracks_the_offset() {
        let track = Rect::new(0, 0, 12, 100);
        let scroll = Scroll {
            viewport: 100,
            content: 400,
            offset: 0,
        };
        let top = thumb(track, scroll, Orientation::Vertical, 96).unwrap();
        assert!(top.height() < 100);
        assert_eq!(top.top, 0);
        let bottom = thumb(
            track,
            Scroll {
                offset: 300,
                ..scroll
            },
            Orientation::Vertical,
            96,
        )
        .unwrap();
        assert_eq!(bottom.bottom, 100);
        assert_eq!(top.height(), bottom.height());
    }

    #[test]
    fn a_horizontal_thumb_runs_along_x() {
        let track = Rect::new(0, 0, 100, 12);
        let scroll = Scroll {
            viewport: 100,
            content: 400,
            offset: 150,
        };
        let thumb = thumb(track, scroll, Orientation::Horizontal, 96).unwrap();
        assert!(thumb.width() < 100);
        assert!(thumb.left > 0);
        assert_eq!(thumb.height(), 8);
    }

    #[test]
    fn dragging_maps_pointer_travel_to_offset() {
        let track = Rect::new(0, 0, 12, 100);
        let scroll = Scroll {
            viewport: 100,
            content: 400,
            offset: 0,
        };
        // A quarter-height thumb has 75px of travel for 300px of content.
        assert_eq!(
            offset_from_drag(track, scroll, Orientation::Vertical, 0, 0, 75, 96),
            300
        );
        assert_eq!(
            offset_from_drag(track, scroll, Orientation::Vertical, 100, 10, 30, 96),
            180
        );
    }
}
