#![forbid(unsafe_code)]

//! The `manifest.toml` a LazyRAD app carries.
//!
//! The grammar is LazyOS's (`docs/packages.md` §3): this module writes the
//! manifest and validates every field against the same rules the reader
//! (`libs/lazypkg`) and `tools/pkg/build.py` enforce, collecting **all**
//! problems so a user fixes them in one pass.
//!
//! What LazyRAD decides, and why:
//!
//! * `system_name` is `user.<author-slug>.<project-slug>` unless the caller sets
//!   one, so two authors' "todo" apps never collide;
//! * `entry.args = ["--project", "resources/project"]`: the player resolves the
//!   relative path against its own install directory (see `lrplay`'s
//!   `args` module in the LazyOS repo), because the install directory name
//!   contains a per-version hash and cannot be written into the manifest;
//! * `[permissions]` is derived from what the project's scripts use (LazyOS plan
//!   P2.2): private storage only when a `file_*`/`dir_*` function appears.

use serde::Serialize;

use crate::lzp::error::LzpError;

/// Where an app's private data lives (plan D5): `<home>/.apps/<system_name>`
/// in the home of whoever runs it, never inside the install tree (`/apps`
/// belongs to LazyOS's `pkgd`). LazyOS's manifest grammar (filesystem plan
/// F5) spells the running user's home `$HOME`, allowed only as a rule's first
/// segment, and refuses absolute paths under `/home`. A rule naming a
/// directory covers everything inside it, so `$HOME/.apps/<system_name>`
/// grants the app's whole folder without a wildcard.
pub const APP_DATA_ROOT: &str = "$HOME/.apps";

/// The binary name inside the package.
pub const PLAYER_ENTRY: &str = "bin/lrplay.elf";

/// The project directory inside the package.
pub const PROJECT_DIR: &str = "resources/project";

const MAX_NAME: usize = 64;
const MAX_AUTHOR: usize = 128;
const MAX_DESCRIPTION: usize = 1024;
const MAX_SYSTEM_NAME: usize = 128;
/// A slug longer than this is cut, so `user.<a>.<p>` stays under 128 bytes.
const MAX_SLUG: usize = 48;

/// What a package is called and who made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity<'a> {
    /// The display name (the project name).
    pub name: &'a str,
    /// The author, shown at install time. Unverified.
    pub author: &'a str,
    /// The project's version string (free-form in `.lrp`).
    pub version: &'a str,
    /// An explicit reverse-DNS id, or `None` to derive one.
    pub system_name: Option<&'a str>,
    /// An optional one-line description.
    pub description: Option<&'a str>,
}

/// The finished manifest: its text plus the fields callers report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltManifest {
    /// The TOML to store as `manifest.toml`.
    pub text: String,
    /// The reverse-DNS id.
    pub system_name: String,
    /// The normalised `MAJOR.MINOR.PATCH` version.
    pub version: String,
    /// The permissions that were derived.
    pub files: Vec<String>,
    /// Interfaces the app declares.
    pub interfaces: Vec<String>,
}

#[derive(Serialize)]
struct Manifest<'a> {
    app: App<'a>,
    entry: Entry<'a>,
    permissions: Permissions,
}

#[derive(Serialize)]
struct App<'a> {
    name: &'a str,
    system_name: &'a str,
    author: &'a str,
    version: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
}

#[derive(Serialize)]
struct Entry<'a> {
    binary: &'a str,
    /// `linux`: the player is a static musl binary, started in the Linux
    /// personality (`init` needs this; an ELF header cannot say).
    abi: &'a str,
    args: Vec<&'a str>,
}

#[derive(Serialize)]
struct Permissions {
    interfaces: Vec<String>,
    topics: Vec<String>,
    files: Vec<String>,
    network: Vec<String>,
}

