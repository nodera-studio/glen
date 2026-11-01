//! Session controls, shared across live sockets, worker inboxes and short-lived RPC.
pub mod ephemeral;
pub mod exec;
pub mod inbox;
pub mod probe;
pub mod route;
use crate::{
    error::{ErrorCode, GlenError},
    ops::{self, ModelRow, OpError},
};
use serde_json::json;

pub fn levels_of(reasoning: bool, efforts: &[String]) -> Vec<String> {
    std::iter::once("off".into())
        .chain(efforts.iter().filter(|_| reasoning).cloned())
        .collect()
}
pub fn row_from_info(model: &omp_rpc::ModelInfo) -> ModelRow {
    let efforts: Vec<_> = model
        .thinking
        .as_ref()
        .map(|t| t.efforts.iter().map(|e| e.as_str().to_owned()).collect())
        .unwrap_or_default();
    ModelRow {
        provider: model.provider.clone(),
        id: model.id.clone(),
        name: model.name.clone(),
        reasoning: model.reasoning,
        thinking_levels: levels_of(model.reasoning, &efforts),
        context_window: model.context_window,
        max_tokens: model.max_tokens,
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum Resolved {
    One(ModelRow),
    None,
    Many(Vec<ModelRow>),
}
pub fn resolve_model(rows: &[ModelRow], spec: &str) -> Resolved {
    if let Some(row) = rows
        .iter()
        .find(|m| format!("{}/{}", m.provider, m.id) == spec)
    {
        return Resolved::One(row.clone());
    }
    let candidates: Vec<_> = rows
        .iter()
        .filter(|m| {
            m.id.eq_ignore_ascii_case(spec)
                || format!("{}/{}", m.provider, m.id).eq_ignore_ascii_case(spec)
        })
        .cloned()
        .collect();
    match candidates.as_slice() {
        [] => Resolved::None,
        [one] => Resolved::One(one.clone()),
        _ => Resolved::Many(candidates),
    }
}
#[derive(Default)]
pub struct ErrCtx<'a> {
    pub handle: &'a str,
    pub session_id: &'a str,
    pub route: &'a str,
    pub job: Option<&'a str>,
    pub ext_version: Option<&'a str>,
}
pub fn to_glen_error(e: OpError, ctx: &ErrCtx<'_>) -> GlenError {
    let (code, reason) = match e.code.as_str() {
        ops::MODEL_NOT_FOUND => (ErrorCode::NotFound, ops::MODEL_NOT_FOUND),
        ops::INVALID_LEVEL => (ErrorCode::Usage, ops::INVALID_LEVEL),
        ops::UNSUPPORTED_OP => (ErrorCode::NoRoute, "extension_outdated"),
        ops::SESSION_MISMATCH => (ErrorCode::ControlFailed, "session_moved"),
        ops::NO_AUTH => (ErrorCode::ControlFailed, ops::NO_AUTH),
        ops::BUSY => (ErrorCode::ControlFailed, ops::BUSY),
        ops::OMP_ERROR => (ErrorCode::ControlFailed, ops::OMP_ERROR),
        ops::TIMEOUT => (ErrorCode::ControlFailed, ops::TIMEOUT),
        ops::JOB_ENDED => (ErrorCode::ControlFailed, ops::JOB_ENDED),
        ops::OUTCOME_UNKNOWN => (ErrorCode::ControlFailed, ops::OUTCOME_UNKNOWN),
        _ => (ErrorCode::ControlFailed, "protocol"),
    };
    let mut details = e
        .details
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| json!({}));
    details["reason"] = json!(reason);
    if reason == "extension_outdated" {
        details["ext_version"] = json!(ctx.ext_version);
    }
    if reason == "session_moved" {
        details["from"] = json!(ctx.session_id);
        details["to"] = details["session_id"].clone();
    }
    let hint = match reason {
        ops::MODEL_NOT_FOUND => Some(format!("run `glen models {}`", ctx.handle)),
        "extension_outdated" => {
            Some("run `glen install-extension`, then restart that omp session".into())
        }
        ops::NO_AUTH => Some("log in to that provider in omp, then retry".into()),
        ops::BUSY => Some(format!(
            "abort the run first (`glen abort {}`), or wait for it to finish",
            ctx.handle
        )),
        "session_moved" => Some("run `glen ls` and retry on the new session handle".into()),
        ops::TIMEOUT if ctx.route == "live" => {
            Some("the op may still finish in that omp session; check it before retrying".into())
        }
        ops::TIMEOUT if ctx.route == "job" => {
            details["outcome"] = json!("unknown");
            Some(format!(
                "check job {} with `glen wait {}`; the control outcome is unknown",
                ctx.job.unwrap_or("j…"),
                ctx.job.unwrap_or("j…")
            ))
        }
        ops::OUTCOME_UNKNOWN => Some(format!(
            "the change may have been applied; check `glen models {}`",
            ctx.handle
        )),
        ops::JOB_ENDED => Some("the glen job ended; retry on the session".into()),
        "protocol" => Some("check the omp and glen versions, then retry".into()),
        _ => None,
    };
    let mut error = GlenError::new(
        code,
        format!("cannot control session {}: {}", ctx.handle, e.message),
    )
    .with_details(details);
    error.hint = hint;
    error
}
#[cfg(test)]
mod tests {
    use super::*;
    fn models() -> Vec<omp_rpc::ModelInfo> {
        serde_json::from_str(include_str!("../../tests/support/fake_models.json")).unwrap()
    }
    #[test]
    fn c2_1_unknown_outcome_maps_to_control_failed_with_check_hint() {
        let error = to_glen_error(
            OpError::new(ops::OUTCOME_UNKNOWN, "disconnected"),
            &ErrCtx {
                handle: "abc",
                route: "job",
                ..ErrCtx::default()
            },
        );
        assert_eq!(error.code, ErrorCode::ControlFailed);
        assert_eq!(error.details.unwrap()["reason"], "outcome_unknown");
        assert_eq!(
            error.hint.as_deref(),
            Some("the change may have been applied; check `glen models abc`")
        );
    }
    #[test]
    fn resolution_exact_pairs_before_case_insensitive_union() {
        let rows: Vec<_> = models().iter().map(row_from_info).collect();
        for (spec, pair) in [
            ("fake/fake-1", "fake/fake-1"),
            ("FAKE/FAKE-1", "fake/fake-1"),
            ("vendor/m-3", "vendor/m-3"),
            ("router/vendor/m-3", "router/vendor/m-3"),
        ] {
            let Resolved::One(row) = resolve_model(&rows, spec) else {
                panic!("{spec}")
            };
            assert_eq!(format!("{}/{}", row.provider, row.id), pair);
        }
        // Both vendor/m-3 and router's vendor/m-3 share the last path component,
        // but bare ids are matched in full, never by their last component.
        assert!(
            matches!(resolve_model(&rows,"m-3"), Resolved::One(row) if row.provider == "vendor")
        );
        let Resolved::Many(rows) = resolve_model(&rows, "fake-2") else {
            panic!("ambiguous bare id")
        };
        assert_eq!(
            rows.iter().map(|m| m.provider.as_str()).collect::<Vec<_>>(),
            ["fake", "other"]
        );
        assert_eq!(resolve_model(&[], "nope"), Resolved::None);
    }
    #[test]
    fn model_rows_and_levels() {
        let models = models();
        assert_eq!(
            row_from_info(&models[0]).thinking_levels,
            ["off", "low", "medium", "high"]
        );
        assert_eq!(row_from_info(&models[1]).thinking_levels, ["off"]);
        assert_eq!(levels_of(false, &["high".into()]), ["off"]);
        let mut unknown_limits = models[0].clone();
        unknown_limits.context_window = None;
        unknown_limits.max_tokens = None;
        let row = serde_json::to_value(row_from_info(&unknown_limits)).unwrap();
        assert!(row["context_window"].is_null());
        assert!(row["max_tokens"].is_null());
    }
    #[test]
    fn error_mapping_preserves_details_and_route_hints() {
        for (wire, code, reason) in [
            (
                ops::MODEL_NOT_FOUND,
                ErrorCode::NotFound,
                ops::MODEL_NOT_FOUND,
            ),
            (ops::INVALID_LEVEL, ErrorCode::Usage, ops::INVALID_LEVEL),
            (
                ops::UNSUPPORTED_OP,
                ErrorCode::NoRoute,
                "extension_outdated",
            ),
            (
                ops::SESSION_MISMATCH,
                ErrorCode::ControlFailed,
                "session_moved",
            ),
            (ops::NO_AUTH, ErrorCode::ControlFailed, ops::NO_AUTH),
            (ops::BUSY, ErrorCode::ControlFailed, ops::BUSY),
            (ops::TIMEOUT, ErrorCode::ControlFailed, ops::TIMEOUT),
            (ops::OMP_ERROR, ErrorCode::ControlFailed, ops::OMP_ERROR),
            (ops::JOB_ENDED, ErrorCode::ControlFailed, ops::JOB_ENDED),
            (ops::INTERNAL, ErrorCode::ControlFailed, "protocol"),
            (ops::BAD_REQUEST, ErrorCode::ControlFailed, "protocol"),
            (
                ops::UNSUPPORTED_VERSION,
                ErrorCode::ControlFailed,
                "protocol",
            ),
        ] {
            let err = to_glen_error(
                OpError::new(wire, "message")
                    .with_details(json!({"available":["off"],"session_id":"new"})),
                &ErrCtx {
                    handle: "abc",
                    session_id: "old",
                    route: "job",
                    job: Some("j1234567"),
                    ext_version: Some("0.1.0"),
                },
            );
            assert_eq!(err.code, code);
            let details = err.details.unwrap();
            assert_eq!(details["reason"], reason);
            assert_eq!(details["available"], json!(["off"]));
            if wire == ops::TIMEOUT {
                assert_eq!(details["outcome"], "unknown");
                assert!(err.hint.unwrap().contains("j1234567"));
            }
            if wire == ops::SESSION_MISMATCH {
                assert_eq!(details["from"], "old");
                assert_eq!(details["to"], "new");
            }
        }
    }
}
