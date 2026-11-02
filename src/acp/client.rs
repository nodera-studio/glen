//! The synchronous ACP transport for `glen __acp` (design §A23): NDJSON JSON-RPC 2.0 over a child's
//! stdio. A reader thread turns stdout lines into [`Frame`]s and a writer thread owns stdin, so a
//! blocked pipe never stalls the 100 ms tick or the cancel decision. Generic over `Read` and
//! `Write`: [`Client::from_io`] is the test seam.
use super::{Answer, CLOSE_TIMEOUT, MAX_EVENTS, MAX_FRAME_BYTES, TICK, Updates};
use crate::{cli::AcpPermission, ctl::ephemeral::Process, error::Warning};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    time::{Duration, Instant},
};

/// One line of omp's stdout, already classified.
#[derive(Debug)]
pub enum Frame {
    Json(Value),
    Malformed(String),
    Eof,
}
/// A JSON-RPC error response, with omp's `data.details` preferred as the text.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: Value,
    pub message: String,
}
impl RpcError {
    fn from_value(error: &Value) -> Self {
        Self {
            code: error.get("code").cloned().unwrap_or(Value::Null),
            message: super::rpc_error_message(error),
        }
    }
}
#[derive(Debug, PartialEq)]
pub enum CallError {
    Rpc(RpcError),
    Exited,
    Malformed(String),
    Timeout,
    Interrupted,
}
/// The result of processing at most one frame.
#[derive(Debug)]
pub enum Pump {
    Response {
        id: Value,
        result: Result<Value, RpcError>,
    },
    /// A request or notification from omp was handled.
    Handled,
    /// Nothing arrived within the wait.
    Idle,
    Closed,
    Malformed(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    Interrupt,
    Deadline,
}
impl CancelReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Interrupt => "interrupt",
            Self::Deadline => "deadline",
        }
    }
}
/// `sent`: the `session/cancel` notification was queued; `confirmed`: omp answered the prompt
/// with `stopReason:"cancelled"` within the grace (it accepted the cancel; cleanup may continue).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelState {
    pub sent: bool,
    pub confirmed: bool,
    pub reason: CancelReason,
}
impl CancelState {
    pub fn json(&self) -> Value {
        json!({"sent":self.sent,"confirmed":self.confirmed,"reason":self.reason.as_str()})
    }
}
#[derive(Debug)]
pub struct PromptOutcome {
    /// The raw `stopReason` of the prompt response; `None` when none arrived in the cancel grace.
    pub stop_reason: Option<String>,
    pub usage: Option<Value>,
    pub cancel: Option<CancelState>,
}

