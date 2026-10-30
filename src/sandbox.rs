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
    let kind = env.non_empty("CODEX_SANDBOX").map(str::to_owned);
    let network = env.get("CODEX_SANDBOX_NETWORK_DISABLED") != Some("1");
    (kind.is_some() || !network).then_some(Sandbox { kind, network })
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
pub fn map_home_write(env: &Env, command: &str, err: io::Error) -> GlenError {
    if err.kind() == io::ErrorKind::PermissionDenied
        && let Some(s) = detect(env)
    {
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
pub fn describe(env: &Env) -> (&'static str, String, String) {
    match detect(env) {
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
            (vec![("CODEX_SANDBOX_NETWORK_DISABLED", "1")], true, false),
        ] {
            let e = Env::for_test(Some(Path::new("/h")), Path::new("/w"), &vars);
            assert_eq!(detect(&e).is_some(), exists);
            assert_eq!(refuse_unreachable(&e, "send").is_err(), !network);
            assert_eq!(refuse_rpc(&e, "send").is_err(), exists);
            let error = map_home_write(&e, "send", io::ErrorKind::PermissionDenied.into());
            assert_eq!(
                error.code,
                if exists {
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
