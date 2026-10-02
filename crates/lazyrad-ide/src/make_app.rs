#![forbid(unsafe_code)]

//! File → Make LazyOS App… (LazyOS plan P4): the pure parts.
//!
//! The IDE saves and checks the project, builds a `.lzp` with the same player it
//! runs programs with, asks the installer what the package would be granted
//! ([`PackageReview`], from `pkgd`'s `Inspect`), shows that to the user and only
//! then installs. The widgets and the message flow are in [`crate::app`]; this
//! module builds the package and writes the consent text, so both are testable
//! without a window.

use std::path::Path;

use lazyrad_packager::lzp::{
    BuiltPackage, HostPermissions, PackageRequest, PackageReview, build_package, read_player,
};

use crate::run;

/// The File menu label.
pub const MENU_LABEL: &str = "Make LazyOS &App…";

/// Builds the package for `project_file` with the player the IDE runs
/// programs with (`player_override` is the `player_path` setting).
///
/// The caller has already saved the project and run the compile check, so
/// neither is repeated here.
pub fn build(
    project_file: &Path,
    player_override: Option<&Path>,
    author: &str,
) -> Result<BuiltPackage, String> {
    let player_path = run::resolve_player(player_override).map_err(|error| error.to_string())?;
    let player = read_player(&player_path).map_err(|error| error.to_string())?;
    // The platform knows which system services the scripts call (LazyOS:
    // `sys::confd::get(..)` needs `os.lazy.confd.v1`), so the app is granted
    // exactly those and the consent screen lists them.
    let permissions = |scripts: &[&str]| {
        let found = lazyrad_runtime::platform::current().script_permissions(scripts);
        HostPermissions {
            interfaces: found.interfaces,
            topics: found.topics,
        }
    };
    build_package(&PackageRequest {
        project: project_file,
        player: &player,
        author,
        system_name: None,
        description: None,
        icons: None,
        check: None,
        permissions: Some(&permissions),
    })
    .map_err(|error| error.to_string())
}

/// The consent dialog's title.
pub fn consent_title(review: &PackageReview) -> String {
    format!("Install {} {}?", review.name, review.version)
}

/// The consent dialog's body: who made it, what it is called on the system and
/// every permission with the installer's own explanation. Nothing is left out or
/// shortened: the user is agreeing to exactly this list.
pub fn consent_text(review: &PackageReview) -> String {
    let mut text = format!(
        "By {}. System name: {}.\n",
        review.author, review.system_name
    );
    if review.permissions.is_empty() {
        text.push_str("\nIt asks for no special permissions.");
    } else {
        text.push_str("\nIt asks for permission to:");
        for permission in &review.permissions {
            text.push_str(&format!(
                "\n- [{}] {} ({}: {})",
                permission.risk, permission.explanation, permission.kind, permission.value
            ));
        }
    }
    text
}

/// One line per reason the installer refused the package.
pub fn problem_lines(review: &PackageReview) -> Vec<String> {
    review
        .problems
        .iter()
        .map(|problem| format!("The app cannot be installed: {problem}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use lazyrad_packager::lzp::PermissionNote;

    use super::*;

    fn review(permissions: Vec<PermissionNote>) -> PackageReview {
        PackageReview {
            name: "Todo".into(),
            system_name: "user.ada.todo".into(),
            author: "Ada".into(),
            version: "1.0.0".into(),
            permissions,
            problems: Vec::new(),
        }
    }

    #[test]
    fn the_consent_text_lists_every_permission_with_its_risk() {
        let notes = vec![
            PermissionNote {
                kind: "interface".into(),
                value: "os.lazy.display.v1".into(),
                risk: "low".into(),
                explanation: "Show windows on the desktop".into(),
            },
            PermissionNote {
                kind: "file".into(),
                value: "write:/data/apps/user.ada.todo/data".into(),
                risk: "medium".into(),
                explanation: "Save files in its own folder".into(),
            },
        ];
        let text = consent_text(&review(notes));
        assert!(text.contains("By Ada. System name: user.ada.todo."));
        assert!(text.contains("[low] Show windows on the desktop (interface: os.lazy.display.v1)"));
        assert!(text.contains("[medium] Save files in its own folder"));
        assert_eq!(consent_title(&review(Vec::new())), "Install Todo 1.0.0?");
    }

    #[test]
    fn no_permissions_is_said_plainly() {
        assert!(consent_text(&review(Vec::new())).contains("no special permissions"));
    }

    #[test]
    fn problems_become_one_line_each() {
        let mut r = review(Vec::new());
        r.problems = vec!["a".into(), "b".into()];
        assert_eq!(problem_lines(&r).len(), 2);
        assert!(problem_lines(&r)[0].ends_with(": a"));
    }
}