/// Builds and validates the manifest for `identity`, deriving permissions from
/// the project's script sources.
pub fn build<'a>(
    identity: &Identity<'_>,
    scripts: impl IntoIterator<Item = &'a str>,
) -> Result<BuiltManifest, LzpError> {
    let mut problems = Vec::new();
    let system_name = match identity.system_name {
        Some(explicit) => explicit.to_owned(),
        None => derive_system_name(identity.author, identity.name).unwrap_or_else(|| {
            problems.push(
                "cannot derive app.system_name: the author and project name need at least \
                 one ASCII letter or digit each; set system_name explicitly"
                    .to_owned(),
            );
            String::new()
        }),
    };
    let version = normalise_version(identity.version);
    if version.is_none() {
        problems.push(format!(
            "project version \"{}\" is not MAJOR.MINOR.PATCH (each part a number below 65536)",
            identity.version
        ));
    }
    check_text("app.name", identity.name, MAX_NAME, &mut problems);
    check_text("app.author", identity.author, MAX_AUTHOR, &mut problems);
    if let Some(description) = identity.description {
        if description.chars().count() > MAX_DESCRIPTION || description.chars().any(is_bad_char) {
            problems.push(format!(
                "app.description must be at most {MAX_DESCRIPTION} characters without control characters"
            ));
        }
    }
    // A derive failure already reported its own problem (and left this empty).
    let derive_failed = identity.system_name.is_none() && system_name.is_empty();
    if !derive_failed && !valid_system_name(&system_name) {
        problems.push(format!(
            "app.system_name \"{system_name}\" is not a reverse-DNS name (lowercase letters, \
             digits and `-`, at least three dot-separated labels, at most {MAX_SYSTEM_NAME} bytes)"
        ));
    }
    let (Some(version), true) = (version, problems.is_empty()) else {
        return Err(LzpError::Manifest(problems));
    };

    let uses_files = scripts.into_iter().any(uses_private_storage);
    let files = if uses_files {
        let data = format!("{APP_DATA_ROOT}/{system_name}");
        vec![format!("read:{data}"), format!("write:{data}")]
    } else {
        Vec::new()
    };
    // Every xui app talks to the compositor; declaring it keeps the manifest an
    // honest summary of what the app touches.
    let interfaces = vec!["os.lazy.display.v1".to_owned()];

    let manifest = Manifest {
        app: App {
            name: identity.name,
            system_name: &system_name,
            author: identity.author,
            version: &version,
            description: identity.description,
        },
        entry: Entry {
            binary: PLAYER_ENTRY,
            abi: "linux",
            args: vec!["--project", PROJECT_DIR],
        },
        permissions: Permissions {
            interfaces: interfaces.clone(),
            topics: Vec::new(),
            files: files.clone(),
            network: Vec::new(),
        },
    };
    let text = toml::to_string(&manifest)
        .map_err(|error| LzpError::Manifest(vec![format!("cannot write the manifest: {error}")]))?;
    Ok(BuiltManifest {
        text,
        system_name,
        version,
        files,
        interfaces,
    })
}

/// Whether a script calls a function of the private-storage stdlib modules.
pub fn uses_private_storage(source: &str) -> bool {
    ["file_", "dir_"].iter().any(|prefix| {
        source.match_indices(prefix).any(|(at, _)| {
            let before = source[..at].chars().next_back();
            let after = &source[at + prefix.len()..];
            let name_len = after
                .find(|c: char| !(c.is_ascii_lowercase() || c == '_'))
                .unwrap_or(after.len());
            let rest = &after[name_len..];
            let follows_paren = rest.trim_start().starts_with('(');
            // `Fn("file_read_text").call(..)` reaches the same function by name.
            let as_fn_pointer = before == Some('"') && rest.starts_with('"');
            // A call (`file_read_text(`) that is not the tail of a longer name.
            let boundary = !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            boundary && name_len > 0 && (follows_paren || as_fn_pointer)
        })
    })
}

fn is_bad_char(c: char) -> bool {
    c.is_control()
}

fn check_text(field: &str, value: &str, max: usize, problems: &mut Vec<String>) {
    let chars = value.chars().count();
    if chars == 0 || chars > max || value.chars().any(is_bad_char) {
        problems.push(format!(
            "{field} must be 1..{max} characters without control characters"
        ));
    }
}

/// `user.<author>.<project>`, each slugged, or `None` when a slug is empty.
pub fn derive_system_name(author: &str, project: &str) -> Option<String> {
    let author = slug(author)?;
    let project = slug(project)?;
    Some(format!("user.{author}.{project}"))
}

