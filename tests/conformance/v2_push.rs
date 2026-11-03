//! v0.2 W2 (replies without polling, push tier 1) conformance: AC-v2-201 … AC-v2-206.
//!
//! Derived from the settled acceptance criteria and "AC details" of
//! `.claude/plans/v020-wave-2-push.md` (with the binding judge edits) and from the public
//! contracts they name: `glen wait --help` and `glen inbox --help`, `docs/json.md`,
//! `docs/design.md` §A5, §A7, §A11, §A15 and §A20, the rendered skill (through `install-skill`),
//! the fake peers' documented knobs and the files under `tests/acceptance`. Written without
//! reading the implementation or the Implementer's own tests.
//!
//! The timing cases measure from the moment a reply is observed in `jobs/<id>/replies.ndjson`
//! (the file both routes append to, §A5, §A6) or in `inbox/<session>.ndjson` to the moment the
//! waiting `glen` exits. Jobs are held in a known state by the peers' release files
//! (`FAKE_OMP_RUN_RELEASE`, `FAKE_OMP_RESULT_RELEASE`, `FAKE_EXT_RUN_RELEASE`,
//! `FAKE_EXT_RESULT_RELEASE`) so no case races a job's own end. Every `glen` runs with a cleared
//! environment against a scratch HOME and GLEN_HOME and a fake peer, never a live `~/.omp`.
//!
//! Under `scripts/flake-check.sh` the waits and the upper bounds on the product's timing (the
//! AC-v2-201 and AC-v2-202 one-second wakes, the 2 s SIGINT exits, "at once" bounds) are multiplied
//! by `GLEN_TEST_TIME_SCALE` (`flake::scaled`, exact in a plain run); lower bounds and the windows
//! in which nothing may happen are not.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rustix::process::Signal;
use serde_json::{Value, json};

use crate::flake::{Captured, await_sigint_handler, scaled, try_output};
use crate::live_support::*;
use crate::reader::*;
use crate::rpc_support::*;
use crate::support::*;

/// AC-v2-201 and AC-v2-202: a reply wakes the waiting `glen` within this.
const ONE_SECOND: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------------------------
// Helpers: a background `glen`, file watchers, `next` commands, durations.
// ---------------------------------------------------------------------------------------------

/// A running `glen` whose exit instant is recorded by the thread that reaps it. Its output goes to
/// files (`flake::Captured`): the exit instant is the reap, never a pipe's end of file, which a
/// process that inherited the pipe from a concurrent spawn could hold back.
struct Bg {
    pid: u32,
    rx: mpsc::Receiver<(Out, Instant)>,
    done: Option<(Out, Instant)>,
    /// The reaping thread reported the exit, so the pid may be reused and is never signalled.
    reaped: bool,
}

impl Bg {
    fn spawn(mut cmd: Command, desc: &str) -> Bg {
        let child = Captured::spawn(&mut cmd).unwrap_or_else(|e| panic!("spawn glen {desc}: {e}"));
        let pid = child.id();
        let started = Instant::now();
        let desc = format!("glen {desc}");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = child.wait_with_output();
            let at = Instant::now();
            let out = Out {
                desc,
                status: r.status,
                stdout: r.stdout,
                stderr: r.stderr,
                elapsed: at.duration_since(started),
            };
            let _ = tx.send((out, at));
        });
        Bg {
            pid,
            rx,
            done: None,
            reaped: false,
        }
    }

    /// Still running (it has not exited yet).
    fn running(&mut self) -> bool {
        if self.done.is_none()
            && let Ok(x) = self.rx.try_recv()
        {
            self.done = Some(x);
            self.reaped = true;
        }
        self.done.is_none()
    }

    fn signal(&self, sig: Signal) {
        signal(i64::from(self.pid), sig);
    }

    /// Wait for the exit (bounded); the output and the instant the process was reaped.
    fn join(mut self, timeout: Duration) -> (Out, Instant) {
        if let Some(x) = self.done.take() {
            return x;
        }
        match self.rx.recv_timeout(timeout) {
            Ok(x) => {
                self.reaped = true;
                x
            }
            Err(_) => {
                signal(i64::from(self.pid), Signal::KILL);
                panic!(
                    "a background glen (pid {}) did not exit within {timeout:?}",
                    self.pid
                );
            }
        }
    }
}

impl Drop for Bg {
    fn drop(&mut self) {
        if !self.reaped
            && let Some(p) = i32::try_from(self.pid)
                .ok()
                .filter(|p| *p > 1)
                .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process(p, Signal::KILL);
        }
    }
}

/// The instant `path` first holds at least `lines` newline-terminated lines (polled every 2 ms).
fn watch_lines(path: PathBuf, lines: usize) -> mpsc::Receiver<Instant> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let deadline = Instant::now() + scaled(Duration::from_secs(30));
        while Instant::now() < deadline {
            if let Ok(b) = fs::read(&path)
                && b.iter().filter(|c| **c == b'\n').count() >= lines
            {
                let _ = tx.send(Instant::now());
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    rx
}

/// Split a command line the way a POSIX shell would (quotes and backslashes; no expansion).
fn shell_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '"' => {
                in_word = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => match chars.peek() {
                            Some(&n) if matches!(n, '"' | '\\' | '$' | '`') => {
                                cur.push(n);
                                chars.next();
                            }
                            _ => cur.push('\\'),
                        },
                        _ => cur.push(q),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

/// `<n>ms|s|m|h` or bare seconds (§A9 Durations).
fn parse_dur(s: &str) -> Option<Duration> {
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: u64 = num.parse().ok()?;
    match unit {
        "ms" => Some(Duration::from_millis(n)),
        "" | "s" => Some(Duration::from_secs(n)),
        "m" => Some(Duration::from_secs(n * 60)),
        "h" => Some(Duration::from_secs(n * 3600)),
        _ => None,
    }
}

/// The word after `flag` in `words`.
fn flag_value<'a>(words: &'a [String], flag: &str) -> Option<&'a str> {
    words
        .iter()
        .position(|w| w == flag)
        .and_then(|i| words.get(i + 1))
        .map(String::as_str)
}

/// A `data.next` of `wait --for reply`: `glen wait <handle> --for reply [--after X] [--timeout T]`,
/// with `--timeout` present only when the caller's bound differs from the 90 s default (§A20).
fn assert_wait_next(d: &Value, handle: &str, after: Option<&str>, timeout: Option<Duration>) {
    let next = d["next"]
        .as_str()
        .unwrap_or_else(|| panic!("data.next is not a string: {d}"));
    assert!(!next.contains('\n'), "next is one line: {next:?}");
    let w = shell_words(next);
    assert_eq!(
        w.iter().take(3).map(String::as_str).collect::<Vec<_>>(),
        ["glen", "wait", handle],
        "next: {next:?}"
    );
    assert_eq!(flag_value(&w, "--for"), Some("reply"), "next: {next:?}");
    assert_eq!(flag_value(&w, "--after"), after, "next: {next:?}");
    let mut len = 5 + 2 * usize::from(after.is_some());
    match timeout {
        Some(t) => {
            let v = flag_value(&w, "--timeout")
                .unwrap_or_else(|| panic!("next keeps a non-default --timeout: {next:?}"));
            assert_eq!(parse_dur(v), Some(t), "next --timeout: {next:?}");
            len += 2;
        }
        None => assert!(
            flag_value(&w, "--timeout").is_none(),
            "a default bound is not repeated: {next:?}"
        ),
    }
    assert_eq!(w.len(), len, "next has extra words: {next:?}");
}

fn reply_ids(v: &Value) -> Vec<String> {
    arr(v)
        .iter()
        .map(|r| r["id"].as_str().expect("reply id").to_owned())
        .collect()
}

/// `data.replies` equals the on-disk records `want` in order (identity, text, final, owner).
fn assert_replies(got: &Value, want: &[&Value]) {
    let got = arr(got);
    assert_eq!(got.len(), want.len(), "replies: {got:?} want {want:?}");
    for (g, w) in got.iter().zip(want) {
        for k in ["id", "text", "final", "job_id", "session_id"] {
            assert_eq!(g[k], w[k], "reply `{k}`: {g} vs {w}");
        }
    }
}

/// The complete reply records of a job's `replies.ndjson` (a torn tail line is not counted).
fn replies_on_disk(rb: &RpcBox, id: &str) -> Vec<Value> {
    let bytes = fs::read(rb.job_dir(id).join("replies.ndjson")).unwrap_or_default();
    let mut lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    lines.pop(); // empty, or the unterminated tail
    lines
        .iter()
        .filter_map(|l| serde_json::from_slice(l).ok())
        .collect()
}

/// A job held in a known state by the peer's release files.
struct HeldJob {
    id: String,
    handle: String,
    run: PathBuf,
    result: PathBuf,
}

impl HeldJob {
    fn new(id: String, run: PathBuf, result: PathBuf) -> HeldJob {
        HeldJob {
            handle: job_handle(&id),
            id,
            run,
            result,
        }
    }

    /// Let the peer produce its replies.
    fn release_run(&self) {
        fs::write(&self.run, "").unwrap();
    }

    /// Let the peer finish the job.
    fn release_result(&self) {
        fs::write(&self.result, "").unwrap();
    }
}

/// `fake_omp` `progress` (two replies, then the result): `accepted`, nothing replied yet.
fn held_rpc_job(rb: &RpcBox) -> HeldJob {
    let run = rb.bx.root.join("run-release");
    let result = rb.bx.root.join("result-release");
    let out = rb
        .glen(["send", A.handle, "hello", "--no-wait", "--json"])
        .env("FAKE_OMP_SCENARIO", "progress")
        .env("FAKE_OMP_RUN_RELEASE", &run)
        .env("FAKE_OMP_RESULT_RELEASE", &result)
        .run();
    let id = job_id(&data_of(&out, 10));
    rb.wait_status(&id, "accepted", scaled(LONG), |s| s["status"] == "accepted");
    HeldJob::new(id, run, result)
}

/// Both `progress` replies are on disk, the result is still held, the job is `accepted`.
fn replied_rpc_job(rb: &RpcBox) -> (HeldJob, Vec<Value>) {
    let h = held_rpc_job(rb);
    h.release_run();
    let replies = poll_until("two replies on disk", scaled(LONG), || {
        let r = replies_on_disk(rb, &h.id);
        (r.len() >= 2).then_some(r)
    });
    (h, replies)
}

/// Let a held job finish and make sure no worker or peer outlives the test.
fn finish_held(rb: &RpcBox, h: &HeldJob) {
    h.release_run();
    h.release_result();
    rb.wait_terminal(&h.id);
    rb.assert_quiet();
}

/// `wait --for reply [--after X] [--timeout T] --json` as a background process.
fn spawn_reply_wait(rb: &RpcBox, h: &HeldJob, extra: &[&str]) -> Bg {
    let mut args = vec!["wait", h.handle.as_str(), "--for", "reply"];
    args.extend_from_slice(extra);
    args.push("--json");
    Bg::spawn(rb.command(&args), &args.join(" "))
}

/// `wait --for reply …` run to completion.
fn reply_wait(rb: &RpcBox, handle: &str, extra: &[&str]) -> Out {
    let mut args = vec!["wait", handle, "--for", "reply"];
    args.extend_from_slice(extra);
    args.push("--json");
    rb.glen(args).run()
}

/// A reply-wake document: exit 10, `woke:"reply"`, `status:"running"`, the replies `want`.
fn assert_wake(out: &Out, h: &HeldJob, want: &[&Value]) -> Value {
    let d = data_of(out, 10);
    assert_eq!(d["status"], "running", "{d}");
    assert_eq!(d["woke"], "reply", "{d}");
    assert_eq!(d["interrupted"], false, "{d}");
    assert_eq!(d["job"]["id"], h.id.as_str(), "{d}");
    assert_eq!(d["job"]["handle"], h.handle.as_str(), "{d}");
    assert_eq!(d["job"]["status"], "accepted", "{d}");
    assert_eq!(d["job"]["task_state"], "working", "{d}");
    assert!(d["session"]["id"].is_string(), "{d}");
    assert_replies(&d["replies"], want);
    d
}

/// A bound-expiry document: exit 10 without `woke`, `replies:[]`.
fn assert_expiry(out: &Out, status: &str) -> Value {
    let d = data_of(out, 10);
    assert_eq!(d["status"], "running", "{d}");
    assert!(
        d.get("woke").is_none(),
        "a bound expiry carries no `woke`: {d}"
    );
    assert_eq!(d["replies"], json!([]), "{d}");
    assert_eq!(d["interrupted"], false, "{d}");
    assert_eq!(d["job"]["status"], status, "{d}");
    assert!(d["session"]["id"].is_string(), "{d}");
    d
}

