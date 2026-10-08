#![forbid(unsafe_code)]

//! `run_with_backend` is the entry LazyOS uses: it must run a project on any
//! backend the caller supplies and report a backend that cannot be created.

use std::path::PathBuf;
use std::rc::Rc;

use lazyrad_player::{EXIT_COMPILE, EXIT_OK, EXIT_RUNTIME, run_with_backend};
use xui_canvas::OffscreenBackend;
use xui_core::backend::Backend;

fn sample(name: &str) -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    dir.join(name).to_string_lossy().into_owned()
}

#[test]
fn a_sample_runs_on_a_caller_supplied_backend() {
    let mut created = 0;
    let code = run_with_backend(&[sample("hello")], &mut |runtime| {
        assert!(runtime.is_some(), "a project run hands over its runtime");
        created += 1;
        Ok(Rc::new(OffscreenBackend::new()) as Rc<dyn Backend>)
    });
    assert_eq!(code, EXIT_OK);
    assert_eq!(created, 1, "the backend is created exactly once");
}

#[test]
fn a_backend_that_cannot_connect_is_a_runtime_failure() {
    let code = run_with_backend(&[sample("hello")], &mut |_| {
        Err("no display server".to_owned())
    });
    assert_eq!(code, EXIT_RUNTIME);
}

#[test]
fn a_bad_project_fails_before_any_backend_is_created() {
    let code = run_with_backend(&[sample("does-not-exist")], &mut |_| {
        panic!("the backend must not be created for a project that does not load")
    });
    assert_eq!(code, EXIT_COMPILE);
}

#[test]
fn extra_arguments_are_a_usage_error() {
    let code = run_with_backend(&["a".to_owned(), "b".to_owned()], &mut |_| {
        panic!("no backend for a usage error")
    });
    assert_eq!(code, EXIT_COMPILE);
}

#[test]
fn watch_before_a_project_enables_hot_reload() {
    let mut created = 0;
    let code = run_with_backend(&["--watch".to_owned(), sample("hello")], &mut |runtime| {
        let runtime = runtime.expect("a project run hands over its runtime");
        assert!(runtime.watch_enabled(), "--watch enables hot reload");
        created += 1;
        Ok(Rc::new(OffscreenBackend::new()) as Rc<dyn Backend>)
    });
    assert_eq!(code, EXIT_OK);
    assert_eq!(created, 1);
}

#[test]
fn watch_without_a_project_is_a_usage_error() {
    let code = run_with_backend(&["--watch".to_owned()], &mut |_| {
        panic!("no backend for a usage error")
    });
    assert_eq!(code, EXIT_COMPILE);
}
