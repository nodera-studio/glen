//! W3 live route through `glen send`, `glen wait`, `glen jobs` and `glen ls`, driven through the
//! `fake_ext` peer and in-test endpoints (AC-307 as seen by glen, AC-314…AC-318, AC-322…AC-326;
//! docs/design.md §A3, §A4, §A7, §A10, §A11).

use std::fs;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use rustix::process::Signal;
use serde_json::{Value, json};

use crate::live_support::*;
use crate::reader::*;
use crate::rpc_support::*;
use crate::support::*;

const ABORT_BOUND: Duration = Duration::from_millis(1500);

/// The hint glen gives when an endpoint is present but unhealthy (AC-314).
fn assert_install_hint(err: &Value) {
    let hint = plain(&err["hint"]);
    assert!(
        hint.contains("glen install-extension") && hint.contains("restart"),
        "AC-314 hint names `glen install-extension` and a restart: {err}"
    );
}

/// Exit 6 `no_route` before any job exists and without starting omp (AC-314).
fn refused_before_a_job(lb: &LiveBox, out: &Out) -> Value {
    let err = failed(out, "no_route");
    assert!(
        lb.job_ids().is_empty(),
        "a job was created before the refusal\n{}",
        lb.diag()
    );
    lb.assert_no_omp();
    err
}

/// The job id inside a `job_failed` error.
fn live_job_id_from_error(err: &Value) -> String {
    err["details"]["job"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no details.job.id: {err}"))
        .to_owned()
}

// ---------------------------------------------------------------------------------------------
// AC-314 — the live route wins, and stale registries fall through.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac314_ac316_a_live_endpoint_wins_over_a_held_lease_and_needs_no_omp() {
    let lb = LiveBox::new();
    let ext = lb.start_ext(A.id, "ok", &[], &[]);
    let _lease = hold_lease(&lb.bx.owners(), A.id);
    let before = snapshot(&lb.bx.sessions);
    let out = lb
        .send(A.handle, "hi", &["--wait", "20s"])
        .env("GLEN_OMP_BIN", "/nonexistent/omp")
        .run();
    let d = data_of(&out, 0);
    for k in COMPLETED_KEYS {
        assert!(d.get(k).is_some(), "completed data lacks `{k}`: {d}");
    }
    let id = job_id(&d);
    assert_eq!(d["status"], "completed", "{d}");
    assert_job_view(&d["job"]);
    assert_eq!(d["job"]["route"], "live", "{d}");
    assert_eq!(d["job"]["session_id"], A.id, "{d}");
    assert_eq!(d["session"]["id"], A.id, "{d}");
    assert_eq!(d["session"]["handle"], A.handle, "{d}");
    assert_eq!(d["assistant_text"], "pong: hi", "{d}");
    assert_eq!(d["delivered_as"], "prompt", "{d}");
    assert_eq!(d["session_settled"], true, "AC-316 (AC-203 shape): {d}");
    assert_eq!(d["command_output"], json!([]), "AC-316: {d}");
    assert!(d["forked"].is_null(), "{d}");

    // The wire: hello (AC-308 args), then exactly one send carrying the job (AC-316).
    let ins = ext.frames("in");
    let ops: Vec<&str> = ins
        .iter()
        .map(|f| f["op"].as_str().unwrap_or("?"))
        .collect();
    let hello = ops.iter().position(|o| *o == "hello").expect("a hello");
    let send_at = ops.iter().position(|o| *o == "send").expect("a send");
    assert!(hello < send_at, "hello precedes send: {ops:?}");
    assert_eq!(ops.iter().filter(|o| **o == "send").count(), 1, "{ops:?}");
    let h = &ins[hello];
    assert_eq!(h["v"], 1, "{h}");
    assert!(h["id"].is_string(), "{h}");
    assert_eq!(h["args"]["protocols"], json!([1]), "AC-308 hello args: {h}");
    assert!(h["args"]["client"].is_string(), "{h}");
    assert!(h["args"]["client_version"].is_string(), "{h}");
    let send = &ins[send_at];
    assert_eq!(send["v"], 1, "{send}");
    let rid = send["id"].as_str().expect("send id is a string").to_owned();
    assert_eq!(send["args"]["job_id"], id.as_str(), "{send}");
    assert_eq!(send["args"]["text"], "hi", "{send}");
    assert_eq!(send["args"]["deliver_as"], "auto", "{send}");
    assert_eq!(send["args"]["expect_session_id"], A.id, "{send}");

    // job.json freezes the endpoint and has no omp binary; status.json carries the request id.
    let job = lb.job_json(&id);
    assert_eq!(job["route"], "live", "{job}");
    assert!(
        job.get("omp_bin").is_none(),
        "§A7: no omp_bin on the live route: {job}"
    );
    assert_eq!(job["live_pid"], ext.pid, "{job}");
    assert_path(&job["live_socket"], &ext.socket);
    let st = lb.status(&id);
    assert_eq!(st["status"], "completed", "{st}");
    assert_eq!(st["request_id"], rid.as_str(), "{st}");
    assert_eq!(st["delivered_as"], "prompt", "{st}");

    // Replies are stored as streamed, in the result and the job's replies file; the inbox keeps
    // only the extension's own line (AC-316: glen does not write the inbox on this route).
    let streamed: Vec<Value> = ext
        .frames("out")
        .into_iter()
        .filter(|f| f["event"] == "reply")
        .map(|f| f["reply"].clone())
        .collect();
    assert_eq!(streamed.len(), 1);
    assert_eq!(d["replies"], json!(streamed), "{d}");
    let res = lb.result(&id);
    assert_eq!(res["replies"], d["replies"], "{res}");
    assert_eq!(res["assistant_text"], "pong: hi", "{res}");
    assert_eq!(res["command_output"], json!([]), "{res}");
    assert_eq!(res["session_settled"], true, "{res}");
    assert_eq!(
        ndjson(&lb.job_dir(&id).join("replies.ndjson")),
        streamed,
        "jobs/<id>/replies.ndjson"
    );
    assert_eq!(
        ndjson(&lb.inbox_file(A.id)).len(),
        1,
        "the inbox holds the extension's line only"
    );

    lb.assert_no_omp();
    lb.assert_quiet();
    assert_eq!(
        snapshot(&lb.bx.sessions),
        before,
        "AC-322: the omp sessions dir is untouched"
    );
}

