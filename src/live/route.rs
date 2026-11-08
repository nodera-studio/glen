use super::{
    client::{self, ClientError, ConnectError, Hello},
    registry::{self, Registry},
};
use crate::{
    env::Env,
    error::{GlenError, NoRouteReason, Warning},
    glen_home::GlenHome,
    omp::lease::Lease,
};
use std::{io, time::Duration};
#[derive(Debug)]
pub enum Endpoint {
    None,
    Stale(String),
    Live {
        reg: Box<Registry>,
        hello: Box<Hello>,
    },
    Unhealthy {
        reason: String,
        kind: Unhealthy,
        /// The underlying I/O error, when this came from a socket connect or a
        /// registry-cleanup write; lets the caller distinguish a sandbox
        /// permission denial from a genuine endpoint problem.
        io: Option<io::Error>,
    },
}
#[derive(Clone, Copy, Debug)]
pub enum Unhealthy {
    Unreadable,
    PathTooLong,
    Unresponsive,
    Incompatible,
    /// S-18: the registry's socket or its kernel peer is not the registered endpoint process, or
    /// that process runs more confined than glen. Never cleaned up: glen cannot prove it dead.
    Untrusted,
}
fn cleanup(home: &GlenHome, reg: &Registry, socket_too: bool) -> Option<Endpoint> {
    let (reason, io_err) = match registry::remove_if_unchanged(home, reg, socket_too) {
        Ok(true)
            if matches!(
                registry::read(home, &reg.session_id),
                registry::Read::Missing
            ) =>
        {
            return None;
        }
        Ok(_) => (
            "live registry changed during stale cleanup; retry delivery".into(),
            None,
        ),
        Err(e) => (e.message.clone(), Some(e.io)),
    };
    Some(Endpoint::Unhealthy {
        reason,
        kind: Unhealthy::Unreadable,
        io: io_err,
    })
}
pub fn find(home: &GlenHome, id: &str, timeout: Duration) -> Endpoint {
    let (reg, refused) = match registry::read(home, id) {
        registry::Read::Missing => return Endpoint::None,
        registry::Read::Unreadable(reason) => {
            return Endpoint::Unhealthy {
                reason,
                kind: Unhealthy::Unreadable,
                io: None,
            };
        }
        registry::Read::Untrusted(r, reason) => (r, Some(reason)),
        registry::Read::Ok(r) => (r, None),
    };
    // A dead pid's registry is provably stale whatever it names: cleanup unlinks only
    // `run/live/<pid>.sock`, never the registry's own socket path.
    if registry::pid_dead(&reg) {
        if let Some(e) = cleanup(home, &reg, true) {
            return e;
        }
        return Endpoint::Stale(format!("pid {} is gone", reg.pid));
    }
    if !super::socket_path_ok(&reg.socket) {
        return Endpoint::Unhealthy {
            reason: "socket path too long".into(),
            kind: Unhealthy::PathTooLong,
            io: None,
        };
    }
    if let Some(reason) = refused {
        return untrusted(reason);
    }
    match client::hello(&reg.socket, reg.pid, timeout) {
        Ok(hello) if hello.pid != reg.pid => untrusted(format!(
            "the endpoint answers as pid {}, not as the registered pid {}",
            hello.pid, reg.pid
        )),
        Ok(hello) if hello.session_id != id => {
            if let Some(e) = cleanup(home, &reg, false) {
                return e;
            }
            Endpoint::Stale(format!("now on {}", hello.session_id))
        }
        Ok(hello) if hello.protocol == 1 && hello.ops.iter().any(|s| s == "send") => {
            Endpoint::Live {
                reg,
                hello: Box::new(hello),
            }
        }
        Ok(_) => Endpoint::Unhealthy {
            reason: "endpoint does not support protocol 1 send".into(),
            kind: Unhealthy::Incompatible,
            io: None,
        },
        Err(ClientError::Untrusted(reason)) => untrusted(reason),
        Err(ClientError::Connect(ConnectError::Missing | ConnectError::Refused)) => {
            if let Some(e) = cleanup(home, &reg, false) {
                return e;
            }
            Endpoint::Stale("socket went away".into())
        }
        Err(e) => {
            let reason = e.to_string();
            let kind = if matches!(e, ClientError::Refused(_)) {
                Unhealthy::Incompatible
            } else {
                Unhealthy::Unresponsive
            };
            let io = match e {
                ClientError::Connect(ConnectError::Io(io_err)) => Some(io_err),
                _ => None,
            };
            Endpoint::Unhealthy { reason, kind, io }
        }
    }
}
fn untrusted(reason: String) -> Endpoint {
    Endpoint::Unhealthy {
        reason,
        kind: Unhealthy::Untrusted,
        io: None,
    }
}
pub fn hint(kind: Unhealthy) -> String {
    if matches!(kind, Unhealthy::Untrusted) {
        "glen sends only to the omp process a live registry names, and never from outside a sandbox into one; restart that omp session outside any sandbox, or run `glen doctor`".into()
    } else if matches!(kind, Unhealthy::PathTooLong) {
        "shorten GLEN_HOME, run `glen install-extension`, then restart that omp session".into()
    } else {
        "run `glen install-extension`, then restart that omp session (only TUI sessions accept glen messages)".into()
    }
}

