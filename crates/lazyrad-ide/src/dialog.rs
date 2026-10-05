#![forbid(unsafe_code)]

//! A small in-window choice dialog: a scrim, a card and up to three buttons.
//!
//! xui's own [`Dialog`](xui_core::widget::Dialog) covers messages, confirms and
//! prompts, but the "save changes?" prompt needs three outcomes (Save, Discard,
//! Cancel) and custom labels, which that widget does not offer. Canvas has no
//! modal windows either (PLAN.md §10, gap G14), so this is an in-window card
//! that covers the client area and takes the focus, exactly like xui's dialog
//! does. It is deliberately generic: the app passes the button labels and maps
//! the chosen index to its own message.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use xui_core::app::Ui;
use xui_core::arrange::{Entry, Handle, IntoEntry, LayoutExt, Mounted, absolute, build, button};
use xui_core::backend::{Event, NodeKind, NodeSpec, Result, TextStyle, WidgetId};
use xui_core::geometry::Rect;
use xui_core::message::Key;
use xui_core::theme::Theme;
use xui_core::units::{Dip, Px};
use xui_core::widget::{Button, Control};

/// Padding between the card edge and its content.
const PADDING: Dip = Dip(16.0);
/// Vertical gap between the card's blocks.
const GAP: Dip = Dip(12.0);
/// The smallest and largest card widths.
const MIN_WIDTH: Dip = Dip(280.0);
const MAX_WIDTH: Dip = Dip(460.0);
/// The button size.
const BUTTON_WIDTH: Dip = Dip(88.0);
const BUTTON_HEIGHT: Dip = Dip(28.0);
/// The margin kept between the card and the window edges.
const MARGIN: Dip = Dip(24.0);
/// The title and message text sizes.
const TITLE_SIZE: Dip = Dip(15.0);
const MESSAGE_SIZE: Dip = Dip(12.0);
/// The card's corner radius, in pixels.
const RADIUS: f32 = 8.0;

/// Maps the chosen button's index to an optional app message.
type ActionMapper<M> = RefCell<Option<Box<dyn Fn(usize) -> Option<M>>>>;

/// The geometry the painter draws from.
#[derive(Clone, Copy, Default)]
struct Layout {
    card: Rect,
    title: Rect,
    message: Rect,
    visible: bool,
}

/// State the painter, the key listeners and the buttons share.
struct Shared<M: 'static> {
    ui: Ui<M>,
    /// Every node the dialog owns, shown and hidden together.
    nodes: RefCell<Vec<WidgetId>>,
    title: RefCell<String>,
    message: RefCell<String>,
    action: ActionMapper<M>,
    open: Cell<bool>,
    /// The index Enter picks, and the index Escape picks.
    accept: Cell<usize>,
    cancel: Cell<usize>,
}

/// A modal, in-window card with up to three labelled buttons.
pub struct ChoiceDialog<M: 'static> {
    shared: Rc<Shared<M>>,
    layout: Rc<Cell<Layout>>,
    scrim: Control<M>,
    buttons: Vec<Rc<Button<M>>>,
    /// The layout that places the buttons on the card, mounted again each
    /// time the dialog opens.
    placed: RefCell<Mounted<M>>,
}

