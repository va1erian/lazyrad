#![forbid(unsafe_code)]

//! The [`Factories::xui`] registry: a factory for every portable `xui-core`
//! widget in [`Catalog::xui`](crate::Catalog::xui).
//!
//! Each factory mirrors the widget's real API: it passes the node's design
//! rectangle to the widget constructor, forwards the properties the widget can
//! set, and wires only the events the widget raises. Geometry is applied at
//! construction, so a node's `left`/`top`/`width`/`height` are in place as soon
//! as the form is built.

use std::cell::{Cell, RefCell};

use xui_core::app::Ui;
use xui_core::backend::Result as BackendResult;
use xui_core::geometry::Rect;
use xui_core::{HasText, Properties, WidgetId};

use crate::build::{BuildCx, Factories, LiveWidget, SetError, WidgetFactory, WidgetProps};
use crate::doc::Node;
use crate::value::Value;

impl<M: 'static> Factories<M> {
    /// A registry with a factory for every built-in `xui-core` kind.
    pub fn xui() -> Self {
        let mut factories = Factories::new();
        factories.register(LabelFactory);
        factories.register(ButtonFactory);
        factories.register(CheckBoxFactory);
        factories.register(ToggleButtonFactory);
        factories.register(RadioGroupFactory);
        factories.register(EditFactory);
        factories.register(MultilineEditFactory);
        factories.register(NumberFieldFactory);
        factories.register(SliderFactory);
        factories.register(ProgressBarFactory);
        factories.register(ComboBoxFactory);
        factories.register(ListViewFactory);
        factories.register(GroupBoxFactory);
        factories.register(PanelFactory);
        factories.register(SeparatorFactory);
        factories.register(HyperlinkFactory);
        factories
    }
}

/// `Label`: non-interactive text.
struct LabelFactory;

impl<M: 'static> WidgetFactory<M> for LabelFactory {
    fn kind(&self) -> &str {
        "Label"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let label = xui_core::Label::new(cx.ui(), cx.rect(), &cx.text("text"))?;
        Ok(cx.live(LabelProps { label }))
    }
}

struct LabelProps<M: 'static> {
    label: xui_core::Label<M>,
}

impl<M: 'static> WidgetProps<M> for LabelProps<M> {
    fn id(&self) -> WidgetId {
        self.label.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.label.text())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.label.set_text(text);
                Ok(())
            }
            ("text", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }
}

/// `Button`: a push button.
struct ButtonFactory;

impl<M: 'static> WidgetFactory<M> for ButtonFactory {
    fn kind(&self) -> &str {
        "Button"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let mut button = xui_core::Button::new(cx.ui(), cx.rect(), &cx.text("text"))?;
        if let Some(handler) = cx.handler("Click") {
            button = button.on_click(move || handler(&[]));
        }
        Ok(cx.live(ButtonProps { button }))
    }
}

struct ButtonProps<M: 'static> {
    button: xui_core::Button<M>,
}

impl<M: 'static> WidgetProps<M> for ButtonProps<M> {
    fn id(&self) -> WidgetId {
        self.button.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.button.text())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.button.set_text(text);
                Ok(())
            }
            ("text", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.button.set_enabled(enabled);
    }
}

/// `CheckBox`: a labelled check box.
struct CheckBoxFactory;

impl<M: 'static> WidgetFactory<M> for CheckBoxFactory {
    fn kind(&self) -> &str {
        "CheckBox"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let mut check = xui_core::CheckBox::new(cx.ui(), cx.rect(), &cx.text("text"))?;
        check.set_checked(cx.bool("checked", false));
        if let Some(handler) = cx.handler("Toggle") {
            check = check.on_toggle(move |checked| handler(&[Value::Bool(checked)]));
        }
        Ok(cx.live(CheckBoxProps { check }))
    }
}

struct CheckBoxProps<M: 'static> {
    check: xui_core::CheckBox<M>,
}

