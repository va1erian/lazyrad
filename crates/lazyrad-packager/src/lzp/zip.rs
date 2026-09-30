#![forbid(unsafe_code)]

//! A minimal zip writer for LazyOS `.lzp` packages.
//!
//! LazyOS's `libs/lazypkg` reader accepts a strict subset of zip, and this writer
//! produces exactly that subset (`docs/packages.md` §1-§2 in the LazyOS repo):
//!
//! * methods 0 (stored) and 8 (deflate) only; `.png` entries are stored, every
//!   other entry is deflated unless deflating does not make it smaller;
//! * no zip64, no encryption, no data descriptors, no multi-disk, no comments;
//! * entry names are relative, `/`-separated, at most 255 bytes, free of control
//!   characters, backslashes, drive letters and `.`/`..` components, and no two
//!   names may be equal or differ only in case;
//! * at most [`MAX_ENTRIES`] entries, [`MAX_ENTRY_UNCOMPRESSED`] bytes per entry
//!   and [`MAX_TOTAL_UNCOMPRESSED`] bytes in all.
//!
//! The limits are checked **before** any data is compressed, so a hostile or
//! careless project cannot make the writer allocate or spin. Timestamps are
//! fixed (1980-01-01), so the same input always produces the same bytes.

use std::collections::BTreeSet;

use miniz_oxide::deflate::compress_to_vec;

/// Most entries in a package (`MAX_ENTRIES` in `lazypkg`).
pub const MAX_ENTRIES: usize = 1024;
/// Most bytes all entries may expand to.
pub const MAX_TOTAL_UNCOMPRESSED: u64 = 64 * 1024 * 1024;
/// Most bytes one entry may expand to.
pub const MAX_ENTRY_UNCOMPRESSED: u64 = 16 * 1024 * 1024;
/// Longest entry name, in bytes.
pub const MAX_NAME_LEN: usize = 255;

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;
/// "Version needed to extract": 2.0 (deflate).
const VERSION: u16 = 20;
/// DOS date for 1980-01-01; the time is midnight.
const DOS_DATE: u16 = (1 << 5) | 1;
/// miniz level 6: the usual speed/size trade-off.
const DEFLATE_LEVEL: u8 = 6;

/// Why an entry or the archive was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ZipError {
    /// The entry name breaks a path rule.
    #[error("entry `{name}` {reason}")]
    BadName {
        /// The offending name.
        name: String,
        /// Which rule it breaks, phrased to follow the name.
        reason: &'static str,
    },
    /// An entry with this name (or one differing only in case) exists.
    #[error("entry `{0}` is already in the package (names are case-insensitive)")]
    Duplicate(String),
    /// Adding the entry would exceed [`MAX_ENTRIES`].
    #[error("too many entries: the package is limited to {MAX_ENTRIES}")]
    TooManyEntries,
    /// One entry is larger than [`MAX_ENTRY_UNCOMPRESSED`].
    #[error("`{name}` is {size} bytes; one file may be at most {MAX_ENTRY_UNCOMPRESSED}")]
    EntryTooLarge {
        /// The entry.
        name: String,
        /// Its size in bytes.
        size: u64,
    },
    /// The entries together exceed [`MAX_TOTAL_UNCOMPRESSED`].
    #[error("the package would expand to {0} bytes; the limit is {MAX_TOTAL_UNCOMPRESSED}")]
    TotalTooLarge(u64),
}

/// One record kept for the central directory.
struct Record {
    name: String,
    method: u16,
    crc: u32,
    compressed: u32,
    size: u32,
    offset: u32,
}

/// Builds an archive in memory, one entry at a time.
#[derive(Default)]
pub struct ZipWriter {
    out: Vec<u8>,
    records: Vec<Record>,
    folded: BTreeSet<String>,
    total: u64,
}

impl ZipWriter {
    /// An empty archive.
    pub fn new() -> ZipWriter {
        ZipWriter::default()
    }

