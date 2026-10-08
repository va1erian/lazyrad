#![forbid(unsafe_code)]

//! The [`Factories::xui`] registry: a factory for every portable `xui-core`
//! widget in [`Catalog::xui`](crate::Catalog::xui).
//!
//! Each factory mirrors the widget's real API: it describes the widget with its
//! `xui_core::arrange` builder, forwards the properties the widget can set, and
//! wires only the events the widget raises. The form places the builder at the
//! node's `left`/`top`/`width`/`height`, so the geometry is in place as soon as
//! the form is mounted.

use std::cell::{Cell, RefCell};

use xui_core::arrange::{self, Handle};
use xui_core::{HasText, Properties, WidgetId};

use crate::build::{BuildCx, Factories, Made, SetError, WidgetFactory};
use crate::doc::Node;
use crate::live::WidgetProps;
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
        factories.register(crate::canvas::CanvasFactory);
        factories.register(TimerFactory);
        factories
    }
}

/// `Label`: non-interactive text.
struct LabelFactory;

impl<M: 'static> WidgetFactory<M> for LabelFactory {
    fn kind(&self) -> &str {
        "Label"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let label = Handle::new();
        let build = arrange::label(cx.text("text")).bind(&label);
        cx.made(build, &label.clone(), LabelProps { label })
    }
}

struct LabelProps<M: 'static> {
    label: Handle<xui_core::Label<M>>,
}

impl<M: 'static> WidgetProps<M> for LabelProps<M> {
    fn id(&self) -> WidgetId {
        self.label.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.label.get().text())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.label.get().set_text(text);
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

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let button = Handle::new();
        let mut build = arrange::button(cx.text("text")).bind(&button);
        if let Some(handler) = cx.handler("Click") {
            build = build.then(move |button| button.on_click(move || handler(&[])));
        }
        cx.made(build, &button.clone(), ButtonProps { button })
    }
}

struct ButtonProps<M: 'static> {
    button: Handle<xui_core::Button<M>>,
}

impl<M: 'static> WidgetProps<M> for ButtonProps<M> {
    fn id(&self) -> WidgetId {
        self.button.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.button.get().text())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.button.get().set_text(text);
                Ok(())
            }
            ("text", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.button.get().set_enabled(enabled);
    }
}

/// `CheckBox`: a labelled check box.
struct CheckBoxFactory;

impl<M: 'static> WidgetFactory<M> for CheckBoxFactory {
    fn kind(&self) -> &str {
        "CheckBox"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let check = Handle::new();
        let mut build = arrange::checkbox(cx.text("text"))
            .checked(cx.bool("checked", false))
            .bind(&check);
        if let Some(handler) = cx.handler("Toggle") {
            build = build.then(move |check| {
                check.on_toggle(move |checked| handler(&[Value::Bool(checked)]))
            });
        }
        cx.made(build, &check.clone(), CheckBoxProps { check })
    }
}

struct CheckBoxProps<M: 'static> {
    check: Handle<xui_core::CheckBox<M>>,
}

impl<M: 'static> WidgetProps<M> for CheckBoxProps<M> {
    fn id(&self) -> WidgetId {
        self.check.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.check.get().text())),
            "checked" => Some(Value::Bool(self.check.get().is_checked())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.check.get().set_text(text);
                Ok(())
            }
            ("checked", Value::Bool(checked)) => {
                self.check.get().set_checked(*checked);
                Ok(())
            }
            ("text" | "checked", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.check.get().set_enabled(enabled);
    }
}

/// `ToggleButton`: a latched push button.
struct ToggleButtonFactory;

impl<M: 'static> WidgetFactory<M> for ToggleButtonFactory {
    fn kind(&self) -> &str {
        "ToggleButton"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let toggle = Handle::new();
        let mut build = arrange::toggle_button(cx.text("text"))
            .checked(cx.bool("checked", false))
            .bind(&toggle);
        if let Some(handler) = cx.handler("Toggle") {
            build = build.then(move |toggle| {
                toggle.on_toggle(move |checked| handler(&[Value::Bool(checked)]))
            });
        }
        cx.made(build, &toggle.clone(), ToggleButtonProps { toggle })
    }
}

