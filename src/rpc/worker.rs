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
        self.status.waiting_for_lock = false;
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
        self.check_stop()?;
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
        self.status.omp_pid = client.pid();
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
            self.status.actual_session_id = Some(baseline.session_id.clone());
            self.status.actual_session_file =
                baseline.session_file.clone().map(std::path::PathBuf::from);
            sink.set_session(baseline.session_id.clone());
            // Startup recovery can claim our own lease. Only the pre-spawn probe is safe.
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
        let mut run = Run::default();
        let mut ack_pending = true;
        std::thread::scope(|scope| -> Result<(), Failure> {
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
                // Keep submission and cancellation ordered through the actual enqueue, not merely
                // a flag checked before calling the SDK. The ack wait is capped at 100ms.
                let mut guard = phase
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *guard == Submission::Cancelled {
                    return;
                }
                *guard = Submission::Submitted;
                let ack = client.call(&SubmitPrompt(cmd));
                drop(guard);
                std::thread::sleep(test_delay("FAKE_OMP_ACK_FORWARD_DELAY_MS"));
                if tx.send(ack).is_err() {
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
                    Ok(ack) => {
                        ack_pending = false;
                        run.ack(ack);
                    }
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
                if self.stop_requested() {
                    run.stop = Some(StopReason::Cancelled);
                }
                if Instant::now() >= self.deadline && run.prompt.is_none() && run.stop.is_none() {
                    run.stop = Some(StopReason::Timeout);
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
                            kill_group(pid, rustix::process::Signal::TERM);
                            std::thread::sleep(rpc::KILL_GRACE);
                            kill_group(pid, rustix::process::Signal::KILL);
                        }
                        run.killed = true;
                    }
                    Next::Finish(t) => {
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
        })
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
        self.save()?;
        self.respond_pending();
        self.log("finish", self.status.status.as_str());
        Ok(())
    }
}
fn route_probe(lease: Lease, handle: &str) -> Result<(), Failure> {
    let message = match lease {
        Lease::Free => return Ok(()),
        Lease::Held => {
            format!("before spawning omp: session {handle} is open in another omp process")
        }
        Lease::Unknown(r) => {
            format!(
                "before spawning omp: cannot tell whether another omp process owns session {handle} ({r})"
            )
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
// These calls must not hold the polling thread (or submission gate) for CALL_TIMEOUT.
macro_rules! polling_command {
    ($name:ident, $inner:ty, $timeout:expr) => {
        #[derive(serde::Serialize)]
        #[serde(transparent)]
        struct $name($inner);
        impl Command for $name {
            const NAME: &'static str = <$inner as Command>::NAME;
            const TIMEOUT_MS: Option<u64> = Some($timeout);
            type Output = <$inner as Command>::Output;
            fn decode(data: Option<Value>) -> Result<Self::Output, serde_json::Error> {
                <$inner as Command>::decode(data)
            }
        }
    };
}
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
    fn review11_refusal_identifies_pre_spawn_phase() {
        for lease in [Lease::Held, Lease::Unknown("denied".into())] {
            let e = route_probe(lease, "e27c0de").unwrap_err();
            assert_eq!(e.code, "no_route");
            assert!(e.error_message.starts_with("before spawning omp:"));
            assert!(e.error_message.contains("e27c0de"));
        }
        assert!(route_probe(Lease::Free, "e27c0de").is_ok());
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