// ---------------------------------------------------------------------------------------------
// AC-v2-201 on the RPC route: wake, unseen replies, --after, expiry, SIGINT.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac_v2_201_rpc_a_reply_wakes_wait_for_reply_within_one_second() {
    let rb = RpcBox::new();
    let h = held_rpc_job(&rb);
    let mut w = spawn_reply_wait(&rb, &h, &[]);
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        w.running(),
        "nothing was replied yet, so nothing may wake the wait\n{}",
        rb.diag()
    );
    let landed = watch_lines(rb.job_dir(&h.id).join("replies.ndjson"), 1);
    h.release_run();
    let (out, exited) = w.join(scaled(LONG));
    let at = landed
        .recv_timeout(scaled(LONG))
        .expect("the peer appended a reply to replies.ndjson");
    let lag = exited.saturating_duration_since(at);
    eprintln!("rpc reply wake lag: {lag:?}");
    assert!(
        lag <= scaled(ONE_SECOND),
        "the wait returned {lag:?} after the reply landed\n{}",
        out.ctx()
    );
    let disk = replies_on_disk(&rb, &h.id);
    assert!(!disk.is_empty(), "{}", rb.diag());
    let d = data_of(&out, 10);
    let got = arr(&d["replies"]);
    assert!(
        !got.is_empty() && got.len() <= disk.len(),
        "at least one reply, never more than exist: {d}"
    );
    // The unseen replies come back in file order, starting with the first one.
    let want: Vec<&Value> = disk.iter().take(got.len()).collect();
    let d = assert_wake(&out, &h, &want);
    let last = d["replies"].as_array().unwrap().last().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_wait_next(&d, &h.handle, Some(&last), None);
    finish_held(&rb, &h);
}

#[test]
fn ac_v2_201_rpc_replies_already_present_wake_at_once_and_only_unseen_ones_are_returned() {
    let rb = RpcBox::new();
    let (h, disk) = replied_rpc_job(&rb);
    let (r1, r2) = (&disk[0], &disk[1]);
    let (id1, id2) = (r1["id"].as_str().unwrap(), r2["id"].as_str().unwrap());
    assert_ne!(id1, id2);

    // Without --after nothing is seen: both replies come back at once, not at the bound.
    let out = reply_wait(&rb, &h.handle, &["--timeout", "20s"]);
    assert!(
        out.elapsed < scaled(Duration::from_secs(10)),
        "replies already present wake the call at once ({:?})",
        out.elapsed
    );
    let d = assert_wake(&out, &h, &[r1, r2]);
    assert_wait_next(&d, &h.handle, Some(id2), Some(Duration::from_secs(20)));

    // --after skips every reply up to and including the id it names.
    let out = reply_wait(&rb, &h.handle, &["--after", id1, "--timeout", "20s"]);
    assert!(
        out.elapsed < scaled(Duration::from_secs(10)),
        "{}",
        out.ctx()
    );
    let d = assert_wake(&out, &h, &[r2]);
    assert_wait_next(&d, &h.handle, Some(id2), Some(Duration::from_secs(20)));

    // Running `next` after the last reply has nothing unseen: it waits for the bound.
    let out = reply_wait(&rb, &h.handle, &["--after", id2, "--timeout", "700ms"]);
    assert!(
        out.elapsed >= Duration::from_millis(600),
        "nothing is unseen, so the wait lasts to its bound ({:?})",
        out.elapsed
    );
    let d = assert_expiry(&out, "accepted");
    assert_eq!(d["job"]["task_state"], "working", "{d}");
    assert_wait_next(&d, &h.handle, Some(id2), Some(Duration::from_millis(700)));
    finish_held(&rb, &h);
}

#[test]
fn ac_v2_201_rpc_each_wake_returns_the_next_replies_and_none_twice() {
    let rb = RpcBox::new();
    let h = held_rpc_job(&rb);
    let mut seen: Vec<String> = Vec::new();
    let mut after: Option<String> = None;
    h.release_run();
    let want = poll_until("both replies on disk", scaled(LONG), || {
        let r = replies_on_disk(&rb, &h.id);
        (r.len() >= 2).then(|| reply_ids(&json!(r)))
    });
    // Follow `next` the way a caller would until every reply was returned once.
    for _ in 0..3 {
        let mut extra = vec![];
        if let Some(a) = &after {
            extra.extend(["--after", a.as_str()]);
        }
        extra.extend(["--timeout", "700ms"]);
        let out = reply_wait(&rb, &h.handle, &extra);
        let d = data_of(&out, 10);
        if d.get("woke").is_none() {
            break;
        }
        assert_eq!(d["woke"], "reply", "{d}");
        let ids = reply_ids(&d["replies"]);
        assert!(!ids.is_empty(), "{d}");
        for id in &ids {
            assert!(!seen.contains(id), "reply {id} was returned twice: {d}");
        }
        seen.extend(ids);
        after = seen.last().cloned();
    }
    assert_eq!(seen, want, "every reply once, in file order");
    finish_held(&rb, &h);
}

#[test]
fn ac_v2_201_rpc_bound_expiry_has_no_woke_and_a_next_that_keeps_the_caller_flags() {
    let rb = RpcBox::new();
    let h = held_rpc_job(&rb);
    let out = reply_wait(&rb, &h.handle, &["--timeout", "500ms"]);
    assert!(
        out.elapsed >= Duration::from_millis(450) && out.elapsed < scaled(Duration::from_secs(10)),
        "the bound is honoured ({:?})",
        out.elapsed
    );
    let d = assert_expiry(&out, "accepted");
    assert_eq!(d["job"]["task_state"], "working", "{d}");
    assert_wait_next(&d, &h.handle, None, Some(Duration::from_millis(500)));
    // `next` is a command glen accepts as it stands.
    let words = shell_words(d["next"].as_str().unwrap());
    let again = rb.glen(&words[1..]).run();
    assert_expiry(&again, "accepted");
    finish_held(&rb, &h);
}

#[test]
fn ac_v2_201_rpc_plain_wait_ignores_replies_and_keeps_its_documents() {
    let rb = RpcBox::new();
    let (h, _disk) = replied_rpc_job(&rb);
    let out = rb
        .glen(["wait", &h.handle, "--timeout", "600ms", "--json"])
        .run();
    assert!(
        out.elapsed >= Duration::from_millis(550),
        "a plain wait is not woken by a reply ({:?})",
        out.elapsed
    );
    let d = data_of(&out, 10);
    assert_eq!(d["status"], "running", "{d}");
    assert!(d.get("woke").is_none() && d.get("replies").is_none(), "{d}");
    assert_eq!(d["next"], format!("glen wait {}", h.handle), "{d}");
    assert_eq!(d["interrupted"], false, "{d}");
    finish_held(&rb, &h);
}

#[test]
fn ac_v2_201_rpc_sigint_gives_the_expiry_document_with_interrupted_true_and_exit_130() {
    let rb = RpcBox::new();
    let (h, disk) = replied_rpc_job(&rb);
    let last = disk[1]["id"].as_str().unwrap().to_owned();
    let mut w = spawn_reply_wait(&rb, &h, &["--after", &last]);
    await_sigint_handler(w.pid, Duration::from_millis(700));
    assert!(w.running(), "no unseen reply, so the wait keeps waiting");
    let sent = Instant::now();
    w.signal(Signal::INT);
    let (out, exited) = w.join(scaled(LONG));
    assert!(
        exited.duration_since(sent) < scaled(Duration::from_secs(2)),
        "SIGINT ends the wait promptly"
    );
    let d = data_of(&out, 130);
    assert_eq!(d["status"], "running", "{d}");
    assert_eq!(d["interrupted"], true, "{d}");
    assert!(d.get("woke").is_none(), "{d}");
    assert_eq!(d["replies"], json!([]), "{d}");
    assert_wait_next(&d, &h.handle, Some(&last), None);
    // The job keeps running: interrupting an observer stops nothing.
    assert_eq!(rb.status(&h.id)["status"], "accepted");
    finish_held(&rb, &h);
}

#[test]
fn ac_v2_201_rpc_an_after_id_outside_the_jobs_replies_is_usage_unknown_reply() {
    let rb = RpcBox::new();
    // A reply id of another job is no more known to this job than a made-up one.
    let other = rb.completed_job("other");
    let foreign = replies_on_disk(&rb, &other)
        .first()
        .expect("the other job replied")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (h, disk) = replied_rpc_job(&rb);
    let known = disk[0]["id"].as_str().unwrap();
    for bogus in [
        foreign.as_str(),
        "01a19ad3-989d-7070-9273-90556dd5f602",
        "nope",
    ] {
        let out = reply_wait(&rb, &h.handle, &["--after", bogus, "--timeout", "20s"]);
        let err = failed(&out, "usage");
        assert_eq!(err["details"]["reason"], "unknown_reply", "{err}");
        assert_eq!(err["details"]["after"], bogus, "{err}");
        let job = err["details"]["job"].as_str().unwrap_or("");
        assert!(
            job == h.id || job == h.handle,
            "details.job names the job: {err}"
        );
        assert!(
            out.elapsed < scaled(Duration::from_secs(10)),
            "an unknown id is refused at once, never waited on ({:?})",
            out.elapsed
        );
    }
    // A known id is not an error.
    let out = reply_wait(&rb, &h.handle, &["--after", known, "--timeout", "20s"]);
    assert_wake(&out, &h, &[&disk[1]]);
    finish_held(&rb, &h);
}

#[test]
fn ac_v2_201_rpc_a_queued_job_has_no_replies_so_after_is_unknown_and_expiry_reports_queued() {
    let rb = RpcBox::new();
    let lock = rb.hold_glen_lock(A.id);
    let d = data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .run(),
        10,
    );
    assert_eq!(d["job"]["status"], "queued", "{d}");
    let handle = d["job"]["handle"].as_str().unwrap().to_owned();
    let id = job_id(&d);
    let out = reply_wait(
        &rb,
        &handle,
        &["--after", "01a19ad3-989d-7070-9273-90556dd5f602"],
    );
    let err = failed(&out, "usage");
    assert_eq!(err["details"]["reason"], "unknown_reply", "{err}");
    let out = reply_wait(&rb, &handle, &["--timeout", "500ms"]);
    let d = assert_expiry(&out, "queued");
    assert_eq!(d["job"]["task_state"], "submitted", "{d}");
    assert_wait_next(&d, &handle, None, Some(Duration::from_millis(500)));
    drop(lock);
    rb.wait_terminal(&id);
    rb.assert_quiet();
}

#[test]
fn ac_v2_201_after_needs_for_for_takes_only_reply_and_a_missing_job_is_not_found() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["wait", "jabcdef0", "--after", "x", "--json"])
        .run();
    failed(&out, "usage");
    let out = rb
        .glen(["wait", "jabcdef0", "--for", "nope", "--json"])
        .run();
    failed(&out, "usage");
    let out = rb
        .glen(["wait", "jabcdef0", "--for", "reply", "--json"])
        .run();
    failed(&out, "not_found");
    let out = rb
        .glen([
            "wait", "jabcdef0", "--for", "reply", "--after", "x", "--json",
        ])
        .run();
    failed(&out, "not_found");
}

