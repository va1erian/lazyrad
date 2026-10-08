#![forbid(unsafe_code)]

//! One form window's timers: the event-source poller and every `Timer`
//! control, dispatched by id through the window's single timer mapper.
//!
//! xui lets a window install one `on_timer` mapper ([`Ui::on_timer`]) and
//! raises every timer's tick through it with a [`TimerId`]. A form may have two
//! kinds of timer at once: the poller that drives the thread's event sources
//! ([`crate::events::Poller`]) and any number of `Timer` controls. [`Timers`]
//! keeps a route from each live [`TimerId`] to what its tick means, so the
//! mapper can answer for both.
//!
//! After every message the window handles, [`Timers::sync`] brings the running
//! timers in line with the form's state: a source starting or stopping its
//! work, or a script setting `timer1.enabled`/`timer1.interval`. It also drops
//! the timer of a control that was removed or renamed, so no tick is delivered
//! to a control that no longer exists.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use xui_core::app::Ui;
use xui_core::backend::TimerId;
use xui_form::{LiveForm, Value};

use crate::ScriptError;
use crate::events::Poller;
use crate::form::{FormInstance, Msg};

/// How often a window looks for the answer of an open-file dialog, in
/// milliseconds. A person takes seconds to pick a file, so this is quick enough.
const DIALOG_POLL_MS: u32 = 50;

/// What a window timer's tick means.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Route {
    /// The event-source poller's tick.
    Poll,
    /// A `Timer` control's tick.
    Tick(String),
    /// The dialog poller's tick: collect the answers of open-file dialogs.
    Dialogs,
}

impl Route {
    /// The message a tick raises.
    fn message(&self) -> Msg {
        match self {
            // An answered dialog is delivered by the same message as work from
            // the event sources: both mean "something arrived for this form".
            Route::Poll | Route::Dialogs => Msg::Poll,
            Route::Tick(control) => Msg::Tick {
                control: control.clone(),
            },
        }
    }
}

/// A running `Timer` control.
struct Running {
    /// The window timer that drives it.
    id: TimerId,
    /// The interval it currently runs at, in milliseconds.
    interval: u32,
}

/// Every window timer one form owns.
pub(crate) struct Timers {
    /// The thread's event sources, polled while they have work.
    poller: Poller,
    /// The poller's window timer, when a source has work.
    poll_timer: Option<TimerId>,
    /// The timer that collects open-file dialog answers while one is open.
    dialog_timer: Option<TimerId>,
    /// Each `Timer` control's window timer, keyed by control name.
    controls: BTreeMap<String, Running>,
    /// `TimerId` to what its tick means, shared with the window's mapper.
    routes: Rc<RefCell<BTreeMap<usize, Route>>>,
}

impl Timers {
    /// The timers for `form`'s window, installing the window's timer mapper.
    ///
    /// The mapper answers only for the ids this window started, so a form with
    /// no timers costs nothing beyond the lookup.
    pub(crate) fn new(ui: &Ui<Msg>, form: &str) -> Timers {
        let routes: Rc<RefCell<BTreeMap<usize, Route>>> = Rc::new(RefCell::new(BTreeMap::new()));
        let for_mapper = Rc::clone(&routes);
        ui.on_timer(move |id| for_mapper.borrow().get(&id.0).map(Route::message));
        Timers {
            poller: Poller::new(form),
            poll_timer: None,
            dialog_timer: None,
            controls: BTreeMap::new(),
            routes,
        }
    }

    /// Timers for a window whose form failed to build: nothing runs.
    pub(crate) fn idle() -> Timers {
        Timers {
            poller: Poller::idle(),
            poll_timer: None,
            dialog_timer: None,
            controls: BTreeMap::new(),
            routes: Rc::new(RefCell::new(BTreeMap::new())),
        }
    }

    /// Whether the window is polling the host's event sources for its form.
    pub(crate) fn is_polling(&self) -> bool {
        self.poll_timer.is_some()
    }

    /// Whether the `Timer` control named `name` currently has a running timer.
    pub(crate) fn is_control_running(&self, name: &str) -> bool {
        self.controls.contains_key(name)
    }

    /// Brings every window timer in line with the form's current state.
    pub(crate) fn sync(&mut self, ui: &Ui<Msg>, form: &LiveForm<Msg>) {
        self.sync_poll(ui);
        self.sync_controls(ui, form);
    }

