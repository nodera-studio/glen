use crate::{
    env::Env,
    error::{ErrorCode, GlenError},
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
    GlenError::new(ErrorCode::NoRoute, message)
        .with_hint(hint)
        .with_details(json!({"reason":"sandboxed", "sandbox":s.kind, "network":s.network}))
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
        if let Some(details) = error.details.as_mut() {
            details["glen_home_writable"] = json!(false);
        }
        return error;
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
pub fn map_probe_failure(
    env: &Env,
    command: &str,
    lease: &crate::omp::lease::Lease,
) -> Option<GlenError> {
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
    )
}
fn describe_with_extension(env: &Env, extension: bool) -> (&'static str, String, String) {
    match detect(env) {
        None if env.get("CODEX_SANDBOX_NETWORK_DISABLED") == Some("1") => (
            "info",
            "host network off".into(),
            "network disabled by the host; escalated commands still work".into(),
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
}

#[cfg(test)]
mod cycle1_tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn only_nonempty_sandbox_refuses_and_erofs_maps_for_host_and_sandbox() {
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
                describe_with_extension(&env, visible)
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
        for errno in [None, Some(libc::EINVAL)] {
            let lease = Lease::Unknown {
                reason: io::Error::from_raw_os_error(libc::EPERM).to_string(),
                errno,
            };
            assert!(map_probe_failure(&env, "send", &lease).is_none());
        }
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
