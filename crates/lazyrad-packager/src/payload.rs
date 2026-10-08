#![forbid(unsafe_code)]

//! The payload archive and its footer.
//!
//! An exported executable is `stub bytes | payload | footer`. The payload is an
//! uncompressed archive of the project's `.lrp`, `.lfm` and `.rhai` files; the
//! footer is fixed-size and sits at the very end of the file so a reader finds
//! it with one seek.
//!
//! ```text
//! payload = entry*                     (entry_count entries, no padding)
//! entry   = name_len:u16  name:utf8  data_len:u64  data
//! footer  = magic:[u8;8]  version:u32  entry_count:u32
//!           offset:u64  length:u64  checksum:u64          (40 bytes, little-endian)
//! ```
//!
//! `offset` is where the payload starts (the stub's length), `length` its size,
//! and `checksum` is FNV-1a 64 over the payload. The checksum catches damage,
//! not tampering: the payload is the program, and anyone who can edit the
//! executable can replace it.
//!
//! Reading validates everything before it trusts anything: the version, that
//! `offset + length + footer` is exactly the file size, the limits, the
//! checksum, then every entry name against the same plain-file-name rule the
//! `.lrp` item paths use.
//!
//! A project `assets` file is stored under the `assets/` prefix
//! ([`ASSET_PREFIX`]) with its project-relative path (`assets/songs/song.mod`),
//! so it cannot be mistaken for an item file and the runtime can read it by the
//! same relative path a project folder uses.

use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use lazyrad_project::{CODE_EXTENSION, FORM_EXTENSION, PROJECT_EXTENSION, Project};

use crate::error::PayloadError;

/// The eight bytes that mark a footer.
pub const MAGIC: [u8; 8] = *b"LAZYRAD\0";
/// The payload format this build reads and writes.
pub const FORMAT_VERSION: u32 = 1;
/// The footer's size in bytes.
pub const FOOTER_LEN: u64 = 40;
/// The most files a payload may hold.
pub const MAX_ENTRIES: usize = 1024;
/// The largest single file, in bytes.
pub const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
/// The largest whole payload, in bytes.
pub const MAX_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;
/// The longest entry name, in bytes.
pub const MAX_NAME_BYTES: usize = 255;
/// The entry-name prefix an asset file is stored under.
pub const ASSET_PREFIX: &str = "assets/";

/// One packed file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The plain file name, for example `main_form.lfm`.
    pub name: String,
    /// The file's bytes.
    pub data: Vec<u8>,
}

/// A validated set of project files: exactly one `.lrp`, and `.lfm` / `.rhai`
/// files, all with plain names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payload {
    entries: Vec<Entry>,
}

impl Payload {
    /// Builds a payload from `entries`, applying every limit and name check.
    pub fn new(entries: Vec<Entry>) -> Result<Payload, PayloadError> {
        if entries.len() > MAX_ENTRIES {
            return Err(PayloadError::TooManyEntries {
                count: entries.len(),
                max: MAX_ENTRIES,
            });
        }
        let mut seen = BTreeSet::new();
        let mut total: u64 = 0;
        let mut projects = 0;
        for entry in &entries {
            check_name(&entry.name)?;
            if !seen.insert(entry.name.to_lowercase()) {
                return Err(PayloadError::Duplicate(entry.name.clone()));
            }
            let size = entry.data.len() as u64;
            if size > MAX_ENTRY_BYTES {
                return Err(PayloadError::TooLarge(format!(
                    "`{}` is {size} bytes; the limit is {MAX_ENTRY_BYTES}",
                    entry.name
                )));
            }
            total = total.saturating_add(size);
            if !is_asset_name(&entry.name) && has_extension(&entry.name, PROJECT_EXTENSION) {
                projects += 1;
            }
        }
        if total > MAX_PAYLOAD_BYTES {
            return Err(PayloadError::TooLarge(format!(
                "the files total {total} bytes; the limit is {MAX_PAYLOAD_BYTES}"
            )));
        }
        if projects != 1 {
            return Err(PayloadError::ProjectFileCount(projects));
        }
        Ok(Payload { entries })
    }

