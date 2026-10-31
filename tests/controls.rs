mod common;
use common::*;
use glen::{
    glen_home::GlenHome,
    jobs::lock::{Holder, HolderKind},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Output, Stdio},
    time::{Duration, Instant},
};
fn command(b: &RpcBox, args: &[&str]) -> std::process::Command {
    let mut c = b.glen(args);
    c.env("FAKE_OMP_SERIAL", "1");
    c
}
fn checked(out: Output, exit: i32) -> Value {
    assert_eq!(
        out.status.code(),
        Some(exit),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.stdout.contains(&27));
    assert_eq!(out.stdout.iter().filter(|c| **c == b'\n').count(), 1);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["v"], 1);
    value
}
fn run(b: &RpcBox, args: &[&str], exit: i32) -> Value {
    checked(command(b, args).output().unwrap(), exit)
}
fn warning(value: &Value, code: &str) -> bool {
    value["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|w| w["code"] == code)
}
fn home(b: &RpcBox) -> GlenHome {
    GlenHome {
        root: b.omp.glen_home.clone(),
    }
}
fn accepted(b: &RpcBox, id: &str) {
    poll(Duration::from_secs(5), || {
        (b.job(id).1["status"] == "accepted").then_some(())
    });
}
fn job_id(value: &Value) -> &str {
    value["data"]["job"]["id"].as_str().unwrap()
}
fn launch(b: &RpcBox) -> Value {
    checked(
        command(b, &["send", "e27c0de", "hold", "--no-wait", "--json"])
            .env("FAKE_OMP_RUN_RELEASE", b.omp.tmp.path().join("release"))
            .output()
            .unwrap(),
        10,
    )
}
fn terminate_job(b: &RpcBox, id: &str) {
    let value = run(b, &["abort", "e27c0de", "--timeout", "15s", "--json"], 0);
    assert_eq!(value["data"]["job_status"], "aborted");
    b.wait_worker_gone(id);
}
#[test]
fn ac357_models_no_session() {
    let b = RpcBox::new();
    let before = session_lines(&b.omp.session('A'));
    let value = run(&b, &["models", "--json"], 0);
    let data = &value["data"];
    assert!(data["session"].is_null());
    assert_eq!(data["route"], "rpc");
    assert!(data["current"].is_null());
    assert!(data["thinking_level"].is_null());
    assert_eq!(
        data["default_model"],
        json!({"provider":"fake","id":"fake-1"})
    );
    assert_eq!(data["total"], 6);
    assert_eq!(data["models"][1]["thinking_levels"], json!(["off"]));
    let pairs = data["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            format!(
                "{}/{}",
                m["provider"].as_str().unwrap(),
                m["id"].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>();
    let mut sorted = pairs.clone();
    sorted.sort();
    assert_eq!(pairs, sorted);
    let filtered = run(&b, &["models", "--provider", "OTHER", "--json"], 0);
    assert_eq!(filtered["data"]["total"], 1);
    assert!(!b.omp.glen_home.exists());
    assert_eq!(session_lines(&b.omp.session('A')), before);
    let starts = b
        .fake_log()
        .into_iter()
        .filter(|r| r["event"] == "start")
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 2);
    for start in starts {
        assert!(
            start["argv"]
                .as_array()
                .unwrap()
                .contains(&json!("--no-session"))
        );
        assert_eq!(start["env"]["GLEN_MANAGED"], "1");
    }
}
#[test]
fn ac355_persisted_and_ac359_resolution() {
    let b = RpcBox::new();
    let ambiguous = run(&b, &["model", "e27c0de", "fake-2", "--json"], 4);
    assert_eq!(
        ambiguous["error"]["details"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(b.frames_in("set_model").is_empty());
    let missing = run(&b, &["model", "e27c0de", "nope", "--json"], 3);
    assert!(
        missing["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("glen models e27c0de")
    );
    let value = run(&b, &["model", "e27c0de", "fake/fake-2", "--json"], 0);
    let data = &value["data"];
    assert_eq!(data["route"], "rpc");
    assert_eq!(data["scope"], "session");
    assert_eq!(data["model"]["id"], "fake-2");
    assert_eq!(data["thinking_level"], "off");
    assert_eq!(data["changed"], true);
    assert!(warning(&value, "thinking_changed"));
    let lines = session_lines(&b.omp.session('A'));
    assert_eq!(lines[lines.len() - 2]["type"], "model_change");
    assert_eq!(lines[lines.len() - 2]["model"], "fake/fake-2");
    assert_eq!(lines[lines.len() - 2]["role"], "default");
    assert_eq!(lines.last().unwrap()["thinkingLevel"], "off");
    let listed = run(&b, &["models", "e27c0de", "--json"], 0);
    assert_eq!(listed["data"]["current_source"], "disk");
    assert_eq!(listed["data"]["current"]["id"], "fake-2");
    assert_eq!(listed["data"]["thinking_level"], "off");
    run(&b, &["thinking", "e27c0de", "off", "--json"], 0);
    assert_eq!(session_lines(&b.omp.session('A')).len(), lines.len());
    let same = run(&b, &["model", "e27c0de", "fake/fake-2", "--json"], 0);
    assert_eq!(same["data"]["changed"], false);
    assert_eq!(session_lines(&b.omp.session('A')).len(), lines.len() + 1);
    assert_eq!(
        session_lines(&b.omp.session('A')).last().unwrap()["type"],
        "model_change"
    );
    for (spec, provider, id) in [
        ("FAKE/FAKE-1", "fake", "fake-1"),
        ("vendor/m-3", "vendor", "m-3"),
        ("router/vendor/m-3", "router", "vendor/m-3"),
    ] {
        let value = run(&b, &["model", "e27c0de", spec, "--json"], 0);
        assert_eq!(value["data"]["model"]["provider"], provider);
        assert_eq!(value["data"]["model"]["id"], id);
    }
}
#[test]
fn ac356_idle_abort_and_ac365_control_holder() {
    let b = RpcBox::new();
    let result = run(&b, &["stop", "e27c0de", "--json"], 0);
    assert_eq!(result["data"]["route"], "none");
    assert_eq!(result["data"]["aborted"], false);
    assert_eq!(result["data"]["was"], "idle");
    assert_eq!(result["data"]["queued_jobs"], json!([]));
    assert!(!b.omp.glen_home.exists());
    assert!(b.fake_log().is_empty());
    let identity = glen::jobs::liveness::current().unwrap();
    let _lock = hold_glen_lock_as(
        &home(&b),
        A_ID,
        Some(Holder {
            v: 1,
            kind: HolderKind::Control,
            job_id: None,
            pid: identity.pid,
            start: Some(identity.start),
        }),
    );
    let busy = run(&b, &["abort", "e27c0de", "--json"], 7);
    assert_eq!(busy["error"]["details"]["reason"], "busy");
}
#[test]
fn ac361_thinking_and_ac360_no_auth() {
    let b = RpcBox::new();
    let error = run(&b, &["thinking", "e27c0de", "xhigh", "--json"], 2);
    assert_eq!(error["error"]["details"]["reason"], "invalid_level");
    assert_eq!(
        error["error"]["details"]["available"],
        json!(["off", "low", "medium", "high"])
    );
    assert!(b.frames_in("set_thinking_level").is_empty());
    let ok = run(&b, &["thinking", "e27c0de", "high", "--json"], 0);
    assert_eq!(ok["data"]["thinking_level"], "high");
    assert_eq!(ok["data"]["scope"], "session");
    let error = run(&b, &["model", "e27c0de", "nokey/locked-1", "--json"], 7);
    assert_eq!(error["error"]["details"]["reason"], "no_auth");
    assert!(error["error"]["hint"].as_str().unwrap().contains("log in"));
    run(&b, &["thinking", "e27c0de", "banana", "--json"], 2);
    for args in [
        vec!["models", "--timeout", "0", "--json"],
        vec!["model", "@new", "fake/fake-1", "--json"],
        vec!["abort", "j1234567", "--json"],
    ] {
        run(&b, &args, 2);
    }
}
#[test]
fn ac362_compact_and_instructions() {
    let b = RpcBox::new();
    let value = run(
        &b,
        &[
            "compact",
            "e27c0de",
            "--instructions",
            "keep exact facts",
            "--json",
        ],
        0,
    );
    assert_eq!(value["data"]["compacted"], true);
    assert_eq!(value["data"]["tokens_before"], 1234);
    assert_eq!(value["data"]["tokens_after"], 123);
    assert_eq!(
        b.frames_in("compact")[0]["customInstructions"],
        "keep exact facts"
    );
    assert_eq!(
        session_lines(&b.omp.session('A')).last().unwrap()["type"],
        "compaction"
    );
    let out = checked(
        command(&b, &["compact", "e27c0de", "--json"])
            .env("FAKE_OMP_SCENARIO", "compact_error")
            .output()
            .unwrap(),
        7,
    );
    assert_eq!(out["error"]["details"]["reason"], "omp_error");
    assert!(
        out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Nothing to compact")
    );
}
#[test]
fn ac354_rpc_safety() {
    let b = RpcBox::new();
    let moved = checked(
        command(&b, &["model", "e27c0de", "fake/fake-2", "--json"])
            .env("FAKE_OMP_SCENARIO", "moved")
            .output()
            .unwrap(),
        7,
    );
    assert_eq!(moved["error"]["details"]["reason"], "session_moved");
    assert_eq!(moved["error"]["details"]["from"], A_ID);
    assert!(b.frames_in("set_model").is_empty());
    let fork = checked(
        command(&b, &["thinking", "e27c0de", "high", "--json"])
            .env("FAKE_OMP_SCENARIO", "fork_control")
            .output()
            .unwrap(),
        11,
    );
    assert!(
        fork["data"]["forked"]["to_handle"]
            .as_str()
            .unwrap()
            .ends_with("f0f0")
    );
    assert!(!glen::jobs::lock::is_held(&home(&b), A_ID));
    assert_eq!(
        glen::jobs::lock::holder(&home(&b), A_ID).unwrap().kind,
        HolderKind::Control
    );
    for start in b.fake_log().into_iter().filter(|r| r["event"] == "start") {
        assert!(
            start["argv"]
                .as_array()
                .unwrap()
                .contains(&json!("--resume"))
        );
        assert_eq!(start["env"]["GLEN_MANAGED"], "1");
        let pid = start["pid"].as_u64().unwrap() as u32;
        assert!(matches!(
            glen::jobs::liveness::start_token(pid),
            glen::jobs::liveness::Probe::Gone
        ));
    }
}
#[test]
fn ac363_rpc_bounds() {
    for interrupt in [false, true] {
        let b = RpcBox::new();
        let before = fs::read(b.omp.session('A')).unwrap();
        let mut cmd = command(
            &b,
            &[
                "compact",
                "e27c0de",
                "--timeout",
                if interrupt { "15s" } else { "1s" },
                "--json",
            ],
        );
        cmd.env("FAKE_OMP_COMPACT_DELAY_MS", "8000")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let start = Instant::now();
        let child = cmd.spawn().unwrap();
        if interrupt {
            poll(Duration::from_secs(3), || {
                (!b.frames_in("compact").is_empty()).then_some(())
            });
            let pid = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
            rustix::process::kill_process(pid, rustix::process::Signal::INT).unwrap();
        }
        let value = checked(
            child.wait_with_output().unwrap(),
            if interrupt { 130 } else { 7 },
        );
        assert!(start.elapsed() < Duration::from_secs(6));
        if interrupt {
            assert_eq!(value["data"]["interrupted"], true);
        } else {
            assert_eq!(value["error"]["details"]["reason"], "timeout");
        }
        assert!(b.frames_in("abort").is_empty());
        assert_eq!(fs::read(b.omp.session('A')).unwrap(), before);
        for start in b.fake_log().into_iter().filter(|r| r["event"] == "start") {
            poll(Duration::from_secs(2), || {
                matches!(
                    glen::jobs::liveness::start_token(start["pid"].as_u64().unwrap() as u32),
                    glen::jobs::liveness::Probe::Gone
                )
                .then_some(())
            });
        }
    }
}
#[test]
fn ac352_job_route_and_ac364_controls_share_the_client() {
    let b = RpcBox::new();
    let launched = launch(&b);
    let id = job_id(&launched);
    accepted(&b, id);
    let holder = glen::jobs::lock::holder(&home(&b), A_ID).unwrap();
    assert_eq!(holder.kind, HolderKind::Job);
    assert_eq!(holder.job_id.as_deref(), Some(id));
    for args in [
        vec!["models", "e27c0de", "--json"],
        vec!["model", "e27c0de", "fake/fake-2", "--json"],
        vec!["thinking", "e27c0de", "off", "--json"],
    ] {
        let result = run(&b, &args, 0);
        assert_eq!(result["data"]["route"], "job");
    }
    assert_eq!(
        b.fake_log()
            .iter()
            .filter(|r| r["event"] == "start")
            .count(),
        1
    );
    let error = run(&b, &["compact", "e27c0de", "--json"], 7);
    assert_eq!(error["error"]["details"]["reason"], "busy");
    assert!(b.frames_in("compact").is_empty());
    let start = Instant::now();
    terminate_job(&b, id);
    assert!(start.elapsed() < Duration::from_secs(3));
}
#[test]
fn ac364_deferred_and_expired_controls() {
    let b = RpcBox::new();
    let release = b.omp.tmp.path().join("ready");
    let launched = checked(
        command(&b, &["send", "e27c0de", "hold", "--no-wait", "--json"])
            .env("FAKE_OMP_READY_RELEASE", &release)
            .env("FAKE_OMP_RUN_RELEASE", b.omp.tmp.path().join("run-release"))
            .output()
            .unwrap(),
        10,
    );
    let id = job_id(&launched);
    poll(Duration::from_secs(3), || {
        glen::jobs::lock::holder(&home(&b), A_ID)
            .is_some()
            .then_some(())
    });
    let error = run(
        &b,
        &[
            "thinking",
            "e27c0de",
            "high",
            "--timeout",
            "250ms",
            "--json",
        ],
        7,
    );
    assert_eq!(error["error"]["details"]["reason"], "timeout");
    assert_eq!(error["error"]["details"]["outcome"], "unknown");
    assert!(
        error["error"]["hint"]
            .as_str()
            .unwrap()
            .contains(&format!("j{}", &id.replace('-', "")[25..]))
    );
    fs::write(&release, "go").unwrap();
    accepted(&b, id);
    let control = b.job_dir(id).join("control");
    poll(Duration::from_secs(3), || {
        fs::read_dir(&control).ok().and_then(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .find(|p| p.to_string_lossy().ends_with(".done.json"))
        })
    });
    assert!(b.frames_in("set_thinking_level").is_empty());
    terminate_job(&b, id);
}
#[test]
fn ac358_disk_without_thinking_and_ac367_width() {
    let b = RpcBox::new();
    let path = b.omp.session('A');
    let lines = session_lines(&path)
        .into_iter()
        .filter(|line| line["type"] != "thinking_level_change")
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&path, lines).unwrap();
    let value = run(&b, &["models", "e27c0de", "--json"], 0);
    assert!(value["data"]["thinking_level"].is_null());
    assert!(!b.omp.glen_home.exists());
    let out = command(&b, &["models", "--human"])
        .env("COLUMNS", "60")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .all(|line| unicode_width::UnicodeWidthStr::width(line) <= 60)
    );
}
#[test]
fn ac352_refusals_and_models_fallback() {
    let b = RpcBox::new();
    let mut lease = b.hold_lease(A_ID);
    for installed in [false, true] {
        if installed {
            let dir = b.omp.agent.join("extensions/glen");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("index.ts"), "test installed marker").unwrap();
        }
        let value = run(&b, &["model", "e27c0de", "fake/fake-2", "--json"], 6);
        assert_eq!(value["error"]["code"], "no_route");
        assert!(
            value["error"]["hint"]
                .as_str()
                .unwrap()
                .contains(if installed {
                    "restart"
                } else {
                    "glen install-extension"
                })
        );
    }
    run(&b, &["abort", "e27c0de", "--json"], 6);
    assert!(b.fake_log().is_empty());
    let value = run(&b, &["models", "e27c0de", "--json"], 0);
    assert!(warning(&value, "session_unreachable"));
    assert_eq!(value["data"]["current_source"], "disk");
    assert!(!b.omp.glen_home.exists());
    lease.kill().unwrap();
    lease.wait().unwrap();
}
#[test]
fn ac352_live_controls_and_ac353_old_extension() {
    for old in [false, true] {
        let mut b = LiveBox::new();
        b.start_ext(
            A_ID,
            "ok",
            &[("FAKE_EXT_OPS", if old { "w3" } else { "current" })],
        );
        for args in [
            vec!["abort", "e27c0de", "--json"],
            vec!["model", "e27c0de", "fake/fake-2", "--json"],
            vec!["thinking", "e27c0de", "off", "--json"],
            vec!["compact", "e27c0de", "--json"],
        ] {
            let value = checked(
                command(&b.rpc, &args)
                    .env("GLEN_OMP_BIN", "/nonexistent")
                    .output()
                    .unwrap(),
                if old { 6 } else { 0 },
            );
            if old {
                assert_eq!(value["error"]["details"]["reason"], "extension_outdated");
            } else {
                assert_eq!(value["data"]["route"], "live");
            }
        }
        if old {
            assert_eq!(
                b.ext_log()
                    .iter()
                    .filter(|r| r["dir"] == "in" && r["frame"]["op"] == "hello")
                    .count(),
                4
            );
            assert!(
                b.ext_log()
                    .iter()
                    .filter(|r| r["dir"] == "in")
                    .all(|r| r["frame"]["op"] == "hello")
            );
        }
        let models = run(&b.rpc, &["models", "e27c0de", "--json"], 0);
        assert_eq!(
            models["data"]["current_source"],
            if old { "disk" } else { "session" }
        );
        if old {
            assert!(warning(&models, "extension_outdated"));
        }
    }
}
#[test]
fn ac356_live_abort_settles_the_job() {
    let mut b = LiveBox::new();
    b.start_ext(A_ID, "never_settles", &[]);
    let launched = checked(
        command(&b.rpc, &["send", "e27c0de", "hold", "--no-wait", "--json"])
            .output()
            .unwrap(),
        10,
    );
    let id = job_id(&launched);
    accepted(&b.rpc, id);
    let start = Instant::now();
    let aborted = run(&b.rpc, &["abort", "e27c0de", "--json"], 0);
    assert!(start.elapsed() < Duration::from_secs(7));
    assert_eq!(aborted["data"]["aborted"], true);
    let result = run(&b.rpc, &["wait", id, "--timeout", "5s", "--json"], 7);
    assert_eq!(result["error"]["details"]["error"]["code"], "aborted");
    b.rpc.wait_worker_gone(id);
}
#[test]
fn ac366_post_identity_and_endpoint_only_selection() {
    let id = "01a0f6b1-2680-7a3c-8b21-4a1f97777777";
    let mut b = LiveBox::new();
    b.start_ext(id, "ok", &[("FAKE_EXT_POST_ID", "1")]);
    for args in [
        vec!["model", "7777777", "fake/fake-2", "--json"],
        vec!["thinking", "7777777", "off", "--json"],
    ] {
        let value = run(&b.rpc, &args, 11);
        assert!(
            value["data"]["forked"]["to"]
                .as_str()
                .unwrap()
                .ends_with("f0f0")
        );
        assert!(warning(&value, "not_yet_persisted"));
        assert!(value["data"]["session"]["path"].is_null());
    }
}
#[test]
fn ac369_same_version_build() {
    let b = RpcBox::new();
    run(&b, &["install-extension", "--json"], 0);
    let path = b.omp.agent.join("extensions/glen/index.ts");
    let installed = fs::read_to_string(&path).unwrap();
    let body = installed
        .split_once('\n')
        .unwrap()
        .1
        .replace("export const OPS =", "export const OLD_OPS =");
    fs::write(
        &path,
        format!(
            "// glen-extension version=0.1.0 protocol=1 digest={:016x}\n{body}",
            xxhash_rust::xxh64::xxh64(body.as_bytes(), 0)
        ),
    )
    .unwrap();
    let doctor = run(&b, &["doctor", "--json"], 0);
    let checks = doctor["data"]["checks"].as_array().unwrap();
    assert!(
        checks
            .iter()
            .any(|check| check["name"] == "extension" && check["status"] == "warn")
    );
    let install = run(&b, &["install-extension", "--json"], 0);
    assert_eq!(install["data"]["action"], "upgraded");
}
#[test]
fn ac368_no_unexpected_omp_writes() {
    fn snapshot(root: &Path, excluded: &Path) -> Vec<(String, Vec<u8>)> {
        let mut result = vec![];
        for entry in fs::read_dir(root).unwrap().flatten() {
            let path = entry.path();
            if path == excluded {
                continue;
            }
            if path.is_dir() {
                for (name, bytes) in snapshot(&path, excluded) {
                    result.push((
                        format!("{}/{}", entry.file_name().to_string_lossy(), name),
                        bytes,
                    ));
                }
            } else {
                result.push((
                    entry.file_name().to_string_lossy().into_owned(),
                    fs::read(path).unwrap(),
                ));
            }
        }
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }
    let b = RpcBox::new();
    let path = b.omp.session('A');
    let before = snapshot(&b.omp.agent, &path);
    run(&b, &["model", "e27c0de", "fake/fake-2", "--json"], 0);
    run(&b, &["thinking", "e27c0de", "off", "--json"], 0);
    run(&b, &["compact", "e27c0de", "--json"], 0);
    assert_eq!(snapshot(&b.omp.agent, &path), before);
}

