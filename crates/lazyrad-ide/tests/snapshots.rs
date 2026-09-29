#![forbid(unsafe_code)]

//! Headless snapshots of the IDE with the `examples/hello` project open.
//!
//! The test renders the window in the light and dark themes (1280x800 DIP),
//! writes `target/snapshots/ide-light.png` and `ide-dark.png`, and asserts on
//! pixels rather than golden files: every toolbar cell paints its icon, and the
//! Project Explorer paints an icon in each row's leading icon column. Both
//! themes are checked, so a colour that only works on one is caught.
//!
//! The app builds through [`IdeApp::build`] and opens the project with
//! [`IdeApp::open_project`], which bypasses the native folder dialog the same
//! way the in-crate tests set the session directly.

use std::path::{Path, PathBuf};

use lazyrad_ide::{IdeApp, Settings, ThemeChoice};
use xui_canvas::snapshot::{Snapshot, render_with};
use xui_core::backend::BackendError;
use xui_core::image::Image;
use xui_core::{Dip, Theme};

/// The snapshot window's size in design units.
const WIDTH: Dip = Dip(1280.0);
const HEIGHT: Dip = Dip(800.0);
/// The menu bar's height, matching the app; the toolbar sits right below it.
const MENU_HEIGHT: i32 = 24;
/// The toolbar's height.
const TOOLBAR_HEIGHT: i32 = 32;
/// The status bar's height, matching the app; it is carved off the bottom
/// before the split area, so every pane ends above it.
const STATUS_HEIGHT: i32 = 22;
/// The number of main-toolbar items.
const TOOLBAR_ITEMS: usize = 11;
/// The Project Explorer's icons live in the right-hand column, whose width is
/// the default 240 design units.
const RIGHT_COLUMN: i32 = 240;
/// The tree is placed 28px below the top of the Project pane.
const TREE_TOP_OFFSET: i32 = 28;
/// The tree row height, from xui's tree view at 96dpi.
const TREE_ROW: i32 = 22;
/// The tree's indent step and leading pad, at 96dpi.
const TREE_INDENT: i32 = 16;
const TREE_PAD: i32 = 4;
/// The xui toolbar icon side at 96dpi.
const TOOLBAR_ICON: i32 = 20;
/// The leading gap of a labelled toolbar item.
const TOOLBAR_GAP: i32 = 6;
/// The split divider thickness, matching the app.
const DIVIDER: i32 = 5;
/// The device-pixel height of a pane title, matching the app's `PANE_TITLE`.
const PANE_TITLE: i32 = 28;
/// The design width of a toolbox tile at 96dpi, matching the widget.
const TOOLBOX_TILE: i32 = 96;
/// The explorer's row depths: the project, a group, an item.
const ROW_DEPTHS: [u16; 5] = [0, 1, 2, 1, 2];

/// The workspace root, from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the workspace root exists")
}

/// The `examples/hello` project directory.
fn hello_project() -> PathBuf {
    workspace_root().join("examples").join("hello")
}

/// Default settings with `theme`. Defaults are never saved to disk, so the
/// render cannot touch the user's real settings file.
fn settings_with(theme: ThemeChoice) -> Settings {
    let mut settings = Settings::default();
    settings.theme = theme;
    settings
}

/// Renders the IDE with `settings` and the hello project open.
fn render_ide(settings: Settings, theme: Theme) -> Image {
    let hello = hello_project();
    render_with(
        Snapshot::new(WIDTH, HEIGHT).theme(theme).title("LazyRAD"),
        move |ui| -> Result<IdeApp, BackendError> {
            let mut app = IdeApp::build(ui, settings, Vec::new())?;
            app.open_project(&hello, ui);
            Ok(app)
        },
        |_stage| {},
    )
    .expect("the IDE snapshots")
}

/// Writes `image` under `target/snapshots/`, creating the directory.
fn save(image: &Image, name: &str) {
    let dir = workspace_root().join("target").join("snapshots");
    std::fs::create_dir_all(&dir).expect("the snapshot directory is writable");
    image
        .save_png(dir.join(name))
        .expect("the snapshot is written");
}

