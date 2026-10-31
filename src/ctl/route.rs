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
    let warnings = if hello.ext_version.as_deref() != Some(env!("CARGO_PKG_VERSION")) {
        vec![Warning {
            code: "extension_version_mismatch".into(),
            message: format!(
                "extension version {} differs from glen {}; run `glen install-extension` and restart omp",
                hello.ext_version.as_deref().unwrap_or("unknown"),
                env!("CARGO_PKG_VERSION")
            ),
        }]
    } else {
        vec![]
    };
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
fn unavailable(opened: &Opened, probe: &Lease, paths: &paths::OmpPaths) -> GlenError {
    let message = match probe {
        Lease::Unknown { .. } => format!(
            "cannot tell whether another omp process owns session {} ({})",
            opened.handle,
            lease::state_detail(probe).unwrap_or_default()
        ),
        _ => format!("session {} is open in another omp process", opened.handle),
    };
    let reason = if matches!(probe, Lease::Unknown { .. }) {
        NoRouteReason::UnknownLease
    } else {
        NoRouteReason::LiveOwner
    };
    GlenError::no_route(reason, message).with_hint(live::route::lease_held_hint(paths))
}
pub(crate) fn route(
    cli: &Cli,
    env: &Env,
    opened: &Opened,
    op_name: &str,
    deadline: Instant,
    stop: Arc<AtomicBool>,
) -> Result<(Target, Vec<Warning>), GlenError> {
    crate::sandbox::refuse_unreachable(env, cli.command.name())?;
    let home = glen_home::resolve(env)?;
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
                    return Err(GlenError::no_route(NoRouteReason::RpcJobRunning,format!("session {} is open in a live omp session and a glen RPC job is still running on it",opened.handle)).with_hint(format!("wait for that job (`glen jobs --active --session {}`), then send again",opened.handle)));
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
                if let Some(error) = io {
                    let mapped = match kind {
                        live::route::Unhealthy::Unresponsive => {
                            crate::sandbox::map_socket_failure(env, "connect omp socket", &error)
                        }
                        live::route::Unhealthy::Unreadable
                            if crate::sandbox::restricted(env, &error) =>
                        {
                            Some(crate::sandbox::map_home_write(
                                env,
                                cli.command.name(),
                                error,
                            ))
                        }
                        _ => None,
                    };
                    if let Some(error) = mapped {
                        return Err(error);
                    }
                }
                return Err(GlenError::no_route(
                    NoRouteReason::UnhealthyEndpoint,
                    format!(
                        "session {} has a glen live endpoint that cannot take controls ({reason})",
                        opened.handle
                    ),
                )
                .with_hint(live::route::hint(kind)));
            }
            _ => (),
        }
        crate::sandbox::refuse_rpc(env, cli.command.name())?;
        if lock::is_held(&home, id) {
            let holder = owner(&store, id, deadline, &stop);
            stopped(deadline, &stop, opened)?;
            match holder {
                Owner::Job(job) => return Ok((job_target(store, job, id, deadline, stop), vec![])),
                Owner::Free => continue,
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
            return Err(unavailable(opened, &probe, &paths));
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
    let home = glen_home::resolve(env)?;
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
        if lock::is_held(&home, id)
            && let Owner::Job(job) = owner(&store, id, deadline, &stop)
        {
            return Ok((
                job_target(store, job, id, deadline, stop),
                warnings,
                ModelsSource::Session,
            ));
        }
        let probe = lease::probe(&paths.session_owners_dir, id);
        if !matches!(probe, Lease::Free) {
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
}
