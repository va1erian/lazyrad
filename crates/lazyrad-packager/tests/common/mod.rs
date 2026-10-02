//! An independent verifier for `.lzp` packages, used by every packager test.
//!
//! It is written from the LazyOS specification (`docs/packages.md`), not from
//! the writer's code, so a writer bug cannot hide behind a matching reader bug.
//! The LazyOS repository re-opens the same packages with `libs/lazypkg` and
//! `tools/pkg/build.py`. Every rule the reader enforces is checked here.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

const MAX_ENTRIES: usize = 1024;
const MAX_TOTAL: u64 = 64 * 1024 * 1024;
const MAX_ENTRY: u64 = 16 * 1024 * 1024;

/// What a valid package holds.
pub struct Verified {
    /// Entry name -> decompressed contents, in archive order of names.
    pub files: BTreeMap<String, Vec<u8>>,
    /// The parsed `manifest.toml`.
    pub manifest: toml::Table,
    /// Compression method per entry (0 stored, 8 deflate).
    pub methods: BTreeMap<String, u16>,
}

impl Verified {
    /// The manifest string at `app.<key>`.
    pub fn app(&self, key: &str) -> &str {
        self.manifest["app"][key].as_str().expect("app string")
    }

    /// The strings at `permissions.<key>`.
    pub fn permission(&self, key: &str) -> Vec<String> {
        self.manifest["permissions"][key]
            .as_array()
            .expect("permissions array")
            .iter()
            .map(|v| v.as_str().expect("string").to_owned())
            .collect()
    }
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| format!("truncated at {at}"))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| format!("truncated at {at}"))
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn check_name(name: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("entry {name:?}: {why}"));
    if name.is_empty() || name.len() > 255 {
        return bad("empty or longer than 255 bytes");
    }
    if name.chars().any(char::is_control) {
        return bad("control character");
    }
    if name.starts_with('/') || name.contains('\\') {
        return bad("absolute or backslash");
    }
    let b = name.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return bad("drive letter");
    }
    let path = name.strip_suffix('/').unwrap_or(name);
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return bad("empty, `.` or `..` component");
        }
    }
    Ok(())
}

fn check_layout(name: &str, is_dir: bool) -> Result<(), String> {
    if is_dir {
        return Ok(());
    }
    if name == "manifest.toml" {
        return Ok(());
    }
    let top = name.split('/').next().unwrap_or("");
    let ok = match top {
        "bin" => name.ends_with(".elf"),
        "icons" => name.ends_with(".png"),
        "idl" => name.ends_with(".midl"),
        "docs" => name.ends_with(".md"),
        "resources" => true,
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("entry {name:?} breaks the layout rules"))
    }
}

/// LazyOS's `[permissions] files` grammar (`lazypkg::files`, filesystem plan
/// F5): `read:` or `write:`, then a path that is absolute or starts with
/// `$HOME/`; `$HOME` only as the first segment; segments `[A-Za-z0-9_.-]+` or
/// `*`, no `..`; and no absolute path inside a home directory (`/home/...` or
/// the legacy `/data/home/...`), which must be written with `$HOME`.
fn files_rule_ok(rule: &str) -> bool {
    let Some(path) = rule
        .strip_prefix("read:")
        .or_else(|| rule.strip_prefix("write:"))
    else {
        return false;
    };
    let (relative, rest) = match path.strip_prefix("$HOME") {
        Some(rest) => (true, rest),
        None => (false, path),
    };
    let Some(rest) = rest.strip_prefix('/') else {
        return false;
    };
    let segments_ok = rest.split('/').all(|seg| {
        !seg.is_empty()
            && seg != ".."
            && seg
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-*".contains(&b))
    });
    let in_home = |root: &str| path == root || path.starts_with(&format!("{root}/"));
    segments_ok && (relative || !(in_home("/home") || in_home("/data/home")))
}

