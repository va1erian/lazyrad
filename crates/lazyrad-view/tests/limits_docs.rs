//! The limits are stated once, in `src/limits.rs`, and what it states is true.
//!
//! Two failure modes this guards against: a table that drifted from the
//! constants it describes, and the same number repeated in the README or in
//! another module's docs, where it goes stale the next time a limit moves. The
//! other docs refer to `limits.rs` and say no numbers of their own.

use std::fs;
use std::path::{Path, PathBuf};

use lazyrad_view::limits::{
    MAX_ARRAY_SIZE, MAX_CALL_LEVELS, MAX_EXPR_DEPTHS, MAX_MAP_SIZE, MAX_OPERATIONS,
    MAX_STRING_SIZE, PEAK_MEMORY_CEILING,
};

fn crate_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// The cells of every row of the markdown tables in the module docs of
/// `limits.rs`, each trimmed.
fn table_rows() -> Vec<Vec<String>> {
    let source = fs::read_to_string(crate_path("src/limits.rs")).expect("src/limits.rs");
    source
        .lines()
        // The module docs are the leading block of `//!` lines.
        .map_while(|line| line.strip_prefix("//!"))
        .map(str::trim)
        .filter(|line| line.starts_with('|') && !line.starts_with("|---"))
        .map(|line| {
            line.trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_owned())
                .collect()
        })
        .collect()
}

/// `512`, `32768`, `16, 10`, `192 MiB`: the numbers in a value cell, with
/// mebibytes scaled to bytes.
fn numbers(cell: &str) -> Vec<u64> {
    let (cell, scale) = match cell.strip_suffix("MiB") {
        Some(rest) => (rest, 1024 * 1024),
        None => (cell, 1),
    };
    cell.split(',')
        .map(|part| {
            let part = part.trim();
            let parsed: u64 = part
                .parse()
                .unwrap_or_else(|_| panic!("`{part}` is not a number in the limits table"));
            parsed * scale
        })
        .collect()
}

fn as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap()
}

#[test]
fn the_table_states_every_limit_exactly_once_and_matches_the_constants() {
    let rows = table_rows();
    let expected: [(&str, Vec<u64>); 7] = [
        ("MAX_ARRAY_SIZE", vec![as_u64(MAX_ARRAY_SIZE)]),
        ("MAX_MAP_SIZE", vec![as_u64(MAX_MAP_SIZE)]),
        ("MAX_STRING_SIZE", vec![as_u64(MAX_STRING_SIZE)]),
        ("MAX_OPERATIONS", vec![MAX_OPERATIONS]),
        ("MAX_CALL_LEVELS", vec![as_u64(MAX_CALL_LEVELS)]),
        (
            "MAX_EXPR_DEPTHS",
            vec![as_u64(MAX_EXPR_DEPTHS.0), as_u64(MAX_EXPR_DEPTHS.1)],
        ),
        ("PEAK_MEMORY_CEILING", vec![PEAK_MEMORY_CEILING]),
    ];
    for (name, value) in expected {
        let label = format!("[`{name}`]");
        let found: Vec<&Vec<String>> = rows.iter().filter(|row| row[0] == label).collect();
        assert_eq!(found.len(), 1, "{name} must be in the limits table once");
        assert_eq!(
            numbers(&found[0][1]),
            value,
            "{name}: the table in src/limits.rs says something else than the constant"
        );
    }
}

