#![forbid(unsafe_code)]

//! A form window's side of [`EventSource`]s: the timer that polls them.
//!
//! Each [`FormApp`](crate::FormApp) owns one [`Poller`]. After every message
//! the window handles, [`Poller::sync`] asks the sources whether the form has
//! work pending (a script subscribed to something, say) and starts or stops a
//! window timer to match, so an idle form costs nothing. Each tick becomes a
//! [`Msg::Poll`], and [`Poller::poll`] lets every source run what is ready in
//! the form's script. Dropping the poller (the window closed) releases what
//! the form registered.

use std::rc::Rc;

use xui_core::app::Ui;
use xui_core::backend::TimerId;

use crate::ScriptError;
use crate::extensions::{self, EventSource};
use crate::form::{FormInstance, Msg};

/// Polls the thread's event sources for one form.
pub(crate) struct Poller {
    form: String,
    sources: Vec<Rc<dyn EventSource>>,
    timer: Option<TimerId>,
}

impl Poller {
    /// A poller for `form`'s window. Registers the window's timer handler
    /// only when there is a source to poll.
    pub(crate) fn new(ui: &Ui<Msg>, form: &str) -> Poller {
        let sources = extensions::event_sources();
        if !sources.is_empty() {
            ui.on_timer(|_| Some(Msg::Poll));
        }
        Poller {
            form: form.to_owned(),
            sources,
            timer: None,
        }
    }

    /// A poller with nothing to poll, for a window whose form failed to build.
    pub(crate) fn idle() -> Poller {
        Poller {
            form: String::new(),
            sources: Vec::new(),
            timer: None,
        }
    }

    /// Whether the window's timer is running.
    pub(crate) fn is_polling(&self) -> bool {
        self.timer.is_some()
    }

    /// Starts the timer when a source has work for the form, stops it when
    /// none has.
    pub(crate) fn sync(&mut self, ui: &Ui<Msg>) {
        let wanted = self.sources.iter().any(|source| source.active(&self.form));
        match (wanted, self.timer) {
            (true, None) => {
                let interval = self
                    .sources
                    .iter()
                    .map(|source| source.interval_ms())
                    .min()
                    .unwrap_or(50)
                    .max(1);
                self.timer = Some(ui.set_timer(interval));
            }
            (false, Some(timer)) => {
                ui.kill_timer(timer);
                self.timer = None;
            }
            _ => {}
        }
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

    /// Stops the polling timer and releases what the form registered.
    ///
    /// This is what a hot reload calls before rebuilding the form (issue #91):
    /// `release` is keyed by form name, so it must run before the new
    /// `form_load` re-subscribes, or it would drop the new subscriptions too.
    /// Consuming `self` leaves nothing for [`Drop`] to release again.
    pub(crate) fn release(mut self, ui: &Ui<Msg>) {
        if let Some(timer) = self.timer.take() {
            ui.kill_timer(timer);
        }
        for source in &self.sources {
            source.release(&self.form);
        }
        self.sources.clear();
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        for source in &self.sources {
            source.release(&self.form);
        }
    }
}
