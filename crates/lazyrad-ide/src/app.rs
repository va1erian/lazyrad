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
    Designer, DesignerMsg, PropertyGrid, PropertyGridMsg, Target, ToolboxMsg, handler_events,
    rename_handlers,
};
use lazyrad_project::Catalog;
use xui_code_editor::{
    Editor, FontConfig, Marker, MarkerKind, Options as EditorOptions, Query, RhaiHighlighter,
};
use xui_core::app::{App, Ui};
use xui_core::arrange::{
    Build, Entry, Handle, IntoEntry, LayoutExt, Mounted, absolute, build, column, combo_box,
    menu_bar, panel, row, tabs,
};
use xui_core::backend::{BackendError, Event, PlatformSpec, Result as UiResult, TimerId, WidgetId};
use xui_core::geometry::Point;
use xui_core::layout::Insets;
use xui_core::units::Px;
use xui_core::widget::{
    ComboBox, Dialog, DialogAction, HasText, Label, ListModel, ListView, Menu, MenuId, MenuScope,
    Panel, Split, Tabs, TreeView,
};
use xui_core::{Dip, Lucide, Rect, dip};

mod frame;

use crate::command::{Command, Dispatcher};
use crate::compile::{self, CodeDiagnostic, CompileScheduler};
use crate::completion::{self, LibraryFn, ScriptCompleter, ScriptContext};
use crate::dialog::ChoiceDialog;
use crate::edit_state::EditAvailability;
use crate::explorer::{DoubleClick, Explorer, ExplorerItem};
use crate::file_dialogs::{self, Asked, FileRequest, InWindowDialogs};
use crate::platform::dialogs;
use crate::procedures::{self, ObjectEntry};
use crate::project::{DEFAULT_PROJECT, ProjectSession};
use crate::run::{self, RunEvent, RunId, RunState};
use crate::settings::{Settings, ThemeChoice};
use crate::start_page::StartPage;
use crate::{make_app, make_exe};

/// The menu bar's height.
const MENU_HEIGHT: Dip = dip(24.0);
/// The toolbar's height.
const TOOLBAR_HEIGHT: Dip = dip(32.0);
/// The status bar's height.
const STATUS_HEIGHT: Dip = dip(22.0);
/// The divider thickness xui's [`Split`] draws, so computed pane sizes are
/// exact.
const DIVIDER: f32 = 5.0;
/// The code view's procedure-combo header height.
const CODE_HEADER: Dip = dip(26.0);
/// The gap around and between the code view's procedure combos.
const CODE_GAP: Dip = dip(12.0);
/// The height of a pane's title strip; the pane's content starts below it.
const PANE_TITLE: Dip = dip(28.0);
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
    /// A window-level chord fired a command. Unlike [`Msg::Command`] it is
    /// dropped while the command is disabled or a prompt is open, since the
    /// shortcut backend sees keys typed into any window, prompts included.
    Shortcut(Command),
    /// Re-derive which Edit commands the active document can act on.
    RefreshEdit,
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
    /// The in-window file dialog was accepted with this path.
    FileChosen(PathBuf),
    /// The in-window file dialog was cancelled.
    FileCancelled,
    /// The Make LazyOS App consent dialog was answered (`true` = Install).
    AppConsent(bool),
    /// The "Run it now?" dialog was answered (`true` = Run).
    AppRun(bool),
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
        _page: Rc<Panel<Msg>>,
    },
    /// The code editor, with the two procedure combos for a form's code.
    Code(Box<CodeView>),
}

/// One open code document: the editor and, for a form, its procedure combos.
struct CodeView {
    /// The page panel the view's layout is mounted in.
    page: Rc<Panel<Msg>>,
    /// The page's layout, mounted again when the procedure combo is rebuilt.
    layout: Mounted<Msg>,
    /// The object combo (the form and its controls). `None` for a module, which
    /// has no events to bind.
    object: Option<Rc<ComboBox<Msg>>>,
    /// The event combo, rebuilt when the object changes.
    procedure: Option<Rc<ComboBox<Msg>>>,
    /// The code editor.
    editor: Rc<Editor<Msg>>,
    /// The object entries the combos are built from.
    objects: Vec<ObjectEntry>,
    /// The index of the selected object, or `None` for a module.
    object_index: Option<usize>,
    /// What the editor's completion sees besides the text: the form's
    /// controls and the project's modules, refreshed after every message.
    completion: Rc<RefCell<ScriptContext>>,
}

impl CodeView {
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

/// A milestone the IDE reports to its host (see [`IdeApp::set_observer`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdeEvent {
    /// A project finished opening; carries its name.
    ProjectOpened(String),
    /// The player was started on the project (Run / F5); carries its name.
    RunStarted(String),
    /// The player process ended, with its exit code when known.
    RunExited(Option<i32>),
    /// A code document changed; carries its item name and new length in
    /// characters (LazyOS's typing-latency benchmark watches this).
    DocumentEdited(String, usize),
    /// The installer reviewed a package; carries its system name and the number
    /// of permissions it will show.
    PackageReviewed(String, usize),
    /// The package was installed (`true`) or only saved (`false`); carries its
    /// system name.
    PackageInstalled(String, bool),
    /// Making or installing the app failed; carries the friendly message.
    PackageFailed(String),
    /// The installed app was started; carries its system name.
    AppLaunched(String),
}

/// A built package waiting for the user's consent.
struct PendingApp {
    built: lazyrad_packager::lzp::BuiltPackage,
    name: String,
}

/// The callback type for [`IdeEvent`]s.
pub type IdeObserver = Rc<dyn Fn(&IdeEvent)>;

/// The IDE shell's application state.
pub struct IdeApp {
    /// Told about project and run milestones (LazyOS prints serial markers).
    observer: Option<IdeObserver>,
    /// The package installer, when the platform has one.
    installer: Option<Rc<dyn lazyrad_packager::lzp::Installer>>,
    /// The author the made apps declare.
    author: String,
    /// The consent or "run it" dialog that is open.
    app_dialog: Option<ChoiceDialog<Msg>>,
    /// The package the consent dialog is about.
    pending_app: Option<PendingApp>,
    /// The in-window file dialogs, built on first use when the platform offers a
    /// filesystem for them.
    file_dialogs: Option<InWindowDialogs>,
    /// What the open in-window file dialog is answering.
    file_request: Option<FileRequest>,
    /// A filesystem and start folder for the in-window dialogs that an embedder
    /// set directly; it wins over the platform's.
    dialog_fs: Option<(Rc<dyn xui_core::widget::FileSystem>, PathBuf)>,
    settings: Settings,
    dispatcher: Dispatcher,
    /// Every menu entry that dispatches a command, for enabling/disabling.
    menu_commands: MenuCommands,
    /// The menu bar and the layout that places it in its slot, both replaced
    /// when the recent list changes, since xui menus cannot be rebuilt in
    /// place.
    menu: Rc<Menu<Msg>>,
    menu_layout: Mounted<Msg>,
    /// The plain panel the menu bar is mounted in.
    menu_slot: Rc<Panel<Msg>>,
    /// The Project Explorer's context menu, shown at a right-clicked row.
    context_menu: Menu<Msg>,
    /// The item the context menu was opened on.
    context_target: Option<String>,

