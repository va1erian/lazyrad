#![forbid(unsafe_code)]

//! Turning a file's bytes into an [`Image`] for the `PictureBox`.
//!
//! PNG and JPEG go to `xui-core`'s decoders; BMP and GIF (its first frame) are
//! decoded here, because `xui-core` has no decoder for them and a picture
//! viewer is expected to open both. Every format's size is read from its header
//! first ([`probe`]) and a picture above [`MAX_PIXELS`] is refused before any
//! pixel buffer is allocated, so a small hostile file cannot claim gigabytes.
//!
//! The bytes are untrusted: every read is bounds-checked and a malformed file
//! is an error, never a panic.

use xui_core::image::Image;

/// The most pixels a picture may have (about 32 megapixels: 128 MB as RGBA).
pub const MAX_PIXELS: u64 = 32_000_000;

/// A picture format the box recognises by its signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Portable Network Graphics.
    Png,
    /// JPEG (baseline or progressive).
    Jpeg,
    /// Windows bitmap.
    Bmp,
    /// GIF (the first frame).
    Gif,
}

impl Format {
    /// The name a script sees (`picture.format`).
    pub fn name(self) -> &'static str {
        match self {
            Format::Png => "PNG",
            Format::Jpeg => "JPEG",
            Format::Bmp => "BMP",
            Format::Gif => "GIF",
        }
    }

    /// The format whose signature `bytes` start with.
    pub fn sniff(bytes: &[u8]) -> Option<Format> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Format::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8]) {
            Some(Format::Jpeg)
        } else if bytes.starts_with(b"BM") {
            Some(Format::Bmp)
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Some(Format::Gif)
        } else {
            None
        }
    }
}

/// Why a picture could not be decoded.
pub type DecodeResult<T> = Result<T, String>;

/// Decodes `bytes` into an image, refusing pictures above [`MAX_PIXELS`].
pub fn decode(bytes: &[u8]) -> DecodeResult<(Image, Format)> {
    let format = Format::sniff(bytes).ok_or("not a PNG, JPEG, BMP or GIF picture")?;
    let (width, height) = probe(bytes, format)?;
    check_size(width, height)?;
    let image = match format {
        Format::Png | Format::Jpeg => Image::decode(bytes).map_err(|error| error.to_string())?,
        Format::Bmp => decode_bmp(bytes)?,
        Format::Gif => decode_gif(bytes)?,
    };
    // A decoder may report a size its header did not (a JPEG with several
    // frames); check what was actually built too.
    check_size(image.width(), image.height())?;
    Ok((image, format))
}

/// Refuses an empty picture or one above [`MAX_PIXELS`].
fn check_size(width: u32, height: u32) -> DecodeResult<()> {
    if width == 0 || height == 0 {
        return Err("the picture is empty".into());
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!(
            "the picture is too large ({width} x {height}; at most {MAX_PIXELS} pixels)"
        ));
    }
    Ok(())
}

/// The picture's size from its header, without decoding it.
pub fn probe(bytes: &[u8], format: Format) -> DecodeResult<(u32, u32)> {
    let size = match format {
        // IHDR is always the first chunk: width and height at 16 and 20.
        Format::Png => be32(bytes, 16).zip(be32(bytes, 20)),
        Format::Jpeg => jpeg_size(bytes),
        Format::Bmp => BmpHeader::parse(bytes)
            .ok()
            .map(|header| (header.width, header.height)),
        Format::Gif => le16(bytes, 6)
            .zip(le16(bytes, 8))
            .map(|(w, h)| (u32::from(w), u32::from(h))),
    };
    size.ok_or_else(|| format!("the {} header is truncated or invalid", format.name()))
}

/// The size in a JPEG's first start-of-frame marker.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    loop {
        // Markers may be padded with any number of 0xFF bytes.
        while *bytes.get(at)? == 0xFF && *bytes.get(at + 1)? == 0xFF {
            at += 1;
        }
        if *bytes.get(at)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        let length = usize::from(be16(bytes, at + 2)?);
        let is_frame = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_frame {
            let height = be16(bytes, at + 5)?;
            let width = be16(bytes, at + 7)?;
            return Some((u32::from(width), u32::from(height)));
        }
        if length < 2 {
            return None;
        }
        at = at.checked_add(2 + length)?;
    }
}

fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn le16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]))
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