struct ToggleButtonProps<M: 'static> {
    toggle: Handle<xui_core::ToggleButton<M>>,
}

impl<M: 'static> WidgetProps<M> for ToggleButtonProps<M> {
    fn id(&self) -> WidgetId {
        self.toggle.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.toggle.get().text())),
            "checked" => Some(Value::Bool(self.toggle.get().is_checked())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.toggle.get().set_text(text);
                Ok(())
            }
            ("checked", Value::Bool(checked)) => {
                self.toggle.get().set_checked(*checked);
                Ok(())
            }
            ("text" | "checked", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.toggle.get().set_enabled(enabled);
    }
}

/// `RadioGroup`: a set of radio options.
struct RadioGroupFactory;

impl<M: 'static> WidgetFactory<M> for RadioGroupFactory {
    fn kind(&self) -> &str {
        "RadioGroup"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let items = cx.list("items");
        let labels: Vec<&str> = items.iter().map(String::as_str).collect();
        let group = Handle::new();
        let mut build = arrange::radio_group(&labels).bind(&group);
        let selected = cx.int("selected", 0);
        if selected >= 0 {
            build = build.selected(selected as usize);
        }
        if let Some(handler) = cx.handler("Select") {
            build = build.then(move |group| {
                group.on_select(move |index| handler(&[Value::Int(index as i64)]))
            });
        }
        cx.made(build, &group.clone(), RadioGroupProps { group, items })
    }
}

struct RadioGroupProps<M: 'static> {
    group: Handle<xui_core::RadioGroup<M>>,
    items: Vec<String>,
}

impl<M: 'static> WidgetProps<M> for RadioGroupProps<M> {
    fn id(&self) -> WidgetId {
        self.group
            .get()
            .ids()
            .first()
            .copied()
            .unwrap_or(WidgetId::NONE)
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "items" => Some(Value::List(self.items.clone())),
            "selected" => Some(Value::Int(self.group.get().selected() as i64)),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("selected", Value::Int(index)) if *index >= 0 => {
                self.group.get().select(*index as usize);
                Ok(())
            }
            ("selected", Value::Int(_)) => Err(SetError::TypeMismatch),
            ("selected", _) => Err(SetError::TypeMismatch),
            ("items", _) => Err(SetError::ReadOnly),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.group.get().set_enabled(enabled);
    }

    fn node_ids(&self) -> Vec<WidgetId> {
        self.group.get().ids()
    }
}

/// `Edit`: a single-line text field.
struct EditFactory;

impl<M: 'static> WidgetFactory<M> for EditFactory {
    fn kind(&self) -> &str {
        "Edit"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let cue = cx.text("cue");
        let edit = Handle::new();
        let mut build = arrange::edit().text(cx.text("text")).bind(&edit);
        if !cue.is_empty() {
            build = build.placeholder(cue.clone());
        }
        if let Some(handler) = cx.handler("Change") {
            build = build.then(move |edit| {
                edit.on_change(move |text| handler(&[Value::Text(text.to_owned())]))
            });
        }
        cx.made(build, &edit.clone(), EditProps { edit, cue })
    }
}

struct EditProps<M: 'static> {
    edit: Handle<xui_core::Edit<M>>,
    cue: String,
}

