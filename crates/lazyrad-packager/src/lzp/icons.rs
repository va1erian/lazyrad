#![forbid(unsafe_code)]

//! The three icons every package needs (`icons/app-{16,32,128}.png`).
//!
//! A project's own icon is an `.ico` (for the Windows export); resampling it is
//! a later step (LazyOS plan P4). Until then a package gets a generated default:
//! a rounded colour tile whose hue comes from a hash of the project name, so
//! different apps are told apart in the Start menu. The PNG encoder is tiny and
//! dependency-free apart from the deflate already used by the zip writer.

use miniz_oxide::deflate::compress_to_vec_zlib;

use crate::lzp::zip::crc32;

/// The sizes the LazyOS shell uses.
pub const SIZES: [u32; 3] = [16, 32, 128];

/// The eight-byte PNG signature.
pub const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// The entry name of the icon of `size` pixels.
pub fn entry_name(size: u32) -> String {
    format!("icons/app-{size}.png")
}

/// The default icon of `size` pixels for an app called `name`.
pub fn default_icon(name: &str, size: u32) -> Vec<u8> {
    let (r, g, b) = hue_to_rgb(name_hue(name));
    let radius = (size / 5).max(2) as i32;
    let edge = (size / 16).max(1) as i32;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size as i32 {
        for x in 0..size as i32 {
            let inside = inside_rounded_square(x, y, size as i32, radius);
            if !inside {
                pixels.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            // A lighter top edge and a darker bottom edge give a little depth.
            let shade = if y < edge {
                40
            } else if y >= size as i32 - edge {
                -40
            } else {
                0
            };
            let lift = |channel: u8| (i32::from(channel) + shade).clamp(0, 255) as u8;
            pixels.extend_from_slice(&[lift(r), lift(g), lift(b), 255]);
        }
    }
    encode_rgba(size, size, &pixels)
}

/// Whether `(x, y)` lies inside a `size`-wide square with rounded corners.
fn inside_rounded_square(x: i32, y: i32, size: i32, radius: i32) -> bool {
    let cx = x.clamp(radius, size - 1 - radius);
    let cy = y.clamp(radius, size - 1 - radius);
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= radius * radius
}

/// A stable hue in `0..360` from the name (FNV-1a).
fn name_hue(name: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash % 360
}

/// A saturated mid-brightness colour of `hue` degrees.
fn hue_to_rgb(hue: u32) -> (u8, u8, u8) {
    let sector = hue / 60;
    let fraction = (hue % 60) as f32 / 60.0;
    let (high, low) = (210.0_f32, 60.0_f32);
    let rising = low + (high - low) * fraction;
    let falling = high - (high - low) * fraction;
    let (r, g, b) = match sector {
        0 => (high, rising, low),
        1 => (falling, high, low),
        2 => (low, high, rising),
        3 => (low, falling, high),
        4 => (rising, low, high),
        _ => (high, low, falling),
    };
    (r as u8, g as u8, b as u8)
}

/// Encodes 8-bit RGBA `pixels` (row-major, `width * height * 4` bytes) as a PNG.
pub fn encode_rgba(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in pixels.chunks_exact(stride) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    let mut out = PNG_SIGNATURE.to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &compress_to_vec_zlib(&raw, 6));
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    let crc = crc32(&body);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Whether `data` starts with the PNG signature (all the reader checks).
pub fn is_png(data: &[u8]) -> bool {
    data.starts_with(&PNG_SIGNATURE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes just enough of a PNG to check it: dimensions and that IDAT
    /// inflates to exactly `height` rows.
    fn check(png: &[u8], size: u32) {
        assert!(is_png(png));
        let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        assert_eq!((width, height), (size, size));
        // First chunk after IHDR is IDAT: length at 33, type at 37.
        let len = u32::from_be_bytes([png[33], png[34], png[35], png[36]]) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let data = &png[41..41 + len];
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(data).expect("valid zlib");
        assert_eq!(raw.len(), (size as usize * 4 + 1) * size as usize);
        // The CRC of the IDAT chunk verifies.
        let crc = u32::from_be_bytes([png[41 + len], png[42 + len], png[43 + len], png[44 + len]]);
        assert_eq!(crc, crc32(&png[37..41 + len]));
        assert!(png.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
    }

    #[test]
    fn every_required_size_is_a_valid_png() {
        for size in SIZES {
            check(&default_icon("Todo", size), size);
        }
    }

    #[test]
    fn the_icon_is_stable_and_depends_on_the_name() {
        assert_eq!(default_icon("Todo", 32), default_icon("Todo", 32));
        assert_ne!(default_icon("Todo", 32), default_icon("Calculator", 32));
    }

    #[test]
    fn corners_are_transparent_and_the_centre_is_opaque() {
        let size = 32u32;
        let png = default_icon("Todo", size);
        let len = u32::from_be_bytes([png[33], png[34], png[35], png[36]]) as usize;
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&png[41..41 + len]).unwrap();
        let pixel = |x: usize, y: usize| {
            let at = y * (size as usize * 4 + 1) + 1 + x * 4;
            raw[at + 3]
        };
        assert_eq!(pixel(0, 0), 0);
        assert_eq!(pixel(16, 16), 255);
    }
}
