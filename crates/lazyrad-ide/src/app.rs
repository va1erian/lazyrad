#![forbid(unsafe_code)]

//! The IDE's main window: the VB6 layout, menu bar, toolbar and command
//! dispatch (PLAN.md §9, issue #7).
//!
//! The window is a menu bar and toolbar docked at the top, then nested splits:
//! the toolbox on the left, the tabbed document area in the centre, the Output
//! pane at the bottom, and the Project and Properties panes in a right-hand
//! column. Pane sizes come from [`Settings`] and are written back when a
//! divider moves.

use std::path::PathBuf;
use std::rc::Rc;

use xui_core::app::{App, Ui};
use xui_core::backend::{Event, PlatformSpec, Result, WidgetId};
use xui_core::layout::Dock;
use xui_core::units::Px;
use xui_core::widget::{HasText, Label, Menu, MenuId, Panel, Split, Tabs, Toolbar};
use xui_core::{Dip, Rect, dip};

use crate::command::{Command, Dispatcher};
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

/// A message the IDE app handles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Msg {
    /// A command was chosen from the menu, the toolbar or a shortcut.
    Command(Command),
    /// The window resized; re-flow the frame.
    Relayout,
    /// A divider moved, carrying the first pane's new extent in design units.
    PaneMoved(PaneSlot, f32),
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

/// The IDE shell's application state.
pub struct IdeApp {
    settings: Settings,
    dispatcher: Dispatcher,
    /// Every menu entry that dispatches a command, for enabling/disabling.
    menu_commands: MenuCommands,
    /// Kept alive so the menu's nodes live as long as the app.
    _menu: Menu<Msg>,
    menu_id: WidgetId,
    _toolbar: Toolbar<Msg>,
    toolbar_id: WidgetId,
    outer: Split<Msg>,
    rest: Split<Msg>,
    centre: Split<Msg>,
    right: Split<Msg>,
    /// The pane containers, kept alive (and available to a later View toggle).
    _panels: Vec<Panel<Msg>>,
    _tabs: Tabs<Msg>,
    _labels: Vec<Label<Msg>>,
    /// The Output pane's label, rewritten as commands are logged.
    output: Label<Msg>,
    output_lines: Vec<String>,
    /// Set when saving the settings on exit failed; the next Exit quits
    /// without saving, so a read-only config directory cannot trap the user.
    exit_save_failed: bool,
}