#[test]
fn ac_v2_201_wait_for_reply_never_touches_the_inbox_read_marker() {
    let rb = RpcBox::new();
    let (h, disk) = replied_rpc_job(&rb);
    let want_ids = reply_ids(&json!(disk));
    let out = reply_wait(&rb, &h.handle, &["--timeout", "20s"]);
    assert_wake(&out, &h, &[&disk[0], &disk[1]]);
    assert!(
        !rb.bx.glen_home.join("inbox/.read").exists(),
        "wait wrote an inbox cursor: {:?}",
        list_tree(&rb.bx.glen_home.join("inbox"))
    );
    // The replies are still unread in the inbox.
    let inbox = data_of(&rb.glen(["inbox", "--peek", "--json"]).run(), 0);
    let mine: Vec<String> = arr(&inbox["messages"])
        .iter()
        .filter(|m| m["job_id"] == h.id.as_str())
        .map(|m| m["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        mine, want_ids,
        "the inbox still holds both replies: {inbox}"
    );
    // Reading the inbox marks it; the job's own receiver is independent of that marker.
    let read = data_of(&rb.glen(["inbox", "--json"]).run(), 0);
    assert_eq!(read["marked_read"], true, "{read}");
    let out = reply_wait(&rb, &h.handle, &["--timeout", "20s"]);
    assert_wake(&out, &h, &[&disk[0], &disk[1]]);
    finish_held(&rb, &h);
}

// ---------------------------------------------------------------------------------------------
// AC-v2-201: the job ends first. Exit 0, 6, 7 and 11 each carry every unseen reply.
// ---------------------------------------------------------------------------------------------

/// A failed, aborted or lost outcome of `wait --for reply`: exit 7 `job_failed` (or `code` for
/// exit 6), the job's end in `details`, and `details.replies` holding exactly `want`.
fn assert_ended_with(out: &Out, code: &str, status: &str, state: &str, want: &[&Value]) -> Value {
    let err = failed(out, code);
    let d = &err["details"];
    for k in ["status", "job", "error", "forked", "replies"] {
        assert!(d.get(k).is_some(), "details lack `{k}`: {err}");
    }
    assert_eq!(d["status"], status, "{err}");
    assert_eq!(d["job"]["status"], status, "{err}");
    assert_eq!(d["job"]["task_state"], state, "{err}");
    assert!(
        d["replies"].is_array(),
        "details.replies is an array: {err}"
    );
    assert_replies(&d["replies"], want);
    err
}

#[test]
fn ac_v2_201_rpc_completed_first_returns_exit_0_with_only_the_unseen_replies() {
    let rb = RpcBox::new();
    let (h, disk) = replied_rpc_job(&rb);
    let (r1, r2) = (&disk[0], &disk[1]);
    let (id1, id2) = (r1["id"].as_str().unwrap(), r2["id"].as_str().unwrap());
    // Nothing is unseen, so the end of the job overtakes the wait.
    let mut w = spawn_reply_wait(&rb, &h, &["--after", id2]);
    std::thread::sleep(Duration::from_millis(500));
    assert!(w.running(), "no unseen reply and no end yet");
    h.release_result();
    let (out, _) = w.join(scaled(LONG));
    let d = data_of(&out, 0);
    for k in COMPLETED_KEYS {
        assert!(d.get(k).is_some(), "completed data lacks `{k}`: {d}");
    }
    assert_eq!(d["status"], "completed", "{d}");
    assert!(
        d.get("woke").is_none(),
        "a terminal document has no `woke`: {d}"
    );
    assert_eq!(d["replies"], json!([]), "only unseen replies: {d}");
    assert_eq!(d["job"]["task_state"], "completed", "{d}");
    assert!(d["forked"].is_null(), "{d}");
    rb.wait_terminal(&h.id);
    // The job is over: the end wins over any unseen reply, and the document carries them.
    let d = data_of(&reply_wait(&rb, &h.handle, &[]), 0);
    assert_eq!(d["status"], "completed", "{d}");
    assert!(d.get("woke").is_none(), "{d}");
    assert_replies(&d["replies"], &[r1, r2]);
    let d = data_of(&reply_wait(&rb, &h.handle, &["--after", id1]), 0);
    assert_replies(&d["replies"], &[r2]);
    let d = data_of(&reply_wait(&rb, &h.handle, &["--after", id2]), 0);
    assert_eq!(d["replies"], json!([]), "{d}");
    // A plain wait is unchanged: every reply, as before.
    let d = data_of(&rb.glen(["wait", &h.handle, "--json"]).run(), 0);
    assert_replies(&d["replies"], &[r1, r2]);
    // Human output prints only the unseen replies.
    let human = rb
        .glen([
            "wait", &h.handle, "--for", "reply", "--after", id1, "--human",
        ])
        .run();
    human.assert_exit(0);
    assert!(
        human.out().contains(r2["text"].as_str().unwrap()),
        "{}",
        human.ctx()
    );
    assert_eq!(human.out().matches('↩').count(), 1, "{}", human.ctx());
    let human = rb
        .glen(["wait", &h.handle, "--for", "reply", "--human"])
        .run();
    human.assert_exit(0);
    assert_eq!(human.out().matches('↩').count(), 2, "{}", human.ctx());
    rb.assert_quiet();
}

#[test]
fn ac_v2_201_rpc_aborted_first_returns_exit_7_with_the_unseen_replies_in_the_details() {
    let rb = RpcBox::new();
    let (h, disk) = replied_rpc_job(&rb);
    let (r1, r2) = (&disk[0], &disk[1]);
    let (id1, id2) = (r1["id"].as_str().unwrap(), r2["id"].as_str().unwrap());
    let worker = rb.status(&h.id)["worker_pid"]
        .as_i64()
        .expect("a worker pid");
    let mut w = spawn_reply_wait(&rb, &h, &["--after", id2]);
    std::thread::sleep(Duration::from_millis(500));
    assert!(w.running());
    signal(worker, Signal::TERM);
    let (out, _) = w.join(scaled(LONG));
    let err = assert_ended_with(&out, "job_failed", "aborted", "canceled", &[]);
    assert_eq!(err["details"]["error"]["code"], "cancelled", "{err}");
    rb.wait_terminal(&h.id);
    assert_ended_with(
        &reply_wait(&rb, &h.handle, &[]),
        "job_failed",
        "aborted",
        "canceled",
        &[r1, r2],
    );
    assert_ended_with(
        &reply_wait(&rb, &h.handle, &["--after", id1]),
        "job_failed",
        "aborted",
        "canceled",
        &[r2],
    );
    // Plain wait keeps today's envelope, without the new key.
    let plain = rb.glen(["wait", &h.handle, "--json"]).run();
    let err = failed(&plain, "job_failed");
    assert!(err["details"].get("replies").is_none(), "{err}");
    assert_eq!(err["details"]["job"]["task_state"], "canceled", "{err}");
    rb.assert_quiet();
}

#[test]
fn ac_v2_201_rpc_failed_first_returns_exit_7_with_the_unseen_replies_in_the_details() {
    let rb = RpcBox::new();
    let (h, disk) = replied_rpc_job(&rb);
    let (r1, r2) = (&disk[0], &disk[1]);
    // The omp process dies under the running job.
    let omp = rb.starts()[0]["pid"].as_i64().expect("the fake omp's pid");
    signal(omp, Signal::KILL);
    let st = rb.wait_terminal(&h.id);
    assert_eq!(st["status"], "failed", "{st}");
    let err = assert_ended_with(
        &reply_wait(&rb, &h.handle, &[]),
        "job_failed",
        "failed",
        "failed",
        &[r1, r2],
    );
    assert_ne!(err["details"]["error"]["code"], "no_route", "{err}");
    assert_ended_with(
        &reply_wait(&rb, &h.handle, &["--after", r1["id"].as_str().unwrap()]),
        "job_failed",
        "failed",
        "failed",
        &[r2],
    );
    rb.assert_quiet();
}

#[test]
fn ac_v2_201_rpc_lost_first_returns_exit_7_with_the_unseen_replies_in_the_details() {
    let rb = RpcBox::new();
    let (h, disk) = replied_rpc_job(&rb);
    let worker = rb.status(&h.id)["worker_pid"]
        .as_i64()
        .expect("a worker pid");
    signal(worker, Signal::KILL);
    for s in rb.starts() {
        let pgid = s["pgid"].as_i64().unwrap_or(0);
        if pgid > 1 && pgid != own_pgid() {
            signal_group(pgid, Signal::KILL);
        }
    }
    poll_until("the worker to be gone", scaled(LONG), || {
        (!pid_alive(worker)).then_some(())
    });
    let out = reply_wait(&rb, &h.handle, &["--timeout", "20s"]);
    assert_ended_with(&out, "job_failed", "lost", "failed", &[&disk[0], &disk[1]]);
    rb.assert_quiet();
}

#[test]
fn ac_v2_201_rpc_a_worker_side_refusal_returns_exit_6_with_an_empty_unseen_list() {
    let rb = RpcBox::new();
    let lock = rb.hold_glen_lock(A.id);
    let d = data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .run(),
        10,
    );
    assert_eq!(d["job"]["status"], "queued", "{d}");
    let handle = d["job"]["handle"].as_str().unwrap().to_owned();
    let id = job_id(&d);
    let _lease = hold_lease(&rb.bx.owners(), A.id);
    drop(lock);
    let out = reply_wait(&rb, &handle, &["--timeout", "20s"]);
    let err = assert_ended_with(&out, "no_route", "failed", "rejected", &[]);
    assert_eq!(err["details"]["error"]["code"], "no_route", "{err}");
    rb.wait_terminal(&id);
    rb.assert_quiet();
    assert!(rb.starts().is_empty(), "omp was spawned\n{}", rb.diag());
}

#[test]
fn ac_v2_201_rpc_forked_first_returns_exit_11_with_the_unseen_replies() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hello", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "fork_reply")
        .run();
    let d = data_of(&out, 11);
    let id = job_id(&d);
    let disk = replies_on_disk(&rb, &id);
    assert!(!disk.is_empty(), "the fork scenario replied\n{}", rb.diag());
    let handle = job_handle(&id);
    let d = data_of(&reply_wait(&rb, &handle, &[]), 11);
    assert_eq!(d["status"], "completed", "{d}");
    assert!(d["forked"].is_object(), "{d}");
    assert!(d.get("woke").is_none(), "{d}");
    assert_eq!(d["job"]["task_state"], "completed", "{d}");
    assert_replies(&d["replies"], &disk.iter().collect::<Vec<_>>());
    let last = disk.last().unwrap()["id"].as_str().unwrap();
    let d = data_of(&reply_wait(&rb, &handle, &["--after", last]), 11);
    assert_eq!(d["replies"], json!([]), "{d}");
    rb.assert_quiet();
}

#[test]
fn ac_v2_201_rpc_a_reply_racing_the_end_of_the_job_is_never_lost_or_repeated() {
    // Replies and the result arrive back to back, so a wait sees either the wake or the end,
    // or the wake and then the end. Following `next` must return every reply exactly once.
    let rb = RpcBox::new();
    for round in 0..5 {
        let run = rb.bx.root.join(format!("run-{round}"));
        let out = rb
            .glen(["send", A.handle, "hello", "--no-wait", "--json"])
            .env("FAKE_OMP_SCENARIO", "progress")
            .env("FAKE_OMP_RUN_RELEASE", &run)
            .run();
        let id = job_id(&data_of(&out, 10));
        rb.wait_status(&id, "accepted", scaled(LONG), |s| s["status"] == "accepted");
        let handle = job_handle(&id);
        let args = ["wait", handle.as_str(), "--for", "reply", "--json"];
        let mut w = Bg::spawn(rb.command(&args), "wait --for reply (race)");
        std::thread::sleep(Duration::from_millis(300));
        assert!(w.running());
        fs::write(&run, "").unwrap();
        let mut seen: Vec<String> = Vec::new();
        let (mut out, _) = w.join(scaled(LONG));
        for _ in 0..4 {
            let code = out.code();
            let d = out.json()["data"].clone();
            let ids = reply_ids(&d["replies"]);
            for id in &ids {
                assert!(
                    !seen.contains(id),
                    "round {round}: {id} twice\n{}",
                    out.ctx()
                );
            }
            seen.extend(ids);
            if code == 0 {
                assert_eq!(d["status"], "completed", "{d}");
                assert!(d.get("woke").is_none(), "{d}");
                break;
            }
            assert_eq!(code, 10, "{}", out.ctx());
            assert_eq!(
                d["woke"], "reply",
                "round {round}: a bound cannot have passed: {d}"
            );
            let last = seen.last().unwrap().clone();
            out = reply_wait(&rb, &handle, &["--after", &last, "--timeout", "20s"]);
        }
        rb.wait_terminal(&id);
        let disk = reply_ids(&json!(replies_on_disk(&rb, &id)));
        assert_eq!(seen, disk, "round {round}: every reply once, in order");
    }
    rb.assert_quiet();
}

#[test]
fn ac_v2_201_rpc_an_unknown_after_is_refused_even_after_the_job_ended() {
    let rb = RpcBox::new();
    let (h, disk) = replied_rpc_job(&rb);
    finish_held(&rb, &h);
    let out = reply_wait(&rb, &h.handle, &["--after", "no-such-reply"]);
    let err = failed(&out, "usage");
    assert_eq!(err["details"]["reason"], "unknown_reply", "{err}");
    let known = disk[0]["id"].as_str().unwrap();
    let d = data_of(&reply_wait(&rb, &h.handle, &["--after", known]), 0);
    assert_replies(&d["replies"], &[&disk[1]]);
}

#[test]
fn ac_v2_201_rpc_a_large_wake_spills_and_the_spilled_file_holds_woke_and_next() {
    let rb = RpcBox::new();
    let (h, disk) = replied_rpc_job(&rb);
    let out = rb
        .glen([
            "wait",
            &h.handle,
            "--for",
            "reply",
            "--timeout",
            "20s",
            "--json",
        ])
        .env("GLEN_MAX_OUTPUT", "500")
        .run();
    out.assert_exit(10);
    let env = out.json();
    assert_eq!(env["ok"], true, "{env}");
    assert!(
        env.get("data").is_none(),
        "a spilled success has no data: {env}"
    );
    let spilled = &env["spilled"];
    assert_eq!(spilled["format"], "json", "{env}");
    let path = PathBuf::from(spilled["path"].as_str().expect("spilled.path"));
    let bytes = fs::read(&path).expect("the spilled file");
    assert_eq!(spilled["bytes"], bytes.len(), "{env}");
    let full: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(full["ok"], true, "{full}");
    let d = &full["data"];
    assert_eq!(d["woke"], "reply", "{full}");
    assert_replies(&d["replies"], &[&disk[0], &disk[1]]);
    assert_wait_next(
        d,
        &h.handle,
        Some(disk[1]["id"].as_str().unwrap()),
        Some(Duration::from_secs(20)),
    );
    finish_held(&rb, &h);
}

#[test]
fn ac_v2_201_rpc_a_reply_line_is_not_returned_until_its_newline_has_landed() {
    use std::io::Write;
    let rb = RpcBox::new();
    let h = held_rpc_job(&rb);
    let rec = json!({
        "v": 1,
        "id": "01a19ad3-9832-7013-a6b8-266395477783",
        "session_id": A.id,
        "job_id": h.id,
        "request_id": null,
        "tool_call_id": "call_torn",
        "text": "half written",
        "final": false,
        "at": jiff::Timestamp::now().to_string(),
    });
    let line = rec.to_string();
    let path = rb.job_dir(&h.id).join("replies.ndjson");
    let append = |bytes: &[u8]| {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(bytes).unwrap();
        f.sync_all().unwrap();
    };
    // Half a line, then a whole JSON object with no newline: neither is a complete reply yet.
    append(&line.as_bytes()[..40]);
    let out = reply_wait(&rb, &h.handle, &["--timeout", "600ms"]);
    assert_expiry(&out, "accepted");
    append(&line.as_bytes()[40..]);
    let out = reply_wait(&rb, &h.handle, &["--timeout", "600ms"]);
    assert_expiry(&out, "accepted");
    // The newline lands: the wait wakes within a second with exactly that reply.
    let mut w = spawn_reply_wait(&rb, &h, &[]);
    std::thread::sleep(Duration::from_millis(500));
    assert!(w.running(), "an unterminated line must not wake the wait");
    append(b"\n");
    let landed = Instant::now();
    let (out, exited) = w.join(scaled(LONG));
    assert!(
        exited.duration_since(landed) <= scaled(ONE_SECOND),
        "{}",
        out.ctx()
    );
    assert_wake(&out, &h, &[&rec]);
    finish_held(&rb, &h);
}

