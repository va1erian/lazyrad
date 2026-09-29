//! The property grid's vertical scrollbar, driven with real pointer events and
//! rendered headlessly in both themes.
//!
//! The grid sits at a known place in a 560x300 window, so its local
//! coordinates and the window's differ by a fixed offset: every event goes in
//! window coordinates and every rectangle the grid reports is grid-local.

use std::cell::RefCell;
use std::rc::Rc;

use xui_canvas::snapshot::{Snapshot, Stage, render_with};
use xui_core::app::{App, Ui};
use xui_core::backend::{BackendError, Event};
use xui_core::geometry::{Point, Rect};
use xui_core::{Dip, Modifiers, MouseButton, Theme};
use xui_form::{FormDoc, Node, Value};

use lazyrad_designer::{Designer, DesignerMsg, PropertyGrid, PropertyGridMsg};

#[derive(Clone, Debug)]
enum Msg {
    Designer(DesignerMsg),
    Grid(PropertyGridMsg),
}

struct Editor {
    designer: Rc<RefCell<Designer<Msg>>>,
    grid: Rc<PropertyGrid<Msg>>,
}

impl App for Editor {
    type Msg = Msg;

    fn update(&mut self, msg: Msg, ui: &mut Ui<Msg>) {
        match msg {
            Msg::Designer(msg) => self.designer.borrow().update(msg, ui),
            Msg::Grid(msg) => self.grid.update(msg, ui),
        }
    }
}

/// The window's size in design units (96 dpi, so also pixels).
const WIDTH: f32 = 560.0;
const HEIGHT: f32 = 300.0;
/// Where the grid sits in the window.
const ORIGIN: Point = Point { x: 340, y: 20 };

fn button_doc() -> FormDoc {
    let mut doc = FormDoc::new("main_form");
    let mut button = Node::new("Button", "ok_button");
    button.set_prop("left", Value::Int(16));
    button.set_prop("top", Value::Int(16));
    button.set_prop("width", Value::Int(80));
    button.set_prop("height", Value::Int(24));
    button.set_prop("text", Value::Text("Go".into()));
    doc.insert(button);
    doc
}

/// Builds the grid `height` pixels tall and hands `step` its `Stage`; returns
/// what `step` returns and the final frame.
fn run<R: 'static>(
    theme: Theme,
    height: i32,
    step: impl FnOnce(&Stage<'_, Msg>, &PropertyGrid<Msg>) -> R + 'static,
) -> (R, xui_core::Image) {
    let out: Rc<RefCell<Option<R>>> = Rc::new(RefCell::new(None));
    let sink = Rc::clone(&out);
    let grid_slot: Rc<RefCell<Option<Rc<PropertyGrid<Msg>>>>> = Rc::new(RefCell::new(None));
    let grid_for_build = Rc::clone(&grid_slot);
    let image = render_with(
        Snapshot::new(Dip(WIDTH), Dip(HEIGHT)).theme(theme),
        move |ui| -> Result<Editor, BackendError> {
            let catalog = Rc::new(lazyrad_project::lazyrad_catalog());
            let designer = Designer::new(
                ui,
                Rect::new(0, 0, 320, 200),
                button_doc(),
                Rc::clone(&catalog),
                Msg::Designer,
            )
            .expect("the designer builds");
            let designer = Rc::new(RefCell::new(designer));
            let grid = PropertyGrid::new(
                ui,
                Rect::new(ORIGIN.x, ORIGIN.y, ORIGIN.x + 216, ORIGIN.y + height),
                Rc::clone(&designer),
                catalog,
                Msg::Grid,
            )
            .expect("the grid builds");
            designer.borrow().select_node("ok_button", ui);
            let grid = Rc::new(grid);
            *grid_for_build.borrow_mut() = Some(Rc::clone(&grid));
            Ok(Editor { designer, grid })
        },
        move |stage| {
            let grid = grid_slot.borrow().clone().expect("the grid was built");
            *sink.borrow_mut() = Some(step(stage, &grid));
        },
    )
    .expect("the grid renders");
    let result = out.borrow_mut().take().expect("the step ran");
    (result, image)
}