    /// Starts the dialog poller while an open-file dialog is waiting and stops it
    /// once none is. A worker cannot send this window a message (`Msg` carries
    /// script function pointers and is not `Send`), so the window looks.
    pub(crate) fn sync_dialogs(&mut self, ui: &Ui<Msg>, waiting: bool) {
        match (waiting, self.dialog_timer) {
            (true, None) => {
                let id = ui.set_timer(DIALOG_POLL_MS);
                self.routes.borrow_mut().insert(id.0, Route::Dialogs);
                self.dialog_timer = Some(id);
            }
            (false, Some(id)) => {
                ui.kill_timer(id);
                self.routes.borrow_mut().remove(&id.0);
                self.dialog_timer = None;
            }
            _ => {}
        }
    }

    /// Lets every source run what is ready in `instance`'s script.
    pub(crate) fn poll(&self, instance: &FormInstance) -> Vec<ScriptError> {
        self.poller.poll(instance)
    }

    /// Starts or stops the event-source poller's timer.
    fn sync_poll(&mut self, ui: &Ui<Msg>) {
        match (self.poller.active(), self.poll_timer) {
            (true, None) => {
                let interval = self.poller.interval();
                let id = ui.set_timer(interval);
                self.routes.borrow_mut().insert(id.0, Route::Poll);
                self.poll_timer = Some(id);
            }
            (false, Some(id)) => {
                ui.kill_timer(id);
                self.routes.borrow_mut().remove(&id.0);
                self.poll_timer = None;
            }
            _ => {}
        }
    }

    /// Starts, stops or restarts each `Timer` control to match its `enabled`
    /// and `interval` properties.
    fn sync_controls(&mut self, ui: &Ui<Msg>, form: &LiveForm<Msg>) {
        let names: Vec<String> = form
            .names()
            .filter(|name| form.kind(name) == Some("Timer"))
            .map(str::to_owned)
            .collect();
        // A control that was removed or renamed no longer has a timer.
        let live: BTreeSet<&str> = names.iter().map(String::as_str).collect();
        let stale: Vec<String> = self
            .controls
            .keys()
            .filter(|name| !live.contains(name.as_str()))
            .cloned()
            .collect();
        for name in stale {
            self.stop(ui, &name);
        }
        for name in names {
            let enabled = form
                .get(&name, "enabled")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let interval = interval_of(form, &name);
            match (
                enabled,
                self.controls.get(&name).map(|running| running.interval),
            ) {
                (false, Some(_)) => self.stop(ui, &name),
                (true, None) => self.start(ui, &name, interval),
                (true, Some(current)) if current != interval => {
                    self.stop(ui, &name);
                    self.start(ui, &name, interval);
                }
                _ => {}
            }
        }
    }

    /// Starts `name`'s timer at `interval`.
    fn start(&mut self, ui: &Ui<Msg>, name: &str, interval: u32) {
        let id = ui.set_timer(interval);
        self.routes
            .borrow_mut()
            .insert(id.0, Route::Tick(name.to_owned()));
        self.controls
            .insert(name.to_owned(), Running { id, interval });
    }

    /// Stops `name`'s timer, if it is running.
    fn stop(&mut self, ui: &Ui<Msg>, name: &str) {
        if let Some(running) = self.controls.remove(name) {
            ui.kill_timer(running.id);
            self.routes.borrow_mut().remove(&running.id.0);
        }
    }
}

/// A `Timer` control's interval in milliseconds, clamped to at least one.
fn interval_of(form: &LiveForm<Msg>, name: &str) -> u32 {
    form.get(name, "interval")
        .and_then(|value| match value {
            Value::Int(millis) => Some(millis),
            _ => None,
        })
        .map_or(100, clamp_interval)
}