// ---------------------------------------------------------------------------------------------
// AC-v2-201 on the live route (`fake_ext` `progress` with the hold knobs).
// ---------------------------------------------------------------------------------------------

/// `fake_ext` `progress` held before its first reply: the live job is `accepted`.
fn held_live_job(lb: &LiveBox) -> (Ext, HeldJob, Value) {
    let run = lb.bx.root.join("run-release");
    let result = lb.bx.root.join("result-release");
    let (run_s, result_s) = (s(&run), s(&result));
    let ext = lb.start_ext(
        A.id,
        "progress",
        &[
            ("FAKE_EXT_RUN_RELEASE", run_s.as_str()),
            ("FAKE_EXT_RESULT_RELEASE", result_s.as_str()),
        ],
        &[],
    );
    let send = data_of(&lb.send(A.handle, "hi", &["--no-wait"]).run(), 10);
    let id = job_id(&send);
    lb.wait_status(&id, "accepted", scaled(LONG), |s| s["status"] == "accepted");
    (ext, HeldJob::new(id, run, result), send)
}

#[test]
fn ac_v2_201_live_a_reply_wakes_wait_for_reply_within_one_second() {
    let lb = LiveBox::new();
    let (_ext, h, _) = held_live_job(&lb);
    let mut w = spawn_reply_wait(&lb, &h, &[]);
    std::thread::sleep(Duration::from_millis(500));
    assert!(w.running(), "nothing replied yet\n{}", lb.diag());
    let landed = watch_lines(lb.job_dir(&h.id).join("replies.ndjson"), 1);
    h.release_run();
    let (out, exited) = w.join(scaled(LONG));
    let at = landed
        .recv_timeout(scaled(LONG))
        .expect("the live job recorded a reply");
    let lag = exited.saturating_duration_since(at);
    eprintln!("live reply wake lag: {lag:?}");
    assert!(
        lag <= scaled(ONE_SECOND),
        "the wait returned {lag:?} after the reply landed\n{}",
        out.ctx()
    );
    let disk = replies_on_disk(&lb, &h.id);
    let d = data_of(&out, 10);
    let n = arr(&d["replies"]).len();
    assert!(n >= 1 && n <= disk.len(), "{d}");
    let want: Vec<&Value> = disk.iter().take(n).collect();
    let d = assert_wake(&out, &h, &want);
    assert_eq!(d["job"]["route"], "live", "{d}");
    let last = want.last().unwrap()["id"].as_str().unwrap();
    assert_wait_next(&d, &h.handle, Some(last), None);
    finish_held(&lb, &h);
}

#[test]
fn ac_v2_201_live_after_skips_earlier_replies_and_the_end_of_the_job_wins() {
    let lb = LiveBox::new();
    let (_ext, h, _) = held_live_job(&lb);
    h.release_run();
    let disk = poll_until("two live replies", scaled(LONG), || {
        let r = replies_on_disk(&lb, &h.id);
        (r.len() >= 2).then_some(r)
    });
    let (r1, r2) = (&disk[0], &disk[1]);
    let (id1, id2) = (r1["id"].as_str().unwrap(), r2["id"].as_str().unwrap());
    let out = reply_wait(&lb, &h.handle, &["--timeout", "20s"]);
    assert_wake(&out, &h, &[r1, r2]);
    let out = reply_wait(&lb, &h.handle, &["--after", id1, "--timeout", "20s"]);
    assert_wake(&out, &h, &[r2]);
    // Nothing is unseen after the last reply: the wait lasts to its bound, then the job ends.
    let out = reply_wait(&lb, &h.handle, &["--after", id2, "--timeout", "600ms"]);
    assert!(out.elapsed >= Duration::from_millis(550), "{}", out.ctx());
    assert_expiry(&out, "accepted");
    let mut w = spawn_reply_wait(&lb, &h, &["--after", id2]);
    std::thread::sleep(Duration::from_millis(500));
    assert!(w.running());
    h.release_result();
    let (out, _) = w.join(scaled(LONG));
    let d = data_of(&out, 0);
    assert_eq!(d["status"], "completed", "{d}");
    assert_eq!(d["replies"], json!([]), "{d}");
    assert_eq!(d["job"]["route"], "live", "{d}");
    assert_eq!(d["job"]["task_state"], "completed", "{d}");
    lb.wait_terminal(&h.id);
    let d = data_of(&reply_wait(&lb, &h.handle, &["--after", id1]), 0);
    assert_replies(&d["replies"], &[r2]);
    // The inbox still holds both replies: wait never consumed them.
    assert!(!lb.glen_home().join("inbox/.read").exists());
    let inbox = data_of(&lb.glen(["inbox", "--peek", "--json"]).run(), 0);
    let mine: Vec<String> = arr(&inbox["messages"])
        .iter()
        .filter(|m| m["job_id"] == h.id.as_str())
        .map(|m| m["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(mine, [id1, id2], "{inbox}");
    lb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-v2-203: `task_state` in every job view, for every row of the §A7 table.
// ---------------------------------------------------------------------------------------------

fn design() -> String {
    fs::read_to_string(repo_root().join("docs/design.md")).expect("docs/design.md")
}

/// Body lines of design section `## §A<n> …` (up to the next `## ` heading), fenced code dropped.
fn section(doc: &str, n: u32) -> Vec<String> {
    let mut in_fence = false;
    let mut lines = Vec::new();
    for l in doc.lines() {
        if l.trim_start().starts_with("```") {
            in_fence = !in_fence;
        } else if !in_fence {
            lines.push(l.to_owned());
        }
    }
    let start = lines
        .iter()
        .position(|l| l.starts_with(&format!("## §A{n} ")))
        .unwrap_or_else(|| panic!("docs/design.md has no §A{n}"));
    lines[start + 1..]
        .iter()
        .take_while(|l| !l.starts_with("## "))
        .cloned()
        .collect()
}

fn cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|c| c.trim().to_owned())
        .collect()
}

fn plain(cell: &str) -> String {
    cell.replace('`', "").trim().to_owned()
}

/// The pipe table of `lines` whose header satisfies `pick`: (header cells, rows of cells).
fn table_where(
    lines: &[String],
    pick: impl Fn(&[String]) -> bool,
) -> (Vec<String>, Vec<Vec<String>>) {
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].trim_start().starts_with('|') {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && lines[i].trim_start().starts_with('|') {
            i += 1;
        }
        let run = &lines[start..i];
        let header = cells(&run[0]);
        let sep = run.get(1).map(|l| cells(l)).unwrap_or_default();
        let is_sep = !sep.is_empty()
            && sep
                .iter()
                .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')));
        if is_sep && pick(&header) {
            return (header, run[2..].iter().map(|l| cells(l)).collect());
        }
    }
    panic!("no matching table in:\n{}", lines.join("\n"));
}

/// §A7's `task_state` table as (glen status, condition cell, task_state).
fn task_state_rows() -> Vec<(String, String, String)> {
    let a7 = section(&design(), 7);
    let (_, rows) = table_where(&a7, |h| h.iter().any(|c| plain(c).contains("task_state")));
    rows.into_iter()
        .map(|r| {
            assert!(r.len() >= 3, "task_state row: {r:?}");
            (plain(&r[0]), r[1].clone(), plain(&r[r.len() - 1]))
        })
        .collect()
}

/// The `task_state` §A7 assigns to a job with `status` and (for failed jobs) error `code`.
fn expected_state(status: &str, code: Option<&str>) -> String {
    let rows: Vec<_> = task_state_rows()
        .into_iter()
        .filter(|r| r.0 == status && r.2 != "input_required")
        .collect();
    assert!(!rows.is_empty(), "§A7 has no row for status {status}");
    if rows.len() == 1 {
        return rows[0].2.clone();
    }
    let pick = rows
        .iter()
        .find(|r| r.1.contains("no_route") == (code == Some("no_route")));
    pick.unwrap_or_else(|| panic!("no §A7 row for {status}/{code:?}: {rows:?}"))
        .2
        .clone()
}

/// The statuses the drivers below reach; a new status row in §A7 must get a driver.
const DRIVEN: [&str; 6] = [
    "queued",
    "accepted",
    "completed",
    "aborted",
    "failed",
    "lost",
];

#[test]
fn ac_v2_203_the_a7_table_names_only_statuses_this_suite_drives_and_every_driven_one() {
    let rows = task_state_rows();
    let statuses: BTreeSet<String> = rows
        .iter()
        .filter(|r| r.2 != "input_required")
        .map(|r| r.0.clone())
        .collect();
    let driven: BTreeSet<String> = DRIVEN.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(statuses, driven, "§A7 task_state rows: {rows:#?}");
    // `input_required` is reserved for ACP (W3): no job status maps to it.
    assert!(rows.iter().any(|r| r.2 == "input_required"), "{rows:#?}");
}

/// The job view of a document: `data.job` of a success, `error.details.job` of a failure.
fn view_of(out: &Out) -> Value {
    let env = out.json();
    let job = if env["ok"] == true {
        env["data"]["job"].clone()
    } else {
        env["error"]["details"]["job"].clone()
    };
    assert!(job.is_object(), "no job view in {env}");
    job
}

fn assert_view(view: &Value, status: &str, code: Option<&str>, what: &str) {
    assert_eq!(view["status"], status, "{what}: {view}");
    let want = expected_state(status, code);
    assert_eq!(view["task_state"], want.as_str(), "{what}: {view}");
}

/// `wait`, `wait --for reply` and `jobs` all report the §A7 `task_state` of job `id`.
fn assert_state_in_every_view(rb: &RpcBox, id: &str, status: &str, code: Option<&str>) {
    let handle = job_handle(id);
    let wait = rb
        .glen(["wait", &handle, "--timeout", "400ms", "--json"])
        .run();
    assert_view(&view_of(&wait), status, code, "wait");
    let reply = rb
        .glen([
            "wait",
            &handle,
            "--for",
            "reply",
            "--timeout",
            "400ms",
            "--json",
        ])
        .run();
    assert_view(&view_of(&reply), status, code, "wait --for reply");
    let jobs = data_of(&rb.glen(["jobs", "--json"]).run(), 0);
    let rows = arr(&jobs["jobs"]);
    assert!(!rows.is_empty(), "{jobs}");
    for r in rows {
        assert!(
            r["task_state"].is_string(),
            "every `jobs` row carries task_state: {r}"
        );
    }
    let row = rows
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("job {id} is not in `jobs`: {jobs}"));
    assert_view(row, status, code, "jobs");
}

#[test]
fn ac_v2_203_queued_is_submitted_in_send_wait_and_jobs() {
    let rb = RpcBox::new();
    let lock = rb.hold_glen_lock(A.id);
    let out = rb
        .glen(["send", A.handle, "hi", "--no-wait", "--json"])
        .run();
    data_of(&out, 10);
    let view = view_of(&out);
    assert_view(&view, "queued", None, "send");
    let id = view["id"].as_str().unwrap().to_owned();
    assert_state_in_every_view(&rb, &id, "queued", None);
    drop(lock);
    rb.wait_terminal(&id);
    rb.assert_quiet();
}

#[test]
fn ac_v2_203_accepted_is_working_in_send_wait_and_jobs() {
    let rb = RpcBox::new();
    let h = held_rpc_job(&rb);
    assert_state_in_every_view(&rb, &h.id, "accepted", None);
    finish_held(&rb, &h);
    // A `send --no-wait` document is whichever of queued or accepted the job was at that moment.
    let out = rb
        .glen(["send", A.handle, "again", "--no-wait", "--json"])
        .run();
    data_of(&out, 10);
    let view = view_of(&out);
    let status = view["status"].as_str().unwrap().to_owned();
    assert_view(&view, &status, None, "send --no-wait");
    rb.wait_terminal(view["id"].as_str().unwrap());
    rb.assert_quiet();
}

#[test]
fn ac_v2_203_completed_is_completed_in_send_wait_and_jobs_forked_too() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .run();
    data_of(&out, 0);
    let view = view_of(&out);
    assert_view(&view, "completed", None, "send");
    assert_state_in_every_view(&rb, view["id"].as_str().unwrap(), "completed", None);
    rb.assert_quiet();

    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "fork")
        .run();
    let d = data_of(&out, 11);
    assert!(d["forked"].is_object(), "{d}");
    let view = view_of(&out);
    assert_view(&view, "completed", None, "send (forked)");
    let id = view["id"].as_str().unwrap();
    let wait = rb.glen(["wait", &job_handle(id), "--json"]).run();
    data_of(&wait, 11);
    assert_view(&view_of(&wait), "completed", None, "wait (forked)");
    rb.assert_quiet();
}

#[test]
fn ac_v2_203_aborted_is_canceled_whichever_side_stopped_the_job() {
    let rb = RpcBox::new();
    // omp aborted the prompt itself: error code `aborted`.
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "aborted")
        .run();
    let err = failed(&out, "job_failed");
    assert_eq!(err["details"]["status"], "aborted", "{err}");
    let code = err["details"]["error"]["code"].as_str().unwrap().to_owned();
    let view = view_of(&out);
    assert_view(&view, "aborted", Some(&code), "send");
    assert_state_in_every_view(&rb, view["id"].as_str().unwrap(), "aborted", Some(&code));
    rb.assert_quiet();

    // A caller stop: the worker's signal ends the job with `cancelled`.
    let h = held_rpc_job(&rb);
    let worker = rb.status(&h.id)["worker_pid"]
        .as_i64()
        .expect("a worker pid");
    signal(worker, Signal::TERM);
    let st = rb.wait_terminal(&h.id);
    assert_eq!(st["status"], "aborted", "{st}");
    assert_eq!(st["error"]["code"], "cancelled", "{st}");
    assert_state_in_every_view(&rb, &h.id, "aborted", Some("cancelled"));
    rb.assert_quiet();
}