impl<M: 'static> ChoiceDialog<M> {
    /// Builds a hidden dialog with `buttons`' labels.
    ///
    /// `accept` is the index Enter picks and the one given the initial focus;
    /// `cancel` is the index Escape picks. Both must be valid indices.
    pub fn new(
        ui: &Ui<M>,
        title: &str,
        message: &str,
        labels: &[&str],
        accept: usize,
        cancel: usize,
    ) -> Result<ChoiceDialog<M>> {
        assert!(!labels.is_empty(), "a dialog needs at least one button");
        assert!(accept < labels.len() && cancel < labels.len());

        let shared = Rc::new(Shared {
            ui: ui.clone(),
            nodes: RefCell::new(Vec::new()),
            title: RefCell::new(title.to_string()),
            message: RefCell::new(message.to_string()),
            action: RefCell::new(None),
            open: Cell::new(false),
            accept: Cell::new(accept),
            cancel: Cell::new(cancel),
        });
        let scrim = Control::new(ui, &NodeSpec::new(NodeKind::Custom, Rect::default()))?;
        shared.nodes.borrow_mut().push(scrim.id());

        let layout = Rc::new(Cell::new(Layout::default()));
        {
            let shared = Rc::clone(&shared);
            let layout = Rc::clone(&layout);
            let theme = ui.theme_handle();
            scrim.set_painter(Rc::new(move |canvas| {
                let theme = theme.get();
                let layout = layout.get();
                canvas.clear(scrim_color(theme));
                if !layout.visible {
                    return;
                }
                canvas.fill_rounded_rect(layout.card, RADIUS, theme.raised);
                canvas.stroke_rounded_rect(layout.card, RADIUS, theme.border, 1.0);
                let title = shared.title.borrow();
                canvas.draw_text(
                    &title,
                    layout.title,
                    &TextStyle::new(theme.text, TITLE_SIZE).bold(),
                );
                let message = shared.message.borrow();
                canvas.draw_text(
                    &message,
                    layout.message,
                    &TextStyle::new(theme.text_secondary, MESSAGE_SIZE).wrapped(),
                );
            }));
        }

        {
            let shared = Rc::clone(&shared);
            let scrim_ui = ui.clone();
            scrim.on_events(move |event| {
                if scrim_ui.is_design_mode() && event.is_input() {
                    return None;
                }
                on_key(&shared, event)
            });
        }

        let handles: Vec<Handle<Button<M>>> = labels.iter().map(|_| Handle::new()).collect();
        let mut entries = Vec::new();
        for (index, (label, handle)) in labels.iter().zip(&handles).enumerate() {
            let shared = Rc::clone(&shared);
            entries.push(
                button(*label)
                    .on_click_with(move || dismiss(&shared, index))
                    .bind(handle)
                    .into_entry(),
            );
        }
        // The buttons go where `open` places them; until then they are hidden.
        let placed = ui.mount(absolute().children(entries))?;
        let buttons: Vec<Rc<Button<M>>> = handles.iter().map(Handle::get).collect();
        {
            let mut nodes = shared.nodes.borrow_mut();
            for button in &buttons {
                nodes.push(button.id());
            }
            for id in nodes.iter() {
                ui.set_visible(*id, false);
            }
        }

        Ok(ChoiceDialog {
            shared,
            layout,
            scrim,
            buttons,
            placed: RefCell::new(placed),
        })
    }

    /// Maps a chosen button's index to the app's message.
    pub fn on_action(self, mapper: impl Fn(usize) -> Option<M> + 'static) -> ChoiceDialog<M> {
        *self.shared.action.borrow_mut() = Some(Box::new(mapper));
        self
    }

    /// Replaces the message text; a visible dialog rewrites its card.
    pub fn set_message(&self, message: &str) {
        *self.shared.message.borrow_mut() = message.to_string();
        if self.shared.open.get() {
            self.open();
        }
    }

    /// Opens the dialog: centre the card, raise it and focus a button.
    pub fn open(&self) {
        let ui = &self.shared.ui;
        let title = self.shared.title.borrow().clone();
        let message = self.shared.message.borrow().clone();
        let button_ids: Vec<WidgetId> = self.buttons.iter().map(|button| button.id()).collect();
        let placement = place(ui, self.scrim.id(), &button_ids, &title, &message);

        // Shown first: a layout leaves hidden widgets out.
        for id in self.shared.nodes.borrow().iter() {
            ui.set_visible(*id, true);
        }
        let (scrim, buttons) = placement.moves.split_at(1);
        ui.apply_moves(scrim);
        self.place_buttons(ui, buttons);
        self.layout.set(placement.layout);
        ui.raise(self.scrim.id());
        for button in &self.buttons {
            ui.raise(button.id());
        }
        // Focus the scrim, not a button: the scrim's own listener handles
        // Enter and Escape, and buttons keep their click handlers.
        ui.focus(self.scrim.id());
        self.shared.open.set(true);
        ui.invalidate(self.scrim.id());
    }

    /// Mounts the buttons again at `moves`' rectangles (device pixels).
    fn place_buttons(&self, ui: &Ui<M>, moves: &[(WidgetId, Rect)]) {
        let dpi = ui.dpi();
        let dip = |px: i32| Px(px).to_dip(dpi);
        let entries: Vec<Entry<M>> = self
            .buttons
            .iter()
            .zip(moves)
            .map(|(button, (_, rect))| {
                let button = Rc::clone(button);
                build(move |_| Ok(button)).at(
                    dip(rect.left),
                    dip(rect.top),
                    dip(rect.width()),
                    dip(rect.height()),
                )
            })
            .collect();
        // Re-mounting widgets the dialog already holds creates nothing, so it
        // cannot fail.
        if let Ok(placed) = ui.mount(absolute().children(entries)) {
            self.placed.replace(placed);
        }
    }

    /// Closes the dialog without raising an action.
    pub fn close(&self) {
        if self.shared.open.replace(false) {
            hide(&self.shared);
        }
    }

    /// Whether the dialog is currently open.
    pub fn is_open(&self) -> bool {
        self.shared.open.get()
    }

    /// The scrim's node identity.
    pub fn id(&self) -> WidgetId {
        self.scrim.id()
    }
}

/// The layout of the card and the moves that place every node.
struct Placement {
    layout: Layout,
    moves: Vec<(WidgetId, Rect)>,
}

