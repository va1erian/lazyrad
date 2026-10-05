#![forbid(unsafe_code)]

//! [`Live`]: the [`LiveWidget`] every built-in factory returns, adding the
//! common properties (geometry, `anchor`, `visible`, `enabled`, `tab_index`)
//! and the schema's access and type rules to a widget's own surface.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use xui_core::WidgetId;
use xui_core::app::Ui;

use crate::build::{LiveWidget, SetError};
use crate::placement::{Geometry, Placement};
use crate::schema::{Access, Catalog};
use crate::value::Value;

/// The widget-specific property surface a built-in widget implements.
///
/// The [`Live`] wrapper adds the common properties and the schema's access and
/// type rules. Every method is called after the form is mounted.
pub(crate) trait WidgetProps<M: 'static>: 'static {
    /// The widget's node identity.
    fn id(&self) -> WidgetId;
    /// Reads a widget-specific property.
    fn get_own(&self, prop: &str) -> Option<Value>;
    /// Writes a widget-specific property.
    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError>;
    /// Notifies the widget that its `enabled` common property changed, so a
    /// widget with its own enabled state can dim itself. The default does
    /// nothing.
    fn set_enabled_hint(&self, _enabled: bool) {}

    /// Every node the widget owns; the first is [`WidgetProps::id`].
    fn node_ids(&self) -> Vec<WidgetId> {
        vec![self.id()]
    }
}

/// The common property state every widget shares.
pub(crate) struct Common<M: 'static> {
    pub(crate) ui: Ui<M>,
    pub(crate) geometry: Rc<Geometry>,
    /// The form's placement and this node's index in it, so a geometry edit
    /// moves the widget through its layout.
    pub(crate) placement: Weak<Placement<M>>,
    pub(crate) index: usize,
    pub(crate) visible: Cell<bool>,
    pub(crate) enabled: Cell<bool>,
    pub(crate) tab_index: Cell<i64>,
}

impl<M: 'static> Common<M> {
    /// Reads a common property.
    fn get(&self, prop: &str) -> Option<Value> {
        let (left, top, width, height) = self.geometry.design.get();
        match prop {
            "left" => Some(Value::Int(left)),
            "top" => Some(Value::Int(top)),
            "width" => Some(Value::Int(width)),
            "height" => Some(Value::Int(height)),
            "anchor" => Some(Value::Enum(
                crate::schema::anchor_name(self.geometry.anchor.get()).to_owned(),
            )),
            "visible" => Some(Value::Bool(self.visible.get())),
            "enabled" => Some(Value::Bool(self.enabled.get())),
            "tab_index" => Some(Value::Int(self.tab_index.get())),
            _ => None,
        }
    }

    /// Writes a common property. `None` means "not a common property".
    fn set(&self, prop: &str, value: &Value) -> Option<Result<(), SetError>> {
        match (prop, value) {
            ("left", Value::Int(value)) => self.set_geometry(|rect| rect.0 = *value),
            ("top", Value::Int(value)) => self.set_geometry(|rect| rect.1 = *value),
            ("width", Value::Int(value)) => self.set_geometry(|rect| rect.2 = *value),
            ("height", Value::Int(value)) => self.set_geometry(|rect| rect.3 = *value),
            ("left" | "top" | "width" | "height", _) => Some(Err(SetError::TypeMismatch)),
            ("anchor", Value::Enum(name)) => match crate::schema::anchor_from_name(name) {
                Some(anchor) => {
                    self.geometry.anchor.set(anchor);
                    Some(Ok(()))
                }
                None => Some(Err(SetError::TypeMismatch)),
            },
            ("anchor", _) => Some(Err(SetError::TypeMismatch)),
            ("visible", Value::Bool(visible)) => {
                self.visible.set(*visible);
                Some(Ok(()))
            }
            ("visible", _) => Some(Err(SetError::TypeMismatch)),
            ("enabled", Value::Bool(enabled)) => {
                self.enabled.set(*enabled);
                Some(Ok(()))
            }
            ("enabled", _) => Some(Err(SetError::TypeMismatch)),
            ("tab_index", Value::Int(index)) => {
                self.tab_index.set(*index);
                Some(Ok(()))
            }
            ("tab_index", _) => Some(Err(SetError::TypeMismatch)),
            _ => None,
        }
    }

    /// Mutates one geometry component. [`Live`] then moves the widget.
    fn set_geometry(
        &self,
        mutate: impl FnOnce(&mut (i64, i64, i64, i64)),
    ) -> Option<Result<(), SetError>> {
        let mut design = self.geometry.design.get();
        mutate(&mut design);
        self.geometry.design.set(design);
        Some(Ok(()))
    }
}

/// A [`LiveWidget`] that adds the common properties and the schema's access and
/// type rules to a widget's own surface.
pub(crate) struct Live<M: 'static, W: WidgetProps<M>> {
    pub(crate) common: Common<M>,
    pub(crate) inner: W,
    pub(crate) catalog: Rc<Catalog>,
    pub(crate) kind: String,
    pub(crate) design_mode: bool,
}

impl<M: 'static, W: WidgetProps<M>> LiveWidget<M> for Live<M, W> {
    fn id(&self) -> WidgetId {
        self.inner.id()
    }

    fn get(&self, prop: &str) -> Option<Value> {
        self.common.get(prop).or_else(|| self.inner.get_own(prop))
    }

    fn set(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        let Some(spec) = self.catalog.property(&self.kind, prop) else {
            return Err(SetError::UnknownProperty);
        };
        let writable = if self.design_mode {
            matches!(spec.access, Access::ReadWrite | Access::DesignOnly)
        } else {
            matches!(spec.access, Access::ReadWrite | Access::RuntimeOnly)
        };
        if !writable {
            return Err(SetError::ReadOnly);
        }
        if !spec.accepts(value) {
            return Err(SetError::TypeMismatch);
        }
        if let Some(result) = self.common.set(prop, value) {
            if result.is_ok() {
                self.apply_common(prop, value);
            }
            return result;
        }
        self.inner.set_own(prop, value)
    }

    fn node_ids(&self) -> Vec<WidgetId> {
        self.inner.node_ids()
    }
}

impl<M: 'static, W: WidgetProps<M>> Live<M, W> {
    /// Applies a common property that was just recorded to every node the
    /// widget owns, so a multi-node widget moves, hides and disables as one.
    fn apply_common(&self, prop: &str, value: &Value) {
        let ui = &self.common.ui;
        match (prop, value) {
            ("left" | "top" | "width" | "height" | "anchor", _) => {
                if let Some(placement) = self.common.placement.upgrade() {
                    placement.changed(self.common.index);
                }
            }
            ("visible", Value::Bool(visible)) => {
                for id in self.inner.node_ids() {
                    ui.set_visible(id, *visible);
                }
            }
            ("enabled", Value::Bool(enabled)) => {
                for id in self.inner.node_ids() {
                    ui.set_enabled(id, *enabled);
                }
                self.inner.set_enabled_hint(*enabled);
            }
            _ => {}
        }
    }
}
