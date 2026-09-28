#![forbid(unsafe_code)]

//! Building the Rhai [`Engine`](rhai::Engine) the runtime evaluates scripts
//! with.
//!
//! The feature set is fixed by the workspace manifest (PLAN.md §1): `debugging`
//! for breakpoints and call stacks, `metadata` for completion, and `internals`
//! for the tokenizer, with `sync` deliberately absent so the engine keeps `Rc`.
//! The stdlib registration and the `on_var` control resolver are added in M1.

use rhai::Engine;

/// A fresh Rhai engine configured for LazyRAD.
///
/// Each form gets its own engine because the resolver and the registered
/// control types are per-form state.
pub fn new_engine() -> Engine {
    Engine::new()
}

#[cfg(test)]
mod tests {
    use super::new_engine;

    #[test]
    fn the_engine_evaluates_rhai() {
        let engine = new_engine();
        let value: i64 = engine.eval("40 + 2").expect("a trivial script evaluates");
        assert_eq!(value, 42);
    }

    #[test]
    fn the_metadata_feature_reports_registered_functions() {
        // `gen_fn_metadata_to_json` only exists with the `metadata` feature,
        // so building at all proves the feature is on.
        let mut engine = new_engine();
        engine.register_fn("lazyrad_probe", |x: i64| x + 1);
        let metadata = engine
            .gen_fn_metadata_to_json(true)
            .expect("metadata serialises to JSON");
        assert!(metadata.contains("lazyrad_probe"));
    }
}