impl<M: 'static> WidgetProps<M> for CheckBoxProps<M> {
    fn id(&self) -> WidgetId {
        self.check.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.check.text())),
            "checked" => Some(Value::Bool(self.check.is_checked())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.check.set_text(text);
                Ok(())
            }
            ("checked", Value::Bool(checked)) => {
                self.check.set_checked(*checked);
                Ok(())
            }
            ("text" | "checked", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.check.set_enabled(enabled);
    }
}

/// `ToggleButton`: a latched push button.
struct ToggleButtonFactory;

impl<M: 'static> WidgetFactory<M> for ToggleButtonFactory {
    fn kind(&self) -> &str {
        "ToggleButton"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let mut toggle = xui_core::ToggleButton::new(cx.ui(), cx.rect(), &cx.text("text"))?;
        toggle.set_checked(cx.bool("checked", false));
        if let Some(handler) = cx.handler("Toggle") {
            toggle = toggle.on_toggle(move |checked| handler(&[Value::Bool(checked)]));
        }
        Ok(cx.live(ToggleButtonProps { toggle }))
    }
}

struct ToggleButtonProps<M: 'static> {
    toggle: xui_core::ToggleButton<M>,
}

impl<M: 'static> WidgetProps<M> for ToggleButtonProps<M> {
    fn id(&self) -> WidgetId {
        self.toggle.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.toggle.text())),
            "checked" => Some(Value::Bool(self.toggle.is_checked())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.toggle.set_text(text);
                Ok(())
            }
            ("checked", Value::Bool(checked)) => {
                self.toggle.set_checked(*checked);
                Ok(())
            }
            ("text" | "checked", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.toggle.set_enabled(enabled);
    }
}

/// `RadioGroup`: a set of radio options.
struct RadioGroupFactory;

impl<M: 'static> WidgetFactory<M> for RadioGroupFactory {
    fn kind(&self) -> &str {
        "RadioGroup"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let items = cx.list("items");
        let labels: Vec<&str> = items.iter().map(String::as_str).collect();
        let mut group = xui_core::RadioGroup::new(cx.ui(), cx.rect(), &labels)?;
        let selected = cx.int("selected", 0);
        if selected >= 0 {
            group.select(selected as usize);
        }
        if let Some(handler) = cx.handler("Select") {
            group = group.on_select(move |index| handler(&[Value::Int(index as i64)]));
        }
        Ok(cx.live(RadioGroupProps { group, items }))
    }
}

struct RadioGroupProps<M: 'static> {
    group: xui_core::RadioGroup<M>,
    items: Vec<String>,
}

impl<M: 'static> WidgetProps<M> for RadioGroupProps<M> {
    fn id(&self) -> WidgetId {
        self.group.ids().first().copied().unwrap_or(WidgetId::NONE)
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "items" => Some(Value::List(self.items.clone())),
            "selected" => Some(Value::Int(self.group.selected() as i64)),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("selected", Value::Int(index)) if *index >= 0 => {
                self.group.select(*index as usize);
                Ok(())
            }
            ("selected", Value::Int(_)) => Err(SetError::TypeMismatch),
            ("selected", _) => Err(SetError::TypeMismatch),
            ("items", _) => Err(SetError::ReadOnly),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.group.set_enabled(enabled);
    }

    fn node_ids(&self) -> Vec<WidgetId> {
        self.group.ids()
    }

    /// One row per option, stacked from the top of `rect`. The row height is
    /// read from the first option, as xui sizes the rows itself.
    fn placements(&self, ui: &Ui<M>, rect: Rect) -> Vec<(WidgetId, Rect)> {
        let ids = self.group.ids();
        let row = ids
            .first()
            .map(|id| ui.bounds(*id).height())
            .filter(|height| *height > 0)
            .unwrap_or_else(|| rect.height() / ids.len().max(1) as i32);
        ids.into_iter()
            .enumerate()
            .map(|(index, id)| {
                let top = rect.top + row * index as i32;
                (id, Rect::new(rect.left, top, rect.right, top + row))
            })
            .collect()
    }
}