#[test]
fn ac314_a_dead_pid_registry_and_its_socket_are_removed_and_send_uses_rpc() {
    let lb = LiveBox::new();
    let dead = dead_pid();
    mkdir_private(&lb.run_live());
    let sock = lb.run_live().join(format!("{dead}.sock"));
    stale_socket(&sock);
    let reg = write_registry_doc(&lb, A.id, &registry_doc(A.id, dead, &sock));
    let d = data_of(&lb.send(A.handle, "hi", &["--wait", "20s"]).run(), 0);
    assert_eq!(d["job"]["route"], "rpc", "{d}");
    assert_eq!(d["assistant_text"], "pong: hi", "{d}");
    assert!(!reg.exists(), "the stale registry was deleted");
    assert!(!sock.exists(), "a dead pid's socket file is deleted too");
    lb.assert_quiet();
}

#[test]
fn ac314_a_refused_socket_is_stale_but_a_live_pids_socket_file_stays() {
    let lb = LiveBox::new();
    mkdir_private(&lb.run_live());
    let sock = lb.run_live().join("leftover.sock");
    stale_socket(&sock);
    let reg = write_registry_doc(&lb, A.id, &registry_doc(A.id, std::process::id(), &sock));
    let d = data_of(&lb.send(A.handle, "hi", &["--wait", "20s"]).run(), 0);
    assert_eq!(d["job"]["route"], "rpc", "{d}");
    assert!(!reg.exists(), "the refused registry was deleted");
    assert!(
        sock.exists(),
        "the pid is alive, so its socket file is kept"
    );
    lb.assert_quiet();
}

#[test]
fn ac314_a_missing_socket_is_stale_and_send_uses_rpc() {
    let lb = LiveBox::new();
    let sock = lb.run_live().join("missing.sock");
    let reg = write_registry_doc(&lb, A.id, &registry_doc(A.id, std::process::id(), &sock));
    let d = data_of(&lb.send(A.handle, "hi", &["--wait", "20s"]).run(), 0);
    assert_eq!(d["job"]["route"], "rpc", "{d}");
    assert!(
        !reg.exists(),
        "the registry naming a missing socket was deleted"
    );
    let left: Vec<String> = list_tree(&lb.run_live());
    assert!(
        left.is_empty(),
        "nothing else was created or kept: {left:?}"
    );
    lb.assert_quiet();
}

#[test]
fn ac314_an_endpoint_now_on_another_session_is_stale_and_send_uses_rpc() {
    let lb = LiveBox::new();
    let mut ext = lb.start_ext(A.id, "mismatch", &[], &[]);
    let d = data_of(&lb.send(A.handle, "hi", &["--wait", "20s"]).run(), 0);
    assert_eq!(d["job"]["route"], "rpc", "{d}");
    assert!(
        !ext.registry.exists(),
        "the mismatched registry was deleted"
    );
    assert!(ext.alive(), "glen never touches the endpoint's process");
    assert!(
        !ext.ops_in().iter().any(|o| o == "send"),
        "nothing was delivered to the other session: {:?}",
        ext.ops_in()
    );
    lb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-314 — unhealthy endpoints fail closed before a job exists.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac314_an_endpoint_that_never_answers_hello_refuses_within_two_seconds() {
    let lb = LiveBox::new();
    let _ext = lb.start_ext(A.id, "hang", &[], &[]);
    let out = lb.send(A.handle, "hi", &["--wait", "20s"]).run();
    let err = refused_before_a_job(&lb, &out);
    assert_install_hint(&err);
    assert!(
        out.elapsed < Duration::from_secs(3),
        "AC-314: hello is bounded at 2 s, took {:?}",
        out.elapsed
    );
}

#[test]
fn ac314_an_incompatible_endpoint_refuses_with_the_install_hint() {
    for scenario in ["v2", "no_send"] {
        let lb = LiveBox::new();
        let ext = lb.start_ext(A.id, scenario, &[], &[]);
        let out = lb.send(A.handle, "hi", &["--wait", "20s"]).run();
        let err = refused_before_a_job(&lb, &out);
        assert_install_hint(&err);
        assert!(
            !ext.ops_in().iter().any(|o| o == "send"),
            "{scenario}: nothing was sent"
        );
    }
}

#[test]
fn ac314_an_unreadable_registry_refuses_with_the_install_hint() {
    let lb = LiveBox::new();
    mkdir_private(&lb.run_live());
    write_private(&lb.registry_path(A.id), b"{not json");
    let out = lb.send(A.handle, "hi", &["--wait", "20s"]).run();
    let err = refused_before_a_job(&lb, &out);
    assert_install_hint(&err);
}

#[test]
fn ac314_ac321_a_registry_socket_path_at_the_sun_path_limit_refuses_and_names_glen_home() {
    let lb = LiveBox::new();
    let mut long = String::from("/tmp/");
    while long.len() + 5 < SUN_LIMIT {
        long.push('x');
    }
    long.push_str(".sock");
    assert_eq!(long.len(), SUN_LIMIT);
    write_registry_doc(
        &lb,
        A.id,
        &registry_doc(A.id, std::process::id(), Path::new(&long)),
    );
    let out = lb.send(A.handle, "hi", &["--wait", "20s"]).run();
    let err = refused_before_a_job(&lb, &out);
    assert!(
        plain(&err["hint"]).contains("GLEN_HOME"),
        "AC-314: the path case suggests shortening GLEN_HOME: {err}"
    );
}

#[test]
fn ac314_a_malformed_or_non_v1_hello_refuses() {
    // Garbage instead of a response frame.
    let lb = LiveBox::new();
    let _p = Peer::start(&lb, A.id, |_, _, s| {
        let _ = s.write_all(b"this is not json\n");
    });
    let out = lb.send(A.handle, "hi", &["--wait", "20s"]).run();
    assert_install_hint(&refused_before_a_job(&lb, &out));

    // A well-formed hello for the right session that does not speak protocol 1.
    let lb = LiveBox::new();
    let _p = Peer::start(&lb, A.id, |_, req, s| {
        let mut d = hello_data(A.id);
        d["protocol"] = json!(2);
        d["protocols"] = json!([2]);
        write_frame(s, &json!({"v":1,"id":req["id"],"ok":true,"data":d}));
    });
    let out = lb.send(A.handle, "hi", &["--wait", "20s"]).run();
    assert_install_hint(&refused_before_a_job(&lb, &out));
}

// ---------------------------------------------------------------------------------------------
// AC-315 — refusals instead of forks.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac315_an_endpoint_plus_a_held_glen_lock_refuses_with_the_jobs_hint() {
    let lb = LiveBox::new();
    let _lock = lb.hold_glen_lock(A.id);
    let ext = lb.start_ext(A.id, "ok", &[], &[]);
    let out = lb.send(A.handle, "hi", &["--wait", "20s"]).run();
    let err = failed(&out, "no_route");
    let hint = plain(&err["hint"]);
    assert!(
        hint.contains("glen jobs --active --session") && hint.contains(A.handle),
        "AC-315 hint names `glen jobs --active --session <handle>`: {err}"
    );
    assert!(
        !ext.ops_in().iter().any(|o| o == "send"),
        "nothing was delivered"
    );
    lb.assert_no_omp();
}

