#![forbid(unsafe_code)]

//! Script extensions a host program adds to every LazyRAD engine.
//!
//! The standard library ([`crate::stdlib`]) is the same on every platform. A
//! host can offer more: the LazyOS player adds the `msg` module, so a form
//! script can call system services over Messenger
//! (`msg::connect("os.lazy.confd.v1").info()`). The host registers an
//! extension once, before it loads a project, and [`crate::stdlib::register`]
//! applies every extension to every engine it sets up: the player's, the
//! designer preview's and the syntax checker's.
//!
//! The registry is per thread, because Rhai engines and the forms they script
//! live on the UI thread. Extensions run after the standard library, so an
//! extension may add functions but cannot remove standard ones.

use std::cell::RefCell;
use std::rc::Rc;

use rhai::Engine;

/// One extension: called with each new engine.
pub type Extension = Rc<dyn Fn(&mut Engine)>;

thread_local! {
    static EXTENSIONS: RefCell<Vec<Extension>> = const { RefCell::new(Vec::new()) };
}

/// Adds `extension` to every engine set up on this thread from now on.
pub fn add(extension: impl Fn(&mut Engine) + 'static) {
    EXTENSIONS.with(|list| list.borrow_mut().push(Rc::new(extension)));
}

/// Removes every extension added on this thread.
pub fn clear() {
    EXTENSIONS.with(|list| list.borrow_mut().clear());
}

/// How many extensions are registered on this thread.
pub fn count() -> usize {
    EXTENSIONS.with(|list| list.borrow().len())
}

/// Applies every registered extension to `engine`, in the order they were added.
pub(crate) fn apply(engine: &mut Engine) {
    // Cloned out first, so an extension that itself calls `add` cannot hit a
    // re-entrant borrow.
    let list: Vec<Extension> = EXTENSIONS.with(|list| list.borrow().clone());
    for extension in list {
        extension(engine);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        apply(&mut engine);
        // The later registration of the same signature wins.
        assert_eq!(engine.eval::<i64>("answer()").expect("answer runs"), 42);
        clear();
        assert_eq!(count(), 0);
        let mut bare = crate::new_engine();
        apply(&mut bare);
        assert!(bare.eval::<i64>("answer()").is_err());
    }

    #[test]
    fn an_extension_may_add_another_without_a_borrow_panic() {
        clear();
        add(|_| add(|_| {}));
        apply(&mut crate::new_engine());
        assert_eq!(count(), 2);
        clear();
    }
}
