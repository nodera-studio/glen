//! The MCP layer (v0.2 W5, design §A22): a synchronous stdio JSON-RPC server that exposes glen's
//! visible commands as tools. Every tool call runs glen itself (`current_exe`, argv, `--json`,
//! never a shell), so every tool result is the CLI's own envelope. The tool catalog
//! (`tools.json`) is generated from the `<!-- mcp: … -->` annotations in `docs/json.md` plus clap.
pub mod catalog;
pub(crate) mod channel;
pub mod dispatch;
pub mod server;

use serde_json::Value;
use std::{
    collections::VecDeque,
    io::{self, BufRead, Read, Write},
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread,
    time::{Duration, Instant},
};

/// Protocol revisions served, preferred first (both without JSON-RPC batching).
pub const SUPPORTED_VERSIONS: [&str; 2] = ["2025-11-25", "2025-06-18"];
/// The longest accepted request line, without its newline.
pub const MAX_LINE: usize = 4 * 1024 * 1024;
/// The most stdout a tool call's child may print.
pub const MAX_OUTPUT: usize = 16 * 1024 * 1024;
/// Concurrent `tools/call` children.
pub const MAX_IN_FLIGHT: usize = 16;
/// The main loop's wake-up period (signals are seen within one tick).
pub const TICK: Duration = Duration::from_millis(50);
/// How often a call thread polls its child.
pub const CALL_TICK: Duration = Duration::from_millis(10);
/// SIGINT to SIGKILL for a cancelled call.
pub const CANCEL_GRACE: Duration = Duration::from_millis(400);
/// How long shutdown waits for call threads.
pub const SHUTDOWN_JOIN: Duration = Duration::from_secs(1);
/// The channel notifier's minimum poll period.
pub const CHANNEL_POLL: Duration = Duration::from_secs(1);
/// The bound on the channel baseline taken while answering `initialize`.
pub const CHANNEL_BASELINE: Duration = Duration::from_secs(2);
/// The longest channel notification `content`, in bytes.
pub const CHANNEL_MAX_CONTENT: usize = 8 * 1024;
/// Reply ids the channel notifier remembers once they are no longer listed (ids a poll still
/// lists are always kept).
pub const SEEN_CAP: usize = 10_000;
/// Unwritten stdout bytes past which a call or the notifier waits for the host to read; past
/// twice this a host that keeps sending requests without reading counts as gone.
pub const OUTPUT_QUEUE: usize = 32 * 1024 * 1024;

/// One framed input line.
#[derive(Debug, PartialEq)]
pub(crate) enum Line {
    Text(Vec<u8>),
    /// Longer than `MAX_LINE`; the rest up to the next newline was skipped.
    TooLong,
    Eof,
}

/// Reads one newline-terminated line of at most `max` bytes (a final unterminated line counts).
pub(crate) fn read_line(
    input: &mut impl BufRead,
    buf: &mut Vec<u8>,
    max: usize,
) -> io::Result<Line> {
    buf.clear();
    let n = Read::take(&mut *input, max as u64 + 1).read_until(b'\n', buf)?;
    if n == 0 {
        return Ok(Line::Eof);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(Line::Text(std::mem::take(buf)));
    }
    if buf.len() > max {
        input.skip_until(b'\n')?;
        buf.clear();
        return Ok(Line::TooLong);
    }
    Ok(Line::Text(std::mem::take(buf)))
}

