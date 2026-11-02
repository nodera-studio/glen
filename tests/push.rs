//! v0.2 W2 push tier 1 (design §A20): `glen wait --for reply`, `glen inbox --wait` and the
//! `task_state` job-view field, on the real binary with the `fake_omp` and `fake_ext` peers.
mod common;
use common::*;
use glen::{
    glen_home::GlenHome,
    jobs::{Forked, Job, JobError, JobId, JobResult, JobStore, Reply, State, Status, liveness},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// The wake bound of AC-v2-201 and AC-v2-202, measured from the moment the test sees the record.
const WAKE: Duration = Duration::from_secs(1);

fn envelope(out: &Output, code: i32) -> Value {
    assert_eq!(out.status.code(), Some(code), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    json(out)
}
fn texts(replies: &Value) -> Vec<&str> {
    replies
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {replies}"))
        .iter()
        .map(|r| r["text"].as_str().unwrap())
        .collect()
}
fn ids(replies: &Value) -> Vec<&str> {
    replies
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {replies}"))
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect()
}
/// Spawns `cmd` and returns its output with the instant it exited.
fn spawn_timed(mut cmd: Command) -> JoinHandle<(Output, Instant)> {
    let child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::spawn(move || {
        let out = child.wait_with_output().unwrap();
        (out, Instant::now())
    })
}
/// Polls `path` until it holds `n` complete lines and returns the instant the test saw them.
fn seen_lines(path: &Path, n: usize) -> Instant {
    let start = Instant::now();
    loop {
        let text = fs::read_to_string(path).unwrap_or_default();
        if text.ends_with('\n') && text.lines().count() >= n {
            return Instant::now();
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "{} never had {n} lines",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn reply_ids(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}
fn send_no_wait(b: &RpcBox, vars: &[(&str, &Path)]) -> Value {
    let mut cmd = b.glen(&["send", "e27c0de", "hi", "--no-wait", "--json"]);
    cmd.env("FAKE_OMP_SCENARIO", "progress");
    for (k, v) in vars {
        cmd.env(k, v);
    }
    envelope(&cmd.output().unwrap(), 10)
}
fn accepted(b: &RpcBox, id: &str) {
    poll(Duration::from_secs(10), || {
        (b.job(id).1["status"] == "accepted").then_some(())
    });
}

#[test]
fn rpc_reply_wakes_wait_for_reply_within_a_second() {
    let b = RpcBox::new();
    let run = b.omp.tmp.path().join("run-release");
    let result = b.omp.tmp.path().join("result-release");
    let sent = send_no_wait(
        &b,
        &[
            ("FAKE_OMP_RUN_RELEASE", &run),
            ("FAKE_OMP_RESULT_RELEASE", &result),
        ],
    );
    let job = &sent["data"]["job"];
    assert_eq!(
        job["task_state"],
        if job["status"] == "queued" {
            "submitted"
        } else {
            "working"
        }
    );
    let id = job["id"].as_str().unwrap().to_owned();
    let handle = job["handle"].as_str().unwrap().to_owned();
    accepted(&b, &id);
    let waiter = spawn_timed(b.glen(&[
        "wait",
        &handle,
        "--for",
        "reply",
        "--timeout",
        "20s",
        "--json",
    ]));
    std::thread::sleep(Duration::from_millis(300));
    fs::write(&run, "").unwrap();
    let appended = seen_lines(&b.job_dir(&id).join("replies.ndjson"), 1);
    let (out, exited) = waiter.join().unwrap();
    let latency = exited.saturating_duration_since(appended);
    assert!(latency <= WAKE, "woke {latency:?} after the reply landed");
    let d = &envelope(&out, 10)["data"];
    assert_eq!(d["woke"], "reply", "{d}");
    assert_eq!(d["status"], "running");
    assert_eq!(d["interrupted"], false);
    assert_eq!(d["job"]["task_state"], "working");
    assert_eq!(d["job"]["status"], "accepted");
    assert_eq!(texts(&d["replies"])[0], "progress");
    let last = ids(&d["replies"]).last().unwrap().to_string();
    assert_eq!(
        d["next"],
        format!("glen wait {handle} --for reply --after {last} --timeout 20s")
    );
    fs::write(&result, "").unwrap();
    b.wait_worker_gone(&id);
}

#[test]
fn after_skips_seen_replies_and_the_end_carries_every_unseen_one() {
    let b = RpcBox::new();
    let result = b.omp.tmp.path().join("result-release");
    let sent = send_no_wait(&b, &[("FAKE_OMP_RESULT_RELEASE", &result)]);
    let id = sent["data"]["job"]["id"].as_str().unwrap().to_owned();
    let h = sent["data"]["job"]["handle"].as_str().unwrap().to_owned();
    let file = b.job_dir(&id).join("replies.ndjson");
    seen_lines(&file, 2);
    accepted(&b, &id);
    let [one, two] = <[String; 2]>::try_from(reply_ids(&file)).unwrap();
    let wait = |args: &[&str], code: i32| {
        let mut a = vec!["wait", h.as_str()];
        a.extend_from_slice(args);
        a.push("--json");
        envelope(&b.glen(&a).output().unwrap(), code)
    };
    // --after the first: only the second, as a wake.
    let d = &wait(&["--for", "reply", "--after", &one], 10)["data"];
    assert_eq!(d["woke"], "reply");
    assert_eq!(ids(&d["replies"]), [two.as_str()]);
    // --after the last: the bound, no woke, no replies, next repeats --after and --timeout.
    let d = &wait(
        &["--for", "reply", "--after", &two, "--timeout", "300ms"],
        10,
    )["data"];
    assert!(d.get("woke").is_none(), "{d}");
    assert_eq!(d["replies"], json!([]));
    assert_eq!(
        d["next"],
        format!("glen wait {h} --for reply --after {two} --timeout 300ms")
    );
    // An unknown --after is a usage error, never "everything is unseen".
    let e = &wait(&["--for", "reply", "--after", "not-a-reply"], 2)["error"];
    assert_eq!(e["code"], "usage");
    assert_eq!(e["details"]["reason"], "unknown_reply");
    assert_eq!(e["details"]["after"], "not-a-reply");
    // --after requires --for.
    let o = b
        .glen(&["wait", &h, "--after", &one, "--json"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2), "{o:?}");
    assert_eq!(json(&o)["error"]["code"], "usage");
    // wait never touches the inbox marker: both replies are still unread there.
    assert!(!b.omp.glen_home.join("inbox/.read").exists());
    let inbox = envelope(&b.glen(&["inbox", "--peek", "--json"]).output().unwrap(), 0);
    assert_eq!(inbox["data"]["total"], 2);
    // The job ends before the next wait: exit 0 with every unseen reply.
    fs::write(&result, "").unwrap();
    b.wait_terminal(&id, Duration::from_secs(20));
    let d = &wait(&["--for", "reply", "--after", &one], 0)["data"];
    assert_eq!(d["status"], "completed");
    assert_eq!(ids(&d["replies"]), [two.as_str()]);
    let d = &wait(&["--for", "reply"], 0)["data"];
    assert_eq!(ids(&d["replies"]), [one.as_str(), two.as_str()]);
    assert_eq!(d["job"]["task_state"], "completed");
    b.wait_worker_gone(&id);
}

#[test]
fn live_reply_wakes_wait_for_reply_within_a_second() {
    let mut b = LiveBox::new();
    let run = b.rpc.omp.tmp.path().join("run-release");
    let result = b.rpc.omp.tmp.path().join("result-release");
    b.start_ext(
        A_ID,
        "progress",
        &[
            ("FAKE_EXT_RUN_RELEASE", run.to_str().unwrap()),
            ("FAKE_EXT_RESULT_RELEASE", result.to_str().unwrap()),
        ],
    );
    let sent = envelope(
        &b.rpc
            .glen(&["send", "e27c0de", "hi", "--no-wait", "--json"])
            .env("GLEN_OMP_BIN", "/nonexistent")
            .output()
            .unwrap(),
        10,
    );
    assert_eq!(sent["data"]["job"]["route"], "live");
    let id = sent["data"]["job"]["id"].as_str().unwrap().to_owned();
    let h = sent["data"]["job"]["handle"].as_str().unwrap().to_owned();
    accepted(&b.rpc, &id);
    let waiter =
        spawn_timed(
            b.rpc
                .glen(&["wait", &h, "--for", "reply", "--timeout", "20s", "--json"]),
        );
    std::thread::sleep(Duration::from_millis(300));
    fs::write(&run, "").unwrap();
    let appended = seen_lines(&b.rpc.job_dir(&id).join("replies.ndjson"), 1);
    let (out, exited) = waiter.join().unwrap();
    let latency = exited.saturating_duration_since(appended);
    assert!(latency <= WAKE, "woke {latency:?} after the reply landed");
    let d = &envelope(&out, 10)["data"];
    assert_eq!(d["woke"], "reply", "{d}");
    assert_eq!(d["job"]["route"], "live");
    assert_eq!(d["job"]["task_state"], "working");
    assert_eq!(texts(&d["replies"])[0], "working");
    let last = ids(&d["replies"]).last().unwrap().to_string();
    fs::write(&result, "").unwrap();
    b.rpc.wait_terminal(&id, Duration::from_secs(20));
    let o = b
        .rpc
        .glen(&["wait", &h, "--for", "reply", "--after", &last, "--json"])
        .output()
        .unwrap();
    let d = &envelope(&o, 0)["data"];
    assert_eq!(d["status"], "completed");
    assert!(!ids(&d["replies"]).contains(&last.as_str()), "{d}");
    b.rpc.wait_worker_gone(&id);
}

fn store(b: &RpcBox) -> JobStore {
    JobStore::new(GlenHome {
        root: b.omp.glen_home.clone(),
    })
}
fn reply(job: &JobId, id: &str, text: &str) -> Reply {
    Reply {
        v: 1,
        id: id.into(),
        session_id: A_ID.into(),
        job_id: Some(job.as_str().into()),
        request_id: None,
        tool_call_id: Some("call".into()),
        text: text.into(),
        is_final: false,
        at: "2026-11-02T00:00:00Z".into(),
    }
}
/// A job record written straight through `JobStore` (no process): live while non-terminal,
/// because its worker identity is this test process. Terminal jobs get replies r1 and r2.
fn craft(b: &RpcBox, state: State, code: Option<&str>, forked: bool) -> (JobId, String) {
    let id = JobId::new();
    let s = store(b);
    let now = jiff::Timestamp::now();
    s.create(&Job {
        v: 1,
        id: id.as_str().into(),
        handle: id.handle(),
        created_at: now.to_string(),
        route: glen::jobs::Route::Rpc,
        session_id: Some(A_ID.into()),
        session_file: Some(b.omp.session('A')),
        session_handle: Some("e27c0de".into()),
        deliver_as: glen::jobs::Deliver::Auto,
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
    let own = liveness::current().unwrap();
    s.write_status(
        &id,
        &Status {
            v: 1,
            status: state,
            updated_at: now.to_string(),
            worker_pid: Some(own.pid),
            worker_start: Some(own.start),
            forked: forked.then(|| Forked {
                from: A_ID.into(),
                to: "01a0f6b1-2680-7a3c-8b21-4a1f9e27f0f0".into(),
            }),
            error: code.map(|code| JobError {
                code: code.into(),
                message: format!("{code} for the test"),
                hint: None,
                details: (code == "no_route").then(|| json!({"reason":"live_owner"})),
            }),
            ..Status::default()
        },
    )
    .unwrap();
    if state.is_terminal() {
        let replies = vec![reply(&id, "r1", "one"), reply(&id, "r2", "two")];
        for r in &replies {
            s.append_reply(&id, r).unwrap();
        }
        if state == State::Completed {
            s.write_result(
                &id,
                &JobResult {
                    v: 1,
                    prompt_status: Some("completed".into()),
                    agent_invoked: true,
                    assistant_text: Some("pong".into()),
                    replies,
                    command_output: vec![],
                    stop_reason: Some("stop".into()),
                    session_settled: true,
                    settled_at: Some(now.to_string()),
                },
            )
            .unwrap();
        }
    }
    let handle = id.handle();
    (id, handle)
}

#[test]
fn terminal_mapping_carries_only_the_unseen_replies() {
    let b = RpcBox::new();
    for (state, code, forked, exit, task_state) in [
        (State::Completed, None, false, 0, "completed"),
        (State::Completed, None, true, 11, "completed"),
        (State::Failed, Some("no_route"), false, 6, "rejected"),
        (State::Failed, Some("omp_error"), false, 7, "failed"),
        (State::Aborted, Some("cancelled"), false, 7, "canceled"),
        (State::Lost, Some("lost"), false, 7, "failed"),
    ] {
        let (_, h) = craft(&b, state, code, forked);
        let run = |args: &[&str]| {
            let mut a = vec!["wait", h.as_str()];
            a.extend_from_slice(args);
            a.push("--json");
            envelope(&b.glen(&a).output().unwrap(), exit)
        };
        let ctx = format!("{state:?} {code:?} forked={forked}");
        let with = run(&["--for", "reply", "--after", "r1"]);
        let plain = run(&[]);
        if exit == 0 || exit == 11 {
            assert_eq!(ids(&with["data"]["replies"]), ["r2"], "{ctx}");
            assert_eq!(with["data"]["job"]["task_state"], task_state, "{ctx}");
            assert!(with["data"].get("woke").is_none(), "{ctx}");
            // Plain wait is unchanged: every reply of the result, no --for keys.
            assert_eq!(ids(&plain["data"]["replies"]), ["r1", "r2"], "{ctx}");
        } else {
            let details = &with["error"]["details"];
            assert_eq!(ids(&details["replies"]), ["r2"], "{ctx}");
            assert_eq!(details["job"]["task_state"], task_state, "{ctx}");
            assert_eq!(details["status"], json!(state.as_str()), "{ctx}");
            if exit == 6 {
                assert_eq!(with["error"]["code"], "no_route", "{ctx}");
                assert_eq!(details["reason"], "live_owner", "{ctx}");
            } else {
                assert_eq!(with["error"]["code"], "job_failed", "{ctx}");
            }
            let mut without = with.clone();
            without["error"]["details"]
                .as_object_mut()
                .unwrap()
                .remove("replies");
            assert_eq!(
                without, plain,
                "{ctx}: --for reply only adds details.replies"
            );
        }
        // Without --after every reply is unseen.
        let all = run(&["--for", "reply"]);
        let replies = if exit == 0 || exit == 11 {
            &all["data"]["replies"]
        } else {
            &all["error"]["details"]["replies"]
        };
        assert_eq!(ids(replies), ["r1", "r2"], "{ctx}");
    }
}

#[test]
fn task_state_in_jobs_wait_and_error_details() {
    let b = RpcBox::new();
    let rows = [
        (State::Queued, None, false, "submitted"),
        (State::Accepted, None, false, "working"),
        (State::Completed, None, false, "completed"),
        (State::Completed, None, true, "completed"),
        (State::Aborted, Some("cancelled"), false, "canceled"),
        (State::Aborted, Some("session_switched"), false, "canceled"),
        (State::Failed, Some("no_route"), false, "rejected"),
        (State::Failed, Some("omp_error"), false, "failed"),
        (State::Failed, None, false, "failed"),
        (State::Lost, None, false, "failed"),
    ];
    let mut want = std::collections::BTreeMap::new();
    for (state, code, forked, task_state) in rows {
        let (_, h) = craft(&b, state, code, forked);
        want.insert(h, task_state);
    }
    let jobs = envelope(&b.glen(&["jobs", "-n", "0", "--json"]).output().unwrap(), 0);
    let got: std::collections::BTreeMap<String, &str> = jobs["data"]["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| {
            (
                j["handle"].as_str().unwrap().to_owned(),
                j["task_state"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(got, want);
    // The same field in wait (running and terminal) and in error.details.job.
    for (h, task_state) in &want {
        let o = b
            .glen(&["wait", h, "--timeout", "0", "--json"])
            .output()
            .unwrap();
        let v = json(&o);
        let job = if v["ok"] == true {
            &v["data"]["job"]
        } else {
            &v["error"]["details"]["job"]
        };
        assert_eq!(job["task_state"], *task_state, "{v}");
    }
}

// ---------------------------------------------------------------------------------------------
// glen inbox --wait
// ---------------------------------------------------------------------------------------------

const BETA_D: &str = "01a100c6-f000-7b18-8e6a-5e8b1c4f9a72";

fn record(session: &str, text: &str) -> Value {
    json!({"v":1,"id":format!("r-{text}"),"session_id":session,"job_id":null,"request_id":null,"tool_call_id":null,"text":text,"final":true,"at":"2026-11-02T00:00:00Z"})
}
/// Appends one complete line and returns the instant it was on disk.
fn append(b: &OmpBox, session: &str, r: &Value) -> Instant {
    let p = b.glen_home.join("inbox").join(format!("{session}.ndjson"));
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
        .unwrap();
    f.write_all(format!("{r}\n").as_bytes()).unwrap();
    Instant::now()
}
fn marker(b: &OmpBox) -> std::path::PathBuf {
    b.glen_home.join("inbox/.read")
}
fn inbox_wait(b: &OmpBox, flags: &[&str]) -> Command {
    let mut a = vec!["inbox", "--wait", "--json"];
    a.extend_from_slice(flags);
    b.command(&a, &b.alpha())
}

#[test]
fn inbox_wait_wakes_on_a_new_message_and_marks_it_after_output() {
    for scope in ["session", "cwd"] {
        let b = OmpBox::new();
        let alpha = b.alpha().to_str().unwrap().to_owned();
        let flags: Vec<&str> = match scope {
            "session" => vec!["--session", A_ID, "--timeout", "20s"],
            _ => vec!["--cwd", &alpha, "--timeout", "20s"],
        };
        let waiter = spawn_timed(inbox_wait(&b, &flags));
        std::thread::sleep(Duration::from_millis(300));
        let appended = append(&b, A_ID, &record(A_ID, "hello"));
        let (out, exited) = waiter.join().unwrap();
        let latency = exited.saturating_duration_since(appended);
        assert!(
            latency <= WAKE,
            "{scope}: woke {latency:?} after the append"
        );
        let d = &envelope(&out, 0)["data"];
        assert_eq!(d["total"], 1, "{scope}: {d}");
        assert_eq!(d["marked_read"], true);
        assert_eq!(d["messages"][0]["text"], "hello");
        assert!(
            d.get("next").is_none() && d.get("interrupted").is_none(),
            "{d}"
        );
        assert!(marker(&b).join(A_ID).exists(), "{scope}: marker written");
    }
}

#[test]
fn inbox_wait_with_cwd_ignores_other_projects_and_peek_marks_nothing() {
    let b = OmpBox::new();
    let alpha = b.alpha().to_str().unwrap().to_owned();
    append(&b, BETA_D, &record(BETA_D, "beta"));
    let started = Instant::now();
    let out = inbox_wait(&b, &["--cwd", &alpha, "--timeout", "500ms"])
        .output()
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(500));
    let d = &envelope(&out, 10)["data"];
    assert_eq!(d["messages"], json!([]));
    assert_eq!(
        d["next"],
        format!("glen inbox --wait --timeout 500ms --cwd {alpha}")
    );
    // A message already unread returns at once, even with --timeout 0; --peek marks nothing.
    append(&b, A_ID, &record(A_ID, "alpha"));
    let d = &envelope(
        &inbox_wait(&b, &["--cwd", &alpha, "--peek", "--timeout", "0"])
            .output()
            .unwrap(),
        0,
    )["data"];
    assert_eq!(texts(&d["messages"]), ["alpha"]);
    assert_eq!(d["marked_read"], false);
    assert!(!marker(&b).exists());
}

#[test]
fn inbox_wait_bound_document_marks_nothing_and_repeats_the_flags() {
    let b = OmpBox::new();
    // A malformed line is skipped but, unlike a plain inbox, never marked at the bound.
    let p = b.glen_home.join("inbox").join(format!("{A_ID}.ndjson"));
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, "not json\n").unwrap();
    let out = inbox_wait(&b, &["--timeout", "300ms", "--session", "c0de", "--peek"])
        .output()
        .unwrap();
    let v = envelope(&out, 10);
    assert_eq!(
        v["data"],
        json!({"total":0,"marked_read":false,"skipped_lines":1,"messages":[],
            "next":"glen inbox --wait --timeout 300ms --session c0de --peek","interrupted":false})
    );
    let out = inbox_wait(&b, &["--timeout", "300ms"]).output().unwrap();
    assert_eq!(envelope(&out, 10)["data"]["skipped_lines"], 1);
    assert!(!marker(&b).exists());
    let human = b
        .command(
            &["inbox", "--wait", "--timeout", "1ms", "--human"],
            &b.alpha(),
        )
        .output()
        .unwrap();
    assert_eq!(human.status.code(), Some(10));
    assert_eq!(
        String::from_utf8(human.stdout).unwrap(),
        "no messages yet · glen inbox --wait --timeout 1ms\n"
    );
}

#[test]
fn inbox_wait_sigint_is_exit_130_with_interrupted() {
    use std::os::unix::process::CommandExt;
    let b = OmpBox::new();
    // The handler is installed before the first check, with no readiness signal of its own.
    // A test runner started in the background passes SIGINT on ignored, which would silently
    // drop an early signal, so the child starts with the default disposition: a signal that
    // lands before the handler then ends glen by that disposition, and the test tries later.
    for attempt in 1..=5u64 {
        let mut cmd = inbox_wait(&b, &["--timeout", "30s"]);
        // SAFETY: between fork and exec the closure only calls signal(2), which is
        // async-signal-safe, and touches no memory of the parent.
        unsafe {
            cmd.pre_exec(|| {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                Ok(())
            });
        }
        let child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300 * attempt));
        rustix::process::kill_process(
            rustix::process::Pid::from_child(&child),
            rustix::process::Signal::INT,
        )
        .unwrap();
        let out = child.wait_with_output().unwrap();
        if out.status.code().is_none() {
            continue;
        }
        let d = &envelope(&out, 130)["data"];
        assert_eq!(d["interrupted"], true);
        assert_eq!(d["messages"], json!([]));
        assert_eq!(d["next"], "glen inbox --wait --timeout 30s");
        assert!(!marker(&b).exists());
        return;
    }
    panic!("inbox --wait never reached its SIGINT handler");
}

#[test]
fn inbox_wait_usage_errors() {
    let b = OmpBox::new();
    for args in [
        vec!["inbox", "--wait", "--timeout", "25h", "--json"],
        vec!["inbox", "--timeout", "5s", "--json"],
        vec!["inbox", "--wait", "-f", "--json"],
        vec!["inbox", "--wait", "--all", "--json"],
    ] {
        let out = b.glen(&args, &b.alpha());
        let v = envelope(&out, 2);
        assert_eq!(v["error"]["code"], "usage", "{args:?}");
    }
    assert!(!b.glen_home.exists());
}

#[test]
fn inbox_wait_with_a_closed_stdout_keeps_messages_unread() {
    let b = OmpBox::new();
    append(&b, A_ID, &record(A_ID, "keep"));
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = inbox_wait(&b, &["--timeout", "5s"])
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    assert!(!marker(&b).exists());
    let peek = envelope(&b.glen(&["inbox", "--peek", "--json"], &b.alpha()), 0);
    assert_eq!(peek["data"]["total"], 1);
}
