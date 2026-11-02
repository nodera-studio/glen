//! Controls use the live endpoint, then its owning RPC job, then an unowned client.
use super::{
    ephemeral::{self, Ephemeral, OpenError},
    inbox,
};
use crate::{
    cli::Cli,
    commands::open::Opened,
    env::Env,
    error::{ErrorCode, GlenError, NoRouteReason, Warning},
    glen_home,
    jobs::{self, Forked, JobId, JobStore, liveness, lock},
    live::{
        self,
        client::{self, Hello},
        registry::Registry,
    },
    omp::{
        bin,
        lease::{self, Lease},
        paths,
        session::Selection,
    },
    ops::{self, Op, OpError},
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
pub enum Target {
    Live {
        reg: Box<Registry>,
        hello: Box<Hello>,
        deadline: Instant,
        stop: Arc<AtomicBool>,
    },
    Job {
        id: JobId,
        handle: String,
        session_id: String,
        store: JobStore,
        deadline: Instant,
        stop: Arc<AtomicBool>,
    },
    Rpc(Ephemeral),
    Idle,
}
impl Target {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Live { .. } => "live",
            Self::Job { .. } => "job",
            Self::Rpc(_) => "rpc",
            Self::Idle => "none",
        }
    }
    pub fn job(&self) -> Option<&str> {
        match self {
            Self::Job { handle, .. } => Some(handle),
            _ => None,
        }
    }
    pub fn ext_version(&self) -> Option<&str> {
        match self {
            Self::Live { hello, .. } => hello.ext_version.as_deref(),
            _ => None,
        }
    }
    pub fn call(&mut self, op: &Op) -> Result<Value, OpError> {
        match self {
            Self::Live {
                reg,
                hello,
                deadline,
                stop,
            } => {
                ephemeral::stopped(*deadline, stop)?;
                if !hello.ops.iter().any(|name| name == op.name()) {
                    return Err(OpError::new(
                        ops::UNSUPPORTED_OP,
                        "the extension does not support this control",
                    )
                    .with_details(json!({"ext_version":hello.ext_version})));
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                let timeout = if matches!(op, Op::Abort) {
                    remaining.min(Duration::from_secs(4))
                } else {
                    remaining
                };
                client::unary_for(&reg.socket, op, &reg.session_id, timeout, stop.clone()).map_err(
                    |e| match e {
                        client::ClientError::Refused(error) => OpError {
                            code: error.code,
                            message: error.message,
                            details: error.details,
                        },
                        client::ClientError::Timeout => {
                            OpError::new(ops::TIMEOUT, "the live control timed out")
                        }
                        client::ClientError::Cancelled => {
                            OpError::new("interrupted", "the live control was interrupted")
                        }
                        error => OpError::new(ops::INTERNAL, error.to_string()),
                    },
                )
            }
            Self::Job {
                id,
                session_id,
                store,
                deadline,
                stop,
                ..
            } => {
                ephemeral::stopped(*deadline, stop)?;
                if matches!(op, Op::Compact(_)) {
                    return Err(OpError::new(
                        ops::BUSY,
                        format!(
                            "a glen job is running on this session; wait for it (`glen wait {}`) or abort the run first",
                            id.handle()
                        ),
                    ));
                }
                inbox::request(store, id, op, session_id, *deadline, stop)
            }
            Self::Rpc(client) => {
                let id = client.session_id().to_owned();
                client.call(op, Some(&id))
            }
            Self::Idle => {
                Ok(json!({"was":"idle","aborted":false,"idle_after":true,"queued":false}))
            }
        }
    }
    pub fn finish(self) -> Result<(Option<Forked>, Vec<Warning>), OpError> {
        match self {
            Self::Rpc(client) => client.finish(),
            _ => Ok((None, vec![])),
        }
    }
}
pub(crate) enum ModelsSource {
    Session,
    Disk(Selection),
    Unpersisted,
    Profile,
}
fn context<'a>(opened: &'a Opened) -> super::ErrCtx<'a> {
    super::ErrCtx {
        handle: &opened.handle,
        session_id: &opened.session.header.id,
        ..Default::default()
    }
}
fn stopped(deadline: Instant, stop: &AtomicBool, opened: &Opened) -> Result<(), GlenError> {
    ephemeral::stopped(deadline, stop).map_err(|e| {
        if e.code == "interrupted" {
            GlenError::new(ErrorCode::ControlFailed, e.message)
                .with_details(json!({"reason":"interrupted"}))
        } else {
            super::to_glen_error(e, &context(opened))
        }
    })
}
fn live_target(
    reg: Box<Registry>,
    hello: Box<Hello>,
    deadline: Instant,
    stop: Arc<AtomicBool>,
) -> (Target, Vec<Warning>) {
    let warnings = live::route::extension_version_warning(&hello)
        .into_iter()
        .collect();
    (
        Target::Live {
            reg,
            hello,
            deadline,
            stop,
        },
        warnings,
    )
}
fn open_error(error: OpenError, opened: Option<&Opened>) -> GlenError {
    match error {
        OpenError::Failed(error) => error,
        OpenError::Stopped(error) if error.code == "interrupted" => {
            GlenError::new(ErrorCode::ControlFailed, error.message)
                .with_details(json!({"reason":"interrupted","route":"rpc"}))
        }
        OpenError::Stopped(error) => {
            let mut ctx = opened.map(context).unwrap_or_default();
            ctx.route = "rpc";
            super::to_glen_error(error, &ctx)
        }
        _ => GlenError::new(
            ErrorCode::ControlFailed,
            "the session route changed repeatedly; retry the command",
        )
        .with_details(json!({"reason":"busy"})),
    }
}
enum Owner {
    Job(JobId),
    Control,
    /// An idle warm worker (v0.2 W4) holds the lock: it cools on request, or keeps omp for abort.
    Warm(lock::Holder),
    Finishing,
    Unknown,
    Free,
}
fn owner(store: &JobStore, session_id: &str, deadline: Instant, stop: &AtomicBool) -> Owner {
    let started = Instant::now();
    let mut end = (started + Duration::from_secs(1)).min(deadline);
    let mut finishing = false;
    loop {
        if !lock::is_held(&store.home, session_id) {
            return Owner::Free;
        }
        if let Some(holder) = lock::holder(&store.home, session_id)
            && holder.start.as_deref().is_some_and(|start| {
                liveness::check(holder.pid, start) == liveness::Liveness::Alive
            })
        {
            match holder.kind {
                lock::HolderKind::Control => return Owner::Control,
                // Idle only when no active job names the holder: right after a claim the
                // holder already says `job`, so this is the flip's microsecond window.
                lock::HolderKind::Warm
                    if !store
                        .active_for_session(session_id)
                        .iter()
                        .any(|(_, _, status)| status.worker_pid == Some(holder.pid)) =>
                {
                    return Owner::Warm(holder);
                }
                lock::HolderKind::Warm => (),
                lock::HolderKind::Job => {
                    finishing = holder
                        .job_id
                        .as_deref()
                        .and_then(JobId::parse)
                        .is_some_and(|id| {
                            store.read_status(&id).is_ok_and(|s| s.status.is_terminal())
                        });
                    if finishing {
                        end = (started + Duration::from_secs(2)).min(deadline);
                    }
                    if let Some(id) = holder.job_id.as_deref().and_then(JobId::parse)
                        && store.active_for_session(session_id).iter().any(
                            |(active, job, status)| {
                                active == &id
                                    && job.route != jobs::Route::Live
                                    && status.worker_pid == Some(holder.pid)
                                    && status.worker_start == holder.start
                            },
                        )
                    {
                        return Owner::Job(id);
                    }
                }
            }
        }
        if Instant::now() >= end || stop.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !lock::is_held(&store.home, session_id) {
        return Owner::Free;
    }
    if finishing {
        return Owner::Finishing;
    }
    let mut active = store
        .active_for_session(session_id)
        .into_iter()
        .filter(|(_, job, status)| {
            job.route != jobs::Route::Live
                && !status.waiting_for_lock
                && status
                    .worker_pid
                    .zip(status.worker_start.as_deref())
                    .is_some_and(|(pid, start)| {
                        liveness::check(pid, start) == liveness::Liveness::Alive
                    })
        });
    match (active.next(), active.next()) {
        (Some((id, _, _)), None) => Owner::Job(id),
        _ => Owner::Unknown,
    }
}
fn job_target(
    store: JobStore,
    id: JobId,
    session_id: &str,
    deadline: Instant,
    stop: Arc<AtomicBool>,
) -> Target {
    Target::Job {
        handle: id.handle(),
        id,
        session_id: session_id.into(),
        store,
        deadline,
        stop,
    }
}
pub(crate) fn route(
    cli: &Cli,
    env: &Env,
    opened: &Opened,
    op_name: &str,
    deadline: Instant,
    stop: Arc<AtomicBool>,
) -> Result<(Target, Vec<Warning>), GlenError> {
    crate::sandbox::refuse_unreachable(env, cli.command.name())
        .map_err(|e| crate::sandbox::with_hidden_lease(env, e))?;
    let home = glen_home::resolve_trusted(env, &["run/live", "jobs"])?;
    let store = JobStore::new(home.clone());
    let id = &opened.session.header.id;
    let paths = paths::resolve(env, cli.global.profile.as_deref())?;
    let mut rerouted = false;
    loop {
        stopped(deadline, &stop, opened)?;
        let endpoint = live::route::find(
            &home,
            id,
            live::HELLO_SEND.min(deadline.saturating_duration_since(Instant::now())),
        );
        stopped(deadline, &stop, opened)?;
        match endpoint {
            live::route::Endpoint::Live { reg, hello } => {
                if lock::is_held(&home, id) {
                    // The CLI command the user retries; `watch` routes the same ops.
                    let command = match op_name {
                        "set_model" => "model",
                        "set_thinking_level" => "thinking",
                        other => other,
                    };
                    return Err(
                        if lock::holder_kind(&home, id) == Some(lock::HolderKind::Warm) {
                            live::route::rpc_warm_held(&opened.handle, command)
                        } else {
                            live::route::rpc_job_running(&opened.handle, command)
                        },
                    );
                }
                if !hello.ops.iter().any(|op| op == op_name) {
                    let ctx = super::ErrCtx {
                        ext_version: hello.ext_version.as_deref(),
                        ..context(opened)
                    };
                    return Err(super::to_glen_error(
                        OpError::new(
                            ops::UNSUPPORTED_OP,
                            "this omp session has an older glen extension",
                        ),
                        &ctx,
                    ));
                }
                return Ok(live_target(reg, hello, deadline, stop));
            }
            live::route::Endpoint::Unhealthy { reason, kind, io } => {
                return Err(live::route::unhealthy_refusal(
                    env,
                    cli.command.name(),
                    &opened.handle,
                    &reason,
                    kind,
                    io,
                ));
            }
            _ => (),
        }
        crate::sandbox::refuse_rpc(env, cli.command.name())
            .map_err(|e| crate::sandbox::with_hidden_lease(env, e))?;
        // A hidden lease refuses before any lock wait or lease probe; only a control for a
        // running glen job still reaches that job (design.md §A13, W10 clarification).
        if !lock::is_held(&home, id) {
            crate::sandbox::refuse_hidden_lease(env)?;
        } else {
            let holder = owner(&store, id, deadline, &stop);
            stopped(deadline, &stop, opened)?;
            if !matches!(holder, Owner::Job(_)) {
                crate::sandbox::refuse_hidden_lease(env)?;
            }
            match holder {
                Owner::Job(job) => return Ok((job_target(store, job, id, deadline, stop), vec![])),
                Owner::Free => continue,
                // Nothing runs on an idle warm session; it stays warm (design §A21).
                Owner::Warm(_) if op_name == "abort" => return Ok((Target::Idle, vec![])),
                Owner::Warm(holder) => {
                    cool_for_control(
                        env,
                        cli.command.name(),
                        &home,
                        opened,
                        &holder,
                        deadline,
                        &stop,
                    )?;
                    continue;
                }
                Owner::Finishing if op_name == "abort" => {
                    return Err(super::to_glen_error(
                        OpError::new(ops::BUSY, "a glen job is finishing; retry"),
                        &context(opened),
                    ));
                }
                _ => {
                    if op_name == "abort" {
                        return Err(super::to_glen_error(
                            OpError::new(
                                ops::BUSY,
                                format!("another glen control is running on {}", opened.handle),
                            ),
                            &context(opened),
                        ));
                    }
                    #[cfg(debug_assertions)]
                    if env.get("FAKE_CTL_LOCK_WAIT_GATE") == Some("1") {
                        let gate = home.root.join("run/locks").join(format!(
                            ".{}.{}.control-waiting",
                            lock::lock_key(id),
                            std::process::id()
                        ));
                        crate::glen_home::write_private_atomic(&gate, b"waiting").map_err(|e| {
                            GlenError::new(
                                ErrorCode::Io,
                                format!(
                                    "cannot publish control lock-wait gate {}: {e}",
                                    gate.display()
                                ),
                            )
                        })?;
                    }
                    while lock::is_held(&home, id) {
                        stopped(deadline, &stop, opened)?;
                        std::thread::sleep(
                            Duration::from_millis(200)
                                .min(deadline.saturating_duration_since(Instant::now())),
                        );
                    }
                    continue;
                }
            }
        }
        let probe = lease::probe(&paths.session_owners_dir, id);
        stopped(deadline, &stop, opened)?;
        if let Some(error) = crate::sandbox::map_probe_failure(env, cli.command.name(), &probe) {
            return Err(error);
        }
        if !matches!(probe, Lease::Free) {
            return Err(live::route::lease_refusal(&probe, &opened.handle, &paths));
        }
        if op_name == "abort" {
            return Ok((Target::Idle, vec![]));
        }
        if opened.not_yet_persisted {
            return Err(GlenError::no_route(
                NoRouteReason::EndpointGone,
                "the live session has not persisted a transcript and its endpoint went away",
            )
            .with_hint("restart that omp session and run `glen ls`"));
        }
        let bin = bin::find_omp(env).map_err(|e| {
            GlenError::new(ErrorCode::OmpUnavailable, e)
                .with_hint("install omp or set GLEN_OMP_BIN")
        })?;
        let cwd = opened
            .session
            .header
            .cwd
            .as_deref()
            .map(std::path::Path::new)
            .filter(|p| p.is_dir())
            .unwrap_or(env.cwd());
        match Ephemeral::open_session(
            &home,
            &paths,
            &opened.session,
            cli.global.profile.as_deref(),
            &bin,
            cwd,
            deadline,
            stop.clone(),
            env,
            cli.command.name(),
        ) {
            Ok(client) => return Ok((Target::Rpc(client), vec![])),
            Err(OpenError::Busy) => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(OpenError::Reroute) if !rerouted => {
                rerouted = true;
                continue;
            }
            Err(error) => return Err(open_error(error, Some(opened))),
        }
    }
}
pub(crate) fn route_models(
    cli: &Cli,
    env: &Env,
    opened: Option<&Opened>,
    deadline: Instant,
    stop: Arc<AtomicBool>,
) -> Result<(Target, Vec<Warning>, ModelsSource), GlenError> {
    crate::sandbox::refuse_unreachable(env, "models")?;
    let home = glen_home::resolve_trusted(env, &["run/live", "jobs"])?;
    let store = JobStore::new(home.clone());
    let paths = paths::resolve(env, cli.global.profile.as_deref())?;
    let mut warnings = vec![];
    if let Some(opened) = opened {
        stopped(deadline, &stop, opened)?;
        let id = &opened.session.header.id;
        let endpoint = live::route::find(
            &home,
            id,
            live::HELLO_SEND.min(deadline.saturating_duration_since(Instant::now())),
        );
        stopped(deadline, &stop, opened)?;
        match endpoint {
            live::route::Endpoint::Live { reg, hello }
                if hello.ops.iter().any(|op| op == "list_models") =>
            {
                let (target, warnings) = live_target(reg, hello, deadline, stop);
                return Ok((target, warnings, ModelsSource::Session));
            }
            live::route::Endpoint::Live { .. } => warnings.push(Warning {
                code: "extension_outdated".into(),
                message: "run `glen install-extension`, then restart that omp session".into(),
            }),
            live::route::Endpoint::Unhealthy { kind, .. } => warnings.push(Warning {
                code: "session_unreachable".into(),
                message: live::route::hint(kind),
            }),
            _ => (),
        }
        crate::sandbox::refuse_rpc(env, "models")?;
        if lock::is_held(&home, id) {
            match owner(&store, id, deadline, &stop) {
                Owner::Job(job) => {
                    return Ok((
                        job_target(store, job, id, deadline, stop),
                        warnings,
                        ModelsSource::Session,
                    ));
                }
                Owner::Warm(holder) => {
                    cool_for_control(env, "models", &home, opened, &holder, deadline, &stop)?
                }
                _ => (),
            }
        }
        let probe = lease::probe(&paths.session_owners_dir, id);
        // A hidden lease (Linux under a Codex marker) is unknown whatever it probes as.
        if !matches!(probe, Lease::Free)
            || crate::sandbox::lease_hidden(env, cfg!(target_os = "linux"))
        {
            warnings.push(Warning {
                code: "session_unreachable".into(),
                message: live::route::lease_held_hint(&paths),
            });
        }
    }
    crate::sandbox::refuse_rpc(env, "models")?;
    let bin = bin::find_omp(env).map_err(|e| {
        GlenError::new(ErrorCode::OmpUnavailable, e).with_hint("install omp or set GLEN_OMP_BIN")
    })?;
    let cwd = opened
        .and_then(|o| o.session.header.cwd.as_deref())
        .map(std::path::Path::new)
        .filter(|p| p.is_dir())
        .unwrap_or(env.cwd());
    let client = Ephemeral::open_listing(cli.global.profile.as_deref(), &bin, cwd, deadline, stop)
        .map_err(|e| open_error(e, opened))?;
    let source = match opened {
        None => ModelsSource::Profile,
        Some(o) if o.not_yet_persisted => {
            warnings.push(not_persisted());
            ModelsSource::Unpersisted
        }
        Some(o) => ModelsSource::Disk(o.session.selection()),
    };
    Ok((Target::Rpc(client), warnings, source))
}
/// Asks the idle warm worker named by `holder` to cool, then waits until that holder is gone
/// (bounded by the command's deadline). The caller then routes as if it had never been warm.
fn cool_for_control(
    env: &Env,
    command: &str,
    home: &glen_home::GlenHome,
    opened: &Opened,
    holder: &lock::Holder,
    deadline: Instant,
    stop: &AtomicBool,
) -> Result<(), GlenError> {
    let id = &opened.session.header.id;
    crate::rpc::warm::request_release(
        home,
        id,
        &crate::rpc::warm::ident(holder.pid, holder.start.as_deref()),
    )
    .map_err(|e| crate::sandbox::map_home_write(env, command, e))?;
    wait_holder_gone(home, id, holder, deadline, || {
        stopped(deadline, stop, opened)
    })
}
/// Waits while the session lock is held and its recorded holder is still `holder`. A free lock
/// or any other record (a cold job that took the lock after the warm worker cooled, or the warm
/// worker itself now running an adopted job) returns at once, so the caller's next `owner()`
/// routes to whoever holds the session now instead of waiting for it to finish.
fn wait_holder_gone(
    home: &glen_home::GlenHome,
    id: &str,
    holder: &lock::Holder,
    deadline: Instant,
    stopped: impl Fn() -> Result<(), GlenError>,
) -> Result<(), GlenError> {
    while lock::is_held(home, id) && lock::holder(home, id).as_ref() == Some(holder) {
        stopped()?;
        std::thread::sleep(
            Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    Ok(())
}
pub fn not_persisted() -> Warning {
    Warning {
        code: "not_yet_persisted".into(),
        message: "omp keeps the change and writes it with the session's first answer".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn holder_requires_a_held_lock_a_matching_live_pid_and_an_active_session_job() {
        let temp = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(temp.path());
        let job = crate::jobs::tests::job();
        let id = JobId::parse(&job.id).unwrap();
        let session_id = job.session_id.as_deref().unwrap();
        store.create(&job).unwrap();
        let identity = liveness::current().unwrap();
        let mut status = store.read_status(&id).unwrap();
        status.worker_pid = Some(identity.pid);
        status.worker_start = Some(identity.start.clone());
        status.status = jobs::State::Accepted;
        store.write_status(&id, &status).unwrap();
        let lock = lock::try_acquire(&store.home, session_id).unwrap().unwrap();
        let mut holder = lock::Holder {
            v: 1,
            kind: lock::HolderKind::Job,
            job_id: Some(id.as_str().into()),
            pid: identity.pid,
            start: Some(identity.start),
        };
        lock::mark(&lock, &holder).unwrap();
        let stop = AtomicBool::new(false);
        assert!(
            matches!(owner(&store,session_id,Instant::now()+Duration::from_secs(2),&stop),Owner::Job(found) if found==id)
        );
        holder.start = Some("reused".into());
        lock::mark(&lock, &holder).unwrap();
        // A stale record can only select the same single live job via the legacy fallback.
        assert!(
            matches!(owner(&store,session_id,Instant::now(),&stop),Owner::Job(found) if found==id)
        );
        status.status = jobs::State::Completed;
        store.write_status(&id, &status).unwrap();
        assert!(matches!(
            owner(&store, session_id, Instant::now(), &stop),
            Owner::Unknown
        ));
        // A terminal job with a matching live holder is still finishing until
        // its worker releases the lock.
        holder.start = status.worker_start.clone();
        lock::mark(&lock, &holder).unwrap();
        assert!(matches!(
            owner(&store, session_id, Instant::now(), &stop),
            Owner::Finishing
        ));
        // An idle warm holder (v0.2 W4): `Warm`, unless an active job already names its pid.
        holder.kind = lock::HolderKind::Warm;
        lock::mark(&lock, &holder).unwrap();
        assert!(
            matches!(owner(&store, session_id, Instant::now(), &stop), Owner::Warm(h) if h == holder)
        );
        status.status = jobs::State::Queued;
        status.omp_reused = true;
        store.write_status(&id, &status).unwrap();
        assert!(
            matches!(owner(&store, session_id, Instant::now(), &stop), Owner::Job(found) if found == id),
            "a claimed job is the owner, never Target::Idle"
        );
        status.status = jobs::State::Completed;
        store.write_status(&id, &status).unwrap();
        holder.start = Some("reused".into());
        lock::mark(&lock, &holder).unwrap();
        assert!(matches!(
            owner(&store, session_id, Instant::now(), &stop),
            Owner::Unknown
        ));
        drop(lock);
        // Parallel tests may fork while the lock is held; allow their inherited
        // descriptors to close at exec before requiring the lock to be free.
        assert!(matches!(
            owner(
                &store,
                session_id,
                Instant::now() + Duration::from_secs(2),
                &stop
            ),
            Owner::Free
        ));
    }
    #[test]
    fn cooling_waits_only_while_the_warm_holder_itself_holds_the_session() {
        let temp = tempfile::tempdir().unwrap();
        let home = glen_home::GlenHome {
            root: temp.path().join("glen"),
        };
        let session = "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de";
        let me = liveness::current().unwrap();
        let warm = lock::Holder {
            v: 1,
            kind: lock::HolderKind::Warm,
            job_id: Some(JobId::new().as_str().into()),
            pid: me.pid,
            start: Some("the-cooled-warm-worker".into()),
        };
        // The deadline passing is the only way out of a wait that never ends.
        let deadline = Instant::now() + Duration::from_millis(300);
        let timed_out = || {
            if Instant::now() >= deadline {
                Err(GlenError::new(ErrorCode::ControlFailed, "deadline"))
            } else {
                Ok(())
            }
        };
        assert!(wait_holder_gone(&home, session, &warm, deadline, timed_out).is_ok());
        let held = lock::try_acquire(&home, session).unwrap().unwrap();
        // The warm worker cooled and a cold job took the session: return so `owner()` routes to it.
        let job = lock::Holder {
            kind: lock::HolderKind::Job,
            start: Some(me.start.clone()),
            ..warm.clone()
        };
        lock::mark(&held, &job).unwrap();
        assert!(wait_holder_gone(&home, session, &warm, deadline, timed_out).is_ok());
        // The warm worker still holding the session: wait until the deadline.
        lock::mark(&held, &warm).unwrap();
        let e = wait_holder_gone(&home, session, &warm, deadline, timed_out).unwrap_err();
        assert_eq!(e.message, "deadline");
    }
}
