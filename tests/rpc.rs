mod common;
use common::*;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    process::{Output, Stdio},
    time::{Duration, Instant},
};
fn envelope(o: &Output, code: i32) -> Value {
    assert_eq!(o.status.code(), Some(code), "{o:?}");
    assert!(o.stderr.is_empty(), "{o:?}");
    assert!(!o.stdout.contains(&27));
    let v = json(o);
    assert_eq!(
        v["ok"],
        code == 0 || code == 10 || code == 11 || code == 130
    );
    v
}
fn send(b: &RpcBox, scenario: &str, more: &[&str], code: i32) -> Value {
    let mut args = vec!["send", "e27c0de", "hello", "--json"];
    args.extend(more);
    let o = b
        .glen(&args)
        .env("FAKE_OMP_SCENARIO", scenario)
        .output()
        .unwrap();
    let v = envelope(&o, code);
    let id = job_id(&v);
    b.wait_worker_gone(&id);
    v
}
fn job_id(v: &Value) -> String {
    v["data"]["job"]["id"]
        .as_str()
        .or_else(|| v["error"]["details"]["job"]["id"].as_str())
        .unwrap()
        .into()
}
fn launch(b: &RpcBox, scenario: &str, delay: &str) -> Value {
    let o = b
        .glen(&["send", "e27c0de", "hello", "--no-wait", "--json"])
        .env("FAKE_OMP_SCENARIO", scenario)
        .env("FAKE_OMP_DELAY_MS", delay)
        .output()
        .unwrap();
    envelope(&o, 10)
}
fn wait(b: &RpcBox, v: &Value, code: i32) -> Value {
    let id = job_id(v);
    let o = b
        .glen(&["wait", &id, "--timeout", "25s", "--json"])
        .output()
        .unwrap();
    let v = envelope(&o, code);
    b.wait_worker_gone(&id);
    v
}
fn accepted(b: &RpcBox, id: &str) {
    poll(Duration::from_secs(5), || {
        let s = b.job(id).1;
        (s["status"] == "accepted").then_some(())
    });
}
fn control(b: &RpcBox, id: &str, name: &str, value: Value) {
    let home = glen::glen_home::GlenHome {
        root: b.omp.glen_home.clone(),
    };
    let dir = home.ensure_dir(&format!("jobs/{id}/control")).unwrap();
    glen::glen_home::write_private_atomic(
        &dir.join(format!("{name}.json")),
        &serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}
#[test]
fn ac202_spawns_omp_with_resume_no_ui_and_managed_env() {
    let b = RpcBox::new();
    let v = send(&b, "ok", &[], 0);
    let id = job_id(&v);
    let (j, s) = b.job(&id);
    let start = b
        .fake_log()
        .into_iter()
        .find(|v| v["event"] == "start")
        .unwrap();
    assert_eq!(
        start["argv"],
        json!(["--mode", "rpc", "--no-ui", "--resume", b.omp.session('A')])
    );
    assert_eq!(start["env"]["GLEN_MANAGED"], "1");
    assert_eq!(start["cwd"], json!(b.omp.alpha()));
    assert!(s["worker_start"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(j["owners_dir"], json!(b.owners()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for (p, mode) in [
            (b.job_dir(&id), 0o700),
            (b.job_dir(&id).join("job.json"), 0o600),
            (b.job_dir(&id).join("status.json"), 0o600),
            (b.job_dir(&id).join("worker.log"), 0o600),
        ] {
            assert_eq!(fs::metadata(p).unwrap().permissions().mode() & 0o777, mode);
        }
    }
    let b = RpcBox::new();
    let p = b.omp.session('A');
    let text = fs::read_to_string(&p)
        .unwrap()
        .replace("/home/fixture/work/alpha", b.omp.beta().to_str().unwrap());
    fs::write(&p, text).unwrap();
    send(&b, "ok", &[], 0);
    assert_eq!(
        b.fake_log().iter().find(|v| v["event"] == "start").unwrap()["cwd"],
        json!(b.omp.beta())
    );
}
#[test]
fn w2_latest_cwd_falls_back_and_is_recorded() {
    for header_cwd in [None, Some("missing"), Some("file"), Some("valid")] {
        let b = RpcBox::new();
        let path = b.omp.session('A');
        let text = fs::read_to_string(&path).unwrap();
        let text = text
            .lines()
            .map(|line| {
                let Ok(mut value) = serde_json::from_str::<Value>(line) else {
                    return line.to_owned();
                };
                if value["type"] == "session" {
                    value.as_object_mut().unwrap().remove("cwd");
                    if let Some(kind) = header_cwd {
                        value["cwd"] = json!(match kind {
                            "missing" => b.omp.home.join("missing"),
                            "file" => path.clone(),
                            _ => b.omp.beta(),
                        });
                    }
                }
                serde_json::to_string(&value).unwrap()
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&path, &text).unwrap();
        let out = b
            .glen(&[
                "send",
                "@latest",
                "hello",
                "--cwd",
                b.omp.alpha().to_str().unwrap(),
                "--json",
            ])
            .current_dir(&b.omp.home)
            .output()
            .unwrap();
        let value = envelope(&out, 0);
        let id = job_id(&value);
        b.wait_worker_gone(&id);
        let expected = if header_cwd == Some("valid") {
            b.omp.beta()
        } else {
            b.omp.alpha()
        };
        assert_eq!(b.job(&id).0["session_id"], A_ID);
        assert_eq!(b.job(&id).0["cwd"], json!(expected));
        let start = b
            .fake_log()
            .into_iter()
            .find(|v| v["event"] == "start")
            .unwrap();
        assert_eq!(start["cwd"], json!(expected));
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }
}
#[test]
fn ac203_send_completes_with_reply() {
    let b = RpcBox::new();
    let before = fs::read(b.omp.session('A')).unwrap();
    let v = send(&b, "ok", &[], 0);
    let d = &v["data"];
    assert_eq!(d["status"], "completed");
    assert_eq!(d["assistant_text"], "pong: hello");
    assert_eq!(d["replies"][0]["text"], "reply: hello");
    assert_eq!(d["replies"][0]["final"], true);
    assert_eq!(d["replies"][0].get("request_id"), Some(&Value::Null));
    assert_eq!(d["session_settled"], true);
    assert_eq!(d["delivered_as"], "prompt");
    assert!(d["forked"].is_null());
    let id = job_id(&v);
    let replies = fs::read_to_string(b.job_dir(&id).join("replies.ndjson")).unwrap();
    assert_eq!(
        replies,
        fs::read_to_string(b.omp.glen_home.join("inbox").join(format!("{A_ID}.ndjson"))).unwrap()
    );
    assert_eq!(
        serde_json::from_str::<Value>(replies.trim()).unwrap(),
        d["replies"][0]
    );
    let result: Value =
        serde_json::from_slice(&fs::read(b.job_dir(&id).join("result.json")).unwrap()).unwrap();
    assert_eq!(result["replies"], d["replies"]);
    let tools = &b.frames_in("set_host_tools")[0]["tools"];
    assert_eq!(tools.as_array().unwrap().len(), 1);
    assert_eq!(tools[0]["name"], "glen_reply");
    assert_eq!(tools[0]["parameters"]["additionalProperties"], false);
    assert_eq!(b.frames_in("prompt")[0]["streamingBehavior"], "followUp");
    assert_eq!(before, fs::read(b.omp.session('A')).unwrap());
    assert!(!b.omp.session('A').with_extension("jsonl.lock.os").exists());
}
#[test]
fn ac204_bounded_wait_then_wait() {
    let b = RpcBox::new();
    let release = b.omp.home.join("release-run");
    let o = b
        .glen(&["send", "e27c0de", "hello", "--wait", "300ms", "--json"])
        .env("FAKE_OMP_RUN_RELEASE", &release)
        .output()
        .unwrap();
    let v = envelope(&o, 10);
    assert_eq!(v["data"]["status"], "running");
    accepted(&b, &job_id(&v));
    assert_eq!(b.job(&job_id(&v)).1["status"], "accepted");
    fs::write(release, "go").unwrap();
    wait(&b, &v, 0);
    let start = Instant::now();
    let v = launch(&b, "ok", "1500");
    assert!(start.elapsed() < Duration::from_secs(2));
    wait(&b, &v, 0);
}
#[test]
fn ac206_fork_detected() {
    for scenario in ["fork", "fork_silent"] {
        let b = RpcBox::new();
        let v = send(&b, scenario, &[], 11);
        assert_eq!(v["data"]["forked"]["from"], A_ID);
        assert!(
            v["data"]["forked"]["to"]
                .as_str()
                .unwrap()
                .ends_with("f0f0")
        );
        assert!(b.frames_in("get_state").len() >= 2);
    }
}
#[test]
fn ac207_live_owner_refused() {
    let b = RpcBox::new();
    let mut owner = b.hold_lease(A_ID);
    envelope(
        &b.glen(&["send", "e27c0de", "hi", "--json"])
            .output()
            .unwrap(),
        6,
    );
    assert!(!b.omp.glen_home.exists());
    assert!(b.fake_log().is_empty());
    drop(owner.stdin.take());
    owner.wait().unwrap();
}
#[test]
fn ac207_worker_probe_before_spawn() {
    let b = RpcBox::new();
    let lock = b.hold_glen_lock(A_ID);
    let v = launch(&b, "ok", "0");
    let mut owner = b.hold_lease(A_ID);
    drop(lock);
    let out = wait(&b, &v, 6);
    assert_eq!(out["error"]["details"]["error"]["code"], "no_route");
    assert!(b.fake_log().is_empty());
    drop(owner.stdin.take());
    owner.wait().unwrap();
}
#[test]
fn ac207_startup_recovery_lease_does_not_refuse_our_child() {
    let b = RpcBox::new();
    let before = fs::read(b.omp.session('A')).unwrap();
    let o = b
        .glen(&["send", "e27c0de", "hi", "--json"])
        .env("FAKE_OMP_STARTUP_RECOVERY", "1")
        .env("FAKE_OMP_OWNERS_DIR", b.owners())
        .output()
        .unwrap();
    let v = envelope(&o, 0);
    b.wait_worker_gone(&job_id(&v));
    assert!(fs::read(b.omp.session('A')).unwrap().starts_with(&before));
    assert!(
        b.fake_log()
            .iter()
            .any(|v| v["event"] == "startup_recovery")
    );
    assert!(!b.frames_in("set_host_tools").is_empty());
    assert_eq!(b.frames_in("prompt").len(), 1);
    assert!(v["data"]["forked"].is_null());
}
#[test]
fn ac208_queue_serialises() {
    let b = RpcBox::new();
    let release = b.omp.home.join("release-run");
    let launch = || {
        let o = b
            .glen(&["send", "e27c0de", "hi", "--no-wait", "--json"])
            .env("FAKE_OMP_RUN_RELEASE", &release)
            .env("FAKE_OMP_OWNERS_DIR", b.owners())
            .output()
            .unwrap();
        envelope(&o, 10)
    };
    let one = launch();
    accepted(&b, &job_id(&one));
    let two = launch();
    assert_eq!(two["data"]["job"]["status"], "queued");
    fs::write(release, "go").unwrap();
    wait(&b, &one, 0);
    wait(&b, &two, 0);
    let log = b.fake_log();
    let start: Vec<_> = log
        .iter()
        .enumerate()
        .filter(|(_, v)| v["event"] == "start")
        .map(|(i, _)| i)
        .collect();
    let exit = log.iter().position(|v| v["event"] == "exit").unwrap();
    assert!(start[1] > exit);
}
#[test]
fn ac209_new_session_and_profile() {
    for flag in [false, true] {
        let b = RpcBox::new();
        let cwd = b.omp.alpha();
        let mut args = vec![
            "send",
            "@new",
            "hi",
            "--cwd",
            cwd.to_str().unwrap(),
            "--json",
        ];
        if flag {
            args.extend(["--profile", "work"]);
        }
        let o = b
            .glen(&args)
            .env("OMP_PROFILE", "inherited")
            .output()
            .unwrap();
        let v = envelope(&o, 0);
        b.wait_worker_gone(&job_id(&v));
        let (j, s) = b.job(&job_id(&v));
        assert_eq!(j["route"], "new");
        let id = s["actual_session_id"].as_str().unwrap();
        assert!(
            b.omp
                .glen_home
                .join("run/locks")
                .join(format!("{id}.lock"))
                .exists()
        );
        let start = b
            .fake_log()
            .into_iter()
            .find(|v| v["event"] == "start")
            .unwrap();
        assert!(
            !start["argv"]
                .as_array()
                .unwrap()
                .contains(&json!("--resume"))
        );
        assert_eq!(
            start["env"]["OMP_PROFILE"],
            if flag {
                Value::Null
            } else {
                json!("inherited")
            }
        );
        if flag {
            assert_eq!(
                start["argv"],
                json!(["--mode", "rpc", "--no-ui", "--profile", "work"])
            );
        }
    }
    let b = RpcBox::new();
    envelope(
        &b.glen(&["send", "e27c0de", "hi", "--cwd", "/tmp", "--json"])
            .output()
            .unwrap(),
        2,
    );
    envelope(
        &b.glen(&["send", "@new", "hi", "--cwd", "/missing-glen-dir", "--json"])
            .output()
            .unwrap(),
        3,
    );
    assert!(!b.omp.glen_home.exists());
}
#[test]
fn ac210_bad_reply_args_are_tool_errors() {
    let b = RpcBox::new();
    let v = send(&b, "bad_reply", &[], 0);
    assert_eq!(v["data"]["replies"], json!([]));
    let frames = b.frames_in("host_tool_result");
    assert_eq!(frames.len(), 3);
    assert!(frames.iter().all(|f| f["isError"] == true));
}
#[test]
fn ac210_progress_replies_do_not_finish_the_run() {
    let b = RpcBox::new();
    let v = send(&b, "progress", &[], 0);
    assert_eq!(v["data"]["replies"].as_array().unwrap().len(), 2);
    assert_eq!(v["data"]["replies"][0]["final"], false);
    assert_eq!(v["data"]["assistant_text"], "pong: hello");
}
#[test]
fn ac211_reply_tool_rejected() {
    let b = RpcBox::new();
    let v = send(&b, "reject_tools", &[], 7);
    assert_eq!(
        v["error"]["details"]["error"]["code"],
        "reply_tool_rejected"
    );
    assert!(b.frames_in("prompt").is_empty());
}
#[test]
fn ac212_local_command() {
    let b = RpcBox::new();
    let start = Instant::now();
    let v = send(&b, "local", &[], 0);
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(v["data"]["assistant_text"].is_null());
    assert_eq!(v["data"]["command_output"], json!(["fake command output"]));
}
#[test]
fn ac213_failures() {
    for (scenario, state, code) in [
        ("error", "failed", "omp_error"),
        ("aborted", "aborted", "aborted"),
        ("crash", "failed", "omp_exited"),
    ] {
        let b = RpcBox::new();
        let v = send(&b, scenario, &[], 7);
        assert_eq!(v["error"]["details"]["status"], state);
        assert_eq!(v["error"]["details"]["error"]["code"], code);
    }
}
#[test]
fn ac214_lost_worker() {
    let b = RpcBox::new();
    let v = launch(&b, "hang", "0");
    let id = job_id(&v);
    accepted(&b, &id);
    let pid = b.job(&id).1["worker_pid"].as_u64().unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid as i32).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    let v = wait(&b, &v, 7);
    assert_eq!(v["error"]["details"]["status"], "lost");
    assert_eq!(b.job(&id).1["status"], "lost");
}
#[test]
fn ac215_deadline() {
    for scenario in ["hang", "wedge", "never_settles", "unsettled"] {
        let b = RpcBox::new();
        let o = b
            .glen(&["send", "e27c0de", "hi", "--json"])
            .env("GLEN_JOB_TIMEOUT", "2s")
            .env("FAKE_OMP_SCENARIO", scenario)
            .output()
            .unwrap();
        let code = if matches!(scenario, "never_settles" | "unsettled") {
            0
        } else {
            7
        };
        let v = envelope(&o, code);
        b.wait_worker_gone(&job_id(&v));
        if code == 7 {
            assert_eq!(v["error"]["details"]["error"]["code"], "timeout");
            assert!(!b.frames_in("abort").is_empty());
        } else {
            assert_eq!(v["data"]["session_settled"], scenario == "unsettled");
            if scenario == "never_settles" {
                assert!(
                    v["warnings"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|w| w["code"] == "settle_timeout")
                );
            } else {
                let settle = b
                    .fake_log()
                    .into_iter()
                    .find(|v| v["event"] == "out" && v["frame"]["type"] == "session_settled")
                    .unwrap()["at_ms"]
                    .as_i64()
                    .unwrap();
                let status = b.job(&job_id(&v)).1;
                assert!(
                    status["updated_at"]
                        .as_str()
                        .unwrap()
                        .parse::<jiff::Timestamp>()
                        .unwrap()
                        .as_millisecond()
                        >= settle
                );
            }
        }
    }
}
#[test]
fn ac216_teardown() {
    let b = RpcBox::new();
    send(&b, "ok", &[], 0);
    assert!(!glen::jobs::lock::is_held(
        &glen::glen_home::GlenHome {
            root: b.omp.glen_home.clone()
        },
        A_ID
    ));
}
#[test]
fn ac217_sigint_foreground_keeps_job() {
    let b = RpcBox::new();
    let release = b.omp.home.join("release-run");
    let child = b
        .glen(&["send", "e27c0de", "hi", "--wait", "30s", "--json"])
        .env("FAKE_OMP_RUN_RELEASE", &release)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let id = poll(Duration::from_secs(3), || {
        let ids = glen::jobs::JobStore::new(glen::glen_home::GlenHome {
            root: b.omp.glen_home.clone(),
        })
        .list();
        ids.first().map(|i| i.as_str().to_owned())
    });
    accepted(&b, &id);
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&child),
        rustix::process::Signal::INT,
    )
    .unwrap();
    let v = envelope(&child.wait_with_output().unwrap(), 130);
    assert_eq!(v["data"]["interrupted"], true);
    fs::write(release, "go").unwrap();
    wait(&b, &v, 0);
}
#[test]
fn ac217_sigterm_worker_aborts() {
    let b = RpcBox::new();
    let v = launch(&b, "hang", "0");
    let id = job_id(&v);
    accepted(&b, &id);
    let pid = b.job(&id).1["worker_pid"].as_u64().unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid as i32).unwrap(),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    let v = wait(&b, &v, 7);
    assert_eq!(v["error"]["details"]["status"], "aborted");
    assert_eq!(v["error"]["details"]["error"]["code"], "cancelled");
    assert!(!b.frames_in("abort").is_empty());
}
#[test]
fn ac218_control_inbox() {
    let b = RpcBox::new();
    let v = launch(&b, "hang", "0");
    let id = job_id(&v);
    accepted(&b, &id);
    control(&b, &id, "unsupported", json!({"v":1,"op":"set_model"}));
    let done = b.job_dir(&id).join("control/unsupported.done.json");
    poll(Duration::from_secs(2), || done.exists().then_some(()));
    let d: Value = serde_json::from_slice(&fs::read(done).unwrap()).unwrap();
    assert_eq!(d["error"]["code"], "unsupported_op");
    assert_eq!(b.job(&id).1["status"], "accepted");
    let start = Instant::now();
    control(&b, &id, "a", json!({"v":1,"op":"abort"}));
    b.wait_terminal(&id, Duration::from_secs(2));
    assert!(start.elapsed() < Duration::from_secs(2));
    let v = wait(&b, &v, 7);
    assert_eq!(v["error"]["details"]["error"]["code"], "cancelled");
    let d: Value =
        serde_json::from_slice(&fs::read(b.job_dir(&id).join("control/a.done.json")).unwrap())
            .unwrap();
    assert_eq!(d, json!({"v":1,"ok":true}));
}
#[test]
fn ac218_abort_while_queued() {
    let b = RpcBox::new();
    let lock = b.hold_glen_lock(A_ID);
    let v = launch(&b, "ok", "0");
    let id = job_id(&v);
    control(&b, &id, "a", json!({"v":1,"op":"abort"}));
    b.wait_terminal(&id, Duration::from_secs(2));
    let v = wait(&b, &v, 7);
    assert_eq!(v["error"]["details"]["error"]["code"], "cancelled");
    assert!(b.fake_log().is_empty());
    drop(lock);
}
#[test]
fn ac219_pipes_close_when_send_exits() {
    let b = RpcBox::new();
    let start = Instant::now();
    let v = launch(&b, "ok", "3000");
    assert!(start.elapsed() < Duration::from_secs(1));
    accepted(&b, &job_id(&v));
    assert_eq!(b.job(&job_id(&v)).1["status"], "accepted");
    wait(&b, &v, 0);
}
#[test]
fn ac222_failures_before_the_job_leave_no_state() {
    for (args, bin, code) in [
        (vec!["send", "abcdef0", "hi"], None, 3),
        (vec!["send", "j1234567", "hi"], None, 2),
        (vec!["send", "e27c0de", "hi"], Some("/nonexistent"), 5),
    ] {
        let b = RpcBox::new();
        let mut c = b.glen(&args);
        if let Some(bin) = bin {
            c.env("GLEN_OMP_BIN", bin);
        }
        envelope(&c.output().unwrap(), code);
        assert!(!b.omp.glen_home.exists());
    }
}
#[test]
fn ac224_message_rules() {
    for (bytes, code) in [
        (b"hi".to_vec(), 0),
        (vec![], 2),
        (vec![b'x'; 600 * 1024], 2),
        (vec![255], 2),
    ] {
        let b = RpcBox::new();
        let mut child = b
            .glen(&["send", "e27c0de", "-", "--json"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let written = child.stdin.take().unwrap().write_all(&bytes);
        if let Err(e) = written {
            assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe);
        }
        let v = envelope(&child.wait_with_output().unwrap(), code);
        if code == 0 {
            b.wait_worker_gone(&job_id(&v));
            assert_eq!(v["data"]["assistant_text"], "pong: hi");
        } else {
            assert!(!b.omp.glen_home.exists());
        }
    }
    let b = RpcBox::new();
    let v = send(&b, "ok", &["--steer"], 0);
    assert_eq!(b.frames_in("prompt")[0]["streamingBehavior"], "steer");
    assert_eq!(v["data"]["delivered_as"], "prompt");
}
#[test]
fn ac225_session_moved() {
    let b = RpcBox::new();
    let v = send(&b, "moved", &[], 7);
    assert_eq!(v["error"]["details"]["error"]["code"], "session_moved");
    assert!(b.frames_in("prompt").is_empty());
}
#[test]
fn ac225_queued_job_follows_continuation() {
    let b = RpcBox::new();
    let lock = b.hold_glen_lock(A_ID);
    let v = launch(&b, "ok", "0");
    let new = "01a0f6b2-2680-7a3c-8b21-4a1f9e27c0df";
    let mut content = fs::read_to_string(b.omp.session('A')).unwrap();
    let mut lines: Vec<Value> = content
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let header = lines.iter_mut().find(|v| v["type"] == "session").unwrap();
    header["id"] = json!(new);
    header["continuationOf"] = json!(A_ID);
    lines.push(json!({"type":"message","id":"later","timestamp":"2026-10-30T00:00:00Z","message":{"role":"user","content":"new"}}));
    content = lines.iter().map(|v| v.to_string() + "\n").collect();
    let p = b
        .omp
        .session('A')
        .parent()
        .unwrap()
        .join(format!("2026-10-30T00-00-00-000Z_{new}.jsonl"));
    fs::write(p, content).unwrap();
    drop(lock);
    let v = wait(&b, &v, 7);
    assert_eq!(v["error"]["details"]["error"]["code"], "session_moved");
    assert_eq!(v["error"]["details"]["error"]["details"]["to"], new);
    assert!(b.fake_log().is_empty());
}
#[test]
fn ac226_event_filter() {
    let b = RpcBox::new();
    send(&b, "ok", &[], 0);
    let f = &b.frames_in("set_event_filter")[0]["events"];
    assert!(f.as_array().unwrap().contains(&json!("message_end")));
    for excluded in [
        "message_update",
        "tool_execution_update",
        "tool_stream_update",
    ] {
        assert!(!f.as_array().unwrap().contains(&json!(excluded)));
    }
}
#[test]
fn ac227_json_and_human_modes() {
    let b = RpcBox::new();
    let o = b
        .glen(&["send", "e27c0de", "hello", "--human"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{o:?}");
    assert!(o.stderr.is_empty());
    let human = String::from_utf8(o.stdout).unwrap();
    assert!(human.contains("✓ completed"));
    assert!(human.contains("pong: hello"));
    let id = glen::jobs::JobStore::new(glen::glen_home::GlenHome {
        root: b.omp.glen_home.clone(),
    })
    .list()[0]
        .clone();
    b.wait_worker_gone(id.as_str());
    let o = b
        .glen(&["send", "e27c0de", "hi", "--human", "--no-wait"])
        .env("FAKE_OMP_DELAY_MS", "500")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(10));
    assert!(String::from_utf8(o.stdout).unwrap().contains("glen wait j"));
    let id = glen::jobs::JobStore::new(glen::glen_home::GlenHome {
        root: b.omp.glen_home.clone(),
    })
    .list()[0]
        .clone();
    b.wait_worker_gone(id.as_str());
    let o = b
        .glen(&["send", "e27c0de", "hi", "--json"])
        .env("GLEN_MAX_OUTPUT", "2000")
        .env("FAKE_OMP_REPLY_BYTES", "40000")
        .output()
        .unwrap();
    let v = envelope(&o, 0);
    assert!(v["spilled"]["path"].as_str().is_some());
    let id = glen::jobs::JobStore::new(glen::glen_home::GlenHome {
        root: b.omp.glen_home.clone(),
    })
    .list()[0]
        .clone();
    b.wait_worker_gone(id.as_str());
}
#[test]
fn v2_negotiation() {
    for advertisement in ["1", "partial"] {
        let b = RpcBox::new();
        let o = b
            .glen(&["send", "e27c0de", "hi", "--json"])
            .env("FAKE_OMP_V2", advertisement)
            .output()
            .unwrap();
        let v = envelope(&o, 0);
        b.wait_worker_gone(&job_id(&v));
        assert_eq!(
            !b.frames_in("negotiate_protocol").is_empty(),
            advertisement == "1"
        );
    }
}
#[test]
fn ac202_relative_owners_dir_is_frozen() {
    let b = RpcBox::new();
    let o = b
        .glen(&["send", "e27c0de", "hello", "--json"])
        .env("PI_CONFIG_DIR", "relative-omp")
        .output()
        .unwrap();
    let v = envelope(&o, 0);
    b.wait_worker_gone(&job_id(&v));
    assert_eq!(
        b.job(&job_id(&v)).0["owners_dir"],
        json!(b.omp.home.join("relative-omp/run/session-owners"))
    );
}

#[test]
fn ac218_abort_while_starting() {
    let b = RpcBox::new();
    let out = b
        .glen(&["send", "e27c0de", "hi", "--no-wait", "--json"])
        .env("FAKE_OMP_READY_DELAY_MS", "5000")
        .output()
        .unwrap();
    let value = envelope(&out, 10);
    let id = job_id(&value);
    let pid = poll(Duration::from_secs(3), || {
        b.fake_log()
            .into_iter()
            .find(|v| v["event"] == "start")
            .and_then(|v| v["pid"].as_u64())
    });
    // The explicit start event is the readiness signal: omp is blocked before its ready frame.
    let started = Instant::now();
    control(&b, &id, "a", json!({"v":1,"op":"abort"}));
    let status = b.wait_terminal(&id, Duration::from_millis(1500));
    poll(Duration::from_millis(1500), || {
        b.job_dir(&id)
            .join("control/a.done.json")
            .exists()
            .then_some(())
    });
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert_eq!(status["status"], "aborted");
    assert_eq!(status["error"]["code"], "cancelled");
    let done: Value =
        serde_json::from_slice(&fs::read(b.job_dir(&id).join("control/a.done.json")).unwrap())
            .unwrap();
    assert_eq!(done, json!({"v":1,"ok":true}));
    b.wait_worker_gone(&id);
    assert!(b.frames_in("prompt").is_empty());
    assert!(b.frames_in("set_host_tools").is_empty());
    assert_eq!(
        rustix::process::test_kill_process_group(
            rustix::process::Pid::from_raw(pid as i32).unwrap()
        ),
        Err(rustix::io::Errno::SRCH)
    );
}

#[test]
fn w2_ready_bound_before_deadline_is_omp_unavailable() {
    let b = RpcBox::new();
    let out = b
        .glen(&["send", "e27c0de", "hi", "--no-wait", "--json"])
        .env("GLEN_JOB_TIMEOUT", "61s")
        .env("FAKE_OMP_READY_DELAY_MS", "120000")
        .output()
        .unwrap();
    let value = envelope(&out, 10);
    let id = job_id(&value);
    let status = b.wait_terminal(&id, Duration::from_secs(75));
    assert_eq!(status["status"], "failed");
    assert_eq!(status["error"]["code"], "omp_unavailable");
    b.wait_worker_gone(&id);
    assert!(b.frames_in("prompt").is_empty());
    assert!(b.frames_in("set_host_tools").is_empty());
}
#[test]
fn ac215_deadline_while_starting() {
    for scenario in ["ok", "no_ready"] {
        let b = RpcBox::new();
        let out = b
            .glen(&["send", "e27c0de", "hi", "--no-wait", "--json"])
            .env("GLEN_JOB_TIMEOUT", "500ms")
            .env("FAKE_OMP_SCENARIO", scenario)
            .env(
                "FAKE_OMP_READY_DELAY_MS",
                if scenario == "ok" { "5000" } else { "0" },
            )
            .output()
            .unwrap();
        let value = envelope(&out, 10);
        let id = job_id(&value);
        let status = b.wait_terminal(&id, Duration::from_secs(2));
        assert_eq!(status["status"], "failed");
        assert_eq!(status["error"]["code"], "timeout");
        b.wait_worker_gone(&id);
        assert!(b.frames_in("prompt").is_empty());
        for start in b.fake_log().iter().filter(|v| v["event"] == "start") {
            let pid =
                rustix::process::Pid::from_raw(start["pid"].as_u64().unwrap() as i32).unwrap();
            assert_eq!(
                rustix::process::test_kill_process_group(pid),
                Err(rustix::io::Errno::SRCH)
            );
        }
    }
}
#[test]
fn ac226_rejected_filter_warns_and_continues() {
    let b = RpcBox::new();
    let value = send(&b, "reject_filter", &[], 0);
    assert!(
        value["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "event_filter_rejected")
    );
    assert_eq!(value["data"]["assistant_text"], "pong: hello");
}
#[test]
fn ac202_latest_scope_and_continuation() {
    let b = RpcBox::new();
    for (target, expected) in [
        ("f13a5b6", b.omp.session('C')),
        ("@latest", b.omp.session('G')),
    ] {
        let cwd = b.omp.alpha();
        let mut args = vec!["send", target, "hi", "--json"];
        if target == "@latest" {
            args.extend(["--cwd", cwd.to_str().unwrap()]);
        }
        let out = b.glen(&args).output().unwrap();
        let value = envelope(&out, 0);
        b.wait_worker_gone(&job_id(&value));
        assert_eq!(value["data"]["session"]["path"], json!(expected));
    }
}

#[test]
fn review1_abort_in_prompt_submission_gap_never_sends_prompt() {
    let b = RpcBox::new();
    let o = b
        .glen(&["send", "e27c0de", "hi", "--no-wait", "--json"])
        .env("FAKE_OMP_PROMPT_SUBMIT_DELAY_MS", "5000")
        .output()
        .unwrap();
    let v = envelope(&o, 10);
    let id = job_id(&v);
    poll(Duration::from_secs(5), || {
        fs::read_to_string(b.job_dir(&id).join("worker.log"))
            .ok()
            .filter(|s| s.contains("prompt: sending message"))
    });
    let start = Instant::now();
    control(&b, &id, "gap", json!({"v":1,"op":"abort"}));
    let s = b.wait_terminal(&id, Duration::from_millis(1500));
    assert!(start.elapsed() < Duration::from_millis(1500));
    assert_eq!(s["status"], "aborted");
    assert_eq!(s["error"]["code"], "cancelled");
    wait(&b, &v, 7);
    assert!(b.frames_in("prompt").is_empty());
    let done: Value =
        serde_json::from_slice(&fs::read(b.job_dir(&id).join("control/gap.done.json")).unwrap())
            .unwrap();
    assert_eq!(done, json!({"v":1,"ok":true}));
}
#[test]
fn review4_output_immediately_before_ack_is_preserved() {
    let b = RpcBox::new();
    // Force the ack to be ready before the worker resumes reading its event receiver.
    for _ in 0..8 {
        let v = send(&b, "local", &[], 0);
        assert_eq!(v["data"]["command_output"], json!(["fake command output"]));
        let stored: Value =
            serde_json::from_slice(&fs::read(b.job_dir(&job_id(&v)).join("result.json")).unwrap())
                .unwrap();
        assert_eq!(stored["command_output"], json!(["fake command output"]));
    }
}
#[test]
fn review5_delayed_state_query_does_not_delay_abort() {
    let b = RpcBox::new();
    let o = b
        .glen(&["send", "e27c0de", "hi", "--no-wait", "--json"])
        .env("FAKE_OMP_SCENARIO", "delayed_state")
        .env("FAKE_OMP_STATE_DELAY_MS", "5000")
        .output()
        .unwrap();
    let v = envelope(&o, 10);
    let id = job_id(&v);
    poll(Duration::from_secs(5), || {
        (b.frames_in("get_state").len() >= 2).then_some(())
    });
    let start = Instant::now();
    control(&b, &id, "slow-state", json!({"v":1,"op":"abort"}));
    let s = b.wait_terminal(&id, Duration::from_millis(1500));
    assert_eq!(s["status"], "aborted");
    assert_eq!(s["error"]["code"], "cancelled");
    assert!(start.elapsed() < Duration::from_millis(1500));
    wait(&b, &v, 7);
    assert!(!b.frames_in("abort").is_empty());
}
#[test]
fn review6_stdin_routing_reprobes_after_predecessor_releases_lease() {
    let b = RpcBox::new();
    let o = b
        .glen(&["send", "e27c0de", "first", "--no-wait", "--json"])
        .env("FAKE_OMP_DELAY_MS", "2000")
        .env("FAKE_OMP_OWNERS_DIR", b.owners())
        .output()
        .unwrap();
    let first = envelope(&o, 10);
    accepted(&b, &job_id(&first));
    assert!(matches!(
        glen::omp::lease::probe(&b.owners(), A_ID),
        glen::omp::lease::Lease::Held
    ));
    let mut second = b
        .glen(&["send", "e27c0de", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait(&b, &first, 0);
    second.stdin.take().unwrap().write_all(b"second").unwrap();
    let v = envelope(&second.wait_with_output().unwrap(), 0);
    b.wait_worker_gone(&job_id(&v));
    assert_eq!(v["data"]["assistant_text"], "pong: second");
}
#[test]
fn review8_queue_marker_distinguishes_waiting_from_startup() {
    let b = RpcBox::new();
    let lock = b.hold_glen_lock(A_ID);
    let release = b.omp.home.join("release-queued-startup");
    let o = b
        .glen(&["send", "e27c0de", "hi", "--no-wait", "--json"])
        .env("FAKE_OMP_READY_RELEASE", &release)
        .output()
        .unwrap();
    let v = envelope(&o, 10);
    let id = job_id(&v);
    poll(Duration::from_secs(5), || {
        b.job(&id).1["waited_ms"].as_u64().filter(|n| *n > 0)
    });
    assert_eq!(b.job(&id).1["status"], "queued");
    assert_eq!(b.job(&id).1["waiting_for_lock"], true);
    drop(lock);
    poll(Duration::from_secs(5), || {
        (!b.fake_log().is_empty()).then_some(())
    });
    assert_eq!(b.job(&id).1["status"], "queued");
    assert_ne!(b.job(&id).1["waiting_for_lock"], true);
    fs::write(release, "ready").unwrap();
    wait(&b, &v, 0);
    assert!(b.job(&id).1["waited_ms"].is_u64());
    assert_ne!(b.job(&id).1["waiting_for_lock"], true);
    let b = RpcBox::new();
    let release = b.omp.home.join("release-ready");
    let o = b
        .glen(&["send", "e27c0de", "hi", "--no-wait", "--json"])
        .env("FAKE_OMP_READY_RELEASE", &release)
        .output()
        .unwrap();
    let v = envelope(&o, 10);
    let id = job_id(&v);
    poll(Duration::from_secs(5), || {
        (!b.fake_log().is_empty()).then_some(())
    });
    assert_eq!(b.job(&id).1["status"], "queued");
    assert_ne!(b.job(&id).1["waiting_for_lock"], true);
    fs::write(release, "ready").unwrap();
    wait(&b, &v, 0);
}
#[test]
fn review9_abort_after_completed_result_preserves_answer() {
    let b = RpcBox::new();
    let v = launch(&b, "never_settles", "0");
    let id = job_id(&v);
    poll(Duration::from_secs(5), || {
        b.fake_log()
            .iter()
            .any(|v| v["event"] == "out" && v["frame"]["type"] == "prompt_result")
            .then_some(())
    });
    control(&b, &id, "settle", json!({"v":1,"op":"abort"}));
    let v = wait(&b, &v, 0);
    assert_eq!(v["data"]["assistant_text"], "pong: hello");
    assert_eq!(v["data"]["session_settled"], false);
    assert!(
        v["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "settle_cancelled")
    );
    assert!(b.frames_in("abort").is_empty());
}
#[test]
fn review10_local_result_has_bounded_settle_wait() {
    let b = RpcBox::new();
    let start = Instant::now();
    let o = b
        .glen(&["send", "e27c0de", "hi", "--json"])
        .env("GLEN_JOB_TIMEOUT", "60m")
        .env("FAKE_OMP_SCENARIO", "local_result_unsettled")
        .output()
        .unwrap();
    let v = envelope(&o, 0);
    assert!(start.elapsed() < Duration::from_secs(15));
    b.wait_worker_gone(&job_id(&v));
    assert_eq!(v["data"]["session_settled"], false);
    assert!(
        v["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "settle_timeout")
    );
    let r: Value =
        serde_json::from_slice(&fs::read(b.job_dir(&job_id(&v)).join("result.json")).unwrap())
            .unwrap();
    assert_eq!(r["agent_invoked"], false);
    assert!(b.frames_in("abort").is_empty());
}
#[test]
fn review14_replies_after_fork_use_current_session_inbox() {
    let b = RpcBox::new();
    let v = send(&b, "fork_reply", &[], 11);
    let new = v["data"]["forked"]["to"].as_str().unwrap();
    assert_eq!(v["data"]["replies"][0]["session_id"], new);
    let record: Value = serde_json::from_str(
        fs::read_to_string(b.omp.glen_home.join("inbox").join(format!("{new}.ndjson")))
            .unwrap()
            .trim(),
    )
    .unwrap();
    assert_eq!(record["session_id"], new);
    assert!(
        !b.omp
            .glen_home
            .join("inbox")
            .join(format!("{A_ID}.ndjson"))
            .exists()
    );
}

#[test]
fn review5_delayed_final_state_still_detects_silent_fork() {
    let b = RpcBox::new();
    let o = b
        .glen(&["send", "e27c0de", "hi", "--json"])
        .env("FAKE_OMP_SCENARIO", "fork_silent")
        .env("FAKE_OMP_STATE_DELAY_MS", "500")
        .output()
        .unwrap();
    let v = envelope(&o, 11);
    b.wait_worker_gone(&job_id(&v));
    assert_eq!(v["data"]["forked"]["from"], A_ID);
    assert!(
        v["data"]["forked"]["to"]
            .as_str()
            .unwrap()
            .ends_with("f0f0")
    );
}