/// `Edit`: a single-line text field.
struct EditFactory;

impl<M: 'static> WidgetFactory<M> for EditFactory {
    fn kind(&self) -> &str {
        "Edit"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let cue = cx.text("cue");
        let mut edit = xui_core::Edit::new(cx.ui(), cx.rect(), &cx.text("text"))?;
        if !cue.is_empty() {
            edit = edit.cue(&cue);
        }
        if let Some(handler) = cx.handler("Change") {
            edit = edit.on_change(move |text| handler(&[Value::Text(text.to_owned())]));
        }
        Ok(cx.live(EditProps { edit, cue }))
    }
}

struct EditProps<M: 'static> {
    edit: xui_core::Edit<M>,
    cue: String,
}

impl<M: 'static> WidgetProps<M> for EditProps<M> {
    fn id(&self) -> WidgetId {
        self.edit.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.edit.text())),
            "cue" => Some(Value::Text(self.cue.clone())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.edit.set_text(text);
                Ok(())
            }
            ("text", _) => Err(SetError::TypeMismatch),
            ("cue", _) => Err(SetError::ReadOnly),
            _ => Err(SetError::UnknownProperty),
        }
    }
}

/// `MultilineEdit`: a text area.
struct MultilineEditFactory;

impl<M: 'static> WidgetFactory<M> for MultilineEditFactory {
    fn kind(&self) -> &str {
        "MultilineEdit"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let mut edit = xui_core::MultilineEdit::new(cx.ui(), cx.rect(), &cx.text("text"))?;
        if let Some(handler) = cx.handler("Change") {
            edit = edit.on_change(move |text| handler(&[Value::Text(text.to_owned())]));
        }
        Ok(cx.live(MultilineEditProps { edit }))
    }
}

struct MultilineEditProps<M: 'static> {
    edit: xui_core::MultilineEdit<M>,
}

impl<M: 'static> WidgetProps<M> for MultilineEditProps<M> {
    fn id(&self) -> WidgetId {
        self.edit.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.edit.text())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.edit.set_text(text);
                Ok(())
            }
            ("text", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.edit.set_enabled(enabled);
    }
}

/// `NumberField`: a numeric field with steppers.
struct NumberFieldFactory;

impl<M: 'static> WidgetFactory<M> for NumberFieldFactory {
    fn kind(&self) -> &str {
        "NumberField"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let min = cx.float("min", 0.0);
        let max = cx.float("max", 100.0);
        let step = cx.float("step", 1.0);
        let mut field = xui_core::NumberField::new(cx.ui(), cx.rect(), min, max, step)?;
        field.set_value(cx.float("value", min));
        if let Some(handler) = cx.handler("Change") {
            field = field.on_change(move |value| handler(&[Value::Float(value)]));
        }
        if let Some(handler) = cx.handler("Commit") {
            field = field.on_commit(move |value| handler(&[Value::Float(value)]));
        }
        Ok(cx.live(NumberFieldProps {
            field,
            min: Cell::new(min),
            max: Cell::new(max),
        }))
    }
}

struct NumberFieldProps<M: 'static> {
    field: xui_core::NumberField<M>,
    min: Cell<f64>,
    max: Cell<f64>,
}

