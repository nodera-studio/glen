//! W10 tech-debt paydown (`.claude/plans/v010-wave-10-tech-debt.md`: AC-1001…AC-1029, the
//! orchestrator decisions of 2026-10-31, and fix cycle 1 in `v010-wave-10-tech-debt.fixes.md`),
//! derived from the plan, `docs/json.md` and `docs/design.md` without reading the implementation.
//!
//! Covered elsewhere, so not repeated here:
//! - AC-1013 (the 20-run `flake-proof` workflow) is CI-only.
//! - AC-1020…AC-1024 and the AC-1028 runtime need bwrap on Linux: `tests/linux_sandbox.rs`
//!   (the `linux-sandbox` CI job). `agent.rs` `ac410_network_disabled_*` pin the marker-alone
//!   behaviour in the host namespace on every OS.
//! - AC-1015 (`jobs::lock::acquire` tries once before `TimedOut`) has no CLI-observable seam.
//! - AC-1017 and AC-1019 need a writer racing inside glen's swap window, which only an
//!   in-process hook can open deterministically; the black-box side of AC-1018 is below.
//! - Already in the suite: `write_failure.rs` (AC-1001/AC-1002 for doctor, completions, help,
//!   version, ls, usage), `inbox.rs` ac319/ac320 (AC-1003 cursors), `rpc.rs` ac209 (AC-1008 and
//!   AC-1012 for `send`), `agent.rs` ac412 (the `error.not_found.outside_cwd` contract for `show`).

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use rustix::process::Signal;
use serde_json::{Value, json};

use crate::live_support::*;
use crate::reader::*;
use crate::rpc_support::*;
use crate::support::*;

/// AC-1004: the allowed `details.reason` values of an exit-6 `no_route` envelope.
const NO_ROUTE_REASONS: [&str; 8] = [
    "live_owner",
    "unknown_lease",
    "unhealthy_endpoint",
    "endpoint_gone",
    "endpoint_appeared",
    "rpc_job_running",
    "sandboxed",
    "extension_outdated",
];

/// Fix cycle 1, decision 5: a pre-W10 job record's no_route is normalised to this reason.
const LEGACY_REASON: &str = "unknown";

fn running_as_root() -> bool {
    rustix::process::geteuid().as_raw() == 0
}

fn text_of(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------------------------
// Shared assertions.
// ---------------------------------------------------------------------------------------------

/// Exit 6 `no_route` whose `details.reason` is a string from AC-1004's set and equals `reason`.
fn no_route(out: &Out, reason: &str) -> Value {
    let err = failed(out, "no_route");
    let got = err["details"]["reason"].as_str().unwrap_or_else(|| {
        panic!(
            "AC-1004: an exit-6 no_route carries a string details.reason\n{}",
            out.ctx()
        )
    });
    assert!(
        NO_ROUTE_REASONS.contains(&got) || got == LEGACY_REASON,
        "AC-1004: {got:?} is not an allowed reason\n{}",
        out.ctx()
    );
    assert_eq!(got, reason, "AC-1004/AC-1005 reason\n{}", out.ctx());
    err
}

/// AC-1006: a worker-side refusal keeps `status`, `job`, `error` and `forked` beside the
/// top-level `reason`.
fn assert_worker_refusal(err: &Value, job: &str) {
    let d = &err["details"];
    for k in ["reason", "status", "job", "error", "forked"] {
        assert!(d.get(k).is_some(), "AC-1006: details lack `{k}`: {err}");
    }
    assert_eq!(d["status"], "failed", "{err}");
    assert_eq!(d["job"]["id"], job, "{err}");
    assert_eq!(d["job"]["handle"], job_handle(job), "{err}");
    assert_eq!(d["error"]["code"], "no_route", "{err}");
}

/// Exit `code` with nothing on stderr (AC-1001: no further output after EPIPE).
fn assert_silent(out: &Out, code: i32) {
    out.assert_no_panic();
    assert_eq!(out.code(), code, "AC-1001 exit code\n{}", out.ctx());
    assert!(
        out.stderr.is_empty(),
        "AC-1001: nothing on stderr after EPIPE\n{}",
        out.ctx()
    );
}

/// Wait for a child whose stdout is not a pipe of ours; returns (exit code, stderr).
fn finish_unpiped(mut child: Child, desc: &str, timeout: Duration) -> (Option<i32>, String) {
    let mut se = child.stderr.take().expect("piped stderr");
    let te = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let started = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            panic!("`glen {desc}` did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let err = String::from_utf8_lossy(&te.join().unwrap()).into_owned();
    (status.code(), err)
}

fn texts(d: &Value) -> Vec<String> {
    arr(&d["messages"])
        .iter()
        .map(|m| m["text"].as_str().unwrap().to_owned())
        .collect()
}

fn inbox_path(glen_home: &Path, session: &str) -> PathBuf {
    glen_home.join(format!("inbox/{session}.ndjson"))
}

// ---------------------------------------------------------------------------------------------
// AC-1001, AC-1002 — EPIPE keeps the outcome's own exit code.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac1001_a_not_found_envelope_on_a_closed_stdout_keeps_exit_3() {
    let bx = OmpBox::new();
    for args in [
        vec!["show", "fffffff", "--json"],
        vec!["tail", "fffffff", "--json"],
        vec!["tail", "fffffff", "-f", "--json"],
    ] {
        let out = bx.glen(&args).stdout(closed_stdout()).run();
        assert_silent(&out, 3);
    }
    bx.assert_no_state();
}

#[test]
fn ac1001_a_no_route_envelope_on_a_closed_stdout_keeps_exit_6() {
    let rb = RpcBox::new();
    let _lease = hold_lease(&rb.bx.owners(), A.id);
    let out = rb
        .glen(["send", A.handle, "hi", "--json"])
        .stdout(closed_stdout())
        .run();
    assert_silent(&out, 6);
    assert!(rb.job_ids().is_empty(), "{}", rb.diag());
    assert!(rb.starts().is_empty(), "omp was started\n{}", rb.diag());
}

#[test]
fn ac1001_running_and_forked_outcomes_on_a_closed_stdout_keep_10_and_11() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--no-wait", "--json"])
        .stdout(closed_stdout())
        .run();
    assert_silent(&out, 10);
    let id = rb.wait_one_job();
    assert_eq!(rb.wait_terminal(&id)["status"], "completed");
    rb.assert_quiet();

    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hello", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "fork")
        .stdout(closed_stdout())
        .run();
    assert_silent(&out, 11);
    rb.assert_quiet();
}

