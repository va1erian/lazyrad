#![forbid(unsafe_code)]

//! `lazyrad-pack`: package a LazyRAD project as a LazyOS `.lzp`, with no IDE.
//!
//! ```text
//! lazyrad-pack <project dir | .lrp> --player <lrplay.elf> [--out <dir>]
//!              [--author <name>] [--system-name <id>] [--description <text>]
//!              [--no-check] [--install-dev [<dir>]]
//! ```
//!
//! The player is the LazyOS `lrplay` ELF (`python tools/lazyrad/build.py` in the
//! LazyOS repo); `LAZYRAD_PLAYER` names it when `--player` is omitted. The
//! output is `<out>/<system_name>-<version>.lzp` (`--out` defaults to `dist`).
//! Unless `--no-check` is given the project is compiled first, exactly as the
//! player would, and any problem stops the build.
//!
//! `--install-dev` hands the package to the installer: `pkgd` when available,
//! otherwise it is saved to `<dir>` (default `/data/packages`) and reported as
//! "saved, not installed".
//!
//! Exit status: 0 success, 1 the build or install failed, 2 usage error.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lazyrad_packager::lzp::{
    self, DevInstaller, PackageRequest, PkgdInstaller, build_package, install_with_fallback,
    read_player, write_package,
};

/// A parsed command line.
#[derive(Debug, PartialEq, Eq)]
struct Options {
    project: PathBuf,
    player: Option<PathBuf>,
    out: PathBuf,
    author: Option<String>,
    system_name: Option<String>,
    description: Option<String>,
    check: bool,
    install_dev: Option<Option<PathBuf>>,
}

const USAGE: &str = "usage: lazyrad-pack <project dir | .lrp> --player <lrplay.elf> [--out <dir>] \
[--author <name>] [--system-name <id>] [--description <text>] [--no-check] [--install-dev [<dir>]]";

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Options, String> {
    let mut args = args.into_iter().peekable();
    let mut options = Options {
        project: PathBuf::new(),
        player: None,
        out: PathBuf::from("dist"),
        author: None,
        system_name: None,
        description: None,
        check: true,
        install_dev: None,
    };
    let mut project = None;
    let text = |value: Option<OsString>, flag: &str| -> Result<String, String> {
        value
            .ok_or_else(|| format!("{flag} needs a value"))?
            .into_string()
            .map_err(|_| format!("{flag} is not valid UTF-8"))
    };
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--player" => options.player = Some(PathBuf::from(text(args.next(), "--player")?)),
            "--out" => options.out = PathBuf::from(text(args.next(), "--out")?),
            "--author" => options.author = Some(text(args.next(), "--author")?),
            "--system-name" => options.system_name = Some(text(args.next(), "--system-name")?),
            "--description" => options.description = Some(text(args.next(), "--description")?),
            "--no-check" => options.check = false,
            "--install-dev" => {
                let dir = match args.peek() {
                    // Only once the project is known: `--install-dev myproj` must
                    // not swallow the project as the directory.
                    Some(next) if project.is_some() && !next.to_string_lossy().starts_with('-') => {
                        args.next().map(PathBuf::from)
                    }
                    _ => None,
                };
                options.install_dev = Some(dir);
            }
            flag if flag.starts_with('-') => return Err(format!("unknown option `{flag}`")),
            _ if project.is_none() => project = Some(PathBuf::from(arg)),
            _ => return Err("more than one project given".to_owned()),
        }
    }
    options.project = project.ok_or("no project given")?;
    Ok(options)
}

/// The `.lrp` a directory holds, or `path` itself when it is a file.
fn find_lrp(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    let mut found = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
        let entry = entry.map_err(|e| e.to_string())?.path();
        if entry.extension().is_some_and(|ext| ext == "lrp") {
            found.push(entry);
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!("{} holds no .lrp project file", path.display())),
        _ => Err(format!(
            "{} holds more than one .lrp; name one",
            path.display()
        )),
    }
}

