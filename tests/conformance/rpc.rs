//! W2 RPC driver: `glen send`, `glen wait` and the detached worker, driven through the
//! `fake_omp` peer (AC-201…AC-219, AC-222, AC-224…AC-227; docs/design.md §A3–§A11, §A15).

use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rustix::process::Signal;
use serde_json::{Value, json};

use crate::reader::*;
use crate::rpc_support::*;
use crate::support::*;

fn listed_commands(help: &str) -> Vec<String> {
    let mut names: Vec<String> = help
        .lines()
        .skip_while(|l| l.trim() != "Commands:")
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .filter(|l| l.starts_with(' '))
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    names.sort();
    names
}

/// `id` with its last four hex digits replaced (the fake's fork/move identities).
fn retail(id: &str, last4: &str) -> String {
    format!("{}{last4}", &id[..id.len() - 4])
}

// ---------------------------------------------------------------------------------------------
// Harness positive control: the process probes behind `assert_quiet` see live processes, so a
// passing "nothing left running" check is never vacuous.
// ---------------------------------------------------------------------------------------------

#[test]
fn harness_process_probes_see_a_live_process_group() {
    use std::os::unix::process::CommandExt;
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id() as i64;
    assert!(pid_alive(pid) && group_alive(pid));
    signal_group(pid, Signal::KILL);
    child.wait().unwrap();
    assert!(!pid_alive(pid) && !group_alive(pid));
}

// ---------------------------------------------------------------------------------------------
// AC-201 — the command surface.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac201_help_lists_the_w2_commands_and_hides_the_rest() {
    let sb = Sandbox::new();
    let want = [
        "completions",
        "doctor",
        "help",
        "inbox",
        "install-extension",
        "install-skill",
        "jobs",
        "ls",
        "send",
        "show",
        "tail",
        "wait",
    ];
    for args in [["--help"], ["-h"], ["help"]] {
        let out = sb.glen(args).run();
        out.assert_exit(0);
        assert_eq!(listed_commands(&out.out()), want, "{}", out.ctx());
        assert!(!out.out().contains("__worker"), "{}", out.ctx());
    }
    for name in HIDDEN {
        let out = sb.glen([name]).run();
        let err = out.error_envelope(if name == "watch" {
            "usage"
        } else {
            "not_implemented"
        });
        assert!(
            err["message"].as_str().unwrap().contains(name),
            "{}",
            out.ctx()
        );
    }
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let out = sb.glen(["completions", shell]).run();
        out.assert_exit(0);
        assert!(
            !out.out().contains("__worker"),
            "{shell} script exposes __worker"
        );
        for name in ["send", "wait", "jobs"] {
            assert!(out.out().contains(name), "{shell} script lacks {name}");
        }
        let data = sb.glen(["completions", shell, "--json"]).run().ok_data();
        assert!(!data["script"].as_str().unwrap().contains("__worker"));
    }
    assert!(
        !sb.glen_home.exists(),
        "help and completions create nothing"
    );
}

// ---------------------------------------------------------------------------------------------
// AC-202 — job creation and the omp command line.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac202_send_creates_a_private_uuidv7_job_and_resumes_the_session_over_rpc() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hello", "--wait", "20s", "--json"])
        .run();
    let d = data_of(&out, 0);
    let id = job_id(&d);
    assert_uuid_v7(&id);
    assert_eq!(
        rb.job_ids(),
        std::slice::from_ref(&id),
        "one job directory per send"
    );
    rb.assert_quiet();

    let dir = rb.job_dir(&id);
    assert_eq!(mode(&dir), 0o700);
    for f in ["job.json", "status.json", "worker.log"] {
        assert_eq!(mode(&dir.join(f)), 0o600, "{f}");
    }
    assert_private_tree(&rb.bx.glen_home);

    // §A7 job.json: the frozen request.
    let job = rb.job_json(&id);
    assert_eq!(job["v"], 1, "{job}");
    assert_eq!(job["id"], id.as_str(), "{job}");
    assert_eq!(job["handle"], job_handle(&id), "{job}");
    assert_eq!(job["route"], "rpc", "{job}");
    assert_eq!(job["session_id"], A.id, "{job}");
    assert_eq!(job["deliver_as"], "auto", "{job}");
    assert_eq!(job["text"], "hello", "{job}");
    for k in ["omp_bin", "owners_dir", "cwd"] {
        assert!(
            job[k].as_str().is_some_and(|p| Path::new(p).is_absolute()),
            "job.{k} is an absolute path: {job}"
        );
    }
    assert_eq!(job["job_timeout_ms"], 30_000, "GLEN_JOB_TIMEOUT=30s: {job}");
    let created = utc(&job["created_at"]);
    let deadline = utc(&job["deadline_at"]);
    assert_eq!(
        deadline.duration_since(created).as_millis(),
        30_000,
        "deadline = creation + timeout: {job}"
    );

    // The omp child: `--mode rpc --no-ui --resume <abs file>`, GLEN_MANAGED=1, and glen's cwd
    // because fixture A's header cwd (/home/fixture/work/alpha) does not exist.
    let starts = rb.starts();
    assert_eq!(starts.len(), 1, "{}", rb.diag());
    assert_omp_argv(&starts[0]["argv"], Some(&rb.bx.path(&A)), None);
    assert_eq!(starts[0]["env"]["GLEN_MANAGED"], "1", "{}", starts[0]);
    let cwd = starts[0]["cwd"].as_str().unwrap();
    assert!(same_file(Path::new(cwd), &rb.bx.alpha), "omp cwd {cwd}");
}

#[test]
fn ac202_omp_runs_in_the_session_header_cwd_when_it_exists() {
    let rb = RpcBox::new();
    let proj = rb.bx.work("gamma");
    let id = uuid(0xabc_0202);
    let path = rb.bx.add(
        "-work-gamma",
        "2026-10-06T10:00:00.000Z",
        &id,
        Some(&s(&proj)),
        &[user("aa000001", None, "2026-10-06T10:00:01.000Z", "hi")],
    );
    let out = rb
        .glen(["send", &tail7(&id), "hello", "--wait", "20s", "--json"])
        .run();
    let d = data_of(&out, 0);
    assert_eq!(d["session"]["id"], id.as_str(), "{d}");
    let st = &rb.starts()[0];
    assert_omp_argv(&st["argv"], Some(&path), None);
    assert!(
        same_file(Path::new(st["cwd"].as_str().unwrap()), &proj),
        "omp cwd {} is not the header cwd",
        st["cwd"]
    );
    rb.assert_quiet();
}

#[test]
fn ac202_send_follows_continuations_paths_and_latest() {
    let rb = RpcBox::new();
    // B was continued in C (fixtures README): the session to drive is C.
    let log = rb.log_path("b");
    let d = data_of(
        &rb.glen(["send", B.handle, "hi", "--wait", "20s", "--json"])
            .env("FAKE_OMP_LOG", &log)
            .run(),
        0,
    );
    assert_omp_argv(
        &starts(&records(&log))[0]["argv"],
        Some(&rb.bx.path(&C)),
        None,
    );
    assert_eq!(
        d["session"]["id"], C.id,
        "the resolved session after continuations: {d}"
    );

    // A path argument names that file.
    let log = rb.log_path("path");
    data_of(
        &rb.glen(["send", &s(&rb.bx.path(&A)), "hi", "--wait", "20s", "--json"])
            .env("FAKE_OMP_LOG", &log)
            .run(),
        0,
    );
    assert_omp_argv(
        &starts(&records(&log))[0]["argv"],
        Some(&rb.bx.path(&A)),
        None,
    );

    // @latest = newest session (by mtime) of the cwd bucket: G in -work-alpha.
    let log = rb.log_path("latest");
    let d = data_of(
        &rb.glen(["send", "@latest", "hi", "--wait", "20s", "--json"])
            .env("FAKE_OMP_LOG", &log)
            .run(),
        0,
    );
    assert_omp_argv(
        &starts(&records(&log))[0]["argv"],
        Some(&rb.bx.path(&G)),
        None,
    );
    assert_eq!(d["session"]["id"], G.id, "{d}");
    rb.assert_quiet();
}

#[test]
fn ac202_the_worker_leads_its_own_session_with_null_stdin_and_a_private_log() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hello", "--no-wait", "--json"])
        .env("FAKE_OMP_DELAY_MS", "3000")
        .run();
    let id = job_id(&data_of(&out, 10));
    let st = rb.wait_status(&id, "accepted with a worker identity", LONG, |s| {
        s["status"] == "accepted" && s["worker_pid"].is_u64()
    });
    assert!(
        st["worker_start"].as_str().is_some_and(|t| !t.is_empty()),
        "AC-214: worker_start recorded: {st}"
    );
    let pid = st["worker_pid"].as_i64().unwrap();
    let sid = rustix::process::getsid(rustix::process::Pid::from_raw(pid as i32))
        .expect("getsid of the live worker");
    assert_eq!(
        sid.as_raw_nonzero().get() as i64,
        pid,
        "the worker runs in a new session (setsid)"
    );
    let log = rb.job_dir(&id).join("worker.log");
    match fd_target(pid, 0) {
        Some(p) => assert_eq!(p, "/dev/null", "worker stdin"),
        None => eprintln!("note: cannot inspect fds of {pid}; stdin check skipped"),
    }
    for fd in [1, 2] {
        if let Some(p) = fd_target(pid, fd) {
            assert!(same_file(Path::new(&p), &log), "worker fd {fd} -> {p}");
        }
    }
    data_of(
        &rb.glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
            .run(),
        0,
    );
    rb.assert_quiet();
}

