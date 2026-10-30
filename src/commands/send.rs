use crate::{
    cli::{Cli, SendArgs},
    env::Env,
    error::{ErrorCode, GlenError, Warning},
    glen_home,
    jobs::{Job, JobError, JobId, JobStore, Route, State, lock, wait},
    omp::{
        bin,
        lease::{self, Lease},
        paths,
    },
    output::{Mode, Outcome, OutputCtx},
    render::spinner::Spinner,
    rpc,
};
use std::{
    io::{self, Read},
    path::Path,
    time::{Duration, Instant},
};
pub fn run(cli: &Cli, args: &SendArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    if args.cwd.is_some() && !matches!(args.session.as_str(), "@new" | "@latest") {
        return Err(GlenError::new(
            ErrorCode::Usage,
            "--cwd only applies to @new and @latest",
        ));
    }
    let paths = paths::resolve(env, cli.global.profile.as_deref())?;
    let scope = args
        .cwd
        .as_deref()
        .map(|p| paths::absolute(env, &p.to_string_lossy()));
    let scoped = scope.as_ref().map(|p| env.with_cwd(p));
    let opened = if args.session == "@new" {
        None
    } else {
        Some(super::open::open_unprobed(
            cli,
            scoped.as_ref().unwrap_or(env),
            &args.session,
        )?)
    };
    let cwd = if let Some(o) = &opened {
        o.session
            .header
            .cwd
            .as_deref()
            .map(Path::new)
            .filter(|p| p.is_dir())
            .or(scope.as_deref())
            .unwrap_or(env.cwd())
            .to_path_buf()
    } else {
        scope.unwrap_or_else(|| env.cwd().to_path_buf())
    };
    if !cwd.is_dir() {
        return Err(GlenError::new(
            ErrorCode::NotFound,
            format!("no such directory: {}", cwd.display()),
        ));
    }
    let text = if args.message == "-" {
        let mut bytes = vec![];
        io::stdin()
            .take(rpc::MAX_MESSAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| {
                GlenError::new(
                    ErrorCode::Io,
                    format!(
                        "cannot read send message from stdin for {}: {e}",
                        args.session
                    ),
                )
            })?;
        if bytes.len() > rpc::MAX_MESSAGE_BYTES {
            return Err(GlenError::new(
                ErrorCode::Usage,
                "message is larger than 512 KiB",
            ));
        }
        String::from_utf8(bytes)
            .map_err(|_| GlenError::new(ErrorCode::Usage, "message is not UTF-8"))?
    } else {
        args.message.clone()
    };
    if text.trim().is_empty() {
        return Err(GlenError::new(ErrorCode::Usage, "message is empty"));
    }
    if text.len() > rpc::MAX_MESSAGE_BYTES {
        return Err(GlenError::new(
            ErrorCode::Usage,
            "message is larger than 512 KiB",
        ));
    }
    crate::sandbox::refuse_unreachable(env, "send")?;
    crate::sandbox::refuse_rpc(env, "send")?;
    let omp_bin = bin::find_omp(env).map_err(|m| {
        GlenError::new(ErrorCode::OmpUnavailable, m).with_hint("install omp or set GLEN_OMP_BIN")
    })?;
    let home = glen_home::resolve(env)?;
    if let Some(o) = &opened
        && !lock::is_held(&home, &o.session.header.id)
    {
        #[cfg(debug_assertions)]
        if let Some(path) = env.get("FAKE_OMP_LEASE_PROBE_GATE") {
            let ready = format!("{path}.ready");
            std::fs::write(&ready, b"probing").map_err(|e| {
                GlenError::new(
                    ErrorCode::Io,
                    format!("cannot write probe gate {ready}: {e}"),
                )
            })?;
            let end = Instant::now() + Duration::from_secs(5);
            while !Path::new(path).exists() {
                if Instant::now() >= end {
                    return Err(GlenError::new(
                        ErrorCode::Io,
                        format!("timed out waiting for probe gate {path}"),
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let message = match lease::probe(&paths.session_owners_dir, &o.session.header.id) {
            Lease::Free => None,
            Lease::Held => Some(format!(
                "session {} is open in another omp process",
                o.handle
            )),
            Lease::Unknown(r) => {
                if let Some(error) = crate::sandbox::map_probe_failure(env, "send", &r) {
                    return Err(error);
                }
                Some(format!(
                    "cannot tell whether another omp process owns session {} ({r})",
                    o.handle
                ))
            }
        };
        if let Some(message) = message
            && !lock::is_held(&home, &o.session.header.id)
        {
            return Err(GlenError::new(ErrorCode::NoRoute,message).with_hint("close that omp session and retry; sending into a running session needs the glen omp extension (a later glen version)"));
        }
    }
    let (timeout, warning) = rpc::job_timeout(env);
    let created = Instant::now();
    let now = jiff::Timestamp::now();
    let deadline = now
        .checked_add(jiff::SignedDuration::from_millis(timeout.as_millis() as i64))
        .map_err(|e| GlenError::new(ErrorCode::Internal, e.to_string()))?;
    let id = JobId::new();
    let job = Job {
        v: 1,
        id: id.as_str().into(),
        handle: id.handle(),
        created_at: now.to_string(),
        route: if opened.is_some() {
            Route::Rpc
        } else {
            Route::New
        },
        session_id: opened.as_ref().map(|o| o.session.header.id.clone()),
        session_file: opened.as_ref().map(|o| o.session.path.clone()),
        session_handle: opened.as_ref().map(|o| o.handle.clone()),
        deliver_as: args.deliver().into(),
        text,
        cwd,
        omp_bin,
        profile_flag: cli
            .global
            .profile
            .as_deref()
            .map(str::trim)
            .map(str::to_owned),
        owners_dir: paths.session_owners_dir,
        job_timeout_ms: timeout.as_millis() as u64,
        deadline_at: deadline.to_string(),
    };
    let store = JobStore::new(home.clone());
    store.create_for_send(&job, env)?;
    store.prune(now);
    if let Some(w) = warning {
        let mut s = store.read_status(&id)?;
        s.warnings.push(Warning {
            code: w.code,
            message: w.message,
        });
        store.write_status(&id, &s)?;
    }
    let mut child = match rpc::spawn_worker(&home, &id) {
        Ok(c) => c,
        Err(e) => {
            let mut s = store.read_status(&id)?;
            s.status = State::Failed;
            s.updated_at = jiff::Timestamp::now().to_string();
            s.error = Some(JobError {
                code: "spawn".into(),
                message: e.to_string(),
                hint: None,
                details: None,
            });
            store.write_status(&id, &s)?;
            return Err(GlenError::new(
                ErrorCode::Io,
                format!(
                    "cannot spawn worker for job {} at {}: {e}",
                    job.handle,
                    store.dir(&id).display()
                ),
            ));
        }
    };
    let flag = wait::install_interrupt()?;
    let spinner = Spinner::start(
        ctx.mode == Mode::Human && ctx.stderr_tty,
        &format!(
            "sending to {} · {}",
            job.session_handle.as_deref().unwrap_or("@new"),
            job.handle
        ),
    );
    let waited = wait::observe(
        &store,
        &id,
        created,
        if args.no_wait {
            Duration::ZERO
        } else {
            args.wait
        },
        &flag,
        Some(&mut child),
        |s| spinner.set_message(super::wait::progress(s)),
    );
    spinner.finish();
    super::wait::outcome(&store, &job, waited?, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::{OutputKind, Terminal};
    use clap::Parser;
    #[test]
    fn sandbox_refusal_validates_target_and_message_but_never_probes_lease() {
        let tree = crate::omp::fixtures::tree();
        let target_path = crate::omp::fixtures::path(&tree, 'A');
        let agent = tree.path().join("unused-agent");
        for network in ["0", "1"] {
            let env = Env::for_test(
                Some(tree.path()),
                tree.path(),
                &[
                    ("PI_CODING_AGENT_DIR", agent.to_str().unwrap()),
                    ("CODEX_SANDBOX", "seatbelt"),
                    ("CODEX_SANDBOX_NETWORK_DISABLED", network),
                ],
            );
            for (target, message, code) in [
                (target_path.to_str().unwrap(), "hi", ErrorCode::NoRoute),
                ("fffffff", "hi", ErrorCode::NotFound),
                (target_path.to_str().unwrap(), " ", ErrorCode::Usage),
            ] {
                let cli = Cli::try_parse_from(["glen", "send", target, message]).unwrap();
                let crate::cli::Command::Send(args) = &cli.command else {
                    panic!("send")
                };
                let ctx = OutputCtx::new(
                    &cli.global,
                    OutputKind::Document,
                    &env,
                    &Terminal {
                        stdout_tty: false,
                        stderr_tty: false,
                        stdout_cols: None,
                    },
                )
                .unwrap();
                lease::PROBE_CALLS.with(|calls| calls.set(0));
                let err = match run(&cli, args, &env, &ctx) {
                    Err(e) => e,
                    Ok(_) => panic!("unexpected success"),
                };
                assert_eq!(err.code, code);
                lease::PROBE_CALLS.with(|calls| assert_eq!(calls.get(), 0));
            }
        }
    }
}
