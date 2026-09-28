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
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use lazyrad_editor::{Editor, Marker, MarkerKind, Query};
use xui_core::app::{App, Ui};
use xui_core::backend::{
    Backend, BackendError, Event, PlatformSpec, Result as UiResult, TimerId, WidgetId,
};
use xui_core::geometry::Point;
use xui_core::layout::Dock;
use xui_core::units::Px;
use xui_core::widget::{
    ComboBox, Dialog, DialogAction, HasText, Label, ListView, Menu, MenuId, Panel, Split, Tabs,
    Toolbar, TreeView,
};
use xui_core::{Dip, Rect, dip};

use crate::command::{Command, Dispatcher};
use crate::compile::{self, CodeDiagnostic, CompileScheduler};
use crate::dialog::ChoiceDialog;
use crate::explorer::{DoubleClick, Explorer, ExplorerItem};
use crate::platform::dialogs;
use crate::procedures::{self, ObjectEntry};
use crate::project::{DEFAULT_PROJECT, ProjectSession};
use crate::settings::{Settings, ThemeChoice};

/// The menu bar's height.
const MENU_HEIGHT: Dip = dip(24.0);
/// The toolbar's height.
const TOOLBAR_HEIGHT: Dip = dip(32.0);
/// The divider thickness xui's [`Split`] draws, so computed pane sizes are
/// exact.
const DIVIDER: f32 = 5.0;
/// The tab strip height xui's [`Tabs`] reserves at the top of a page. The code
/// view positions its widgets below it, matching the tab layout.
const TABS_STRIP: Dip = dip(32.0);
/// The code view's procedure-combo header height.
const CODE_HEADER: Dip = dip(26.0);
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

/// The toolbar's items and the command each dispatches.
const TOOLBAR: &[(&str, Command)] = &[
    ("New", Command::NewProject),
    ("Open", Command::OpenProject),
    ("Save", Command::Save),
    ("Save All", Command::SaveAll),
    ("Undo", Command::Undo),
    ("Redo", Command::Redo),
    ("Cut", Command::Cut),
    ("Copy", Command::Copy),
    ("Paste", Command::Paste),
    ("Run", Command::RunStart),
    ("End", Command::RunEnd),
];

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
    /// A form's object (the design surface; a preview until the designer
    /// crate lands).
    Designer,
    /// A form's or module's code-behind.
    Code,
}

