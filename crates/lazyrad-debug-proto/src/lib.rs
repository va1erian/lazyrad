#![forbid(unsafe_code)]

//! The debug protocol between the IDE and a `lazyrad-player --debug` process
//! (PLAN.md §7).
//!
//! The transport is plain text: **one JSON object per line**. The IDE writes
//! [`Command`]s to the player's stdin and reads [`Event`]s from its stdout,
//! where they are interleaved with whatever the program prints (a stdout line
//! that is not a protocol message is ordinary program output).
//!
//! ```text
//! IDE ─ {"v":1,"cmd":"set_breakpoints","file":"main_form.rhai","lines":[12,30]}
//!     ─ {"v":1,"cmd":"continue"}
//! player ─ {"v":1,"event":"stopped","reason":"breakpoint","file":"main_form.rhai","line":12,…}
//! ```
//!
//! # Conventions
//!
//! * Every message carries `"v"`, the [`PROTOCOL_VERSION`]. A message with
//!   another version is refused with [`DecodeError::Version`], so a skewed IDE
//!   and player notice instead of misreading each other.
//! * **Lines are one-based** everywhere in the protocol, as Rhai positions and
//!   diagnostics are. An unknown or unavailable line is `0`.
//! * **File names are project-relative paths with `/` separators**
//!   ([`normalize_file`]), as the project's `.lrp` names them
//!   (`main_form.rhai`).
//! * A decoder never panics: malformed input is a [`DecodeError`] the caller
//!   reports and ignores, and [`LineReader`] caps the size of a line, so a
//!   hostile or broken peer cannot make the other side buffer without bound.
//!
//! The crate has no OS dependency (only `serde` and `serde_json`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version both sides write and require.
pub const PROTOCOL_VERSION: u32 = 1;

/// The longest line [`LineReader`] keeps, in bytes. A longer line is dropped
/// whole (reported once as [`Line::Oversized`]), never split, because half a
/// JSON object is not a message.
pub const MAX_LINE_BYTES: usize = 256 * 1024;

/// The longest event [`encode_event`] writes, in bytes. Kept well under the
/// IDE's own pipe line cap so a large `stopped` payload is trimmed (and flagged
/// [`Stop::truncated`]) instead of being cut mid-object by a reader.
pub const MAX_EVENT_BYTES: usize = 40 * 1024;

/// The longest `output` text one event carries, in characters.
pub const MAX_OUTPUT_CHARS: usize = 8 * 1024;

/// The command names a player understands, as they appear in `"cmd"`.
pub const COMMAND_NAMES: &[&str] = &[
    "set_breakpoints",
    "continue",
    "step_into",
    "step_over",
    "step_out",
    "pause",
    "stop",
    "evaluate",
];

/// The event names a player writes, as they appear in `"event"`.
pub const EVENT_NAMES: &[&str] = &[
    "ready",
    "stopped",
    "resumed",
    "evaluated",
    "output",
    "error",
    "exited",
];

/// Something the IDE asks the player to do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Replaces the breakpoints of one file with `lines` (one-based). An empty
    /// list clears the file. Valid at any time, stopped or running.
    SetBreakpoints {
        /// The project-relative file (`main_form.rhai`).
        file: String,
        /// The lines to stop on.
        lines: Vec<u32>,
    },
    /// Resumes until the next breakpoint, error or pause. Only valid when
    /// stopped.
    Continue,
    /// Runs one line, entering a function call. Only valid when stopped.
    StepInto,
    /// Runs one line, stepping over function calls. Only valid when stopped.
    StepOver,
    /// Runs until the current function returns. Only valid when stopped.
    StepOut,
    /// Stops at the next line executed. When nothing is running the request
    /// stays armed and takes effect on the next handler.
    Pause,
    /// Ends the program.
    Stop,
    /// Evaluates a Rhai expression or statement list in the stopped frame.
    Evaluate {
        /// The source to evaluate.
        expr: String,
        /// The frame to evaluate in, `0` being the innermost. Only frame `0`
        /// is supported (see the player's documentation).
        #[serde(default)]
        frame: u32,
        /// An id echoed in the matching [`Event::Evaluated`].
        #[serde(default)]
        id: u64,
    },
}