impl<M: 'static> WidgetProps<M> for EditProps<M> {
    fn id(&self) -> WidgetId {
        self.edit.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.edit.get().text())),
            "cue" => Some(Value::Text(self.cue.clone())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.edit.get().set_text(text);
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

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let edit = Handle::new();
        let text = cx.text("text");
        let mut build = arrange::multiline_edit()
            .then(move |edit: xui_core::MultilineEdit<M>| {
                edit.set_text(&text);
                edit
            })
            .bind(&edit);
        if let Some(handler) = cx.handler("Change") {
            build = build.then(move |edit| {
                edit.on_change(move |text| handler(&[Value::Text(text.to_owned())]))
            });
        }
        cx.made(build, &edit.clone(), MultilineEditProps { edit })
    }
}

struct MultilineEditProps<M: 'static> {
    edit: Handle<xui_core::MultilineEdit<M>>,
}

impl<M: 'static> WidgetProps<M> for MultilineEditProps<M> {
    fn id(&self) -> WidgetId {
        self.edit.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.edit.get().text())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.edit.get().set_text(text);
                Ok(())
            }
            ("text", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.edit.get().set_enabled(enabled);
    }
}

/// `NumberField`: a numeric field with steppers.
struct NumberFieldFactory;

impl<M: 'static> WidgetFactory<M> for NumberFieldFactory {
    fn kind(&self) -> &str {
        "NumberField"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let min = cx.float("min", 0.0);
        let max = cx.float("max", 100.0);
        let step = cx.float("step", 1.0);
        let value = cx.float("value", min);
        let field = Handle::new();
        let mut build = arrange::number_field(min, max, step)
            .then(move |field: xui_core::NumberField<M>| {
                field.set_value(value);
                field
            })
            .bind(&field);
        if let Some(handler) = cx.handler("Change") {
            build = build
                .then(move |field| field.on_change(move |value| handler(&[Value::Float(value)])));
        }
        if let Some(handler) = cx.handler("Commit") {
            build = build
                .then(move |field| field.on_commit(move |value| handler(&[Value::Float(value)])));
        }
        let props = NumberFieldProps {
            field: field.clone(),
            min: Cell::new(min),
            max: Cell::new(max),
        };
        cx.made(build, &field, props)
    }
}

struct NumberFieldProps<M: 'static> {
    field: Handle<xui_core::NumberField<M>>,
    min: Cell<f64>,
    max: Cell<f64>,
}

impl<M: 'static> WidgetProps<M> for NumberFieldProps<M> {
    fn id(&self) -> WidgetId {
        self.field.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "value" => Some(Value::Float(self.field.get().value())),
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
                self.field.get().set_value(*number);
                Ok(())
            }
            "min" => {
                self.min.set(*number);
                self.field
                    .get()
                    .set_property("min", xui_core::Value::Float(*number));
                Ok(())
            }
            "max" => {
                self.max.set(*number);
                self.field
                    .get()
                    .set_property("max", xui_core::Value::Float(*number));
                Ok(())
            }
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.field.get().set_enabled(enabled);
    }
}

/// `Slider`: a range control.
struct SliderFactory;

impl<M: 'static> WidgetFactory<M> for SliderFactory {
    fn kind(&self) -> &str {
        "Slider"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let min = cx.float("min", 0.0);
        let max = cx.float("max", 100.0);
        let value = cx.float("value", min);
        let slider = Handle::new();
        let mut build = arrange::slider(min, max)
            .then(move |slider: xui_core::Slider<M>| {
                slider.set_value(value);
                slider
            })
            .bind(&slider);
        if let Some(handler) = cx.handler("Change") {
            build = build
                .then(move |slider| slider.on_change(move |value| handler(&[Value::Float(value)])));
        }
        if let Some(handler) = cx.handler("Commit") {
            build = build
                .then(move |slider| slider.on_commit(move |value| handler(&[Value::Float(value)])));
        }
        let props = SliderProps {
            slider: slider.clone(),
            min: Cell::new(min),
            max: Cell::new(max),
        };
        cx.made(build, &slider, props)
    }
}

struct SliderProps<M: 'static> {
    slider: Handle<xui_core::Slider<M>>,
    min: Cell<f64>,
    max: Cell<f64>,
}

