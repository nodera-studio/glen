use crate::{
    env::Env,
    error::{ErrorCode, GlenError, NoRouteReason},
    glen_home,
};
use serde_json::json;
use std::io;
#[derive(Debug, PartialEq, Eq)]
pub struct Sandbox {
    pub kind: Option<String>,
    pub network: bool,
}
pub fn detect(env: &Env) -> Option<Sandbox> {
    let kind = env
        .get("CODEX_SANDBOX")
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    let network = env.get("CODEX_SANDBOX_NETWORK_DISABLED") != Some("1");
    kind.is_some().then_some(Sandbox { kind, network })
}
fn refusal(command: &str, s: &Sandbox) -> GlenError {
    refusal_with_extension(
        command,
        s,
        crate::skill::visible_commands().contains("install-extension"),
    )
}
fn refusal_with_extension(command: &str, s: &Sandbox, extension: bool) -> GlenError {
    let message = if s.kind.is_some() {
        format!("`glen {command}` cannot reach omp from inside Codex's sandbox")
    } else {
        format!(
            "`glen {command}` cannot reach omp; Codex disabled the network for this command, so it may be sandboxed"
        )
    };
    let mut hint = "run this glen command outside the sandbox (request escalated permissions and approve the `glen` prefix)".to_owned();
    if extension {
        hint.push_str(", or enable network for the sandbox and use a session open in omp with glen's extension");
    }
    GlenError::no_route(NoRouteReason::Sandboxed, message)
        .with_hint(hint)
        .with_detail("sandbox", json!(s.kind))
        .with_detail("network", s.network)
}
pub fn refuse_unreachable(env: &Env, command: &str) -> Result<(), GlenError> {
    if let Some(s) = detect(env)
        && !s.network
    {
        return Err(refusal(command, &s));
    }
    Ok(())
}
pub fn refuse_rpc(env: &Env, command: &str) -> Result<(), GlenError> {
    if let Some(s) = detect(env) {
        return Err(refusal(command, &s));
    }
    Ok(())
}
pub fn restricted(env: &Env, err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
    ) && (detect(env).is_some() || env.get("CODEX_SANDBOX_NETWORK_DISABLED") == Some("1"))
}
pub fn map_home_write(env: &Env, command: &str, err: io::Error) -> GlenError {
    if restricted(env, &err) {
        let s = detect(env).unwrap_or(Sandbox {
            kind: None,
            network: false,
        });
        let path = glen_home::resolve(env)
            .map(|h| h.root.display().to_string())
            .unwrap_or_else(|_| "GLEN_HOME".into());
        let hint = if s.kind.is_some() {
            format!(
                "the sandbox cannot write GLEN_HOME ({path}); set GLEN_HOME under $TMPDIR, add it to `sandbox_workspace_write.writable_roots`, or run glen outside the sandbox"
            )
        } else {
            "set GLEN_HOME under $TMPDIR, add it to `sandbox_workspace_write.writable_roots`, or run glen outside the sandbox; Codex disabled the network for this command, so it may be sandboxed".into()
        };
        let mut error = refusal(command, &s).with_hint(hint);
        error.message = if s.kind.is_some() {
            format!("cannot write GLEN_HOME ({path}) for {command} inside Codex's sandbox: {err}")
        } else {
            format!(
                "cannot write GLEN_HOME ({path}): {err}; Codex disabled the network for this command, so it may be sandboxed"
            )
        };
        return error.with_detail("glen_home_writable", false);
    }
    GlenError::new(
        ErrorCode::Io,
        format!("cannot write glen home for {command}: {err}"),
    )
}
/// Map a failed socket connect/bind without inferring a sandbox kind from the network marker.
pub fn map_socket_failure(env: &Env, action: &str, err: &io::Error) -> Option<GlenError> {
    if !restricted(env, err) {
        return None;
    }
    let s = detect(env).unwrap_or(Sandbox {
        kind: None,
        network: false,
    });
    let mut error = refusal(action, &s);
    error.message = format!("cannot {action}: {err}");
    Some(error)
}
/// True when any Codex marker is set: a non-empty `CODEX_SANDBOX` or
/// `CODEX_SANDBOX_NETWORK_DISABLED=1`.
fn marked(env: &Env) -> bool {
    detect(env).is_some() || env.get("CODEX_SANDBOX_NETWORK_DISABLED") == Some("1")
}
/// On Linux omp's session lease is an abstract socket, which is scoped to a network namespace,
/// and a Codex sandbox may run glen in its own (`--unshare-net`, with `--unshare-pid` making
/// PID 1 the sandbox's own bwrap). No in-process signal tells such a namespace from the host's
/// reliably, so under any Codex marker on Linux the lease is hidden: displayed `unknown`, and
/// every RPC resume refuses (AC-1028, W10 fix cycle 2 decision 1). `linux` is the unit seam.
pub fn lease_hidden(env: &Env, linux: bool) -> bool {
    #[cfg(test)]
    if let Some(forced) = FORCE_LEASE_HIDDEN.with(|f| f.get()) {
        return forced;
    }
    linux && marked(env)
}
#[cfg(test)]
thread_local! { static FORCE_LEASE_HIDDEN: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) }; }
/// Test-only seam: forces `lease_hidden`'s result for the current thread, so a caller that
/// passes `cfg!(target_os = "linux")` (`ls::infer_row`, `map_probe_failure`, the worker's
/// `route_probe`) behaves the same on every host OS. Call with `None` to restore the real
/// rule before the test ends.
#[cfg(test)]
pub(crate) fn force_lease_hidden_for_test(value: Option<bool>) {
    FORCE_LEASE_HIDDEN.with(|f| f.set(value));
}
/// The AC-1028 refusal: when the lease is hidden, an RPC resume of an existing session exits 6
/// `sandboxed` with `details.lease:"unknown"`, whatever a probe would say and whether or not a
/// glen job lock is held. Callers run it before creating a job or starting omp.
pub fn refuse_hidden_lease(env: &Env) -> Result<(), GlenError> {
    if !lease_hidden(env, cfg!(target_os = "linux")) {
        return Ok(());
    }
    let kind = detect(env).and_then(|s| s.kind);
    let network = env.get("CODEX_SANDBOX_NETWORK_DISABLED") != Some("1");
    Err(GlenError::no_route(
        NoRouteReason::Sandboxed,
        "cannot tell whether an omp process owns this session: on Linux omp's session lease is an abstract socket, and a sandbox's network namespace can hide it",
    )
    .with_hint(
        "glen will not resume an existing session over RPC from a Codex command on Linux; open the session in omp with glen's extension so glen can use the live route, or run glen from a terminal outside Codex",
    )
    .with_detail("sandbox", json!(kind))
    .with_detail("network", network)
    .with_detail("lease", json!("unknown")))
}
/// Every lease value is refused when `lease_hidden` (`refuse_hidden_lease`); otherwise only a
/// real probe error (`Lease::Unknown` with an errno) is mapped.
pub fn map_probe_failure(
    env: &Env,
    command: &str,
    lease: &crate::omp::lease::Lease,
) -> Option<GlenError> {
    if let Err(error) = refuse_hidden_lease(env) {
        return Some(error);
    }
    let crate::omp::lease::Lease::Unknown {
        reason,
        errno: Some(errno),
    } = lease
    else {
        return None;
    };
    let mut error = map_socket_failure(
        env,
        &format!("probe omp session lease for {command}"),
        &io::Error::from_raw_os_error(*errno),
    )?;
    error.message = format!("cannot probe omp session lease for {command}: {reason}");
    Some(error)
}
/// Map omp's transport diagnostics and framed I/O permission failures under Codex markers.
pub fn map_worker_failure(env: &Env, message: &str) -> Option<GlenError> {
    if detect(env).is_none() && env.get("CODEX_SANDBOX_NETWORK_DISABLED") != Some("1") {
        return None;
    }
    let lower = message.to_ascii_lowercase();
    let network_failure = [
        "failedtoopensocket",
        "couldn't reach",
        "unable to connect",
        "connection refused",
        "connectionrefused",
        "dns resolution",
        "name resolution",
        "getaddrinfo",
        "failed to lookup address",
        "could not resolve",
        "enotfound",
        "econnrefused",
        "etimeout",
        "etimedout",
        "enetunreach",
        "eai_again",
        "eai_noname",
    ]
    .iter()
    .any(|s| lower.contains(s));
    // Permission words and OS error suffixes can come from a provider. Require
    // connect/I/O framing or a known operation prefix together with an OS error.
    let io_framing = lower.starts_with("connect: ")
        || lower.contains(": connect: ")
        || lower.starts_with("io: ")
        || lower.contains(": io: ")
        || (lower.contains("(os error")
            && [
                "spawn omp: ",
                "bind omp socket: ",
                "write omp socket: ",
                "cannot connect ",
                "cannot bind ",
                "cannot spawn worker for ",
                "cannot write ",
            ]
            .iter()
            .any(|prefix| lower.starts_with(prefix)));
    let permission_failure = io_framing
        && [
            "permission denied",
            "operation not permitted",
            "read-only file system",
            "eperm",
            "eacces",
            "erofs",
        ]
        .iter()
        .any(|s| lower.contains(s));
    if !network_failure && !permission_failure {
        return None;
    }
    let s = detect(env).unwrap_or(Sandbox {
        kind: None,
        network: false,
    });
    let mut error = refusal("send", &s);
    error.message = format!("cannot run omp: {message}");
    Some(error)
}
pub fn describe(env: &Env) -> (&'static str, String, String) {
    describe_with_extension(
        env,
        crate::skill::visible_commands().contains("install-extension"),
        cfg!(target_os = "linux"),
    )
}
fn describe_with_extension(
    env: &Env,
    extension: bool,
    linux: bool,
) -> (&'static str, String, String) {
    match detect(env) {
        None if env.get("CODEX_SANDBOX_NETWORK_DISABLED") == Some("1") => (
            "info",
            "host network off".into(),
            if lease_hidden(env, linux) {
                "network disabled by the host; on Linux glen will not resume an existing session over RPC here (exit 6 sandboxed); the live route and new sessions still work".into()
            } else {
                "network disabled by the host; escalated commands still work".into()
            },
        ),
        None => ("info", "none".into(), String::new()),
        Some(s) if !s.network => (
            "warn",
            format!(
                "codex {}, network off",
                s.kind.as_deref().unwrap_or("unknown")
            ),
            "send and session controls cannot reach omp from here; run them outside the sandbox"
                .into(),
        ),
        Some(s) => (
            "warn",
            format!("codex {}", s.kind.as_deref().unwrap_or("unknown")),
            if extension {
                "send works only into sessions open in omp with glen's extension; GLEN_HOME must be writable".into()
            } else {
                "RPC sends require running outside the sandbox; GLEN_HOME must be writable".into()
            },
        ),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn worker_network_diagnostics_require_a_codex_marker() {
        for message in [
            "Couldn't reach foo.example.com (FailedToOpenSocket). Check the URL, your network, or proxy.",
            "FailedToOpenSocket",
            "Couldn't reach foo.example.com. Check the URL, your network, or proxy.",
            "Unable to connect. Is the computer able to access the url?",
            "ConnectionRefused",
            "Couldn't connect to 127.0.0.1:443 (connection refused). Check that the server is running and reachable.",
            "No internet connection: couldn't reach api.anthropic.com (getaddrinfo ETIMEOUT). Check your network or proxy and try again.",
            "ETIMEOUT",
            "connect ETIMEDOUT 1.2.3.4:443",
            "connect ENETUNREACH 1.2.3.4:443",
            "connect ECONNREFUSED 127.0.0.1:443",
            "Network error: getaddrinfo ENOTFOUND api.openai.com",
            "ENOTFOUND",
            "getaddrinfo EAI_NONAME api.anthropic.com",
            "EAI_NONAME",
            "EAI_AGAIN",
            "eai_again",
            "connect: Connection refused",
            "DNS resolution failed",
            "connect: Operation not permitted",
        ] {
            for vars in [
                vec![("CODEX_SANDBOX", "seatbelt")],
                vec![("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
            ] {
                let env = Env::for_test(None, Path::new("/"), &vars);
                let error = map_worker_failure(&env, message).unwrap();
                assert_eq!(error.code.exit_code(), 6);
                assert_eq!(error.details.unwrap()["reason"], "sandboxed");
                assert!(error.hint.unwrap().contains("outside the sandbox"));
            }
            let env = Env::for_test(None, Path::new("/"), &[]);
            assert!(map_worker_failure(&env, message).is_none());
        }
    }
    #[test]
    fn worker_io_permission_failures_require_framing_and_a_codex_marker() {
        for message in [
            "cannot start omp: io: Permission denied (os error 13) (see worker.log)",
            "spawn omp: Operation not permitted (os error 1)",
            "bind omp socket: Permission denied (os error 13)",
            "write omp socket: Read-only file system (os error 30)",
            "cannot connect omp socket: Permission denied (os error 13)",
            "cannot bind lease socket: Operation not permitted (os error 1)",
            "cannot spawn worker for job j_123 at /glen/jobs/j_123: Permission denied (os error 13)",
            "cannot write glen home for send: Read-only file system (os error 30)",
            "io: Permission denied",
            "cannot start omp: io: Operation not permitted (see worker.log)",
        ] {
            for vars in [
                vec![("CODEX_SANDBOX", "seatbelt")],
                vec![("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
            ] {
                let env = Env::for_test(None, Path::new("/"), &vars);
                let error = map_worker_failure(&env, message).unwrap();
                assert_eq!(error.code.exit_code(), 6);
                assert_eq!(error.details.unwrap()["reason"], "sandboxed");
            }
            let env = Env::for_test(None, Path::new("/"), &[]);
            assert!(map_worker_failure(&env, message).is_none());
        }
    }
    #[test]
    fn worker_permission_prose_is_not_a_sandbox_failure() {
        for vars in [
            vec![],
            vec![("CODEX_SANDBOX", "seatbelt")],
            vec![("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
        ] {
            let env = Env::for_test(None, Path::new("/"), &vars);
            for message in [
                "403: Permission denied",
                "403: Permission denied (os error 13)",
                "403: Operation not permitted",
                "provider: read-only file system",
                "spawn omp: Operation not permitted",
                "connect EPERM",
                "provider EACCES",
                "provider EROFS",
                "provider scenario: Permission denied",
            ] {
                assert!(map_worker_failure(&env, message).is_none());
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn socket_connect_eperm_is_a_sandbox_failure() {
        let env = Env::for_test(None, Path::new("/"), &[("CODEX_SANDBOX", "seatbelt")]);
        let error = map_socket_failure(
            &env,
            "connect omp socket",
            &io::Error::from_raw_os_error(libc::EPERM),
        )
        .unwrap();
        assert_eq!(error.code.exit_code(), 6);
        assert_eq!(error.details.unwrap()["reason"], "sandboxed");
        assert!(error.hint.unwrap().contains("outside the sandbox"));
    }
    #[test]
    fn detection_and_refusals() {
        for (vars, exists, network) in [
            (vec![], false, true),
            (vec![("CODEX_SANDBOX", "")], false, true),
            (vec![("CODEX_SANDBOX", "seatbelt")], true, true),
            (vec![("CODEX_SANDBOX_NETWORK_DISABLED", "1")], false, true),
            (
                vec![
                    ("CODEX_SANDBOX", "seatbelt"),
                    ("CODEX_SANDBOX_NETWORK_DISABLED", "1"),
                ],
                true,
                false,
            ),
        ] {
            let e = Env::for_test(Some(Path::new("/h")), Path::new("/w"), &vars);
            assert_eq!(detect(&e).is_some(), exists);
            assert_eq!(refuse_unreachable(&e, "send").is_err(), !network);
            assert_eq!(refuse_rpc(&e, "send").is_err(), exists);
            let error = map_home_write(&e, "send", io::ErrorKind::PermissionDenied.into());
            assert_eq!(
                error.code,
                if exists || e.get("CODEX_SANDBOX_NETWORK_DISABLED") == Some("1") {
                    ErrorCode::NoRoute
                } else {
                    ErrorCode::Io
                }
            );
            if exists {
                assert_eq!(error.details.as_ref().unwrap()["glen_home_writable"], false);
                assert_eq!(error.details.as_ref().unwrap()["network"], network);
            }
        }
    }
    #[test]
    fn lease_hidden_truth_table() {
        // W10 fix cycle 2 decision 1: on Linux any Codex marker hides the lease; elsewhere
        // (flock leases) nothing is ever hidden.
        let both = vec![
            ("CODEX_SANDBOX", "seatbelt"),
            ("CODEX_SANDBOX_NETWORK_DISABLED", "1"),
        ];
        for (vars, marked) in [
            (vec![], false),
            (vec![("CODEX_SANDBOX", "")], false),
            (vec![("CODEX_SANDBOX_NETWORK_DISABLED", "0")], false),
            (
                vec![
                    ("CODEX_SANDBOX", ""),
                    ("CODEX_SANDBOX_NETWORK_DISABLED", "0"),
                ],
                false,
            ),
            (vec![("CODEX_SANDBOX", "seatbelt")], true),
            (vec![("CODEX_SANDBOX_NETWORK_DISABLED", "1")], true),
            (
                vec![
                    ("CODEX_SANDBOX", ""),
                    ("CODEX_SANDBOX_NETWORK_DISABLED", "1"),
                ],
                true,
            ),
            (both, true),
        ] {
            let e = Env::for_test(None, Path::new("/"), &vars);
            assert_eq!(lease_hidden(&e, true), marked, "{vars:?} linux");
            assert!(!lease_hidden(&e, false), "{vars:?} not linux");
        }
    }
    #[test]
    fn the_doctor_row_for_network_disabled_alone_names_the_linux_rule() {
        let e = Env::for_test(
            None,
            Path::new("/"),
            &[("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
        );
        let (status, value, detail) = describe_with_extension(&e, true, true);
        assert_eq!((status, value.as_str()), ("info", "host network off"));
        assert_eq!(
            detail,
            "network disabled by the host; on Linux glen will not resume an existing session over RPC here (exit 6 sandboxed); the live route and new sessions still work"
        );
        assert_eq!(
            describe_with_extension(&e, true, false).2,
            "network disabled by the host; escalated commands still work"
        );
    }
    #[test]
    fn refuse_hidden_lease_follows_the_predicate() {
        let e = Env::for_test(
            None,
            Path::new("/"),
            &[("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
        );
        force_lease_hidden_for_test(Some(false));
        assert!(refuse_hidden_lease(&e).is_ok());
        force_lease_hidden_for_test(Some(true));
        let error = refuse_hidden_lease(&e).unwrap_err();
        force_lease_hidden_for_test(None);
        assert_eq!(error.code.exit_code(), 6);
        let details = error.details.unwrap();
        assert_eq!(details["reason"], "sandboxed");
        assert_eq!(details["lease"], "unknown");
        assert_eq!(details["network"], false);
        assert!(details["sandbox"].is_null());
        // With no forcing, the real rule: refused on Linux under a marker, never elsewhere.
        assert_eq!(refuse_hidden_lease(&e).is_err(), cfg!(target_os = "linux"));
        assert!(refuse_hidden_lease(&Env::for_test(None, Path::new("/"), &[])).is_ok());
    }
    #[test]
    fn map_probe_failure_when_hidden_refuses_every_lease_value() {
        use crate::omp::lease::Lease;
        let e = Env::for_test(
            None,
            Path::new("/"),
            &[("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
        );
        force_lease_hidden_for_test(Some(true));
        for lease in [
            Lease::Free,
            Lease::Held,
            Lease::Unknown {
                reason: "probe failed".into(),
                errno: Some(libc::EACCES),
            },
        ] {
            let error = map_probe_failure(&e, "send", &lease).expect("sandboxed refusal");
            assert_eq!(error.code.exit_code(), 6);
            let details = error.details.unwrap();
            assert_eq!(details["reason"], "sandboxed");
            assert_eq!(details["lease"], "unknown");
            assert!(!error.hint.unwrap().contains("escalat"));
        }
        force_lease_hidden_for_test(None);
    }
    #[test]
    fn map_probe_failure_when_not_hidden_only_maps_a_real_probe_error() {
        use crate::omp::lease::Lease;
        let e = Env::for_test(None, Path::new("/"), &[]);
        assert!(map_probe_failure(&e, "send", &Lease::Free).is_none());
        assert!(map_probe_failure(&e, "send", &Lease::Held).is_none());
        assert!(
            map_probe_failure(
                &e,
                "send",
                &Lease::Unknown {
                    reason: "no marker".into(),
                    errno: Some(libc::EACCES),
                }
            )
            .is_none()
        );
    }
}

#[cfg(test)]
mod cycle1_tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn only_nonempty_sandbox_refuses_and_erofs_maps_for_host_and_sandbox() {
        // The raw-errno lease mapping below is the non-hidden path (macOS); on Linux a marker
        // hides the lease first (W10 fix cycle 2 decision 1, tested in `tests`).
        force_lease_hidden_for_test(Some(false));
        for vars in [
            vec![("CODEX_SANDBOX", "seatbelt")],
            vec![("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
        ] {
            let env = Env::for_test(Some(Path::new("/h")), Path::new("/w"), &vars);
            for kind in [
                io::ErrorKind::PermissionDenied,
                io::ErrorKind::ReadOnlyFilesystem,
            ] {
                let error = map_home_write(&env, "send", kind.into());
                assert_eq!(error.code, ErrorCode::NoRoute);
                assert_eq!(error.details.unwrap()["reason"], "sandboxed");
            }
            #[cfg(unix)]
            for errno in [libc::EPERM, libc::EACCES, libc::EROFS] {
                let error = map_probe_failure(
                    &env,
                    "send",
                    &crate::omp::lease::Lease::Unknown {
                        reason: "probe failed".into(),
                        errno: Some(errno),
                    },
                )
                .unwrap();
                assert_eq!(error.details.unwrap()["reason"], "sandboxed");
                assert!(error.hint.unwrap().contains("outside the sandbox"));
            }
        }
        force_lease_hidden_for_test(None);
        let env = Env::for_test(None, Path::new("/w"), &[("CODEX_SANDBOX", " ")]);
        assert!(refuse_rpc(&env, "send").is_err());
        let env = Env::for_test(None, Path::new("/w"), &[]);
        assert_eq!(
            map_home_write(&env, "send", io::ErrorKind::ReadOnlyFilesystem.into()).code,
            ErrorCode::Io
        );
    }
}

#[cfg(all(test, unix))]
mod cycle2_tests {
    use super::*;
    use crate::omp::lease::Lease;
    use std::path::Path;
    #[test]
    fn marker_only_write_diagnostic() {
        let env = Env::for_test(
            Some(Path::new("/h")),
            Path::new("/w"),
            &[("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
        );
        let e = map_home_write(&env, "ls", io::ErrorKind::PermissionDenied.into());
        assert!(e.message.starts_with("cannot write GLEN_HOME (/h/.glen)"));
        assert!(e.message.contains("may be sandboxed"));
        assert!(!e.message.contains("inside Codex") && !e.message.contains("cannot reach omp"));
    }
    #[test]
    fn extension_wording_follows_command_visibility() {
        for visible in [false, true] {
            let s = Sandbox {
                kind: Some("seatbelt".into()),
                network: true,
            };
            let e = refusal_with_extension("send", &s, visible);
            assert_eq!(e.hint.unwrap().contains("glen's extension"), visible);
            let env = Env::for_test(None, Path::new("/"), &[("CODEX_SANDBOX", "seatbelt")]);
            assert_eq!(
                describe_with_extension(&env, visible, false)
                    .2
                    .contains("glen's extension"),
                visible
            );
        }
    }
    #[test]
    fn linux_markers_map_socket_bind_connect_home_and_raw_lease_errno() {
        let env = Env::for_test(
            Some(Path::new("/h")),
            Path::new("/w"),
            &[("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
        );
        assert!(detect(&env).is_none());
        assert!(refuse_unreachable(&env, "send").is_ok());
        for errno in [libc::EPERM, libc::EACCES, libc::EROFS] {
            for action in ["connect omp socket", "bind lease socket"] {
                let e =
                    map_socket_failure(&env, action, &io::Error::from_raw_os_error(errno)).unwrap();
                assert_eq!(e.code.exit_code(), 6);
                assert_eq!(e.details.unwrap()["reason"], "sandboxed");
            }
            let lease = Lease::Unknown {
                reason: "arbitrary backend diagnostic".into(),
                errno: Some(errno),
            };
            assert_eq!(
                map_probe_failure(&env, "send", &lease)
                    .unwrap()
                    .code
                    .exit_code(),
                6
            );
            assert_eq!(
                map_home_write(&env, "ls", io::Error::from_raw_os_error(errno))
                    .code
                    .exit_code(),
                6
            );
        }
        let ordinary = Env::for_test(None, Path::new("/"), &[]);
        for errno in [libc::EPERM, libc::EACCES, libc::EROFS] {
            assert_eq!(
                map_home_write(&ordinary, "ls", io::Error::from_raw_os_error(errno)).code,
                ErrorCode::Io
            );
            assert!(
                map_socket_failure(
                    &ordinary,
                    "connect omp socket",
                    &io::Error::from_raw_os_error(errno)
                )
                .is_none()
            );
        }
        // Not hidden (macOS, or Linux with the seam): only a raw permission errno maps. On
        // Linux the marker hides the lease and refuses every value (W10 fix cycle 2 decision 1).
        force_lease_hidden_for_test(Some(false));
        for errno in [None, Some(libc::EINVAL)] {
            let lease = Lease::Unknown {
                reason: io::Error::from_raw_os_error(libc::EPERM).to_string(),
                errno,
            };
            assert!(map_probe_failure(&env, "send", &lease).is_none());
        }
        force_lease_hidden_for_test(None);
        for message in [
            "connect: Connection refused",
            "DNS resolution failed",
            "getaddrinfo EAI_AGAIN",
        ] {
            let error = map_worker_failure(&env, message).unwrap();
            assert_eq!(error.code.exit_code(), 6);
            assert_eq!(error.details.unwrap()["reason"], "sandboxed");
            assert!(error.hint.unwrap().contains("outside the sandbox"));
            assert!(
                map_worker_failure(&Env::for_test(None, Path::new("/"), &[]), message).is_none()
            );
        }
    }
}
