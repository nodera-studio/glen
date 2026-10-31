use crate::{
    cli::{Cli, DeliverAs, SendArgs},
    env::Env,
    error::{ErrorCode, GlenError, NoRouteReason, Warning},
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
    path::{Path, PathBuf},
    process::Child,
    time::{Duration, Instant},
};
pub fn run(cli: &Cli, args: &SendArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    paths::resolve(env, cli.global.profile.as_deref())?;
    let scope = args
        .cwd
        .as_deref()
        .map(|p| paths::absolute(env, &p.to_string_lossy()));
    let opened = if args.session == "@new" {
        None
    } else {
        Some(super::open::open_unprobed_in(
            cli,
            env,
            &args.session,
            scope.as_deref(),
        )?)
    };
    let cwd = send_cwd(opened.as_ref(), scope.as_deref(), env)?;
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
    let Submitted {
        job,
        id,
        store,
        mut child,
        created,
    } = submit(cli, env, opened.as_ref(), cwd, text, args.deliver())?;
    let flag = wait::install_interrupt()?;
    let spinner = Spinner::start(
        ctx.mode == Mode::Human && ctx.stderr_tty,
        &super::wait::job_progress(&job, &store.read_status(&id)?),
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
        |s| spinner.set_message(&super::wait::job_progress(&job, s)),
    );
    spinner.finish();
    super::wait::outcome(&store, &job, waited?, ctx)
}