pub struct Client {
    writer: Option<Sender<Vec<u8>>>,
    frames: Receiver<Frame>,
    process: Option<Process>,
    next_id: u64,
    policy: AcpPermission,
    pub updates: Updates,
    events: Vec<Value>,
    events_dropped: u64,
    cancel: Option<CancelState>,
}
impl Client {
    /// Spawns `command` (see [`super::command`]) in its own process group.
    pub fn start(command: Command, policy: AcpPermission) -> io::Result<Self> {
        let (process, stdin, stdout) = Process::spawn_piped(command)?;
        Ok(Self::from_io(stdout, stdin, Some(process), policy))
    }
    pub(crate) fn from_io(
        read: impl Read + Send + 'static,
        write: impl Write + Send + 'static,
        process: Option<Process>,
        policy: AcpPermission,
    ) -> Self {
        let (frame_tx, frames) = mpsc::channel();
        // Never joined: a grandchild holding stdout open must not block glen's exit.
        std::thread::spawn(move || read_frames(read, frame_tx));
        let (writer, lines) = mpsc::channel();
        std::thread::spawn(move || write_frames(write, lines));
        Self {
            writer: Some(writer),
            frames,
            process,
            next_id: 1,
            policy,
            updates: Updates::default(),
            events: vec![],
            events_dropped: 0,
            cancel: None,
        }
    }
    pub fn pid(&self) -> Option<u32> {
        self.process.as_ref().map(|p| p.child.id())
    }
    pub fn events(&self) -> &[Value] {
        &self.events
    }
    pub fn events_dropped(&self) -> u64 {
        self.events_dropped
    }
    pub fn cancel(&self) -> Option<&CancelState> {
        self.cancel.as_ref()
    }
    /// Queues one frame for the writer thread; false once the writer has gone.
    fn send(&self, frame: &Value) -> bool {
        let Some(writer) = &self.writer else {
            return false;
        };
        let Ok(mut bytes) = serde_json::to_vec(frame) else {
            return false;
        };
        bytes.push(b'\n');
        writer.send(bytes).is_ok()
    }
    /// Writes a request and returns its id (integers from 1).
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, CallError> {
        let id = json!(self.next_id);
        self.next_id += 1;
        if self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})) {
            Ok(id)
        } else {
            Err(CallError::Exited)
        }
    }
    pub fn notify(&mut self, method: &str, params: Value) -> bool {
        self.send(&json!({"jsonrpc":"2.0","method":method,"params":params}))
    }
    /// Processes at most one frame, waiting up to `wait` for it. A frame with `method` is a
    /// request (with `id`) or a notification (without); one with `result` or `error` and no
    /// `method` is a response, returned to the caller unmatched. `stop` and `deadline` are read
    /// when a request is answered: once either is reached, a cancel is due.
    pub fn pump(&mut self, wait: Duration, stop: &AtomicBool, deadline: Instant) -> Pump {
        let value = match self.frames.recv_timeout(wait) {
            Ok(Frame::Json(value)) => value,
            Ok(Frame::Malformed(reason)) => return Pump::Malformed(reason),
            Ok(Frame::Eof) | Err(RecvTimeoutError::Disconnected) => return Pump::Closed,
            Err(RecvTimeoutError::Timeout) => return Pump::Idle,
        };
        if let Some(method) = value.get("method") {
            let Some(method) = method.as_str() else {
                return Pump::Malformed("a frame's method is not a string".into());
            };
            match value.get("id") {
                Some(id) => {
                    let halted = stop.load(Ordering::SeqCst) || Instant::now() >= deadline;
                    self.answer_request(method, id.clone(), &value["params"], halted);
                }
                None if method == "session/update" => self.updates.apply(&value["params"]),
                // Unknown notifications (including `$/cancel_request`) are ignored.
                None => (),
            }
            return Pump::Handled;
        }
        let error = value.get("error").filter(|e| !e.is_null());
        if error.is_some() || value.get("result").is_some() {
            return Pump::Response {
                id: value.get("id").cloned().unwrap_or(Value::Null),
                result: match error {
                    Some(error) => Err(RpcError::from_value(error)),
                    None => Ok(value["result"].clone()),
                },
            };
        }
        Pump::Malformed("a frame is neither a request, a notification nor a response".into())
    }
    fn answer_request(&mut self, method: &str, id: Value, params: &Value, halted: bool) {
        let reply = if method == "session/request_permission" {
            // Once glen has asked to cancel, or is about to (`halted`: the stop or the deadline
            // came before the tick that writes the cancel), nothing is allowed any more.
            let answer = if halted || self.cancel.is_some() {
                Answer::Cancelled
            } else {
                let options = params["options"].as_array().map_or(&[][..], Vec::as_slice);
                super::answer(self.policy, options)
            };
            let event = super::input_required_event(params, self.policy, &answer);
            if self.events.len() < MAX_EVENTS {
                self.events.push(event);
            } else {
                self.events_dropped += 1;
            }
            json!({"jsonrpc":"2.0","id":id,"result":answer.result()})
        } else {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("Method not found: {method}")}})
        };
        // A gone writer shows up as EOF on a later pump.
        self.send(&reply);
    }
    /// One request, answered within `timeout` and `deadline`, interruptible every tick. A stop or
    /// a bound already reached writes nothing.
    pub fn call(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
        stop: &AtomicBool,
        deadline: Instant,
    ) -> Result<Value, CallError> {
        if stop.load(Ordering::SeqCst) {
            return Err(CallError::Interrupted);
        }
        let end = (Instant::now() + timeout).min(deadline);
        if end <= Instant::now() {
            return Err(CallError::Timeout);
        }
        let id = self.request(method, params)?;
        loop {
            if stop.load(Ordering::SeqCst) {
                return Err(CallError::Interrupted);
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(CallError::Timeout);
            }
            match self.pump(TICK.min(left), stop, deadline) {
                Pump::Response { id: got, result } if got == id => {
                    return result.map_err(CallError::Rpc);
                }
                Pump::Response { .. } | Pump::Handled | Pump::Idle => (),
                Pump::Closed => return Err(CallError::Exited),
                Pump::Malformed(reason) => return Err(CallError::Malformed(reason)),
            }
        }
    }
    /// One `session/prompt` turn. A stop or a deadline already reached starts no turn and writes
    /// nothing (`Interrupted`, `Timeout`). Otherwise, on the first tick that sees `stop` (or the
    /// deadline) it queues `session/cancel` once and waits at most `grace` more for the prompt
    /// response. When it returns, the turn's updates are frozen.
    pub fn prompt(
        &mut self,
        session_id: &str,
        text: &str,
        stop: &AtomicBool,
        deadline: Instant,
        grace: Duration,
    ) -> Result<PromptOutcome, CallError> {
        if stop.load(Ordering::SeqCst) {
            return Err(CallError::Interrupted);
        }
        if Instant::now() >= deadline {
            return Err(CallError::Timeout);
        }
        self.updates.begin_turn();
        let outcome = self
            .request(
                "session/prompt",
                json!({"sessionId":session_id,"prompt":[{"type":"text","text":text}]}),
            )
            .and_then(|id| self.turn(&id, session_id, stop, deadline, grace));
        self.updates.end_turn();
        outcome
    }
    fn turn(
        &mut self,
        id: &Value,
        session_id: &str,
        stop: &AtomicBool,
        deadline: Instant,
        grace: Duration,
    ) -> Result<PromptOutcome, CallError> {
        let mut grace_end = None;
        loop {
            let now = Instant::now();
            if self.cancel.is_none() {
                let reason = if stop.load(Ordering::SeqCst) {
                    Some(CancelReason::Interrupt)
                } else if now >= deadline {
                    Some(CancelReason::Deadline)
                } else {
                    None
                };
                if let Some(reason) = reason {
                    let sent = self.notify("session/cancel", json!({"sessionId":session_id}));
                    self.cancel = Some(CancelState {
                        sent,
                        confirmed: false,
                        reason,
                    });
                    grace_end = Some(now + grace);
                }
            }
            let left = grace_end.unwrap_or(deadline).saturating_duration_since(now);
            if grace_end.is_some() && left.is_zero() {
                return Ok(PromptOutcome {
                    stop_reason: None,
                    usage: None,
                    cancel: self.cancel.clone(),
                });
            }
            let result = match self.pump(TICK.min(left), stop, deadline) {
                Pump::Response { id: got, result } if got == *id => result.map_err(CallError::Rpc),
                Pump::Response { .. } | Pump::Handled | Pump::Idle => continue,
                Pump::Closed => Err(CallError::Exited),
                Pump::Malformed(reason) => Err(CallError::Malformed(reason)),
            };
            let outcome = result.and_then(|result| {
                let stop_reason = result
                    .get("stopReason")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(cancel) = &mut self.cancel {
                    cancel.confirmed = stop_reason.as_deref() == Some("cancelled");
                }
                self.drain(stop, deadline)?;
                Ok(PromptOutcome {
                    stop_reason,
                    usage: result.get("usage").filter(|u| u.is_object()).cloned(),
                    cancel: None,
                })
            });
            return outcome.map(|outcome| PromptOutcome {
                cancel: self.cancel.clone(),
                ..outcome
            });
        }
    }
    /// The trailing drain: an update that overtook the prompt response is still applied, for
    /// at most one tick. A malformed frame in it fails the turn like one before the response.
    fn drain(&mut self, stop: &AtomicBool, deadline: Instant) -> Result<(), CallError> {
        let end = Instant::now() + TICK;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(());
            }
            match self.pump(left, stop, deadline) {
                Pump::Handled | Pump::Response { .. } => (),
                Pump::Idle | Pump::Closed => return Ok(()),
                Pump::Malformed(reason) => return Err(CallError::Malformed(reason)),
            }
        }
    }
    /// Best-effort `session/close`, bounded by [`CLOSE_TIMEOUT`].
    pub fn close_session(&mut self, session_id: &str, stop: &AtomicBool) -> Result<(), CallError> {
        self.call(
            "session/close",
            json!({"sessionId":session_id}),
            CLOSE_TIMEOUT,
            stop,
            Instant::now() + CLOSE_TIMEOUT,
        )
        .map(drop)
    }
    /// Closes stdin (once the writer has drained) and stops the process group within `grace`
    /// plus the TERM and KILL escalation (`Process::finish`). Idempotent.
    pub fn shutdown(&mut self, grace: Duration) -> Vec<Warning> {
        self.writer.take();
        self.process
            .take()
            .map(|mut process| process.finish(grace))
            .unwrap_or_default()
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        drop(self.shutdown(Duration::ZERO));
    }
}
fn write_frames(mut write: impl Write, lines: Receiver<Vec<u8>>) {
    for line in lines {
        if write.write_all(&line).and_then(|()| write.flush()).is_err() {
            return;
        }
    }
    // Dropping `write` here is stdin EOF.
}
fn read_frames(read: impl Read, frames: Sender<Frame>) {
    let mut reader = BufReader::with_capacity(64 * 1024, read);
    loop {
        let frame = match read_line(&mut reader, MAX_FRAME_BYTES) {
            Ok(Some(Ok(line))) => match classify(&line) {
                Some(frame) => frame,
                None => continue,
            },
            Ok(Some(Err(reason))) => Frame::Malformed(reason),
            Ok(None) | Err(_) => Frame::Eof,
        };
        // A malformed frame ends the conversation; glen stops reading there.
        let last = !matches!(frame, Frame::Json(_));
        if frames.send(frame).is_err() || last {
            return;
        }
    }
}
/// One newline-terminated line of at most `max` bytes (newline excluded); `None` at a clean EOF,
/// and a final unterminated line is returned as a line (omp's `ndJsonStream` does the same).
fn read_line(reader: &mut impl BufRead, max: usize) -> io::Result<Option<Result<Vec<u8>, String>>> {
    let mut line = Vec::new();
    loop {
        let buf = match reader.fill_buf() {
            Ok(buf) => buf,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if buf.is_empty() {
            return Ok((!line.is_empty()).then_some(Ok(line)));
        }
        let newline = buf.iter().position(|b| *b == b'\n');
        let chunk = newline.unwrap_or(buf.len());
        if line.len() + chunk > max {
            return Ok(Some(Err(format!("a frame is larger than {max} bytes"))));
        }
        line.extend_from_slice(&buf[..chunk]);
        reader.consume(newline.map_or(chunk, |i| i + 1));
        if newline.is_some() {
            return Ok(Some(Ok(line)));
        }
    }
}
/// `None` for a blank line (skipped, like omp); otherwise a JSON-RPC 2.0 object or the reason it
/// is malformed.
fn classify(line: &[u8]) -> Option<Frame> {
    let Ok(text) = std::str::from_utf8(line) else {
        return Some(Frame::Malformed("a frame is not valid UTF-8".into()));
    };
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(match serde_json::from_str::<Value>(text) {
        Err(e) => Frame::Malformed(format!("a frame is not JSON: {e}")),
        Ok(value) if !value.is_object() => Frame::Malformed("a frame is not a JSON object".into()),
        Ok(value) if value["jsonrpc"] != "2.0" => {
            Frame::Malformed("a frame lacks \"jsonrpc\":\"2.0\"".into())
        }
        Ok(value) => Frame::Json(value),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{PipeReader, PipeWriter},
        sync::Arc,
        thread,
    };
    const WAIT: Duration = Duration::from_secs(5);
    /// The agent side of a client: frames the client wrote, and a writer for the agent's frames.
    struct Peer {
        inbox: Receiver<Value>,
        out: PipeWriter,
    }
    impl Peer {
        fn send(&mut self, frame: Value) {
            self.raw(format!("{frame}\n").as_bytes());
        }
        fn raw(&mut self, bytes: &[u8]) {
            self.out.write_all(bytes).unwrap();
            self.out.flush().unwrap();
        }
        fn next(&self) -> Value {
            self.inbox
                .recv_timeout(WAIT)
                .expect("no frame from the client")
        }
        fn quiet(&self) -> bool {
            self.inbox.recv_timeout(Duration::from_millis(200)).is_err()
        }
    }
    fn pair(policy: AcpPermission) -> (Client, Peer) {
        let (client_read, agent_write) = std::io::pipe().unwrap();
        let (agent_read, client_write) = std::io::pipe().unwrap();
        let client = Client::from_io(client_read, client_write, None, policy);
        let (tx, inbox) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(agent_read).lines() {
                let Ok(line) = line else { return };
                if tx.send(serde_json::from_str(&line).unwrap()).is_err() {
                    return;
                }
            }
        });
        (
            client,
            Peer {
                inbox,
                out: agent_write,
            },
        )
    }
    fn far() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }
    /// One pump with no stop and no deadline in sight.
    fn pump(client: &mut Client, wait: Duration) -> Pump {
        client.pump(wait, &AtomicBool::new(false), far())
    }
    fn response(id: Value, result: Value) -> Value {
        json!({"jsonrpc":"2.0","id":id,"result":result})
    }
    fn chunk(session: &str, text: &str, id: &str) -> Value {
        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text},"messageId":id}}})
    }
    fn permission(id: Value, kinds: &[&str]) -> Value {
        let options: Vec<_> = kinds
            .iter()
            .map(|k| json!({"optionId":k,"name":k,"kind":k}))
            .collect();
        json!({"jsonrpc":"2.0","id":id,"method":"session/request_permission","params":{"sessionId":"s1","toolCall":{"toolCallId":"t1","title":"rm x","kind":"execute","status":"pending"},"options":options}})
    }
    const ALL: [&str; 4] = ["allow_once", "allow_always", "reject_once", "reject_always"];
    #[test]
    fn calls_correlate_ids_across_interleaved_notifications() {
        let (mut client, mut peer) = pair(AcpPermission::Deny);
        client.updates.for_session("s1");
        peer.send(chunk("s1", "replayed", "r1"));
        peer.send(json!({"jsonrpc":"2.0","method":"$/cancel_request","params":{"requestId":3}}));
        peer.send(json!({"jsonrpc":"2.0","method":"_omp/unknown"}));
        peer.send(response(json!(1), json!({"protocolVersion":1})));
        let got = client
            .call(
                "initialize",
                json!({}),
                WAIT,
                &AtomicBool::new(false),
                far(),
            )
            .unwrap();
        assert_eq!(got, json!({"protocolVersion":1}));
        let sent = peer.next();
        assert_eq!(sent["jsonrpc"], "2.0");
        assert_eq!(sent["id"], 1);
        assert_eq!(sent["method"], "initialize");
        assert_eq!(client.updates.replayed, 1);
        assert!(peer.quiet(), "notifications are never answered");
    }
    #[test]
    fn a_late_response_is_ignored() {
        let (mut client, mut peer) = pair(AcpPermission::Deny);
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        assert_eq!(
            client.call("a", json!({}), Duration::from_millis(150), &stop, far()),
            Err(CallError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        peer.send(response(json!(1), json!("late")));
        peer.send(response(json!(2), json!("mine")));
        assert_eq!(
            client.call("b", json!({}), WAIT, &stop, far()),
            Ok(json!("mine"))
        );
    }
    #[test]
    fn a_response_matches_only_by_exact_id_type_and_value() {
        let (mut client, mut peer) = pair(AcpPermission::Deny);
        // An agent request that reuses the outstanding id is answered, never taken as the reply.
        peer.send(json!({"jsonrpc":"2.0","id":1,"method":"fs/read_text_file","params":{"sessionId":"s1","path":"/x"}}));
        peer.send(response(json!("1"), json!("string id")));
        peer.send(response(json!(1.0), json!("float id")));
        peer.send(response(json!(1), json!("integer id")));
        let got = client.call("x", json!({}), WAIT, &AtomicBool::new(false), far());
        assert_eq!(got, Ok(json!("integer id")));
        assert_eq!(peer.next()["method"], "x");
        let answered = peer.next();
        assert_eq!(answered["id"], 1);
        assert_eq!(answered["error"]["code"], -32601);
        assert!(answered.get("result").is_none());
    }
    #[test]
    fn errors_are_rpc_errors_with_the_details_text() {
        let (mut client, mut peer) = pair(AcpPermission::Deny);
        peer.send(json!({"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"Internal error","data":{"details":"ACP session not found: s9"}}}));
        assert_eq!(
            client.call(
                "session/load",
                json!({}),
                WAIT,
                &AtomicBool::new(false),
                far()
            ),
            Err(CallError::Rpc(RpcError {
                code: json!(-32603),
                message: "ACP session not found: s9".into()
            }))
        );
    }
    #[test]
    fn frames_up_to_the_cap_are_read_and_one_byte_over_is_malformed() {
        let mut at_cap = vec![b'x'; 16];
        at_cap.push(b'\n');
        let mut reader = io::Cursor::new(at_cap);
        assert_eq!(
            read_line(&mut reader, 16).unwrap(),
            Some(Ok(vec![b'x'; 16]))
        );
        assert_eq!(read_line(&mut reader, 16).unwrap(), None);
        let mut reader = io::Cursor::new(vec![b'x'; 17]);
        assert!(matches!(read_line(&mut reader, 16).unwrap(), Some(Err(_))));
        // A final unterminated line is still a line.
        let mut reader = io::Cursor::new(b"{}".to_vec());
        assert_eq!(
            read_line(&mut reader, 16).unwrap(),
            Some(Ok(b"{}".to_vec()))
        );
        // The real cap, through the reader thread: a valid frame of exactly 16 MiB passes.
        let pad = MAX_FRAME_BYTES - r#"{"jsonrpc":"2.0","method":"x","params":""}"#.len();
        let exact = format!(
            r#"{{"jsonrpc":"2.0","method":"x","params":"{}"}}"#,
            "a".repeat(pad)
        );
        assert_eq!(exact.len(), MAX_FRAME_BYTES);
        for (line, ok) in [(exact.clone(), true), (format!("{exact} "), false)] {
            let (tx, rx) = mpsc::channel();
            let mut bytes = line.into_bytes();
            bytes.push(b'\n');
            read_frames(io::Cursor::new(bytes), tx);
            let first = rx.recv().unwrap();
            assert_eq!(matches!(first, Frame::Json(_)), ok, "{first:?}");
            assert_eq!(matches!(first, Frame::Malformed(_)), !ok);
        }
    }
    #[test]
    fn malformed_frames_end_the_call() {
        for (bytes, what) in [
            (&b"\xff\xfe{}\n"[..], "UTF-8"),
            (&b"not json\n"[..], "JSON"),
            (
                &b"{\"jsonrpc\":\"1.0\",\"id\":1,\"result\":{}}\n"[..],
                "jsonrpc",
            ),
            (&b"{\"id\":1,\"result\":{}}\n"[..], "jsonrpc"),
            (&b"[1,2]\n"[..], "object"),
            (&b"{\"jsonrpc\":\"2.0\",\"id\":1}\n"[..], "neither"),
            (&b"{\"jsonrpc\":\"2.0\",\"method\":7}\n"[..], "method"),
        ] {
            let (mut client, mut peer) = pair(AcpPermission::Deny);
            peer.raw(b"\n  \n");
            peer.raw(bytes);
            match client.call("x", json!({}), WAIT, &AtomicBool::new(false), far()) {
                Err(CallError::Malformed(reason)) => assert!(reason.contains(what), "{reason}"),
                other => panic!("{what}: {other:?}"),
            }
        }
    }
    #[test]
    fn eof_mid_request_is_exited() {
        let (mut client, peer) = pair(AcpPermission::Deny);
        let Peer { inbox, out } = peer;
        drop(out);
        assert_eq!(
            client.call("x", json!({}), WAIT, &AtomicBool::new(false), far()),
            Err(CallError::Exited)
        );
        drop(inbox);
    }
    #[test]
    fn an_interrupted_call_writes_nothing_or_stops_within_a_tick() {
        let (mut client, peer) = pair(AcpPermission::Deny);
        let stop = Arc::new(AtomicBool::new(true));
        assert_eq!(
            client.call("x", json!({}), WAIT, &stop, far()),
            Err(CallError::Interrupted)
        );
        assert!(peer.quiet());
        stop.store(false, Ordering::SeqCst);
        let flag = Arc::clone(&stop);
        let setter = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            flag.store(true, Ordering::SeqCst);
            Instant::now()
        });
        assert_eq!(
            client.call("y", json!({}), WAIT, &stop, far()),
            Err(CallError::Interrupted)
        );
        let set_at = setter.join().unwrap();
        assert!(set_at.elapsed() < TICK + Duration::from_millis(150));
    }
    #[test]
    fn a_call_past_the_deadline_writes_nothing() {
        let (mut client, peer) = pair(AcpPermission::Deny);
        let stop = AtomicBool::new(false);
        assert_eq!(
            client.call("x", json!({}), WAIT, &stop, Instant::now()),
            Err(CallError::Timeout)
        );
        assert_eq!(
            client.call("y", json!({}), Duration::ZERO, &stop, far()),
            Err(CallError::Timeout)
        );
        assert!(peer.quiet(), "no request was written");
    }
    #[test]
    fn permission_requests_follow_the_policy() {
        for (policy, kinds, want) in [
            (
                AcpPermission::Allow,
                &ALL[..],
                json!({"outcome":"selected","optionId":"allow_once"}),
            ),
            (
                AcpPermission::Deny,
                &ALL[..],
                json!({"outcome":"selected","optionId":"reject_once"}),
            ),
            (
                AcpPermission::Allow,
                &["reject_once", "reject_always"][..],
                json!({"outcome":"cancelled"}),
            ),
            (
                AcpPermission::Deny,
                &["allow_once"][..],
                json!({"outcome":"cancelled"}),
            ),
        ] {
            let (mut client, mut peer) = pair(policy);
            peer.send(permission(json!("p-7"), kinds));
            assert!(matches!(pump(&mut client, WAIT), Pump::Handled));
            let answered = peer.next();
            assert_eq!(answered["id"], "p-7");
            assert_eq!(answered["result"]["outcome"], want, "{policy:?} {kinds:?}");
            let event = &client.events()[0];
            assert_eq!(event["task_state"], "input_required");
            assert_eq!(event["policy"], policy.as_str());
            assert_eq!(event["tool_call_id"], "t1");
        }
    }
    #[test]
    fn events_are_capped() {
        let (mut client, mut peer) = pair(AcpPermission::Allow);
        for i in 0..(MAX_EVENTS + 3) {
            peer.send(permission(json!(i), &ALL));
            assert!(matches!(pump(&mut client, WAIT), Pump::Handled));
        }
        assert_eq!(client.events().len(), MAX_EVENTS);
        assert_eq!(client.events_dropped(), 3);
    }
    #[test]
    fn unknown_agent_requests_get_method_not_found_and_the_loop_continues() {
        let (mut client, mut peer) = pair(AcpPermission::Deny);
        for (id, method) in [
            (json!(101), "fs/read_text_file"),
            (json!("t"), "terminal/create"),
            (json!(null), "elicitation/create"),
        ] {
            peer.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":{}}));
        }
        peer.send(response(json!(1), json!({"ok":true})));
        assert_eq!(
            client.call("x", json!({}), WAIT, &AtomicBool::new(false), far()),
            Ok(json!({"ok":true}))
        );
        assert_eq!(peer.next()["method"], "x");
        for id in [json!(101), json!("t"), json!(null)] {
            let answered = peer.next();
            assert_eq!(answered["id"], id);
            assert_eq!(answered["error"]["code"], -32601);
        }
    }
    #[test]
    fn begin_turn_separates_replay_from_the_turn_and_the_drain_is_one_tick() {
        let (mut client, mut peer) = pair(AcpPermission::Deny);
        client.updates.for_session("s1");
        peer.send(chunk("s1", "REPLAYED ANSWER", "replay-1"));
        assert!(matches!(pump(&mut client, WAIT), Pump::Handled));
        peer.send(chunk("s1", "first", "m1"));
        peer.send(response(
            json!(1),
            json!({"stopReason":"end_turn","usage":{"totalTokens":3}}),
        ));
        // Overtakes nothing but lands right after the response: inside the drain.
        peer.send(chunk("s1", "after", "m2"));
        let Peer { inbox, mut out } = peer;
        let (returned, has_returned) = mpsc::channel();
        // Written only once `prompt()` has returned, so outside the drain by construction.
        let late = thread::spawn(move || {
            has_returned.recv_timeout(WAIT).unwrap();
            writeln!(out, "{}", chunk("s1", "too late", "m3")).unwrap();
            out
        });
        let outcome = client
            .prompt("s1", "hi", &AtomicBool::new(false), far(), WAIT)
            .unwrap();
        returned.send(()).unwrap();
        assert_eq!(outcome.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(outcome.usage, Some(json!({"totalTokens":3})));
        assert_eq!(outcome.cancel, None);
        let _out = late.join().unwrap();
        // The late chunk is still read, but the turn is over: it changes nothing.
        assert!(matches!(pump(&mut client, WAIT), Pump::Handled));
        assert_eq!(client.updates.assistant_text(), Some("after"));
        assert_eq!(client.updates.replayed, 1);
        assert_eq!(client.updates.during_turn, 2);
        let prompt = inbox.recv_timeout(WAIT).unwrap();
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(
            prompt["params"],
            json!({"sessionId":"s1","prompt":[{"type":"text","text":"hi"}]})
        );
    }
    /// Runs a prompt on its own thread with an agent that streams one chunk, then reports when
    /// `session/cancel` arrives and how many arrived.
    fn cancel_run(
        respond: Option<Value>,
        grace: Duration,
        permission_after_cancel: bool,
    ) -> (PromptOutcome, Duration, usize, Client) {
        let (mut client, peer) = pair(AcpPermission::Allow);
        client.updates.for_session("s1");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let Peer { inbox, mut out } = peer;
        let agent = thread::spawn(move || {
            let prompt = inbox.recv_timeout(WAIT).unwrap();
            writeln!(out, "{}", chunk("s1", "partial", "m1")).unwrap();
            thread::sleep(Duration::from_millis(150));
            let signalled = Instant::now();
            flag.store(true, Ordering::SeqCst);
            let cancel = inbox.recv_timeout(WAIT).unwrap();
            let latency = signalled.elapsed();
            assert_eq!(cancel["method"], "session/cancel");
            assert_eq!(cancel["params"], json!({"sessionId":"s1"}));
            assert!(cancel.get("id").is_none());
            if permission_after_cancel {
                writeln!(out, "{}", permission(json!(100), &ALL)).unwrap();
                let answered = inbox.recv_timeout(WAIT).unwrap();
                assert_eq!(
                    answered["result"],
                    json!({"outcome":{"outcome":"cancelled"}})
                );
            }
            // Let several ticks pass: the cancel must not be written again.
            thread::sleep(Duration::from_millis(350));
            if let Some(result) = respond {
                writeln!(out, "{}", response(prompt["id"].clone(), result)).unwrap();
            }
            let more = inbox
                .recv_timeout(Duration::from_millis(300))
                .into_iter()
                .count();
            (latency, 1 + more, out)
        });
        let outcome = client.prompt("s1", "hi", &stop, far(), grace).unwrap();
        let (latency, cancels, _out) = agent.join().unwrap();
        (outcome, latency, cancels, client)
    }
    #[test]
    fn a_stop_writes_one_cancel_within_a_tick_and_waits_for_the_confirmation() {
        let (outcome, latency, cancels, client) =
            cancel_run(Some(json!({"stopReason":"cancelled"})), WAIT, false);
        // The AC bound: the cancel is on the wire within 250 ms of the signal.
        assert!(latency < Duration::from_millis(250), "{latency:?}");
        assert_eq!(cancels, 1);
        assert_eq!(outcome.stop_reason.as_deref(), Some("cancelled"));
        let cancel = outcome.cancel.unwrap();
        assert_eq!(
            cancel,
            CancelState {
                sent: true,
                confirmed: true,
                reason: CancelReason::Interrupt
            }
        );
        assert_eq!(
            cancel.json(),
            json!({"sent":true,"confirmed":true,"reason":"interrupt"})
        );
        assert_eq!(client.updates.assistant_text(), Some("partial"));
    }
    #[test]
    fn an_unconfirmed_cancel_ends_after_the_grace() {
        let started = Instant::now();
        let (outcome, _, cancels, _) = cancel_run(None, Duration::from_millis(300), false);
        assert_eq!(cancels, 1);
        assert_eq!(outcome.stop_reason, None);
        assert_eq!(
            outcome.cancel,
            Some(CancelState {
                sent: true,
                confirmed: false,
                reason: CancelReason::Interrupt
            })
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn a_permission_request_after_the_cancel_is_answered_cancelled_under_allow() {
        let (outcome, _, _, client) =
            cancel_run(Some(json!({"stopReason":"cancelled"})), WAIT, true);
        assert!(outcome.cancel.unwrap().confirmed);
        assert_eq!(client.events()[0]["answer"], "cancelled");
        assert_eq!(client.events()[0]["policy"], "allow");
        assert_eq!(client.events()[0]["option_id"], Value::Null);
    }
    #[test]
    fn the_deadline_cancels_like_a_signal() {
        let (mut client, peer) = pair(AcpPermission::Deny);
        let deadline = Instant::now() + Duration::from_millis(200);
        let outcome = client
            .prompt(
                "s1",
                "hi",
                &AtomicBool::new(false),
                deadline,
                Duration::from_millis(200),
            )
            .unwrap();
        assert_eq!(
            outcome.cancel,
            Some(CancelState {
                sent: true,
                confirmed: false,
                reason: CancelReason::Deadline
            })
        );
        assert_eq!(peer.next()["method"], "session/prompt");
        assert_eq!(peer.next()["method"], "session/cancel");
    }
    #[test]
    fn a_blocked_stdin_write_does_not_delay_the_tick() {
        let (client_read, _agent_write) = std::io::pipe().unwrap();
        // Nobody reads the agent's end, so the pipe fills and the writer thread blocks.
        let (_agent_read, client_write): (PipeReader, PipeWriter) = std::io::pipe().unwrap();
        let mut client = Client::from_io(client_read, client_write, None, AcpPermission::Deny);
        let big = "x".repeat(4 * 1024 * 1024);
        client
            .request("session/prompt", json!({"text":big}))
            .unwrap();
        thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        assert!(client.notify("session/cancel", json!({"sessionId":"s1"})));
        assert!(matches!(pump(&mut client, TICK), Pump::Idle));
        assert!(started.elapsed() < TICK + Duration::from_millis(150));
    }
    #[test]
    fn a_stop_or_an_expired_deadline_before_the_turn_writes_nothing() {
        let (mut client, peer) = pair(AcpPermission::Deny);
        let grace = Duration::from_millis(300);
        let stopped = client.prompt("s1", "hi", &AtomicBool::new(true), far(), grace);
        assert!(
            matches!(stopped, Err(CallError::Interrupted)),
            "{stopped:?}"
        );
        let expired = client.prompt("s1", "hi", &AtomicBool::new(false), Instant::now(), grace);
        assert!(matches!(expired, Err(CallError::Timeout)), "{expired:?}");
        assert!(
            peer.quiet(),
            "neither session/prompt nor session/cancel was written"
        );
        assert_eq!(client.cancel(), None);
    }
    #[test]
    fn a_permission_request_that_lands_with_the_stop_is_answered_cancelled_under_allow() {
        let (mut client, peer) = pair(AcpPermission::Allow);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let Peer { inbox, mut out } = peer;
        let agent = thread::spawn(move || {
            let prompt = inbox.recv_timeout(WAIT).unwrap();
            // The client is inside a tick wait: the signal and the request land in it together,
            // before the next tick could write the cancel.
            thread::sleep(Duration::from_millis(30));
            flag.store(true, Ordering::SeqCst);
            writeln!(out, "{}", permission(json!(100), &ALL)).unwrap();
            let answer = loop {
                let frame = inbox.recv_timeout(WAIT).unwrap();
                if frame["id"] == 100 {
                    break frame;
                }
            };
            writeln!(
                out,
                "{}",
                response(prompt["id"].clone(), json!({"stopReason":"cancelled"}))
            )
            .unwrap();
            (answer, out)
        });
        let outcome = client.prompt("s1", "hi", &stop, far(), WAIT).unwrap();
        let (answer, _out) = agent.join().unwrap();
        assert_eq!(answer["result"], json!({"outcome":{"outcome":"cancelled"}}));
        assert_eq!(client.events()[0]["answer"], "cancelled");
        assert!(outcome.cancel.is_some());
    }
    #[test]
    fn a_queued_permission_request_is_answered_cancelled_once_stop_or_deadline_is_reached() {
        let (mut client, mut peer) = pair(AcpPermission::Allow);
        let cancelled = json!({"outcome":{"outcome":"cancelled"}});
        // Already queued, with no cancel written yet (`client.cancel()` is `None`).
        peer.send(permission(json!(1), &ALL));
        peer.send(permission(json!(2), &ALL));
        let stop = AtomicBool::new(true);
        assert!(matches!(client.pump(WAIT, &stop, far()), Pump::Handled));
        assert_eq!(peer.next()["result"], cancelled);
        let passed = Instant::now();
        let running = AtomicBool::new(false);
        assert!(matches!(client.pump(WAIT, &running, passed), Pump::Handled));
        assert_eq!(peer.next()["result"], cancelled);
        assert_eq!(client.cancel(), None);
        assert!(client.events().iter().all(|e| e["answer"] == "cancelled"));
        // Neither reached: the policy answers.
        peer.send(permission(json!(3), &ALL));
        assert!(matches!(pump(&mut client, WAIT), Pump::Handled));
        assert_eq!(peer.next()["result"]["outcome"]["optionId"], "allow_once");
    }
    #[test]
    fn a_malformed_frame_in_the_trailing_drain_fails_the_turn() {
        let (mut client, mut peer) = pair(AcpPermission::Deny);
        client.updates.for_session("s1");
        peer.send(chunk("s1", "answer", "m1"));
        peer.send(response(json!(1), json!({"stopReason":"end_turn"})));
        peer.raw(b"not json\n");
        match client.prompt("s1", "hi", &AtomicBool::new(false), far(), WAIT) {
            Err(CallError::Malformed(reason)) => assert!(reason.contains("JSON"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }
    #[test]
    fn updates_during_session_close_do_not_change_the_turn() {
        let (mut client, peer) = pair(AcpPermission::Deny);
        client.updates.for_session("s1");
        let Peer { inbox, mut out } = peer;
        let agent = thread::spawn(move || {
            let prompt = inbox.recv_timeout(WAIT).unwrap();
            writeln!(out, "{}", chunk("s1", "answer", "m1")).unwrap();
            writeln!(
                out,
                "{}",
                response(prompt["id"].clone(), json!({"stopReason":"end_turn"}))
            )
            .unwrap();
            let close = inbox.recv_timeout(WAIT).unwrap();
            assert_eq!(close["method"], "session/close");
            // After session/close arrives and before its response: a late chunk, and a request
            // that must still be answered.
            writeln!(out, "{}", chunk("s1", "late", "m2")).unwrap();
            writeln!(
                out,
                "{}",
                json!({"jsonrpc":"2.0","id":"r1","method":"fs/read_text_file","params":{}})
            )
            .unwrap();
            writeln!(out, "{}", response(close["id"].clone(), json!({}))).unwrap();
            let answered = inbox.recv_timeout(WAIT).unwrap();
            (answered, out)
        });
        let stop = AtomicBool::new(false);
        client.prompt("s1", "hi", &stop, far(), WAIT).unwrap();
        client.close_session("s1", &stop).unwrap();
        let (answered, _out) = agent.join().unwrap();
        assert_eq!(answered["id"], "r1");
        assert_eq!(answered["error"]["code"], -32601);
        assert_eq!(client.updates.assistant_text(), Some("answer"));
        assert_eq!(client.updates.during_turn, 1);
        assert_eq!(client.updates.messages(), 1);
        assert_eq!(client.updates.replayed, 0);
        assert_eq!(
            client.updates.json()["kinds"],
            json!({"agent_message_chunk":1})
        );
    }
}
