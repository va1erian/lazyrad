#![forbid(unsafe_code)]

//! The IDE's main window: the VB6 layout, menu bar, toolbar and command
//! dispatch (PLAN.md §9, issues #7 and #8).
//!
//! The window is a menu bar and toolbar docked at the top, then nested splits:
//! the toolbox on the left, the tabbed document area in the centre, the Output
//! pane at the bottom, and the Project Explorer and Properties panes in a
//! right-hand column. Pane sizes come from [`Settings`] and are written back
//! when a divider moves.
//!
//! Project management lives here too: New/Open/Save operate on a
//! [`ProjectSession`], the Project Explorer is a [`TreeView`] over its items,
//! forms and modules open as document tabs, and a dirty project shows `*` in
//! the title bar and prompts on exit.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use lazyrad_designer::{
    Designer, DesignerMsg, PropertyGrid, PropertyGridMsg, Target, Toolbox, ToolboxMsg,
    handler_events, rename_handlers,
};
use lazyrad_project::Catalog;
use xui_code_editor::{
    Editor, FontConfig, Marker, MarkerKind, Options as EditorOptions, Query, RhaiHighlighter,
};
use xui_core::app::{App, Ui};
use xui_core::backend::{BackendError, Event, PlatformSpec, Result as UiResult, TimerId, WidgetId};
use xui_core::geometry::Point;
use xui_core::layout::Dock;
use xui_core::units::Px;
use xui_core::widget::{
    ComboBox, Dialog, DialogAction, HasText, Label, ListModel, ListView, Menu, MenuId, MenuScope,
    Panel, Split, Tabs, Toolbar, TreeView,
};
use xui_core::{Dip, Lucide, Rect, dip};

use crate::command::{Command, Dispatcher};
use crate::compile::{self, CodeDiagnostic, CompileScheduler};
use crate::dialog::ChoiceDialog;
use crate::explorer::{DoubleClick, Explorer, ExplorerItem};
use crate::platform::dialogs;
use crate::procedures::{self, ObjectEntry};
use crate::project::{DEFAULT_PROJECT, ProjectSession};
use crate::run::{self, RunEvent, RunId, RunState};
use crate::settings::{Settings, ThemeChoice};

/// The menu bar's height.
const MENU_HEIGHT: Dip = dip(24.0);
/// The toolbar's height.
const TOOLBAR_HEIGHT: Dip = dip(32.0);
/// The status bar's height.
const STATUS_HEIGHT: Dip = dip(22.0);
/// The divider thickness xui's [`Split`] draws, so computed pane sizes are
/// exact.
const DIVIDER: f32 = 5.0;
/// The tab strip height xui's [`Tabs`] reserves at the top of a page. The code
/// view positions its widgets below it, matching the tab layout.
const TABS_STRIP: Dip = dip(32.0);
/// The code view's procedure-combo header height.
const CODE_HEADER: Dip = dip(26.0);
/// The device-pixel height of a pane title, so a pane's widget starts below it.
const PANE_TITLE: i32 = 28;
/// The margin between a pane's edge and its title, and between the window's
/// left edge and the status text.
const PANE_MARGIN: Dip = dip(8.0);
/// How often the compile scheduler is polled, in milliseconds.
const COMPILE_POLL_MS: u32 = 100;
/// How many output lines the pane keeps.
const OUTPUT_LINES: usize = 500;
/// The Start Page's welcome text.
const WELCOME: &str = "LazyRAD — the IDE. Open or create a project to begin.";

/// A message the IDE app handles.
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    /// A command was chosen from the menu, the toolbar or a shortcut.
    Command(Command),
    /// The window resized; re-flow the frame.
    Relayout,
    /// A divider moved, carrying the first pane's new extent in design units.
    PaneMoved(PaneSlot, f32),
    /// A Project Explorer row was selected.
    ExplorerSelected(usize),
    /// A Project Explorer row was right-clicked, at a node-local point.
    ExplorerContext(usize, Point),
    /// A context-menu item was chosen.
    ContextAction(ContextAction),
    /// A prompt dialog was accepted, carrying its text.
    PromptSubmitted(String),
    /// A prompt dialog was cancelled.
    PromptCancelled,
    /// The save-changes dialog was answered.
    SaveChoice(SaveChoice),
    /// A code document's text changed.
    DocumentEdited(String, String),
    /// The selected document tab changed (page 0 is the Start Page).
    TabChanged(usize),
    /// A background compile finished. `revision` guards against a stale result
    /// arriving after a newer edit.
    CompileFinished {
        /// The item whose source was compiled.
        name: String,
        /// The revision the result belongs to.
        revision: u64,
        /// The parse errors, or empty when the source compiled.
        errors: Vec<CodeDiagnostic>,
    },
    /// An Error List row was activated (double-clicked or Return).
    ErrorActivated(usize),
    /// The object combo of a form's code window changed.
    ObjectChanged(String, usize),
    /// The procedure combo of a form's code window changed.
    ProcedureChanged(String, usize),
    /// A designer input message, tagged with the form document it belongs to,
    /// so a message for a closed tab is dropped rather than reaching another.
    Designer {
        /// The form the designer is showing.
        document: String,
        /// The designer input to apply.
        msg: DesignerMsg,
    },
    /// A toolbox message; the host forwards it to the active designer.
    Toolbox(ToolboxMsg),
    /// A property-grid message, tagged with the form the grid was bound to.
    PropertyGrid {
        /// The form the grid was editing.
        form: String,
        /// The grid input to apply.
        msg: PropertyGridMsg,
    },
    /// A designer renamed a control; the form's `.rhai` handlers are rewritten.
    RenamedControl {
        /// The form whose script is rewritten.
        form: String,
        /// The control's old name.
        old: String,
        /// The control's new name.
        new: String,
    },
    /// A designer double-clicked a control (or the form); open its default
    /// event handler in the form's code tab, creating the function if missing.
    OpenDefaultHandler {
        /// The form the designer belongs to.
        form: String,
        /// The object that was double-clicked.
        target: Target,
    },
    /// A running program reported output, a diagnostic or its exit.
    Run(RunId, RunEvent),
}

/// The Error List's rows: one line per diagnostic, each led by the error icon.
struct ErrorRows(Vec<String>);

impl ListModel for ErrorRows {
    fn rows(&self) -> usize {
        self.0.len()
    }

    fn cell(&self, row: usize, column: usize) -> Option<&str> {
        (column == 0).then(|| self.0.get(row).map(String::as_str))?
    }

    fn icon(&self, _row: usize) -> Option<xui_core::icon::IconRef> {
        // Every entry is an error today; warnings would take TriangleAlert.
        Some(Lucide::CircleX.into())
    }
}

/// One diagnostic shown in the Error List, tagged with the document it belongs
/// to so activating the row can open that document.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ErrorEntry {
    /// The item's name.
    name: String,
    /// The parse error.
    diagnostic: CodeDiagnostic,
}

/// Which persisted pane size a divider move changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneSlot {
    /// The toolbox width (the outer split's first pane).
    Toolbox,
    /// The Output height (the rest split's second pane).
    Output,
    /// The right column's width (the centre split's second pane).
    Right,
    /// The Project height (the right split's first pane).
    Project,
}

/// A Project Explorer context-menu action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextAction {
    /// Open the item's code in the editor.
    ViewCode,
    /// Open the form in the designer.
    ViewObject,
    /// Rename the item (and its files).
    Rename,
    /// Remove the item from the project.
    Remove,
    /// Make the item the project's startup.
    SetStartup,
}

/// What a prompt dialog is asking for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptKind {
    /// A name for a new project.
    NewProject,
    /// A new name for an item.
    Rename(String),
    /// A find query, searched from the caret.
    Find,
    /// The find half of replace-all.
    ReplaceFind,
    /// The replacement text of replace-all.
    ReplaceWith,
    /// A one-based line number to jump to.
    GoToLine,
}

/// The three answers to the save-changes prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveChoice {
    /// Write the project and continue.
    Save,
    /// Throw the changes away and continue.
    Discard,
    /// Stay where we are.
    Cancel,
}

/// What to do once an unsaved project has been dealt with.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    /// Quit the IDE.
    Exit,
    /// Close the project but keep the IDE open.
    Close,
    /// Show the New Project prompt.
    NewProject,
    /// Open the project in this folder.
    Open(PathBuf),
}

/// The map from each command menu entry's [`MenuId`] to the command it raises,
/// shared between the app (which enables/disables entries) and the menu's
/// selection closure.
pub type MenuCommands = Rc<Vec<(MenuId, Command)>>;

/// One main-toolbar entry: the Lucide icon, the command it dispatches, and an
/// optional visible label (`Run` and `End` keep one; the rest are icon-only).
struct ToolbarItem {
    /// The Lucide outline the item draws.
    icon: Lucide,
    /// The command a click (or Return on the focused item) dispatches.
    command: Command,
    /// The visible label, or `None` for an icon-only item.
    label: Option<&'static str>,
}

impl ToolbarItem {
    /// The hover tooltip: the command's name, plus its shortcut when it has one.
    fn tooltip(&self) -> String {
        match self.command.shortcut_text() {
            Some(shortcut) => format!("{} ({shortcut})", self.command.label()),
            None => self.command.label(),
        }
    }
}

/// The toolbar's items, in order, and the command each dispatches.
const TOOLBAR: &[ToolbarItem] = &[
    ToolbarItem {
        icon: Lucide::FilePlus,
        command: Command::NewProject,
        label: None,
    },
    ToolbarItem {
        icon: Lucide::FolderOpen,
        command: Command::OpenProject,
        label: None,
    },
    ToolbarItem {
        icon: Lucide::Save,
        command: Command::Save,
        label: None,
    },
    ToolbarItem {
        icon: Lucide::SaveAll,
        command: Command::SaveAll,
        label: None,
    },
    ToolbarItem {
        icon: Lucide::Undo2,
        command: Command::Undo,
        label: None,
    },
    ToolbarItem {
        icon: Lucide::Redo2,
        command: Command::Redo,
        label: None,
    },
    ToolbarItem {
        icon: Lucide::Scissors,
        command: Command::Cut,
        label: None,
    },
    ToolbarItem {
        icon: Lucide::Copy,
        command: Command::Copy,
        label: None,
    },
    ToolbarItem {
        icon: Lucide::ClipboardPaste,
        command: Command::Paste,
        label: None,
    },
    ToolbarItem {
        icon: Lucide::Play,
        command: Command::RunStart,
        label: Some("Run"),
    },
    ToolbarItem {
        icon: Lucide::Square,
        command: Command::RunEnd,
        label: Some("End"),
    },
];

/// The [`TOOLBAR`] indices that start a new group, drawn with a separator
/// before them: file | undo/redo | clipboard | run. xui separators take no item
/// index, so `on_click` indices still map straight into [`TOOLBAR`].
const TOOLBAR_GROUP_STARTS: &[usize] = &[4, 6, 9];

/// The five context-menu entries, in id order.
const CONTEXT_MENU: &[(usize, ContextAction)] = &[
    (0, ContextAction::ViewCode),
    (1, ContextAction::ViewObject),
    (2, ContextAction::Rename),
    (3, ContextAction::Remove),
    (4, ContextAction::SetStartup),
];

/// Which document a tab holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKind {
    /// A form's object: the live [`lazyrad_designer::Designer`] surface.
    Designer,
    /// A form's or module's code-behind.
    Code,
}

/// The widgets behind one open document.
enum DocumentView {
    /// A live form designer plus the page panel it is parented to and the
    /// scoped handle it was built with (the handle that owns its design-mode
    /// scope, so the rest of the IDE stays live).
    Designer {
        /// The designer, shared with the property grid.
        designer: Rc<RefCell<Designer<Msg>>>,
        /// The handle the designer was built with, for every follow-up call.
        designer_ui: Ui<Msg>,
        /// The page panel the designer lives in, kept so its node lives as long
        /// as the document.
        _page: Panel<Msg>,
    },
    /// The code editor, with the two procedure combos for a form's code.
    Code(Box<CodeView>),
}

/// One open code document: the editor and, for a form, its procedure combos.
struct CodeView {
    /// The object combo (the form and its controls). `None` for a module, which
    /// has no events to bind.
    object: Option<ComboBox<Msg>>,
    /// The event combo, rebuilt when the object changes.
    procedure: Option<ComboBox<Msg>>,
    /// The code editor.
    editor: Rc<Editor<Msg>>,
    /// The object entries the combos are built from.
    objects: Vec<ObjectEntry>,
    /// The index of the selected object, or `None` for a module.
    object_index: Option<usize>,
}

impl CodeView {
    /// The page children, in the order the tab lays them out.
    fn children(&self) -> Vec<WidgetId> {
        let mut ids = Vec::new();
        if let Some(object) = &self.object {
            ids.push(object.id());
        }
        if let Some(procedure) = &self.procedure {
            ids.push(procedure.id());
        }
        ids.push(self.editor.id());
        ids
    }

    /// The selected object's entry, if any.
    fn selected_object(&self) -> Option<&ObjectEntry> {
        self.object_index.and_then(|index| self.objects.get(index))
    }
}

/// The open form tab the Edit commands and the property grid act on: the form
/// name, the designer and the scoped handle it was built with.
type ActiveDesigner = (String, Rc<RefCell<Designer<Msg>>>, Ui<Msg>);

/// One open document tab.
struct Document {
    name: String,
    kind: DocKind,
    title: String,
    dirty: bool,
    view: DocumentView,
}

impl Document {
    /// The tab strip's title: `*` appended while the document is dirty.
    fn tab_title(&self) -> String {
        if self.dirty {
            format!("{}*", self.title)
        } else {
            self.title.clone()
        }
    }
}

/// The IDE shell's application state.
pub struct IdeApp {
    settings: Settings,
    dispatcher: Dispatcher,
    /// Every menu entry that dispatches a command, for enabling/disabling.
    menu_commands: MenuCommands,
    /// Kept alive so the menu's nodes live as long as the app; replaced when
    /// the recent list changes, since xui menus cannot be rebuilt in place.
    menu: Menu<Msg>,
    menu_id: WidgetId,
    _toolbar: Toolbar<Msg>,
    toolbar_id: WidgetId,
    /// The Project Explorer's context menu, shown at a right-clicked row.
    context_menu: Menu<Msg>,
    /// The item the context menu was opened on.
    context_target: Option<String>,

    outer: Split<Msg>,
    rest: Split<Msg>,
    centre: Split<Msg>,
    right: Split<Msg>,
    /// The pane containers, kept alive.
    toolbox_panel: Panel<Msg>,
    output_panel: Panel<Msg>,
    project_panel: Panel<Msg>,
    properties_panel: Panel<Msg>,
    /// The pane titles. A dropped widget destroys its node, so they live as
    /// long as the app.
    pane_labels: Vec<Label<Msg>>,
    /// The control catalog shared by every designer and the property grid.
    catalog: Rc<Catalog>,
    /// The toolbox tiles: one for the whole IDE, routed to the active designer.
    toolbox: Toolbox<Msg>,
    /// The property grid, bound to the active designer. It is rebuilt when the
    /// active form tab changes, so a closed tab's grid cannot keep editing.
    properties_grid: Option<PropertyGrid<Msg>>,
    /// The form the grid is currently bound to, so a queued grid message for a
    /// closed or inactive form is dropped rather than applied elsewhere.
    grid_form: Option<String>,
    /// The centre split's scoped UI, where document tabs are built.
    docs_ui: Ui<Msg>,
    /// The document tab container; `None` only while it is being rebuilt.
    docs: Option<Tabs<Msg>>,
    /// The Start Page's welcome label, kept alive across tab rebuilds.
    start_label: Option<Label<Msg>>,
    documents: Vec<Document>,
    /// The live editors, so the window's timer tick can reach them.
    editors: Rc<RefCell<Vec<Rc<Editor<Msg>>>>>,

    /// The Project Explorer and the entries its rows name.
    explorer: Explorer,
    tree: TreeView<Msg>,
    double_click: DoubleClick,

    /// The open project, if any.
    session: Option<ProjectSession>,
    /// The name prompt currently open, if any.
    prompt: Option<(PromptKind, Dialog<Msg>)>,
    /// The save-changes dialog currently open, if any.
    save_prompt: Option<ChoiceDialog<Msg>>,
    /// What to do once the save prompt is answered.
    pending: Option<Pending>,

    /// The Output pane's label, rewritten as commands are logged.
    output: Label<Msg>,
    output_lines: Vec<String>,
    /// The Error List filling the Output pane below the log line.
    error_list: ListView<Msg>,
    /// Every diagnostic currently shown, across documents, for the Error List
    /// and for jumping to an activated row.
    errors: Vec<ErrorEntry>,
    /// The debounced background-compile scheduler, shared with the timer
    /// mapper.
    diagnostics: Rc<RefCell<CompileScheduler>>,
    /// The repeating timer that polls the scheduler.
    _compile_timer: TimerId,
    /// The last find query text, offered as the prompt's initial value.
    last_find: String,
    /// The last replacement text, offered as the prompt's initial value.
    last_replace: String,
    /// The query of an in-progress replace-all, between its two prompts.
    replace_query: Option<Query>,
    /// The status bar, showing the IDE's Design/Run state.
    status: Label<Msg>,
    /// Launches the player for Run; injectable so tests drive a fake child.
    launcher: Rc<dyn run::Launcher>,
    /// The one running child, if any (issue #16).
    run: RunState,
    /// Set when saving the settings on exit failed; the next Exit quits
    /// without saving, so a read-only config directory cannot trap the user.
    exit_save_failed: bool,
}

