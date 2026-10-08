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
///
/// The cost is polynomial in the two lengths: a pattern with many wildcards
/// (`**a**a**a**b`) cannot make the matcher backtrack exponentially.
pub fn matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let path: Vec<char> = path.chars().collect();
    let mut dead = vec![false; (pattern.len() + 1) * (path.len() + 1)];
    match_from(&pattern, &path, 0, 0, &mut dead)
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

/// Whether `pattern[pi..]` matches `path[si..]`.
///
/// `dead` records the `(pi, si)` states of a wildcard that were already tried
/// and failed, so each is explored at most once.
fn match_from(
    pattern: &[char],
    path: &[char],
    mut pi: usize,
    mut si: usize,
    dead: &mut [bool],
) -> bool {
    let width = path.len() + 1;
    loop {
        let Some(&token) = pattern.get(pi) else {
            return si == path.len();
        };
        match token {
            '*' => {
                if dead[pi * width + si] {
                    return false;
                }
                let matched = match_star(pattern, path, pi, si, dead);
                if !matched {
                    dead[pi * width + si] = true;
                }
                return matched;
            }
            '?' => {
                if !matches!(path.get(si), Some(character) if *character != '/') {
                    return false;
                }
            }
            literal => {
                if path.get(si) != Some(&literal) {
                    return false;
                }
            }
        }
        pi += 1;
        si += 1;
    }
}

/// The wildcard case of [`match_from`], with `pattern[pi]` a `*`.
fn match_star(pattern: &[char], path: &[char], pi: usize, si: usize, dead: &mut [bool]) -> bool {
    // `**/` matches zero or more whole directory segments.
    if pattern.get(pi + 1) == Some(&'*') && pattern.get(pi + 2) == Some(&'/') {
        let rest = pi + 3;
        if match_from(pattern, path, rest, si, dead) {
            return true;
        }
        return (si..path.len())
            .any(|index| path[index] == '/' && match_from(pattern, path, rest, index + 1, dead));
    }
    // A `**` crosses directory separators; a `*` stops at one.
    let (rest, crosses) = if pattern.get(pi + 1) == Some(&'*') {
        (pi + 2, true)
    } else {
        (pi + 1, false)
    };
    // Try to consume zero or more characters, then match the rest.
    let mut skip = si;
    loop {
        if match_from(pattern, path, rest, skip, dead) {
            return true;
        }
        match path.get(skip) {
            None => return false,
            Some('/') if !crosses => return false,
            Some(_) => skip += 1,
        }
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

    #[test]
    fn a_pathological_pattern_does_not_backtrack_exponentially() {
        // Without memoization `**a**a**a**a**a**a**b` against a run of `a`s
        // with no `b` explores an exponential number of splits.
        let pattern = "**a**a**a**a**a**a**a**a**b";
        let path = "a".repeat(200);
        let started = std::time::Instant::now();
        assert!(!matches(pattern, &path));
        assert!(matches(pattern, &format!("{path}b")));
        assert!(!matches("*a*a*a*a*a*a*a*a*b", &path));
        assert!(
            started.elapsed() < std::time::Duration::from_millis(100),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn wildcards_interleaved_with_literals_still_match_correctly() {
        assert!(matches("a*b*c", "aXXbYYc"));
        assert!(!matches("a*b*c", "aXXbYY"));
        assert!(matches("**/a/**/b", "x/y/a/z/b"));
        assert!(!matches("*/b", "x/y/b"), "a `*` stops at a separator");
        assert!(matches("**/b", "x/y/b"));
        assert!(matches("**", ""), "a double star matches nothing too");
        assert!(matches("*", ""));
        assert!(!matches("?", ""));
    }
}
