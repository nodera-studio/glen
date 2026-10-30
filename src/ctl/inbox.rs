//! Deadline-bearing requests to a job's held RPC client.
use crate::{
    glen_home::write_private_atomic,
    jobs::{JobId, JobStore},
    ops::{self, Op, OpError},
};
use serde_json::{Value, json};
use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
pub fn request(
    store: &JobStore,
    job: &JobId,
    op: &Op,
    expect: &str,
    deadline: Instant,
    stop: &AtomicBool,
) -> Result<Value, OpError> {
    super::ephemeral::stopped(deadline, stop)?;
    let sub = format!("jobs/{}/control", job.as_str());
    let dir = store.home.ensure_dir(&sub).map_err(|e| {
        OpError::new(
            ops::INTERNAL,
            format!("cannot create control inbox for {}: {e}", job.handle()),
        )
    })?;
    let name = uuid::Uuid::now_v7().to_string();
    let path = dir.join(format!("{name}.json"));
    let done = path.with_extension("done.json");
    let mut request = op.request_for(&name, Some(expect));
    if let Some(object) = request.as_object_mut() {
        object.remove("id");
    }
    request["deadline_ms"] = json!(
        jiff::Timestamp::now().as_millisecond().max(0) as u64
            + deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u128::from(u64::MAX)) as u64
    );
    let bytes =
        serde_json::to_vec(&request).map_err(|e| OpError::new(ops::INTERNAL, e.to_string()))?;
    write_private_atomic(&path, &bytes).map_err(|e| {
        OpError::new(
            ops::INTERNAL,
            format!("cannot write control {}: {e}", path.display()),
        )
    })?;
    let mut ended = None;
    loop {
        super::ephemeral::stopped(deadline, stop)?;
        match std::fs::read(&done) {
            Ok(bytes) => {
                let response: Value = serde_json::from_slice(&bytes).map_err(|e| {
                    OpError::new(
                        ops::INTERNAL,
                        format!("invalid control answer {}: {e}", done.display()),
                    )
                })?;
                std::fs::remove_file(&done).map_err(|e| {
                    OpError::new(
                        ops::INTERNAL,
                        format!("cannot remove control answer {}: {e}", done.display()),
                    )
                })?;
                if response["v"] != 1 || !response["ok"].is_boolean() {
                    return Err(OpError::new(
                        ops::INTERNAL,
                        "invalid control answer protocol",
                    ));
                }
                if response["ok"] == true {
                    return Ok(response.get("data").cloned().unwrap_or_else(|| json!({})));
                }
                let error: OpError =
                    serde_json::from_value(response["error"].clone()).map_err(|e| {
                        OpError::new(ops::INTERNAL, format!("invalid control error: {e}"))
                    })?;
                if error.code == ops::UNSUPPORTED_OP {
                    return Err(OpError::new(
                        ops::BUSY,
                        format!(
                            "job {} was started by an older glen; wait for it (`glen wait {}`) and retry",
                            job.handle(),
                            job.handle()
                        ),
                    ));
                }
                return Err(error);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => {
                return Err(OpError::new(
                    ops::INTERNAL,
                    format!("cannot read control answer {}: {e}", done.display()),
                ));
            }
        }
        if store.read_status(job).is_ok_and(|s| s.status.is_terminal())
            && ended.get_or_insert_with(Instant::now).elapsed() >= Duration::from_secs(1)
        {
            return Err(OpError::new(
                ops::JOB_ENDED,
                format!("job {} ended before answering the control", job.handle()),
            ));
        }
        std::thread::sleep(
            Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}
