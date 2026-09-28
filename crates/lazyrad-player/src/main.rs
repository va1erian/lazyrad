#![forbid(unsafe_code)]

//! The LazyRAD runtime host.
//!
//! `lazyrad-player` runs a project directory, an exported payload appended to
//! its own executable, or a debug session (`--debug`). M0 leaves it as the
//! empty-window shell the runtime exposes; M1 gives it the project loader and
//! form host, and M4 the debug protocol (PLAN.md §4, §7, §8).

fn main() {
    if let Err(error) = lazyrad_runtime::shell::run_empty_window("LazyRAD Player") {
        eprintln!("lazyrad-player: {error}");
        std::process::exit(1);
    }
}