#[test]
fn ac1001_an_interrupted_send_on_a_closed_stdout_keeps_exit_130() {
    let rb = RpcBox::new();
    let mut cmd = rb.command(&["send", A.handle, "hello", "--wait", "30s", "--json"]);
    cmd.env("FAKE_OMP_DELAY_MS", "3000")
        .process_group(0)
        .stdout(closed_stdout())
        .stderr(Stdio::piped());
    let child = cmd.spawn().expect("spawn glen send");
    let pgid = child.id() as i64;
    let id = rb.wait_one_job();
    rb.wait_status(&id, "accepted", LONG, |s| s["status"] == "accepted");
    signal_group(pgid, Signal::INT);
    let (code, err) = finish_unpiped(child, "send --wait (SIGINT, closed stdout)", LONG);
    assert_eq!(
        code,
        Some(130),
        "AC-1001: interrupted keeps 130; stderr: {err}"
    );
    assert!(
        err.is_empty(),
        "AC-1001: nothing on stderr after EPIPE: {err}"
    );
    assert_eq!(rb.wait_terminal(&id)["status"], "completed");
    rb.assert_quiet();
}

#[test]
fn ac1002_a_failure_envelope_on_a_non_pipe_write_error_exits_1() {
    let bx = OmpBox::new();
    for args in [
        vec!["show", "fffffff", "--json"],
        vec!["show", A.handle, "--json"],
    ] {
        let file = tempfile::NamedTempFile::new().expect("stdout file");
        // Read-only: every write fails EBADF, which is not EPIPE.
        let stdout = fs::File::open(file.path()).expect("open read-only stdout");
        let out = bx.glen(&args).stdout(Stdio::from(stdout)).run();
        out.assert_no_panic();
        assert_eq!(
            out.code(),
            1,
            "AC-1002: a non-EPIPE stdout failure exits 1 whatever the outcome\n{}",
            out.ctx()
        );
        out.assert_stderr_empty();
        assert_eq!(file.as_file().metadata().unwrap().len(), 0);
    }
}

// ---------------------------------------------------------------------------------------------
// AC-1003 — follow modes stop silently on EPIPE.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac1003_tail_follow_stops_silently_when_its_reader_goes_away() {
    let bx = OmpBox::new();
    for mode in ["--json", "--human"] {
        let out = bx
            .glen(["tail", A.handle, "-f", mode])
            .stdout(closed_stdout())
            .run();
        assert_silent(&out, 0);
    }

    let mut cmd = Command::new(BIN);
    cmd.env_clear()
        .args(["tail", A.handle, "-n", "1", "-f", "--json"])
        .env("HOME", &bx.home)
        .env("GLEN_HOME", &bx.glen_home)
        .env("PI_CODING_AGENT_DIR", &bx.agent)
        .env("TMPDIR", &bx.tmpdir)
        .env("TZ", "UTC")
        .env("PATH", &bx.empty_bin)
        .current_dir(&bx.alpha)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn glen tail -f");
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    assert!(
        first.contains("\"event\""),
        "a stream line first: {first:?}"
    );
    drop(reader);
    append_line(
        &bx.path(&A),
        &user(
            "10000011",
            Some("10000007"),
            "2026-10-01T09:10:00.000Z",
            "after the reader left",
        ),
    );
    let mut se = child.stderr.take().unwrap();
    let status = poll_until(
        "tail -f to stop after the broken pipe",
        Duration::from_secs(10),
        || child.try_wait().unwrap(),
    );
    let mut err = String::new();
    let _ = se.read_to_string(&mut err);
    assert_eq!(status.code(), Some(0), "AC-1003: exit 0; stderr: {err}");
    assert!(err.is_empty(), "AC-1003: no stderr text: {err}");
}

#[test]
fn ac1003_inbox_on_a_closed_stdout_is_silent_and_keeps_its_messages_unread() {
    let sb = Sandbox::new();
    append_record(
        &inbox_path(&sb.glen_home, A.id),
        &inbox_record(A.id, None, "kept", "2026-10-30T10:00:00Z"),
    );
    for args in [
        vec!["inbox", "--human"],
        vec!["inbox", "--json"],
        vec!["inbox", "-f", "--json"],
    ] {
        let out = sb.glen(&args).stdout(closed_stdout()).run();
        out.assert_no_panic();
        assert_eq!(out.code(), 0, "AC-1003\n{}", out.ctx());
        assert!(
            out.stderr.is_empty(),
            "AC-1003: no stderr text, no \"messages remain unread\" line\n{}",
            out.ctx()
        );
    }
    let d = data_of(&sb.glen(["inbox", "--peek", "--json"]).run(), 0);
    assert_eq!(texts(&d), ["kept"], "AC-1003: nothing was marked read: {d}");
}

// ---------------------------------------------------------------------------------------------
// AC-1004, AC-1005 — every foreground no_route names its reason.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac1005_a_held_lease_without_an_endpoint_is_live_owner() {
    let rb = RpcBox::new();
    let _lease = hold_lease(&rb.bx.owners(), A.id);
    let out = rb.glen(["send", A.handle, "hi", "--json"]).run();
    no_route(&out, "live_owner");
    let out = rb.glen(["abort", A.handle, "--json"]).run();
    no_route(&out, "live_owner");
    assert!(rb.job_ids().is_empty(), "{}", rb.diag());
    assert!(rb.starts().is_empty(), "omp was started\n{}", rb.diag());
}

#[test]
fn ac1005_a_lease_that_cannot_be_read_is_unknown_lease() {
    // §A3: an id outside `^[A-Za-z0-9_-]{1,128}$` is named by a hash glen does not port, so its
    // lease always probes as unknown, on every OS.
    let rb = RpcBox::new();
    let odd = "w10.odd.lease";
    let path = rb.bx.add(
        ALPHA,
        "2026-10-06T10:00:00.000Z",
        odd,
        Some(&s(&rb.bx.alpha)),
        &[user("aa001005", None, "2026-10-06T10:00:01.000Z", "hi")],
    );
    let out = rb.glen(["send", &s(&path), "hi", "--json"]).run();
    no_route(&out, "unknown_lease");
    assert!(rb.starts().is_empty(), "omp was started\n{}", rb.diag());

    // macOS: the lease file exists but cannot be opened (EACCES is never evidence of a free lease).
    if cfg!(target_os = "macos") && !running_as_root() {
        let rb = RpcBox::new();
        let owners = rb.bx.owners();
        mkdir_private(&owners);
        let lock = owners.join(format!("{}.lock", A.id));
        fs::write(&lock, b"").unwrap();
        set_mode(&lock, 0o000);
        let out = rb.glen(["send", A.handle, "hi", "--json"]).run();
        no_route(&out, "unknown_lease");
        assert!(rb.starts().is_empty(), "omp was started\n{}", rb.diag());
    }
}