#[test]
fn ac202_an_explicit_profile_is_passed_to_omp_and_the_profile_env_is_dropped() {
    let rb = RpcBox::new();
    let out = rb
        .glen([
            "--profile",
            "work",
            "send",
            "@new",
            "hi",
            "--wait",
            "20s",
            "--json",
        ])
        .env("OMP_PROFILE", "other")
        .env("PI_PROFILE", "other")
        .run();
    data_of(&out, 0);
    let st = &rb.starts()[0];
    assert_omp_argv(&st["argv"], None, Some("work"));
    assert!(st["env"]["OMP_PROFILE"].is_null(), "{st}");
    assert!(st["env"]["PI_PROFILE"].is_null(), "{st}");

    // Without the flag, the inherited profile env reaches omp unchanged and no flag is added.
    let log = rb.log_path("inherit");
    let out = rb
        .glen(["send", "@new", "hi", "--wait", "20s", "--json"])
        .env("OMP_PROFILE", "work")
        .env("PI_PROFILE", "other")
        .env("FAKE_OMP_LOG", &log)
        .run();
    data_of(&out, 0);
    let st = &starts(&records(&log))[0];
    assert_omp_argv(&st["argv"], None, None);
    assert_eq!(st["env"]["OMP_PROFILE"], "work", "{st}");
    assert_eq!(st["env"]["PI_PROFILE"], "other", "{st}");
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-203 — completion.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac203_send_completes_with_the_reply_document() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hello", "--wait", "20s", "--json"])
        .run();
    let d = data_of(&out, 0);
    for k in COMPLETED_KEYS {
        assert!(d.get(k).is_some(), "completed data lacks `{k}`: {d}");
    }
    let id = job_id(&d);
    assert_eq!(d["status"], "completed", "{d}");
    assert_job_view(&d["job"]);
    let job = &d["job"];
    assert_eq!(job["status"], "completed", "{d}");
    assert_eq!(job["route"], "rpc", "{d}");
    assert_eq!(job["deliver_as"], "auto", "{d}");
    assert_eq!(job["session_id"], A.id, "{d}");
    assert_eq!(job["session_handle"], A.handle, "{d}");
    assert_eq!(d["session"]["id"], A.id, "{d}");
    assert_eq!(d["session"]["handle"], A.handle, "{d}");
    assert_path(&d["session"]["path"], &rb.bx.path(&A));
    assert_eq!(d["assistant_text"], "pong: hello", "{d}");
    let replies = arr(&d["replies"]);
    assert_eq!(replies.len(), 1, "{d}");
    assert_reply(&replies[0], A.id, &id, "call_1", "reply: hello", true);
    assert_eq!(d["command_output"], json!([]), "{d}");
    assert_eq!(d["stop_reason"], "stop", "{d}");
    assert_eq!(d["session_settled"], true, "{d}");
    assert_eq!(d["delivered_as"], "prompt", "{d}");
    assert!(d["forked"].is_null(), "{d}");

    // The worker's authoritative files agree.
    let st = rb.status(&id);
    assert_eq!(st["status"], "completed", "{st}");
    utc(&st["updated_at"]);
    let res = rb.result(&id);
    assert_eq!(res["v"], 1, "{res}");
    assert_eq!(res["assistant_text"], d["assistant_text"], "{res}");
    assert_eq!(res["replies"], d["replies"], "{res}");
    assert_eq!(res["session_settled"], true, "{res}");
    rb.assert_quiet();
}

#[test]
fn ac203_an_unsettled_result_completes_only_after_session_settled() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hello", "--no-wait", "--json"])
        .env("FAKE_OMP_SCENARIO", "unsettled")
        .env("FAKE_OMP_SETTLE_DELAY_MS", "1500")
        .run();
    let id = job_id(&data_of(&out, 10));
    let mut saw_window = false;
    poll_until("the job to finish", LONG, || {
        // Read the status first, then the log: the fake logs a frame before writing it, so a
        // terminal status seen before `session_settled` is in the log was written too early.
        let st = rb.try_status(&id);
        let recs = rb.recs();
        let settled_sent = recs.iter().any(|r| is_frame(r, "out", "session_settled"));
        let result_sent = recs.iter().any(|r| is_frame(r, "out", "prompt_result"));
        saw_window |= result_sent && !settled_sent;
        let st = st.filter(is_terminal)?;
        assert!(
            settled_sent,
            "job became {} before omp sent session_settled (prompt_result said sessionSettled:false)",
            st["status"]
        );
        assert!(
            rb.job_dir(&id).join("result.json").is_file(),
            "§A7: result.json is published before the terminal status"
        );
        Some(())
    });
    assert!(saw_window, "the poll never observed the unsettled window");
    let w = data_of(
        &rb.glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
            .run(),
        0,
    );
    assert_eq!(w["status"], "completed", "{w}");
    assert_eq!(w["session_settled"], true, "{w}");
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-204, AC-205 — bounded waits and `glen wait`.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac204_a_bounded_wait_exits_10_and_the_worker_keeps_running() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hello", "--wait", "300ms", "--json"])
        .env("FAKE_OMP_DELAY_MS", "3000")
        .run();
    let d = data_of(&out, 10);
    let id = job_id(&d);
    let h = job_handle(&id);
    assert_eq!(d["status"], "running", "{d}");
    assert_job_view(&d["job"]);
    assert_eq!(d["job"]["handle"], h.as_str(), "{d}");
    assert!(
        ["queued", "accepted"].contains(&d["job"]["status"].as_str().unwrap()),
        "{d}"
    );
    assert_eq!(d["next"], format!("glen wait {h}"), "{d}");
    assert_eq!(d["interrupted"], false, "{d}");
    assert!(d.get("session").is_some(), "{d}");

    let w = data_of(
        &rb.glen(["wait", &h, "--timeout", "200ms", "--json"]).run(),
        10,
    );
    assert_eq!(w["status"], "running", "{w}");
    assert_eq!(w["next"], format!("glen wait {h}"), "{w}");

    let w = data_of(
        &rb.glen(["wait", &h, "--timeout", "20s", "--json"]).run(),
        0,
    );
    assert_eq!(w["assistant_text"], "pong: hello", "{w}");
    rb.assert_quiet();
}

#[test]
fn ac204_no_wait_and_wait_0_return_within_two_seconds() {
    let rb = RpcBox::new();
    for flags in [vec!["--no-wait"], vec!["--wait", "0"]] {
        let mut args = vec!["send", A.handle, "hi"];
        args.extend(flags.iter().copied());
        args.push("--json");
        let out = rb.glen(&args).env("FAKE_OMP_DELAY_MS", "1500").run();
        let d = data_of(&out, 10);
        assert!(
            out.elapsed < Duration::from_secs(2),
            "{flags:?} took {:?}",
            out.elapsed
        );
        assert_eq!(d["status"], "running", "{d}");
        let w = data_of(
            &rb.glen([
                "wait",
                &job_handle(&job_id(&d)),
                "--timeout",
                "20s",
                "--json",
            ])
            .run(),
            0,
        );
        assert_eq!(w["assistant_text"], "pong: hi", "{w}");
    }
    rb.assert_quiet();
}

