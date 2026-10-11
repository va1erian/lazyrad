#![forbid(unsafe_code)]

//! `lazyrad-player --debug`: the player's end of the debug protocol over its
//! standard streams (PLAN.md §7).
//!
//! [`lazyrad_runtime::debug`] holds the session logic; this module is the
//! transport. Commands arrive on **stdin** and events leave on **stdout**, one
//! JSON object per line ([`lazyrad_debug_proto`]); everything the program
//! prints shares stdout as plain lines.
//!
//! # Reading without blocking the program
//!
//! While the script runs, the session must notice a `pause` or a new set of
//! breakpoints without waiting for input, and while it is stopped it must wait.
//! [`ReaderTransport`] gets both from one **reader thread** that turns stdin
//! into complete commands on a channel: the UI thread then polls the channel
//! (running) or blocks on it (stopped).
//!
//! The thread is a convenience, not a requirement. A platform on which a
//! thread cannot use the process's stdin (LazyOS threads do not share a
//! descriptor table, so such a read fails at once) falls back to reading on the
//! calling thread: stopped programs still wait for commands, and the commands
//! that would have arrived mid-run are read at the next stop or handler start
//! instead. A host with a better idea (a non-blocking descriptor polled from
//! the window timer) installs it with [`install_transport`] before the player
//! starts.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use lazyrad_debug_proto::{Event, Line, LineReader, encode_event};
use lazyrad_runtime::debug::decode_incoming;
use lazyrad_runtime::{DebugTransport, Incoming};

/// A function that builds the transport once the session is created.
type TransportFactory = Box<dyn FnOnce() -> Rc<dyn DebugTransport>>;

thread_local! {
    /// The host's replacement for the stdio transport, taken by the player.
    static INSTALLED: RefCell<Option<TransportFactory>> = const { RefCell::new(None) };
}

/// Replaces the transport the next `--debug` run uses (a host whose stdin
/// cannot be read by a thread installs its own). Must be called on the thread
/// that runs the player.
pub fn install_transport(factory: impl FnOnce() -> Rc<dyn DebugTransport> + 'static) {
    INSTALLED.with(|slot| *slot.borrow_mut() = Some(Box::new(factory)));
}

/// The installed transport, or the stdio one.
pub(crate) fn transport() -> Rc<dyn DebugTransport> {
    let installed = INSTALLED.with(|slot| slot.borrow_mut().take());
    match installed {
        Some(factory) => factory(),
        None => Rc::new(ReaderTransport::stdio()),
    }
}

/// What the reader thread sends.
enum Item {
    Incoming(Incoming),
    /// The read failed (not end of stream): the thread cannot be used.
    Failed,
}

/// Commands read from a stream, events written to another.
pub struct ReaderTransport {
    /// The channel from the reader thread, until it fails or never started.
    channel: RefCell<Option<Receiver<Item>>>,
    /// Reading on the calling thread, once the channel is gone.
    direct: RefCell<Direct>,
    make_reader: Box<dyn Fn() -> Box<dyn Read>>,
    out: RefCell<Box<dyn Write>>,
}

/// The calling-thread reader.
#[derive(Default)]
struct Direct {
    reader: Option<Box<dyn Read>>,
    lines: LineReader,
    ready: VecDeque<Incoming>,
    closed: bool,
}

impl ReaderTransport {
    /// The transport over this process's stdin and stdout.
    pub fn stdio() -> ReaderTransport {
        ReaderTransport::new(
            || Box::new(std::io::stdin()),
            Box::new(std::io::stdout()),
            true,
        )
    }

    /// A transport reading what `make_reader` yields and writing to `out`.
    ///
    /// `make_reader` runs on the reader thread when `threaded` is set, and
    /// again on the calling thread if that thread's read fails.
    pub fn new(
        make_reader: impl Fn() -> Box<dyn Read> + Send + Sync + Clone + 'static,
        out: Box<dyn Write>,
        threaded: bool,
    ) -> ReaderTransport {
        let channel = threaded
            .then(|| spawn_reader(make_reader.clone()))
            .flatten();
        ReaderTransport {
            channel: RefCell::new(channel),
            direct: RefCell::new(Direct::default()),
            make_reader: Box::new(make_reader),
            out: RefCell::new(out),
        }
    }

    /// Whether commands are being read on a thread (`false` after a fallback).
    pub fn is_threaded(&self) -> bool {
        self.channel.borrow().is_some()
    }

