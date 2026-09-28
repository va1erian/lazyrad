#![forbid(unsafe_code)]

//! The project model: [`Project`] and [`ProjectItem`].
//!
//! A project names the forms and modules that make it up. The forms themselves
//! are [`xui_form::FormDoc`]s (see [`crate::io`] and [`crate::lazyrad_catalog`]);
//! this crate no longer has a form model of its own.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A `.lrp` project: the file that ties the items together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// The project name, also the stem of the `.lrp` file.
    pub name: String,
    /// The project version, as a free-form string.
    pub version: String,
    /// The name of the item (form or module) run first.
    pub startup: String,
    /// The forms and modules that make up the project.
    pub items: Vec<ProjectItem>,
}

impl Project {
    /// A project with the given name, no items and itself as startup.
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            name: name.clone(),
            version: "0.1.0".to_owned(),
            startup: name,
            items: Vec::new(),
        }
    }

    /// The item named `name`, if any.
    pub fn item(&self, name: &str) -> Option<&ProjectItem> {
        self.items.iter().find(|item| item.name() == name)
    }

    /// The startup item, if it exists.
    pub fn startup_item(&self) -> Option<&ProjectItem> {
        self.item(&self.startup)
    }

    /// The file name of the `.lrp` on disk (`<name>.lrp`).
    pub fn file_name(&self) -> String {
        format!("{}.lrp", self.name)
    }
}

/// One form or standard module in a project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectItem {
    /// A form, with a layout file and a code-behind file.
    Form {
        /// The item name, matching `[window].name` in the layout.
        name: String,
        /// The `.lfm` layout, relative to the project directory.
        layout: PathBuf,
        /// The `.rhai` code-behind, relative to the project directory.
        code: PathBuf,
    },
    /// A standard module: code only.
    Module {
        /// The module name.
        name: String,
        /// The `.rhai` source, relative to the project directory.
        code: PathBuf,
    },
}

impl ProjectItem {
    /// The item's name.
    pub fn name(&self) -> &str {
        match self {
            Self::Form { name, .. } | Self::Module { name, .. } => name,
        }
    }

    /// The `.rhai` code path, relative to the project directory.
    pub fn code(&self) -> &Path {
        match self {
            Self::Form { code, .. } | Self::Module { code, .. } => code,
        }
    }

    /// The `.lfm` layout path for a form, or `None` for a module.
    pub fn layout(&self) -> Option<&Path> {
        match self {
            Self::Form { layout, .. } => Some(layout),
            Self::Module { .. } => None,
        }
    }

    /// Whether this item is a form.
    pub fn is_form(&self) -> bool {
        matches!(self, Self::Form { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_item_accessors_cover_both_variants() {
        let form = ProjectItem::Form {
            name: "main_form".to_owned(),
            layout: PathBuf::from("main_form.lfm"),
            code: PathBuf::from("main_form.rhai"),
        };
        let module = ProjectItem::Module {
            name: "util".to_owned(),
            code: PathBuf::from("util.rhai"),
        };
        assert_eq!(form.name(), "main_form");
        assert_eq!(form.code(), Path::new("main_form.rhai"));
        assert_eq!(form.layout(), Some(Path::new("main_form.lfm")));
        assert!(form.is_form());
        assert_eq!(module.layout(), None);
        assert!(!module.is_form());
    }

    #[test]
    fn startup_item_resolves() {
        let mut project = Project::new("MyApp");
        project.items.push(ProjectItem::Module {
            name: "util".to_owned(),
            code: PathBuf::from("util.rhai"),
        });
        assert!(project.startup_item().is_none());
        project.startup = "util".to_owned();
        assert_eq!(project.startup_item().map(ProjectItem::name), Some("util"));
        assert_eq!(project.file_name(), "MyApp.lrp");
    }
}