    /// Reads the project named by `lrp` and every file it references from its
    /// folder.
    ///
    /// The `.lrp` goes through [`Project::load_file`], so item paths are held
    /// to the same rules as everywhere else and links are refused. The icon is
    /// not packed: it becomes a resource of the executable.
    pub fn from_project(lrp: &Path) -> Result<Payload, PayloadError> {
        let project = Project::load_file(lrp)?;
        let dir = lrp
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let lrp_name = project.file_name();

        let mut entries = vec![Entry {
            data: read_regular(&dir.join(&lrp_name), MAX_ENTRY_BYTES)?,
            name: lrp_name,
        }];
        let mut seen = BTreeSet::new();
        for item in &project.items {
            for relative in std::iter::once(item.code()).chain(item.layout()) {
                let name = relative.to_string_lossy().into_owned();
                if seen.insert(name.clone()) {
                    entries.push(Entry {
                        data: read_regular(&dir.join(relative), MAX_ENTRY_BYTES)?,
                        name,
                    });
                }
            }
        }
        // A glob that matches nothing is a warning, not a failure; the packager
        // has nowhere to show it, so the assets are simply left out.
        let (assets, _warnings) = lazyrad_project::collect_assets(dir, &project.assets)?;
        for relative in assets {
            entries.push(Entry {
                data: read_regular(&dir.join(&relative), MAX_ENTRY_BYTES)?,
                name: format!("{ASSET_PREFIX}{relative}"),
            });
        }
        Payload::new(entries)
    }

    /// The files, in the order they were packed.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The bytes of the file called `name`, if the payload has it.
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.data.as_slice())
    }

    /// The payload's one `.lrp` entry.
    pub fn project_entry(&self) -> &Entry {
        self.entries
            .iter()
            .find(|entry| {
                !is_asset_name(&entry.name) && has_extension(&entry.name, PROJECT_EXTENSION)
            })
            .expect("Payload::new guarantees exactly one .lrp")
    }

    /// The asset entries, as `(project-relative path, bytes)`, in packed order.
    pub fn assets(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries.iter().filter_map(|entry| {
            entry
                .name
                .strip_prefix(ASSET_PREFIX)
                .map(|rel| (rel, &entry.data[..]))
        })
    }

    /// The archive bytes (without the footer).
    pub fn to_body(&self) -> Vec<u8> {
        let mut body = Vec::new();
        for entry in &self.entries {
            let name = entry.name.as_bytes();
            body.extend_from_slice(&(name.len() as u16).to_le_bytes());
            body.extend_from_slice(name);
            body.extend_from_slice(&(entry.data.len() as u64).to_le_bytes());
            body.extend_from_slice(&entry.data);
        }
        body
    }

    /// Writes the payload and its footer to `out`, given that the payload will
    /// start `offset` bytes into the file.
    pub fn write_to(&self, out: &mut impl Write, offset: u64) -> std::io::Result<()> {
        let body = self.to_body();
        out.write_all(&body)?;
        let footer = Footer {
            version: FORMAT_VERSION,
            entry_count: self.entries.len() as u32,
            offset,
            length: body.len() as u64,
            checksum: checksum(&body),
        };
        out.write_all(&footer.to_bytes())
    }

    /// Looks for a payload at the end of `file`.
    ///
    /// `Ok(None)` means there is no footer (an ordinary player); anything that
    /// has the magic but does not check out is an error, so a damaged export
    /// says so instead of quietly behaving like a bare player.
    pub fn read_from<R: Read + Seek>(file: &mut R) -> Result<Option<Payload>, PayloadError> {
        let no_path = Path::new("");
        let len = file
            .seek(SeekFrom::End(0))
            .map_err(|source| PayloadError::io(no_path, source))?;
        if len < FOOTER_LEN {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(len - FOOTER_LEN))
            .map_err(|source| PayloadError::io(no_path, source))?;
        let mut raw = [0u8; FOOTER_LEN as usize];
        file.read_exact(&mut raw)
            .map_err(|source| PayloadError::io(no_path, source))?;
        if raw[..8] != MAGIC {
            return Ok(None);
        }
        let footer = Footer::from_bytes(&raw);
        if footer.version != FORMAT_VERSION {
            return Err(PayloadError::UnsupportedVersion {
                found: footer.version,
                supported: FORMAT_VERSION,
            });
        }
        if footer.length > MAX_PAYLOAD_BYTES {
            return Err(PayloadError::TooLarge(format!(
                "the payload claims {} bytes; the limit is {MAX_PAYLOAD_BYTES}",
                footer.length
            )));
        }
        if footer.entry_count as usize > MAX_ENTRIES {
            return Err(PayloadError::TooManyEntries {
                count: footer.entry_count as usize,
                max: MAX_ENTRIES,
            });
        }
        let end = footer.offset.checked_add(footer.length);
        match end {
            Some(end) if end.saturating_add(FOOTER_LEN) == len => {}
            Some(end) if end.saturating_add(FOOTER_LEN) > len => {
                return Err(PayloadError::Truncated(format!(
                    "the footer expects {} bytes but the file has {len}",
                    end.saturating_add(FOOTER_LEN)
                )));
            }
            _ => {
                return Err(PayloadError::Corrupt(
                    "the footer's offset and length do not match the file size".to_owned(),
                ));
            }
        }
        file.seek(SeekFrom::Start(footer.offset))
            .map_err(|source| PayloadError::io(no_path, source))?;
        let mut body = vec![0u8; footer.length as usize];
        file.read_exact(&mut body)
            .map_err(|source| PayloadError::io(no_path, source))?;
        if checksum(&body) != footer.checksum {
            return Err(PayloadError::Corrupt(
                "the checksum does not match".to_owned(),
            ));
        }
        Payload::decode(&body, footer.entry_count as usize).map(Some)
    }

    /// Parses the archive bytes, expecting exactly `count` entries and no
    /// trailing bytes.
    fn decode(body: &[u8], count: usize) -> Result<Payload, PayloadError> {
        let mut cursor = Cursor { rest: body };
        let mut entries = Vec::with_capacity(count);
        for index in 0..count {
            let name_len = usize::from(cursor.u16().ok_or_else(|| truncated_entry(index))?);
            let name = cursor
                .take(name_len)
                .ok_or_else(|| truncated_entry(index))?;
            let name = std::str::from_utf8(name)
                .map_err(|_| PayloadError::Corrupt(format!("entry {index} has a non-UTF-8 name")))?
                .to_owned();
            let data_len = cursor.u64().ok_or_else(|| truncated_entry(index))?;
            if data_len > MAX_ENTRY_BYTES {
                return Err(PayloadError::TooLarge(format!(
                    "entry `{name}` claims {data_len} bytes; the limit is {MAX_ENTRY_BYTES}"
                )));
            }
            let data = cursor
                .take(data_len as usize)
                .ok_or_else(|| truncated_entry(index))?
                .to_vec();
            entries.push(Entry { name, data });
        }
        if !cursor.rest.is_empty() {
            return Err(PayloadError::Corrupt(format!(
                "{} unexpected bytes after the last entry",
                cursor.rest.len()
            )));
        }
        Payload::new(entries)
    }
}

