#![forbid(unsafe_code)]

//! The toolkit window the player and the IDE share.
//!
//! [`run_empty_window`] opens a `winit`-backed xui window on an application
//! that draws nothing but the theme background. M0 uses it to prove the
//! windowed canvas path links and runs on both targets; the real widget trees
//! replace it as the milestones land (PLAN.md §9, §11).

use std::rc::Rc;

use xui_canvas::WinitBackend;
use xui_core::app::{App, Ui, run_app};
use xui_core::backend::{Backend, PlatformSpec, Result};

/// An application whose window is empty.
struct EmptyApp;

impl App for EmptyApp {
    type Msg = ();

    fn update(&mut self, _msg: (), _ui: &mut Ui<()>) {}
}

/// Opens an empty xui window titled `title` and runs until the user closes it.
///
/// The window uses the portable [`WinitBackend`], the same backend LazyOS
/// builds on, so the code run here is the code that ships.
pub fn run_empty_window(title: &str) -> Result<()> {
    let backend: Rc<dyn Backend> = Rc::new(WinitBackend::new());
    run_app(backend, PlatformSpec::new(title), |_ui| EmptyApp)
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use xui_canvas::OffscreenBackend;
    use xui_core::app::run_app;
    use xui_core::backend::{Backend, PlatformSpec};
    use xui_core::units::Dip;

    use super::EmptyApp;

    #[test]
    fn an_empty_window_builds_and_runs_headlessly() {
        // The offscreen backend never opens a real window, so the whole
        // open/build/drain path runs on a CI runner without a display.
        let backend: Rc<dyn Backend> = Rc::new(OffscreenBackend::new());
        let spec = PlatformSpec::new("lazyrad test").size(Dip(320.0), Dip(200.0));
        run_app(backend, spec, |_ui| EmptyApp).expect("an empty window runs to completion");
    }
}