    outer: Rc<Split<Msg>>,
    rest: Rc<Split<Msg>>,
    centre: Rc<Split<Msg>>,
    right: Rc<Split<Msg>>,
    /// The pane cards.
    project_panel: Rc<Panel<Msg>>,
    properties_panel: Rc<Panel<Msg>>,
    /// The control catalog shared by every designer and the property grid.
    catalog: Rc<Catalog>,
    /// The standard library's functions, offered by code completion.
    library: Vec<LibraryFn>,
    /// The property grid, bound to the active designer, and the layout that
    /// places it in the Properties pane. Both are rebuilt when the active form
    /// tab changes, so a closed tab's grid cannot keep editing.
    properties_grid: Option<Rc<PropertyGrid<Msg>>>,
    grid_layout: Option<Mounted<Msg>>,
    /// The form the grid is currently bound to, so a queued grid message for a
    /// closed or inactive form is dropped rather than applied elsewhere.
    grid_form: Option<String>,
    /// The plain panel filling the document area, where the tabs are mounted.
    docs_slot: Rc<Panel<Msg>>,
    /// The document tab container and the layout that places it; `None` only
    /// while it is being rebuilt.
    docs: Option<Rc<Tabs<Msg>>>,
    docs_layout: Option<Mounted<Msg>>,
    /// The Start Page, kept alive across tab rebuilds.
    start_page: Option<StartPage<Msg>>,
    documents: Vec<Document>,
    /// The live editors, so the window's timer tick can reach them.
    editors: Rc<RefCell<Vec<Rc<Editor<Msg>>>>>,

