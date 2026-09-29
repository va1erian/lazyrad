#![forbid(unsafe_code)]
//! A tiny scrollbar: track/thumb geometry for the vertical and horizontal
//! bars.
//!
//! xui's scrollbar widget and its painter are private (`pub(crate)`), so the
//! code editor and the property grid share this reimplementation until xui
//! exports `ScrollBar` as a widget (PLAN.md gap G1). The geometry mirrors xui's
//! own so the two look alike. All rectangles are in the coordinates of the
//! canvas being painted, which the caller chooses (node-local for a widget).

use xui_core::Color;
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

/// The thumb's interaction state, for its colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThumbState {
    /// Neither hovered nor dragged.
    #[default]
    Normal,
    /// The pointer is over the bar.
    Hover,
    /// The thumb is being dragged.
    Pressed,
}

/// Where a press on a bar landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackHit {
    /// On the thumb: start a drag.
    Thumb,
    /// On the track before the thumb (above or left of it): page back.
    Before,
    /// On the track after the thumb: page forward.
    After,
}

/// What a press at `pointer` (along the bar's axis) hits, or `None` when
/// nothing scrolls.
pub fn hit(
    track: Rect,
    scroll: Scroll,
    orientation: Orientation,
    pointer: i32,
    dpi: u32,
) -> Option<TrackHit> {
    let thumb = thumb(track, scroll, orientation, dpi)?;
    let (start, end) = match orientation {
        Orientation::Vertical => (thumb.top, thumb.bottom),
        Orientation::Horizontal => (thumb.left, thumb.right),
    };
    Some(if pointer < start {
        TrackHit::Before
    } else if pointer >= end {
        TrackHit::After
    } else {
        TrackHit::Thumb
    })
}

/// The offset after paging in `direction` (-1 back, 1 forward) by `page`,
/// clamped to the scrollable range.
pub fn paged_offset(scroll: Scroll, direction: i32, page: i32) -> i32 {
    (scroll.offset + direction.signum() * page.max(0)).clamp(0, scroll.max_offset())
}

/// The thumb colour for `state`: the theme's thumb, drawn towards the text
/// colour when hovered and further when pressed, so it works in both themes.
pub fn thumb_color(theme: Theme, state: ThumbState) -> Color {
    match state {
        ThumbState::Normal => theme.scrollbar,
        ThumbState::Hover => theme.scrollbar.lerp(theme.text, 0.25),
        ThumbState::Pressed => theme.scrollbar.lerp(theme.text, 0.5),
    }
}

/// Paints a bar's track and thumb, into `track`.
pub fn paint(
    canvas: &mut dyn Canvas,
    track: Rect,
    scroll: Scroll,
    orientation: Orientation,
    theme: Theme,
) {
    paint_state(
        canvas,
        track,
        scroll,
        orientation,
        theme,
        ThumbState::Normal,
    );
}

/// Paints a bar's track and thumb with the thumb in `state`.
pub fn paint_state(
    canvas: &mut dyn Canvas,
    track: Rect,
    scroll: Scroll,
    orientation: Orientation,
    theme: Theme,
    state: ThumbState,
) {
    canvas.fill_rect(track, theme.scrollbar_track);
    if let Some(thumb) = thumb(track, scroll, orientation, canvas.dpi()) {
        canvas.fill_rounded_rect(thumb, 2.0, thumb_color(theme, state));
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

    #[test]
    fn a_press_hits_the_thumb_or_either_side_of_it() {
        let track = Rect::new(0, 0, 12, 100);
        let scroll = Scroll {
            viewport: 100,
            content: 400,
            offset: 150,
        };
        let thumb = thumb(track, scroll, Orientation::Vertical, 96).unwrap();
        let hit = |pointer| hit(track, scroll, Orientation::Vertical, pointer, 96);
        assert_eq!(hit(thumb.top - 1), Some(TrackHit::Before));
        assert_eq!(hit(thumb.top), Some(TrackHit::Thumb));
        assert_eq!(hit(thumb.bottom - 1), Some(TrackHit::Thumb));
        assert_eq!(hit(thumb.bottom), Some(TrackHit::After));
        let fit = Scroll {
            content: 100,
            ..scroll
        };
        assert_eq!(hit_fit(track, fit), None);
    }

    fn hit_fit(track: Rect, scroll: Scroll) -> Option<TrackHit> {
        hit(track, scroll, Orientation::Vertical, 10, 96)
    }

    #[test]
    fn paging_clamps_at_both_ends() {
        let scroll = Scroll {
            viewport: 100,
            content: 250,
            offset: 20,
        };
        assert_eq!(paged_offset(scroll, -1, 80), 0);
        assert_eq!(paged_offset(scroll, 1, 80), 100);
        assert_eq!(
            paged_offset(
                Scroll {
                    offset: 140,
                    ..scroll
                },
                1,
                80
            ),
            150
        );
        assert_eq!(paged_offset(scroll, 1, -5), 20);
    }

    #[test]
    fn the_thumb_colour_differs_per_state_in_both_themes() {
        for theme in [Theme::light(), Theme::dark()] {
            let normal = thumb_color(theme, ThumbState::Normal);
            let hover = thumb_color(theme, ThumbState::Hover);
            let pressed = thumb_color(theme, ThumbState::Pressed);
            assert_ne!(normal, hover);
            assert_ne!(hover, pressed);
        }
    }

    #[test]
    fn a_degenerate_track_has_no_thumb_and_a_drag_maps_to_zero() {
        let scroll = Scroll {
            viewport: 100,
            content: 400,
            offset: 0,
        };
        let empty = Rect::new(0, 0, 12, 0);
        assert!(thumb(empty, scroll, Orientation::Vertical, 96).is_none());
        assert_eq!(
            offset_from_drag(empty, scroll, Orientation::Vertical, 5, 0, 50, 96),
            0
        );
    }

    #[test]
    fn dragging_clamps_at_both_ends() {
        let track = Rect::new(0, 0, 12, 100);
        let scroll = Scroll {
            viewport: 100,
            content: 400,
            offset: 100,
        };
        assert_eq!(
            offset_from_drag(track, scroll, Orientation::Vertical, 100, 0, 5000, 96),
            300
        );
        assert_eq!(
            offset_from_drag(track, scroll, Orientation::Vertical, 100, 50, -5000, 96),
            0
        );
    }
}