impl IdeApp {
    /// Builds the whole window and returns the app the runtime drives.
    pub fn build(ui: &Ui<Msg>, settings: Settings, recent: Vec<PathBuf>) -> UiResult<IdeApp> {
        let dpi = ui.dpi();
        let client = ui.client_rect();

        // The top strips: menu bar, then toolbar, then the split area.
        let menu_band = Dock::new().top(MENU_HEIGHT).split(client, dpi);
        let menu_rect = menu_band.top.unwrap_or(client);
        let toolbar_band = Dock::new().top(TOOLBAR_HEIGHT).split(menu_band.fill, dpi);
        let toolbar_rect = toolbar_band.top.unwrap_or(client);
        // A status bar along the bottom shows Design/Run (issue #16).
        let status_band = Dock::new()
            .bottom(STATUS_HEIGHT)
            .split(toolbar_band.fill, dpi);
        let status_rect = status_band.bottom.unwrap_or_default();
        let main_rect = status_band.fill;

        let (menu, menu_commands) = build_menu(ui, menu_rect, &recent)?;
        let menu_id = menu.id().unwrap_or(WidgetId::NONE);

        let mut toolbar = Toolbar::empty(ui, toolbar_rect)?;
        for (index, entry) in TOOLBAR.iter().enumerate() {
            if TOOLBAR_GROUP_STARTS.contains(&index) {
                toolbar = toolbar.separator();
            }
            let tooltip = entry.tooltip();
            toolbar = match entry.label {
                Some(label) => toolbar.item_with_text(entry.icon, &tooltip, label),
                None => toolbar.item(entry.icon, &tooltip),
            };
        }
        let toolbar =
            toolbar.on_click(|index| TOOLBAR.get(index).map(|entry| Msg::Command(entry.command)));
        let toolbar_id = toolbar.id();

        // The nested splits. Each pane is a container created through the
        // split's own scoped `Ui`, so the split can place its children.
        let outer = Split::row(ui, main_rect)?
            .on_moved(|position| Some(Msg::PaneMoved(PaneSlot::Toolbox, position.value())));
        let toolbox_panel = Panel::new(outer.ui(), Rect::default())?;
        let rest = Split::column(outer.ui(), Rect::default())?
            .on_moved(|position| Some(Msg::PaneMoved(PaneSlot::Output, position.value())));
        outer.pane_a(&[toolbox_panel.id()]);
        outer.pane_b(&[rest.id()]);
        outer.set_min(dip(80.0), dip(200.0));

        let centre = Split::row(rest.ui(), Rect::default())?
            .on_moved(|position| Some(Msg::PaneMoved(PaneSlot::Right, position.value())));
        let output_panel = Panel::new(rest.ui(), Rect::default())?;
        rest.pane_a(&[centre.id()]);
        rest.pane_b(&[output_panel.id()]);
        rest.set_min(dip(200.0), dip(40.0));

        let docs_ui = centre.ui().clone();
        let right = Split::column(centre.ui(), Rect::default())?
            .on_moved(|position| Some(Msg::PaneMoved(PaneSlot::Project, position.value())));
        // The document tabs are built in `rebuild_tabs`, once the whole frame
        // exists; pane A is filled there.
        centre.pane_b(&[right.id()]);
        centre.set_min(dip(200.0), dip(120.0));

        let project_panel = Panel::new(right.ui(), Rect::default())?;
        let properties_panel = Panel::new(right.ui(), Rect::default())?;
        right.pane_a(&[project_panel.id()]);
        right.pane_b(&[properties_panel.id()]);
        right.set_min(dip(60.0), dip(60.0));

        // Pane contents: a title per pane, the Toolbox tiles, the Project
        // Explorer tree, and the Output label.
        let mut labels = Vec::new();
        labels.push(Label::new(
            toolbox_panel.ui(),
            Rect::new(8, 6, 220, 26),
            "Toolbox",
        )?);
        // The toolbox fills its pane below the title. It is parented to the
        // toolbox panel, so its local coordinates start at the panel's origin.
        let toolbox = Toolbox::new(
            toolbox_panel.ui(),
            Rect::new(0, PANE_TITLE, 140, 200),
            Msg::Toolbox,
        )?;
        labels.push(Label::new(
            project_panel.ui(),
            Rect::new(8, 6, 220, 26),
            "Project",
        )?);
        labels.push(Label::new(
            properties_panel.ui(),
            Rect::new(8, 6, 220, 26),
            "Properties",
        )?);
        let output = Label::new(output_panel.ui(), Rect::new(8, 8, 480, 24), "Output")?;
        // The status bar label; `update_status` rewrites it as runs start and end.
        let status = Label::new(ui, status_rect, "Design")?;
        // The Error List: one row per compile diagnostic. Activating a row
        // (double-click or Return) jumps to its position.
        let error_list = ListView::new(output_panel.ui(), Rect::default(), &[])?
            .on_activate(|row| Some(Msg::ErrorActivated(row)));

        // The Project Explorer: an empty tree until a project is opened.
        let tree = TreeView::new(project_panel.ui(), Rect::default(), &[])?
            .indent_guides(false)
            .on_select(|node| Some(Msg::ExplorerSelected(node)))
            .on_context(|node, at| Some(Msg::ExplorerContext(node, at)));

        // The Editor context menu. xui menus cannot be rebuilt at runtime
        // (PLAN.md §10, gap G11), so it is built once and the right-clicked
        // item is remembered separately.
        let context_menu = Menu::context(ui)
            .build(|menu| {
                menu.item(MenuId::new(0), "&View Code")
                    .item(MenuId::new(1), "View &Object")
                    .separator()
                    .item(MenuId::new(2), "&Rename…")
                    .item(MenuId::new(3), "Re&move")
                    .separator()
                    .item(MenuId::new(4), "Set as &Startup");
            })
            .on_select(|id| {
                CONTEXT_MENU
                    .iter()
                    .find(|(entry, _)| MenuId::new(*entry) == id)
                    .map(|(_, action)| Msg::ContextAction(*action))
            });

        // Each editor blinks its caret on its own widget timer, stopped when
        // the editor is dropped, so there is no window timer to forward.
        let editors: Rc<RefCell<Vec<Rc<Editor<Msg>>>>> = Rc::new(RefCell::new(Vec::new()));
        // Editors blink their carets on their own widget timers; the window
        // timer only drives the compile debounce.
        let diagnostics = Rc::new(RefCell::new(CompileScheduler::new()));
        {
            let diagnostics = Rc::clone(&diagnostics);
            let proxy = ui.proxy();
            ui.on_timer(move |_id| {
                // A change armed the debounce; when it comes due, hand the
                // newest source to a worker. The engine is built there, since
                // it is not `Send`.
                let due = diagnostics.borrow_mut().take_due(Instant::now());
                for job in due {
                    let proxy = proxy.clone();
                    std::thread::spawn(move || {
                        let errors = compile::compile_source(&job.source);
                        let _ = proxy.send(Msg::CompileFinished {
                            name: job.name,
                            revision: job.revision,
                            errors,
                        });
                    });
                }
                None
            });
        }
        let compile_timer = ui.set_timer(COMPILE_POLL_MS);

        let mut app = IdeApp {
            settings,
            dispatcher: Dispatcher::new(),
            menu_commands,
            menu,
            menu_id,
            _toolbar: toolbar,
            toolbar_id,
            context_menu,
            context_target: None,
            outer,
            rest,
            centre,
            right,
            toolbox_panel,
            output_panel,
            project_panel,
            properties_panel,
            pane_labels: labels,
            catalog: Rc::new(lazyrad_project::lazyrad_catalog()),
            toolbox,
            properties_grid: None,
            grid_form: None,
            docs_ui,
            docs: None,
            start_label: None,
            documents: Vec::new(),
            editors,
            explorer: Explorer::empty(),
            tree,
            double_click: DoubleClick::new(),
            session: None,
            prompt: None,
            save_prompt: None,
            pending: None,
            output,
            output_lines: Vec::new(),
            error_list,
            errors: Vec::new(),
            diagnostics,
            _compile_timer: compile_timer,
            last_find: String::new(),
            last_replace: String::new(),
            replace_query: None,
            status,
            launcher: Rc::new(run::PlayerLauncher),
            run: RunState::new(),
            exit_save_failed: false,
        };

        app.apply_theme(ui);
        app.dispatcher.set_project_open(false);
        app.refresh_menu();
        if let Err(error) = app.rebuild_tabs() {
            eprintln!("lazyrad-ide: document area: {error}");
        }
        app.layout_frame(ui);
        app.update_title(ui);
        app.log(ui, "LazyRAD IDE ready.");
        app.log(ui, "Create or open a project to begin.");

        // A focused node is what makes the backend deliver `KeyDown` at all;
        // the shortcut backend then routes it to the dispatcher (gap G10).
        ui.focus(app.centre.id());

        // Window-level events reach the null node; re-flow the frame when the
        // window resizes. (`Split` does not re-flow itself on resize.)
        ui.register_events(WidgetId::NONE, |event| {
            matches!(event, Event::Resize { .. }).then_some(Msg::Relayout)
        });

        // The window's close button goes through Exit, which prompts for
        // unsaved changes and saves the settings before quitting.
        ui.on_close(|| Some(Msg::Command(Command::Exit)));
        Ok(app)
    }

    /// Re-flows the whole frame from the window's current client rectangle.
    pub fn layout_frame(&mut self, ui: &Ui<Msg>) {
        let dpi = ui.dpi();
        let client = ui.client_rect();
        let menu_band = Dock::new().top(MENU_HEIGHT).split(client, dpi);
        let menu_rect = menu_band.top.unwrap_or(client);
        let toolbar_band = Dock::new().top(TOOLBAR_HEIGHT).split(menu_band.fill, dpi);
        let toolbar_rect = toolbar_band.top.unwrap_or(client);
        let status_band = Dock::new()
            .bottom(STATUS_HEIGHT)
            .split(toolbar_band.fill, dpi);
        let status_rect = status_band.bottom.unwrap_or_default();
        let main_rect = status_band.fill;

        // The status text sits a little in from the window's left edge.
        let margin = PANE_MARGIN.to_px(dpi).value();
        let status_text = Rect::new(
            status_rect.left + margin,
            status_rect.top,
            status_rect.right,
            status_rect.bottom,
        );
        ui.apply_moves(&[
            (self.menu_id, menu_rect),
            (self.toolbar_id, toolbar_rect),
            (self.status.id(), status_text),
        ]);

        // The outer split's first pane is the toolbox, so its stored size maps
        // straight through. The others store their second pane's size, so the
        // first pane's extent follows from the laid-out node.
        self.outer.set_position(dip(self.settings.panes.toolbox));
        self.outer.set_bounds(main_rect);
        self.rest.set_position(first_pane(
            self.rest.id(),
            self.settings.panes.output,
            true,
            ui,
            dpi,
        ));
        self.centre.set_position(first_pane(
            self.centre.id(),
            self.settings.panes.right,
            false,
            ui,
            dpi,
        ));
        self.right.set_position(first_pane(
            self.right.id(),
            self.settings.panes.project,
            true,
            ui,
            dpi,
        ));

        ui.relayout();

        // Each pane's title strip spans its pane, less a margin either side, so
        // it reads as a header rather than a box cut off partway.
        let panes = [
            self.toolbox_panel.id(),
            self.project_panel.id(),
            self.properties_panel.id(),
        ];
        let titles: Vec<(WidgetId, Rect)> = self
            .pane_labels
            .iter()
            .zip(panes)
            .map(|(label, pane)| {
                let width = ui.bounds(pane).width();
                let title = Rect::new(margin, 4, (width - margin).max(margin), 24);
                (label.id(), title)
            })
            .collect();
        ui.apply_moves(&titles);

        // The Project Explorer fills its panel below the title label.
        let panel = ui.bounds(self.project_panel.id());
        ui.apply_moves(&[(
            self.tree.id(),
            Rect::new(0, 28, panel.width().max(0), panel.height().max(0)),
        )]);

        // The Toolbox tiles fill their pane below the title; the grid fills the
        // Properties pane below its title.
        let toolbox = ui.bounds(self.toolbox_panel.id());
        ui.apply_moves(&[(
            self.toolbox.id(),
            Rect::new(
                0,
                PANE_TITLE,
                toolbox.width().max(0),
                toolbox.height().max(0),
            ),
        )]);
        let properties = ui.bounds(self.properties_panel.id());
        if let Some(grid) = &self.properties_grid {
            grid.set_bounds(Rect::new(
                0,
                PANE_TITLE,
                properties.width().max(0),
                properties.height().max(0),
            ));
        }

        // The Output pane: the log line on top, the Error List below it.
        let output = ui.bounds(self.output_panel.id());
        let header = Dip(24.0).to_px(dpi).value();
        ui.apply_moves(&[
            (
                self.output.id(),
                Rect::new(0, 0, output.width().max(0), header),
            ),
            (
                self.error_list.id(),
                Rect::new(0, header, output.width().max(0), output.height().max(0)),
            ),
        ]);

        if let Some(docs) = &self.docs {
            docs.relayout();
        }
        self.layout_code_views(ui, dpi);
    }

    /// Positions the procedure combos and the editor of the selected code
    /// document within its tab page.
    ///
    /// The tab lays every page child over the whole page, so the code view owns
    /// the finer placement: a combo row at the top for a form, then the editor
    /// filling the rest.
    fn layout_code_views(&self, ui: &Ui<Msg>, dpi: u32) {
        let Some(docs) = &self.docs else {
            return;
        };
        let selected = docs.selected();
        // A rebuilt procedure combo is not in the tab's own page list, so the
        // view manages the visibility of every code child itself.
        for (index, other) in self.documents.iter().enumerate() {
            if let DocumentView::Code(other) = &other.view {
                let visible = index + 1 == selected;
                for id in other.children() {
                    ui.set_visible(id, visible);
                }
            }
        }
        let Some(document) = selected
            .checked_sub(1)
            .and_then(|index| self.documents.get(index))
        else {
            return;
        };
        let DocumentView::Code(view) = &document.view else {
            return;
        };
        let node = ui.bounds(docs.id());
        if node.is_empty() {
            return;
        }
        // The page area of the tab container, in the container's coordinates.
        let bounds = Rect::from_size(node.size());
        let page = Dock::new().top(TABS_STRIP).split(bounds, dpi).fill;
        let header = CODE_HEADER.to_px(dpi).value();
        let gap = (dpi / 8).max(1) as i32;

        let mut moves = Vec::new();
        let mut editor_top = page.top;
        if let (Some(object), Some(procedure)) = (&view.object, &view.procedure) {
            let half = ((page.width() - gap * 3) / 2).max(0);
            moves.push((
                object.id(),
                Rect::new(
                    page.left + gap,
                    page.top,
                    page.left + gap + half,
                    page.top + header,
                ),
            ));
            moves.push((
                procedure.id(),
                Rect::new(
                    page.left + gap * 2 + half,
                    page.top,
                    page.right - gap,
                    page.top + header,
                ),
            ));
            editor_top = page.top + header;
        }
        moves.push((
            view.editor.id(),
            Rect::new(page.left, editor_top, page.right, page.bottom),
        ));
        ui.apply_moves(&moves);
    }

    /// Pushes `settings.theme` into the window's palette.
    fn apply_theme(&self, ui: &Ui<Msg>) {
        ui.set_theme(crate::theme::palette(self.settings.theme));
    }

    /// Applies the dispatcher's enabled state to every mapped menu entry.
    fn refresh_menu(&self) {
        for (id, command) in self.menu_commands.iter() {
            self.menu
                .set_enabled(*id, self.dispatcher.is_enabled(*command));
        }
    }

    /// Rebuilds the menu bar, replacing its widget, so the Recent submenu
    /// reflects the current list. xui menus cannot be rebuilt in place
    /// (PLAN.md §10, gap G11).
    fn rebuild_menu(&mut self, ui: &Ui<Msg>) {
        let bounds = ui.bounds(self.menu_id);
        match build_menu(ui, bounds, &self.settings.recent_projects) {
            Ok((menu, commands)) => {
                self.menu_commands = commands;
                self.menu_id = menu.id().unwrap_or(WidgetId::NONE);
                self.menu = menu;
                self.refresh_menu();
                ui.apply_moves(&[(self.menu_id, bounds)]);
            }
            Err(error) => self.log(ui, format!("the menu could not be rebuilt: {error}")),
        }
    }

    /// The code editors' options: the configured monospace family and size.
    /// The editor is a monospace grid, so without a monospace family it would
    /// fall back to the proportional UI font and space its tokens apart.
    fn editor_options(&self) -> EditorOptions {
        EditorOptions {
            font: FontConfig {
                family: Some(self.settings.editor_font_family.clone()),
                size: Dip(self.settings.editor_font_size),
            },
            ..EditorOptions::default()
        }
    }

    /// Updates the window title, which carries the project name, a `*` when
    /// anything is unsaved and `[run]` while a program is running.
    fn update_title(&self, ui: &Ui<Msg>) {
        let mut title = match &self.session {
            Some(session) => format!("LazyRAD - {}", session.name()),
            None => "LazyRAD".to_owned(),
        };
        if self.project_dirty() {
            title.push('*');
        }
        if self.run.is_running() {
            title.push_str(" [run]");
        }
        ui.set_window_title(&title);
    }