pub(crate) struct Submitted {
    pub job: Job,
    pub id: JobId,
    pub store: JobStore,
    pub child: Child,
    pub created: Instant,
}
/// Validate before reading stdin, preserving send's error precedence.
pub(crate) fn send_cwd(
    opened: Option<&super::open::Opened>,
    scope: Option<&Path>,
    env: &Env,
) -> Result<PathBuf, GlenError> {
    let cwd = if let Some(o) = opened {
        o.session
            .header
            .cwd
            .as_deref()
            .map(Path::new)
            .filter(|p| p.is_dir())
            .or(scope)
            .unwrap_or(env.cwd())
            .to_path_buf()
    } else {
        scope.unwrap_or(env.cwd()).to_path_buf()
    };
    if !cwd.is_dir() {
        return Err(GlenError::new(
            ErrorCode::NotFound,
            format!("no such directory: {}", cwd.display()),
        ));
    }
    Ok(cwd)
}
/// Submit an already-read message. No stdin, signal handlers, or terminal output.
pub(crate) fn submit(
    cli: &Cli,
    env: &Env,
    opened: Option<&super::open::Opened>,
    cwd: PathBuf,
    text: String,
    deliver: DeliverAs,
) -> Result<Submitted, GlenError> {
    let paths = paths::resolve(env, cli.global.profile.as_deref())?;
    if text.trim().is_empty() {
        return Err(GlenError::new(ErrorCode::Usage, "message is empty"));
    }
    if text.len() > rpc::MAX_MESSAGE_BYTES {
        return Err(GlenError::new(
            ErrorCode::Usage,
            "message is larger than 512 KiB",
        ));
    }
    crate::sandbox::refuse_unreachable(env, "send").map_err(|e| match opened {
        Some(_) => crate::sandbox::with_hidden_lease(env, e),
        None => e,
    })?;
    // Inside a sandbox, only the live route can work: @new is always RPC, and an
    // existing session is refused below once no live endpoint answers.
    if opened.is_none() {
        crate::sandbox::refuse_rpc(env, "send")?;
    }
    let home = glen_home::resolve(env)?;
    let mut live = None;
    let mut route_warnings = vec![];
    if let Some(o) = &opened {
        match crate::live::route::find(&home, &o.session.header.id, crate::live::HELLO_SEND) {
            crate::live::route::Endpoint::Live { reg, hello } => {
                if lock::is_held(&home, &o.session.header.id) {
                    return Err(GlenError::no_route(NoRouteReason::RpcJobRunning,format!("session {} is open in a live omp session and a glen RPC job is still running on it",o.handle)).with_hint(format!("wait for that job (`glen jobs --active --session {}`), then send again",o.handle)));
                }
                if hello.ext_version.as_deref() != Some(env!("CARGO_PKG_VERSION")) {
                    route_warnings.push(Warning{code:"extension_version_mismatch".into(),message:format!("extension version {} differs from glen {}; run `glen install-extension` and restart omp",hello.ext_version.as_deref().unwrap_or("unknown"),env!("CARGO_PKG_VERSION"))});
                }
                live = Some(reg);
            }
            crate::live::route::Endpoint::Unhealthy { reason, kind, io } => {
                if let Some(e) = io {
                    let mapped = match kind {
                        crate::live::route::Unhealthy::Unresponsive => {
                            crate::sandbox::map_socket_failure(env, "connect omp socket", &e)
                        }
                        crate::live::route::Unhealthy::Unreadable
                            if crate::sandbox::restricted(env, &e) =>
                        {
                            Some(crate::sandbox::map_home_write(env, "send", e))
                        }
                        _ => None,
                    };
                    if let Some(err) = mapped {
                        return Err(err);
                    }
                }
                return Err(GlenError::no_route(
                    NoRouteReason::UnhealthyEndpoint,
                    format!(
                        "session {} has a glen live endpoint that cannot take messages ({reason})",
                        o.handle
                    ),
                )
                .with_hint(crate::live::route::hint(kind)));
            }
            _ => (),
        }
        if live.is_none() {
            crate::sandbox::refuse_rpc(env, "send")
                .map_err(|e| crate::sandbox::with_hidden_lease(env, e))?;
        }
        if live.is_none() && o.not_yet_persisted {
            return Err(GlenError::no_route(
                NoRouteReason::EndpointGone,
                "the live session has not persisted a transcript and its endpoint went away",
            )
            .with_hint("restart that omp session and run `glen ls`"));
        }
        // Before the lock check: a held glen lock would otherwise skip the lease probe and
        // queue a job the worker could only refuse (W10 fix cycle 2 decision 2).
        if live.is_none() {
            crate::sandbox::refuse_hidden_lease(env)?;
        }
        if live.is_none() && !lock::is_held(&home, &o.session.header.id) {
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
            let lease = lease::probe(&paths.session_owners_dir, &o.session.header.id);
            if let Some(error) = crate::sandbox::map_probe_failure(env, "send", &lease) {
                return Err(error);
            }
            if !matches!(lease, Lease::Free) && !lock::is_held(&home, &o.session.header.id) {
                let message = match &lease {
                    unknown @ Lease::Unknown { .. } => format!(
                        "cannot tell whether another omp process owns session {} ({})",
                        o.handle,
                        lease::state_detail(unknown).unwrap_or_default()
                    ),
                    _ => format!("session {} is open in another omp process", o.handle),
                };
                let hint = crate::live::route::lease_held_hint(&paths);
                let reason = if matches!(lease, Lease::Held) {
                    NoRouteReason::LiveOwner
                } else {
                    NoRouteReason::UnknownLease
                };
                return Err(GlenError::no_route(reason, message).with_hint(hint));
            }
        }
    }
    let omp_bin = if live.is_none() {
        Some(bin::find_omp(env).map_err(|m| {
            GlenError::new(ErrorCode::OmpUnavailable, m)
                .with_hint("install omp or set GLEN_OMP_BIN")
        })?)
    } else {
        None
    };
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
        route: if live.is_some() {
            Route::Live
        } else if opened.is_some() {
            Route::Rpc
        } else {
            Route::New
        },
        session_id: opened.as_ref().map(|o| o.session.header.id.clone()),
        session_file: opened
            .as_ref()
            .filter(|o| !o.not_yet_persisted)
            .map(|o| o.session.path.clone()),
        session_handle: opened.as_ref().map(|o| o.handle.clone()),
        deliver_as: deliver.into(),
        text,
        cwd,
        omp_bin,
        live_socket: live.as_ref().map(|r| r.socket.clone()),
        live_pid: live.as_ref().map(|r| r.pid),
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
    route_warnings.extend(warning);
    if !route_warnings.is_empty() {
        let mut s = store.read_status(&id)?;
        s.warnings.extend(route_warnings);
        store.write_status(&id, &s)?;
    }
    let child = match rpc::spawn_worker(&home, &id) {
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
    Ok(Submitted {
        job,
        id,
        store,
        child,
        created,
    })
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
    /// W10 fix cycle 2 decision 2: a hidden lease refuses an RPC resume whether or not a glen
    /// job lock is held, before the lease probe, the omp lookup and any job write.
    #[test]
    fn a_hidden_lease_refuses_rpc_resume_even_while_the_glen_lock_is_held() {
        let tree = crate::omp::fixtures::tree();
        let target = crate::omp::fixtures::path(&tree, 'A');
        let glen = tempfile::tempdir().unwrap();
        let home = crate::glen_home::GlenHome {
            root: glen.path().join("glen"),
        };
        let agent = tree.path().join("unused-agent");
        let env = Env::for_test(
            Some(tree.path()),
            tree.path(),
            &[
                ("GLEN_HOME", home.root.to_str().unwrap()),
                ("PI_CODING_AGENT_DIR", agent.to_str().unwrap()),
                // A lookup would fail as omp_unavailable, never sandboxed.
                ("GLEN_OMP_BIN", "/nonexistent/omp"),
                ("CODEX_SANDBOX_NETWORK_DISABLED", "1"),
            ],
        );
        let id = crate::omp::session::Session::load(&target)
            .unwrap()
            .header
            .id;
        crate::sandbox::force_lease_hidden_for_test(Some(true));
        for held in [true, false] {
            let lock = held.then(|| lock::try_acquire(&home, &id).unwrap().unwrap());
            let cli =
                Cli::try_parse_from(["glen", "send", target.to_str().unwrap(), "hi"]).unwrap();
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
                Ok(_) => panic!("held={held}: unexpected success"),
            };
            assert_eq!(err.code, ErrorCode::NoRoute, "held={held}: {}", err.message);
            let details = err.details.unwrap();
            assert_eq!(details["reason"], "sandboxed", "held={held}");
            assert_eq!(details["lease"], "unknown", "held={held}");
            lease::PROBE_CALLS.with(|calls| assert_eq!(calls.get(), 0, "held={held}"));
            assert!(!home.root.join("jobs").exists(), "held={held}: no job");
            drop(lock);
        }
        crate::sandbox::force_lease_hidden_for_test(None);
    }
    /// W10 fix cycle 3 (Codex P2): `refuse_rpc` (non-empty `CODEX_SANDBOX`) and the
    /// network-disabled early return (both markers) fire before `refuse_hidden_lease`, and still
    /// name the hidden lease for an existing session; `@new` resumes nothing and never does.
    #[test]
    fn every_sandbox_refusal_of_an_existing_session_names_a_hidden_lease() {
        let tree = crate::omp::fixtures::tree();
        let target = crate::omp::fixtures::path(&tree, 'A');
        let glen = tempfile::tempdir().unwrap();
        let agent = tree.path().join("unused-agent");
        let home = glen.path().join("glen");
        for network in [None, Some("1")] {
            let mut vars = vec![
                ("GLEN_HOME", home.to_str().unwrap()),
                ("PI_CODING_AGENT_DIR", agent.to_str().unwrap()),
                ("GLEN_OMP_BIN", "/nonexistent/omp"),
                ("CODEX_SANDBOX", "seatbelt"),
            ];
            vars.extend(network.map(|v| ("CODEX_SANDBOX_NETWORK_DISABLED", v)));
            let env = Env::for_test(Some(tree.path()), tree.path(), &vars);
            for hidden in [true, false] {
                crate::sandbox::force_lease_hidden_for_test(Some(hidden));
                for (to, existing) in [(target.to_str().unwrap(), true), ("@new", false)] {
                    let cli = Cli::try_parse_from(["glen", "send", to, "hi"]).unwrap();
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
                    let case = format!("network={network:?} hidden={hidden} to={to}");
                    let err = match run(&cli, args, &env, &ctx) {
                        Err(e) => e,
                        Ok(_) => panic!("{case}: unexpected success"),
                    };
                    assert_eq!(err.code, ErrorCode::NoRoute, "{case}: {}", err.message);
                    let details = err.details.unwrap();
                    assert_eq!(details["reason"], "sandboxed", "{case}");
                    assert_eq!(details["sandbox"], "seatbelt", "{case}");
                    assert_eq!(details["network"], network.is_none(), "{case}");
                    assert_eq!(
                        details.get("lease"),
                        (hidden && existing).then_some(&serde_json::json!("unknown")),
                        "{case}: {details}"
                    );
                    lease::PROBE_CALLS.with(|calls| assert_eq!(calls.get(), 0, "{case}"));
                    assert!(!home.join("jobs").exists(), "{case}: no job");
                }
            }
        }
        crate::sandbox::force_lease_hidden_for_test(None);
    }
}

