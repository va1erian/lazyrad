#![forbid(unsafe_code)]

//! The toolbox pane: a pointer tool plus one icon tile per Iteration 1 control.
//!
//! xui's [`Toolbar`](xui_core::widget::Toolbar) has text labels only (PLAN.md
//! gap G9), so the toolbox is a small [`Custom`](xui_core::backend::NodeKind)
//! grid that draws each entry with xui's icon API: a [`Lucide`] outline from
//! [`xui_core::icon::draw_icon`] plus the control's kind name (`Button`, `Edit`,
//! …). No icon is hand-drawn here.
//!
//! The widget owns the *tool selection*, not the form. It maps a click to
//! [`ToolboxMsg::Select`] and a double-click to [`ToolboxMsg::Activate`], each
//! wrapping the [`Tool`] the click landed on, and hands that to the host through
//! the `wrap` closure given to [`Toolbox::new`]. The host forwards the message
//! to [`Designer::handle_toolbox`](crate::Designer::handle_toolbox), which arms
//! the tool for click-then-drag creation or drops the control in the centre of
//! the form.
//!
//! Unlike the portable widgets, the toolbox deliberately does **not** ignore
//! input in design mode: the designer puts the whole window in design mode (see
//! the crate docs), and a toolbox that honoured that flag could never be used.

use std::cell::RefCell;
use std::rc::Rc;

use xui_core::app::Ui;
use xui_core::backend::{BackendError, Canvas, Event, NodeKind, NodeSpec, TextStyle, WidgetId};
use xui_core::geometry::Rect;
use xui_core::icon::draw_icon;
use xui_core::message::{Key, MouseButton};
use xui_core::units::Dip;
use xui_core::widget::Control;
use xui_core::{Lucide, Theme};

use crate::local_paint::paint_local;

/// The Iteration 1 control kinds the toolbox offers, by xui kind name.
pub const CONTROL_KINDS: [&str; 8] = [
    "Button",
    "Edit",
    "Label",
    "CheckBox",
    "RadioGroup",
    "GroupBox",
    "ListView",
    "ComboBox",
];

/// The design width of one tile.
const TILE_WIDTH: Dip = Dip(96.0);
/// The design height of one tile.
const TILE_HEIGHT: Dip = Dip(28.0);
/// The gap between tiles, on both axes.
const TILE_GAP: Dip = Dip(2.0);
/// The space between the pane's edge and the tiles, so a selected tile does not
/// touch the pane's border.
const TILE_INSET: Dip = Dip(4.0);
/// The design size of a tile's icon.
const ICON_SIZE: Dip = Dip(20.0);
/// The design size of a tile's label.
const LABEL_SIZE: Dip = Dip(11.0);
/// The corner radius of a tile's selection fill.
const TILE_RADIUS: f32 = 3.0;

/// One toolbox entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tool {
    /// The pointer: no creation, the normal selection tool.
    Pointer,
    /// A control kind the next drag creates.
    Control(String),
}

impl Tool {
    /// The pointer tool.
    pub fn pointer() -> Tool {
        Tool::Pointer
    }

    /// A control tool for `kind` (a catalog kind or alias).
    pub fn control(kind: impl Into<String>) -> Tool {
        Tool::Control(kind.into())
    }

    /// The control kind this tool creates, or `None` for the pointer.
    pub fn kind(&self) -> Option<&str> {
        match self {
            Tool::Pointer => None,
            Tool::Control(kind) => Some(kind),
        }
    }

    /// The label shown on the tile: the pointer's name, or the control's kind.
    pub fn label(&self) -> &str {
        match self {
            Tool::Pointer => "Pointer",
            Tool::Control(kind) => kind,
        }
    }

    /// The Lucide outline drawn on the tile.
    ///
    /// The mapping is per control kind; an unknown kind falls back to
    /// [`Lucide::Square`] so a catalog entry added without an icon still draws
    /// something recognisable rather than nothing.
    pub fn lucide(&self) -> Lucide {
        match self {
            Tool::Pointer => Lucide::MousePointer2,
            Tool::Control(kind) => match kind.as_str() {
                "Button" => Lucide::SquareMousePointer,
                "Edit" => Lucide::TextCursorInput,
                "Label" => Lucide::Type,
                "CheckBox" => Lucide::SquareCheck,
                "RadioGroup" => Lucide::CircleDot,
                "GroupBox" => Lucide::Group,
                "ListView" => Lucide::List,
                "ComboBox" => Lucide::ChevronsUpDown,
                _ => Lucide::Square,
            },
        }
    }
}