    /// Reads on the calling thread until a command is ready or the stream ends.
    fn direct_wait(&self) -> Incoming {
        let mut direct = self.direct.borrow_mut();
        let direct = &mut *direct;
        loop {
            if let Some(item) = direct.ready.pop_front() {
                return item;
            }
            if direct.closed {
                return Incoming::Closed;
            }
            let reader = direct.reader.get_or_insert_with(|| (self.make_reader)());
            let mut chunk = [0u8; 4096];
            match reader.read(&mut chunk) {
                Ok(0) => {
                    direct.closed = true;
                    if let Some(line) = direct.lines.finish() {
                        direct.ready.extend(line_to_incoming(line));
                    }
                }
                Ok(read) => {
                    for line in direct.lines.push(&chunk[..read]) {
                        direct.ready.extend(line_to_incoming(line));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => direct.closed = true,
            }
        }
    }
}

impl DebugTransport for ReaderTransport {
    fn send(&self, event: &Event) {
        let line = encode_event(event);
        let mut out = self.out.borrow_mut();
        // A closed pipe is the IDE going away: the session notices on its next
        // read, so a failed write is dropped here.
        let _ = writeln!(out, "{line}").and_then(|()| out.flush());
    }

    fn poll(&self) -> Incoming {
        let outcome = {
            let channel = self.channel.borrow();
            let Some(receiver) = channel.as_ref() else {
                // No thread: nothing can be read without blocking.
                return Incoming::Empty;
            };
            receiver.try_recv()
        };
        match outcome {
            Ok(Item::Incoming(item)) => item,
            Ok(Item::Failed) => {
                *self.channel.borrow_mut() = None;
                Incoming::Empty
            }
            Err(TryRecvError::Empty) => Incoming::Empty,
            Err(TryRecvError::Disconnected) => {
                // The thread ended after reporting end of stream, or died.
                *self.channel.borrow_mut() = None;
                self.direct.borrow_mut().closed = true;
                Incoming::Closed
            }
        }
    }

    fn wait(&self) -> Incoming {
        loop {
            let received = {
                let channel = self.channel.borrow();
                channel.as_ref().map(Receiver::recv)
            };
            match received {
                Some(Ok(Item::Incoming(item))) => return item,
                Some(Ok(Item::Failed)) | None => {
                    *self.channel.borrow_mut() = None;
                    return self.direct_wait();
                }
                Some(Err(_)) => {
                    *self.channel.borrow_mut() = None;
                    self.direct.borrow_mut().closed = true;
                    return Incoming::Closed;
                }
            }
        }
    }
}

/// One complete line as an incoming item, or nothing for a blank line.
fn line_to_incoming(line: Line) -> Option<Incoming> {
    match line {
        Line::Text(text) => decode_incoming(&text),
        Line::Oversized => Some(Incoming::Rejected(
            "a command line was too long and was dropped".to_owned(),
        )),
    }
}

/// Starts the reader thread, or `None` when no thread could be created.
fn spawn_reader(
    make_reader: impl Fn() -> Box<dyn Read> + Send + 'static,
) -> Option<Receiver<Item>> {
    let (sender, receiver) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("lazyrad-debug-stdin".to_owned())
        .spawn(move || {
            let mut reader = make_reader();
            let mut lines = LineReader::new();
            let mut chunk = [0u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => {
                        if let Some(line) = lines.finish()
                            && let Some(item) = line_to_incoming(line)
                        {
                            let _ = sender.send(Item::Incoming(item));
                        }
                        let _ = sender.send(Item::Incoming(Incoming::Closed));
                        return;
                    }
                    Ok(read) => {
                        for line in lines.push(&chunk[..read]) {
                            if let Some(item) = line_to_incoming(line)
                                && sender.send(Item::Incoming(item)).is_err()
                            {
                                return;
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        let _ = sender.send(Item::Failed);
                        return;
                    }
                }
            }
        });
    spawned.ok().map(|_| receiver)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    use lazyrad_debug_proto::{Command, Stream, encode_command};

    use super::*;

    /// A writer whose bytes the test can read back.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A reader that fails the way a thread without the process's pipe does.
    struct Broken;

    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("bad descriptor"))
        }
    }

    fn transport(input: Vec<u8>, threaded: bool) -> (ReaderTransport, Shared) {
        let out = Shared::default();
        let transport = ReaderTransport::new(
            move || Box::new(Cursor::new(input.clone())) as Box<dyn Read>,
            Box::new(out.clone()),
            threaded,
        );
        (transport, out)
    }

