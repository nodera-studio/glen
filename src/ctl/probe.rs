//! `glen doctor --probe`: a foreground, lockless RPC capability check. It starts omp with
//! `--no-session`, performs the handshake and two capability calls inside one budget, sends
//! no prompt, writes nothing under GLEN_HOME, and always tears the process group down.
use super::ephemeral::{Process, drain_stderr};
use crate::rpc::{self, reply};
use omp_rpc::{Client, ClientOptions, SetEventFilterCommand};
use serde::Serialize;
use std::{
    path::Path,
    process::{ChildStdin, ChildStdout, Stdio},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
/// The whole probe: startup and every call share it.
pub const BUDGET: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(100);
/// What the probe learned about one capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Cap {
    Ok,
    Missing,
    Unknown,
}
impl Cap {
    pub fn as_str(self) -> &'static str {
        match self {
            Cap::Ok => "ok",
            Cap::Missing => "missing",
            Cap::Unknown => "unknown",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub protocol: Option<u32>,
    pub host_tools: Cap,
    pub event_filter: Cap,
}
impl Capabilities {
    /// `fail` only when the one required capability (`host_tools`) is proven missing; `ok`
    /// when everything is known and present; otherwise `warn`.
    pub fn status(&self) -> &'static str {
        if self.host_tools == Cap::Missing {
            "fail"
        } else if self.protocol.is_some()
            && self.host_tools == Cap::Ok
            && self.event_filter == Cap::Ok
        {
            "ok"
        } else {
            "warn"
        }
    }
    /// The doctor row value: `protocol=<1|2|unknown> host_tools=<…> event_filter=<…>`.
    pub fn value(&self) -> String {
        format!(
            "protocol={} host_tools={} event_filter={}",
            self.protocol
                .map_or_else(|| "unknown".to_owned(), |p| p.to_string()),
            self.host_tools.as_str(),
            self.event_filter.as_str()
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub capabilities: Capabilities,
    /// Why a capability is not `ok`, single-line; empty when everything is `ok`.
    pub detail: String,
}
impl Probe {
    fn unknown(detail: String) -> Self {
        Probe {
            capabilities: Capabilities {
                protocol: None,
                host_tools: Cap::Unknown,
                event_filter: Cap::Unknown,
            },
            detail,
        }
    }
    fn note(&mut self, reason: &str) {
        let reason = one_line(reason);
        if reason.is_empty() {
            return;
        }
        if !self.detail.is_empty() {
            self.detail.push_str("; ");
        }
        self.detail.push_str(&reason);
    }
}
/// Starts `bin --mode rpc --no-ui --no-session` in `cwd` and checks its capabilities. The
/// result is final at `budget`; teardown after it is bounded by [`Process::finish`]. Once
/// `stop` is set (the caller's interrupt flag) it tears omp down and returns `interrupted`:
/// omp runs in its own process group, so a terminal's Ctrl-C reaches only glen.
pub fn run(
    bin: &Path,
    profile: Option<&str>,
    cwd: &Path,
    budget: Duration,
    stop: &AtomicBool,
) -> Probe {
    let deadline = Instant::now() + budget;
    let mut command = rpc::base_command(bin, profile, cwd);
    command.arg("--no-session");
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let child = match command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return Probe::unknown(one_line(&format!("cannot start omp: {e}"))),
    };
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
        let mut probe = Probe::unknown("missing omp pipe".into());
        teardown(&mut probe, &mut process, Duration::from_millis(100), false);
        return probe;
    };
    process.stderr = Some(std::thread::spawn(move || drain_stderr(stderr)));
    let shared = Arc::new(Mutex::new(Shared {
        probe: Probe::unknown(String::new()),
        stage: "startup",
    }));
    let (done_tx, done) = mpsc::channel();
    let worker = Arc::clone(&shared);
    std::thread::spawn(move || {
        let wants_stderr = handshake(stdout, stdin, deadline, &worker);
        // After the deadline run() no longer listens, so a late result has nowhere to go.
        done_tx.send((wants_stderr, Instant::now())).ok();
    });
    let snapshot = || {
        shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    };
    loop {
        let waited = done.recv_timeout(POLL.min(remaining(deadline, Instant::now())));
        match next(
            waited,
            Instant::now(),
            deadline,
            stop.load(Ordering::SeqCst),
        ) {
            Next::Wait => (),
            Next::Finished { wants_stderr } => {
                // The worker closed the client, so omp sees stdin EOF and can exit on its own.
                let mut probe = snapshot().probe;
                teardown(
                    &mut probe,
                    &mut process,
                    Duration::from_secs(1),
                    wants_stderr,
                );
                return probe;
            }
            Next::Lost => {
                let mut probe = snapshot().probe;
                probe.note("probe thread ended unexpectedly");
                teardown(&mut probe, &mut process, Duration::from_millis(100), false);
                return probe;
            }
            Next::Interrupted => {
                let mut probe = Probe::unknown("interrupted".into());
                teardown(&mut probe, &mut process, Duration::from_millis(100), false);
                return probe;
            }
            Next::Late => {
                // Final at the deadline: what the worker published before it is the result.
                let Shared { mut probe, stage } = snapshot();
                let learned = std::mem::take(&mut probe.detail);
                probe.note(&format!(
                    "probe exceeded the {budget:?} budget during {stage}"
                ));
                probe.note(&learned);
                teardown(&mut probe, &mut process, Duration::from_millis(100), false);
                return probe;
            }
        }
    }
}
/// What the main thread does after one wait on the worker.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    Wait,
    Finished { wants_stderr: bool },
    Lost,
    Interrupted,
    Late,
}
fn next(
    waited: Result<(bool, Instant), mpsc::RecvTimeoutError>,
    now: Instant,
    deadline: Instant,
    stop: bool,
) -> Next {
    match waited {
        Ok((wants_stderr, finished)) if !is_late(finished, deadline) => {
            Next::Finished { wants_stderr }
        }
        Ok(_) => Next::Late,
        Err(mpsc::RecvTimeoutError::Disconnected) => Next::Lost,
        Err(mpsc::RecvTimeoutError::Timeout) if stop => Next::Interrupted,
        Err(mpsc::RecvTimeoutError::Timeout) if is_late(now, deadline) => Next::Late,
        Err(mpsc::RecvTimeoutError::Timeout) => Next::Wait,
    }
}
/// Stops the process group and joins the stderr drain. A failed teardown is noted; the last
/// stderr line is appended when the probe ended because omp exited before `ready`.
fn teardown(probe: &mut Probe, process: &mut Process, grace: Duration, wants_stderr: bool) {
    let warnings = process.finish(grace);
    if wants_stderr && let Some(line) = warnings.iter().rev().find(|w| w.code == "omp_stderr") {
        probe.detail = format!("{}: {}", probe.detail, one_line(&line.message));
    }
    for w in warnings.iter().filter(|w| w.code == "teardown_error") {
        probe.note(&format!("teardown: {}", w.message));
    }
}
/// What the worker thread has published: the probe so far and the stage in flight.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shared {
    probe: Probe,
    stage: &'static str,
}
/// At or after the deadline is late: a clamped call times out at the deadline, never before
/// it, so the deadline is what ended it.
fn is_late(at: Instant, deadline: Instant) -> bool {
    at >= deadline
}
/// Applies one stage's result unless it is late. The clock is read under the lock, so nothing
/// lands after the main thread has seen the deadline: the result is final at the budget.
fn publish(shared: &Mutex<Shared>, deadline: Instant, stage: impl FnOnce(&mut Shared)) {
    let mut state = shared.lock().unwrap_or_else(PoisonError::into_inner);
    if !is_late(Instant::now(), deadline) {
        stage(&mut state);
    }
}
/// Runs on the worker thread. Each stage is published to `shared`, so a deadline still returns
/// what was learned. Returns whether the reason needs omp's last stderr line.
fn handshake(
    stdout: ChildStdout,
    stdin: ChildStdin,
    deadline: Instant,
    shared: &Mutex<Shared>,
) -> bool {
    let left = remaining(deadline, Instant::now());
    let (mut client, _events) = match Client::from_io(
        stdout,
        stdin,
        ClientOptions {
            tools: vec![],
            uris: vec![],
            ready_timeout: left,
            default_timeout: left,
        },
    ) {
        Ok(connected) => connected,
        Err(e) => {
            let (detail, wants_stderr) = start_failure(&e);
            publish(shared, deadline, |s| s.probe.note(&detail));
            return wants_stderr;
        }
    };
    let protocol = client.protocol_version();
    publish(shared, deadline, |s| {
        s.probe.capabilities.protocol = Some(protocol);
        s.stage = "set_host_tools";
    });
    // The receiver is dropped: no prompt is ever sent, so omp has no reason to call the tool.
    let (replies, _) = mpsc::channel();
    client.set_default_timeout(remaining(deadline, Instant::now()));
    let (cap, reason) = host_tools(&client.set_custom_tools(vec![reply::glen_reply_tool(replies)]));
    publish(shared, deadline, |s| {
        s.probe.capabilities.host_tools = cap;
        s.probe.note(&reason);
        s.stage = "set_event_filter";
    });
    client.set_default_timeout(remaining(deadline, Instant::now()));
    let (cap, reason) = event_filter(&client.call(&SetEventFilterCommand {
        events: Some(rpc::EVENT_ALLOWLIST.iter().map(|s| (*s).into()).collect()),
    }));
    publish(shared, deadline, |s| {
        s.probe.capabilities.event_filter = cap;
        s.probe.note(&reason);
        s.stage = "close";
    });
    // Close before signalling completion so omp sees stdin EOF inside the teardown grace.
    if let Err(e) = client.close() {
        publish(shared, deadline, |s| {
            s.probe.note(&format!("cannot close the omp client: {e}"));
        });
    }
    false
}
/// The time left before `deadline`, zero once it has passed.
fn remaining(deadline: Instant, now: Instant) -> Duration {
    deadline.saturating_duration_since(now)
}
/// A rejected command proves the capability missing; any other failure proves nothing.
fn cap_of(e: &omp_rpc::Error) -> Cap {
    match e {
        omp_rpc::Error::Command { .. } => Cap::Missing,
        _ => Cap::Unknown,
    }
}
fn host_tools(result: &Result<Vec<String>, omp_rpc::Error>) -> (Cap, String) {
    match result {
        Ok(names) if names.iter().any(|n| n == "glen_reply") => (Cap::Ok, String::new()),
        Ok(_) => (Cap::Missing, "omp did not register glen_reply".into()),
        Err(e) => (cap_of(e), e.to_string()),
    }
}
fn event_filter<T>(result: &Result<T, omp_rpc::Error>) -> (Cap, String) {
    match result {
        Ok(_) => (Cap::Ok, String::new()),
        Err(e) => (cap_of(e), e.to_string()),
    }
}
/// The reason for a failed `Client::from_io`, and whether omp's last stderr line explains it.
fn start_failure(e: &omp_rpc::Error) -> (String, bool) {
    match e {
        omp_rpc::Error::Closed => ("omp exited before ready".into(), true),
        omp_rpc::Error::Timeout { .. } => (format!("omp did not start in time: {e}"), false),
        _ => (format!("omp startup failed: {e}"), false),
    }
}
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
#[cfg(test)]
mod tests {
    use super::*;
    fn command_error() -> omp_rpc::Error {
        omp_rpc::Error::Command {
            command: "set_host_tools".into(),
            error: "RPC host tool \"glen_reply\" conflicts\nwith an existing tool".into(),
            code: None,
        }
    }
    #[test]
    fn host_tools_classification() {
        assert_eq!(
            host_tools(&Ok(vec!["other".into(), "glen_reply".into()])),
            (Cap::Ok, String::new())
        );
        assert_eq!(
            host_tools(&Ok(vec!["other".into()])).0,
            Cap::Missing,
            "names without glen_reply prove it missing"
        );
        let (cap, reason) = host_tools(&Err(command_error()));
        assert_eq!(cap, Cap::Missing);
        assert!(reason.contains("conflicts"), "{reason}");
        for e in [
            omp_rpc::Error::Timeout {
                command: "set_host_tools".into(),
            },
            omp_rpc::Error::Closed,
            omp_rpc::Error::Io(std::io::Error::other("pipe")),
            omp_rpc::Error::Protocol("bad".into()),
            omp_rpc::Error::InvalidArgument("bad".into()),
            omp_rpc::Error::Json(serde_json::from_str::<u8>("x").unwrap_err()),
        ] {
            assert_eq!(host_tools(&Err(e)).0, Cap::Unknown);
        }
    }
    #[test]
    fn event_filter_classification() {
        assert_eq!(
            event_filter(&Ok::<_, omp_rpc::Error>(None::<Vec<String>>)),
            (Cap::Ok, String::new())
        );
        assert_eq!(event_filter::<()>(&Err(command_error())).0, Cap::Missing);
        assert_eq!(
            event_filter::<()>(&Err(omp_rpc::Error::Closed)).0,
            Cap::Unknown
        );
    }
    #[test]
    fn start_failures_say_why() {
        assert_eq!(
            start_failure(&omp_rpc::Error::Closed),
            ("omp exited before ready".into(), true)
        );
        let (detail, stderr) = start_failure(&omp_rpc::Error::Timeout {
            command: "ready".into(),
        });
        assert!(!stderr && detail.contains("ready timed out"), "{detail}");
        let (detail, stderr) = start_failure(&omp_rpc::Error::Protocol("negotiation".into()));
        assert!(!stderr && detail.contains("negotiation"), "{detail}");
    }
    #[test]
    fn status_and_value() {
        let caps = |protocol, host_tools, event_filter| Capabilities {
            protocol,
            host_tools,
            event_filter,
        };
        let all = caps(Some(2), Cap::Ok, Cap::Ok);
        assert_eq!(all.status(), "ok");
        assert_eq!(all.value(), "protocol=2 host_tools=ok event_filter=ok");
        assert_eq!(caps(None, Cap::Ok, Cap::Ok).status(), "warn");
        assert_eq!(caps(Some(1), Cap::Ok, Cap::Missing).status(), "warn");
        assert_eq!(caps(Some(1), Cap::Unknown, Cap::Ok).status(), "warn");
        assert_eq!(caps(Some(1), Cap::Missing, Cap::Ok).status(), "fail");
        let none = caps(None, Cap::Unknown, Cap::Unknown);
        assert_eq!(
            none.value(),
            "protocol=unknown host_tools=unknown event_filter=unknown"
        );
        assert_eq!(
            serde_json::to_value(&none).unwrap(),
            serde_json::json!({"protocol":null,"host_tools":"unknown","event_filter":"unknown"})
        );
    }
    #[test]
    fn remaining_is_clamped() {
        let now = Instant::now();
        let deadline = now + Duration::from_millis(1500);
        assert_eq!(remaining(deadline, now), Duration::from_millis(1500));
        assert_eq!(
            remaining(deadline, now + Duration::from_millis(500)),
            Duration::from_millis(1000)
        );
        assert_eq!(
            remaining(deadline, now + Duration::from_secs(60)),
            Duration::ZERO
        );
        assert!(remaining(now + BUDGET, now) < rpc::READY_TIMEOUT);
    }
    #[test]
    fn the_deadline_itself_is_late() {
        let deadline = Instant::now() + Duration::from_secs(1);
        assert!(is_late(deadline, deadline));
        assert!(is_late(deadline + Duration::from_nanos(1), deadline));
        assert!(!is_late(deadline - Duration::from_nanos(1), deadline));
    }
    #[test]
    fn a_handshake_finished_at_the_deadline_is_late() {
        use mpsc::RecvTimeoutError::{Disconnected, Timeout};
        let deadline = Instant::now() + Duration::from_secs(1);
        let before = deadline - Duration::from_nanos(1);
        // The worker's own finish time decides, not when the main thread reads it.
        let after = deadline + Duration::from_secs(1);
        assert_eq!(
            next(Ok((true, deadline)), before, deadline, false),
            Next::Late
        );
        assert_eq!(
            next(Ok((true, before)), after, deadline, false),
            Next::Finished { wants_stderr: true }
        );
        assert_eq!(next(Err(Timeout), before, deadline, false), Next::Wait);
        assert_eq!(next(Err(Timeout), deadline, deadline, false), Next::Late);
        assert_eq!(next(Err(Timeout), after, deadline, true), Next::Interrupted);
        assert_eq!(next(Err(Disconnected), after, deadline, true), Next::Lost);
    }
    #[test]
    fn a_stage_published_at_or_after_the_deadline_is_discarded() {
        let shared = Mutex::new(Shared {
            probe: Probe::unknown(String::new()),
            stage: "startup",
        });
        let start = Instant::now();
        publish(&shared, start + Duration::from_secs(3600), |s| {
            s.probe.capabilities.protocol = Some(2);
            s.stage = "set_host_tools";
        });
        let on_time = shared.lock().unwrap().clone();
        assert_eq!(on_time.probe.capabilities.protocol, Some(2));
        assert_eq!(on_time.stage, "set_host_tools");
        // `start` has passed by the time publish reads the clock: the stage is late.
        publish(&shared, start, |s| {
            s.probe.capabilities.host_tools = Cap::Ok;
            s.probe.note("timed out");
            s.stage = "set_event_filter";
        });
        assert_eq!(*shared.lock().unwrap(), on_time);
    }
    #[test]
    fn details_stay_single_line() {
        let mut p = Probe::unknown(String::new());
        p.note("first\nline");
        p.note("");
        p.note("second");
        assert_eq!(p.detail, "first line; second");
        let (_, reason) = host_tools(&Err(command_error()));
        p.note(&reason);
        assert!(!p.detail.contains('\n'));
    }
}