// ------------------------------------------------------------------ BMP ---

/// What a BMP's headers say.
#[derive(Debug)]
struct BmpHeader {
    /// Where the pixel rows start.
    data_offset: usize,
    width: u32,
    height: u32,
    /// Whether row 0 is the top row (a negative height on disk).
    top_down: bool,
    bits: u16,
    /// Red, green, blue and alpha masks for 16- and 32-bit bitfields.
    masks: [u32; 4],
    /// The colour table, as RGB.
    palette: Vec<[u8; 3]>,
}

impl BmpHeader {
    /// Reads the file and info headers. Only uncompressed (`BI_RGB`) and
    /// `BI_BITFIELDS` bitmaps are accepted; RLE ones are refused.
    fn parse(bytes: &[u8]) -> DecodeResult<BmpHeader> {
        let bad = || "the BMP header is truncated or invalid".to_owned();
        let data_offset = le32(bytes, 10).ok_or_else(bad)? as usize;
        let info_size = le32(bytes, 14).ok_or_else(bad)? as usize;
        if info_size < 40 {
            // The OS/2 BITMAPCOREHEADER: rare enough to refuse.
            return Err("this BMP variant is not supported".into());
        }
        let width = le32(bytes, 18).ok_or_else(bad)? as i32;
        let raw_height = le32(bytes, 22).ok_or_else(bad)? as i32;
        let bits = le16(bytes, 28).ok_or_else(bad)?;
        let compression = le32(bytes, 30).ok_or_else(bad)?;
        let colors_used = le32(bytes, 46).ok_or_else(bad)?;
        if width <= 0 || raw_height == 0 || raw_height == i32::MIN {
            return Err(bad());
        }
        let masks = match (compression, bits) {
            (0, 16) => [0x7C00, 0x03E0, 0x001F, 0],
            (0, 32) => [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0],
            (0, _) => [0; 4],
            // BI_BITFIELDS: masks follow a 40-byte header, or sit inside a
            // larger (V4/V5) one at the same place.
            (3, 16 | 32) => [
                le32(bytes, 54).ok_or_else(bad)?,
                le32(bytes, 58).ok_or_else(bad)?,
                le32(bytes, 62).ok_or_else(bad)?,
                if info_size >= 56 {
                    le32(bytes, 66).ok_or_else(bad)?
                } else {
                    0
                },
            ],
            _ => return Err("compressed BMP pictures are not supported".into()),
        };
        if !matches!(bits, 1 | 4 | 8 | 16 | 24 | 32) {
            return Err(format!("{bits}-bit BMP pictures are not supported"));
        }
        let mut palette = Vec::new();
        if bits <= 8 {
            let max = 1usize << bits;
            let count = match colors_used as usize {
                0 => max,
                n => n.min(max),
            };
            // The table follows the info header (and the masks of a 40-byte
            // BI_BITFIELDS header, which only 16/32-bit pictures have).
            let table = 14 + info_size;
            for index in 0..count {
                let at = table + index * 4;
                let entry = bytes.get(at..at + 3).ok_or_else(bad)?;
                palette.push([entry[2], entry[1], entry[0]]);
            }
        }
        Ok(BmpHeader {
            data_offset,
            width: width as u32,
            height: raw_height.unsigned_abs(),
            top_down: raw_height < 0,
            bits,
            masks,
            palette,
        })
    }
}

/// Decodes an uncompressed or bitfields BMP.
fn decode_bmp(bytes: &[u8]) -> DecodeResult<Image> {
    let header = BmpHeader::parse(bytes)?;
    let (width, height) = (header.width as usize, header.height as usize);
    // Rows are padded to four bytes.
    let stride = (width * usize::from(header.bits)).div_ceil(32) * 4;
    let needed = stride
        .checked_mul(height)
        .and_then(|size| size.checked_add(header.data_offset))
        .ok_or("the BMP is too large")?;
    if bytes.len() < needed {
        return Err("the BMP pixel data is truncated".into());
    }
    let alpha_mask = header.masks[3];
    let mut pixels = vec![0u8; width * height * 4];
    for row in 0..height {
        let source_row = if header.top_down {
            row
        } else {
            height - 1 - row
        };
        let line = &bytes[header.data_offset + source_row * stride..][..stride];
        let out = &mut pixels[row * width * 4..][..width * 4];
        for x in 0..width {
            let rgba = bmp_pixel(&header, line, x, alpha_mask)?;
            out[x * 4..x * 4 + 4].copy_from_slice(&rgba);
        }
    }
    Image::from_rgba(header.width, header.height, pixels).map_err(|error| error.to_string())
}