/// Why the program stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// A breakpoint line was reached.
    Breakpoint,
    /// A step command finished.
    Step,
    /// A pause was requested.
    Pause,
    /// A script error: the program is stopped at the failing position and will
    /// report the error when resumed.
    Exception,
}

/// The two output streams an [`Event::Output`] can carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    /// Text the program itself printed (`debug(...)`).
    Program,
    /// A note from the debugger.
    Debugger,
}

/// One entry of the call stack, innermost first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    /// The function's name (`go_button_click`), or `<top level>`.
    pub function: String,
    /// The file the frame is executing in, `""` when unknown.
    pub file: String,
    /// The one-based line, `0` when unknown.
    pub line: u32,
}

/// One variable in the stopped frame's scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variable {
    /// The variable's name.
    pub name: String,
    /// The script-level type name (`int`, `string`, `array`).
    #[serde(rename = "type")]
    pub ty: String,
    /// The value, rendered and truncated for display.
    pub value: String,
}

/// Everything a `stopped` event reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stop {
    /// Why the program stopped.
    pub reason: StopReason,
    /// The file stopped in (`""` when unknown).
    pub file: String,
    /// The one-based line (`0` when unknown).
    pub line: u32,
    /// For [`StopReason::Exception`], the error message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The call stack, innermost first.
    #[serde(default)]
    pub stack: Vec<Frame>,
    /// The locals of the innermost frame.
    #[serde(default)]
    pub locals: Vec<Variable>,
    /// Whether `stack` or `locals` were cut to fit [`MAX_EVENT_BYTES`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// Something the player reports to the IDE.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The debugger is attached and listening for commands.
    Ready {
        /// A human-readable player identification.
        player: String,
    },
    /// The program stopped; it stays stopped until a resume command.
    Stopped(Stop),
    /// The program is running again.
    Resumed,
    /// The answer to an [`Command::Evaluate`].
    Evaluated {
        /// The request's id.
        id: u64,
        /// Whether the evaluation succeeded; on failure `value` is the error.
        ok: bool,
        /// The rendered result, or the error message.
        value: String,
        /// The result's type name, `""` on failure.
        #[serde(rename = "type", default)]
        ty: String,
    },
    /// Text for the Output pane.
    Output {
        /// The text, without a trailing newline.
        text: String,
        /// Where it came from.
        stream: Stream,
    },
    /// A problem the debugger itself wants to report (a refused command, a
    /// malformed line, a script error that is not a stop).
    Error {
        /// The description.
        message: String,
    },
    /// The program ended.
    Exited {
        /// The exit code.
        code: i32,
    },
}

