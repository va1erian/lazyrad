#![forbid(unsafe_code)]

//! Child process launch flags and executable naming.

use std::process::Command;

/// `CREATE_NO_WINDOW` from the Windows SDK's `WinBase.h` process creation
/// flags: start a console application without a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Keeps `command`'s child from opening a console window of its own.
///
/// The IDE is a GUI-subsystem app, so Windows would give the console-subsystem
/// player a console window; its output is piped back to the IDE instead.
/// Other platforms have no such thing, so this changes nothing there.
pub fn hide_console_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// `stem` with the platform's executable suffix (`.exe` on Windows, nothing
/// elsewhere).
pub fn executable_file_name(stem: &str) -> String {
    format!("{stem}{}", std::env::consts::EXE_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_still_runs_with_the_console_hidden() {
        let exe = std::env::current_exe().expect("the test executable has a path");
        let mut command = Command::new(exe);
        command.arg("--list");
        let output = hide_console_window(&mut command)
            .output()
            .expect("the child starts");
        assert!(output.status.success());
    }

    #[test]
    fn the_suffix_follows_the_host() {
        assert_eq!(
            executable_file_name("app"),
            format!("app{}", std::env::consts::EXE_SUFFIX)
        );
    }
}