/// Centres the card in the client area and lays the buttons out right-aligned,
/// the last one rightmost.
fn place<M: 'static>(
    ui: &Ui<M>,
    scrim: WidgetId,
    buttons: &[WidgetId],
    title: &str,
    message: &str,
) -> Placement {
    let dpi = ui.dpi();
    let px = |value: Dip| value.to_px(dpi).value();
    let client = ui.client_rect();
    let theme = ui.theme();

    let pad = px(PADDING);
    let gap = px(GAP);
    let button_w = px(BUTTON_WIDTH);
    let button_h = px(BUTTON_HEIGHT);
    let margin = px(MARGIN);
    let title_metrics = ui.measure_text(title, &TextStyle::new(theme.text, TITLE_SIZE).bold(), dpi);
    let message_metrics = ui.measure_text(message, &TextStyle::new(theme.text, MESSAGE_SIZE), dpi);

    let count = buttons.len();
    let row_w = row_width(count, button_w, gap);
    // The card never grows past the window, but is never squeezed below the
    // minimum width unless the window itself is narrower than that.
    let avail = (client.width() - margin * 2).max(px(MIN_WIDTH).min(client.width()));
    let wanted = title_metrics.width.max(message_metrics.width).max(row_w) + pad * 2;
    let card_w = wanted.clamp(px(MIN_WIDTH), px(MAX_WIDTH)).min(avail);
    let content_w = (card_w - pad * 2).max(1);
    let lines = if message_metrics.width > content_w {
        (message_metrics.width + content_w - 1) / content_w
    } else {
        1
    };
    let message_h = message_metrics.height.max(1) * lines;
    let card_h = pad * 2 + title_metrics.height + gap + message_h + gap + button_h;

    let left = client.left + (client.width() - card_w).max(0) / 2;
    let top = client.top + (client.height() - card_h).max(0) / 2;
    let card = Rect::new(left, top, left + card_w, top + card_h);
    let title_rect = Rect::new(
        card.left + pad,
        card.top + pad,
        card.right - pad,
        card.top + pad + title_metrics.height,
    );
    let message_rect = Rect::new(
        card.left + pad,
        title_rect.bottom + gap,
        card.right - pad,
        title_rect.bottom + gap + message_h,
    );
    let row_top = card.bottom - pad - button_h;

    // A window narrower than the row shrinks the buttons to fit the card.
    let (button_w, gap) = fitted_row(count, button_w, gap, content_w);
    let mut moves = vec![(scrim, client)];
    for (index, button) in buttons.iter().enumerate() {
        let offset = (count - 1 - index) as i32 * (button_w + gap);
        let right = card.right - pad - offset;
        moves.push((
            *button,
            Rect::new(right - button_w, row_top, right, row_top + button_h),
        ));
    }

    Placement {
        layout: Layout {
            card,
            title: title_rect,
            message: message_rect,
            visible: true,
        },
        moves,
    }
}

/// The width of `count` buttons of `button_w` with `gap` between them.
fn row_width(count: usize, button_w: i32, gap: i32) -> i32 {
    count as i32 * button_w + count.saturating_sub(1) as i32 * gap
}

/// The button width and gap to use: the given ones, or smaller when the row
/// would not fit in `content_w`. The buttons keep at least 1px each, and the
/// gap gives way first so a hopelessly small card still holds the whole row.
fn fitted_row(count: usize, button_w: i32, gap: i32, content_w: i32) -> (i32, i32) {
    if row_width(count, button_w, gap) <= content_w {
        return (button_w, gap);
    }
    let n = count.max(1) as i32;
    let between = count.saturating_sub(1) as i32;
    let gap = if between == 0 {
        0
    } else {
        gap.min((content_w - n).max(0) / between)
    };
    (((content_w - gap * between) / n).max(1), gap)
}

/// Handles Escape and Enter while the dialog is open.
fn on_key<M: 'static>(shared: &Shared<M>, event: &Event) -> Option<M> {
    if !shared.open.get() {
        return None;
    }
    let Event::KeyDown {
        key,
        repeat,
        system,
        ..
    } = event
    else {
        return None;
    };
    if *repeat > 1 || *system {
        return None;
    }
    match *key {
        Key::ESCAPE => dismiss(shared, shared.cancel.get()),
        Key::RETURN => dismiss(shared, shared.accept.get()),
        _ => None,
    }
}

/// Hides the dialog and raises the chosen button's action once.
fn dismiss<M: 'static>(shared: &Shared<M>, index: usize) -> Option<M> {
    if !shared.open.replace(false) {
        return None;
    }
    hide(shared);
    let mapper = shared.action.borrow();
    mapper.as_ref().and_then(|mapper| mapper(index))
}