#[test]
fn ac1005_an_endpoint_that_does_not_answer_is_unhealthy_endpoint() {
    let lb = LiveBox::new();
    let _p = Peer::start(&lb, A.id, |_, _, s| {
        let _ = s.write_all(b"this is not json\n");
    });
    no_route(
        &lb.send(A.handle, "hi", &["--wait", "20s"]).run(),
        "unhealthy_endpoint",
    );
    no_route(
        &lb.glen(["abort", A.handle, "--json"]).run(),
        "unhealthy_endpoint",
    );
    assert!(lb.job_ids().is_empty(), "{}", lb.diag());
    lb.assert_no_omp();

    // Accepts and reads, never answers: the peer holds the connection until glen gives up.
    let lb = LiveBox::new();
    let _p = Peer::start(&lb, A.id, |_, _, s| {
        let mut sink = Vec::new();
        let _ = s.read_to_end(&mut sink);
    });
    no_route(
        &lb.send(A.handle, "hi", &["--wait", "20s"]).run(),
        "unhealthy_endpoint",
    );
    assert!(lb.job_ids().is_empty(), "{}", lb.diag());
    lb.assert_no_omp();
}

#[test]
fn ac1004_an_endpoint_beside_a_running_rpc_job_is_rpc_job_running() {
    let lb = LiveBox::new();
    let _lock = lb.hold_glen_lock(A.id);
    let ext = lb.start_ext(A.id, "ok", &[], &[]);
    no_route(
        &lb.send(A.handle, "hi", &["--wait", "20s"]).run(),
        "rpc_job_running",
    );
    no_route(
        &lb.glen(["abort", A.handle, "--json"]).run(),
        "rpc_job_running",
    );
    assert!(
        !ext.ops_in().iter().any(|o| o == "send" || o == "abort"),
        "nothing was delivered: {:?}",
        ext.ops_in()
    );
    lb.assert_no_omp();
}