#[test]
fn ac364_abort_during_a_serial_control_and_no_dispatch_after_stop() {
    let b = RpcBox::new();
    let launched = checked(
        command(&b, &["send", "e27c0de", "hold", "--no-wait", "--json"])
            .env("FAKE_OMP_RUN_RELEASE", b.omp.tmp.path().join("release"))
            .env("FAKE_OMP_STATE_DELAY_MS", "3000")
            .output()
            .unwrap(),
        10,
    );
    let id = job_id(&launched);
    accepted(&b, id);
    let dir = b.job_dir(id).join("control");
    fs::create_dir_all(&dir).unwrap();
    let now = jiff::Timestamp::now().as_millisecond() + 20000;
    fs::write(
        dir.join("a.json"),
        json!({"v":1,"op":"list_models","deadline_ms":now}).to_string(),
    )
    .unwrap();
    poll(Duration::from_secs(2), || {
        (b.frames_in("get_state").len() >= 2).then_some(())
    });
    // Worker polling can emit get_state, but only a.json asks for the model catalog.
    // Its final get_state is delayed by the serial fake, keeping a.json in flight.
    poll(Duration::from_secs(10), || {
        (!b.frames_in("get_available_models").is_empty()).then_some(())
    });
    assert!(!dir.join("a.done.json").exists());
    let start = Instant::now();
    fs::write(dir.join("b.json"), r#"{"v":1,"op":"abort"}"#).unwrap();
    fs::write(
        dir.join("c.json"),
        json!({"v":1,"op":"set_thinking_level","args":{"level":"high"},"deadline_ms":now})
            .to_string(),
    )
    .unwrap();
    b.wait_terminal(id, Duration::from_secs(13));
    assert!(start.elapsed() < Duration::from_secs(13));
    b.wait_worker_gone(id);
    let log = b.fake_log();
    let incoming: Vec<_> = log
        .iter()
        .filter(|r| r["event"] == "in")
        .map(|r| &r["frame"])
        .collect();
    let models = incoming
        .iter()
        .position(|f| f["type"] == "get_available_models")
        .unwrap();
    assert_eq!(b.frames_in("get_available_models").len(), 1);
    assert_eq!(incoming[models - 1]["type"], "get_state");
    assert_eq!(incoming[models + 1]["type"], "get_state");
    let abort = log
        .iter()
        .position(|r| r["event"] == "in" && r["frame"]["type"] == "abort")
        .unwrap();
    // The catalog and the final state response must both precede abort dispatch.
    for frame in &incoming[models..=models + 1] {
        let response = log
            .iter()
            .position(|r| {
                r["event"] == "out"
                    && r["frame"]["type"] == "response"
                    && r["frame"]["id"] == frame["id"]
            })
            .unwrap();
        assert_eq!(log[response]["frame"]["success"], true);
        assert!(
            response < abort,
            "abort preceded a.json's response: {log:?}"
        );
    }
    let a: Value = serde_json::from_slice(&fs::read(dir.join("a.done.json")).unwrap()).unwrap();
    assert_eq!(a["v"], 1);
    assert_eq!(a["ok"], true);
    assert_eq!(a["data"]["session_id"], A_ID);
    assert_eq!(
        a["data"]["current"],
        json!({"provider":"fake","id":"fake-1"})
    );
    assert_eq!(a["data"]["thinking_level"], "high");
    assert_eq!(a["data"]["models"].as_array().unwrap().len(), 6);
    assert!(b.frames_in("set_thinking_level").is_empty());
    let c: Value = serde_json::from_slice(&fs::read(dir.join("c.done.json")).unwrap()).unwrap();
    assert_eq!(c["error"]["code"], "job_ended");
    let ack: Value = serde_json::from_slice(&fs::read(dir.join("b.done.json")).unwrap()).unwrap();
    assert_eq!(ack["data"]["job_status"], "aborted");
}
#[test]
fn ac364_request_before_ready_runs_after_release_and_job_ended_reroutes_once() {
    for end_job in [false, true] {
        let b = RpcBox::new();
        let release = b.omp.tmp.path().join("ready");
        let launched = checked(
            command(&b, &["send", "e27c0de", "hold", "--no-wait", "--json"])
                .env("FAKE_OMP_READY_RELEASE", &release)
                .env("FAKE_OMP_RUN_RELEASE", b.omp.tmp.path().join("run-release"))
                .output()
                .unwrap(),
            10,
        );
        let id = job_id(&launched);
        poll(Duration::from_secs(3), || {
            glen::jobs::lock::holder(&home(&b), A_ID)
                .is_some()
                .then_some(())
        });
        poll(Duration::from_secs(3), || {
            b.fake_log()
                .iter()
                .any(|r| r["event"] == "start")
                .then_some(())
        });
        let child = command(
            &b,
            &[
                "model",
                "e27c0de",
                "fake/fake-2",
                "--timeout",
                "10s",
                "--json",
            ],
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
        let dir = b.job_dir(id).join("control");
        poll(Duration::from_secs(3), || {
            fs::read_dir(&dir)
                .ok()
                .and_then(|mut entries| entries.next().map(|_| ()))
        });
        assert!(b.frames_in("set_model").is_empty());
        if end_job {
            fs::write(dir.join("stop.json"), r#"{"v":1,"op":"abort"}"#).unwrap();
            b.wait_terminal(id, Duration::from_secs(3));
        }
        fs::write(&release, "go").unwrap();
        let changed = checked(child.wait_with_output().unwrap(), 0);
        assert_eq!(
            changed["data"]["route"],
            if end_job { "rpc" } else { "job" }
        );
        assert_eq!(
            b.fake_log()
                .iter()
                .filter(|r| r["event"] == "start")
                .count(),
            if end_job { 2 } else { 1 }
        );
        if !end_job {
            terminate_job(&b, id);
        } else {
            b.wait_worker_gone(id);
        }
    }
}
#[test]
fn ac356_queued_jobs_and_ac365_legacy_holder_fallback() {
    let b = RpcBox::new();
    let first = launch(&b);
    let first_id = job_id(&first);
    accepted(&b, first_id);
    // An old worker may hold a lock without a holder record. The single active,
    // non-waiting live worker is the only valid fallback.
    fs::write(glen::jobs::lock::lock_path(&home(&b), A_ID), b"").unwrap();
    let second_ready = b.omp.tmp.path().join("second-ready");
    let second = checked(
        command(&b, &["send", "e27c0de", "second", "--no-wait", "--json"])
            .env("FAKE_OMP_READY_RELEASE", &second_ready)
            .env(
                "FAKE_OMP_RUN_RELEASE",
                b.omp.tmp.path().join("second-release"),
            )
            .output()
            .unwrap(),
        10,
    );
    let second_id = job_id(&second);
    poll(Duration::from_secs(2), || {
        (b.job(second_id).1["waiting_for_lock"] == true).then_some(())
    });
    let listed = run(&b, &["models", "e27c0de", "--json"], 0);
    assert_eq!(listed["data"]["route"], "job");
    let aborted = run(&b, &["abort", "e27c0de", "--timeout", "15s", "--json"], 0);
    assert!(warning(&aborted, "jobs_queued"));
    assert!(
        aborted["data"]["queued_jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == &json!(b.job(second_id).0["handle"]))
    );
    b.wait_worker_gone(first_id);
    fs::write(second_ready, "ready").unwrap();
    accepted(&b, second_id);
    terminate_job(&b, second_id);
}
#[test]
#[cfg(unix)]
fn abort_reports_the_pre_abort_queue_even_after_the_next_job_is_accepted() {
    use rustix::process::{Pid, Signal, kill_process};
    struct Paused(Pid);
    impl Paused {
        fn new(pid: u32) -> Self {
            let pid = Pid::from_raw(pid as i32).unwrap();
            kill_process(pid, Signal::STOP).unwrap();
            Self(pid)
        }
    }
    impl Drop for Paused {
        fn drop(&mut self) {
            let _ = kill_process(self.0, Signal::CONT);
        }
    }
    struct Abort(std::process::Child);
    impl Drop for Abort {
        fn drop(&mut self) {
            if self.0.try_wait().unwrap().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    let b = RpcBox::new();
    let first = launch(&b);
    let first_id = job_id(&first);
    accepted(&b, first_id);
    let second = checked(
        command(&b, &["send", "e27c0de", "second", "--no-wait", "--json"])
            .env(
                "FAKE_OMP_RUN_RELEASE",
                b.omp.tmp.path().join("second-release"),
            )
            .output()
            .unwrap(),
        10,
    );
    let second_id = job_id(&second);
    poll(Duration::from_secs(3), || {
        let status = b.job(second_id).1;
        (status["waiting_for_lock"] == true && status["status"] == "queued").then_some(())
    });
    let worker = Paused::new(b.job(first_id).1["worker_pid"].as_u64().unwrap() as u32);
    let mut abort = Abort(
        command(&b, &["abort", "e27c0de", "--timeout", "15s", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let requests = b.job_dir(first_id).join("control");
    poll(Duration::from_secs(3), || {
        fs::read_dir(&requests)
            .ok()?
            .filter_map(Result::ok)
            .any(|entry| {
                fs::read(entry.path())
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .is_some_and(|request| request["op"] == "abort")
            })
            .then_some(())
    });
    // execute() is waiting on j1's answer. Keep the caller descheduled until j2 has
    // actually acquired the released lock, so a post-answer queue scan cannot see it.
    let caller = Paused::new(abort.0.id());
    drop(worker);
    assert_eq!(
        b.wait_terminal(first_id, Duration::from_secs(5))["status"],
        "aborted"
    );
    accepted(&b, second_id);
    assert!(abort.0.try_wait().unwrap().is_none());
    drop(caller);
    let status = poll(Duration::from_secs(3), || abort.0.try_wait().unwrap());
    use std::io::Read;
    let mut stdout = vec![];
    let mut stderr = vec![];
    abort
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    abort
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .unwrap();
    let result = checked(
        Output {
            status,
            stdout,
            stderr,
        },
        0,
    );
    assert_eq!(result["data"]["route"], "job");
    assert_eq!(result["data"]["job_status"], "aborted");
    assert_eq!(
        result["data"]["queued_jobs"],
        json!([b.job(second_id).0["handle"]])
    );
    assert!(warning(&result, "jobs_queued"));
    b.wait_worker_gone(first_id);
    terminate_job(&b, second_id);
}

#[test]
fn ac363_ready_deadline_stops_without_mutation() {
    let b = RpcBox::new();
    let start = Instant::now();
    let value = checked(
        command(
            &b,
            &["thinking", "e27c0de", "high", "--timeout", "2s", "--json"],
        )
        .env(
            "FAKE_OMP_READY_RELEASE",
            b.omp.tmp.path().join("never-ready"),
        )
        .output()
        .unwrap(),
        7,
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(value["error"]["details"]["reason"], "timeout");
    assert!(b.frames_in("set_thinking_level").is_empty());
    for start in b.fake_log().into_iter().filter(|r| r["event"] == "start") {
        poll(Duration::from_secs(3), || {
            matches!(
                glen::jobs::liveness::start_token(start["pid"].as_u64().unwrap() as u32),
                glen::jobs::liveness::Probe::Gone
            )
            .then_some(())
        });
    }
}
#[test]
fn fakes_cover_every_op() {
    use glen::ops::{CompactArgs, Op, SetModelArgs, ThinkingArgs};
    let b = RpcBox::new();
    let mut cmd = std::process::Command::new(&b.fake);
    cmd.args(["--mode", "rpc", "--no-ui", "--resume"])
        .arg(b.omp.session('A'))
        .env("FAKE_OMP_SERIAL", "1")
        .env("GLEN_MANAGED", "1");
    let (client, _events) = omp_rpc::Client::spawn(
        cmd,
        omp_rpc::ClientOptions {
            tools: vec![],
            uris: vec![],
            ready_timeout: Duration::from_secs(3),
            default_timeout: Duration::from_secs(2),
        },
    )
    .unwrap();
    for op in [
        Op::ListModels,
        Op::SetModel(SetModelArgs {
            provider: "fake".into(),
            model_id: "fake-2".into(),
        }),
        Op::SetThinkingLevel(ThinkingArgs {
            level: "off".into(),
        }),
        Op::Compact(CompactArgs::default()),
    ] {
        let value = glen::ctl::exec::exec(&client, &op, Some(A_ID)).unwrap();
        assert_eq!(value["session_id"], A_ID);
    }
    client.call(&omp_rpc::AbortCommand {}).unwrap();
    client.close().unwrap();
    let mut live = LiveBox::new();
    live.start_ext(A_ID, "ok", &[]);
    let registry = match glen::live::registry::read(&home(&live.rpc), A_ID) {
        glen::live::registry::Read::Ok(reg) => reg,
        _ => panic!("missing fake registry"),
    };
    for op in [
        Op::Abort,
        Op::ListModels,
        Op::SetModel(SetModelArgs {
            provider: "fake".into(),
            model_id: "fake-2".into(),
        }),
        Op::SetThinkingLevel(ThinkingArgs {
            level: "off".into(),
        }),
        Op::Compact(CompactArgs::default()),
    ] {
        assert_eq!(
            glen::live::client::unary(&registry.socket, &op, Duration::from_secs(3)).unwrap()["session_id"],
            A_ID
        );
    }
}
#[cfg(debug_assertions)]
#[test]
fn ac352_endpoint_after_lock_wait_and_live_lock_conflict() {
    let mut live = LiveBox::new();
    let lock = hold_glen_lock_as(&home(&live.rpc), A_ID, None);
    let child = command(
        &live.rpc,
        &[
            "model",
            "e27c0de",
            "fake/fake-2",
            "--timeout",
            "8s",
            "--json",
        ],
    )
    .env("FAKE_CTL_LOCK_WAIT_GATE", "1")
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    let gate = live.rpc.omp.glen_home.join("run/locks").join(format!(
        ".{}.{}.control-waiting",
        glen::jobs::lock::lock_key(A_ID),
        child.id()
    ));
    poll(Duration::from_secs(3), || gate.exists().then_some(()));
    live.start_ext(A_ID, "ok", &[]);
    drop(lock);
    let value = checked(child.wait_with_output().unwrap(), 0);
    assert_eq!(value["data"]["route"], "live");
    assert!(live.rpc.fake_log().is_empty());
    let _held = live.rpc.hold_glen_lock(A_ID);
    run(&live.rpc, &["thinking", "e27c0de", "off", "--json"], 6);
}
#[test]
fn ac358_endpoint_only_fallback_and_ac363_live_timeout() {
    let endpoint_only = "01a0f6b1-2680-7a3c-8b21-4a1f97777777";
    {
        let mut live = LiveBox::new();
        live.start_ext(endpoint_only, "ok", &[("FAKE_EXT_OPS", "w3")]);
        let value = run(&live.rpc, &["models", "7777777", "--json"], 0);
        assert!(value["data"]["current"].is_null());
        assert!(value["data"]["current_source"].is_null());
        assert!(warning(&value, "not_yet_persisted"));
    }
    {
        let mut live = LiveBox::new();
        live.start_ext(A_ID, "ok", &[("FAKE_EXT_COMPACT_DELAY_MS", "5000")]);
        let start = Instant::now();
        let value = run(
            &live.rpc,
            &["compact", "e27c0de", "--timeout", "1s", "--json"],
            7,
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(value["error"]["details"]["reason"], "timeout");
        assert!(
            value["error"]["hint"]
                .as_str()
                .unwrap()
                .contains("may still finish")
        );
    }
}
#[test]
fn ac356_abort_pending_and_ac362_live_busy() {
    {
        let mut live = LiveBox::new();
        live.start_ext(A_ID, "busy", &[("FAKE_EXT_ABORT_STUCK", "1")]);
        let value = run(&live.rpc, &["abort", "e27c0de", "--json"], 0);
        assert!(warning(&value, "abort_pending"));
        run(&live.rpc, &["compact", "e27c0de", "--json"], 7);
    }
    {
        let mut live = LiveBox::new();
        live.start_ext(A_ID, "busy", &[]);
        let output = command(&live.rpc, &["model", "e27c0de", "fake/fake-2", "--human"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("next model request")
        );
    }
}

#[test]
fn ac352_unresponsive_live_endpoint_and_ac363_whole_hello_deadline() {
    let mut live = LiveBox::new();
    live.start_ext(A_ID, "hang", &[]);
    let start = Instant::now();
    let value = run(
        &live.rpc,
        &["abort", "e27c0de", "--timeout", "500ms", "--json"],
        7,
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(value["error"]["details"]["reason"], "timeout");
    let start = Instant::now();
    run(
        &live.rpc,
        &[
            "model",
            "e27c0de",
            "fake/fake-2",
            "--timeout",
            "5s",
            "--json",
        ],
        6,
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(live.rpc.fake_log().is_empty());
    assert!(
        live.ext_log()
            .iter()
            .filter(|r| r["dir"] == "in")
            .all(|r| r["frame"]["op"] == "hello")
    );
}

#[test]
fn fix1_terminal_job_waits_for_in_flight_ack_without_duplicate_model_change() {
    let b = RpcBox::new();
    let before = session_lines(&b.omp.session('A')).len();
    let release = b.omp.tmp.path().join("release");
    let launched = checked(
        command(&b, &["send", "e27c0de", "hold", "--no-wait", "--json"])
            .env("FAKE_OMP_RUN_RELEASE", &release)
            .env("FAKE_OMP_STATE_DELAY_MS", "950")
            .output()
            .unwrap(),
        10,
    );
    let id = job_id(&launched);
    accepted(&b, id);
    let child = command(&b, &["model", "e27c0de", "fake/fake-2", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    fs::write(release, "go").unwrap();
    let changed = checked(child.wait_with_output().unwrap(), 0);
    assert_eq!(changed["data"]["route"], "job");
    b.wait_worker_gone(id);
    assert_eq!(b.frames_in("set_model").len(), 1);
    assert_eq!(
        session_lines(&b.omp.session('A'))
            .iter()
            .skip(before)
            .filter(|line| line["type"] == "model_change")
            .count(),
        1
    );
}

#[test]
fn fix2_inbox_preflight_past_request_deadline_never_sets_model() {
    let b = RpcBox::new();
    let launched = checked(
        command(&b, &["send", "e27c0de", "hold", "--no-wait", "--json"])
            .env("FAKE_OMP_RUN_RELEASE", b.omp.tmp.path().join("release"))
            .env("FAKE_OMP_STATE_DELAY_MS", "1500")
            .output()
            .unwrap(),
        10,
    );
    let id = job_id(&launched);
    accepted(&b, id);
    let before = b.frames_in("get_state").len();
    let error = run(
        &b,
        &[
            "model",
            "e27c0de",
            "fake/fake-2",
            "--timeout",
            "500ms",
            "--json",
        ],
        7,
    );
    assert_eq!(error["error"]["details"]["reason"], "timeout");
    assert!(b.frames_in("get_state").len() > before);
    let dir = b.job_dir(id).join("control");
    let ack = poll(Duration::from_secs(5), || {
        fs::read_dir(&dir).ok()?.flatten().find_map(|entry| {
            let path = entry.path();
            path.to_string_lossy()
                .ends_with(".done.json")
                .then(|| serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap())
        })
    });
    assert_eq!(ack["error"]["code"], "timeout");
    assert!(b.frames_in("set_model").is_empty());
    terminate_job(&b, id);
}

#[test]
fn fix7_closed_in_flight_control_after_terminal_retries_once() {
    let b = RpcBox::new();
    let launched = checked(
        command(&b, &["send", "e27c0de", "hold", "--no-wait", "--json"])
            .env("FAKE_OMP_RUN_RELEASE", b.omp.tmp.path().join("release"))
            .env("FAKE_OMP_STATE_DELAY_MS", "8000")
            .output()
            .unwrap(),
        10,
    );
    let id = job_id(&launched);
    accepted(&b, id);
    let before = b.frames_in("get_state").len();
    let child = command(
        &b,
        &[
            "model",
            "e27c0de",
            "fake/fake-2",
            "--timeout",
            "25s",
            "--json",
        ],
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    poll(Duration::from_secs(3), || {
        (b.frames_in("get_state").len() > before).then_some(())
    });
    fs::write(
        b.job_dir(id).join("control/stop.json"),
        r#"{"v":1,"op":"abort"}"#,
    )
    .unwrap();
    b.wait_terminal(id, Duration::from_secs(13));
    let fake_pid = b.fake_log().iter().find(|r| r["event"] == "start").unwrap()["pid"]
        .as_u64()
        .unwrap();
    // Closing the RPC peer unblocks the control while the worker joins it.
    let pid = rustix::process::Pid::from_raw(fake_pid as i32).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::KILL).unwrap();
    let changed = checked(child.wait_with_output().unwrap(), 0);
    assert_eq!(changed["data"]["route"], "rpc");
    assert_eq!(b.frames_in("set_model").len(), 1);
    assert_eq!(
        b.fake_log()
            .iter()
            .filter(|r| r["event"] == "start")
            .count(),
        2
    );
    b.wait_worker_gone(id);
}

#[test]
fn fix8_terminal_lock_holder_waits_or_reports_finishing() {
    for release in [true, false] {
        let b = RpcBox::new();
        let launched = launch(&b);
        let id = job_id(&launched);
        accepted(&b, id);
        terminate_job(&b, id);
        let identity = glen::jobs::liveness::current().unwrap();
        let lock = hold_glen_lock_as(
            &home(&b),
            A_ID,
            Some(Holder {
                v: 1,
                kind: HolderKind::Job,
                job_id: Some(id.into()),
                pid: identity.pid,
                start: Some(identity.start),
            }),
        );
        let mut child = command(&b, &["abort", "e27c0de", "--timeout", "5s", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        if release {
            std::thread::sleep(Duration::from_millis(400));
            assert!(child.try_wait().unwrap().is_none());
            drop(lock);
            let result = checked(child.wait_with_output().unwrap(), 0);
            assert_eq!(result["data"]["was"], "idle");
        } else {
            let error = checked(child.wait_with_output().unwrap(), 7);
            assert_eq!(error["error"]["details"]["reason"], "busy");
            assert!(
                error["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("a glen job is finishing; retry")
            );
            assert!(start.elapsed() >= Duration::from_millis(1900));
            assert!(start.elapsed() < Duration::from_secs(4));
            drop(lock);
        }
    }
}

#[test]
fn fix3_startup_cancel_reaps_omp_before_releasing_session_lock() {
    for (interrupt, baseline) in [(false, false), (true, false), (false, true), (true, true)] {
        let b = RpcBox::new();
        let mut child = command(
            &b,
            &[
                "model",
                "e27c0de",
                "fake/fake-2",
                "--timeout",
                if interrupt { "30s" } else { "1500ms" },
                "--json",
            ],
        )
        // The ready sleep requires KILL; delayed get_state observes TERM.
        .env(
            "FAKE_OMP_READY_DELAY_MS",
            if baseline { "0" } else { "60000" },
        )
        .env(
            "FAKE_OMP_BASELINE_DELAY_MS",
            if baseline { "60000" } else { "0" },
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
        let pid = poll(Duration::from_secs(5), || {
            b.fake_log().iter().find(|r| r["event"] == "start")?["pid"].as_u64()
        });
        let pid = rustix::process::Pid::from_raw(pid as i32).unwrap();
        if baseline {
            poll(Duration::from_secs(1), || {
                (!b.frames_in("get_state").is_empty()).then_some(())
            });
        }
        assert!(
            glen::jobs::lock::try_acquire(&home(&b), A_ID)
                .unwrap()
                .is_none()
        );
        if interrupt {
            rustix::process::kill_process(
                rustix::process::Pid::from_raw(child.id() as i32).unwrap(),
                rustix::process::Signal::INT,
            )
            .unwrap();
        }
        let start = Instant::now();
        // Observe the actual lock continuously, including while TERM is pending.
        let lock = poll(Duration::from_secs(5), || {
            let lock = glen::jobs::lock::try_acquire(&home(&b), A_ID).unwrap()?;
            assert_eq!(
                rustix::process::test_kill_process_group(pid),
                Err(rustix::io::Errno::SRCH),
                "session lock became free while the omp process group survived"
            );
            assert!(matches!(
                glen::jobs::liveness::start_token(pid.as_raw_nonzero().get() as u32),
                glen::jobs::liveness::Probe::Gone
            ));
            Some(lock)
        });
        poll(Duration::from_secs(5), || child.try_wait().unwrap());
        let value = checked(
            child.wait_with_output().unwrap(),
            if interrupt { 130 } else { 7 },
        );
        if interrupt {
            assert_eq!(value["data"]["interrupted"], true);
            assert_eq!(value["data"]["status"], "interrupted");
        } else {
            assert_eq!(value["error"]["details"]["reason"], "timeout");
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(b.frames_in("set_model").is_empty());
        drop(lock);
    }
}

#[test]
fn fix3_ephemeral_teardown_closes_transport_before_escalating() {
    for args in [
        vec!["models", "--json"],
        vec!["models", "e27c0de", "--json"],
        vec!["model", "e27c0de", "fake/fake-2", "--json"],
        vec!["thinking", "e27c0de", "low", "--json"],
        vec!["compact", "e27c0de", "--json"],
    ] {
        let b = RpcBox::new();
        // This peer ignores TERM, so killing before stdin EOF prevents its exit log.
        let value = checked(
            command(&b, &args)
                .env("FAKE_OMP_SCENARIO", "wedge")
                .output()
                .unwrap(),
            0,
        );
        assert!(!warning(&value, "teardown_error"), "{args:?}: {value}");
        let log = b.fake_log();
        assert!(
            log.iter()
                .any(|r| r["event"] == "exit" && r["reason"] == "stdin eof"),
            "peer did not exit on EOF for {args:?}: {log:?}"
        );
        let pid = log.iter().find(|r| r["event"] == "start").unwrap()["pid"]
            .as_u64()
            .unwrap() as u32;
        assert!(matches!(
            glen::jobs::liveness::start_token(pid),
            glen::jobs::liveness::Probe::Gone
        ));
        if args[0] != "models" {
            assert!(
                glen::jobs::lock::try_acquire(&home(&b), A_ID)
                    .unwrap()
                    .is_some()
            );
        }
    }
}

#[test]
fn fix4_startup_stderr_is_drained_into_json_warnings() {
    for args in [
        vec!["model", "e27c0de", "fake/fake-2", "--json"],
        vec!["models", "--json"],
    ] {
        let b = RpcBox::new();
        // More than a pipe buffer, with an unterminated final line, must neither
        // block startup nor leak to stderr or produce unbounded warnings.
        let message = format!(
            "startup diagnostic\n{}\nlast diagnostic",
            "x".repeat(100_000)
        );
        let value = checked(
            command(&b, &args)
                .env("FAKE_OMP_STARTUP_STDERR", &message)
                .output()
                .unwrap(),
            0,
        );
        let warnings = value["warnings"].as_array().unwrap();
        let stderr = warnings
            .iter()
            .filter(|w| w["code"] == "omp_stderr")
            .collect::<Vec<_>>();
        assert_eq!(stderr.len(), 3);
        assert_eq!(stderr[0]["message"], "startup diagnostic");
        assert_eq!(stderr[1]["message"].as_str().unwrap().len(), 1024);
        assert_eq!(stderr[2]["message"], "last diagnostic");
        assert!(!warning(&value, "teardown_error"));
    }
}

#[test]
fn c2_3_startup_failure_surfaces_stderr_in_details_and_message() {
    for args in [
        vec!["model", "e27c0de", "fake/fake-2", "--json"],
        vec!["models", "--json"],
    ] {
        for lines in [
            vec!["fatal startup error".to_owned()],
            (0..20).map(|i| format!("fatal {i}")).collect(),
        ] {
            let b = RpcBox::new();
            let value = checked(
                command(&b, &args)
                    .env("FAKE_OMP_STARTUP_STDERR", lines.join("\n"))
                    .env("FAKE_OMP_SCENARIO", "startup_exit")
                    .output()
                    .unwrap(),
                5,
            );
            let captured = &lines[..lines.len().min(16)];
            assert_eq!(value["error"]["code"], "omp_unavailable");
            assert_eq!(value["error"]["details"]["stderr"], json!(captured));
            let message = value["error"]["message"].as_str().unwrap();
            assert!(
                message.starts_with("cannot start omp for session control: "),
                "{value}"
            );
            assert!(
                message.ends_with(&format!(": {}", captured.last().unwrap())),
                "{value}"
            );
        }
    }
    let b = RpcBox::new();
    let out = command(&b, &["models", "--human"])
        .env("FAKE_OMP_STARTUP_STDERR", "fatal startup error\n")
        .env("FAKE_OMP_SCENARIO", "startup_exit")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&out.stderr).contains("fatal startup error"));
}

#[test]
fn c2_3_startup_deadline_keeps_stderr_and_timeout_reason() {
    for baseline in [false, true] {
        let b = RpcBox::new();
        let value = checked(
            command(&b, &["models", "--timeout", "1s", "--json"])
                .env("FAKE_OMP_STARTUP_STDERR", "startup diagnostic\n")
                .env(
                    "FAKE_OMP_READY_DELAY_MS",
                    if baseline { "0" } else { "60000" },
                )
                .env(
                    "FAKE_OMP_BASELINE_DELAY_MS",
                    if baseline { "60000" } else { "0" },
                )
                .output()
                .unwrap(),
            7,
        );
        assert_eq!(value["error"]["details"]["reason"], "timeout");
        assert_eq!(
            value["error"]["details"]["stderr"],
            json!(["startup diagnostic"])
        );
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .ends_with(": startup diagnostic")
        );
    }
}

#[test]
fn c2_1_disconnect_after_submission_reports_unknown_without_retry() {
    for drop_at in ["before_reply", "after_reply"] {
        let b = RpcBox::new();
        let value = checked(
            command(&b, &["model", "e27c0de", "fake/fake-2", "--json"])
                .env("FAKE_OMP_SET_MODEL_DROP", drop_at)
                .output()
                .unwrap(),
            7,
        );
        assert_eq!(value["error"]["code"], "control_failed");
        assert_eq!(value["error"]["details"]["reason"], "outcome_unknown");
        assert_eq!(
            value["error"]["hint"],
            "the change may have been applied; check `glen models e27c0de`"
        );
        assert_eq!(b.frames_in("set_model").len(), 1);
        assert_eq!(
            b.fake_log()
                .iter()
                .filter(|r| r["event"] == "start")
                .count(),
            1
        );
    }
}

#[test]
fn c2_1_terminal_job_submitted_control_never_retries() {
    let b = RpcBox::new();
    let launched = checked(
        command(&b, &["send", "e27c0de", "hold", "--no-wait", "--json"])
            .env("FAKE_OMP_RUN_RELEASE", b.omp.tmp.path().join("release"))
            .env("FAKE_OMP_SET_MODEL_DELAY_MS", "20000")
            .output()
            .unwrap(),
        10,
    );
    let id = job_id(&launched);
    accepted(&b, id);
    let child = command(
        &b,
        &[
            "model",
            "e27c0de",
            "fake/fake-2",
            "--timeout",
            "25s",
            "--json",
        ],
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    poll(Duration::from_secs(3), || {
        (!b.frames_in("set_model").is_empty()).then_some(())
    });
    fs::write(
        b.job_dir(id).join("control/stop.json"),
        r#"{"v":1,"op":"abort"}"#,
    )
    .unwrap();
    b.wait_terminal(id, Duration::from_secs(13));
    let error = checked(child.wait_with_output().unwrap(), 7);
    assert_eq!(error["error"]["code"], "control_failed");
    assert_eq!(error["error"]["details"]["reason"], "outcome_unknown");
    assert_eq!(b.frames_in("set_model").len(), 1);
    assert_eq!(
        b.fake_log()
            .iter()
            .filter(|r| r["event"] == "start")
            .count(),
        1
    );
    b.wait_worker_gone(id);
}

#[test]
fn c2_2_sigint_during_preflight_exits_130_without_set_model() {
    let b = RpcBox::new();
    let child = command(
        &b,
        &[
            "model",
            "e27c0de",
            "fake/fake-2",
            "--timeout",
            "30s",
            "--json",
        ],
    )
    .env("FAKE_OMP_STATE_DELAY_AT", "4")
    .env("FAKE_OMP_PREFLIGHT_DELAY_MS", "20000")
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    poll(Duration::from_secs(3), || {
        (b.frames_in("get_state").len() == 4).then_some(())
    });
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(child.id() as i32).unwrap(),
        rustix::process::Signal::INT,
    )
    .unwrap();
    let value = checked(child.wait_with_output().unwrap(), 130);
    assert_eq!(value["data"]["interrupted"], true);
    assert!(b.frames_in("set_model").is_empty());
}

#[test]
fn c2_4_compact_timeout_unblocks_teardown_promptly() {
    let b = RpcBox::new();
    let start = Instant::now();
    let value = checked(
        command(&b, &["compact", "e27c0de", "--timeout", "1s", "--json"])
            .env("FAKE_OMP_COMPACT_DELAY_MS", "20000")
            .output()
            .unwrap(),
        7,
    );
    assert!(
        start.elapsed() < Duration::from_millis(1500),
        "elapsed: {:?}",
        start.elapsed()
    );
    assert_eq!(value["error"]["details"]["reason"], "timeout");
    assert_eq!(b.frames_in("compact").len(), 1);
    let pid = b.fake_log().iter().find(|r| r["event"] == "start").unwrap()["pid"]
        .as_u64()
        .unwrap();
    assert_eq!(
        rustix::process::test_kill_process_group(
            rustix::process::Pid::from_raw(pid as i32).unwrap()
        ),
        Err(rustix::io::Errno::SRCH)
    );
}

#[test]
fn c2_7_inbox_controls_cannot_outlive_job_deadline() {
    for request_deadline in [Some(json!(u64::MAX)), None, Some(json!("malformed"))] {
        let b = RpcBox::new();
        let launched = checked(
            command(&b, &["send", "e27c0de", "hold", "--no-wait", "--json"])
                .env("GLEN_JOB_TIMEOUT", "2s")
                .env("FAKE_OMP_RUN_RELEASE", b.omp.tmp.path().join("release"))
                .env("FAKE_OMP_STATE_DELAY_MS", "3000")
                .env("FAKE_OMP_SERIAL", "0")
                .output()
                .unwrap(),
            10,
        );
        let id = job_id(&launched);
        accepted(&b, id);
        let dir = b.job_dir(id).join("control");
        fs::create_dir_all(&dir).unwrap();
        let mut request = json!({"v":1,"op":"set_thinking_level","args":{"level":"high"}});
        let malformed = request_deadline.as_ref().is_some_and(Value::is_string);
        if let Some(deadline) = request_deadline {
            request["deadline_ms"] = deadline;
        }
        fs::write(
            dir.join("c2-deadline.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        let ack = poll(Duration::from_secs(8), || {
            fs::read(dir.join("c2-deadline.done.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        });
        assert_eq!(
            ack["error"]["code"],
            if malformed { "bad_request" } else { "timeout" }
        );
        assert!(b.frames_in("set_thinking_level").is_empty());
        b.wait_terminal(id, Duration::from_secs(8));
        b.wait_worker_gone(id);
    }
}

#[test]
fn c2_9_success_after_apply_deadline_reaches_cli_with_warning() {
    let b = RpcBox::new();
    let value = checked(
        command(
            &b,
            &[
                "model",
                "e27c0de",
                "fake/fake-2",
                "--timeout",
                "1s",
                "--json",
            ],
        )
        // Fifth state request is the post-mutation read (after baseline and listing).
        .env("FAKE_OMP_STATE_DELAY_AT", "5")
        .env("FAKE_OMP_PREFLIGHT_DELAY_MS", "1200")
        .output()
        .unwrap(),
        0,
    );
    assert_eq!(value["data"]["model"]["id"], "fake-2");
    assert_eq!(
        value["data"]["warnings"][0]["code"],
        "deadline_passed_after_apply"
    );
    assert_eq!(b.frames_in("set_model").len(), 1);
}