impl<M: 'static> WidgetProps<M> for NumberFieldProps<M> {
    fn id(&self) -> WidgetId {
        self.field.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "value" => Some(Value::Float(self.field.value())),
            "min" => Some(Value::Float(self.min.get())),
            "max" => Some(Value::Float(self.max.get())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        // An int is accepted wherever a float is expected (see `ValueType`).
        let Some(number) = value.as_float() else {
            return match prop {
                "value" | "min" | "max" => Err(SetError::TypeMismatch),
                _ => Err(SetError::UnknownProperty),
            };
        };
        let number = &number;
        match prop {
            "value" => {
                self.field.set_value(*number);
                Ok(())
            }
            "min" => {
                self.min.set(*number);
                self.field
                    .set_property("min", xui_core::Value::Float(*number));
                Ok(())
            }
            "max" => {
                self.max.set(*number);
                self.field
                    .set_property("max", xui_core::Value::Float(*number));
                Ok(())
            }
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.field.set_enabled(enabled);
    }
}

/// `Slider`: a range control.
struct SliderFactory;

impl<M: 'static> WidgetFactory<M> for SliderFactory {
    fn kind(&self) -> &str {
        "Slider"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let min = cx.float("min", 0.0);
        let max = cx.float("max", 100.0);
        let mut slider = xui_core::Slider::new(cx.ui(), cx.rect(), min, max)?;
        slider.set_value(cx.float("value", min));
        if let Some(handler) = cx.handler("Change") {
            slider = slider.on_change(move |value| handler(&[Value::Float(value)]));
        }
        if let Some(handler) = cx.handler("Commit") {
            slider = slider.on_commit(move |value| handler(&[Value::Float(value)]));
        }
        Ok(cx.live(SliderProps {
            slider,
            min: Cell::new(min),
            max: Cell::new(max),
        }))
    }
}

struct SliderProps<M: 'static> {
    slider: xui_core::Slider<M>,
    min: Cell<f64>,
    max: Cell<f64>,
}

impl<M: 'static> WidgetProps<M> for SliderProps<M> {
    fn id(&self) -> WidgetId {
        self.slider.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "value" => Some(Value::Float(self.slider.value())),
            "min" => Some(Value::Float(self.min.get())),
            "max" => Some(Value::Float(self.max.get())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        // An int is accepted wherever a float is expected (see `ValueType`).
        let Some(number) = value.as_float() else {
            return match prop {
                "value" | "min" | "max" => Err(SetError::TypeMismatch),
                _ => Err(SetError::UnknownProperty),
            };
        };
        let number = &number;
        match prop {
            "value" => {
                self.slider.set_value(*number);
                Ok(())
            }
            "min" => {
                self.min.set(*number);
                self.slider.set_range(*number, self.max.get());
                Ok(())
            }
            "max" => {
                self.max.set(*number);
                self.slider.set_range(self.min.get(), *number);
                Ok(())
            }
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.slider.set_enabled(enabled);
    }
}

/// `ProgressBar`: a read-only indicator.
struct ProgressBarFactory;

impl<M: 'static> WidgetFactory<M> for ProgressBarFactory {
    fn kind(&self) -> &str {
        "ProgressBar"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let max = cx.int("max", 100) as i32;
        let bar = xui_core::ProgressBar::new(cx.ui(), cx.rect(), max)?;
        bar.set_value(cx.int("value", 0) as i32);
        Ok(cx.live(ProgressBarProps { bar }))
    }
}

struct ProgressBarProps<M: 'static> {
    bar: xui_core::ProgressBar<M>,
}

impl<M: 'static> WidgetProps<M> for ProgressBarProps<M> {
    fn id(&self) -> WidgetId {
        self.bar.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "value" => Some(Value::Int(self.bar.value() as i64)),
            "max" => Some(Value::Int(self.bar.max() as i64)),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("value", Value::Int(value)) => {
                self.bar.set_value(*value as i32);
                Ok(())
            }
            ("max", Value::Int(value)) => {
                self.bar.set_max(*value as i32);
                Ok(())
            }
            ("value" | "max", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.bar.set_enabled(enabled);
    }
}

/// `ComboBox`: a drop-down list.
struct ComboBoxFactory;

