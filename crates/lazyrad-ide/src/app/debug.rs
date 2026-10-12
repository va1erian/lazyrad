#![forbid(unsafe_code)]

//! The debugger's half of the IDE window: breakpoints in the code editors, the
//! stop marker, the Locals / Call Stack / Immediate panes and the commands that
//! drive a debug run (PLAN.md §7).
//!
//! The transport is the run state's child: commands go out through
//! [`ChildProcess::send_line`](crate::run::ChildProcess::send_line) and events
//! come back as [`RunEvent::Debug`](crate::run::RunEvent::Debug) through the
//! same sink as program output. Nothing here blocks or needs a thread.
//!
//! # Coordinate conventions
//!
//! Lines are **one-based** in the breakpoint store, the debug protocol and
//! [`DebugView`]; they become the code editor's **zero-based** lines only when
//! markers are built ([`IdeApp::markers_for`]) or the caret is moved. Files are
//! project-relative with `/` separators in the protocol; the IDE keys
//! breakpoints by project *item name* and converts at the boundary
//! ([`IdeApp::code_file_of`], [`IdeApp::item_of_file`]).
//!
//! # What reaches the player when
//!
//! The player compiles the project from disk, so line numbers in a command are
//! those of the saved files. Starting a run saves everything first. Afterwards
//! the full breakpoint set of an item is sent when a breakpoint is toggled and
//! when the project is saved; edits move the IDE's breakpoints in the editor
//! at once but reach the player with the next save (a hot reload), which is
//! the moment the player's text matches again.

use std::collections::BTreeSet;

use lazyrad_debug_proto::{Command as DebugCommand, Event, Stop, StopReason};
use xui_code_editor::{Marker, MarkerKind};
use xui_core::app::Ui;
use xui_core::widget::HasText;

use super::frame::{TAB_IMMEDIATE, TAB_LOCALS, TAB_OUTPUT};
use super::{IdeApp, IdeEvent, Msg, StopKind};
use crate::debugging::{DebugPhase, frame_row, local_row, shift_lines};
use crate::run::{LaunchMode, SendError};

/// What the stopped program looked like at its last stop, and the Immediate
/// pane's history. Cleared whenever the run ends or resumes.
#[derive(Debug, Default)]
pub(super) struct DebugView {
    /// Where the program is stopped: the item and one-based line.
    pub(super) at: Option<(String, usize)>,
    /// Why it stopped.
    pub(super) reason: Option<StopReason>,
    /// The Locals pane's rows.
    pub(super) locals: Vec<String>,
    /// The Call Stack pane's rows: text, the project item (when the frame's
    /// file is one) and the one-based line.
    pub(super) stack: Vec<(String, Option<String>, usize)>,
    /// The Immediate pane's lines.
    pub(super) immediate: Vec<String>,
    /// What the Immediate input holds.
    pub(super) input: String,
    /// The id the next evaluation gets.
    pub(super) next_id: u64,
    /// Evaluations sent and not yet answered. An answer for an id not in here
    /// (a stale one from a run that ended) is dropped.
    pub(super) in_flight: BTreeSet<u64>,
}

impl DebugView {
    /// Forgets the stop (running again, or the run ended) but keeps the
    /// Immediate history.
    fn clear_stop(&mut self) {
        self.at = None;
        self.reason = None;
        self.locals.clear();
        self.stack.clear();
    }
}

impl IdeApp {
    // ---- Breakpoints -------------------------------------------------------

    /// The code file of project item `item`, as the protocol names it.
    pub(super) fn code_file_of(&self, item: &str) -> Option<String> {
        let session = self.session.as_ref()?;
        let entry = session.project().item(item)?;
        Some(lazyrad_debug_proto::normalize_file(
            &entry.code().to_string_lossy(),
        ))
    }

    /// The project item whose code file is `file`.
    pub(super) fn item_of_file(&self, file: &str) -> Option<String> {
        let want = lazyrad_debug_proto::normalize_file(file);
        let session = self.session.as_ref()?;
        session
            .project()
            .items
            .iter()
            .find(|item| {
                lazyrad_debug_proto::normalize_file(&item.code().to_string_lossy()) == want
            })
            .map(|item| item.name().to_owned())
    }