#[test]
fn ac_v2_203_failed_is_failed_unless_the_code_is_no_route_which_is_rejected() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "error")
        .run();
    let err = failed(&out, "job_failed");
    let code = err["details"]["error"]["code"].as_str().unwrap().to_owned();
    assert_ne!(code, "no_route", "{err}");
    let view = view_of(&out);
    assert_view(&view, "failed", Some(&code), "send");
    assert_eq!(view["task_state"], "failed", "{view}");
    assert_state_in_every_view(&rb, view["id"].as_str().unwrap(), "failed", Some(&code));
    rb.assert_quiet();

    // A worker-side refusal is `no_route`: exit 6, task_state `rejected`.
    let lock = rb.hold_glen_lock(A.id);
    let d = data_of(
        &rb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .run(),
        10,
    );
    let id = job_id(&d);
    let _lease = hold_lease(&rb.bx.owners(), A.id);
    drop(lock);
    let wait = rb
        .glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
        .run();
    failed(&wait, "no_route");
    let view = view_of(&wait);
    assert_view(&view, "failed", Some("no_route"), "wait");
    assert_eq!(view["task_state"], "rejected", "{view}");
    assert_state_in_every_view(&rb, &id, "failed", Some("no_route"));
    rb.assert_quiet();
}

#[test]
fn ac_v2_203_lost_is_failed_and_a_failed_job_without_an_error_record_is_failed() {
    let rb = RpcBox::new();
    let h = held_rpc_job(&rb);
    let worker = rb.status(&h.id)["worker_pid"]
        .as_i64()
        .expect("a worker pid");
    signal(worker, Signal::KILL);
    for st in rb.starts() {
        let pgid = st["pgid"].as_i64().unwrap_or(0);
        if pgid > 1 && pgid != own_pgid() {
            signal_group(pgid, Signal::KILL);
        }
    }
    poll_until("the worker to be gone", scaled(LONG), || {
        (!pid_alive(worker)).then_some(())
    });
    let wait = rb
        .glen(["wait", &h.handle, "--timeout", "20s", "--json"])
        .run();
    failed(&wait, "job_failed");
    assert_view(&view_of(&wait), "lost", None, "wait");
    assert_state_in_every_view(&rb, &h.id, "lost", None);
    rb.assert_quiet();

    // A failed job with no error record (a hand-written or older status.json) is `failed`.
    let template = rb.completed_job("template");
    let crafted = job_uuid(
        ms_since_epoch(std::time::SystemTime::now()) + 1000,
        0xabc1234,
    );
    rb.clone_job(
        &template,
        &crafted,
        std::time::SystemTime::now(),
        json!({"status": "failed", "error": null}),
        true,
    );
    let jobs = data_of(&rb.glen(["jobs", "--json"]).run(), 0);
    let row = arr(&jobs["jobs"])
        .iter()
        .find(|r| r["id"] == crafted.as_str())
        .unwrap_or_else(|| panic!("the crafted job is listed: {jobs}"));
    assert_view(row, "failed", None, "jobs (no error record)");
}

#[test]
fn ac_v2_203_live_jobs_carry_task_state_too_including_a_session_switch() {
    let lb = LiveBox::new();
    let (_ext, h, send) = held_live_job(&lb);
    let status = send["job"]["status"].as_str().unwrap();
    assert_view(&send["job"], status, None, "live send");
    assert_state_in_every_view(&lb, &h.id, "accepted", None);
    finish_held(&lb, &h);
    assert_state_in_every_view(&lb, &h.id, "completed", None);
    drop(_ext);

    let lb = LiveBox::new();
    let _ext = lb.start_ext(A.id, "switched", &[], &[]);
    let out = lb.send(A.handle, "hi", &["--wait", "20s"]).run();
    let err = failed(&out, "job_failed");
    assert_eq!(err["details"]["status"], "aborted", "{err}");
    let code = err["details"]["error"]["code"].as_str().unwrap().to_owned();
    assert_eq!(code, "session_switched", "{err}");
    let view = view_of(&out);
    assert_view(&view, "aborted", Some(&code), "live send (switched)");
    assert_state_in_every_view(&lb, view["id"].as_str().unwrap(), "aborted", Some(&code));
    lb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-v2-202: `inbox --wait [--timeout]`.
// ---------------------------------------------------------------------------------------------

fn inbox_file_of(rb: &RpcBox, session: &str) -> PathBuf {
    rb.bx.glen_home.join(format!("inbox/{session}.ndjson"))
}

fn cursor_of(rb: &RpcBox, session: &str) -> PathBuf {
    rb.bx.glen_home.join(format!("inbox/.read/{session}"))
}

/// Append one §A5 inbox record for `session` (as the extension would) and return it.
fn put(rb: &RpcBox, session: &str, text: &str) -> Value {
    let rec = inbox_record(session, None, text, &jiff::Timestamp::now().to_string());
    append_record(&inbox_file_of(rb, session), &rec);
    rec
}

fn spawn_inbox(rb: &RpcBox, args: &[&str]) -> Bg {
    let mut a = vec!["inbox", "--wait"];
    a.extend_from_slice(args);
    a.push("--json");
    Bg::spawn(rb.command(&a), &a.join(" "))
}

fn inbox_wait(rb: &RpcBox, args: &[&str]) -> Out {
    let mut a = vec!["inbox", "--wait"];
    a.extend_from_slice(args);
    a.push("--json");
    rb.glen(a).run()
}

/// An `inbox --wait` bound document: exit `exit` (10, or 130 when interrupted), no message,
/// nothing marked read, and a `next` command.
fn assert_inbox_bound(out: &Out, exit: i32) -> Value {
    let d = data_of(out, exit);
    assert_eq!(d["total"], 0, "{d}");
    assert_eq!(d["messages"], json!([]), "{d}");
    assert_eq!(d["marked_read"], false, "{d}");
    assert!(d["skipped_lines"].is_u64(), "{d}");
    assert_eq!(d["interrupted"], exit == 130, "{d}");
    assert!(d["next"].is_string(), "{d}");
    d
}

/// `data.next` of an `inbox --wait` bound document, split into words.
fn inbox_next(d: &Value) -> Vec<String> {
    let next = d["next"].as_str().expect("data.next");
    assert!(!next.contains('\n'), "{next:?}");
    let w = shell_words(next);
    assert_eq!(
        w.iter().take(3).map(String::as_str).collect::<Vec<_>>(),
        ["glen", "inbox", "--wait"],
        "next: {next:?}"
    );
    w
}

/// The text of the option block of `flag` in clap help (the option line plus its wrapped lines).
fn help_option(help: &str, flag: &str) -> String {
    let lines: Vec<&str> = help.lines().collect();
    let start = lines
        .iter()
        .position(|l| {
            let t = l.trim_start();
            let t = t.strip_prefix("-h, ").unwrap_or(t);
            let t = t
                .split_once(", ")
                .map_or(t, |(a, b)| if a.len() <= 2 { b } else { t });
            t.split([' ', '=']).next() == Some(flag)
        })
        .unwrap_or_else(|| panic!("no `{flag}` option in help:\n{help}"));
    let mut block = lines[start].to_owned();
    for l in &lines[start + 1..] {
        if l.starts_with("        ") && !l.trim().is_empty() {
            block.push(' ');
            block.push_str(l.trim());
        } else {
            break;
        }
    }
    block
}

#[test]
fn ac_v2_202_help_documents_wait_and_timeout_with_a_90s_default() {
    let sb = Sandbox::new();
    let help = sb
        .glen(["inbox", "--help"])
        .env("COLUMNS", "200")
        .env("NO_COLOR", "1")
        .run();
    help.assert_exit(0).assert_no_ansi();
    let help = help.out();
    assert!(help_option(&help, "--wait").contains("--wait"));
    let timeout = help_option(&help, "--timeout");
    assert!(timeout.contains("[default: 90s]"), "{timeout}");
    assert!(
        timeout.contains("24h"),
        "the 24 h maximum is documented: {timeout}"
    );

    let wait = sb
        .glen(["wait", "--help"])
        .env("COLUMNS", "200")
        .env("NO_COLOR", "1")
        .run();
    wait.assert_exit(0);
    let wait = wait.out();
    assert!(
        help_option(&wait, "--timeout").contains("[default: 90s]"),
        "{wait}"
    );
    let for_reply = help_option(&wait, "--for");
    assert!(for_reply.contains("reply"), "{for_reply}");
    assert!(
        for_reply.contains("[possible values: reply]"),
        "`reply` is the only value: {for_reply}"
    );
    assert!(help_option(&wait, "--after").contains("--after"));
}

#[test]
fn ac_v2_202_timeout_is_capped_at_24h_and_needs_wait_and_wait_conflicts_with_follow_and_all() {
    let rb = RpcBox::new();
    for args in [
        &["inbox", "--wait", "--timeout", "25h", "--json"][..],
        &["inbox", "--wait", "--timeout", "86401s", "--json"][..],
        &["inbox", "--timeout", "30s", "--json"][..],
        &["inbox", "--wait", "-f", "--json"][..],
        &["inbox", "--wait", "--all", "--json"][..],
    ] {
        let out = rb.glen(args).run();
        failed(&out, "usage");
        assert!(
            out.elapsed < scaled(Duration::from_secs(10)),
            "{args:?} is refused at once"
        );
    }
    assert!(
        !rb.bx.glen_home.exists(),
        "a refused command creates no state"
    );
    // The cap itself is allowed: with a message already unread the call returns at once.
    put(&rb, A.id, "here");
    let out = inbox_wait(&rb, &["--timeout", "24h", "--peek"]);
    let d = data_of(&out, 0);
    assert_eq!(d["total"], 1, "{d}");
}

#[test]
fn ac_v2_202_an_unread_message_returns_at_once_with_the_normal_document_and_is_marked_read() {
    let rb = RpcBox::new();
    let one = put(&rb, A.id, "one");
    let two = put(&rb, A.id, "two");
    let out = inbox_wait(&rb, &["--timeout", "20s"]);
    assert!(
        out.elapsed < scaled(Duration::from_secs(10)),
        "unread messages do not wait for the bound ({:?})",
        out.elapsed
    );
    let d = data_of(&out, 0);
    assert_eq!(d["total"], 2, "{d}");
    assert_eq!(d["marked_read"], true, "{d}");
    assert_eq!(d["skipped_lines"], 0, "{d}");
    assert!(
        d.get("next").is_none() && d.get("interrupted").is_none(),
        "{d}"
    );
    let m = arr(&d["messages"]);
    assert_eq!(m[0]["id"], one["id"], "{d}");
    assert_eq!(m[1]["id"], two["id"], "{d}");
    assert_eq!(m[0]["session_handle"], A.handle, "{d}");
    assert!(m[0]["job_handle"].is_null(), "{d}");
    // They were marked read once the document was out: the next call finds nothing unread.
    let again = inbox_wait(&rb, &["--timeout", "300ms"]);
    assert_inbox_bound(&again, 10);
    let all = data_of(&rb.glen(["inbox", "--all", "--json"]).run(), 0);
    assert_eq!(all["total"], 2, "{all}");
}

#[test]
fn ac_v2_202_a_message_arriving_during_the_wait_wakes_it_within_one_second() {
    let rb = RpcBox::new();
    let alpha = s(&rb.bx.alpha);
    let scopes: [&[&str]; 2] = [&[], &["--cwd", alpha.as_str()]];
    for (i, scope) in scopes.iter().enumerate() {
        let mut args = vec!["--timeout", "20s"];
        args.extend_from_slice(scope);
        let mut w = spawn_inbox(&rb, &args);
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            w.running(),
            "nothing is unread, so the wait goes on ({scope:?})"
        );
        let text = format!("knock {i}");
        let rec = put(&rb, A.id, &text);
        let sent = Instant::now();
        let (out, exited) = w.join(scaled(LONG));
        let lag = exited.duration_since(sent);
        eprintln!("inbox wake lag ({scope:?}): {lag:?}");
        assert!(
            lag <= scaled(ONE_SECOND),
            "woke {lag:?} after the append ({scope:?})\n{}",
            out.ctx()
        );
        let d = data_of(&out, 0);
        assert_eq!(d["total"], 1, "{d}");
        assert_eq!(d["marked_read"], true, "{d}");
        assert_eq!(d["messages"][0]["id"], rec["id"], "{d}");
        assert_eq!(d["messages"][0]["text"], text.as_str(), "{d}");
    }
}

