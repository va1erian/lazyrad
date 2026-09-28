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

use lazyrad_editor::Editor;
use xui_core::app::{App, Ui};
use xui_core::backend::{Backend, Event, PlatformSpec, Result as UiResult, WidgetId};
use xui_core::geometry::Point;
use xui_core::layout::Dock;
use xui_core::units::Px;
use xui_core::widget::{
    Dialog, DialogAction, HasText, Label, Menu, MenuId, Panel, Split, Tabs, Toolbar, TreeView,
};
use xui_core::{Dip, Rect, dip};

use crate::command::{Command, Dispatcher};
use crate::dialog::ChoiceDialog;
use crate::explorer::{DoubleClick, Explorer, ExplorerItem};
use crate::platform::dialogs;
use crate::project::{DEFAULT_PROJECT, ProjectSession};
use crate::settings::{Settings, ThemeChoice};

/// The menu bar's height.
const MENU_HEIGHT: Dip = dip(24.0);
/// The toolbar's height.
const TOOLBAR_HEIGHT: Dip = dip(32.0);
/// The divider thickness xui's [`Split`] draws, so computed pane sizes are
/// exact.
const DIVIDER: f32 = 5.0;
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
    /// The code editor.
    Code(Rc<Editor<Msg>>),
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
        for (index, name) in ["Label", "TextBox", "Button", "CheckBox", "ListBox"]
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

        let editors: Rc<RefCell<Vec<Rc<Editor<Msg>>>>> = Rc::new(RefCell::new(Vec::new()));
        {
            let editors = Rc::clone(&editors);
            ui.on_timer(move |id| {
                for editor in editors.borrow().iter() {
                    editor.handle_timer(id);
                }
                None
            });
        }

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
            _panels: vec![toolbox, output_panel],
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
        if let Some(docs) = &self.docs {
            docs.relayout();
        }
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
                if let Some(name) = self.selected_item_name() {
                    self.open_code(&name, ui);
                } else {
                    self.log(ui, "Select an item in the Project Explorer first.");
                }
            }
            Command::ViewObject => {
                if let Some(name) = self.selected_item_name() {
                    self.open_object(&name, ui);
                } else {
                    self.log(ui, "Select an item in the Project Explorer first.");
                }
            }
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
                let saved = {
                    self.sync_documents();
                    self.session
                        .as_mut()
                        .is_some_and(|session| session.save().is_ok())
                };
                if saved {
                    for document in &mut self.documents {
                        document.dirty = false;
                    }
                    self.update_title(ui);
                    self.continue_pending(pending, ui);
                } else {
                    self.log(ui, "The project could not be saved; it is still open.");
                }
            }
        }
    }

    /// Opens a prompt dialog for `kind`.
    fn show_prompt(&mut self, ui: &mut Ui<Msg>, kind: PromptKind) {
        let (title, message, initial) = match &kind {
            PromptKind::NewProject => ("New Project", "Project name:", DEFAULT_PROJECT),
            PromptKind::Rename(old) => ("Rename Item", "New name:", old.as_str()),
        };
        match Dialog::prompt(ui, title, message, initial) {
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
        let text = text.trim().to_owned();
        if text.is_empty() {
            self.log(ui, "No name given; nothing was changed.");
            return;
        }
        match kind {
            PromptKind::NewProject => self.new_project(&text, ui),
            PromptKind::Rename(old) => self.rename_item(&old, &text, ui),
        }
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
        self.documents.retain(|document| document.name != name);
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
            return Ok(());
        }

        let scoped = self
            .docs
            .as_ref()
            .expect("the document tabs exist")
            .ui()
            .clone();
        let (view, id, title) = self.build_view(&scoped, name, kind)?;
        let tabs = self.docs.take().expect("the document tabs exist");
        self.documents.push(Document {
            name: name.to_owned(),
            kind,
            title: title.clone(),
            dirty: false,
            view,
        });
        self.docs = Some(tabs.page(&title, &[id]));
        Ok(())
    }

    /// Builds the widgets behind a document, returning the view, its node id
    /// and the tab title.
    fn build_view(
        &mut self,
        ui: &Ui<Msg>,
        name: &str,
        kind: DocKind,
    ) -> UiResult<(DocumentView, WidgetId, String)> {
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
                    id,
                    name.to_owned(),
                ))
            }
            DocKind::Code => {
                let editor = Editor::new(ui, Rect::default())?.on_change({
                    let name = name.to_owned();
                    move |text| Some(Msg::DocumentEdited(name.clone(), text.to_string()))
                });
                let editor = Rc::new(editor);
                self.editors.borrow_mut().push(Rc::clone(&editor));
                if let Some(source) = self.session.as_ref().and_then(|s| s.code(name)) {
                    editor.set_text(source);
                }
                let id = editor.id();
                Ok((DocumentView::Code(editor), id, format!("{name}.rhai")))
            }
        }
    }

    /// Closes every document and rebuilds the tabs with just the Start Page.
    fn reset_documents(&mut self) -> UiResult<()> {
        for editor in self.editors.borrow().iter() {
            self.docs_ui.kill_timer(editor.timer_id());
        }
        self.editors.borrow_mut().clear();
        self.documents.clear();
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
        // Stop the old editors' blink timers before their widgets go away.
        for editor in self.editors.borrow().iter() {
            self.docs_ui.kill_timer(editor.timer_id());
        }
        self.editors.borrow_mut().clear();
        self.documents.clear();
        self.docs = None;
        self.start_label = None;

        let tabs = Tabs::new(&self.docs_ui, Rect::default())?;
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
            if let DocumentView::Code(editor) = &document.view
                && let Some(session) = self.session.as_mut()
            {
                // Only write back real edits: `set_code` marks the project
                // dirty, and an unchanged document must not.
                let text = editor.text();
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
                .item(ids.id(Command::Find), "&Find…");
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
        let mut control = lazyrad_project::Node::new("CommandButton", "cmdGo");
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
}