impl<M: 'static> WidgetFactory<M> for ComboBoxFactory {
    fn kind(&self) -> &str {
        "ComboBox"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let items = cx.list("items");
        let labels: Vec<&str> = items.iter().map(String::as_str).collect();
        let mut combo = xui_core::ComboBox::new(cx.ui(), cx.rect(), &labels)?;
        let selected = cx.int("selected", 0);
        if selected >= 0 {
            combo.select(selected as usize);
        }
        if let Some(handler) = cx.handler("Select") {
            combo = combo.on_select(move |index| handler(&[Value::Int(index as i64)]));
        }
        Ok(cx.live(ComboBoxProps { combo, items }))
    }
}

struct ComboBoxProps<M: 'static> {
    combo: xui_core::ComboBox<M>,
    items: Vec<String>,
}

impl<M: 'static> WidgetProps<M> for ComboBoxProps<M> {
    fn id(&self) -> WidgetId {
        self.combo.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "items" => Some(Value::List(self.items.clone())),
            "selected" => Some(Value::Int(self.combo.selected() as i64)),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("selected", Value::Int(index)) if *index >= 0 => {
                self.combo.select(*index as usize);
                Ok(())
            }
            ("selected", Value::Int(_)) => Err(SetError::TypeMismatch),
            ("selected", _) => Err(SetError::TypeMismatch),
            ("items", _) => Err(SetError::ReadOnly),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.combo.set_enabled(enabled);
    }
}

/// `ListView`: a list of rows.
struct ListViewFactory;

impl<M: 'static> WidgetFactory<M> for ListViewFactory {
    fn kind(&self) -> &str {
        "ListView"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let items = cx.list("items");
        let labels: Vec<&str> = items.iter().map(String::as_str).collect();
        let multi = cx.bool("multi_select", false);
        let mut list = xui_core::ListView::new(cx.ui(), cx.rect(), &labels)?.multi_select(multi);
        // The schema default is -1 (no selection), and the writer drops
        // defaults, so a missing key means "nothing selected".
        let selected = cx.int("selected", -1);
        if selected >= 0 {
            list.select(Some(selected as usize));
        } else {
            list.select(None);
        }
        if let Some(handler) = cx.handler("Select") {
            list = list.on_select(move |index| handler(&[Value::Int(index as i64)]));
        }
        if let Some(handler) = cx.handler("Activate") {
            list = list.on_activate(move |index| handler(&[Value::Int(index as i64)]));
        }
        Ok(cx.live(ListViewProps {
            list,
            items: RefCell::new(items),
            multi,
        }))
    }
}

struct ListViewProps<M: 'static> {
    list: xui_core::ListView<M>,
    items: RefCell<Vec<String>>,
    multi: bool,
}

impl<M: 'static> WidgetProps<M> for ListViewProps<M> {
    fn id(&self) -> WidgetId {
        self.list.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "items" => Some(Value::List(self.items.borrow().clone())),
            "selected" => Some(Value::Int(
                self.list.selected().map_or(-1, |index| index as i64),
            )),
            "multi_select" => Some(Value::Bool(self.multi)),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("selected", Value::Int(index)) => {
                self.list.select((*index >= 0).then_some(*index as usize));
                Ok(())
            }
            ("selected", _) => Err(SetError::TypeMismatch),
            ("items", Value::List(items)) => {
                let rows: Vec<&str> = items.iter().map(String::as_str).collect();
                self.list.set_items(&rows);
                *self.items.borrow_mut() = items.clone();
                Ok(())
            }
            ("items", _) => Err(SetError::TypeMismatch),
            ("multi_select", _) => Err(SetError::ReadOnly),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.list.set_enabled(enabled);
    }
}

/// `GroupBox`: a titled frame.
struct GroupBoxFactory;

impl<M: 'static> WidgetFactory<M> for GroupBoxFactory {
    fn kind(&self) -> &str {
        "GroupBox"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let group = xui_core::GroupBox::new(cx.ui(), cx.rect(), &cx.text("text"))?;
        let scoped = cx.ui().with_parent(group.id());
        Ok(cx.live(GroupBoxProps { group, scoped }))
    }
}