    /// The Project Explorer and the entries its rows name.
    explorer: Explorer,
    tree: Rc<TreeView<Msg>>,
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
    output: Rc<Label<Msg>>,
    output_lines: Vec<String>,
    /// The Error List filling the Output pane below the log line.
    error_list: Rc<ListView<Msg>>,
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
    status: Rc<Label<Msg>>,
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
        // The menu bar, toolbar, the nested panes and the status line, as one
        // layout; the split positions follow from the settings in
        // `layout_frame`.
        let frame = frame::mount(ui)?;
        let (menu, menu_layout, menu_commands) =
            build_menu(&frame.menu_slot, ui, &recent, None, false)?;

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
                // Text selections and the clipboard change without any message,
                // so the Edit menu's enabled state is re-read on each tick.
                Some(Msg::RefreshEdit)
            });
        }
        let compile_timer = ui.set_timer(COMPILE_POLL_MS);

        let mut app = IdeApp {
            settings,
            dispatcher: Dispatcher::new(),
            menu_commands,
            menu,
            menu_layout,
            menu_slot: frame.menu_slot,
            context_menu,
            context_target: None,
            outer: frame.outer,
            rest: frame.rest,
            centre: frame.centre,
            right: frame.right,
            project_panel: frame.project_panel,
            properties_panel: frame.properties_panel,
            catalog: Rc::new(lazyrad_project::lazyrad_catalog()),
            library: Vec::new(),
            properties_grid: None,
            grid_layout: None,
            grid_form: None,
            docs_slot: frame.docs_slot,
            docs: None,
            docs_layout: None,
            start_page: None,
            documents: Vec::new(),
            editors,
            explorer: Explorer::empty(),
            tree: frame.tree,
            double_click: DoubleClick::new(),
            session: None,
            prompt: None,
            save_prompt: None,
            pending: None,
            observer: None,
            file_dialogs: None,
            file_request: None,
            dialog_fs: None,
            installer: None,
            author: String::new(),
            app_dialog: None,
            pending_app: None,
            output: frame.output,
            output_lines: Vec::new(),
            error_list: frame.error_list,
            errors: Vec::new(),
            diagnostics,
            _compile_timer: compile_timer,
            last_find: String::new(),
            last_replace: String::new(),
            replace_query: None,
            status: frame.status,
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

        // Window-level events reach the null node. The layout re-flows the
        // frame on a resize, and the splits then get their stored pane sizes
        // back (a split keeps its first pane's extent as it resizes).
        ui.register_events(WidgetId::NONE, |event| {
            matches!(event, Event::Resize { .. }).then_some(Msg::Relayout)
        });

        // The window's close button goes through Exit, which prompts for
        // unsaved changes and saves the settings before quitting.
        ui.on_close(|| Some(Msg::Command(Command::Exit)));
        Ok(app)
    }

    /// Sets the splits' positions from the stored pane sizes and the window's
    /// current size, then lays the window out again.
    pub fn layout_frame(&mut self, ui: &Ui<Msg>) {
        let dpi = ui.dpi();
        // The outer split's first pane is the toolbox, so its stored size maps
        // straight through. The others store their second pane's size, so the
        // first pane's extent follows from the laid-out node.
        self.outer.set_position(dip(self.settings.panes.toolbox));
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
        match build_menu(
            &self.menu_slot,
            ui,
            &self.settings.recent_projects,
            self.session.as_ref().map(ProjectSession::name),
            self.installer.is_some(),
        ) {
            Ok((menu, layout, commands)) => {
                self.menu_commands = commands;
                self.menu = menu;
                self.menu_layout = layout;
                self.refresh_menu();
            }
            Err(error) => self.log(ui, format!("the menu could not be rebuilt: {error}")),
        }
    }

    /// The code editors' options: the configured monospace family and size.
    /// The editor is a monospace grid, so without a monospace family it would
    /// fall back to the proportional UI font and space its tokens apart.
    /// What completion offers in the code window of project item `name`,
    /// reading the stdlib's functions on first use.
    fn completion_context(&mut self, name: &str) -> ScriptContext {
        if self.library.is_empty() {
            self.library = completion::stdlib_functions(&self.catalog);
        }
        match &self.session {
            Some(session) => completion::context_for(session, name, &self.library),
            None => ScriptContext::default(),
        }
    }

    /// Brings every code window's completion context up to date with the
    /// project: controls drawn, renamed or deleted, modules added or edited.
    fn refresh_completion(&self) {
        let Some(session) = &self.session else {
            return;
        };
        for document in &self.documents {
            if let DocumentView::Code(view) = &document.view {
                *view.completion.borrow_mut() =
                    completion::context_for(session, &document.name, &self.library);
            }
        }
    }

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
                if let Asked::Now(Some(file)) = self.ask_file(FileRequest::OpenProject, ui) {
                    self.file_chosen(FileRequest::OpenProject, file, ui);
                }
            }
            Command::OpenRecent(index) => {
                if let Some(dir) = self.settings.recent_projects.get(index).cloned() {
                    self.open_replacing(dir, ui);
                }
            }
            Command::Save | Command::SaveAll => self.save_project(ui),
            Command::SaveAs => self.save_project_as(ui),
            Command::MakeExe => self.make_exe(ui),
            Command::MakeApp => self.make_app(ui),
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
        if !self.check_for_run(&dir, "the program was not started", ui) {
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
                self.notify(&IdeEvent::RunStarted(name.clone()));
            }
            Err(error) => self.log(ui, format!("The program could not start: {error}")),
        }
    }

    /// Makes the project into a self-contained executable (File → Make
    /// `<Project>`.exe…): saves, runs the same whole-project check as a run and
    /// refuses on any problem, then asks where to write the file and exports.
    fn make_exe(&mut self, ui: &mut Ui<Msg>) {
        let Some((dir, name)) = self
            .session
            .as_ref()
            .map(|session| (session.dir().to_path_buf(), session.name().to_owned()))
        else {
            self.log(ui, "Open a project before making an executable.");
            return;
        };

        if let Err(error) = self.save_for_run(ui) {
            self.log(
                ui,
                format!("Make cancelled: the project could not be saved ({error})."),
            );
            return;
        }
        if !self.check_for_run(&dir, "the executable was not made", ui) {
            return;
        }
        if let Err(error) = run::resolve_player(self.settings.player_path.as_deref()) {
            self.log(ui, error.to_string());
            return;
        }
        let request = FileRequest::MakeExe(make_exe::suggested_file_name(&name));
        if let Asked::Now(Some(output)) = self.ask_file(request.clone(), ui) {
            self.file_chosen(request, output, ui);
        }
    }

    /// Exports the (already saved and checked) project to `output`.
    fn write_exe(&mut self, output: PathBuf, ui: &mut Ui<Msg>) {
        let Some(project_file) = self.session.as_ref().map(ProjectSession::project_file) else {
            return;
        };
        let stub = match run::resolve_player(self.settings.player_path.as_deref()) {
            Ok(stub) => stub,
            Err(error) => {
                self.log(ui, error.to_string());
                return;
            }
        };
        match make_exe::export_project(&project_file, &stub, &output) {
            Ok(report) => self.log(
                ui,
                format!("Made {} ({} bytes).", report.output.display(), report.bytes),
            ),
            Err(error) => self.log(ui, format!("Make failed: {error}")),
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
    fn check_for_run(&mut self, dir: &Path, refusal: &str, ui: &mut Ui<Msg>) -> bool {
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
        let errors = problems.iter().filter(|problem| !problem.warning).count();
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
        if errors == 0 {
            // Only lint warnings: show them, but let the project run.
            self.log(ui, format!("{} warning(s).", problems.len()));
            return true;
        }
        self.log(ui, format!("{errors} error(s): {refusal}."));
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
        if let RunEvent::Exited(code) = &event {
            self.notify(&IdeEvent::RunExited(*code));
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
        let request = FileRequest::NewProject(name.to_owned());
        if let Asked::Now(Some(dir)) = self.ask_file(request.clone(), ui) {
            self.file_chosen(request, dir, ui);
        }
    }

    /// Creates the project `name` in `dir` (made when missing) and opens it.
    fn create_project(&mut self, name: &str, dir: PathBuf, ui: &mut Ui<Msg>) {
        if let Err(error) = std::fs::create_dir_all(&dir) {
            self.log(ui, format!("The project could not be created: {error}"));
            return;
        }
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

    /// Makes the IDE ask for files with the in-window dialog over `fs`, starting
    /// in `start_dir`, instead of the platform's blocking dialogs.
    pub fn set_file_system(
        &mut self,
        fs: Rc<dyn xui_core::widget::FileSystem>,
        start_dir: PathBuf,
    ) {
        self.dialog_fs = Some((fs, start_dir));
        self.file_dialogs = None;
    }

    /// Whether an in-window file dialog is showing.
    pub fn file_dialog_open(&self) -> bool {
        self.file_dialogs
            .as_ref()
            .is_some_and(InWindowDialogs::is_open)
    }

    /// Replaces how the player is started (LazyOS installs a launcher that polls
    /// its pipes on the window timer instead of using reader threads).
    pub fn set_launcher(&mut self, launcher: Rc<dyn run::Launcher>) {
        self.launcher = launcher;
    }

    /// Builds the in-window file dialogs when the platform (or an embedder) offers
    /// a filesystem for them. The embedder calls this once at start-up, so the
    /// dialog widgets exist before the first event rather than being created in
    /// the middle of one.
    pub fn prepare_file_dialogs(&mut self, ui: &mut Ui<Msg>) {
        let platform = lazyrad_runtime::platform::current();
        let source = self.dialog_fs.clone().or_else(|| {
            platform
                .file_system()
                .map(|fs| (fs, platform.projects_dir()))
        });
        let Some((fs, start_dir)) = source else {
            return;
        };
        match InWindowDialogs::new(ui, fs, start_dir) {
            Ok(dialogs) => self.file_dialogs = Some(dialogs),
            Err(error) => self.log(ui, format!("The file dialog could not be built: {error}")),
        }
    }

    /// Asks for a file for `request`: through the blocking OS dialog (answered
    /// now) or, when the platform offers a filesystem, through the in-window
    /// dialog (answered later by [`Msg::FileChosen`]).
    fn ask_file(&mut self, request: FileRequest, ui: &mut Ui<Msg>) -> Asked {
        if self.file_dialogs.is_none() {
            self.prepare_file_dialogs(ui);
        }
        if let Some(dialogs) = &self.file_dialogs {
            // One question at a time: a second request would replace the first
            // one's `file_request`, and its answer would then be applied to the
            // wrong action (an export written over a chosen `.lrp`).
            if dialogs.is_open() && self.file_request.is_some() {
                return Asked::Later;
            }
            dialogs.show(&request);
            self.file_request = Some(request);
            return Asked::Later;
        }
        Asked::Now(file_dialogs::ask_blocking(&request))
    }

    /// Continues what `request` was asking a file for, now that it has `path`.
    fn file_chosen(&mut self, request: FileRequest, path: PathBuf, ui: &mut Ui<Msg>) {
        match request {
            FileRequest::OpenProject => self.open_replacing(dialogs::containing_folder(&path), ui),
            FileRequest::NewProject(name) => self.create_project(&name, path, ui),
            FileRequest::SaveProjectAs(_) => self.write_project_as(path, ui),
            FileRequest::MakeExe(_) => self.write_exe(path, ui),
        }
    }

    /// Gives the IDE a package installer: File then offers "Make LazyOS App…".
    /// `author` is who the made apps declare; empty uses the `USER` or
    /// `USERNAME` environment variable, else "unknown".
    pub fn set_installer(
        &mut self,
        installer: Rc<dyn lazyrad_packager::lzp::Installer>,
        author: &str,
        ui: &Ui<Msg>,
    ) {
        self.installer = Some(installer);
        self.author = if author.is_empty() {
            ["USER", "USERNAME"]
                .iter()
                .find_map(|key| std::env::var(key).ok())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "unknown".to_owned())
        } else {
            author.to_owned()
        };
        self.rebuild_menu(ui);
    }

    /// Whether File shows Make LazyOS App.
    pub fn can_make_app(&self) -> bool {
        self.installer.is_some()
    }

    /// File → Make LazyOS App…: save, check, build the `.lzp`, ask the installer
    /// what it would grant and show that for consent. Nothing is installed
    /// until the user says so ([`Msg::AppConsent`]).
    fn make_app(&mut self, ui: &mut Ui<Msg>) {
        let Some(installer) = self.installer.clone() else {
            self.log(ui, "This system cannot install LazyOS apps.");
            return;
        };
        let Some((dir, project_file)) = self
            .session
            .as_ref()
            .map(|session| (session.dir().to_path_buf(), session.project_file()))
        else {
            self.log(ui, "Open a project before making an app.");
            return;
        };
        if let Err(error) = self.save_for_run(ui) {
            self.log(
                ui,
                format!("Make cancelled: the project could not be saved ({error})."),
            );
            return;
        }
        if !self.check_for_run(&dir, "the app was not made", ui) {
            return;
        }
        let built = match make_app::build(
            &project_file,
            self.settings.player_path.as_deref(),
            &self.author,
        ) {
            Ok(built) => built,
            Err(error) => {
                self.package_failed(format!("Make failed: {error}"), ui);
                return;
            }
        };
        let name = self
            .session
            .as_ref()
            .map_or_else(String::new, |session| session.name().to_owned());
        match installer.review(&built) {
            Err(error) => self.package_failed(error.to_string(), ui),
            Ok(None) => {
                self.pending_app = Some(PendingApp { built, name });
                self.resolve_consent(true, ui);
            }
            Ok(Some(review)) if !review.problems.is_empty() => {
                for line in make_app::problem_lines(&review) {
                    self.log(ui, line);
                }
                self.package_failed("The installer refused the package.".to_owned(), ui);
            }
            Ok(Some(review)) => {
                self.notify(&IdeEvent::PackageReviewed(
                    review.system_name.clone(),
                    review.permissions.len(),
                ));
                self.show_app_dialog(
                    &make_app::consent_title(&review),
                    &make_app::consent_text(&review),
                    ["Install", "Cancel"],
                    |index| Some(Msg::AppConsent(index == 0)),
                    ui,
                );
                self.pending_app = Some(PendingApp { built, name });
            }
        }
    }

    /// Opens a two-button dialog; Enter picks the first button, Escape the second.
    fn show_app_dialog(
        &mut self,
        title: &str,
        message: &str,
        labels: [&str; 2],
        mapper: impl Fn(usize) -> Option<Msg> + 'static,
        ui: &mut Ui<Msg>,
    ) {
        match ChoiceDialog::new(ui, title, message, &labels, 0, 1) {
            Ok(dialog) => {
                let dialog = dialog.on_action(mapper);
                dialog.open();
                self.app_dialog = Some(dialog);
            }
            Err(error) => self.log(ui, format!("the dialog could not open: {error}")),
        }
    }

    /// Logs a failure and tells the observer.
    fn package_failed(&mut self, message: String, ui: &mut Ui<Msg>) {
        self.log(ui, message.clone());
        self.notify(&IdeEvent::PackageFailed(message));
    }

    /// Applies the answer to the consent dialog.
    fn resolve_consent(&mut self, install: bool, ui: &mut Ui<Msg>) {
        self.app_dialog = None;
        let (Some(pending), Some(installer)) = (self.pending_app.take(), self.installer.clone())
        else {
            return;
        };
        if !install {
            self.log(ui, "Install cancelled.");
            return;
        }
        match installer.install(&pending.built) {
            Err(error) => self.package_failed(error.to_string(), ui),
            Ok(app) => {
                self.log(ui, app.summary());
                let installed = app.state == lazyrad_packager::lzp::InstallState::Installed;
                self.notify(&IdeEvent::PackageInstalled(
                    app.system_name.clone(),
                    installed,
                ));
                if installed {
                    let system_name = app.system_name.clone();
                    self.pending_app = Some(PendingApp {
                        built: pending.built,
                        name: pending.name.clone(),
                    });
                    self.show_app_dialog(
                        &format!("{} is installed", pending.name),
                        &format!("Run it now? It is also in the Start menu as {system_name}."),
                        ["Run", "Not now"],
                        |index| Some(Msg::AppRun(index == 0)),
                        ui,
                    );
                }
            }
        }
    }

    /// Applies the answer to the "run it now" dialog.
    fn resolve_run_offer(&mut self, run: bool, ui: &mut Ui<Msg>) {
        self.app_dialog = None;
        let (Some(pending), Some(installer)) = (self.pending_app.take(), self.installer.clone())
        else {
            return;
        };
        if !run {
            return;
        }
        let system_name = pending.built.system_name;
        match installer.launch(&system_name) {
            Ok(()) => {
                self.log(ui, format!("Started {system_name}."));
                self.notify(&IdeEvent::AppLaunched(system_name));
            }
            Err(error) => self.package_failed(error.to_string(), ui),
        }
    }

    /// Installs the observer told about [`IdeEvent`]s.
    pub fn set_observer(&mut self, observer: IdeObserver) {
        self.observer = Some(observer);
    }

    /// Tells the observer, if any.
    fn notify(&self, event: &IdeEvent) {
        if let Some(observer) = &self.observer {
            observer(event);
        }
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
    pub(crate) fn open_dir(&mut self, dir: PathBuf, ui: &mut Ui<Msg>) {
        match ProjectSession::open(&dir) {
            Ok(session) => {
                let name = session.name().to_owned();
                self.adopt_project(session, ui);
                self.notify(&IdeEvent::ProjectOpened(name));
            }
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
        let request = FileRequest::SaveProjectAs(session.project().file_name());
        if let Asked::Now(Some(file)) = self.ask_file(request.clone(), ui) {
            self.file_chosen(request, file, ui);
        }
    }

    /// Saves the project (already synced) to the chosen `.lrp` `file`.
    fn write_project_as(&mut self, file: PathBuf, ui: &mut Ui<Msg>) {
        // The answer arrives later from the in-window dialog: the project may
        // have been closed in between.
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let result = session.save_as(&file);
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
        self.rebuild_menu(ui);
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
        self.drop_grid();
        self.grid_form = active_name;
        let Some((name, designer, _)) = active else {
            return;
        };
        let catalog = Rc::clone(&self.catalog);
        let grid = Handle::new();
        let build_grid = build(move |ui| {
            PropertyGrid::new(ui, Rect::default(), designer, catalog, move |msg| {
                Msg::PropertyGrid {
                    form: name.clone(),
                    msg,
                }
            })
            .map_err(|error| BackendError::Other(error.to_string()))
        })
        .bind(&grid);
        // The grid fills the Properties pane below its title.
        let panel = &self.properties_panel;
        match panel.ui().mount_in(
            panel.id(),
            column()
                .padding(Insets::new(dip(0.0), PANE_TITLE, dip(0.0), dip(0.0)))
                .child(build_grid.fill(1)),
        ) {
            Ok(layout) => {
                self.properties_grid = Some(grid.get());
                self.grid_layout = Some(layout);
            }
            Err(error) => self.log(ui, format!("the property grid could not be built: {error}")),
        }
    }

    /// Drops the property grid and the layout that places it, releasing the
    /// designer the grid holds.
    fn drop_grid(&mut self) {
        self.grid_layout = None;
        self.properties_grid = None;
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

    /// Runs a window-level chord's command, if it may run now.
    ///
    /// The shortcut backend also sees keys typed into the prompt dialogs' own
    /// windows, so nothing fires while one is open; a disabled command (Run →
    /// End with nothing running) is ignored like its greyed-out menu entry.
    fn run_shortcut(&mut self, command: Command, ui: &mut Ui<Msg>) {
        if self.prompt.is_some() || self.save_prompt.is_some() {
            return;
        }
        if !self.dispatcher.is_enabled(command) {
            return;
        }
        self.run_command(command, ui);
    }

    /// What the active document can do for each Edit command, right now.
    ///
    /// The Start Page and an empty window have no document, so nothing is
    /// available there.
    fn edit_availability(&self) -> EditAvailability {
        if let Some((_, designer, _)) = self.active_designer() {
            let designer = designer.borrow();
            let selected = designer.has_selection();
            return EditAvailability {
                undo: designer.can_undo(),
                redo: designer.can_redo(),
                cut: selected,
                copy: selected,
                paste: designer.can_paste(),
                delete: selected,
                select_all: designer.has_controls(),
            };
        }
        if let Some((_, editor)) = self.active_code_editor() {
            let has_text = !editor.is_empty();
            return EditAvailability {
                undo: editor.can_undo(),
                redo: editor.can_redo(),
                // Cut and Copy take the caret's line when nothing is selected.
                cut: has_text,
                copy: has_text,
                paste: editor.can_paste(),
                delete: editor.selection().is_some(),
                select_all: has_text,
            };
        }
        EditAvailability::NONE
    }

    /// Re-reads [`IdeApp::edit_availability`] into the Edit menu.
    fn refresh_edit_availability(&mut self) {
        let availability = self.edit_availability();
        if self.dispatcher.set_edit_availability(availability) {
            self.refresh_menu();
        }
    }

    /// Routes Edit menu commands to the active designer, or to the active code
    /// editor when a code tab is in front.
    ///
    /// Choosing a menu entry or a toolbar button moves focus off the document,
    /// so it is handed back first: the user carries on typing where they were.
    fn dispatch_edit(&mut self, command: Command, ui: &mut Ui<Msg>) {
        if let Some((name, designer, designer_ui)) = self.active_designer() {
            designer_ui.focus(designer.borrow().id());
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
            self.refresh_edit_availability();
            return;
        }

        let Some((name, editor)) = self.active_code_editor() else {
            return;
        };
        editor.focus();
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
        self.refresh_edit_availability();
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
    /// xui's [`ComboBox`] cannot replace its items, so the code page is mounted
    /// again with a new procedure combo and the same editor and object combo.
    fn change_object(&mut self, name: &str, index: usize, ui: &mut Ui<Msg>) {
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
            let items: Vec<String> = entry
                .events
                .iter()
                .map(|event| event.name.clone())
                .collect();
            let procedure = Handle::new();
            let combos = view.object.clone().map(|object| {
                (
                    build(move |_| Ok(object)).into_entry(),
                    procedure_combo(name, &items).bind(&procedure).into_entry(),
                )
            });
            let editor = Rc::clone(&view.editor);
            match mount_code_page(&view.page, combos, build(move |_| Ok(editor)).into_entry()) {
                Ok(layout) => {
                    view.layout = layout;
                    view.procedure = Some(procedure.get());
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
        // `at` is node-local, and `ui.bounds` is relative to the parent, so
        // walk up the split nest to the window, which `show_context` expects.
        let point = self.tree_point_in_window(at, ui);
        self.context_menu.show_context(point.x, point.y);
    }

    /// The node-local point `at` in the tree, in window (client) coordinates.
    ///
    /// Every node's bounds are relative to its parent, so the window origin of
    /// the tree is the sum of the bounds' origins up the chain: the tree, its
    /// pane card, then the splits that nest it (its own column, the centre
    /// row, the rest column) and the outer row, which sits at the window. A
    /// split holds each pane's layout in a panel of its own at the pane's
    /// rectangle: the first pane's starts at the split's origin, the second's
    /// ends at its far edge, so its offset is the split's extent less the
    /// pane's.
    fn tree_point_in_window(&self, at: Point, ui: &Ui<Msg>) -> Point {
        let origin = |id: WidgetId| {
            let bounds = ui.bounds(id);
            Point::new(bounds.left, bounds.top)
        };
        // The second pane's panel within a row split, which `pane` fills.
        let second_pane = |split: WidgetId, pane: WidgetId| {
            Point::new(ui.bounds(split).width() - ui.bounds(pane).width(), 0)
        };
        let offsets = [
            origin(self.tree.id()),
            origin(self.project_panel.id()),
            origin(self.right.id()),
            second_pane(self.centre.id(), self.right.id()),
            origin(self.centre.id()),
            origin(self.rest.id()),
            second_pane(self.outer.id(), self.rest.id()),
            origin(self.outer.id()),
        ];
        offsets.iter().fold(at, |point, offset| {
            Point::new(point.x + offset.x, point.y + offset.y)
        })
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
        let docs_ui = self.docs_slot.ui().clone();
        if let Some(index) = self
            .documents
            .iter()
            .position(|document| document.name == name && document.kind == kind)
        {
            if let Some(docs) = &self.docs {
                // Page 0 is the Start Page.
                docs.select(index + 1);
            }
            // `select` raises no change message, so rebind the grid here.
            self.refresh_property_grid(&docs_ui);
            return Ok(());
        }

        // The tabs can be missing if rebuilding them failed earlier; report it
        // instead of panicking, since every caller logs this error.
        let Some(tabs) = self.docs.clone() else {
            return Err(BackendError::Other(
                "the document tabs are not available".to_owned(),
            ));
        };
        let pages = tabs.page_count();
        let (view, title) = match self.build_view(&tabs, name, kind) {
            Ok(built) => built,
            Err(error) => {
                // A page the failed build added has no document behind it.
                if tabs.page_count() > pages {
                    tabs.remove_page(pages);
                }
                return Err(error);
            }
        };
        self.documents.push(Document {
            name: name.to_owned(),
            kind,
            title,
            dirty: false,
            view,
        });
        // Bring the new document to the front (page 0 is the Start Page).
        tabs.select(self.documents.len());
        self.refresh_property_grid(&docs_ui);
        Ok(())
    }

    /// Builds the widgets behind a document in a new page of `tabs`,
    /// returning the view and the tab title.
    fn build_view(
        &mut self,
        tabs: &Tabs<Msg>,
        name: &str,
        kind: DocKind,
    ) -> UiResult<(DocumentView, String)> {
        // Every page is a panel filling the page: a card for a designer, which
        // scopes the designer's design mode to the document area (leaving the
        // tab strip and the other panes live), and a plain one for code. The
        // document's widgets are parented to it, so the tab shows and hides
        // them with the page.
        let page = Handle::new();
        let title = match kind {
            DocKind::Designer => name.to_owned(),
            DocKind::Code => format!("{name}.rhai"),
        };
        let card = match kind {
            DocKind::Designer => panel(absolute()),
            DocKind::Code => panel(absolute()).plain(),
        };
        tabs.add_layout_page(&title, column().child(card.bind(&page).fill(1)))?;
        let page = page.get();
        match kind {
            DocKind::Designer => {
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
                Ok((
                    DocumentView::Designer {
                        designer: Rc::new(RefCell::new(designer)),
                        designer_ui,
                        _page: page,
                    },
                    title,
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

                let object = Handle::new();
                let procedure = Handle::new();
                let combos = if is_form {
                    let items: Vec<&str> =
                        objects.iter().map(|entry| entry.label.as_str()).collect();
                    let count = items.len();
                    let document = name.to_owned();
                    let object_combo = combo_box(&items)
                        .then(move |combo| {
                            with_icons(combo, count, Lucide::Box).on_select(move |index| {
                                Some(Msg::ObjectChanged(document.clone(), index))
                            })
                        })
                        .bind(&object);
                    let events = objects
                        .first()
                        .map(ObjectEntry::event_names)
                        .unwrap_or_default();
                    Some((
                        object_combo.into_entry(),
                        procedure_combo(name, &events).bind(&procedure).into_entry(),
                    ))
                } else {
                    None
                };

                let options = self.editor_options();
                let document = name.to_owned();
                let editor = Handle::new();
                let completion = Rc::new(RefCell::new(self.completion_context(name)));
                let completer =
                    ScriptCompleter::new(Rc::clone(&self.catalog), Rc::clone(&completion));
                let editor_build = build(move |ui| {
                    Ok(Editor::with_options(ui, Rect::default(), options)?
                        .with_highlighter(RhaiHighlighter)
                        .with_completer(completer)
                        .on_change(move |text| {
                            Some(Msg::DocumentEdited(document.clone(), text.to_string()))
                        }))
                })
                .bind(&editor);
                let layout = mount_code_page(&page, combos, editor_build.into_entry())?;
                let editor = editor.get();
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

                let view = CodeView {
                    page,
                    layout,
                    object: object.try_get(),
                    procedure: procedure.try_get(),
                    editor,
                    objects,
                    object_index: is_form.then_some(0),
                    completion,
                };
                Ok((DocumentView::Code(Box::new(view)), title))
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
        self.drop_grid();
        self.grid_form = None;
        self.documents.clear();
        self.errors.clear();
        self.error_list.set_items(&[]);
        self.rebuild_tabs()
    }

    /// Rebuilds the document tabs and their widgets.
    ///
    /// Removing a page from xui's [`Tabs`] keeps its layout's widgets alive
    /// until the container goes, so a rebuild mounts a new container and
    /// recreates the open documents from the session. The Start Page is always
    /// first.
    fn rebuild_tabs(&mut self) -> UiResult<()> {
        let open: Vec<(String, DocKind)> = self
            .documents
            .iter()
            .map(|document| (document.name.clone(), document.kind))
            .collect();
        self.editors.borrow_mut().clear();
        // The grid holds a designer alive; drop it before the documents so a
        // closed tab's designer is really dropped.
        self.drop_grid();
        self.grid_form = None;
        self.documents.clear();
        self.docs = None;
        self.docs_layout = None;
        self.start_page = None;

        let container = Handle::new();
        let slot = &self.docs_slot;
        let layout = slot.ui().mount_in(
            slot.id(),
            column().child(tabs().on_change(Msg::TabChanged).bind(&container).fill(1)),
        )?;
        let tabs = container.get();
        let start_page = StartPage::new(tabs.ui(), WELCOME, &self.editor_options().font)?;
        tabs.add_page("Start Page", &[start_page.id()]);
        self.start_page = Some(start_page);
        self.docs = Some(tabs);
        self.docs_layout = Some(layout);

        for (name, kind) in open {
            self.open_document(&name, kind)?;
        }

        // The selected page's designer may have changed; rebind it.
        let docs_ui = self.docs_slot.ui().clone();
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
        // Neither the edit-availability timer nor typing in a form's script
        // changes what completion reads besides the text (a module's exports
        // do change as it is typed in).
        let idle = match &msg {
            Msg::RefreshEdit => true,
            Msg::DocumentEdited(name, _) => self
                .session
                .as_ref()
                .is_some_and(|session| session.form(name).is_some()),
            _ => false,
        };
        match msg {
            Msg::Command(command) => self.run_command(command, ui),
            Msg::Shortcut(command) => self.run_shortcut(command, ui),
            Msg::RefreshEdit => {
                // The window timer also gives a polling launcher (LazyOS) its turn.
                self.run.poll();
                self.refresh_edit_availability();
            }
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
                if self.observer.is_some() {
                    self.notify(&IdeEvent::DocumentEdited(
                        name.clone(),
                        text.chars().count(),
                    ));
                }
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
                self.refresh_property_grid(ui);
                self.layout_frame(ui);
                self.refresh_edit_availability();
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
            Msg::FileChosen(path) => {
                if let Some(request) = self.file_request.take() {
                    self.file_chosen(request, path, ui);
                }
            }
            Msg::FileCancelled => self.file_request = None,
            Msg::AppConsent(install) => self.resolve_consent(install, ui),
            Msg::AppRun(run) => self.resolve_run_offer(run, ui),
        }
        if !idle {
            self.refresh_completion();
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

/// Builds the menu bar in `slot`, returning it, the layout that places it (which
/// destroys it when dropped) and the map from each entry's [`MenuId`] to its
/// command.
fn build_menu(
    slot: &Panel<Msg>,
    ui: &Ui<Msg>,
    recent: &[PathBuf],
    project_name: Option<&str>,
    make_app_item: bool,
) -> UiResult<(Rc<Menu<Msg>>, Mounted<Msg>, MenuCommands)> {
    let recent = recent.to_vec();
    let make_exe_label = make_exe::menu_label(project_name);
    // The menu is filled when the layout is mounted; the command entries it
    // hands out come back through this cell, which the selection reads.
    let map: Rc<RefCell<Vec<(MenuId, Command)>>> = Rc::default();
    let filled = Rc::clone(&map);
    let fill = move |bar: &mut MenuScope<'_>| {
        let mut ids = MenuIds::new();
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
                .separator()
                .command(&mut ids, Command::MakeExe, &make_exe_label)
                .separator();
            if make_app_item {
                file.command(&mut ids, Command::MakeApp, make_app::MENU_LABEL);
            }
            file.separator()
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
        *filled.borrow_mut() = ids.map;
    };
    let chosen = Rc::clone(&map);
    let handle = Handle::new();
    let bar = menu_bar(fill)
        .on_select_with(move |id| {
            chosen
                .borrow()
                .iter()
                .find(|(entry, _)| *entry == id)
                .map(|(_, command)| Msg::Command(*command))
        })
        .bind(&handle);
    let layout = ui.mount_in(slot.id(), column().child(bar.fill(1)))?;
    let commands: MenuCommands = Rc::new(map.borrow().clone());
    Ok((handle.get(), layout, commands))
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

/// The procedure combo of `document`'s code page over `events`, each marked
/// with the event icon.
fn procedure_combo(document: &str, events: &[String]) -> Build<ComboBox<Msg>, Msg> {
    let items: Vec<&str> = events.iter().map(String::as_str).collect();
    let count = items.len();
    let document = document.to_owned();
    combo_box(&items).then(move |combo| {
        with_icons(combo, count, Lucide::Zap)
            .on_select(move |index| Some(Msg::ProcedureChanged(document.clone(), index)))
    })
}

/// Mounts a code page in `page`: for a form, the object and procedure combos
/// in a row above the editor; for a module, the editor alone.
fn mount_code_page(
    page: &Panel<Msg>,
    combos: Option<(Entry<Msg>, Entry<Msg>)>,
    editor: Entry<Msg>,
) -> UiResult<Mounted<Msg>> {
    let mut layout = column();
    if let Some((object, procedure)) = combos {
        layout = layout.child(
            row()
                .padding(Insets::new(CODE_GAP, dip(0.0), CODE_GAP, dip(0.0)))
                .gap(CODE_GAP)
                .children((object.fill(1), procedure.fill(1)))
                .height(CODE_HEADER),
        );
    }
    page.ui().mount_in(page.id(), layout.child(editor.fill(1)))
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
        self.item(ids.id(command), &command.label_with_shortcut(text));
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

/// Maps a key-down event to the window-level command it triggers, for the
/// shortcut backend.
///
/// Auto-repeat and system (Alt) combinations are ignored, and so are the Edit
/// chords (Ctrl+Z/Y/X/C/V/A, Delete), which the focused widget handles itself.
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
        // Edit chords are not intercepted: the focused widget handles them.
        return Command::from_global_keydown(*key, *modifiers).map(Msg::Shortcut);
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
            Some(Msg::Shortcut(Command::Save))
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

    /// An installer that records what it was asked and answers from a script.
    struct MockInstaller {
        review: Option<lazyrad_packager::lzp::PackageReview>,
        installs: std::cell::Cell<u32>,
        launches: RefCell<Vec<String>>,
    }

    impl lazyrad_packager::lzp::Installer for MockInstaller {
        fn review(
            &self,
            _: &lazyrad_packager::lzp::BuiltPackage,
        ) -> Result<Option<lazyrad_packager::lzp::PackageReview>, lazyrad_packager::lzp::InstallError>
        {
            Ok(self.review.clone())
        }

        fn install(
            &self,
            package: &lazyrad_packager::lzp::BuiltPackage,
        ) -> Result<lazyrad_packager::lzp::InstalledApp, lazyrad_packager::lzp::InstallError>
        {
            self.installs.set(self.installs.get() + 1);
            Ok(lazyrad_packager::lzp::InstalledApp {
                system_name: package.system_name.clone(),
                version: package.version.clone(),
                state: lazyrad_packager::lzp::InstallState::Installed,
                location: None,
            })
        }

        fn launch(&self, system_name: &str) -> Result<(), lazyrad_packager::lzp::InstallError> {
            self.launches.borrow_mut().push(system_name.to_owned());
            Ok(())
        }
    }

    /// A player file that passes the ELF check.
    fn elf_player(dir: &Path) -> PathBuf {
        let mut elf = vec![0u8; 256];
        elf[..4].copy_from_slice(b"ELF");
        elf[4] = 2;
        elf[5] = 1;
        elf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes());
        let path = dir.join("player.elf");
        std::fs::write(&path, elf).expect("write the player");
        path
    }

    fn review_for(
        system_name: &str,
        problems: Vec<String>,
    ) -> lazyrad_packager::lzp::PackageReview {
        lazyrad_packager::lzp::PackageReview {
            name: "MyApp".into(),
            system_name: system_name.into(),
            author: "Ada".into(),
            version: "0.1.0".into(),
            permissions: vec![lazyrad_packager::lzp::PermissionNote {
                kind: "interface".into(),
                value: "os.lazy.display.v1".into(),
                risk: "low".into(),
                explanation: "Show windows".into(),
            }],
            problems,
        }
    }

    #[test]
    fn make_app_asks_for_consent_installs_then_offers_to_run() {
        let dir = run_scratch("makeapp");
        let cleanup = dir.clone();
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            assert!(!app.can_make_app(), "no installer, no menu item");
            let session = ProjectSession::create("MyApp", &dir).expect("create");
            app.session = Some(session);
            app.dispatcher.set_project_open(true);
            app.settings.player_path = Some(elf_player(&dir));
            let installer = Rc::new(MockInstaller {
                review: Some(review_for("user.ada.myapp", Vec::new())),
                installs: std::cell::Cell::new(0),
                launches: RefCell::new(Vec::new()),
            });
            let events: Rc<RefCell<Vec<IdeEvent>>> = Rc::new(RefCell::new(Vec::new()));
            let sink = Rc::clone(&events);
            app.set_observer(Rc::new(move |event| sink.borrow_mut().push(event.clone())));
            app.set_installer(installer.clone(), "Ada", ui);
            assert!(app.can_make_app());

            // Make shows the consent dialog and installs nothing yet.
            app.update(Msg::Command(Command::MakeApp), ui);
            assert!(app.app_dialog.is_some() && app.pending_app.is_some());
            assert_eq!(
                installer.installs.get(),
                0,
                "nothing installed before consent"
            );

            // Cancel: still nothing installed, the pending package is dropped.
            app.update(Msg::AppConsent(false), ui);
            assert_eq!(installer.installs.get(), 0);
            assert!(app.pending_app.is_none());

            // Make again and accept: installed, and Run is offered.
            app.update(Msg::Command(Command::MakeApp), ui);
            app.update(Msg::AppConsent(true), ui);
            assert_eq!(installer.installs.get(), 1);
            assert!(app.app_dialog.is_some(), "the run offer is showing");
            app.update(Msg::AppRun(true), ui);
            assert_eq!(*installer.launches.borrow(), ["user.ada.myapp"]);

            let events = events.borrow();
            assert!(events.contains(&IdeEvent::PackageReviewed("user.ada.myapp".into(), 1)));
            assert!(events.contains(&IdeEvent::PackageInstalled("user.ada.myapp".into(), true)));
            assert!(events.contains(&IdeEvent::AppLaunched("user.ada.myapp".into())));
            ui.quit();
            drop(events);
            app
        })
        .expect("the window runs");
        let _ = std::fs::remove_dir_all(cleanup);
    }

    #[test]
    fn make_app_stops_when_the_installer_refuses_the_package() {
        let dir = run_scratch("makeapp-refused");
        let cleanup = dir.clone();
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            app.session = Some(ProjectSession::create("MyApp", &dir).expect("create"));
            app.dispatcher.set_project_open(true);
            app.settings.player_path = Some(elf_player(&dir));
            let installer = Rc::new(MockInstaller {
                review: Some(review_for("user.ada.myapp", vec!["bad manifest".into()])),
                installs: std::cell::Cell::new(0),
                launches: RefCell::new(Vec::new()),
            });
            app.set_installer(installer.clone(), "Ada", ui);
            app.update(Msg::Command(Command::MakeApp), ui);
            assert!(app.app_dialog.is_none(), "no consent for a refused package");
            assert_eq!(installer.installs.get(), 0);
            assert!(app.output_lines.iter().any(|l| l.contains("bad manifest")));
            ui.quit();
            app
        })
        .expect("the window runs");
        let _ = std::fs::remove_dir_all(cleanup);
    }

    #[test]
    fn a_second_file_request_does_not_replace_the_one_being_answered() {
        let dir = run_scratch("onedialog");
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        let start = dir.clone();
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            app.set_file_system(Rc::new(xui_core::widget::StdFileSystem), start.clone());
            app.update(Msg::Command(Command::OpenProject), ui);
            assert_eq!(app.file_request, Some(FileRequest::OpenProject));

            // Another menu command while the dialog is up must not retarget it:
            // its answer would be applied to the wrong action.
            let asked = app.ask_file(FileRequest::MakeExe("x.exe".into()), ui);
            assert!(matches!(asked, Asked::Later));
            assert_eq!(app.file_request, Some(FileRequest::OpenProject));
            ui.quit();
            app
        })
        .expect("the window runs");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn open_project_asks_in_window_and_continues_when_answered() {
        let dir = run_scratch("inwindow");
        let session = ProjectSession::create("Picked", &dir).expect("create a project to pick");
        drop(session);
        let lrp = dir.join("Picked.lrp");
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        let start = dir.clone();
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            app.set_file_system(Rc::new(xui_core::widget::StdFileSystem), start.clone());
            assert!(!app.file_dialog_open());

            // Open Project does not block: it opens the dialog and waits.
            app.update(Msg::Command(Command::OpenProject), ui);
            assert!(app.file_dialog_open(), "the in-window dialog is showing");
            assert_eq!(app.file_request, Some(FileRequest::OpenProject));
            assert!(app.session.is_none(), "nothing opened yet");

            // Cancelling forgets the question and opens nothing.
            app.update(Msg::FileCancelled, ui);
            assert_eq!(app.file_request, None);
            assert!(app.session.is_none());

            // Answering continues the same code the blocking dialog would.
            app.update(Msg::Command(Command::OpenProject), ui);
            app.update(Msg::FileChosen(lrp.clone()), ui);
            assert_eq!(app.file_request, None);
            let opened = app.session.as_ref().map(|s| s.name().to_owned());
            assert_eq!(opened.as_deref(), Some("Picked"));

            // A stray answer with no pending question is ignored.
            app.update(Msg::FileChosen(lrp.clone()), ui);
            ui.quit();
            app
        })
        .expect("the window runs");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Typing a path into the in-window Open dialog, one character at a time,
    /// must never panic (a double borrow inside xui's list view did on LazyOS).
    #[test]
    fn typing_a_path_into_the_in_window_dialog_is_safe() {
        let dir = run_scratch("inwindow-typing");
        std::fs::create_dir_all(&dir).unwrap();
        let session = ProjectSession::create("Typed", &dir).expect("create");
        drop(session);
        let typed = format!("{}/Typed.lrp", dir.display());
        let backend = Rc::new(OffscreenBackend::new());
        let injector = Rc::clone(&backend);
        let start = dir.clone();
        run_app(
            backend as Rc<dyn Backend>,
            default_platform_spec(),
            move |ui| {
                let mut app =
                    IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
                app.set_file_system(Rc::new(xui_core::widget::StdFileSystem), start.clone());
                app.update(Msg::Command(Command::OpenProject), ui);
                for c in typed.chars() {
                    let _ = injector.inject(ui.window(), Event::Char(c));
                }
                app
            },
        )
        .expect("typing into the dialog does not panic");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_project_in_window_creates_the_chosen_folder() {
        let dir = run_scratch("inwindow-new");
        let target = dir.join("fresh").join("project");
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        let start = dir.clone();
        let chosen = target.clone();
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app =
                IdeApp::build(ui, Settings::default(), Vec::new()).expect("the IDE builds");
            app.set_file_system(Rc::new(xui_core::widget::StdFileSystem), start.clone());
            app.new_project("Fresh", ui);
            assert_eq!(
                app.file_request,
                Some(FileRequest::NewProject("Fresh".into()))
            );
            app.update(Msg::FileChosen(chosen.clone()), ui);
            assert!(
                chosen.join("Fresh.lrp").is_file(),
                "the folder and project were made"
            );
            assert_eq!(
                app.session.as_ref().map(|s| s.name().to_owned()).as_deref(),
                Some("Fresh")
            );
            ui.quit();
            app
        })
        .expect("the window runs");
        let _ = std::fs::remove_dir_all(dir);
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

    /// Right-clicks the Project Explorer's first row, 10px in from the tree's
    /// left edge, in a 1280x800 window with the given pane sizes. Returns the
    /// click and the context popup's bounds, both in window coordinates.
    fn right_click_explorer(
        width: f32,
        panes: crate::settings::PaneSizes,
    ) -> (Point, xui_core::Rect) {
        use xui_canvas::snapshot::{Snapshot, render_with};
        use xui_core::backend::Event;
        use xui_core::{Modifiers, MouseButton};

        let hello = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("examples")
            .join("hello");
        let result = Rc::new(std::cell::RefCell::new(None));
        let sink = Rc::clone(&result);
        let popup = Rc::new(Cell::new(WidgetId::NONE));
        let popup_out = Rc::clone(&popup);
        let ids = Rc::new(Cell::new([WidgetId::NONE; 6]));
        let ids_out = Rc::clone(&ids);
        let tree_left = Rc::new(Cell::new(0));
        let tree_left_out = Rc::clone(&tree_left);
        render_with(
            Snapshot::new(Dip(width), Dip(800.0)),
            move |ui| {
                let mut settings = Settings::default();
                settings.panes = panes;
                let mut app = IdeApp::build(ui, settings, Vec::new())?;
                app.open_project(&hello, ui);
                popup_out.set(app.context_menu.popup_id(0).unwrap_or(WidgetId::NONE));
                // The tree's window origin, summed independently of the fix:
                // the right column starts `right` design units from the edge.
                tree_left_out
                    .set(ui.client_rect().right - Dip(panes.right).to_px(ui.dpi()).value());
                ids_out.set([
                    app.tree.id(),
                    app.project_panel.id(),
                    app.right.id(),
                    app.centre.id(),
                    app.rest.id(),
                    app.outer.id(),
                ]);
                Ok(app)
            },
            move |stage| {
                // The first row, 28px below the pane top (menu 24, toolbar 32).
                let click = Point::new(tree_left.get() + 10, 24 + 32 + 28 + 11);
                stage.inject(Event::MouseDown {
                    x: click.x,
                    y: click.y,
                    button: MouseButton::Right,
                    modifiers: Modifiers::NONE,
                });
                *sink.borrow_mut() = Some((click, stage.ui().bounds(popup.get())));
            },
        )
        .expect("the IDE renders");
        let out = result.borrow_mut().take();
        out.expect("the step ran")
    }

    #[test]
    fn the_explorer_context_menu_opens_at_the_click_in_the_default_layout() {
        let (click, popup) = right_click_explorer(1280.0, crate::settings::PaneSizes::default());
        assert!((popup.left - click.x).abs() <= 3, "{popup:?} vs {click:?}");
        assert!((popup.top - click.y).abs() <= 3, "{popup:?} vs {click:?}");
    }

    #[test]
    fn the_explorer_context_menu_opens_at_the_click_in_a_narrow_window() {
        // The narrower window puts the tree column much nearer the origin.
        let (click, popup) = right_click_explorer(800.0, crate::settings::PaneSizes::default());
        assert!(click.x < 700, "the tree is nearer the origin: {click:?}");
        assert!((popup.left - click.x).abs() <= 3, "{popup:?} vs {click:?}");
        assert!((popup.top - click.y).abs() <= 3, "{popup:?} vs {click:?}");
    }

    #[test]
    fn a_pane_move_reflows_the_tree_and_grid_into_their_panes() {
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
            let title = PANE_TITLE.to_px(ui.dpi()).value();

            // Moving the toolbox divider moves the split it belongs to.
            app.on_pane_moved(PaneSlot::Toolbox, 220.0, ui);
            assert_eq!(app.outer.position(), dip(220.0));

            // Moving the right column's divider re-flows the tree and the grid
            // into their panes, below the titles.
            app.on_pane_moved(PaneSlot::Right, 400.0, ui);
            let project = ui.bounds(app.project_panel.id());
            let tree = ui.bounds(app.tree.id());
            assert_eq!(tree.left, 0);
            assert_eq!(tree.top, title);
            assert_eq!(tree.width(), project.width());
            assert_eq!(tree.height(), project.height() - title);

            let grid = app.properties_grid.as_ref().expect("the grid is bound");
            let properties = ui.bounds(app.properties_panel.id());
            let grid_bounds = ui.bounds(grid.id());
            assert_eq!(grid_bounds.left, 0);
            assert_eq!(grid_bounds.top, title);
            assert_eq!(grid_bounds.width(), properties.width());
            assert_eq!(grid_bounds.height(), properties.height() - title);
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

#[cfg(test)]
#[path = "shortcut_tests.rs"]
mod shortcut_tests;

#[cfg(test)]
#[path = "shortcut_app_tests.rs"]
mod shortcut_app_tests;
