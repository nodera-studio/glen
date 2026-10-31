use crate::ops::{Op, SendArgs};
use serde::Deserialize;
use serde_json::Value;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::Path,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Deserialize)]
pub struct WireError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Option<Value>,
}
#[derive(Debug)]
pub enum Frame {
    Response {
        id: Option<String>,
        ok: bool,
        data: Value,
        error: Option<WireError>,
    },
    Event {
        id: Option<String>,
        kind: String,
        body: Value,
    },
}
pub fn parse_frame(line: &[u8]) -> Result<Frame, String> {
    let v: Value = serde_json::from_slice(line).map_err(|e| e.to_string())?;
    if !v.is_object() || v["v"] != 1 {
        return Err("invalid frame or protocol version".into());
    }
    let id = v["id"].as_str().map(str::to_owned);
    if let Some(ok) = v["ok"].as_bool() {
        let error = if ok {
            None
        } else {
            Some(
                serde_json::from_value(v["error"].clone())
                    .map_err(|e| format!("invalid error: {e}"))?,
            )
        };
        if ok && !v["data"].is_object() {
            return Err("response data must be an object".into());
        }
        Ok(Frame::Response {
            id,
            ok,
            data: v["data"].clone(),
            error,
        })
    } else if let Some(kind) = v["event"].as_str() {
        Ok(Frame::Event {
            id,
            kind: kind.into(),
            body: v,
        })
    } else {
        Err("frame has neither response nor event".into())
    }
}
#[derive(Debug)]
pub enum ConnectError {
    PathTooLong,
    Missing,
    Refused,
    Io(io::Error),
}
#[derive(Debug)]
pub enum ClientError {
    Connect(ConnectError),
    Timeout,
    Protocol(String),
    Refused(WireError),
    Cancelled,
}
impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(ConnectError::PathTooLong) => {
                f.write_str("endpoint socket path is too long")
            }
            Self::Connect(ConnectError::Missing) => f.write_str("endpoint socket is missing"),
            Self::Connect(ConnectError::Refused) => f.write_str("endpoint refused the connection"),
            Self::Connect(ConnectError::Io(e)) => write!(f, "cannot connect to endpoint: {e}"),
            Self::Timeout => f.write_str("endpoint request timed out"),
            Self::Protocol(message) => write!(f, "invalid endpoint response: {message}"),
            Self::Refused(e) => write!(f, "endpoint refused: {} ({})", e.code, e.message),
            Self::Cancelled => f.write_str("endpoint request cancelled"),
        }
    }
}
impl std::error::Error for ClientError {}
pub fn connect(path: &Path) -> Result<UnixStream, ConnectError> {
    if !super::socket_path_ok(path) {
        return Err(ConnectError::PathTooLong);
    }
    let stream = UnixStream::connect(path).map_err(|e| match e.raw_os_error() {
        Some(libc::ENOENT) => ConnectError::Missing,
        Some(libc::ECONNREFUSED) => ConnectError::Refused,
        _ => ConnectError::Io(e),
    })?;
    stream
        .set_write_timeout(Some(super::WRITE))
        .map_err(ConnectError::Io)?;
    Ok(stream)
}
fn bounded_connect(
    path: &Path,
    end: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Result<UnixStream, ClientError> {
    let (tx, rx) = mpsc::channel();
    let path = path.to_owned();
    std::thread::spawn(move || {
        let _ = tx.send(connect(&path));
    });
    loop {
        if cancelled() {
            return Err(ClientError::Cancelled);
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ClientError::Timeout);
        }
        match rx.recv_timeout(left.min(Duration::from_millis(100))) {
            Ok(r) => return r.map_err(ClientError::Connect),
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(_) => return Err(ClientError::Protocol("connect thread ended".into())),
        }
    }
}
fn read_line(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    reader
        .take(super::MAX_FRAME as u64 + 1)
        .read_until(b'\n', &mut line)?;
    if line.is_empty() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof"));
    }
    if line.len() > super::MAX_FRAME || !line.ends_with(b"\n") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized or incomplete frame",
        ));
    }
    Ok(line)
}
fn read_frame(reader: &mut impl BufRead) -> Result<Frame, String> {
    parse_frame(&read_line(reader).map_err(|e| e.to_string())?)
}
fn unary_io(error: io::Error) -> ClientError {
    if matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        ClientError::Timeout
    } else {
        ClientError::Protocol(error.to_string())
    }
}
/// The read the drain still makes once its deadline has passed (NEW-LOW-002).
const DRAIN_FLOOR: Duration = Duration::from_millis(1);
/// Reads with an absolute deadline: the remaining time is recomputed before every read, so
/// a peer trickling bytes cannot stretch the drain. Once `until` has passed, one read still
/// waits `DRAIN_FLOOR` and every later read takes only bytes already buffered, so an answer
/// the peer wrote before closing is used even when the write deadline ran out at the EPIPE.
struct DrainReader<'a> {
    stream: &'a UnixStream,
    until: Instant,
    expired: bool,
}
impl DrainReader<'_> {
    fn read_for(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        // macOS can reject a timeout change once the peer has already closed; reads on such a
        // socket return what is buffered, then EOF, without blocking.
        if let Err(e) = self.stream.set_read_timeout(Some(timeout))
            && e.raw_os_error() != Some(libc::EINVAL)
        {
            return Err(e);
        }
        self.stream.read(buf)
    }
}
impl Read for DrainReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if !left.is_zero() {
            return self.read_for(buf, left);
        }
        if !self.expired {
            self.expired = true;
            return self.read_for(buf, DRAIN_FLOOR);
        }
        use std::os::fd::AsRawFd;
        // SAFETY: `buf` is a live, writable slice of `buf.len()` bytes, and the borrowed stream
        // keeps the descriptor open for the call. MSG_DONTWAIT leaves the socket's flags alone.
        let n = unsafe {
            libc::recv(
                self.stream.as_raw_fd(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                libc::MSG_DONTWAIT,
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }
}
/// Reads one line for the EPIPE drain without reading ahead: open_send's reader still needs
/// the frames after it.
fn drain_line(stream: &UnixStream, until: Instant) -> io::Result<Vec<u8>> {
    read_line(&mut BufReader::with_capacity(
        1,
        DrainReader {
            stream,
            until,
            expired: false,
        },
    ))
}
fn write_request(
    stream: &mut UnixStream,
    bytes: &[u8],
    end: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<u8>>, ClientError> {
    let mut offset = 0;
    while offset < bytes.len() {
        if cancelled() {
            return Err(ClientError::Cancelled);
        }
        if Instant::now() >= end {
            return Err(ClientError::Timeout);
        }
        match stream.write(&bytes[offset..]) {
            Ok(0) => return Err(ClientError::Protocol("socket closed during send".into())),
            Ok(n) => offset += n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::NotConnected
                ) =>
            {
                if cancelled() {
                    return Err(ClientError::Cancelled);
                }
                // AC-1014: the drain has one absolute deadline, min(1s, what's left of the
                // write deadline), never the caller's own (possibly longer) read timeout.
                // `DrainReader` recomputes what is left of it before every read, so a peer
                // trickling bytes cannot stretch the drain past it.
                let until = end.min(Instant::now() + Duration::from_secs(1));
                return drain_line(stream, until).map(Some).map_err(|_| {
                    ClientError::Protocol(
                        "endpoint closed the connection before reading the request".into(),
                    )
                });
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(ClientError::Protocol(e.to_string())),
        }
    }
    Ok(None)
}
fn unary_inner(path: &Path, op: &Op, end: Instant) -> Result<Value, ClientError> {
    let mut stream = connect(path).map_err(ClientError::Connect)?;
    let id = uuid::Uuid::now_v7().to_string();
    stream
        .set_write_timeout(Some(
            end.saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1)),
        ))
        .map_err(|e| ClientError::Protocol(e.to_string()))?;
    let mut bytes = crate::ops::request_bytes(&op.request(&id))
        .map_err(|e| ClientError::Protocol(format!("cannot serialize endpoint request: {e}")))?;
    bytes.push(b'\n');
    stream
        .set_read_timeout(Some(
            end.saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1)),
        ))
        .map_err(|e| ClientError::Protocol(e.to_string()))?;
    let pre_read = write_request(&mut stream, &bytes, end, &|| false)?;
    #[cfg(test)]
    unary_tests::after_write(path);
    let line = match pre_read {
        Some(line) => line,
        None => read_line(&mut BufReader::new(stream)).map_err(unary_io)?,
    };
    match parse_frame(&line).map_err(ClientError::Protocol)? {
        Frame::Response {
            id: Some(got),
            ok,
            data,
            error,
        } if got == id => {
            if ok {
                Ok(data)
            } else {
                Err(ClientError::Refused(error.ok_or_else(|| {
                    ClientError::Protocol("missing error".into())
                })?))
            }
        }
        _ => Err(ClientError::Protocol(
            "unexpected response id or event".into(),
        )),
    }
}
pub fn unary(path: &Path, op: &Op, timeout: Duration) -> Result<Value, ClientError> {
    // Own the entire connect/write/read budget, including a full unix backlog.
    let path = path.to_owned();
    let op = op.clone();
    let end = Instant::now() + timeout;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(unary_inner(&path, &op, end));
    });
    rx.recv_timeout(timeout).map_err(|_| ClientError::Timeout)?
}
/// Control requests carry an expected session id and close their connection on
/// interruption. The worker checks the deadline again before sending any bytes.
pub fn unary_for(
    path: &Path,
    op: &Op,
    expect: &str,
    timeout: Duration,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<Value, ClientError> {
    use std::sync::{Arc, Mutex, atomic::Ordering};
    let end = Instant::now() + timeout;
    let active = Arc::new(Mutex::new(None::<UnixStream>));
    let thread_active = active.clone();
    let thread_stop = stop.clone();
    let path = path.to_owned();
    let id = uuid::Uuid::now_v7().to_string();
    let request = op.request_for(&id, Some(expect));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let mut stream = bounded_connect(&path, end, &|| thread_stop.load(Ordering::SeqCst))?;
            *thread_active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(stream.try_clone().map_err(unary_io)?);
            if thread_stop.load(Ordering::SeqCst) {
                return Err(ClientError::Cancelled);
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ClientError::Timeout);
            }
            stream.set_write_timeout(Some(left)).map_err(unary_io)?;
            stream.set_read_timeout(Some(left)).map_err(unary_io)?;
            let mut bytes = crate::ops::request_bytes(&request).map_err(|e| {
                ClientError::Protocol(format!("cannot serialize endpoint request: {e}"))
            })?;
            bytes.push(b'\n');
            let pre_read = write_request(&mut stream, &bytes, end, &|| {
                thread_stop.load(Ordering::SeqCst)
            })?;
            let line = match pre_read {
                Some(line) => line,
                None => read_line(&mut BufReader::new(stream)).map_err(unary_io)?,
            };
            match parse_frame(&line).map_err(ClientError::Protocol)? {
                Frame::Response {
                    id: Some(got),
                    ok,
                    data,
                    error,
                } if got == id => {
                    if ok {
                        Ok(data)
                    } else {
                        Err(ClientError::Refused(error.ok_or_else(|| {
                            ClientError::Protocol("missing response error".into())
                        })?))
                    }
                }
                _ => Err(ClientError::Protocol(
                    "unexpected response id or event".into(),
                )),
            }
        })();
        if tx.send(result).is_err() {
            drop(thread_active);
        }
    });
    loop {
        let stopped = stop.load(Ordering::SeqCst);
        let left = end.saturating_duration_since(Instant::now());
        if stopped || left.is_zero() {
            if let Some(stream) = active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                && let Err(error) = stream.shutdown(Shutdown::Both)
                && error.kind() != io::ErrorKind::NotConnected
            {
                return Err(ClientError::Protocol(format!(
                    "cannot close control socket: {error}"
                )));
            }
            return Err(if stopped {
                ClientError::Cancelled
            } else {
                ClientError::Timeout
            });
        }
        match rx.recv_timeout(left.min(Duration::from_millis(50))) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(_) => return Err(ClientError::Protocol("control socket thread ended".into())),
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Hello {
    pub protocol: u32,
    pub ops: Vec<String>,
    pub ext_version: Option<String>,
    pub omp_version: Option<String>,
    pub pid: u32,
    pub session_id: String,
    pub session_file: Option<std::path::PathBuf>,
    pub mode: Option<String>,
    pub state: String,
    pub busy: bool,
    pub queued: bool,
    pub active_requests: usize,
}
pub fn hello(path: &Path, timeout: Duration) -> Result<Hello, ClientError> {
    let data = unary(
        path,
        &Op::Hello {
            client_version: env!("CARGO_PKG_VERSION").into(),
        },
        timeout,
    )?;
    if data["protocol"].as_u64().is_none()
        || data["session_id"].as_str().is_none_or(str::is_empty)
        || data["ops"]
            .as_array()
            .is_none_or(|ops| ops.iter().any(|op| !op.is_string()))
    {
        return Err(ClientError::Protocol("malformed hello data".into()));
    }
    serde_json::from_value(data).map_err(|e| ClientError::Protocol(e.to_string()))
}
pub fn state(path: &Path, timeout: Duration) -> Result<Hello, ClientError> {
    let data = unary(path, &Op::State, timeout)?;
    if data["session_id"].as_str().is_none_or(str::is_empty) || !data["busy"].is_boolean() {
        return Err(ClientError::Protocol("malformed state data".into()));
    }
    serde_json::from_value(data).map_err(|e| ClientError::Protocol(e.to_string()))
}
pub struct SendConn {
    pub stream: UnixStream,
    pub rx: Receiver<Result<Frame, String>>,
}
impl SendConn {
    pub fn close(&self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}
impl Drop for SendConn {
    fn drop(&mut self) {
        self.close();
    }
}
pub fn open_send(
    path: &Path,
    id: &str,
    args: SendArgs,
    cancelled: &dyn Fn() -> bool,
) -> Result<SendConn, ClientError> {
    let end = Instant::now() + super::WRITE;
    let mut stream = bounded_connect(path, end, cancelled)?;
    stream
        .set_write_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| ClientError::Protocol(e.to_string()))?;
    let mut bytes = crate::ops::request_bytes(&Op::Send(args).request(id))
        .map_err(|e| ClientError::Protocol(format!("cannot serialize endpoint request: {e}")))?;
    bytes.push(b'\n');
    stream
        .set_read_timeout(Some(
            end.saturating_duration_since(Instant::now())
                .min(Duration::from_secs(1))
                .max(Duration::from_millis(1)),
        ))
        .map_err(|e| ClientError::Protocol(e.to_string()))?;
    let pre_read = write_request(&mut stream, &bytes, end, cancelled)?;
    // macOS rejects timeout changes after a fast peer has answered and closed.
    if let Err(e) = stream.set_read_timeout(None)
        && e.raw_os_error() != Some(libc::EINVAL)
    {
        return Err(ClientError::Protocol(e.to_string()));
    }
    let reader = stream
        .try_clone()
        .map_err(|e| ClientError::Protocol(e.to_string()))?;
    let (tx, rx) = mpsc::channel();
    if let Some(line) = pre_read {
        tx.send(parse_frame(&line))
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
    }
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        loop {
            let frame = read_frame(&mut reader);
            let stop = frame.is_err();
            if tx.send(frame).is_err() || stop {
                break;
            }
        }
    });
    Ok(SendConn { stream, rx })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn client_errors_display_actionable_text_without_debug_payloads() {
        for (error, expected) in [
            (ClientError::Timeout, "endpoint request timed out"),
            (
                ClientError::Connect(ConnectError::Missing),
                "endpoint socket is missing",
            ),
            (
                ClientError::Connect(ConnectError::Refused),
                "endpoint refused the connection",
            ),
            (
                ClientError::Connect(ConnectError::PathTooLong),
                "endpoint socket path is too long",
            ),
            (
                ClientError::Protocol("malformed JSON".into()),
                "invalid endpoint response: malformed JSON",
            ),
            (ClientError::Cancelled, "endpoint request cancelled"),
            (
                ClientError::Refused(WireError {
                    code: "unsupported_version".into(),
                    message: "unsupported protocol".into(),
                    details: Some(serde_json::json!({"supported":[1]})),
                }),
                "endpoint refused: unsupported_version (unsupported protocol)",
            ),
        ] {
            assert_eq!(error.to_string(), expected);
        }
    }
    #[test]
    fn frames_and_path_bytes() {
        for v in [
            serde_json::json!({"v":1,"id":"r","ok":true,"data":{}}),
            serde_json::json!({"v":1,"id":"r","ok":false,"error":{"code":"x","message":"m"}}),
            serde_json::json!({"v":1,"id":"r","event":"future"}),
        ] {
            assert!(parse_frame(v.to_string().as_bytes()).is_ok());
        }
        for s in ["[]", "{}", "{\"v\":2,\"event\":\"x\"}"] {
            assert!(parse_frame(s.as_bytes()).is_err());
        }
        let p = format!("/{}", "a".repeat(super::super::SUN_PATH_MAX - 2));
        assert!(super::super::socket_path_ok(Path::new(&p)));
        assert!(!super::super::socket_path_ok(Path::new(&(p.clone() + "a"))));
        assert!(!super::super::socket_path_ok(Path::new(&(p + "é"))));
        let multibyte = format!("/{}", "é".repeat((super::super::SUN_PATH_MAX - 2) / 2));
        assert!(super::super::socket_path_ok(Path::new(&multibyte)));
        assert!(!super::super::socket_path_ok(Path::new(&(multibyte + "a"))));
    }
}