struct GroupBoxProps<M: 'static> {
    group: xui_core::GroupBox<M>,
    scoped: xui_core::app::Ui<M>,
}

impl<M: 'static> WidgetProps<M> for GroupBoxProps<M> {
    fn id(&self) -> WidgetId {
        self.group.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.group.text())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.group.set_text(text);
                Ok(())
            }
            ("text", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn container_ui(&self) -> Option<&xui_core::app::Ui<M>> {
        Some(&self.scoped)
    }
}

/// `Panel`: a container that owns its children.
struct PanelFactory;

impl<M: 'static> WidgetFactory<M> for PanelFactory {
    fn kind(&self) -> &str {
        "Panel"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let panel = xui_core::Panel::new(cx.ui(), cx.rect())?;
        Ok(cx.live(PanelProps { panel }))
    }
}

struct PanelProps<M: 'static> {
    panel: xui_core::Panel<M>,
}

impl<M: 'static> WidgetProps<M> for PanelProps<M> {
    fn id(&self) -> WidgetId {
        self.panel.id()
    }

    fn get_own(&self, _prop: &str) -> Option<Value> {
        None
    }

    fn set_own(&self, _prop: &str, _value: &Value) -> Result<(), SetError> {
        Err(SetError::UnknownProperty)
    }

    fn container_ui(&self) -> Option<&xui_core::app::Ui<M>> {
        Some(self.panel.ui())
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.panel.set_enabled(enabled);
    }
}

/// `Separator`: a divider line.
struct SeparatorFactory;

impl<M: 'static> WidgetFactory<M> for SeparatorFactory {
    fn kind(&self) -> &str {
        "Separator"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let vertical = cx.text("orientation") == "vertical";
        let separator = if vertical {
            xui_core::Separator::vertical(cx.ui(), cx.rect())?
        } else {
            xui_core::Separator::new(cx.ui(), cx.rect())?
        };
        Ok(cx.live(SeparatorProps {
            separator,
            vertical,
        }))
    }
}

struct SeparatorProps<M: 'static> {
    separator: xui_core::Separator<M>,
    vertical: bool,
}

impl<M: 'static> WidgetProps<M> for SeparatorProps<M> {
    fn id(&self) -> WidgetId {
        self.separator.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "orientation" => Some(Value::Enum(
                if self.vertical {
                    "vertical"
                } else {
                    "horizontal"
                }
                .to_owned(),
            )),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, _value: &Value) -> Result<(), SetError> {
        match prop {
            "orientation" => Err(SetError::ReadOnly),
            _ => Err(SetError::UnknownProperty),
        }
    }
}

/// `Hyperlink`: a clickable link label.
struct HyperlinkFactory;

impl<M: 'static> WidgetFactory<M> for HyperlinkFactory {
    fn kind(&self) -> &str {
        "Hyperlink"
    }

    fn create(
        &self,
        cx: &mut BuildCx<'_, M>,
        _node: &Node,
    ) -> BackendResult<Box<dyn LiveWidget<M>>> {
        let mut link = xui_core::Hyperlink::new(cx.ui(), cx.rect(), &cx.text("text"))?;
        if let Some(handler) = cx.handler("Click") {
            link = link.on_click(move || handler(&[]));
        }
        Ok(cx.live(HyperlinkProps { link }))
    }
}

struct HyperlinkProps<M: 'static> {
    link: xui_core::Hyperlink<M>,
}

impl<M: 'static> WidgetProps<M> for HyperlinkProps<M> {
    fn id(&self) -> WidgetId {
        self.link.id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.link.text())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.link.set_text(text);
                Ok(())
            }
            ("text", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.link.set_enabled(enabled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_xui_registry_covers_every_catalog_kind() {
        let catalog = crate::Catalog::xui();
        let factories: Factories<()> = Factories::xui();
        for kind in catalog.kinds() {
            assert!(
                factories.get(kind).is_some(),
                "no factory registered for {kind}"
            );
        }
    }
}
