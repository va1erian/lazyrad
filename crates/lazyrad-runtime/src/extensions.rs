#![forbid(unsafe_code)]

//! Script extensions a host program adds to every LazyRAD engine, and event
//! sources that call back into a form's script.
//!
//! The standard library ([`crate::stdlib`]) is the same on every platform. A
//! host can offer more: the LazyOS player adds the `msg` module and the
//! generated `sys::*` modules, so a form script can call system services over
//! Messenger (`sys::confd::get("sys/ui/theme")`). The host registers an
//! extension once, before it loads a project, and [`crate::stdlib::register`]
//! applies every extension to every engine it sets up: the player's, the
//! designer preview's and the syntax checker's. [`add_scoped`] also tells the
//! extension which form the engine scripts, so what a script registers can be
//! filed under its form.
//!
//! Some extensions deliver work later: a Messenger event, a call another
//! program makes to a service the script serves. Such a host adds an
//! [`EventSource`]. While a form has work pending ([`EventSource::active`]),
//! its window runs a timer and hands the source a way to call the form's
//! script ([`EventSource::poll`]); when the window closes the source drops what
//! the form registered ([`EventSource::release`]). A source never blocks, so
//! the window keeps painting.
//!
//! The registries are per thread, because Rhai engines and the forms they
//! script live on the UI thread. Extensions run after the standard library, so
//! an extension may add functions but cannot remove standard ones.

use std::cell::RefCell;
use std::rc::Rc;

use rhai::{Dynamic, Engine, EvalAltResult, FnPtr};

/// What an extension learns about the engine it is applied to.
#[derive(Clone, Copy, Debug)]
pub struct ExtensionScope<'a> {
    /// The form the engine scripts (the checker uses the form it checks).
    pub form: &'a str,
}

/// One extension: called with each new engine and its scope.
pub type Extension = Rc<dyn Fn(&mut Engine, &ExtensionScope<'_>)>;

/// Runs a function pointer in the form's own engine and script: a named
/// function or a closure the script handed to the extension.
pub type ScriptCall<'a> =
    dyn FnMut(&FnPtr, Vec<Dynamic>) -> Result<Dynamic, Box<EvalAltResult>> + 'a;

/// Work that arrives outside the window and runs in a form's script.
pub trait EventSource {
    /// How often a window with pending work polls, in milliseconds.
    fn interval_ms(&self) -> u32;
    /// Whether `form` registered anything to poll for. Checked after every
    /// message the window handles, so the timer runs only while needed.
    fn active(&self, form: &str) -> bool;
    /// Run whatever is ready for `form` through `call`, without waiting.
    /// Returns the errors to show the user (the runtime shows each like an
    /// event handler's error and keeps running).
    fn poll(&self, form: &str, call: &mut ScriptCall<'_>) -> Vec<Box<EvalAltResult>>;
    /// The form's window closed: drop everything it registered.
    fn release(&self, form: &str);
}

thread_local! {
    static EXTENSIONS: RefCell<Vec<Extension>> = const { RefCell::new(Vec::new()) };
    static SOURCES: RefCell<Vec<Rc<dyn EventSource>>> = const { RefCell::new(Vec::new()) };
}

/// Adds `extension` to every engine set up on this thread from now on.
pub fn add(extension: impl Fn(&mut Engine) + 'static) {
    add_scoped(move |engine, _| extension(engine));
}

/// Like [`add`], and the extension also learns which form each engine scripts.
pub fn add_scoped(extension: impl Fn(&mut Engine, &ExtensionScope<'_>) + 'static) {
    EXTENSIONS.with(|list| list.borrow_mut().push(Rc::new(extension)));
}

/// Adds `source`; every form window on this thread polls it while it has work.
pub fn add_event_source(source: Rc<dyn EventSource>) {
    SOURCES.with(|list| list.borrow_mut().push(source));
}

/// Removes every extension and event source added on this thread.
pub fn clear() {
    EXTENSIONS.with(|list| list.borrow_mut().clear());
    SOURCES.with(|list| list.borrow_mut().clear());
}

/// How many extensions are registered on this thread.
pub fn count() -> usize {
    EXTENSIONS.with(|list| list.borrow().len())
}

/// Every event source registered on this thread.
pub fn event_sources() -> Vec<Rc<dyn EventSource>> {
    SOURCES.with(|list| list.borrow().clone())
}

/// Applies every registered extension to `engine`, in the order they were added.
pub(crate) fn apply(engine: &mut Engine, scope: &ExtensionScope<'_>) {
    // Cloned out first, so an extension that itself calls `add` cannot hit a
    // re-entrant borrow.
    let list: Vec<Extension> = EXTENSIONS.with(|list| list.borrow().clone());
    for extension in list {
        extension(engine, scope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCOPE: ExtensionScope<'static> = ExtensionScope { form: "main" };

    #[test]
    fn extensions_apply_in_order_and_clear() {
        clear();
        add(|engine| {
            engine.register_fn("answer", || 41_i64);
        });
        add(|engine| {
            engine.register_fn("answer", || 42_i64);
        });
        assert_eq!(count(), 2);
        let mut engine = crate::new_engine();
        apply(&mut engine, &SCOPE);
        // The later registration of the same signature wins.
        assert_eq!(engine.eval::<i64>("answer()").expect("answer runs"), 42);
        clear();
        assert_eq!(count(), 0);
        let mut bare = crate::new_engine();
        apply(&mut bare, &SCOPE);
        assert!(bare.eval::<i64>("answer()").is_err());
    }

    #[test]
    fn an_extension_may_add_another_without_a_borrow_panic() {
        clear();
        add(|_| add(|_| {}));
        apply(&mut crate::new_engine(), &SCOPE);
        assert_eq!(count(), 2);
        clear();
    }

    #[test]
    fn a_scoped_extension_learns_the_form() {
        clear();
        add_scoped(|engine, scope| {
            let form = scope.form.to_owned();
            engine.register_fn("owner", move || form.clone());
        });
        let mut engine = crate::new_engine();
        apply(&mut engine, &ExtensionScope { form: "settings" });
        assert_eq!(engine.eval::<String>("owner()").unwrap(), "settings");
        clear();
    }

    struct Quiet;

    impl EventSource for Quiet {
        fn interval_ms(&self) -> u32 {
            50
        }
        fn active(&self, _form: &str) -> bool {
            false
        }
        fn poll(&self, _form: &str, _call: &mut ScriptCall<'_>) -> Vec<Box<EvalAltResult>> {
            Vec::new()
        }
        fn release(&self, _form: &str) {}
    }

    #[test]
    fn event_sources_register_and_clear() {
        clear();
        add_event_source(Rc::new(Quiet));
        assert_eq!(event_sources().len(), 1);
        clear();
        assert!(event_sources().is_empty());
    }
}
