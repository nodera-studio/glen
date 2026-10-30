mod common;
use common::{A_ID, LiveBox, data, json, poll};
use glen::{
    glen_home::GlenHome,
    jobs::Deliver,
    live::{
        self,
        client::{self, Frame},
        route::Endpoint,
    },
    ops::{Op, SendArgs},
};
use serde_json::Value;
use std::{
    fs,
    sync::mpsc::RecvTimeoutError,
    time::{Duration, Instant},
};
fn home(b: &LiveBox) -> GlenHome {
    GlenHome {
        root: b.rpc.omp.glen_home.clone(),
    }
}
fn send(b: &LiveBox, args: &[&str]) -> std::process::Output {
    let mut a = vec!["send", "e27c0de", "hi", "--json"];
    a.extend_from_slice(args);
    b.rpc
        .glen(&a)
        .env("GLEN_OMP_BIN", "/nonexistent")
        .output()
        .unwrap()
}
fn job_id(v: &Value) -> &str {
    v["job"]["id"].as_str().unwrap()
}
fn error_code(o: &std::process::Output) -> Value {
    json(o)["error"]["details"]["error"]["code"].clone()
}
fn wait_failed(b: &LiveBox, o: &std::process::Output) {
    let v = json(o);
    let id = v["error"]["details"]["job"]["id"].as_str().unwrap();
    b.rpc.wait_worker_gone(id);
}
#[test]
fn fake_ext_protocol_table() {
    for scenario in [
        "ok",
        "busy",
        "progress",
        "slow",
        "never_settles",
        "aborted",
        "error",
        "moved",
        "switched",
        "shutdown",
        "crash",
        "mismatch",
        "rekey",
        "hang",
        "no_send",
        "v2",
        "malformed",
        "no_ack",
        "queued",
    ] {
        let mut b = LiveBox::new();
        b.start_ext(A_ID, scenario, &[("FAKE_EXT_DELAY_MS", "10")]);
        let r = match glen::live::registry::read(&home(&b), A_ID) {
            glen::live::registry::Read::Ok(r) => r,
            _ => panic!("registry missing"),
        };
        let hello = client::hello(&r.socket, Duration::from_millis(200));
        if scenario == "hang" {
            assert!(matches!(hello, Err(client::ClientError::Timeout)));
            continue;
        }
        if scenario == "v2" {
            assert!(matches!(hello, Err(client::ClientError::Refused(_))));
            continue;
        }
        assert!(hello.is_ok());
        assert!(client::state(&r.socket, Duration::from_secs(1)).is_ok());
        let err = client::unary(&r.socket, &Op::Abort, Duration::from_secs(1)).unwrap_err();
        match err {
            client::ClientError::Refused(e) => {
                assert_eq!(e.code, "unsupported_op");
                assert_eq!(e.details.unwrap()["reserved"], true)
            }
            _ => panic!("{err:?}"),
        }
        let conn = client::open_send(
            &r.socket,
            "r",
            SendArgs {
                text: "hi".into(),
                deliver_as: Deliver::Auto,
                job_id: None,
                expect_session_id: Some(A_ID.into()),
            },
            &|| false,
        )
        .unwrap();
        if scenario == "no_ack" {
            assert!(matches!(
                conn.rx.recv_timeout(Duration::from_millis(100)),
                Err(RecvTimeoutError::Timeout)
            ));
            continue;
        }
        let first = conn
            .rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert!(matches!(first, Frame::Response { .. }));
        if matches!(scenario, "no_send" | "rekey" | "never_settles" | "queued") {
            continue;
        }
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            let f = conn
                .rx
                .recv_timeout(end.saturating_duration_since(Instant::now()))
                .unwrap();
            match f {
                Ok(Frame::Event { kind, .. }) if kind == "settled" || kind == "error" => break,
                Err(_) => break,
                _ => (),
            }
        }
    }
}
#[test]
fn ac314_live_route_beats_lease() {
    let mut b = LiveBox::new();
    b.start_ext(A_ID, "ok", &[]);
    let mut lease = b.rpc.hold_lease(A_ID);
    let o = send(&b, &[]);
    let d = data(&o);
    assert_eq!(d["job"]["route"], "live");
    assert_eq!(d["assistant_text"], "pong: hi");
    assert_eq!(d["replies"][0]["text"], "reply: hi");
    assert_eq!(d["job"]["delivered_as"], "prompt");
    let id = job_id(&d);
    let job = b.rpc.job(id).0;
    assert!(job.get("omp_bin").is_none());
    assert!(job["live_socket"].is_string());
    assert!(b.rpc.fake_log().is_empty());
    let frames = b.ext_log();
    let sent = frames
        .iter()
        .find(|v| v["dir"] == "in" && v["frame"]["op"] == "send")
        .unwrap();
    assert_eq!(sent["frame"]["args"]["expect_session_id"], A_ID);
    assert_eq!(sent["frame"]["args"]["job_id"], id);
    b.rpc.wait_worker_gone(id);
    lease.kill().unwrap();
    lease.wait().unwrap();
}
#[test]
fn ac314_stale_registries_fall_through() {
    for scenario in ["dead", "missing", "mismatch"] {
        let mut b = LiveBox::new();
        let pid = if scenario == "dead" {
            common::dead_pid()
        } else {
            std::process::id()
        };
        let socket = live::run_dir(&home(&b)).join(format!("{pid}.sock"));
        if scenario == "mismatch" {
            b.start_ext(A_ID, "mismatch", &[]);
        } else {
            b.write_registry(A_ID, pid, &socket);
            if scenario == "dead" {
                fs::write(&socket, "").unwrap();
            }
        }
        let o = b
            .rpc
            .glen(&["send", "e27c0de", "hi", "--json"])
            .output()
            .unwrap();
        let d = data(&o);
        assert_eq!(d["job"]["route"], "rpc");
        assert!(!b.registry(A_ID).exists());
        assert!(!socket.exists());
        b.rpc.wait_worker_gone(job_id(&d));
    }
}
#[test]
fn ac314_unhealthy_refuses_before_job_creation() {
    for scenario in ["hang", "v2", "no_send", "unreadable", "long"] {
        let mut b = LiveBox::new();
        if scenario == "unreadable" {
            b.write_registry(A_ID, std::process::id(), std::path::Path::new("/tmp/none"));
            fs::write(b.registry(A_ID), "bad").unwrap();
        } else if scenario == "long" {
            b.write_registry(
                A_ID,
                std::process::id(),
                &std::path::PathBuf::from(format!("/{}", "x".repeat(110))),
            );
        } else {
            b.start_ext(A_ID, scenario, &[]);
        }
        let start = Instant::now();
        let o = send(&b, &[]);
        assert_eq!(o.status.code(), Some(6), "{o:?}");
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(!b.rpc.omp.glen_home.join("jobs").exists());
        let hint = json(&o)["error"]["hint"].as_str().unwrap().to_owned();
        assert!(hint.contains("glen install-extension"));
        if scenario == "long" {
            assert!(hint.contains("GLEN_HOME"));
        }
        assert!(o.stderr.is_empty());
    }
}
#[test]
fn ac315_endpoint_and_glen_lock_refuse() {
    let mut b = LiveBox::new();
    let lock = b.rpc.hold_glen_lock(A_ID);
    b.start_ext(A_ID, "ok", &[]);
    let o = send(&b, &[]);
    assert_eq!(o.status.code(), Some(6));
    assert!(
        json(&o)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("glen jobs --active")
    );
    drop(lock);
}
#[test]
fn ac315_lease_held_hints() {
    let b = LiveBox::new();
    let mut lease = b.rpc.hold_lease(A_ID);
    let o = send(&b, &[]);
    assert_eq!(o.status.code(), Some(6));
    assert!(
        json(&o)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("glen install-extension")
    );
    fs::create_dir_all(b.rpc.omp.agent.join("extensions/glen")).unwrap();
    fs::write(b.rpc.omp.agent.join("extensions/glen/index.ts"), "").unwrap();
    let o = send(&b, &[]);
    assert!(
        json(&o)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("restart")
    );
    lease.kill().unwrap();
    lease.wait().unwrap();
}
#[test]
fn ac315_worker_refuses_late_endpoint() {
    let mut b = LiveBox::new();
    let lock = b.rpc.hold_glen_lock(A_ID);
    let o = b
        .rpc
        .glen(&["send", "e27c0de", "hi", "--no-wait", "--json"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(10));
    let d = json(&o)["data"].clone();
    b.start_ext(A_ID, "ok", &[]);
    drop(lock);
    let o = b
        .rpc
        .glen(&["wait", job_id(&d), "--timeout", "10s", "--json"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(6));
    assert_eq!(error_code(&o), "no_route");
    assert!(b.rpc.fake_log().is_empty());
    wait_failed(&b, &o);
}
#[test]
fn ac316_outcomes() {
    for (scenario, exit, code) in [
        ("progress", 0, ""),
        ("busy", 0, ""),
        ("aborted", 7, "aborted"),
        ("error", 7, "omp_error"),
        ("moved", 11, ""),
        ("switched", 7, "session_switched"),
        ("shutdown", 7, "omp_exited"),
        ("crash", 7, "omp_exited"),
        ("malformed", 7, "protocol"),
        ("rekey", 7, "session_moved"),
    ] {
        let mut b = LiveBox::new();
        b.start_ext(A_ID, scenario, &[]);
        let o = send(
            &b,
            if scenario == "busy" {
                &["--steer"]
            } else {
                &[]
            },
        );
        assert_eq!(o.status.code(), Some(exit), "{scenario}: {o:?}");
        assert!(o.stderr.is_empty());
        assert!(!o.stdout.contains(&27));
        let v = json(&o);
        let id = if exit == 7 {
            assert_eq!(error_code(&o), code);
            v["error"]["details"]["job"]["id"].as_str().unwrap()
        } else {
            job_id(&v["data"])
        };
        b.rpc.wait_worker_gone(id);
        let result: Value =
            serde_json::from_slice(&fs::read(b.rpc.job_dir(id).join("result.json")).unwrap())
                .unwrap();
        if scenario == "progress" {
            assert_eq!(result["replies"].as_array().unwrap().len(), 2);
            assert_eq!(result["replies"][0]["text"], "working");
            assert_eq!(
                fs::read_to_string(b.rpc.omp.glen_home.join(format!("inbox/{A_ID}.ndjson")))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
            assert_eq!(
                fs::read_to_string(b.rpc.job_dir(id).join("replies.ndjson"))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
        }
        if scenario == "busy" {
            assert_eq!(v["data"]["job"]["delivered_as"], "steer");
        }
        if scenario == "crash" {
            assert_eq!(result["replies"].as_array().unwrap().len(), 1);
        }
        if scenario == "moved" {
            assert!(
                v["data"]["forked"]["to_handle"]
                    .as_str()
                    .unwrap()
                    .ends_with("f0f0")
            );
        }
        if scenario == "rekey" {
            assert!(
                v["error"]["details"]["error"]["details"]["to"]
                    .as_str()
                    .unwrap()
                    .ends_with("f0f0")
            );
            assert!(result["replies"].as_array().unwrap().is_empty());
        }
    }
}
#[test]
fn ac317_timeouts_ack_and_consumption() {
    for (scenario, timeout, expected, hint) in [
        ("never_settles", "2s", "timeout", "keeps running"),
        ("queued", "2s", "timeout", "still queued"),
        ("no_ack", "20s", "protocol", ""),
    ] {
        let mut b = LiveBox::new();
        b.start_ext(A_ID, scenario, &[]);
        let start = Instant::now();
        let o = b
            .rpc
            .glen(&["send", "e27c0de", "hi", "--wait", "15s", "--json"])
            .env("GLEN_JOB_TIMEOUT", timeout)
            .env("GLEN_OMP_BIN", "/nonexistent")
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(7), "{o:?}");
        assert_eq!(error_code(&o), expected);
        if !hint.is_empty() {
            assert!(json(&o)["error"]["hint"].as_str().unwrap().contains(hint));
        }
        assert!(start.elapsed() < Duration::from_secs(12));
        wait_failed(&b, &o);
    }
}
#[test]
fn ac317_foreground_wait_control_and_cancellation() {
    let mut b = LiveBox::new();
    b.start_ext(A_ID, "slow", &[("FAKE_EXT_DELAY_MS", "1800")]);
    let o = send(&b, &["--wait", "300ms"]);
    assert_eq!(o.status.code(), Some(10));
    let d = json(&o)["data"].clone();
    let id = job_id(&d);
    let dir = b.rpc.job_dir(id).join("control");
    fs::create_dir_all(&dir).unwrap();
    let start = Instant::now();
    fs::write(dir.join("abort.json"), r#"{"v":1,"op":"abort"}"#).unwrap();
    let result: Value = poll(Duration::from_millis(1500), || {
        fs::read(dir.join("abort.done.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    });
    assert_eq!(result["error"]["code"], "unsupported_op");
    assert!(start.elapsed() < Duration::from_millis(1500));
    let out = b
        .rpc
        .glen(&["wait", id, "--timeout", "10s", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    b.rpc.wait_worker_gone(id);
    let mut b = LiveBox::new();
    b.start_ext(A_ID, "never_settles", &[]);
    let start = Instant::now();
    let o = send(&b, &["--no-wait"]);
    assert_eq!(o.status.code(), Some(10));
    assert!(start.elapsed() < Duration::from_secs(2));
    let v = json(&o);
    let id = job_id(&v["data"]);
    let status = poll(Duration::from_secs(3), || {
        let s = b.rpc.job(id).1;
        (s["status"] == "accepted").then_some(s)
    });
    let pid =
        rustix::process::Pid::from_raw(status["worker_pid"].as_i64().unwrap() as i32).unwrap();
    let start = Instant::now();
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    let s = b.rpc.wait_terminal(id, Duration::from_millis(1500));
    assert_eq!(s["status"], "aborted");
    assert_eq!(s["error"]["code"], "cancelled");
    assert_eq!(s["warnings"][0]["code"], "live_turn_continues");
    assert!(start.elapsed() < Duration::from_millis(1500));
    b.rpc.wait_worker_gone(id);
}
#[test]
fn ac318_ls_extension_state_and_parallel_budget() {
    for scenario in ["ok", "busy"] {
        let mut b = LiveBox::new();
        b.start_ext(A_ID, scenario, &[]);
        let d = data(
            &b.rpc
                .glen(&["ls", "--all", "--live", "--json"])
                .output()
                .unwrap(),
        );
        let row = d["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == A_ID)
            .unwrap();
        assert_eq!(
            row["state"],
            if scenario == "busy" { "busy" } else { "live" }
        );
        assert_eq!(row["state_source"], "extension");
        assert_eq!(row["state_detail"], "tui");
    }
    let mut b = LiveBox::new();
    let start = Instant::now();
    b.rpc.glen(&["ls", "--all", "--json"]).output().unwrap();
    let baseline = start.elapsed();
    for id in [
        A_ID,
        "01a0f6b1-2680-7a3c-8b21-4a1f9e2aaaaa",
        "01a0f6b1-2680-7a3c-8b21-4a1f9e2bbbbb",
    ] {
        b.start_ext(id, "hang", &[]);
    }
    let start = Instant::now();
    let d = data(&b.rpc.glen(&["ls", "--all", "--json"]).output().unwrap());
    assert!(start.elapsed().saturating_sub(baseline) < Duration::from_secs(1));
    assert!(
        d["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["state_source"] == "disk")
    );
}
#[test]
fn ac323_version_mismatch_warns() {
    let mut b = LiveBox::new();
    b.start_ext(A_ID, "ok", &[("FAKE_EXT_VERSION", "0.0.1")]);
    let o = send(&b, &[]);
    assert!(o.status.success());
    let v = json(&o);
    assert_eq!(v["warnings"][0]["code"], "extension_version_mismatch");
    b.rpc.wait_worker_gone(job_id(&v["data"]));
}
#[test]
fn ac326_endpoint_only_catalog_show_tail_latest_and_send() {
    let mut b = LiveBox::new();
    let id = "01a0f6b1-2680-7a3c-8b21-4a1f9e212345";
    b.start_ext(id, "ok", &[]);
    let d = data(&b.rpc.glen(&["ls", "--all", "--json"]).output().unwrap());
    let row = d["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap();
    assert_eq!(row["state_source"], "extension");
    assert_eq!(row["state"], "live");
    assert!(row["path"].is_null());
    let handle = row["handle"].as_str().unwrap();
    for command in ["show", "tail"] {
        for arg in [handle, id, "@latest"] {
            let o = b.rpc.glen(&[command, arg, "--json"]).output().unwrap();
            let d = data(&o);
            assert!(d["items"].as_array().unwrap().is_empty());
            assert_eq!(d["session"]["id"], id);
            assert!(d["session"]["path"].is_null());
            assert_eq!(d["session"]["state_source"], "extension");
            assert_eq!(d["session"]["state"], "live");
            assert_eq!(json(&o)["warnings"][0]["code"], "not_yet_persisted");
        }
    }
    for arg in [handle, id, "@latest"] {
        let o = b
            .rpc
            .glen(&["send", arg, "hi", "--json"])
            .env("GLEN_OMP_BIN", "/nonexistent")
            .output()
            .unwrap();
        let d = data(&o);
        assert_eq!(d["session"]["id"], id);
        assert_eq!(d["job"]["route"], "live");
        b.rpc.wait_worker_gone(job_id(&d));
    }
    assert!(matches!(
        live::route::find(&home(&b), id, Duration::from_secs(1)),
        Endpoint::Live { .. }
    ));
}

#[test]
fn ac326_unpersisted_readers_and_follow_do_not_need_a_socket() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    let b = LiveBox::new();
    let id = "01a0f6b1-2680-7a3c-8b21-4a1f9e212345";
    fs::create_dir_all(live::run_dir(&home(&b))).unwrap();
    fs::write(b.registry(id),serde_json::json!({"v":1,"pid":std::process::id(),"session_id":id,"session_file":null,"socket":live::run_dir(&home(&b)).join("unbound.sock"),"protocol":1,"mode":"tui","started_at":jiff::Timestamp::now().to_string(),"cwd":b.rpc.omp.alpha()}).to_string()).unwrap();
    for command in ["show", "tail"] {
        for arg in [id, "e212345", "@latest"] {
            let o = b.rpc.glen(&[command, arg, "--json"]).output().unwrap();
            let d = data(&o);
            assert_eq!(d["session"]["id"], id);
            assert!(d["session"]["path"].is_null());
            assert_eq!(d["session"]["state_source"], "extension");
            assert_eq!(d["session"]["state"], "live");
            assert!(d["items"].as_array().unwrap().is_empty());
            assert_eq!(json(&o)["warnings"][0]["code"], "not_yet_persisted");
        }
    }
    let mut child = b
        .rpc
        .glen(&["tail", id, "-f", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let _ = tx.send(serde_json::from_str::<Value>(&line.unwrap()).unwrap());
        }
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap()["event"],
        "session"
    );
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap()["code"],
        "not_yet_persisted"
    );
    assert!(child.try_wait().unwrap().is_none());
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(child.id() as i32).unwrap(),
        rustix::process::Signal::INT,
    )
    .unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap()["event"],
        "end"
    );
    assert_eq!(child.wait().unwrap().code(), Some(130));
}

#[test]
fn ac315_worker_rechecks_endpoint_at_prompt_submission() {
    let mut b = LiveBox::new();
    let o = b
        .rpc
        .glen(&["send", "e27c0de", "hi", "--no-wait", "--json"])
        .env("FAKE_OMP_PROMPT_SUBMIT_DELAY_MS", "1200")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(10));
    let value = json(&o);
    let id = job_id(&value["data"]);
    poll(Duration::from_secs(5), || {
        b.rpc
            .fake_log()
            .iter()
            .any(|e| e["event"] == "start")
            .then_some(())
    });
    b.start_ext(A_ID, "ok", &[]);
    let out = b
        .rpc
        .glen(&["wait", id, "--timeout", "10s", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    assert_eq!(error_code(&out), "no_route");
    assert!(b.rpc.frames_in("prompt").is_empty());
    b.rpc.wait_worker_gone(id);
}

#[test]
fn ac315_worker_refuses_an_unhealthy_endpoint_at_submission() {
    let b = LiveBox::new();
    let o = b
        .rpc
        .glen(&["send", "e27c0de", "hi", "--no-wait", "--json"])
        .env("FAKE_OMP_PROMPT_SUBMIT_DELAY_MS", "1200")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(10));
    let value = json(&o);
    let id = job_id(&value["data"]);
    poll(Duration::from_secs(5), || {
        b.rpc
            .fake_log()
            .iter()
            .any(|e| e["event"] == "start")
            .then_some(())
    });
    fs::create_dir_all(b.registry(A_ID).parent().unwrap()).unwrap();
    fs::write(b.registry(A_ID), "malformed registry").unwrap();
    let out = b
        .rpc
        .glen(&["wait", id, "--timeout", "10s", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    assert_eq!(error_code(&out), "no_route");
    assert!(b.rpc.frames_in("prompt").is_empty());
    b.rpc.wait_worker_gone(id);
}

#[test]
fn not_delivered_from_extension_is_preserved_in_live_job_result() {
    let mut b = LiveBox::new();
    b.start_ext(A_ID, "not_delivered", &[]);
    let o = send(&b, &[]);
    assert_eq!(o.status.code(), Some(7));
    assert_eq!(error_code(&o), "not_delivered");
    wait_failed(&b, &o);
}

#[test]
fn endpoint_only_tail_ends_not_found_after_disappearance_grace() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    let b = LiveBox::new();
    let id = "01a0f6b1-2680-7a3c-8b21-4a1f9e212345";
    fs::create_dir_all(live::run_dir(&home(&b))).unwrap();
    fs::write(b.registry(id), serde_json::json!({"v":1,"pid":std::process::id(),"session_id":id,"session_file":null,"socket":live::run_dir(&home(&b)).join("unbound.sock"),"protocol":1,"mode":"tui","started_at":jiff::Timestamp::now().to_string(),"cwd":b.rpc.omp.alpha()}).to_string()).unwrap();
    let mut child = b
        .rpc
        .glen(&["tail", id, "-f", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx
                .send(serde_json::from_str::<Value>(&line.unwrap()).unwrap())
                .is_err()
            {
                break;
            }
        }
    });
    let first = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(first["event"], "session");
    assert!(first["session"]["path"].is_null());
    assert_eq!(first["session"]["state_source"], "extension");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap()["code"],
        "not_yet_persisted"
    );
    fs::remove_file(b.registry(id)).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        child.try_wait().unwrap().is_none(),
        "disappearance grace must be observed"
    );
    let error = rx.recv_timeout(Duration::from_secs(7)).unwrap();
    assert_eq!(error["ok"], false);
    assert_eq!(error["error"]["code"], "not_found");
    assert_eq!(child.wait().unwrap().code(), Some(3));
    reader.join().unwrap();
}

#[test]
fn show_and_tail_use_extension_state_for_persisted_sessions() {
    for scenario in ["ok", "busy"] {
        let mut b = LiveBox::new();
        b.start_ext(A_ID, scenario, &[]);
        for command in ["show", "tail"] {
            let o = b.rpc.glen(&[command, A_ID, "--json"]).output().unwrap();
            let row = &data(&o)["session"];
            assert_eq!(row["state_source"], "extension");
            assert_eq!(row["state_detail"], "tui");
            assert_eq!(
                row["state"],
                if scenario == "busy" { "busy" } else { "live" }
            );
            assert!(!row["path"].is_null());
        }
    }
}

#[test]
fn endpoint_only_tail_waits_for_persistence_then_streams_fixture_items() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    let b = LiveBox::new();
    let id = "01a0f6b1-2680-7a3c-8b21-4a1f9e212345";
    let path = b
        .rpc
        .omp
        .session('A')
        .parent()
        .unwrap()
        .join(format!("2026-10-30T09-00-00-000Z_{id}.jsonl"));
    fs::create_dir_all(live::run_dir(&home(&b))).unwrap();
    fs::write(b.registry(id), serde_json::json!({"v":1,"pid":std::process::id(),"session_id":id,"session_file":path,"socket":live::run_dir(&home(&b)).join("unbound.sock"),"protocol":1,"mode":"tui","started_at":jiff::Timestamp::now().to_string(),"cwd":b.rpc.omp.alpha()}).to_string()).unwrap();
    let mut child = b
        .rpc
        .glen(&["tail", id, "-f", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx
                .send(serde_json::from_str::<Value>(&line.unwrap()).unwrap())
                .is_err()
            {
                break;
            }
        }
    });
    let initial = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(initial["event"], "session");
    assert_eq!(initial["session"]["state_source"], "extension");
    assert!(initial["session"]["path"].is_null());
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap()["code"],
        "not_yet_persisted"
    );
    assert!(child.try_wait().unwrap().is_none());
    assert!(rx.recv_timeout(Duration::from_millis(350)).is_err());
    let fixture = fs::read_to_string(b.rpc.omp.session('A'))
        .unwrap()
        .replace(A_ID, id);
    fs::write(path, fixture).unwrap();
    let mut received = Vec::new();
    let mut saw_user = false;
    for _ in 0..10 {
        let item = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(item["event"], "item");
        saw_user = item["item"]
            .to_string()
            .contains("Why does parser_test flake?");
        received.push(item);
        if saw_user {
            break;
        }
    }
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(child.id() as i32).unwrap(),
        rustix::process::Signal::INT,
    )
    .unwrap();
    let mut ended = false;
    for _ in 0..30 {
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(frame) if frame["event"] == "end" => {
                ended = true;
                break;
            }
            Ok(_) => (),
            Err(_) => break,
        }
    }
    assert!(ended);
    assert_eq!(child.wait().unwrap().code(), Some(130));
    reader.join().unwrap();
    assert!(
        saw_user,
        "expected fixture user message after persistence, got {received:?}"
    );
}

#[test]
fn live_batch_warning_is_returned_to_the_calling_agent() {
    let mut b = LiveBox::new();
    b.start_ext(A_ID, "batched", &[]);
    let o = send(&b, &[]);
    assert!(o.status.success(), "{o:?}");
    let value = json(&o);
    assert!(
        value["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "batched" && w["message"].as_str().unwrap().contains("j7654321"))
    );
    assert_eq!(data(&o)["assistant_text"], "pong: hi");
    b.rpc.wait_worker_gone(job_id(&value["data"]));
}