/// Verifies `bytes` against every container, name, layout, limit and manifest
/// rule, returning the contents.
pub fn verify(bytes: &[u8]) -> Result<Verified, String> {
    // EOCD: scan back over at most 64 KiB + 22 bytes of comment.
    if bytes.len() < 22 {
        return Err("too short".into());
    }
    let lowest = bytes.len().saturating_sub(22 + 0xFFFF);
    let mut eocd = None;
    let mut at = bytes.len() - 22;
    loop {
        if u32_at(bytes, at)? == 0x0605_4b50 {
            let comment = u16_at(bytes, at + 20)? as usize;
            if at + 22 + comment == bytes.len() {
                eocd = Some(at);
                break;
            }
        }
        if at == lowest {
            break;
        }
        at -= 1;
    }
    let eocd = eocd.ok_or("no end of central directory")?;
    let disk = u16_at(bytes, eocd + 4)?;
    let dir_disk = u16_at(bytes, eocd + 6)?;
    let here = u16_at(bytes, eocd + 8)?;
    let count = u16_at(bytes, eocd + 10)?;
    let dir_size = u32_at(bytes, eocd + 12)? as usize;
    let dir_offset = u32_at(bytes, eocd + 16)? as usize;
    if disk != 0 || dir_disk != 0 || here != count {
        return Err("multi-disk".into());
    }
    if count == u16::MAX || dir_size == u32::MAX as usize || dir_offset == u32::MAX as usize {
        return Err("zip64 sentinel".into());
    }
    if count as usize > MAX_ENTRIES {
        return Err(format!("too many entries: {count}"));
    }
    if dir_offset + dir_size > eocd {
        return Err("central directory overlaps the EOCD".into());
    }

    let mut files = BTreeMap::new();
    let mut methods = BTreeMap::new();
    let mut folded = BTreeSet::new();
    let mut total = 0u64;
    let mut at = dir_offset;
    for _ in 0..count {
        if u32_at(bytes, at)? != 0x0201_4b50 {
            return Err("bad central record".into());
        }
        let flags = u16_at(bytes, at + 8)?;
        let method = u16_at(bytes, at + 10)?;
        let crc = u32_at(bytes, at + 16)?;
        let compressed = u32_at(bytes, at + 20)? as usize;
        let size = u32_at(bytes, at + 24)? as usize;
        let name_len = u16_at(bytes, at + 28)? as usize;
        let extra_len = u16_at(bytes, at + 30)? as usize;
        let comment_len = u16_at(bytes, at + 32)? as usize;
        let local = u32_at(bytes, at + 42)? as usize;
        let name_bytes = bytes
            .get(at + 46..at + 46 + name_len)
            .ok_or("truncated name")?;
        let name = std::str::from_utf8(name_bytes).map_err(|_| "non-UTF-8 name")?;
        if extra_len != 0 {
            return Err(format!("{name}: extra fields are not written (zip64 risk)"));
        }
        let is_dir = name.ends_with('/');
        check_name(name)?;
        check_layout(name, is_dir)?;
        if flags & 0x0001 != 0 || flags & 0x0008 != 0 {
            return Err(format!("{name}: encryption or data descriptor"));
        }
        if method != 0 && method != 8 {
            return Err(format!("{name}: method {method}"));
        }
        if !folded.insert(name.to_lowercase()) {
            return Err(format!("{name}: duplicate or case collision"));
        }
        if size as u64 > MAX_ENTRY {
            return Err(format!("{name}: entry too large"));
        }
        total += size as u64;
        if total > MAX_TOTAL {
            return Err("total too large".into());
        }

        // The local header must agree with the central record.
        if local >= dir_offset || u32_at(bytes, local)? != 0x0403_4b50 {
            return Err(format!("{name}: bad local header"));
        }
        let l_flags = u16_at(bytes, local + 6)?;
        let l_method = u16_at(bytes, local + 8)?;
        let l_crc = u32_at(bytes, local + 14)?;
        let l_comp = u32_at(bytes, local + 18)? as usize;
        let l_size = u32_at(bytes, local + 22)? as usize;
        let l_name = u16_at(bytes, local + 26)? as usize;
        let l_extra = u16_at(bytes, local + 28)? as usize;
        if l_flags & 0x0009 != 0
            || l_method != method
            || l_crc != crc
            || l_comp != compressed
            || l_size != size
            || bytes.get(local + 30..local + 30 + l_name) != Some(name.as_bytes())
        {
            return Err(format!("{name}: local header disagrees with the directory"));
        }
        let start = local + 30 + l_name + l_extra;
        let end = start + compressed;
        if end > dir_offset {
            return Err(format!("{name}: data runs into the directory"));
        }
        let raw = &bytes[start..end];
        let data = match method {
            0 => {
                if compressed != size {
                    return Err(format!("{name}: stored size mismatch"));
                }
                raw.to_vec()
            }
            _ => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, size)
                .map_err(|e| format!("{name}: inflate failed: {e:?}"))?,
        };
        if data.len() != size {
            return Err(format!(
                "{name}: inflated to {} bytes, declared {size}",
                data.len()
            ));
        }
        if crc32(&data) != crc {
            return Err(format!("{name}: CRC mismatch"));
        }
        if !is_dir {
            files.insert(name.to_owned(), data);
            methods.insert(name.to_owned(), method);
        }
        at += 46 + name_len + extra_len + comment_len;
    }
    if at != dir_offset + dir_size {
        return Err("central directory size mismatch".into());
    }

    // Structure: manifest, three PNG icons, the entry binary.
    let manifest_bytes = files.get("manifest.toml").ok_or("no manifest.toml")?;
    if manifest_bytes.len() > 1024 * 1024 {
        return Err("manifest larger than 1 MiB".into());
    }
    for icon in ["icons/app-16.png", "icons/app-32.png", "icons/app-128.png"] {
        let data = files.get(icon).ok_or_else(|| format!("missing {icon}"))?;
        if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(format!("{icon} is not a PNG"));
        }
        if methods[icon] != 0 {
            return Err(format!("{icon} must be stored"));
        }
    }
    let manifest: toml::Table = std::str::from_utf8(manifest_bytes)
        .map_err(|_| "manifest is not UTF-8")?
        .parse()
        .map_err(|e| format!("manifest does not parse: {e}"))?;
    let verified = Verified {
        files,
        manifest,
        methods,
    };
    check_manifest(&verified)?;
    Ok(verified)
}