#[cfg(test)]
mod submit_tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn literal_hyphen_is_submitted_without_reading_stdin() {
        let tree = tempfile::tempdir().unwrap();
        let home = tree.path().join("glen");
        let agent = tree.path().join("agent");
        let bin = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bin/fake-omp");
        let env = Env::for_test(
            Some(tree.path()),
            tree.path(),
            &[
                ("GLEN_HOME", home.to_str().unwrap()),
                ("PI_CODING_AGENT_DIR", agent.to_str().unwrap()),
                ("GLEN_OMP_BIN", bin.to_str().unwrap()),
            ],
        );
        let cli = Cli::try_parse_from(["glen", "send", "@new", "-"]).unwrap();
        let cwd = send_cwd(None, None, &env).unwrap();
        let mut submitted = submit(&cli, &env, None, cwd, "-".into(), DeliverAs::FollowUp).unwrap();
        // The worker child is this unit-test binary, not a live omp process.
        if submitted.child.try_wait().unwrap().is_none() {
            submitted.child.kill().unwrap();
        }
        submitted.child.wait().unwrap();
        let stored = submitted.store.read_job(&submitted.id).unwrap();
        assert_eq!(stored.text, "-");
        assert_eq!(stored.deliver_as, crate::jobs::Deliver::FollowUp);
    }
    #[test]
    fn submit_rejects_empty_and_oversize_before_creating_home() {
        let tree = tempfile::tempdir().unwrap();
        let home = tree.path().join("glen");
        let env = Env::for_test(
            Some(tree.path()),
            tree.path(),
            &[("GLEN_HOME", home.to_str().unwrap())],
        );
        let cli = Cli::try_parse_from(["glen", "send", "@new", "text"]).unwrap();
        for text in [" \n".to_string(), "x".repeat(rpc::MAX_MESSAGE_BYTES + 1)] {
            let e = submit(&cli, &env, None, tree.path().into(), text, DeliverAs::Auto)
                .err()
                .unwrap();
            assert_eq!(e.code, ErrorCode::Usage);
            assert!(!home.exists());
        }
    }
    #[test]
    fn run_checks_missing_cwd_before_stdin() {
        let tree = tempfile::tempdir().unwrap();
        let missing = tree.path().join("missing");
        let env = Env::for_test(Some(tree.path()), tree.path(), &[]);
        let cli = Cli::try_parse_from([
            "glen",
            "send",
            "@new",
            "-",
            "--cwd",
            missing.to_str().unwrap(),
        ])
        .unwrap();
        let crate::cli::Command::Send(args) = &cli.command else {
            panic!("send")
        };
        let ctx = OutputCtx::fallback(
            Mode::Json,
            &env,
            &crate::output::Terminal {
                stdout_tty: false,
                stderr_tty: false,
                stdout_cols: None,
            },
        );
        let err = run(&cli, args, &env, &ctx).err().unwrap();
        assert_eq!(err.code, ErrorCode::NotFound);
        assert_eq!(
            err.message,
            format!("no such directory: {}", missing.display())
        );
    }
}
