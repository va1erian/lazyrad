#![forbid(unsafe_code)]
// A GUI application: on Windows, don't open a console window behind the IDE.
// (Ignored on other platforms.) Messages go to the IDE's Output pane.
#![windows_subsystem = "windows"]

//! The `lazyrad-ide` binary: thin wrapper over [`lazyrad_ide::run`].

fn main() {
    if let Err(error) = lazyrad_ide::run() {
        let message = format!("LazyRAD could not start: {error}");
        eprintln!("lazyrad-ide: {error}");
        // With no console (GUI subsystem), the message box is what the user sees.
        lazyrad_ide::platform::show_fatal_error(&message);
        std::process::exit(1);
    }
}
