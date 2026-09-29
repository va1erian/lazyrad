#![forbid(unsafe_code)]

//! The Project Explorer's rows and the double-click detection the [`TreeView`]
//! needs a hand with.
//!
//! xui's [`TreeView`] reports a row selection but not a double click, so
//! [`DoubleClick`] folds two selections of the same row inside a short interval
//! into one "open" event. The rows themselves are a flat list — a `Forms` group
//! and a `Modules` group, each holding its items — with an entry per row so a
//! selected node id maps straight back to the item it names.
//!
//! [`TreeView`]: xui_core::widget::TreeView

use std::time::{Duration, Instant};

use xui_core::Lucide;
use xui_core::widget::{NodeId, TreeRow};

use crate::project::ProjectSession;

/// How long two selections of the same row count as a double click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// The group a row belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// The Forms group.
    Forms,
    /// The Modules group.
    Modules,
}

impl Group {
    /// The group's label in the tree.
    pub fn label(self) -> &'static str {
        match self {
            Group::Forms => "Forms",
            Group::Modules => "Modules",
        }
    }
}

/// What a Project Explorer row stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExplorerItem {
    /// The project root row.
    Project,
    /// A group header.
    Group(Group),
    /// A form item.
    Form(String),
    /// A standard module item.
    Module(String),
}

impl ExplorerItem {
    /// The item's name, or `None` for the project root and group headers.
    pub fn name(&self) -> Option<&str> {
        match self {
            ExplorerItem::Form(name) | ExplorerItem::Module(name) => Some(name),
            ExplorerItem::Project | ExplorerItem::Group(_) => None,
        }
    }

    /// Whether the item is a form (so it has an object view).
    pub fn is_form(&self) -> bool {
        matches!(self, ExplorerItem::Form(_))
    }
}

/// The rows of the Project Explorer and the entry each row names.
pub struct Explorer {
    /// The flat rows handed to the tree.
    pub rows: Vec<TreeRow>,
    /// One entry per row, index-aligned with `rows`.
    pub entries: Vec<ExplorerItem>,
}

impl Explorer {
    /// Builds the project root, the groups and their items for `session`.
    pub fn build(session: &ProjectSession) -> Explorer {
        let mut explorer = Explorer {
            rows: Vec::new(),
            entries: Vec::new(),
        };
        explorer.push(
            TreeRow::new(session.name(), 0)
                .expandable(true)
                .expanded(true)
                .icon(Lucide::Package),
            ExplorerItem::Project,
        );
        explorer.group(Group::Forms, &session.form_names());
        explorer.group(Group::Modules, &session.module_names());
        explorer
    }

    /// An empty explorer, for when no project is open.
    pub fn empty() -> Explorer {
        Explorer {
            rows: Vec::new(),
            entries: Vec::new(),
        }
    }

    /// Appends a group header and its item rows.
    fn group(&mut self, group: Group, names: &[&str]) {
        self.push(
            TreeRow::new(group.label(), 1)
                .expandable(true)
                .expanded(true)
                .icon(Lucide::Folder),
            ExplorerItem::Group(group),
        );
        for name in names {
            let (kind, icon) = match group {
                Group::Forms => (ExplorerItem::Form((*name).to_owned()), Lucide::AppWindow),
                Group::Modules => (ExplorerItem::Module((*name).to_owned()), Lucide::FileCode),
            };
            self.push(TreeRow::new(*name, 2).icon(icon), kind);
        }
    }

    /// Appends a row and its entry.
    fn push(&mut self, row: TreeRow, entry: ExplorerItem) {
        self.rows.push(row);
        self.entries.push(entry);
    }

    /// The entry a selected node id names.
    pub fn entry(&self, node: NodeId) -> Option<&ExplorerItem> {
        self.entries.get(node)
    }

    /// The row id of an item named `name`, so the app can select it.
    pub fn node_of(&self, name: &str) -> Option<NodeId> {
        self.entries
            .iter()
            .position(|entry| entry.name() == Some(name))
    }
}