#[test]
fn ac205_wait_on_a_terminal_job_reports_at_once_with_the_send_mapping() {
    let rb = RpcBox::new();
    let d = data_of(
        &rb.glen(["send", A.handle, "hello", "--wait", "20s", "--json"])
            .run(),
        0,
    );
    let id = job_id(&d);
    rb.assert_quiet();
    let out = rb
        .glen(["wait", &job_handle(&id), "--timeout", "60s", "--json"])
        .run();
    let w = data_of(&out, 0);
    assert!(
        out.elapsed < Duration::from_secs(5),
        "took {:?}",
        out.elapsed
    );
    for k in COMPLETED_KEYS {
        assert!(w.get(k).is_some(), "wait data lacks `{k}`: {w}");
    }
    assert_eq!(w["job"]["id"], id.as_str(), "{w}");
    assert_eq!(w["assistant_text"], d["assistant_text"], "{w}");
    assert_eq!(w["replies"], d["replies"], "{w}");

    let out = rb
        .glen(["send", A.handle, "hello", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "error")
        .run();
    let err = failed(&out, "job_failed");
    assert_job_failed(&err, "failed", "omp_error");
    rb.assert_quiet();
    let fid = err["details"]["job"]["id"].as_str().unwrap().to_owned();
    let out = rb
        .glen(["wait", &job_handle(&fid), "--timeout", "60s", "--json"])
        .run();
    let err = failed(&out, "job_failed");
    assert!(
        out.elapsed < Duration::from_secs(5),
        "took {:?}",
        out.elapsed
    );
    assert_job_failed(&err, "failed", "omp_error");
    assert!(err["details"]["forked"].is_null(), "{err}");
}

// ---------------------------------------------------------------------------------------------
// AC-206 — fork detection.
// ---------------------------------------------------------------------------------------------

fn fork_case(scenario: &str) {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hello", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", scenario)
        .run();
    let d = data_of(&out, 11);
    let to = retail(A.id, "f0f0");
    assert_eq!(d["status"], "completed", "{d}");
    assert_eq!(d["forked"]["from"], A.id, "{d}");
    assert_eq!(d["forked"]["to"], to.as_str(), "{d}");
    assert_eq!(d["forked"]["to_handle"], tail7(&to), "{d}");
    let id = job_id(&d);
    let st = rb.wait_terminal(&id);
    assert_eq!(st["forked"]["from"], A.id, "{st}");
    assert_eq!(st["forked"]["to"], to.as_str(), "{st}");

    let w = data_of(&rb.glen(["wait", &job_handle(&id), "--json"]).run(), 11);
    assert_eq!(w["forked"]["to"], to.as_str(), "{w}");
    assert_eq!(w["forked"]["to_handle"], tail7(&to), "{w}");

    if scenario == "fork" {
        let recs = rb.recs();
        let notice = index_of(&recs, |r| {
            is_frame(r, "out", "notice") && r["frame"]["source"] == "session-persistence"
        })
        .expect("the fake sent a persistence notice");
        assert!(
            recs[notice..]
                .iter()
                .any(|r| is_frame(r, "in", "get_state")),
            "a persistence notice is followed by a get_state re-read\n{}",
            rb.diag()
        );
    }
    rb.assert_quiet();
}

#[test]
fn ac206_a_persistence_notice_fork_exits_11_with_both_ids() {
    fork_case("fork");
}

#[test]
fn ac206_a_silent_fork_is_caught_before_finishing() {
    fork_case("fork_silent");
}

// ---------------------------------------------------------------------------------------------
// AC-207 — no silent fork: owner-lease refusals.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac207_a_live_owner_is_refused_before_any_job_or_omp_exists() {
    let rb = RpcBox::new();
    let _lease = hold_lease(&rb.bx.owners(), A.id);
    let out = rb.glen(["send", A.handle, "hi", "--json"]).run();
    let err = failed(&out, "no_route");
    assert!(
        err["hint"].as_str().is_some_and(|h| !h.trim().is_empty()),
        "§A4 W2 hint: {err}"
    );
    assert!(!rb.bx.glen_home.exists(), "GLEN_HOME was created");
    assert!(rb.starts().is_empty(), "omp was started\n{}", rb.diag());
}

/// §A3 (AC-207 revised): with the glen lock free, `send` probes the owner lease again after it has
/// read stdin, so an omp that opened the session meanwhile is still refused before any state.
#[test]
fn ac207_send_reprobes_the_owner_lease_after_reading_stdin() {
    let rb = RpcBox::new();
    let mut cmd = rb.command(&["send", A.handle, "-", "--json"]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    // `send` resolves (and probes) the target before it blocks on stdin; give it time to get
    // there. A slower start only makes this row less discriminating, never wrongly red.
    std::thread::sleep(Duration::from_millis(500));
    let _lease = hold_lease(&rb.bx.owners(), A.id);
    stdin.write_all(b"hi").unwrap();
    drop(stdin);
    let out = finish(child, "send - (owner appears while stdin is read)", LONG);
    failed(&out, "no_route");
    assert!(!rb.bx.glen_home.exists(), "GLEN_HOME was created");
    assert!(rb.starts().is_empty(), "omp was started\n{}", rb.diag());
}

#[test]
fn ac207_the_worker_probes_again_before_spawning_omp() {
    let rb = RpcBox::new();
    let lock = rb.hold_glen_lock(A.id);
    let d = data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .run(),
        10,
    );
    assert_eq!(d["job"]["status"], "queued", "{d}");
    let id = job_id(&d);
    let _lease = hold_lease(&rb.bx.owners(), A.id);
    drop(lock);
    let out = rb
        .glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
        .run();
    failed(&out, "no_route");
    let st = rb.wait_terminal(&id);
    assert_eq!(st["status"], "failed", "{st}");
    assert_eq!(st["error"]["code"], "no_route", "{st}");
    rb.assert_quiet();
    assert!(rb.starts().is_empty(), "omp was spawned\n{}", rb.diag());
}

/// §A3 (AC-207 revised): there is no pre-prompt probe. omp's startup recovery may write to the
/// session and claim the owner lease before `ready`; that lease is glen's own child's, so the job
/// runs. (Supersedes the original plan's "probe right before prompting" row.)
#[test]
fn ac207_a_lease_claimed_by_omps_own_startup_recovery_does_not_refuse_the_job() {
    let rb = RpcBox::new();
    let release = rb.bx.root.join("run-release");
    let d = data_of(
        &rb.glen(["send", A.handle, "hello", "--no-wait", "--json"])
            .env("FAKE_OMP_STARTUP_RECOVERY", "1")
            .env("FAKE_OMP_OWNERS_DIR", s(&rb.bx.owners()))
            .env("FAKE_OMP_RUN_RELEASE", &release)
            .run(),
        10,
    );
    let id = job_id(&d);
    let recs = rb.wait_log("the prompt", |r| {
        r.iter().any(|x| is_frame(x, "in", "prompt"))
    });
    let recovered = index_of(&recs, |r| r["event"] == "startup_recovery")
        .expect("the fake claimed the lease and wrote a recovery record during startup");
    let ready = index_of(&recs, |r| is_frame(r, "out", "ready")).expect("ready");
    let prompt = index_of(&recs, |r| is_frame(r, "in", "prompt")).unwrap();
    assert!(recovered < ready && ready < prompt, "{}", rb.diag());
    // The prompt went out while glen's own omp child held the owner lease (W1 probe via ls).
    let ls = data_of(&rb.glen(["ls", "--json"]).run(), 0);
    let state = row(&ls, A.id)["state"].as_str().unwrap_or("").to_owned();
    assert!(
        ["live", "busy"].contains(&state.as_str()),
        "omp's own lease is held during the run: state {state}"
    );
    fs::write(&release, "").unwrap();
    let w = data_of(
        &rb.glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
            .run(),
        0,
    );
    assert_eq!(w["status"], "completed", "{w}");
    assert_eq!(w["assistant_text"], "pong: hello", "{w}");
    assert!(w["forked"].is_null(), "{w}");
    rb.assert_quiet();
}

/// §A7 queue display: `waiting_for_lock` is published once the worker has seen the glen lock
/// held, cleared when it gets the lock, and `waited_ms` keeps the wait.
#[test]
fn a7_a_queued_job_publishes_waiting_for_lock_until_it_gets_the_lock() {
    let rb = RpcBox::new();
    let lock = rb.hold_glen_lock(A.id);
    let id = job_id(&data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .run(),
        10,
    ));
    let st = rb.wait_status(&id, "waiting_for_lock", LONG, |s| {
        s["waiting_for_lock"] == true
    });
    assert_eq!(st["status"], "queued", "{st}");
    std::thread::sleep(Duration::from_millis(300));
    drop(lock);
    data_of(
        &rb.glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
            .run(),
        0,
    );
    let st = rb.status(&id);
    assert_ne!(st["waiting_for_lock"], true, "cleared on acquisition: {st}");
    assert!(
        st["waited_ms"].as_u64().is_some_and(|ms| ms >= 300),
        "waited_ms keeps the lock wait: {st}"
    );
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-208 — one omp child per session; glen's own lease does not refuse.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac208_a_send_behind_a_running_glen_job_queues_and_runs_after_it() {
    let rb = RpcBox::new();
    let owners = s(&rb.bx.owners());
    let run = |text: &str| {
        rb.glen(["send", A.handle, text, "--no-wait", "--json"])
            .env("FAKE_OMP_OWNERS_DIR", &owners)
            .env("FAKE_OMP_DELAY_MS", "2000")
            .run()
    };
    let first = job_id(&data_of(&run("one"), 10));
    rb.wait_status(&first, "accepted", LONG, |s| s["status"] == "accepted");
    // The first omp child holds the owner lease from its prompt on, like omp does.
    rb.wait_log("the first prompt", |r| {
        r.iter().any(|x| is_frame(x, "in", "prompt"))
    });
    let d2 = data_of(&run("two"), 10);
    assert_eq!(d2["job"]["status"], "queued", "{d2}");
    let second = job_id(&d2);

    let w1 = data_of(
        &rb.glen(["wait", &job_handle(&first), "--timeout", "30s", "--json"])
            .run(),
        0,
    );
    assert_eq!(w1["assistant_text"], "pong: one", "{w1}");
    let w2 = data_of(
        &rb.glen(["wait", &job_handle(&second), "--timeout", "30s", "--json"])
            .run(),
        0,
    );
    assert_eq!(w2["assistant_text"], "pong: two", "{w2}");
    rb.assert_quiet();

    let recs = rb.recs();
    let at: Vec<usize> = recs
        .iter()
        .enumerate()
        .filter(|(_, r)| r["event"] == "start")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(at.len(), 2, "{}", rb.diag());
    assert!(
        recs[at[0]..at[1]].iter().any(|r| r["event"] == "exit"),
        "the second omp child started before the first one exited\n{}",
        rb.diag()
    );
}

