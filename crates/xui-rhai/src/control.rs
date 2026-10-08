#![forbid(unsafe_code)]

//! The Rhai custom types that expose a live form to a script.
//!
//! A script never sees the raw `xui` widgets. It sees two small Rhai types:
//!
//! * [`Control`] wraps a shared [`FormHost`] handle plus a control name; its
//!   properties read and write the live widget through `LiveForm::get` /
//!   `LiveForm::set`, and its methods (`canvas1.fill_rect(...)`) go through
//!   `LiveForm::call`. One type covers every control kind, so nothing here is
//!   written per widget type.
//! * [`Form`] is `form` in scripts: the form's `title`, its `state` object map
//!   (data that outlives a single event) and the `show`/`hide` methods.
//!
//! The property names are exactly the [`Catalog`]'s (PLAN.md §1.1): there are
//! no aliases.

use std::any::TypeId;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Map, Position};
use xui_form::{CallError, Catalog, LiveForm, MethodSpec, SetError, Value, ValueType};

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

    /// The canonical widget kind of the control named `control` (`Edit`).
    fn kind(&self, control: &str) -> Option<String>;

    /// The property names the control named `control` accepts, in catalog
    /// order.
    fn property_names(&self, control: &str) -> Vec<String>;

    /// Calls `method` on the control named `control` with `args`, returning
    /// its result (`None` for a method that returns nothing). The default
    /// knows no methods, so a host without live widgets need not implement
    /// it.
    fn call(
        &self,
        control: &str,
        method: &str,
        args: &[Value],
    ) -> Result<Option<Value>, CallError> {
        let _ = (control, method, args);
        Err(CallError::UnknownMethod)
    }

    /// The spec of `method` on the control named `control`, if its kind
    /// declares that method. The default knows none.
    fn method_spec(&self, control: &str, method: &str) -> Option<MethodSpec> {
        let _ = (control, method);
        None
    }

    /// The method names the control named `control` has, in catalog order.
    /// The default knows none.
    fn method_names(&self, control: &str) -> Vec<String> {
        let _ = control;
        Vec::new()
    }
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

    fn kind(&self, control: &str) -> Option<String> {
        LiveForm::kind(self, control).map(str::to_owned)
    }

    fn property_names(&self, control: &str) -> Vec<String> {
        LiveForm::kind(self, control)
            .map(|kind| self.catalog().property_names(kind))
            .unwrap_or_default()
    }

    fn call(
        &self,
        control: &str,
        method: &str,
        args: &[Value],
    ) -> Result<Option<Value>, CallError> {
        LiveForm::call(self, control, method, args)
    }

    fn method_spec(&self, control: &str, method: &str) -> Option<MethodSpec> {
        LiveForm::method_spec(self, control, method).cloned()
    }

    fn method_names(&self, control: &str) -> Vec<String> {
        LiveForm::kind(self, control)
            .and_then(|kind| self.catalog().get(kind))
            .map(|spec| {
                spec.methods
                    .iter()
                    .map(|method| method.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Every method name a control may have, across every kind in `catalog`, with
/// the largest argument count any kind declares for it.
pub fn control_methods(catalog: &Catalog) -> BTreeMap<String, usize> {
    let mut methods: BTreeMap<String, usize> = BTreeMap::new();
    for kind in catalog.kinds() {
        if let Some(spec) = catalog.get(kind) {
            for method in &spec.methods {
                let arity = methods.entry(method.name.clone()).or_default();
                *arity = (*arity).max(method.args.len());
            }
        }
    }
    methods
}

/// Every property name a control accepts: the common and widget properties from
/// `catalog`, in sorted order.
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

    /// Reads a property.
    fn get(&self, property: &str) -> Result<Dynamic, Box<EvalAltResult>> {
        match self.host.get(&self.name, property) {
            Some(value) => Ok(crate::value::to_dynamic(&value)),
            None => Err(self.unknown_property(property)),
        }
    }

    /// The error for a property this control's kind does not have.
    ///
    /// It suggests the script name when `property` differs from a real one only
    /// by case or by CamelCase (`Text`, `TabIndex`); otherwise it lists the
    /// names the kind accepts.
    fn unknown_property(&self, property: &str) -> Box<EvalAltResult> {
        let kind = self.host.kind(&self.name).unwrap_or_default();
        let names = self.host.property_names(&self.name);
        let wanted = snake_case(property);
        let message = match names.iter().find(|name| **name == wanted) {
            Some(suggestion) => format!(
                "unknown property '{property}' on {} ({kind}); did you mean '{suggestion}'?",
                self.name
            ),
            None => format!(
                "unknown property '{property}' on {} ({kind}); properties: {}",
                self.name,
                names.join(", ")
            ),
        };
        runtime_error(message)
    }

    /// Calls a method, converting each script argument with the method's
    /// schema ([`crate::value::to_arg_value`]: a float for an int parameter is
    /// truncated, an int for a float one widened, a colour is a `"#rrggbb"`
    /// string or a `0xRRGGBB` int) and the result back (`()` for none).
    fn call(&self, method: &str, args: Vec<Dynamic>) -> Result<Dynamic, Box<EvalAltResult>> {
        let Some(spec) = self.host.method_spec(&self.name, method) else {
            return Err(self.unknown_method(method));
        };
        if args.len() != spec.args.len() {
            let names: Vec<&str> = spec.args.iter().map(|arg| arg.name.as_str()).collect();
            return Err(runtime_error(format!(
                "{}.{method} takes {} argument{} ({}), got {}",
                self.name,
                names.len(),
                if names.len() == 1 { "" } else { "s" },
                names.join(", "),
                args.len()
            )));
        }
        let mut values = Vec::with_capacity(args.len());
        for (index, (arg, dynamic)) in spec.args.iter().zip(args).enumerate() {
            let value = crate::value::to_arg_value(dynamic, &arg.ty).map_err(|error| {
                runtime_error(format!(
                    "{}.{method}: argument {} (`{}`): {error}",
                    self.name,
                    index + 1,
                    arg.name
                ))
            })?;
            values.push(value);
        }
        match self.host.call(&self.name, method, &values) {
            Ok(Some(value)) => Ok(crate::value::to_dynamic(&value)),
            Ok(None) => Ok(Dynamic::UNIT),
            Err(CallError::UnknownMethod) => Err(self.unknown_method(method)),
            Err(error) => Err(runtime_error(format!("{}.{method}: {error}", self.name))),
        }
    }

    /// The error for a method this control's kind does not have, listing the
    /// ones it does.
    fn unknown_method(&self, method: &str) -> Box<EvalAltResult> {
        let kind = self.host.kind(&self.name).unwrap_or_default();
        let names = self.host.method_names(&self.name);
        let listed = if names.is_empty() {
            "none".to_owned()
        } else {
            names.join(", ")
        };
        runtime_error(format!(
            "unknown method '{method}' on {} ({kind}); methods: {listed}",
            self.name
        ))
    }

    /// Writes a property, decoding the script value against the schema.
    fn set(&self, property: &str, value: Dynamic) -> Result<(), Box<EvalAltResult>> {
        let Some(ty) = self.host.property_type(&self.name, property) else {
            return Err(self.unknown_property(property));
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

/// `name` in snake_case: `TabIndex` becomes `tab_index`, `TEXT` becomes `text`.
fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (index, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let after_lower = index > 0 && chars[index - 1].is_lowercase();
            let before_lower = chars.get(index + 1).is_some_and(|n| n.is_lowercase());
            let after_upper = index > 0 && chars[index - 1].is_uppercase();
            if after_lower || (after_upper && before_lower) {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The form object a script calls `form`.
///
/// `state` is a Rhai object map shared for the life of the form, so a value a
/// handler writes survives to the next event. `title` starts empty and is
/// owned by the script.
#[derive(Clone)]
pub struct Form {
    host: Rc<dyn FormHost>,
    title: Rc<RefCell<String>>,
    state: Rc<RefCell<Map>>,
}

impl Form {
    /// Creates the form object for `host`.
    pub fn new(host: Rc<dyn FormHost>) -> Form {
        Form {
            host,
            title: Rc::new(RefCell::new(String::new())),
            state: Rc::new(RefCell::new(Map::new())),
        }
    }

    /// The shared `state` map, so a host that reloads the form can carry the
    /// old state into the new script's `form_reload` (issue #91).
    pub(crate) fn state_handle(&self) -> Rc<RefCell<Map>> {
        Rc::clone(&self.state)
    }

    /// Shows or hides every control.
    fn set_visible(&self, visible: bool) {
        for name in self.host.names() {
            let _ = self.host.set(&name, "visible", &Value::Bool(visible));
        }
    }
}

/// How many arguments past a method's own count are still routed to it, so a
/// call with too many arguments gets the method's own arity error rather than
/// Rhai's "function not found".
const EXTRA_ARITY: usize = 2;

/// Registers the [`Control`] type, a getter/setter for every property name
/// in `catalog`, and every method any kind declares.
///
/// The property names are enumerated from the catalog and each one becomes a
/// `get$name`/`set$name` pair that routes through [`FormHost`], so a new widget
/// kind needs no new binding code here.
///
/// A string indexer is registered too. Rhai falls back to an indexer with the
/// property name as the key when no getter or setter exists for `obj.name`
/// (`ErrorDotExpr`, see `eval/chaining.rs`), so `edit1.Text = ...` lands in the
/// indexer instead of Rhai's generic "No writable property" error, and the
/// indexer can answer with a message that names the control and suggests the
/// right property. Rhai reports that error at the property's position. The
/// indexer also makes `edit1["text"]` work as a dynamic property access.
///
/// Each method name is registered on `Control` for every argument count from
/// none up to a couple past the largest any kind declares, with untyped
/// (`Dynamic`) arguments. So `label1.fill_rect(...)` reaches the control and
/// fails with "unknown method 'fill_rect' on label1 (Label)", and
/// `canvas1.fill_rect(1, 2)` fails with the method's real signature, rather
/// than with Rhai's generic "function not found"; the method's own schema
/// then converts each argument.
pub fn register_control(engine: &mut Engine, catalog: &Catalog) {
    engine.register_type_with_name::<Control>("Control");
    engine.register_indexer_get(|control: &mut Control, property: ImmutableString| {
        control.get(&property)
    });
    engine.register_indexer_set(
        |control: &mut Control, property: ImmutableString, value: Dynamic| {
            control.set(&property, value)
        },
    );
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
    for (name, arity) in control_methods(catalog) {
        for count in 0..=arity + EXTRA_ARITY {
            let mut types = vec![TypeId::of::<Control>()];
            types.extend(std::iter::repeat_n(TypeId::of::<Dynamic>(), count));
            let method = name.clone();
            engine.register_raw_fn(
                name.as_str(),
                types,
                move |_context, args| -> Result<Dynamic, Box<EvalAltResult>> {
                    let (receiver, rest) = args
                        .split_first_mut()
                        .ok_or_else(|| runtime_error(format!("{method} needs a control")))?;
                    let control = receiver.clone_cast::<Control>();
                    let values = rest.iter_mut().map(|arg| std::mem::take(*arg)).collect();
                    control.call(&method, values)
                },
            );
        }
    }
}

/// Registers the [`Form`] (`form`) type.
pub fn register_form(engine: &mut Engine) {
    engine.register_type_with_name::<Form>("Form");
    engine.register_get("title", |form: &mut Form| form.title.borrow().clone());
    engine.register_set("title", |form: &mut Form, title: ImmutableString| {
        *form.title.borrow_mut() = title.to_string();
    });
    engine.register_get("state", |form: &mut Form| form.state.borrow().clone());
    engine.register_set(
        "state",
        |form: &mut Form, value: Dynamic| -> Result<(), Box<EvalAltResult>> {
            let found = value.type_name();
            let map = value.try_cast::<Map>().ok_or_else(|| {
                runtime_error(format!("`form.state` must be a map, found {found}"))
            })?;
            *form.state.borrow_mut() = map;
            Ok(())
        },
    );
    // Rhai's fallback for a property without a getter/setter (see
    // `register_control`): a helpful error instead of the generic one.
    engine.register_indexer_get(
        |_form: &mut Form, property: ImmutableString| -> Result<Dynamic, Box<EvalAltResult>> {
            Err(unknown_form_property(&property))
        },
    );
    engine.register_indexer_set(
        |_form: &mut Form,
         property: ImmutableString,
         _value: Dynamic|
         -> Result<(), Box<EvalAltResult>> { Err(unknown_form_property(&property)) },
    );
    engine.register_fn("show", |form: &mut Form| form.set_visible(true));
    engine.register_fn("hide", |form: &mut Form| form.set_visible(false));
}

/// The properties `form` has, for the unknown-property message.
const FORM_PROPERTIES: [&str; 2] = ["title", "state"];

fn unknown_form_property(property: &str) -> Box<EvalAltResult> {
    let wanted = snake_case(property);
    runtime_error(match FORM_PROPERTIES.iter().find(|name| **name == wanted) {
        Some(suggestion) => {
            format!("unknown property '{property}' on form; did you mean '{suggestion}'?")
        }
        None => format!(
            "unknown property '{property}' on form; properties: {}",
            FORM_PROPERTIES.join(", ")
        ),
    })
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

        fn kind(&self, _control: &str) -> Option<String> {
            Some("Label".to_owned())
        }

        fn property_names(&self, _control: &str) -> Vec<String> {
            vec!["text".to_owned(), "left".to_owned(), "tab_index".to_owned()]
        }
    }

    #[test]
    fn catalog_property_names_are_the_catalogs_own() {
        let names = control_property_names(&Catalog::xui());
        assert!(!names.contains(&"caption".to_owned()), "no VB aliases");
        assert!(!names.contains(&"list_index".to_owned()), "no VB aliases");
        assert!(names.contains(&"selected".to_owned()));
        assert!(names.contains(&"text".to_owned()));
        assert!(names.contains(&"enabled".to_owned()));
    }

    #[test]
    fn a_control_reads_and_writes_through_the_host() {
        let host: Rc<dyn FormHost> = Rc::new(MockHost::new());
        let control = Control::new(Rc::clone(&host), "lbl");
        assert_eq!(control.get("text").expect("text reads").to_string(), "hi");
        control
            .set("text", Dynamic::from("bye".to_owned()))
            .expect("text writes through");
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
