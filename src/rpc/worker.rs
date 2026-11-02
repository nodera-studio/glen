// `crate::rpc::{self, …}`, not `super::{self as rpc, …}`: the MSRV, Rust 1.89, rejects
// importing `super` through `self` ("no `super` in the root").
use crate::rpc::{
    self, control,
    reply::{self, ReplySink},
};
use crate::{
    env::Env,
    error::{ErrorCode, GlenError, NoRouteReason, Warning},
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
    AbortCommand, AgentMessage, AssistantContent, Client, ClientOptions, Command, Event,
    GetStateCommand, PromptAck, PromptCommand, PromptStatus, PromptTurn, RpcAgentEvent,
    RpcNotification, SetEventFilterCommand,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    process::ExitCode,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};
#[derive(Debug, thiserror::Error)]
#[error("{error_message}")]
pub(crate) struct Failure {
    pub(crate) state: State,
    pub(crate) code: &'static str,
    pub(crate) error_message: String,
    pub(crate) hint: Option<String>,
    pub(crate) details: Option<Value>,
}
impl Failure {
    pub(crate) fn no_route(reason: NoRouteReason, message: impl Into<String>) -> Self {
        Self {
            details: Some(json!({"reason":reason.as_str()})),
            ..Self::new(ErrorCode::NoRoute.as_str(), message)
        }
    }
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            state: State::Failed,
            code,
            error_message: message.into(),
            hint: None,
            details: None,
        }
    }
    pub(crate) fn cancelled() -> Self {
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
pub(crate) struct Worker {
    pub(crate) store: JobStore,
    pub(crate) id: JobId,
    pub(crate) job: Job,
    pub(crate) status: Status,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) deadline: Instant,
    pub(crate) control_in_flight: Option<std::path::PathBuf>,
    pub(crate) omp_queued: bool,
}
pub fn main(arg: &str) -> ExitCode {
    let loaded = (|| -> Result<Worker, GlenError> {
        let env = Env::from_process();
        let store = JobStore::new(glen_home::resolve(&env)?);
        let id = JobId::parse(arg)
            .ok_or_else(|| GlenError::new(ErrorCode::Usage, "invalid worker job id"))?;
        let job = store.read_job(&id)?;
        let status = store.read_status(&id)?;
        let deadline = job.deadline_at.parse::<jiff::Timestamp>().map_err(|e| {
            GlenError::new(
                ErrorCode::Internal,
                format!("invalid deadline for job {}: {e}", job.handle),
            )
        })?;
        let left = deadline
            .as_millisecond()
            .saturating_sub(jiff::Timestamp::now().as_millisecond())
            .max(0) as u64;
        Ok(Worker {
            store,
            id,
            job,
            status,
            control_in_flight: None,
            omp_queued: false,
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
    pub(crate) fn log(&self, phase: &str, detail: &str) {
        eprintln!(
            "{} {} {phase}: {detail}",
            jiff::Timestamp::now(),
            self.job.handle
        );
    }
    pub(crate) fn save(&mut self) -> Result<(), Failure> {
        self.status.updated_at = jiff::Timestamp::now().to_string();
        self.store.write_status(&self.id, &self.status)?;
        Ok(())
    }
    fn controls(&self) -> (bool, Vec<control::Request>) {
        let mut abort = false;
        let mut pending = Vec::new();
        for req in control::poll(&self.store.dir(&self.id)) {
            if self.control_in_flight.as_ref() == Some(&req.path) {
                continue;
            }
            if control::expired(&req) {
                self.answer_control(&req, Err(control::expired_error()));
                continue;
            }
            match &req.op {
                Ok(control::ControlOp::Abort) => {
                    let actual = self
                        .status
                        .actual_session_id
                        .as_deref()
                        .or(self.job.session_id.as_deref());
                    if req
                        .expect_session_id
                        .as_deref()
                        .is_some_and(|expected| actual != Some(expected))
                    {
                        self.answer_control(
                            &req,
                            Err(crate::ops::OpError::new(
                                crate::ops::SESSION_MISMATCH,
                                "the job is driving another session",
                            )
                            .with_details(json!({"session_id":actual}))),
                        );
                    } else {
                        abort = true;
                    }
                }
                Ok(control::ControlOp::Ctl(_)) => pending.push(req.clone()),
                Err(error) => {
                    if let Err(e) = control::respond(&req, Err(error.clone())) {
                        self.log("control", &e.to_string());
                    }
                }
            }
        }
        (abort, pending)
    }
    fn stop_requested(&self) -> bool {
        self.cancel.load(Ordering::SeqCst) || self.controls().0
    }
    pub(crate) fn check_stop(&self) -> Result<(), Failure> {
        if self.stop_requested() {
            Err(Failure::cancelled())
        } else if Instant::now() >= self.deadline {
            Err(Failure::new("timeout", "the glen job deadline passed"))
        } else {
            Ok(())
        }
    }
    fn mark_holder(&self, lock: &lock::SessionLock) {
        let holder = lock::Holder {
            v: 1,
            kind: lock::HolderKind::Job,
            job_id: Some(self.id.as_str().into()),
            pid: std::process::id(),
            start: liveness::current().map(|i| i.start),
        };
        if let Err(e) = lock::mark(lock, &holder) {
            self.log("holder", &e.to_string());
        }
    }
    fn answer_control(
        &self,
        req: &control::Request,
        result: Result<Option<Value>, crate::ops::OpError>,
    ) {
        if let Err(e) = control::respond(req, result) {
            self.log("control", &e.to_string());
        }
    }
    fn respond_pending(&self) {
        for req in control::poll(&self.store.dir(&self.id)) {
            if self.control_in_flight.as_ref() == Some(&req.path) {
                continue;
            }
            let result = if control::expired(&req) {
                Err(control::expired_error())
            } else {
                match &req.op {
                    Ok(control::ControlOp::Abort) => Ok(Some(
                        json!({"job_status": self.status.status,"session_id":self.status.actual_session_id.as_ref().or(self.job.session_id.as_ref()),"queued":self.omp_queued}),
                    )),
                    Ok(control::ControlOp::Ctl(_)) => Err(crate::ops::OpError::new(
                        crate::ops::JOB_ENDED,
                        "the glen job ended before the control ran; glen retries on the session",
                    )),
                    Err(error) => Err(error.clone()),
                }
            };
            if let Err(e) = control::respond(&req, result) {
                self.log("control", &e.to_string());
            }
        }
    }
    pub(crate) fn finish_failure(&mut self, f: Failure) -> Result<(), Failure> {
        self.status.waiting_for_lock = false;
        self.status.status = f.state;
        self.status.error = Some(JobError {
            code: f.code.into(),
            message: f.error_message,
            hint: f.hint,
            details: f.details,
        });
        self.map_sandbox_error();
        self.save()?;
        self.respond_pending();
        Ok(())
    }
    fn map_sandbox_error(&mut self) {
        if let Some(error) = self.status.error.as_mut()
            && let Some(mapped) =
                crate::sandbox::map_worker_failure(&Env::from_process(), &error.message)
        {
            error.code = "no_route".into();
            error.message = mapped.message;
            error.hint = mapped.hint;
            error.details = mapped.details.map(|d| *d);
        }
    }
    fn endpoint_probe(&self) -> Result<(), Failure> {
        endpoint_check(
            &self.store.home,
            self.job.session_id.as_deref(),
            self.job.session_handle.as_deref(),
        )
    }
    fn probe(&self) -> Result<(), Failure> {
        self.endpoint_probe()?;
        let Some(id) = self.job.session_id.as_deref() else {
            return Ok(());
        };
        route_probe(
            lease::probe(&self.job.owners_dir, id),
            self.job.session_handle.as_deref().unwrap_or(id),
            &Env::from_process(),
            self.job.profile_flag.as_deref(),
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
            signal_hook::flag::register(sig, self.cancel.clone()).map_err(|e| {
                Failure::new(
                    "io",
                    format!(
                        "cannot register signal {sig} for job {}: {e}",
                        self.job.handle
                    ),
                )
            })?;
        }
        if self.job.route == Route::Live {
            return crate::live::drive::run(self);
        }
        self.check_stop()?;
        if self.job.omp_bin.is_none() {
            return Err(Failure::new("internal", "job has no omp binary"));
        }
        let mut session_lock = None;
        if self.job.route == Route::Rpc {
            let id = self
                .job
                .session_id
                .as_deref()
                .ok_or_else(|| Failure::new("internal", "resumed job has no session id"))?
                .to_owned();
            self.log("lock", "waiting for glen session lock");
            let home = self.store.home.clone();
            let acquired =
                lock::acquire(&home, &id, self.deadline, &|| self.stop_requested(), || {
                    let mut status = self.status.clone();
                    status.waiting_for_lock = true;
                    status.waited_ms = Some(1);
                    self.store
                        .write_status(&self.id, &status)
                        .map_err(std::io::Error::other)
                })?;
            match acquired {
                lock::Acquired::Locked { lock, waited } => {
                    self.mark_holder(&lock);
                    session_lock = Some(lock);
                    self.status.waited_ms = Some(waited.as_millis().min(u64::MAX as u128) as u64);
                    self.status.waiting_for_lock = false;
                    self.save()?;
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
            let followed = continuation::follow(path).map_err(|e| {
                Failure::new(
                    "io",
                    format!("cannot follow session {}: {e}", path.display()),
                )
            })?;
            let s = Session::load(&followed.path).map_err(|e| Failure::new("io", e.to_string()))?;
            if Some(&s.header.id) != self.job.session_id.as_ref() {
                return Err(self.moved(&s.header.id));
            }
            self.probe()?;
        }
        self.check_stop()?;
        self.log("spawn", "starting omp RPC");
        let sink = ReplySink::new(self.store.clone(), self.store.home.clone(), self.id.clone());
        let Some(started) = self.start()? else {
            // Cancellation is already durable. The startup thread has joined and dropped its client.
            return Ok(());
        };
        let Started {
            client,
            events,
            baseline,
            warnings,
            replies,
        } = started;
        self.omp_queued = baseline.queued_message_count > 0;
        self.status.omp_pid = client.pid();
        self.status.rpc_protocol = Some(client.protocol_version());
        self.status.warnings.extend(warnings);
        self.save()?;
        // Drain at the source before adding a forwarder, so buffered startup frames cannot lag into the run.
        for _ in events.try_iter() {}
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
            if self.job.route == Route::New
                && let Some(lock) = &session_lock
            {
                self.mark_holder(lock);
            }
            self.status.actual_session_id = Some(baseline.session_id.clone());
            self.status.actual_session_file =
                baseline.session_file.clone().map(std::path::PathBuf::from);
            sink.set_session(baseline.session_id.clone());
            // Startup recovery can claim our own lease. Only the pre-spawn lease probe is safe.
            self.check_stop()?;
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
            self.drive(&client, &events, &replies, &sink, &baseline.session_id)
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
    fn start(&mut self) -> Result<Option<Started>, Failure> {
        let abandon = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(AtomicBool::new(false));
        let job = self.job.clone();
        let deadline = self.deadline;
        let ready_deadline = Instant::now() + rpc::READY_TIMEOUT;
        std::thread::scope(|scope| {
            let (tx, rx) = mpsc::channel();
            let startup_abandon = abandon.clone();
            let startup_ready = ready.clone();
            let startup = scope.spawn(move || {
                let result = Started::spawn(
                    &job,
                    deadline,
                    ready_deadline,
                    &startup_ready,
                    &startup_abandon,
                );
                if let Err(undelivered) = tx.send(result) {
                    // A late Started owns its Client; dropping it tears omp down.
                    drop(undelivered);
                }
            });
            let result = loop {
                if let Err(failure) = self.check_start_stop(ready_deadline, &ready) {
                    abandon.store(true, Ordering::SeqCst);
                    // Persist and acknowledge before joining the bounded blocking call.
                    let finished = self.finish_failure(failure);
                    break finished.map(|()| None);
                }
                match rx.recv_timeout(rpc::TICK) {
                    Ok(result) => {
                        if let Err(failure) = self.check_start_stop(ready_deadline, &ready) {
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
    fn check_start_stop(&self, ready_deadline: Instant, ready: &AtomicBool) -> Result<(), Failure> {
        let stop = self.check_stop();
        // Preserve the first bound even while Client::spawn is tearing down a never-ready peer.
        if !stop.as_ref().is_err_and(|f| f.code == "cancelled")
            && !ready.load(Ordering::SeqCst)
            && ready_deadline < self.deadline
            && Instant::now() >= ready_deadline
        {
            Err(Failure::new(
                "omp_unavailable",
                "omp did not become ready within 60s (see worker.log)",
            ))
        } else {
            stop
        }
    }
    fn apply_state(&mut self, s: omp_rpc::SessionState, baseline: &str, sink: &ReplySink) {
        self.omp_queued = s.queued_message_count > 0;
        if s.session_id != baseline {
            self.status.forked = Some(Forked {
                from: baseline.into(),
                to: s.session_id.clone(),
            });
        }
        sink.set_session(s.session_id.clone());
        self.status.actual_session_id = Some(s.session_id);
        self.status.actual_session_file = s.session_file.map(std::path::PathBuf::from);
    }
    fn event(&mut self, ev: Event, run: &mut Run) -> Result<(), Failure> {
        match ev {
            Event::Notification(RpcNotification::RpcAgentEvent(ev)) => {
                self.accept()?;
                if let RpcAgentEvent::Notice(n) = &ev
                    && n.source.as_deref() == Some("session-persistence")
                {
                    run.recheck_requested = true;
                    if n.level.as_str() == "error" {
                        self.status.warnings.push(Warning {
                            code: "persistence_error".into(),
                            message: n.message.clone(),
                        });
                    }
                }
                run.events.push(ev);
            }
            Event::Notification(RpcNotification::PromptResult(r)) => {
                run.prompt = Some(collected_turn(std::mem::take(&mut run.events), Some(r)));
                run.completed_at = Some(Instant::now());
                self.accept()?;
            }
            Event::Notification(RpcNotification::SessionSettled(_)) => run.settled_seen = true,
            Event::Notification(RpcNotification::SessionInfoUpdate(u))
                if Some(&u.session_id) != self.status.actual_session_id.as_ref() =>
            {
                run.recheck_requested = true;
            }
            Event::Notification(RpcNotification::CommandOutput(c)) => {
                run.command_output.push(c.text)
            }
            // A timed-out SDK call has no worker sequence attached to its unmatched response.
            // Only sequenced call results may publish identity; late frames are never authoritative.
            Event::UnmatchedResponse(r) if r.command == "get_state" => (),
            Event::UnmatchedResponse(r) if r.command == "prompt" => {
                if r.success {
                    run.ack(PromptCommand::decode(r.data).map_err(omp_rpc::Error::Json));
                } else {
                    run.prompt = Some(Err(omp_rpc::Error::Command {
                        command: r.command,
                        error: r.error.unwrap_or_default(),
                        code: r.code,
                    }));
                }
            }
            Event::Undecodable { raw, error } => {
                if raw["type"] == "prompt_result" {
                    run.prompt = Some(Err(omp_rpc::Error::Protocol(error)));
                } else {
                    self.log("decode", &error);
                }
            }
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
        events: &Receiver<Event>,
        replies: &Receiver<reply::PendingReply>,
        sink: &ReplySink,
        baseline: &str,
    ) -> Result<(), Failure> {
        let cmd = PromptCommand {
            message: self.job.text.clone(),
            images: None,
            streaming_behavior: Some(rpc::streaming_behavior(self.job.deliver_as)),
        };
        let phase = Mutex::new(Submission::Pending);
        let delay = test_delay("FAKE_OMP_PROMPT_SUBMIT_DELAY_MS");
        let live_home = self.store.home.clone();
        let live_id = self.job.session_id.clone();
        let live_handle = self.job.session_handle.clone();
        let mut run = Run::default();
        let mut ack_pending = true;
        let (control_tx, control_rx) =
            mpsc::channel::<(control::Request, Result<Value, crate::ops::OpError>)>();
        let result = std::thread::scope(|scope| -> Result<(), Failure> {
            let (tx, rx) = mpsc::channel();
            let phase = &phase;
            scope.spawn(move || {
                let end = Instant::now() + delay;
                while Instant::now() < end {
                    if *phase
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        == Submission::Cancelled
                    {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                if !submit_gate(|| {
                    *phase
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        == Submission::Cancelled
                }) {
                    return;
                }
                // Startup may claim our RPC child's own lease. Check only the endpoint,
                // on this thread so a slow hello cannot block cancellation on the worker.
                let endpoint =
                    endpoint_check(&live_home, live_id.as_deref(), live_handle.as_deref());
                // Keep submission and cancellation ordered through the actual enqueue, not merely
                // a flag checked before calling the SDK. The ack wait is capped at 100ms.
                let mut guard = phase
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *guard == Submission::Cancelled {
                    return;
                }
                if let Err(failure) = endpoint {
                    drop(guard);
                    if tx.send(Err(failure)).is_err() {
                        eprintln!("worker stopped waiting for endpoint check");
                    }
                    return;
                }
                *guard = Submission::Submitted;
                let ack = client.call(&SubmitPrompt(cmd));
                drop(guard);
                std::thread::sleep(test_delay("FAKE_OMP_ACK_FORWARD_DELAY_MS"));
                if tx.send(Ok(ack)).is_err() {
                    eprintln!("worker prompt ack receiver closed");
                }
            });
            let (state_tx, state_rx) = mpsc::channel::<(
                u64,
                bool,
                Result<omp_rpc::SessionState, omp_rpc::Error>,
                Option<reply::PendingReply>,
            )>();
            let mut states = StateSequence::default();
            let mut reply_queue = VecDeque::new();
            let mut reply_pending = false;
            let mut state_pending = None;
            let mut final_checked = false;
            let mut final_sequence = None;
            // Terminal status is published before joining any outstanding state call.
            let mut persistence_failure = None;
            loop {
                // Read the SDK receiver directly. Output preceding an ack is already queued
                // here when the ack arrives; there is no asynchronous forwarder to overtake.
                match events.recv_timeout(Duration::from_millis(50)) {
                    Ok(ev) => {
                        if let Err(e) = self.event(ev, &mut run) {
                            persistence_failure = Some(e);
                            run.stop = Some(StopReason::Cancelled);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        run.closed = true;
                        // EOF stays ready forever; other channels still need a bounded polling tick.
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                }
                match rx.try_recv() {
                    Ok(Ok(ack)) => {
                        ack_pending = false;
                        run.ack(ack);
                    }
                    Ok(Err(failure)) => return self.finish_failure(failure),
                    Err(mpsc::TryRecvError::Disconnected) => ack_pending = false,
                    Err(mpsc::TryRecvError::Empty) => (),
                }
                // The submission thread may already own a successful local acknowledgement.
                // Reconcile it before turning EOF into a transport failure.
                if run.closed && !ack_pending && run.prompt.is_none() {
                    run.prompt = Some(Err(omp_rpc::Error::Closed));
                }
                while let Ok((sequence, final_query, state, reply)) = state_rx.try_recv() {
                    if state_pending == Some(sequence) {
                        state_pending = None;
                    }
                    let checked = match state {
                        Ok(s) => {
                            if states.apply(sequence) {
                                self.apply_state(s, baseline, sink);
                            }
                            Ok(())
                        }
                        Err(e) => {
                            self.log("get_state", &e.to_string());
                            Err(GlenError::new(
                                ErrorCode::JobFailed,
                                format!("cannot check session {baseline} for glen_reply: {e}"),
                            ))
                        }
                    };
                    if let Some(reply) = reply {
                        reply_pending = false;
                        reply::PendingReply::finish(reply, sink, checked);
                    }
                    if final_query {
                        final_checked = true;
                    }
                }
                // Host tools run on SDK threads. Resolve identity after receiving each reply,
                // before appending it, even if an earlier notice query is still in flight.
                reply_queue.extend(replies.try_iter());
                if !reply_pending && let Some(reply) = reply_queue.pop_front() {
                    // Preserve host-call arrival order while notice queries can overlap.
                    reply_pending = true;
                    let sequence = states.issue();
                    let tx = state_tx.clone();
                    scope.spawn(move || {
                        let state = client.call(&StateSecond(GetStateCommand {}));
                        if tx.send((sequence, false, state, Some(reply))).is_err() {
                            eprintln!("worker reply identity receiver closed");
                        }
                    });
                }
                if run.recheck_requested && state_pending.is_none() {
                    run.recheck_requested = false;
                    let sequence = states.issue();
                    state_pending = Some(sequence);
                    let tx = state_tx.clone();
                    let left = self.deadline.saturating_duration_since(Instant::now());
                    scope.spawn(move || {
                        if tx
                            .send((sequence, false, query_state(client, left), None))
                            .is_err()
                        {
                            eprintln!("worker state receiver closed");
                        }
                    });
                }
                while let Ok((req, result)) = control_rx.try_recv() {
                    self.answer_control(
                        &req,
                        result.map(Some).map_err(|e| {
                            crate::ctl::exec::job_error(e, self.status.status.is_terminal())
                        }),
                    );
                    self.control_in_flight = None;
                }
                let (abort, pending) = self.controls();
                if self.cancel.load(Ordering::SeqCst) || abort {
                    run.stop = Some(StopReason::Cancelled);
                }
                if Instant::now() >= self.deadline && run.prompt.is_none() && run.stop.is_none() {
                    run.stop = Some(StopReason::Timeout);
                }
                if run.stop.is_none()
                    && self.control_in_flight.is_none()
                    && let Some(req) = pending.into_iter().next()
                    && let Ok(control::ControlOp::Ctl(op)) = &req.op
                {
                    if matches!(op, crate::ops::Op::Compact(_)) {
                        self.answer_control(&req, Err(crate::ops::OpError::new(crate::ops::BUSY, "a glen job is running on this session; wait for it (`glen wait <j>`) or `glen abort <session>` first")));
                    } else if control::expired(&req) {
                        self.answer_control(&req, Err(control::expired_error()));
                    } else {
                        self.control_in_flight = Some(req.path.clone());
                        let op = op.clone();
                        let tx = control_tx.clone();
                        let deadline = req
                            .deadline_ms
                            .and_then(|end| {
                                Instant::now().checked_add(Duration::from_millis(
                                    end.saturating_sub(
                                        jiff::Timestamp::now().as_millisecond().max(0) as u64,
                                    ),
                                ))
                            })
                            .map_or(self.deadline, |request| request.min(self.deadline));
                        let stop = Arc::clone(&self.cancel);
                        scope.spawn(move || {
                            let result = crate::ctl::exec::exec_until(
                                client,
                                &op,
                                req.expect_session_id.as_deref(),
                                Some(deadline),
                                &stop,
                                None,
                            );
                            if tx.send((req, result)).is_err() {
                                eprintln!("worker control receiver closed");
                            }
                        });
                    }
                }
                if run.stop.is_some() {
                    let mut guard = phase
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if *guard == Submission::Pending {
                        *guard = Submission::Cancelled;
                        run.prompt = Some(Err(omp_rpc::Error::Closed));
                    }
                }
                match next(&run, Instant::now(), self.deadline) {
                    Next::Wait => (),
                    Next::SendAbort => {
                        self.log("abort", "aborting omp");
                        run.abort_sent_at = Some(Instant::now());
                        if let Err(e) = client.call(&PollAbort(AbortCommand {})) {
                            self.log("abort", &e.to_string());
                        }
                    }
                    Next::HardKill => {
                        if let Some(pid) = client.pid() {
                            if let Err(e) = rpc::kill_group(pid, rustix::process::Signal::TERM) {
                                self.log("kill", &e.to_string());
                            }
                            std::thread::sleep(rpc::KILL_GRACE);
                            if let Err(e) = rpc::kill_group(pid, rustix::process::Signal::KILL) {
                                self.log("kill", &e.to_string());
                            }
                        }
                        run.killed = true;
                    }
                    Next::Finish(t) => {
                        if t.state == State::Aborted
                            && run.stop == Some(StopReason::Cancelled)
                            && self.controls().0
                        {
                            match client.call(&PollState(GetStateCommand {})) {
                                Ok(state) => self.apply_state(state, baseline, sink),
                                Err(error) => self.log("abort state", &error.to_string()),
                            }
                        }
                        // Source drain is the completion barrier, including command_output
                        // emitted immediately before a local ack.
                        for ev in events.try_iter() {
                            if let Err(e) = self.event(ev, &mut run) {
                                persistence_failure = Some(e);
                            }
                        }
                        if t.state == State::Completed && !final_checked {
                            if final_sequence.is_none() {
                                let sequence = states.issue();
                                final_sequence = Some(sequence);
                                let tx = state_tx.clone();
                                scope.spawn(move || {
                                    // Completion always gets its own budget, including cancellation
                                    // after a result and an exhausted absolute job deadline.
                                    let state = client.call(&StateSecond(GetStateCommand {}));
                                    if tx.send((sequence, true, state, None)).is_err() {
                                        eprintln!("worker final state receiver closed");
                                    }
                                });
                            }
                            continue;
                        }
                        if let Some(e) = persistence_failure {
                            return self.finish_failure(e);
                        }
                        return self.finish_run(&run, t, sink);
                    }
                }
            }
        });
        // The terminal publication above skips the in-flight request. Only its
        // actual result may acknowledge it after the scoped thread has joined.
        while let Ok((req, answer)) = control_rx.try_recv() {
            self.answer_control(
                &req,
                answer
                    .map(Some)
                    .map_err(|e| crate::ctl::exec::job_error(e, self.status.status.is_terminal())),
            );
            self.control_in_flight = None;
        }
        if self.control_in_flight.take().is_some() {
            self.respond_pending();
        }
        result
    }
    fn finish_run(
        &mut self,
        run: &Run,
        terminal: Terminal,
        sink: &ReplySink,
    ) -> Result<(), Failure> {
        if terminal.settle_timeout {
            self.status.warnings.push(Warning {
                code: if run.stop == Some(StopReason::Cancelled) {
                    "settle_cancelled"
                } else {
                    "settle_timeout"
                }
                .into(),
                message: "omp completed but the settlement wait ended before the session settled"
                    .into(),
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
            command_output: run.command_output.clone(),
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
        self.map_sandbox_error();
        self.save()?;
        self.respond_pending();
        self.log("finish", self.status.status.as_str());
        Ok(())
    }
}
fn endpoint_check(
    home: &glen_home::GlenHome,
    id: Option<&str>,
    handle: Option<&str>,
) -> Result<(), Failure> {
    let Some(id) = id else {
        return Ok(());
    };
    endpoint_result(
        crate::live::route::find(home, id, crate::live::HELLO_PROBE),
        handle.unwrap_or(id),
        &Env::from_process(),
    )
}
fn endpoint_result(
    endpoint: crate::live::route::Endpoint,
    handle: &str,
    env: &Env,
) -> Result<(), Failure> {
    match endpoint {
        crate::live::route::Endpoint::Unhealthy { kind, io, .. } => {
            if let Some(err) = crate::live::route::unhealthy_sandbox(env, "send", kind, io) {
                let mut failure = Failure::no_route(NoRouteReason::Sandboxed, err.message);
                failure.hint = err.hint;
                failure.details = err.details.map(|d| *d);
                return Err(failure);
            }
            let mut failure = Failure::no_route(
                NoRouteReason::UnhealthyEndpoint,
                format!("session {handle} was opened in a live omp session after glen’s check"),
            );
            failure.hint = Some("send again; glen will deliver through the live session".into());
            Err(failure)
        }
        crate::live::route::Endpoint::Live { .. } => {
            let mut failure = Failure::no_route(
                NoRouteReason::EndpointAppeared,
                format!("session {handle} was opened in a live omp session after glen’s check"),
            );
            failure.hint = Some("send again; glen will deliver through the live session".into());
            Err(failure)
        }
        _ => Ok(()),
    }
}

fn route_probe(
    lease: Lease,
    handle: &str,
    env: &Env,
    profile: Option<&str>,
) -> Result<(), Failure> {
    // Checked before every lease value, including Free: on Linux under any Codex marker the
    // lease may be hidden by the sandbox's network namespace, so even "free" cannot be trusted.
    if let Some(e) = crate::sandbox::map_probe_failure(env, "send", &lease) {
        let mut failure = Failure::no_route(NoRouteReason::Sandboxed, e.message);
        failure.hint = e.hint;
        failure.details = e.details.map(|d| *d);
        return Err(failure);
    }
    if matches!(lease, Lease::Free) {
        return Ok(());
    }
    let paths = crate::omp::paths::resolve(env, profile)?;
    let e = crate::live::route::lease_refusal(&lease, handle, &paths);
    let mut f = Failure::new(
        e.code.as_str(),
        format!("before spawning omp: {}", e.message),
    );
    f.hint = e.hint;
    f.details = e.details.map(|d| *d);
    Err(f)
}
struct Started {
    client: Client,
    events: Receiver<Event>,
    baseline: omp_rpc::SessionState,
    warnings: Vec<Warning>,
    replies: Receiver<reply::PendingReply>,
}
impl Started {
    fn spawn(
        job: &Job,
        deadline: Instant,
        ready_deadline: Instant,
        ready: &AtomicBool,
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
                ready_timeout: deadline
                    .min(ready_deadline)
                    .saturating_duration_since(Instant::now()),
                default_timeout: rpc::CALL_TIMEOUT,
            },
        );
        check()?;
        let (client, events) = spawned.map_err(|e| {
            let code = if matches!(e, omp_rpc::Error::Timeout { .. }) && deadline <= ready_deadline
            {
                "timeout"
            } else if matches!(
                e,
                omp_rpc::Error::Io(_) | omp_rpc::Error::Timeout { .. } | omp_rpc::Error::Closed
            ) {
                "omp_unavailable"
            } else {
                "rpc_error"
            };
            Failure::new(code, format!("cannot start omp: {e} (see worker.log)"))
        })?;
        ready.store(true, Ordering::SeqCst);
        let (reply_tx, replies) = mpsc::channel();
        let tools = client.set_custom_tools(vec![reply::glen_reply_tool(reply_tx)]);
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
            replies,
        })
    }
}
// Integration tests execute the debug binary. Release workers never read fake-peer knobs.
fn test_delay(key: &str) -> Duration {
    #[cfg(debug_assertions)]
    {
        Env::from_process()
            .get(key)
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|n| *n <= 20_000)
            .map(Duration::from_millis)
            .unwrap_or_default()
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = key;
        Duration::ZERO
    }
}
// Debug workers only: with FAKE_OMP_PROMPT_SUBMIT_GATE=<path>, the submission thread writes
// `<path>.ready` and waits (20 s bound) until a test creates <path>, so a test can change the
// endpoint before the submission-time check without racing a wall-clock delay. Returns false when
// `cancelled` reports a cancellation during the wait.
fn submit_gate(cancelled: impl Fn() -> bool) -> bool {
    #[cfg(debug_assertions)]
    if let Some(path) = Env::from_process().get("FAKE_OMP_PROMPT_SUBMIT_GATE") {
        let gate = std::path::Path::new(&path);
        if let Err(e) = std::fs::write(format!("{path}.ready"), b"submitting") {
            eprintln!("cannot write submit gate {path}.ready: {e}");
        }
        let end = Instant::now() + Duration::from_secs(20);
        while !gate.exists() && Instant::now() < end {
            if cancelled() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = &cancelled;
    true
}
use super::polling_command;
polling_command!(SubmitPrompt, PromptCommand, 100);
polling_command!(PollState, GetStateCommand, 100);
polling_command!(StateSecond, GetStateCommand, 1000);
fn query_state(client: &Client, left: Duration) -> Result<omp_rpc::SessionState, omp_rpc::Error> {
    if left >= rpc::CALL_TIMEOUT {
        client.call(&GetStateCommand {})
    } else if left >= Duration::from_secs(1) {
        client.call(&StateSecond(GetStateCommand {}))
    } else if left >= Duration::from_millis(100) {
        client.call(&PollState(GetStateCommand {}))
    } else {
        Err(omp_rpc::Error::Timeout {
            command: "get_state".into(),
        })
    }
}
polling_command!(PollAbort, AbortCommand, 100);
#[derive(PartialEq, Eq)]
enum Submission {
    Pending,
    Submitted,
    Cancelled,
}

// Keep the SDK's terminal-message semantics while collecting from its ordered event receiver.
fn collected_turn(
    events: Vec<RpcAgentEvent>,
    result: Option<omp_rpc::PromptResultEvent>,
) -> Result<PromptTurn, omp_rpc::Error> {
    let mut messages = Vec::new();
    if let Some(end) = events
        .iter()
        .rposition(|e| matches!(e, RpcAgentEvent::AgentEnd(_)))
        && let RpcAgentEvent::AgentEnd(terminal) = &events[end]
    {
        messages = terminal.messages.clone();
        if let Some(count) = terminal
            .message_count
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n > messages.len())
        {
            let start = events[..end]
                .iter()
                .rposition(|e| matches!(e, RpcAgentEvent::AgentStart(_)))
                .map_or(0, |i| i + 1);
            let streamed: Vec<_> = events[start..end]
                .iter()
                .filter_map(|e| {
                    if let RpcAgentEvent::MessageEnd(m) = e {
                        Some(m.message.clone())
                    } else {
                        None
                    }
                })
                .collect();
            let prefix = count - messages.len();
            if prefix > streamed.len() {
                return Err(omp_rpc::Error::Protocol(
                    "compacted agent_end references missing streamed messages".into(),
                ));
            }
            messages = streamed.into_iter().take(prefix).chain(messages).collect();
        }
    }
    let assistant_message = messages
        .iter()
        .rev()
        .find_map(|m| {
            if let AgentMessage::Assistant(a) = m {
                Some(a.clone())
            } else {
                None
            }
        })
        .or_else(|| {
            events.iter().rev().find_map(|e| {
                let m = match e {
                    RpcAgentEvent::MessageStart(e) => &e.message,
                    RpcAgentEvent::MessageUpdate(e) => &e.message,
                    RpcAgentEvent::MessageEnd(e) => &e.message,
                    RpcAgentEvent::TurnEnd(e) => &e.message,
                    _ => return None,
                };
                if let AgentMessage::Assistant(a) = m {
                    Some(a.clone())
                } else {
                    None
                }
            })
        });
    let assistant_text = assistant_message
        .as_ref()
        .map(|m| {
            m.content
                .iter()
                .flatten()
                .filter_map(|b| {
                    if let AssistantContent::Text(t) = b {
                        t.text.as_deref()
                    } else {
                        None
                    }
                })
                .collect::<String>()
        })
        .filter(|s| !s.is_empty());
    Ok(PromptTurn {
        events,
        messages,
        assistant_message,
        assistant_text,
        result,
    })
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum StopReason {
    Cancelled,
    Timeout,
}
// A later observation wins. Timed-out/unmatched SDK responses are deliberately ignored.
#[derive(Default)]
struct StateSequence {
    issued: u64,
    applied: u64,
}
impl StateSequence {
    fn issue(&mut self) -> u64 {
        self.issued += 1;
        self.issued
    }
    fn apply(&mut self, sequence: u64) -> bool {
        if sequence <= self.applied {
            return false;
        }
        self.applied = sequence;
        true
    }
}
#[derive(Default)]
struct Run {
    prompt: Option<Result<PromptTurn, omp_rpc::Error>>,
    events: Vec<RpcAgentEvent>,
    completed_at: Option<Instant>,
    recheck_requested: bool,
    settled_seen: bool,
    stop: Option<StopReason>,
    abort_sent_at: Option<Instant>,
    killed: bool,
    command_output: Vec<String>,
    closed: bool,
}
impl Run {
    fn ack(&mut self, ack: Result<PromptAck, omp_rpc::Error>) {
        match ack {
            Ok(a) if a.agent_invoked == Some(false) && self.prompt.is_none() => {
                self.prompt = Some(collected_turn(std::mem::take(&mut self.events), None));
            }
            Err(omp_rpc::Error::Timeout { .. }) | Ok(_) => (), // A late ack/result still arrives on events.
            Err(e) if self.prompt.is_none() => self.prompt = Some(Err(e)),
            Err(_) => (),
        }
    }
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
                        } else if now >= deadline
                            || run.closed
                            || run.stop.is_some()
                            || (!r.agent_invoked
                                && run.completed_at.is_some_and(|t| {
                                    now.saturating_duration_since(t) >= rpc::ABORT_GRACE
                                }))
                        {
                            Terminal::completed(false)
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
    #[test]
    fn cycle3_closed_completed_result_does_not_wait_for_deadline() {
        let now = Instant::now();
        let run = Run {
            prompt: Some(Ok(turn(Some(PromptStatus::Completed), false))),
            closed: true,
            ..Run::default()
        };
        assert!(matches!(
            next(&run, now, now + Duration::from_secs(3600)),
            Next::Finish(Terminal {
                state: State::Completed,
                settled: false,
                settle_timeout: true,
                ..
            })
        ));
    }
    #[test]
    fn cycle3_state_sequences_reject_older_results_and_unmatched_frames() {
        let mut sequences = StateSequence::default();
        let old = sequences.issue();
        let new = sequences.issue();
        assert!(sequences.apply(new));
        assert!(!sequences.apply(old));
        assert!(!sequences.apply(new));
        let newest = sequences.issue();
        assert!(sequences.apply(newest));

        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        let job = crate::jobs::tests::job();
        let id = JobId::parse(&job.id).unwrap();
        store.create(&job).unwrap();
        let sink = ReplySink::new(store.clone(), store.home.clone(), id.clone());
        sink.set_session("new".into());
        let mut worker = Worker {
            status: Status {
                actual_session_id: Some("new".into()),
                ..store.read_status(&id).unwrap()
            },
            store,
            id,
            job,
            control_in_flight: None,
            omp_queued: false,
            cancel: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + Duration::from_secs(10),
        };
        let frame = serde_json::from_value(
            json!({"id":"old-query", "command":"get_state", "success":true,
            "data":{"sessionId":"old", "isStreaming":false,"isSettled":false,"messageCount":0}}),
        )
        .unwrap();
        worker
            .event(Event::UnmatchedResponse(frame), &mut Run::default())
            .unwrap();
        assert_eq!(worker.status.actual_session_id.as_deref(), Some("new"));
        sink.record("late reply".into(), true, "call".into())
            .unwrap();
        assert_eq!(sink.replies()[0].session_id, "new");
    }
    #[cfg(not(debug_assertions))]
    #[test]
    fn cycle3_release_worker_has_no_fake_delay() {
        assert_eq!(
            test_delay("FAKE_OMP_PROMPT_SUBMIT_DELAY_MS"),
            Duration::ZERO
        );
        assert_eq!(test_delay("FAKE_OMP_ACK_FORWARD_DELAY_MS"), Duration::ZERO);
    }
    #[test]
    fn startup_bounds_preserve_first_timeout_during_teardown() {
        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        let job = crate::jobs::tests::job();
        let id = JobId::parse(&job.id).unwrap();
        store.create(&job).unwrap();
        let now = Instant::now();
        let mut worker = Worker {
            status: store.read_status(&id).unwrap(),
            store,
            id,
            job,
            control_in_flight: None,
            omp_queued: false,
            cancel: Arc::new(AtomicBool::new(false)),
            deadline: now - Duration::from_secs(1),
        };
        let ready_deadline = now - Duration::from_secs(2);
        assert_eq!(
            worker
                .check_start_stop(ready_deadline, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            "omp_unavailable"
        );
        // A peer that already became ready remains subject to the job deadline.
        assert_eq!(
            worker
                .check_start_stop(ready_deadline, &AtomicBool::new(true))
                .unwrap_err()
                .code,
            "timeout"
        );
        worker.deadline = now - Duration::from_secs(3);
        assert_eq!(
            worker
                .check_start_stop(ready_deadline, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            "timeout"
        );
        worker.deadline = now + Duration::from_secs(10);
        assert_eq!(
            worker
                .check_start_stop(ready_deadline, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            "omp_unavailable"
        );
        assert!(
            worker
                .check_start_stop(ready_deadline, &AtomicBool::new(true))
                .is_ok()
        );
        worker.cancel.store(true, Ordering::SeqCst);
        assert_eq!(
            worker
                .check_start_stop(ready_deadline, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            "cancelled"
        );
    }
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
    fn cycle2_worker_probe_uses_raw_permission_errno() {
        let env = Env::for_test(
            None,
            std::path::Path::new("/"),
            &[("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
        );
        // The raw-errno mapping is the non-hidden path (macOS); on Linux the marker hides the
        // lease first (W10 fix cycle 2 decision 1; see the next test).
        crate::sandbox::force_lease_hidden_for_test(Some(false));
        for errno in [libc::EPERM, libc::EACCES, libc::EROFS] {
            let error = route_probe(
                Lease::Unknown {
                    reason: "backend diagnostic".into(),
                    errno: Some(errno),
                },
                "e27c0de",
                &env,
                None,
            )
            .unwrap_err();
            assert_eq!(error.code, "no_route");
            assert_eq!(error.details.unwrap()["reason"], "sandboxed");
            assert!(error.hint.unwrap().contains("outside the sandbox"));
        }
        crate::sandbox::force_lease_hidden_for_test(None);
    }
    #[test]
    fn route_probe_refuses_even_a_free_lease_when_hidden() {
        crate::sandbox::force_lease_hidden_for_test(Some(true));
        let env = Env::for_test(
            None,
            std::path::Path::new("/"),
            &[("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
        );
        let error = route_probe(Lease::Free, "e27c0de", &env, None).unwrap_err();
        assert_eq!(error.code, "no_route");
        let details = error.details.unwrap();
        assert_eq!(details["reason"], "sandboxed");
        assert_eq!(details["lease"], "unknown");
        crate::sandbox::force_lease_hidden_for_test(None);
        // The real rule (W10 fix cycle 2 decision 1): any marker on Linux, never elsewhere.
        assert_eq!(
            route_probe(Lease::Free, "e27c0de", &env, None).is_err(),
            cfg!(target_os = "linux")
        );
    }
    #[test]
    fn review11_refusal_identifies_pre_spawn_phase() {
        let tree = tempfile::tempdir().unwrap();
        let env = Env::for_test(Some(tree.path()), std::path::Path::new("/"), &[]);
        let paths = crate::omp::paths::resolve(&env, None).unwrap();
        for (lease, reason) in [
            (Lease::Held, "live_owner"),
            (
                Lease::Unknown {
                    reason: "denied".into(),
                    errno: None,
                },
                "unknown_lease",
            ),
        ] {
            let e = route_probe(lease, "e27c0de", &env, None).unwrap_err();
            assert_eq!(e.code, "no_route");
            assert!(e.error_message.starts_with("before spawning omp:"));
            assert!(e.error_message.contains("e27c0de"));
            assert_eq!(e.details.unwrap()["reason"], reason);
            // W8-DEBT-1: the worker's hint is the shared lease hint, not "a later glen version".
            assert_eq!(e.hint, Some(crate::live::route::lease_held_hint(&paths)));
        }
        assert!(
            route_probe(
                Lease::Free,
                "e27c0de",
                &Env::for_test(None, std::path::Path::new("/"), &[]),
                None,
            )
            .is_ok()
        );
    }
    #[test]
    fn endpoint_check_distinguishes_appeared_unhealthy_and_sandboxed() {
        use crate::live::route::{Endpoint, Unhealthy};
        let outside = Env::for_test(None, std::path::Path::new("/"), &[]);
        let reg = serde_json::from_value(json!({"v":1,"pid":1,"session_id":"s","socket":"/tmp/s"}))
            .unwrap();
        let appeared = endpoint_result(
            Endpoint::Live {
                reg: Box::new(reg),
                hello: Box::default(),
            },
            "e27c0de",
            &outside,
        )
        .unwrap_err();
        assert_eq!(appeared.code, "no_route");
        assert_eq!(appeared.details.unwrap()["reason"], "endpoint_appeared");
        assert!(appeared.hint.unwrap().contains("send again"));
        for (kind, io) in [
            (Unhealthy::PathTooLong, None),
            (
                Unhealthy::Unresponsive,
                Some(std::io::Error::from_raw_os_error(libc::ETIMEDOUT)),
            ),
            (
                Unhealthy::Unreadable,
                Some(std::io::Error::from_raw_os_error(libc::EPERM)),
            ),
        ] {
            let error = endpoint_result(
                Endpoint::Unhealthy {
                    reason: "unavailable".into(),
                    kind,
                    io,
                },
                "e27c0de",
                &outside,
            )
            .unwrap_err();
            assert_eq!(error.details.unwrap()["reason"], "unhealthy_endpoint");
        }
        let sandbox = Env::for_test(
            None,
            std::path::Path::new("/"),
            &[("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
        );
        for kind in [Unhealthy::Unresponsive, Unhealthy::Unreadable] {
            let error = endpoint_result(
                Endpoint::Unhealthy {
                    reason: "denied".into(),
                    kind,
                    io: Some(std::io::Error::from_raw_os_error(libc::EPERM)),
                },
                "e27c0de",
                &sandbox,
            )
            .unwrap_err();
            assert_eq!(error.details.as_ref().unwrap()["reason"], "sandboxed");
            assert_eq!(error.details.as_ref().unwrap()["network"], false);
            assert!(error.hint.is_some());
        }
        assert!(endpoint_result(Endpoint::None, "e27c0de", &outside).is_ok());
        assert!(endpoint_result(Endpoint::Stale("gone".into()), "e27c0de", &outside).is_ok());
    }
    #[test]
    fn review9_completed_result_cancellation_table() {
        let now = Instant::now();
        let run = Run {
            prompt: Some(Ok(turn(Some(PromptStatus::Completed), false))),
            stop: Some(StopReason::Cancelled),
            ..Run::default()
        };
        let Next::Finish(t) = next(&run, now, now + Duration::from_secs(3600)) else {
            panic!("not terminal")
        };
        assert_eq!(t.state, State::Completed);
        assert!(!t.settled);
        assert!(t.settle_timeout);
        assert!(t.error.is_none());
    }
    #[test]
    fn review10_local_result_settlement_bound_table() {
        let now = Instant::now();
        let mut t = turn(Some(PromptStatus::Completed), false);
        t.result.as_mut().unwrap().agent_invoked = false;
        let mut run = Run {
            prompt: Some(Ok(t)),
            completed_at: Some(now),
            ..Run::default()
        };
        assert!(matches!(
            next(&run, now, now + Duration::from_secs(3600)),
            Next::Wait
        ));
        let Next::Finish(t) = next(
            &run,
            now + rpc::ABORT_GRACE,
            now + Duration::from_secs(3600),
        ) else {
            panic!("not bounded")
        };
        assert_eq!(t.state, State::Completed);
        assert!(!t.settled);
        run.settled_seen = true;
        assert!(matches!(
            next(&run, now, now + Duration::from_secs(3600)),
            Next::Finish(Terminal { settled: true, .. })
        ));
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

#[cfg(test)]
mod control_tests {
    use super::*;
    fn worker(root: &std::path::Path) -> Worker {
        let store = crate::jobs::tests::store(root);
        let job = crate::jobs::tests::job();
        let id = JobId::parse(&job.id).unwrap();
        store.create(&job).unwrap();
        Worker {
            status: store.read_status(&id).unwrap(),
            store,
            id,
            job,
            cancel: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + Duration::from_secs(10),
            control_in_flight: None,
            omp_queued: false,
        }
    }
    fn write(w: &Worker, name: &str, op: Value) -> std::path::PathBuf {
        let dir = w
            .store
            .home
            .ensure_dir(&format!("jobs/{}/control", w.id.as_str()))
            .unwrap();
        let path = dir.join(format!("{name}.json"));
        crate::jobs::write_json(&path, &op).unwrap();
        path
    }
    fn done(path: &std::path::Path) -> Value {
        serde_json::from_slice(&std::fs::read(path.with_extension("done.json")).unwrap()).unwrap()
    }
    #[test]
    fn controls_classify_without_dispatching_before_ready_and_expire_abort() {
        let temp = tempfile::tempdir().unwrap();
        let w = worker(temp.path());
        let first = write(&w, "1", json!({"v":1,"op":"list_models"}));
        write(
            &w,
            "2",
            json!({"v":1,"op":"set_thinking_level","args":{"level":"high"}}),
        );
        let expired = write(&w, "3", json!({"v":1,"op":"abort","deadline_ms":1}));
        let malformed = write(&w, "4", json!({"v":1,"op":"set_model"}));
        let mismatch = write(
            &w,
            "5",
            json!({"v":1,"op":"abort","args":{"expect_session_id":"other"}}),
        );
        let (abort, pending) = w.controls();
        assert!(!abort);
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].path, first);
        assert!(first.exists());
        assert_eq!(done(&expired)["error"]["code"], crate::ops::TIMEOUT);
        assert_eq!(done(&malformed)["error"]["code"], crate::ops::BAD_REQUEST);
        assert_eq!(
            done(&mismatch)["error"]["code"],
            crate::ops::SESSION_MISMATCH
        );
        write(&w, "6", json!({"v":1,"op":"abort"}));
        assert!(w.controls().0);
    }
    #[test]
    fn terminal_acknowledgements_skip_inflight_and_keep_typed_errors() {
        let temp = tempfile::tempdir().unwrap();
        let mut w = worker(temp.path());
        w.status.status = State::Aborted;
        w.omp_queued = true;
        let abort = write(&w, "1", json!({"v":1,"op":"abort"}));
        let deferred = write(&w, "2", json!({"v":1,"op":"list_models"}));
        let inflight = write(
            &w,
            "3",
            json!({"v":1,"op":"set_thinking_level","args":{"level":"high"}}),
        );
        w.control_in_flight = Some(inflight.clone());
        w.respond_pending();
        assert_eq!(
            done(&abort)["data"],
            json!({"job_status":"aborted","session_id":w.job.session_id,"queued":true})
        );
        assert_eq!(done(&deferred)["error"]["code"], crate::ops::JOB_ENDED);
        assert!(inflight.exists());
        assert!(!inflight.with_extension("done.json").exists());
        w.control_in_flight = None;
        w.respond_pending();
        assert_eq!(done(&inflight)["error"]["code"], crate::ops::JOB_ENDED);
    }
    #[test]
    fn holder_is_published_on_the_held_inode() {
        let temp = tempfile::tempdir().unwrap();
        let w = worker(temp.path());
        let id = w.job.session_id.as_deref().unwrap();
        let lock = lock::try_acquire(&w.store.home, id).unwrap().unwrap();
        w.mark_holder(&lock);
        let holder = lock::holder(&w.store.home, id).unwrap();
        assert_eq!(holder.kind, lock::HolderKind::Job);
        assert_eq!(holder.job_id.as_deref(), Some(w.id.as_str()));
        assert_eq!(holder.pid, std::process::id());
        assert!(lock::is_held(&w.store.home, id));
    }
}