// ---------------------------------------------------------------------------------------------
// AC-209 — @new.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac209_new_runs_omp_without_resume_and_locks_the_revealed_id() {
    let rb = RpcBox::new();
    let proj = rb.bx.work("fresh");
    let d = data_of(
        &rb.glen([
            "send",
            "@new",
            "hello",
            "--cwd",
            &s(&proj),
            "--no-wait",
            "--json",
        ])
        .env("FAKE_OMP_DELAY_MS", "1500")
        .run(),
        10,
    );
    let id = job_id(&d);
    assert_eq!(d["job"]["route"], "new", "{d}");
    let recs = rb.wait_log("the prompt", |r| {
        r.iter().any(|x| is_frame(x, "in", "prompt"))
    });
    let st = &starts(&recs)[0];
    assert_omp_argv(&st["argv"], None, None);
    assert!(
        same_file(Path::new(st["cwd"].as_str().unwrap()), &proj),
        "omp cwd {}",
        st["cwd"]
    );
    let new_id = reported_session_ids(&recs)
        .first()
        .cloned()
        .expect("omp reported a session id");
    assert!(
        !rb.glen_lock_free(&new_id),
        "run/locks/<new id>.lock is held once the prompt is out"
    );

    let w = data_of(
        &rb.glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
            .run(),
        0,
    );
    assert_eq!(rb.job_json(&id)["route"], "new");
    assert_eq!(rb.status(&id)["actual_session_id"], new_id.as_str());
    assert_eq!(w["job"]["actual_session_id"], new_id.as_str(), "{w}");
    assert_eq!(w["session"]["id"], new_id.as_str(), "{w}");
    assert_eq!(w["session"]["handle"], tail7(&new_id), "{w}");
    rb.assert_quiet();
    assert!(rb.glen_lock_free(&new_id), "the lock is released at exit");
    assert_private_tree(&rb.bx.glen_home);
}

#[test]
fn ac209_new_defaults_to_glens_cwd_and_makes_a_relative_cwd_absolute() {
    let rb = RpcBox::new();
    data_of(
        &rb.glen(["send", "@new", "hi", "--wait", "20s", "--json"])
            .run(),
        0,
    );
    let cwd = rb.starts()[0]["cwd"].as_str().unwrap().to_owned();
    assert!(same_file(Path::new(&cwd), &rb.bx.alpha), "omp cwd {cwd}");

    let log = rb.log_path("relative");
    data_of(
        &rb.glen([
            "send", "@new", "hi", "--cwd", "../beta", "--wait", "20s", "--json",
        ])
        .env("FAKE_OMP_LOG", &log)
        .run(),
        0,
    );
    let cwd = starts(&records(&log))[0]["cwd"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(same_file(Path::new(&cwd), &rb.bx.beta), "omp cwd {cwd}");
    rb.assert_quiet();
}

#[test]
fn ac209_cwd_applies_only_to_new_and_latest_and_must_be_a_directory() {
    let rb = RpcBox::new();
    failed(
        &rb.glen(["send", A.handle, "hi", "--cwd", "/tmp", "--json"])
            .run(),
        "usage",
    );
    let missing = rb.bx.root.join("nope");
    failed(
        &rb.glen(["send", "@new", "hi", "--cwd", &s(&missing), "--json"])
            .run(),
        "not_found",
    );
    let file = rb.bx.root.join("a-file");
    fs::write(&file, "x").unwrap();
    failed(
        &rb.glen(["send", "@new", "hi", "--cwd", &s(&file), "--json"])
            .run(),
        "not_found",
    );
    assert!(!rb.bx.glen_home.exists(), "no job for a refused --cwd");
    assert!(rb.starts().is_empty());

    // @latest --cwd DIR picks the newest session of DIR's bucket: E in -work-beta.
    let d = data_of(
        &rb.glen([
            "send",
            "@latest",
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
    assert_eq!(d["session"]["id"], E.id, "{d}");
    assert_omp_argv(&rb.starts()[0]["argv"], Some(&rb.bx.path(&E)), None);
    let cwd = rb.starts()[0]["cwd"].as_str().unwrap().to_owned();
    assert!(
        same_file(Path::new(&cwd), &rb.bx.beta),
        "§A7: E's header cwd does not exist, so omp runs in --cwd DIR, not {cwd}"
    );
    rb.assert_quiet();
}

/// §A7 worker cwd: an existing header cwd wins over `--cwd DIR`.
#[test]
fn a7_an_existing_header_cwd_wins_over_the_cwd_flag() {
    let rb = RpcBox::new();
    let gamma = rb.bx.work("gamma");
    let delta = rb.bx.work("delta");
    let id = uuid(0xa7_0207);
    let path = rb.bx.add(
        "-work-gamma",
        "2026-10-06T11:00:00.000Z",
        &id,
        Some(&s(&delta)),
        &[user("aa000002", None, "2026-10-06T11:00:01.000Z", "hi")],
    );
    let d = data_of(
        &rb.glen([
            "send",
            "@latest",
            "hi",
            "--cwd",
            &s(&gamma),
            "--wait",
            "20s",
            "--json",
        ])
        .run(),
        0,
    );
    assert_eq!(d["session"]["id"], id.as_str(), "{d}");
    let st = &rb.starts()[0];
    assert_omp_argv(&st["argv"], Some(&path), None);
    let cwd = st["cwd"].as_str().unwrap();
    assert!(
        same_file(Path::new(cwd), &delta),
        "omp cwd {cwd} is not the header cwd"
    );
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-210, AC-211 — the glen_reply host tool.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac210_glen_reply_is_the_one_host_tool_registered_before_the_prompt() {
    let rb = RpcBox::new();
    data_of(
        &rb.glen(["send", A.handle, "hello", "--wait", "20s", "--json"])
            .run(),
        0,
    );
    rb.assert_quiet();
    let recs = rb.recs();
    let sets = frames_in(&recs, "in", "set_host_tools");
    assert_eq!(sets.len(), 1, "{}", rb.diag());
    let tools = arr(&sets[0]["tools"]);
    assert_eq!(tools.len(), 1, "exactly one host tool: {}", sets[0]);
    let t = &tools[0];
    assert_eq!(t["name"], "glen_reply", "{t}");
    assert_eq!(t["description"], A5_DESCRIPTION, "{t}");
    let p = &t["parameters"];
    assert_eq!(p["type"], "object", "{t}");
    assert_eq!(p["properties"]["text"]["type"], "string", "{t}");
    assert_eq!(p["properties"]["final"]["type"], "boolean", "{t}");
    assert_eq!(p["required"], json!(["text"]), "{t}");
    let tools_at = index_of(&recs, |r| is_frame(r, "in", "set_host_tools")).unwrap();
    let prompt_at = index_of(&recs, |r| is_frame(r, "in", "prompt")).unwrap();
    assert!(
        tools_at < prompt_at,
        "set_host_tools comes before the prompt"
    );

    let results = frames_in(&recs, "in", "host_tool_result");
    assert_eq!(results.len(), 1, "{}", rb.diag());
    assert_eq!(results[0]["id"], "htc_1", "{}", results[0]);
    assert!(!is_tool_error(&results[0]), "{}", results[0]);
    assert_eq!(tool_text(&results[0]), "delivered", "{}", results[0]);
}

#[test]
fn ac210_replies_are_recorded_in_call_order_in_the_result_the_job_and_the_inbox() {
    let rb = RpcBox::new();
    let d = data_of(
        &rb.glen(["send", A.handle, "hello", "--wait", "20s", "--json"])
            .env("FAKE_OMP_SCENARIO", "progress")
            .run(),
        0,
    );
    let id = job_id(&d);
    rb.assert_quiet();
    let replies = arr(&d["replies"]);
    assert_eq!(replies.len(), 2, "{d}");
    assert_reply(&replies[0], A.id, &id, "call_1", "progress", false);
    assert_reply(&replies[1], A.id, &id, "call_2", "reply: hello", true);
    assert_eq!(rb.result(&id)["replies"], d["replies"], "result.json");
    let job_file = rb.job_dir(&id).join("replies.ndjson");
    assert_eq!(
        Value::Array(ndjson(&job_file)),
        d["replies"],
        "replies.ndjson"
    );
    let inbox_dir = rb.bx.glen_home.join("inbox");
    let inbox = inbox_dir.join(format!("{}.ndjson", A.id));
    assert_eq!(Value::Array(ndjson(&inbox)), d["replies"], "inbox");
    assert_eq!(mode(&job_file), 0o600);
    assert_eq!(mode(&inbox), 0o600);
    assert_eq!(mode(&inbox_dir), 0o700);
    assert_private_tree(&rb.bx.glen_home);
    let results = rb.inbound("host_tool_result");
    assert_eq!(results.len(), 2, "{}", rb.diag());
    for r in &results {
        assert!(!is_tool_error(r), "{r}");
        assert_eq!(tool_text(r), "delivered", "{r}");
    }
}

#[test]
fn ac210_invalid_reply_calls_are_tool_errors_and_record_nothing() {
    let rb = RpcBox::new();
    let d = data_of(
        &rb.glen(["send", A.handle, "hello", "--wait", "20s", "--json"])
            .env("FAKE_OMP_SCENARIO", "bad_reply")
            .run(),
        0,
    );
    let id = job_id(&d);
    rb.assert_quiet();
    assert_eq!(d["replies"], json!([]), "{d}");
    assert_eq!(rb.result(&id)["replies"], json!([]));
    assert!(ndjson(&rb.job_dir(&id).join("replies.ndjson")).is_empty());
    let inbox = rb
        .bx
        .glen_home
        .join("inbox")
        .join(format!("{}.ndjson", A.id));
    assert!(ndjson(&inbox).is_empty(), "nothing in the inbox");
    let results = rb.inbound("host_tool_result");
    assert_eq!(results.len(), 3, "{}", rb.diag());
    for r in &results {
        assert!(is_tool_error(r), "invalid call must be a tool error: {r}");
    }
}

#[test]
fn ac211_a_rejected_reply_tool_fails_the_job_without_a_prompt() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "reject_tools")
        .run();
    let err = failed(&out, "job_failed");
    assert_job_failed(&err, "failed", "reply_tool_rejected");
    rb.assert_quiet();
    assert!(rb.inbound("prompt").is_empty(), "{}", rb.diag());
}

// ---------------------------------------------------------------------------------------------
// AC-212, AC-213 — local prompts and omp failures.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac212_a_locally_handled_prompt_completes_without_result_or_settlement() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "/local", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "local")
        .run();
    let d = data_of(&out, 0);
    assert!(
        out.elapsed < Duration::from_secs(10),
        "took {:?}",
        out.elapsed
    );
    assert_eq!(d["status"], "completed", "{d}");
    assert!(d.get("assistant_text").is_some_and(Value::is_null), "{d}");
    assert_eq!(d["command_output"], json!(["fake command output"]), "{d}");
    rb.assert_quiet();
}

fn omp_failure(scenario: &str, status: &str, code: &str) {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", scenario)
        .run();
    let err = failed(&out, "job_failed");
    assert_job_failed(&err, status, code);
    let id = err["details"]["job"]["id"].as_str().unwrap().to_owned();
    let st = rb.wait_terminal(&id);
    assert_eq!(st["status"], status, "{st}");
    assert_eq!(st["error"]["code"], code, "{st}");
    if scenario == "error" {
        assert!(
            err["details"]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("fake provider error"),
            "the message comes from omp: {err}"
        );
    }
    rb.assert_quiet();
}

#[test]
fn ac213_a_prompt_error_is_failed_omp_error() {
    omp_failure("error", "failed", "omp_error");
}

#[test]
fn ac213_an_aborted_prompt_is_aborted() {
    omp_failure("aborted", "aborted", "aborted");
}

#[test]
fn ac213_omp_exiting_before_its_result_is_failed_omp_exited() {
    omp_failure("crash", "failed", "omp_exited");
}

// ---------------------------------------------------------------------------------------------
// AC-214 — lost workers.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac214_a_killed_worker_is_reported_lost_and_a_live_one_never_is() {
    let rb = RpcBox::new();
    let d = data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .env("FAKE_OMP_DELAY_MS", "8000")
            .run(),
        10,
    );
    let id = job_id(&d);
    let st = rb.wait_status(&id, "accepted", LONG, |s| {
        s["status"] == "accepted" && s["worker_pid"].is_u64()
    });
    assert!(
        st["worker_start"].as_str().is_some_and(|t| !t.is_empty()),
        "{st}"
    );
    for _ in 0..3 {
        let j = data_of(&rb.glen(["jobs", "--json"]).run(), 0);
        assert_eq!(j["jobs"][0]["id"], id.as_str(), "{j}");
        assert_eq!(
            j["jobs"][0]["status"], "accepted",
            "a live worker is never lost: {j}"
        );
        let w = data_of(
            &rb.glen(["wait", &job_handle(&id), "--timeout", "100ms", "--json"])
                .run(),
            10,
        );
        assert_eq!(w["job"]["status"], "accepted", "{w}");
    }
    let pid = st["worker_pid"].as_i64().unwrap();
    signal(pid, Signal::KILL);
    poll_until("the killed worker to disappear", LONG, || {
        (!pid_alive(pid)).then_some(())
    });
    let out = rb
        .glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
        .run();
    let err = failed(&out, "job_failed");
    assert_eq!(err["details"]["status"], "lost", "{err}");
    let st = rb.status(&id);
    assert_eq!(st["status"], "lost", "lost is persisted: {st}");
    // The orphaned omp child exits on stdin EOF.
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-215 — the job deadline.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac215_the_deadline_aborts_a_running_prompt_and_fails_with_timeout() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "hang")
        .env("GLEN_JOB_TIMEOUT", "2s")
        .run();
    let err = failed(&out, "job_failed");
    assert_job_failed(&err, "failed", "timeout");
    rb.assert_quiet();
    assert!(
        !rb.inbound("abort").is_empty(),
        "abort was sent\n{}",
        rb.diag()
    );
}

