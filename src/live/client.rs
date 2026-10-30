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
fn unary_inner(path: &Path, op: &Op, end: Instant) -> Result<Value, ClientError> {
    let mut stream = connect(path).map_err(ClientError::Connect)?;
    let id = uuid::Uuid::now_v7().to_string();
    stream
        .set_write_timeout(Some(
            end.saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1)),
        ))
        .map_err(|e| ClientError::Protocol(e.to_string()))?;
    let bytes = format!("{}\n", op.request(&id));
    stream.write_all(bytes.as_bytes()).map_err(unary_io)?;
    stream
        .set_read_timeout(Some(
            end.saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1)),
        ))
        .map_err(|e| ClientError::Protocol(e.to_string()))?;
    let line = read_line(&mut BufReader::new(stream)).map_err(unary_io)?;
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
    let bytes = format!("{}\n", Op::Send(args).request(id)).into_bytes();
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
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(ClientError::Protocol(e.to_string())),
        }
    }
    let reader = stream
        .try_clone()
        .map_err(|e| ClientError::Protocol(e.to_string()))?;
    let (tx, rx) = mpsc::channel();
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