    /// The number of entries so far.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether no entry has been added.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Adds `data` as the file `name`.
    pub fn add(&mut self, name: &str, data: &[u8]) -> Result<(), ZipError> {
        check_name(name)?;
        if self.records.len() >= MAX_ENTRIES {
            return Err(ZipError::TooManyEntries);
        }
        let size = data.len() as u64;
        if size > MAX_ENTRY_UNCOMPRESSED {
            return Err(ZipError::EntryTooLarge {
                name: name.to_owned(),
                size,
            });
        }
        let total = self.total + size;
        if total > MAX_TOTAL_UNCOMPRESSED {
            return Err(ZipError::TotalTooLarge(total));
        }
        if !self.folded.insert(fold_case(name)) {
            return Err(ZipError::Duplicate(name.to_owned()));
        }
        self.total = total;

        let crc = crc32(data);
        let deflated = (!name.ends_with(".png")).then(|| compress_to_vec(data, DEFLATE_LEVEL));
        let (method, body): (u16, &[u8]) = match &deflated {
            Some(packed) if packed.len() < data.len() => (8, packed),
            _ => (0, data),
        };
        // All three are bounded by `MAX_ENTRY_UNCOMPRESSED` (16 MiB) and the
        // archive by 64 MiB of input, so they fit a u32. Deflate of
        // incompressible data can exceed its input slightly, but then the
        // stored body is chosen above.
        let offset = self.out.len() as u32;
        self.write_local(name, method, crc, body.len() as u32, data.len() as u32);
        self.out.extend_from_slice(body);
        self.records.push(Record {
            name: name.to_owned(),
            method,
            crc,
            compressed: body.len() as u32,
            size: data.len() as u32,
            offset,
        });
        Ok(())
    }

    /// Writes the central directory and returns the finished archive.
    pub fn finish(mut self) -> Vec<u8> {
        let directory = self.out.len() as u32;
        let count = self.records.len() as u16;
        for record in std::mem::take(&mut self.records) {
            self.write_central(&record);
        }
        let size = self.out.len() as u32 - directory;
        push_u32(&mut self.out, EOCD_SIG);
        push_u16(&mut self.out, 0); // this disk
        push_u16(&mut self.out, 0); // disk with the directory
        push_u16(&mut self.out, count);
        push_u16(&mut self.out, count);
        push_u32(&mut self.out, size);
        push_u32(&mut self.out, directory);
        push_u16(&mut self.out, 0); // comment length
        self.out
    }

    fn write_local(&mut self, name: &str, method: u16, crc: u32, compressed: u32, size: u32) {
        push_u32(&mut self.out, LOCAL_SIG);
        push_u16(&mut self.out, VERSION);
        push_u16(&mut self.out, 0); // flags: no encryption, no descriptor
        push_u16(&mut self.out, method);
        push_u16(&mut self.out, 0); // time
        push_u16(&mut self.out, DOS_DATE);
        push_u32(&mut self.out, crc);
        push_u32(&mut self.out, compressed);
        push_u32(&mut self.out, size);
        push_u16(&mut self.out, name.len() as u16);
        push_u16(&mut self.out, 0); // extra length
        self.out.extend_from_slice(name.as_bytes());
    }

    fn write_central(&mut self, record: &Record) {
        push_u32(&mut self.out, CENTRAL_SIG);
        push_u16(&mut self.out, VERSION); // made by
        push_u16(&mut self.out, VERSION); // needed
        push_u16(&mut self.out, 0); // flags
        push_u16(&mut self.out, record.method);
        push_u16(&mut self.out, 0); // time
        push_u16(&mut self.out, DOS_DATE);
        push_u32(&mut self.out, record.crc);
        push_u32(&mut self.out, record.compressed);
        push_u32(&mut self.out, record.size);
        push_u16(&mut self.out, record.name.len() as u16);
        push_u16(&mut self.out, 0); // extra
        push_u16(&mut self.out, 0); // comment
        push_u16(&mut self.out, 0); // disk start
        push_u16(&mut self.out, 0); // internal attributes
        push_u32(&mut self.out, 0); // external attributes
        push_u32(&mut self.out, record.offset);
        self.out.extend_from_slice(record.name.as_bytes());
    }
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Case folding for the collision check. The target filesystem may fold case, so
/// `Bin/A.elf` and `bin/a.elf` are the same name; Unicode simple lowercase is
/// what a case-folding volume does.
fn fold_case(name: &str) -> String {
    name.to_lowercase()
}

/// Applies the reader's name rules (`libs/lazypkg/src/path.rs`) and the layout
/// rules (`layout.rs`): only `manifest.toml` at the top, else one of the five
/// directories with its extension.
pub fn check_name(name: &str) -> Result<(), ZipError> {
    let bad = |reason| ZipError::BadName {
        name: name.to_owned(),
        reason,
    };
    if name.is_empty() {
        return Err(bad("is empty"));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(bad("is longer than 255 bytes"));
    }
    if name.chars().any(char::is_control) {
        return Err(bad("contains a control character"));
    }
    if name.starts_with('/') {
        return Err(bad("is absolute"));
    }
    if name.contains('\\') {
        return Err(bad("contains a backslash"));
    }
    let bytes = name.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(bad("has a drive letter"));
    }
    for component in name.split('/') {
        match component {
            "" => return Err(bad("has an empty path component")),
            "." | ".." => return Err(bad("contains a `.` or `..` component")),
            _ => {}
        }
    }
    if name.ends_with('/') {
        return Err(bad("is a directory; only files are written"));
    }
    check_layout(name).map_err(bad)
}