impl<M: 'static> WidgetProps<M> for SliderProps<M> {
    fn id(&self) -> WidgetId {
        self.slider.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "value" => Some(Value::Float(self.slider.get().value())),
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
                self.slider.get().set_value(*number);
                Ok(())
            }
            "min" => {
                self.min.set(*number);
                self.slider.get().set_range(*number, self.max.get());
                Ok(())
            }
            "max" => {
                self.max.set(*number);
                self.slider.get().set_range(self.min.get(), *number);
                Ok(())
            }
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.slider.get().set_enabled(enabled);
    }
}

/// `ProgressBar`: a read-only indicator.
struct ProgressBarFactory;

impl<M: 'static> WidgetFactory<M> for ProgressBarFactory {
    fn kind(&self) -> &str {
        "ProgressBar"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let max = cx.int("max", 100) as i32;
        let bar = Handle::new();
        let build = arrange::progress(max)
            .value(cx.int("value", 0) as i32)
            .bind(&bar);
        cx.made(build, &bar.clone(), ProgressBarProps { bar })
    }
}

struct ProgressBarProps<M: 'static> {
    bar: Handle<xui_core::ProgressBar<M>>,
}

impl<M: 'static> WidgetProps<M> for ProgressBarProps<M> {
    fn id(&self) -> WidgetId {
        self.bar.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "value" => Some(Value::Int(self.bar.get().value() as i64)),
            "max" => Some(Value::Int(self.bar.get().max() as i64)),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("value", Value::Int(value)) => {
                self.bar.get().set_value(*value as i32);
                Ok(())
            }
            ("max", Value::Int(value)) => {
                self.bar.get().set_max(*value as i32);
                Ok(())
            }
            ("value" | "max", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.bar.get().set_enabled(enabled);
    }
}

/// `ComboBox`: a drop-down list.
struct ComboBoxFactory;

impl<M: 'static> WidgetFactory<M> for ComboBoxFactory {
    fn kind(&self) -> &str {
        "ComboBox"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let items = cx.list("items");
        let labels: Vec<&str> = items.iter().map(String::as_str).collect();
        let combo = Handle::new();
        let selected = cx.int("selected", 0);
        let mut build = arrange::combo_box(&labels)
            .then(move |combo: xui_core::ComboBox<M>| {
                if selected >= 0 {
                    combo.select(selected as usize);
                }
                combo
            })
            .bind(&combo);
        if let Some(handler) = cx.handler("Select") {
            build = build.then(move |combo| {
                combo.on_select(move |index| handler(&[Value::Int(index as i64)]))
            });
        }
        cx.made(build, &combo.clone(), ComboBoxProps { combo, items })
    }
}

struct ComboBoxProps<M: 'static> {
    combo: Handle<xui_core::ComboBox<M>>,
    items: Vec<String>,
}

impl<M: 'static> WidgetProps<M> for ComboBoxProps<M> {
    fn id(&self) -> WidgetId {
        self.combo.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "items" => Some(Value::List(self.items.clone())),
            "selected" => Some(Value::Int(self.combo.get().selected() as i64)),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("selected", Value::Int(index)) if *index >= 0 => {
                self.combo.get().select(*index as usize);
                Ok(())
            }
            ("selected", Value::Int(_)) => Err(SetError::TypeMismatch),
            ("selected", _) => Err(SetError::TypeMismatch),
            ("items", _) => Err(SetError::ReadOnly),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.combo.get().set_enabled(enabled);
    }
}

/// `ListView`: a list of rows.
struct ListViewFactory;