#[test]
fn the_derived_rows_follow_from_the_constants() {
    let mut records = Vec::new();
    let mut seen_numbers = false;
    let mut seen_steps = false;
    for row in table_rows() {
        let what = row[0].as_str();
        if let Some(fields) = what
            .strip_prefix("a list of ")
            .and_then(|rest| rest.strip_suffix("-field rows"))
        {
            let fields: usize = fields.parse().expect("a field count");
            let rows: usize = row[1]
                .strip_suffix(" rows")
                .and_then(|rows| rows.parse().ok())
                .unwrap_or_else(|| panic!("`{}` is not `N rows`", row[1]));
            // One map entry per field, and one for the entry that holds the
            // list.
            assert_eq!(
                rows,
                ((MAX_MAP_SIZE - 1) / fields).min(MAX_ARRAY_SIZE),
                "{fields}-field rows"
            );
            records.push(fields);
        } else if what == "a list of numbers or names" {
            assert_eq!(row[1].parse::<usize>(), Ok(MAX_ARRAY_SIZE), "{what}");
            seen_numbers = true;
        } else if what == "five-operation steps over a full array" {
            let steps = MAX_OPERATIONS / as_u64(5 * MAX_ARRAY_SIZE);
            assert_eq!(row[1].parse::<u64>(), Ok(steps), "{what}");
            seen_steps = true;
        }
    }
    assert!(seen_numbers && seen_steps, "the derived table lost a row");
    assert!(
        records.len() >= 3,
        "the table should show several row widths, found {records:?}"
    );
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("a directory of the crate") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// Words that make a number a statement about a limit.
const UNITS: [&str; 6] = ["KiB", "MiB", "GiB", "elements", "entries", "operations"];

/// What in `line` states a limit: a number equal to one of `values`, with or
/// without thousands grouping (`32 768`, `32_768`, `32,768`), or any number
/// followed by a unit word (`1024 elements`, `154 MiB`).
fn limit_statements(line: &str, values: &[u64]) -> Vec<String> {
    let words: Vec<&str> = line
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let mut found = Vec::new();
    for (index, word) in words.iter().enumerate() {
        if !word.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let next = words.get(index + 1).copied();
        let mut candidates = vec![word.parse::<u64>().ok()];
        if let Some(group) =
            next.filter(|group| group.len() == 3 && group.bytes().all(|b| b.is_ascii_digit()))
        {
            candidates.push(format!("{word}{group}").parse::<u64>().ok());
        }
        let is_value = candidates
            .iter()
            .flatten()
            .any(|number| values.contains(number));
        let has_unit = next.is_some_and(|next| UNITS.contains(&next));
        if is_value || has_unit {
            let tail = next.map_or(String::new(), |next| format!(" {next}"));
            found.push(format!("{word}{tail}"));
        }
    }
    found
}

#[test]
fn no_other_doc_states_a_limit() {
    // The values worth searching for. Small ones (4, 16, 10) are too common
    // to find by value; those are caught by their unit words, and the table
    // test above keeps the table itself true.
    let values = [
        as_u64(MAX_ARRAY_SIZE),
        as_u64(MAX_MAP_SIZE),
        as_u64(MAX_STRING_SIZE),
        MAX_OPERATIONS,
        PEAK_MEMORY_CEILING / (1024 * 1024),
    ];

    let mut files = vec![crate_path("README.md")];
    for directory in ["src", "tests"] {
        rust_files(&crate_path(directory), &mut files);
    }
    let limits = crate_path("src/limits.rs");
    let this_file = crate_path("tests/limits_docs.rs");

    let mut offences = Vec::new();
    for file in files {
        if file == limits || file == this_file {
            continue;
        }
        let source = fs::read_to_string(&file).expect("a source file");
        let is_markdown = file.extension().is_some_and(|extension| extension == "md");
        for (number, line) in source.lines().enumerate() {
            let trimmed = line.trim_start();
            let is_doc = is_markdown || trimmed.starts_with("//!") || trimmed.starts_with("///");
            if !is_doc {
                continue;
            }
            for statement in limit_statements(line, &values) {
                offences.push(format!(
                    "{}:{}: `{statement}` in `{}`",
                    file.strip_prefix(crate_path("")).unwrap_or(&file).display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "limits are stated in src/limits.rs only; refer to it instead:\n{}",
        offences.join("\n")
    );
}

#[test]
fn the_scanner_finds_what_it_is_meant_to_find() {
    let values = [512, 32_768];
    assert_eq!(limit_statements("at most 512 rows", &values), ["512 rows"]);
    assert_eq!(
        limit_statements("32 768 operations", &values),
        ["32 768", "768 operations"]
    );
    assert_eq!(limit_statements("the 32_768 budget", &values), ["32 768"]);
    assert_eq!(limit_statements("about 154 MiB peak", &values), ["154 MiB"]);
    assert_eq!(
        limit_statements("1024 elements", &values),
        ["1024 elements"]
    );
    assert!(limit_statements("5-field rows and 128 levels", &values).is_empty());
    assert!(limit_statements("no numbers here", &values).is_empty());
}