#[test]
fn ac_v2_202_the_bound_gives_exit_10_with_no_messages_a_next_command_and_no_state() {
    let rb = RpcBox::new();
    let out = inbox_wait(&rb, &["--timeout", "300ms"]);
    assert!(
        out.elapsed >= Duration::from_millis(250) && out.elapsed < scaled(Duration::from_secs(10)),
        "the bound is honoured ({:?})",
        out.elapsed
    );
    let d = assert_inbox_bound(&out, 10);
    assert_eq!(d["skipped_lines"], 0, "{d}");
    assert!(
        !rb.bx.glen_home.exists(),
        "waiting for nothing writes nothing: {:?}",
        list_tree(&rb.bx.glen_home)
    );
    // `next` is a command glen accepts, and it keeps the caller's bound.
    let w = inbox_next(&d);
    assert_eq!(
        parse_dur(flag_value(&w, "--timeout").expect("--timeout")),
        Some(Duration::from_millis(300))
    );
    let again = rb.glen(&w[1..]).run();
    assert_inbox_bound(&again, 10);
    // Malformed lines are counted, never marked.
    append_bytes(&inbox_file_of(&rb, A.id), b"this is not json\n");
    let out = inbox_wait(&rb, &["--timeout", "300ms"]);
    let d = assert_inbox_bound(&out, 10);
    assert_eq!(d["skipped_lines"], 1, "{d}");
    assert!(
        !rb.bx.glen_home.join("inbox/.read").exists(),
        "nothing was marked"
    );
}

#[test]
fn ac_v2_202_next_repeats_session_cwd_and_peek_each_as_one_shell_word() {
    let rb = RpcBox::new();
    let spaced = rb.bx.home.join("work/with space");
    fs::create_dir_all(&spaced).unwrap();
    let out = inbox_wait(
        &rb,
        &[
            "--timeout",
            "300ms",
            "--session",
            A.handle,
            "--cwd",
            &s(&spaced),
            "--peek",
        ],
    );
    let d = assert_inbox_bound(&out, 10);
    let w = inbox_next(&d);
    assert_eq!(flag_value(&w, "--session"), Some(A.handle), "{w:?}");
    let cwd = flag_value(&w, "--cwd").expect("--cwd");
    assert!(
        same_file(Path::new(cwd), &spaced),
        "--cwd {cwd:?} is {spaced:?} as one word"
    );
    assert!(w.iter().any(|x| x == "--peek"), "{w:?}");
    assert_eq!(
        parse_dur(flag_value(&w, "--timeout").expect("--timeout")),
        Some(Duration::from_millis(300))
    );
    for banned in ["-f", "--follow", "--all"] {
        assert!(!w.iter().any(|x| x == banned), "{w:?}");
    }
    let again = rb.glen(&w[1..]).run();
    assert_inbox_bound(&again, 10);
    // Without them none is invented.
    let d = assert_inbox_bound(&inbox_wait(&rb, &["--timeout", "300ms"]), 10);
    let w = inbox_next(&d);
    for absent in ["--session", "--cwd", "--peek"] {
        assert!(!w.iter().any(|x| x == absent), "{w:?}");
    }
}

#[test]
fn ac_v2_202_messages_are_marked_read_only_after_output_never_with_peek() {
    let rb = RpcBox::new();
    put(&rb, A.id, "one");
    // --peek --wait is a one-shot read: it returns the unread message on every call, marking none.
    for _ in 0..2 {
        let d = data_of(&inbox_wait(&rb, &["--peek", "--timeout", "20s"]), 0);
        assert_eq!(d["total"], 1, "{d}");
        assert_eq!(d["marked_read"], false, "{d}");
    }
    assert!(!rb.bx.glen_home.join("inbox/.read").exists());
    // A reader that has gone away leaves the messages unread: no output, no mark.
    let out = rb
        .glen(["inbox", "--wait", "--timeout", "20s", "--json"])
        .stdout(closed_stdout())
        .run();
    out.assert_exit(0).assert_stderr_empty();
    assert!(
        !cursor_of(&rb, A.id).exists(),
        "a message that was never delivered must not be marked read"
    );
    let peek = data_of(&rb.glen(["inbox", "--peek", "--json"]).run(), 0);
    assert_eq!(peek["total"], 1, "still unread: {peek}");
    // Delivered, it is marked: the cursor reaches the end of the file.
    let d = data_of(&inbox_wait(&rb, &["--timeout", "20s"]), 0);
    assert_eq!(d["marked_read"], true, "{d}");
    let cursor: Value = serde_json::from_slice(&fs::read(cursor_of(&rb, A.id)).unwrap()).unwrap();
    assert_eq!(
        cursor["offset"],
        fs::metadata(inbox_file_of(&rb, A.id)).unwrap().len(),
        "{cursor}"
    );
    assert_inbox_bound(&inbox_wait(&rb, &["--peek", "--timeout", "300ms"]), 10);
}

#[test]
fn ac_v2_202_session_decides_what_wakes_the_wait() {
    let rb = RpcBox::new();
    put(&rb, D.id, "beta one");
    let out = inbox_wait(
        &rb,
        &["--session", A.handle, "--peek", "--timeout", "700ms"],
    );
    assert_inbox_bound(&out, 10);
    let mut w = spawn_inbox(&rb, &["--session", A.handle, "--timeout", "20s"]);
    std::thread::sleep(Duration::from_millis(500));
    put(&rb, D.id, "beta two");
    std::thread::sleep(Duration::from_millis(700));
    assert!(
        w.running(),
        "another session's message must not wake the wait"
    );
    let rec = put(&rb, A.id, "alpha");
    let sent = Instant::now();
    let (out, exited) = w.join(scaled(LONG));
    assert!(
        exited.duration_since(sent) <= scaled(ONE_SECOND),
        "{}",
        out.ctx()
    );
    let d = data_of(&out, 0);
    assert_eq!(d["total"], 1, "only the requested session: {d}");
    assert_eq!(d["messages"][0]["id"], rec["id"], "{d}");
    assert!(cursor_of(&rb, A.id).is_file());
    assert!(
        !cursor_of(&rb, D.id).exists(),
        "the other session stays unread"
    );
}

#[test]
fn ac_v2_202_cwd_decides_what_wakes_the_wait() {
    let rb = RpcBox::new();
    let alpha = s(&rb.bx.alpha);
    let beta = s(&rb.bx.beta);
    put(&rb, D.id, "beta one");
    let out = inbox_wait(&rb, &["--cwd", &alpha, "--peek", "--timeout", "700ms"]);
    assert_inbox_bound(&out, 10);
    let mut w = spawn_inbox(&rb, &["--cwd", &alpha, "--timeout", "20s"]);
    std::thread::sleep(Duration::from_millis(500));
    put(&rb, D.id, "beta two");
    std::thread::sleep(Duration::from_millis(700));
    assert!(
        w.running(),
        "a message of another project must not wake the wait"
    );
    let rec = put(&rb, A.id, "alpha");
    let sent = Instant::now();
    let (out, exited) = w.join(scaled(LONG));
    assert!(
        exited.duration_since(sent) <= scaled(ONE_SECOND),
        "{}",
        out.ctx()
    );
    let d = data_of(&out, 0);
    assert_eq!(d["total"], 1, "only the requested project: {d}");
    assert_eq!(d["messages"][0]["id"], rec["id"], "{d}");
    assert!(
        !cursor_of(&rb, D.id).exists(),
        "the other project stays unread"
    );
    // The other project's own wait returns its messages at once.
    let d = data_of(&inbox_wait(&rb, &["--cwd", &beta, "--timeout", "20s"]), 0);
    assert_eq!(d["total"], 2, "{d}");
}

#[test]
fn ac_v2_202_sigint_gives_the_bound_document_with_interrupted_true_and_exit_130() {
    let rb = RpcBox::new();
    let mut w = spawn_inbox(&rb, &["--timeout", "60s"]);
    await_sigint_handler(w.pid, Duration::from_millis(700));
    assert!(w.running());
    let sent = Instant::now();
    w.signal(Signal::INT);
    let (out, exited) = w.join(scaled(LONG));
    assert!(
        exited.duration_since(sent) < scaled(Duration::from_secs(2)),
        "SIGINT ends the wait promptly"
    );
    let d = assert_inbox_bound(&out, 130);
    assert_eq!(d["interrupted"], true, "{d}");
    inbox_next(&d);
    assert!(!rb.bx.glen_home.exists(), "interrupting writes nothing");
}

#[test]
fn ac_v2_202_sigterm_keeps_its_default_disposition() {
    let rb = RpcBox::new();
    put(&rb, A.id, "read me later");
    let peek = data_of(&rb.glen(["inbox", "--peek", "--json"]).run(), 0);
    assert_eq!(peek["total"], 1);
    // Wait on a scope with nothing in it, then terminate.
    let mut w = spawn_inbox(&rb, &["--session", D.handle, "--timeout", "60s"]);
    std::thread::sleep(Duration::from_millis(700));
    assert!(w.running());
    w.signal(Signal::TERM);
    let (out, _) = w.join(scaled(LONG));
    assert!(
        out.status.signal() == Some(15) || out.status.code() == Some(143),
        "SIGTERM terminates the process as usual: {:?}\n{}",
        out.status,
        out.ctx()
    );
    assert!(!out.out().contains("\"interrupted\":true"), "{}", out.ctx());
    assert!(!cursor_of(&rb, A.id).exists());
}

#[test]
fn ac_v2_202_inbox_wait_covers_the_replies_of_a_job_that_already_ended() {
    let rb = RpcBox::new();
    let (h, disk) = replied_rpc_job(&rb);
    finish_held(&rb, &h);
    // The job's own receiver returns them (the end of the job) without consuming the inbox.
    let d = data_of(&reply_wait(&rb, &h.handle, &[]), 0);
    assert_replies(&d["replies"], &[&disk[0], &disk[1]]);
    let peek = data_of(&inbox_wait(&rb, &["--peek", "--timeout", "20s"]), 0);
    assert_eq!(peek["total"], 2, "{peek}");
    assert_eq!(peek["marked_read"], false, "{peek}");
    for m in arr(&peek["messages"]) {
        assert_eq!(m["job_id"], h.id.as_str(), "{m}");
        assert_eq!(m["job_handle"], h.handle.as_str(), "{m}");
    }
    let read = data_of(&inbox_wait(&rb, &["--timeout", "20s"]), 0);
    assert_eq!(read["total"], 2, "{read}");
    assert_eq!(read["marked_read"], true, "{read}");
    assert_inbox_bound(&inbox_wait(&rb, &["--timeout", "300ms"]), 10);
}

#[test]
fn ac_v2_202_a_live_reply_reaching_the_inbox_wakes_inbox_wait_within_one_second() {
    let lb = LiveBox::new();
    let (_ext, h, _) = held_live_job(&lb);
    let mut w = spawn_inbox(&lb, &["--timeout", "20s"]);
    std::thread::sleep(Duration::from_millis(500));
    assert!(w.running(), "nothing is unread yet\n{}", lb.diag());
    let landed = watch_lines(lb.inbox_file(A.id), 1);
    h.release_run();
    let (out, exited) = w.join(scaled(LONG));
    let at = landed
        .recv_timeout(scaled(LONG))
        .expect("the extension wrote an inbox record");
    let lag = exited.saturating_duration_since(at);
    eprintln!("live inbox wake lag: {lag:?}");
    assert!(
        lag <= scaled(ONE_SECOND),
        "woke {lag:?} after the record landed\n{}",
        out.ctx()
    );
    let d = data_of(&out, 0);
    assert!(d["total"].as_u64().unwrap() >= 1, "{d}");
    assert_eq!(d["marked_read"], true, "{d}");
    assert_eq!(d["messages"][0]["session_id"], A.id, "{d}");
    assert_eq!(d["messages"][0]["job_id"], h.id.as_str(), "{d}");
    finish_held(&lb, &h);
}

// ---------------------------------------------------------------------------------------------
// AC-v2-204: the skill. Rendered through `install-skill`, as an agent receives it.
// ---------------------------------------------------------------------------------------------

/// The rendered Claude and Codex skill files (project scope, in a scratch directory).
fn rendered_skills() -> (String, String) {
    let sb = Sandbox::new();
    sb.glen(["install-skill", "--project", "--json"])
        .run()
        .ok_data();
    let read = |rel: &str| {
        fs::read_to_string(sb.cwd.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
    };
    (
        read(".claude/skills/glen/SKILL.md"),
        read(".agents/skills/glen/SKILL.md"),
    )
}

/// The skill after its `<!-- glen-skill … -->` marker line (the part both agents share).
fn skill_body(rendered: &str) -> &str {
    let at = rendered
        .find("<!-- glen-skill ")
        .expect("a glen-skill marker line");
    let rest = &rendered[at..];
    &rest[rest.find('\n').expect("marker line ends") + 1..]
}

/// Lines of the `## <heading>` section (up to the next `## ` line).
fn skill_section(body: &str, heading: &str) -> Vec<String> {
    let lines: Vec<&str> = body.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim_end() == format!("## {heading}"))
        .unwrap_or_else(|| panic!("the skill has no `## {heading}` section"));
    lines[start + 1..]
        .iter()
        .take_while(|l| !l.starts_with("## "))
        .map(|l| (*l).to_owned())
        .collect()
}

/// Markdown bullets: a `- ` line plus its indented continuation lines, joined by a space.
fn bullets(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in lines {
        if let Some(rest) = l.strip_prefix("- ") {
            out.push(rest.trim().to_owned());
        } else if l.starts_with("  ")
            && !l.trim().is_empty()
            && let Some(last) = out.last_mut()
        {
            last.push(' ');
            last.push_str(l.trim());
        }
    }
    out
}

/// Inline code spans and fenced-block lines of a Markdown body.
fn code_spans(body: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut in_fence = false;
    let mut prose = String::new();
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        } else if in_fence {
            spans.push(line.to_owned());
        } else {
            prose.push_str(line);
            prose.push('\n');
        }
    }
    for (i, part) in prose.split('`').enumerate() {
        if i % 2 == 1 {
            spans.push(part.to_owned());
        }
    }
    spans
}

