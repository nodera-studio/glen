//! `glen __acp <session|@new> <message> [--permission allow|deny]` (hidden, v0.2 W3 spike; design
//! §A23): one message to omp over ACP v1, the reply rebuilt from `session/update`. An existing
//! session gets the RPC resume's guard (glen lock, endpoint and lease probes) before omp starts;
//! no job is created and glen writes no omp file.
use super::{open, send::send_cwd};
use crate::{
    acp::{
        self, Reason, Stage,
        client::{CallError, CancelReason, CancelState, Client, PromptOutcome},
    },
    cli::{AcpArgs, Cli},
    ctl::ephemeral,
    env::Env,
    error::{ErrorCode, GlenError, NoRouteReason, OkExit, Warning},
    glen_home::{self, GlenHome},
    jobs::{
        liveness,
        lock::{self, SessionLock},
        wait,
    },
    live,
    omp::{
        bin, continuation,
        lease::{self, Lease},
        paths::{self, OmpPaths},
        session::Session,
    },
    output::{Mode, Outcome, OutputCtx},
    render::{palette::Token, spinner::Spinner, text},
    rpc, sandbox,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

const COMMAND: &str = "__acp";

pub fn run(cli: &Cli, args: &AcpArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    let paths = paths::resolve(env, cli.global.profile.as_deref())?;
    validate(&args.message)?;
    let existing = args.session != "@new";
    let hidden = |e: GlenError| {
        if existing {
            sandbox::with_hidden_lease(env, e)
        } else {
            e
        }
    };
    sandbox::refuse_unreachable(env, COMMAND).map_err(hidden)?;
    // ACP never takes the live route, so every sandbox refuses it like an RPC resume.
    sandbox::refuse_rpc(env, COMMAND).map_err(hidden)?;
    let opened = if existing {
        Some(open::open_unprobed_in(cli, env, &args.session, None)?)
    } else {
        None
    };
    let cwd = acp_cwd(env, &send_cwd(opened.as_ref(), None, env)?)?;
    let home = glen_home::resolve_trusted(env, &["run/live"])?;
    let (timeout, warning) = rpc::job_timeout(env);
    let deadline = Instant::now() + timeout;
    let mut warnings: Vec<Warning> = warning.into_iter().collect();
    let target = match &opened {
        Some(o) => Some(guard(env, &home, &paths, o, deadline, &mut warnings)?),
        None => None,
    };
    let bin = bin::find_omp(env).map_err(|m| acp::error(Stage::Handshake, Reason::AcpSpawn, m))?;
    let stop = wait::install_interrupt_all()?;
    let (id, label, session, lock) = match target {
        Some(t) => {
            let path = t.path.as_deref().map(Path::to_string_lossy);
            let session = json!({"id":t.id,"handle":t.handle,"path":path,"created":false});
            (Some(t.id), t.handle, session, Some(t.lock))
        }
        None => (None, "@new".to_owned(), Value::Null, None),
    };
    let spinner = Spinner::start(
        ctx.mode == Mode::Human && ctx.stderr_tty,
        &format!("asking omp over ACP · {label}"),
    );
    let turn = Turn {
        env,
        home: &home,
        args,
        cwd: &cwd,
        existing: id.as_deref(),
        deadline,
        stop: &stop,
    };
    let profile = cli.global.profile.as_deref();
    let command = acp::command(&bin, profile, Path::new(&cwd));
    let report = talk(&turn, command, lock, session);
    spinner.finish();
    let mut report = report?;
    warnings.append(&mut report.warnings);
    report.warnings = warnings;
    finish(report, args, ctx)
}

/// omp's working directory for the turn, both the child's `current_dir` and the wire `cwd`:
/// absolute (omp rejects a relative `cwd`, and a stored header cwd can be relative) and UTF-8 (it
/// travels as a JSON string).
fn acp_cwd(env: &Env, cwd: &Path) -> Result<String, GlenError> {
    let utf8 = |path: &Path| {
        path.to_str().map(str::to_owned).ok_or_else(|| {
            GlenError::new(
                ErrorCode::Usage,
                format!(
                    "omp acp needs a UTF-8 working directory, not {}",
                    path.display()
                ),
            )
            .with_detail("reason", "non_utf8_cwd")
            .with_detail("cwd", path.to_string_lossy())
            .with_hint("run glen __acp from a directory whose path is valid UTF-8")
        })
    };
    utf8(&paths::absolute(env, &utf8(cwd)?))
}
fn validate(message: &str) -> Result<(), GlenError> {
    if message.trim().is_empty() {
        return Err(GlenError::new(ErrorCode::Usage, "message is empty"));
    }
    if message.len() > rpc::MAX_MESSAGE_BYTES {
        return Err(GlenError::new(
            ErrorCode::Usage,
            "message is larger than 512 KiB",
        ));
    }
    Ok(())
}

/// An existing session that passed the guard; the lock is held until omp's group is gone.
struct Target {
    id: String,
    handle: String,
    path: Option<PathBuf>,
    lock: SessionLock,
}
fn holder_warning(lock: &SessionLock, id: &str) -> Option<Warning> {
    let holder = lock::Holder {
        v: 1,
        kind: lock::HolderKind::Control,
        job_id: None,
        pid: std::process::id(),
        start: liveness::current().map(|i| i.start),
    };
    lock::mark(lock, &holder).err().map(|e| Warning {
        code: "holder_unavailable".into(),
        message: format!("cannot mark control holder for session {id}: {e}"),
    })
}
/// The RPC resume's guard, in `Ephemeral::open_session`'s order: the glen lock first, then the
/// continuation check, the live endpoint and the omp lease. Every refusal comes before omp starts.
fn guard(
    env: &Env,
    home: &GlenHome,
    paths: &OmpPaths,
    opened: &open::Opened,
    deadline: Instant,
    warnings: &mut Vec<Warning>,
) -> Result<Target, GlenError> {
    let id = &opened.session.header.id;
    let handle = &opened.handle;
    let Some(lock) =
        lock::try_acquire(home, id).map_err(|e| sandbox::map_home_write(env, COMMAND, e))?
    else {
        return Err(GlenError::no_route(
            NoRouteReason::RpcJobRunning,
            format!("session {handle} is held by a running glen job or control"),
        )
        .with_hint(format!(
            "wait for that job (`glen jobs --active --session {handle}`), then retry `glen {COMMAND}`"
        )));
    };
    warnings.extend(holder_warning(&lock, id));
    let path = if opened.not_yet_persisted {
        None
    } else {
        let followed = continuation::follow(&opened.session.path)?;
        let current = Session::load(&followed.path).map_err(|e| {
            GlenError::new(
                ErrorCode::Io,
                format!("cannot re-read session {}: {e}", followed.path.display()),
            )
        })?;
        if current.header.id != *id {
            return Err(ephemeral::moved(id, &current.header.id));
        }
        Some(followed.path)
    };
    let probe = live::HELLO_PROBE.min(deadline.saturating_duration_since(Instant::now()));
    match live::route::find(home, id, probe) {
        live::route::Endpoint::Live { .. } => {
            return Err(GlenError::no_route(
                NoRouteReason::LiveOwner,
                format!("session {handle} is open in a live omp session"),
            )
            .with_hint(format!(
                "use glen send {handle} (the live route); {COMMAND} never joins a live session"
            )));
        }
        live::route::Endpoint::Unhealthy { reason, kind, io } => {
            return Err(live::route::unhealthy_refusal(
                env, COMMAND, handle, &reason, kind, io,
            ));
        }
        _ => (),
    }
    if opened.not_yet_persisted {
        return Err(GlenError::no_route(
            NoRouteReason::EndpointGone,
            "the live session has not persisted a transcript and its endpoint went away",
        )
        .with_hint("restart that omp session and run `glen ls`"));
    }
    sandbox::refuse_hidden_lease(env)?;
    let probe = lease::probe(&paths.session_owners_dir, id);
    if let Some(error) = sandbox::map_probe_failure(env, COMMAND, &probe) {
        return Err(error);
    }
    if !matches!(probe, Lease::Free) {
        return Err(live::route::lease_refusal(&probe, handle, paths));
    }
    Ok(Target {
        id: id.clone(),
        handle: handle.clone(),
        path,
        lock,
    })
}

/// Everything the conversation needs besides the client.
struct Turn<'a> {
    env: &'a Env,
    home: &'a GlenHome,
    args: &'a AcpArgs,
    /// Absolute and UTF-8 ([`acp_cwd`]).
    cwd: &'a str,
    /// The id of an existing session; `None` for `@new`.
    existing: Option<&'a str>,
    deadline: Instant,
    stop: &'a AtomicBool,
}
#[derive(Debug)]
enum Ending {
    Completed {
        usage: Option<Value>,
    },
    Interrupted {
        stop_reason: Option<String>,
        cancel: Value,
        confirmed: bool,
    },
    Failed(GlenError),
}
struct Report {
    ending: Ending,
    agent: Value,
    session: Value,
    events: Vec<Value>,
    events_dropped: u64,
    assistant_text: Option<String>,
    text_truncated: bool,
    updates: Value,
    stderr: Vec<String>,
    warnings: Vec<Warning>,
}
/// Starts omp, runs the conversation, and tears omp down before the session lock is released.
fn talk(
    turn: &Turn<'_>,
    command: std::process::Command,
    mut lock: Option<SessionLock>,
    session: Value,
) -> Result<Report, GlenError> {
    let mut client = Client::start(command, turn.args.permission).map_err(|e| {
        acp::error(
            Stage::Handshake,
            Reason::AcpSpawn,
            format!("cannot start omp acp: {e}"),
        )
    })?;
    let mut facts = Facts {
        agent: Value::Null,
        session,
        warnings: vec![],
    };
    let ending = converse(turn, &mut client, &mut lock, &mut facts);
    let grace = match &ending {
        Ending::Completed { .. }
        | Ending::Interrupted {
            confirmed: true, ..
        } => acp::EXIT_GRACE,
        _ => acp::STOP_GRACE,
    };
    let (stderr, mut teardown): (Vec<_>, Vec<_>) = client
        .shutdown(grace)
        .into_iter()
        .partition(|w| w.code == "omp_stderr");
    let report = Report {
        ending,
        agent: facts.agent,
        session: facts.session,
        events: client.events().to_vec(),
        events_dropped: client.events_dropped(),
        assistant_text: client.updates.assistant_text().map(str::to_owned),
        text_truncated: client.updates.text_truncated(),
        updates: client.updates.json(),
        stderr: stderr.into_iter().map(|w| w.message).collect(),
        warnings: {
            facts.warnings.append(&mut teardown);
            facts.warnings
        },
    };
    // The group is gone (or survived SIGKILL, reported above); only now release the lock.
    drop(client);
    drop(lock);
    Ok(report)
}
struct Facts {
    agent: Value,
    session: Value,
    warnings: Vec<Warning>,
}
fn initialize_params() -> Value {
    json!({"protocolVersion":acp::PROTOCOL_VERSION,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},"clientInfo":{"name":"glen","title":"glen","version":env!("CARGO_PKG_VERSION")}})
}
fn converse(
    turn: &Turn<'_>,
    client: &mut Client,
    lock: &mut Option<SessionLock>,
    facts: &mut Facts,
) -> Ending {
    let setup = acp::setup_timeout(turn.env);
    let init = match client.call(
        "initialize",
        initialize_params(),
        setup,
        turn.stop,
        turn.deadline,
    ) {
        Ok(result) => result,
        Err(CallError::Rpc(e)) => {
            return Ending::Failed(
                protocol_error(&Value::Null).with_detail("message", e.message.clone()),
            );
        }
        Err(e) => return call_failed(Stage::Handshake, "initialize", e),
    };
    if init["protocolVersion"].as_u64() != Some(acp::PROTOCOL_VERSION) {
        return Ending::Failed(protocol_error(&init["protocolVersion"]));
    }
    facts.agent = agent(&init);
    let caps = &init["agentCapabilities"];
    if turn.existing.is_some() && caps["loadSession"] != true {
        return Ending::Failed(
            acp::error(
                Stage::Handshake,
                Reason::AcpCapability,
                "omp acp cannot load a stored session (agentCapabilities.loadSession is not true)",
            )
            .with_detail("capability", "loadSession"),
        );
    }
    let can_close = !caps["sessionCapabilities"]["close"].is_null();
    let id = match turn.existing {
        Some(id) => {
            client.updates.for_session(id);
            let params = json!({"sessionId":id,"cwd":turn.cwd,"mcpServers":[]});
            if let Err(e) = client.call("session/load", params, setup, turn.stop, turn.deadline) {
                return call_failed(Stage::Session, "session/load", e);
            }
            id.to_owned()
        }
        None => {
            let params = json!({"cwd":turn.cwd,"mcpServers":[]});
            let result = match client.call("session/new", params, setup, turn.stop, turn.deadline) {
                Ok(result) => result,
                Err(e) => return call_failed(Stage::Session, "session/new", e),
            };
            let Some(id) = result["sessionId"].as_str().filter(|s| !s.is_empty()) else {
                return Ending::Failed(acp::error(
                    Stage::Session,
                    Reason::AcpMalformed,
                    "omp acp answered session/new without a sessionId",
                ));
            };
            facts.session =
                json!({"id":id,"handle":rpc::session_handle(id),"path":null,"created":true});
            client.updates.for_session(id);
            // The id exists only now: lock it as the RPC worker does after omp reveals it.
            match lock::try_acquire(turn.home, id) {
                Ok(Some(held)) => {
                    facts.warnings.extend(holder_warning(&held, id));
                    *lock = Some(held);
                }
                Ok(None) => {
                    return Ending::Failed(GlenError::new(
                        ErrorCode::Io,
                        format!("the new session {id} is already locked by another glen process"),
                    ));
                }
                Err(e) => return Ending::Failed(sandbox::map_home_write(turn.env, COMMAND, e)),
            }
            id.to_owned()
        }
    };
    let outcome = client.prompt(
        &id,
        &turn.args.message,
        turn.stop,
        turn.deadline,
        acp::cancel_grace(turn.env),
    );
    let answered = matches!(&outcome, Ok(o) if o.cancel.is_none());
    let ending = prompt_ending(outcome, client.cancel().cloned());
    if answered
        && can_close
        && !turn.stop.load(Ordering::SeqCst)
        && let Err(e) = client.close_session(&id, turn.stop)
    {
        facts.warnings.push(Warning {
            code: "acp_close_failed".into(),
            message: format!("session/close {}", describe(&e)),
        });
    }
    ending
}
fn agent(init: &Value) -> Value {
    let info = &init["agentInfo"];
    if info.is_object() {
        json!({"name":info["name"],"version":info["version"]})
    } else {
        Value::Null
    }
}
fn protocol_error(got: &Value) -> GlenError {
    acp::error(
        Stage::Handshake,
        Reason::AcpProtocol,
        format!(
            "omp acp does not speak ACP protocol version {} (got {got})",
            acp::PROTOCOL_VERSION
        ),
    )
    .with_detail("requested", acp::PROTOCOL_VERSION)
    .with_detail("got", got.clone())
}
fn describe(e: &CallError) -> String {
    match e {
        CallError::Rpc(e) => format!("failed: {}", e.message),
        CallError::Exited => "failed: omp acp exited".into(),
        CallError::Malformed(reason) => format!("failed: {reason}"),
        CallError::Timeout => "timed out".into(),
        CallError::Interrupted => "was interrupted".into(),
    }
}
fn before_prompt_interrupt() -> Ending {
    Ending::Interrupted {
        stop_reason: None,
        cancel: json!({"sent":false,"confirmed":false,"reason":CancelReason::Interrupt.as_str()}),
        confirmed: false,
    }
}
/// A failed `initialize`, `session/new` or `session/load` call.
fn call_failed(stage: Stage, method: &str, e: CallError) -> Ending {
    let error = match e {
        CallError::Interrupted => return before_prompt_interrupt(),
        CallError::Rpc(e) => acp::error(
            stage,
            Reason::AcpRpcError,
            format!("omp acp rejected {method}: {}", e.message),
        )
        .with_detail("method", method)
        .with_detail("code", e.code)
        .with_detail("message", e.message),
        CallError::Exited => acp::error(
            stage,
            Reason::AcpExited,
            format!("omp acp exited before answering {method}"),
        ),
        CallError::Timeout => acp::error(
            stage,
            Reason::AcpTimeout,
            format!("omp acp did not answer {method} in time"),
        ),
        CallError::Malformed(reason) => acp::error(
            stage,
            Reason::AcpMalformed,
            format!("omp acp sent a malformed frame during {method}: {reason}"),
        ),
    };
    Ending::Failed(error)
}
fn deadline_error(stop_reason: Option<&str>, cancel: &CancelState) -> GlenError {
    acp::error(
        Stage::Session,
        Reason::AcpTimeout,
        "the __acp deadline (GLEN_JOB_TIMEOUT) passed during the turn; glen sent session/cancel",
    )
    .with_detail("cancel", cancel.json())
    .with_detail("stop_reason", stop_reason)
    .with_detail("task_state", stop_reason.map_or("failed", acp::task_state))
}
/// The §A23 exit mapping of a prompt: only `end_turn` with no glen cancel is a success.
fn prompt_ending(result: Result<PromptOutcome, CallError>, cancel: Option<CancelState>) -> Ending {
    match (result, cancel) {
        (Ok(outcome), _) => match outcome.cancel {
            Some(c) if c.reason == CancelReason::Interrupt => Ending::Interrupted {
                stop_reason: outcome.stop_reason,
                cancel: c.json(),
                confirmed: c.confirmed,
            },
            Some(c) => Ending::Failed(deadline_error(outcome.stop_reason.as_deref(), &c)),
            None => match outcome.stop_reason.as_deref() {
                Some("end_turn") => Ending::Completed {
                    usage: outcome.usage,
                },
                other => Ending::Failed(
                    acp::error(
                        Stage::Session,
                        Reason::AcpStopReason,
                        format!(
                            "omp ended the turn with stop reason {}",
                            other.unwrap_or("(none)")
                        ),
                    )
                    .with_detail("stop_reason", other)
                    .with_detail("task_state", acp::task_state(other.unwrap_or(""))),
                ),
            },
        },
        (Err(_), Some(c)) if c.reason == CancelReason::Interrupt => Ending::Interrupted {
            stop_reason: None,
            cancel: c.json(),
            confirmed: false,
        },
        (Err(_), Some(c)) => Ending::Failed(deadline_error(None, &c)),
        (Err(e), None) => match call_failed(Stage::Session, "session/prompt", e) {
            Ending::Failed(error) => Ending::Failed(error.with_detail("task_state", "failed")),
            other => other,
        },
    }
}
/// The success (or interrupted) document, or the error with its context repeated in details.
fn finish(report: Report, args: &AcpArgs, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    let (task_state, stop_reason, usage, cancel, exit) = match report.ending {
        Ending::Failed(mut error) => {
            if !report.session.is_null() {
                error = error.with_detail("session", report.session);
            }
            error = error.with_detail("events", report.events);
            if let Some(text) = report.assistant_text {
                error = error.with_detail("assistant_text", text);
            }
            error = error.with_detail("stderr", report.stderr);
            error.warnings.extend(report.warnings);
            return Err(error);
        }
        Ending::Completed { usage } => (
            "completed",
            Some("end_turn".to_owned()),
            usage,
            Value::Null,
            OkExit::Done,
        ),
        Ending::Interrupted {
            stop_reason,
            cancel,
            ..
        } => ("canceled", stop_reason, None, cancel, OkExit::Interrupted),
    };
    let data = json!({"transport":"acp","protocol_version":acp::PROTOCOL_VERSION,"agent":report.agent,"permission":args.permission.as_str(),"session":report.session,"task_state":task_state,"stop_reason":stop_reason,"assistant_text":report.assistant_text,"text_truncated":report.text_truncated,"events":report.events,"events_dropped":report.events_dropped,"updates":report.updates,"usage":usage,"cancel":cancel});
    let human = human(ctx, &data);
    let mut outcome = Outcome::new(data, human);
    outcome.exit = exit;
    outcome.warnings = report.warnings;
    Ok(outcome)
}
fn human(ctx: &OutputCtx, data: &Value) -> String {
    let mut lines = vec![];
    if let Some(reply) = data["assistant_text"].as_str() {
        for line in text::clean_lines(reply) {
            lines.extend(text::wrap(&line, ctx.width.map(usize::from)));
        }
    }
    for event in data["events"].as_array().into_iter().flatten() {
        let line = format!(
            "? input required · {} · answered {}",
            text::single_line(event["title"].as_str().unwrap_or("tool call")),
            text::single_line(event["answer"].as_str().unwrap_or("?"))
        );
        lines.push(ctx.painter.paint(Token::Muted, &line));
    }
    let handle = text::single_line(data["session"]["handle"].as_str().unwrap_or("@new"));
    let stop = text::single_line(data["stop_reason"].as_str().unwrap_or("no stop reason"));
    let (token, footer) = match data["task_state"].as_str() {
        Some("completed") => (
            Token::Success,
            format!("✓ completed · {handle} · {stop} · acp"),
        ),
        state => (
            Token::Warning,
            format!("■ {} · {handle} · {stop} · acp", state.unwrap_or("?")),
        ),
    };
    lines.push(ctx.painter.paint(token, &footer));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        acp::client::RpcError,
        cli::AcpPermission,
        output::{Mode, Terminal},
    };
    use std::{
        io::{BufRead, BufReader, Write},
        thread,
        time::Duration,
    };
    fn outcome(stop: Option<&str>, cancel: Option<CancelState>) -> PromptOutcome {
        PromptOutcome {
            stop_reason: stop.map(str::to_owned),
            usage: Some(json!({"totalTokens":3})),
            cancel,
        }
    }
    fn failed(ending: Ending) -> GlenError {
        match ending {
            Ending::Failed(e) => e,
            other => panic!("not a failure: {other:?}"),
        }
    }
    fn reason(e: &GlenError) -> &str {
        e.details.as_ref().unwrap()["reason"].as_str().unwrap()
    }
    #[test]
    fn every_stop_reason_maps_to_its_exit() {
        assert!(matches!(
            prompt_ending(Ok(outcome(Some("end_turn"), None)), None),
            Ending::Completed { usage: Some(_) }
        ));
        for (stop, state) in [
            (Some("cancelled"), "canceled"),
            (Some("refusal"), "rejected"),
            (Some("max_tokens"), "failed"),
            (Some("max_turn_requests"), "failed"),
            (Some("something_new"), "failed"),
            (None, "failed"),
        ] {
            let e = failed(prompt_ending(Ok(outcome(stop, None)), None));
            assert_eq!((e.code, e.code.exit_code()), (ErrorCode::JobFailed, 7));
            assert_eq!(reason(&e), "acp_stop_reason");
            let details = e.details.unwrap();
            assert_eq!(details["stop_reason"], json!(stop));
            assert_eq!(details["task_state"], state, "{stop:?}");
        }
    }
    #[test]
    fn cancels_map_to_interrupted_or_timeout() {
        let interrupt = |confirmed| CancelState {
            sent: true,
            confirmed,
            reason: CancelReason::Interrupt,
        };
        let c = interrupt(true);
        match prompt_ending(Ok(outcome(Some("cancelled"), Some(c.clone()))), Some(c)) {
            Ending::Interrupted {
                stop_reason,
                cancel,
                confirmed,
            } => {
                assert_eq!(stop_reason.as_deref(), Some("cancelled"));
                assert!(confirmed);
                assert_eq!(
                    cancel,
                    json!({"sent":true,"confirmed":true,"reason":"interrupt"})
                );
            }
            other => panic!("{other:?}"),
        }
        let c = interrupt(false);
        for result in [
            Ok(outcome(None, Some(c.clone()))),
            Err(CallError::Exited),
            Err(CallError::Malformed("x".into())),
        ] {
            match prompt_ending(result, Some(c.clone())) {
                Ending::Interrupted {
                    stop_reason: None,
                    confirmed: false,
                    cancel,
                } => assert_eq!(cancel["confirmed"], false),
                other => panic!("{other:?}"),
            }
        }
        let deadline = CancelState {
            sent: true,
            confirmed: false,
            reason: CancelReason::Deadline,
        };
        for result in [
            Ok(outcome(None, Some(deadline.clone()))),
            Err(CallError::Exited),
        ] {
            let e = failed(prompt_ending(result, Some(deadline.clone())));
            assert_eq!((e.code.exit_code(), reason(&e)), (7, "acp_timeout"));
            assert_eq!(e.details.unwrap()["cancel"]["reason"], "deadline");
        }
    }
    #[test]
    fn prompt_failures_are_session_stage() {
        for (error, want) in [
            (CallError::Exited, "acp_exited"),
            (CallError::Malformed("bad".into()), "acp_malformed"),
            (
                CallError::Rpc(RpcError {
                    code: json!(-32603),
                    message: "ACP session not found: s".into(),
                }),
                "acp_rpc_error",
            ),
        ] {
            let e = failed(prompt_ending(Err(error), None));
            assert_eq!((e.code.exit_code(), reason(&e)), (7, want));
            let details = e.details.unwrap();
            assert_eq!(details["task_state"], "failed");
            if want == "acp_rpc_error" {
                assert_eq!(details["method"], "session/prompt");
                assert_eq!(details["code"], -32603);
                assert_eq!(details["message"], "ACP session not found: s");
            }
        }
        assert!(matches!(
            call_failed(Stage::Handshake, "initialize", CallError::Interrupted),
            Ending::Interrupted {
                confirmed: false,
                ..
            }
        ));
    }
    fn ctx() -> OutputCtx {
        let env = Env::for_test(None, Path::new("/w"), &[]);
        OutputCtx::fallback(
            Mode::Human,
            &env,
            &Terminal {
                stdout_tty: false,
                stderr_tty: false,
                stdout_cols: None,
            },
        )
    }
    fn args(permission: AcpPermission) -> AcpArgs {
        AcpArgs {
            session: "@new".into(),
            message: "hi".into(),
            permission,
        }
    }
    fn report(ending: Ending) -> Report {
        Report {
            ending,
            agent: json!({"name":"omp","version":"18.14.0"}),
            session: json!({"id":"s-1","handle":"abc1234","path":null,"created":true}),
            events: vec![
                json!({"event":"task_state","task_state":"input_required","title":"rm \u{1b}[31mx","answer":"reject_once"}),
            ],
            events_dropped: 0,
            assistant_text: Some("reply: \u{1b}]0;hi".into()),
            text_truncated: false,
            updates: json!({"replayed":0,"during_turn":3,"messages":1,"kinds":{}}),
            stderr: vec!["omp said".into()],
            warnings: vec![Warning {
                code: "w".into(),
                message: "m".into(),
            }],
        }
    }
    #[test]
    fn documents_carry_every_key_and_the_exit() {
        let ctx = ctx();
        let done = finish(
            report(Ending::Completed {
                usage: Some(json!({"totalTokens":3})),
            }),
            &args(AcpPermission::Deny),
            &ctx,
        )
        .unwrap();
        assert_eq!(done.exit.exit_code(), 0);
        let mut keys: Vec<_> = done.data.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "agent",
                "assistant_text",
                "cancel",
                "events",
                "events_dropped",
                "permission",
                "protocol_version",
                "session",
                "stop_reason",
                "task_state",
                "text_truncated",
                "transport",
                "updates",
                "usage"
            ]
        );
        assert_eq!(done.data["transport"], "acp");
        assert_eq!(done.data["protocol_version"], 1);
        assert_eq!(done.data["permission"], "deny");
        assert_eq!(done.data["task_state"], "completed");
        assert_eq!(done.data["stop_reason"], "end_turn");
        assert_eq!(done.data["cancel"], Value::Null);
        // JSON keeps raw strings; the human view escapes controls.
        assert_eq!(done.data["assistant_text"], "reply: \u{1b}]0;hi");
        assert!(!done.human.contains('\u{1b}'), "{:?}", done.human);
        assert!(done.human.contains("? input required · rm"));
        assert!(
            done.human
                .ends_with("✓ completed · abc1234 · end_turn · acp")
        );
        assert_eq!(done.warnings.len(), 1);
        let stopped = finish(
            report(Ending::Interrupted {
                stop_reason: None,
                cancel: json!({"sent":true,"confirmed":false,"reason":"interrupt"}),
                confirmed: false,
            }),
            &args(AcpPermission::Allow),
            &ctx,
        )
        .unwrap();
        assert_eq!(stopped.exit.exit_code(), 130);
        assert_eq!(stopped.data["task_state"], "canceled");
        assert_eq!(stopped.data["stop_reason"], Value::Null);
        assert_eq!(stopped.data["usage"], Value::Null);
        assert_eq!(stopped.data["cancel"]["reason"], "interrupt");
        assert!(stopped.human.contains("■ canceled · abc1234"));
        let Err(error) = finish(
            report(Ending::Failed(acp::error(
                Stage::Session,
                Reason::AcpExited,
                "gone",
            ))),
            &args(AcpPermission::Deny),
            &ctx,
        ) else {
            panic!("a failure became a document")
        };
        let details = error.details.unwrap();
        assert_eq!(details["reason"], "acp_exited");
        assert_eq!(details["session"]["id"], "s-1");
        assert_eq!(details["events"].as_array().unwrap().len(), 1);
        assert_eq!(details["assistant_text"], "reply: \u{1b}]0;hi");
        assert_eq!(details["stderr"], json!(["omp said"]));
        assert_eq!(error.warnings.len(), 1);
    }
    #[test]
    fn messages_are_validated_before_anything_else() {
        for bad in ["", "  \n\t"] {
            assert_eq!(validate(bad).unwrap_err().code, ErrorCode::Usage);
        }
        let big = "x".repeat(rpc::MAX_MESSAGE_BYTES + 1);
        assert_eq!(validate(&big).unwrap_err().code, ErrorCode::Usage);
        assert!(validate(&big[1..]).is_ok());
        // Through `run`: no omp is found or started and no glen home is created.
        let tree = tempfile::tempdir().unwrap();
        let glen_home = tree.path().join("glen");
        let log = tree.path().join("fake.log");
        let env = Env::for_test(
            Some(tree.path()),
            tree.path(),
            &[
                ("GLEN_HOME", glen_home.to_str().unwrap()),
                ("GLEN_OMP_BIN", "/nonexistent/omp"),
                ("FAKE_OMP_LOG", log.to_str().unwrap()),
            ],
        );
        let cli = <Cli as clap::Parser>::try_parse_from(["glen", "__acp", "@new", " "]).unwrap();
        let crate::cli::Command::Acp(a) = &cli.command else {
            panic!("not __acp")
        };
        let Err(error) = run(&cli, a, &env, &ctx()) else {
            panic!("an empty message was accepted")
        };
        assert_eq!(error.code, ErrorCode::Usage);
        assert!(!glen_home.exists() && !log.exists());
    }

    /// A scripted ACP agent over pipes: `answer` maps each client request to the lines to send
    /// back (none: no answer; `"EOF"`: close the agent's stdout).
    fn scripted(
        answer: impl Fn(&Value) -> Vec<String> + Send + 'static,
        permission: AcpPermission,
    ) -> Client {
        let (client_read, mut agent_write) = std::io::pipe().unwrap();
        let (agent_read, client_write) = std::io::pipe().unwrap();
        thread::spawn(move || {
            for line in BufReader::new(agent_read).lines() {
                let Ok(line) = line else { return };
                let frame: Value = serde_json::from_str(&line).unwrap();
                for out in answer(&frame) {
                    if out == "EOF" {
                        return;
                    }
                    if writeln!(agent_write, "{out}").is_err() {
                        return;
                    }
                }
            }
        });
        Client::from_io(client_read, client_write, None, permission)
    }
    fn ok(frame: &Value, result: Value) -> String {
        json!({"jsonrpc":"2.0","id":frame["id"],"result":result}).to_string()
    }
    fn init(protocol: Value, load: bool) -> Value {
        json!({"protocolVersion":protocol,"agentInfo":{"name":"omp","version":"18.14.0"},"agentCapabilities":{"loadSession":load,"sessionCapabilities":{"close":{}}}})
    }
    /// Runs `converse` against a scripted agent; `existing` is a session id or `None` for @new.
    fn converse_with(
        answer: impl Fn(&Value) -> Vec<String> + Send + 'static,
        existing: Option<&str>,
        vars: &[(&str, &str)],
    ) -> (Ending, Facts, Option<SessionLock>) {
        let tree = tempfile::tempdir().unwrap();
        let env = Env::for_test(Some(tree.path()), tree.path(), vars);
        let home = GlenHome {
            root: tree.path().join("glen"),
        };
        let args = args(AcpPermission::Deny);
        let stop = AtomicBool::new(false);
        let turn = Turn {
            env: &env,
            home: &home,
            args: &args,
            cwd: tree.path().to_str().unwrap(),
            existing,
            deadline: Instant::now() + Duration::from_secs(30),
            stop: &stop,
        };
        let mut client = scripted(answer, AcpPermission::Deny);
        let mut facts = Facts {
            agent: Value::Null,
            session: Value::Null,
            warnings: vec![],
        };
        let mut lock = None;
        let ending = converse(&turn, &mut client, &mut lock, &mut facts);
        (ending, facts, lock)
    }
    fn turn_script(frame: &Value) -> Vec<String> {
        let id = frame["params"]["sessionId"].clone();
        match frame["method"].as_str() {
            Some("initialize") => vec![ok(frame, init(json!(1), true))],
            Some("session/new") => vec![ok(frame, json!({"sessionId":"s-new"}))],
            Some("session/load") => vec![ok(frame, json!({}))],
            Some("session/prompt") => vec![
                json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":id,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"pong"},"messageId":"m1"}}}).to_string(),
                ok(frame, json!({"stopReason":"end_turn"})),
            ],
            Some("session/close") => vec![ok(frame, json!({}))],
            _ => vec![],
        }
    }
    #[test]
    fn a_scripted_turn_completes_and_locks_a_new_session() {
        let (ending, facts, lock) = converse_with(turn_script, None, &[]);
        assert!(matches!(ending, Ending::Completed { .. }), "{ending:?}");
        assert_eq!(facts.agent, json!({"name":"omp","version":"18.14.0"}));
        assert_eq!(facts.session["id"], "s-new");
        assert_eq!(facts.session["created"], true);
        assert!(lock.is_some());
        let (ending, facts, lock) = converse_with(turn_script, Some("s-old"), &[]);
        assert!(matches!(ending, Ending::Completed { .. }), "{ending:?}");
        assert_eq!(facts.session, Value::Null);
        assert!(lock.is_none());
    }
    #[test]
    fn the_cwd_is_made_absolute_and_must_be_utf8() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
        let env = Env::for_test(None, Path::new("/w"), &[]);
        assert_eq!(acp_cwd(&env, Path::new("sub")).unwrap(), "/w/sub");
        assert_eq!(acp_cwd(&env, Path::new("/a/./b")).unwrap(), "/a/b");
        // Linux allows such a directory (macOS does not); it must not reach `json!` and panic.
        let bad = Path::new(OsStr::from_bytes(b"/tmp/\xff"));
        let relative = Path::new(OsStr::from_bytes(b"\xff"));
        let under_bad = Env::for_test(None, bad, &[]);
        for (env, cwd) in [(&env, bad), (&under_bad, Path::new("sub"))] {
            let e = acp_cwd(env, cwd).unwrap_err();
            assert_eq!(e.code, ErrorCode::Usage);
            assert_eq!(reason(&e), "non_utf8_cwd");
            let shown = e.details.unwrap()["cwd"].as_str().unwrap().to_owned();
            assert!(shown.contains('\u{fffd}'), "{shown}");
        }
        assert_eq!(
            reason(&acp_cwd(&env, relative).unwrap_err()),
            "non_utf8_cwd"
        );
    }
    #[test]
    fn the_failure_table_through_a_scripted_client() {
        type Script = Box<dyn Fn(&Value) -> Vec<String> + Send>;
        /// (name, existing session, agent script, env, exit, details.reason)
        type Case = (
            &'static str,
            Option<&'static str>,
            Script,
            &'static [(&'static str, &'static str)],
            u8,
            &'static str,
        );
        let on = |method: &'static str, lines: fn(&Value) -> Vec<String>| -> Script {
            Box::new(move |frame: &Value| {
                if frame["method"] == method {
                    lines(frame)
                } else {
                    turn_script(frame)
                }
            })
        };
        let cases: Vec<Case> = vec![
            (
                "proto2",
                None,
                on("initialize", |f| vec![ok(f, init(json!(2), true))]),
                &[],
                5,
                "acp_protocol",
            ),
            (
                "proto float",
                None,
                on("initialize", |f| vec![ok(f, init(json!(1.0), true))]),
                &[],
                5,
                "acp_protocol",
            ),
            (
                "proto missing",
                None,
                on("initialize", |f| {
                    vec![ok(f, json!({"agentCapabilities":{}}))]
                }),
                &[],
                5,
                "acp_protocol",
            ),
            (
                "init error",
                None,
                on("initialize", |f| {
                    vec![json!({"jsonrpc":"2.0","id":f["id"],"error":{"code":-32603,"message":"Internal error"}}).to_string()]
                }),
                &[],
                5,
                "acp_protocol",
            ),
            (
                "init eof",
                None,
                on("initialize", |_| vec!["EOF".into()]),
                &[],
                5,
                "acp_exited",
            ),
            (
                "init noise",
                None,
                on("initialize", |_| vec!["noise".into()]),
                &[],
                5,
                "acp_malformed",
            ),
            (
                "init hang",
                None,
                on("initialize", |_| vec![]),
                &[("GLEN_ACP_SETUP_MS", "200")],
                5,
                "acp_timeout",
            ),
            (
                "no load",
                Some("s-old"),
                on("initialize", |f| vec![ok(f, init(json!(1), false))]),
                &[],
                5,
                "acp_capability",
            ),
            (
                "load error",
                Some("s-old"),
                on("session/load", |f| {
                    vec![json!({"jsonrpc":"2.0","id":f["id"],"error":{"code":-32603,"message":"Internal error","data":{"details":"ACP session not found: s-old"}}}).to_string()]
                }),
                &[],
                7,
                "acp_rpc_error",
            ),
            (
                "load eof",
                Some("s-old"),
                on("session/load", |_| vec!["EOF".into()]),
                &[],
                7,
                "acp_exited",
            ),
            (
                "new noise",
                None,
                on("session/new", |_| vec!["{\"jsonrpc\":\"2.0\"".into()]),
                &[],
                7,
                "acp_malformed",
            ),
            (
                "new hang",
                None,
                on("session/new", |_| vec![]),
                &[("GLEN_ACP_SETUP_MS", "200")],
                7,
                "acp_timeout",
            ),
            (
                "new without id",
                None,
                on("session/new", |f| vec![ok(f, json!({"sessionId":""}))]),
                &[],
                7,
                "acp_malformed",
            ),
            (
                "prompt eof",
                None,
                on("session/prompt", |_| vec!["EOF".into()]),
                &[],
                7,
                "acp_exited",
            ),
            (
                "refusal",
                None,
                on("session/prompt", |f| {
                    vec![ok(f, json!({"stopReason":"refusal"}))]
                }),
                &[],
                7,
                "acp_stop_reason",
            ),
        ];
        for (name, existing, script, vars, exit, want) in cases {
            if vars.iter().any(|(k, _)| k.starts_with("GLEN_ACP")) && !cfg!(debug_assertions) {
                continue;
            }
            let (ending, _, _) = converse_with(script, existing, vars);
            let e = failed(ending);
            assert_eq!(e.code.exit_code(), exit, "{name}: {e:?}");
            assert_eq!(reason(&e), want, "{name}: {e:?}");
            if want == "acp_protocol" {
                assert_eq!(e.details.as_ref().unwrap()["requested"], 1, "{name}");
            }
            if name == "load error" {
                assert_eq!(
                    e.details.as_ref().unwrap()["message"],
                    "ACP session not found: s-old"
                );
            }
        }
    }
}