impl IdeApp {
    /// Builds the whole window and returns the app the runtime drives.
    pub fn build(ui: &Ui<Msg>, settings: Settings, recent: Vec<PathBuf>) -> Result<IdeApp> {
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

        let docs = Tabs::new(centre.ui(), Rect::default())?;
        let right = Split::column(centre.ui(), Rect::default())?
            .on_moved(|position| Some(Msg::PaneMoved(PaneSlot::Project, position.value())));
        centre.pane_a(&[docs.id()]);
        centre.pane_b(&[right.id()]);
        centre.set_min(dip(200.0), dip(120.0));

        let project = Panel::new(right.ui(), Rect::default())?;
        let properties = Panel::new(right.ui(), Rect::default())?;
        right.pane_a(&[project.id()]);
        right.pane_b(&[properties.id()]);
        right.set_min(dip(60.0), dip(60.0));

        // Pane contents: a title per pane, a welcome tab, and the Output label.
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
            project.ui(),
            Rect::new(8, 8, 220, 28),
            "Project",
        )?);
        labels.push(Label::new(
            properties.ui(),
            Rect::new(8, 8, 220, 28),
            "Properties",
        )?);
        let welcome = Label::new(
            docs.ui(),
            Rect::new(16, 16, 480, 48),
            "LazyRAD — the IDE shell. Open a project to begin.",
        )?;
        let docs = docs.page("Start Page", &[welcome.id()]);
        let output = Label::new(output_panel.ui(), Rect::new(8, 8, 480, 24), "Output")?;

        let mut app = IdeApp {
            settings,
            dispatcher: Dispatcher::new(),
            menu_commands,
            _menu: menu,
            menu_id,
            _toolbar: toolbar,
            toolbar_id,
            outer,
            rest,
            centre,
            right,
            _panels: vec![toolbox, output_panel, project, properties],
            _tabs: docs,
            _labels: labels,
            output,
            output_lines: Vec::new(),
            exit_save_failed: false,
        };

        app.apply_theme(ui);
        app.refresh_menu();
        app.layout_frame(ui);
        app.log(ui, "LazyRAD IDE ready.");
        app.log(ui, "Commands are logged until their features land.");

        // A focused node is what makes the backend deliver `KeyDown` at all;
        // the shortcut backend then routes it to the dispatcher (gap G10).
        ui.focus(app.centre.id());

        // Window-level events reach the null node; re-flow the frame when the
        // window resizes. (`Split` does not re-flow itself on resize.)
        ui.register_events(WidgetId::NONE, |event| {
            matches!(event, Event::Resize { .. }).then_some(Msg::Relayout)
        });

        // The window's close button goes through Exit, which saves the
        // settings before quitting (and is where "save changes?" will hook in).
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
    }

    /// Pushes `settings.theme` into the window's palette.
    fn apply_theme(&self, ui: &Ui<Msg>) {
        ui.set_theme(crate::theme::palette(self.settings.theme));
    }

    /// Applies the dispatcher's enabled state to every mapped menu entry.
    fn refresh_menu(&self) {
        for (id, command) in self.menu_commands.iter() {
            self._menu
                .set_enabled(*id, self.dispatcher.is_enabled(*command));
        }
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

    /// Runs a command: applying the ones that exist and logging the rest.
    fn run_command(&mut self, command: Command, ui: &mut Ui<Msg>) {
        match command {
            Command::Exit => match self.settings.save() {
                Ok(()) => ui.quit(),
                Err(_) if self.exit_save_failed => ui.quit(),
                Err(error) => {
                    // Keep the IDE open so the user sees why; a second Exit
                    // (or window close) quits and discards the unsaved layout.
                    self.exit_save_failed = true;
                    self.log(
                        ui,
                        format!(
                            "Settings could not be saved ({error}). Exit again to quit without saving them."
                        ),
                    );
                }
            },
            Command::ThemeLight => self.set_theme(ui, ThemeChoice::Light),
            Command::ThemeDark => self.set_theme(ui, ThemeChoice::Dark),
            Command::ThemeSystem => self.set_theme(ui, ThemeChoice::System),
            _ => {
                if let Some(line) = self.dispatcher.dispatch(command) {
                    self.log(ui, line);
                }
            }
        }
        // Commands that open or close a project would flip the dispatcher's
        // project state; until the project loader lands, they are only logged.
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
}

impl App for IdeApp {
    type Msg = Msg;

    fn update(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
        match msg {
            Msg::Command(command) => self.run_command(command, ui),
            Msg::Relayout => self.layout_frame(ui),
            Msg::PaneMoved(slot, position) => self.on_pane_moved(slot, position, ui),
        }
    }
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
fn build_menu(ui: &Ui<Msg>, bounds: Rect, recent: &[PathBuf]) -> Result<(Menu<Msg>, MenuCommands)> {
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
    use xui_canvas::OffscreenBackend;
    use xui_core::backend::Backend;
    use xui_core::run_app;
    use xui_core::{Key, Modifiers};

    #[test]
    fn the_shortcut_mapper_maps_and_ignores_as_expected() {
        assert_eq!(
            shortcut_message(&Event::KeyDown {
                key: Key::S,
                modifiers: Modifiers {
                    ctrl: true,
                    ..Modifiers::NONE
                },
                repeat: 1,
                system: false,
            }),
            Some(Msg::Command(Command::Save))
        );
        assert_eq!(
            shortcut_message(&Event::KeyUp {
                key: Key::S,
                modifiers: Modifiers::NONE,
                system: false,
            }),
            None
        );
        assert_eq!(
            shortcut_message(&Event::KeyDown {
                key: Key::S,
                modifiers: Modifiers {
                    ctrl: true,
                    ..Modifiers::NONE
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
        run_app(backend, default_platform_spec(), |ui| {
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
}
