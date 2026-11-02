//! `tools/call`: the exec-self runner and the result mapping (design §A22).
use crate::{
    error::{ErrorCode, GlenError},
    output::envelope::{error_value, to_json_vec},
};
use serde_json::{Value, json};
use std::{ffi::OsString, path::PathBuf};

/// The program a tool call runs: glen itself in production, a script in unit tests.
#[derive(Clone, Debug)]
pub struct Launcher {
    pub program: PathBuf,
    pub prefix_args: Vec<OsString>,
}

/// A `tools/call` result whose text is `envelope` rendered as the CLI renders it on a pipe.
fn synthesized(envelope: &Value) -> Value {
    let mut text = to_json_vec(envelope, false).unwrap_or_default();
    text.push(b'\n');
    json!({
        "content":[{"type":"text","text":String::from_utf8_lossy(&text)}],
        "isError":true,
    })
}

/// The `internal` / `mcp_exec` result for a call glen could not run to an envelope.
pub(crate) fn exec_failure(tool: &str, message: &str, stderr: &[u8]) -> Value {
    let mut details = json!({"reason":"mcp_exec","tool":tool});
    let stderr = String::from_utf8_lossy(stderr);
    if !stderr.trim().is_empty() {
        details["stderr"] = json!(stderr.trim_end());
    }
    synthesized(&error_value(
        &GlenError::new(ErrorCode::Internal, message).with_details(details),
        &[],
    ))
}
