#![forbid(unsafe_code)]

//! Where a `PictureBox` draws its picture: fit, zoom, pan and rotation, as
//! pure geometry over device pixels so it is tested without a window.

use xui_core::geometry::Rect;
use xui_core::image::Image;

/// The smallest zoom, in percent.
pub const MIN_ZOOM: f64 = 1.0;
/// The largest zoom, in percent.
pub const MAX_ZOOM: f64 = 6400.0;

/// The zoom steps `zoom_in`/`zoom_out` walk, in percent (the classic viewer
/// ladder).
pub const ZOOM_STEPS: [f64; 22] = [
    1.0, 2.0, 3.0, 5.0, 8.0, 12.0, 16.0, 25.0, 33.0, 50.0, 66.0, 75.0, 100.0, 150.0, 200.0, 300.0,
    400.0, 600.0, 800.0, 1600.0, 3200.0, 6400.0,
];

/// How the picture is sized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sizing {
    /// Best fit: shrink to the box; with `enlarge`, also grow a smaller
    /// picture to fill it.
    Fit {
        /// Whether a picture smaller than the box is enlarged.
        enlarge: bool,
    },
    /// A fixed zoom, in percent of the picture's own pixels.
    Zoom(f64),
}

/// The scale (device pixels per picture pixel) for a `picture` of that size
/// in a `view`.
pub fn scale(sizing: Sizing, view: (f64, f64), picture: (f64, f64)) -> f64 {
    match sizing {
        Sizing::Zoom(percent) => percent / 100.0,
        Sizing::Fit { enlarge } => {
            if picture.0 <= 0.0 || picture.1 <= 0.0 || view.0 <= 0.0 || view.1 <= 0.0 {
                return 1.0;
            }
            let fit = (view.0 / picture.0).min(view.1 / picture.1);
            if enlarge { fit } else { fit.min(1.0) }
        }
    }
}

/// Clamps a pan offset so the picture never leaves a gap it could fill: on an
/// axis where the picture is no larger than the view the offset is zero (the
/// picture is centred); otherwise it may move by half the overflow either way.
pub fn clamp_pan(pan: (f64, f64), view: (f64, f64), drawn: (f64, f64)) -> (f64, f64) {
    let axis = |pan: f64, view: f64, drawn: f64| {
        let slack = ((drawn - view) / 2.0).max(0.0);
        pan.clamp(-slack, slack)
    };
    (axis(pan.0, view.0, drawn.0), axis(pan.1, view.1, drawn.1))
}

/// The device rectangle the picture is drawn into, inside `bounds`.
pub fn placement(bounds: Rect, scale: f64, picture: (u32, u32), pan: (f64, f64)) -> Rect {
    let view = (f64::from(bounds.width()), f64::from(bounds.height()));
    let drawn = (f64::from(picture.0) * scale, f64::from(picture.1) * scale);
    let pan = clamp_pan(pan, view, drawn);
    // Bounded well inside i32 so the backend's arithmetic cannot overflow.
    const LIMIT: f64 = (1 << 28) as f64;
    let w = drawn.0.round().clamp(1.0, LIMIT);
    let h = drawn.1.round().clamp(1.0, LIMIT);
    let left = (f64::from(bounds.left) + (view.0 - w) / 2.0 + pan.0)
        .round()
        .clamp(-LIMIT, LIMIT);
    let top = (f64::from(bounds.top) + (view.1 - h) / 2.0 + pan.1)
        .round()
        .clamp(-LIMIT, LIMIT);
    Rect::new(left as i32, top as i32, (left + w) as i32, (top + h) as i32)
}

/// The next step above `current` (or below with `up = false`), clamped to the
/// ladder's ends.
pub fn step_zoom(current: f64, up: bool) -> f64 {
    // A small tolerance, so 99.6% (a fit) steps up to 100%, not 150%.
    const EPSILON: f64 = 0.5;
    if up {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|step| *step > current + EPSILON)
            .unwrap_or(MAX_ZOOM)
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < current - EPSILON)
            .unwrap_or(MIN_ZOOM)
    }
}

