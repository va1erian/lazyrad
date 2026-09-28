#![forbid(unsafe_code)]

//! The `lazyrad-ide` binary: thin wrapper over [`lazyrad_ide::run`].

fn main() {
    if let Err(error) = lazyrad_ide::run() {
        eprintln!("lazyrad-ide: {error}");
        std::process::exit(1);
    }
}