#[test]
fn ac215_an_omp_that_ignores_abort_is_killed_by_process_group() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "40s", "--json"])
        .env("FAKE_OMP_SCENARIO", "wedge")
        .env("GLEN_JOB_TIMEOUT", "2s")
        .run();
    let err = failed(&out, "job_failed");
    assert_job_failed(&err, "failed", "timeout");
    let st = &rb.starts()[0];
    let pgid = st["pgid"].as_i64().unwrap();
    let pid = st["pid"].as_i64().unwrap();
    poll_until(
        "the wedged omp group to be gone",
        Duration::from_secs(5),
        || (!pid_alive(pid) && (pgid == own_pgid() || !group_alive(pgid))).then_some(()),
    );
    assert!(
        !rb.inbound("abort").is_empty(),
        "abort before the kill\n{}",
        rb.diag()
    );
    rb.assert_quiet();
}

#[test]
fn ac215_a_completed_but_unsettled_run_completes_at_the_deadline_with_a_warning() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "never_settles")
        .env("GLEN_JOB_TIMEOUT", "2s")
        .run();
    let env = ok_doc(&out, 0);
    let d = &env["data"];
    assert_eq!(d["status"], "completed", "{d}");
    assert_eq!(d["session_settled"], false, "{d}");
    assert!(
        warning_codes(&env).iter().any(|c| c == "settle_timeout"),
        "warning settle_timeout: {env}"
    );
    rb.assert_quiet();
}

#[test]
fn ac215_a_job_still_queued_at_its_deadline_fails_with_timeout() {
    let rb = RpcBox::new();
    let lock = rb.hold_glen_lock(A.id);
    let d = data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .env("GLEN_JOB_TIMEOUT", "1s")
            .run(),
        10,
    );
    let id = job_id(&d);
    let out = rb
        .glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
        .run();
    let err = failed(&out, "job_failed");
    assert_job_failed(&err, "failed", "timeout");
    drop(lock);
    rb.assert_quiet();
    assert!(
        rb.starts().is_empty(),
        "no omp for a job that never got the lock"
    );
}

