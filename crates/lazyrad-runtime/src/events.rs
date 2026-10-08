#![forbid(unsafe_code)]

//! A form window's side of [`EventSource`]s: the sources and their poller.
//!
//! Each [`FormApp`](crate::FormApp) owns one [`Poller`], wrapped by
//! [`crate::timers::Timers`] together with the form's `Timer` controls. The
//! poller asks the thread's sources whether the form has work pending (a script
//! subscribed to something, say); [`Poller::poll`] lets every source run what
//! is ready in the form's script. Dropping the poller (the window closed)
//! releases what the form registered. The window timer that drives it lives in
//! [`crate::timers`], so it can share one timer mapper with the `Timer`
//! controls.

use std::rc::Rc;

use crate::ScriptError;
use crate::extensions::{self, EventSource};
use crate::form::FormInstance;

/// The thread's event sources, polled for one form.
pub(crate) struct Poller {
    form: String,
    sources: Vec<Rc<dyn EventSource>>,
}

impl Poller {
    /// A poller for `form`.
    pub(crate) fn new(form: &str) -> Poller {
        Poller {
            form: form.to_owned(),
            sources: extensions::event_sources(),
        }
    }

    /// A poller with nothing to poll, for a window whose form failed to build.
    pub(crate) fn idle() -> Poller {
        Poller {
            form: String::new(),
            sources: Vec::new(),
        }
    }

    /// Whether any source has work for the form.
    pub(crate) fn active(&self) -> bool {
        self.sources.iter().any(|source| source.active(&self.form))
    }

    /// The interval a window should poll at while a source has work, in
    /// milliseconds: the smallest any source asks for, never below one.
    pub(crate) fn interval(&self) -> u32 {
        self.sources
            .iter()
            .map(|source| source.interval_ms())
            .min()
            .unwrap_or(50)
            .max(1)
    }

    /// Lets every source run what is ready in `instance`'s script; returns the
    /// errors to show.
    pub(crate) fn poll(&self, instance: &FormInstance) -> Vec<ScriptError> {
        let mut errors = Vec::new();
        for source in &self.sources {
            let mut call =
                |callback: &rhai::FnPtr, args: Vec<rhai::Dynamic>| instance.call_fn(callback, args);
            for error in source.poll(&self.form, &mut call) {
                errors.push(instance.locate(&error));
            }
        }
        errors
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        for source in &self.sources {
            source.release(&self.form);
        }
    }
}