    /// The editor markers of `item`: diagnostics, breakpoints (a gutter dot)
    /// and the stop line (a tint; the editor has no arrow glyph, see
    /// PLAN.md §7). Lines become zero-based here.
    pub(super) fn markers_for(&self, item: &str) -> Vec<Marker> {
        let mut markers: Vec<Marker> = self
            .errors
            .iter()
            .filter(|entry| entry.name == item)
            .map(super::ErrorEntry::marker)
            .collect();
        for line in self.breakpoints.lines(item) {
            markers.push(Marker::line(line - 1, MarkerKind::Breakpoint));
        }
        if let Some((at, line)) = &self.debug.at
            && at == item
            && *line > 0
        {
            let kind = if self.debug.reason == Some(StopReason::Exception) {
                MarkerKind::Error
            } else {
                MarkerKind::Warning
            };
            markers.push(Marker::line(line - 1, kind));
        }
        markers
    }

    /// Re-applies markers to every open code editor.
    pub(super) fn refresh_all_markers(&self) {
        for document in &self.documents {
            if let super::DocumentView::Code(view) = &document.view {
                view.editor.set_markers(self.markers_for(&document.name));
            }
        }
    }

    /// Toggles a breakpoint on the caret's line of the code window in front
    /// (F9). A designer or the Start Page has no lines to stop on.
    pub(super) fn toggle_breakpoint(&mut self, ui: &mut Ui<Msg>) {
        let Some((item, editor)) = self.active_code_editor() else {
            self.log(ui, "Open a code window and put the caret on a line first.");
            return;
        };
        let (line0, _) = editor.caret_line_col();
        let line = line0 + 1;
        let set = self.breakpoints.toggle(&item, line);
        editor.set_markers(self.markers_for(&item));
        self.log(
            ui,
            format!(
                "Breakpoint {} at {item}:{line}.",
                if set { "set" } else { "cleared" }
            ),
        );
        if self.dispatcher.debug_phase() != DebugPhase::Off {
            let dirty = self
                .documents
                .iter()
                .any(|document| document.name == item && document.dirty);
            if dirty {
                self.log(
                    ui,
                    "Save the file so the running program counts lines the same way.",
                );
            }
            self.send_breakpoints_of(&item, ui);
        } else if self.run.is_running() && set {
            self.log(
                ui,
                "The program was started without the debugger: stop it and press F5 to debug.",
            );
        }
    }

    /// Tells the player `item`'s whole breakpoint set (an empty set clears
    /// them). Does nothing when no debug run is active.
    pub(super) fn send_breakpoints_of(&mut self, item: &str, ui: &mut Ui<Msg>) {
        if self.dispatcher.debug_phase() == DebugPhase::Off {
            return;
        }
        let Some(file) = self.code_file_of(item) else {
            return;
        };
        let lines: Vec<u32> = self
            .breakpoints
            .lines(item)
            .into_iter()
            .map(|line| u32::try_from(line).unwrap_or(u32::MAX))
            .collect();
        self.debug_send(DebugCommand::SetBreakpoints { file, lines }, ui);
    }

    /// Tells the player every item's breakpoints (a run just started).
    pub(super) fn send_all_breakpoints(&mut self, ui: &mut Ui<Msg>) {
        let items: Vec<String> = self.breakpoints.items().map(str::to_owned).collect();
        for item in items {
            self.send_breakpoints_of(&item, ui);
        }
    }

    /// Drops the breakpoints of a removed item and, in a debug run, tells the
    /// player its file has none now (`file` was read before the removal).
    pub(super) fn forget_breakpoints_of(
        &mut self,
        item: &str,
        file: Option<String>,
        ui: &mut Ui<Msg>,
    ) {
        let had = !self.breakpoints.lines(item).is_empty();
        self.breakpoints.remove(item);
        if self.debug.at.as_ref().is_some_and(|(at, _)| at == item) {
            self.debug.at = None;
        }
        if had
            && self.dispatcher.debug_phase() != DebugPhase::Off
            && let Some(file) = file
        {
            self.debug_send(
                DebugCommand::SetBreakpoints {
                    file,
                    lines: Vec::new(),
                },
                ui,
            );
        }
    }

    /// Re-sends the sets of items that have breakpoints, after a save made the
    /// player's files match the editors again.
    pub(super) fn resend_breakpoints_after_save(&mut self, ui: &mut Ui<Msg>) {
        if self.dispatcher.debug_phase() != DebugPhase::Off {
            self.send_all_breakpoints(ui);
        }
    }

    /// Moves breakpoints (and the stop line) of `item` to follow an edit.
    pub(super) fn follow_edit(&mut self, item: &str, old: &str, new: &str) {
        self.breakpoints.follow_edit(item, old, new);
        if let Some((at, line)) = &mut self.debug.at
            && at == item
        {
            match shift_lines(old, new, [*line]).first() {
                Some(moved) => *line = *moved,
                None => self.debug.at = None,
            }
        }
        if let Some(editor) = self.code_editor(item) {
            editor.set_markers(self.markers_for(item));
        }
    }