    /// Updates the status bar's Design/Run text.
    fn update_status(&self, ui: &Ui<Msg>) {
        let state = if self.run.is_running() {
            "Run"
        } else {
            "Design"
        };
        self.status.set_text(state);
        ui.invalidate(self.status.id());
    }

    /// Whether a program is running; design-time editing is refused while it is
    /// (issue #16, and the designer's hook once it lands).
    pub fn is_running(&self) -> bool {
        self.run.is_running()
    }

    /// Whether the project or any document has unsaved changes.
    fn project_dirty(&self) -> bool {
        self.session.as_ref().is_some_and(ProjectSession::is_dirty)
            || self.documents.iter().any(|document| document.dirty)
    }

    /// Appends a line to the Output pane.
    fn log(&mut self, ui: &Ui<Msg>, line: impl Into<String>) {
        let line = line.into();
        eprintln!("{line}");
        self.output_lines.push(line);
        if self.output_lines.len() > OUTPUT_LINES {
            let excess = self.output_lines.len() - OUTPUT_LINES;
            self.output_lines.drain(0..excess);
        }
        // The portable `Label` draws a single styled line, so the pane shows
        // the most recent entries on one line; the full buffer is kept for a
        // multi-line output widget to replace it.
        let tail = self
            .output_lines
            .iter()
            .rev()
            .take(5)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("   |   ");
        self.output.set_text(&tail);
        ui.invalidate(self.output.id());
    }

    /// Runs a command.
    fn run_command(&mut self, command: Command, ui: &mut Ui<Msg>) {
        // Design-time editing is refused while a program runs, as VB did
        // (issue #16). Run → End and the view commands stay available.
        if self.run.is_running() && command.is_editing() {
            self.log(
                ui,
                format!(
                    "{} is not available while the program is running.",
                    command.label()
                ),
            );
            return;
        }
        match command {
            Command::Exit => self.request_exit(ui),
            Command::CloseProject => self.request_close(ui),
            // Each of these replaces the open project, so unsaved work goes
            // through the save prompt first, as Exit and Close do.
            Command::NewProject => {
                if self.project_dirty() {
                    self.show_save_prompt(ui, Pending::NewProject);
                } else {
                    self.show_prompt(ui, PromptKind::NewProject);
                }
            }
            Command::OpenProject => {
                if let Some(file) = dialogs::open_project_file() {
                    self.open_replacing(dialogs::containing_folder(&file), ui);
                }
            }
            Command::OpenRecent(index) => {
                if let Some(dir) = self.settings.recent_projects.get(index).cloned() {
                    self.open_replacing(dir, ui);
                }
            }
            Command::Save | Command::SaveAll => self.save_project(ui),
            Command::SaveAs => self.save_project_as(ui),
            Command::AddForm => {
                if let Some(name) = self.session.as_mut().map(ProjectSession::add_form) {
                    self.after_structure_change(ui);
                    let _ = self.open_document(&name, DocKind::Designer);
                    self.log(ui, format!("Added form {name}."));
                }
            }
            Command::AddModule => {
                if let Some(name) = self.session.as_mut().map(ProjectSession::add_module) {
                    self.after_structure_change(ui);
                    let _ = self.open_document(&name, DocKind::Code);
                    self.log(ui, format!("Added module {name}."));
                }
            }
            Command::Remove => {
                if let Some(name) = self.selected_item_name() {
                    self.remove_item(&name, ui);
                } else {
                    self.log(ui, "Select an item in the Project Explorer first.");
                }
            }
            Command::ViewCode => {
                if let Some(name) = self.current_item_name() {
                    self.open_code(&name, ui);
                } else {
                    self.log(ui, "Select an item in the Project Explorer first.");
                }
            }
            Command::ViewObject => {
                if let Some(name) = self.current_item_name() {
                    self.open_object(&name, ui);
                } else {
                    self.log(ui, "Select an item in the Project Explorer first.");
                }
            }
            Command::Find => self.show_prompt(ui, PromptKind::Find),
            Command::FindNext => self.find_next(ui),
            Command::Replace => self.show_prompt(ui, PromptKind::ReplaceFind),
            Command::GoToLine => self.show_prompt(ui, PromptKind::GoToLine),
            Command::Undo
            | Command::Redo
            | Command::Cut
            | Command::Copy
            | Command::Paste
            | Command::Delete
            | Command::SelectAll => self.dispatch_edit(command, ui),
            Command::RunStart => self.start_run(ui),
            Command::RunEnd => self.end_run(ui),
            Command::ThemeLight => self.set_theme(ui, ThemeChoice::Light),
            Command::ThemeDark => self.set_theme(ui, ThemeChoice::Dark),
            Command::ThemeSystem => self.set_theme(ui, ThemeChoice::System),
            _ => {
                if let Some(line) = self.dispatcher.dispatch(command) {
                    self.log(ui, line);
                }
            }
        }
    }

    /// Switches theme, applies it and persists the choice.
    fn set_theme(&mut self, ui: &mut Ui<Msg>, choice: ThemeChoice) {
        self.settings.theme = choice;
        self.apply_theme(ui);
        match self.settings.save() {
            Ok(()) => self.log(ui, format!("Theme: {}", choice.label())),
            Err(error) => self.log(
                ui,
                format!(
                    "Theme: {} (applied, but the setting could not be saved: {error})",
                    choice.label()
                ),
            ),
        }
    }

    /// Records a moved divider in the settings. They are written to disk on
    /// exit (see [`Command::Exit`]), not on every drag step.
    fn on_pane_moved(&mut self, slot: PaneSlot, position: f32, ui: &Ui<Msg>) {
        let dpi = ui.dpi();
        match slot {
            PaneSlot::Toolbox => self.settings.panes.toolbox = position.max(0.0),
            PaneSlot::Project => self.settings.panes.project = position.max(0.0),
            PaneSlot::Output => {
                self.settings.panes.output = second_extent(self.rest.id(), position, true, ui, dpi)
            }
            PaneSlot::Right => {
                self.settings.panes.right =
                    second_extent(self.centre.id(), position, false, ui, dpi)
            }
        }
        // Re-flow so the pane contents (tree, toolbox, grid, output) follow the
        // panels the split just resized.
        self.layout_frame(ui);
    }

    // ---- Running the program (issue #16) ----------------------------------

    /// Starts the open project: saves dirty documents, compile-checks it and
    /// launches the player on the project directory.
    ///
    /// A failure at any step cancels the run: a save that fails leaves the
    /// previous state intact, a compile check that finds problems fills the
    /// Error List, and a failed spawn logs and stays in Design.
    fn start_run(&mut self, ui: &mut Ui<Msg>) {
        if self.run.is_running() {
            self.log(ui, "A program is already running.");
            return;
        }
        let Some(dir) = self
            .session
            .as_ref()
            .map(|session| session.dir().to_path_buf())
        else {
            self.log(ui, "Open a project before running it.");
            return;
        };
        let name = self
            .session
            .as_ref()
            .map(|session| session.name().to_owned())
            .unwrap_or_default();

        // 1. Save every dirty document through the existing save path, so the
        //    player reads exactly what the user sees.
        if let Err(error) = self.save_for_run(ui) {
            self.log(
                ui,
                format!("Run cancelled: the project could not be saved ({error})."),
            );
            return;
        }

        // 2. Compile-check; problems go to the Error List and nothing spawns.
        if !self.check_for_run(&dir, ui) {
            return;
        }

        // 3. Locate the player and spawn it.
        let player = match run::resolve_player(self.settings.player_path.as_deref()) {
            Ok(player) => player,
            Err(error) => {
                self.log(ui, error.to_string());
                return;
            }
        };
        let proxy = ui.proxy();
        let sink: run::EventSink = Arc::new(move |run, event| {
            let _ = proxy.send(Msg::Run(run, event));
        });
        match self.run.start(self.launcher.as_ref(), &player, &dir, sink) {
            Ok(_) => {
                self.dispatcher.set_running(true);
                self.refresh_menu();
                self.update_title(ui);
                self.update_status(ui);
                self.log(ui, format!("Running {name}..."));
            }
            Err(error) => self.log(ui, format!("The program could not start: {error}")),
        }
    }