/// `image` turned clockwise by `quarters` quarter turns.
pub fn rotate(image: &Image, quarters: u8) -> Image {
    let quarters = quarters % 4;
    if quarters == 0 {
        return image.clone();
    }
    let (w, h) = (image.width() as usize, image.height() as usize);
    let source = image.pixels();
    let (out_w, out_h) = if quarters == 2 { (w, h) } else { (h, w) };
    let mut out = vec![0u8; source.len()];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match quarters {
                1 => (h - 1 - y, x),
                2 => (w - 1 - x, h - 1 - y),
                _ => (y, w - 1 - x),
            };
            let from = (y * w + x) * 4;
            let to = (ny * out_w + nx) * 4;
            out[to..to + 4].copy_from_slice(&source[from..from + 4]);
        }
    }
    // The size is unchanged in area, so this cannot fail.
    Image::from_rgba(out_w as u32, out_h as u32, out).unwrap_or_else(|_| image.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_fit_shrinks_but_does_not_enlarge() {
        let fit = Sizing::Fit { enlarge: false };
        assert_eq!(scale(fit, (400.0, 300.0), (800.0, 300.0)), 0.5);
        assert_eq!(scale(fit, (400.0, 300.0), (100.0, 100.0)), 1.0);
        let stretch = Sizing::Fit { enlarge: true };
        assert_eq!(scale(stretch, (400.0, 300.0), (100.0, 100.0)), 3.0);
        assert_eq!(scale(Sizing::Zoom(250.0), (1.0, 1.0), (5.0, 5.0)), 2.5);
        assert_eq!(scale(fit, (0.0, 0.0), (5.0, 5.0)), 1.0, "an empty view");
    }

    #[test]
    fn a_small_picture_is_centred_and_cannot_pan() {
        let bounds = Rect::new(0, 0, 400, 300);
        let rect = placement(bounds, 1.0, (100, 50), (500.0, -500.0));
        assert_eq!(rect, Rect::new(150, 125, 250, 175));
    }

    #[test]
    fn a_large_picture_pans_up_to_its_edges() {
        let bounds = Rect::new(10, 20, 410, 320);
        // 800 wide: 400 of overflow, so the pan is clamped to +-200.
        let rect = placement(bounds, 1.0, (800, 300), (1000.0, 0.0));
        assert_eq!(rect, Rect::new(10, 20, 810, 320), "the left edge shows");
        let rect = placement(bounds, 1.0, (800, 300), (-1000.0, 0.0));
        assert_eq!(rect.right, 410, "the right edge shows");
    }

    #[test]
    fn zoom_walks_the_ladder() {
        assert_eq!(step_zoom(100.0, true), 150.0);
        assert_eq!(step_zoom(100.0, false), 75.0);
        assert_eq!(step_zoom(99.8, true), 150.0, "a near-100 fit counts as 100");
        assert_eq!(step_zoom(37.0, true), 50.0);
        assert_eq!(step_zoom(37.0, false), 33.0);
        assert_eq!(step_zoom(MAX_ZOOM, true), MAX_ZOOM);
        assert_eq!(step_zoom(MIN_ZOOM, false), MIN_ZOOM);
    }

    #[test]
    fn rotation_moves_corners_clockwise() {
        // 2x1: red, green.
        let image = Image::from_rgba(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
        let cw = rotate(&image, 1);
        assert_eq!(cw.size(), (1, 2));
        assert_eq!(
            cw.pixel(0, 0),
            Some([255, 0, 0, 255]),
            "left goes to the top"
        );
        assert_eq!(cw.pixel(0, 1), Some([0, 255, 0, 255]));
        let half = rotate(&image, 2);
        assert_eq!(half.pixel(0, 0), Some([0, 255, 0, 255]));
        let ccw = rotate(&image, 3);
        assert_eq!(
            ccw.pixel(0, 0),
            Some([0, 255, 0, 255]),
            "right goes to the top"
        );
        assert_eq!(rotate(&rotate(&cw, 1), 2).pixels(), image.pixels());
    }
}
