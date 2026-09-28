#![forbid(unsafe_code)]

//! The LazyRAD runtime host binary.
//!
//! This is a thin wrapper around [`lazyrad_player::run_cli`]: the command line,
//! the diagnostics and the exit codes live in the library so they can be tested
//! without opening a window. See the library for the output format.

fn main() {
    std::process::exit(lazyrad_player::run_cli());
}
