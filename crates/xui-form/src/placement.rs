#![forbid(unsafe_code)]

//! Where a built form's widgets go: one [`absolute`] layout per level of the
//! node tree (the window, then each container), each node at its design
//! rectangle and moved by its [`Anchor`] as the level resizes.
//!
//! A layout's positions are fixed when it is mounted, so a geometry or anchor
//! edit mounts the affected level again over the same widgets (a layout holds
//! them through `Rc`, so they are not recreated). The window level is placed
//! in the window's client area, or in a container node the host chose; a
//! container's level is placed in the container's own node, with the
//! container's design size, so its children follow it as it resizes.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use xui_core::WidgetId;
use xui_core::app::Ui;
use xui_core::arrange::{Entry, IntoEntry, LayoutExt, Mounted, absolute, build};
use xui_core::backend::Result as BackendResult;
use xui_core::layout::{Anchor, Insets};
use xui_core::units::Dip;
use xui_core::widget::Placeable;

/// The room, in design units, a level has past each of its edges, so a node
/// that overflows its parent stays where it was drawn. A multiple of four, so
/// it scales to whole pixels at the common DPIs.
const SLACK: f32 = 32768.0;

/// A node's design rectangle, in DIPs: `(left, top, width, height)`.
pub(crate) type DesignRect = (i64, i64, i64, i64);

/// The shared widget behind a node, once its level is mounted.
pub(crate) type Shared<M> = Box<dyn Fn() -> Option<Rc<dyn Placeable<M>>>>;

/// The geometry a node is placed by, shared between its live widget (which
/// reports and edits it) and the [`Placement`].
pub(crate) struct Geometry {
    pub(crate) design: Cell<DesignRect>,
    pub(crate) anchor: Cell<Anchor>,
}

/// One node as the placement sees it.
pub(crate) struct Slot<M: 'static> {
    /// The index of the container node the node sits in, or `None` for the
    /// window level.
    pub(crate) parent: Option<usize>,
    pub(crate) geometry: Rc<Geometry>,
    pub(crate) shared: Shared<M>,
    pub(crate) is_container: bool,
}

/// The mounted levels of one form and the geometry they are built from.
pub(crate) struct Placement<M: 'static> {
    ui: Ui<M>,
    /// The node the window level is mounted in, or `None` for the window.
    container: Option<WidgetId>,
    /// The window level's design size, in DIPs.
    design_size: Cell<(i64, i64)>,
    slots: RefCell<Vec<Slot<M>>>,
    levels: RefCell<BTreeMap<Option<usize>, Mounted<M>>>,
    /// How many [`Placement::batch`] calls are open.
    batching: Cell<u32>,
    /// Levels to mount again when the outermost batch ends.
    dirty: RefCell<BTreeSet<Option<usize>>>,
}