/// The colour of pixel `x` of one BMP row.
fn bmp_pixel(header: &BmpHeader, line: &[u8], x: usize, alpha_mask: u32) -> DecodeResult<[u8; 4]> {
    let from_palette = |index: usize| {
        header
            .palette
            .get(index)
            .map(|[r, g, b]| [*r, *g, *b, 255])
            .ok_or_else(|| "a BMP pixel names a colour outside the palette".to_owned())
    };
    match header.bits {
        1 => from_palette(usize::from(line[x / 8] >> (7 - x % 8) & 1)),
        4 => from_palette(usize::from(
            line[x / 2] >> (if x % 2 == 0 { 4 } else { 0 }) & 0xF,
        )),
        8 => from_palette(usize::from(line[x])),
        24 => Ok([line[x * 3 + 2], line[x * 3 + 1], line[x * 3], 255]),
        16 => {
            let value = u32::from(u16::from_le_bytes([line[x * 2], line[x * 2 + 1]]));
            Ok(masked(value, header.masks, alpha_mask))
        }
        _ => {
            let at = x * 4;
            let value = u32::from_le_bytes([line[at], line[at + 1], line[at + 2], line[at + 3]]);
            Ok(masked(value, header.masks, alpha_mask))
        }
    }
}

/// Extracts the channels of a bitfields pixel, scaling each to 8 bits. With
/// no alpha mask the pixel is opaque.
fn masked(value: u32, masks: [u32; 4], alpha_mask: u32) -> [u8; 4] {
    let channel = |mask: u32| -> u8 {
        if mask == 0 {
            return 0;
        }
        let shift = mask.trailing_zeros();
        let max = mask >> shift;
        let raw = (value & mask) >> shift;
        ((u64::from(raw) * 255 + u64::from(max) / 2) / u64::from(max)) as u8
    };
    let alpha = if alpha_mask == 0 {
        255
    } else {
        channel(alpha_mask)
    };
    [
        channel(masks[0]),
        channel(masks[1]),
        channel(masks[2]),
        alpha,
    ]
}

// ------------------------------------------------------------------ GIF ---

/// The largest LZW code a GIF uses.
const GIF_MAX_CODES: usize = 4096;

