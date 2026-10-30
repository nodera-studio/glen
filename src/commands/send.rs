use crate::{
    cli::{Cli, SendArgs},
    env::Env,
    error::{ErrorCode, GlenError, Warning},
    glen_home,
    jobs::{Job, JobError, JobId, JobStore, Route, State, lock, wait},
    omp::{bin, lease::Lease, paths},
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
        Some(super::open::open(
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
            .read_to_end(&mut bytes)?;
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
    let omp_bin = bin::find_omp(env).map_err(|m| {
        GlenError::new(ErrorCode::OmpUnavailable, m).with_hint("install omp or set GLEN_OMP_BIN")
    })?;
    let home = glen_home::resolve(env)?;
    if let Some(o) = &opened
        && !lock::is_held(&home, &o.session.header.id)
    {
        let message = match &o.lease {
            Lease::Free => None,
            Lease::Held => Some(format!(
                "session {} is open in another omp process",
                o.handle
            )),
            Lease::Unknown(r) => Some(format!(
                "cannot tell whether another omp process owns session {} ({r})",
                o.handle
            )),
        };
        if let Some(message) = message {
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
    store.create(&job)?;
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
            return Err(e.into());
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
        |s| {
            spinner.set_message(if s.status == State::Accepted {
                "omp is working"
            } else {
                "queued behind another glen job"
            })
        },
    );
    spinner.finish();
    super::wait::outcome(&store, &job, waited?, ctx)
}