#[test]
fn ac1004_docs_json_lists_every_no_route_reason() {
    let doc = text_of(&repo_root().join("docs/json.md"));
    let line = doc
        .lines()
        .find(|l| l.contains("Allowed `no_route` reasons"))
        .expect("docs/json.md lists the allowed no_route reasons");
    let listed: Vec<&str> = line
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|t| *t != "no_route")
        .collect();
    for r in NO_ROUTE_REASONS {
        assert!(
            listed.contains(&r),
            "docs/json.md lacks reason `{r}`: {line}"
        );
    }
    for r in &listed {
        assert!(
            NO_ROUTE_REASONS.contains(r) || *r == LEGACY_REASON,
            "docs/json.md lists an undeclared reason `{r}`: {line}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// AC-1006 — worker-side refusals lift the reason to the top level in send and wait.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac1006_a_worker_side_held_lease_is_live_owner_in_send_and_wait() {
    let rb = RpcBox::new();
    let lock = rb.hold_glen_lock(A.id);
    let mut cmd = rb.command(&["send", A.handle, "hi", "--wait", "20s", "--json"]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn().expect("spawn glen send");
    let id = rb.wait_one_job();
    rb.wait_status(&id, "waiting_for_lock", LONG, |s| {
        s["waiting_for_lock"] == true
    });
    let _lease = hold_lease(&rb.bx.owners(), A.id);
    drop(lock);
    let out = finish(child, "send --wait (worker-side refusal)", LONG);
    assert_worker_refusal(&no_route(&out, "live_owner"), &id);
    let out = rb
        .glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
        .run();
    assert_worker_refusal(&no_route(&out, "live_owner"), &id);
    rb.assert_quiet();
    assert!(rb.starts().is_empty(), "omp was spawned\n{}", rb.diag());
}

#[test]
fn ac1006_an_endpoint_that_appeared_before_the_worker_ran_is_endpoint_appeared() {
    let lb = LiveBox::new();
    let lock = lb.hold_glen_lock(A.id);
    let d = data_of(&lb.send(A.handle, "hi", &["--no-wait"]).run(), 10);
    assert_eq!(d["job"]["route"], "rpc", "{d}");
    let id = job_id(&d);
    let _ext = lb.start_ext(A.id, "ok", &[], &[]);
    drop(lock);
    let out = lb
        .glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
        .run();
    assert_worker_refusal(&no_route(&out, "endpoint_appeared"), &id);
    lb.assert_quiet();
    lb.assert_no_omp();
}

/// The endpoint answers the foreground hello, then its socket disappears before the worker
/// connects.
fn vanishing_peer(lb: &LiveBox) -> Peer {
    let sock = lb.run_live().join(format!("{}.sock", std::process::id()));
    Peer::start(lb, A.id, move |idx, req, s| {
        if idx == 0 && answer_unary(A.id, req, s) {
            let _ = fs::remove_file(&sock);
        }
    })
}

#[test]
fn ac1006_an_endpoint_gone_before_the_worker_connects_is_endpoint_gone_in_send_and_wait() {
    let lb = LiveBox::new();
    let _p = vanishing_peer(&lb);
    let out = lb.send(A.handle, "hi", &["--wait", "20s"]).run();
    let ids = lb.job_ids();
    assert_eq!(ids.len(), 1, "a live job was created\n{}", out.ctx());
    assert_worker_refusal(&no_route(&out, "endpoint_gone"), &ids[0]);
    let out = lb
        .glen(["wait", &job_handle(&ids[0]), "--timeout", "20s", "--json"])
        .run();
    assert_worker_refusal(&no_route(&out, "endpoint_gone"), &ids[0]);
    lb.assert_quiet();
    lb.assert_no_omp();
}

#[test]
fn ac1004_a_pre_w10_no_route_job_without_a_reason_is_read_as_unknown() {
    // A real worker-side refusal is the template; its copy loses `error.details.reason`, the
    // shape a job persisted before W10 has.
    let rb = RpcBox::new();
    let lock = rb.hold_glen_lock(A.id);
    let template = job_id(&data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .run(),
        10,
    ));
    let lease = hold_lease(&rb.bx.owners(), A.id);
    drop(lock);
    let st = rb.wait_terminal(&template);
    assert_eq!(st["error"]["code"], "no_route", "{st}");
    drop(lease);
    rb.assert_quiet();

    let mut error = st["error"].clone();
    if let Some(d) = error["details"].as_object_mut() {
        d.remove("reason");
        if d.is_empty() {
            error.as_object_mut().unwrap().remove("details");
        }
    }
    let created = SystemTime::now() - Duration::from_secs(60);
    let legacy = job_uuid(ms_since_epoch(created), 0x0010_0400);
    rb.clone_job(&template, &legacy, created, json!({"error": error}), false);
    let out = rb.glen(["wait", &job_handle(&legacy), "--json"]).run();
    assert_worker_refusal(&no_route(&out, LEGACY_REASON), &legacy);
}

// ---------------------------------------------------------------------------------------------
// AC-1007…AC-1010 — `--cwd` scopes show and tail.
// ---------------------------------------------------------------------------------------------

/// `show` and `tail` of `arg` from `cwd` with `extra`; returns each resolved session id.
fn resolved(bx: &OmpBox, cwd: &Path, arg: &str, extra: &[&str]) -> [String; 2] {
    let mut show = vec!["show", arg, "--json"];
    show.extend_from_slice(extra);
    let mut tail = vec!["tail", arg, "-n", "1", "--json"];
    tail.extend_from_slice(extra);
    [show, tail].map(|args| {
        let d = bx.data_in(cwd, &args);
        d["session"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("{args:?}: no session.id: {d}"))
            .to_owned()
    })
}

#[test]
fn ac1007_every_handle_ls_cwd_prints_resolves_with_the_same_cwd() {
    let bx = OmpBox::new();
    for dir in [&bx.alpha, &bx.beta] {
        let d = bx.data(["ls", "--cwd", &s(dir), "--json"]);
        let rows = arr(&d["sessions"]);
        assert!(!rows.is_empty(), "{d}");
        for r in rows {
            // Compared with the unscoped resolution: `show` follows continuations (§A3), so a
            // continued row resolves to its newest copy either way.
            let h = r["handle"].as_str().unwrap();
            let unscoped = resolved(&bx, &bx.alpha, h, &[]);
            let scoped = resolved(&bx, &bx.alpha, h, &["--cwd", &s(dir)]);
            assert_eq!(scoped, unscoped, "AC-1007: {h} --cwd {}", dir.display());
        }
    }
}

#[test]
fn ac1007_latest_with_cwd_is_that_projects_newest_session() {
    let bx = OmpBox::new();
    for (dir, newest) in [(&bx.beta, &E), (&bx.alpha, &G)] {
        for got in resolved(&bx, &bx.alpha, "@latest", &["--cwd", &s(dir)]) {
            assert_eq!(got, newest.id, "AC-1007: @latest --cwd {}", dir.display());
        }
    }
    // A relative DIR is taken from the process cwd.
    for got in resolved(&bx, &bx.alpha, "@latest", &["--cwd", "../beta"]) {
        assert_eq!(got, E.id, "AC-1007: @latest --cwd ../beta");
    }
}

fn assert_outside_cwd(out: &Out, dir: &Path) {
    let err = failed(out, "not_found");
    assert_eq!(
        err["details"]["reason"],
        "outside_cwd",
        "AC-1008\n{}",
        out.ctx()
    );
    let cwd = Path::new(err["details"]["cwd"].as_str().unwrap_or_else(|| {
        panic!("AC-1008: details.cwd is a string\n{}", out.ctx());
    }));
    assert!(
        cwd.is_absolute() && same_file(cwd, dir),
        "AC-1008: details.cwd is the absolute DIR {}\n{}",
        dir.display(),
        out.ctx()
    );
    let hint = err["hint"].as_str().unwrap_or("");
    assert!(
        hint.contains(A.cwd) || hint.contains(A.bucket),
        "AC-1008: the hint names the session's project\n{}",
        out.ctx()
    );
}

#[test]
fn ac1008_a_session_of_another_project_is_not_found_outside_cwd() {
    let bx = OmpBox::new();
    let beta = s(&bx.beta);
    for arg in [A.handle, A.id, &A.id[..13]] {
        for args in [
            vec!["show", arg, "--cwd", &beta, "--json"],
            vec!["tail", arg, "--cwd", &beta, "--json"],
            vec!["tail", arg, "-f", "--cwd", &beta, "--json"],
            vec!["show", arg, "--cwd", "../beta", "--json"],
        ] {
            assert_outside_cwd(&bx.glen(&args).run(), &bx.beta);
        }
    }
    // Matching nothing anywhere stays a plain not_found.
    let out = bx.glen(["show", "fffffff", "--cwd", &beta, "--json"]).run();
    let err = failed(&out, "not_found");
    assert_ne!(err["details"]["reason"], "outside_cwd", "{}", out.ctx());
    bx.assert_no_state();
}

#[test]
fn ac1009_a_handle_ambiguous_across_projects_is_unique_within_cwd() {
    let bx = OmpBox::new();
    // `c0de` ends both A (alpha) and F (beta).
    failed(&bx.glen(["show", "c0de", "--json"]).run(), "ambiguous");
    failed(&bx.glen(["tail", "c0de", "--json"]).run(), "ambiguous");
    for (dir, fx) in [(&bx.alpha, &A), (&bx.beta, &F)] {
        for got in resolved(&bx, &bx.alpha, "c0de", &["--cwd", &s(dir)]) {
            assert_eq!(got, fx.id, "AC-1009: c0de --cwd {}", dir.display());
        }
    }
}

#[test]
fn ac1010_a_file_path_with_cwd_opens_that_file_unfiltered() {
    let bx = OmpBox::new();
    let beta = s(&bx.beta);
    let path = bx.path(&A);
    let name = path.file_name().unwrap().to_str().unwrap().to_owned();
    let bucket = path.parent().unwrap().to_path_buf();
    let rel = format!("./{}/{name}", A.bucket);
    for (cwd, arg) in [
        (&bx.alpha, s(&path)),
        (&bx.sessions, rel),
        // A bare `.jsonl` name is a path too, relative to the process cwd.
        (&bucket, name),
    ] {
        for got in resolved(&bx, cwd, &arg, &["--cwd", &beta]) {
            assert_eq!(
                got,
                A.id,
                "AC-1010: {arg} --cwd beta from {}",
                cwd.display()
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// AC-1011 — `inbox --cwd`.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac1011_inbox_cwd_lists_only_that_project_and_leaves_the_rest_unread() {
    let bx = OmpBox::new();
    let orphan = uuid(0x0010_1100);
    append_record(
        &inbox_path(&bx.glen_home, A.id),
        &inbox_record(A.id, None, "alpha-1", "2026-10-30T10:00:00Z"),
    );
    append_record(
        &inbox_path(&bx.glen_home, D.id),
        &inbox_record(D.id, None, "beta-1", "2026-10-30T10:00:01Z"),
    );
    append_record(
        &inbox_path(&bx.glen_home, &orphan),
        &inbox_record(&orphan, None, "orphan-1", "2026-10-30T10:00:02Z"),
    );
    let (alpha, beta) = (s(&bx.alpha), s(&bx.beta));

    // --session and --cwd intersect; a session matched only outside the scope warns.
    let env = ok_doc(
        &bx.glen([
            "inbox",
            "--cwd",
            &alpha,
            "--session",
            D.handle,
            "--peek",
            "--json",
        ])
        .run(),
        0,
    );
    assert_eq!(env["data"]["total"], 0, "{env}");
    assert!(
        warning_codes(&env).iter().any(|c| c == "outside_cwd"),
        "§A9 inbox: only foreign stems match → outside_cwd warning: {env}"
    );

    let d = bx.data(["inbox", "--cwd", &alpha, "--json"]);
    assert_eq!(texts(&d), ["alpha-1"], "AC-1011: {d}");
    let d = bx.data(["inbox", "--peek", "--json"]);
    assert_eq!(
        texts(&d),
        ["beta-1", "orphan-1"],
        "AC-1011: other projects and the orphan stay unread; no --cwd is the aggregate: {d}"
    );
    let d = bx.data(["inbox", "--cwd", &beta, "--json"]);
    assert_eq!(texts(&d), ["beta-1"], "AC-1011: {d}");
    let d = bx.data(["inbox", "--cwd", &alpha, "--json"]);
    assert_eq!(texts(&d), Vec::<String>::new(), "{d}");
    let d = bx.data(["inbox", "--peek", "--json"]);
    assert_eq!(
        texts(&d),
        ["orphan-1"],
        "AC-1011: a message whose session is in no project is left out and stays unread: {d}"
    );
    let d = bx.data(["inbox", "--json"]);
    assert_eq!(
        texts(&d),
        ["orphan-1"],
        "without --cwd the inbox aggregates: {d}"
    );
}

#[test]
fn ac1011_inbox_follow_with_cwd_picks_up_a_session_created_after_it_started() {
    let bx = OmpBox::new();
    append_record(
        &inbox_path(&bx.glen_home, A.id),
        &inbox_record(A.id, None, "warm", "2026-10-30T10:00:00Z"),
    );
    let mut st = bx.follow(&["inbox", "-f", "--cwd", &s(&bx.alpha), "--json"]);
    let first = st.expect_event("message", T);
    assert_eq!(first["message"]["text"], "warm", "{}", st.ctx());

    // A beta reply sorts first if it were (wrongly) in scope; the new alpha session comes later.
    append_record(
        &inbox_path(&bx.glen_home, D.id),
        &inbox_record(D.id, None, "beta-late", "2026-10-30T10:00:01Z"),
    );
    let fresh = uuid(0x0010_1101);
    bx.add(
        ALPHA,
        "2026-10-30T10:00:01.500Z",
        &fresh,
        Some(&s(&bx.alpha)),
        &[user("aa001011", None, "2026-10-30T10:00:01.600Z", "hi")],
    );
    append_record(
        &inbox_path(&bx.glen_home, &fresh),
        &inbox_record(&fresh, None, "new-alpha", "2026-10-30T10:00:02Z"),
    );
    let next = st.expect_event("message", T);
    assert_eq!(
        next["message"]["text"],
        "new-alpha",
        "AC-1011: -f includes a session that appeared in scope after it started\n{}",
        st.ctx()
    );
    st.interrupt();
    let status = st.wait(T);
    assert_eq!(status.code(), Some(130), "{}", st.ctx());
    assert!(
        !st.lines.concat().contains("beta-late"),
        "AC-1011: a beta reply never streams under --cwd alpha\n{}",
        st.ctx()
    );
    let d = bx.data(["inbox", "--peek", "--json"]);
    assert_eq!(texts(&d), ["beta-late"], "out of scope stays unread: {d}");
}

// ---------------------------------------------------------------------------------------------
// AC-1012 — `send <handle> --cwd DIR`.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac1012_a_handle_whose_recorded_cwd_is_missing_runs_omp_in_dir() {
    // A's header cwd `/home/fixture/work/alpha` does not exist, so DIR is used; the process cwd
    // (beta) is not.
    let rb = RpcBox::new();
    let d = data_of(
        &rb.glen_in(
            &rb.bx.beta.clone(),
            [
                "send",
                A.handle,
                "hi",
                "--cwd",
                &s(&rb.bx.alpha),
                "--wait",
                "20s",
                "--json",
            ],
        )
        .run(),
        0,
    );
    assert_eq!(d["session"]["id"], A.id, "{d}");
    let cwd = rb.starts()[0]["cwd"].as_str().unwrap().to_owned();
    assert!(
        same_file(Path::new(&cwd), &rb.bx.alpha),
        "AC-1012: omp runs in --cwd DIR when the recorded cwd is missing, not {cwd}"
    );
    rb.assert_quiet();
}

#[test]
fn ac1010_send_to_a_file_path_ignores_a_foreign_cwd_scope() {
    let rb = RpcBox::new();
    let d = data_of(
        &rb.glen([
            "send",
            &s(&rb.bx.path(&A)),
            "hi",
            "--cwd",
            &s(&rb.bx.beta),
            "--wait",
            "20s",
            "--json",
        ])
        .run(),
        0,
    );
    assert_eq!(d["session"]["id"], A.id, "AC-1010: {d}");
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-1014 — the live client reads an answer the endpoint gave before it closed.
// ---------------------------------------------------------------------------------------------

/// What the partial peer does with a request too large for its 4 KiB read.
#[derive(Clone, Copy, PartialEq)]
enum OnLarge {
    /// Answer the request (ack, consumed, settled) from its first 4 KiB, then close.
    Answer,
    /// Close without writing anything.
    Close,
}

/// An endpoint that answers small requests normally and reads only the first 4 KiB of a large
/// one, so glen's write is still blocked when the peer closes.
struct PartialPeer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// The first 4 KiB of every large request.
    large: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Drop for PartialPeer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The top-level request `id` from a request prefix (`"id":"…"`; `job_id` and the like never
/// match because a quote must precede `id`).
fn request_id(prefix: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(prefix);
    let at = text.find("\"id\":\"")? + 6;
    let end = text[at..].find('"')?;
    Some(text[at..at + end].to_owned())
}

fn serve_partial(mut s: UnixStream, on_large: OnLarge, large: &Mutex<Vec<Vec<u8>>>) {
    let _ = s.set_nonblocking(false);
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while buf.len() < 4096 && !buf.contains(&b'\n') {
        let want = chunk.len().min(4096 - buf.len());
        match s.read(&mut chunk[..want]) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    if let Some(nl) = buf.iter().position(|b| *b == b'\n') {
        let req: Value = serde_json::from_slice(&buf[..nl]).unwrap_or(Value::Null);
        answer_unary(A.id, &req, &mut s);
        return;
    }
    large.lock().unwrap().push(buf.clone());
    if on_large == OnLarge::Answer {
        let id = request_id(&buf).unwrap_or_else(|| {
            panic!(
                "plan Step 3 premise: the request id is within the first 4 KiB: {:?}",
                String::from_utf8_lossy(&buf[..buf.len().min(200)])
            )
        });
        write_frame(
            &mut s,
            &json!({"v":1,"id":id,"ok":true,"data":{"delivered_as":"prompt","session_id":A.id,"request_id":id}}),
        );
        write_frame(&mut s, &json!({"v":1,"id":id,"event":"consumed"}));
        write_frame(
            &mut s,
            &json!({"v":1,"id":id,"event":"settled","status":"completed","assistant_text":"drained","stop_reason":"stop","error_message":null}),
        );
    }
    // Dropping `s` closes the connection with most of the request unread.
}

fn partial_peer(lb: &LiveBox, on_large: OnLarge) -> PartialPeer {
    mkdir_private(&lb.run_live());
    let socket = lb.run_live().join(format!("{}.sock", std::process::id()));
    let listener = UnixListener::bind(&socket).expect("bind the peer socket");
    set_mode(&socket, 0o600);
    listener.set_nonblocking(true).unwrap();
    write_registry_doc(lb, A.id, &registry_doc(A.id, std::process::id(), &socket));
    let stop = Arc::new(AtomicBool::new(false));
    let large = Arc::new(Mutex::new(Vec::new()));
    let (st, lg) = (Arc::clone(&stop), Arc::clone(&large));
    let thread = std::thread::spawn(move || {
        let mut conns = Vec::new();
        while !st.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((s, _)) => {
                    let lg = Arc::clone(&lg);
                    conns.push(std::thread::spawn(move || serve_partial(s, on_large, &lg)));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        for c in conns {
            let _ = c.join();
        }
    });
    PartialPeer {
        stop,
        thread: Some(thread),
        large,
    }
}

/// 500 KiB of ASCII: below the 512 KiB stdin limit, far above any socket buffer.
fn large_message() -> Vec<u8> {
    vec![b'x'; 500 * 1024]
}

#[test]
fn ac1014_an_answer_written_before_the_peer_closed_is_used() {
    let lb = LiveBox::new();
    let peer = partial_peer(&lb, OnLarge::Answer);
    let out = lb.glen_stdin(
        &["send", A.handle, "-", "--wait", "20s", "--json"],
        &[],
        &large_message(),
    );
    let d = data_of(&out, 0);
    assert_eq!(d["job"]["route"], "live", "{d}");
    assert_eq!(
        d["assistant_text"], "drained",
        "AC-1014: the buffered answer is used, not a protocol error: {d}"
    );
    assert_eq!(
        peer.large.lock().unwrap().len(),
        1,
        "AC-1014: the request is never sent again"
    );
    lb.assert_quiet();
    lb.assert_no_omp();
}

#[test]
fn ac1014_a_peer_that_closes_without_answering_fails_protocol_once() {
    let lb = LiveBox::new();
    let peer = partial_peer(&lb, OnLarge::Close);
    let out = lb.glen_stdin(
        &["send", A.handle, "-", "--wait", "20s", "--json"],
        &[],
        &large_message(),
    );
    let err = failed(&out, "job_failed");
    assert_job_failed(&err, "failed", "protocol");
    let msg = err["details"]["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("endpoint closed the connection before reading the request"),
        "AC-1014 message: {err}"
    );
    assert!(
        out.elapsed < Duration::from_secs(10),
        "AC-1014: the drain read is bounded; took {:?}",
        out.elapsed
    );
    assert_eq!(
        peer.large.lock().unwrap().len(),
        1,
        "AC-1014: the request is never sent again"
    );
    lb.assert_quiet();
    lb.assert_no_omp();
}

// ---------------------------------------------------------------------------------------------
// AC-1029 — answer-then-close.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac1029_an_answer_followed_by_an_immediate_close_is_used() {
    let lb = LiveBox::new();
    // The peer writes its answer and closes at once (the stream is dropped after `answer_unary`).
    let _p = Peer::start(&lb, A.id, |_, req, s| {
        answer_unary(A.id, req, s);
    });
    for n in 0..20 {
        let d = data_of(&lb.glen(["ls", "--json"]).run(), 0);
        assert_eq!(
            row(&d, A.id)["state_source"],
            "extension",
            "AC-1029 run {n}: the endpoint's answer is used: {d}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// AC-1016 — test peers serve accepted sockets in blocking mode.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac1016_fake_ext_answers_a_request_written_after_a_delay() {
    let lb = LiveBox::new();
    let ext = lb.start_ext(A.id, "ok", &[], &[]);
    let mut s = UnixStream::connect(&ext.socket).expect("connect to fake_ext");
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    // The request arrives after the peer accepted: a non-blocking accepted socket would read
    // WouldBlock, close, and fail this write or the read below.
    std::thread::sleep(Duration::from_millis(300));
    s.write_all(
        b"{\"v\":1,\"id\":\"w10-late\",\"op\":\"hello\",\"args\":{\"client\":\"conformance\",\"client_version\":\"0\",\"protocols\":[1]}}\n",
    )
    .expect("AC-1016: the peer still reads a late request");
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).unwrap();
    let f: Value = serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("{e}: {line:?}"));
    assert_eq!(f["id"], "w10-late", "{f}");
    assert_eq!(f["ok"], true, "AC-1016: {f}");
}

// ---------------------------------------------------------------------------------------------
// AC-1018 — a replace on a filesystem that can swap is atomic and silent.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac1018_forcing_over_a_modified_extension_swaps_without_a_warning_or_a_temp_file() {
    let sb = Sandbox::new();
    let agent = sb.dir("agent");
    let run = |args: &[&str]| {
        let mut a = vec!["install-extension"];
        a.extend_from_slice(args);
        a.push("--json");
        ok_doc(&sb.glen(a).env("PI_CODING_AGENT_DIR", &agent).run(), 0)
    };
    assert_eq!(run(&[])["data"]["action"], "installed");
    let file = agent.join("extensions/glen/index.ts");
    let fresh = fs::read(&file).unwrap();
    let mut edited = fresh.clone();
    edited.extend_from_slice(b"// a local edit\n");
    fs::write(&file, &edited).unwrap();
    let env = run(&["--force"]);
    assert_eq!(env["data"]["action"], "replaced", "{env}");
    let codes = warning_codes(&env);
    for code in ["replace_not_atomic", "changed_during_install"] {
        assert!(
            !codes.iter().any(|c| c == code),
            "AC-1018: no `{code}` where the filesystem swaps: {env}"
        );
    }
    assert_eq!(fs::read(&file).unwrap(), fresh, "the current rendering");
    assert_eq!(
        list_tree(file.parent().unwrap()),
        ["index.ts"],
        "AC-1017: no temp file is left"
    );
}

#[test]
fn ac1018_forcing_over_a_modified_skill_swaps_without_a_warning_or_a_temp_file() {
    let sb = Sandbox::new();
    let run = |args: &[&str]| {
        let mut a = vec!["install-skill", "--claude"];
        a.extend_from_slice(args);
        a.push("--json");
        ok_doc(&sb.glen(a).run(), 0)
    };
    let env = run(&[]);
    let file = PathBuf::from(env["data"]["targets"][0]["path"].as_str().unwrap());
    let fresh = fs::read(&file).unwrap();
    let mut edited = fresh.clone();
    edited.extend_from_slice(b"\nA local edit.\n");
    fs::write(&file, &edited).unwrap();
    let env = run(&["--force"]);
    assert_ne!(env["data"]["targets"][0]["action"], "skipped", "{env}");
    let codes = warning_codes(&env);
    for code in ["replace_not_atomic", "changed_during_install"] {
        assert!(
            !codes.iter().any(|c| c == code),
            "AC-1018: no `{code}` where the filesystem swaps: {env}"
        );
    }
    assert_eq!(fs::read(&file).unwrap(), fresh, "the current skill");
    assert_eq!(
        list_tree(file.parent().unwrap()),
        ["SKILL.md"],
        "AC-1017: no temp file is left"
    );
}

#[test]
fn ac1018_docs_json_lists_the_new_warning_codes() {
    let doc = text_of(&repo_root().join("docs/json.md"));
    let rows: Vec<&str> = doc.lines().filter(|l| l.starts_with("| `")).collect();
    let row_for = |code: &str| {
        rows.iter()
            .find(|l| l.starts_with(&format!("| `{code}`")))
            .copied()
    };
    let rna = row_for("replace_not_atomic").unwrap_or_else(|| {
        panic!("docs/json.md Warning codes has no `replace_not_atomic` row (AC-1018, Step 23)")
    });
    assert!(
        rna.contains("install-extension") && rna.contains("install-skill"),
        "{rna}"
    );
    let oc = row_for("outside_cwd").unwrap_or_else(|| {
        panic!("docs/json.md Warning codes has no `outside_cwd` row (inbox, Step 23)")
    });
    assert!(oc.contains("inbox"), "{oc}");
}

// ---------------------------------------------------------------------------------------------
// AC-1025 — light accent_2.
// ---------------------------------------------------------------------------------------------

/// The xterm-256 colour of index `i` (≥16).
fn xterm_rgb(i: u64) -> (u8, u8, u8) {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match i {
        16..=231 => {
            let n = (i - 16) as usize;
            (LEVELS[n / 36], LEVELS[(n / 6) % 6], LEVELS[n % 6])
        }
        232..=255 => {
            let g = (8 + 10 * (i - 232)) as u8;
            (g, g, g)
        }
        _ => panic!("index {i} is terminal-defined"),
    }
}

fn hex_rgb(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim_start_matches('#');
    let p = |k: usize| u8::from_str_radix(&h[k..k + 2], 16).unwrap();
    (p(0), p(2), p(4))
}

/// WCAG 2 contrast ratio against `#FFFFFF`.
fn contrast_on_white((r, g, b): (u8, u8, u8)) -> f64 {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
    1.05 / (l + 0.05)
}

#[test]
fn ac1025_every_light_text_token_meets_aa_on_white() {
    let p: Value =
        serde_json::from_str(&text_of(&repo_root().join("src/render/palette.json"))).unwrap();
    let tokens = arr(&p["tokens"]);
    let tok = |name: &str| {
        tokens
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("no `{name}` token"))
    };
    let a2 = tok("accent_2");
    assert_eq!(
        a2["light"]["hex"].as_str().unwrap().to_uppercase(),
        "#0E7C70"
    );
    assert_eq!(a2["light"]["xterm"], 23);
    assert_eq!(
        a2["dark"]["hex"].as_str().unwrap().to_uppercase(),
        "#12A594"
    );
    assert_eq!(a2["dark"]["xterm"], 36);
    for t in tokens {
        let name = t["name"].as_str().unwrap();
        if name == "border" {
            continue;
        }
        let hex = t["light"]["hex"].as_str().unwrap();
        let c = contrast_on_white(hex_rgb(hex));
        assert!(c >= 4.5, "AC-1025: light {name} {hex} is {c:.2}:1 on white");
        let x = t["light"]["xterm"].as_u64().unwrap();
        if x >= 16 {
            let c = contrast_on_white(xterm_rgb(x));
            assert!(
                c >= 4.5,
                "AC-1025: light {name} xterm {x} is {c:.2}:1 on white"
            );
        }
    }
}

/// (env, the accent_2 sequence that must appear, the other theme's that must not).
type ColourCase = (
    &'static [(&'static str, &'static str)],
    &'static str,
    &'static str,
);

#[test]
fn ac1025_the_assistant_role_is_painted_with_the_theme_accent_2() {
    let bx = OmpBox::new();
    let cases: [ColourCase; 4] = [
        (
            &[("GLEN_THEME", "light"), ("COLORTERM", "truecolor")],
            "38;2;14;124;112m",
            "38;2;18;165;148m",
        ),
        (
            &[("GLEN_THEME", "light"), ("TERM", "xterm-256color")],
            "38;5;23m",
            "38;5;36m",
        ),
        (
            &[("GLEN_THEME", "dark"), ("COLORTERM", "truecolor")],
            "38;2;18;165;148m",
            "38;2;14;124;112m",
        ),
        (
            &[("GLEN_THEME", "dark"), ("TERM", "xterm-256color")],
            "38;5;36m",
            "38;5;23m",
        ),
    ];
    for (env, want, not) in cases {
        let mut run = bx.glen(["show", A.handle, "--human", "--color", "always"]);
        for (k, v) in env {
            run = run.env(k, v);
        }
        let out = run.run();
        out.assert_exit(0);
        let text = out.out();
        assert!(
            text.contains(want),
            "AC-1025 {env:?}: {want:?}\n{}",
            out.ctx()
        );
        assert!(
            !text.contains(not),
            "AC-1025 {env:?}: not {not:?}\n{}",
            out.ctx()
        );
    }
}

#[test]
fn ac1025_the_design_palette_table_mirrors_the_new_accent_2() {
    let design = text_of(&repo_root().join("docs/design.md"));
    let row = design
        .lines()
        .find(|l| l.starts_with("| accent_2"))
        .expect("docs/design.md has an accent_2 palette row");
    let cells: Vec<String> = row
        .split('|')
        .map(|c| c.trim().to_uppercase())
        .filter(|c| !c.is_empty())
        .collect();
    assert_eq!(
        cells.get(1).map(String::as_str),
        Some("#0E7C70"),
        "design.md §A10 says its table mirrors palette.json: light hex (AC-1025, Step 23): {row}"
    );
    assert_eq!(cells.get(2).map(String::as_str), Some("#12A594"), "{row}");
    assert_eq!(
        cells.get(3).map(String::as_str),
        Some("23 / 36"),
        "xterm L/D (AC-1025, Step 23): {row}"
    );
}

// ---------------------------------------------------------------------------------------------
// AC-1026 — README.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac1026_readme_documents_the_installers_and_the_headless_skill_permission() {
    let readme = text_of(&repo_root().join("README.md"));
    for cmd in ["glen install-extension", "glen install-skill"] {
        assert!(readme.contains(cmd), "AC-1026: README documents `{cmd}`");
    }
    let paragraph = readme.split("\n\n").find(|p| {
        p.contains("claude -p")
            && p.contains("--permission-mode dontAsk")
            && p.contains("--allowedTools \"Skill(glen)\"")
    });
    assert!(
        paragraph.is_some(),
        "AC-1026: README states that `claude -p --permission-mode dontAsk` needs \
         `--allowedTools \"Skill(glen)\"`"
    );
    assert!(
        !readme.contains("Coming in a later release"),
        "item 16: the W3/W4 README sections are filled"
    );
}

// ---------------------------------------------------------------------------------------------
// AC-1027 — the AC source check before Role 3.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac1027_the_orchestrator_checks_every_ac_against_the_source_before_role_3() {
    let dir = repo_root().join(".claude/agents/implementation");
    let orch = text_of(&dir.join("orchestrator.md"));
    let lines: Vec<&str> = orch.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.starts_with("### AC source check"))
        .expect("AC-1027: orchestrator.md has an `### AC source check` section");
    let next = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with("## ") || l.starts_with("### "))
        .map(|i| start + 1 + i)
        .expect("a heading follows the section");
    assert!(
        lines[next].starts_with("### Role 3"),
        "AC-1027: the check sits directly above Role 3, not {:?}",
        lines[next]
    );
    assert!(
        lines[..start].iter().all(|l| !l.starts_with("### Role 3")),
        "AC-1027: the check comes before Role 3"
    );
    assert!(
        next - start <= 31,
        "Step 21: at most 30 lines ({} found)",
        next - start - 1
    );
    let section = lines[start..next].join("\n");
    for needle in [
        "consistent",
        "omp-verified",
        "stale",
        "contradictory",
        "missing",
        "unverifiable",
        "v18.14.0",
        "rev-parse HEAD",
        "ac-check.md",
    ] {
        assert!(
            section.contains(needle),
            "AC-1027: the section names `{needle}`:\n{section}"
        );
    }
    let writer = text_of(&dir.join("conformance-test-writer.md"));
    let rule = writer
        .lines()
        .find(|l| l.contains("ac-check.md"))
        .expect("AC-1027: the conformance writer is told about *.ac-check.md");
    let lower = rule.to_lowercase();
    assert!(
        lower.contains("never") || lower.contains("not"),
        "AC-1027: the writer must not read the AC check file: {rule}"
    );
}

// ---------------------------------------------------------------------------------------------
// AC-1028 — macOS keeps its flock leases unchanged under Codex markers.
// ---------------------------------------------------------------------------------------------

#[cfg(target_os = "macos")]
#[test]
fn ac1028_on_macos_codex_markers_leave_lease_states_alone() {
    let bx = OmpBox::new();
    let markers: [&[(&str, &str)]; 3] = [
        &[("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
        &[("CODEX_SANDBOX", "seatbelt")],
        &[
            ("CODEX_SANDBOX", "seatbelt"),
            ("CODEX_SANDBOX_NETWORK_DISABLED", "1"),
        ],
    ];
    let state = |vars: &[(&str, &str)]| {
        let mut run = bx.glen(["ls", "--json"]);
        for (k, v) in vars {
            run = run.env(k, v);
        }
        let out = run.run();
        let d = out.ok_data();
        row(&d, A.id)["state"].as_str().unwrap().to_owned()
    };
    for vars in markers {
        assert_eq!(state(vars), "idle", "{vars:?}");
    }
    let _lease = hold_lease(&bx.owners(), A.id);
    for vars in markers {
        assert_eq!(
            state(vars),
            "live",
            "AC-1028: macOS is unchanged ({vars:?})"
        );
    }
}
