#![forbid(unsafe_code)]

//! The Rhai custom types that expose a live form to a script.
//!
//! A script never sees the raw `xui` widgets. It sees two small Rhai types:
//!
//! * [`Control`] wraps a shared [`FormHost`] handle plus a control name; its
//!   properties read and write the live widget through `LiveForm::get` /
//!   `LiveForm::set`. One type covers every control kind, so nothing here is
//!   written per widget type.
//! * [`Form`] is `Me`: the form's `caption`, its `state` object map (data that
//!   outlives a single event) and the `show`/`hide` methods.
//!
//! The property names are taken from the [`Catalog`], plus a short table of
//! VB-style aliases such as `caption` for `text` and `list_index` for
//! `selected`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Map, Position};
use xui_form::{Catalog, LiveForm, SetError, Value, ValueType};

/// The property surface a live form exposes to controls.
///
/// It is implemented for [`xui_form::LiveForm`] but kept as a trait so a host
/// without live widgets (a syntax checker, a mock) can drive the same engine.
pub trait FormHost {
    /// Reads `property` from the control named `control`.
    fn get(&self, control: &str, property: &str) -> Option<Value>;

    /// Writes `property` on the control named `control`.
    fn set(&self, control: &str, property: &str, value: &Value) -> Result<(), SetError>;

    /// The schema type of `property` on the control named `control`.
    fn property_type(&self, control: &str, property: &str) -> Option<ValueType>;

    /// The control names in the form.
    fn names(&self) -> Vec<String>;
}

impl<M: 'static> FormHost for LiveForm<M> {
    fn get(&self, control: &str, property: &str) -> Option<Value> {
        LiveForm::get(self, control, property)
    }

    fn set(&self, control: &str, property: &str, value: &Value) -> Result<(), SetError> {
        LiveForm::set(self, control, property, value)
    }

    fn property_type(&self, control: &str, property: &str) -> Option<ValueType> {
        LiveForm::property_type(self, control, property)
    }

    fn names(&self) -> Vec<String> {
        LiveForm::names(self).map(str::to_owned).collect()
    }
}

/// LazyRAD's VB-style property spellings, mapped onto catalog names.
const PROPERTY_ALIASES: &[(&str, &str)] = &[("caption", "text"), ("list_index", "selected")];

/// The catalog property a script name refers to, following a VB alias.
fn resolve_property(name: &str) -> &str {
    PROPERTY_ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, canonical)| canonical)
}

/// Every property name a control accepts: the common and widget properties from
/// `catalog`, plus the VB aliases, in sorted order.
pub fn control_property_names(catalog: &Catalog) -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for property in catalog.common_properties() {
        names.insert(property.name.clone());
    }
    for kind in catalog.kinds() {
        if let Some(spec) = catalog.get(kind) {
            for property in &spec.properties {
                names.insert(property.name.clone());
            }
        }
    }
    for (alias, _) in PROPERTY_ALIASES {
        names.insert((*alias).to_owned());
    }
    names.into_iter().collect()
}

/// A live control, addressed by name through the form handle.
#[derive(Clone)]
pub struct Control {
    host: Rc<dyn FormHost>,
    name: String,
}

impl Control {
    /// Wraps `host` for the control named `name`.
    pub fn new(host: Rc<dyn FormHost>, name: impl Into<String>) -> Control {
        Control {
            host,
            name: name.into(),
        }
    }

    /// The control's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Reads a property, following a VB alias.
    fn get(&self, property: &str) -> Result<Dynamic, Box<EvalAltResult>> {
        let property = resolve_property(property);
        match self.host.get(&self.name, property) {
            Some(value) => Ok(crate::value::to_dynamic(&value)),
            None => Err(runtime_error(format!(
                "`{}` has no property `{property}`",
                self.name
            ))),
        }
    }

    /// Writes a property, decoding the script value against the schema.
    fn set(&self, property: &str, value: Dynamic) -> Result<(), Box<EvalAltResult>> {
        let property = resolve_property(property);
        let Some(ty) = self.host.property_type(&self.name, property) else {
            return Err(runtime_error(format!(
                "`{}` has no property `{property}`",
                self.name
            )));
        };
        let value = crate::value::to_value(value, &ty).map_err(runtime_error)?;
        self.host
            .set(&self.name, property, &value)
            .map_err(|error| {
                runtime_error(format!(
                    "cannot set `{property}` on `{}`: {error}",
                    self.name
                ))
            })
    }
}

/// The form object a script calls `Me`.
///
/// `state` is a Rhai object map shared for the life of the form, so a value a
/// handler writes survives to the next event. `caption` starts empty and is
/// owned by the script.
#[derive(Clone)]
pub struct Form {
    host: Rc<dyn FormHost>,
    caption: Rc<RefCell<String>>,
    state: Rc<RefCell<Map>>,
}

impl Form {
    /// Creates the form object for `host`.
    pub fn new(host: Rc<dyn FormHost>) -> Form {
        Form {
            host,
            caption: Rc::new(RefCell::new(String::new())),
            state: Rc::new(RefCell::new(Map::new())),
        }
    }

    /// Shows or hides every control.
    fn set_visible(&self, visible: bool) {
        for name in self.host.names() {
            let _ = self.host.set(&name, "visible", &Value::Bool(visible));
        }
    }
}