/// Hides every node the dialog owns.
fn hide<M: 'static>(shared: &Shared<M>) {
    for id in shared.nodes.borrow().iter() {
        shared.ui.set_visible(*id, false);
    }
    shared.ui.invalidate(
        shared
            .nodes
            .borrow()
            .first()
            .copied()
            .unwrap_or(WidgetId::NONE),
    );
}

/// The scrim colour: the window background darkened. Opaque, because a painted
/// child window does not blend with the widgets behind it on every backend.
fn scrim_color(theme: Theme) -> xui_core::Color {
    theme.background.lerp(xui_core::Color::rgb(0, 0, 0), 0.35)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xui_canvas::snapshot::{Snapshot, try_render};
    use xui_core::app::App;
    use xui_core::{Dip, Theme};

    /// Keeps the dialog alive while the snapshot renders.
    struct Host(#[allow(dead_code)] ChoiceDialog<()>);

    impl App for Host {
        type Msg = ();
        fn update(&mut self, _msg: (), _ui: &mut Ui<()>) {}
    }

    /// The card and the button bounds a `labels` dialog gets in a window of
    /// `width` x 600 DIP, opened with `message`.
    fn placed(theme: Theme, width: f32, message: &str, labels: &[&str]) -> (Rect, Vec<Rect>) {
        let out = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&out);
        let message = message.to_string();
        let labels: Vec<String> = labels.iter().map(|l| l.to_string()).collect();
        try_render(
            Snapshot::new(Dip(width), Dip(600.0)).theme(theme),
            move |ui| -> Result<Host> {
                let names: Vec<&str> = labels.iter().map(String::as_str).collect();
                let dialog =
                    ChoiceDialog::new(ui, "Save changes?", &message, &names, 0, names.len() - 1)?;
                dialog.open();
                let bounds = dialog.buttons.iter().map(|b| ui.bounds(b.id())).collect();
                *sink.borrow_mut() = Some((dialog.layout.get().card, bounds));
                Ok(Host(dialog))
            },
        )
        .expect("the dialog renders");
        let placed = out.borrow_mut().take();
        placed.expect("the build ran")
    }

    fn assert_inside(card: Rect, buttons: &[Rect], what: &str) {
        for (index, button) in buttons.iter().enumerate() {
            assert!(
                button.left >= card.left
                    && button.right <= card.right
                    && button.top >= card.top
                    && button.bottom <= card.bottom,
                "{what}: button {index} {button:?} sticks out of the card {card:?}"
            );
            assert!(button.width() > 0, "{what}: button {index} has no width");
        }
        for pair in buttons.windows(2) {
            assert!(pair[0].right <= pair[1].left, "{what}: buttons overlap");
        }
    }

    const SAVE: [&str; 3] = ["Save", "Discard", "Cancel"];

    #[test]
    fn three_buttons_fit_a_card_sized_for_a_short_message() {
        for (theme, name) in [(Theme::light(), "light"), (Theme::dark(), "dark")] {
            let (card, buttons) = placed(theme, 1280.0, "Save changes to a?", &SAVE);
            assert_eq!(buttons.len(), 3);
            assert_inside(card, &buttons, name);
            // The card grew to hold the row instead of the buttons shrinking.
            assert!(buttons.iter().all(|b| b.width() == 88), "{name}");
        }
    }

    #[test]
    fn a_narrow_window_shrinks_the_buttons_into_the_card() {
        for (theme, name) in [(Theme::light(), "light"), (Theme::dark(), "dark")] {
            let (card, buttons) = placed(theme, 360.0, "Save changes to a?", &SAVE);
            assert_inside(card, &buttons, name);
            assert!(card.width() <= 360, "{name}: the card fits the window");
            // A very narrow window degrades the same way.
            let (card, buttons) = placed(theme, 200.0, "Save changes to a?", &SAVE);
            assert_inside(card, &buttons, name);
            assert!(card.width() <= 200, "{name}: the card fits a tiny window");
        }
    }

    #[test]
    fn a_long_message_and_a_single_button_still_fit() {
        let long = "Save changes to a rather long project name before continuing? ".repeat(6);
        let (card, buttons) = placed(Theme::light(), 800.0, &long, &SAVE);
        assert_inside(card, &buttons, "long message");
        let (card, buttons) = placed(Theme::dark(), 360.0, "", &["OK"]);
        assert_inside(card, &buttons, "one button, empty message");
    }

    #[test]
    fn fitted_row_never_exceeds_the_content() {
        for content in 0..400 {
            for count in 1..=3usize {
                let (w, gap) = fitted_row(count, 88, 12, content);
                let row = row_width(count, w, gap);
                assert!(w >= 1 && gap >= 0);
                // Only the 1px-per-button floor may exceed a hopeless width.
                assert!(
                    row <= content.max(count as i32),
                    "{count} in {content}: {row}"
                );
            }
        }
    }
}