/// A message the toolbox maps its input to, wrapped into the host's `Msg` by
/// the closure given to [`Toolbox::new`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolboxMsg {
    /// A tile was clicked; the host arms this tool.
    Select(Tool),
    /// A tile was double-clicked; the host drops this control.
    Activate(Tool),
}

/// Every toolbox entry, the pointer first.
pub fn tools() -> Vec<Tool> {
    let mut tools = vec![Tool::Pointer];
    tools.extend(CONTROL_KINDS.iter().map(|kind| Tool::control(*kind)));
    tools
}

/// The toolbox's live selection and hover.
///
/// The default is the pointer (index zero) and nothing hovered.
#[derive(Clone, Copy, Debug, Default)]
struct State {
    selected: usize,
    hover: Option<usize>,
}

/// The device-pixel tile layout for a toolbox of a given size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    width: i32,
    height: i32,
    gap: i32,
    inset: i32,
    columns: i32,
}

impl Layout {
    /// The layout for a toolbox `area` device pixels wide at `dpi`.
    fn new(area: Rect, dpi: u32) -> Layout {
        let width = TILE_WIDTH.to_px(dpi).value().max(1);
        let height = TILE_HEIGHT.to_px(dpi).value().max(1);
        let gap = TILE_GAP.to_px(dpi).value().max(0);
        let inset = TILE_INSET.to_px(dpi).value().max(0);
        let columns = ((area.width() - inset * 2 + gap) / (width + gap)).max(1);
        Layout {
            width,
            height,
            gap,
            inset,
            columns,
        }
    }

    /// The local rectangle of the tile at `index`.
    fn tile_rect(&self, index: usize) -> Rect {
        let column = (index as i32) % self.columns;
        let row = (index as i32) / self.columns;
        let left = self.inset + column * (self.width + self.gap);
        let top = self.inset + row * (self.height + self.gap);
        Rect::new(left, top, left + self.width, top + self.height)
    }

    /// The tile index under local `(x, y)`, or `None` over a gap or past the
    /// end.
    fn index_at(&self, x: i32, y: i32, len: usize) -> Option<usize> {
        let (x, y) = (x - self.inset, y - self.inset);
        if x < 0 || y < 0 {
            return None;
        }
        let stride_x = self.width + self.gap;
        let stride_y = self.height + self.gap;
        if x % stride_x > self.width || y % stride_y > self.height {
            return None;
        }
        let column = x / stride_x;
        let row = y / stride_y;
        if column >= self.columns {
            return None;
        }
        let index = (row * self.columns + column) as usize;
        (index < len).then_some(index)
    }
}

/// The toolbox pane: a grid of icon tiles.
pub struct Toolbox<M: 'static> {
    control: Control<M>,
    state: Rc<RefCell<State>>,
    tools: Rc<Vec<Tool>>,
}

