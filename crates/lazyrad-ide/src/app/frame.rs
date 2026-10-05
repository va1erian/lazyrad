#![forbid(unsafe_code)]

//! The IDE window's frame as one layout: the menu bar slot, the toolbar, the
//! nested splits with their panes, and the status line.
//!
//! The panes are cards with a title strip above their content: the Toolbox,
//! the Project Explorer, the Properties (whose grid is mounted later, when a
//! form tab is in front) and the Output pane with its Error List. The menu bar
//! and the document tabs sit in plain slots, because the app replaces them
//! while the window lives.

use std::rc::Rc;

use lazyrad_designer::Toolbox;
use xui_core::app::Ui;
use xui_core::arrange::{
    Entry, Handle, IntoEntry, Layout, LayoutExt, build, column, label, list, panel, row, split,
    toolbar, tree_view,
};
use xui_core::backend::Result as UiResult;
use xui_core::geometry::Rect;
use xui_core::layout::Insets;
use xui_core::widget::{Label, ListView, Panel, Split, Toolbar, TreeView};
use xui_core::{Dip, dip};

use super::{
    MENU_HEIGHT, Msg, PANE_MARGIN, PANE_TITLE, PaneSlot, STATUS_HEIGHT, TOOLBAR,
    TOOLBAR_GROUP_STARTS, TOOLBAR_HEIGHT,
};

/// The gap above and below a pane title.
const TITLE_PAD: Dip = dip(4.0);
/// The height of the Output pane's log line.
const OUTPUT_LINE: Dip = dip(24.0);

/// The frame's widgets the app keeps reaching after the layout is mounted.
pub(super) struct Frame {
    pub(super) menu_slot: Rc<Panel<Msg>>,
    pub(super) outer: Rc<Split<Msg>>,
    pub(super) rest: Rc<Split<Msg>>,
    pub(super) centre: Rc<Split<Msg>>,
    pub(super) right: Rc<Split<Msg>>,
    pub(super) project_panel: Rc<Panel<Msg>>,
    pub(super) properties_panel: Rc<Panel<Msg>>,
    pub(super) docs_slot: Rc<Panel<Msg>>,
    pub(super) tree: Rc<TreeView<Msg>>,
    pub(super) output: Rc<Label<Msg>>,
    pub(super) error_list: Rc<ListView<Msg>>,
    pub(super) status: Rc<Label<Msg>>,
}

/// The handles [`build`] fills as the layout is mounted.
#[derive(Default)]
struct Handles {
    menu_slot: Handle<Panel<Msg>>,
    toolbar: Handle<Toolbar<Msg>>,
    outer: Handle<Split<Msg>>,
    rest: Handle<Split<Msg>>,
    centre: Handle<Split<Msg>>,
    right: Handle<Split<Msg>>,
    project_panel: Handle<Panel<Msg>>,
    properties_panel: Handle<Panel<Msg>>,
    docs_slot: Handle<Panel<Msg>>,
    tree: Handle<TreeView<Msg>>,
    output: Handle<Label<Msg>>,
    error_list: Handle<ListView<Msg>>,
    status: Handle<Label<Msg>>,
}

/// Mounts the frame as the window's content. The split positions are set
/// afterwards, from the settings, by the app's `layout_frame`.
pub(super) fn mount(ui: &Ui<Msg>) -> UiResult<Frame> {
    let h = Handles::default();
    ui.root(
        column().children((
            panel(column())
                .plain()
                .bind(&h.menu_slot)
                .height(MENU_HEIGHT),
            main_toolbar().bind(&h.toolbar).height(TOOLBAR_HEIGHT),
            outer(&h).fill(1),
            row()
                .padding(Insets::new(PANE_MARGIN, dip(0.0), dip(0.0), dip(0.0)))
                .child(label("Design").bind(&h.status).fill(1))
                .height(STATUS_HEIGHT),
        )),
    )?;
    Ok(Frame {
        menu_slot: h.menu_slot.get(),
        outer: h.outer.get(),
        rest: h.rest.get(),
        centre: h.centre.get(),
        right: h.right.get(),
        project_panel: h.project_panel.get(),
        properties_panel: h.properties_panel.get(),
        docs_slot: h.docs_slot.get(),
        tree: h.tree.get(),
        output: h.output.get(),
        error_list: h.error_list.get(),
        status: h.status.get(),
    })
}