#[cfg(test)]
mod unary_tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::{path::PathBuf, sync::Mutex};

    #[test]
    fn partial_large_write_drains_the_peers_answer_without_reading_ahead() {
        let (mut client, mut peer) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        client
            .set_write_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        peer.set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let bytes = crate::ops::request_bytes(
            &Op::Send(SendArgs {
                text: "x".repeat(1024 * 1024),
                deliver_as: crate::jobs::Deliver::Auto,
                job_id: None,
                expect_session_id: None,
            })
            .request("large"),
        )
        .unwrap();
        let worker = std::thread::spawn(move || {
            let mut prefix = [0; 4096];
            peer.read_exact(&mut prefix).unwrap();
            let id = serde_json::Deserializer::from_slice(&prefix[6..])
                .into_iter::<String>()
                .next()
                .unwrap()
                .unwrap();
            writeln!(
                peer,
                "{}",
                serde_json::json!({"v":1,"id":id,"ok":true,"data":{"answer":42}})
            )
            .unwrap();
            writeln!(
                peer,
                "{}",
                serde_json::json!({"v":1,"id":id,"event":"error"})
            )
            .unwrap();
        });
        let line = write_request(
            &mut client,
            &bytes,
            Instant::now() + Duration::from_secs(10),
            &|| false,
        )
        .unwrap()
        .unwrap();
        assert!(
            matches!(parse_frame(&line).unwrap(), Frame::Response { ok: true, data, .. } if data["answer"] == 42)
        );
        assert!(
            matches!(read_frame(&mut BufReader::new(&client)).unwrap(), Frame::Event { kind, .. } if kind == "error")
        );
        worker.join().unwrap();
    }

    /// NEW-LOW-002 (W10 fix cycle 3): a drain whose deadline has already passed still reads,
    /// so an answer the peer wrote before closing is used, whole, rather than lost; with nothing
    /// buffered it gives up at once instead of waiting.
    #[test]
    fn an_expired_drain_still_returns_a_buffered_answer() {
        let answer = br#"{"v":1,"type":"response","id":"x","ok":false,"error":{"code":"busy","message":"no"}}"#;
        for closed in [true, false] {
            let (client, mut peer) = UnixStream::pair().unwrap();
            peer.write_all(answer).unwrap();
            peer.write_all(b"\n").unwrap();
            peer.write_all(b"next frame\n").unwrap();
            let peer = (!closed).then_some(peer);
            let until = Instant::now();
            std::thread::sleep(Duration::from_millis(5));
            let line =
                drain_line(&client, until).unwrap_or_else(|e| panic!("closed={closed}: {e}"));
            assert_eq!(
                line.strip_suffix(b"\n").unwrap(),
                &answer[..],
                "closed={closed}"
            );
            // The drain did not read ahead into the next frame.
            assert_eq!(
                read_line(&mut BufReader::new(&client)).unwrap(),
                b"next frame\n",
                "closed={closed}"
            );
            drop(peer);
        }
        let (client, _peer) = UnixStream::pair().unwrap();
        let start = Instant::now();
        assert!(drain_line(&client, Instant::now()).is_err());
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "an expired drain with nothing buffered waited {:?}",
            start.elapsed()
        );
    }

    /// AC-1014 / LOW-004 (fixes2 decision 5): the drain has one absolute deadline. The peer
    /// stays connected on a half-closed socket (the write fails with EPIPE) and trickles a frame one byte every 200 ms, each gap well under any single read timeout,
    /// for ~3 s before its newline. A per-read timeout would keep the drain alive until that
    /// line completes; the absolute deadline ends it at ~1 s with no frame.
    #[test]
    fn a_trickling_half_closed_peer_cannot_stretch_the_drain_past_its_deadline() {
        let (mut client, mut peer) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        client
            .set_write_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        // Half-close the connection: our write side is shut, so the write fails with EPIPE
        // on Linux and macOS alike, while the peer can still send. (A peer `SHUT_RD` gives
        // EPIPE on Linux only.)
        client.shutdown(Shutdown::Write).unwrap();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let trickle = std::thread::spawn(move || {
            for _ in 0..15 {
                if peer.write_all(b"x").is_err()
                    || done_rx.recv_timeout(Duration::from_millis(200)).is_ok()
                {
                    return;
                }
            }
            let _ = peer.write_all(b"\n");
        });
        let bytes = crate::ops::request_bytes(
            &Op::Send(SendArgs {
                text: "x".repeat(1024 * 1024),
                deliver_as: crate::jobs::Deliver::Auto,
                job_id: None,
                expect_session_id: None,
            })
            .request("half-closed"),
        )
        .unwrap();
        let start = Instant::now();
        let result = write_request(
            &mut client,
            &bytes,
            Instant::now() + Duration::from_secs(10),
            &|| false,
        );
        let elapsed = start.elapsed();
        let _ = done_tx.send(());
        trickle.join().unwrap();
        assert!(
            matches!(result, Err(ClientError::Protocol(ref m)) if m == "endpoint closed the connection before reading the request"),
            "the drain must give up before the trickled line completes: {result:?}"
        );
        // 1 s drain bound, plus a scheduling allowance well under the ~3 s trickle.
        assert!(
            elapsed >= Duration::from_millis(900) && elapsed < Duration::from_millis(2500),
            "drain took {elapsed:?}; its absolute deadline is 1 s"
        );
    }

    #[test]
    fn write_request_drains_only_one_line_and_preserves_closed_transport_errors() {
        for answer in [
            Some(b"{\"v\":1}\nsecond\n".as_slice()),
            None,
            Some(b"partial".as_slice()),
        ] {
            let (mut client, mut peer) = UnixStream::pair().unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            client
                .set_write_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            if let Some(answer) = answer {
                peer.write_all(answer).unwrap();
            }
            drop(peer);
            let result = write_request(
                &mut client,
                b"request\n",
                Instant::now() + Duration::from_secs(10),
                &|| false,
            );
            if answer.is_some_and(|bytes| bytes.ends_with(b"\n")) {
                assert_eq!(result.unwrap(), Some(b"{\"v\":1}\n".to_vec()));
                assert_eq!(
                    read_line(&mut BufReader::new(&client)).unwrap(),
                    b"second\n"
                );
            } else {
                assert!(
                    matches!(result, Err(ClientError::Protocol(ref message)) if message == "endpoint closed the connection before reading the request")
                );
            }
        }
    }

    #[test]
    fn write_request_cancel_and_deadline_do_not_write_or_drain() {
        let (mut client, mut peer) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        peer.set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        assert!(matches!(
            write_request(&mut client, b"request\n", Instant::now(), &|| true),
            Err(ClientError::Cancelled)
        ));
        assert!(matches!(
            write_request(&mut client, b"request\n", Instant::now(), &|| false),
            Err(ClientError::Timeout)
        ));
        assert!(
            matches!(peer.read(&mut [0]), Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut))
        );
        // If cancellation arrives during a failed write, do not start the drain read.
        drop(peer);
        let checks = std::cell::Cell::new(0);
        let cancelled = || {
            checks.set(checks.get() + 1);
            checks.get() > 1
        };
        assert!(matches!(
            write_request(
                &mut client,
                b"request\n",
                Instant::now() + Duration::from_secs(10),
                &cancelled
            ),
            Err(ClientError::Cancelled)
        ));
        assert_eq!(checks.get(), 2);
    }

    // Match by path so concurrently running client tests never consume this hook.
    static AFTER_WRITE: Mutex<Option<(PathBuf, Receiver<()>)>> = Mutex::new(None);
    pub(super) fn after_write(path: &Path) {
        let hook = {
            let mut hook = AFTER_WRITE.lock().unwrap();
            if hook.as_ref().is_some_and(|(expected, _)| expected == path) {
                hook.take()
            } else {
                None
            }
        };
        if let Some((_, closed)) = hook {
            closed.recv_timeout(Duration::from_secs(10)).unwrap();
        }
    }

    #[test]
    fn unary_reads_a_reply_after_peer_closed_before_the_read() {
        let tmp = tempfile::Builder::new()
            .prefix("gf5-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = tmp.path().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let (closed_tx, closed_rx) = mpsc::channel();
        *AFTER_WRITE.lock().unwrap() = Some((path.clone(), closed_rx));
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let line = read_line(&mut BufReader::new(&stream)).unwrap();
            let request: Value = serde_json::from_slice(&line).unwrap();
            writeln!(
                stream,
                "{}",
                serde_json::json!({"v":1,"id":request["id"],"ok":true,"data":{"answer":42}})
            )
            .unwrap();
            drop(stream);
            closed_tx.send(()).unwrap();
        });
        let answer = unary(&path, &Op::State, Duration::from_secs(10)).unwrap();
        assert_eq!(answer["answer"], 42);
        peer.join().unwrap();
        assert!(AFTER_WRITE.lock().unwrap().is_none());
    }
    #[test]
    fn unary_response_ids_malformed_and_total_timeout() {
        for scenario in ["ok", "garbage", "wrong_id", "timeout"] {
            let t = tempfile::Builder::new()
                .prefix("gcli-")
                .tempdir_in("/tmp")
                .unwrap();
            let path = t.path().join("s");
            let listener = UnixListener::bind(&path).unwrap();
            let peer = std::thread::spawn(move || {
                let (mut s, _) = listener.accept().unwrap();
                s.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                let mut line = String::new();
                BufReader::new(s.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                match scenario {"garbage"=>writeln!(s,"garbage").unwrap(),"timeout"=>std::thread::sleep(Duration::from_millis(400)),_=>writeln!(s,"{}",serde_json::json!({"v":1,"id":if scenario=="wrong_id"{serde_json::json!("other")}else{request["id"].clone()},"ok":true,"data":{"answer":1}})).unwrap()}
            });
            let start = Instant::now();
            let result = unary(&path, &Op::State, Duration::from_millis(200));
            assert!(start.elapsed() < Duration::from_millis(350));
            match scenario {
                "ok" => assert_eq!(result.unwrap()["answer"], 1),
                "timeout" => assert!(matches!(result, Err(ClientError::Timeout))),
                _ => assert!(matches!(result, Err(ClientError::Protocol(_)))),
            }
            peer.join().unwrap();
        }
    }
}
