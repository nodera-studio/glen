//! W3b seam: dispatch future set_model {provider, model_id}, set_thinking_level
//! {level}, steer {text}, follow_up {text} operations on the worker's held Client.
//! W2 implements only abort. No control operation starts another omp process.
use crate::{error::GlenError, jobs::write_json};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
pub enum ControlOp {
    Abort,
}
pub struct Request {
    pub path: PathBuf,
    pub op: Result<ControlOp, String>,
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
            let op = match value {
                Some(v) if v["v"] == 1 && v["op"] == "abort" => Ok(ControlOp::Abort),
                Some(v) => Err(v["op"].as_str().unwrap_or("malformed").into()),
                None => Err("malformed".into()),
            };
            Request { path, op }
        })
        .collect()
}
pub fn respond(req: &Request, result: Result<(), (&str, String)>) -> Result<(), GlenError> {
    let value = match result {
        Ok(()) => json!({"v":1,"ok":true}),
        Err((code, message)) => json!({"v":1,"ok":false,"error":{"code":code,"message":message}}),
    };
    let done = req.path.with_extension("done.json");
    write_json(&done, &value)?;
    fs::remove_file(&req.path)?;
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
        assert_eq!(reqs[0].op.as_ref().err().unwrap(), "set_model");
        assert!(matches!(reqs[1].op, Ok(ControlOp::Abort)));
        assert_eq!(reqs[2].op.as_ref().err().unwrap(), "malformed");
        assert!(reqs[3].op.is_err());
    }
    #[test]
    fn responses_are_atomic_private_and_remove_requests() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("control");
        fs::create_dir(&dir).unwrap();
        for (name, result, expected) in [
            ("ok", Ok(()), json!({"v":1,"ok":true})),
            (
                "bad",
                Err(("unsupported_op", "unknown".into())),
                json!({"v":1,"ok":false,"error":{"code":"unsupported_op","message":"unknown"}}),
            ),
        ] {
            let path = dir.join(format!("{name}.json"));
            fs::write(&path, "{}").unwrap();
            let req = Request {
                path: path.clone(),
                op: Ok(ControlOp::Abort),
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
        assert_eq!(fs::read_dir(dir).unwrap().count(), 2);
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
        };
        assert!(respond(&r, Ok(())).is_err());
        assert!(p.exists());
    }
}
