#![forbid(unsafe_code)]

//! The LazyRAD runtime host.
//!
//! `lazyrad-player` runs a project directory, an exported payload appended to
//! its own executable, or a debug session (`--debug`). Given a directory it
//! loads the project and opens its startup form through [`lazyrad_runtime::form`];
//! with no argument it falls back to the empty-window shell. The exported payload
//! and the debug protocol land in later milestones (PLAN.md §4, §7, §8).

fn main() {
    let mut args = std::env::args().skip(1);
    let result = match args.next() {
        Some(dir) => lazyrad_runtime::form::run_project(dir),
        None => lazyrad_runtime::shell::run_empty_window("LazyRAD Player")
            .map_err(lazyrad_runtime::RuntimeError::from),
    };
    if let Err(error) = result {
        eprintln!("lazyrad-player: {error}");
        std::process::exit(1);
    }
}
