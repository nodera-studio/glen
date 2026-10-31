//! SEC-3 read paths (W8 review MED-001): glen refuses a GLEN_HOME another user could write to
//! before it reads a live registry, a job record or an inbox reply from it, not only before it
//! writes there.
#![cfg(unix)]
mod common;
use common::*;
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Output};

fn refused(out: Output) -> Value {
    assert_eq!(
        out.status.code(),
        Some(2),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["error"]["code"], "invalid_env", "{value}");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("refusing GLEN_HOME directory"),
        "{value}"
    );
    value
}
fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn live_route_controls_and_send_refuse_an_untrusted_home() {
    let mut b = LiveBox::new();
    b.start_ext(A_ID, "ok", &[]);
    let root = b.rpc.omp.glen_home.clone();
    chmod(&root, 0o777);
    for args in [
        &["abort", "e27c0de", "--json"][..],
        &["thinking", "e27c0de", "high", "--json"],
        &["models", "e27c0de", "--json"],
        &["send", "e27c0de", "planted socket", "--no-wait", "--json"],
    ] {
        refused(b.rpc.glen(args).output().unwrap());
    }
    chmod(&root, 0o700);
    let ops: Vec<_> = b
        .ext_log()
        .into_iter()
        .filter_map(|v| v["frame"]["op"].as_str().map(str::to_owned))
        .filter(|op| op != "hello" && op != "state")
        .collect();
    assert!(ops.is_empty(), "nothing reached the endpoint: {ops:?}");
    assert!(!root.join("jobs").exists(), "no job was created");
}

#[test]
fn inbox_peek_refuses_planted_replies() {
    let b = OmpBox::new();
    let inbox = b.glen_home.join("inbox");
    fs::create_dir_all(&inbox).unwrap();
    fs::write(
        inbox.join(format!("{A_ID}.ndjson")),
        format!(
            "{}\n",
            serde_json::json!({"v":1,"id":"r1","session_id":A_ID,"text":"forged","final":true,"at":"2026-10-31T00:00:00Z"})
        ),
    )
    .unwrap();
    chmod(&inbox, 0o777);
    refused(b.glen(&["inbox", "--peek", "--json"], &b.alpha()));
    chmod(&inbox, 0o700);
    let value = json(&b.glen(&["inbox", "--peek", "--json"], &b.alpha()));
    assert_eq!(value["ok"], true, "{value}");
}

#[test]
fn job_reads_refuse_an_untrusted_jobs_directory() {
    let b = OmpBox::new();
    let jobs = b.glen_home.join("jobs");
    fs::create_dir_all(&jobs).unwrap();
    chmod(&jobs, 0o777);
    refused(b.glen(&["jobs", "--json"], &b.alpha()));
    refused(b.glen(&["wait", "j1234567", "--json"], &b.alpha()));
    chmod(&jobs, 0o700);
    assert_eq!(json(&b.glen(&["jobs", "--json"], &b.alpha()))["ok"], true);
}
