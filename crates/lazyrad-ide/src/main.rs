#![forbid(unsafe_code)]

//! The LazyRAD IDE shell.
//!
//! M0 opens the empty window the runtime provides; M1 adds the menu, toolbar
//! and project explorer, and later milestones dock the editor, designer,
//! debugger and output panes around it (PLAN.md §9, §11).

fn main() {
    if let Err(error) = lazyrad_runtime::shell::run_empty_window("LazyRAD") {
        eprintln!("lazyrad-ide: {error}");
        std::process::exit(1);
    }
}