/// The main toolbar: one icon button per [`TOOLBAR`] entry, in groups.
fn main_toolbar() -> xui_core::arrange::Build<Toolbar<Msg>, Msg> {
    let mut bar = toolbar();
    for (index, entry) in TOOLBAR.iter().enumerate() {
        if TOOLBAR_GROUP_STARTS.contains(&index) {
            bar = bar.separator();
        }
        let tooltip = entry.tooltip();
        bar = match entry.label {
            Some(label) => bar.item_with_text(entry.icon, tooltip, label),
            None => bar.item(entry.icon, tooltip),
        };
    }
    bar.then(|bar| {
        bar.on_click(|index| TOOLBAR.get(index).map(|entry| Msg::Command(entry.command)))
    })
}

/// The nested splits: the toolbox on the left of the rest, which stacks the
/// centre over the Output pane; the centre puts the documents left of the
/// Project/Properties column.
fn outer(h: &Handles) -> Entry<Msg> {
    let toolbox = build(|ui| Toolbox::new(ui, Rect::default(), Msg::Toolbox));
    let toolbox_pane = pane("Toolbox", toolbox, None);

    let tree = tree_view().bind(&h.tree).then(|tree| {
        tree.indent_guides(false)
            .on_select(|node| Some(Msg::ExplorerSelected(node)))
            .on_context(|node, at| Some(Msg::ExplorerContext(node, at)))
    });
    let project_pane = pane("Project", tree, Some(&h.project_panel));
    // The grid is mounted in this panel, below the title, when a form tab is
    // in front.
    let properties_pane =
        filled(panel(column().child(title("Properties"))).bind(&h.properties_panel));
    let right = split(project_pane, properties_pane)
        .stacked()
        .min(dip(60.0), dip(60.0))
        .on_moved(|position| Msg::PaneMoved(PaneSlot::Project, position.value()))
        .bind(&h.right);

    let docs = filled(panel(column()).plain().bind(&h.docs_slot));
    let centre = split(docs, filled(right))
        .min(dip(200.0), dip(120.0))
        .on_moved(|position| Msg::PaneMoved(PaneSlot::Right, position.value()))
        .bind(&h.centre);

    let output = filled(panel(
        column().children((
            label("Output").bind(&h.output).height(OUTPUT_LINE),
            list()
                .on_activate(Msg::ErrorActivated)
                .bind(&h.error_list)
                .fill(1),
        )),
    ));
    let rest = split(filled(centre), output)
        .stacked()
        .min(dip(200.0), dip(40.0))
        .on_moved(|position| Msg::PaneMoved(PaneSlot::Output, position.value()))
        .bind(&h.rest);

    split(toolbox_pane, filled(rest))
        .min(dip(80.0), dip(200.0))
        .on_moved(|position| Msg::PaneMoved(PaneSlot::Toolbox, position.value()))
        .bind(&h.outer)
        .into_entry()
}

/// A pane: a card filling the split's pane, its title above `content`. The
/// card fills `card` when given.
fn pane(
    title_text: &str,
    content: impl IntoEntry<Msg>,
    card: Option<&Handle<Panel<Msg>>>,
) -> Layout<Msg> {
    let mut built = panel(column().children((title(title_text), content.fill(1))));
    if let Some(card) = card {
        built = built.bind(card);
    }
    filled(built)
}

/// A pane's title strip: the title inset from the pane's edges.
fn title(text: &str) -> Entry<Msg> {
    row()
        .padding(Insets::new(PANE_MARGIN, TITLE_PAD, PANE_MARGIN, TITLE_PAD))
        .child(label(text).fill(1))
        .height(PANE_TITLE)
}

/// A layout that `entry` fills, for a split's pane.
fn filled(entry: impl IntoEntry<Msg>) -> Layout<Msg> {
    column().child(entry.fill(1))
}