impl<M: 'static> WidgetFactory<M> for ListViewFactory {
    fn kind(&self) -> &str {
        "ListView"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let items = cx.list("items");
        let labels: Vec<&str> = items.iter().map(String::as_str).collect();
        let multi = cx.bool("multi_select", false);
        // The schema default is -1 (no selection), and the writer drops
        // defaults, so a missing key means "nothing selected".
        let selected = cx.int("selected", -1);
        let list = Handle::new();
        let mut build = arrange::list()
            .items(&labels)
            .then(move |list: xui_core::ListView<M>| {
                let list = list.multi_select(multi);
                list.select((selected >= 0).then_some(selected as usize));
                list
            })
            .bind(&list);
        if let Some(handler) = cx.handler("Select") {
            build = build.then(move |list| {
                list.on_select(move |index| handler(&[Value::Int(index as i64)]))
            });
        }
        if let Some(handler) = cx.handler("Activate") {
            build = build.then(move |list| {
                list.on_activate(move |index| handler(&[Value::Int(index as i64)]))
            });
        }
        let props = ListViewProps {
            list: list.clone(),
            items: RefCell::new(items),
            multi,
        };
        cx.made(build, &list, props)
    }
}

struct ListViewProps<M: 'static> {
    list: Handle<xui_core::ListView<M>>,
    items: RefCell<Vec<String>>,
    multi: bool,
}

impl<M: 'static> WidgetProps<M> for ListViewProps<M> {
    fn id(&self) -> WidgetId {
        self.list.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "items" => Some(Value::List(self.items.borrow().clone())),
            "selected" => Some(Value::Int(
                self.list.get().selected().map_or(-1, |index| index as i64),
            )),
            "multi_select" => Some(Value::Bool(self.multi)),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("selected", Value::Int(index)) => {
                self.list
                    .get()
                    .select((*index >= 0).then_some(*index as usize));
                Ok(())
            }
            ("selected", _) => Err(SetError::TypeMismatch),
            ("items", Value::List(items)) => {
                let rows: Vec<&str> = items.iter().map(String::as_str).collect();
                self.list.get().set_items(&rows);
                *self.items.borrow_mut() = items.clone();
                Ok(())
            }
            ("items", _) => Err(SetError::TypeMismatch),
            ("multi_select", _) => Err(SetError::ReadOnly),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.list.get().set_enabled(enabled);
    }
}

/// `GroupBox`: a titled frame.
struct GroupBoxFactory;

impl<M: 'static> WidgetFactory<M> for GroupBoxFactory {
    fn kind(&self) -> &str {
        "GroupBox"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        // The children are mounted in the frame's own node once it exists (see
        // `placement`), so its content layout here stays empty.
        let group = Handle::new();
        let build = arrange::group(cx.text("text"), arrange::absolute()).bind(&group);
        cx.made(build, &group.clone(), GroupBoxProps { group })
    }
}

struct GroupBoxProps<M: 'static> {
    group: Handle<xui_core::GroupBox<M>>,
}

impl<M: 'static> WidgetProps<M> for GroupBoxProps<M> {
    fn id(&self) -> WidgetId {
        self.group.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.group.get().text())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.group.get().set_text(text);
                Ok(())
            }
            ("text", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }
}

/// `Panel`: a container that owns its children.
struct PanelFactory;

impl<M: 'static> WidgetFactory<M> for PanelFactory {
    fn kind(&self) -> &str {
        "Panel"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        // The children are mounted in the panel's node once it exists (see
        // `placement`), so its content layout here stays empty.
        let panel = Handle::new();
        let build = arrange::panel(arrange::absolute()).bind(&panel);
        cx.made(build, &panel.clone(), PanelProps { panel })
    }
}

struct PanelProps<M: 'static> {
    panel: Handle<xui_core::Panel<M>>,
}

impl<M: 'static> WidgetProps<M> for PanelProps<M> {
    fn id(&self) -> WidgetId {
        self.panel.get().id()
    }

    fn get_own(&self, _prop: &str) -> Option<Value> {
        None
    }

    fn set_own(&self, _prop: &str, _value: &Value) -> Result<(), SetError> {
        Err(SetError::UnknownProperty)
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.panel.get().set_enabled(enabled);
    }
}

/// `Separator`: a divider line.
struct SeparatorFactory;