#[test]
fn a7_glen_job_timeout_is_recorded_at_creation_and_invalid_values_warn() {
    let rb = RpcBox::new();
    let d = data_of(
        &rb.glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
            .env("GLEN_JOB_TIMEOUT", "45s")
            .run(),
        0,
    );
    assert_eq!(rb.job_json(&job_id(&d))["job_timeout_ms"], 45_000);
    for bad in ["abc", "0", "25h"] {
        let out = rb
            .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
            .env("GLEN_JOB_TIMEOUT", bad)
            .run();
        let env = ok_doc(&out, 0);
        let id = job_id(&env["data"]);
        assert_eq!(
            rb.job_json(&id)["job_timeout_ms"],
            3_600_000,
            "GLEN_JOB_TIMEOUT={bad} falls back to 60m"
        );
        assert!(
            warning_codes(&env).iter().any(|c| c == "invalid_env"),
            "GLEN_JOB_TIMEOUT={bad} warns: {env}"
        );
        rb.wait_terminal(&id);
    }
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-216 — teardown.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac216_after_the_result_no_omp_process_remains_and_the_glen_lock_is_free() {
    let rb = RpcBox::new();
    let d = data_of(
        &rb.glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
            .run(),
        0,
    );
    let st = &rb.starts()[0];
    let pid = st["pid"].as_i64().unwrap();
    let pgid = st["pgid"].as_i64().unwrap();
    let worker = rb.status(&job_id(&d))["worker_pid"].as_i64().unwrap();
    poll_until(
        "omp's group gone, the worker gone and the glen lock free",
        Duration::from_secs(5),
        || {
            (!pid_alive(pid)
                && (pgid == own_pgid() || !group_alive(pgid))
                && !pid_alive(worker)
                && rb.glen_lock_free(A.id))
            .then_some(())
        },
    );
    assert_ne!(pgid, own_pgid(), "omp shares the test's process group");
}

// ---------------------------------------------------------------------------------------------
// AC-217 — signals.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac217_ctrl_c_on_send_prints_the_running_document_and_the_job_completes() {
    let rb = RpcBox::new();
    let child = rb.spawn_group(
        &["send", A.handle, "hello", "--wait", "30s", "--json"],
        &[("FAKE_OMP_DELAY_MS", "3000")],
    );
    let pid = child.id() as i64;
    let id = rb.wait_one_job();
    rb.wait_status(&id, "accepted", LONG, |s| s["status"] == "accepted");
    signal_group(pid, Signal::INT);
    let out = finish(child, "send (SIGINT)", LONG);
    let d = data_of(&out, 130);
    assert_eq!(d["interrupted"], true, "{d}");
    assert_eq!(d["status"], "running", "{d}");
    assert_eq!(d["job"]["id"], id.as_str(), "{d}");
    assert_eq!(d["next"], format!("glen wait {}", job_handle(&id)), "{d}");
    let w = data_of(
        &rb.glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
            .run(),
        0,
    );
    assert_eq!(w["assistant_text"], "pong: hello", "{w}");
    assert!(
        rb.inbound("abort").is_empty(),
        "Ctrl-C on send must not abort"
    );
    rb.assert_quiet();
}

#[test]
fn ac217_ctrl_c_on_wait_keeps_the_job_running() {
    let rb = RpcBox::new();
    let d = data_of(
        &rb.glen(["send", A.handle, "hello", "--no-wait", "--json"])
            .env("FAKE_OMP_DELAY_MS", "5000")
            .run(),
        10,
    );
    let id = job_id(&d);
    let h = job_handle(&id);
    rb.wait_status(&id, "accepted", LONG, |s| s["status"] == "accepted");
    let child = rb.spawn_group(&["wait", &h, "--timeout", "30s", "--json"], &[]);
    // Let `wait` start observing (it has no readiness signal of its own).
    std::thread::sleep(Duration::from_millis(1000));
    signal_group(child.id() as i64, Signal::INT);
    let out = finish(child, "wait (SIGINT)", LONG);
    let w = data_of(&out, 130);
    assert_eq!(w["interrupted"], true, "{w}");
    assert_eq!(w["status"], "running", "{w}");
    assert_eq!(w["job"]["id"], id.as_str(), "{w}");
    let w = data_of(
        &rb.glen(["wait", &h, "--timeout", "20s", "--json"]).run(),
        0,
    );
    assert_eq!(w["assistant_text"], "pong: hello", "{w}");
    assert!(
        rb.inbound("abort").is_empty(),
        "Ctrl-C on wait must not abort"
    );
    rb.assert_quiet();
}

fn worker_signal_cancels(sig: Signal) {
    let rb = RpcBox::new();
    let d = data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .env("FAKE_OMP_SCENARIO", "hang")
            .run(),
        10,
    );
    let id = job_id(&d);
    let st = rb.wait_status(&id, "accepted", LONG, |s| {
        s["status"] == "accepted" && s["worker_pid"].is_u64()
    });
    rb.wait_log("the prompt", |r| {
        r.iter().any(|x| is_frame(x, "in", "prompt"))
    });
    let t = Instant::now();
    signal(st["worker_pid"].as_i64().unwrap(), sig);
    let fin = rb.wait_status(&id, "terminal", LONG, is_terminal);
    let took = t.elapsed();
    assert_eq!(fin["status"], "aborted", "{fin}");
    assert!(
        took <= ABORT_BOUND,
        "§A7: aborted within 1 s of the signal, took {took:?}"
    );
    let out = rb
        .glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
        .run();
    let err = failed(&out, "job_failed");
    assert_job_failed(&err, "aborted", "cancelled");
    rb.assert_quiet();
    assert!(
        !rb.inbound("abort").is_empty(),
        "the run was aborted\n{}",
        rb.diag()
    );
}

#[test]
fn ac217_sigterm_on_the_worker_cancels_the_job() {
    worker_signal_cancels(Signal::TERM);
}

#[test]
fn ac217_sigint_on_the_worker_cancels_the_job() {
    worker_signal_cancels(Signal::INT);
}

#[test]
fn ac217_sighup_on_the_worker_cancels_the_job() {
    worker_signal_cancels(Signal::HUP);
}

// ---------------------------------------------------------------------------------------------
// AC-218 — the control inbox.
// ---------------------------------------------------------------------------------------------

const ABORT: &str = r#"{"v":1,"op":"abort"}"#;
const ABORT_BOUND: Duration = Duration::from_millis(1500);

fn assert_cancelled_quickly(rb: &RpcBox, id: &str, since: Instant) {
    let st = rb.wait_status(id, "terminal", LONG, is_terminal);
    let took = since.elapsed();
    assert_eq!(st["status"], "aborted", "{st}");
    assert_eq!(st["error"]["code"], "cancelled", "{st}");
    assert!(took <= ABORT_BOUND, "abort took {took:?}");
    let done = rb.wait_file(&rb.job_dir(id).join("control/a.done.json"), LONG);
    assert_eq!(done, json!({"v": 1, "ok": true}));
}

#[test]
fn ac218_an_abort_request_cancels_a_running_job_within_a_second() {
    let rb = RpcBox::new();
    let id = job_id(&data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .env("FAKE_OMP_SCENARIO", "hang")
            .run(),
        10,
    ));
    rb.wait_log("the prompt", |r| {
        r.iter().any(|x| is_frame(x, "in", "prompt"))
    });
    rb.wait_status(&id, "accepted", LONG, |s| s["status"] == "accepted");
    let t = Instant::now();
    write_control(&rb, &id, "a.json", ABORT);
    assert_cancelled_quickly(&rb, &id, t);
    poll_until("the request file to be removed", LONG, || {
        (!rb.job_dir(&id).join("control/a.json").exists()).then_some(())
    });
    let err = failed(
        &rb.glen(["wait", &job_handle(&id), "--json"]).run(),
        "job_failed",
    );
    assert_job_failed(&err, "aborted", "cancelled");
    rb.assert_quiet();
}

#[test]
fn ac218_unknown_malformed_and_ignored_control_files_leave_the_job_running() {
    let rb = RpcBox::new();
    let id = job_id(&data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .env("FAKE_OMP_SCENARIO", "hang")
            .run(),
        10,
    ));
    rb.wait_log("the prompt", |r| {
        r.iter().any(|x| is_frame(x, "in", "prompt"))
    });
    rb.wait_status(&id, "accepted", LONG, |s| s["status"] == "accepted");
    let control = rb.job_dir(&id).join("control");
    // §A7: dotfiles and `.done.json` files are never requests.
    write_control(&rb, &id, ".d.json", ABORT);
    write_control(&rb, &id, "e.done.json", ABORT);
    write_control(&rb, &id, "b.json", r#"{"v":1,"op":"set_model"}"#);
    write_control(&rb, &id, "c.json", "not json");
    for name in ["b", "c"] {
        let done = rb.wait_file(&control.join(format!("{name}.done.json")), LONG);
        assert_eq!(done["v"], 1, "{name}: {done}");
        assert_eq!(done["ok"], false, "{name}: {done}");
        assert_eq!(done["error"]["code"], "unsupported_op", "{name}: {done}");
        assert!(done["error"]["message"].is_string(), "{name}: {done}");
    }
    std::thread::sleep(Duration::from_millis(600));
    let st = rb.status(&id);
    assert_eq!(st["status"], "accepted", "the job continues: {st}");
    assert!(!control.join(".d.done.json").exists());
    assert!(!control.join("e.done.done.json").exists());

    let t = Instant::now();
    write_control(&rb, &id, "a.json", ABORT);
    assert_cancelled_quickly(&rb, &id, t);
    rb.assert_quiet();
}

#[test]
fn ac218_an_abort_while_queued_is_honoured_without_starting_omp() {
    let rb = RpcBox::new();
    let lock = rb.hold_glen_lock(A.id);
    let d = data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .run(),
        10,
    );
    assert_eq!(d["job"]["status"], "queued", "{d}");
    let id = job_id(&d);
    rb.wait_status(&id, "claimed by its worker", LONG, |s| {
        s["worker_pid"].is_u64()
    });
    let t = Instant::now();
    write_control(&rb, &id, "a.json", ABORT);
    assert_cancelled_quickly(&rb, &id, t);
    assert!(!rb.glen_lock_free(A.id), "still held by the test");
    drop(lock);
    rb.assert_quiet();
    assert!(rb.starts().is_empty(), "omp was started\n{}", rb.diag());
}