/// Whether `bytes` ends with a payload footer's magic at the footer position.
pub fn has_footer(bytes: &[u8]) -> bool {
    bytes.len() as u64 >= FOOTER_LEN && bytes[bytes.len() - FOOTER_LEN as usize..][..8] == MAGIC
}

/// The fixed-size record at the end of the file.
struct Footer {
    version: u32,
    entry_count: u32,
    offset: u64,
    length: u64,
    checksum: u64,
}

impl Footer {
    fn to_bytes(&self) -> [u8; FOOTER_LEN as usize] {
        let mut out = [0u8; FOOTER_LEN as usize];
        out[..8].copy_from_slice(&MAGIC);
        out[8..12].copy_from_slice(&self.version.to_le_bytes());
        out[12..16].copy_from_slice(&self.entry_count.to_le_bytes());
        out[16..24].copy_from_slice(&self.offset.to_le_bytes());
        out[24..32].copy_from_slice(&self.length.to_le_bytes());
        out[32..40].copy_from_slice(&self.checksum.to_le_bytes());
        out
    }

    fn from_bytes(raw: &[u8; FOOTER_LEN as usize]) -> Footer {
        let word = |from: usize| -> [u8; 8] { raw[from..from + 8].try_into().expect("8 bytes") };
        Footer {
            version: u32::from_le_bytes(raw[8..12].try_into().expect("4 bytes")),
            entry_count: u32::from_le_bytes(raw[12..16].try_into().expect("4 bytes")),
            offset: u64::from_le_bytes(word(16)),
            length: u64::from_le_bytes(word(24)),
            checksum: u64::from_le_bytes(word(32)),
        }
    }
}

/// A forward-only reader over the archive bytes.
struct Cursor<'a> {
    rest: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.rest.len() < n {
            return None;
        }
        let (head, tail) = self.rest.split_at(n);
        self.rest = tail;
        Some(head)
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