impl<M: 'static> WidgetFactory<M> for SeparatorFactory {
    fn kind(&self) -> &str {
        "Separator"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let vertical = cx.text("orientation") == "vertical";
        let separator = Handle::new();
        let build = if vertical {
            arrange::vertical_separator()
        } else {
            arrange::separator()
        }
        .bind(&separator);
        let props = SeparatorProps {
            separator: separator.clone(),
            vertical,
        };
        cx.made(build, &separator, props)
    }
}

struct SeparatorProps<M: 'static> {
    separator: Handle<xui_core::Separator<M>>,
    vertical: bool,
}

impl<M: 'static> WidgetProps<M> for SeparatorProps<M> {
    fn id(&self) -> WidgetId {
        self.separator.get().id()
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

    fn create(&self, cx: &mut BuildCx<'_, M>, _node: &Node) -> Made<M> {
        let link = Handle::new();
        let mut build = arrange::hyperlink(cx.text("text")).bind(&link);
        if let Some(handler) = cx.handler("Click") {
            build = build.then(move |link| link.on_click(move || handler(&[])));
        }
        cx.made(build, &link.clone(), HyperlinkProps { link })
    }
}

struct HyperlinkProps<M: 'static> {
    link: Handle<xui_core::Hyperlink<M>>,
}

impl<M: 'static> WidgetProps<M> for HyperlinkProps<M> {
    fn id(&self) -> WidgetId {
        self.link.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "text" => Some(Value::Text(self.link.get().text())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("text", Value::Text(text)) => {
                self.link.get().set_text(text);
                Ok(())
            }
            ("text", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
    }

    fn set_enabled_hint(&self, enabled: bool) {
        self.link.get().set_enabled(enabled);
    }
}

/// `Timer`: a non-visual control that raises `Tick` periodically.
///
/// The timer is not a toolkit widget, so it is represented by a `Label`
/// placeholder: the designer draws it with the node's name (so it can be
/// selected and moved), while at run time it is hidden as soon as it is
/// created. Because the form uses an absolute layout, the hidden placeholder
/// never takes space from the other controls. The running timer itself lives in
/// the form window, not in this widget.
struct TimerFactory;

impl<M: 'static> WidgetFactory<M> for TimerFactory {
    fn kind(&self) -> &str {
        "Timer"
    }

    fn create(&self, cx: &mut BuildCx<'_, M>, node: &Node) -> Made<M> {
        let timer = Handle::new();
        let design = cx.design_mode();
        let text = if design {
            node.name.clone()
        } else {
            String::new()
        };
        let mut build = arrange::label(text).bind(&timer);
        if !design {
            // Hide the placeholder at run time. `then_with` runs when the
            // layout creates the widget, so the node is hidden before the
            // first paint.
            build = build.then_with(|label, ui| {
                ui.set_visible(label.id(), false);
                Ok(label)
            });
        }
        let props = TimerProps {
            label: timer.clone(),
            interval: Cell::new(cx.int("interval", 100).max(1)),
        };
        cx.made(build, &timer, props)
    }
}

struct TimerProps<M: 'static> {
    label: Handle<xui_core::Label<M>>,
    interval: Cell<i64>,
}

impl<M: 'static> WidgetProps<M> for TimerProps<M> {
    fn id(&self) -> WidgetId {
        self.label.get().id()
    }

    fn get_own(&self, prop: &str) -> Option<Value> {
        match prop {
            "interval" => Some(Value::Int(self.interval.get())),
            _ => None,
        }
    }

    fn set_own(&self, prop: &str, value: &Value) -> Result<(), SetError> {
        match (prop, value) {
            ("interval", Value::Int(interval)) if *interval >= 1 => {
                self.interval.set(*interval);
                Ok(())
            }
            ("interval", _) => Err(SetError::TypeMismatch),
            _ => Err(SetError::UnknownProperty),
        }
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
