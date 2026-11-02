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
    io::{self, BufRead, Read, Write},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
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
/// Reply ids the channel notifier remembers.
pub const SEEN_CAP: usize = 10_000;

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

/// The one writer every protocol message goes through: compact JSON, one line, flushed.
#[derive(Clone)]
pub(crate) struct Writer(Arc<Mutex<WriterState>>);
struct WriterState {
    out: Box<dyn Write + Send>,
    gone: bool,
}
impl Writer {
    pub(crate) fn new(out: impl Write + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(WriterState {
            out: Box::new(out),
            gone: false,
        })))
    }
    /// Writes one message; a write error (EPIPE included) marks the client gone.
    pub(crate) fn send(&self, message: &Value) {
        let Ok(mut line) = serde_json::to_vec(message) else {
            return;
        };
        line.push(b'\n');
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.gone {
            return;
        }
        if state
            .out
            .write_all(&line)
            .and_then(|()| state.out.flush())
            .is_err()
        {
            state.gone = true;
        }
    }
    pub(crate) fn gone(&self) -> bool {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).gone
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
                        writer.send(&serde_json::json!({"t":t,"i":i,"pad":big}));
                    }
                });
            }
        });
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
        assert!(writer.gone());
    }
}