impl<M: 'static> Toolbox<M> {
    /// Creates a toolbox along `bounds`. `wrap` turns a [`ToolboxMsg`] into the
    /// host's message type; the host routes it back with
    /// [`Designer::handle_toolbox`](crate::Designer::handle_toolbox).
    pub fn new(
        ui: &Ui<M>,
        bounds: Rect,
        wrap: impl Fn(ToolboxMsg) -> M + 'static,
    ) -> Result<Toolbox<M>, BackendError> {
        let control = Control::new(ui, &NodeSpec::new(NodeKind::Custom, bounds).tab_stop())?;
        let state = Rc::new(RefCell::new(State::default()));
        let tools = Rc::new(tools());

        {
            let tools = Rc::clone(&tools);
            let state = Rc::clone(&state);
            let theme = ui.theme_handle();
            control.set_painter(Rc::new(move |canvas| {
                paint_local(canvas, |canvas, area| {
                    paint(canvas, area, &tools, &state.borrow(), &theme.get());
                });
            }));
        }
        {
            let tools = Rc::clone(&tools);
            let state = Rc::clone(&state);
            let wrap: Rc<dyn Fn(ToolboxMsg) -> M> = Rc::new(wrap);
            let ui = ui.clone();
            let id = control.id();
            control.on_events(move |event| toolbox_message(&ui, id, event, &tools, &state, &wrap));
        }

        Ok(Toolbox {
            control,
            state,
            tools,
        })
    }

    /// The toolbox's node identity.
    pub fn id(&self) -> WidgetId {
        self.control.id()
    }

    /// The number of entries (the pointer plus every control).
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the toolbox has no entries.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// The selected entry index (zero is the pointer).
    pub fn selected(&self) -> usize {
        self.state.borrow().selected
    }

    /// The selected tool.
    pub fn selected_tool(&self) -> Tool {
        self.tool_at(self.selected()).unwrap_or(Tool::Pointer)
    }

    /// The tool at `index`, if any.
    pub fn tool_at(&self, index: usize) -> Option<Tool> {
        self.tools.get(index).cloned()
    }

    /// The index of `tool`, if it is present.
    pub fn index_of(&self, tool: &Tool) -> Option<usize> {
        self.tools.iter().position(|entry| entry == tool)
    }

    /// Selects an entry without raising an event; an out-of-range index clears
    /// the selection to the pointer.
    pub fn select(&self, index: usize) {
        let index = if index < self.tools.len() { index } else { 0 };
        self.state.borrow_mut().selected = index;
        self.control.invalidate();
    }

    /// Moves/resizes the toolbox.
    pub fn set_bounds(&self, bounds: Rect) {
        self.control.set_bounds(bounds);
    }
}

/// Paints the tile background, the icon and the label for every entry, over
/// the toolbox's `area` in its own coordinates.
fn paint(canvas: &mut dyn Canvas, area: Rect, tools: &[Tool], state: &State, theme: &Theme) {
    let dpi = canvas.dpi();
    canvas.fill_rect(area, theme.surface);

    let layout = Layout::new(area, dpi);
    let icon = ICON_SIZE.to_px(dpi).value().max(1);
    for (index, tool) in tools.iter().enumerate() {
        let tile = layout.tile_rect(index);
        if index == state.selected {
            canvas.fill_rounded_rect(tile, TILE_RADIUS, theme.selection);
        } else if state.hover == Some(index) {
            canvas.fill_rounded_rect(tile, TILE_RADIUS, theme.hover);
        }

        let icon_rect = Rect::new(
            tile.left + 4,
            tile.top + (tile.height() - icon) / 2,
            tile.left + 4 + icon,
            tile.top + (tile.height() + icon) / 2,
        );
        draw_icon(canvas, tool.lucide(), icon_rect, theme.text, dpi);

        let label_rect = Rect::new(icon_rect.right + 6, tile.top, tile.right - 2, tile.bottom);
        let style = TextStyle::new(theme.text, LABEL_SIZE).middle();
        canvas.draw_text(tool.label(), label_rect, &style);
    }
}

/// Turns one input event into a [`ToolboxMsg`] wrapped into the host's `Msg`.
fn toolbox_message<M: 'static>(
    ui: &Ui<M>,
    id: WidgetId,
    event: &Event,
    tools: &[Tool],
    state: &Rc<RefCell<State>>,
    wrap: &Rc<dyn Fn(ToolboxMsg) -> M>,
) -> Option<M> {
    let dpi = ui.dpi();
    let bounds = ui.bounds(id);
    let area = Rect::new(0, 0, bounds.width(), bounds.height());
    let layout = Layout::new(area, dpi);

    match event {
        Event::MouseMove { x, y, .. } => {
            let next = layout.index_at(*x, *y, tools.len());
            if state.borrow().hover != next {
                state.borrow_mut().hover = next;
                ui.invalidate(id);
            }
            None
        }
        Event::MouseLeave | Event::CaptureChanged => {
            if state.borrow_mut().hover.take().is_some() {
                ui.invalidate(id);
            }
            None
        }
        Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            ..
        } => {
            let index = layout.index_at(*x, *y, tools.len())?;
            ui.focus(id);
            state.borrow_mut().selected = index;
            ui.invalidate(id);
            Some(wrap(ToolboxMsg::Select(tools[index].clone())))
        }
        Event::MouseDoubleClick {
            x,
            y,
            button: MouseButton::Left,
            ..
        } => {
            let index = layout.index_at(*x, *y, tools.len())?;
            Some(wrap(ToolboxMsg::Activate(tools[index].clone())))
        }
        Event::KeyDown {
            key,
            repeat,
            system,
            ..
        } if *repeat <= 1 && !*system => keyboard(*key, id, tools, state, &layout, ui, wrap),
        _ => None,
    }
}