fn label_of(bullet: &str) -> &str {
    bullet.split(':').next().unwrap_or(bullet)
}

#[test]
fn ac_v2_204_both_rendered_skills_have_the_section_with_identical_bodies_within_budget() {
    let (claude, codex) = rendered_skills();
    for (name, text) in [("claude", &claude), ("codex", &codex)] {
        assert!(
            text.lines()
                .any(|l| l.trim_end() == "## Replies without polling"),
            "{name} skill lacks `## Replies without polling`"
        );
        assert!(
            text.lines().count() <= 160,
            "{name}: {} lines",
            text.lines().count()
        );
        assert!(text.len() <= 12 * 1024, "{name}: {} bytes", text.len());
    }
    assert_eq!(
        skill_body(&claude),
        skill_body(&codex),
        "the two agents' bodies are byte-identical"
    );
}

#[test]
fn ac_v2_204_the_section_names_the_receiver_per_caller() {
    let (claude, _) = rendered_skills();
    let section = skill_section(skill_body(&claude), "Replies without polling");
    let text = section.join("\n");
    assert!(text.contains("--for reply"), "{text}");
    let all = bullets(&section);
    let lower = |b: &String| b.to_lowercase();

    let code = all
        .iter()
        .find(|b| {
            label_of(b).contains("Claude Code")
                && !lower(&label_of(b).to_owned()).contains("subagent")
        })
        .unwrap_or_else(|| panic!("no Claude Code bullet: {all:#?}"));
    assert!(lower(code).contains("background"), "{code}");
    assert!(
        code.contains("Bash") || code.contains("run_in_background"),
        "{code}"
    );

    let sub = all
        .iter()
        .find(|b| lower(&label_of(b).to_owned()).contains("subagent"))
        .unwrap_or_else(|| panic!("no subagent bullet: {all:#?}"));
    for needle in ["foreground", "parent", "handle"] {
        assert!(
            lower(sub).contains(needle),
            "subagent bullet lacks {needle:?}: {sub}"
        );
    }

    let codex = all
        .iter()
        .find(|b| label_of(b).contains("Codex"))
        .unwrap_or_else(|| panic!("no Codex bullet: {all:#?}"));
    assert!(codex.contains("exec_command"), "{codex}");
    assert!(
        lower(codex).contains("yield") || lower(codex).contains("poll"),
        "{codex}"
    );
    assert!(codex.contains("inbox --peek"), "{codex}");
    assert!(
        lower(codex).contains("turn"),
        "inbox --peek at the start of each turn: {codex}"
    );
}

/// Code spans that invoke a bare `glen inbox`: no `--wait`, `--peek` or `-f`/`--follow`.
fn bare_inbox_spans(body: &str) -> Vec<String> {
    code_spans(body)
        .into_iter()
        .filter(|sp| sp.contains("glen inbox"))
        .filter(|sp| {
            !(sp.contains("--wait")
                || sp.contains("--peek")
                || sp.split_whitespace().any(|w| w == "-f" || w == "--follow"))
        })
        .collect()
}

/// Lines that talk about looping, polling or repeating the inbox without forbidding it, and
/// code spans that loop in the shell.
fn inbox_loop_instructions(body: &str) -> Vec<String> {
    let mut found: Vec<String> = code_spans(body)
        .into_iter()
        .filter(|sp| sp.contains("sleep ") || sp.contains("while ") || sp.contains("until "))
        .collect();
    for line in body.lines().filter(|l| l.contains("inbox")) {
        let l = line.to_lowercase();
        let loops = l.contains("loop") || l.contains("poll") || l.contains("repeat");
        let forbids = [
            "never", "don't", "do not", "not ", "no ", "instead", "without",
        ]
        .iter()
        .any(|n| l.contains(n));
        if loops && !forbids {
            found.push(line.to_owned());
        }
    }
    // The watch bullet no longer sends agents to `glen inbox -f`.
    found.extend(
        body.lines()
            .filter(|l| l.contains("glen watch") && l.contains("inbox -f"))
            .map(str::to_owned),
    );
    found
}

#[test]
fn ac_v2_204_no_inbox_loop_instruction_remains_every_glen_inbox_span_waits_peeks_or_follows() {
    let (claude, codex) = rendered_skills();
    for (name, text) in [("claude", &claude), ("codex", &codex)] {
        let body = skill_body(text);
        assert!(
            code_spans(body).iter().any(|s| s.contains("glen inbox")),
            "{name}: the skill still documents the inbox"
        );
        let bare = bare_inbox_spans(body);
        assert!(
            bare.is_empty(),
            "{name}: a bare `glen inbox` invites a polling loop: {bare:?}"
        );
        let looping = inbox_loop_instructions(body);
        assert!(
            looping.is_empty(),
            "{name}: an inbox-loop instruction: {looping:?}"
        );
    }
}

#[test]
fn ac_v2_204_the_inbox_loop_checks_catch_what_they_are_meant_to_catch() {
    let old = "5. Later: run `glen inbox` for replies.\n";
    assert_eq!(bare_inbox_spans(old), ["glen inbox"]);
    assert!(
        bare_inbox_spans("`glen inbox --wait` and `glen inbox --peek` and `glen inbox -f`")
            .is_empty()
    );
    assert_eq!(bare_inbox_spans("`glen inbox --session e27c0de`").len(), 1);
    assert!(!inbox_loop_instructions("Poll the inbox every minute.\n").is_empty());
    assert!(!inbox_loop_instructions("Check `glen inbox --peek` in a loop.\n").is_empty());
    assert!(!inbox_loop_instructions("`while true; do glen inbox --peek; done`\n").is_empty());
    assert!(inbox_loop_instructions("Never poll the inbox in a loop.\n").is_empty());
    assert!(!inbox_loop_instructions("use `glen watch <h>` or `glen inbox -f`\n").is_empty());
}

#[test]
fn ac_v2_205_the_harness_helpers_parse_what_the_contract_tests_rely_on() {
    assert_eq!(
        shell_words("glen inbox --wait --cwd '/tmp/a b' --session \"x y\" it\\'s"),
        [
            "glen",
            "inbox",
            "--wait",
            "--cwd",
            "/tmp/a b",
            "--session",
            "x y",
            "it's"
        ]
    );
    assert_eq!(shell_words("  a   b "), ["a", "b"]);
    assert_eq!(shell_words("a ''"), ["a", ""]);
    assert_eq!(parse_dur("300ms"), Some(Duration::from_millis(300)));
    assert_eq!(parse_dur("90s"), Some(Duration::from_secs(90)));
    assert_eq!(parse_dur("90"), Some(Duration::from_secs(90)));
    assert_eq!(parse_dur("2m"), Some(Duration::from_secs(120)));
    assert_eq!(parse_dur("24h"), Some(Duration::from_secs(86_400)));
    assert_eq!(parse_dur("soon"), None);
    let words = shell_words("glen wait j1234567 --for reply --after r1 --timeout 20s");
    assert_eq!(flag_value(&words, "--after"), Some("r1"));
    assert_eq!(flag_value(&words, "--nope"), None);
    let bullets = bullets(&[
        "intro".to_owned(),
        "- Claude Code: a".to_owned(),
        "  b".to_owned(),
        "- Codex: c".to_owned(),
    ]);
    assert_eq!(bullets, ["Claude Code: a b", "Codex: c"]);
    assert_eq!(label_of(&bullets[0]), "Claude Code");
    // The §A7 table drives the expectations: rejected is the no_route row of `failed`.
    assert_eq!(expected_state("failed", Some("no_route")), "rejected");
    assert_eq!(expected_state("failed", Some("omp_error")), "failed");
    assert_eq!(expected_state("failed", None), "failed");
    assert_eq!(expected_state("lost", None), "failed");
}

#[test]
fn ac_v2_204_the_exit_table_says_a_woke_tells_a_wake_from_a_bound_expiry() {
    let (claude, _) = rendered_skills();
    let row = claude
        .lines()
        .find(|l| l.starts_with("| 10 |"))
        .expect("the skill's exit table has an exit 10 row");
    assert!(row.contains("woke"), "{row}");
    assert!(row.contains("--for reply"), "{row}");
}

#[test]
fn ac_v2_205_the_a20_table_and_the_skill_agree_per_caller() {
    let (claude, _) = rendered_skills();
    let skill = bullets(&skill_section(
        skill_body(&claude),
        "Replies without polling",
    ));
    let a20 = section(&design(), 20);
    let (header, rows) = table_where(&a20, |h| h.iter().any(|c| plain(c).contains("Caller")));
    assert!(header.len() >= 3, "{header:?}");
    let find_row = |needle: &str| {
        rows.iter()
            .find(|r| plain(&r[0]).contains(needle))
            .unwrap_or_else(|| panic!("§A20 table has no `{needle}` row: {rows:?}"))
    };
    let skill_bullet = |pred: &dyn Fn(&str) -> bool| {
        skill
            .iter()
            .find(|b| pred(label_of(b)))
            .unwrap_or_else(|| panic!("no matching skill bullet: {skill:#?}"))
            .clone()
    };
    let timeouts = |text: &str| -> Vec<String> {
        shell_words(&text.replace('`', " "))
            .windows(2)
            .filter(|w| w[0] == "--timeout")
            .map(|w| w[1].trim_end_matches([',', '.', ';', ')']).to_owned())
            .collect()
    };

    let interactive = find_row("interactive");
    let row = interactive.join(" | ");
    assert!(
        row.contains("--for reply") && row.contains("run_in_background"),
        "{row}"
    );
    assert!(
        plain(interactive.last().unwrap()).starts_with("yes"),
        "{row}"
    );
    let bullet =
        skill_bullet(&|l| l.contains("Claude Code") && !l.to_lowercase().contains("subagent"));
    for needle in ["run_in_background", "--no-wait"] {
        assert!(
            row.contains(needle) && bullet.contains(needle),
            "{needle}: {row} / {bullet}"
        );
    }

    let sub = find_row("subagent");
    let row = sub.join(" | ");
    assert!(
        row.contains("foreground") && plain(sub.last().unwrap()) == "no",
        "{row}"
    );
    let bullet = skill_bullet(&|l| l.to_lowercase().contains("subagent"));
    assert!(
        !timeouts(&row).is_empty(),
        "the subagent row names its bound: {row}"
    );
    for t in timeouts(&row) {
        assert!(bullet.contains(&format!("--timeout {t}")), "{t}: {bullet}");
    }

    let codex = find_row("Codex");
    let row = codex.join(" | ");
    assert!(row.contains("Codex: no push"), "{row}");
    let bullet = skill_bullet(&|l| l.contains("Codex"));
    for needle in ["exec_command", "inbox --peek", "--for reply"] {
        assert!(
            row.contains(needle) && bullet.contains(needle),
            "{needle}: {row} / {bullet}"
        );
    }
    assert!(
        !timeouts(&row).is_empty(),
        "the Codex row names its bound: {row}"
    );
    for t in timeouts(&row) {
        assert!(bullet.contains(&format!("--timeout {t}")), "{t}: {bullet}");
    }
    assert!(
        !bullet.to_lowercase().contains("wakes you"),
        "Codex is not woken: {bullet}"
    );
}

#[test]
fn ac_v2_205_the_headless_row_and_the_skill_do_not_contradict_each_other() {
    let (claude, _) = rendered_skills();
    let skill = bullets(&skill_section(
        skill_body(&claude),
        "Replies without polling",
    ));
    let a20 = section(&design(), 20);
    let (_, rows) = table_where(&a20, |h| h.iter().any(|c| plain(c).contains("Caller")));
    let row = rows
        .iter()
        .find(|r| r[0].contains("-p"))
        .expect("§A20 has a `claude -p` row");
    let delivery = row[1..row.len() - 1].join(" ").to_lowercase();
    let push = plain(row.last().unwrap()).to_lowercase();
    assert!(push.starts_with("yes") || push.starts_with("no"), "{row:?}");
    assert!(
        delivery.contains("background") || delivery.contains("foreground"),
        "{row:?}"
    );
    let headless = skill.iter().find(|b| {
        let l = label_of(b).to_lowercase();
        l.contains("-p") || l.contains("headless")
    });
    if let Some(b) = headless {
        let b = b.to_lowercase();
        if b.contains("foreground") && !b.contains("background") {
            assert!(
                push.starts_with("no"),
                "skill says foreground, table says {push}: {b}"
            );
        }
        if b.contains("background") {
            assert!(
                push.starts_with("yes"),
                "skill says background, table says {push}: {b}"
            );
        }
    } else {
        // No `claude -p` bullet: the evidence is still pending, or the Claude Code bullet applies.
        assert!(
            push.contains("pending") || push.starts_with("yes"),
            "a foreground-only headless row needs its own skill bullet: {row:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// AC-v2-203 and AC-v2-205: the written contract (`docs/json.md`, §A7, §A9, §A11, §A20).
// ---------------------------------------------------------------------------------------------

fn read_repo(rel: &str) -> String {
    fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// `<!-- contract: NAME -->` examples of `docs/json.md` whose fenced block is one JSON document.
fn contract_examples() -> Vec<(String, Value)> {
    let doc = read_repo("docs/json.md");
    let lines: Vec<&str> = doc.lines().collect();
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let Some(rest) = l.strip_prefix("<!-- contract: ") else {
            continue;
        };
        let name = rest.trim_end_matches("-->").trim().to_owned();
        let Some(open) = lines[i + 1..].iter().position(|l| l.starts_with("```json")) else {
            continue;
        };
        let body: Vec<&str> = lines[i + 1 + open + 1..]
            .iter()
            .take_while(|l| !l.starts_with("```"))
            .copied()
            .collect();
        if let Ok(v) = serde_json::from_str::<Value>(&body.join("\n")) {
            out.push((name, v));
        }
    }
    out
}

fn example(name: &str) -> Value {
    contract_examples()
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("docs/json.md has no `{name}` contract example"))
        .1
}

fn walk(v: &Value, f: &mut impl FnMut(&Value)) {
    f(v);
    match v {
        Value::Array(a) => a.iter().for_each(|x| walk(x, f)),
        Value::Object(o) => o.values().for_each(|x| walk(x, f)),
        _ => {}
    }
}

#[test]
fn ac_v2_203_json_md_job_views_all_carry_a_task_state_consistent_with_their_status() {
    let doc = read_repo("docs/json.md");
    let line = doc
        .lines()
        .find(|l| l.starts_with("Job views share"))
        .expect("docs/json.md lists the job view fields");
    let fields = doc
        .split("Job views share")
        .nth(1)
        .unwrap()
        .split("Routes are")
        .next()
        .unwrap();
    assert!(fields.contains("task_state"), "{line} {fields}");
    let mut views = 0;
    for (name, v) in contract_examples() {
        walk(&v, &mut |o| {
            let Some(m) = o.as_object() else { return };
            if !(m.contains_key("handle")
                && m.contains_key("elapsed_ms")
                && m.contains_key("route"))
            {
                return;
            }
            views += 1;
            let state = m
                .get("task_state")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("docs/json.md `{name}`: a job view without task_state: {o}")
                });
            let status = m["status"].as_str().unwrap_or("");
            match status {
                "failed" => assert!(["failed", "rejected"].contains(&state), "{name}: {o}"),
                "queued" | "accepted" | "completed" | "aborted" | "lost" => {
                    assert_eq!(state, expected_state(status, None), "{name}: {o}")
                }
                other => panic!("docs/json.md `{name}`: status {other:?}"),
            }
        });
    }
    assert!(views >= 8, "only {views} job views in the json.md examples");
}