    // ---- Sending commands --------------------------------------------------

    /// Writes one debug command to the player. A failure is reported once in
    /// the Output pane; the program's exit notice does the cleaning up.
    pub(super) fn debug_send(&mut self, command: DebugCommand, ui: &mut Ui<Msg>) -> bool {
        let line = lazyrad_debug_proto::encode_command(&command);
        match self.run.send_line(&line) {
            Ok(()) => true,
            Err(SendError::Unsupported) => {
                self.log(
                    ui,
                    "[debugger] this platform cannot send commands to the program.",
                );
                false
            }
            Err(error) => {
                self.log(
                    ui,
                    format!("[debugger] could not reach the program: {error}"),
                );
                false
            }
        }
    }

    /// Resumes a stopped program with `command` (a continue or a step).
    pub(super) fn resume_with(&mut self, command: DebugCommand, ui: &mut Ui<Msg>) {
        if self.dispatcher.debug_phase() != DebugPhase::Stopped {
            self.log(ui, "The program is not stopped.");
            return;
        }
        if self.debug_send(command, ui) {
            // The program is running again as far as the IDE can tell; the
            // player's `resumed` event (or the next stop) confirms it.
            self.enter_phase(DebugPhase::Running, ui);
        }
    }

    /// Asks a running program to stop at the next line it executes (Break).
    pub(super) fn request_break(&mut self, ui: &mut Ui<Msg>) {
        if self.dispatcher.debug_phase() != DebugPhase::Running {
            self.log(ui, "Break needs a program running under the debugger.");
            return;
        }
        if self.debug_send(DebugCommand::Pause, ui) {
            self.log(
                ui,
                "Break requested: the program stops at the next line it runs.",
            );
        }
    }

    // ---- Phase and panes ---------------------------------------------------

    /// Moves the debug run to `phase`: clears or fills the stop state and
    /// refreshes the commands, status and panes.
    pub(super) fn enter_phase(&mut self, phase: DebugPhase, ui: &Ui<Msg>) {
        if phase != DebugPhase::Stopped {
            self.debug.clear_stop();
        }
        self.dispatcher.set_debug_phase(phase);
        self.refresh_menu();
        self.update_title(ui);
        self.update_status(ui);
        self.refresh_debug_panes();
        self.refresh_all_markers();
    }

    /// Clears the whole debug state: a run ended or was ended. The breakpoints
    /// stay (they belong to the project session); the stop, the panes and any
    /// evaluation still in flight do not.
    pub(super) fn clear_debug_state(&mut self, ui: &Ui<Msg>) {
        let had = self.debug.at.is_some()
            || self.dispatcher.debug_phase() != DebugPhase::Off
            || !self.debug.in_flight.is_empty();
        self.debug.clear_stop();
        self.debug.in_flight.clear();
        if had {
            self.dispatcher.set_debug_phase(DebugPhase::Off);
            self.refresh_debug_panes();
            self.refresh_all_markers();
            // Back to the log, where the program's last words are.
            self.bottom_tabs.select(TAB_OUTPUT);
        }
        let _ = ui;
    }

    /// Pushes the Locals, Call Stack and Immediate rows into their lists.
    pub(super) fn refresh_debug_panes(&self) {
        let locals: Vec<&str> = self.debug.locals.iter().map(String::as_str).collect();
        self.locals_list.set_items(&locals);
        let stack: Vec<&str> = self
            .debug
            .stack
            .iter()
            .map(|(row, _, _)| row.as_str())
            .collect();
        self.stack_list.set_items(&stack);
        let immediate: Vec<&str> = self.debug.immediate.iter().map(String::as_str).collect();
        self.immediate_list.set_items(&immediate);
        if let Some(last) = immediate.len().checked_sub(1) {
            self.immediate_list.ensure_visible(last);
        }
    }

    // ---- Events from the player -------------------------------------------

    /// Handles one debug event of the active run.
    pub(super) fn on_debug_event(&mut self, event: Event, ui: &mut Ui<Msg>) {
        match event {
            Event::Ready { player } => {
                self.log(ui, format!("Debugger attached ({player})."));
                self.send_all_breakpoints(ui);
            }
            Event::Stopped(stop) => self.on_stopped(stop, ui),
            Event::Resumed => {
                if self.dispatcher.debug_phase() == DebugPhase::Stopped {
                    self.enter_phase(DebugPhase::Running, ui);
                }
            }
            Event::Evaluated { id, ok, value, ty } => {
                if self.debug.in_flight.remove(&id) {
                    let line = match (ok, ty.is_empty()) {
                        (true, true) => format!("= {value}"),
                        (true, false) => format!("= {value}   ({ty})"),
                        (false, _) => format!("! {value}"),
                    };
                    self.debug.immediate.push(line);
                    self.refresh_debug_panes();
                }
            }
            Event::Output { text, .. } => self.log(ui, text),
            Event::Error { message } => self.log(ui, format!("[debugger] {message}")),
            // The process's own exit notice ends the run.
            Event::Exited { .. } => {}
        }
    }