/// The one writer every protocol message goes through: compact JSON, one line, flushed, in
/// queue order. A thread of its own does the blocking writes, so a host that stops reading
/// stdout never stalls the main loop (cancellation, EOF and signals stay live).
#[derive(Clone)]
pub(crate) struct Writer(Arc<Pipe>);
#[derive(Default)]
struct Pipe {
    queue: Mutex<Queue>,
    changed: Condvar,
}
#[derive(Default)]
struct Queue {
    lines: VecDeque<Vec<u8>>,
    bytes: usize,
    /// The writer thread is writing a line it already took off `lines`.
    writing: bool,
    /// A write failed (EPIPE included), or the host left too much unread.
    gone: bool,
    /// Shutdown began: `send` stops waiting for room.
    closing: bool,
    /// Shutdown drained the queue: the writer thread ends once it is empty.
    done: bool,
}
impl Pipe {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn wait<'a>(&self, queue: MutexGuard<'a, Queue>) -> MutexGuard<'a, Queue> {
        self.changed
            .wait(queue)
            .unwrap_or_else(PoisonError::into_inner)
    }
    fn write_all(&self, mut out: impl Write) {
        let mut queue = self.lock();
        loop {
            if queue.gone {
                return;
            }
            let Some(line) = queue.lines.pop_front() else {
                if queue.done {
                    return;
                }
                queue = self.wait(queue);
                continue;
            };
            queue.writing = true;
            drop(queue);
            let written = out.write_all(&line).and_then(|()| out.flush());
            queue = self.lock();
            queue.writing = false;
            // `push` empties the queue when it declares the host gone mid-write.
            queue.bytes = queue.bytes.saturating_sub(line.len());
            if written.is_err() {
                queue.gone = true;
                queue.lines.clear();
                queue.bytes = 0;
            }
            self.changed.notify_all();
        }
    }
}
impl Writer {
    pub(crate) fn new(out: impl Write + Send + 'static) -> Self {
        let pipe = Arc::new(Pipe::default());
        let shared = pipe.clone();
        let spawned = thread::Builder::new()
            .name("glen-mcp-stdout".into())
            .spawn(move || shared.write_all(out));
        if spawned.is_err() {
            pipe.lock().gone = true;
        }
        Self(pipe)
    }
    /// Queues one message from a call or the notifier: while more than `OUTPUT_QUEUE` bytes
    /// wait unread, it waits for the host to catch up (until shutdown begins).
    pub(crate) fn send(&self, message: &Value) {
        self.push(message, true, || ());
    }
    /// `send`, running `queued` once the wait is over, under the queue lock, before the host
    /// can read the line (a call frees its slot there).
    pub(crate) fn send_then(&self, message: &Value, queued: impl FnOnce()) {
        self.push(message, true, queued);
    }
    /// Queues one message from the main loop, never waiting.
    pub(crate) fn post(&self, message: &Value) {
        self.push(message, false, || ());
    }
    fn push(&self, message: &Value, wait: bool, queued: impl FnOnce()) {
        // A `Value` always serializes; should it not, nothing is queued.
        let mut line = serde_json::to_vec(message).unwrap_or_default();
        if line.is_empty() {
            return queued();
        }
        line.push(b'\n');
        let mut queue = self.0.lock();
        while wait
            && !queue.gone
            && !queue.closing
            && queue.bytes > 0
            && queue.bytes + line.len() > OUTPUT_QUEUE
        {
            queue = self.0.wait(queue);
        }
        queued();
        if queue.gone {
            return;
        }
        if queue.bytes + line.len() > 2 * OUTPUT_QUEUE {
            // A host that keeps asking but leaves this much unread is not reading at all.
            queue.gone = true;
            queue.lines.clear();
            queue.bytes = 0;
        } else {
            queue.bytes += line.len();
            queue.lines.push_back(line);
        }
        self.0.changed.notify_all();
    }
    pub(crate) fn gone(&self) -> bool {
        self.0.lock().gone
    }
    /// Shutdown begins: no sender waits for room any longer.
    pub(crate) fn close(&self) {
        self.0.lock().closing = true;
        self.0.changed.notify_all();
    }
    /// Shutdown ends: waits until everything queued is written, the client is gone, or
    /// `deadline` passes (a wedged write is left to the process exit), then lets the writer
    /// thread end.
    pub(crate) fn finish(&self, deadline: Instant) {
        let mut queue = self.0.lock();
        queue.closing = true;
        while !queue.gone && (queue.writing || !queue.lines.is_empty()) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            queue = self
                .0
                .changed
                .wait_timeout(queue, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        queue.done = true;
        self.0.changed.notify_all();
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex, PoisonError},
    };
    /// A `Write` the test can read while the server runs.
    #[derive(Clone, Default)]
    pub(crate) struct Shared(pub Arc<Mutex<Vec<u8>>>);
    impl Write for Shared {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Shared {
        pub(crate) fn lines(&self) -> Vec<serde_json::Value> {
            let bytes = self
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            String::from_utf8(bytes)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn lines_are_bounded_and_oversize_lines_are_skipped() {
        let mut input = Cursor::new(b"ab\r\n\n0123456789\nxyz\ntail".to_vec());
        let mut buf = Vec::new();
        let mut next = || read_line(&mut input, &mut buf, 5).unwrap();
        assert_eq!(next(), Line::Text(b"ab".to_vec()));
        assert_eq!(next(), Line::Text(vec![]));
        assert_eq!(next(), Line::TooLong);
        assert_eq!(next(), Line::Text(b"xyz".to_vec()));
        assert_eq!(next(), Line::Text(b"tail".to_vec()));
        assert_eq!(next(), Line::Eof);
        // Exactly `max` bytes is accepted; an oversize final line without a newline is skipped.
        let mut input = Cursor::new(b"12345\n123456".to_vec());
        let mut buf = Vec::new();
        assert_eq!(
            read_line(&mut input, &mut buf, 5).unwrap(),
            Line::Text(b"12345".to_vec())
        );
        assert_eq!(read_line(&mut input, &mut buf, 5).unwrap(), Line::TooLong);
        assert_eq!(read_line(&mut input, &mut buf, 5).unwrap(), Line::Eof);
    }

    #[test]
    fn the_writer_never_interleaves_messages() {
        let out = test_support::Shared::default();
        let writer = Writer::new(out.clone());
        let big = "x".repeat(64 * 1024);
        std::thread::scope(|s| {
            for t in 0..4 {
                let (writer, big) = (writer.clone(), &big);
                s.spawn(move || {
                    for i in 0..25 {
                        if i % 2 == 0 {
                            writer.send(&serde_json::json!({"t":t,"i":i,"pad":big}));
                        } else {
                            writer.post(&serde_json::json!({"t":t,"i":i,"pad":big}));
                        }
                    }
                });
            }
        });
        writer.finish(Instant::now() + Duration::from_secs(10));
        let lines = out.lines();
        assert_eq!(lines.len(), 100);
        assert!(
            lines
                .iter()
                .all(|l| l["pad"].as_str() == Some(big.as_str()))
        );
    }

    #[test]
    fn a_failed_write_marks_the_client_gone() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let writer = Writer::new(Broken);
        assert!(!writer.gone());
        writer.send(&serde_json::json!({}));
        writer.finish(Instant::now() + Duration::from_secs(10));
        assert!(writer.gone());
        writer.post(&serde_json::json!({}));
        assert!(writer.gone());
    }