pub fn lease_held_hint(paths: &crate::omp::paths::OmpPaths) -> String {
    if paths.extensions_dir.join("glen/index.ts").is_file() {
        "restart that omp session so it loads the glen extension (only TUI sessions accept glen messages)".into()
    } else {
        "run `glen install-extension`, then restart that omp session".into()
    }
}

/// The refusal for a session whose omp lease is not free: `Held` is `live_owner`, `Unknown` is
/// `unknown_lease`. The single source for this mapping (send, controls, the RPC worker).
/// Callers check `Lease::Free` first; it maps like `Held` here.
pub(crate) fn lease_refusal(
    lease: &Lease,
    handle: &str,
    paths: &crate::omp::paths::OmpPaths,
) -> GlenError {
    let (reason, message) = match lease {
        Lease::Unknown { reason, .. } => (
            NoRouteReason::UnknownLease,
            format!("cannot tell whether another omp process owns session {handle} ({reason})"),
        ),
        Lease::Held | Lease::Free => (
            NoRouteReason::LiveOwner,
            format!("session {handle} is open in another omp process"),
        ),
    };
    GlenError::no_route(reason, message).with_hint(lease_held_hint(paths))
}

/// The sandbox refusal for an unhealthy endpoint whose I/O error a sandbox explains, if any.
pub(crate) fn unhealthy_sandbox(
    env: &Env,
    command: &str,
    kind: Unhealthy,
    io: Option<io::Error>,
) -> Option<GlenError> {
    let e = io?;
    match kind {
        Unhealthy::Unresponsive => {
            crate::sandbox::map_socket_failure(env, "connect omp socket", &e)
        }
        Unhealthy::Unreadable if crate::sandbox::restricted(env, &e) => {
            Some(crate::sandbox::map_home_write(env, command, e))
        }
        _ => None,
    }
}

/// The refusal for a session whose live endpoint is unhealthy: the sandbox refusal when a
/// sandbox explains it, otherwise `unhealthy_endpoint`.
pub(crate) fn unhealthy_refusal(
    env: &Env,
    command: &str,
    handle: &str,
    reason: &str,
    kind: Unhealthy,
    io: Option<io::Error>,
) -> GlenError {
    if let Some(e) = unhealthy_sandbox(env, command, kind, io) {
        return e;
    }
    let what = if command == "send" {
        "messages"
    } else {
        "controls"
    };
    if matches!(kind, Unhealthy::Untrusted) {
        return GlenError::no_route(
            NoRouteReason::UntrustedEndpoint,
            format!(
                "session {handle} has a live endpoint glen does not trust with {what} ({reason})"
            ),
        )
        .with_hint(hint(kind));
    }
    GlenError::no_route(
        NoRouteReason::UnhealthyEndpoint,
        format!("session {handle} has a glen live endpoint that cannot take {what} ({reason})"),
    )
    .with_hint(hint(kind))
}

/// The refusal while a glen RPC job holds the lock of a session that is also live.
pub(crate) fn rpc_job_running(handle: &str, command: &str) -> GlenError {
    let then = if command == "send" {
        "then send again".to_owned()
    } else {
        format!("then retry `glen {command}`")
    };
    GlenError::no_route(
        NoRouteReason::RpcJobRunning,
        format!(
            "session {handle} is open in a live omp session and a glen RPC job is still running on it"
        ),
    )
    .with_hint(format!(
        "wait for that job (`glen jobs --active --session {handle}`), {then}"
    ))
}