/// `millis` as a window timer interval: at least 1, and saturating at
/// `u32::MAX` rather than wrapping (a plain `as u32` turns 4294967296 into 0).
fn clamp_interval(millis: i64) -> u32 {
    millis.clamp(1, i64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use xui_canvas::OffscreenBackend;
    use xui_core::app::{App, run_app};
    use xui_core::backend::{Backend, PlatformSpec};
    use xui_core::units::Dip;
    use xui_form::{
        Binder, EventHandler, EventRef, Factories, FormDoc, LiveForm, Node, build_with,
    };

    use super::*;

    #[test]
    fn an_interval_saturates_instead_of_wrapping() {
        assert_eq!(clamp_interval(-5), 1);
        assert_eq!(clamp_interval(0), 1);
        assert_eq!(clamp_interval(250), 250);
        assert_eq!(clamp_interval(4_294_967_295), u32::MAX);
        assert_eq!(
            clamp_interval(4_294_967_296),
            u32::MAX,
            "2^32 must not wrap to 0"
        );
        assert_eq!(clamp_interval(5_000_000_000), u32::MAX);
        assert_eq!(clamp_interval(i64::MAX), u32::MAX);
    }

    #[test]
    fn a_route_maps_to_its_message() {
        assert!(matches!(Route::Poll.message(), Msg::Poll));
        assert!(matches!(
            Route::Tick("timer1".to_owned()).message(),
            Msg::Tick { control } if control == "timer1"
        ));
    }

    #[test]
    fn an_idle_poller_has_no_timers() {
        let timers = Timers::idle();
        assert!(!timers.is_polling());
        assert!(timers.controls.is_empty());
    }

    /// A binder that wires nothing.
    struct Noop;

    impl<M: 'static> Binder<M> for Noop {
        fn bind(&self, _event: EventRef<'_>) -> Option<EventHandler<M>> {
            None
        }
    }

    /// An app that quits at once; the test's assertions run before it.
    struct Quit;

    impl App for Quit {
        type Msg = Msg;

        fn update(&mut self, _msg: Msg, ui: &mut Ui<Msg>) {
            ui.quit();
        }
    }

    /// Builds a live form with the two `Timer` controls the tests use.
    fn live_form(ui: &Ui<Msg>, timers: &mut Timers) -> Rc<LiveForm<Msg>> {
        let mut doc = FormDoc::new("main_form");
        let mut timer1 = Node::new("Timer", "timer1");
        timer1.set_prop("enabled", Value::Bool(true));
        timer1.set_prop("interval", Value::Int(16));
        doc.insert(timer1);
        doc.insert(Node::new("Timer", "timer2"));
        let catalog = lazyrad_project::lazyrad_catalog();
        let factories: Factories<Msg> = Factories::xui();
        let form = Rc::new(
            build_with(ui, &doc, &catalog, &factories, &Noop, Default::default())
                .expect("the form builds"),
        );
        timers.sync(ui, &form);
        form
    }

    fn run(mut check: impl FnMut(&Ui<Msg>, &mut Timers)) {
        let backend = Rc::new(OffscreenBackend::new());
        let spec = PlatformSpec::new("timers").size(Dip(320.0), Dip(200.0));
        run_app(backend as Rc<dyn Backend>, spec, move |ui| {
            let mut timers = Timers::new(ui, "main_form");
            check(ui, &mut timers);
            ui.quit();
            Quit
        })
        .expect("the event loop runs");
    }

    #[test]
    fn sync_starts_stops_and_restarts_a_control_timer() {
        run(|ui, timers| {
            let form = live_form(ui, timers);
            assert!(timers.is_control_running("timer1"), "enabled starts it");
            assert!(
                !timers.is_control_running("timer2"),
                "a disabled timer stays stopped"
            );

            form.set("timer1", "enabled", &Value::Bool(false))
                .expect("disable");
            timers.sync(ui, &form);
            assert!(!timers.is_control_running("timer1"), "disabling stops it");

            form.set("timer1", "enabled", &Value::Bool(true))
                .expect("enable");
            timers.sync(ui, &form);
            assert!(timers.is_control_running("timer1"), "enabling restarts it");
        });
    }

    #[test]
    fn sync_drops_the_timer_of_a_removed_control() {
        run(|ui, timers| {
            let form = live_form(ui, timers);
            assert!(timers.is_control_running("timer1"));

            // A form without `timer1`: the control was removed or renamed.
            let catalog = lazyrad_project::lazyrad_catalog();
            let factories: Factories<Msg> = Factories::xui();
            let mut doc = FormDoc::new("main_form");
            doc.insert(Node::new("Timer", "timer2"));
            let replacement = build_with(ui, &doc, &catalog, &factories, &Noop, Default::default())
                .expect("the replacement builds");
            timers.sync(ui, &replacement);
            assert!(
                !timers.is_control_running("timer1"),
                "a stale timer is dropped"
            );
            let _ = form;
        });
    }
}