    /// The program stopped: show where, with its locals and call stack.
    fn on_stopped(&mut self, stop: Stop, ui: &mut Ui<Msg>) {
        let item = self.item_of_file(&stop.file);
        let line = usize::try_from(stop.line).unwrap_or(0);
        self.debug.reason = Some(stop.reason);
        self.debug.at = item.clone().map(|item| (item, line));
        self.debug.locals = stop.locals.iter().map(local_row).collect();
        self.debug.stack = stop
            .stack
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                (
                    frame_row(index, frame),
                    self.item_of_file(&frame.file),
                    usize::try_from(frame.line).unwrap_or(0),
                )
            })
            .collect();
        if stop.truncated {
            self.debug.locals.push("(more not shown)".to_owned());
        }

        let place = format!("{}:{}", stop.file, stop.line);
        match (stop.reason, stop.message.as_deref()) {
            (StopReason::Exception, Some(message)) => {
                self.log(ui, format!("Stopped on an error at {place}: {message}"));
            }
            (StopReason::Breakpoint, _) => self.log(ui, format!("Breakpoint at {place}.")),
            (StopReason::Pause, _) => self.log(ui, format!("Break at {place}.")),
            _ => self.log(ui, format!("Stopped at {place}.")),
        }

        // Show the code first, so the markers land on an open editor.
        if let Some(item) = &item {
            self.open_code(item, ui);
        }
        self.enter_phase(DebugPhase::Stopped, ui);
        if let Some(item) = &item
            && let Some(editor) = self.code_editor(item)
        {
            editor.goto(line.saturating_sub(1), 0);
        }
        self.bottom_tabs.select(TAB_LOCALS);
        self.notify(&IdeEvent::DebugStopped {
            kind: match stop.reason {
                StopReason::Breakpoint => StopKind::Breakpoint,
                StopReason::Step => StopKind::Step,
                StopReason::Pause => StopKind::Break,
                StopReason::Exception => StopKind::Error,
            },
            item: item.unwrap_or_default(),
            line,
            locals: stop.locals.len(),
        });
    }

    /// A Call Stack row was activated: show that frame's code.
    pub(super) fn stack_activated(&mut self, row: usize, ui: &mut Ui<Msg>) {
        let Some((_, Some(item), line)) = self.debug.stack.get(row).cloned() else {
            return;
        };
        if line == 0 {
            return;
        }
        self.open_code(&item, ui);
        if let Some(editor) = self.code_editor(&item) {
            editor.goto(line.saturating_sub(1), 0);
        }
    }

    // ---- Immediate pane ----------------------------------------------------

    /// Evaluates the Immediate input in the stopped frame.
    pub(super) fn immediate_evaluate(&mut self, ui: &mut Ui<Msg>) {
        let expression = self.debug.input.trim().to_owned();
        if expression.is_empty() {
            return;
        }
        self.bottom_tabs.select(TAB_IMMEDIATE);
        self.debug.immediate.push(format!("> {expression}"));
        if self.dispatcher.debug_phase() != DebugPhase::Stopped {
            self.debug.immediate.push(
                "(not available while the program runs: break or hit a breakpoint first)"
                    .to_owned(),
            );
            self.refresh_debug_panes();
            return;
        }
        let id = self.debug.next_id;
        self.debug.next_id += 1;
        let sent = self.debug_send(
            DebugCommand::Evaluate {
                expr: expression,
                frame: 0,
                id,
            },
            ui,
        );
        if sent {
            self.debug.in_flight.insert(id);
            self.debug.input.clear();
            self.immediate_edit.set_text("");
        } else {
            self.debug
                .immediate
                .push("(the program could not be reached)".to_owned());
        }
        self.refresh_debug_panes();
    }

    /// The launch mode a plain Start uses: the debugger when breakpoints
    /// exist, otherwise a normal run with hot reload.
    pub(super) fn start_mode(&self) -> LaunchMode {
        if self.breakpoints.is_empty() {
            LaunchMode::Run
        } else {
            LaunchMode::Debug
        }
    }
}