fn truncated_entry(index: usize) -> PayloadError {
    PayloadError::Truncated(format!("entry {index} runs past the end of the data"))
}

/// FNV-1a, 64 bit.
fn checksum(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn has_extension(name: &str, extension: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|found| found.to_str())
        .is_some_and(|found| found.eq_ignore_ascii_case(extension))
}

/// Applies the plain-file-name rule (the same one `.lrp` item paths use) plus
/// the payload's own: a short name, no separators or control characters, and
/// one of the three project file extensions.
///
/// An asset entry is instead named `assets/<relative>`; its relative path may
/// contain `/` but must stay inside the project (no `..`, no absolute path, no
/// `\`). Its extension is free, since an asset is any file the project ships.
fn check_name(name: &str) -> Result<(), PayloadError> {
    if let Some(relative) = name.strip_prefix(ASSET_PREFIX) {
        return check_asset_path(name, relative);
    }
    let bad = |reason: &'static str| PayloadError::BadName {
        name: name.to_owned(),
        reason,
    };
    if name.is_empty() {
        return Err(bad("it is empty"));
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(bad("it is too long"));
    }
    if name.chars().any(|c| {
        c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    }) {
        return Err(bad("it contains a path separator or a reserved character"));
    }
    if !lazyrad_project::is_plain_file_name(Path::new(name)) {
        return Err(bad("it is not a plain file name"));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(bad("it ends with a dot or a space"));
    }
    let allowed = [PROJECT_EXTENSION, FORM_EXTENSION, CODE_EXTENSION];
    if !allowed
        .iter()
        .any(|extension| has_extension(name, extension))
    {
        return Err(bad("only .lrp, .lfm and .rhai files are packed"));
    }
    Ok(())
}

/// Checks an asset entry's `relative` path (the part after `assets/`).
fn check_asset_path(name: &str, relative: &str) -> Result<(), PayloadError> {
    let bad = |reason: &'static str| PayloadError::BadName {
        name: name.to_owned(),
        reason,
    };
    if name.len() > MAX_NAME_BYTES {
        return Err(bad("it is too long"));
    }
    if relative.is_empty() || relative.starts_with('/') || relative.contains('\\') {
        return Err(bad("it is not a relative path inside the project"));
    }
    for component in relative.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(bad("it contains an unsafe path component"));
        }
        if component
            .chars()
            .any(|c| c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        {
            return Err(bad("it contains a reserved character"));
        }
    }
    Ok(())
}

/// Whether `name` is an asset entry (a path under the `assets/` prefix).
fn is_asset_name(name: &str) -> bool {
    name.starts_with(ASSET_PREFIX)
}

/// Reads a regular file of at most `limit` bytes, refusing links, folders and
/// oversized files without reading them whole.
pub(crate) fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>, PayloadError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| PayloadError::io(path, source))?;
    if !metadata.file_type().is_file() {
        return Err(PayloadError::NotRegular(path.to_path_buf()));
    }
    if metadata.len() > limit {
        return Err(PayloadError::TooLarge(format!(
            "`{}` is {} bytes; the limit is {limit}",
            path.display(),
            metadata.len()
        )));
    }
    let file = fs::File::open(path).map_err(|source| PayloadError::io(path, source))?;
    // Read one byte past the limit so a file that grew after the check is
    // still caught.
    let mut data = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|source| PayloadError::io(path, source))?;
    if data.len() as u64 > limit {
        return Err(PayloadError::TooLarge(format!(
            "`{}` is larger than {limit} bytes",
            path.display()
        )));
    }
    Ok(data)
}

/// Encodes `entries` and a footer with no validation, for tests that need to
/// build hostile payloads with a valid checksum.
#[doc(hidden)]
pub fn encode_unchecked(entries: &[(&str, &[u8])], offset: u64, version: u32) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, data) in entries {
        body.extend_from_slice(&(name.len() as u16).to_le_bytes());
        body.extend_from_slice(name.as_bytes());
        body.extend_from_slice(&(data.len() as u64).to_le_bytes());
        body.extend_from_slice(data);
    }
    let footer = Footer {
        version,
        entry_count: entries.len() as u32,
        offset,
        length: body.len() as u64,
        checksum: checksum(&body),
    };
    body.extend_from_slice(&footer.to_bytes());
    body
}