/// Decodes the first image of a GIF onto its logical screen.
///
/// The screen starts transparent (a viewer shows its own background, as
/// browsers do); the frame is drawn at its offset, honouring its transparent
/// colour and interlacing.
fn decode_gif(bytes: &[u8]) -> DecodeResult<Image> {
    let bad = || "the GIF is truncated or invalid".to_owned();
    let width = usize::from(le16(bytes, 6).ok_or_else(bad)?);
    let height = usize::from(le16(bytes, 8).ok_or_else(bad)?);
    let flags = *bytes.get(10).ok_or_else(bad)?;
    let mut at = 13;
    let global = if flags & 0x80 != 0 {
        let count = 2usize << (flags & 7);
        let table = read_table(bytes, at, count).ok_or_else(bad)?;
        at += count * 3;
        table
    } else {
        Vec::new()
    };
    let mut transparent: Option<u8> = None;
    loop {
        match *bytes.get(at).ok_or_else(bad)? {
            // Extension: remember a graphic control block's transparency.
            0x21 => {
                let label = *bytes.get(at + 1).ok_or_else(bad)?;
                if label == 0xF9 && *bytes.get(at + 2).ok_or_else(bad)? >= 4 {
                    let packed = *bytes.get(at + 3).ok_or_else(bad)?;
                    let index = *bytes.get(at + 6).ok_or_else(bad)?;
                    transparent = (packed & 1 != 0).then_some(index);
                }
                at = skip_sub_blocks(bytes, at + 2).ok_or_else(bad)?;
            }
            0x2C => break,
            // The trailer before any image: nothing to show.
            _ => return Err("the GIF has no picture".into()),
        }
    }
    let left = usize::from(le16(bytes, at + 1).ok_or_else(bad)?);
    let top = usize::from(le16(bytes, at + 3).ok_or_else(bad)?);
    let frame_w = usize::from(le16(bytes, at + 5).ok_or_else(bad)?);
    let frame_h = usize::from(le16(bytes, at + 7).ok_or_else(bad)?);
    // The frame has a size of its own, independent of the logical screen
    // checked above: refuse it before the index buffer is reserved.
    let frame_pixels = frame_w as u64 * frame_h as u64;
    if frame_pixels > MAX_PIXELS {
        return Err(format!(
            "the GIF frame is too large ({frame_w} x {frame_h}; at most {MAX_PIXELS} pixels)"
        ));
    }
    let packed = *bytes.get(at + 9).ok_or_else(bad)?;
    at += 10;
    let palette = if packed & 0x80 != 0 {
        let count = 2usize << (packed & 7);
        let table = read_table(bytes, at, count).ok_or_else(bad)?;
        at += count * 3;
        table
    } else {
        global
    };
    let interlaced = packed & 0x40 != 0;
    let min_code_size = *bytes.get(at).ok_or_else(bad)?;
    let data = gather_sub_blocks(bytes, at + 1).ok_or_else(bad)?;
    let indices = lzw_decode(&data, min_code_size, frame_w * frame_h)?;

    let mut pixels = vec![0u8; width * height * 4];
    let rows = interlace_order(frame_h, interlaced);
    for (source_row, &dest_row) in rows.iter().enumerate() {
        let y = top + dest_row;
        if y >= height {
            continue;
        }
        for column in 0..frame_w {
            let x = left + column;
            let Some(&index) = indices.get(source_row * frame_w + column) else {
                break;
            };
            if x >= width || transparent == Some(index) {
                continue;
            }
            if let Some([r, g, b]) = palette.get(usize::from(index)) {
                pixels[(y * width + x) * 4..][..4].copy_from_slice(&[*r, *g, *b, 255]);
            }
        }
    }
    Image::from_rgba(width as u32, height as u32, pixels).map_err(|error| error.to_string())
}

/// A colour table of `count` RGB entries at `at`.
fn read_table(bytes: &[u8], at: usize, count: usize) -> Option<Vec<[u8; 3]>> {
    let table = bytes.get(at..at + count * 3)?;
    Some(table.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
}

/// The offset just past a chain of sub-blocks starting at `at`.
fn skip_sub_blocks(bytes: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let size = usize::from(*bytes.get(at)?);
        at += 1;
        if size == 0 {
            return Some(at);
        }
        at = at.checked_add(size)?;
    }
}

/// The concatenated payload of a chain of sub-blocks starting at `at`.
fn gather_sub_blocks(bytes: &[u8], mut at: usize) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    loop {
        let size = usize::from(*bytes.get(at)?);
        at += 1;
        if size == 0 {
            return Some(data);
        }
        data.extend_from_slice(bytes.get(at..at + size)?);
        at += size;
    }
}

/// The destination row of each stored row: interlaced GIFs store every 8th
/// row from 0, every 8th from 4, every 4th from 2, then every 2nd from 1.
fn interlace_order(height: usize, interlaced: bool) -> Vec<usize> {
    if !interlaced {
        return (0..height).collect();
    }
    [(0, 8), (4, 8), (2, 4), (1, 2)]
        .iter()
        .flat_map(|&(start, step)| (start..height).step_by(step))
        .collect()
}