/// Handles the toolbox's keyboard: Return drops the selected control and the
/// arrows move the selection.
fn keyboard<M: 'static>(
    key: Key,
    id: WidgetId,
    tools: &[Tool],
    state: &Rc<RefCell<State>>,
    layout: &Layout,
    ui: &Ui<M>,
    wrap: &Rc<dyn Fn(ToolboxMsg) -> M>,
) -> Option<M> {
    if tools.is_empty() {
        return None;
    }
    let len = tools.len();
    let selected = state.borrow().selected.min(len - 1);
    let index = match key {
        Key::RETURN | Key::SPACE => {
            return Some(wrap(ToolboxMsg::Activate(tools[selected].clone())));
        }
        Key::LEFT => selected.saturating_sub(1),
        Key::RIGHT => (selected + 1).min(len - 1),
        Key::UP => selected.saturating_sub(layout.columns as usize),
        Key::DOWN => (selected + layout.columns as usize).min(len - 1),
        _ => return None,
    };
    if index != selected {
        state.borrow_mut().selected = index;
        ui.invalidate(id);
        return Some(wrap(ToolboxMsg::Select(tools[index].clone())));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toolbox_lists_the_pointer_and_every_control() {
        let tools = tools();
        assert_eq!(tools.len(), CONTROL_KINDS.len() + 1);
        assert_eq!(tools[0], Tool::Pointer);
        assert_eq!(tools[1].kind(), Some("Button"));
        assert_eq!(tools[1].label(), "Button");
        assert_eq!(tools.last().and_then(Tool::kind), Some("ComboBox"));
    }

    #[test]
    fn the_pointer_has_no_kind() {
        assert_eq!(Tool::Pointer.kind(), None);
        assert_eq!(Tool::Pointer.label(), "Pointer");
    }

    #[test]
    fn every_control_kind_has_a_lucide_icon() {
        let expected = [
            ("Button", Lucide::SquareMousePointer),
            ("Edit", Lucide::TextCursorInput),
            ("Label", Lucide::Type),
            ("CheckBox", Lucide::SquareCheck),
            ("RadioGroup", Lucide::CircleDot),
            ("GroupBox", Lucide::Group),
            ("ListView", Lucide::List),
            ("ComboBox", Lucide::ChevronsUpDown),
        ];
        for (kind, icon) in expected {
            assert!(
                CONTROL_KINDS.contains(&kind),
                "{kind} is in the catalog mapping but not in CONTROL_KINDS"
            );
            assert_eq!(Tool::control(kind).lucide(), icon, "{kind}");
        }
        assert_eq!(Tool::Pointer.lucide(), Lucide::MousePointer2);
        // An unknown kind still draws a fallback rather than nothing.
        assert_eq!(Tool::control("Unknown").lucide(), Lucide::Square);
    }

    #[test]
    fn the_layout_wraps_tiles_into_columns() {
        // At 96dpi a tile is 96x28 with a 2px gap and a 4px inset from the pane's
        // edge, so a 202px-wide box holds two columns and a 201px one only one.
        let layout = Layout::new(Rect::new(0, 0, 202, 200), 96);
        assert_eq!(layout.columns, 2);
        assert_eq!(Layout::new(Rect::new(0, 0, 201, 200), 96).columns, 1);
        assert_eq!(layout.tile_rect(0), Rect::new(4, 4, 100, 32));
        assert_eq!(layout.tile_rect(1), Rect::new(102, 4, 198, 32));
        assert_eq!(layout.tile_rect(2), Rect::new(4, 34, 100, 62));
    }

    #[test]
    fn hit_testing_finds_tiles_and_skips_gaps() {
        let tools = tools();
        let layout = Layout::new(Rect::new(0, 0, 202, 200), 96);
        assert_eq!(layout.index_at(8, 8, tools.len()), Some(0));
        assert_eq!(layout.index_at(104, 8, tools.len()), Some(1));
        // The inset along the pane's edge and the 2px gap between the columns
        // are dead space.
        assert_eq!(layout.index_at(2, 8, tools.len()), None);
        assert_eq!(layout.index_at(101, 8, tools.len()), None);
        // Past the last tile.
        assert_eq!(layout.index_at(0, 500, tools.len()), None);
    }
}