fn check_manifest(v: &Verified) -> Result<(), String> {
    let table = &v.manifest;
    let allowed_top = ["app", "entry", "mime", "permissions"];
    for key in table.keys() {
        if !allowed_top.contains(&key.as_str()) {
            return Err(format!("unknown manifest table {key}"));
        }
    }
    let app = table
        .get("app")
        .and_then(|v| v.as_table())
        .ok_or("no [app]")?;
    let entry = table
        .get("entry")
        .and_then(|v| v.as_table())
        .ok_or("no [entry]")?;
    let s = |t: &toml::Table, k: &str| -> Result<String, String> {
        t.get(k)
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .ok_or_else(|| format!("missing string {k}"))
    };
    for key in app.keys() {
        if !["name", "system_name", "author", "version", "description"].contains(&key.as_str()) {
            return Err(format!("unknown app field {key}"));
        }
    }
    let name = s(app, "name")?;
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err("app.name".into());
    }
    let author = s(app, "author")?;
    if author.is_empty() || author.chars().count() > 128 {
        return Err("app.author".into());
    }
    let system_name = s(app, "system_name")?;
    let labels: Vec<&str> = system_name.split('.').collect();
    let label_ok = |l: &str| {
        !l.is_empty()
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    if system_name.len() > 128 || labels.len() < 3 || !labels.iter().all(|l| label_ok(l)) {
        return Err(format!("app.system_name {system_name:?}"));
    }
    let version = s(app, "version")?;
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3
        || !parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u32>().is_ok_and(|n| n < 65536)
        })
    {
        return Err(format!("app.version {version:?}"));
    }
    if let Some(d) = app.get("description") {
        if d.as_str().is_none_or(|d| d.chars().count() > 1024) {
            return Err("app.description".into());
        }
    }
    let binary = s(entry, "binary")?;
    if !v.files.contains_key(&binary) {
        return Err(format!("entry.binary {binary:?} is not in the archive"));
    }
    if let Some(args) = entry.get("args") {
        let args = args.as_array().ok_or("entry.args is not a list")?;
        if args.len() > 16
            || args
                .iter()
                .any(|a| a.as_str().is_none_or(|a| a.len() > 256))
        {
            return Err("entry.args".into());
        }
    }
    let permissions = table.get("permissions").and_then(|v| v.as_table());
    if let Some(p) = permissions {
        for file in p
            .get("files")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let rule = file.as_str().ok_or("files rule is not a string")?;
            if !files_rule_ok(rule) {
                return Err(format!("files rule {rule:?}"));
            }
        }
        let network = p.get("network").and_then(|v| v.as_array());
        if network.is_some_and(|n| !n.is_empty() && n != &vec![toml::Value::from("outbound")]) {
            return Err("permissions.network".into());
        }
        for iface in p
            .get("interfaces")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let i = iface.as_str().ok_or("interface is not a string")?;
            let (head, tail) = i
                .rsplit_once(".v")
                .ok_or_else(|| format!("interface {i}"))?;
            if head.is_empty() || tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("interface {i}"));
            }
        }
    }
    Ok(())
}

#[test]
fn the_files_grammar_matches_lazyos() {
    for good in [
        "read:$HOME/.apps/user.ada.todo",
        "write:$HOME/.apps/user.ada.todo",
        "read:$HOME/Documents/*",
        "read:/system/share/x",
    ] {
        assert!(files_rule_ok(good), "{good}");
    }
    for bad in [
        "read:/home/*/.apps/user.ada.todo",
        "write:/data/home/*/x",
        "read:/home",
        "read:$HOME",
        "read:/x/$HOME/y",
        "read:$HOME/../x",
        "exec:$HOME/x",
    ] {
        assert!(!files_rule_ok(bad), "{bad}");
    }
}
