mod common;
use common::*;
use glen::{
    glen_home::GlenHome,
    jobs::{Job, JobId, JobResult, JobStore, State, Status, liveness},
};
use serde_json::{Value, json};
use std::{fs, time::Duration};
fn store(b: &RpcBox) -> JobStore {
    JobStore::new(GlenHome {
        root: b.omp.glen_home.clone(),
    })
}
fn make(b: &RpcBox, state: State, raw: Option<&str>) -> JobId {
    let id = raw.and_then(JobId::parse).unwrap_or_default();
    let s = store(b);
    let now = jiff::Timestamp::now();
    let j = Job {
        v: 1,
        id: id.as_str().into(),
        handle: id.handle(),
        created_at: now.to_string(),
        route: glen::jobs::Route::Rpc,
        session_id: Some(A_ID.into()),
        session_file: Some(b.omp.session('A')),
        session_handle: Some("e27c0de".into()),
        deliver_as: glen::jobs::Deliver::Auto,
        text: "hello\x1b world\nsecond line".into(),
        cwd: b.omp.alpha(),
        omp_bin: b.fake.clone(),
        profile_flag: None,
        owners_dir: b.owners(),
        job_timeout_ms: 30000,
        deadline_at: now.to_string(),
    };
    s.create(&j).unwrap();
    let own = liveness::current().unwrap();
    let status = Status {
        v: 1,
        status: state,
        updated_at: now.to_string(),
        worker_pid: Some(own.pid),
        worker_start: Some(own.start),
        actual_session_id: Some("01a1ffff-0000-7000-8000-0000abcdef01".into()),
        ..Status::default()
    };
    s.write_status(&id, &status).unwrap();
    if state == State::Completed {
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
                settled_at: Some(now.to_string()),
            },
        )
        .unwrap();
    }
    id
}
fn run(b: &RpcBox, args: &[&str], code: i32) -> Value {
    let o = b.glen(args).output().unwrap();
    assert_eq!(o.status.code(), Some(code), "{o:?}");
    assert!(o.stderr.is_empty());
    assert!(!o.stdout.contains(&27));
    json(&o)
}
#[test]
fn ac220_empty_lazy() {
    let b = RpcBox::new();
    let v = run(&b, &["jobs", "--json"], 0);
    assert_eq!(v["data"], json!({"total":0,"jobs":[]}));
    assert!(!b.omp.glen_home.exists());
    let o = b.glen(&["jobs", "--human"]).output().unwrap();
    assert_eq!(o.stdout, b"no glen jobs yet\n");
    assert!(!b.omp.glen_home.exists());
}
#[test]
fn ac220_order_limits_filters_and_lost_check() {
    let b = RpcBox::new();
    let one = make(&b, State::Completed, None);
    let two = make(&b, State::Accepted, None);
    let three = make(&b, State::Queued, None);
    let v = run(&b, &["jobs", "-n", "2", "--json"], 0);
    assert_eq!(v["data"]["total"], 3);
    assert_eq!(v["data"]["jobs"].as_array().unwrap().len(), 2);
    assert_eq!(v["data"]["jobs"][0]["id"], three.as_str());
    assert_eq!(v["data"]["jobs"][1]["id"], two.as_str());
    assert_eq!(v["data"]["jobs"][0]["text_preview"], "hello world");
    let v = run(&b, &["jobs", "-n", "0", "--json"], 0);
    assert_eq!(v["data"]["jobs"].as_array().unwrap().len(), 3);
    assert_eq!(v["data"]["jobs"][2]["id"], one.as_str());
    for filter in [A_ID, "C0DE", "abcdef01"] {
        let v = run(&b, &["jobs", "--session", filter, "--json"], 0);
        assert_eq!(v["data"]["total"], 3);
    }
    assert_eq!(
        run(&b, &["jobs", "--session", "cde", "--json"], 0)["data"]["total"],
        0
    );
    assert_eq!(
        run(&b, &["jobs", "--active", "--json"], 0)["data"]["total"],
        2
    );
    let s = store(&b);
    let mut status = s.read_status(&two).unwrap();
    status.worker_start = Some("reused pid".into());
    s.write_status(&two, &status).unwrap();
    assert_eq!(
        run(&b, &["jobs", "--active", "--json"], 0)["data"]["total"],
        1
    );
    assert_eq!(s.read_status(&two).unwrap().status, State::Lost);
    // New-session jobs match their actual id too.
    let mut j = s.read_job(&three).unwrap();
    j.route = glen::jobs::Route::New;
    j.session_id = None;
    j.session_handle = None;
    glen::glen_home::write_private_atomic(
        &s.dir(&three).join("job.json"),
        &serde_json::to_vec(&j).unwrap(),
    )
    .unwrap();
    assert_eq!(
        run(&b, &["jobs", "--session", "ef01", "--json"], 0)["data"]["total"],
        3
    );
    let o = b
        .glen(&["jobs", "--human", "-n", "2"])
        .env("COLUMNS", "60")
        .output()
        .unwrap();
    assert!(o.status.success());
    let text = String::from_utf8(o.stdout).unwrap();
    assert!(text.contains("3 jobs"));
    assert!(text.contains("showing 2"));
    assert!(
        text.lines().all(|l| glen::render::text::width(l) <= 60),
        "{text}"
    );
}
#[test]
fn ac221_job_resolution() {
    let b = RpcBox::new();
    let id = make(&b, State::Completed, None);
    for arg in [
        id.handle(),
        id.as_str().into(),
        id.simple(),
        format!("j{}", id.simple()),
    ] {
        let v = run(&b, &["wait", &arg, "--json"], 0);
        assert_eq!(v["data"]["assistant_text"], "pong");
    }
    for (raw, code) in [("jfffffff", 3), ("e27c0de", 2), ("j123", 2)] {
        let v = run(&b, &["wait", raw, "--json"], code);
        assert_eq!(v["ok"], false);
    }
    for raw in [
        "01a00000-0000-7000-8000-000001234567",
        "01a00001-0000-7000-8000-000001234567",
    ] {
        make(&b, State::Completed, Some(raw));
    }
    let v = run(&b, &["wait", "J1234567", "--json"], 4);
    assert_eq!(
        v["error"]["details"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let empty = RpcBox::new();
    run(&empty, &["wait", "j1234567", "--json"], 3);
    assert!(!empty.omp.glen_home.exists());
}
#[test]
fn ac223_send_prunes_only_old_terminal_jobs() {
    let b = RpcBox::new();
    let s = store(&b);
    let now = jiff::Timestamp::now();
    let mut expected = vec![];
    for (state, days, keep) in [
        (State::Completed, 8, false),
        (State::Lost, 8, false),
        (State::Accepted, 8, true),
        (State::Failed, 6, true),
    ] {
        let id = make(&b, state, None);
        let mut status = s.read_status(&id).unwrap();
        status.updated_at = now
            .checked_sub(jiff::SignedDuration::from_hours(days * 24))
            .unwrap()
            .to_string();
        s.write_status(&id, &status).unwrap();
        expected.push((id, keep));
    }
    let v = run(&b, &["send", "e27c0de", "hi", "--json"], 0);
    let id = v["data"]["job"]["id"].as_str().unwrap();
    b.wait_worker_gone(id);
    for (id, keep) in expected {
        assert_eq!(s.dir(&id).exists(), keep);
    }
    assert!(b.wait_terminal(id, Duration::from_secs(1))["status"] == "completed");
    assert!(fs::metadata(s.dir(&JobId::parse(id).unwrap())).is_ok());
}