/// Whether any pixel in `(x, y, w, h)` differs from `background`. The caller
/// guarantees non-negative coordinates.
fn painted(image: &Image, x: i32, y: i32, w: i32, h: i32, background: [u8; 4]) -> bool {
    let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
        return false;
    };
    let (Ok(w), Ok(h)) = (u32::try_from(w), u32::try_from(h)) else {
        return false;
    };
    (y..y + h)
        .any(|row| (x..x + w).any(|col| image.pixel(col, row).is_some_and(|px| px != background)))
}

/// The window background at a point every theme leaves unpainted (top-left of
/// the toolbar strip, above the first cell's icon).
fn toolbar_background(image: &Image) -> [u8; 4] {
    image
        .pixel(1, u32::try_from(MENU_HEIGHT + 1).unwrap())
        .expect("the point is inside the window")
}

/// The tree background at a point left of the first row's icon.
fn tree_background(image: &Image, tree_left: u32, tree_top: u32) -> [u8; 4] {
    image
        .pixel(tree_left + 1, tree_top + 2)
        .expect("the point is inside the window")
}

#[test]
fn every_toolbar_cell_paints_its_icon_in_both_themes() {
    for (theme, choice, name) in [
        (Theme::light(), ThemeChoice::Light, "ide-light.png"),
        (Theme::dark(), ThemeChoice::Dark, "ide-dark.png"),
    ] {
        let settings = settings_with(choice);
        let image = render_ide(settings, theme);
        save(&image, name);
        assert_eq!((image.width(), image.height()), (1280, 800));

        let bg = toolbar_background(&image);
        let width = i32::try_from(image.width()).unwrap();
        let toolbar_top = MENU_HEIGHT;
        for index in 0..TOOLBAR_ITEMS {
            let start = width * index as i32 / TOOLBAR_ITEMS as i32;
            let end = width * (index + 1) as i32 / TOOLBAR_ITEMS as i32;
            let cell = end - start;
            // Icon-only items centre the icon; Run and End lead with it.
            let left = if index + 2 >= TOOLBAR_ITEMS {
                start + TOOLBAR_GAP
            } else {
                start + (cell - TOOLBAR_ICON) / 2
            };
            let top = toolbar_top + (TOOLBAR_HEIGHT - TOOLBAR_ICON) / 2;
            assert!(
                painted(&image, left, top, TOOLBAR_ICON, TOOLBAR_ICON, bg),
                "toolbar item {index} painted no icon in {name}"
            );
        }
    }
}

#[test]
fn every_explorer_row_paints_its_icon_in_both_themes() {
    for (theme, choice) in [
        (Theme::light(), ThemeChoice::Light),
        (Theme::dark(), ThemeChoice::Dark),
    ] {
        let settings = settings_with(choice);
        // Only the toolbar test writes the PNGs, so parallel tests never race
        // on the same file; this one renders and inspects.
        let image = render_ide(settings, theme);

        let width = i32::try_from(image.width()).unwrap();
        // The right column is the default 240 design units wide, so the Project
        // pane starts there; the tree sits 28px below its top.
        let tree_left = width - RIGHT_COLUMN;
        let tree_top = MENU_HEIGHT + TOOLBAR_HEIGHT + TREE_TOP_OFFSET;
        let bg = tree_background(&image, u32::try_from(tree_left).unwrap(), tree_top as u32);

        for (row, depth) in ROW_DEPTHS.into_iter().enumerate() {
            // xui reserves the chevron slot then indents by depth; the icon is
            // 16px into the row's content box.
            let icon_left = tree_left + TREE_PAD + i32::from(depth) * TREE_INDENT + TREE_INDENT;
            let top = tree_top + row as i32 * TREE_ROW;
            let side = 16;
            assert!(
                painted(&image, icon_left, top + 3, side, side, bg),
                "explorer row {row} (depth {depth}) painted no icon"
            );
        }
    }
}