impl<M: 'static> Placement<M> {
    pub(crate) fn new(
        ui: Ui<M>,
        container: Option<WidgetId>,
        design_size: (i64, i64),
    ) -> Placement<M> {
        Placement {
            ui,
            container,
            design_size: Cell::new(design_size),
            slots: RefCell::new(Vec::new()),
            levels: RefCell::new(BTreeMap::new()),
            batching: Cell::new(0),
            dirty: RefCell::new(BTreeSet::new()),
        }
    }

    /// Mounts every level for the first time, creating the widgets from
    /// `entries` (indexed like the slots), the window level first and each
    /// container before the containers inside it.
    pub(crate) fn mount(&self, slots: Vec<Slot<M>>, entries: Vec<Entry<M>>) -> BackendResult<()> {
        let containers: Vec<Option<usize>> = std::iter::once(None)
            .chain(
                slots
                    .iter()
                    .enumerate()
                    .filter(|(_, slot)| slot.is_container)
                    .map(|(index, _)| Some(index)),
            )
            .collect();
        *self.slots.borrow_mut() = slots;
        let mut entries: Vec<Option<Entry<M>>> = entries.into_iter().map(Some).collect();
        for level in containers {
            let mounted = self.mount_level(level, |index, _| entries[index].take())?;
            self.levels.borrow_mut().insert(level, mounted);
        }
        Ok(())
    }

    /// Records that the node at `index` moved, resized or changed anchor:
    /// its level is mounted again, and so is its own level for a container,
    /// whose children were designed against its size.
    pub(crate) fn changed(&self, index: usize) {
        {
            let slots = self.slots.borrow();
            let Some(slot) = slots.get(index) else {
                return;
            };
            let mut dirty = self.dirty.borrow_mut();
            dirty.insert(slot.parent);
            if slot.is_container {
                dirty.insert(Some(index));
            }
        }
        if self.batching.get() == 0 {
            self.flush();
        }
    }

    /// Changes the window level's design size, mounting the level again.
    pub(crate) fn set_design_size(&self, size: (i64, i64)) {
        if self.design_size.replace(size) == size {
            return;
        }
        self.dirty.borrow_mut().insert(None);
        if self.batching.get() == 0 {
            self.flush();
        }
    }

    /// Runs `f`, mounting the levels its edits changed once, at the end.
    pub(crate) fn batch<R>(&self, f: impl FnOnce() -> R) -> R {
        self.batching.set(self.batching.get() + 1);
        let result = f();
        self.batching.set(self.batching.get() - 1);
        if self.batching.get() == 0 {
            self.flush();
        }
        result
    }

    /// Lays every level out again now.
    pub(crate) fn relayout(&self) {
        for mounted in self.levels.borrow().values() {
            mounted.relayout();
        }
    }

    /// Mounts every dirty level again, outer levels first (a container's
    /// level follows its parent's, so it sees the container's new size).
    fn flush(&self) {
        let dirty = std::mem::take(&mut *self.dirty.borrow_mut());
        for level in dirty {
            let mounted = self.mount_level(level, |_, slot| {
                let widget = (slot.shared)()?;
                Some(build(move |_| Ok(widget)).into_entry())
            });
            // Re-mounting shared widgets creates nothing, so it cannot fail.
            if let Ok(mounted) = mounted {
                self.levels.borrow_mut().insert(level, mounted);
            }
        }
    }

    /// Mounts the absolute layout of `level`'s nodes, each taken from `entry`
    /// and placed at its current geometry.
    fn mount_level(
        &self,
        level: Option<usize>,
        mut entry: impl FnMut(usize, &Slot<M>) -> Option<Entry<M>>,
    ) -> BackendResult<Mounted<M>> {
        let slots = self.slots.borrow();
        let (width, height) = match level {
            None => self.design_size.get(),
            Some(index) => {
                let (_, _, width, height) = slots[index].geometry.design.get();
                (width, height)
            }
        };
        // `absolute` slides a node that overflows its parent back inside it.
        // A form keeps such a node where it was drawn (partly off the form or
        // its container, clipped), so the level gets `SLACK` design units of
        // room past every edge: the positions shift into the slack and the
        // negative padding shifts them back, while the anchors see the same
        // growth as without it.
        let mut layout = absolute()
            .padding(Insets::all(Dip(-SLACK)))
            .design_size(width as f32 + 2.0 * SLACK, height as f32 + 2.0 * SLACK);
        for (index, slot) in slots.iter().enumerate() {
            if slot.parent != level {
                continue;
            }
            let Some(item) = entry(index, slot) else {
                continue;
            };
            let (left, top, width, height) = slot.geometry.design.get();
            layout = layout.child(
                item.at(
                    left as f32 + SLACK,
                    top as f32 + SLACK,
                    width as f32,
                    height as f32,
                )
                .anchor(slot.geometry.anchor.get()),
            );
        }
        let container = match level {
            None => self.container,
            // A container is mounted before its level, so its widget exists.
            Some(index) => {
                Some((slots[index].shared)().map_or(WidgetId::NONE, |widget| widget.id()))
            }
        };
        drop(slots);
        match container {
            None => self.ui.mount(layout),
            Some(id) => self.ui.mount_in(id, layout),
        }
    }
}