/// Registers the [`Control`] type and a getter/setter for every property name
/// in `catalog`.
///
/// The property names are enumerated from the catalog and each one becomes a
/// `get$name`/`set$name` pair that routes through [`FormHost`], so a new widget
/// kind needs no new binding code here.
pub fn register_control(engine: &mut Engine, catalog: &Catalog) {
    engine.register_type_with_name::<Control>("Control");
    for name in control_property_names(catalog) {
        let getter = name.clone();
        engine.register_fn(format!("get${name}"), move |control: &mut Control| {
            control.get(&getter)
        });
        let setter = name;
        engine.register_fn(
            format!("set${setter}"),
            move |control: &mut Control, value: Dynamic| control.set(&setter, value),
        );
    }
}

/// Registers the [`Form`] (`Me`) type.
pub fn register_form(engine: &mut Engine) {
    engine.register_type_with_name::<Form>("Form");
    engine.register_get("caption", |form: &mut Form| form.caption.borrow().clone());
    engine.register_set("caption", |form: &mut Form, title: ImmutableString| {
        *form.caption.borrow_mut() = title.to_string();
    });
    engine.register_get("state", |form: &mut Form| form.state.borrow().clone());
    engine.register_set("state", |form: &mut Form, value: Dynamic| {
        if let Some(map) = value.try_cast::<Map>() {
            *form.state.borrow_mut() = map;
        }
    });
    engine.register_fn("show", |form: &mut Form| form.set_visible(true));
    engine.register_fn("hide", |form: &mut Form| form.set_visible(false));
}

/// Builds a Rhai runtime error with no position; the VM fills it in.
pub(crate) fn runtime_error(message: impl Into<String>) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(message.into()),
        Position::NONE,
    ))
}

/// A map of controls by name, for the `on_var` resolver.
pub(crate) fn controls_by_name(host: &Rc<dyn FormHost>) -> BTreeMap<String, Control> {
    host.names()
        .into_iter()
        .map(|name| {
            let control = Control::new(Rc::clone(host), name.clone());
            (name, control)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A host with two controls and no live widgets.
    struct MockHost {
        values: RefCell<BTreeMap<(String, String), Value>>,
    }

    impl MockHost {
        fn new() -> Self {
            let mut values = BTreeMap::new();
            values.insert(
                ("lbl".to_owned(), "text".to_owned()),
                Value::Text("hi".to_owned()),
            );
            MockHost {
                values: RefCell::new(values),
            }
        }
    }

    impl FormHost for MockHost {
        fn get(&self, control: &str, property: &str) -> Option<Value> {
            self.values
                .borrow()
                .get(&(control.to_owned(), property.to_owned()))
                .cloned()
        }

        fn set(&self, control: &str, property: &str, value: &Value) -> Result<(), SetError> {
            self.values
                .borrow_mut()
                .insert((control.to_owned(), property.to_owned()), value.clone());
            Ok(())
        }

        fn property_type(&self, control: &str, property: &str) -> Option<ValueType> {
            match (control, property) {
                ("lbl", "text") => Some(ValueType::Text { multiline: false }),
                ("cmd", "enabled") => Some(ValueType::Bool),
                _ => None,
            }
        }

        fn names(&self) -> Vec<String> {
            vec!["lbl".to_owned(), "cmd".to_owned()]
        }
    }

    #[test]
    fn aliases_resolve_to_catalog_names() {
        assert_eq!(resolve_property("caption"), "text");
        assert_eq!(resolve_property("list_index"), "selected");
        assert_eq!(resolve_property("enabled"), "enabled");
    }

    #[test]
    fn catalog_property_names_include_aliases_and_common_properties() {
        let names = control_property_names(&Catalog::xui());
        assert!(names.contains(&"caption".to_owned()));
        assert!(names.contains(&"list_index".to_owned()));
        assert!(names.contains(&"text".to_owned()));
        assert!(names.contains(&"enabled".to_owned()));
    }

    #[test]
    fn a_control_reads_and_writes_through_the_host() {
        let host: Rc<dyn FormHost> = Rc::new(MockHost::new());
        let control = Control::new(Rc::clone(&host), "lbl");
        assert_eq!(control.get("text").expect("text reads").to_string(), "hi");
        control
            .set("caption", Dynamic::from("bye".to_owned()))
            .expect("caption writes through");
        assert_eq!(host.get("lbl", "text"), Some(Value::Text("bye".to_owned())));
        assert!(control.get("text").is_ok());
    }

    #[test]
    fn an_unknown_property_is_an_error() {
        let host: Rc<dyn FormHost> = Rc::new(MockHost::new());
        let control = Control::new(host, "lbl");
        assert!(control.get("nope").is_err());
    }

    #[test]
    fn show_and_hide_toggle_every_control() {
        let host: Rc<dyn FormHost> = Rc::new(MockHost::new());
        let form = Form::new(Rc::clone(&host));
        form.set_visible(false);
        assert_eq!(host.get("lbl", "visible"), Some(Value::Bool(false)));
        assert_eq!(host.get("cmd", "visible"), Some(Value::Bool(false)));
        form.set_visible(true);
        assert_eq!(host.get("lbl", "visible"), Some(Value::Bool(true)));
    }
}