#[test]
fn ac218_an_abort_while_omp_is_starting_is_honoured_and_never_prompts() {
    let rb = RpcBox::new();
    let id = job_id(&data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .env("FAKE_OMP_READY_DELAY_MS", "5000")
            .run(),
        10,
    ));
    rb.wait_log("the fake omp start", |r| !starts(r).is_empty());
    let t = Instant::now();
    write_control(&rb, &id, "a.json", ABORT);
    assert_cancelled_quickly(&rb, &id, t);
    rb.assert_quiet();
    assert!(
        rb.inbound("prompt").is_empty(),
        "a prompt was sent\n{}",
        rb.diag()
    );
}

// ---------------------------------------------------------------------------------------------
// AC-219 — detached pipes.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac219_send_pipes_reach_eof_within_a_second_of_its_exit() {
    let rb = RpcBox::new();
    let mut cmd = rb.command(&["send", A.handle, "hi", "--no-wait", "--json"]);
    cmd.env("FAKE_OMP_DELAY_MS", "4000")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let eof = |mut r: Box<dyn Read + Send>| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = r.read_to_end(&mut b);
            let _ = tx.send((b, Instant::now()));
        });
        rx
    };
    let out_rx = eof(Box::new(child.stdout.take().unwrap()));
    let err_rx = eof(Box::new(child.stderr.take().unwrap()));
    let started = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        assert!(started.elapsed() < LONG, "send did not exit");
        std::thread::sleep(Duration::from_millis(5));
    };
    let exited = Instant::now();
    let (stdout, t_out) = out_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("stdout EOF");
    let (stderr, t_err) = err_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("stderr EOF");
    let one = Duration::from_secs(1);
    assert!(
        t_out.saturating_duration_since(exited) <= one,
        "stdout EOF {:?} after exit",
        t_out.saturating_duration_since(exited)
    );
    assert!(
        t_err.saturating_duration_since(exited) <= one,
        "stderr EOF {:?} after exit",
        t_err.saturating_duration_since(exited)
    );
    assert_eq!(status.code(), Some(10));
    assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
    let env: Value = serde_json::from_slice(&stdout).expect("one JSON envelope");
    let id = job_id(&env["data"]);
    let st = rb.status(&id);
    assert!(!is_terminal(&st), "the worker is still running: {st}");
    data_of(
        &rb.glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
            .run(),
        0,
    );
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-222 — failures before the job exists.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac222_send_failures_before_the_job_leave_no_glen_home() {
    let rb = RpcBox::new();
    let missing = s(&rb.bx.root.join("nope"));
    let cases: Vec<(Vec<&str>, Option<&str>, &str)> = vec![
        (vec!["send", "0000000", "hi"], None, "not_found"),
        (vec!["send", "j1234567", "hi"], None, "usage"),
        (vec!["send", "c0de", "hi"], None, "ambiguous"),
        (vec!["send", A.handle, "   "], None, "usage"),
        (vec!["send", A.handle, ""], None, "usage"),
        (vec!["send", A.handle, "hi", "--cwd", "/tmp"], None, "usage"),
        (
            vec!["send", "@new", "hi", "--cwd", &missing],
            None,
            "not_found",
        ),
        (
            vec!["--profile", "Bad Name", "send", "@new", "hi"],
            None,
            "usage",
        ),
        (
            vec!["send", A.handle, "hi"],
            Some("/nonexistent/omp"),
            "omp_unavailable",
        ),
        (vec!["send", "@new", "hi"], Some(""), "omp_unavailable"),
    ];
    for (mut args, omp, code) in cases {
        args.push("--json");
        let mut run = rb.glen(&args);
        match omp {
            Some("") => run = run.env_remove("GLEN_OMP_BIN"),
            Some(p) => run = run.env("GLEN_OMP_BIN", p),
            None => {}
        }
        let out = run.run();
        let err = failed(&out, code);
        if code == "ambiguous" {
            let c = candidates(&err, 2);
            assert!(c.contains(A.id) && c.contains(F.id), "{c}");
        }
        if args[1] == "j1234567" {
            assert!(
                err["hint"]
                    .as_str()
                    .is_some_and(|h| h.contains("glen wait")),
                "§A8: a job handle where a session is expected hints at glen wait: {err}"
            );
        }
        assert!(
            !rb.bx.glen_home.exists(),
            "GLEN_HOME created by {args:?}: {:?}",
            list_tree(&rb.bx.glen_home)
        );
        assert!(rb.starts().is_empty(), "omp started by {args:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// AC-224 — message rules and delivery modes.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac224_a_dash_reads_the_message_from_stdin() {
    let rb = RpcBox::new();
    let out = rb.glen_stdin(
        &["send", A.handle, "-", "--wait", "20s", "--json"],
        &[],
        b"hi from stdin",
    );
    let d = data_of(&out, 0);
    assert_eq!(d["assistant_text"], "pong: hi from stdin", "{d}");
    assert_eq!(rb.inbound("prompt")[0]["message"], "hi from stdin");
    rb.assert_quiet();
}

#[test]
fn ac224_empty_oversized_and_non_utf8_messages_are_usage_errors() {
    let rb = RpcBox::new();
    let big = vec![b'a'; 524_289];
    let inputs: [&[u8]; 4] = [b"", b"  \n\t \n", &big, &[0xff, 0xfe, b'a']];
    for input in inputs {
        let out = rb.glen_stdin(&["send", A.handle, "-", "--json"], &[], input);
        failed(&out, "usage");
    }
    let out = rb
        .glen([
            OsStr::new("send"),
            OsStr::new(A.handle),
            OsStr::from_bytes(b"\xff\xfe"),
            OsStr::new("--json"),
        ])
        .run();
    failed(&out, "usage");
    assert!(!rb.bx.glen_home.exists(), "no job for a refused message");
    assert!(rb.starts().is_empty());
}

#[test]
fn ac224_a_message_of_exactly_512_kib_is_accepted() {
    let rb = RpcBox::new();
    let msg = vec![b'a'; 524_288];
    let out = rb.glen_stdin(
        &["send", A.handle, "-", "--wait", "20s", "--json"],
        &[("FAKE_OMP_SCENARIO", "local")],
        &msg,
    );
    let d = data_of(&out, 0);
    assert_eq!(d["status"], "completed", "{d}");
    let prompts = rb.inbound("prompt");
    assert_eq!(prompts[0]["message"].as_str().map(str::len), Some(524_288));
    rb.assert_quiet();
}

#[test]
fn ac224_delivery_modes_map_to_streaming_behavior_and_idle_reports_prompt() {
    let rb = RpcBox::new();
    let cases: [(&[&str], &str, &str); 6] = [
        (&[], "followUp", "auto"),
        (&["--as", "auto"], "followUp", "auto"),
        (&["--as", "steer"], "steer", "steer"),
        (&["--steer"], "steer", "steer"),
        (&["--as", "follow-up"], "followUp", "follow_up"),
        (&["--follow-up"], "followUp", "follow_up"),
    ];
    for (i, (flags, behavior, deliver)) in cases.into_iter().enumerate() {
        let log = rb.log_path(&format!("deliver-{i}"));
        let mut args = vec!["send", A.handle, "hi"];
        args.extend_from_slice(flags);
        args.extend(["--wait", "20s", "--json"]);
        let d = data_of(&rb.glen(&args).env("FAKE_OMP_LOG", &log).run(), 0);
        assert_eq!(d["delivered_as"], "prompt", "{flags:?}: {d}");
        assert_eq!(d["job"]["deliver_as"], deliver, "{flags:?}: {d}");
        let prompts = frames_in(&records(&log), "in", "prompt");
        assert_eq!(prompts.len(), 1, "{flags:?}");
        assert_eq!(
            prompts[0]["streamingBehavior"], behavior,
            "{flags:?}: {}",
            prompts[0]
        );
    }
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-225 — moved sessions.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac225_a_baseline_id_mismatch_is_refused_before_the_prompt() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "moved")
        .run();
    let err = failed(&out, "job_failed");
    assert_job_failed(&err, "failed", "session_moved");
    let details = &err["details"]["error"]["details"];
    assert_eq!(details["from"], A.id, "{err}");
    assert_eq!(details["to"], retail(A.id, "beef"), "{err}");
    rb.assert_quiet();
    assert!(rb.inbound("prompt").is_empty(), "{}", rb.diag());
}

/// Publish a continuation of A in A's bucket the way omp does (stage a dotfile, rename): A's
/// entries plus a newer turn, header `continuationOf` A.
fn continue_a(bx: &OmpBox, new_id: &str) -> PathBuf {
    let created = "2026-10-01T10:00:00.000Z";
    let path = bx.sessions.join(ALPHA).join(session_name(created, new_id));
    let mut h = header(new_id, Some(created), Some(A.cwd));
    h["parentSession"] = Value::from(A.id);
    h["continuationOf"] = Value::from(A.id);
    let mut text = format!("{h}\n");
    for l in entry_lines(&bx.path(&A)) {
        text.push_str(&l);
        text.push('\n');
    }
    let turn = user(
        "aa225001",
        Some("10000009"),
        "2026-10-07T10:00:00.000Z",
        "continued elsewhere",
    );
    text.push_str(&format!("{turn}\n"));
    let name = path.file_name().unwrap().to_str().unwrap().to_owned();
    let stage = path.with_file_name(format!(".{name}.staged.tmp"));
    fs::write(&stage, text).unwrap();
    fs::rename(&stage, &path).unwrap();
    path
}

#[test]
fn ac225_a_queued_job_whose_session_moved_meanwhile_is_refused_without_omp() {
    let rb = RpcBox::new();
    let lock = rb.hold_glen_lock(A.id);
    let d = data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .run(),
        10,
    );
    assert_eq!(d["job"]["status"], "queued", "{d}");
    let id = job_id(&d);
    let moved_to = uuid(0x225_0225);
    continue_a(&rb.bx, &moved_to);
    drop(lock);
    let err = failed(
        &rb.glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
            .run(),
        "job_failed",
    );
    assert_job_failed(&err, "failed", "session_moved");
    let details = &err["details"]["error"]["details"];
    assert_eq!(details["from"], A.id, "{err}");
    assert_eq!(details["to"], moved_to.as_str(), "{err}");
    rb.assert_quiet();
    assert!(rb.starts().is_empty(), "omp was started\n{}", rb.diag());
}