/// Decodes GIF LZW data into at most `limit` colour indices.
fn lzw_decode(data: &[u8], min_code_size: u8, limit: usize) -> DecodeResult<Vec<u8>> {
    if !(2..=11).contains(&min_code_size) {
        return Err("the GIF has an invalid code size".into());
    }
    let clear = 1usize << min_code_size;
    let end = clear + 1;
    // Each code is a prefix code plus one suffix byte; the roots have none.
    let mut prefix = vec![0u16; GIF_MAX_CODES];
    let mut suffix = vec![0u8; GIF_MAX_CODES];
    let mut length = vec![0u16; GIF_MAX_CODES];
    for code in 0..clear {
        suffix[code] = code as u8;
        length[code] = 1;
    }
    let mut next = end + 1;
    let mut size = u32::from(min_code_size) + 1;
    let mut previous: Option<usize> = None;
    let mut out = Vec::with_capacity(limit);
    let (mut bits, mut held) = (0u32, 0u32);
    let mut bytes = data.iter();
    let mut scratch = Vec::new();
    while out.len() < limit {
        while held < size {
            let Some(&byte) = bytes.next() else {
                // A short stream: show what arrived (decoders are lenient).
                return Ok(out);
            };
            bits |= u32::from(byte) << held;
            held += 8;
        }
        let code = (bits & ((1 << size) - 1)) as usize;
        bits >>= size;
        held -= size;
        if code == clear {
            next = end + 1;
            size = u32::from(min_code_size) + 1;
            previous = None;
            continue;
        }
        if code == end {
            break;
        }
        let Some(prev) = previous else {
            if code >= clear {
                return Err("the GIF data is corrupt".into());
            }
            out.push(code as u8);
            previous = Some(code);
            continue;
        };
        // The KwKwK case: the code being defined right now.
        let known = code < next;
        if !known && code != next {
            return Err("the GIF data is corrupt".into());
        }
        let first_of = |code: usize, prefix: &[u16], length: &[u16]| -> u8 {
            let mut c = code;
            for _ in 1..length[code] {
                c = usize::from(prefix[c]);
            }
            suffix[c]
        };
        let first = if known {
            first_of(code, &prefix, &length)
        } else {
            first_of(prev, &prefix, &length)
        };
        if next < GIF_MAX_CODES {
            prefix[next] = prev as u16;
            suffix[next] = first;
            length[next] = length[prev] + 1;
            next += 1;
            if next == 1 << size && size < 12 {
                size += 1;
            }
        }
        // Emit the string for `code` (now defined in either case).
        scratch.clear();
        let mut c = code;
        loop {
            scratch.push(suffix[c]);
            if length[c] <= 1 {
                break;
            }
            c = usize::from(prefix[c]);
        }
        out.extend(scratch.iter().rev().take(limit - out.len()));
        previous = Some(code);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 24-bit bottom-up BMP: red, green on top; blue, white below.
    fn bmp_24() -> Vec<u8> {
        let mut file = Vec::new();
        file.extend_from_slice(b"BM");
        file.extend_from_slice(&(54u32 + 16).to_le_bytes());
        file.extend_from_slice(&[0; 4]);
        file.extend_from_slice(&54u32.to_le_bytes());
        file.extend_from_slice(&40u32.to_le_bytes());
        file.extend_from_slice(&2i32.to_le_bytes());
        file.extend_from_slice(&2i32.to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&24u16.to_le_bytes());
        file.extend_from_slice(&[0; 24]);
        // Bottom row first: blue, white, then two bytes of padding.
        file.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]);
        file.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
        file
    }

    /// A 3x1 GIF with a 4-colour global table, uncompressed-style LZW.
    fn gif_3x1() -> Vec<u8> {
        let mut file = b"GIF89a".to_vec();
        file.extend_from_slice(&3u16.to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&[0x81, 0, 0]);
        // Black, red, green, blue.
        file.extend_from_slice(&[0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
        // Graphic control: colour 0 is transparent.
        file.extend_from_slice(&[0x21, 0xF9, 4, 1, 0, 0, 0, 0]);
        file.extend_from_slice(&[0x2C, 0, 0, 0, 0, 3, 0, 1, 0, 0]);
        // Min code size 2: clear(4), 1, 2, 0, end(5), each 3 bits.
        let codes = [4u32, 1, 2, 0, 5];
        let (mut bits, mut held, mut data) = (0u32, 0, Vec::new());
        for code in codes {
            bits |= code << held;
            held += 3;
            while held >= 8 {
                data.push(bits as u8);
                bits >>= 8;
                held -= 8;
            }
        }
        if held > 0 {
            data.push(bits as u8);
        }
        file.push(2);
        file.push(data.len() as u8);
        file.extend_from_slice(&data);
        file.extend_from_slice(&[0, 0x3B]);
        file
    }

    #[test]
    fn formats_are_sniffed_by_signature() {
        assert_eq!(Format::sniff(b"\x89PNG\r\n\x1a\nxx"), Some(Format::Png));
        assert_eq!(Format::sniff(&[0xFF, 0xD8, 0xFF]), Some(Format::Jpeg));
        assert_eq!(Format::sniff(b"BMxx"), Some(Format::Bmp));
        assert_eq!(Format::sniff(b"GIF87a"), Some(Format::Gif));
        assert_eq!(Format::sniff(b"hello"), None);
    }

    #[test]
    fn a_24_bit_bmp_decodes_top_row_first() {
        let (image, format) = decode(&bmp_24()).expect("decodes");
        assert_eq!(format, Format::Bmp);
        assert_eq!(image.size(), (2, 2));
        assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(image.pixel(1, 0), Some([0, 255, 0, 255]));
        assert_eq!(image.pixel(0, 1), Some([0, 0, 255, 255]));
        assert_eq!(image.pixel(1, 1), Some([255, 255, 255, 255]));
    }

    #[test]
    fn a_truncated_bmp_is_an_error() {
        let file = bmp_24();
        for cut in 0..file.len() {
            assert!(decode(&file[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn a_gif_decodes_its_first_frame_with_transparency() {
        let (image, format) = decode(&gif_3x1()).expect("decodes");
        assert_eq!(format, Format::Gif);
        assert_eq!(image.size(), (3, 1));
        assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(image.pixel(1, 0), Some([0, 255, 0, 255]));
        assert_eq!(
            image.pixel(2, 0),
            Some([0, 0, 0, 0]),
            "colour 0 is transparent"
        );
    }

    #[test]
    fn lzw_handles_the_code_being_defined() {
        // clear, 1, 6 (= "1 1", defined by this very code), end.
        let codes = [4u32, 1, 6, 5];
        let (mut bits, mut held, mut data) = (0u32, 0, Vec::new());
        for code in codes {
            bits |= code << held;
            held += 3;
            while held >= 8 {
                data.push(bits as u8);
                bits >>= 8;
                held -= 8;
            }
        }
        data.push(bits as u8);
        assert_eq!(lzw_decode(&data, 2, 10), Ok(vec![1, 1, 1]));
    }

    #[test]
    fn interlaced_rows_follow_the_four_passes() {
        assert_eq!(interlace_order(8, true), [0, 4, 2, 6, 1, 3, 5, 7]);
        assert_eq!(interlace_order(3, false), [0, 1, 2]);
    }

    #[test]
    fn oversized_headers_are_refused_before_decoding() {
        // A 1-byte "PNG" claiming 100000 x 100000.
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&100_000u32.to_be_bytes());
        png.extend_from_slice(&100_000u32.to_be_bytes());
        let error = decode(&png).expect_err("too large");
        assert!(error.contains("too large"), "{error}");
        let mut gif = gif_3x1();
        gif[6..10].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(decode(&gif).expect_err("too large").contains("too large"));
    }

    #[test]
    fn an_oversized_gif_frame_is_refused_before_decoding() {
        // A 3x1 screen whose frame claims 65535 x 65535: the screen passes the
        // header check, the frame must not reach the LZW buffer.
        let mut gif = gif_3x1();
        let descriptor = gif.iter().position(|b| *b == 0x2C).expect("descriptor");
        gif[descriptor + 5..descriptor + 9].copy_from_slice(&[0xFF; 4]);
        let error = decode(&gif).expect_err("too large");
        assert!(error.contains("GIF frame is too large"), "{error}");
    }

    #[test]
    fn a_jpeg_size_is_found_past_other_segments() {
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend_from_slice(&[0xFF, 0xE0, 0, 4, 0, 0]);
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0, 11, 8, 0, 20, 0, 30, 3]);
        assert_eq!(probe(&jpeg, Format::Jpeg), Ok((30, 20)));
        assert!(probe(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 1], Format::Jpeg).is_err());
    }

    #[test]
    fn noise_never_panics() {
        // A tiny deterministic generator: every prefix of every format gets
        // random tails, which must decode or fail, never panic.
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        };
        let seeds = [
            bmp_24(),
            gif_3x1(),
            b"\x89PNG\r\n\x1a\n".to_vec(),
            vec![0xFF, 0xD8],
        ];
        for seed in seeds {
            for round in 0..300 {
                let mut file = seed.clone();
                let cut = round % (file.len() + 1);
                file.truncate(cut.max(2));
                for _ in 0..(round % 64) {
                    file.push(next());
                }
                if round % 3 == 0 && file.len() > 2 {
                    let at = 2 + usize::from(next()) % (file.len() - 2);
                    file[at] = next();
                }
                let _ = decode(&file);
            }
        }
    }
}
