//! W8 audit fixes for jobs: ERR-2 (unpersistable `lost`), DOC-10 (`forked.to_handle` in
//! `glen jobs`) and DOC-11 (job `not_found` hint).
mod common;
use common::*;
use glen::{
    glen_home::GlenHome,
    jobs::{Deliver, Forked, Job, JobId, JobResult, JobStore, Route, State, Status, liveness},
};
use serde_json::Value;
use std::{fs, path::Path};

fn store(b: &RpcBox) -> JobStore {
    JobStore::new(GlenHome {
        root: b.omp.glen_home.clone(),
    })
}
fn make(b: &RpcBox, state: State, worker_start: &str) -> JobId {
    let id = JobId::new();
    let s = store(b);
    let now = jiff::Timestamp::now();
    s.create(&Job {
        v: 1,
        id: id.as_str().into(),
        handle: id.handle(),
        created_at: now.to_string(),
        route: Route::Rpc,
        session_id: Some(A_ID.into()),
        session_file: Some(b.omp.session('A')),
        session_handle: Some("e27c0de".into()),
        deliver_as: Deliver::Auto,
        text: "hello".into(),
        cwd: b.omp.alpha(),
        omp_bin: Some(b.fake.clone()),
        live_socket: None,
        live_pid: None,
        profile_flag: None,
        thinking_flag: None,
        owners_dir: b.owners(),
        job_timeout_ms: 30000,
        deadline_at: now.to_string(),
    })
    .unwrap();
    s.write_status(
        &id,
        &Status {
            v: 1,
            status: state,
            updated_at: now.to_string(),
            worker_pid: Some(std::process::id()),
            worker_start: Some(worker_start.into()),
            ..Status::default()
        },
    )
    .unwrap();
    id
}
fn run(b: &RpcBox, args: &[&str], code: i32) -> Value {
    let o = b.glen(args).output().unwrap();
    assert_eq!(o.status.code(), Some(code), "{o:?}");
    assert!(
        o.stderr.is_empty(),
        "stderr must stay empty in JSON mode: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    json(&o)
}
fn lost_warnings(v: &Value) -> Vec<&Value> {
    v["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|w| w["code"] == "lost_write_failed")
        .collect()
}
fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
fn assert_lost_but_unpersisted(b: &RpcBox, id: &JobId) {
    let handle = id.handle();
    let v = run(b, &["wait", &handle, "--timeout", "2s", "--json"], 7);
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["error"]["code"], "job_failed", "{v}");
    assert_eq!(v["error"]["details"]["status"], "lost", "{v}");
    assert_eq!(v["error"]["details"]["error"]["code"], "lost", "{v}");
    let warnings = lost_warnings(&v);
    assert_eq!(warnings.len(), 1, "{v}");
    let message = warnings[0]["message"].as_str().unwrap();
    assert!(message.contains(&handle), "{message}");
    assert!(
        message.contains("status.lock") || message.contains("status.json"),
        "{message}"
    );

    let v = run(b, &["jobs", "--active", "--json"], 0);
    assert_eq!(v["data"]["total"], 0, "{v}");
    assert_eq!(lost_warnings(&v).len(), 1, "{v}");
    let v = run(b, &["jobs", "--json"], 0);
    assert_eq!(v["data"]["jobs"][0]["status"], "lost", "{v}");
    assert_eq!(lost_warnings(&v).len(), 1, "{v}");
    // Nothing was persisted: the stored status still says the job runs.
    assert_eq!(store(b).read_status(id).unwrap().status, State::Accepted);
}

#[cfg(unix)]
#[test]
fn err2_dead_job_in_read_only_dir_reports_lost_with_a_warning() {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root ignores directory permissions");
        return;
    }
    let b = RpcBox::new();
    let id = make(&b, State::Accepted, "dead identity");
    let dir = b.job_dir(id.as_str());
    let _ = fs::remove_file(dir.join("status.lock"));
    chmod(&dir, 0o555);
    assert_lost_but_unpersisted(&b, &id);
    chmod(&dir, 0o700);
}

#[cfg(unix)]
#[test]
fn err2_dead_job_with_a_directory_status_lock_reports_lost_with_a_warning() {
    let b = RpcBox::new();
    let id = make(&b, State::Accepted, "dead identity");
    let lock = b.job_dir(id.as_str()).join("status.lock");
    let _ = fs::remove_file(&lock);
    fs::create_dir(&lock).unwrap();
    assert_lost_but_unpersisted(&b, &id);
}

#[test]
fn err2_writable_dead_job_is_persisted_without_a_warning() {
    let b = RpcBox::new();
    let id = make(&b, State::Accepted, "dead identity");
    let v = run(&b, &["wait", &id.handle(), "--timeout", "2s", "--json"], 7);
    assert_eq!(v["error"]["details"]["status"], "lost", "{v}");
    assert!(lost_warnings(&v).is_empty(), "{v}");
    assert_eq!(store(&b).read_status(&id).unwrap().status, State::Lost);
}

#[test]
fn doc10_jobs_rows_carry_the_forked_to_handle() {
    let b = RpcBox::new();
    let own = liveness::current().unwrap();
    let id = make(&b, State::Completed, &own.start);
    let s = store(&b);
    let mut status = s.read_status(&id).unwrap();
    status.forked = Some(Forked {
        from: A_ID.into(),
        to: "01a00000-0000-7000-8000-000001234567".into(),
    });
    s.write_status(&id, &status).unwrap();
    s.write_result(
        &id,
        &JobResult {
            v: 1,
            prompt_status: Some("completed".into()),
            agent_invoked: true,
            assistant_text: Some("pong".into()),
            replies: vec![],
            command_output: vec![],
            stop_reason: Some("stop".into()),
            session_settled: true,
            settled_at: None,
        },
    )
    .unwrap();
    let v = run(&b, &["jobs", "--json"], 0);
    let forked = &v["data"]["jobs"][0]["forked"];
    assert_eq!(forked["from"], A_ID, "{v}");
    assert_eq!(forked["to"], "01a00000-0000-7000-8000-000001234567", "{v}");
    assert_eq!(forked["to_handle"], "1234567", "{v}");
    // Same shape as `glen wait`'s forked field.
    let waited = run(&b, &["wait", &id.handle(), "--json"], 11);
    assert_eq!(&waited["data"]["forked"], forked, "{waited}");
    // A job that did not fork keeps `forked: null`.
    let plain = make(&b, State::Accepted, &own.start);
    let v = run(&b, &["jobs", "--json"], 0);
    let row = v["data"]["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == plain.as_str())
        .unwrap();
    assert!(row["forked"].is_null(), "{v}");
}

#[test]
fn doc11_unknown_job_not_found_carries_a_hint() {
    let b = RpcBox::new();
    make(&b, State::Accepted, "irrelevant");
    let v = run(&b, &["wait", "j1234567", "--json"], 3);
    assert_eq!(v["error"]["code"], "not_found", "{v}");
    assert_eq!(v["error"]["hint"], "run `glen jobs`", "{v}");
}