// ---------------------------------------------------------------------------------------------
// AC-226 — the event filter.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac226_the_event_filter_drops_streaming_updates_and_precedes_the_prompt() {
    let rb = RpcBox::new();
    data_of(
        &rb.glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
            .run(),
        0,
    );
    rb.assert_quiet();
    let recs = rb.recs();
    let filters = frames_in(&recs, "in", "set_event_filter");
    assert_eq!(filters.len(), 1, "{}", rb.diag());
    let events = strings(&filters[0]["events"]);
    for keep in ["message_end", "agent_end", "notice"] {
        assert!(events.iter().any(|e| e == keep), "{keep} kept: {events:?}");
    }
    for drop in [
        "message_update",
        "tool_execution_update",
        "tool_stream_update",
    ] {
        assert!(
            !events.iter().any(|e| e == drop),
            "{drop} excluded: {events:?}"
        );
    }
    let filter_at = index_of(&recs, |r| is_frame(r, "in", "set_event_filter")).unwrap();
    let prompt_at = index_of(&recs, |r| is_frame(r, "in", "prompt")).unwrap();
    assert!(filter_at < prompt_at, "the filter is set before the prompt");
}

#[test]
fn ac226_a_rejected_event_filter_only_adds_a_warning() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "reject_filter")
        .run();
    let env = ok_doc(&out, 0);
    assert_eq!(env["data"]["status"], "completed", "{env}");
    assert_eq!(env["data"]["assistant_text"], "pong: hi", "{env}");
    assert!(
        warning_codes(&env)
            .iter()
            .any(|c| c == "event_filter_rejected"),
        "{env}"
    );
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-227 — output modes.
// ---------------------------------------------------------------------------------------------

fn has_spinner_frames(s: &str) -> bool {
    s.chars().any(|c| ('\u{2800}'..='\u{28ff}').contains(&c)) || s.contains('\r')
}

#[test]
fn ac227_human_mode_shows_the_reply_and_no_spinner_on_a_pipe() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hello", "--wait", "20s", "--human"])
        .env("FAKE_OMP_SCENARIO", "progress")
        .run();
    out.assert_exit(0).assert_no_ansi().assert_no_panic();
    let text = out.out();
    assert!(!out.is_json(), "{}", out.ctx());
    assert!(text.contains("✓ completed"), "{}", out.ctx());
    assert!(text.contains("pong: hello"), "{}", out.ctx());
    assert!(
        text.contains("↩") && text.contains("progress") && text.contains("reply: hello"),
        "{}",
        out.ctx()
    );
    assert!(
        !has_spinner_frames(&out.err()),
        "spinner on a pipe\n{}",
        out.ctx()
    );

    let out = rb
        .glen(["send", A.handle, "hi", "--no-wait", "--human"])
        .env("FAKE_OMP_DELAY_MS", "1500")
        .run();
    out.assert_exit(10).assert_no_ansi();
    assert!(out.out().contains("glen wait j"), "{}", out.ctx());
    assert!(!has_spinner_frames(&out.err()), "{}", out.ctx());

    // uuidv7 ids sort by creation: the newest directory is the running job.
    let id = rb.job_ids().pop().expect("the running job");
    let out = rb
        .glen(["wait", &job_handle(&id), "--timeout", "20s", "--human"])
        .run();
    out.assert_exit(0).assert_no_ansi();
    assert!(out.out().contains("✓ completed"), "{}", out.ctx());
    assert!(!has_spinner_frames(&out.err()), "{}", out.ctx());
    rb.assert_quiet();
}

#[test]
fn ac227_json_mode_is_one_envelope_for_every_outcome() {
    let rb = RpcBox::new();
    // data_of / failed check one document, empty stderr and no ESC for each run.
    let d = data_of(
        &rb.glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
            .run(),
        0,
    );
    data_of(
        &rb.glen(["wait", &job_handle(&job_id(&d)), "--json"]).run(),
        0,
    );
    data_of(&rb.glen(["jobs", "--json"]).run(), 0);
    failed(
        &rb.glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
            .env("FAKE_OMP_SCENARIO", "error")
            .run(),
        "job_failed",
    );
    failed(&rb.glen(["wait", "j7777777", "--json"]).run(), "not_found");
    rb.assert_quiet();
}

#[test]
fn ac227_a_large_document_spills_to_a_private_file() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("GLEN_MAX_OUTPUT", "2000")
        .env("FAKE_OMP_REPLY_BYTES", "40000")
        .run();
    let env = ok_doc(&out, 0);
    let sp = &env["spilled"];
    assert_eq!(sp["format"], "json", "{env}");
    let path = PathBuf::from(sp["path"].as_str().expect("spilled.path"));
    assert!(path.is_absolute());
    assert_eq!(mode(&path), 0o600);
    let doc = read_json(&path).expect("spilled JSON");
    assert_eq!(sp["bytes"], fs::metadata(&path).unwrap().len(), "{env}");
    assert_eq!(
        doc["data"]["assistant_text"].as_str().map(str::len),
        Some(40_000),
        "the full document is in the file"
    );
    rb.assert_quiet();
}

#[test]
fn a10_model_output_is_escaped_in_human_mode_and_raw_in_json() {
    let rb = RpcBox::new();
    let msg = "red \u{1b}[31m alert";
    let out = rb
        .glen([
            "send", A.handle, msg, "--wait", "20s", "--human", "--color", "never",
        ])
        .run();
    out.assert_exit(0);
    assert!(
        !has_esc(&out.stdout),
        "raw ESC from model output\n{}",
        out.ctx()
    );
    assert!(out.out().contains("\\x1b[31m"), "{}", out.ctx());
    let d = data_of(
        &rb.glen(["send", A.handle, msg, "--wait", "20s", "--json"])
            .run(),
        0,
    );
    assert_eq!(d["assistant_text"], format!("pong: {msg}"), "{d}");
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// Plan Step 13 extras: protocol negotiation and an omp that never becomes ready.
// ---------------------------------------------------------------------------------------------

#[test]
fn plan_a_full_v2_advertisement_is_negotiated_and_a_partial_one_stays_on_v1() {
    let rb = RpcBox::new();
    for (v2, negotiated) in [("1", true), ("partial", false)] {
        let log = rb.log_path(&format!("v2-{v2}"));
        let d = data_of(
            &rb.glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
                .env("FAKE_OMP_V2", v2)
                .env("FAKE_OMP_LOG", &log)
                .run(),
            0,
        );
        assert_eq!(d["assistant_text"], "pong: hi", "{d}");
        let n = frames_in(&records(&log), "in", "negotiate_protocol").len();
        assert_eq!(
            n > 0,
            negotiated,
            "FAKE_OMP_V2={v2}: {n} negotiate_protocol frames"
        );
    }
    rb.assert_quiet();
}

#[test]
fn plan_an_omp_that_never_becomes_ready_fails_the_job_without_a_prompt() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "no_ready")
        .env("GLEN_JOB_TIMEOUT", "2s")
        .run();
    let err = failed(&out, "job_failed");
    // §A7: whichever bound fires decides the code; the 2 s job deadline beats the 60 s ready bound.
    assert_job_failed(&err, "failed", "timeout");
    rb.assert_quiet();
    assert!(rb.inbound("prompt").is_empty());
}
