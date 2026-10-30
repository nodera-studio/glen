//! Versioned control-file seam. Requests are dispatched on the worker's held
//! client; a deadline prevents an abandoned request from applying later.
use crate::{
    error::GlenError,
    jobs::write_json,
    ops::{self, Op, OpError},
};
use serde_json::{Value, json};
use std::{fs, path::Path, path::PathBuf};
#[derive(Clone, Debug)]
pub enum ControlOp {
    Abort,
    Ctl(Op),
}
#[derive(Clone, Debug)]
pub struct Request {
    pub path: PathBuf,
    pub op: Result<ControlOp, OpError>,
    pub deadline_ms: Option<u64>,
    pub expect_session_id: Option<String>,
}
pub fn poll(job_dir: &Path) -> Vec<Request> {
    let mut paths: Vec<_> = fs::read_dir(job_dir.join("control"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|s| s.to_str()).is_some_and(|s| {
                !s.starts_with('.') && s.ends_with(".json") && !s.ends_with(".done.json")
            })
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let value = fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
            let op = value
                .as_ref()
                .ok_or_else(|| OpError::new(ops::BAD_REQUEST, "malformed control request"))
                .and_then(|v| {
                    if v["v"] != 1 {
                        return Err(OpError::new(
                            ops::UNSUPPORTED_VERSION,
                            "unsupported control protocol version",
                        ));
                    }
                    if v.get("deadline_ms")
                        .is_some_and(|d| !d.is_null() && d.as_u64().is_none())
                    {
                        return Err(OpError::new(
                            ops::BAD_REQUEST,
                            "invalid control deadline_ms",
                        ));
                    }
                    let name = v["op"]
                        .as_str()
                        .ok_or_else(|| OpError::new(ops::BAD_REQUEST, "missing control op"))?;
                    let op = ops::decode(name, v.get("args"))?;
                    match op {
                        Op::Abort => Ok(ControlOp::Abort),
                        Op::ListModels
                        | Op::SetModel(_)
                        | Op::SetThinkingLevel(_)
                        | Op::Compact(_) => Ok(ControlOp::Ctl(op)),
                        _ => Err(OpError::new(
                            ops::UNSUPPORTED_OP,
                            format!("unsupported control op: {name}"),
                        )),
                    }
                });
            Request {
                path,
                op,
                deadline_ms: value.as_ref().and_then(|v| v["deadline_ms"].as_u64()),
                expect_session_id: ops::expect_session_id(
                    value.as_ref().and_then(|v| v.get("args")),
                ),
            }
        })
        .collect()
}
pub fn expired(req: &Request) -> bool {
    req.deadline_ms
        .is_some_and(|deadline| jiff::Timestamp::now().as_millisecond().max(0) as u64 >= deadline)
}
pub fn expired_error() -> OpError {
    OpError::new(
        ops::TIMEOUT,
        "the glen command gave up before the job reached this op; nothing changed",
    )
}
pub fn respond(req: &Request, result: Result<Option<Value>, OpError>) -> Result<(), GlenError> {
    let value = match result {
        Ok(None) => json!({"v":1,"ok":true}),
        Ok(Some(data)) => json!({"v":1,"ok":true,"data":data}),
        Err(error) => json!({"v":1,"ok":false,"error":error}),
    };
    write_json(&req.path.with_extension("done.json"), &value)?;
    fs::remove_file(&req.path).map_err(|e| {
        GlenError::new(
            crate::error::ErrorCode::Io,
            format!("cannot remove answered control {}: {e}", req.path.display()),
        )
    })?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn polling_orders_requests_and_skips_results_and_temps() {
        let t = tempfile::tempdir().unwrap();
        assert!(poll(t.path()).is_empty());
        assert!(!t.path().join("control").exists());
        let dir = t.path().join("control");
        fs::create_dir(&dir).unwrap();
        for (name, content) in [
            ("b.json", r#"{"v":1,"op":"abort"}"#),
            ("a.json", r#"{"v":1,"op":"set_model"}"#),
            ("c.json", "bad"),
            ("d.json", r#"{"v":2,"op":"abort"}"#),
            (".hidden.json", "{}"),
            ("x.done.json", "{}"),
            ("partial.tmp", "{}"),
        ] {
            fs::write(dir.join(name), content).unwrap();
        }
        let reqs = poll(t.path());
        assert_eq!(reqs.len(), 4);
        assert_eq!(reqs[0].path.file_name().unwrap(), "a.json");
        assert_eq!(reqs[0].op.as_ref().unwrap_err().code, ops::BAD_REQUEST);
        assert!(matches!(reqs[1].op, Ok(ControlOp::Abort)));
        assert_eq!(reqs[2].op.as_ref().unwrap_err().code, ops::BAD_REQUEST);
        assert_eq!(
            reqs[3].op.as_ref().unwrap_err().code,
            ops::UNSUPPORTED_VERSION
        );
    }
    #[test]
    fn controls_preserve_deadline_and_identity_and_reject_non_controls() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join("control")).unwrap();
        for (op, args, error) in [
            ("list_models", json!({}), None),
            ("set_model", json!({"provider":"p","model_id":"m"}), None),
            ("set_thinking_level", json!({"level":"high"}), None),
            ("compact", json!({}), None),
            ("hello", json!({}), Some(ops::UNSUPPORTED_OP)),
            ("state", json!({}), Some(ops::UNSUPPORTED_OP)),
            (
                "send",
                json!({"text":"hi","deliver_as":"auto"}),
                Some(ops::UNSUPPORTED_OP),
            ),
        ] {
            let mut args = args;
            args["expect_session_id"] = json!("s");
            fs::write(
                t.path().join("control/a.json"),
                json!({"v":1,"op":op,"args":args,"deadline_ms":1234}).to_string(),
            )
            .unwrap();
            let req = poll(t.path()).pop().unwrap();
            assert_eq!(req.deadline_ms, Some(1234));
            assert_eq!(req.expect_session_id.as_deref(), Some("s"));
            assert_eq!(req.op.as_ref().err().map(|e| e.code.as_str()), error);
        }
    }
    #[test]
    fn responses_are_atomic_private_and_remove_requests() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join("control")).unwrap();
        for (name, result, expected) in [
            ("ok", Ok(None), json!({"v":1,"ok":true})),
            (
                "data",
                Ok(Some(json!({"job_status":"aborted"}))),
                json!({"v":1,"ok":true,"data":{"job_status":"aborted"}}),
            ),
            (
                "bad",
                Err(OpError::new(ops::INVALID_LEVEL, "unknown")
                    .with_details(json!({"available":["off"]}))),
                json!({"v":1,"ok":false,"error":{"code":"invalid_level","message":"unknown","details":{"available":["off"]}}}),
            ),
        ] {
            let path = t.path().join(format!("control/{name}.json"));
            fs::write(&path, "{}").unwrap();
            let req = Request {
                path: path.clone(),
                op: Ok(ControlOp::Abort),
                deadline_ms: None,
                expect_session_id: None,
            };
            respond(&req, result).unwrap();
            assert!(!path.exists());
            let done = path.with_extension("done.json");
            let v: Value = serde_json::from_slice(&fs::read(&done).unwrap()).unwrap();
            assert_eq!(v, expected);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(done).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        assert!(poll(t.path()).is_empty());
        assert_eq!(fs::read_dir(t.path().join("control")).unwrap().count(), 3);
    }
    #[test]
    fn failed_response_preserves_request() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.json");
        fs::write(&p, "{}").unwrap();
        fs::create_dir(p.with_extension("done.json")).unwrap();
        let r = Request {
            path: p.clone(),
            op: Ok(ControlOp::Abort),
            deadline_ms: None,
            expect_session_id: None,
        };
        assert!(respond(&r, Ok(None)).is_err());
        assert!(p.exists());
    }
}
