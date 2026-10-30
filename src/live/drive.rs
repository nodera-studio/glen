//! Live jobs observe the TUI's turn. They neither acquire a session lock nor
//! interrupt the turn when observation is cancelled or reaches its deadline.
use super::client::{self, ClientError, ConnectError, Frame};
use crate::{
    error::Warning,
    jobs::{Forked, JobError, JobResult, Reply, State},
    ops::SendArgs,
    rpc::{
        self, control,
        worker::{Failure, Worker},
    },
};
use serde_json::json;
use std::{
    sync::{atomic::Ordering, mpsc::RecvTimeoutError},
    time::Instant,
};
#[derive(Default)]
struct Observation {
    acked: bool,
    consumed: bool,
    replies: Vec<Reply>,
    settled: bool,
    prompt_status: Option<String>,
    assistant: Option<String>,
    stop: Option<String>,
}
fn protocol(message: impl Into<String>) -> Failure {
    Failure::new("protocol", message)
}
fn response_error(w: &Worker, error: client::WireError) -> Failure {
    let mut f = match error.code.as_str() {
        "session_mismatch" => {
            let mut f = Failure::new(
                "session_moved",
                "the session moved before glen could deliver the message",
            );
            f.details = Some(
                json!({"from":w.job.session_id,"to":error.details.as_ref().and_then(|d|d.get("session_id"))}),
            );
            f.hint = Some("run `glen ls` and send to the new handle".into());
            f
        }
        "send_failed" => Failure::new("omp_error", error.message),
        _ => protocol(error.message),
    };
    if f.code == "protocol" {
        f.hint = Some(super::route::hint(super::route::Unhealthy::Incompatible));
    }
    f
}
fn optional_text(body: &serde_json::Value, key: &str) -> Result<Option<String>, Failure> {
    match body.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        _ => Err(protocol(format!("invalid {key} in settled event"))),
    }
}
fn on_frame(
    w: &mut Worker,
    s: &mut Observation,
    frame: Frame,
) -> Result<Option<Result<(), Failure>>, Failure> {
    let id = match &frame {
        Frame::Response { id, .. } | Frame::Event { id, .. } => id,
    };
    if id.as_ref() != w.status.request_id.as_ref() {
        return Ok(Some(Err(protocol("frame request id does not match"))));
    }
    match frame {
        Frame::Response {
            ok, data, error, ..
        } => {
            if s.acked {
                return Ok(Some(Err(protocol("duplicate response"))));
            }
            if !ok {
                return Ok(Some(Err(response_error(
                    w,
                    error.ok_or_else(|| protocol("missing response error"))?,
                ))));
            }
            let delivered = data["delivered_as"]
                .as_str()
                .filter(|d| ["prompt", "steer", "follow_up"].contains(d))
                .ok_or_else(|| protocol("invalid send acknowledgement"))?;
            s.acked = true;
            w.status.status = State::Accepted;
            w.status.delivered_as = Some(delivered.into());
            w.save()?;
        }
        Frame::Event { kind, body, .. } => {
            if !s.acked {
                return Ok(Some(Err(protocol("event before acknowledgement"))));
            }
            match kind.as_str() {
                "consumed" => s.consumed = true,
                "reply" => match serde_json::from_value::<Reply>(body["reply"].clone()) {
                    Ok(r) => {
                        if let Err(e) = w.store.append_reply(&w.id, &r) {
                            w.log("reply", &e.to_string());
                        }
                        s.replies.push(r);
                    }
                    Err(e) => w.log("reply", &format!("skipping malformed reply: {e}")),
                },
                "session_changed" => {
                    let to = body["to"]
                        .as_str()
                        .ok_or_else(|| protocol("session_changed has no to id"))?;
                    w.status.forked = Some(Forked {
                        from: w.job.session_id.clone().unwrap_or_default(),
                        to: to.into(),
                    });
                    w.status.actual_session_id = Some(to.into());
                    w.save()?;
                }
                "settled" => {
                    let status = body["status"]
                        .as_str()
                        .ok_or_else(|| protocol("settled has no status"))?;
                    if !["completed", "aborted", "error"].contains(&status) {
                        return Ok(Some(Err(protocol("invalid settled status"))));
                    }
                    if let Some(warnings) =
                        body.get("warnings").and_then(serde_json::Value::as_array)
                    {
                        for warning in warnings {
                            match serde_json::from_value::<Warning>(warning.clone()) {
                                Ok(warning) => w.status.warnings.push(warning),
                                Err(e) => w.log(
                                    "warning",
                                    &format!("skipping malformed settled warning: {e}"),
                                ),
                            }
                        }
                    }
                    let assistant = optional_text(&body, "assistant_text")?;
                    let stop = optional_text(&body, "stop_reason")?;
                    let error_message = optional_text(&body, "error_message")?;
                    s.settled = true;
                    s.prompt_status = Some(status.into());
                    s.assistant = assistant;
                    s.stop = stop;
                    return Ok(Some(match status {
                        "completed" => Ok(()),
                        "aborted" => {
                            let mut f = Failure::new("aborted", "the omp turn was aborted");
                            f.state = State::Aborted;
                            Err(f)
                        }
                        _ => Err(Failure::new(
                            "omp_error",
                            error_message.as_deref().unwrap_or("the omp turn failed"),
                        )),
                    }));
                }
                "error" => {
                    let code = body["error"]["code"].as_str().unwrap_or("internal");
                    return Ok(Some(Err(match code {
                        "session_switched" => {
                            let mut f = Failure::new(
                                "session_switched",
                                "the omp session switched to another conversation before the request settled",
                            );
                            f.state = State::Aborted;
                            f
                        }
                        "not_delivered" => Failure::new(
                            "not_delivered",
                            "omp did not append the live request's user message; delivery failed or the queued message was removed",
                        ),
                        "shutdown" => Failure::new(
                            "omp_exited",
                            "the live omp session closed before the request settled",
                        ),
                        _ => protocol(
                            body["error"]["message"]
                                .as_str()
                                .unwrap_or("live endpoint error"),
                        ),
                    })));
                }
                _ => w.log("event", &format!("ignoring {kind}")),
            }
        }
    }
    Ok(None)
}
fn controls(w: &Worker) {
    for req in control::poll(&w.store.dir(&w.id)) {
        let result = match &req.op {
            Ok(control::ControlOp::Abort) => Err(crate::ops::OpError::new(
                crate::ops::UNSUPPORTED_OP,
                "stop the run with `glen abort <session>`",
            )),
            Ok(control::ControlOp::Ctl(_)) => Err(crate::ops::OpError::new(
                crate::ops::UNSUPPORTED_OP,
                "live sessions take controls on their socket; run `glen <command> <session>`",
            )),
            Err(error) => Err(error.clone()),
        };
        if let Err(e) = control::respond(&req, result) {
            w.log("control", &e.to_string());
        }
    }
}
fn on_tick(w: &mut Worker, s: &Observation, started: Instant) -> Option<Failure> {
    controls(w);
    if w.cancel.load(Ordering::SeqCst) {
        w.status.warnings.push(Warning {
            code: "live_turn_continues".into(),
            message: "the live omp turn keeps running; replies still reach `glen inbox`".into(),
        });
        return Some(Failure::cancelled());
    }
    if Instant::now() >= w.deadline {
        let mut f = Failure::new(
            "timeout",
            if s.acked {
                "the live session did not settle before the job deadline"
            } else {
                "no acknowledgement from the live session before the job deadline"
            },
        );
        if s.acked {
            f.hint=Some(if s.consumed{"the message was delivered and the omp session keeps running; replies still reach `glen inbox`"}else{"the message is still queued in the omp session; replies still reach `glen inbox`"}.into());
        }
        return Some(f);
    }
    if !s.acked && started.elapsed() >= super::ACK {
        w.status.warnings.push(Warning {
            code: "delivery_unknown".into(),
            message: "the endpoint did not acknowledge; delivery is unknown".into(),
        });
        return Some(protocol("the live session did not acknowledge within 10 s"));
    }
    None
}
pub(crate) fn run(w: &mut Worker) -> Result<(), Failure> {
    let socket = w
        .job
        .live_socket
        .as_deref()
        .ok_or_else(|| Failure::new("internal", "live job has no socket"))?
        .to_owned();
    w.status.request_id = Some(uuid::Uuid::now_v7().to_string());
    w.save()?;
    // Live controls cannot stop the TUI; answer them without W2's abort behaviour.
    controls(w);
    if w.cancel.load(Ordering::SeqCst) {
        return Err(Failure::cancelled());
    }
    if Instant::now() >= w.deadline {
        return Err(Failure::new("timeout", "the glen job deadline passed"));
    }
    let args = SendArgs {
        text: w.job.text.clone(),
        deliver_as: w.job.deliver_as,
        job_id: Some(w.job.id.clone()),
        expect_session_id: w.job.session_id.clone(),
    };
    let conn = client::open_send(
        &socket,
        w.status.request_id.as_deref().unwrap_or_default(),
        args,
        &|| {
            controls(w);
            w.cancel.load(Ordering::SeqCst) || Instant::now() >= w.deadline
        },
    )
    .map_err(|e| match e {
        ClientError::Cancelled if w.cancel.load(Ordering::SeqCst) => Failure::cancelled(),
        ClientError::Cancelled => {
            Failure::new("timeout", "the glen job deadline passed before delivery")
        }
        ClientError::Connect(ConnectError::Missing | ConnectError::Refused) => {
            let mut f = Failure::new(
                "no_route",
                format!(
                    "the live endpoint of session {} went away before glen delivered the message",
                    w.job.session_handle.as_deref().unwrap_or("?")
                ),
            );
            f.hint = Some("send again".into());
            f
        }
        ClientError::Connect(ConnectError::PathTooLong) => {
            let mut f = Failure::new("no_route", "live socket path too long");
            f.hint = Some(super::route::hint(super::route::Unhealthy::PathTooLong));
            f
        }
        ClientError::Connect(ConnectError::Io(e)) => Failure::new(
            "io",
            format!("cannot connect to live endpoint {}: {e}", socket.display()),
        ),
        other => protocol(format!("cannot send to {}: {other}", socket.display())),
    })?;
    let started = Instant::now();
    let mut s = Observation::default();
    let result = loop {
        if let Some(f) = on_tick(w, &s, started) {
            break Err(f);
        }
        match conn.rx.recv_timeout(rpc::TICK) {
            Ok(Ok(frame)) => match on_frame(w, &mut s, frame) {
                Ok(Some(result)) => break result,
                Ok(None) => (),
                Err(e) => break Err(e),
            },
            Ok(Err(e)) => {
                break Err(if e == "eof" {
                    Failure::new(
                        "omp_exited",
                        "the live omp session closed before the request settled",
                    )
                } else {
                    protocol(e)
                });
            }
            Err(RecvTimeoutError::Timeout) => (),
            Err(_) => break Err(Failure::new("omp_exited", "live endpoint reader stopped")),
        }
    };
    conn.close();
    w.store.write_result(
        &w.id,
        &JobResult {
            v: 1,
            prompt_status: s.prompt_status,
            agent_invoked: true,
            assistant_text: s.assistant,
            replies: s.replies,
            command_output: vec![],
            stop_reason: s.stop,
            session_settled: s.settled,
            settled_at: s.settled.then(|| jiff::Timestamp::now().to_string()),
        },
    )?;
    controls(w);
    match result {
        Ok(()) => {
            w.status.status = State::Completed;
            w.status.error = None;
            w.save()?;
        }
        Err(f) => {
            w.status.status = f.state;
            w.status.error = Some(JobError {
                code: f.code.into(),
                message: f.error_message,
                hint: f.hint,
                details: f.details,
            });
            w.save()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobId, JobStore, Status};
    use std::sync::{Arc, atomic::AtomicBool};
    fn worker() -> (tempfile::TempDir, Worker) {
        let t = tempfile::tempdir().unwrap();
        let store = JobStore::new(crate::glen_home::GlenHome {
            root: t.path().join("g"),
        });
        let mut job = crate::jobs::tests::job();
        job.route = crate::jobs::Route::Live;
        job.omp_bin = None;
        job.live_socket = Some("/tmp/test.sock".into());
        job.live_pid = Some(1);
        store.create(&job).unwrap();
        let id = JobId::parse(&job.id).unwrap();
        let status = Status {
            v: 1,
            request_id: Some("r".into()),
            ..Default::default()
        };
        (
            t,
            Worker {
                store,
                id,
                job,
                status,
                control_in_flight: None,
                omp_queued: false,
                cancel: Arc::new(AtomicBool::new(false)),
                deadline: Instant::now() + std::time::Duration::from_secs(20),
            },
        )
    }
    fn frame(v: serde_json::Value) -> Frame {
        client::parse_frame(v.to_string().as_bytes()).unwrap()
    }
    fn ack() -> Frame {
        frame(json!({"v":1,"id":"r","ok":true,"data":{"delivered_as":"prompt"}}))
    }
    #[test]
    fn settled_batch_warning_survives_to_job_status() {
        let (_t, mut w) = worker();
        let mut s = Observation::default();
        on_frame(&mut w, &mut s, ack()).unwrap();
        let done = on_frame(&mut w, &mut s, frame(json!({"v":1,"id":"r","event":"settled","status":"completed","assistant_text":"shared answer","warnings":[{"code":"batched","message":"shared with j7654321"},{"future":"unknown"}]}))).unwrap().unwrap();
        done.unwrap();
        w.save().unwrap();
        let saved = w.store.read_status(&w.id).unwrap();
        assert_eq!(saved.warnings.len(), 1);
        assert_eq!(saved.warnings[0].code, "batched");
        assert!(saved.warnings[0].message.contains("j7654321"));
        assert_eq!(s.assistant.as_deref(), Some("shared answer"));
    }
    #[test]
    fn frame_outcome_table() {
        for (status, code, state) in [
            ("completed", None, State::Completed),
            ("aborted", Some("aborted"), State::Aborted),
            ("error", Some("omp_error"), State::Failed),
        ] {
            let (_t, mut w) = worker();
            let mut s = Observation::default();
            assert!(on_frame(&mut w, &mut s, ack()).unwrap().is_none());
            assert!(s.acked);
            assert_eq!(w.status.status, State::Accepted);
            let terminal=on_frame(&mut w,&mut s,frame(json!({"v":1,"id":"r","event":"settled","status":status,"assistant_text":"new","stop_reason":"stop","error_message":"model error"}))).unwrap().unwrap();
            if let Some(code) = code {
                let f = terminal.unwrap_err();
                assert_eq!(f.code, code);
                assert_eq!(f.state, state);
            } else {
                terminal.unwrap();
            }
            assert!(s.settled);
            assert_eq!(s.assistant.as_deref(), Some("new"));
        }
        for (wire, code, state) in [
            ("session_switched", "session_switched", State::Aborted),
            ("shutdown", "omp_exited", State::Failed),
            ("not_delivered", "not_delivered", State::Failed),
            ("internal", "protocol", State::Failed),
        ] {
            let (_t, mut w) = worker();
            let mut s = Observation {
                acked: true,
                ..Default::default()
            };
            let f=on_frame(&mut w,&mut s,frame(json!({"v":1,"id":"r","event":"error","error":{"code":wire,"message":"reason"}}))).unwrap().unwrap().unwrap_err();
            assert_eq!(f.code, code);
            assert_eq!(f.state, state);
        }
        for (wire, code) in [
            ("session_mismatch", "session_moved"),
            ("unsupported_version", "protocol"),
            ("unsupported_op", "protocol"),
            ("bad_request", "protocol"),
            ("internal", "protocol"),
            ("send_failed", "omp_error"),
        ] {
            let (_t, mut w) = worker();
            let mut s = Observation::default();
            let f=on_frame(&mut w,&mut s,frame(json!({"v":1,"id":"r","ok":false,"error":{"code":wire,"message":"m","details":{"session_id":"new"}}}))).unwrap().unwrap().unwrap_err();
            assert_eq!(f.code, code);
            if wire == "session_mismatch" {
                assert_eq!(f.details.unwrap()["to"], "new");
            }
        }
    }
    #[test]
    fn consumption_fork_unknown_and_protocol_order() {
        let (_t, mut w) = worker();
        let mut s = Observation::default();
        let early = frame(json!({"v":1,"id":"r","event":"consumed"}));
        assert_eq!(
            on_frame(&mut w, &mut s, early)
                .unwrap()
                .unwrap()
                .unwrap_err()
                .code,
            "protocol"
        );
        on_frame(&mut w, &mut s, ack()).unwrap();
        for event in [
            json!({"v":1,"id":"r","event":"future"}),
            json!({"v":1,"id":"r","event":"consumed"}),
            json!({"v":1,"id":"r","event":"session_changed","to":"new"}),
        ] {
            assert!(on_frame(&mut w, &mut s, frame(event)).unwrap().is_none());
        }
        assert!(s.consumed);
        assert_eq!(w.status.actual_session_id.as_deref(), Some("new"));
        assert!(w.status.forked.is_some());
        assert_eq!(
            on_frame(&mut w, &mut s, ack())
                .unwrap()
                .unwrap()
                .unwrap_err()
                .code,
            "protocol"
        );
    }
    #[test]
    fn tick_deadline_ack_cancel_and_control() {
        for (acked, consumed, hint) in [
            (false, false, None),
            (true, false, Some("still queued")),
            (true, true, Some("keeps running")),
        ] {
            let (_t, mut w) = worker();
            w.deadline = Instant::now();
            let s = Observation {
                acked,
                consumed,
                ..Default::default()
            };
            let f = on_tick(&mut w, &s, Instant::now()).unwrap();
            assert_eq!(f.code, "timeout");
            if let Some(h) = hint {
                assert!(f.hint.unwrap().contains(h));
            } else {
                assert!(f.hint.is_none());
            }
        }
        let (_t, mut w) = worker();
        let s = Observation::default();
        let f = on_tick(&mut w, &s, Instant::now() - super::super::ACK).unwrap();
        assert_eq!(f.code, "protocol");
        assert_eq!(w.status.warnings[0].code, "delivery_unknown");
        w.cancel.store(true, Ordering::SeqCst);
        let f = on_tick(&mut w, &s, Instant::now()).unwrap();
        assert_eq!(f.state, State::Aborted);
        assert_eq!(f.code, "cancelled");
        assert!(
            w.status
                .warnings
                .iter()
                .any(|w| w.code == "live_turn_continues")
        );
        let dir = w.store.dir(&w.id).join("control");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("abort.json"), r#"{"v":1,"op":"abort"}"#).unwrap();
        controls(&w);
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("abort.done.json")).unwrap()).unwrap();
        assert_eq!(result["error"]["code"], "unsupported_op");
    }
}

#[cfg(test)]
mod malformed_tests {
    use super::*;
    #[test]
    fn terminal_text_fields_accept_nulls_and_reject_wrong_types() {
        for key in ["assistant_text", "stop_reason", "error_message"] {
            assert!(optional_text(&json!({}), key).unwrap().is_none());
            assert!(optional_text(&json!({key:null}), key).unwrap().is_none());
            assert_eq!(
                optional_text(&json!({key:"text"}), key).unwrap().as_deref(),
                Some("text")
            );
            assert_eq!(
                optional_text(&json!({key:42}), key).unwrap_err().code,
                "protocol"
            );
        }
    }
}
