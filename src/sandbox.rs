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
/// On Linux, a sandboxed glen in its own network namespace cannot see the abstract-socket
/// lease a host-namespace omp holds, so an RPC resume could fork a session omp already owns.
/// True under any Codex marker when glen's network namespace differs from PID 1's (the init
/// namespace escalated commands still run in) or that comparison cannot be made.
pub fn lease_hidden(env: &Env, linux: bool) -> bool {
    if let Some(forced) = FORCE_LEASE_HIDDEN.with(|f| f.get()) {
        return forced;
    }
    if !linux {
        return false;
    }
    let marker = detect(env).is_some() || env.get("CODEX_SANDBOX_NETWORK_DISABLED") == Some("1");
    marker && net_namespace_hidden()
}
thread_local! { static FORCE_LEASE_HIDDEN: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) }; }
/// Test-only seam: forces `lease_hidden`'s result for the current thread, so a caller like
/// `ls::infer_row`'s display mapping can be exercised deterministically on every host OS,
/// never the real `cfg!(target_os)` / `/proc` namespace identity. Call with `None` to restore
/// the real logic before the test ends.
#[cfg(test)]
pub(crate) fn force_lease_hidden_for_test(value: Option<bool>) {
    FORCE_LEASE_HIDDEN.with(|f| f.set(value));
}
/// Decides whether this process's network namespace is hidden from PID 1's, given three
/// injectable facts so the whole truth table is a deterministic unit test on every OS,
/// never real `/proc` reads:
/// - `self_ns`: this process's own namespace identity (following the `/proc/self/ns/net`
///   symlink to the nsfs target, never the per-process link's own inode — `fs::metadata`,
///   not `symlink_metadata`/`lstat`, which differs per pid even in the same namespace).
/// - `init_ns`: PID 1's namespace identity, the same way; unreadable on most hosts, since a
///   non-root process cannot follow another uid's `/proc/<pid>/ns/net` when PID 1 is root.
/// - `loopback_only`: true when this namespace has no interface but `lo` (the shape of a
///   freshly `--unshare-net`'d namespace), used only when `init_ns` is unreadable.
///
/// Hidden when: `self_ns` itself cannot be read (fail closed); or both are readable and
/// differ; or `init_ns` is unreadable and this namespace is loopback-only. Visible only when
/// both are readable and equal, or `init_ns` is unreadable and this namespace has a real
/// interface (the common non-root, non-sandboxed case: §A13's escalated commands).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // only called by the Linux probe; kept visible for the cross-platform unit test
fn net_namespace_hidden_with(
    self_ns: impl FnOnce() -> std::io::Result<(u64, u64)>,
    init_ns: impl FnOnce() -> std::io::Result<(u64, u64)>,
    loopback_only: impl FnOnce() -> bool,
) -> bool {
    let Ok(self_id) = self_ns() else {
        return true;
    };
    match init_ns() {
        Ok(init_id) => self_id != init_id,
        Err(_) => loopback_only(),
    }
}
#[cfg(target_os = "linux")]
fn ns_identity(path: &std::path::Path) -> std::io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    // Follow the magic symlink to the nsfs target; `symlink_metadata` (lstat) would instead
    // return the per-process link's own, always-distinct procfs inode.
    std::fs::metadata(path).map(|m| (m.dev(), m.ino()))
}
/// Reads this process's own network namespace's interfaces from `/proc/self/net/dev`, never
/// `/sys/class/net`: sysfs shows the namespace of whoever mounted it, and a bind-mounted host
/// `/sys` (bwrap's recursive `--ro-bind / /`, or an explicit `/sys` bind) lists the host's
/// interfaces inside a fresh `--unshare-net`, while procfs net entries follow the reader.
#[cfg(target_os = "linux")]
fn is_loopback_only() -> bool {
    // Can't tell: do not assume isolation from an unrelated read failure.
    std::fs::read_to_string("/proc/self/net/dev").is_ok_and(|dev| net_dev_loopback_only(&dev))
}
/// True when a `/proc/<pid>/net/dev` table names exactly one interface, `lo`. The table has
/// two header lines, then one `<name>: <counters>` line per interface.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // only called by the Linux probe; kept visible for the cross-platform unit test
fn net_dev_loopback_only(dev: &str) -> bool {
    let mut names: Vec<&str> = dev
        .lines()
        .filter_map(|line| line.split_once(':').map(|(name, _)| name.trim()))
        .filter(|name| !name.is_empty())
        .collect();
    names.sort_unstable();
    names == ["lo"]
}
#[cfg(target_os = "linux")]
fn net_namespace_hidden() -> bool {
    net_namespace_hidden_with(
        || ns_identity(std::path::Path::new("/proc/self/ns/net")),
        || ns_identity(std::path::Path::new("/proc/1/ns/net")),
        is_loopback_only,
    )
}
#[cfg(not(target_os = "linux"))]
fn net_namespace_hidden() -> bool {
    false
}
/// Every lease value is treated as unknown when `lease_hidden`; otherwise only a real probe
/// error (`Lease::Unknown` with an errno) is mapped.
pub fn map_probe_failure(
    env: &Env,
    command: &str,
    lease: &crate::omp::lease::Lease,
) -> Option<GlenError> {
    let _ = command;
    if lease_hidden(env, cfg!(target_os = "linux")) {
        let kind = detect(env).and_then(|s| s.kind);
        let network = env.get("CODEX_SANDBOX_NETWORK_DISABLED") != Some("1");
        let error = GlenError::no_route(
            NoRouteReason::Sandboxed,
            "cannot tell whether an omp process owns this session: on Linux omp's session lease is an abstract socket, and a sandbox's network namespace can hide it",
        )
        .with_hint(
            "glen will not resume an existing session over RPC from a Codex command on Linux; open the session in omp with glen's extension so glen can use the live route, or run glen from a terminal outside Codex",
        )
        .with_detail("sandbox", json!(kind))
        .with_detail("network", network)
        .with_detail("lease", json!("unknown"));
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
    )
}
fn describe_with_extension(env: &Env, extension: bool) -> (&'static str, String, String) {
    match detect(env) {
        None if env.get("CODEX_SANDBOX_NETWORK_DISABLED") == Some("1") => (
            "info",
            "host network off".into(),
            if cfg!(target_os = "linux") && lease_hidden(env, true) {
                "network disabled by the host; this process is in its own network namespace, so RPC resume of an existing session refuses as sandboxed; the live route and new sessions still work".into()
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
    fn net_namespace_hidden_with_truth_table() {
        let ok = |v: (u64, u64)| -> std::io::Result<(u64, u64)> { Ok(v) };
        let eacces = || -> std::io::Result<(u64, u64)> {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        };
        // Same target via distinct links: readable and equal -> visible.
        assert!(!net_namespace_hidden_with(
            || ok((5, 100)),
            || ok((5, 100)),
            || true
        ));
        // Different targets -> hidden, regardless of loopback_only.
        assert!(net_namespace_hidden_with(
            || ok((5, 100)),
            || ok((5, 101)),
            || false
        ));
        // Reference (PID 1) unreadable + loopback-only (a fresh --unshare-net) -> hidden.
        assert!(net_namespace_hidden_with(|| ok((5, 100)), eacces, || true));
        // Reference unreadable but this namespace has a real interface (eth0 present, the
        // common non-root escalated-command case) -> visible.
        assert!(!net_namespace_hidden_with(
            || ok((5, 100)),
            eacces,
            || false
        ));
        // Our own /proc/self/ns/net unreadable -> fail closed, hidden.
        assert!(net_namespace_hidden_with(eacces, || ok((5, 100)), || false));
    }
    #[test]
    fn net_dev_loopback_only_reads_interface_names() {
        let header = "Inter-|   Receive                            |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n";
        let lo = "    lo:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0\n";
        let eth = "  eth0: 1234567    8901    0    0    0     0          0         0   765432    1098    0    0    0     0       0          0\n";
        // A fresh --unshare-net namespace: loopback only, whatever a host-bound /sys lists.
        assert!(net_dev_loopback_only(&format!("{header}{lo}")));
        // The host namespace (or an escalated command in it): a real interface.
        assert!(!net_dev_loopback_only(&format!("{header}{lo}{eth}")));
        assert!(!net_dev_loopback_only(&format!("{header}{eth}")));
        // Empty or header-only: cannot tell, so not loopback-only.
        assert!(!net_dev_loopback_only(""));
        assert!(!net_dev_loopback_only(header));
    }
    #[test]
    fn lease_hidden_truth_table() {
        // Linux is a pure function of (linux, marker, the namespace comparison). macOS never
        // hides a lease; the comparison itself is exhaustively tested above through the seam.
        for (vars, linux, expected) in [
            (vec![], true, false),
            (vec![("CODEX_SANDBOX", "")], true, false),
            (vec![("CODEX_SANDBOX", "seatbelt")], false, false),
            (vec![("CODEX_SANDBOX_NETWORK_DISABLED", "0")], true, false),
        ] {
            let e = Env::for_test(None, Path::new("/"), &vars);
            assert_eq!(lease_hidden(&e, linux), expected, "{vars:?} linux={linux}");
        }
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
