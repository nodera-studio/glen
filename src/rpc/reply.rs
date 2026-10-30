use crate::{
    error::{ErrorCode, GlenError},
    glen_home::GlenHome,
    jobs::{JobId, JobStore, Reply, append_line},
};
use serde_json::{Map, Value, json};
use std::sync::{Arc, Mutex};
pub struct ReplySink {
    store: JobStore,
    home: GlenHome,
    job_id: JobId,
    session_id: Mutex<Option<String>>,
    replies: Mutex<Vec<Reply>>,
}
impl ReplySink {
    pub fn new(store: JobStore, home: GlenHome, job_id: JobId) -> Self {
        Self {
            store,
            home,
            job_id,
            session_id: Mutex::new(None),
            replies: Mutex::new(vec![]),
        }
    }
    pub fn set_session(&self, id: String) {
        *self
            .session_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id);
    }
    pub fn replies(&self) -> Vec<Reply> {
        self.replies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    pub fn record(
        &self,
        text: String,
        is_final: bool,
        tool_call_id: String,
    ) -> Result<(), GlenError> {
        // Serialise disk and memory records together, preserving concurrent host-call order.
        let mut replies = self
            .replies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session_id = self
            .session_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| GlenError::new(ErrorCode::Internal, "reply before session baseline"))?;
        let reply = Reply {
            v: 1,
            id: uuid::Uuid::now_v7().to_string(),
            session_id: session_id.clone(),
            job_id: Some(self.job_id.as_str().into()),
            request_id: None,
            tool_call_id: Some(tool_call_id),
            text,
            is_final,
            at: jiff::Timestamp::now().to_string(),
        };
        self.store.append_reply(&self.job_id, &reply)?;
        let inbox = (|| -> Result<(), GlenError> {
            let dir = self.home.ensure_dir("inbox").map_err(|e| {
                GlenError::new(
                    ErrorCode::Io,
                    format!(
                        "cannot create {}: {e}",
                        self.home.root.join("inbox").display()
                    ),
                )
            })?;
            let key = crate::jobs::lock::lock_key(&session_id);
            append_line(
                &dir.join(format!("{key}.ndjson")),
                &serde_json::to_vec(&reply)
                    .map_err(|e| GlenError::new(ErrorCode::Internal, e.to_string()))?,
            )?;
            Ok(())
        })();
        if let Err(e) = inbox {
            eprintln!("cannot write glen inbox: {e}");
        }
        replies.push(reply);
        Ok(())
    }
}
pub(crate) fn parse_reply_args(args: &Map<String, Value>) -> Result<(String, bool), GlenError> {
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= 65536)
        .ok_or_else(|| {
            GlenError::new(
                ErrorCode::Usage,
                "glen_reply: text must be a non-empty string of at most 65536 bytes",
            )
        })?;
    let is_final = match args.get("final") {
        None => false,
        Some(Value::Bool(b)) => *b,
        _ => {
            return Err(GlenError::new(
                ErrorCode::Usage,
                "glen_reply: final must be a boolean",
            ));
        }
    };
    Ok((text.into(), is_final))
}
pub fn glen_reply_tool(sink: Arc<ReplySink>) -> omp_rpc::HostTool {
    let schema = Map::from_iter([
        ("type".into(), json!("object")),
        (
            "properties".into(),
            json!({"text":{"type":"string","description":"The message for the calling agent."},"final":{"type":"boolean","description":"true when this is your final answer to the request."}}),
        ),
        ("required".into(), json!(["text"])),
        ("additionalProperties".into(), json!(false)),
    ]);
    omp_rpc::HostTool::new(
        "glen_reply",
        "Send a message back to the agent that contacted you through glen (Claude Code or Codex). Use it to answer a glen request or report progress.",
        schema,
        move |args, ctx| {
            if ctx.is_cancelled() {
                return Err(GlenError::new(ErrorCode::JobFailed, "cancelled").into());
            }
            let (text, is_final) = parse_reply_args(&args)?;
            sink.record(text, is_final, ctx.tool_call_id().into())?;
            Ok("delivered".into())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_text_bytes_and_final_type() {
        for bad in [
            json!({}),
            json!({"text":""}),
            json!({"text":" \n\t"}),
            json!({"text":1}),
            json!({"text":null}),
            json!({"text":"x".repeat(65537)}),
            json!({"text":"é".repeat(32769)}),
            json!({"text":"ok","final":"true"}),
            json!({"text":"ok","final":null}),
        ] {
            assert!(parse_reply_args(bad.as_object().unwrap()).is_err(), "{bad}");
        }
        for final_value in [None, Some(false), Some(true)] {
            let mut args = json!({"text":" text "});
            if let Some(v) = final_value {
                args["final"] = json!(v);
            }
            assert_eq!(
                parse_reply_args(args.as_object().unwrap()).unwrap(),
                (" text ".into(), final_value.unwrap_or(false))
            );
        }
        assert_eq!(
            parse_reply_args(json!({"text":"é".repeat(32768)}).as_object().unwrap())
                .unwrap()
                .0
                .len(),
            65536
        );
    }
    #[test]
    fn records_durably_in_order_and_private_modes() {
        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        let j = crate::jobs::tests::job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        let sink = ReplySink::new(store.clone(), store.home.clone(), id.clone());
        assert!(sink.record("early".into(), false, "early".into()).is_err());
        let session = j.session_id.unwrap();
        sink.set_session(session.clone());
        sink.record("progress".into(), false, "one".into()).unwrap();
        sink.record("done".into(), true, "two".into()).unwrap();
        let records = store.read_replies(&id);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].text, "progress");
        assert_eq!(records[1].text, "done");
        assert!(records[1].is_final);
        assert_eq!(records[1].tool_call_id.as_deref(), Some("two"));
        assert_eq!(
            serde_json::to_value(&records).unwrap(),
            serde_json::to_value(sink.replies()).unwrap()
        );
        let a = store.dir(&id).join("replies.ndjson");
        let b = store
            .home
            .root
            .join("inbox")
            .join(format!("{session}.ndjson"));
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&a, &b] {
                assert_eq!(
                    std::fs::metadata(p).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
            assert_eq!(
                std::fs::metadata(b.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        let tool = glen_reply_tool(Arc::new(sink));
        let d = tool.definition();
        assert_eq!(d.name, "glen_reply");
        assert_eq!(d.parameters["required"], json!(["text"]));
        assert_eq!(d.parameters["additionalProperties"], false);
    }
    #[test]
    fn failed_job_write_records_nothing_in_memory() {
        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        let sink = ReplySink::new(store.clone(), store.home.clone(), JobId::new());
        sink.set_session("session".into());
        assert!(sink.record("reply".into(), false, "call".into()).is_err());
        assert!(sink.replies().is_empty());
        assert!(!store.home.root.exists());
    }
    #[test]
    fn failed_inbox_write_keeps_job_reply() {
        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        let j = crate::jobs::tests::job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        std::fs::write(store.home.root.join("inbox"), "blocked").unwrap();
        let sink = ReplySink::new(store.clone(), store.home.clone(), id.clone());
        sink.set_session("session".into());
        sink.record("reply".into(), true, "call".into()).unwrap();
        assert_eq!(sink.replies().len(), 1);
        assert_eq!(store.read_replies(&id).len(), 1);
    }
}
