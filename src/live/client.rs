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
const DRAIN_CHUNK: usize = 4096;
/// The read the drain still makes once its deadline has passed (NEW-LOW-002).
const DRAIN_FLOOR: Duration = Duration::from_millis(1);
fn drain_closed() -> ClientError {
    ClientError::Protocol("endpoint closed the connection before reading the request".into())
}
/// Read the peer's one answer line after a failed write. AC-1014, W8-ERR-9: the drain is
/// bounded at an absolute instant, min(now + 1s, the write deadline), never the caller's own
/// (possibly longer) read timeout; the time left is recomputed before every read, so a peer
/// trickling bytes cannot stretch it. It reads at most `DRAIN_CHUNK` bytes at a time and checks
/// `cancelled` before every read. Once that instant has passed, one read still waits
/// `DRAIN_FLOOR` and every later read takes only bytes already buffered, so an answer the peer
/// wrote before closing is used even when the write deadline ran out at the EPIPE
/// (NEW-LOW-002). It peeks first and then consumes only up to the newline: open_send's reader
/// still needs the frames after it.
fn drain_line(
    mut stream: &UnixStream,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>, ClientError> {
    use std::os::fd::AsRawFd;
    let end = deadline.min(Instant::now() + Duration::from_secs(1));
    let mut line = Vec::new();
    let mut chunk = [0u8; DRAIN_CHUNK];
    let mut floor_read = false;
    loop {
        if cancelled() {
            return Err(ClientError::Cancelled);
        }
        let left = end.saturating_duration_since(Instant::now());
        let (timeout, flags) = if !left.is_zero() {
            (Some(left), 0)
        } else if !floor_read {
            floor_read = true;
            (Some(DRAIN_FLOOR), 0)
        } else {
            (None, libc::MSG_DONTWAIT)
        };
        // macOS can reject a timeout change once the peer has already closed; reads on such a
        // socket return what is buffered, then EOF, without blocking.
        if let Some(timeout) = timeout
            && let Err(e) = stream.set_read_timeout(Some(timeout))
            && e.raw_os_error() != Some(libc::EINVAL)
        {
            return Err(ClientError::Protocol(e.to_string()));
        }
        let want = DRAIN_CHUNK.min(super::MAX_FRAME + 1 - line.len());
        // SAFETY: `chunk` is a live, writable buffer of at least `want` bytes and the fd
        // belongs to `stream`, which outlives the call. MSG_DONTWAIT leaves the socket's
        // flags alone.
        let peeked = unsafe {
            libc::recv(
                stream.as_raw_fd(),
                chunk.as_mut_ptr().cast(),
                want,
                libc::MSG_PEEK | flags,
            )
        };
        let n = match usize::try_from(peeked) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => match io::Error::last_os_error().kind() {
                io::ErrorKind::Interrupted => continue,
                // A timed-out read loops to the next phase; nothing buffered after the
                // deadline ends the drain.
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut if flags == 0 => continue,
                _ => break,
            },
        };
        let newline = chunk[..n].iter().position(|&b| b == b'\n');
        let take = newline.map_or(n, |i| i + 1);
        stream
            .read_exact(&mut chunk[..take])
            .map_err(|_| drain_closed())?;
        line.extend_from_slice(&chunk[..take]);
        if newline.is_some() {
            return Ok(line);
        }
        if line.len() > super::MAX_FRAME {
            break;
        }
    }
    Err(drain_closed())
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
                return drain_line(stream, end, cancelled).map(Some);
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
/// Read the one answer line of a unary request by the absolute `end` (F10). The time left is
/// recomputed before every read, so a trickling peer cannot stretch the budget. Once `end` has
/// passed, reads take only bytes already buffered: the verdict is whether the answer had arrived
/// when glen looked, never whether glen's own thread got scheduled before `end`.
fn read_reply(mut stream: &UnixStream, end: Instant) -> Result<Vec<u8>, ClientError> {
    let mut line = Vec::new();
    let mut chunk = [0u8; DRAIN_CHUNK];
    let oversized = || ClientError::Protocol("oversized or incomplete frame".into());
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            stream
                .set_nonblocking(true)
                .map_err(|e| ClientError::Protocol(e.to_string()))?;
        } else if let Err(e) = stream.set_read_timeout(Some(left))
            // macOS rejects a timeout change once the peer has closed; reads on such a socket
            // return what is buffered, then EOF, without blocking.
            && e.raw_os_error() != Some(libc::EINVAL)
        {
            return Err(ClientError::Protocol(e.to_string()));
        }
        let n = match stream.read(&mut chunk) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            // A timed-out read loops once more, which then takes only what is buffered.
            Err(e)
                if !left.is_zero()
                    && matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
            {
                continue;
            }
            Err(e) => return Err(unary_io(e)),
        };
        if n == 0 {
            return Err(if line.is_empty() {
                ClientError::Protocol("eof".into())
            } else {
                oversized()
            });
        }
        let newline = chunk[..n].iter().position(|&b| b == b'\n');
        line.extend_from_slice(&chunk[..newline.map_or(n, |i| i + 1)]);
        if line.len() > super::MAX_FRAME {
            return Err(oversized());
        }
        if newline.is_some() {
            return Ok(line);
        }
    }
}
/// One request and its answer within `timeout`: connect, write and read share one absolute
/// deadline. The answer is read on the caller's thread, so an answer that arrived in time is
/// never lost to a competing timer while a glen I/O thread waited to be scheduled (F10).
pub fn unary(path: &Path, op: &Op, timeout: Duration) -> Result<Value, ClientError> {
    let end = Instant::now() + timeout;
    // A full unix backlog can block connect(2); `bounded_connect` owns that wait.
    let mut stream = bounded_connect(path, end, &|| false)?;
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
        None => read_reply(&stream, end)?,
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
            let line = drain_line(&client, until, &|| false)
                .unwrap_or_else(|e| panic!("closed={closed}: {e}"));
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
        assert!(drain_line(&client, Instant::now(), &|| false).is_err());
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

    /// W8-ERR-9: a peer that trickles one byte every 300 ms, never a newline, must not hold
    /// the drain past ~1s, however long the write deadline, and a cancel must end it.
    #[test]
    fn drain_against_a_trickling_peer_is_bounded_and_cancellable() {
        for cancel_after in [None, Some(Duration::from_millis(400))] {
            let (client, mut peer) = UnixStream::pair().unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let trickle = std::thread::spawn(move || {
                for _ in 0..20 {
                    if peer.write_all(b"x").is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(300));
                }
            });
            let start = Instant::now();
            let result = drain_line(&client, start + Duration::from_secs(10), &|| {
                cancel_after.is_some_and(|after| start.elapsed() >= after)
            });
            let elapsed = start.elapsed();
            drop(client);
            trickle.join().unwrap();
            if cancel_after.is_some() {
                assert!(matches!(result, Err(ClientError::Cancelled)), "{result:?}");
                assert!(
                    elapsed < Duration::from_millis(900),
                    "cancel took {elapsed:?}"
                );
            } else {
                assert!(
                    matches!(result, Err(ClientError::Protocol(ref m)) if m == "endpoint closed the connection before reading the request"),
                    "{result:?}"
                );
                assert!(
                    elapsed >= Duration::from_millis(900) && elapsed < Duration::from_millis(1600),
                    "drain took {elapsed:?}, not the ~1s bound"
                );
            }
        }
    }

    /// W8-ERR-9 end to end: the peer half-closes its read side, so the large write fails with
    /// EPIPE, then trickles one byte every 300 ms with no newline. Linux only: macOS silently
    /// discards writes to a peer that shut its read side, so the write never fails there.
    #[cfg(target_os = "linux")]
    fn trickling_peer(prefix: &str) -> (tempfile::TempDir, PathBuf, std::thread::JoinHandle<()>) {
        let tmp = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in("/tmp")
            .unwrap();
        let path = tmp.path().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.shutdown(Shutdown::Read).unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            for _ in 0..20 {
                if stream.write_all(b"x").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        });
        (tmp, path, peer)
    }
    #[cfg(target_os = "linux")]
    fn large_send() -> SendArgs {
        SendArgs {
            text: "x".repeat(1024 * 1024),
            deliver_as: crate::jobs::Deliver::Auto,
            job_id: None,
            expect_session_id: None,
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn open_send_drain_against_a_trickling_peer_is_bounded_and_cancellable() {
        let (_tmp, path, peer) = trickling_peer("gdrain-");
        let start = Instant::now();
        let result = open_send(&path, "trickle", large_send(), &|| false);
        let elapsed = start.elapsed();
        assert!(
            matches!(result, Err(ClientError::Protocol(ref m)) if m == "endpoint closed the connection before reading the request"),
            "{:?}",
            result.err()
        );
        assert!(
            elapsed < Duration::from_millis(1600),
            "drain took {elapsed:?}, past the ~1.2s bound"
        );
        drop(result);
        peer.join().unwrap();

        let (_tmp, path, peer) = trickling_peer("gdrainc-");
        let start = Instant::now();
        let cancel_at = start + Duration::from_millis(400);
        let result = open_send(&path, "trickle", large_send(), &|| {
            Instant::now() >= cancel_at
        });
        let elapsed = start.elapsed();
        assert!(
            matches!(result, Err(ClientError::Cancelled)),
            "{:?}",
            result.err()
        );
        assert!(
            elapsed < Duration::from_millis(900),
            "cancel took {elapsed:?} to end the drain"
        );
        drop(result);
        peer.join().unwrap();
    }

    // Keyed by path, so concurrently running client tests never take each other's hook.
    static AFTER_WRITE: Mutex<Vec<(PathBuf, Receiver<()>)>> = Mutex::new(Vec::new());
    /// Hold `unary` for `path` after its request is written, until the returned sender fires.
    fn hold_after_write(path: &Path) -> mpsc::Sender<()> {
        let (release, held) = mpsc::channel();
        AFTER_WRITE.lock().unwrap().push((path.to_owned(), held));
        release
    }
    fn hook_pending(path: &Path) -> bool {
        AFTER_WRITE.lock().unwrap().iter().any(|(p, _)| p == path)
    }
    pub(super) fn after_write(path: &Path) {
        let hook = {
            let mut hooks = AFTER_WRITE.lock().unwrap();
            hooks
                .iter()
                .position(|(expected, _)| expected == path)
                .map(|i| hooks.swap_remove(i))
        };
        if let Some((_, release)) = hook {
            release.recv_timeout(Duration::from_secs(10)).unwrap();
        }
    }

    /// F10 (W12): the peer's answer is in glen's socket buffer, but the thread that reads it is
    /// held (as if descheduled) until after the deadline. A competing timer on the caller's
    /// thread used to report `Timeout` for this answer; the verdict now comes from what the
    /// kernel had buffered when glen read. The connection stays open, so no EOF path helps.
    #[test]
    fn unary_uses_an_answer_that_arrived_while_glen_was_held_past_its_deadline() {
        let tmp = tempfile::Builder::new()
            .prefix("gf10-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = tmp.path().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let release = hold_after_write(&path);
        let timeout = Duration::from_secs(1);
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let line = read_line(&mut BufReader::new(&stream)).unwrap();
            // glen set its deadline before it wrote this request.
            let deadline_passed = Instant::now() + timeout + Duration::from_millis(300);
            let request: Value = serde_json::from_slice(&line).unwrap();
            let mut answer =
                serde_json::json!({"v":1,"id":request["id"],"ok":true,"data":{"answer":42}})
                    .to_string()
                    .into_bytes();
            answer.push(b'\n');
            stream.write_all(&answer).unwrap();
            std::thread::sleep(deadline_passed.saturating_duration_since(Instant::now()));
            release.send(()).unwrap();
            stream
        });
        let result = unary(&path, &Op::State, timeout);
        let _open = peer.join().unwrap();
        assert_eq!(result.unwrap()["answer"], 42);
        assert!(!hook_pending(&path));
    }

    /// The answer read has one absolute deadline: a peer that trickles bytes, never a newline,
    /// for 3 s cannot hold a 300 ms `unary` past it.
    #[test]
    fn unary_read_ends_at_its_deadline_against_a_trickling_peer() {
        let tmp = tempfile::Builder::new()
            .prefix("gtrk-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = tmp.path().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            read_line(&mut BufReader::new(&stream)).unwrap();
            for _ in 0..60 {
                if stream.write_all(b"x").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let start = Instant::now();
        let result = unary(&path, &Op::State, Duration::from_millis(300));
        let elapsed = start.elapsed();
        assert!(matches!(result, Err(ClientError::Timeout)), "{result:?}");
        assert!(
            elapsed < Duration::from_secs(2),
            "a trickling peer stretched a 300 ms read to {elapsed:?}"
        );
        drop(result);
        peer.join().unwrap();
    }

    #[test]
    fn unary_reads_a_reply_after_peer_closed_before_the_read() {
        let tmp = tempfile::Builder::new()
            .prefix("gf5-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = tmp.path().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let closed_tx = hold_after_write(&path);
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
        assert!(!hook_pending(&path));
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