/// Whether the text label at `rect` (in the pane's own coordinates) painted
/// inside the pane at `(pane_left, pane_top)`. A missing label leaves the pane's
/// uniform surface there, so the rectangle's first pixel stands for it.
fn label_painted(image: &Image, pane_left: i32, pane_top: i32, rect: (i32, i32, i32, i32)) -> bool {
    let (left, top, right, bottom) = rect;
    let (x, y) = (pane_left + left, pane_top + top);
    let bg = image
        .pixel(x as u32, y as u32)
        .expect("the label is inside the window");
    painted(image, x, y, right - left, bottom - top, bg)
}

#[test]
fn every_pane_shows_its_title() {
    let settings = settings_with(ThemeChoice::Light);
    let panes = settings.panes;
    let image = render_ide(settings, Theme::light());

    let width = i32::try_from(image.width()).unwrap();
    let height = i32::try_from(image.height()).unwrap();
    let main_top = MENU_HEIGHT + TOOLBAR_HEIGHT;
    let right_left = width - RIGHT_COLUMN;
    // The centre row ends above the Output pane, its divider and the status
    // bar; the Properties pane is the bottom of the right column within it.
    let centre_bottom = height - STATUS_HEIGHT - panes.output as i32 - DIVIDER;
    let properties_top = centre_bottom - panes.project as i32;

    for (name, left, top, rect) in [
        ("Toolbox", 0, main_top, (8, 6, 220, 26)),
        ("Project", right_left, main_top, (8, 6, 220, 26)),
        ("Properties", right_left, properties_top, (8, 6, 220, 26)),
    ] {
        assert!(
            label_painted(&image, left, top, rect),
            "{name} is not painted inside its pane"
        );
    }
}

#[test]
fn the_toolbox_tiles_and_property_grid_paint_in_their_panes() {
    for (theme, choice) in [
        (Theme::light(), ThemeChoice::Light),
        (Theme::dark(), ThemeChoice::Dark),
    ] {
        let settings = settings_with(choice);
        let panes = settings.panes;
        let image = render_ide(settings, theme);

        let width = i32::try_from(image.width()).unwrap();
        let height = i32::try_from(image.height()).unwrap();
        let main_top = MENU_HEIGHT + TOOLBAR_HEIGHT;
        let right_left = width - RIGHT_COLUMN;

        // The toolbox tiles start below the pane title; sample the empty
        // surface to the right of the one-column 96px tiles.
        let tile_top = main_top + PANE_TITLE;
        let surface = image
            .pixel(
                u32::try_from(120).unwrap(),
                u32::try_from(tile_top + 4).unwrap(),
            )
            .expect("the toolbox surface is inside the window");
        assert!(
            painted(&image, 2, tile_top + 4, TOOLBOX_TILE - 4, 20, surface),
            "the toolbox painted no tiles in {choice:?}"
        );

        // The property grid fills the Properties pane below its title.
        let centre_bottom = height - panes.output as i32 - DIVIDER;
        let properties_top = centre_bottom - panes.project as i32;
        let grid_top = properties_top + PANE_TITLE;
        let grid_bg = image
            .pixel(
                u32::try_from(right_left + 2).unwrap(),
                u32::try_from(grid_top + 2).unwrap(),
            )
            .expect("the property grid surface is inside the window");
        assert!(
            painted(
                &image,
                right_left + 4,
                grid_top + 6,
                RIGHT_COLUMN - 8,
                60,
                grid_bg
            ),
            "the property grid painted nothing in {choice:?}"
        );
    }
}

#[test]
fn the_two_themes_differ() {
    let light = render_ide(settings_with(ThemeChoice::Light), Theme::light());
    let dark = render_ide(settings_with(ThemeChoice::Dark), Theme::dark());
    assert_ne!(light.pixels(), dark.pixels(), "light and dark differ");
}