#[test]
fn ac315_a_held_lease_without_an_endpoint_names_install_or_restart() {
    let lb = LiveBox::new();
    let _lease = hold_lease(&lb.bx.owners(), A.id);
    let out = lb.send(A.handle, "hi", &["--wait", "20s"]).run();
    let err = failed(&out, "no_route");
    assert!(
        plain(&err["hint"]).contains("run glen install-extension, then restart that omp session"),
        "AC-315 hint without the extension file: {err}"
    );

    let ext = lb.bx.agent.join("extensions/glen/index.ts");
    fs::create_dir_all(ext.parent().unwrap()).unwrap();
    fs::write(&ext, "// placeholder\n").unwrap();
    let out = lb.send(A.handle, "hi", &["--wait", "20s"]).run();
    let err = failed(&out, "no_route");
    assert!(
        plain(&err["hint"]).contains(
            "restart that omp session so it loads the glen extension (only TUI sessions accept glen messages)"
        ),
        "AC-315 hint with the extension file installed: {err}"
    );
    assert!(lb.job_ids().is_empty(), "{}", lb.diag());
    lb.assert_no_omp();
}

#[test]
fn ac315_the_rpc_worker_refuses_an_endpoint_that_appeared_before_spawning() {
    let lb = LiveBox::new();
    let lock = lb.hold_glen_lock(A.id);
    let d = data_of(&lb.send(A.handle, "hi", &["--no-wait"]).run(), 10);
    assert_eq!(d["job"]["status"], "queued", "{d}");
    assert_eq!(d["job"]["route"], "rpc", "{d}");
    let id = job_id(&d);
    let ext = lb.start_ext(A.id, "ok", &[], &[]);
    drop(lock);
    let out = lb
        .glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
        .run();
    failed(&out, "no_route");
    let st = lb.wait_terminal(&id);
    assert_eq!(st["status"], "failed", "{st}");
    assert_eq!(st["error"]["code"], "no_route", "{st}");
    lb.assert_quiet();
    lb.assert_no_omp();
    assert!(
        !ext.ops_in().iter().any(|o| o == "send"),
        "no RPC-to-live handoff: {:?}",
        ext.ops_in()
    );
}

#[test]
fn ac315_the_rpc_worker_rechecks_the_endpoint_right_before_prompting() {
    let lb = LiveBox::new();
    let release = lb.bx.root.join("ready-release");
    let d = data_of(
        &lb.send(A.handle, "hi", &["--no-wait"])
            .env("FAKE_OMP_READY_RELEASE", &release)
            .run(),
        10,
    );
    let id = job_id(&d);
    lb.wait_log("omp to start", |r| !starts(r).is_empty());
    let ext = lb.start_ext(A.id, "ok", &[], &[]);
    fs::write(&release, "go").unwrap();
    let out = lb
        .glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
        .run();
    failed(&out, "no_route");
    let st = lb.wait_terminal(&id);
    assert_eq!(st["error"]["code"], "no_route", "{st}");
    lb.assert_quiet();
    assert!(
        lb.inbound("prompt").is_empty(),
        "AC-315: no prompt frame once an endpoint answers\n{}",
        lb.diag()
    );
    assert!(!ext.ops_in().iter().any(|o| o == "send"));
}

// ---------------------------------------------------------------------------------------------
// AC-316 — live job outcomes.
// ---------------------------------------------------------------------------------------------

/// Start `fake_ext` with `scenario` on A and run `send A hi --wait 20s` plus `extra`.
fn live_run(scenario: &str, env: &[(&str, &str)], extra: &[&str]) -> (LiveBox, Ext, Out) {
    let lb = LiveBox::new();
    let ext = lb.start_ext(A.id, scenario, env, &[]);
    let mut args = vec!["--wait", "20s"];
    args.extend_from_slice(extra);
    let out = lb.send(A.handle, "hi", &args).run();
    (lb, ext, out)
}

/// A failed or aborted live job: exit 7 `job_failed`, `result.json` written (the send frame was
/// written before the failure), every process gone. Returns the job id and the error.
fn live_failure(scenario: &str, status: &str, code: &str) -> (LiveBox, Ext, String, Value) {
    let (lb, ext, out) = live_run(scenario, &[], &[]);
    let err = failed(&out, "job_failed");
    assert_job_failed(&err, status, code);
    assert_eq!(err["details"]["job"]["route"], "live", "{err}");
    let id = live_job_id_from_error(&err);
    let st = lb.wait_terminal(&id);
    assert_eq!(st["status"], status, "{st}");
    assert_eq!(st["error"]["code"], code, "{st}");
    assert!(
        lb.job_dir(&id).join("result.json").is_file(),
        "AC-316: a failure after the send frame writes result.json ({scenario})"
    );
    lb.assert_quiet();
    lb.assert_no_omp();
    (lb, ext, id, err)
}

