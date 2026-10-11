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
    Entry, Handle, IntoEntry, Layout, LayoutExt, build, button, column, edit, label, list, panel,
    row, split, tabs, toolbar, tree_view,
};
use xui_core::backend::Result as UiResult;
use xui_core::geometry::Rect;
use xui_core::layout::Insets;
use xui_core::widget::{Edit, Label, ListView, Panel, Split, Tabs, Toolbar, TreeView};
use xui_core::{Dip, dip};

use super::{
    MENU_HEIGHT, Msg, PANE_MARGIN, PANE_TITLE, PaneSlot, STATUS_HEIGHT, TOOLBAR,
    TOOLBAR_GROUP_STARTS, TOOLBAR_HEIGHT,
};

/// The gap above and below a pane title.
const TITLE_PAD: Dip = dip(4.0);
/// The height of the Output pane's log line.
const OUTPUT_LINE: Dip = dip(24.0);
/// The Immediate pane's input row height, the Evaluate button's width and the
/// gap between them.
const IMMEDIATE_ROW: Dip = dip(30.0);
const IMMEDIATE_BUTTON: Dip = dip(88.0);
const IMMEDIATE_GAP: Dip = dip(6.0);

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
    /// The bottom area's tabs (Output, Locals, Call Stack, Immediate).
    pub(super) bottom_tabs: Rc<Tabs<Msg>>,
    pub(super) locals_list: Rc<ListView<Msg>>,
    pub(super) stack_list: Rc<ListView<Msg>>,
    pub(super) immediate_list: Rc<ListView<Msg>>,
    pub(super) immediate_edit: Rc<Edit<Msg>>,
}

/// The bottom area's tab pages, in order. Selecting a page by index goes
/// through these constants.
pub(super) const TAB_OUTPUT: usize = 0;
pub(super) const TAB_LOCALS: usize = 1;
pub(super) const TAB_IMMEDIATE: usize = 3;

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
    bottom_tabs: Handle<Tabs<Msg>>,
    locals_list: Handle<ListView<Msg>>,
    stack_list: Handle<ListView<Msg>>,
    immediate_list: Handle<ListView<Msg>>,
    immediate_edit: Handle<Edit<Msg>>,
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
        bottom_tabs: h.bottom_tabs.get(),
        locals_list: h.locals_list.get(),
        stack_list: h.stack_list.get(),
        immediate_list: h.immediate_list.get(),
        immediate_edit: h.immediate_edit.get(),
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

    // The bottom area: the Output pane with its Error List, and the debugger's
    // Locals, Call Stack and Immediate panes as tabs beside it.
    let output = column().children((
        label("Output").bind(&h.output).height(OUTPUT_LINE),
        list()
            .on_activate(Msg::ErrorActivated)
            .bind(&h.error_list)
            .fill(1),
    ));
    let locals = column().child(list().bind(&h.locals_list).fill(1));
    let stack = column().child(
        list()
            .on_activate(Msg::StackActivated)
            .bind(&h.stack_list)
            .fill(1),
    );
    let immediate = column().children((
        list().bind(&h.immediate_list).fill(1),
        row()
            .gap(IMMEDIATE_GAP)
            .children((
                edit()
                    .placeholder("Expression to evaluate in the stopped frame")
                    .on_change(Msg::ImmediateChanged)
                    .bind(&h.immediate_edit)
                    .fill(1),
                button("Evaluate")
                    .on_click(Msg::ImmediateEvaluate)
                    .fixed(IMMEDIATE_BUTTON),
            ))
            .height(IMMEDIATE_ROW),
    ));
    let bottom = filled(
        tabs()
            .page("Output", output)
            .page("Locals", locals)
            .page("Call Stack", stack)
            .page("Immediate", immediate)
            .bind(&h.bottom_tabs),
    );
    let rest = split(filled(centre), bottom)
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