/// [`rpc_job_running`] while an idle warm worker (v0.2 W4) holds the lock: the hint adds how to
/// end it.
pub(crate) fn rpc_warm_held(handle: &str, command: &str) -> GlenError {
    let mut e = rpc_job_running(handle, command);
    e.hint = e
        .hint
        .map(|h| format!("{h}, or end the warm worker with `glen release {handle}`"));
    e
}
/// Warns when the live session's glen extension is not this glen's version.
pub(crate) fn extension_version_warning(hello: &Hello) -> Option<Warning> {
    let ours = env!("CARGO_PKG_VERSION");
    (hello.ext_version.as_deref() != Some(ours)).then(|| Warning {
        code: "extension_version_mismatch".into(),
        message: format!(
            "extension version {} differs from glen {ours}; run `glen install-extension` and restart omp",
            hello.ext_version.as_deref().unwrap_or("unknown"),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rpc_job_running_names_the_command_to_retry() {
        let send = rpc_job_running("e27c0de", "send");
        assert_eq!(send.details.as_ref().unwrap()["reason"], "rpc_job_running");
        assert!(send.hint.as_deref().unwrap().ends_with("then send again"));
        let compact = rpc_job_running("e27c0de", "compact");
        assert!(
            compact
                .hint
                .as_deref()
                .unwrap()
                .ends_with("then retry `glen compact`")
        );
        assert!(!compact.hint.unwrap().contains("send again"));
    }
    #[test]
    fn rpc_warm_held_adds_the_release_command() {
        let e = rpc_warm_held("e27c0de", "send");
        assert_eq!(e.details.as_ref().unwrap()["reason"], "rpc_job_running");
        let hint = e.hint.unwrap();
        assert!(hint.contains("then send again"), "{hint}");
        assert!(
            hint.ends_with("or end the warm worker with `glen release e27c0de`"),
            "{hint}"
        );
    }
    #[test]
    fn lease_refusal_maps_held_and_unknown() {
        let tree = tempfile::tempdir().unwrap();
        let env = Env::for_test(Some(tree.path()), tree.path(), &[]);
        let paths = crate::omp::paths::resolve(&env, None).unwrap();
        let held = lease_refusal(&Lease::Held, "e27c0de", &paths);
        assert_eq!(held.code, crate::error::ErrorCode::NoRoute);
        assert_eq!(held.details.as_ref().unwrap()["reason"], "live_owner");
        assert_eq!(held.hint, Some(lease_held_hint(&paths)));
        let unknown = lease_refusal(
            &Lease::Unknown {
                reason: "denied".into(),
                errno: None,
            },
            "e27c0de",
            &paths,
        );
        assert_eq!(unknown.details.as_ref().unwrap()["reason"], "unknown_lease");
        assert!(unknown.message.contains("(denied)"));
        assert_eq!(unknown.hint, Some(lease_held_hint(&paths)));
    }
}

#[cfg(all(test, unix))]
mod s04_tests {
    use super::*;
    use crate::jobs::liveness::force_pids_hidden_for_test;
    /// A registry whose pid is gone from this process's view, with its owned socket bound by a
    /// listener that never answers (so a hello times out instead of proving anything).
    fn planted(t: &tempfile::TempDir) -> (GlenHome, Registry, std::os::unix::net::UnixListener) {
        let home = GlenHome {
            root: t.path().join("g"),
        };
        let dir = home.ensure_dir("run/live").unwrap();
        let pid = crate::jobs::liveness::tests_dead_pid();
        let socket = dir.join(format!("{pid}.sock"));
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let reg = Registry {
            v: 1,
            pid,
            session_id: "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de".into(),
            session_file: None,
            socket,
            ext_version: None,
            omp_version: None,
            protocol: Some(1),
            mode: Some("tui".into()),
            started_at: None,
            cwd: None,
            title: None,
            pidns: None,
            boot_id: None,
        };
        std::fs::write(
            registry::path(&home, &reg.session_id),
            serde_json::to_vec(&reg).unwrap(),
        )
        .unwrap();
        (home, reg, listener)
    }
    /// S-04: under a Codex marker on Linux (forced here) a TUI in the host's pid namespace looks
    /// gone; its registry and socket must survive `find`. Without the marker they are cleaned.
    #[test]
    fn a_gone_pid_never_removes_a_registry_or_socket_where_pids_may_be_hidden() {
        for hidden in [true, false] {
            let t = tempfile::tempdir().unwrap();
            let (home, reg, _listener) = planted(&t);
            force_pids_hidden_for_test(Some(hidden));
            let found = find(&home, &reg.session_id, Duration::from_millis(200));
            force_pids_hidden_for_test(None);
            let kept = registry::path(&home, &reg.session_id).exists();
            assert_eq!(kept, hidden, "hidden {hidden}: {found:?}");
            assert_eq!(reg.socket.exists(), hidden, "hidden {hidden}");
            if hidden {
                assert!(
                    matches!(found, Endpoint::Unhealthy { .. }),
                    "an unanswered hello proves nothing: {found:?}"
                );
            } else {
                assert!(matches!(found, Endpoint::Stale(_)), "{found:?}");
            }
        }
    }
}