    fn wire(commands: &[Command]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for command in commands {
            bytes.extend_from_slice(encode_command(command).as_bytes());
            bytes.push(b'\n');
        }
        bytes
    }

    #[test]
    fn events_are_written_one_per_line() {
        let (transport, out) = transport(Vec::new(), false);
        transport.send(&Event::Resumed);
        transport.send(&Event::Output {
            text: "hi".to_owned(),
            stream: Stream::Program,
        });
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains(r#""event":"resumed""#));
    }

    #[test]
    fn a_threaded_transport_delivers_commands_then_end_of_stream() {
        let commands = [Command::Continue, Command::StepOver];
        let (transport, _) = transport(wire(&commands), true);
        assert!(transport.is_threaded());
        assert_eq!(
            transport.wait(),
            Incoming::Command(Command::Continue),
            "blocking waits for the thread"
        );
        assert_eq!(transport.wait(), Incoming::Command(Command::StepOver));
        assert_eq!(transport.wait(), Incoming::Closed);
        assert_eq!(transport.wait(), Incoming::Closed, "closed stays closed");
    }

    #[test]
    fn polling_never_blocks_and_ends_with_closed() {
        let (transport, _) = transport(wire(&[Command::Pause]), true);
        let mut seen = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            match transport.poll() {
                Incoming::Empty => std::thread::sleep(std::time::Duration::from_millis(1)),
                other => {
                    seen.push(other.clone());
                    if other == Incoming::Closed {
                        break;
                    }
                }
            }
        }
        assert_eq!(
            seen,
            [Incoming::Command(Command::Pause), Incoming::Closed],
            "{seen:?}"
        );
    }

    #[test]
    fn a_thread_that_cannot_read_falls_back_to_the_calling_thread() {
        let out = Shared::default();
        let attempts = Arc::new(Mutex::new(0u32));
        let counter = Arc::clone(&attempts);
        let good = wire(&[Command::Stop]);
        let transport = ReaderTransport::new(
            move || {
                let mut count = counter.lock().unwrap();
                *count += 1;
                // The first reader (the thread's) is broken; the fallback is
                // a working one.
                if *count == 1 {
                    Box::new(Broken) as Box<dyn Read>
                } else {
                    Box::new(Cursor::new(good.clone()))
                }
            },
            Box::new(out),
            true,
        );
        assert_eq!(transport.wait(), Incoming::Command(Command::Stop));
        assert!(!transport.is_threaded());
        assert_eq!(transport.poll(), Incoming::Empty, "cannot poll without a thread");
        assert_eq!(transport.wait(), Incoming::Closed);
    }

    #[test]
    fn bad_lines_are_rejected_and_blank_lines_skipped() {
        let mut input = b"\n   \nnot json\n".to_vec();
        input.extend_from_slice(&[0xff, 0xfe, b'\n']);
        input.extend_from_slice(br#"{"v":9,"cmd":"continue"}"#);
        input.push(b'\n');
        input.extend_from_slice(&vec![b'x'; lazyrad_debug_proto::MAX_LINE_BYTES + 5]);
        input.extend_from_slice(b"\n");
        input.extend_from_slice(&wire(&[Command::Continue]));
        let (transport, _) = transport(input, false);
        let mut rejected = 0;
        loop {
            match transport.wait() {
                Incoming::Rejected(_) => rejected += 1,
                Incoming::Command(Command::Continue) => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(rejected, 4, "garbage, bad utf-8, version, oversized");
    }

    #[test]
    fn a_final_line_without_a_newline_is_still_a_command() {
        let mut input = encode_command(&Command::Pause).into_bytes();
        input.truncate(input.len());
        let (transport, _) = transport(input, false);
        assert_eq!(transport.wait(), Incoming::Command(Command::Pause));
        assert_eq!(transport.wait(), Incoming::Closed);
    }

    #[test]
    fn an_installed_transport_replaces_stdio_once() {
        let (custom, _) = transport(wire(&[Command::Continue]), false);
        let custom: Rc<dyn DebugTransport> = Rc::new(custom);
        let for_factory = Rc::clone(&custom);
        install_transport(move || for_factory);
        let taken = super::transport();
        assert!(Rc::ptr_eq(&taken, &custom));
        // The slot is empty again, so the next call would build stdio (not
        // exercised here: it would read this test process's stdin).
        assert!(INSTALLED.with(|slot| slot.borrow().is_none()));
    }
}