    #[test]
    fn send_then_runs_before_the_host_can_read_the_line() {
        let out = test_support::Shared::default();
        let writer = Writer::new(out.clone());
        let mut ran = false;
        writer.send_then(&serde_json::json!({"id":1}), || {
            assert!(out.lines().is_empty(), "the line was readable first");
            ran = true;
        });
        assert!(ran);
        writer.finish(Instant::now() + Duration::from_secs(10));
        assert_eq!(out.lines(), [serde_json::json!({"id":1})]);
    }

    #[test]
    fn a_host_that_stops_reading_never_blocks_the_main_loop() {
        // Nobody reads `unread`: the first writes fill the pipe and the writer thread blocks.
        let (unread, out) = io::pipe().unwrap();
        let writer = Writer::new(out);
        let big = serde_json::json!({"pad":"x".repeat(64 * 1024)});
        let start = Instant::now();
        for _ in 0..32 {
            writer.post(&big);
        }
        assert!(!writer.gone());
        assert!(start.elapsed() < Duration::from_secs(2), "post waited");
        // Shutdown takes `send` out of its wait for room, and the drain is bounded.
        let huge = serde_json::json!({"pad":"y".repeat(OUTPUT_QUEUE)});
        let blocked = {
            let (writer, huge) = (writer.clone(), huge.clone());
            thread::spawn(move || writer.send(&huge))
        };
        thread::sleep(Duration::from_millis(100));
        assert!(!blocked.is_finished(), "send waits while the queue is full");
        writer.close();
        blocked.join().unwrap();
        let start = Instant::now();
        writer.finish(Instant::now() + Duration::from_millis(200));
        let took = start.elapsed();
        assert!(
            took >= Duration::from_millis(200) && took < Duration::from_secs(2),
            "{took:?}"
        );
        assert!(!writer.gone());
        // Past twice `OUTPUT_QUEUE` unread, the host counts as gone.
        writer.post(&huge);
        assert!(writer.gone());
        // The blocked write fails once the reading end is gone, and the writer thread ends.
        drop(unread);
    }
}
