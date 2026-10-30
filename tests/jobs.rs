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
fn w2_wait_shape_before_lookup_with_missing_home_or_no_match() {
    let b = RpcBox::new();
    for populated in [false, true] {
        if populated {
            make(&b, State::Completed, None);
        }
        for (arg, exit, code) in [
            ("garbage", 2, "usage"),
            ("j123", 2, "usage"),
            ("j123456g", 2, "usage"),
            ("J1234567", 3, "not_found"),
            ("01a00000-0000-7000-8000-000001234567", 3, "not_found"),
        ] {
            let value = run(&b, &["wait", arg, "--json"], exit);
            assert_eq!(value["error"]["code"], code);
            if exit == 2 {
                assert!(
                    value["error"]["hint"]
                        .as_str()
                        .unwrap()
                        .contains("glen jobs")
                );
            }
            assert_eq!(b.omp.glen_home.exists(), populated);
        }
    }
}
#[test]
fn w2_lost_age_uses_updated_at_instead_of_mtime() {
    for (updated_at, old_mtime, expected) in [
        ("2000-01-01T00:00:00Z", false, State::Lost),
        ("2099-01-01T00:00:00Z", true, State::Queued),
        ("invalid timestamp", true, State::Queued),
    ] {
        let b = RpcBox::new();
        let id = make(&b, State::Queued, None);
        let store = store(&b);
        let mut status = store.read_status(&id).unwrap();
        status.worker_pid = None;
        status.worker_start = None;
        status.updated_at = updated_at.into();
        store.write_status(&id, &status).unwrap();
        if old_mtime {
            fs::File::open(store.dir(&id).join("status.json"))
                .unwrap()
                .set_modified(std::time::SystemTime::UNIX_EPOCH)
                .unwrap();
        }
        let value = run(
            &b,
            &["wait", id.as_str(), "--timeout", "0", "--json"],
            if expected == State::Lost { 7 } else { 10 },
        );
        assert_eq!(store.read_status(&id).unwrap().status, expected);
        if expected == State::Lost {
            assert_eq!(value["error"]["details"]["error"]["code"], "lost");
        }
    }
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

#[test]
fn review13_human_duration_multiline_replies_and_session_handle() {
    let b = RpcBox::new();
    let id = make(&b, State::Completed, None);
    let store = store(&b);
    let mut j = store.read_job(&id).unwrap();
    j.created_at = "2026-10-30T00:00:00Z".into();
    j.route = glen::jobs::Route::New;
    j.session_handle = None;
    glen::glen_home::write_private_atomic(
        &store.dir(&id).join("job.json"),
        &serde_json::to_vec(&j).unwrap(),
    )
    .unwrap();
    let mut s = store.read_status(&id).unwrap();
    s.updated_at = "2026-10-30T00:01:05Z".into();
    store.write_status(&id, &s).unwrap();
    let mut r = store.read_result(&id).unwrap();
    r.replies = vec![glen::jobs::Reply {
        v: 1,
        id: "reply".into(),
        session_id: s.actual_session_id.clone().unwrap(),
        job_id: Some(j.id.clone()),
        request_id: None,
        tool_call_id: None,
        text: "first line\nsecond line wraps with enough words for a narrow terminal".into(),
        is_final: true,
        at: s.updated_at,
    }];
    store.write_result(&id, &r).unwrap();
    let v = run(&b, &["wait", id.as_str(), "--json"], 0);
    assert_eq!(v["data"]["job"]["elapsed_ms"], 65000);
    let o = b
        .glen(&["wait", id.as_str(), "--human"])
        .env("COLUMNS", "40")
        .output()
        .unwrap();
    assert!(o.status.success());
    let text = String::from_utf8(o.stdout).unwrap();
    assert!(text.contains("1m 05s"), "{text}");
    let lines: Vec<_> = text.lines().collect();
    let first = lines.iter().position(|s| s.starts_with("↩ ")).unwrap();
    assert_eq!(lines[first], "↩ first line ✓");
    assert!(
        lines[first + 1..]
            .iter()
            .all(|s| s.starts_with("  ") && !s.contains('↩') && !s.contains('✓'))
    );
    assert_eq!(text.matches('↩').count(), 1);
    assert!(lines.iter().all(|s| glen::render::text::width(s) <= 40));
    for width in ["20", "30", "40", "60", "100"] {
        let o = b
            .glen(&["jobs", "--human"])
            .env("COLUMNS", width)
            .output()
            .unwrap();
        assert!(o.status.success());
        let text = String::from_utf8(o.stdout).unwrap();
        assert!(text.contains("bcdef01"), "{width}: {text}");
        assert!(
            text.lines()
                .all(|s| glen::render::text::width(s) <= width.parse::<usize>().unwrap()),
            "{width}: {text}"
        );
    }
}
#[test]
fn review13_all_rpc_options_have_help() {
    use clap::CommandFactory;
    for name in ["send", "wait", "jobs"] {
        let mut cmd = glen::cli::Cli::command();
        let sub = cmd.find_subcommand_mut(name).unwrap();
        for arg in sub.get_arguments() {
            assert!(
                arg.get_help().is_some(),
                "{name}: {} lacks help",
                arg.get_id()
            );
        }
    }
}

#[test]
fn cycle3_narrow_jobs_preserve_full_job_handle() {
    let b = RpcBox::new();
    let id = make(&b, State::Completed, None);
    for width in [30, 40] {
        let o = b
            .glen(&["jobs", "--human"])
            .env("COLUMNS", width.to_string())
            .output()
            .unwrap();
        assert!(o.status.success());
        let text = String::from_utf8(o.stdout).unwrap();
        assert!(text.contains(&id.handle()), "{width}: {text}");
        assert!(text.contains("e27c0de"), "{width}: {text}");
        assert!(text.lines().all(|s| glen::render::text::width(s) <= width));
    }
}

#[test]
fn cycle3_status_lock_open_failure_reports_writing_but_reads_still_report_reading() {
    let b = RpcBox::new();
    let id = make(&b, State::Queued, None);
    let store = store(&b);
    let status = store.read_status(&id).unwrap();
    let path = store.dir(&id).join("status.lock");
    // Replace only the test-created lock file to force an open error, even as root.
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    let err = store.write_status(&id, &status).unwrap_err();
    assert_eq!(err.code, glen::error::ErrorCode::Io);
    assert!(
        err.message
            .starts_with(&format!("cannot open {} for writing: ", path.display())),
        "{err}"
    );
    let absent = JobId::new();
    let err = store.read_status(&absent).unwrap_err();
    assert_eq!(err.code, glen::error::ErrorCode::NotFound);
    assert!(
        err.message.starts_with(&format!(
            "{}: ",
            store.dir(&absent).join("status.json").display()
        )),
        "{err}"
    );
}