fn window(local: Point) -> Point {
    Point::new(ORIGIN.x + local.x, ORIGIN.y + local.y)
}

fn centre(rect: Rect) -> Point {
    Point::new((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2)
}

fn mouse(stage: &Stage<'_, Msg>, kind: &str, at: Point) {
    let (x, y) = (at.x, at.y);
    let modifiers = Modifiers::NONE;
    let button = MouseButton::Left;
    stage.inject(match kind {
        "down" => Event::MouseDown {
            x,
            y,
            button,
            modifiers,
        },
        "up" => Event::MouseUp {
            x,
            y,
            button,
            modifiers,
        },
        _ => Event::MouseMove { x, y, modifiers },
    });
}

/// The RGB the image has at window point `at`.
fn pixel(image: &xui_core::Image, at: Point) -> [u8; 3] {
    let px = image
        .pixel(at.x as u32, at.y as u32)
        .expect("the point is in the window");
    [px[0], px[1], px[2]]
}

fn rgb(color: xui_core::Color) -> [u8; 3] {
    [color.r, color.g, color.b]
}

#[test]
fn the_bar_appears_only_when_the_rows_overflow() {
    // A short grid overflows; a tall one shows every row.
    let (short, _) = run(Theme::light(), 110, |stage, grid| {
        (
            grid.scrollbar_track(stage.ui()),
            grid.scrollbar_thumb(stage.ui()),
        )
    });
    let (track, thumb) = short;
    let track = track.expect("a 110px grid cannot show every row");
    let thumb = thumb.expect("an overflowing grid has a thumb");
    assert_eq!(track.right, 216, "the bar hugs the grid's right edge");
    assert!(thumb.top >= track.top && thumb.bottom <= track.bottom);
    assert!(thumb.height() < track.height());

    let (none, _) = run(Theme::light(), 800, |stage, grid| {
        grid.scrollbar_track(stage.ui())
    });
    assert!(none.is_none(), "no bar when every row fits an 800px grid");
}

#[test]
fn dragging_the_thumb_maps_to_the_offset_and_clamps_at_both_ends() {
    let ((mid, bottom, top, max), _) = run(Theme::light(), 110, |stage, grid| {
        let ui = stage.ui();
        let track = grid.scrollbar_track(ui).expect("the bar shows");
        let thumb = grid.scrollbar_thumb(ui).expect("the thumb shows");
        // The largest offset, found through the wheel path, then back to 0.
        grid.update(PropertyGridMsg::Scroll(1_000_000), ui);
        let max = grid.scroll_offset();
        grid.update(PropertyGridMsg::Scroll(-1_000_000), ui);

        let start = centre(thumb);
        mouse(stage, "down", window(start));
        // Half the thumb's travel is half the content's overflow.
        let travel = track.height() - thumb.height();
        mouse(
            stage,
            "move",
            window(Point::new(start.x, start.y + travel / 2)),
        );
        let mid = grid.scroll_offset();
        // Far past either end of the track, even outside the grid.
        mouse(
            stage,
            "move",
            window(Point::new(start.x + 300, start.y + 5000)),
        );
        let bottom = grid.scroll_offset();
        mouse(stage, "move", window(Point::new(start.x, start.y - 5000)));
        let top = grid.scroll_offset();
        mouse(stage, "up", window(Point::new(start.x, start.y - 5000)));
        (mid, bottom, top, max)
    });
    assert!(max > 0);
    assert!((mid - max / 2).abs() <= max / 20 + 2, "mid {mid} of {max}");
    assert_eq!(bottom, max, "dragging past the end stops at the last row");
    assert_eq!(top, 0, "dragging past the start stops at the first row");
}

#[test]
fn moving_after_the_release_no_longer_drags() {
    let (after, _) = run(Theme::light(), 110, |stage, grid| {
        let ui = stage.ui();
        let thumb = grid.scrollbar_thumb(ui).expect("the thumb shows");
        let start = centre(thumb);
        mouse(stage, "down", window(start));
        mouse(stage, "up", window(start));
        mouse(stage, "move", window(Point::new(start.x, start.y + 40)));
        grid.scroll_offset()
    });
    assert_eq!(after, 0, "a release ends the drag");
}

#[test]
fn clicking_the_track_pages_and_clamps() {
    let ((down, up, max, page), _) = run(Theme::light(), 110, |stage, grid| {
        let ui = stage.ui();
        let track = grid.scrollbar_track(ui).expect("the bar shows");
        let thumb = grid.scrollbar_thumb(ui).expect("the thumb shows");
        grid.update(PropertyGridMsg::Scroll(1_000_000), ui);
        let max = grid.scroll_offset();
        grid.update(PropertyGridMsg::Scroll(-1_000_000), ui);
        // A page is the body less one row, so consecutive pages overlap.
        let page = track.height() - 22;

        // Below the thumb pages down; repeated clicks stop at the end.
        let below = Point::new(track.left + 6, track.bottom - 1);
        assert!(below.y >= thumb.bottom, "the click is below the thumb");
        mouse(stage, "down", window(below));
        mouse(stage, "up", window(below));
        let down = grid.scroll_offset();
        for _ in 0..50 {
            mouse(stage, "down", window(below));
            mouse(stage, "up", window(below));
        }
        assert_eq!(grid.scroll_offset(), max, "paging stops at the last row");

        // Above the thumb pages back up.
        let above = Point::new(track.left + 6, track.top);
        mouse(stage, "down", window(above));
        mouse(stage, "up", window(above));
        let up = grid.scroll_offset();
        (down, up, max, page)
    });
    assert_eq!(down, page.min(max), "one click pages by a page");
    assert_eq!(up, (max - page).max(0), "a click above pages back");
}

#[test]
fn rows_and_the_bar_do_not_share_pixels() {
    let (edited, _) = run(Theme::light(), 110, |stage, grid| {
        let ui = stage.ui();
        let track = grid.scrollbar_track(ui).expect("the bar shows");
        let y = 62; // the first row: body top 58, rows 22px
        // A click on the bar's column never begins an edit.
        let on_bar = Point::new(track.left + 6, y);
        mouse(stage, "down", window(on_bar));
        mouse(stage, "up", window(on_bar));
        let after_bar = grid.current_row();
        // A click just left of it does.
        let beside = Point::new(track.left - 2, y);
        mouse(stage, "down", window(beside));
        mouse(stage, "up", window(beside));
        (after_bar, grid.current_row())
    });
    assert_eq!(edited.0, None, "the bar's column is not a row");
    assert_eq!(edited.1, Some(0), "the column beside it still is");
}

#[test]
fn the_thumb_paints_in_both_themes_and_shows_hover_and_pressed() {
    for theme in [Theme::light(), Theme::dark()] {
        let bar_point = |grid: &PropertyGrid<Msg>, ui: &Ui<Msg>| {
            let thumb = grid.scrollbar_thumb(ui).expect("the thumb shows");
            window(centre(thumb))
        };
        let (at, image) = run(theme, 110, move |stage, grid| bar_point(grid, stage.ui()));
        assert_eq!(pixel(&image, at), rgb(theme.scrollbar), "resting thumb");

        let (at, image) = run(theme, 110, move |stage, grid| {
            let at = bar_point(grid, stage.ui());
            mouse(stage, "move", at);
            at
        });
        let hover = pixel(&image, at);
        assert_ne!(hover, rgb(theme.scrollbar), "a hovered thumb changes");

        let (at, image) = run(theme, 110, move |stage, grid| {
            let at = bar_point(grid, stage.ui());
            mouse(stage, "down", at);
            at
        });
        let pressed = pixel(&image, at);
        assert_ne!(pressed, hover, "a pressed thumb differs from a hovered one");
        assert_ne!(pressed, rgb(theme.scrollbar));
    }
}
