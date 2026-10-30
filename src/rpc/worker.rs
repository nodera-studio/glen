use super::{
    self as rpc, control,
    reply::{self, ReplySink},
};
use crate::{
    env::Env,
    error::{ErrorCode, GlenError, Warning},
    glen_home,
    jobs::{
        Deliver, Forked, Job, JobError, JobId, JobResult, JobStore, Route, State, Status, liveness,
        lock,
    },
    omp::{
        continuation,
        lease::{self, Lease},
        session::Session,
    },
};
use omp_rpc::{
    AbortCommand, Client, ClientOptions, Event, GetStateCommand, PromptCommand, PromptStatus,
    PromptTurn, RpcAgentEvent, RpcNotification, SetEventFilterCommand,
};
use serde_json::{Value, json};
use std::{
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};
#[derive(Debug, thiserror::Error)]
#[error("{error_message}")]
struct Failure {
    state: State,
    code: &'static str,
    error_message: String,
    hint: Option<String>,
    details: Option<Value>,
}
impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            state: State::Failed,
            code,
            error_message: message.into(),
            hint: None,
            details: None,
        }
    }
    fn cancelled() -> Self {
        Self {
            state: State::Aborted,
            ..Self::new("cancelled", "the glen job was cancelled")
        }
    }
}
impl From<GlenError> for Failure {
    fn from(e: GlenError) -> Self {
        Self::new("io", e.to_string())
    }
}
impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Self::new("io", e.to_string())
    }
}
struct Worker {
    store: JobStore,
    id: JobId,
    job: Job,
    status: Status,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
}
pub fn main(arg: &str) -> ExitCode {
    let loaded = (|| -> Result<Worker, GlenError> {
        let env = Env::from_process();
        let store = JobStore::new(glen_home::resolve(&env)?);
        let id = JobId::parse(arg)
            .ok_or_else(|| GlenError::new(ErrorCode::Usage, "invalid worker job id"))?;
        let job = store.read_job(&id)?;
        let status = store.read_status(&id)?;
        let deadline = job
            .deadline_at
            .parse::<jiff::Timestamp>()
            .map_err(|e| GlenError::new(ErrorCode::Internal, e.to_string()))?;
        let left = deadline
            .as_millisecond()
            .saturating_sub(jiff::Timestamp::now().as_millisecond())
            .max(0) as u64;
        Ok(Worker {
            store,
            id,
            job,
            status,
            cancel: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + Duration::from_millis(left),
        })
    })();
    let mut w = match loaded {
        Ok(w) => w,
        Err(e) => {
            eprintln!("worker: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.run()));
    match result {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(e)) => {
            w.log("failed", &e.to_string());
            match w.finish_failure(e) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("cannot persist worker failure: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            if let Err(e) = w.finish_failure(Failure::new(
                "internal",
                format!("worker panicked: {message}"),
            )) {
                eprintln!("cannot persist worker panic: {e}");
            }
            ExitCode::FAILURE
        }
    }
}
impl Worker {
    fn log(&self, phase: &str, detail: &str) {
        eprintln!(
            "{} {} {phase}: {detail}",
            jiff::Timestamp::now(),
            self.job.handle
        );
    }
    fn save(&mut self) -> Result<(), Failure> {
        self.status.updated_at = jiff::Timestamp::now().to_string();
        self.store.write_status(&self.id, &self.status)?;
        Ok(())
    }
    fn controls(&self) -> bool {
        let mut abort = false;
        for req in control::poll(&self.store.dir(&self.id)) {
            match &req.op {
                Ok(control::ControlOp::Abort) => abort = true,
                Err(op) => {
                    if let Err(e) = control::respond(
                        &req,
                        Err(("unsupported_op", format!("unsupported control op: {op}"))),
                    ) {
                        self.log("control", &e.to_string());
                    }
                }
            }
        }
        abort
    }
    fn stop_requested(&self) -> bool {
        self.cancel.load(Ordering::SeqCst) || self.controls()
    }
    fn check_stop(&self) -> Result<(), Failure> {
        if self.stop_requested() {
            Err(Failure::cancelled())
        } else if Instant::now() >= self.deadline {
            Err(Failure::new("timeout", "the glen job deadline passed"))
        } else {
            Ok(())
        }
    }
    fn respond_pending(&self) {
        for req in control::poll(&self.store.dir(&self.id)) {
            let result = match &req.op {
                Ok(_) => Ok(()),
                Err(op) => Err(("unsupported_op", format!("unsupported control op: {op}"))),
            };
            if let Err(e) = control::respond(&req, result) {
                self.log("control", &e.to_string());
            }
        }
    }
    fn finish_failure(&mut self, f: Failure) -> Result<(), Failure> {
        self.status.status = f.state;
        self.status.error = Some(JobError {
            code: f.code.into(),
            message: f.error_message,
            hint: f.hint,
            details: f.details,
        });
        self.save()?;
        self.respond_pending();
        Ok(())
    }
    fn probe(&self) -> Result<(), Failure> {
        let Some(id) = self.job.session_id.as_deref() else {
            return Ok(());
        };
        route_probe(
            lease::probe(&self.job.owners_dir, id),
            self.job.session_handle.as_deref().unwrap_or(id),
        )
    }
    fn moved(&self, to: &str) -> Failure {
        let mut f = Failure::new(
            "session_moved",
            "the session moved before glen could prompt it",
        );
        f.details = Some(json!({"from":self.job.session_id,"to":to}));
        f.hint = Some("run `glen ls` and send to the new handle".into());
        f
    }
    fn run(&mut self) -> Result<(), Failure> {
        if self.status.status.is_terminal() {
            self.log("claim", "job already terminal");
            return Ok(());
        }
        self.log("claim", "claiming job");
        if let Some(i) = liveness::current() {
            self.status.worker_pid = Some(i.pid);
            self.status.worker_start = Some(i.start);
        } else {
            self.status.worker_pid = Some(std::process::id());
        }
        self.save()?;
        for sig in [
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGHUP,
        ] {
            signal_hook::flag::register(sig, self.cancel.clone())?;
        }
        self.check_stop()?;
        let mut session_lock = None;
        if self.job.route == Route::Rpc {
            let id = self
                .job
                .session_id
                .as_deref()
                .ok_or_else(|| Failure::new("internal", "resumed job has no session id"))?;
            self.log("lock", "waiting for glen session lock");
            match lock::acquire(&self.store.home, id, self.deadline, &|| {
                self.stop_requested()
            })? {
                lock::Acquired::Locked { lock, waited } => {
                    session_lock = Some(lock);
                    self.status.waited_ms = Some(waited.as_millis().min(u64::MAX as u128) as u64);
                    if self.job.deliver_as == Deliver::Steer && waited >= rpc::TICK {
                        self.status.warnings.push(Warning{code:"steer_queued".into(),message:"another glen job held the session, so the steer message waited and was sent as a prompt".into()});
                    }
                }
                lock::Acquired::TimedOut => {
                    return Err(Failure::new(
                        "timeout",
                        "waited for another glen job on this session until the job deadline",
                    ));
                }
                lock::Acquired::Cancelled => return Err(Failure::cancelled()),
            }
            self.log("continuation", "re-following after lock");
            let path = self
                .job
                .session_file
                .as_deref()
                .ok_or_else(|| Failure::new("internal", "resumed job has no session file"))?;
            let followed = continuation::follow(path)?;
            let s = Session::load(&followed.path).map_err(|e| Failure::new("io", e.to_string()))?;
            if Some(&s.header.id) != self.job.session_id.as_ref() {
                return Err(self.moved(&s.header.id));
            }
            self.probe()?;
        }
        self.check_stop()?;
        self.log("spawn", "starting omp RPC");
        let sink = Arc::new(ReplySink::new(
            self.store.clone(),
            self.store.home.clone(),
            self.id.clone(),
        ));
        let Some(started) = self.start(sink.clone())? else {
            // Cancellation is already durable. The startup thread has joined and dropped its client.
            return Ok(());
        };
        let Started {
            client,
            events,
            baseline,
            warnings,
        } = started;
        self.status.omp_pid = client.pid();
        self.status.warnings.extend(warnings);
        self.save()?;
        // Drain at the source before adding a forwarder, so buffered startup frames cannot lag into the run.
        for _ in events.try_iter() {}
        let (tx, rx) = mpsc::channel();
        let forward = tx.clone();
        std::thread::spawn(move || {
            for ev in events {
                if forward.send(WorkerMsg::Rpc(Box::new(ev))).is_err() {
                    break;
                }
            }
        });
        let result = (|| -> Result<(), Failure> {
            if self.job.route == Route::Rpc
                && Some(&baseline.session_id) != self.job.session_id.as_ref()
            {
                return Err(self.moved(&baseline.session_id));
            }
            if self.job.route == Route::New {
                session_lock = Some(
                    lock::try_acquire(&self.store.home, &baseline.session_id)?
                        .ok_or_else(|| Failure::new("internal", "cannot lock the new session"))?,
                );
            }
            self.status.actual_session_id = Some(baseline.session_id.clone());
            self.status.actual_session_file =
                baseline.session_file.clone().map(std::path::PathBuf::from);
            sink.set_session(baseline.session_id.clone());
            if self.job.route == Route::Rpc {
                self.probe()?;
            }
            self.check_stop()?;
            for _ in rx.try_iter() {}
            self.status.delivered_as = Some(
                if !baseline.is_streaming {
                    "prompt"
                } else if self.job.deliver_as == Deliver::Steer {
                    "steer"
                } else {
                    "follow_up"
                }
                .into(),
            );
            self.log("prompt", "sending message");
            self.drive(&client, &rx, tx, &sink, &baseline.session_id)
        })();
        // Terminal state must precede teardown, and the session lock stays held through close.
        let result = match result {
            Err(f) => self.finish_failure(f),
            ok => ok,
        };
        self.log("close", "closing omp client");
        if let Err(e) = client.close() {
            self.log("close", &e.to_string());
        }
        drop(session_lock);
        result
    }
    fn start(&mut self, sink: Arc<ReplySink>) -> Result<Option<Started>, Failure> {
        let abandon = Arc::new(AtomicBool::new(false));
        let job = self.job.clone();
        let deadline = self.deadline;
        std::thread::scope(|scope| {
            let (tx, rx) = mpsc::channel();
            let startup_abandon = abandon.clone();
            let startup = scope.spawn(move || {
                let result = Started::spawn(&job, deadline, sink, &startup_abandon);
                if let Err(undelivered) = tx.send(result) {
                    // A late Started owns its Client; dropping it tears omp down.
                    drop(undelivered);
                }
            });
            let result = loop {
                if let Err(failure) = self.check_stop() {
                    abandon.store(true, Ordering::SeqCst);
                    // Persist and acknowledge before joining the bounded blocking call.
                    let finished = self.finish_failure(failure);
                    break finished.map(|()| None);
                }
                match rx.recv_timeout(rpc::TICK) {
                    Ok(result) => {
                        if let Err(failure) = self.check_stop() {
                            abandon.store(true, Ordering::SeqCst);
                            let finished = self.finish_failure(failure);
                            drop(result);
                            break finished.map(|()| None);
                        }
                        break result.map(Some);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        break Err(Failure::new(
                            "internal",
                            "startup thread closed without a result",
                        ));
                    }
                }
            };
            if startup.join().is_err() {
                return Err(Failure::new("internal", "worker startup thread panicked"));
            }
            // Drop any value that raced with abandonment before returning to release the lock.
            drop(rx);
            result
        })
    }
    fn recheck(&mut self, client: &Client, baseline: &str) {
        match client.call(&GetStateCommand {}) {
            Ok(s) => {
                if s.session_id != baseline {
                    self.status.forked = Some(Forked {
                        from: baseline.into(),
                        to: s.session_id.clone(),
                    });
                }
                self.status.actual_session_id = Some(s.session_id);
                self.status.actual_session_file = s.session_file.map(std::path::PathBuf::from);
            }
            Err(e) => self.log("get_state", &e.to_string()),
        }
    }
    fn event(
        &mut self,
        client: &Client,
        ev: Event,
        run: &mut Run,
        baseline: &str,
    ) -> Result<(), Failure> {
        match ev {
            Event::Notification(RpcNotification::RpcAgentEvent(ev)) => {
                self.accept()?;
                if let RpcAgentEvent::Notice(n) = ev
                    && n.source.as_deref() == Some("session-persistence")
                {
                    self.recheck(client, baseline);
                    if n.level.as_str() == "error" {
                        self.status.warnings.push(Warning {
                            code: "persistence_error".into(),
                            message: n.message,
                        });
                    }
                }
            }
            Event::Notification(RpcNotification::SessionSettled(_)) => run.settled_seen = true,
            Event::Notification(RpcNotification::SessionInfoUpdate(u))
                if Some(&u.session_id) != self.status.actual_session_id.as_ref() =>
            {
                self.recheck(client, baseline)
            }
            Event::Notification(RpcNotification::CommandOutput(c)) => {
                run.command_output.push(c.text)
            }
            Event::Undecodable { error, .. } => self.log("decode", &error),
            _ => (),
        }
        Ok(())
    }
    fn accept(&mut self) -> Result<(), Failure> {
        if self.status.status == State::Queued {
            self.status.status = State::Accepted;
            self.save()?;
        }
        Ok(())
    }
    fn drive(
        &mut self,
        client: &Client,
        rx: &Receiver<WorkerMsg>,
        tx: Sender<WorkerMsg>,
        sink: &ReplySink,
        baseline: &str,
    ) -> Result<(), Failure> {
        let cmd = PromptCommand {
            message: self.job.text.clone(),
            images: None,
            streaming_behavior: Some(rpc::streaming_behavior(self.job.deliver_as)),
        };
        let timeout = self.deadline.saturating_duration_since(Instant::now()) + rpc::PROMPT_GRACE;
        let mut run = Run::default();
        let terminal = std::thread::scope(|scope| -> Result<Terminal, Failure> {
            scope.spawn(|| {
                if tx
                    .send(WorkerMsg::Prompt(Box::new(
                        client.prompt_and_wait(&cmd, timeout),
                    )))
                    .is_err()
                {
                    eprintln!("worker prompt result receiver closed");
                }
            });
            // A persistence error still must drain the scoped prompt thread. Stop and escalate first.
            let mut persistence_failure = None;
            loop {
                match rx.recv_timeout(rpc::TICK) {
                    Ok(WorkerMsg::Rpc(ev)) => {
                        if let Err(e) = self.event(client, *ev, &mut run, baseline) {
                            persistence_failure = Some(e);
                            run.stop = Some(StopReason::Cancelled);
                        }
                    }
                    Ok(WorkerMsg::Prompt(outcome)) => {
                        run.prompt = Some(*outcome);
                        if let Err(e) = self.accept() {
                            persistence_failure = Some(e);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        run.prompt = Some(Err(omp_rpc::Error::Closed))
                    }
                }
                if self.stop_requested() {
                    run.stop = Some(StopReason::Cancelled);
                }
                if Instant::now() >= self.deadline && run.prompt.is_none() && run.stop.is_none() {
                    run.stop = Some(StopReason::Timeout);
                }
                match next(&run, Instant::now(), self.deadline) {
                    Next::Wait => (),
                    Next::SendAbort => {
                        self.log("abort", "aborting omp");
                        run.abort_sent_at = Some(Instant::now());
                        if let Err(e) = client.call(&AbortCommand {}) {
                            self.log("abort", &e.to_string());
                        }
                    }
                    Next::HardKill => {
                        if let Some(pid) = client.pid() {
                            kill_group(pid, rustix::process::Signal::TERM);
                            std::thread::sleep(rpc::KILL_GRACE);
                            kill_group(pid, rustix::process::Signal::KILL);
                        }
                        run.killed = true;
                    }
                    Next::Finish(t) => {
                        for msg in rx.try_iter() {
                            if let WorkerMsg::Rpc(ev) = msg
                                && let Err(e) = self.event(client, *ev, &mut run, baseline)
                            {
                                persistence_failure = Some(e);
                            }
                        }
                        return if let Some(e) = persistence_failure {
                            Err(e)
                        } else {
                            Ok(t)
                        };
                    }
                }
            }
        })?;
        if !run.killed {
            self.recheck(client, baseline);
        }
        if terminal.settle_timeout {
            self.status.warnings.push(Warning {
                code: "settle_timeout".into(),
                message: "omp completed but did not settle before the job deadline".into(),
            });
        }
        let turn = run.prompt.as_ref().and_then(|r| r.as_ref().ok());
        let result = JobResult {
            v: 1,
            prompt_status: turn
                .and_then(|t| t.result.as_ref())
                .map(|r| r.status.as_str().into()),
            agent_invoked: turn.is_none_or(|t| t.result.as_ref().is_some_and(|r| r.agent_invoked)),
            assistant_text: turn.and_then(|t| t.assistant_text.clone()),
            replies: sink.replies(),
            command_output: run.command_output,
            stop_reason: turn
                .and_then(|t| t.assistant_message.as_ref())
                .and_then(|m| m.stop_reason)
                .map(|s| s.as_str().into()),
            session_settled: terminal.settled,
            settled_at: terminal.settled.then(|| jiff::Timestamp::now().to_string()),
        };
        self.status.request_id = turn
            .and_then(|t| t.result.as_ref())
            .and_then(|r| r.id.clone());
        self.store.write_result(&self.id, &result)?;
        self.status.status = terminal.state;
        self.status.error = terminal.error;
        self.save()?;
        self.respond_pending();
        self.log("finish", self.status.status.as_str());
        Ok(())
    }
}
fn route_probe(lease: Lease, handle: &str) -> Result<(), Failure> {
    let message = match lease {
        Lease::Free => return Ok(()),
        Lease::Held => format!("session {handle} is open in another omp process"),
        Lease::Unknown(r) => {
            format!("cannot tell whether another omp process owns session {handle} ({r})")
        }
    };
    let mut f = Failure::new("no_route", message);
    f.hint=Some("close that omp session and retry; sending into a running session needs the glen omp extension (a later glen version)".into());
    Err(f)
}
fn kill_group(pid: u32, signal: rustix::process::Signal) {
    if let Some(pid) = rustix::process::Pid::from_raw(pid as i32)
        && let Err(e) = rustix::process::kill_process_group(pid, signal)
        && e != rustix::io::Errno::SRCH
    {
        eprintln!("cannot signal omp group: {e}");
    }
}
struct Started {
    client: Client,
    events: Receiver<Event>,
    baseline: omp_rpc::SessionState,
    warnings: Vec<Warning>,
}
impl Started {
    fn spawn(
        job: &Job,
        deadline: Instant,
        sink: Arc<ReplySink>,
        abandon: &AtomicBool,
    ) -> Result<Self, Failure> {
        let check = || {
            if abandon.load(Ordering::SeqCst) {
                Err(Failure::cancelled())
            } else {
                Ok(())
            }
        };
        check()?;
        let spawned = Client::spawn(
            rpc::omp_command(job),
            ClientOptions {
                tools: vec![],
                uris: vec![],
                ready_timeout: rpc::READY_TIMEOUT
                    .min(deadline.saturating_duration_since(Instant::now())),
                default_timeout: rpc::CALL_TIMEOUT,
            },
        );
        check()?;
        let (client, events) = spawned.map_err(|e| {
            let code = if matches!(
                e,
                omp_rpc::Error::Io(_) | omp_rpc::Error::Timeout { .. } | omp_rpc::Error::Closed
            ) {
                "omp_unavailable"
            } else {
                "rpc_error"
            };
            Failure::new(code, format!("cannot start omp: {e} (see worker.log)"))
        })?;
        let tools = client.set_custom_tools(vec![reply::glen_reply_tool(sink)]);
        check()?;
        let tools = tools.map_err(|e| {
            let mut failure = Failure::new("reply_tool_rejected", e.to_string());
            failure.hint = Some(
                "an omp extension already registers glen_reply; check GLEN_MANAGED handling".into(),
            );
            failure
        })?;
        if !tools.iter().any(|s| s == "glen_reply") {
            return Err(Failure::new(
                "reply_tool_rejected",
                "omp did not register glen_reply",
            ));
        }
        let filter = client.call(&SetEventFilterCommand {
            events: Some(rpc::EVENT_ALLOWLIST.iter().map(|s| (*s).into()).collect()),
        });
        check()?;
        let warnings = filter
            .err()
            .map(|e| Warning {
                code: "event_filter_rejected".into(),
                message: e.to_string(),
            })
            .into_iter()
            .collect();
        let baseline = client.call(&GetStateCommand {});
        check()?;
        let baseline = baseline.map_err(|e| Failure::new("rpc_error", e.to_string()))?;
        check()?;
        Ok(Self {
            client,
            events,
            baseline,
            warnings,
        })
    }
}
enum WorkerMsg {
    Rpc(Box<Event>),
    Prompt(Box<Result<PromptTurn, omp_rpc::Error>>),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum StopReason {
    Cancelled,
    Timeout,
}
#[derive(Default)]
struct Run {
    prompt: Option<Result<PromptTurn, omp_rpc::Error>>,
    settled_seen: bool,
    stop: Option<StopReason>,
    abort_sent_at: Option<Instant>,
    killed: bool,
    command_output: Vec<String>,
}
struct Terminal {
    state: State,
    error: Option<JobError>,
    settled: bool,
    settle_timeout: bool,
}
impl Terminal {
    fn completed(settled: bool) -> Self {
        Self {
            state: State::Completed,
            error: None,
            settled,
            settle_timeout: !settled,
        }
    }
    fn failed(state: State, code: &str, message: impl Into<String>) -> Self {
        Self {
            state,
            error: Some(JobError {
                code: code.into(),
                message: message.into(),
                hint: None,
                details: None,
            }),
            settled: false,
            settle_timeout: false,
        }
    }
}
enum Next {
    Wait,
    SendAbort,
    HardKill,
    Finish(Terminal),
}
fn stopped(stop: Option<StopReason>) -> Terminal {
    match stop {
        Some(StopReason::Timeout) => {
            Terminal::failed(State::Failed, "timeout", "the glen job deadline passed")
        }
        Some(StopReason::Cancelled) => {
            Terminal::failed(State::Aborted, "cancelled", "the glen job was cancelled")
        }
        None => Terminal::failed(State::Aborted, "aborted", "omp aborted the prompt"),
    }
}
fn next(run: &Run, now: Instant, deadline: Instant) -> Next {
    if let Some(prompt) = &run.prompt {
        return Next::Finish(match prompt {
            Ok(turn) => match &turn.result {
                None => Terminal::completed(true),
                Some(r) => match r.status {
                    PromptStatus::Completed => {
                        if r.session_settled || run.settled_seen {
                            Terminal::completed(true)
                        } else if now >= deadline {
                            Terminal::completed(false)
                        } else if run.stop.is_some() {
                            stopped(run.stop)
                        } else {
                            return Next::Wait;
                        }
                    }
                    PromptStatus::Aborted => stopped(run.stop),
                    PromptStatus::Error => {
                        let mut t = Terminal::failed(
                            State::Failed,
                            "omp_error",
                            r.error
                                .as_ref()
                                .map(|e| e.message.as_str())
                                .unwrap_or("omp prompt failed"),
                        );
                        if let (Some(e), Some(detail)) = (&mut t.error, &r.error) {
                            e.details = Some(json!({"retryable":detail.retryable}));
                        }
                        t
                    }
                },
            },
            Err(e) => {
                if run.stop.is_some() {
                    stopped(run.stop)
                } else {
                    Terminal::failed(
                        State::Failed,
                        match e {
                            omp_rpc::Error::Closed => "omp_exited",
                            omp_rpc::Error::Timeout { .. } => "timeout",
                            omp_rpc::Error::Command { .. } => "rpc_error",
                            omp_rpc::Error::Protocol(_) | omp_rpc::Error::Json(_) => "protocol",
                            omp_rpc::Error::Io(_) => "io",
                            omp_rpc::Error::InvalidArgument(_) => "rpc_error",
                        },
                        e.to_string(),
                    )
                }
            }
        });
    }
    if run.stop.is_some() {
        if run.abort_sent_at.is_none() {
            return Next::SendAbort;
        }
        if !run.killed
            && run
                .abort_sent_at
                .is_some_and(|t| now.duration_since(t) >= rpc::ABORT_GRACE)
        {
            return Next::HardKill;
        }
    }
    Next::Wait
}

#[cfg(test)]
mod tests {
    use super::*;
    fn turn(status: Option<PromptStatus>, settled: bool) -> PromptTurn {
        PromptTurn {
            events: vec![],
            messages: vec![],
            assistant_message: None,
            assistant_text: None,
            result: status.map(|status| omp_rpc::PromptResultEvent {
                agent_invoked: true,
                status,
                session_settled: settled,
                id: Some("request".into()),
                error: (status == PromptStatus::Error).then(|| omp_rpc::PromptError {
                    message: "provider failed".into(),
                    retryable: true,
                    provider: None,
                    model: None,
                    http_status: None,
                    code: None,
                }),
            }),
        }
    }
    #[test]
    fn completion_and_settlement_table() {
        let now = Instant::now();
        for (local, settled, seen, expired, finish, settle_timeout) in [
            (true, false, false, false, true, false),
            (false, true, false, false, true, false),
            (false, false, true, false, true, false),
            (false, false, false, false, false, false),
            (false, false, false, true, true, true),
        ] {
            let r = Run {
                prompt: Some(Ok(turn(
                    (!local).then_some(PromptStatus::Completed),
                    settled,
                ))),
                settled_seen: seen,
                ..Run::default()
            };
            match next(
                &r,
                now,
                if expired {
                    now
                } else {
                    now + Duration::from_secs(1)
                },
            ) {
                Next::Finish(t) => {
                    assert!(finish);
                    assert_eq!(t.state, State::Completed);
                    assert_eq!(t.settle_timeout, settle_timeout);
                    assert_eq!(t.settled, !settle_timeout);
                }
                Next::Wait => assert!(!finish),
                _ => panic!("unexpected transition"),
            }
        }
        let mut r = Run {
            settled_seen: true,
            ..Run::default()
        };
        assert!(matches!(next(&r, now, now), Next::Wait));
        r.prompt = Some(Ok(turn(Some(PromptStatus::Completed), false)));
        assert!(matches!(
            next(&r, now, now + Duration::from_secs(1)),
            Next::Finish(Terminal {
                state: State::Completed,
                settled: true,
                ..
            })
        ));
    }
    #[test]
    fn aborted_and_error_mapping() {
        let now = Instant::now();
        for (stop, state, code) in [
            (None, State::Aborted, "aborted"),
            (Some(StopReason::Cancelled), State::Aborted, "cancelled"),
            (Some(StopReason::Timeout), State::Failed, "timeout"),
        ] {
            let r = Run {
                prompt: Some(Ok(turn(Some(PromptStatus::Aborted), true))),
                stop,
                ..Run::default()
            };
            let Next::Finish(t) = next(&r, now, now) else {
                panic!("not terminal")
            };
            assert_eq!(t.state, state);
            assert_eq!(t.error.unwrap().code, code);
        }
        let r = Run {
            prompt: Some(Ok(turn(Some(PromptStatus::Error), false))),
            ..Run::default()
        };
        let Next::Finish(t) = next(&r, now, now) else {
            panic!("not terminal")
        };
        let error = t.error.unwrap();
        assert_eq!(error.code, "omp_error");
        assert_eq!(error.message, "provider failed");
        assert_eq!(error.details.unwrap()["retryable"], true);
    }
    #[test]
    fn transport_error_mapping() {
        let now = Instant::now();
        for (error, code) in [
            (omp_rpc::Error::Closed, "omp_exited"),
            (
                omp_rpc::Error::Timeout {
                    command: "prompt".into(),
                },
                "timeout",
            ),
            (
                omp_rpc::Error::Command {
                    command: "prompt".into(),
                    error: "rejected".into(),
                    code: None,
                },
                "rpc_error",
            ),
            (omp_rpc::Error::Protocol("bad".into()), "protocol"),
            (
                omp_rpc::Error::Json(serde_json::from_str::<Value>("bad").unwrap_err()),
                "protocol",
            ),
            (omp_rpc::Error::Io(std::io::Error::other("broken")), "io"),
        ] {
            let r = Run {
                prompt: Some(Err(error)),
                ..Run::default()
            };
            let Next::Finish(t) = next(&r, now, now) else {
                panic!("not terminal")
            };
            assert_eq!(t.state, State::Failed);
            assert_eq!(t.error.unwrap().code, code);
        }
    }
    #[test]
    fn cancel_deadline_and_escalation() {
        let now = Instant::now();
        let mut r = Run {
            stop: Some(StopReason::Cancelled),
            ..Run::default()
        };
        assert!(matches!(next(&r, now, now), Next::SendAbort));
        r.abort_sent_at = Some(now);
        assert!(matches!(next(&r, now, now), Next::Wait));
        assert!(matches!(
            next(&r, now + rpc::ABORT_GRACE, now),
            Next::HardKill
        ));
        r.killed = true;
        assert!(matches!(next(&r, now + rpc::ABORT_GRACE, now), Next::Wait));
        r.prompt = Some(Err(omp_rpc::Error::Closed));
        let Next::Finish(t) = next(&r, now, now) else {
            panic!("not terminal")
        };
        assert_eq!(t.error.unwrap().code, "cancelled");
        r.stop = Some(StopReason::Timeout);
        let Next::Finish(t) = next(&r, now, now) else {
            panic!("not terminal")
        };
        assert_eq!(t.error.unwrap().code, "timeout");
    }
}
