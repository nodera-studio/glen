//! v0.2 W3 ACP spike (design §A23): the hidden `glen __acp` on the real binary against the
//! `fake_omp acp` peer (AC-v2-301 to AC-v2-303). Every omp is the fake, with the box's own HOME.
mod common;
use common::*;
use glen::{glen_home::GlenHome, jobs::lock};
use rustix::process::{Pid, Signal};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Child, Output, Stdio},
    time::{Duration, Instant, SystemTime},
};

fn run(b: &RpcBox, args: &[&str], vars: &[(&str, &str)]) -> Output {
    let mut full = vec!["__acp"];
    full.extend_from_slice(args);
    full.push("--json");
    b.glen(&full).envs(vars.iter().copied()).output().unwrap()
}
fn spawn(b: &RpcBox, args: &[&str], vars: &[(&str, &str)]) -> Child {
    let mut full = vec!["__acp"];
    full.extend_from_slice(args);
    full.push("--json");
    b.glen(&full)
        .envs(vars.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}
/// The JSON envelope of an exit; JSON mode never writes stderr.
fn envelope(out: &Output, code: i32) -> Value {
    assert_eq!(out.status.code(), Some(code), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    json(out)
}
fn failure(out: &Output, code: i32, error: &str, reason: &str) -> Value {
    let v = envelope(out, code);
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["error"]["code"], error, "{v}");
    assert_eq!(v["error"]["details"]["reason"], reason, "{v}");
    v["error"].clone()
}
fn home(b: &RpcBox) -> GlenHome {
    GlenHome {
        root: b.omp.glen_home.clone(),
    }
}
/// ESRCH alone means the group is gone: macOS answers EPERM while exited members are unreaped
/// (the `group_gone` rule of `src/ctl/ephemeral.rs`).
fn group_gone(pgid: i64) -> bool {
    Pid::from_raw(pgid as i32).is_some_and(|pid| {
        rustix::process::test_kill_process_group(pid) == Err(rustix::io::Errno::SRCH)
    })
}
fn assert_groups_gone(b: &RpcBox) {
    let starts: Vec<_> = b
        .fake_log()
        .into_iter()
        .filter(|r| r["event"] == "start")
        .collect();
    for start in starts {
        let pgid = start["pgid"].as_i64().unwrap();
        assert_eq!(start["pid"].as_i64(), Some(pgid), "omp leads its own group");
        poll(Duration::from_secs(3), || group_gone(pgid).then_some(()));
    }
}
/// AC-v2-303: no durable job, no inbox; the only glen state is the session lock.
fn assert_no_job(b: &RpcBox) {
    for dir in ["jobs", "inbox"] {
        assert!(!b.omp.glen_home.join(dir).exists(), "{dir} was created");
    }
}
/// Names, sizes and mtimes of every file under the fixture sessions tree.
fn sessions_tree(b: &RpcBox) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, (u64, SystemTime)>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let meta = e.metadata().unwrap();
            if meta.is_dir() {
                walk(&e.path(), out);
            } else {
                out.insert(e.path(), (meta.len(), meta.modified().unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&b.omp.agent.join("sessions"), &mut out);
    out
}
fn frames_in(b: &RpcBox, method: &str) -> Vec<Value> {
    b.fake_log()
        .into_iter()
        .filter(|r| r["event"] == "in" && r["frame"]["method"] == method)
        .collect()
}
fn signal(child: &Child, signal: Signal) -> u128 {
    let at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    rustix::process::kill_process(Pid::from_raw(child.id() as i32).unwrap(), signal).unwrap();
    at
}
/// Waits until the fake has written its first prompt chunk ("Hel").
fn wait_first_chunk(b: &RpcBox) {
    poll(Duration::from_secs(10), || {
        b.fake_log()
            .iter()
            .any(|r| {
                r["event"] == "out"
                    && r["frame"]["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
            })
            .then_some(())
    });
}

#[test]
fn ac_v2_302_303_new_session_turn_rebuilds_the_last_message() {
    let b = RpcBox::new();
    let before = sessions_tree(&b);
    let out = run(&b, &["@new", "ping pong"], &[]);
    let v = envelope(&out, 0);
    assert_eq!(v["ok"], true);
    let d = &v["data"];
    let mut keys: Vec<_> = d.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "agent",
            "assistant_text",
            "cancel",
            "events",
            "events_dropped",
            "permission",
            "protocol_version",
            "session",
            "stop_reason",
            "task_state",
            "text_truncated",
            "transport",
            "updates",
            "usage"
        ]
    );
    assert_eq!(d["transport"], "acp");
    assert_eq!(d["protocol_version"], 1);
    assert_eq!(d["agent"], json!({"name":"omp","version":"18.14.0"}));
    assert_eq!(d["permission"], "deny");
    // Only the last message group: "Hello " (m1) is not part of it.
    assert_eq!(d["assistant_text"], "reply: ping pong");
    assert_eq!(d["stop_reason"], "end_turn");
    assert_eq!(d["task_state"], "completed");
    assert_eq!(d["text_truncated"], false);
    assert_eq!(d["events"], json!([]));
    assert_eq!(d["events_dropped"], 0);
    assert_eq!(d["cancel"], Value::Null);
    assert_eq!(d["usage"]["totalTokens"], 3);
    assert_eq!(d["updates"]["replayed"], 0);
    assert_eq!(d["updates"]["during_turn"], 7);
    assert_eq!(d["updates"]["messages"], 2);
    assert_eq!(
        d["updates"]["kinds"],
        json!({"agent_message_chunk":4,"agent_thought_chunk":1,"tool_call":1,"tool_call_update":1})
    );
    let id = d["session"]["id"].as_str().unwrap();
    assert_eq!(d["session"]["created"], true);
    assert_eq!(d["session"]["path"], Value::Null);
    assert_eq!(d["session"]["handle"], id.replace('-', "")[25..]);
    let log = b.fake_log();
    let start = log.iter().find(|r| r["event"] == "start").unwrap();
    assert_eq!(start["argv"], json!(["acp"]));
    assert_eq!(start["env"]["GLEN_MANAGED"], "1");
    let init = &frames_in(&b, "initialize")[0]["frame"];
    assert_eq!(init["jsonrpc"], "2.0");
    assert_eq!(init["params"]["protocolVersion"], 1);
    assert_eq!(
        init["params"]["clientCapabilities"],
        json!({"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false})
    );
    assert_eq!(init["params"]["clientInfo"]["name"], "glen");
    let new = &frames_in(&b, "session/new")[0]["frame"]["params"];
    assert_eq!(new["mcpServers"], json!([]));
    assert_eq!(Path::new(new["cwd"].as_str().unwrap()), b.omp.alpha());
    assert_eq!(
        frames_in(&b, "session/close")[0]["frame"]["params"]["sessionId"],
        id
    );
    assert!(frames_in(&b, "session/load").is_empty());
    // The lock file stays (it is never unlinked) and is free again; nothing else changed.
    assert!(lock::lock_path(&home(&b), id).is_file());
    assert!(lock::try_acquire(&home(&b), id).unwrap().is_some());
    assert_no_job(&b);
    assert_eq!(sessions_tree(&b), before);
    assert_groups_gone(&b);
}

#[test]
fn ac_v2_303_an_explicit_profile_reaches_omp_and_drops_the_env_profiles() {
    let b = RpcBox::new();
    let out = b
        .glen(&["--profile", "p", "__acp", "@new", "hi", "--json"])
        .env("OMP_PROFILE", "other")
        .env("PI_PROFILE", "other")
        .output()
        .unwrap();
    envelope(&out, 0);
    let start = b
        .fake_log()
        .into_iter()
        .find(|r| r["event"] == "start")
        .unwrap();
    assert_eq!(start["argv"], json!(["acp", "--profile", "p"]));
    assert_eq!(start["env"]["OMP_PROFILE"], Value::Null);
    assert_eq!(start["env"]["PI_PROFILE"], Value::Null);
    assert_eq!(start["env"]["GLEN_MANAGED"], "1");
    assert_groups_gone(&b);
}

#[test]
fn ac_v2_302_303_existing_session_loads_and_excludes_the_replay() {
    let b = RpcBox::new();
    let before = sessions_tree(&b);
    let file = fs::read(b.omp.session('A')).unwrap();
    let out = run(&b, &["e27c0de", "hi"], &[("FAKE_OMP_ACP_REPLAY", "3")]);
    let d = envelope(&out, 0)["data"].clone();
    assert_eq!(d["assistant_text"], "reply: hi");
    assert!(d["updates"]["replayed"].as_u64().unwrap() >= 6, "{d}");
    assert_eq!(d["session"]["id"], A_ID);
    assert_eq!(d["session"]["handle"], "e27c0de");
    assert_eq!(d["session"]["created"], false);
    assert_eq!(
        Path::new(d["session"]["path"].as_str().unwrap()),
        b.omp.session('A')
    );
    let load = &frames_in(&b, "session/load")[0]["frame"]["params"];
    assert_eq!(load["sessionId"], A_ID);
    assert_eq!(load["mcpServers"], json!([]));
    // The fixture header's cwd is not a directory here, so glen's own (absolute) cwd is used.
    assert_eq!(Path::new(load["cwd"].as_str().unwrap()), b.omp.alpha());
    assert!(frames_in(&b, "session/new").is_empty());
    assert_eq!(fs::read(b.omp.session('A')).unwrap(), file);
    assert_eq!(sessions_tree(&b), before);
    assert!(lock::try_acquire(&home(&b), A_ID).unwrap().is_some());
    assert_no_job(&b);
    assert_groups_gone(&b);
}

#[test]
fn ac_v2_302_a_relative_stored_cwd_reaches_omp_absolute() {
    let b = RpcBox::new();
    let sub = b.omp.alpha().join("sub");
    fs::create_dir(&sub).unwrap();
    // The header names `sub`, which exists relative to glen's own cwd (alpha).
    let file = b.omp.session('A');
    let text = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        text.replacen(r#""cwd":"/home/fixture/work/alpha""#, r#""cwd":"sub""#, 1),
    )
    .unwrap();
    let out = run(&b, &["e27c0de", "hi"], &[]);
    let d = envelope(&out, 0)["data"].clone();
    assert_eq!(d["assistant_text"], "reply: hi");
    let load = &frames_in(&b, "session/load")[0]["frame"]["params"];
    assert_eq!(Path::new(load["cwd"].as_str().unwrap()), sub);
    // The same absolute path is omp's working directory.
    let start = b
        .fake_log()
        .into_iter()
        .find(|r| r["event"] == "start")
        .unwrap();
    assert_eq!(Path::new(start["cwd"].as_str().unwrap()), sub);
    assert_groups_gone(&b);
}

#[test]
fn ac_v2_302_the_initialize_result_must_be_protocol_1() {
    for (scenario, got) in [
        ("proto2", json!(2)),
        ("proto_missing", Value::Null),
        ("init_error", Value::Null),
    ] {
        let b = RpcBox::new();
        let out = run(&b, &["@new", "hi"], &[("FAKE_OMP_ACP", scenario)]);
        let e = failure(&out, 5, "omp_unavailable", "acp_protocol");
        assert_eq!(e["details"]["requested"], 1, "{scenario}");
        assert_eq!(e["details"]["got"], got, "{scenario}");
        assert!(frames_in(&b, "session/new").is_empty(), "{scenario}");
        assert_no_job(&b);
        assert_groups_gone(&b);
    }
}

#[test]
fn ac_v2_302_load_needs_the_capability_and_new_does_not() {
    let b = RpcBox::new();
    let out = run(&b, &["e27c0de", "hi"], &[("FAKE_OMP_ACP", "no_load")]);
    let e = failure(&out, 5, "omp_unavailable", "acp_capability");
    assert_eq!(e["details"]["capability"], "loadSession");
    assert!(frames_in(&b, "session/load").is_empty());
    assert_groups_gone(&b);
    let b = RpcBox::new();
    let out = run(&b, &["@new", "hi"], &[("FAKE_OMP_ACP", "no_load")]);
    let d = envelope(&out, 0)["data"].clone();
    assert_eq!(d["assistant_text"], "reply: hi");
    // No close capability, so no session/close.
    assert!(frames_in(&b, "session/close").is_empty());
    assert_groups_gone(&b);
}

#[test]
fn ac_v2_302_startup_failures_map_to_omp_unavailable() {
    let b = RpcBox::new();
    let out = run(
        &b,
        &["@new", "hi"],
        &[
            ("FAKE_OMP_ACP", "startup_exit"),
            ("FAKE_OMP_STARTUP_STDERR", "fatal: no credentials\n"),
        ],
    );
    let e = failure(&out, 5, "omp_unavailable", "acp_exited");
    assert_eq!(e["details"]["stderr"], json!(["fatal: no credentials"]));
    assert_groups_gone(&b);

    let b = RpcBox::new();
    let started = Instant::now();
    let out = run(
        &b,
        &["@new", "hi"],
        &[("FAKE_OMP_ACP", "init_hang"), ("GLEN_ACP_SETUP_MS", "300")],
    );
    failure(&out, 5, "omp_unavailable", "acp_timeout");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_groups_gone(&b);

    let b = RpcBox::new();
    let out = run(&b, &["@new", "hi"], &[("GLEN_OMP_BIN", "/nonexistent/omp")]);
    let e = failure(&out, 5, "omp_unavailable", "acp_spawn");
    assert_eq!(e["hint"], "install omp or set GLEN_OMP_BIN");
    assert!(!b.log.exists());
    assert_no_job(&b);
}

#[test]
fn ac_v2_302_a_bad_frame_after_initialize_is_malformed() {
    let b = RpcBox::new();
    let out = run(&b, &["@new", "hi"], &[("FAKE_OMP_ACP", "noise")]);
    let e = failure(&out, 7, "job_failed", "acp_malformed");
    assert!(e["details"]["stderr"].is_array());
    assert_groups_gone(&b);
}

#[test]
fn ac_v2_302_permission_requests_are_answered_by_policy_as_input_required() {
    for (flags, want) in [
        (&["--permission", "allow"][..], "allow_once"),
        (&["--permission", "deny"][..], "reject_once"),
        (&[][..], "reject_once"),
    ] {
        let b = RpcBox::new();
        let mut args = vec!["@new", "rm it"];
        args.extend_from_slice(flags);
        let d = envelope(&run(&b, &args, &[("FAKE_OMP_ACP", "permission")]), 0)["data"].clone();
        let events = d["events"].as_array().unwrap();
        assert_eq!(events.len(), 1, "{d}");
        let event = &events[0];
        assert_eq!(event["event"], "task_state");
        assert_eq!(event["task_state"], "input_required");
        assert_eq!(event["tool_call_id"], "t1");
        assert_eq!(event["kind"], "execute");
        assert_eq!(event["answer"], want);
        assert_eq!(event["option_id"], want);
        assert_eq!(
            event["policy"],
            if want == "allow_once" {
                "allow"
            } else {
                "deny"
            }
        );
        assert_eq!(d["assistant_text"], format!("permission:{want}"));
        assert_eq!(d["task_state"], "completed");
        let wire: Vec<_> = b
            .fake_log()
            .into_iter()
            .filter(|r| r["event"] == "permission")
            .collect();
        assert_eq!(
            wire[0]["response"]["result"],
            json!({"outcome":{"outcome":"selected","optionId":want}})
        );
        assert_eq!(wire[0]["response"]["id"], 100);
        assert_groups_gone(&b);
    }
    for (options, policy) in [("allow_only", "deny"), ("reject_only", "allow")] {
        let b = RpcBox::new();
        let out = run(
            &b,
            &["@new", "rm it", "--permission", policy],
            &[
                ("FAKE_OMP_ACP", "permission"),
                ("FAKE_OMP_ACP_OPTIONS", options),
            ],
        );
        let d = envelope(&out, 0)["data"].clone();
        assert_eq!(d["events"][0]["answer"], "cancelled", "{options}");
        assert_eq!(d["events"][0]["option_id"], Value::Null);
        assert_eq!(d["assistant_text"], "permission:cancelled");
        let wire: Vec<_> = b
            .fake_log()
            .into_iter()
            .filter(|r| r["event"] == "permission")
            .collect();
        assert_eq!(
            wire[0]["response"]["result"],
            json!({"outcome":{"outcome":"cancelled"}}),
            "never an *_always option"
        );
    }
}

#[test]
fn ac_v2_302_other_agent_requests_get_method_not_found() {
    let b = RpcBox::new();
    let out = run(&b, &["@new", "read"], &[("FAKE_OMP_ACP", "agent_request")]);
    let d = envelope(&out, 0)["data"].clone();
    assert_eq!(d["assistant_text"], "agent-request:-32601");
    assert_eq!(d["stop_reason"], "end_turn");
    let answer = b
        .fake_log()
        .into_iter()
        .find(|r| r["event"] == "in" && r["frame"]["id"] == 101)
        .unwrap();
    assert_eq!(answer["frame"]["error"]["code"], -32601);
}

#[test]
fn ac_v2_302_sigint_and_sigterm_send_session_cancel() {
    for sig in [Signal::INT, Signal::TERM] {
        let b = RpcBox::new();
        let child = spawn(&b, &["@new", "long job"], &[("FAKE_OMP_ACP", "cancel")]);
        wait_first_chunk(&b);
        let at = signal(&child, sig);
        let out = child.wait_with_output().unwrap();
        let v = envelope(&out, 130);
        assert_eq!(v["ok"], true, "{sig:?}");
        let d = &v["data"];
        assert_eq!(d["task_state"], "canceled");
        assert_eq!(
            d["cancel"],
            json!({"sent":true,"confirmed":true,"reason":"interrupt"})
        );
        assert_eq!(d["stop_reason"], "cancelled");
        let cancels = frames_in(&b, "session/cancel");
        assert_eq!(cancels.len(), 1, "{sig:?}");
        let logged = cancels[0]["at_ms"].as_u64().unwrap() as u128;
        assert!(
            logged.saturating_sub(at) < 500,
            "{sig:?}: {}ms",
            logged - at
        );
        assert_eq!(
            cancels[0]["frame"]["params"]["sessionId"],
            d["session"]["id"]
        );
        // Interrupted turns are not closed.
        assert!(frames_in(&b, "session/close").is_empty());
        assert_no_job(&b);
        assert_groups_gone(&b);
    }
}

#[test]
fn ac_v2_302_an_ignored_cancel_ends_after_the_grace() {
    let b = RpcBox::new();
    let child = spawn(
        &b,
        &["@new", "long job"],
        &[
            ("FAKE_OMP_ACP", "cancel_ignored"),
            ("GLEN_ACP_CANCEL_GRACE_MS", "300"),
        ],
    );
    wait_first_chunk(&b);
    signal(&child, Signal::INT);
    let started = Instant::now();
    let out = child.wait_with_output().unwrap();
    assert!(started.elapsed() < Duration::from_secs(4));
    let d = envelope(&out, 130)["data"].clone();
    assert_eq!(d["task_state"], "canceled");
    assert_eq!(
        d["cancel"],
        json!({"sent":true,"confirmed":false,"reason":"interrupt"})
    );
    assert_eq!(d["stop_reason"], Value::Null);
    assert_eq!(frames_in(&b, "session/cancel").len(), 1);
    assert_groups_gone(&b);
}

#[test]
fn ac_v2_302_sigint_before_the_prompt_sends_no_cancel() {
    let b = RpcBox::new();
    let child = spawn(&b, &["@new", "hi"], &[("FAKE_OMP_ACP", "init_hang")]);
    poll(Duration::from_secs(10), || {
        (!frames_in(&b, "initialize").is_empty()).then_some(())
    });
    signal(&child, Signal::INT);
    let started = Instant::now();
    let out = child.wait_with_output().unwrap();
    assert!(started.elapsed() < Duration::from_millis(3500));
    let d = envelope(&out, 130)["data"].clone();
    assert_eq!(d["task_state"], "canceled");
    assert_eq!(
        d["cancel"],
        json!({"sent":false,"confirmed":false,"reason":"interrupt"})
    );
    assert_eq!(d["session"], Value::Null);
    assert!(frames_in(&b, "session/cancel").is_empty());
    assert!(frames_in(&b, "session/prompt").is_empty());
    assert_groups_gone(&b);
}

#[test]
fn ac_v2_302_failures_after_the_handshake_are_job_failed() {
    let b = RpcBox::new();
    let e = failure(
        &run(&b, &["@new", "hi"], &[("FAKE_OMP_ACP", "crash")]),
        7,
        "job_failed",
        "acp_exited",
    );
    assert_eq!(e["details"]["task_state"], "failed");
    assert!(e["details"]["session"]["id"].is_string());
    assert_groups_gone(&b);

    let b = RpcBox::new();
    let e = failure(
        &run(
            &b,
            &["e27c0de", "hi"],
            &[("FAKE_OMP_ACP", "rpc_error_load")],
        ),
        7,
        "job_failed",
        "acp_rpc_error",
    );
    assert_eq!(e["details"]["method"], "session/load");
    assert_eq!(e["details"]["code"], -32603);
    assert_eq!(
        e["details"]["message"],
        format!("ACP session not found: {A_ID}")
    );
    assert_groups_gone(&b);

    let b = RpcBox::new();
    let e = failure(
        &run(&b, &["@new", "hi"], &[("FAKE_OMP_ACP", "rpc_error_prompt")]),
        7,
        "job_failed",
        "acp_rpc_error",
    );
    assert_eq!(e["details"]["method"], "session/prompt");
    assert!(
        e["details"]["message"]
            .as_str()
            .unwrap()
            .starts_with("ACP session not found: ")
    );
    assert_groups_gone(&b);

    let b = RpcBox::new();
    let started = Instant::now();
    let e = failure(
        &run(
            &b,
            &["@new", "hi"],
            &[
                ("FAKE_OMP_ACP", "hang"),
                ("GLEN_JOB_TIMEOUT", "2s"),
                ("GLEN_ACP_CANCEL_GRACE_MS", "300"),
            ],
        ),
        7,
        "job_failed",
        "acp_timeout",
    );
    assert!(started.elapsed() < Duration::from_secs(8));
    assert_eq!(
        e["details"]["cancel"],
        json!({"sent":true,"confirmed":false,"reason":"deadline"})
    );
    assert_eq!(frames_in(&b, "session/cancel").len(), 1);
    assert_groups_gone(&b);

    for (stop, state) in [("refusal", "rejected"), ("max_tokens", "failed")] {
        let b = RpcBox::new();
        let e = failure(
            &run(
                &b,
                &["@new", "hi"],
                &[("FAKE_OMP_ACP", "stop"), ("FAKE_OMP_ACP_STOP", stop)],
            ),
            7,
            "job_failed",
            "acp_stop_reason",
        );
        assert_eq!(e["details"]["stop_reason"], stop);
        assert_eq!(e["details"]["task_state"], state);
        assert_eq!(e["details"]["assistant_text"], format!("stopped: {stop}"));
        assert_groups_gone(&b);
    }
}

#[test]
fn ac_v2_303_the_lock_is_held_until_the_omp_group_is_gone() {
    let b = RpcBox::new();
    let child = spawn(
        &b,
        &["e27c0de", "long job"],
        &[
            ("FAKE_OMP_ACP", "cancel_ignored"),
            ("GLEN_ACP_CANCEL_GRACE_MS", "300"),
        ],
    );
    wait_first_chunk(&b);
    let pgid = b
        .fake_log()
        .into_iter()
        .find(|r| r["event"] == "start")
        .unwrap()["pgid"]
        .as_i64()
        .unwrap();
    assert!(lock::try_acquire(&home(&b), A_ID).unwrap().is_none());
    signal(&child, Signal::INT);
    // Observe the lock continuously: the moment it is free, omp's group must be gone.
    let held = poll(Duration::from_secs(5), || {
        let lock = lock::try_acquire(&home(&b), A_ID).unwrap()?;
        assert!(
            group_gone(pgid),
            "the session lock became free while the omp group survived"
        );
        Some(lock)
    });
    drop(held);
    envelope(&child.wait_with_output().unwrap(), 130);
}

/// AC-v2-303: every refusal exits 6 before omp starts.
fn assert_refused(b: &RpcBox, reason: &str) -> Value {
    let e = failure(&run(b, &["e27c0de", "hi"], &[]), 6, "no_route", reason);
    assert!(!b.log.exists(), "omp started for {reason}");
    assert_no_job(b);
    // The lock is taken before the probes; its file stays.
    assert!(lock::lock_path(&home(b), A_ID).is_file());
    e
}

#[test]
fn ac_v2_303_a_held_lease_refuses_as_live_owner() {
    let b = RpcBox::new();
    let mut holder = b.hold_lease(A_ID);
    assert_refused(&b, "live_owner");
    drop(holder.stdin.take());
    holder.wait().unwrap();
}

#[test]
fn ac_v2_303_live_and_unhealthy_endpoints_refuse() {
    let mut b = LiveBox::new();
    b.start_ext(A_ID, "ok", &[]);
    let e = assert_refused(&b.rpc, "live_owner");
    assert!(
        e["hint"]
            .as_str()
            .unwrap()
            .contains("use glen send e27c0de"),
        "{e}"
    );
    let mut b = LiveBox::new();
    b.start_ext(A_ID, "hang", &[]);
    assert_refused(&b.rpc, "unhealthy_endpoint");
}

#[test]
fn ac_v2_303_a_held_glen_lock_refuses_as_rpc_job_running() {
    let b = RpcBox::new();
    let lock = b.hold_glen_lock(A_ID);
    let e = assert_refused(&b, "rpc_job_running");
    assert!(
        e["hint"]
            .as_str()
            .unwrap()
            .contains("glen jobs --active --session e27c0de")
    );
    drop(lock);
}

#[test]
fn ac_v2_301_acp_is_hidden_but_has_a_help_page() {
    let b = RpcBox::new();
    let alpha = b.omp.alpha();
    let text = |args: &[&str]| {
        let out = b.omp.glen(args, &alpha);
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    for args in [&["--help"][..], &["-h"][..], &["help"][..]] {
        assert!(!text(args).contains("__acp"), "{args:?}");
    }
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        assert!(!text(&["completions", shell]).contains("__acp"), "{shell}");
    }
    for args in [&["__acp", "--help"][..], &["help", "__acp"][..]] {
        let page = text(args);
        assert!(page.contains("Usage: glen __acp"), "{page}");
        assert!(page.contains("--permission"), "{page}");
    }
    assert!(!text(&["install-skill", "--project", "--dry-run", "--json"]).contains("__acp"));
    // The rendered skills themselves, installed into the box's project directory.
    text(&["install-skill", "--project"]);
    for skill in [
        ".claude/skills/glen/SKILL.md",
        ".agents/skills/glen/SKILL.md",
    ] {
        let body = fs::read_to_string(alpha.join(skill)).unwrap();
        assert!(body.contains("glen send"), "{skill}");
        assert!(!body.contains("__acp"), "{skill}");
    }
}