/// The widgets behind one open document.
enum DocumentView {
    /// A minimal form preview: a label per control at its model geometry.
    Designer {
        _panel: Panel<Msg>,
        _labels: Vec<Label<Msg>>,
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

/// One open document tab.
struct Document {
    name: String,
    kind: DocKind,
    title: String,
    dirty: bool,
    view: DocumentView,
}

/// The IDE shell's application state.
pub struct IdeApp {
    settings: Settings,
    dispatcher: Dispatcher,
    /// The backend, so the window title can show the project and its dirty `*`
    /// (xui's [`Ui`] has no retitle call).
    backend: Rc<dyn Backend>,
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
    _panels: Vec<Panel<Msg>>,
    output_panel: Panel<Msg>,
    project_panel: Panel<Msg>,
    _properties_panel: Panel<Msg>,
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
    /// Set when saving the settings on exit failed; the next Exit quits
    /// without saving, so a read-only config directory cannot trap the user.
    exit_save_failed: bool,
}

impl IdeApp {
    /// Builds the whole window and returns the app the runtime drives.
    pub fn build(
        ui: &Ui<Msg>,
        settings: Settings,
        recent: Vec<PathBuf>,
        backend: Rc<dyn Backend>,
    ) -> UiResult<IdeApp> {
        let dpi = ui.dpi();
        let client = ui.client_rect();

        // The top strips: menu bar, then toolbar, then the split area.
        let menu_band = Dock::new().top(MENU_HEIGHT).split(client, dpi);
        let menu_rect = menu_band.top.unwrap_or(client);
        let toolbar_band = Dock::new().top(TOOLBAR_HEIGHT).split(menu_band.fill, dpi);
        let toolbar_rect = toolbar_band.top.unwrap_or(client);
        let main_rect = toolbar_band.fill;

        let (menu, menu_commands) = build_menu(ui, menu_rect, &recent)?;
        let menu_id = menu.id().unwrap_or(WidgetId::NONE);

        let toolbar_labels: Vec<&str> = TOOLBAR.iter().map(|(label, _)| *label).collect();
        let toolbar = Toolbar::new(ui, toolbar_rect, &toolbar_labels)?.on_click(|index| {
            TOOLBAR
                .get(index)
                .map(|(_, command)| Msg::Command(*command))
        });
        let toolbar_id = toolbar.id();

        // The nested splits. Each pane is a container created through the
        // split's own scoped `Ui`, so the split can place its children.
        let outer = Split::row(ui, main_rect)?
            .on_moved(|position| Some(Msg::PaneMoved(PaneSlot::Toolbox, position.value())));
        let toolbox = Panel::new(outer.ui(), Rect::default())?;
        let rest = Split::column(outer.ui(), Rect::default())?
            .on_moved(|position| Some(Msg::PaneMoved(PaneSlot::Output, position.value())));
        outer.pane_a(&[toolbox.id()]);
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

        // Pane contents: a title per pane, the Project Explorer tree, and the
        // Output label.
        let mut labels = Vec::new();
        labels.push(Label::new(
            toolbox.ui(),
            Rect::new(8, 8, 132, 28),
            "Toolbox",
        )?);
        for (index, name) in ["Label", "Edit", "Button", "CheckBox", "ListView"]
            .iter()
            .enumerate()
        {
            labels.push(Label::new(
                toolbox.ui(),
                Rect::new(8, 36 + 22 * index as i32, 132, 56 + 22 * index as i32),
                name,
            )?);
        }
        labels.push(Label::new(
            project_panel.ui(),
            Rect::new(8, 6, 220, 26),
            "Project",
        )?);
        labels.push(Label::new(
            properties_panel.ui(),
            Rect::new(8, 8, 220, 28),
            "Properties",
        )?);
        let output = Label::new(output_panel.ui(), Rect::new(8, 8, 480, 24), "Output")?;
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
            backend,
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
            _panels: vec![toolbox],
            output_panel,
            project_panel,
            _properties_panel: properties_panel,
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
        let main_rect = toolbar_band.fill;

        ui.apply_moves(&[(self.menu_id, menu_rect), (self.toolbar_id, toolbar_rect)]);

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

        // The Project Explorer fills its panel below the title label.
        let panel = ui.bounds(self.project_panel.id());
        ui.apply_moves(&[(
            self.tree.id(),
            Rect::new(0, 28, panel.width().max(0), panel.height().max(0)),
        )]);

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

    /// Updates the window title, which carries the project name and a `*` when
    /// anything is unsaved.
    fn update_title(&self, ui: &Ui<Msg>) {
        let mut title = match &self.session {
            Some(session) => format!("LazyRAD - {}", session.name()),
            None => "LazyRAD".to_owned(),
        };
        if self.project_dirty() {
            title.push('*');
        }
        self.backend.set_window_title(ui.window(), &title);
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
        self.sync_documents();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match session.save() {
            Ok(report) => {
                let name = session.name().to_owned();
                for document in &mut self.documents {
                    document.dirty = false;
                }
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
                for document in &mut self.documents {
                    document.dirty = false;
                }
                self.settings.push_recent(dialogs::containing_folder(&file));
                let _ = self.settings.save();
                self.rebuild_menu(ui);
                self.log(ui, format!("Saved as {}.", file.display()));
            }
            Err(error) => self.log(ui, format!("Save As failed: {error}")),
        }
        self.update_title(ui);
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
                self.sync_documents();
                match self.session.as_mut().map(ProjectSession::save) {
                    Some(Ok(_)) => {
                        for document in &mut self.documents {
                            document.dirty = false;
                        }
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
        match lazyrad_editor::find::replace_all(
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
        if let Some(document) = self
            .documents
            .iter_mut()
            .find(|document| document.name == name && document.kind == DocKind::Code)
        {
            document.dirty = true;
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
        let labels: Vec<String> = self
            .errors
            .iter()
            .map(|entry| format!("{}{}", entry.name, entry.diagnostic.label()))
            .collect();
        let rows: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.error_list.set_items(&rows);
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
            match ComboBox::new(&scoped, Rect::default(), &items) {
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
        if let Some(offset) = procedures::find_handler(&editor.text(), &signature) {
            editor.set_caret(offset);
            editor.focus();
            return;
        }
        let text = editor.text();
        let end = text.chars().count();
        let separator = if text.is_empty() || text.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        let snippet = procedures::handler_snippet(&signature, &args);
        let insert = format!("{separator}{snippet}");
        // The caret lands just after the opening brace of the new handler.
        let before_brace = format!("fn {signature}({args}) ").chars().count();
        editor.set_caret(end);
        editor.insert_text(&insert);
        editor.set_caret(end + separator.chars().count() + before_brace + 1);
        editor.focus();
        self.after_programmatic_edit(name, ui);
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
        match action {
            ContextAction::ViewCode => self.open_code(&name, ui),
            ContextAction::ViewObject => self.open_object(&name, ui),
            ContextAction::Rename => self.show_prompt(ui, PromptKind::Rename(name)),
            ContextAction::Remove => self.remove_item(&name, ui),
            ContextAction::SetStartup => self.set_startup(&name, ui),
        }
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
            // children (the procedure combos) here.
            let docs_ui = self.docs_ui.clone();
            self.layout_code_views(&docs_ui, docs_ui.dpi());
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
                let panel = Panel::new(ui, Rect::default())?;
                let mut labels = Vec::new();
                if let Some(form) = self.session.as_ref().and_then(|s| s.form(name)) {
                    labels.push(Label::new(
                        panel.ui(),
                        Rect::new(4, 4, 200, 24),
                        &format!("{} — {}", form.window.name, window_title(form)),
                    )?);
                    for control in &form.nodes {
                        let text = control_label(control);
                        labels.push(Label::new(panel.ui(), control_rect(control), &text)?);
                    }
                }
                let id = panel.id();
                Ok((
                    DocumentView::Designer {
                        _panel: panel,
                        _labels: labels,
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
                    Some(ComboBox::new(ui, Rect::default(), &items)?.on_select({
                        let name = name.to_owned();
                        move |index| Some(Msg::ObjectChanged(name.clone(), index))
                    }))
                } else {
                    None
                };
                let procedure = if is_form {
                    let items = objects
                        .first()
                        .map(ObjectEntry::event_names)
                        .unwrap_or_default();
                    let items: Vec<&str> = items.iter().map(String::as_str).collect();
                    Some(ComboBox::new(ui, Rect::default(), &items)?.on_select({
                        let name = name.to_owned();
                        move |index| Some(Msg::ProcedureChanged(name.clone(), index))
                    }))
                } else {
                    None
                };

                let editor = Editor::new(ui, Rect::default())?.on_change({
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
                if let Some(document) = self
                    .documents
                    .iter_mut()
                    .find(|document| document.name == name && document.kind == DocKind::Code)
                {
                    document.dirty = true;
                }
                self.update_title(ui);
            }
            Msg::TabChanged(_) => {
                self.layout_code_views(ui, ui.dpi());
            }
            Msg::CompileFinished {
                name,
                revision,
                errors,
            } => self.apply_compile(&name, revision, errors, ui),
            Msg::ErrorActivated(row) => self.activate_error(row, ui),
            Msg::ObjectChanged(name, index) => self.change_object(&name, index, ui),
            Msg::ProcedureChanged(name, index) => self.insert_procedure(&name, index, ui),
        }
    }
}

/// The label a designer preview draws for `control`: its caption when it has
/// one, otherwise its type and name.
fn control_label(control: &lazyrad_project::Node) -> String {
    let caption = control
        .prop("text")
        .and_then(lazyrad_project::Value::as_str)
        .filter(|caption| !caption.is_empty());
    match caption {
        Some(caption) => format!("{caption} [{}]", control.kind),
        None => format!("{} {}", control.kind, control.name),
    }
}

/// The window title a form declares, or an empty string.
fn window_title(form: &lazyrad_project::FormDoc) -> &str {
    form.window
        .prop("title")
        .and_then(lazyrad_project::Value::as_str)
        .unwrap_or_default()
}

/// A control's rectangle in the form's own coordinates. Geometry the node
/// leaves out uses the placeholder size the preview draws.
fn control_rect(control: &lazyrad_project::Node) -> Rect {
    let int = |name: &str, default: i64| {
        control
            .prop(name)
            .and_then(lazyrad_project::Value::as_int)
            .unwrap_or(default) as i32
    };
    let (left, top) = (int("left", 0), int("top", 0));
    Rect::new(left, top, left + int("width", 120), top + int("height", 24))
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
            file.item(ids.id(Command::NewProject), "&New Project")
                .item(ids.id(Command::OpenProject), "&Open Project…");
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
                .item(ids.id(Command::Save), "&Save")
                .item(ids.id(Command::SaveAs), "Save &As…")
                .item(ids.id(Command::SaveAll), "Save &All")
                .item(ids.id(Command::CloseProject), "&Close Project")
                .separator()
                .item(ids.id(Command::Exit), "E&xit");
        });
        bar.submenu(ids.plain(), "&Edit", |edit| {
            edit.item(ids.id(Command::Undo), "&Undo")
                .item(ids.id(Command::Redo), "&Redo")
                .separator()
                .item(ids.id(Command::Cut), "Cu&t")
                .item(ids.id(Command::Copy), "&Copy")
                .item(ids.id(Command::Paste), "&Paste")
                .item(ids.id(Command::Delete), "&Delete")
                .separator()
                .item(ids.id(Command::SelectAll), "Select &All")
                .item(ids.id(Command::Find), "&Find…")
                .item(ids.id(Command::FindNext), "Find &Next")
                .item(ids.id(Command::Replace), "&Replace…")
                .item(ids.id(Command::GoToLine), "&Go To Line…");
        });
        bar.submenu(ids.plain(), "&View", |view| {
            view.item(ids.id(Command::ViewCode), "&Code")
                .item(ids.id(Command::ViewObject), "&Object")
                .separator()
                .item(ids.id(Command::ViewProject), "&Project")
                .item(ids.id(Command::ViewProperties), "P&roperties")
                .item(ids.id(Command::ViewToolbox), "&Toolbox")
                .item(ids.id(Command::ViewOutput), "&Output")
                .separator()
                .submenu(ids.plain(), "&Theme", |theme| {
                    theme.item(ids.id(Command::ThemeLight), "&Light");
                    theme.item(ids.id(Command::ThemeDark), "&Dark");
                    theme.item(ids.id(Command::ThemeSystem), "&System");
                });
        });
        bar.submenu(ids.plain(), "&Project", |project| {
            project
                .item(ids.id(Command::AddForm), "Add &Form")
                .item(ids.id(Command::AddModule), "Add &Module")
                .separator()
                .item(ids.id(Command::Remove), "&Remove")
                .separator()
                .item(ids.id(Command::ProjectProperties), "P&roperties");
        });
        bar.submenu(ids.plain(), "&Run", |run| {
            run.item(ids.id(Command::RunStart), "&Start")
                .item(ids.id(Command::RunEnd), "&End");
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
    use xui_canvas::OffscreenBackend;
    use xui_core::Key;
    use xui_core::run_app;

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
        let backend_for_app = Rc::clone(&backend);
        run_app(backend, default_platform_spec(), move |ui| {
            IdeApp::build(ui, Settings::default(), Vec::new(), backend_for_app)
                .expect("the IDE builds")
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
    fn a_designer_label_prefers_the_caption() {
        let mut control = lazyrad_project::Node::new("Label", "lblOne");
        assert_eq!(control_label(&control), "Label lblOne");
        control.set_prop("text", lazyrad_project::Value::Text("Hello".to_owned()));
        assert_eq!(control_label(&control), "Hello [Label]");
    }

    #[test]
    fn a_control_rect_maps_the_geometry() {
        let mut control = lazyrad_project::Node::new("Button", "go_button");
        control.set_prop("left", lazyrad_project::Value::Int(16));
        control.set_prop("top", lazyrad_project::Value::Int(32));
        control.set_prop("width", lazyrad_project::Value::Int(120));
        control.set_prop("height", lazyrad_project::Value::Int(24));
        assert_eq!(control_rect(&control), Rect::new(16, 32, 136, 56));
    }

    #[test]
    fn session_errors_read_well() {
        let error = SessionError::InvalidName("`bad` is not valid".to_owned());
        assert_eq!(error.to_string(), "`bad` is not valid");
    }

    #[test]
    fn a_code_window_shows_diagnostics_and_inserts_handlers() {
        let dir = std::env::temp_dir().join(format!("lazyrad-ide-code-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup = dir.clone();

        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        let backend_for_app = Rc::clone(&backend);
        run_app(backend, default_platform_spec(), move |ui| {
            let mut app = IdeApp::build(ui, Settings::default(), Vec::new(), backend_for_app)
                .expect("the IDE builds");
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
}
