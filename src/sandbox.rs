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
    GlenError::new(ErrorCode::NoRoute, format!("`glen {command}` cannot reach omp from inside Codex's sandbox"))
        .with_hint("run this glen command outside the sandbox (request escalated permissions and approve the `glen` prefix), or enable network for the sandbox and use a session open in omp with glen's extension")
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
        let mut error = refusal(command, &s).with_hint(format!("the sandbox cannot write GLEN_HOME ({path}); set GLEN_HOME under $TMPDIR, add it to `sandbox_workspace_write.writable_roots`, or run glen outside the sandbox"));
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
/// Lease backends currently retain OS errors as diagnostic text.
pub fn map_probe_failure(env: &Env, command: &str, reason: &str) -> Option<GlenError> {
    #[cfg(unix)]
    for errno in [libc::EPERM, libc::EACCES, libc::EROFS] {
        let error = io::Error::from_raw_os_error(errno);
        if reason == error.to_string() && restricted(env, &error) {
            let s = detect(env).unwrap_or(Sandbox {
                kind: None,
                network: false,
            });
            let mut error = refusal(command, &s);
            error.message = format!("cannot probe omp session lease for {command}: {reason}");
            return Some(error);
        }
    }
    #[cfg(not(unix))]
    let _ = (env, command, reason);
    None
}
pub fn describe(env: &Env) -> (&'static str, String, String) {
    match detect(env) {
        None if env.get("CODEX_SANDBOX_NETWORK_DISABLED") == Some("1") => ("info", "host network off".into(), "network disabled by the host; escalated commands still work".into()),
        None => ("info", "none".into(), String::new()),
        Some(s) if !s.network => ("warn",format!("codex {}, network off",s.kind.as_deref().unwrap_or("unknown")), "send and session controls cannot reach omp from here; run them outside the sandbox".into()),
        Some(s) => ("warn",format!("codex {}",s.kind.as_deref().unwrap_or("unknown")), "send works only into sessions open in omp with glen's extension; GLEN_HOME must be writable".into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
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
                    &io::Error::from_raw_os_error(errno).to_string(),
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