/// Lowercase ASCII letters and digits, runs of anything else become one `-`,
/// no leading or trailing `-`, at most [`MAX_SLUG`] bytes. `None` if nothing is
/// left.
pub fn slug(text: &str) -> Option<String> {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.truncate(MAX_SLUG);
    let trimmed = out.trim_end_matches('-');
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// The reader's `system_name` grammar.
pub fn valid_system_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_SYSTEM_NAME {
        return false;
    }
    let labels: Vec<&str> = name.split('.').collect();
    labels.len() >= 3
        && labels.iter().all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// `1`, `1.2` or `1.2.3` become `1.0.0`, `1.2.0`, `1.2.3`; anything else, or a
/// part of 65536 or more, is `None`.
pub fn normalise_version(text: &str) -> Option<String> {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut numbers = [0u16; 3];
    for (slot, part) in numbers.iter_mut().zip(&parts) {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    Some(format!("{}.{}.{}", numbers[0], numbers[1], numbers[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity<'a>(name: &'a str, author: &'a str, version: &'a str) -> Identity<'a> {
        Identity {
            name,
            author,
            version,
            system_name: None,
            description: None,
        }
    }

    #[test]
    fn slugs_are_lowercase_ascii_with_single_dashes() {
        assert_eq!(slug("My To-Do App!").as_deref(), Some("my-to-do-app"));
        assert_eq!(slug("  --x--  ").as_deref(), Some("x"));
        assert_eq!(slug("Jörg").as_deref(), Some("j-rg"));
        assert_eq!(slug("日本語"), None);
        assert_eq!(slug("").as_deref(), None);
        assert!(slug(&"a".repeat(500)).unwrap().len() <= MAX_SLUG);
    }

    #[test]
    fn a_derived_system_name_is_valid_and_bounded() {
        let name = derive_system_name(&"A".repeat(300), &"b c ".repeat(100)).unwrap();
        assert!(valid_system_name(&name), "{name}");
        assert!(name.len() <= MAX_SYSTEM_NAME);
        assert_eq!(
            derive_system_name("Ada L.", "Todo").as_deref(),
            Some("user.ada-l.todo")
        );
    }

    #[test]
    fn versions_are_normalised_or_refused() {
        assert_eq!(normalise_version("1").as_deref(), Some("1.0.0"));
        assert_eq!(normalise_version("1.2").as_deref(), Some("1.2.0"));
        assert_eq!(normalise_version("1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(normalise_version("65535.0.1").as_deref(), Some("65535.0.1"));
        for bad in [
            "",
            "1.2.3.4",
            "a",
            "1.b",
            "65536.0.0",
            "-1",
            "1..2",
            "1.2.3-beta",
            "+1",
        ] {
            assert_eq!(normalise_version(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn system_name_grammar_matches_the_reader() {
        for ok in ["user.a.b", "org.lazy.paint", "a-b.c1.d-e"] {
            assert!(valid_system_name(ok), "{ok}");
        }
        for bad in [
            "", "a.b", "User.a.b", "a..b", "-a.b.c", "a-.b.c", "a.b.c_d", "a.b.é",
        ] {
            assert!(!valid_system_name(bad), "{bad}");
        }
        assert!(!valid_system_name(&format!("a.b.{}", "c".repeat(200))));
    }

    #[test]
    fn storage_use_is_detected_only_for_real_calls() {
        assert!(uses_private_storage("let t = file_read_text(\"a\");"));
        assert!(uses_private_storage("dir_list ( \".\" )"));
        assert!(!uses_private_storage("let profile_path = 1;"));
        assert!(!uses_private_storage("my_file_read_text(1)"));
        assert!(!uses_private_storage("let file_ = 1;"));
        // Reached by name through a function pointer.
        assert!(uses_private_storage("Fn(\"file_read_text\").call([\"a\"])"));
        assert!(!uses_private_storage("let s = \"my_file_read_text\";"));
        assert!(!uses_private_storage(
            "// file_read_text is documented elsewhere"
        ));
    }

    #[test]
    fn a_minimal_manifest_has_no_file_access() {
        let built = build(&identity("Todo", "Ada", "1.0"), ["fn go() {}"]).unwrap();
        assert_eq!(built.system_name, "user.ada.todo");
        assert_eq!(built.version, "1.0.0");
        assert!(built.files.is_empty());
        assert!(built.text.contains("binary = \"bin/lrplay.elf\""));
        assert!(built.text.contains("abi = \"linux\""));
        assert!(built.text.contains("\"--project\""));
        assert!(built.text.contains("network = []"));
    }

    #[test]
    fn storage_use_grants_only_the_private_directory() {
        let built = build(
            &identity("Todo", "Ada", "1.0.0"),
            ["file_write_text(\"x\", \"y\");"],
        )
        .unwrap();
        assert_eq!(
            built.files,
            vec![
                "read:$HOME/.apps/user.ada.todo".to_owned(),
                "write:$HOME/.apps/user.ada.todo".to_owned()
            ]
        );
    }

    #[test]
    fn every_problem_is_reported_at_once() {
        let error = build(&identity("", "", "x"), []).unwrap_err();
        let LzpError::Manifest(problems) = error else {
            panic!("expected manifest problems");
        };
        assert!(problems.len() >= 3, "{problems:?}");
    }

    #[test]
    fn an_explicit_bad_system_name_is_refused() {
        let mut id = identity("Todo", "Ada", "1.0.0");
        id.system_name = Some("Bad Name");
        assert!(build(&id, []).is_err());
        id.system_name = Some("org.example.todo");
        assert_eq!(build(&id, []).unwrap().system_name, "org.example.todo");
    }

    #[test]
    fn control_characters_in_text_fields_are_refused() {
        assert!(build(&identity("To\ndo", "Ada", "1.0.0"), []).is_err());
        let mut id = identity("Todo", "Ada", "1.0.0");
        id.description = Some("bad\u{7}");
        assert!(build(&id, []).is_err());
    }
}