/// Folds two selections of the same node inside [`DOUBLE_CLICK`] into one.
///
/// The tree's selection event fires on the first click, so the second click
/// arrives as another selection of the same node. The detector keeps only the
/// most recent node and time; a different node, or a slow second click, starts
/// a fresh pair.
pub struct DoubleClick {
    last: Option<(NodeId, Instant)>,
    threshold: Duration,
}

impl DoubleClick {
    /// A detector with the default [`DOUBLE_CLICK`] threshold.
    pub fn new() -> DoubleClick {
        DoubleClick {
            last: None,
            threshold: DOUBLE_CLICK,
        }
    }

    /// A detector with an explicit threshold, for tests.
    pub fn with_threshold(threshold: Duration) -> DoubleClick {
        DoubleClick {
            last: None,
            threshold,
        }
    }

    /// Registers a selection of `node` now, returning whether it is the second
    /// click of a double click.
    pub fn register(&mut self, node: NodeId) -> bool {
        self.register_at(node, Instant::now())
    }

    /// Registers a selection of `node` at `now`.
    pub fn register_at(&mut self, node: NodeId, now: Instant) -> bool {
        let double = self
            .last
            .is_some_and(|(last, at)| last == node && now.duration_since(at) <= self.threshold);
        self.last = if double { None } else { Some((node, now)) };
        double
    }
}

impl Default for DoubleClick {
    fn default() -> Self {
        DoubleClick::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "lazyrad-ide-explorer-{label}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn the_tree_has_a_project_root_and_a_group_per_item_kind() {
        let dir = scratch("build");
        let mut session = ProjectSession::create("MyApp", &dir).expect("create");
        session.add_module();
        let explorer = Explorer::build(&session);

        let labels: Vec<&str> = explorer.rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(
            labels,
            ["MyApp", "Forms", "Form1", "Modules", "Module1"],
            "the project is a root row above the groups"
        );
        assert_eq!(explorer.entry(0), Some(&ExplorerItem::Project));
        assert_eq!(explorer.entry(1), Some(&ExplorerItem::Group(Group::Forms)));
        assert_eq!(
            explorer.entry(2),
            Some(&ExplorerItem::Form("Form1".to_owned()))
        );
        assert_eq!(
            explorer.entry(4),
            Some(&ExplorerItem::Module("Module1".to_owned()))
        );
        assert_eq!(explorer.node_of("Form1"), Some(2));
        assert_eq!(explorer.node_of("Module1"), Some(4));
        assert_eq!(explorer.entry(99), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_row_carries_the_icon_of_its_kind() {
        let dir = scratch("icons");
        let mut session = ProjectSession::create("MyApp", &dir).expect("create");
        session.add_module();
        let explorer = Explorer::build(&session);

        let icon = |row: &TreeRow| row.icon.clone();
        assert_eq!(icon(&explorer.rows[0]), Some(Lucide::Package.into()));
        assert_eq!(icon(&explorer.rows[1]), Some(Lucide::Folder.into()));
        assert_eq!(icon(&explorer.rows[2]), Some(Lucide::AppWindow.into()));
        assert_eq!(icon(&explorer.rows[4]), Some(Lucide::FileCode.into()));

        for row in &explorer.rows {
            assert!(row.icon.is_some(), "every row has an icon: {}", row.label);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_explorer_has_no_rows() {
        assert!(Explorer::empty().rows.is_empty());
    }

    #[test]
    fn a_second_click_on_the_same_node_is_a_double_click() {
        let start = Instant::now();
        let mut detector = DoubleClick::with_threshold(Duration::from_millis(500));

        assert!(!detector.register_at(1, start));
        assert!(detector.register_at(1, start + Duration::from_millis(100)));
        // A third quick click starts a new pair.
        assert!(!detector.register_at(1, start + Duration::from_millis(150)));
        // A different node never pairs with the previous one.
        assert!(!detector.register_at(2, start + Duration::from_millis(160)));
    }

    #[test]
    fn a_slow_second_click_is_not_a_double_click() {
        let start = Instant::now();
        let mut detector = DoubleClick::with_threshold(Duration::from_millis(500));
        assert!(!detector.register_at(1, start));
        assert!(!detector.register_at(1, start + Duration::from_millis(600)));
    }
}