/// Why a line could not be decoded. Never fatal: callers report it and carry on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The line was empty or only whitespace.
    Empty,
    /// The line exceeded [`MAX_LINE_BYTES`].
    Oversized,
    /// The line is not one JSON object.
    NotJson(String),
    /// The `"v"` field is missing or not [`PROTOCOL_VERSION`].
    Version {
        /// The version the peer wrote, if it wrote one.
        found: Option<u64>,
    },
    /// The command or event name is not one this version knows.
    Unknown(String),
    /// The name is known but its fields are wrong.
    Invalid(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Empty => formatter.write_str("empty line"),
            DecodeError::Oversized => formatter.write_str("line too long"),
            DecodeError::NotJson(why) => write!(formatter, "not a JSON object: {why}"),
            DecodeError::Version { found: Some(found) } => write!(
                formatter,
                "protocol version {found} (this side speaks {PROTOCOL_VERSION})"
            ),
            DecodeError::Version { found: None } => write!(
                formatter,
                "no protocol version (this side speaks {PROTOCOL_VERSION})"
            ),
            DecodeError::Unknown(name) => write!(formatter, "unknown message `{name}`"),
            DecodeError::Invalid(why) => write!(formatter, "malformed message: {why}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// `file` with `\` turned into `/` and a leading `./` removed, the one spelling
/// both sides compare.
pub fn normalize_file(file: &str) -> String {
    let unified = file.replace('\\', "/");
    let mut trimmed = unified.as_str();
    while let Some(rest) = trimmed.strip_prefix("./") {
        trimmed = rest;
    }
    trimmed.to_owned()
}

/// Encodes `command` as one line, without the newline.
pub fn encode_command(command: &Command) -> String {
    with_version(serde_json::to_value(command))
}

/// Encodes `event` as one line, without the newline, trimmed to
/// [`MAX_EVENT_BYTES`].
///
/// An `output` text is cut to [`MAX_OUTPUT_CHARS`] characters; a `stopped`
/// event drops locals and then frames from the end until it fits and sets
/// [`Stop::truncated`]. Everything else is small by construction.
pub fn encode_event(event: &Event) -> String {
    match event {
        Event::Output { text, stream } if text.chars().count() > MAX_OUTPUT_CHARS => {
            let cut: String = text.chars().take(MAX_OUTPUT_CHARS).collect();
            encode_event(&Event::Output {
                text: cut,
                stream: *stream,
            })
        }
        Event::Stopped(stop) => {
            let mut stop = stop.clone();
            loop {
                let line = with_version(serde_json::to_value(Event::Stopped(stop.clone())));
                if line.len() <= MAX_EVENT_BYTES {
                    return line;
                }
                stop.truncated = true;
                // Halve what is left of the bigger list; a payload with
                // neither (a huge message) has its message cut instead.
                if stop.locals.len() >= stop.stack.len() && !stop.locals.is_empty() {
                    let keep = stop.locals.len() / 2;
                    stop.locals.truncate(keep);
                } else if !stop.stack.is_empty() {
                    let keep = stop.stack.len() / 2;
                    stop.stack.truncate(keep);
                } else if let Some(message) = stop.message.as_mut() {
                    let keep = message.chars().count() / 2;
                    *message = message.chars().take(keep).collect();
                } else {
                    // Nothing left to cut; names and values are bounded by
                    // the producer, so this only guards a hostile caller.
                    stop.file.clear();
                }
            }
        }
        other => with_version(serde_json::to_value(other)),
    }
}

/// Adds `"v"` to a serialised message and renders it.
fn with_version(value: serde_json::Result<Value>) -> String {
    let mut value = value.unwrap_or(Value::Null);
    if let Value::Object(map) = &mut value {
        map.insert("v".to_owned(), Value::from(PROTOCOL_VERSION));
    }
    value.to_string()
}

/// Decodes one line the IDE wrote.
pub fn decode_command(line: &str) -> Result<Command, DecodeError> {
    decode(line, "cmd", COMMAND_NAMES)
}

/// Decodes one line the player wrote.
pub fn decode_event(line: &str) -> Result<Event, DecodeError> {
    decode(line, "event", EVENT_NAMES)
}

/// The shared decoder: JSON object, version, known name, then the typed parse.
fn decode<T: for<'de> Deserialize<'de>>(
    line: &str,
    tag: &str,
    known: &[&str],
) -> Result<T, DecodeError> {
    let line = line.trim();
    if line.is_empty() {
        return Err(DecodeError::Empty);
    }
    if line.len() > MAX_LINE_BYTES {
        return Err(DecodeError::Oversized);
    }
    let value: Value =
        serde_json::from_str(line).map_err(|error| DecodeError::NotJson(error.to_string()))?;
    let Some(object) = value.as_object() else {
        return Err(DecodeError::NotJson("not an object".to_owned()));
    };
    match object.get("v").and_then(Value::as_u64) {
        Some(version) if version == u64::from(PROTOCOL_VERSION) => {}
        found => return Err(DecodeError::Version { found }),
    }
    let name = object
        .get(tag)
        .and_then(Value::as_str)
        .ok_or_else(|| DecodeError::Invalid(format!("missing `{tag}`")))?;
    if !known.contains(&name) {
        return Err(DecodeError::Unknown(name.to_owned()));
    }
    serde_json::from_value(value).map_err(|error| DecodeError::Invalid(error.to_string()))
}

/// What [`LineReader`] hands back for one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// A complete line, without its newline or a trailing carriage return.
    /// Invalid UTF-8 has been replaced, so it may fail to decode but is never
    /// an error here.
    Text(String),
    /// A line longer than [`MAX_LINE_BYTES`] was dropped.
    Oversized,
}

/// Splits a byte stream read in arbitrary chunks into lines.
///
/// A partial line waits for its newline across calls to [`LineReader::push`].
/// An oversized line is discarded to its newline and reported once, so the
/// reader recovers with the next line.
#[derive(Debug, Default)]
pub struct LineReader {
    pending: Vec<u8>,
    discarding: bool,
}

impl LineReader {
    /// An empty reader.
    pub fn new() -> LineReader {
        LineReader::default()
    }

    /// Adds `chunk` and returns the lines it completed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Line> {
        let mut lines = Vec::new();
        for &byte in chunk {
            if byte == b'\n' {
                if self.discarding {
                    self.discarding = false;
                    self.pending.clear();
                } else {
                    lines.push(self.take());
                }
            } else if self.discarding {
                // Dropping the rest of an oversized line.
            } else {
                self.pending.push(byte);
                if self.pending.len() > MAX_LINE_BYTES {
                    self.pending.clear();
                    self.discarding = true;
                    lines.push(Line::Oversized);
                }
            }
        }
        lines
    }

    /// The held partial line at end of stream, if any.
    pub fn finish(&mut self) -> Option<Line> {
        if self.discarding {
            self.discarding = false;
            self.pending.clear();
            return None;
        }
        (!self.pending.is_empty()).then(|| self.take())
    }

    fn take(&mut self) -> Line {
        let mut bytes = std::mem::take(&mut self.pending);
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        Line::Text(String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_commands() -> Vec<Command> {
        vec![
            Command::SetBreakpoints {
                file: "main_form.rhai".to_owned(),
                lines: vec![3, 12, 30],
            },
            Command::SetBreakpoints {
                file: "util.rhai".to_owned(),
                lines: Vec::new(),
            },
            Command::Continue,
            Command::StepInto,
            Command::StepOver,
            Command::StepOut,
            Command::Pause,
            Command::Stop,
            Command::Evaluate {
                expr: "name_edit.text + \"\\n\"".to_owned(),
                frame: 0,
                id: 7,
            },
        ]
    }

    fn stop() -> Stop {
        Stop {
            reason: StopReason::Breakpoint,
            file: "main_form.rhai".to_owned(),
            line: 12,
            message: None,
            stack: vec![
                Frame {
                    function: "go_button_click".to_owned(),
                    file: "main_form.rhai".to_owned(),
                    line: 12,
                },
                Frame {
                    function: "<top level>".to_owned(),
                    file: String::new(),
                    line: 0,
                },
            ],
            locals: vec![Variable {
                name: "count".to_owned(),
                ty: "int".to_owned(),
                value: "3".to_owned(),
            }],
            truncated: false,
        }
    }

    #[test]
    fn every_command_round_trips_on_one_line() {
        for command in all_commands() {
            let line = encode_command(&command);
            assert!(!line.contains('\n'), "one line: {line}");
            assert!(line.contains(r#""v":1"#), "{line}");
            assert_eq!(decode_command(&line), Ok(command));
        }
    }

    #[test]
    fn every_event_round_trips() {
        let events = vec![
            Event::Ready {
                player: "lazyrad-player 0.1.0".to_owned(),
            },
            Event::Stopped(stop()),
            Event::Stopped(Stop {
                reason: StopReason::Exception,
                message: Some("Division by zero".to_owned()),
                truncated: true,
                ..stop()
            }),
            Event::Resumed,
            Event::Evaluated {
                id: 3,
                ok: true,
                value: "\"hi\"".to_owned(),
                ty: "string".to_owned(),
            },
            Event::Evaluated {
                id: 4,
                ok: false,
                value: "Variable not found: nope".to_owned(),
                ty: String::new(),
            },
            Event::Output {
                text: "line with \"quotes\" and \u{1F600}".to_owned(),
                stream: Stream::Program,
            },
            Event::Error {
                message: "unknown message `bogus`".to_owned(),
            },
            Event::Exited { code: 2 },
        ];
        for event in events {
            let line = encode_event(&event);
            assert!(!line.contains('\n'), "one line: {line}");
            assert_eq!(decode_event(&line), Ok(event));
        }
    }

    #[test]
    fn the_wire_names_are_the_documented_ones() {
        assert_eq!(
            encode_command(&Command::StepOver),
            r#"{"cmd":"step_over","v":1}"#
        );
        let line = encode_event(&Event::Stopped(stop()));
        assert!(line.contains(r#""event":"stopped""#), "{line}");
        assert!(line.contains(r#""reason":"breakpoint""#), "{line}");
        assert!(line.contains(r#""type":"int""#), "{line}");
        assert!(!line.contains("truncated"), "default flag is omitted");
        for command in all_commands() {
            let line = encode_command(&command);
            let value: Value = serde_json::from_str(&line).unwrap();
            let name = value["cmd"].as_str().unwrap();
            assert!(COMMAND_NAMES.contains(&name), "{name}");
        }
    }

    #[test]
    fn optional_evaluate_fields_default() {
        let command = decode_command(r#"{"v":1,"cmd":"evaluate","expr":"1+1"}"#).unwrap();
        assert_eq!(
            command,
            Command::Evaluate {
                expr: "1+1".to_owned(),
                frame: 0,
                id: 0
            }
        );
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert_eq!(decode_command(""), Err(DecodeError::Empty));
        assert_eq!(decode_command("   \t"), Err(DecodeError::Empty));
        assert!(matches!(
            decode_command("not json"),
            Err(DecodeError::NotJson(_))
        ));
        assert!(matches!(
            decode_command("[1,2]"),
            Err(DecodeError::NotJson(_))
        ));
        assert!(matches!(
            decode_command(r#"{"v":1}"#),
            Err(DecodeError::Invalid(_))
        ));
        assert!(matches!(
            decode_command(r#"{"v":1,"cmd":"set_breakpoints","file":3}"#),
            Err(DecodeError::Invalid(_))
        ));
        assert!(matches!(
            decode_command(r#"{"v":1,"cmd":"set_breakpoints","file":"a","lines":[-1]}"#),
            Err(DecodeError::Invalid(_))
        ));
        assert!(matches!(
            decode_command(r#"{"v":1,"cmd":"set_breakpoints","file":"a","lines":"x"}"#),
            Err(DecodeError::Invalid(_))
        ));
        let huge = format!(r#"{{"v":1,"cmd":"{}"}}"#, "x".repeat(MAX_LINE_BYTES));
        assert_eq!(decode_command(&huge), Err(DecodeError::Oversized));
    }

    #[test]
    fn an_unknown_command_is_named_so_it_can_be_reported() {
        assert_eq!(
            decode_command(r#"{"v":1,"cmd":"step_sideways"}"#),
            Err(DecodeError::Unknown("step_sideways".to_owned()))
        );
        // A name valid for the other direction is unknown here.
        assert_eq!(
            decode_command(r#"{"v":1,"event":"resumed"}"#),
            Err(DecodeError::Invalid("missing `cmd`".to_owned()))
        );
        assert_eq!(
            decode_event(r#"{"v":1,"event":"teleported"}"#),
            Err(DecodeError::Unknown("teleported".to_owned()))
        );
    }

    #[test]
    fn a_version_mismatch_is_detected() {
        assert_eq!(
            decode_command(r#"{"v":2,"cmd":"continue"}"#),
            Err(DecodeError::Version { found: Some(2) })
        );
        assert_eq!(
            decode_event(r#"{"cmd":"continue"}"#),
            Err(DecodeError::Version { found: None })
        );
        assert_eq!(
            decode_event(r#"{"v":"1","event":"resumed"}"#),
            Err(DecodeError::Version { found: None }),
            "a version that is not a number is no version"
        );
        let shown = DecodeError::Version { found: Some(9) }.to_string();
        assert!(shown.contains('9') && shown.contains('1'), "{shown}");
    }

    #[test]
    fn file_names_have_one_spelling() {
        assert_eq!(normalize_file("main_form.rhai"), "main_form.rhai");
        assert_eq!(normalize_file(".\\forms\\main.rhai"), "forms/main.rhai");
        assert_eq!(normalize_file("./././a.rhai"), "a.rhai");
    }

    #[test]
    fn a_reader_joins_partial_chunks() {
        let mut reader = LineReader::new();
        let wire = format!("{}\n{}\n", encode_command(&Command::Pause), "second");
        let mut seen = Vec::new();
        for byte in wire.as_bytes() {
            seen.extend(reader.push(std::slice::from_ref(byte)));
        }
        assert_eq!(
            seen,
            [
                Line::Text(encode_command(&Command::Pause)),
                Line::Text("second".to_owned())
            ]
        );
        assert_eq!(reader.finish(), None);
    }

    #[test]
    fn a_reader_handles_crlf_blank_lines_and_a_final_partial_line() {
        let mut reader = LineReader::new();
        assert_eq!(
            reader.push(b"a\r\n\r\nb"),
            [Line::Text("a".to_owned()), Line::Text(String::new())]
        );
        assert_eq!(reader.finish(), Some(Line::Text("b".to_owned())));
        assert_eq!(reader.finish(), None);
    }

    #[test]
    fn a_reader_replaces_invalid_utf8() {
        let mut reader = LineReader::new();
        let lines = reader.push(&[b'{', 0xff, 0xfe, b'}', b'\n']);
        assert_eq!(lines.len(), 1);
        let Line::Text(text) = &lines[0] else {
            panic!("a text line");
        };
        assert!(text.contains('\u{fffd}'));
        assert!(matches!(
            decode_command(text),
            Err(DecodeError::NotJson(_))
        ));
    }

    #[test]
    fn an_oversized_line_is_dropped_whole_and_the_reader_recovers() {
        let mut reader = LineReader::new();
        let mut wire = vec![b'a'; MAX_LINE_BYTES + 10];
        wire.extend_from_slice(b"\nnext\n");
        // Delivered in awkward chunks, including one that ends the long line.
        let mut seen = Vec::new();
        for chunk in wire.chunks(MAX_LINE_BYTES / 3 + 1) {
            seen.extend(reader.push(chunk));
        }
        assert_eq!(seen, [Line::Oversized, Line::Text("next".to_owned())]);

        // An endless line with no newline never accumulates.
        let mut reader = LineReader::new();
        for _ in 0..10 {
            reader.push(&vec![b'x'; MAX_LINE_BYTES]);
        }
        assert!(reader.pending.len() <= MAX_LINE_BYTES);
        assert_eq!(reader.finish(), None, "an oversized tail is discarded");
    }

    #[test]
    fn a_large_stop_is_trimmed_to_fit_and_flagged() {
        let locals = (0..2000)
            .map(|index| Variable {
                name: format!("variable_{index}"),
                ty: "string".to_owned(),
                value: "v".repeat(100),
            })
            .collect();
        let big = Event::Stopped(Stop {
            locals,
            ..stop()
        });
        let line = encode_event(&big);
        assert!(line.len() <= MAX_EVENT_BYTES, "{} bytes", line.len());
        let Ok(Event::Stopped(trimmed)) = decode_event(&line) else {
            panic!("still a stopped event");
        };
        assert!(trimmed.truncated);
        assert!(!trimmed.locals.is_empty() && trimmed.locals.len() < 2000);
        assert_eq!(trimmed.stack.len(), 2, "the stack is kept when locals fit");
        assert_eq!(trimmed.line, 12);

        let message = Event::Stopped(Stop {
            message: Some("e".repeat(MAX_EVENT_BYTES * 2)),
            stack: Vec::new(),
            locals: Vec::new(),
            reason: StopReason::Exception,
            ..stop()
        });
        assert!(encode_event(&message).len() <= MAX_EVENT_BYTES);
    }

    #[test]
    fn long_output_is_cut_on_a_character_boundary() {
        let text = "é".repeat(MAX_OUTPUT_CHARS + 50);
        let line = encode_event(&Event::Output {
            text,
            stream: Stream::Program,
        });
        let Ok(Event::Output { text, .. }) = decode_event(&line) else {
            panic!("still output");
        };
        assert_eq!(text.chars().count(), MAX_OUTPUT_CHARS);
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        // A deterministic xorshift stands in for a fuzzer.
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut reader = LineReader::new();
        for _ in 0..200 {
            let length = (next() % 300) as usize;
            let bytes: Vec<u8> = (0..length).map(|_| next() as u8).collect();
            for line in reader.push(&bytes) {
                if let Line::Text(text) = line {
                    let _ = decode_command(&text);
                    let _ = decode_event(&text);
                }
            }
        }
        // Valid-looking prefixes with garbage tails.
        for tail in ["", "{", "}", "\"", "\\u", "{\"cmd\":", "null", "1e999"] {
            let line = format!(r#"{{"v":1,"cmd":"continue"{tail}"#);
            let _ = decode_command(&line);
        }
    }
}
