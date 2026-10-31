//! Foreground, lock-owning RPC clients. Only omp writes the resumed transcript.
use crate::{
    error::{ErrorCode, GlenError, Warning},
    glen_home::GlenHome,
    jobs::{
        Forked,
        lock::{self, SessionLock},
    },
    omp::{
        continuation,
        lease::{self, Lease},
        paths::OmpPaths,
        session::Session,
    },
    ops::{self, Op, OpError},
    rpc,
};
use omp_rpc::{Client, ClientOptions, Event, SessionState};
use serde_json::Value;
use std::{
    io::{self, Read},
    path::Path,
    process::{Child, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    time::{Duration, Instant},
};
// Glen owns the child before the blocking RPC handshake starts. A startup error or
// cancellation drops this guard before the session lock can leave spawn's scope.
struct Process {
    child: Child,
    stderr: Option<std::thread::JoinHandle<io::Result<Vec<Warning>>>>,
    reaped: bool,
}
impl Process {
    fn stop(&mut self, grace: Duration) -> io::Result<()> {
        if self.reaped {
            return Ok(());
        }
        let pid = self.child.id();
        let end = Instant::now() + grace;
        loop {
            if self.child.try_wait()?.is_some() {
                let group = rustix::process::Pid::from_raw(pid as i32)
                    .ok_or_else(|| io::Error::other("invalid omp pid"))?;
                match rustix::process::test_kill_process_group(group) {
                    Err(rustix::io::Errno::SRCH) => {
                        self.reaped = true;
                        return Ok(());
                    }
                    Ok(()) => (),
                    Err(error) => return Err(error.into()),
                }
            }
            if Instant::now() >= end {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        for signal in [rustix::process::Signal::TERM, rustix::process::Signal::KILL] {
            rpc::kill_group(pid, signal)?;
            let end = Instant::now() + Duration::from_secs(1);
            loop {
                let exited = self.child.try_wait()?.is_some();
                let group = rustix::process::Pid::from_raw(pid as i32)
                    .ok_or_else(|| io::Error::other("invalid omp pid"))?;
                match rustix::process::test_kill_process_group(group) {
                    Err(rustix::io::Errno::SRCH) if exited => {
                        self.reaped = true;
                        return Ok(());
                    }
                    Ok(()) | Err(rustix::io::Errno::SRCH) => (),
                    Err(error) => return Err(error.into()),
                }
                if Instant::now() >= end {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        Err(io::Error::other("omp process group survived SIGKILL"))
    }
    fn finish(&mut self, grace: Duration) -> Vec<Warning> {
        let mut warnings = vec![];
        if let Err(error) = self.stop(grace) {
            warnings.push(Warning {
                code: "teardown_error".into(),
                message: error.to_string(),
            });
        }
        if let Some(reader) = self.stderr.take() {
            let end = Instant::now() + Duration::from_secs(1);
            while !reader.is_finished() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(10));
            }
            let result = if reader.is_finished() {
                reader
                    .join()
                    .unwrap_or_else(|_| Err(io::Error::other("omp stderr reader panicked")))
            } else {
                Err(io::Error::other("omp stderr did not close after teardown"))
            };
            match result {
                Ok(lines) => warnings.extend(lines),
                Err(error) => warnings.push(Warning {
                    code: "teardown_error".into(),
                    message: error.to_string(),
                }),
            }
        }
        warnings
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        drop(self.finish(Duration::ZERO));
    }
}
fn drain_stderr(mut stderr: impl Read) -> io::Result<Vec<Warning>> {
    // Keep memory bounded even for a noisy child or an unterminated line, but
    // continue draining after the capture budget so the child cannot block.
    const MAX_LINES: usize = 16;
    const MAX_LINE_BYTES: usize = 1024;
    let mut warnings = vec![];
    let mut line = vec![];
    let mut buffer = [0; 4096];
    loop {
        let count = match stderr.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        for byte in &buffer[..count] {
            if *byte == b'\n' {
                if !line.is_empty() && warnings.len() < MAX_LINES {
                    warnings.push(Warning {
                        code: "omp_stderr".into(),
                        message: String::from_utf8_lossy(&line).trim().into(),
                    });
                }
                line.clear();
            } else if line.len() < MAX_LINE_BYTES && warnings.len() < MAX_LINES {
                line.push(*byte);
            }
        }
        if count == 0 {
            if !line.is_empty() && warnings.len() < MAX_LINES {
                warnings.push(Warning {
                    code: "omp_stderr".into(),
                    message: String::from_utf8_lossy(&line).trim().into(),
                });
            }
            return Ok(warnings);
        }
    }
}
pub(crate) enum OpenError {
    Busy,
    Reroute,
    Failed(GlenError),
    Stopped(OpError),
}
impl From<GlenError> for OpenError {
    fn from(e: GlenError) -> Self {
        Self::Failed(e)
    }
}
pub struct Ephemeral {
    client: Option<Arc<Client>>,
    process: Process,
    events: Receiver<Event>,
    lock: Option<SessionLock>,
    baseline: Box<SessionState>,
    deadline: Instant,
    stop: Arc<AtomicBool>,
    warnings: Vec<Warning>,
    applied_session: Option<String>,
}
pub(crate) fn stopped(deadline: Instant, stop: &AtomicBool) -> Result<(), OpError> {
    if stop.load(Ordering::SeqCst) {
        Err(OpError::new(
            "interrupted",
            "the glen command was interrupted",
        ))
    } else if Instant::now() >= deadline {
        Err(OpError::new(
            ops::TIMEOUT,
            "the glen command deadline passed",
        ))
    } else {
        Ok(())
    }
}
impl Ephemeral {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_session(
        home: &GlenHome,
        paths: &OmpPaths,
        session: &Session,
        profile: Option<&str>,
        bin: &Path,
        cwd: &Path,
        deadline: Instant,
        stop: Arc<AtomicBool>,
        env: &crate::env::Env,
        command: &str,
    ) -> Result<Self, OpenError> {
        stopped(deadline, &stop).map_err(OpenError::Stopped)?;
        let id = &session.header.id;
        let Some(lock) = lock::try_acquire(home, id)
            .map_err(|e| GlenError::new(ErrorCode::Io, format!("cannot lock session {id}: {e}")))?
        else {
            return Err(OpenError::Busy);
        };
        let mut warnings = vec![];
        let holder = lock::Holder {
            v: 1,
            kind: lock::HolderKind::Control,
            job_id: None,
            pid: std::process::id(),
            start: crate::jobs::liveness::current().map(|i| i.start),
        };
        if let Err(e) = lock::mark(&lock, &holder) {
            warnings.push(Warning {
                code: "holder_unavailable".into(),
                message: format!("cannot mark control holder for session {id}: {e}"),
            });
        }
        let followed = continuation::follow(&session.path)?;
        let current = Session::load(&followed.path).map_err(|e| {
            GlenError::new(
                ErrorCode::Io,
                format!("cannot re-read session {}: {e}", followed.path.display()),
            )
        })?;
        if current.header.id != *id {
            return Err(OpenError::Failed(moved(id, &current.header.id)));
        }
        match crate::live::route::find(
            home,
            id,
            crate::live::HELLO_PROBE.min(deadline.saturating_duration_since(Instant::now())),
        ) {
            crate::live::route::Endpoint::Live { .. }
            | crate::live::route::Endpoint::Unhealthy { .. } => return Err(OpenError::Reroute),
            _ => (),
        }
        let probe = lease::probe(&paths.session_owners_dir, id);
        if let Some(e) = crate::sandbox::map_probe_failure(env, command, &probe) {
            return Err(OpenError::Failed(e));
        }
        if !matches!(probe, Lease::Free) {
            return Err(OpenError::Failed(crate::live::route::lease_refusal(
                &probe, id, paths,
            )));
        }
        let mut command = rpc::base_command(bin, profile, cwd);
        command.arg("--resume").arg(&followed.path);
        let mut opened = Self::spawn(command, Some(lock), deadline, stop)?;
        opened.warnings.extend(warnings);
        if opened.baseline.session_id != *id {
            return Err(OpenError::Failed(moved(id, &opened.baseline.session_id)));
        }
        Ok(opened)
    }
    pub(crate) fn open_listing(
        profile: Option<&str>,
        bin: &Path,
        cwd: &Path,
        deadline: Instant,
        stop: Arc<AtomicBool>,
    ) -> Result<Self, OpenError> {
        let mut command = rpc::base_command(bin, profile, cwd);
        command.arg("--no-session");
        Self::spawn(command, None, deadline, stop)
    }
    fn spawn(
        mut command: std::process::Command,
        lock: Option<SessionLock>,
        deadline: Instant,
        stop: Arc<AtomicBool>,
    ) -> Result<Self, OpenError> {
        stopped(deadline, &stop).map_err(OpenError::Stopped)?;
        let ready_deadline = (Instant::now() + rpc::READY_TIMEOUT).min(deadline);
        // Client::spawn hides the child until ready and therefore cannot be
        // cancelled safely. Own it here and let from_io handle only the protocol.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| OpenError::Failed(start_error(error, vec![])))?;
        let mut process = Process {
            child,
            stderr: None,
            reaped: false,
        };
        let (Some(stdin), Some(stdout), Some(stderr)) = (
            process.child.stdin.take(),
            process.child.stdout.take(),
            process.child.stderr.take(),
        ) else {
            let warnings = process.finish(Duration::from_millis(100));
            return Err(OpenError::Failed(start_error("missing omp pipe", warnings)));
        };
        process.stderr = Some(std::thread::spawn(move || drain_stderr(stderr)));
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = Client::from_io(
                stdout,
                stdin,
                ClientOptions {
                    tools: vec![],
                    uris: vec![],
                    ready_timeout: ready_deadline.saturating_duration_since(Instant::now()),
                    default_timeout: rpc::CALL_TIMEOUT,
                },
            )
            .and_then(|(client, events)| {
                let baseline = client.call(&omp_rpc::GetStateCommand {})?;
                Ok((Arc::new(client), events, baseline))
            });
            if let Err(late) = tx.send(result) {
                drop(late);
            }
        });
        loop {
            if let Err(error) = stopped(deadline, &stop) {
                let warnings = process.finish(Duration::from_millis(100));
                return Err(startup_stopped(error, warnings));
            }
            match rx.recv_timeout(
                Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
            ) {
                Ok(Ok((client, events, baseline))) => {
                    return Ok(Self {
                        client: Some(client),
                        process,
                        events,
                        lock,
                        baseline: Box::new(baseline),
                        deadline,
                        stop,
                        warnings: vec![],
                        applied_session: None,
                    });
                }
                Ok(Err(error)) => {
                    let timed_out = Instant::now() >= deadline;
                    let warnings = process.finish(Duration::from_millis(100));
                    if timed_out {
                        return Err(startup_stopped(
                            OpError::new(ops::TIMEOUT, "omp startup exceeded the command deadline"),
                            warnings,
                        ));
                    }
                    return Err(OpenError::Failed(start_error(error, warnings)));
                }
                Err(RecvTimeoutError::Timeout) => (),
                Err(RecvTimeoutError::Disconnected) => {
                    let warnings = process.finish(Duration::from_millis(100));
                    return Err(startup_stopped(
                        OpError::new(ops::INTERNAL, "omp startup thread ended"),
                        warnings,
                    ));
                }
            }
        }
    }
    fn terminate(&mut self) {
        let Some(client) = self.client.take() else {
            self.warnings.extend(self.process.finish(Duration::ZERO));
            return;
        };
        if Arc::strong_count(&client) > 1 {
            // Stop the peer first so a blocked call releases its client reference.
            if let Err(error) = self.process.stop(Duration::ZERO) {
                self.warnings.push(Warning {
                    code: "teardown_error".into(),
                    message: format!("cannot stop omp with a blocked control thread: {error}"),
                });
            }
            let end = Instant::now() + Duration::from_millis(100);
            while Arc::strong_count(&client) > 1 && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        match Arc::try_unwrap(client) {
            Ok(client) => {
                if let Err(e) = client.close() {
                    self.warnings.push(Warning {
                        code: "teardown_error".into(),
                        message: e.to_string(),
                    });
                }
            }
            Err(client) => {
                self.warnings.push(Warning {
                    code: "teardown_error".into(),
                    message: "omp control thread did not return during process teardown".into(),
                });
                drop(client);
            }
        }
        // from_io closes the transport; Process still owns and must reap the child.
        // Give stdin EOF a short grace period before TERM/KILL escalation.
        self.warnings
            .extend(self.process.finish(Duration::from_millis(100)));
    }
    fn run(&mut self, op: Option<Op>, expect: Option<String>) -> Result<Value, OpError> {
        stopped(self.deadline, &self.stop)?;
        let (tx, rx) = mpsc::channel();
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| OpError::new(ops::OMP_ERROR, "omp client is closed"))?
            .clone();
        let deadline = self.deadline;
        let stop = Arc::clone(&self.stop);
        let applied = Arc::new(AtomicBool::new(false));
        let thread_applied = Arc::clone(&applied);
        std::thread::spawn(move || {
            let result = match op {
                Some(op) => super::exec::exec_until(
                    &client,
                    &op,
                    expect.as_deref(),
                    Some(deadline),
                    &stop,
                    Some(&thread_applied),
                ),
                None => super::exec::state(&client).and_then(|s| {
                    serde_json::to_value(s).map_err(|e| OpError::new(ops::INTERNAL, e.to_string()))
                }),
            };
            drop(client);
            if let Err(late) = tx.send(result) {
                drop(late);
            }
        });
        let result = loop {
            if let Ok(result) = rx.try_recv() {
                break result;
            }
            // Once omp confirms application, let the bounded post-state read
            // finish even if the overall deadline has just passed.
            if let Err(error) = stopped(self.deadline, &self.stop)
                && (self.stop.load(Ordering::SeqCst) || !applied.load(Ordering::SeqCst))
            {
                self.terminate();
                break Err(error);
            }
            let wait = if applied.load(Ordering::SeqCst) {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(100)
                    .min(self.deadline.saturating_duration_since(Instant::now()))
            };
            match rx.recv_timeout(wait) {
                Ok(result) => break result,
                Err(RecvTimeoutError::Timeout) => (),
                Err(_) => break Err(OpError::new(ops::INTERNAL, "omp control thread ended")),
            }
        };
        for event in self.events.try_iter() {
            if let Event::Notification(omp_rpc::RpcNotification::RpcAgentEvent(
                omp_rpc::RpcAgentEvent::Notice(notice),
            )) = event
                && notice.source.as_deref() == Some("session-persistence")
                && notice.level.as_str() == "error"
            {
                self.warnings.push(Warning {
                    code: "persistence_error".into(),
                    message: notice.message,
                });
            }
        }
        if result.as_ref().is_err_and(|e| e.code == ops::TIMEOUT) {
            self.terminate();
        }
        result
    }
    pub fn session_id(&self) -> &str {
        &self.baseline.session_id
    }
    pub fn call(&mut self, op: &Op, expect: Option<&str>) -> Result<Value, OpError> {
        let data = self.run(Some(op.clone()), expect.map(str::to_owned))?;
        if matches!(
            op,
            Op::SetModel(_) | Op::SetThinkingLevel(_) | Op::Compact(_)
        ) {
            self.applied_session = data["session_id"].as_str().map(str::to_owned);
        }
        Ok(data)
    }
    pub fn finish(mut self) -> Result<(Option<Forked>, Vec<Warning>), OpError> {
        let final_state = if let Some(id) = &self.applied_session {
            // The executor already read the post-mutation identity successfully.
            serde_json::json!({"sessionId":id})
        } else {
            self.run(None, None)?
        };
        let to = final_state["sessionId"]
            .as_str()
            .ok_or_else(|| OpError::new(ops::INTERNAL, "final omp state has no session id"))?;
        let forked = (to != self.baseline.session_id).then(|| Forked {
            from: self.baseline.session_id.clone(),
            to: to.into(),
        });
        self.terminate();
        self.lock.take();
        Ok((forked, std::mem::take(&mut self.warnings)))
    }
}
fn startup_details(message: &mut String, warnings: Vec<Warning>) -> Value {
    let stderr: Vec<_> = warnings
        .into_iter()
        .filter(|warning| warning.code == "omp_stderr")
        .map(|warning| warning.message)
        .collect();
    if let Some(last) = stderr.last() {
        message.push_str(": ");
        message.push_str(last);
    }
    serde_json::json!({"stderr": stderr})
}
fn startup_stopped(mut error: OpError, warnings: Vec<Warning>) -> OpenError {
    let details = startup_details(&mut error.message, warnings);
    OpenError::Stopped(error.with_details(details))
}
fn start_error(error: impl std::fmt::Display, warnings: Vec<Warning>) -> GlenError {
    let mut message = format!("cannot start omp for session control: {error}");
    let details = startup_details(&mut message, warnings);
    GlenError::new(ErrorCode::OmpUnavailable, message)
        .with_details(details)
        .with_hint("check omp or set GLEN_OMP_BIN")
}
impl Drop for Ephemeral {
    fn drop(&mut self) {
        self.terminate();
        self.lock.take();
    }
}
fn moved(from: &str, to: &str) -> GlenError {
    GlenError::new(
        ErrorCode::ControlFailed,
        "the session moved before the control could run",
    )
    .with_details(serde_json::json!({"reason":"session_moved","from":from,"to":to}))
    .with_hint("run `glen ls` and retry on the new handle")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn c2_3_startup_errors_keep_only_stderr_and_append_the_last_line() {
        let warnings = || {
            vec![
                Warning {
                    code: "omp_stderr".into(),
                    message: "first diagnostic".into(),
                },
                Warning {
                    code: "omp_stderr".into(),
                    message: "fatal startup error".into(),
                },
                Warning {
                    code: "teardown_error".into(),
                    message: "cleanup diagnostic".into(),
                },
            ]
        };
        let error = start_error("handshake failed", warnings());
        assert_eq!(
            error.message,
            "cannot start omp for session control: handshake failed: fatal startup error"
        );
        assert_eq!(
            error.details.unwrap()["stderr"],
            serde_json::json!(["first diagnostic", "fatal startup error"])
        );
        let OpenError::Stopped(error) =
            startup_stopped(OpError::new(ops::TIMEOUT, "startup timed out"), warnings())
        else {
            panic!("startup timeout changed error kind");
        };
        assert_eq!(error.code, ops::TIMEOUT);
        assert_eq!(error.message, "startup timed out: fatal startup error");
        assert_eq!(
            error.details.unwrap()["stderr"],
            serde_json::json!(["first diagnostic", "fatal startup error"])
        );
        let error = start_error("handshake failed", vec![]);
        assert_eq!(
            error.message,
            "cannot start omp for session control: handshake failed"
        );
        assert_eq!(error.details.unwrap()["stderr"], serde_json::json!([]));
    }
    #[test]
    fn stderr_capture_bounds_lines_and_handles_invalid_utf8() {
        let mut input = b"first\n\xfftail".to_vec();
        input.extend_from_slice(b"\n");
        input.extend_from_slice("long line\n".repeat(100).as_bytes());
        let warnings = drain_stderr(input.as_slice()).unwrap();
        assert_eq!(warnings.len(), 16);
        assert!(warnings.iter().all(|w| w.code == "omp_stderr"));
        assert_eq!(warnings[0].message, "first");
        assert_eq!(warnings[1].message, "\u{fffd}tail");
        assert!(drain_stderr(&b""[..]).unwrap().is_empty());
    }
}