#[test]
fn ac316_progress_replies_arrive_in_order_everywhere_and_the_inbox_is_not_duplicated() {
    let (lb, _ext, out) = live_run("progress", &[], &[]);
    let d = data_of(&out, 0);
    let id = job_id(&d);
    let texts: Vec<&str> = arr(&d["replies"])
        .iter()
        .map(|r| r["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["working", "reply: hi"], "{d}");
    assert_eq!(d["replies"][0]["final"], false, "{d}");
    assert_eq!(d["replies"][1]["final"], true, "{d}");
    for r in arr(&d["replies"]) {
        assert_eq!(r["job_id"], id.as_str(), "{r}");
        assert_eq!(r["session_id"], A.id, "{r}");
    }
    assert_eq!(lb.result(&id)["replies"], d["replies"]);
    assert_eq!(
        json!(ndjson(&lb.job_dir(&id).join("replies.ndjson"))),
        d["replies"]
    );
    assert_eq!(
        ndjson(&lb.inbox_file(A.id)).len(),
        2,
        "two inbox lines, written by the extension only"
    );
    lb.assert_quiet();
}

#[test]
fn ac316_ac309_delivery_modes_reach_the_endpoint_and_the_ack_sets_delivered_as() {
    let lb = LiveBox::new();
    let ext = lb.start_ext(A.id, "busy", &[], &[]);
    let cases: [(&[&str], &str, &str); 4] = [
        (&[], "auto", "follow_up"),
        (&["--steer"], "steer", "steer"),
        (&["--follow-up"], "follow_up", "follow_up"),
        (&["--as", "steer"], "steer", "steer"),
    ];
    for (i, (flags, deliver, delivered)) in cases.iter().enumerate() {
        let mut args = vec!["--wait", "20s"];
        args.extend_from_slice(flags);
        let d = data_of(&lb.send(A.handle, "hi", &args).run(), 0);
        assert_eq!(d["delivered_as"], *delivered, "{flags:?}: {d}");
        assert_eq!(d["job"]["delivered_as"], *delivered, "{flags:?}: {d}");
        assert_eq!(d["job"]["deliver_as"], *deliver, "{flags:?}: {d}");
        let sends: Vec<Value> = ext
            .frames("in")
            .into_iter()
            .filter(|f| f["op"] == "send")
            .collect();
        assert_eq!(sends.len(), i + 1);
        assert_eq!(sends[i]["args"]["deliver_as"], *deliver, "{flags:?}");
    }
    lb.assert_quiet();
}

#[test]
fn ac316_a_settled_aborted_run_is_aborted() {
    live_failure("aborted", "aborted", "aborted");
}

#[test]
fn ac316_a_settled_error_run_is_failed_omp_error() {
    live_failure("error", "failed", "omp_error");
}

#[test]
fn ac316_a_session_change_before_settlement_is_a_fork_exit_11() {
    let (lb, _ext, out) = live_run("moved", &[], &[]);
    let d = data_of(&out, 11);
    let to = moved_id(A.id);
    assert_eq!(d["status"], "completed", "{d}");
    assert_eq!(d["forked"]["from"], A.id, "{d}");
    assert_eq!(d["forked"]["to"], to.as_str(), "{d}");
    assert_eq!(d["forked"]["to_handle"], tail7(&to), "{d}");
    let id = job_id(&d);
    let st = lb.wait_terminal(&id);
    assert_eq!(st["forked"]["from"], A.id, "{st}");
    assert_eq!(st["forked"]["to"], to.as_str(), "{st}");
    let w = data_of(&lb.glen(["wait", &job_handle(&id), "--json"]).run(), 11);
    assert_eq!(w["forked"]["to"], to.as_str(), "{w}");
    lb.assert_quiet();
}

#[test]
fn ac316_a_session_switch_is_aborted_session_switched() {
    live_failure("switched", "aborted", "session_switched");
}

#[test]
fn ac316_an_endpoint_shutdown_is_failed_omp_exited() {
    live_failure("shutdown", "failed", "omp_exited");
}

#[test]
fn ac316_eof_before_a_terminal_event_keeps_the_replies_so_far() {
    let (lb, _ext, id, _) = live_failure("crash", "failed", "omp_exited");
    let res = lb.result(&id);
    let replies = arr(&res["replies"]);
    assert_eq!(replies.len(), 1, "AC-316: replies so far: {res}");
    assert_eq!(replies[0]["text"], "reply: hi", "{res}");
}

#[test]
fn ac316_a_malformed_frame_is_failed_protocol() {
    live_failure("malformed", "failed", "protocol");
}

#[test]
fn ac316_an_ack_session_mismatch_is_failed_session_moved_with_from_and_to() {
    let (lb, _ext, id, err) = live_failure("rekey", "failed", "session_moved");
    let to = moved_id(A.id);
    let details = &err["details"]["error"]["details"];
    assert_eq!(details["from"], A.id, "{err}");
    assert_eq!(details["to"], to.as_str(), "{err}");
    let st = lb.status(&id);
    assert_eq!(st["error"]["details"]["from"], A.id, "{st}");
    assert_eq!(st["error"]["details"]["to"], to.as_str(), "{st}");
    assert_eq!(
        lb.result(&id)["replies"],
        json!([]),
        "no reply arrived before the refusal"
    );
}

/// An endpoint that answers hello/state correctly and the `send` request with `on_send`.
fn peer_with_send(
    lb: &LiveBox,
    on_send: impl Fn(&Value, &mut UnixStream) + Send + Sync + 'static,
) -> Peer {
    Peer::start(lb, A.id, move |_, req, s| {
        if !answer_unary(A.id, req, s) {
            on_send(req, s);
        }
    })
}

#[test]
fn ac307_ac316_error_responses_to_send_fail_the_job_with_protocol() {
    for (code, details) in [
        (
            "unsupported_op",
            json!({"op": "send", "ops": ["hello", "state"], "reserved": false}),
        ),
        ("unsupported_version", json!({"supported": [1]})),
        ("bad_request", Value::Null),
    ] {
        let lb = LiveBox::new();
        let _p = peer_with_send(&lb, move |req, s| {
            let mut e = json!({"code": code, "message": "refused by the test peer"});
            if !details.is_null() {
                e["details"] = details.clone();
            }
            write_frame(s, &json!({"v":1,"id":req["id"],"ok":false,"error":e}));
        });
        let out = lb.send(A.handle, "hi", &["--wait", "20s"]).run();
        let err = failed(&out, "job_failed");
        assert_job_failed(&err, "failed", "protocol");
        lb.assert_quiet();
    }
}

#[test]
fn ac307_ac325_unknown_events_and_fields_are_ignored_and_reply_ids_may_be_missing() {
    let lb = LiveBox::new();
    let p = peer_with_send(&lb, |req, s| {
        let id = req["id"].clone();
        write_frame(
            s,
            &json!({"v":1,"id":id,"ok":true,"data":{"delivered_as":"prompt","session_id":A.id,"request_id":id,"future_field":{"x":1}}}),
        );
        write_frame(s, &json!({"v":1,"id":id,"event":"consumed","extra":true}));
        write_frame(s, &json!({"v":1,"id":id,"event":"progress_tick","pct":50}));
        // A reply whose optional ids are missing altogether (AC-325: readers accept it).
        write_frame(
            s,
            &json!({"v":1,"id":id,"event":"reply","reply":{"v":1,"id":"r-1","session_id":A.id,"text":"lean reply","final":true,"at":jiff::Timestamp::now().to_string()}}),
        );
        write_frame(
            s,
            &json!({"v":1,"id":id,"event":"settled","status":"completed","assistant_text":"done","stop_reason":"stop","error_message":null,"later":"ignored"}),
        );
    });
    let d = data_of(&lb.send(A.handle, "hi", &["--wait", "20s"]).run(), 0);
    assert_eq!(d["job"]["route"], "live", "{d}");
    assert_eq!(d["assistant_text"], "done", "{d}");
    let replies = arr(&d["replies"]);
    assert_eq!(replies.len(), 1, "{d}");
    assert_eq!(replies[0]["text"], "lean reply", "{d}");
    for k in ["job_id", "request_id", "tool_call_id"] {
        assert!(
            replies[0].get(k).is_some(),
            "§A5: `{k}` is always present (null when absent): {d}"
        );
    }
    assert_eq!(p.ops_in().last().map(String::as_str), Some("send"));
    lb.assert_quiet();
}

/// The endpoint answers hello, then vanishes (`gone`) or leaves a dead socket file behind before
/// the worker connects.
fn vanishing_peer(lb: &LiveBox, leave_dead_socket: bool) -> Peer {
    let sock = lb.run_live().join(format!("{}.sock", std::process::id()));
    Peer::start(lb, A.id, move |idx, req, s| {
        if idx == 0 && answer_unary(A.id, req, s) {
            let _ = fs::remove_file(&sock);
            if leave_dead_socket {
                stale_socket(&sock);
            }
        }
    })
}

#[test]
fn ac316_an_endpoint_gone_before_the_worker_connects_is_failed_no_route() {
    for dead_socket in [false, true] {
        let lb = LiveBox::new();
        let p = vanishing_peer(&lb, dead_socket);
        let out = lb.send(A.handle, "hi", &["--wait", "20s"]).run();
        let err = failed(&out, "no_route");
        let ids = lb.job_ids();
        assert_eq!(
            ids.len(),
            1,
            "AC-316: the worker's connect failure fails the live job (dead socket file: {dead_socket})\n{err}"
        );
        let st = lb.wait_terminal(&ids[0]);
        assert_eq!(st["status"], "failed", "{st}");
        assert_eq!(st["error"]["code"], "no_route", "{st}");
        assert_eq!(lb.job_json(&ids[0])["route"], "live");
        assert_eq!(p.ops_in(), ["hello"], "nothing reached a send");
        lb.assert_quiet();
        lb.assert_no_omp();
    }
}

#[test]
fn ac316_no_ack_within_ten_seconds_is_failed_protocol() {
    let (lb, _ext, out) = live_run("no_ack", &[], &[]);
    let err = failed(&out, "job_failed");
    assert_job_failed(&err, "failed", "protocol");
    assert!(
        out.elapsed >= Duration::from_secs(9) && out.elapsed < Duration::from_secs(15),
        "AC-316: the ack bound is 10 s, took {:?}",
        out.elapsed
    );
    lb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-317 — waits, deadlines, cancellation and controls on live jobs.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac317_bounded_waits_exit_10_and_glen_wait_collects_the_result() {
    let lb = LiveBox::new();
    let _ext = lb.start_ext(A.id, "slow", &[("FAKE_EXT_DELAY_MS", "1500")], &[]);
    let d = data_of(&lb.send(A.handle, "hi", &["--wait", "300ms"]).run(), 10);
    assert_eq!(d["status"], "running", "{d}");
    let h = job_handle(&job_id(&d));
    assert_eq!(d["next"], format!("glen wait {h}"), "{d}");
    let w = data_of(
        &lb.glen(["wait", &h, "--timeout", "20s", "--json"]).run(),
        0,
    );
    assert_eq!(w["assistant_text"], "pong: hi", "{w}");
    assert_eq!(w["job"]["route"], "live", "{w}");

    let out = lb.send(A.handle, "again", &["--no-wait"]).run();
    let d = data_of(&out, 10);
    assert!(
        out.elapsed < Duration::from_secs(2),
        "--no-wait took {:?}",
        out.elapsed
    );
    let w = data_of(
        &lb.glen([
            "wait",
            &job_handle(&job_id(&d)),
            "--timeout",
            "20s",
            "--json",
        ])
        .run(),
        0,
    );
    assert_eq!(w["assistant_text"], "pong: again", "{w}");

    // `glen jobs` lists both live jobs with the route.
    let j = data_of(&lb.glen(["jobs", "--json"]).run(), 0);
    let routes: Vec<&str> = arr(&j["jobs"])
        .iter()
        .map(|x| x["route"].as_str().unwrap())
        .collect();
    assert_eq!(routes, ["live", "live"], "{j}");
    lb.assert_quiet();
}

/// `send --wait 20s` with a 2 s job deadline against `scenario`; returns the terminal status.
fn deadline_case(scenario: &str) -> (LiveBox, Ext, Value) {
    let lb = LiveBox::new();
    let ext = lb.start_ext(A.id, scenario, &[], &[]);
    let out = lb
        .send(A.handle, "hi", &["--wait", "20s"])
        .env("GLEN_JOB_TIMEOUT", "2s")
        .run();
    let err = failed(&out, "job_failed");
    assert_job_failed(&err, "failed", "timeout");
    let id = live_job_id_from_error(&err);
    let st = lb.wait_terminal(&id);
    lb.assert_quiet();
    (lb, ext, st)
}

#[test]
fn ac317_a_deadline_after_a_consumed_ack_says_the_session_keeps_running() {
    let (_lb, mut ext, st) = deadline_case("never_settles");
    assert!(
        plain(&st["error"]["hint"]).contains(
            "the message was delivered and the omp session keeps running; replies still reach glen inbox"
        ),
        "AC-317 hint after consumption: {st}"
    );
    assert!(ext.alive(), "the omp turn is not interrupted");
    assert!(
        ext.ops_in().iter().all(|o| o == "hello" || o == "send"),
        "no abort was attempted: {:?}",
        ext.ops_in()
    );
}

#[test]
fn ac317_a_deadline_after_an_unconsumed_ack_says_the_message_is_still_queued() {
    let (_lb, _ext, st) = deadline_case("queued");
    assert!(
        plain(&st["error"]["hint"]).contains(
            "the message is still queued in the omp session; replies still reach glen inbox"
        ),
        "AC-317: an acknowledgement is not consumption: {st}"
    );
}

#[test]
fn ac317_a_deadline_before_the_ack_is_failed_timeout() {
    let lb = LiveBox::new();
    let _ext = lb.start_ext(A.id, "no_ack", &[], &[]);
    let out = lb
        .send(A.handle, "hi", &["--wait", "20s"])
        .env("GLEN_JOB_TIMEOUT", "2s")
        .run();
    let err = failed(&out, "job_failed");
    assert_job_failed(&err, "failed", "timeout");
    assert!(
        out.elapsed < Duration::from_secs(8),
        "the 2 s deadline, not the 10 s ack bound, decides: {:?}",
        out.elapsed
    );
    lb.assert_quiet();
}

fn live_signal_cancels(sig: Signal) {
    let lb = LiveBox::new();
    let mut ext = lb.start_ext(A.id, "never_settles", &[], &[]);
    let d = data_of(&lb.send(A.handle, "hi", &["--no-wait"]).run(), 10);
    let id = job_id(&d);
    let st = lb.wait_status(&id, "accepted", LONG, |s| {
        s["status"] == "accepted" && s["worker_pid"].is_u64()
    });
    let t = Instant::now();
    signal(st["worker_pid"].as_i64().unwrap(), sig);
    let fin = lb.wait_status(&id, "terminal", LONG, is_terminal);
    let took = t.elapsed();
    assert_eq!(fin["status"], "aborted", "{fin}");
    assert_eq!(fin["error"]["code"], "cancelled", "{fin}");
    assert!(
        took <= ABORT_BOUND,
        "AC-317: within 1 s of the signal, took {took:?}"
    );
    assert!(
        codes_of(&fin["warnings"]).contains(&"live_turn_continues".to_owned()),
        "AC-317: the send frame was written, so warn live_turn_continues: {fin}"
    );
    let err = failed(
        &lb.glen(["wait", &job_handle(&id), "--json"]).run(),
        "job_failed",
    );
    assert_job_failed(&err, "aborted", "cancelled");
    lb.assert_quiet();
    assert!(ext.alive(), "the omp turn keeps running");
}

#[test]
fn ac317_sigterm_on_a_live_worker_cancels_observation() {
    live_signal_cancels(Signal::TERM);
}

#[test]
fn ac317_sigint_on_a_live_worker_cancels_observation() {
    live_signal_cancels(Signal::INT);
}

#[test]
fn ac317_sighup_on_a_live_worker_cancels_observation() {
    live_signal_cancels(Signal::HUP);
}

#[test]
fn ac317_ac307_control_ops_on_a_live_job_are_unsupported_and_the_job_continues() {
    let lb = LiveBox::new();
    let _ext = lb.start_ext(A.id, "slow", &[("FAKE_EXT_DELAY_MS", "4000")], &[]);
    let d = data_of(&lb.send(A.handle, "hi", &["--no-wait"]).run(), 10);
    let id = job_id(&d);
    lb.wait_status(&id, "accepted", LONG, |s| s["status"] == "accepted");
    let control = lb.job_dir(&id).join("control");
    let t = Instant::now();
    for op in RESERVED_OPS {
        write_control(
            &lb,
            &id,
            &format!("{op}.json"),
            &json!({"v":1,"op":op,"args":match op {
                "set_model" => json!({"provider":"fake","model_id":"fake-1"}),
                "set_thinking_level" => json!({"level":"high"}),
                _ => json!({}),
            }})
            .to_string(),
        );
    }
    for op in RESERVED_OPS {
        let done = lb.wait_file(&control.join(format!("{op}.done.json")), LONG);
        assert!(
            t.elapsed() <= ABORT_BOUND,
            "AC-317: {op} answered within 1 s, took {:?}",
            t.elapsed()
        );
        assert_eq!(done["v"], 1, "{op}: {done}");
        assert_eq!(done["ok"], false, "{op}: {done}");
        assert_eq!(done["error"]["code"], "unsupported_op", "{op}: {done}");
        assert!(done["error"]["message"].is_string(), "{op}: {done}");
        let st = lb.status(&id);
        assert!(!is_terminal(&st), "{op}: the job continues: {st}");
    }
    let w = data_of(
        &lb.glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
            .run(),
        0,
    );
    assert_eq!(w["status"], "completed", "{w}");
    assert_eq!(w["assistant_text"], "pong: hi", "{w}");
    lb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-318 — `glen ls` reads the extension's state.
// ---------------------------------------------------------------------------------------------

fn ls(lb: &LiveBox, extra: &[&str]) -> (Value, Duration) {
    let mut args = vec!["ls"];
    args.extend_from_slice(extra);
    args.push("--json");
    let out = lb.glen(args).run();
    let d = out.ok_data();
    out.assert_stderr_empty().assert_no_ansi();
    (d, out.elapsed)
}

#[test]
fn ac318_an_answering_endpoint_sets_live_or_busy_from_the_extension() {
    let lb = LiveBox::new();
    let mut ext = lb.start_ext(A.id, "ok", &[], &[]);
    let (d, _) = ls(&lb, &[]);
    let a = row(&d, A.id);
    assert_eq!(a["state"], "live", "{a}");
    assert_eq!(a["state_source"], "extension", "{a}");
    assert_eq!(a["state_detail"], "tui", "{a}");
    for fx in [&B, &C, &G] {
        let r = row(&d, fx.id);
        assert_eq!(r["state_source"], "disk", "AC-318: other rows keep W1: {r}");
    }
    assert!(
        ext.ops_in().iter().all(|o| o == "state"),
        "{:?}",
        ext.ops_in()
    );
    ext.stop();

    let _busy = lb.start_ext(A.id, "busy", &[], &[]);
    let (d, _) = ls(&lb, &[]);
    let a = row(&d, A.id);
    assert_eq!(a["state"], "busy", "{a}");
    assert_eq!(a["state_source"], "extension", "{a}");
}

#[test]
fn ac318_ls_live_includes_an_extension_row_while_the_owner_lease_is_free() {
    let lb = LiveBox::new();
    let _ext = lb.start_ext(A.id, "ok", &[], &[]);
    let (d, _) = ls(&lb, &["--live"]);
    assert_eq!(ids(&d), [A.id], "{d}");
    assert_eq!(row(&d, A.id)["state_source"], "extension");
}

#[test]
fn ac318_unresponsive_or_mismatched_endpoints_fall_back_to_disk_within_a_second() {
    let lb = LiveBox::new();
    let (_, baseline) = ls(&lb, &[]);
    let _hangs: Vec<Ext> = [&A, &C, &G]
        .iter()
        .map(|fx| lb.start_ext(fx.id, "hang", &[], &[]))
        .collect();
    let _mismatch = lb.start_ext(B.id, "mismatch", &[], &[]);
    let (d, took) = ls(&lb, &[]);
    for fx in [&A, &B, &C, &G] {
        let r = row(&d, fx.id);
        assert_eq!(r["state_source"], "disk", "{r}");
        assert_session_object(r);
    }
    assert!(
        took < baseline + Duration::from_millis(1250),
        "AC-318: unresponsive endpoints add at most 1 s (baseline {baseline:?}, took {took:?})"
    );
}

#[test]
fn ac318_ls_never_deletes_a_registry_and_never_creates_glen_home() {
    let lb = LiveBox::new();
    ls(&lb, &[]);
    assert!(!lb.glen_home().exists(), "ls created GLEN_HOME");

    let dead = dead_pid();
    let sock = lb.run_live().join(format!("{dead}.sock"));
    let reg = write_registry_doc(&lb, A.id, &registry_doc(A.id, dead, &sock));
    let refused = lb.run_live().join("refused.sock");
    stale_socket(&refused);
    let reg_c = write_registry_doc(&lb, C.id, &registry_doc(C.id, std::process::id(), &refused));
    let (d, _) = ls(&lb, &[]);
    assert_eq!(row(&d, A.id)["state_source"], "disk");
    assert!(
        reg.exists() && reg_c.exists(),
        "AC-318: ls never deletes a registry"
    );
}

// ---------------------------------------------------------------------------------------------
// AC-323 — version mismatch warns and still routes live.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac323_an_extension_version_mismatch_warns_with_both_versions() {
    let (lb, _ext, out) = live_run("ok", &[("FAKE_EXT_VERSION", "0.0.1")], &[]);
    let env = ok_doc(&out, 0);
    assert_eq!(env["data"]["job"]["route"], "live", "{env}");
    let w = env["warnings"]
        .as_array()
        .and_then(|w| w.iter().find(|x| x["code"] == "extension_version_mismatch"))
        .unwrap_or_else(|| panic!("AC-323 warning missing: {env}"))
        .clone();
    let msg = w["message"].as_str().unwrap();
    assert!(
        msg.contains("0.0.1") && msg.contains(PKG_VERSION),
        "AC-323: the warning names both versions: {w}"
    );
    lb.assert_quiet();
}

#[test]
fn ac323_a_matching_extension_version_does_not_warn() {
    let (lb, _ext, out) = live_run("ok", &[], &[]);
    let env = ok_doc(&out, 0);
    assert!(
        !warning_codes(&env).contains(&"extension_version_mismatch".to_owned()),
        "{env}"
    );
    lb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-326 (orchestrator O3) — endpoint-only sessions that are not on disk yet.
// ---------------------------------------------------------------------------------------------

/// A plain-token uuidv7-shaped id that no fixture has.
const X_ID: &str = "01a2ffff-0326-7000-8000-00000000e0e7";

fn endpoint_only(lb: &LiveBox, cwd: Option<&Path>) -> Ext {
    let cwd_s;
    let env: Vec<(&str, &str)> = match cwd {
        Some(c) => {
            cwd_s = s(c);
            vec![("FAKE_EXT_CWD", cwd_s.as_str())]
        }
        None => vec![],
    };
    lb.start_ext(X_ID, "ok", &env, &[])
}

/// The session object of the endpoint-only session (§A10 W3 clarifications: `state_source` is
/// `"extension"` everywhere, superseding O3's `"endpoint"`; an unwritten session has `path:null`).
fn assert_endpoint_only_session(x: &Value) {
    assert_eq!(x["id"], X_ID, "{x}");
    assert_eq!(x["handle"], tail7(X_ID), "{x}");
    assert_eq!(x["state"], "live", "{x}");
    assert_eq!(x["state_source"], "extension", "{x}");
    assert!(
        x["path"].is_null(),
        "an unwritten session has path:null: {x}"
    );
}

fn assert_endpoint_only_row(d: &Value) {
    assert_endpoint_only_session(row(d, X_ID));
}

#[test]
fn ac326_an_endpoint_only_session_is_listed_in_its_project_and_with_all() {
    let lb = LiveBox::new();
    let _ext = endpoint_only(&lb, Some(&lb.bx.alpha));
    let (d, _) = ls(&lb, &[]);
    assert_endpoint_only_row(&d);
    let out = lb.rb.glen_in(&lb.bx.beta, ["ls", "--json"]).run();
    let beta = out.ok_data();
    assert!(
        !ids(&beta).contains(&X_ID.to_owned()),
        "another project's endpoint is out of scope: {beta}"
    );
    let (all, _) = ls(&lb, &["--all"]);
    assert_endpoint_only_row(&all);
}

#[test]
fn ac326_an_endpoint_without_a_cwd_uses_the_invoking_project() {
    let lb = LiveBox::new();
    let _ext = endpoint_only(&lb, None);
    let (d, _) = ls(&lb, &[]);
    assert_endpoint_only_row(&d);
    let out = lb.rb.glen_in(&lb.bx.beta, ["ls", "--json"]).run();
    assert_endpoint_only_row(&out.ok_data());
}

#[test]
fn ac326_show_and_tail_of_an_endpoint_only_session_are_empty_with_a_warning() {
    let lb = LiveBox::new();
    let _ext = endpoint_only(&lb, Some(&lb.bx.alpha));
    let h = tail7(X_ID);
    for args in [["show", X_ID], ["show", h.as_str()], ["tail", h.as_str()]] {
        let out = lb.glen([args[0], args[1], "--json"]).run();
        let env = ok_doc(&out, 0);
        assert_eq!(env["data"]["items"], json!([]), "{args:?}: {env}");
        assert_endpoint_only_session(&env["data"]["session"]);
        assert!(
            warning_codes(&env).contains(&"not_yet_persisted".to_owned()),
            "{args:?}: O3 warning: {env}"
        );
    }
}

#[test]
fn ac326_send_reaches_an_endpoint_only_session_by_handle_and_latest() {
    let lb = LiveBox::new();
    let ext = endpoint_only(&lb, Some(&lb.bx.alpha));
    let d = data_of(&lb.send(&tail7(X_ID), "hi", &["--wait", "20s"]).run(), 0);
    assert_eq!(d["job"]["route"], "live", "{d}");
    assert_eq!(d["job"]["session_id"], X_ID, "{d}");
    assert_eq!(d["assistant_text"], "pong: hi", "{d}");
    let d = data_of(&lb.send("@latest", "again", &["--wait", "20s"]).run(), 0);
    assert_eq!(
        d["job"]["session_id"], X_ID,
        "O3: @latest covers the endpoint-only session started just now: {d}"
    );
    assert_eq!(d["job"]["route"], "live", "{d}");
    assert_eq!(
        ext.ops_in().iter().filter(|o| *o == "send").count(),
        2,
        "{:?}",
        ext.ops_in()
    );
    lb.assert_quiet();
    lb.assert_no_omp();
}

// ---------------------------------------------------------------------------------------------
// AC-322, AC-324 — no omp writes; JSON stays one envelope on the live route.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac322_the_live_route_never_writes_omp_state() {
    let lb = LiveBox::new();
    let _ext = lb.start_ext(A.id, "progress", &[], &[]);
    let sessions = snapshot(&lb.bx.sessions);
    let agent_before: Vec<String> = list_tree(&lb.bx.agent);
    data_of(&lb.send(A.handle, "hi", &["--wait", "20s"]).run(), 0);
    ls(&lb, &[]);
    lb.glen(["inbox", "--json"]).run().ok_data();
    lb.assert_quiet();
    // The snapshot covers sizes and mtimes of every entry, including the fixture's existing
    // `.<file>.jsonl.lock.os` gate: nothing was written, renamed, created or deleted.
    assert_eq!(snapshot(&lb.bx.sessions), sessions, "omp session files");
    assert_eq!(list_tree(&lb.bx.agent), agent_before, "the omp agent dir");
}

#[test]
fn ac324_live_send_human_mode_reports_the_reply() {
    let lb = LiveBox::new();
    let _ext = lb.start_ext(A.id, "ok", &[], &[]);
    let out = lb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--human"])
        .run();
    out.assert_exit(0).assert_no_ansi();
    assert!(!out.is_json(), "human mode: {}", out.ctx());
    assert!(out.out().contains("pong: hi"), "{}", out.ctx());
    lb.assert_quiet();
}

/// The registry's `session_file`, set the way the extension republishes it after omp's first
/// write (atomic: dotfile, then rename).
fn publish_session_file(lb: &LiveBox, id: &str, file: &Path) {
    let p = lb.registry_path(id);
    let mut reg = read_json(&p).expect("a registry to update");
    reg["session_file"] = json!(file);
    let tmp = lb.run_live().join(".republish.tmp");
    write_private(&tmp, &serde_json::to_vec(&reg).unwrap());
    fs::rename(&tmp, &p).unwrap();
}

#[test]
fn ac326_tail_follow_on_an_endpoint_only_session_waits_for_the_transcript_then_streams_it() {
    let lb = LiveBox::new();
    let _ext = endpoint_only(&lb, Some(&lb.bx.alpha));
    let mut st = lb.bx.follow(&["tail", &tail7(X_ID), "-f", "--json"]);
    // Nothing to stream yet, and the stream keeps waiting.
    let quiet = Instant::now() + Duration::from_secs(3);
    while Instant::now() < quiet
        && let Some(l) = st.next_line(Duration::from_millis(800))
    {
        let ev = parse_stream_line(&l, &st.ctx());
        assert_ne!(
            ev["event"],
            "item",
            "no transcript exists yet\n{}",
            st.ctx()
        );
        assert_ne!(ev["ok"], false, "waiting is not an error\n{}", st.ctx());
    }
    let ts = "2026-10-30T12:00:00.000Z";
    let file = lb.bx.add(
        ALPHA,
        ts,
        X_ID,
        Some(&s(&lb.bx.alpha)),
        &[
            user("x0000001", None, ts, "First words."),
            reply("x0000002", Some("x0000001"), ts, "First answer."),
        ],
    );
    publish_session_file(&lb, X_ID, &file);
    let mut seen = Vec::new();
    let deadline = Instant::now() + T;
    while seen.len() < 2 {
        let left = deadline.saturating_duration_since(Instant::now());
        let ev = st.next_json(left);
        assert_ne!(ev["ok"], false, "{}", st.ctx());
        if ev["event"] == "item" {
            seen.push(ev["item"]["id"].as_str().unwrap().to_owned());
        }
    }
    assert_eq!(seen, ["x0000001", "x0000002"], "{}", st.ctx());
    st.interrupt();
    assert_eq!(st.wait(T).code(), Some(130), "{}", st.ctx());
    assert_eq!(st.last_line(), json!({"v": 1, "event": "end", "ok": true}));
}

#[test]
fn ac326_tail_follow_ends_not_found_when_the_tui_quits_without_writing() {
    let lb = LiveBox::new();
    let mut ext = endpoint_only(&lb, Some(&lb.bx.alpha));
    let mut st = lb.bx.follow(&["tail", &tail7(X_ID), "-f", "--json"]);
    assert!(
        st.next_line(Duration::from_millis(800))
            .is_none_or(|l| parse_stream_line(&l, "")["ok"] != false),
        "{}",
        st.ctx()
    );
    ext.stop();
    assert!(!ext.registry.exists(), "the endpoint is gone");
    let t = Instant::now();
    let status = st.wait(Duration::from_secs(20));
    let took = t.elapsed();
    assert_eq!(status.code(), Some(3), "not_found exits 3\n{}", st.ctx());
    let last = st.last_line();
    assert_eq!(last["ok"], false, "{last}");
    assert_eq!(last["error"]["code"], "not_found", "{last}");
    assert!(
        took >= Duration::from_secs(4) && took < Duration::from_secs(12),
        "after the five-second disappearance grace: {took:?}"
    );
}
