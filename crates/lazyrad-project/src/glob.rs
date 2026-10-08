#![forbid(unsafe_code)]

//! A tiny, dependency-free glob matcher for the project's `assets` patterns.
//!
//! The matcher is deliberately small: `*` matches any run of characters except
//! `/`, `?` matches exactly one character except `/`, and `**` matches any run
//! including `/` (so `songs/**` reaches into subdirectories). Everything else
//! matches literally. Paths use `/` as their separator, so the same pattern
//! works on every host.
//!
//! [`is_safe_glob`] additionally checks that a pattern is one a project may
//! declare: relative, with no `..` component, no leading `/`, and no `\`.

/// Whether `pattern` matches the `/`-separated `path`.
pub fn matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let path: Vec<char> = path.chars().collect();
    match_from(&pattern, &path)
}

/// Whether `pattern` is a safe project asset pattern.
///
/// A safe pattern is a relative path: not empty, not absolute, using `/` as its
/// separator, with no `..` component and no `\`.
pub fn is_safe_glob(pattern: &str) -> bool {
    if pattern.is_empty() || pattern.starts_with('/') || pattern.contains('\\') {
        return false;
    }
    pattern
        .split('/')
        .all(|component| !component.is_empty() && component != "." && component != "..")
}

/// Matches `pattern` against `path`, both as character slices.
fn match_from(pattern: &[char], path: &[char]) -> bool {
    // `**/` matches zero or more whole directory segments.
    if pattern.len() >= 3 && pattern[0] == '*' && pattern[1] == '*' && pattern[2] == '/' {
        let rest = &pattern[3..];
        if match_from(rest, path) {
            return true;
        }
        return path
            .iter()
            .enumerate()
            .any(|(index, character)| *character == '/' && match_from(rest, &path[index + 1..]));
    }
    match pattern.first() {
        None => path.is_empty(),
        Some('*') => {
            // A `**` crosses directory separators; a `*` stops at one.
            let (rest, crosses) = if pattern.get(1) == Some(&'*') {
                (&pattern[2..], true)
            } else {
                (&pattern[1..], false)
            };
            // Try to consume zero or more characters, then match the rest.
            let mut skip = 0;
            loop {
                if match_from(rest, &path[skip..]) {
                    return true;
                }
                match path.get(skip) {
                    None => return false,
                    Some('/') if !crosses => return false,
                    Some(_) => skip += 1,
                }
            }
        }
        Some('?') => {
            matches!(path.first(), Some(character) if *character != '/')
                && match_from(&pattern[1..], &path[1..])
        }
        Some(literal) => path.first() == Some(literal) && match_from(&pattern[1..], &path[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_star_stays_within_one_segment() {
        assert!(matches("*.mod", "song.mod"));
        assert!(matches("songs/*.mod", "songs/song.mod"));
        assert!(!matches("*.mod", "songs/song.mod"));
        assert!(matches("*", "song.mod"));
        assert!(!matches("*", "songs/song.mod"));
    }

    #[test]
    fn a_double_star_crosses_directories() {
        assert!(matches("**/*.mod", "songs/album/song.mod"));
        assert!(matches("**/*.mod", "song.mod"), "zero directories");
        assert!(matches("songs/**", "songs/album/song.mod"));
        assert!(matches("songs/**", "songs/song.mod"));
        assert!(!matches("songs/**", "icons/logo.png"));
    }

    #[test]
    fn a_question_matches_one_non_separator() {
        assert!(matches("a?.mod", "ab.mod"));
        assert!(!matches("a?.mod", "a.mod"));
        assert!(!matches("a?.mod", "a/.mod"));
    }

    #[test]
    fn literal_and_prefix_patterns_work() {
        assert!(matches("icons/logo.png", "icons/logo.png"));
        assert!(!matches("icons/logo.png", "icons/logo.svg"));
        assert!(matches("songs/*", "songs/song.mod"));
    }

    #[test]
    fn safety_rejects_paths_that_leave_the_project() {
        for good in ["*.mod", "songs/*.mod", "icons/logo.png", "**/*.png"] {
            assert!(is_safe_glob(good), "{good}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../secret",
            "songs/../../secret",
            "a\\b.mod",
            "songs/",
            "./song.mod",
        ] {
            assert!(!is_safe_glob(bad), "{bad}");
        }
    }
}