#[test]
fn ac_v2_205_json_md_documents_the_two_exit_10_documents_and_inbox_wait() {
    let wake = example("wait.reply");
    let d = &wake["data"];
    assert_eq!(d["woke"], "reply", "{wake}");
    assert_eq!(d["status"], "running", "{wake}");
    assert_eq!(d["interrupted"], false, "{wake}");
    assert!(!arr(&d["replies"]).is_empty(), "{wake}");
    let last = d["replies"].as_array().unwrap().last().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_wait_next(d, d["job"]["handle"].as_str().unwrap(), Some(&last), None);
    assert!(
        ["working", "submitted"].contains(&d["job"]["task_state"].as_str().unwrap()),
        "{wake}"
    );

    let expired = example("wait.reply.expired");
    let d = &expired["data"];
    assert!(d.get("woke").is_none(), "{expired}");
    assert_eq!(d["replies"], json!([]), "{expired}");
    assert_eq!(d["status"], "running", "{expired}");
    let w = shell_words(d["next"].as_str().unwrap());
    assert_eq!(flag_value(&w, "--for"), Some("reply"), "{expired}");

    let inbox = example("inbox.wait");
    let d = &inbox["data"];
    assert_eq!(d["total"], 0, "{inbox}");
    assert_eq!(d["messages"], json!([]), "{inbox}");
    assert_eq!(d["marked_read"], false, "{inbox}");
    assert_eq!(d["interrupted"], false, "{inbox}");
    assert!(d["skipped_lines"].is_u64(), "{inbox}");
    inbox_next(d);
}

#[test]
fn ac_v2_205_exit_10_rows_of_a11_and_json_md_say_how_to_tell_a_wake_from_an_expiry() {
    let a11 = section(&design(), 11);
    let (_, rows) = table_where(&a11, |h| h.iter().any(|c| plain(c).starts_with("Exit")));
    let row = rows
        .iter()
        .find(|r| plain(&r[0]) == "10")
        .expect("§A11 has an exit 10 row");
    let text = row.join(" | ");
    assert!(
        text.contains("woke") && text.contains("--for reply"),
        "{text}"
    );
    assert!(
        text.contains("running"),
        "the row still describes `running`: {text}"
    );

    let json = read_repo("docs/json.md");
    let row = json
        .lines()
        .find(|l| l.starts_with("| 10 |"))
        .expect("docs/json.md's exit table has an exit 10 row");
    assert!(row.contains("woke"), "{row}");

    let a7 = section(&design(), 7);
    let mapping = a7
        .iter()
        .find(|l| l.contains("Exit mapping"))
        .expect("§A7 has the exit mapping bullet");
    assert!(
        mapping.contains("woke") && mapping.contains("--for reply"),
        "{mapping}"
    );
}

#[test]
fn ac_v2_205_a9_and_a20_describe_the_new_flags_and_state_codex_no_push() {
    let doc = design();
    let a9 = section(&doc, 9);
    let wait_row = a9
        .iter()
        .find(|l| l.contains("`glen wait <job>"))
        .expect("§A9 wait row");
    assert!(
        wait_row.contains("--for reply") && wait_row.contains("--after"),
        "{wait_row}"
    );
    let inbox_row = a9
        .iter()
        .find(|l| l.contains("`glen inbox "))
        .expect("§A9 inbox row");
    assert!(
        inbox_row.contains("--wait") && inbox_row.contains("--timeout"),
        "{inbox_row}"
    );

    let a20 = section(&doc, 20);
    let (_, rows) = table_where(&a20, |h| h.iter().any(|c| plain(c).contains("Caller")));
    let codex: Vec<&Vec<String>> = rows
        .iter()
        .filter(|r| plain(&r[0]).starts_with("Codex"))
        .collect();
    assert!(!codex.is_empty());
    for r in codex {
        assert!(r.join(" | ").contains("Codex: no push"), "{r:?}");
    }
    assert!(a20.iter().any(|l| l.contains("Codex: no push")));
    for needle in ["woke", "--after", "unknown_reply", "Codex: no push"] {
        assert!(
            a20.iter().any(|l| l.contains(needle)),
            "§A20 never mentions {needle:?}"
        );
    }
    let a15 = section(&doc, 15).join("\n");
    for knob in ["FAKE_EXT_RUN_RELEASE", "FAKE_EXT_RESULT_RELEASE"] {
        assert!(a15.contains(knob), "§A15 documents {knob}");
    }
}

#[test]
fn ac_v2_205_the_changelog_announces_the_receivers_and_does_not_call_codex_push() {
    let log = read_repo("CHANGELOG.md");
    let unreleased: String = log
        .lines()
        .skip_while(|l| l.trim_end() != "## Unreleased")
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .collect::<Vec<_>>()
        .join("\n");
    for needle in [
        "--for reply",
        "inbox --wait",
        "task_state",
        "Replies without polling",
        "Codex: blocking wait, not push",
        "woke",
    ] {
        assert!(
            unreleased.contains(needle),
            "CHANGELOG Unreleased lacks {needle:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// AC-v2-206: the acceptance harness (static checks and the offline self-test).
// ---------------------------------------------------------------------------------------------

/// Lines of the shell function `name() {` … `}` (closing brace at column 0).
fn shell_function(script: &str, name: &str) -> Vec<String> {
    let lines: Vec<&str> = script.lines().collect();
    let start = lines
        .iter()
        .position(|l| *l == format!("{name}() {{"))
        .unwrap_or_else(|| panic!("no shell function {name}"));
    lines[start + 1..]
        .iter()
        .take_while(|l| **l != "}")
        .map(|l| (*l).to_owned())
        .collect()
}

#[test]
fn ac_v2_206_acceptance_mode_p_exists_with_an_uncoached_prompt() {
    let acc = read_repo("tests/acceptance/acc.sh");
    assert!(acc.contains("setup_p() {"), "acc.sh has a mode p setup");
    let mode_checks = acc
        .lines()
        .filter(|l| l.contains("=~ ^(") && l.contains("|p)$"))
        .count();
    assert!(
        mode_checks >= 2,
        "setup and check accept mode p ({mode_checks})"
    );
    let readme = read_repo("tests/acceptance/README.md");
    assert!(
        readme.contains("(p) `claude -p`"),
        "README documents mode p"
    );

    let prompt = read_repo("tests/acceptance/prompts/claude-push.md");
    for placeholder in ["{{PROJECT}}", "{{NONCE_P}}"] {
        assert!(prompt.contains(placeholder), "the prompt has {placeholder}");
    }
    let lower = prompt.to_lowercase();
    for coaching in [
        "background",
        "--for reply",
        "poll",
        "run_in_background",
        "glen wait",
        "inbox",
        "--no-wait",
        "receiver",
        "notification",
        "foreground",
    ] {
        assert!(
            !lower.contains(coaching),
            "the push prompt must not coach the receiver, but says {coaching:?}"
        );
    }
}

#[test]
fn ac_v2_206_mode_p_uses_the_fake_omp_and_every_real_omp_leg_runs_under_glen_harness() {
    let acc = read_repo("tests/acceptance/acc.sh");
    assert!(
        acc.lines().any(|l| l == "PROFILE=glen-harness"),
        "acc.sh pins the profile"
    );

    // Mode p: fake omp, the progress scenario, a delay longer than send's 90 s foreground wait.
    let p = shell_function(&acc, "setup_p");
    let p_text = p.join("\n");
    for needle in [
        "GLEN_OMP_BIN",
        "FAKE_OMP_SCENARIO",
        "progress",
        "FAKE_OMP_DELAY_MS",
        "e27c0de",
    ] {
        assert!(p_text.contains(needle), "setup_p lacks {needle}");
    }
    let after = p_text
        .split("FAKE_OMP_DELAY_MS")
        .nth(1)
        .expect("setup_p sets FAKE_OMP_DELAY_MS");
    let digits: String = after
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    let delay: u64 = digits.parse().expect("a delay in milliseconds");
    assert!(
        delay > 90_000,
        "the fake's delay {delay} ms outlasts send's 90 s wait"
    );
    assert!(
        !p_text.contains("--profile"),
        "mode p runs no real omp, so no profile"
    );

    // Real-omp legs: every glen call that reaches omp, and every omp launch, names the profile.
    let setup = shell_function(&acc, "setup").join("\n");
    let mut reaching = 0;
    for line in setup.lines() {
        if let Some(rest) = line.split("\"$G\" ").nth(1) {
            let verb = rest.split_whitespace().next().unwrap_or("");
            if ["send", "ls", "inbox", "doctor"].contains(&verb) {
                reaching += 1;
                assert!(
                    line.contains("--profile \"$PROFILE\""),
                    "a real-omp call without the profile: {line}"
                );
            }
            if verb == "install-extension" {
                assert!(
                    !line.contains("--profile"),
                    "install-extension stays profile-less: {line}"
                );
                assert!(
                    line.contains("PI_CODING_AGENT_DIR="),
                    "…under a scratch agent dir: {line}"
                );
            }
        }
    }
    assert!(reaching >= 1, "setup makes real-omp calls ({reaching})");
    assert!(
        setup.contains("install-extension"),
        "setup installs the scratch extension"
    );
    for line in acc.lines().filter(|l| l.contains("exec omp")) {
        assert!(
            line.contains("--profile"),
            "an omp launch without the profile: {line}"
        );
    }

    let wrapper = read_repo("tests/acceptance/glen-wrapper.sh");
    assert!(
        wrapper.contains("--profile \"$profile\""),
        "the wrapper adds the manifest's profile"
    );
    let person = read_repo("tests/acceptance/person.exp");
    assert!(
        person.contains("--profile glen-harness"),
        "person mode names the profile"
    );
}

#[test]
fn ac_v2_206_the_checker_records_which_class_of_wake_the_run_saw() {
    let check = read_repo("tests/acceptance/check.jq");
    for needle in [
        "AC-206-send",
        "AC-206-bg-wait-started",
        "AC-206-no-inbox-polling",
        "AC-206-evidence",
        "AC-206-nonce",
        "wake=",
        "woke",
        "no_wake",
        "foreground_only",
        "no_wait",
    ] {
        assert!(check.contains(needle), "check.jq lacks {needle}");
    }
    let readme = read_repo("tests/acceptance/README.md");
    for class in ["woke", "no_wake", "foreground_only", "no_wait"] {
        assert!(
            readme.contains(&format!("`{class}`")),
            "README documents `{class}`"
        );
    }
}

#[test]
fn ac_v2_206_the_offline_acceptance_self_test_passes() {
    let tools = ["jq", "perl", "git", "shasum", "gitleaks", "bash"];
    let path = std::env::var_os("PATH").unwrap_or_default();
    let have = |t: &str| std::env::split_paths(&path).any(|d| d.join(t).is_file());
    if let Some(missing) = tools.iter().find(|t| !have(t)) {
        eprintln!("skipped: the offline self-test needs `{missing}`");
        return;
    }
    let sb = Sandbox::new();
    let tmp = sb.dir("tmp");
    let out = try_output(
        Command::new("bash")
            .arg(repo_root().join("tests/acceptance/self-test.sh"))
            .env_clear()
            .env("PATH", &path)
            .env("HOME", &sb.home)
            .env("TMPDIR", &tmp)
            .current_dir(&sb.cwd),
    )
    .expect("run the acceptance self-test");
    assert!(
        out.status.success(),
        "self-test failed ({:?})\n{}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("offline harness fixture checks passed"),
        "the self-test ran to its end:\n{stdout}"
    );
}