fn default_author() -> String {
    ["USER", "USERNAME"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// The runtime's compile check as a package `Check`.
#[cfg(feature = "check")]
fn compile_check(project: &Path) -> Result<(), Vec<String>> {
    let report = lazyrad_runtime::check_project(project).map_err(|e| vec![e.to_string()])?;
    if report.is_empty() {
        return Ok(());
    }
    let mut lines: Vec<String> = report
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.file.display(), d.message))
        .collect();
    lines.extend(
        report
            .scripts
            .iter()
            .map(|s| format!("{}:{}:{}: {}", s.file, s.line, s.column, s.message)),
    );
    Err(lines)
}

fn run(options: &Options) -> Result<(), String> {
    let player_path = options
        .player
        .clone()
        .or_else(|| std::env::var_os("LAZYRAD_PLAYER").map(PathBuf::from))
        .ok_or("no player: pass --player <lrplay.elf> or set LAZYRAD_PLAYER")?;
    let player = read_player(&player_path).map_err(|e| e.to_string())?;
    let lrp = find_lrp(&options.project)?;
    let author = options.author.clone().unwrap_or_else(default_author);

    #[cfg(feature = "check")]
    let check: Option<lzp::Check<'_>> = options.check.then_some(&compile_check as lzp::Check<'_>);
    #[cfg(not(feature = "check"))]
    let check: Option<lzp::Check<'_>> = None;

    let package = build_package(&PackageRequest {
        project: &lrp,
        player: &player,
        author: &author,
        system_name: options.system_name.as_deref(),
        description: options.description.as_deref(),
        icons: None,
        check,
    })
    .map_err(|e| e.to_string())?;

    let path = write_package(&options.out, &package).map_err(|e| e.to_string())?;
    println!(
        "wrote {} ({} bytes, {} entries) system_name={} version={}",
        path.display(),
        package.bytes.len(),
        package.entries,
        package.system_name,
        package.version
    );
    if let Some(dir) = &options.install_dev {
        let fallback = match dir {
            Some(dir) => DevInstaller::new(dir),
            None => DevInstaller::on_lazyos(),
        };
        let app = install_with_fallback(&PkgdInstaller, &fallback, &package)
            .map_err(|e| e.to_string())?;
        println!("{}", app.summary());
    }
    Ok(())
}

fn main() -> ExitCode {
    let options = match parse(std::env::args_os().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("lazyrad-pack: {message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("lazyrad-pack: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(args: &[&str]) -> Result<Options, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn install_dev_before_the_project_does_not_take_it_as_its_directory() {
        let options = parse_str(&["--player", "p.elf", "--install-dev", "proj"]).unwrap();
        assert_eq!(options.project, Path::new("proj"));
        assert_eq!(options.install_dev, Some(None));
    }

    #[test]
    fn a_full_command_line_parses() {
        let options = parse_str(&[
            "proj",
            "--player",
            "p.elf",
            "--out",
            "o",
            "--author",
            "Ada",
            "--system-name",
            "org.x.y",
            "--description",
            "d",
            "--no-check",
            "--install-dev",
            "saved",
        ])
        .unwrap();
        assert_eq!(options.project, Path::new("proj"));
        assert_eq!(options.player.as_deref(), Some(Path::new("p.elf")));
        assert_eq!(options.out, Path::new("o"));
        assert!(!options.check);
        assert_eq!(options.install_dev, Some(Some(PathBuf::from("saved"))));
    }

    #[test]
    fn install_dev_without_a_directory_uses_the_default() {
        let options = parse_str(&["proj", "--install-dev"]).unwrap();
        assert_eq!(options.install_dev, Some(None));
        let options = parse_str(&["--install-dev", "--no-check", "proj"]).unwrap();
        assert_eq!(options.install_dev, Some(None));
        assert!(!options.check);
    }

    #[test]
    fn usage_errors_are_reported() {
        assert!(parse_str(&[]).is_err());
        assert!(parse_str(&["a", "b"]).is_err());
        assert!(parse_str(&["a", "--wat"]).is_err());
        assert!(parse_str(&["a", "--player"]).is_err());
    }
}