/// The package layout rule for a file `name` (see `layout.rs` in `lazypkg`).
fn check_layout(name: &str) -> Result<(), &'static str> {
    if name == "manifest.toml" {
        return Ok(());
    }
    let top = name.split('/').next().unwrap_or("");
    let extension_ok = match top {
        "bin" => name.ends_with(".elf"),
        "icons" => name.ends_with(".png"),
        "idl" => name.ends_with(".midl"),
        "docs" => name.ends_with(".md"),
        "resources" => true,
        _ => return Err("is not under an allowed top-level directory"),
    };
    if extension_ok && name.contains('/') {
        Ok(())
    } else {
        Err("has the wrong file extension or is not inside its directory")
    }
}

/// CRC-32 (IEEE 802.3, the zip polynomial) of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut n = 0;
        while n < 256 {
            let mut c = n as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            table[n] = c;
            n += 1;
        }
        table
    };
    let mut crc = !0u32;
    for &byte in data {
        crc = TABLE[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn plain_names_are_accepted() {
        for name in [
            "manifest.toml",
            "bin/lrplay.elf",
            "icons/app-16.png",
            "resources/project/main.lfm",
            "docs/README.md",
            "idl/x.midl",
        ] {
            assert_eq!(check_name(name), Ok(()), "{name}");
        }
    }

    #[test]
    fn unsafe_and_off_layout_names_are_refused() {
        for name in [
            "",
            "/abs",
            "a\\b",
            "C:x",
            "bin/../x.elf",
            "./bin/a.elf",
            "bin//a.elf",
            "bin/a\u{0}.elf",
            "bin/a\n.elf",
            "bin/",
            "other/file",
            "bin/readme.txt",
            "icons/a.jpg",
            "resources",
            "bin",
            &format!("resources/{}", "x".repeat(256)),
        ] {
            assert!(check_name(name).is_err(), "{name:?} must be refused");
        }
    }

    #[test]
    fn duplicates_and_case_collisions_are_refused() {
        let mut zip = ZipWriter::new();
        zip.add("resources/A.txt", b"a").unwrap();
        assert!(matches!(
            zip.add("resources/A.txt", b"b"),
            Err(ZipError::Duplicate(_))
        ));
        assert!(matches!(
            zip.add("resources/a.TXT", b"b"),
            Err(ZipError::Duplicate(_))
        ));
        assert_eq!(zip.len(), 1);
    }

    #[test]
    fn limits_are_enforced_before_compressing() {
        let mut zip = ZipWriter::new();
        let big = vec![0u8; (MAX_ENTRY_UNCOMPRESSED + 1) as usize];
        assert!(matches!(
            zip.add("resources/big", &big),
            Err(ZipError::EntryTooLarge { .. })
        ));
        // Four 16 MiB entries fill the 64 MiB total; a fifth byte cannot fit.
        let full = vec![0u8; MAX_ENTRY_UNCOMPRESSED as usize];
        for n in 0..4 {
            zip.add(&format!("resources/f{n}"), &full).unwrap();
        }
        assert!(matches!(
            zip.add("resources/one_more", b"x"),
            Err(ZipError::TotalTooLarge(_))
        ));
    }

    #[test]
    fn the_entry_count_limit_is_exact() {
        let mut zip = ZipWriter::new();
        for n in 0..MAX_ENTRIES {
            zip.add(&format!("resources/{n}"), b"x").unwrap();
        }
        assert_eq!(
            zip.add("resources/over", b"x"),
            Err(ZipError::TooManyEntries)
        );
        let bytes = zip.finish();
        // EOCD: entry count is at offset 10 from its start (22 bytes from the end).
        let eocd = bytes.len() - 22;
        assert_eq!(
            u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]) as usize,
            MAX_ENTRIES
        );
    }

    #[test]
    fn output_is_deterministic() {
        let build = || {
            let mut zip = ZipWriter::new();
            zip.add("manifest.toml", b"hello = 1\n".repeat(50).as_slice())
                .unwrap();
            zip.add("icons/app-16.png", b"\x89PNG\r\n\x1a\nxx").unwrap();
            zip.finish()
        };
        assert_eq!(build(), build());
    }
}