    /// Saves the project and every open document for a run, marking them clean.
    fn save_for_run(&mut self, ui: &mut Ui<Msg>) -> Result<(), String> {
        // The same path as Save: open designers' layouts go in with the code.
        self.sync_designers();
        self.sync_documents();
        let Some(result) = self.session.as_mut().map(ProjectSession::save) else {
            return Ok(());
        };
        match result {
            Ok(_) => {
                self.mark_documents_saved();
                self.update_title(ui);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    /// Compile-checks the saved project, filling the Error List with any
    /// problems. Returns whether the project may start.
    fn check_for_run(&mut self, dir: &Path, ui: &mut Ui<Msg>) -> bool {
        let problems = match run::check(dir) {
            Ok(problems) => problems,
            Err(error) => {
                self.log(ui, format!("Compile check failed: {error}"));
                return false;
            }
        };
        if problems.is_empty() {
            // A clean check clears diagnostics an earlier compile left behind.
            self.errors.clear();
            self.refresh_error_list();
            return true;
        }
        let entries: Vec<ErrorEntry> = problems
            .iter()
            .map(|problem| ErrorEntry {
                name: self.item_for_diagnostic_file(&problem.file),
                diagnostic: CodeDiagnostic::new(problem.line, problem.col, problem.message.clone()),
            })
            .collect();
        self.errors = entries;
        self.refresh_error_list();
        for problem in &problems {
            self.log(
                ui,
                format!(
                    "{}:{}:{}: {}",
                    problem.file, problem.line, problem.col, problem.message
                ),
            );
        }
        self.log(
            ui,
            format!("{} error(s): the program was not started.", problems.len()),
        );
        false
    }

    /// Ends the running program (Run → End).
    fn end_run(&mut self, ui: &mut Ui<Msg>) {
        if !self.run.is_running() {
            self.log(ui, "No program is running.");
            return;
        }
        self.run.end();
        self.after_run_stopped(ui);
        self.log(ui, "Program ended.");
    }

    /// Ends the running program because the project is changing or the IDE is
    /// exiting, so a child never outlives the project it was started for.
    fn stop_run(&mut self, ui: &mut Ui<Msg>) {
        if self.run.is_running() {
            self.run.end();
            self.after_run_stopped(ui);
            self.log(ui, "The running program was ended.");
        }
    }

    /// Restores the Design state after a run stops.
    fn after_run_stopped(&mut self, ui: &Ui<Msg>) {
        self.dispatcher.set_running(false);
        self.refresh_menu();
        self.update_title(ui);
        self.update_status(ui);
    }

    /// Handles one event from the active child. A stale run id is dropped, so a
    /// late message cannot act on the run that replaced it (checklist 1).
    fn on_run_event(&mut self, run: RunId, event: RunEvent, ui: &mut Ui<Msg>) {
        if !self.run.accepts(run) {
            return;
        }
        match event {
            RunEvent::Output(line) => self.log(ui, line),
            RunEvent::Diagnostic(report) => self.on_run_diagnostic(report, ui),
            RunEvent::Exited(code) => {
                self.run.finished(run);
                self.after_run_stopped(ui);
                match code {
                    Some(0) | None => self.log(ui, "The program ended."),
                    Some(code) => self.log(ui, format!("The program ended with exit code {code}.")),
                }
            }
        }
    }

    /// Adds a diagnostic the player reported to the Error List and, for a
    /// runtime error, opens the failing file and places the caret on its line.
    fn on_run_diagnostic(&mut self, report: lazyrad_player::Report, ui: &mut Ui<Msg>) {
        let name = self.item_for_diagnostic_file(&report.file);
        let diagnostic = CodeDiagnostic::new(report.line, report.col, report.message.clone());
        self.errors.push(ErrorEntry {
            name: name.clone(),
            diagnostic,
        });
        self.refresh_error_list();
        if report.file.is_empty() {
            self.log(ui, report.message.clone());
        } else {
            self.log(
                ui,
                format!("{}:{}: {}", report.file, report.line, report.message),
            );
        }
        // Jump only when the file is a project item: an error with no location
        // (Rhai gives none for `1 / 0`) is listed but opens nothing.
        let known = self
            .session
            .as_ref()
            .is_some_and(|session| session.code(&name).is_some());
        if report.kind == lazyrad_player::Kind::Runtime && known {
            self.open_code(&name, ui);
            if let Some(editor) = self.code_editor(&name) {
                editor.goto(report.line.saturating_sub(1), report.col.saturating_sub(1));
                editor.focus();
            }
        }
    }

    /// The project item a diagnostic's file belongs to, so activating the row
    /// can open its code tab. Falls back to the raw path for an unknown file.
    fn item_for_diagnostic_file(&self, file: &str) -> String {
        let stem = Path::new(file)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.to_owned());
        let known = self
            .session
            .as_ref()
            .is_some_and(|session| session.code(&stem).is_some() || session.form(&stem).is_some());
        if known { stem } else { file.to_owned() }
    }

    // ---- Project lifecycle -------------------------------------------------

    /// Prompts for a folder and creates a new "Standard EXE" project.
    fn new_project(&mut self, name: &str, ui: &mut Ui<Msg>) {
        let Some(dir) = dialogs::choose_folder("Choose a folder for the project") else {
            return;
        };
        match ProjectSession::create(name, &dir) {
            Ok(session) => self.adopt_project(session, ui),
            Err(error) => self.log(ui, format!("The project could not be created: {error}")),
        }
    }

    /// Opens the project in `dir`, as a known path rather than through a file
    /// dialog. Replaces the current project, saving unsaved changes first.
    pub fn open_project(&mut self, dir: &Path, ui: &mut Ui<Msg>) {
        self.open_replacing(dir.to_path_buf(), ui);
    }

    /// Opens the project in `dir` in place of the current one, asking to save
    /// unsaved changes first.
    fn open_replacing(&mut self, dir: PathBuf, ui: &mut Ui<Msg>) {
        if self.project_dirty() {
            self.show_save_prompt(ui, Pending::Open(dir));
        } else {
            self.open_dir(dir, ui);
        }
    }

    /// Carries out the action the save prompt was guarding.
    fn continue_pending(&mut self, pending: Pending, ui: &mut Ui<Msg>) {
        match pending {
            Pending::Exit => self.finish_exit(ui),
            Pending::Close => self.close_project(ui),
            Pending::NewProject => self.show_prompt(ui, PromptKind::NewProject),
            Pending::Open(dir) => self.open_dir(dir, ui),
        }
    }

    /// Opens the project in `dir`.
    fn open_dir(&mut self, dir: PathBuf, ui: &mut Ui<Msg>) {
        match ProjectSession::open(&dir) {
            Ok(session) => self.adopt_project(session, ui),
            Err(error) => self.log(ui, format!("Could not open {}: {error}", dir.display())),
        }
    }

    /// Makes `session` the open project: resets the document tabs, refreshes
    /// the explorer and menus, records it as recent and opens its startup item.
    fn adopt_project(&mut self, session: ProjectSession, ui: &mut Ui<Msg>) {
        // A running program belongs to the old project, so it is ended first.
        self.stop_run(ui);
        let name = session.name().to_owned();
        let dir = session.dir().to_path_buf();
        let startup = session.startup().to_owned();
        self.session = Some(session);
        // A prompt or context target names an item of the old project; left
        // open, it would act on a same-named item of the new one.
        self.prompt = None;
        self.context_target = None;

        if let Err(error) = self.reset_documents() {
            self.log(ui, format!("the document area could not be reset: {error}"));
        }
        self.refresh_explorer(ui);
        self.dispatcher.set_project_open(true);
        self.settings.push_recent(dir);
        let _ = self.settings.save();
        self.rebuild_menu(ui);
        self.update_title(ui);

        let diagnostics = self
            .session
            .as_ref()
            .map(ProjectSession::diagnostics)
            .unwrap_or_default();
        if diagnostics.is_empty() {
            self.log(ui, format!("Opened project {name}."));
        } else {
            self.log(ui, format!("Opened project {name} with problems:"));
            for diagnostic in diagnostics {
                self.log(ui, diagnostic.to_string());
            }
        }

        // Open the startup item, as VB6 does.
        if !startup.is_empty() {
            let is_form = self
                .session
                .as_ref()
                .and_then(|session| session.form(&startup))
                .is_some();
            let kind = if is_form {
                DocKind::Designer
            } else {
                DocKind::Code
            };
            if let Err(error) = self.open_document(&startup, kind) {
                self.log(ui, format!("the startup item could not be opened: {error}"));
            }
        }
    }

    /// Saves the project and every open code document.
    fn save_project(&mut self, ui: &mut Ui<Msg>) {
        self.sync_designers();
        self.sync_documents();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match session.save() {
            Ok(report) => {
                let name = session.name().to_owned();
                self.mark_documents_saved();
                if report.is_empty() {
                    self.log(ui, format!("Saved {name} (nothing changed)."));
                } else {
                    self.log(ui, format!("Saved {name}: {} file(s).", report.len()));
                }
            }
            Err(error) => self.log(ui, format!("Save failed: {error}")),
        }
        self.update_title(ui);
    }

    /// Saves the project to a chosen `.lrp` location.
    fn save_project_as(&mut self, ui: &mut Ui<Msg>) {
        self.sync_designers();
        self.sync_documents();
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let file_name = session.project().file_name();
        let Some(file) = dialogs::save_project_file(&file_name) else {
            return;
        };
        let result = self
            .session
            .as_mut()
            .expect("the project was just borrowed")
            .save_as(&file);
        match result {
            Ok(_) => {
                self.mark_documents_saved();
                self.settings.push_recent(dialogs::containing_folder(&file));
                let _ = self.settings.save();
                self.rebuild_menu(ui);
                self.log(ui, format!("Saved as {}.", file.display()));
            }
            Err(error) => self.log(ui, format!("Save As failed: {error}")),
        }
        self.update_title(ui);
    }

    /// Clears every document's dirty flag and rewrites its tab title.
    fn mark_documents_saved(&mut self) {
        for document in &mut self.documents {
            document.dirty = false;
        }
        self.refresh_titles();
    }

    /// Asks to save, discard or cancel when the project is dirty, then exits.
    fn request_exit(&mut self, ui: &mut Ui<Msg>) {
        if self.project_dirty() {
            self.show_save_prompt(ui, Pending::Exit);
        } else {
            self.finish_exit(ui);
        }
    }

    /// Asks to save, discard or cancel when the project is dirty, then closes
    /// the project.
    fn request_close(&mut self, ui: &mut Ui<Msg>) {
        if self.project_dirty() {
            self.show_save_prompt(ui, Pending::Close);
        } else {
            self.close_project(ui);
        }
    }

    /// Saves the settings and quits, with the second-chance behaviour for a
    /// read-only config directory.
    fn finish_exit(&mut self, ui: &mut Ui<Msg>) {
        self.stop_run(ui);
        match self.settings.save() {
            Ok(()) => ui.quit(),
            Err(_) if self.exit_save_failed => ui.quit(),
            Err(error) => {
                self.exit_save_failed = true;
                self.log(
                    ui,
                    format!(
                        "Settings could not be saved ({error}). Exit again to quit without saving them."
                    ),
                );
            }
        }
    }

    /// Drops the project, closes its documents and disables project commands.
    fn close_project(&mut self, ui: &mut Ui<Msg>) {
        self.stop_run(ui);
        self.session = None;
        self.prompt = None;
        self.context_target = None;
        if let Err(error) = self.reset_documents() {
            self.log(ui, format!("the document area could not be reset: {error}"));
        }
        self.refresh_explorer(ui);
        self.dispatcher.set_project_open(false);
        self.refresh_menu();
        self.update_title(ui);
        self.log(ui, "Project closed.");
    }

    // ---- Save prompt and generic prompts ----------------------------------

    /// Opens the three-way save prompt for `pending`.
    fn show_save_prompt(&mut self, ui: &mut Ui<Msg>, pending: Pending) {
        let name = self.session.as_ref().map_or_else(
            || "the project".to_owned(),
            |session| session.name().to_owned(),
        );
        let message = format!("Save changes to {name} before continuing?");
        match ChoiceDialog::new(
            ui,
            "Save changes?",
            &message,
            &["Save", "Discard", "Cancel"],
            0,
            2,
        ) {
            Ok(dialog) => {
                let dialog = dialog.on_action(|index| {
                    Some(Msg::SaveChoice(match index {
                        0 => SaveChoice::Save,
                        1 => SaveChoice::Discard,
                        _ => SaveChoice::Cancel,
                    }))
                });
                dialog.open();
                self.save_prompt = Some(dialog);
                self.pending = Some(pending);
            }
            Err(error) => self.log(ui, format!("the save prompt could not open: {error}")),
        }
    }

    /// Applies the answer to the save prompt.
    fn resolve_save_prompt(&mut self, choice: SaveChoice, ui: &mut Ui<Msg>) {
        // The dialog has already hidden itself; dropping it frees its nodes.
        self.save_prompt = None;
        let Some(pending) = self.pending.take() else {
            return;
        };
        match choice {
            SaveChoice::Cancel => {}
            SaveChoice::Discard => self.continue_pending(pending, ui),
            SaveChoice::Save => {
                self.sync_designers();
                self.sync_documents();
                match self.session.as_mut().map(ProjectSession::save) {
                    Some(Ok(_)) => {
                        self.mark_documents_saved();
                        self.update_title(ui);
                        self.continue_pending(pending, ui);
                    }
                    Some(Err(error)) => self.log(
                        ui,
                        format!("The project could not be saved ({error}); it is still open."),
                    ),
                    // Nothing to save: carry on with what the prompt guarded.
                    None => self.continue_pending(pending, ui),
                }
            }
        }
    }

    /// Opens a prompt dialog for `kind`.
    fn show_prompt(&mut self, ui: &mut Ui<Msg>, kind: PromptKind) {
        let (title, message, initial): (String, String, String) = match &kind {
            PromptKind::NewProject => (
                "New Project".to_owned(),
                "Project name:".to_owned(),
                DEFAULT_PROJECT.to_owned(),
            ),
            PromptKind::Rename(old) => (
                "Rename Item".to_owned(),
                "New name:".to_owned(),
                old.clone(),
            ),
            PromptKind::Find | PromptKind::ReplaceFind => (
                if matches!(kind, PromptKind::Find) {
                    "Find".to_owned()
                } else {
                    "Replace".to_owned()
                },
                "Find (wrap it in /…/ to use a regular expression):".to_owned(),
                self.last_find.clone(),
            ),
            PromptKind::ReplaceWith => (
                "Replace".to_owned(),
                "Replace with:".to_owned(),
                self.last_replace.clone(),
            ),
            PromptKind::GoToLine => (
                "Go to Line".to_owned(),
                "Line number:".to_owned(),
                String::new(),
            ),
        };
        match Dialog::prompt(ui, &title, &message, &initial) {
            Ok(dialog) => {
                let dialog = dialog.on_action(|action| match action {
                    DialogAction::Accept(text) => Some(Msg::PromptSubmitted(text)),
                    DialogAction::Cancel => Some(Msg::PromptCancelled),
                });
                dialog.open();
                self.prompt = Some((kind, dialog));
            }
            Err(error) => self.log(ui, format!("the prompt could not open: {error}")),
        }
    }

    /// Applies an accepted prompt.
    fn resolve_prompt(&mut self, text: String, ui: &mut Ui<Msg>) {
        let Some((kind, dialog)) = self.prompt.take() else {
            return;
        };
        drop(dialog);
        match kind {
            PromptKind::NewProject => {
                let name = text.trim().to_owned();
                if name.is_empty() {
                    self.log(ui, "No name given; nothing was changed.");
                } else {
                    self.new_project(&name, ui);
                }
            }
            PromptKind::Rename(old) => {
                let name = text.trim().to_owned();
                if name.is_empty() {
                    self.log(ui, "No name given; nothing was changed.");
                } else {
                    self.rename_item(&old, &name, ui);
                }
            }
            PromptKind::Find => self.start_find(&text, ui),
            PromptKind::ReplaceFind => {
                let query = Query::parse(&text);
                if query.pattern.is_empty() {
                    self.log(ui, "Nothing to find.");
                    return;
                }
                self.last_find = text;
                self.replace_query = Some(query);
                self.show_prompt(ui, PromptKind::ReplaceWith);
            }
            PromptKind::ReplaceWith => self.replace_all(&text, ui),
            PromptKind::GoToLine => self.go_to_line(&text, ui),
        }
    }

    /// Starts a find from the accepted query.
    fn start_find(&mut self, text: &str, ui: &mut Ui<Msg>) {
        let query = Query::parse(text);
        if query.pattern.is_empty() {
            self.log(ui, "Nothing to find.");
            return;
        }
        self.last_find = text.to_owned();
        self.search(&query, true, ui);
    }

    /// Repeats the last find in the forward direction (F3).
    fn find_next(&mut self, ui: &mut Ui<Msg>) {
        let text = self.last_find.clone();
        let query = Query::parse(&text);
        if query.pattern.is_empty() {
            self.log(ui, "Use Find first, or F3 with a previous search.");
            return;
        }
        self.search(&query, true, ui);
    }

    /// Selects the next (or previous) match in the active code document.
    fn search(&mut self, query: &Query, forward: bool, ui: &mut Ui<Msg>) {
        let Some((_, editor)) = self.active_code_editor() else {
            self.log(ui, "Open a code window first.");
            return;
        };
        match editor.find_next(query, self.find_case_sensitive(), forward) {
            Ok(true) => {
                editor.focus();
            }
            Ok(false) => self.log(ui, "No matches."),
            Err(error) => self.log(ui, format!("Invalid regular expression: {error}")),
        }
    }

    /// Replaces every match of the pending query in the active code document.
    fn replace_all(&mut self, replacement: &str, ui: &mut Ui<Msg>) {
        let Some(query) = self.replace_query.take() else {
            return;
        };
        self.last_replace = replacement.to_owned();
        let Some((name, editor)) = self.active_code_editor() else {
            self.log(ui, "Open a code window first.");
            return;
        };
        match xui_code_editor::find::replace_all(
            &editor.text(),
            &query,
            self.find_case_sensitive(),
            replacement,
        ) {
            Ok(Some(text)) => {
                // One replace over the whole text is a single undoable edit;
                // `set_text` would start a new buffer and lose the history.
                let len = editor.text().chars().count();
                editor.replace(0, len, &text);
                self.after_programmatic_edit(&name, ui);
                self.log(ui, "Replaced all matches.");
            }
            Ok(None) => self.log(ui, "No matches."),
            Err(error) => self.log(ui, format!("Invalid regular expression: {error}")),
        }
    }

    /// Jumps the active code document to the accepted line number.
    fn go_to_line(&mut self, text: &str, ui: &mut Ui<Msg>) {
        let Ok(line) = text.trim().parse::<usize>() else {
            self.log(ui, "Enter a line number.");
            return;
        };
        if line == 0 {
            self.log(ui, "Line numbers start at 1.");
            return;
        }
        let Some((_, editor)) = self.active_code_editor() else {
            self.log(ui, "Open a code window first.");
            return;
        };
        editor.goto(line - 1, 0);
        editor.focus();
    }

    /// Whether find and replace match case. VB's Match Case is off by default.
    fn find_case_sensitive(&self) -> bool {
        false
    }

    /// The active code document's name and editor, if a code tab is in front.
    fn active_code_editor(&self) -> Option<(String, Rc<Editor<Msg>>)> {
        let docs = self.docs.as_ref()?;
        let selected = docs.selected();
        if selected == 0 {
            return None;
        }
        let document = self.documents.get(selected - 1)?;
        if document.kind != DocKind::Code {
            return None;
        }
        match &document.view {
            DocumentView::Code(view) => Some((document.name.clone(), Rc::clone(&view.editor))),
            DocumentView::Designer { .. } => None,
        }
    }

    /// The active designer's form name, designer and the scoped handle it was
    /// built with, if a form tab is in front.
    fn active_designer(&self) -> Option<ActiveDesigner> {
        let docs = self.docs.as_ref()?;
        let selected = docs.selected();
        if selected == 0 {
            return None;
        }
        let document = self.documents.get(selected - 1)?;
        if document.kind != DocKind::Designer {
            return None;
        }
        match &document.view {
            DocumentView::Designer {
                designer,
                designer_ui,
                ..
            } => Some((
                document.name.clone(),
                Rc::clone(designer),
                designer_ui.clone(),
            )),
            DocumentView::Code(_) => None,
        }
    }

    /// Binds the property grid to the active designer, rebuilding it when the
    /// active form tab changed.
    ///
    /// Rebuilding drops every sink and inline editor the old grid registered on
    /// the old designer, so nothing it registered can keep firing after the tab
    /// is closed or hidden. A queued grid message is tagged with its form (see
    /// [`Msg::PropertyGrid`]) and dropped here once the binding no longer
    /// matches.
    fn refresh_property_grid(&mut self, ui: &Ui<Msg>) {
        let active = self.active_designer();
        let active_name = active.as_ref().map(|(name, _, _)| name.clone());
        if active_name == self.grid_form {
            if let Some(grid) = &self.properties_grid {
                grid.sync(ui);
            }
            return;
        }
        // The grid registered a selection sink on the designer it was bound to.
        // Drop it while that designer may still be open, so a sink does not
        // accumulate every time the active tab changes.
        self.clear_grid_sink();
        self.properties_grid = None;
        self.grid_form = active_name;
        let Some((name, designer, _)) = active else {
            return;
        };
        let panel = self.properties_panel.ui().clone();
        let wrap_name = name.clone();
        match PropertyGrid::new(
            &panel,
            Rect::default(),
            designer,
            Rc::clone(&self.catalog),
            move |msg| Msg::PropertyGrid {
                form: wrap_name.clone(),
                msg,
            },
        ) {
            Ok(grid) => {
                let bounds = ui.bounds(self.properties_panel.id());
                grid.set_bounds(Rect::new(
                    0,
                    PANE_TITLE,
                    bounds.width().max(0),
                    bounds.height().max(0),
                ));
                self.properties_grid = Some(grid);
            }
            Err(error) => self.log(ui, format!("the property grid could not be built: {error}")),
        }
    }

    /// Pushes a designer's document into the session, marking the form and its
    /// tab dirty when it actually changed.
    fn sync_designer(
        &mut self,
        name: &str,
        designer: &Rc<RefCell<Designer<Msg>>>,
        ui: &mut Ui<Msg>,
    ) {
        let changed = self
            .session
            .as_mut()
            .is_some_and(|session| session.set_form(name, designer.borrow().doc()));
        if !changed {
            return;
        }
        // The grid caches its rows; a canvas drag or an undo changes geometry
        // without a selection change, so refresh it from the designer.
        if self.grid_form.as_deref() == Some(name)
            && let Some(grid) = &self.properties_grid
        {
            grid.sync(self.properties_panel.ui());
        }
        if self.mark_document_dirty(name, DocKind::Designer) {
            self.refresh_titles();
        }
        self.update_title(ui);
    }

    /// Marks a document dirty, returning whether its dirty flag actually
    /// changed (so the tab title only has to be rewritten on the transition).
    fn mark_document_dirty(&mut self, name: &str, kind: DocKind) -> bool {
        if let Some(document) = self
            .documents
            .iter_mut()
            .find(|document| document.name == name && document.kind == kind)
            && !document.dirty
        {
            document.dirty = true;
            return true;
        }
        false
    }

    /// Removes the property grid's selection sink from the designer it was
    /// bound to, if that designer is still open.
    fn clear_grid_sink(&self) {
        let Some(name) = self.grid_form.as_deref() else {
            return;
        };
        let designer = self
            .documents
            .iter()
            .find(|document| document.name == name && document.kind == DocKind::Designer)
            .and_then(|document| match &document.view {
                DocumentView::Designer { designer, .. } => Some(Rc::clone(designer)),
                DocumentView::Code(_) => None,
            });
        if let Some(designer) = designer {
            designer.borrow().clear_selection_sink();
        }
    }

    /// Routes a toolbox message to the active designer, or to nothing when no
    /// form tab is in front.
    fn toolbox_message(&mut self, msg: ToolboxMsg, ui: &mut Ui<Msg>) {
        let Some((name, designer, designer_ui)) = self.active_designer() else {
            return;
        };
        designer.borrow().handle_toolbox(msg, &designer_ui);
        self.sync_designer(&name, &designer, ui);
    }

    /// Routes a property-grid message to the grid, dropping one whose form is
    /// no longer the bound form.
    fn property_grid_message(&mut self, form: &str, msg: PropertyGridMsg, ui: &mut Ui<Msg>) {
        if self.grid_form.as_deref() != Some(form) {
            return;
        }
        let Some((name, designer, _)) = self.active_designer() else {
            return;
        };
        if let Some(grid) = &self.properties_grid {
            // The grid edits from its own pane, so its inline editors parent
            // there; the designer still rebuilds its overlay in its own page.
            grid.update(msg, self.properties_panel.ui());
        }
        self.sync_designer(&name, &designer, ui);
    }

    /// Routes Edit menu commands to the active designer, or to the active code
    /// editor when a code tab is in front.
    fn dispatch_edit(&mut self, command: Command, ui: &mut Ui<Msg>) {
        if let Some((name, designer, designer_ui)) = self.active_designer() {
            let changed = {
                let designer = designer.borrow();
                match command {
                    Command::Undo => designer.undo(&designer_ui),
                    Command::Redo => designer.redo(&designer_ui),
                    // Cut is copy then delete, like the code editor's cut.
                    Command::Cut => {
                        let copied = designer.copy();
                        designer.delete_selection(&designer_ui) || copied
                    }
                    Command::Copy => designer.copy(),
                    Command::Paste => designer.paste(&designer_ui),
                    Command::Delete => designer.delete_selection(&designer_ui),
                    Command::SelectAll => {
                        designer.select_all(&designer_ui);
                        false
                    }
                    _ => false,
                }
            };
            if changed {
                self.sync_designer(&name, &designer, ui);
            }
            return;
        }

        let Some((name, editor)) = self.active_code_editor() else {
            return;
        };
        let changed = match command {
            Command::Undo => editor.undo(),
            Command::Redo => editor.redo(),
            Command::Cut => editor.cut(),
            Command::Paste => editor.paste(),
            Command::Delete => editor.delete_selection(),
            // Copy and Select All leave the buffer alone, so they must not mark
            // the document dirty.
            Command::Copy => {
                editor.copy();
                false
            }
            Command::SelectAll => {
                editor.select_all();
                false
            }
            _ => false,
        };
        if changed {
            self.after_programmatic_edit(&name, ui);
        }
    }

    /// Rewrites a form's `.rhai` handlers after a designer rename, including an
    /// open code tab for that form. Comments and strings are skipped by
    /// [`rename_handlers`].
    fn rename_control(&mut self, form: &str, old: &str, new: &str, ui: &mut Ui<Msg>) {
        let Some(kind) = self
            .session
            .as_ref()
            .and_then(|session| session.form(form))
            .and_then(|doc| doc.node(new))
            .map(|node| node.kind.clone())
        else {
            return;
        };
        let events = handler_events(&self.catalog, &kind);
        let Some(source) = self
            .session
            .as_ref()
            .and_then(|session| session.code(form))
            .map(str::to_owned)
        else {
            return;
        };
        let rewritten = rename_handlers(&source, old, new, &events);
        if rewritten == source {
            return;
        }
        if let Some(session) = self.session.as_mut() {
            session.set_code(form, rewritten.clone());
        }
        // One range replace keeps the editor's undo history, unlike set_text.
        if let Some(editor) = self.code_editor(form) {
            let len = editor.text().chars().count();
            editor.replace(0, len, &rewritten);
        }
        if self.mark_document_dirty(form, DocKind::Code) {
            self.refresh_titles();
        }
        self.schedule_compile(form, &rewritten);
        self.update_title(ui);
    }

    /// Rewrites every tab title so a dirty document shows `*`.
    fn refresh_titles(&self) {
        let Some(docs) = &self.docs else {
            return;
        };
        for (index, document) in self.documents.iter().enumerate() {
            docs.rename_page(index + 1, &document.tab_title());
        }
    }

    /// Pushes every open designer's document into the session, so Save writes
    /// what the user drew even if no message followed the last edit.
    fn sync_designers(&mut self) {
        let mut layouts = Vec::new();
        for document in &self.documents {
            if let DocumentView::Designer { designer, .. } = &document.view {
                layouts.push((document.name.clone(), designer.borrow().doc()));
            }
        }
        if let Some(session) = self.session.as_mut() {
            for (name, doc) in layouts {
                session.set_form(&name, doc);
            }
        }
    }

    /// Marks a document dirty after a programmatic edit, mirrors the editor's
    /// text into the session and arms the background compile.
    fn after_programmatic_edit(&mut self, name: &str, ui: &mut Ui<Msg>) {
        let Some(text) = self
            .documents
            .iter()
            .find(|document| document.name == name && document.kind == DocKind::Code)
            .and_then(|document| match &document.view {
                DocumentView::Code(view) => Some(view.editor.text()),
                DocumentView::Designer { .. } => None,
            })
        else {
            return;
        };
        if let Some(session) = self.session.as_mut() {
            session.set_code(name, text.clone());
        }
        if self.mark_document_dirty(name, DocKind::Code) {
            self.refresh_titles();
        }
        self.schedule_compile(name, &text);
        self.update_title(ui);
    }

    // ---- Code window: compile diagnostics and procedure combos -------------

    /// Arms the background compile for a document's newest text.
    fn schedule_compile(&self, name: &str, source: &str) {
        self.diagnostics
            .borrow_mut()
            .schedule(name, source, Instant::now());
    }

    /// Applies a finished compile: refreshes the Error List and the editor's
    /// markers. A result older than the newest scheduled revision is dropped.
    fn apply_compile(
        &mut self,
        name: &str,
        revision: u64,
        errors: Vec<CodeDiagnostic>,
        ui: &mut Ui<Msg>,
    ) {
        if !self.diagnostics.borrow().is_current(name, revision) {
            return;
        }
        self.errors.retain(|entry| entry.name != name);
        self.errors
            .extend(errors.iter().cloned().map(|diagnostic| ErrorEntry {
                name: name.to_owned(),
                diagnostic,
            }));
        self.refresh_error_list();
        if let Some(editor) = self.code_editor(name) {
            let markers = errors
                .iter()
                .map(|diagnostic| {
                    let line = diagnostic.line.saturating_sub(1);
                    let col = diagnostic.col.saturating_sub(1);
                    Marker::new(line, col, col + 1, MarkerKind::Error)
                })
                .collect();
            editor.set_markers(markers);
        }
        if errors.is_empty() {
            self.log(ui, format!("{name}: no syntax errors."));
        } else {
            for error in &errors {
                self.log(ui, format!("{name}{}", error.label()));
            }
        }
    }

    /// Rebuilds the Error List rows from the collected diagnostics.
    fn refresh_error_list(&self) {
        let rows: Vec<String> = self
            .errors
            .iter()
            .map(|entry| format!("{}{}", entry.name, entry.diagnostic.label()))
            .collect();
        self.error_list.set_model(ErrorRows(rows));
    }

    /// Opens the document an Error List row belongs to and jumps to its
    /// position.
    fn activate_error(&mut self, row: usize, ui: &mut Ui<Msg>) {
        let Some(entry) = self.errors.get(row).cloned() else {
            return;
        };
        self.open_code(&entry.name, ui);
        if let Some(editor) = self.code_editor(&entry.name) {
            editor.goto(
                entry.diagnostic.line.saturating_sub(1),
                entry.diagnostic.col.saturating_sub(1),
            );
            editor.focus();
        }
    }

    /// Handles a change of the object combo: rebuilds the procedure combo for
    /// the chosen object.
    ///
    /// xui's [`ComboBox`] cannot replace its items, so the procedure combo is
    /// destroyed and recreated with the new object's events.
    fn change_object(&mut self, name: &str, index: usize, ui: &mut Ui<Msg>) {
        let Some(scoped) = self.docs.as_ref().map(|docs| docs.ui().clone()) else {
            return;
        };
        let mut failure = None;
        {
            let Some(document) = self
                .documents
                .iter_mut()
                .find(|document| document.name == name && document.kind == DocKind::Code)
            else {
                return;
            };
            let DocumentView::Code(view) = &mut document.view else {
                return;
            };
            let Some(entry) = view.objects.get(index).cloned() else {
                return;
            };
            view.object_index = Some(index);
            let items: Vec<&str> = entry
                .events
                .iter()
                .map(|event| event.name.as_str())
                .collect();
            match ComboBox::new(&scoped, Rect::default(), &items)
                .map(|combo| with_icons(combo, items.len(), Lucide::Zap))
            {
                Ok(combo) => {
                    let name = name.to_owned();
                    view.procedure =
                        Some(combo.on_select(move |index| {
                            Some(Msg::ProcedureChanged(name.clone(), index))
                        }));
                }
                Err(error) => failure = Some(error.to_string()),
            }
        }
        if let Some(error) = failure {
            self.log(
                ui,
                format!("the procedure list could not be rebuilt: {error}"),
            );
        }
        self.layout_code_views(ui, ui.dpi());
    }

    /// Inserts the handler for the chosen event (or jumps to it when it already
    /// exists) and places the caret inside the new function.
    fn insert_procedure(&mut self, name: &str, index: usize, ui: &mut Ui<Msg>) {
        let Some((signature, args, editor)) = self.procedure_target(name, index) else {
            return;
        };
        if reveal_or_insert_handler(&editor, &signature, &args) {
            self.after_programmatic_edit(name, ui);
        }
    }

    /// Opens a form's default event handler for a double-clicked control (or the
    /// form itself), creating the function when it is missing.
    ///
    /// The default event comes from the catalog schema; the form's events use
    /// the `form` prefix (`form_load`). Opening the code tab makes it the active
    /// document, so the caret placed inside the handler is visible.
    fn open_default_handler(&mut self, form: &str, target: &Target, ui: &mut Ui<Msg>) {
        let Some((signature, args)) = self.default_handler(form, target) else {
            // The form or control vanished between the double-click and this
            // update (a closed tab, a queued message), so there is nothing to
            // open.
            return;
        };
        self.open_code(form, ui);
        let Some(editor) = self.code_editor(form) else {
            return;
        };
        if reveal_or_insert_handler(&editor, &signature, &args) {
            self.after_programmatic_edit(form, ui);
        }
    }

    /// The signature and argument list of the default event handler for `target`
    /// in `form`, from the catalog schema.
    fn default_handler(&self, form: &str, target: &Target) -> Option<(String, String)> {
        let form_doc = self.session.as_ref()?.form(form)?;
        let (prefix, event) = match target {
            Target::Form => (
                procedures::FORM_PREFIX.to_owned(),
                self.catalog.window_spec().default_event()?,
            ),
            Target::Node(name) => {
                let node = form_doc.node(name)?;
                (name.clone(), self.catalog.get(&node.kind)?.default_event()?)
            }
        };
        Some((
            procedures::signature(&prefix, &event.name),
            procedures::argument_list(event),
        ))
    }

    /// The signature, argument list and editor for a procedure selection.
    fn procedure_target(
        &self,
        name: &str,
        index: usize,
    ) -> Option<(String, String, Rc<Editor<Msg>>)> {
        let document = self
            .documents
            .iter()
            .find(|document| document.name == name && document.kind == DocKind::Code)?;
        let DocumentView::Code(view) = &document.view else {
            return None;
        };
        let object = view.selected_object()?;
        let event = object.events.get(index)?;
        Some((
            procedures::signature(&object.prefix, &event.name),
            procedures::argument_list(event),
            Rc::clone(&view.editor),
        ))
    }

    // ---- Explorer and context menu ----------------------------------------

    /// Rebuilds the explorer rows from the open project.
    fn refresh_explorer(&mut self, _ui: &Ui<Msg>) {
        self.explorer = match &self.session {
            Some(session) => Explorer::build(session),
            None => Explorer::empty(),
        };
        // Tree node ids are row indexes, so a click pending from the old rows
        // must not pair with a click on whatever now sits at the same index.
        self.double_click = DoubleClick::new();
        self.tree.set_rows(&self.explorer.rows);
    }

    /// Refreshes the explorer and title after a structural change.
    fn after_structure_change(&mut self, ui: &Ui<Msg>) {
        self.refresh_explorer(ui);
        self.update_title(ui);
    }

    /// The name of the selected explorer item, if a leaf is selected.
    fn selected_item_name(&self) -> Option<String> {
        self.tree
            .selected()
            .and_then(|node| self.explorer.entry(node))
            .and_then(ExplorerItem::name)
            .map(str::to_owned)
    }

    /// The item the Object/Code toggle acts on: the active document's item when
    /// one is open, otherwise the explorer selection.
    ///
    /// This is what makes F7/Shift+F7 flip the form in front of the user rather
    /// than whatever row happens to be highlighted in the tree.
    fn current_item_name(&self) -> Option<String> {
        if let Some(docs) = &self.docs {
            let selected = docs.selected();
            if selected > 0
                && let Some(document) = self.documents.get(selected - 1)
            {
                return Some(document.name.clone());
            }
        }
        self.selected_item_name()
    }

    /// The editor of an open code document named `name`, if it is open.
    fn code_editor(&self, name: &str) -> Option<Rc<Editor<Msg>>> {
        self.documents
            .iter()
            .find(|document| document.name == name && document.kind == DocKind::Code)
            .and_then(|document| match &document.view {
                DocumentView::Code(view) => Some(Rc::clone(&view.editor)),
                DocumentView::Designer { .. } => None,
            })
    }

    /// Handles a row selection: a second click on the same row opens it.
    fn on_explorer_selected(&mut self, node: usize, ui: &mut Ui<Msg>) {
        if self.double_click.register(node) {
            match self.explorer.entry(node).cloned() {
                Some(ExplorerItem::Form(name)) => self.open_object(&name, ui),
                Some(ExplorerItem::Module(name)) => self.open_code(&name, ui),
                _ => {}
            }
        }
    }

    /// Shows the context menu at a right-clicked row.
    fn on_explorer_context(&mut self, node: usize, at: Point, ui: &Ui<Msg>) {
        let Some(entry) = self.explorer.entry(node) else {
            return;
        };
        self.context_target = entry.name().map(str::to_owned);
        let bounds = ui.bounds(self.tree.id());
        self.context_menu
            .show_context(bounds.left + at.x, bounds.top + at.y);
    }

    /// Applies a context-menu action to the remembered item.
    fn run_context_action(&mut self, action: ContextAction, ui: &mut Ui<Msg>) {
        let Some(name) = self.context_target.clone() else {
            return;
        };
        // Viewing stays available while a program runs; changing the project
        // structure does not (issue #16).
        let mutates = matches!(
            action,
            ContextAction::Rename | ContextAction::Remove | ContextAction::SetStartup
        );
        if mutates && self.refuse_while_running(ui) {
            return;
        }
        match action {
            ContextAction::ViewCode => self.open_code(&name, ui),
            ContextAction::ViewObject => self.open_object(&name, ui),
            ContextAction::Rename => self.show_prompt(ui, PromptKind::Rename(name)),
            ContextAction::Remove => self.remove_item(&name, ui),
            ContextAction::SetStartup => self.set_startup(&name, ui),
        }
    }

    /// Logs and returns `true` when a program is running, so a caller that
    /// changes the project structure stops. Code edits stay allowed: the player
    /// runs the copy saved at Start, so they cannot affect the running program.
    fn refuse_while_running(&mut self, ui: &mut Ui<Msg>) -> bool {
        if self.run.is_running() {
            self.log(
                ui,
                "The project cannot be changed while the program is running.",
            );
        }
        self.run.is_running()
    }

    /// Removes an item and closes its documents.
    fn remove_item(&mut self, name: &str, ui: &mut Ui<Msg>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session.remove(name) {
            self.log(ui, format!("`{name}` is not in the project."));
            return;
        }
        // A compile of the removed item may still be running; its result must
        // not bring the item's errors back.
        self.diagnostics.borrow_mut().invalidate(name);
        self.documents.retain(|document| document.name != name);
        self.errors.retain(|entry| entry.name != name);
        self.refresh_error_list();
        if let Err(error) = self.rebuild_tabs() {
            self.log(ui, format!("the document area could not be reset: {error}"));
        }
        self.after_structure_change(ui);
        self.log(ui, format!("Removed {name} from the project."));
    }

    /// Renames an item, updating its documents.
    fn rename_item(&mut self, old: &str, new: &str, ui: &mut Ui<Msg>) {
        // A rename prompt opened before Start can be answered during the run.
        if self.refuse_while_running(ui) {
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match session.rename(old, new) {
            Ok(()) => {
                // A compile still running under the old name is stale.
                self.diagnostics.borrow_mut().invalidate(old);
                for document in &mut self.documents {
                    if document.name == old {
                        document.name = new.to_owned();
                        document.title = match document.kind {
                            DocKind::Designer => new.to_owned(),
                            DocKind::Code => format!("{new}.rhai"),
                        };
                    }
                }
                if let Err(error) = self.rebuild_tabs() {
                    self.log(ui, format!("the document area could not be reset: {error}"));
                }
                for entry in &mut self.errors {
                    if entry.name == old {
                        entry.name = new.to_owned();
                    }
                }
                self.refresh_error_list();
                self.after_structure_change(ui);
                self.log(ui, format!("Renamed {old} to {new}."));
            }
            Err(error) => self.log(ui, format!("Rename failed: {error}")),
        }
    }

    /// Marks an item as the startup.
    fn set_startup(&mut self, name: &str, ui: &mut Ui<Msg>) {
        let started = self
            .session
            .as_mut()
            .is_some_and(|session| session.set_startup(name));
        if started {
            self.after_structure_change(ui);
            self.log(ui, format!("{name} is now the startup item."));
        } else {
            self.log(ui, format!("`{name}` is already the startup item."));
        }
    }

    // ---- Documents ---------------------------------------------------------

    /// Opens the code document for `name`.
    fn open_code(&mut self, name: &str, ui: &mut Ui<Msg>) {
        if self.session.as_ref().and_then(|s| s.code(name)).is_none() {
            self.log(ui, format!("`{name}` has no code."));
            return;
        }
        self.open_document(name, DocKind::Code)
            .unwrap_or_else(|error| {
                self.log(ui, format!("the document could not be opened: {error}"));
            });
    }

    /// Opens the designer document for `name`.
    fn open_object(&mut self, name: &str, ui: &mut Ui<Msg>) {
        if self.session.as_ref().and_then(|s| s.form(name)).is_none() {
            self.log(ui, format!("`{name}` is not a form."));
            return;
        }
        self.open_document(name, DocKind::Designer)
            .unwrap_or_else(|error| {
                self.log(ui, format!("the document could not be opened: {error}"));
            });
    }

    /// Opens (or focuses) the document tab for an item.
    fn open_document(&mut self, name: &str, kind: DocKind) -> UiResult<()> {
        if let Some(index) = self
            .documents
            .iter()
            .position(|document| document.name == name && document.kind == kind)
        {
            if let Some(docs) = &self.docs {
                // Page 0 is the Start Page.
                docs.select(index + 1);
            }
            // `select` raises no change message, so place the page's code
            // children (the procedure combos) and rebind the grid here.
            let docs_ui = self.docs_ui.clone();
            self.layout_code_views(&docs_ui, docs_ui.dpi());
            self.refresh_property_grid(&docs_ui);
            return Ok(());
        }

        // The tabs can be missing if rebuilding them failed earlier; report it
        // instead of panicking, since every caller logs this error.
        let Some(scoped) = self.docs.as_ref().map(|docs| docs.ui().clone()) else {
            return Err(BackendError::Other(
                "the document tabs are not available".to_owned(),
            ));
        };
        let (view, ids, title) = self.build_view(&scoped, name, kind)?;
        let Some(tabs) = self.docs.take() else {
            return Err(BackendError::Other(
                "the document tabs are not available".to_owned(),
            ));
        };
        self.documents.push(Document {
            name: name.to_owned(),
            kind,
            title: title.clone(),
            dirty: false,
            view,
        });
        self.docs = Some(tabs.page(&title, &ids));
        // Bring the new document to the front (page 0 is the Start Page).
        if let Some(docs) = &self.docs {
            docs.select(self.documents.len());
        }
        let docs_ui = self.docs_ui.clone();
        self.layout_code_views(&docs_ui, docs_ui.dpi());
        self.refresh_property_grid(&docs_ui);
        Ok(())
    }

    /// Builds the widgets behind a document, returning the view, the ids of its
    /// page children and the tab title.
    fn build_view(
        &mut self,
        ui: &Ui<Msg>,
        name: &str,
        kind: DocKind,
    ) -> UiResult<(DocumentView, Vec<WidgetId>, String)> {
        match kind {
            DocKind::Designer => {
                // The page panel scopes the designer's design mode to the
                // document area, leaving the tab strip and the other panes live.
                // Everything the designer builds is parented to it, so the tab
                // shows and hides the whole form with the page.
                let page = Panel::new(ui, Rect::default())?;
                let designer_ui = page.ui().clone();
                let doc = self
                    .session
                    .as_ref()
                    .and_then(|session| session.form(name))
                    .cloned()
                    .unwrap_or_else(|| lazyrad_project::FormDoc::new(name));
                let document = name.to_owned();
                let designer = Designer::new(
                    &designer_ui,
                    Rect::default(),
                    doc,
                    Rc::clone(&self.catalog),
                    move |msg| Msg::Designer {
                        document: document.clone(),
                        msg,
                    },
                )
                .map_err(|error| BackendError::Other(error.to_string()))?;
                // Rewrite the form's handlers when the grid renames a control.
                // The sink only raises a message; the rewrite happens on the
                // update path, where the session and the code tabs are safe.
                let renamed = name.to_owned();
                let ui_for_sink = designer_ui.clone();
                designer.set_rename_sink(move |old, new| {
                    ui_for_sink.emit(Msg::RenamedControl {
                        form: renamed.clone(),
                        old: old.to_owned(),
                        new: new.to_owned(),
                    });
                });
                // Double-clicking a control (or the form) opens its default
                // handler. The sink only raises a message; opening the code tab
                // and inserting happen on the update path.
                let doubled = name.to_owned();
                let ui_for_double = designer_ui.clone();
                designer.set_double_click_sink(move |target| {
                    ui_for_double.emit(Msg::OpenDefaultHandler {
                        form: doubled.clone(),
                        target: target.clone(),
                    });
                });
                let id = page.id();
                Ok((
                    DocumentView::Designer {
                        designer: Rc::new(RefCell::new(designer)),
                        designer_ui,
                        _page: page,
                    },
                    vec![id],
                    name.to_owned(),
                ))
            }
            DocKind::Code => {
                let catalog = lazyrad_project::lazyrad_catalog();
                let is_form = self.session.as_ref().and_then(|s| s.form(name)).is_some();
                let objects = self
                    .session
                    .as_ref()
                    .and_then(|s| s.form(name))
                    .map(|form| procedures::objects(&catalog, form))
                    .unwrap_or_default();

                let object = if is_form {
                    let items: Vec<&str> =
                        objects.iter().map(|entry| entry.label.as_str()).collect();
                    Some(
                        with_icons(
                            ComboBox::new(ui, Rect::default(), &items)?,
                            items.len(),
                            Lucide::Box,
                        )
                        .on_select({
                            let name = name.to_owned();
                            move |index| Some(Msg::ObjectChanged(name.clone(), index))
                        }),
                    )
                } else {
                    None
                };
                let procedure = if is_form {
                    let items = objects
                        .first()
                        .map(ObjectEntry::event_names)
                        .unwrap_or_default();
                    let items: Vec<&str> = items.iter().map(String::as_str).collect();
                    Some(
                        with_icons(
                            ComboBox::new(ui, Rect::default(), &items)?,
                            items.len(),
                            Lucide::Zap,
                        )
                        .on_select({
                            let name = name.to_owned();
                            move |index| Some(Msg::ProcedureChanged(name.clone(), index))
                        }),
                    )
                } else {
                    None
                };

                let editor = Editor::with_options(ui, Rect::default(), self.editor_options())?
                    .with_highlighter(RhaiHighlighter)
                    .on_change({
                        let name = name.to_owned();
                        move |text| Some(Msg::DocumentEdited(name.clone(), text.to_string()))
                    });
                let editor = Rc::new(editor);
                self.editors.borrow_mut().push(Rc::clone(&editor));
                let source = self
                    .session
                    .as_ref()
                    .and_then(|s| s.code(name))
                    .unwrap_or_default()
                    .to_owned();
                editor.set_text(&source);
                // Re-apply diagnostics an earlier compile left for this item.
                let markers: Vec<Marker> = self
                    .errors
                    .iter()
                    .filter(|entry| entry.name == name)
                    .map(|entry| {
                        let line = entry.diagnostic.line.saturating_sub(1);
                        let col = entry.diagnostic.col.saturating_sub(1);
                        Marker::new(line, col, col + 1, MarkerKind::Error)
                    })
                    .collect();
                editor.set_markers(markers);
                self.schedule_compile(name, &source);

                let mut ids = Vec::new();
                if let Some(combo) = &object {
                    ids.push(combo.id());
                }
                if let Some(combo) = &procedure {
                    ids.push(combo.id());
                }
                ids.push(editor.id());
                let view = CodeView {
                    object,
                    procedure,
                    editor: Rc::clone(&editor),
                    objects,
                    object_index: is_form.then_some(0),
                };
                Ok((
                    DocumentView::Code(Box::new(view)),
                    ids,
                    format!("{name}.rhai"),
                ))
            }
        }
    }

    /// Closes every document and rebuilds the tabs with just the Start Page.
    fn reset_documents(&mut self) -> UiResult<()> {
        // A compile still running for the old project must not land in the new
        // one's Error List.
        self.diagnostics.borrow_mut().reset();
        self.editors.borrow_mut().clear();
        // Drop the grid first: it holds a strong handle on a designer, which
        // would otherwise outlive the document and keep its sinks alive.
        self.properties_grid = None;
        self.grid_form = None;
        self.documents.clear();
        self.errors.clear();
        self.error_list.set_items(&[]);
        self.rebuild_tabs()
    }

    /// Rebuilds the document tabs and their widgets.
    ///
    /// xui's [`Tabs`] cannot remove a page, and destroying its container
    /// destroys the document widgets parented to it, so a rebuild recreates the
    /// open documents from the session. The Start Page is always first.
    fn rebuild_tabs(&mut self) -> UiResult<()> {
        let open: Vec<(String, DocKind)> = self
            .documents
            .iter()
            .map(|document| (document.name.clone(), document.kind))
            .collect();
        self.editors.borrow_mut().clear();
        // The grid holds a designer alive; drop it before the documents so a
        // closed tab's designer is really dropped.
        self.properties_grid = None;
        self.grid_form = None;
        self.documents.clear();
        self.docs = None;
        self.start_label = None;

        let tabs = Tabs::new(&self.docs_ui, Rect::default())?
            .on_change(|index| Some(Msg::TabChanged(index)));
        let welcome = Label::new(tabs.ui(), Rect::new(16, 16, 560, 48), WELCOME)?;
        self.start_label = Some(welcome);
        let welcome_id = self
            .start_label
            .as_ref()
            .expect("the welcome label was just created")
            .id();
        self.docs = Some(tabs.page("Start Page", &[welcome_id]));

        for (name, kind) in open {
            self.open_document(&name, kind)?;
        }

        if let Some(docs) = &self.docs {
            self.centre.pane_a(&[docs.id()]);
            docs.relayout();
        }
        // The selected page's designer may have changed; rebind and reposition.
        let docs_ui = self.docs_ui.clone();
        self.refresh_property_grid(&docs_ui);
        Ok(())
    }

    /// Copies every open code document's text back into the project.
    fn sync_documents(&mut self) {
        for document in &self.documents {
            if let DocumentView::Code(view) = &document.view
                && let Some(session) = self.session.as_mut()
            {
                // Only write back real edits: `set_code` marks the project
                // dirty, and an unchanged document must not.
                let text = view.editor.text();
                if session.code(&document.name) != Some(text.as_str()) {
                    session.set_code(&document.name, text);
                }
            }
        }
    }
}

impl App for IdeApp {
    type Msg = Msg;

    fn update(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
        match msg {
            Msg::Command(command) => self.run_command(command, ui),
            Msg::Relayout => self.layout_frame(ui),
            Msg::PaneMoved(slot, position) => self.on_pane_moved(slot, position, ui),
            Msg::ExplorerSelected(node) => self.on_explorer_selected(node, ui),
            Msg::ExplorerContext(node, at) => self.on_explorer_context(node, at, ui),
            Msg::ContextAction(action) => self.run_context_action(action, ui),
            Msg::PromptSubmitted(text) => self.resolve_prompt(text, ui),
            Msg::PromptCancelled => {
                self.prompt = None;
            }
            Msg::SaveChoice(choice) => self.resolve_save_prompt(choice, ui),
            Msg::DocumentEdited(name, text) => {
                self.schedule_compile(&name, &text);
                if let Some(session) = self.session.as_mut() {
                    session.set_code(&name, text);
                }
                if self.mark_document_dirty(&name, DocKind::Code) {
                    self.refresh_titles();
                }
                self.update_title(ui);
            }
            Msg::TabChanged(_) => {
                self.layout_code_views(ui, ui.dpi());
                self.refresh_property_grid(ui);
                self.layout_frame(ui);
            }
            Msg::CompileFinished {
                name,
                revision,
                errors,
            } => self.apply_compile(&name, revision, errors, ui),
            Msg::ErrorActivated(row) => self.activate_error(row, ui),
            Msg::ObjectChanged(name, index) => self.change_object(&name, index, ui),
            Msg::ProcedureChanged(name, index) => self.insert_procedure(&name, index, ui),
            // Design-time editing is off while a program runs, as VB did: the
            // designer, toolbox and property grid ignore their input.
            Msg::Designer { .. } | Msg::Toolbox(_) | Msg::PropertyGrid { .. }
                if self.run.is_running() => {}
            Msg::Designer { document, msg } => {
                if let Some((_, designer, designer_ui)) = self
                    .active_designer()
                    .filter(|(name, _, _)| *name == document)
                {
                    designer.borrow().update(msg, &designer_ui);
                    self.sync_designer(&document, &designer, ui);
                }
            }
            Msg::Toolbox(msg) => self.toolbox_message(msg, ui),
            Msg::PropertyGrid { form, msg } => self.property_grid_message(&form, msg, ui),
            Msg::RenamedControl { form, old, new } => {
                self.rename_control(&form, &old, &new, ui);
            }
            Msg::OpenDefaultHandler { form, target } => {
                self.open_default_handler(&form, &target, ui);
            }
            Msg::Run(run, event) => self.on_run_event(run, event, ui),
        }
    }
}

/// Finds the handler `signature` in `editor` and places the caret inside its
/// body, or appends it as one undoable edit when it is missing.
///
/// Returns whether text was inserted, so the caller mirrors the editor into the
/// session and schedules a compile (a found handler changes no text). This is
/// the shared insertion path for the procedure combo and a designer
/// double-click (issue #15).
fn reveal_or_insert_handler(editor: &Editor<Msg>, signature: &str, args: &str) -> bool {
    if let Some(offset) = procedures::find_handler(&editor.text(), signature) {
        editor.set_caret(offset);
        editor.focus();
        return false;
    }
    let text = editor.text();
    let end = text.chars().count();
    let separator = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let snippet = procedures::handler_snippet(signature, args);
    let insert = format!("{separator}{snippet}");
    // The caret lands just after the opening brace of the new handler.
    let before_brace = format!("fn {signature}({args}) ").chars().count();
    editor.set_caret(end);
    editor.insert_text(&insert);
    editor.set_caret(end + separator.chars().count() + before_brace + 1);
    editor.focus();
    true
}

/// A split's extent along its split axis: height for a column, width for a row.
fn split_size(id: WidgetId, vertical: bool, ui: &Ui<Msg>) -> i32 {
    let bounds = ui.bounds(id);
    if vertical {
        bounds.height()
    } else {
        bounds.width()
    }
}

/// The first-pane extent (design units) that leaves `second` design units for
/// the split's second pane.
fn first_pane(id: WidgetId, second: f32, vertical: bool, ui: &Ui<Msg>, dpi: u32) -> Dip {
    first_pane_for_size(split_size(id, vertical, ui), second, dpi)
}

/// [`first_pane`]'s arithmetic, over a split extent already in device pixels.
fn first_pane_for_size(size: i32, second: f32, dpi: u32) -> Dip {
    let divider = Dip(DIVIDER).to_px(dpi).value();
    let second_px = Dip(second.max(0.0)).to_px(dpi).value();
    Px((size - second_px - divider).max(0)).to_dip(dpi)
}

/// The second pane's current extent (design units) from the first pane's
/// position.
fn second_extent(id: WidgetId, position: f32, vertical: bool, ui: &Ui<Msg>, dpi: u32) -> f32 {
    second_extent_for_size(split_size(id, vertical, ui), position, dpi)
}

/// [`second_extent`]'s arithmetic, over a split extent already in device pixels.
fn second_extent_for_size(size: i32, position: f32, dpi: u32) -> f32 {
    let divider = Dip(DIVIDER).to_px(dpi).value();
    let position_px = Dip(position.max(0.0)).to_px(dpi).value();
    Px((size - position_px - divider).max(0))
        .to_dip(dpi)
        .value()
}

/// Builds the menu bar and the map from each entry's [`MenuId`] to its command.
fn build_menu(
    ui: &Ui<Msg>,
    bounds: Rect,
    recent: &[PathBuf],
) -> UiResult<(Menu<Msg>, MenuCommands)> {
    let mut ids = MenuIds::new();
    let menu = Menu::bar(ui, bounds)?.build(|bar| {
        bar.submenu(ids.plain(), "&File", |file| {
            file.command(&mut ids, Command::NewProject, "&New Project")
                .command(&mut ids, Command::OpenProject, "&Open Project…");
            file.submenu(ids.plain(), "Recent", |recent_menu| {
                if recent.is_empty() {
                    recent_menu.item(ids.plain(), "(no recent projects)");
                } else {
                    for (index, path) in recent.iter().enumerate() {
                        let name = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string());
                        recent_menu.item(ids.id(Command::OpenRecent(index)), &name);
                    }
                }
            });
            file.separator()
                .command(&mut ids, Command::Save, "&Save")
                .command(&mut ids, Command::SaveAs, "Save &As…")
                .command(&mut ids, Command::SaveAll, "Save &All")
                .command(&mut ids, Command::CloseProject, "&Close Project")
                .separator()
                .command(&mut ids, Command::Exit, "E&xit");
        });
        bar.submenu(ids.plain(), "&Edit", |edit| {
            edit.command(&mut ids, Command::Undo, "&Undo")
                .command(&mut ids, Command::Redo, "&Redo")
                .separator()
                .command(&mut ids, Command::Cut, "Cu&t")
                .command(&mut ids, Command::Copy, "&Copy")
                .command(&mut ids, Command::Paste, "&Paste")
                .command(&mut ids, Command::Delete, "&Delete")
                .separator()
                .command(&mut ids, Command::SelectAll, "Select &All")
                .command(&mut ids, Command::Find, "&Find…")
                .command(&mut ids, Command::FindNext, "Find &Next")
                .command(&mut ids, Command::Replace, "&Replace…")
                .command(&mut ids, Command::GoToLine, "&Go To Line…");
        });
        bar.submenu(ids.plain(), "&View", |view| {
            view.command(&mut ids, Command::ViewCode, "&Code")
                .command(&mut ids, Command::ViewObject, "&Object")
                .separator()
                .command(&mut ids, Command::ViewProject, "&Project")
                .command(&mut ids, Command::ViewProperties, "P&roperties")
                .command(&mut ids, Command::ViewToolbox, "&Toolbox")
                .command(&mut ids, Command::ViewOutput, "&Output")
                .separator()
                .submenu(ids.plain(), "&Theme", |theme| {
                    theme.command(&mut ids, Command::ThemeLight, "&Light");
                    theme.command(&mut ids, Command::ThemeDark, "&Dark");
                    theme.command(&mut ids, Command::ThemeSystem, "&System");
                });
        });
        bar.submenu(ids.plain(), "&Project", |project| {
            project
                .command(&mut ids, Command::AddForm, "Add &Form")
                .command(&mut ids, Command::AddModule, "Add &Module")
                .separator()
                .command(&mut ids, Command::Remove, "&Remove")
                .separator()
                .command(&mut ids, Command::ProjectProperties, "P&roperties");
        });
        bar.submenu(ids.plain(), "&Run", |run| {
            run.command(&mut ids, Command::RunStart, "&Start").command(
                &mut ids,
                Command::RunEnd,
                "&End",
            );
        });
    });
    let commands = Rc::new(ids.map);
    let map_for_select = Rc::clone(&commands);
    let menu = menu.on_select(move |id| {
        map_for_select
            .iter()
            .find(|(entry, _)| *entry == id)
            .map(|(_, command)| Msg::Command(*command))
    });
    Ok((menu, commands))
}

/// Hands out unique [`MenuId`]s while building the menu, remembering which ones
/// dispatch a command.
struct MenuIds {
    next: usize,
    map: Vec<(MenuId, Command)>,
}

impl MenuIds {
    fn new() -> MenuIds {
        MenuIds {
            next: 0,
            map: Vec::new(),
        }
    }

    /// An id that dispatches `command`.
    fn id(&mut self, command: Command) -> MenuId {
        let id = self.plain();
        self.map.push((id, command));
        id
    }

    /// An id that dispatches nothing (a submenu parent or a placeholder).
    fn plain(&mut self) -> MenuId {
        let id = MenuId::new(self.next);
        self.next += 1;
        id
    }
}

/// Gives each of a combo's `count` items the same leading `icon`: the object
/// combo marks objects with a box, the procedure combo events with a zap.
fn with_icons(mut combo: ComboBox<Msg>, count: usize, icon: Lucide) -> ComboBox<Msg> {
    for index in 0..count {
        combo = combo.item_icon(index, icon);
    }
    combo
}

/// Menu entries that dispatch a [`Command`].
trait CommandItems {
    /// Appends an entry for `command`, with the command's toolbar icon when it
    /// has one, so the menu and the toolbar show the same symbol.
    fn command(&mut self, ids: &mut MenuIds, command: Command, text: &str) -> &mut Self;
}

impl CommandItems for MenuScope<'_> {
    fn command(&mut self, ids: &mut MenuIds, command: Command, text: &str) -> &mut Self {
        self.item(ids.id(command), text);
        if let Some(icon) = toolbar_icon(command) {
            self.icon(icon);
        }
        self
    }
}

/// The toolbar icon of `command`, if the toolbar shows it.
fn toolbar_icon(command: Command) -> Option<Lucide> {
    TOOLBAR
        .iter()
        .find(|item| item.command == command)
        .map(|item| item.icon)
}

/// Maps a key-down event to the command it triggers, for the shortcut backend.
///
/// Auto-repeat and system (Alt) combinations are ignored.
pub fn shortcut_message(event: &Event) -> Option<Msg> {
    if let Event::KeyDown {
        key,
        modifiers,
        repeat,
        system,
    } = event
        && !*system
        && *repeat <= 1
    {
        return Command::from_keydown(*key, *modifiers).map(Msg::Command);
    }
    None
}

/// The default window size.
pub fn default_platform_spec() -> PlatformSpec {
    PlatformSpec::new("LazyRAD").size(Dip(1200.0), Dip(800.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::SessionError;
    use lazyrad_designer::Tool;
    use std::cell::Cell;
    use xui_canvas::OffscreenBackend;
    use xui_core::Key;
    use xui_core::backend::Backend;
    use xui_core::run_app;

    #[test]
    fn the_toolbar_groups_are_file_edit_clipboard_and_run() {
        let starts: Vec<Command> = TOOLBAR_GROUP_STARTS
            .iter()
            .map(|&index| TOOLBAR[index].command)
            .collect();
        assert_eq!(
            starts,
            [Command::Undo, Command::Cut, Command::RunStart],
            "each group starts where its first command is"
        );
        assert!(
            TOOLBAR_GROUP_STARTS
                .iter()
                .all(|&index| index > 0 && index < TOOLBAR.len()),
            "no separator at either end"
        );
    }

    #[test]
    fn code_editors_use_the_configured_monospace_font() {
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), |ui| {
            let mut settings = Settings::default();
            settings.editor_font_family = "Test Mono".to_owned();
            settings.editor_font_size = 15.0;
            let app = IdeApp::build(ui, settings, Vec::new()).expect("the IDE builds");
            let options = app.editor_options();
            assert_eq!(options.font.family.as_deref(), Some("Test Mono"));
            assert_eq!(options.font.size, Dip(15.0));
            app
        })
        .expect("the offscreen backend runs to completion");
    }

    #[test]
    fn menu_entries_take_their_toolbar_icon() {
        for item in TOOLBAR {
            assert_eq!(toolbar_icon(item.command), Some(item.icon));
        }
        assert_eq!(
            toolbar_icon(Command::Exit),
            None,
            "a command without a toolbar icon gets none in the menu"
        );
    }

    #[test]
    fn every_error_list_row_leads_with_the_error_icon() {
        let rows = ErrorRows(vec!["Form1 (2:5): boom".to_owned(), "util: bad".to_owned()]);
        assert_eq!(rows.rows(), 2);
        assert_eq!(rows.cell(0, 0), Some("Form1 (2:5): boom"));
        assert_eq!(rows.cell(0, 1), None, "the list has one column");
        assert_eq!(rows.cell(5, 0), None);
        assert_eq!(rows.icon(1), Some(Lucide::CircleX.into()));
    }

    #[test]
    fn every_toolbar_entry_has_an_icon_and_a_tooltip_naming_its_shortcut() {
        let commands: Vec<Command> = TOOLBAR.iter().map(|entry| entry.command).collect();
        assert_eq!(
            commands,
            [
                Command::NewProject,
                Command::OpenProject,
                Command::Save,
                Command::SaveAll,
                Command::Undo,
                Command::Redo,
                Command::Cut,
                Command::Copy,
                Command::Paste,
                Command::RunStart,
                Command::RunEnd,
            ],
            "the toolbar order does not drift"
        );

        for entry in TOOLBAR {
            let tooltip = entry.tooltip();
            assert!(
                tooltip.contains(&entry.command.label()),
                "the tooltip names the command: {tooltip}"
            );
            match entry.command.shortcut_text() {
                Some(shortcut) => assert!(
                    tooltip.contains(&shortcut),
                    "the tooltip of {} includes {shortcut}: {tooltip}",
                    entry.command.label()
                ),
                None => assert!(
                    !tooltip.contains('('),
                    "a command without a shortcut shows no empty chord: {tooltip}"
                ),
            }
        }

        // Only Run and End keep a visible label.
        let labelled: Vec<Option<&str>> = TOOLBAR.iter().map(|entry| entry.label).collect();
        assert_eq!(
            labelled,
            [
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some("Run"),
                Some("End")
            ]
        );

        let icons: Vec<Lucide> = TOOLBAR.iter().map(|entry| entry.icon).collect();
        assert_eq!(
            icons,
            [
                Lucide::FilePlus,
                Lucide::FolderOpen,
                Lucide::Save,
                Lucide::SaveAll,
                Lucide::Undo2,
                Lucide::Redo2,
                Lucide::Scissors,
                Lucide::Copy,
                Lucide::ClipboardPaste,
                Lucide::Play,
                Lucide::Square,
            ]
        );
    }

    #[test]
    fn the_shortcut_mapper_maps_and_ignores_as_expected() {
        assert_eq!(
            shortcut_message(&Event::KeyDown {
                key: Key::S,
                modifiers: xui_core::Modifiers {
                    ctrl: true,
                    ..xui_core::Modifiers::NONE
                },
                repeat: 1,
                system: false,
            }),
            Some(Msg::Command(Command::Save))
        );
        assert_eq!(
            shortcut_message(&Event::KeyUp {
                key: Key::S,
                modifiers: xui_core::Modifiers::NONE,
                system: false,
            }),
            None
        );
        assert_eq!(
            shortcut_message(&Event::KeyDown {
                key: Key::S,
                modifiers: xui_core::Modifiers {
                    ctrl: true,
                    ..xui_core::Modifiers::NONE
                },
                repeat: 2,
                system: false,
            }),
            None,
            "auto-repeat is ignored"
        );
    }

    #[test]
    fn the_ide_shell_builds_and_runs_headlessly() {
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds")
        })
        .expect("the offscreen backend runs to completion");
    }

    #[test]
    fn the_first_pane_leaves_room_for_the_second() {
        // A 1000px split with a 160-unit second pane at 96dpi and a 5px
        // divider leaves 1000 - 160 - 5 = 835 design units for the first pane.
        assert_eq!(first_pane_for_size(1000, 160.0, 96).value(), 835.0);
        // At 192dpi the 160-unit pane is 320px and the divider 10px, so the
        // first pane is 1000 - 320 - 10 = 670px, i.e. 335 design units.
        assert_eq!(first_pane_for_size(1000, 160.0, 192).value(), 335.0);
        // A tiny split clamps at zero rather than going negative.
        assert_eq!(first_pane_for_size(10, 160.0, 96).value(), 0.0);
    }

    #[test]
    fn the_second_extent_inverts_the_first() {
        assert_eq!(second_extent_for_size(1000, 835.0, 96), 160.0);
        assert_eq!(second_extent_for_size(1000, 335.0, 192), 160.0);
    }

    #[test]
    fn session_errors_read_well() {
        let error = SessionError::InvalidName("`bad` is not valid".to_owned());
        assert_eq!(error.to_string(), "`bad` is not valid");
    }

    /// A launcher that records calls and hands back a no-op child.
    struct StubLauncher {
        fail: bool,
        launches: Cell<usize>,
    }

    impl StubLauncher {
        fn ok() -> StubLauncher {
            StubLauncher {
                fail: false,
                launches: Cell::new(0),
            }
        }

        fn failing() -> StubLauncher {
            StubLauncher {
                fail: true,
                launches: Cell::new(0),
            }
        }
    }

    impl run::Launcher for StubLauncher {
        fn launch(
            &self,
            player: &Path,
            _project_dir: &Path,
            _run: RunId,
            _sink: run::EventSink,
        ) -> Result<Box<dyn run::ChildProcess>, run::LaunchError> {
            self.launches.set(self.launches.get() + 1);
            if self.fail {
                return Err(run::LaunchError::new(
                    player,
                    std::io::Error::other("stub launch failure"),
                ));
            }
            Ok(Box::new(StubChild))
        }
    }

    struct StubChild;

    impl run::ChildProcess for StubChild {
        fn kill(&mut self) {}
        fn is_running(&mut self) -> bool {
            false
        }
    }

    /// A fresh project directory for one run test, emptied first.
    fn run_scratch(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lazyrad-ide-run-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Writes a stub player file the launcher never really runs.
    fn player_stub(dir: &Path) -> PathBuf {
        let player = dir.join("lazyrad-player-stub");
        std::fs::write(&player, b"stub").expect("write the stub");
        player
    }

    #[test]
    fn starting_a_run_marks_running_and_routes_events() {
        let dir = run_scratch("start");
        let cleanup = dir.clone();
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let session = ProjectSession::create("MyApp", &dir).expect("create");
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.refresh_explorer(ui);
            app.settings.player_path = Some(player_stub(&dir));
            app.launcher = Rc::new(StubLauncher::ok());

            app.start_run(ui);
            assert!(app.is_running());
            assert!(app.dispatcher.is_running());
            assert_eq!(app.status.text(), "Run");
            let id = app.run.run_id().expect("a run id");
            assert!(app.run.accepts(id));

            // A stdout line becomes an Output pane entry.
            app.on_run_event(
                id,
                RunEvent::Output("hello from the program".to_owned()),
                ui,
            );
            assert!(
                app.output_lines
                    .iter()
                    .any(|line| line == "hello from the program")
            );

            // A runtime diagnostic lists the error and opens the failing code.
            let file = format!("{}.rhai", crate::project::DEFAULT_FORM);
            app.on_run_event(
                id,
                RunEvent::Diagnostic(lazyrad_player::Report {
                    kind: lazyrad_player::Kind::Runtime,
                    file,
                    line: 2,
                    col: 5,
                    message: "boom".to_owned(),
                }),
                ui,
            );
            assert_eq!(app.errors.len(), 1);
            assert!(
                app.code_editor(crate::project::DEFAULT_FORM).is_some(),
                "a runtime error opens the failing file"
            );

            // The exit notice returns the IDE to Design.
            app.on_run_event(id, RunEvent::Exited(Some(0)), ui);
            assert!(!app.is_running());
            assert_eq!(app.status.text(), "Design");

            app
        })
        .expect("the offscreen backend runs to completion");
        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn a_compile_error_stops_the_run_before_spawning() {
        let dir = run_scratch("compile");
        let cleanup = dir.clone();
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let mut session = ProjectSession::create("MyApp", &dir).expect("create");
            session.set_code(
                crate::project::DEFAULT_FORM,
                "fn broken() {\n    let x = ;\n}\n".to_owned(),
            );
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.settings.player_path = Some(player_stub(&dir));
            let launcher = Rc::new(StubLauncher::ok());
            app.launcher = launcher.clone();

            app.start_run(ui);

            assert!(!app.is_running(), "a compile error does not start");
            assert!(!app.errors.is_empty(), "the Error List shows the problem");
            assert_eq!(launcher.launches.get(), 0, "no process was spawned");

            app
        })
        .expect("the offscreen backend runs to completion");
        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn a_failed_save_stops_the_run() {
        let dir = run_scratch("save");
        let cleanup = dir.clone();
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let mut session = ProjectSession::create("MyApp", &dir).expect("create");
            session.set_code(crate::project::DEFAULT_FORM, "// edited".to_owned());
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            // A file where the project folder should be makes every save fail.
            std::fs::remove_dir_all(&dir).expect("remove the folder");
            std::fs::write(&dir, b"in the way").expect("block the path");
            let launcher = Rc::new(StubLauncher::ok());
            app.launcher = launcher.clone();

            app.start_run(ui);

            assert!(!app.is_running(), "a failed save does not start");
            assert_eq!(launcher.launches.get(), 0, "no process was spawned");
            assert!(
                app.output_lines
                    .iter()
                    .any(|line| line.contains("could not be saved"))
            );

            app
        })
        .expect("the offscreen backend runs to completion");
        let _ = std::fs::remove_file(&cleanup);
    }

    #[test]
    fn a_failed_spawn_leaves_the_ide_in_design() {
        let dir = run_scratch("spawn");
        let cleanup = dir.clone();
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let session = ProjectSession::create("MyApp", &dir).expect("create");
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.settings.player_path = Some(player_stub(&dir));
            app.launcher = Rc::new(StubLauncher::failing());

            app.start_run(ui);

            assert!(!app.is_running());
            assert_eq!(app.status.text(), "Design");
            assert!(
                app.output_lines
                    .iter()
                    .any(|line| line.contains("could not start"))
            );

            app
        })
        .expect("the offscreen backend runs to completion");
        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn start_saves_an_unsaved_designer_layout_first() {
        let dir = run_scratch("save-layout");
        let cleanup = dir.clone();
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let session = ProjectSession::create("MyApp", &dir).expect("create");
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.refresh_explorer(ui);
            let form = crate::project::DEFAULT_FORM.to_owned();
            app.open_document(&form, DocKind::Designer)
                .expect("the form opens in a designer");
            app.update(
                Msg::Toolbox(ToolboxMsg::Activate(lazyrad_designer::Tool::control(
                    "Button",
                ))),
                ui,
            );
            assert!(app.documents.iter().any(|document| document.dirty));

            app.settings.player_path = Some(player_stub(&dir));
            app.launcher = Rc::new(StubLauncher::ok());
            app.start_run(ui);
            assert!(app.is_running());

            let layout = std::fs::read_to_string(dir.join(format!("{form}.lfm")))
                .expect("the layout is on disk");
            assert!(
                layout.contains("button1"),
                "the drawn button was saved: {layout}"
            );
            assert!(
                app.documents.iter().all(|document| !document.dirty),
                "every tab is clean after the save"
            );
            app
        })
        .expect("the offscreen backend runs to completion");
        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn the_designer_ignores_input_while_running() {
        let dir = run_scratch("designer-locked");
        let cleanup = dir.clone();
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let session = ProjectSession::create("MyApp", &dir).expect("create");
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.refresh_explorer(ui);
            let form = crate::project::DEFAULT_FORM.to_owned();
            app.open_document(&form, DocKind::Designer)
                .expect("the form opens in a designer");
            app.settings.player_path = Some(player_stub(&dir));
            app.launcher = Rc::new(StubLauncher::ok());
            app.start_run(ui);
            assert!(app.is_running());

            let controls = |app: &IdeApp| {
                app.session
                    .as_ref()
                    .and_then(|session| session.form(&form))
                    .map_or(0, |doc| doc.nodes.len())
            };
            let drop_button = Msg::Toolbox(ToolboxMsg::Activate(lazyrad_designer::Tool::control(
                "Button",
            )));
            app.update(drop_button.clone(), ui);
            assert_eq!(controls(&app), 0, "the toolbox is ignored while running");

            app.end_run(ui);
            app.update(drop_button, ui);
            assert_eq!(controls(&app), 1, "and works again after End");
            app
        })
        .expect("the offscreen backend runs to completion");
        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn the_project_structure_is_locked_while_running() {
        let dir = run_scratch("locked");
        let cleanup = dir.clone();
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let mut session = ProjectSession::create("MyApp", &dir).expect("create");
            let module = session.add_module();
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.refresh_explorer(ui);
            app.settings.player_path = Some(player_stub(&dir));
            app.launcher = Rc::new(StubLauncher::ok());
            app.start_run(ui);
            assert!(app.is_running());

            let has = |app: &IdeApp, name: &str| {
                app.session
                    .as_ref()
                    .is_some_and(|session| session.code(name).is_some())
            };
            // The context menu's Remove and Set Startup are refused.
            app.context_target = Some(module.clone());
            app.run_context_action(ContextAction::Remove, ui);
            assert!(has(&app, &module), "Remove is refused while running");
            app.run_context_action(ContextAction::SetStartup, ui);
            assert_ne!(
                app.session
                    .as_ref()
                    .map(|session| session.startup().to_owned()),
                Some(module.clone()),
                "Set Startup is refused while running"
            );
            // A rename prompt answered during the run is refused too.
            app.rename_item(&module, "renamed_module", ui);
            assert!(has(&app, &module), "Rename is refused while running");

            // After End, the same actions work again.
            app.end_run(ui);
            app.rename_item(&module, "renamed_module", ui);
            assert!(has(&app, "renamed_module"));
            app
        })
        .expect("the offscreen backend runs to completion");
        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn a_runtime_error_without_a_file_is_listed_but_opens_nothing() {
        let dir = run_scratch("no-file");
        let cleanup = dir.clone();
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let session = ProjectSession::create("MyApp", &dir).expect("create");
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.settings.player_path = Some(player_stub(&dir));
            app.launcher = Rc::new(StubLauncher::ok());
            app.start_run(ui);
            let id = app.run.run_id().expect("a run id");
            let tabs = app.documents.len();

            app.on_run_event(
                id,
                RunEvent::Diagnostic(lazyrad_player::Report {
                    kind: lazyrad_player::Kind::Runtime,
                    file: String::new(),
                    line: 0,
                    col: 0,
                    message: "Division by zero: 1 / 0".to_owned(),
                }),
                ui,
            );
            assert_eq!(app.errors.len(), 1, "the error is listed");
            assert_eq!(app.documents.len(), tabs, "no code tab was opened");
            assert!(
                !app.output_lines
                    .iter()
                    .any(|line| line.contains("has no code")),
                "no confusing log line: {:?}",
                app.output_lines
            );
            app
        })
        .expect("the offscreen backend runs to completion");
        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn a_stale_run_event_is_dropped() {
        let dir = run_scratch("stale");
        let cleanup = dir.clone();
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let session = ProjectSession::create("MyApp", &dir).expect("create");
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.settings.player_path = Some(player_stub(&dir));
            app.launcher = Rc::new(StubLauncher::ok());

            app.start_run(ui);
            let first = app.run.run_id().expect("a run id");
            app.end_run(ui);
            app.start_run(ui);
            let second = app.run.run_id().expect("a run id");
            assert_ne!(first, second, "a run id is never reused");

            let before = app.output_lines.len();
            app.on_run_event(first, RunEvent::Output("stale".to_owned()), ui);
            assert_eq!(
                app.output_lines.len(),
                before,
                "output from the ended run is dropped"
            );
            assert!(!app.run.finished(first), "a stale exit is dropped");
            assert!(app.is_running(), "the new run is untouched");

            app.on_run_event(second, RunEvent::Output("fresh".to_owned()), ui);
            assert_eq!(app.output_lines.last().map(String::as_str), Some("fresh"));

            app
        })
        .expect("the offscreen backend runs to completion");
        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn a_code_window_shows_diagnostics_and_inserts_handlers() {
        let dir = std::env::temp_dir().join(format!("lazyrad-ide-code-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup = dir.clone();

        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let session = ProjectSession::create("MyApp", &dir).expect("create");
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.refresh_explorer(ui);
            app.open_document(crate::project::DEFAULT_FORM, DocKind::Code)
                .expect("the code document opens");

            // A finished compile fills the Error List and the editor markers.
            let revision = app
                .diagnostics
                .borrow()
                .revision(crate::project::DEFAULT_FORM)
                .expect("opening the document scheduled a compile");
            app.apply_compile(
                crate::project::DEFAULT_FORM,
                revision,
                vec![CodeDiagnostic::new(1, 1, "boom")],
                ui,
            );
            assert_eq!(app.errors.len(), 1);
            assert_eq!(app.error_list.len(), 1);

            // Choosing the form's second event inserts the missing handler.
            app.insert_procedure(crate::project::DEFAULT_FORM, 1, ui);
            let text = app
                .code_editor(crate::project::DEFAULT_FORM)
                .expect("the editor is open")
                .text();
            assert!(text.contains("fn form_close(cancel) {"), "text was: {text}");

            // Rebuilding the procedure combo replaces a page child, so a
            // following re-layout must tolerate the destroyed old node.
            app.change_object(crate::project::DEFAULT_FORM, 1, ui);
            app.layout_frame(ui);

            // A clean compile clears both the list and the errors.
            let revision = app
                .diagnostics
                .borrow()
                .revision(crate::project::DEFAULT_FORM)
                .expect("opening the document scheduled a compile");
            app.apply_compile(crate::project::DEFAULT_FORM, revision, Vec::new(), ui);
            assert!(app.errors.is_empty());
            assert_eq!(app.error_list.len(), 0);

            app
        })
        .expect("the offscreen backend runs to completion");

        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn the_grid_shows_new_geometry_after_a_canvas_drag() {
        let dir = std::env::temp_dir().join(format!("lazyrad-ide-drag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup = dir.clone();

        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let session = ProjectSession::create("MyApp", &dir).expect("create");
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.refresh_explorer(ui);
            let form = crate::project::DEFAULT_FORM.to_owned();
            app.open_document(&form, DocKind::Designer)
                .expect("the form opens in a designer");
            // The toolbox drops a selected button1, which the grid shows.
            app.update(
                Msg::Toolbox(ToolboxMsg::Activate(lazyrad_designer::Tool::control(
                    "Button",
                ))),
                ui,
            );
            let geometry = |app: &IdeApp| {
                let node = app
                    .session
                    .as_ref()
                    .and_then(|session| session.form(&form))
                    .and_then(|doc| doc.node("button1"))
                    .cloned()
                    .expect("button1 exists");
                let int = |name: &str| {
                    node.prop(name)
                        .and_then(lazyrad_project::Value::as_int)
                        .unwrap_or(0)
                };
                (int("left"), int("top"), int("width"), int("height"))
            };
            let grid_left = |app: &IdeApp| {
                app.properties_grid
                    .as_ref()
                    .expect("the grid is bound")
                    .rows()
                    .into_iter()
                    .find(|row| row.name == "left")
                    .map(|row| row.value)
            };
            let (left, top, width, height) = geometry(&app);
            assert_eq!(grid_left(&app), Some(lazyrad_project::Value::Int(left)));

            // Drag the button by (24, 16) on the canvas.
            let (x, y) = (left + width / 2, top + height / 2);
            for msg in [
                DesignerMsg::PointerDown { x, y, ctrl: false },
                DesignerMsg::PointerMove {
                    x: x + 24,
                    y: y + 16,
                    ctrl: false,
                },
                DesignerMsg::PointerUp {
                    x: x + 24,
                    y: y + 16,
                    ctrl: false,
                },
            ] {
                app.update(
                    Msg::Designer {
                        document: form.clone(),
                        msg,
                    },
                    ui,
                );
            }
            let (moved, _, _, _) = geometry(&app);
            assert_ne!(moved, left, "the drag moved the button");
            assert_eq!(
                grid_left(&app),
                Some(lazyrad_project::Value::Int(moved)),
                "the grid's cached row follows the drag"
            );
            app
        })
        .expect("the offscreen backend runs to completion");
        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn a_form_tab_hosts_a_live_designer_and_the_grid_follows_the_active_tab() {
        let dir = std::env::temp_dir().join(format!("lazyrad-ide-grid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup = dir.clone();

        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let mut session = ProjectSession::create("MyApp", &dir).expect("create");
            session.add_form();
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.refresh_explorer(ui);

            app.open_document(crate::project::DEFAULT_FORM, DocKind::Designer)
                .expect("the form opens in a designer");
            assert!(
                matches!(
                    app.documents.first().map(|document| &document.view),
                    Some(DocumentView::Designer { .. })
                ),
                "a form tab hosts a real designer, not placeholder labels"
            );
            assert_eq!(app.grid_form.as_deref(), Some(crate::project::DEFAULT_FORM));
            assert!(app.properties_grid.is_some());

            app.open_document("form1", DocKind::Designer)
                .expect("the second form opens");
            assert_eq!(
                app.grid_form.as_deref(),
                Some("form1"),
                "the grid follows the newly opened form"
            );

            // Selecting the first form rebinds the grid to it.
            app.docs.as_ref().expect("the tabs exist").select(1);
            app.refresh_property_grid(ui);
            assert_eq!(app.grid_form.as_deref(), Some(crate::project::DEFAULT_FORM));

            // Selecting the Start Page leaves the pane with no grid at all.
            app.docs.as_ref().expect("the tabs exist").select(0);
            app.refresh_property_grid(ui);
            assert_eq!(app.grid_form, None);
            assert!(
                app.properties_grid.is_none(),
                "no designer means no grid and no lingering sink"
            );
            app
        })
        .expect("the offscreen backend runs to completion");

        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn copy_and_select_all_in_a_code_tab_do_not_dirty_it() {
        let dir = std::env::temp_dir().join(format!("lazyrad-ide-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup = dir.clone();

        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let session = ProjectSession::create("MyApp", &dir).expect("create");
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.refresh_explorer(ui);
            app.open_document(crate::project::DEFAULT_FORM, DocKind::Code)
                .expect("the code tab opens");
            // The form template is non-empty, so a copy has something to take.
            app.dispatch_edit(Command::Copy, ui);
            app.dispatch_edit(Command::SelectAll, ui);
            assert!(
                !app.documents.iter().any(|document| document.dirty),
                "copy and select all must not dirty the document"
            );
            app
        })
        .expect("the offscreen backend runs to completion");

        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn a_pane_move_reflows_the_toolbox_and_grid_into_their_panes() {
        let dir = std::env::temp_dir().join(format!("lazyrad-ide-panes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup = dir.clone();

        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let session = ProjectSession::create("MyApp", &dir).expect("create");
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.refresh_explorer(ui);
            app.open_document(crate::project::DEFAULT_FORM, DocKind::Designer)
                .expect("the form opens in a designer");

            // Moving the toolbox divider re-flows its contents into the pane.
            app.on_pane_moved(PaneSlot::Toolbox, 220.0, ui);
            let toolbox_pane = ui.bounds(app.toolbox_panel.id());
            let toolbox = ui.bounds(app.toolbox.id());
            assert_eq!(toolbox.left, 0);
            assert_eq!(toolbox.top, PANE_TITLE);
            assert_eq!(toolbox.width(), toolbox_pane.width());
            assert_eq!(toolbox.height(), toolbox_pane.height() - PANE_TITLE);

            // The grid follows the Properties pane the same way.
            let grid = app.properties_grid.as_ref().expect("the grid is bound");
            let properties = ui.bounds(app.properties_panel.id());
            let grid_bounds = ui.bounds(grid.id());
            assert_eq!(grid_bounds.left, 0);
            assert_eq!(grid_bounds.top, PANE_TITLE);
            assert_eq!(grid_bounds.width(), properties.width());
            assert_eq!(grid_bounds.height(), properties.height() - PANE_TITLE);
            app
        })
        .expect("the offscreen backend runs to completion");

        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn a_rename_rewrites_the_open_code_tab_as_one_undoable_edit() {
        let dir = std::env::temp_dir().join(format!("lazyrad-ide-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup = dir.clone();

        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let mut session = ProjectSession::create("MyApp", &dir).expect("create");
            // The designer renames the model before its rename sink fires, so
            // the session already holds the new name when the rewrite runs.
            let mut form = session
                .form(crate::project::DEFAULT_FORM)
                .cloned()
                .expect("a form");
            form.insert(lazyrad_project::Node::new("Button", "go_button"));
            session.set_form(crate::project::DEFAULT_FORM, form);
            session.set_code(
                crate::project::DEFAULT_FORM,
                "fn button1_click() {\n}\n".to_owned(),
            );
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.refresh_explorer(ui);
            app.open_document(crate::project::DEFAULT_FORM, DocKind::Code)
                .expect("the code tab opens");

            app.rename_control(crate::project::DEFAULT_FORM, "button1", "go_button", ui);

            let editor = app
                .code_editor(crate::project::DEFAULT_FORM)
                .expect("the editor is open");
            assert_eq!(editor.text(), "fn go_button_click() {\n}\n");
            assert!(
                app.session
                    .as_ref()
                    .and_then(|session| session.code(crate::project::DEFAULT_FORM))
                    .is_some_and(|code| code.contains("go_button_click")),
                "the session's code follows the rename"
            );
            // The rewrite is one range replace, so the editor's history keeps
            // the pre-rename text.
            assert!(editor.undo());
            assert_eq!(editor.text(), "fn button1_click() {\n}\n");

            // Renaming a control that does not exist is a no-op.
            app.rename_control(crate::project::DEFAULT_FORM, "ghost", "still_ghost", ui);
            assert_eq!(editor.text(), "fn button1_click() {\n}\n");
            app
        })
        .expect("the offscreen backend runs to completion");

        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn double_clicking_a_control_opens_its_handler_once() {
        let dir = std::env::temp_dir().join(format!("lazyrad-ide-dblclick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup = dir.clone();

        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let session = ProjectSession::create("MyApp", &dir).expect("create");
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.refresh_explorer(ui);
            app.open_document(crate::project::DEFAULT_FORM, DocKind::Designer)
                .expect("the form opens in a designer");

            // Add a Button through the toolbox, as a user would.
            app.update(
                Msg::Toolbox(ToolboxMsg::Activate(Tool::control("Button"))),
                ui,
            );
            assert!(
                app.session
                    .as_ref()
                    .and_then(|session| session.form(crate::project::DEFAULT_FORM))
                    .is_some_and(|form| form.node("button1").is_some()),
                "the toolbox dropped a button1"
            );

            let form = crate::project::DEFAULT_FORM.to_owned();
            // Double-click the button.
            app.update(
                Msg::OpenDefaultHandler {
                    form: form.clone(),
                    target: Target::Node("button1".to_owned()),
                },
                ui,
            );

            // The code tab is active, the handler is present once, and the
            // caret sits inside its body.
            assert_eq!(
                app.active_code_editor().map(|(name, _)| name),
                Some(form.clone()),
                "the form's code tab is in front"
            );
            let editor = app
                .code_editor(&form)
                .expect("the code tab opened with the handler");
            let text = editor.text();
            assert_eq!(
                text.matches("fn button1_click() {").count(),
                1,
                "text was: {text}"
            );
            let body = text
                .find("fn button1_click() {")
                .expect("the handler is present")
                + "fn button1_click() {".len();
            // The editor's caret is a char offset, so convert the byte index.
            let body = text[..body].chars().count();
            assert_eq!(editor.caret(), body, "the caret is inside the body");

            // A second double-click finds the handler rather than duplicating it.
            app.update(
                Msg::OpenDefaultHandler {
                    form: form.clone(),
                    target: Target::Node("button1".to_owned()),
                },
                ui,
            );
            assert_eq!(
                editor.text().matches("fn button1_click() {").count(),
                1,
                "the handler is not duplicated"
            );

            // Double-clicking the form opens `form_load`, which the default
            // template already declares, so it too is found, not duplicated.
            app.update(
                Msg::OpenDefaultHandler {
                    form: form.clone(),
                    target: Target::Form,
                },
                ui,
            );
            assert_eq!(
                editor.text().matches("fn form_load() {").count(),
                1,
                "text was: {}",
                editor.text()
            );
            app
        })
        .expect("the offscreen backend runs to completion");

        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn a_double_click_message_for_another_form_is_dropped() {
        // A queued double-click for a form that is not open must not open, or
        // edit, the active form's code.
        let dir = std::env::temp_dir().join(format!("lazyrad-ide-ghost-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup = dir.clone();

        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let session = ProjectSession::create("MyApp", &dir).expect("create");
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.refresh_explorer(ui);
            app.open_document(crate::project::DEFAULT_FORM, DocKind::Designer)
                .expect("the form opens in a designer");

            app.update(
                Msg::OpenDefaultHandler {
                    form: "ghost_form".to_owned(),
                    target: Target::Form,
                },
                ui,
            );

            assert!(
                app.active_code_editor().is_none(),
                "no code tab was opened for a missing form"
            );
            assert!(
                app.code_editor(crate::project::DEFAULT_FORM).is_none(),
                "the active form's code was left untouched"
            );
            app
        })
        .expect("the offscreen backend runs to completion");

        let _ = std::fs::remove_dir_all(&cleanup);
    }

    #[test]
    fn deleting_a_control_leaves_its_handlers_in_place() {
        let dir = std::env::temp_dir().join(format!("lazyrad-ide-delete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup = dir.clone();

        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            let mut session = ProjectSession::create("MyApp", &dir).expect("create");
            let mut form = session
                .form(crate::project::DEFAULT_FORM)
                .cloned()
                .expect("a form");
            form.insert(lazyrad_project::Node::new("Button", "button1"));
            session.set_form(crate::project::DEFAULT_FORM, form);
            session.set_code(
                crate::project::DEFAULT_FORM,
                "fn button1_click() {\n}\n".to_owned(),
            );
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.refresh_explorer(ui);
            app.open_document(crate::project::DEFAULT_FORM, DocKind::Designer)
                .expect("the form opens in a designer");

            // Select the control and delete it through the designer's edit path.
            {
                let (_, designer, designer_ui) =
                    app.active_designer().expect("the designer is active");
                assert!(designer.borrow().select_node("button1", &designer_ui));
            }
            app.dispatch_edit(Command::Delete, ui);

            assert!(
                app.session
                    .as_ref()
                    .and_then(|session| session.form(crate::project::DEFAULT_FORM))
                    .is_some_and(|form| form.node("button1").is_none()),
                "the control is gone from the form"
            );
            assert_eq!(
                app.session
                    .as_ref()
                    .and_then(|session| session.code(crate::project::DEFAULT_FORM)),
                Some("fn button1_click() {\n}\n"),
                "deleting the control leaves its handler in place (VB behaviour)"
            );
            app
        })
        .expect("the offscreen backend runs to completion");

        let _ = std::fs::remove_dir_all(&cleanup);
    }
}

#[cfg(test)]
#[path = "exit_criterion_tests.rs"]
mod exit_criterion_tests;
