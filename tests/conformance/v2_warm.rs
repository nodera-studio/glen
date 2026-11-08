//! v0.2 W4 (opt-in warm worker) conformance: AC-v2-401 … AC-v2-407.
//!
//! Derived from the settled acceptance criteria and "AC details" of
//! `.claude/plans/v020-wave-4-warm-worker.md` (with the binding judge edits J1–J14) and from the
//! public contracts they name: `glen --help`, `glen send --help` and `glen release --help`,
//! `docs/json.md`, `docs/design.md` §A6, §A7, §A9, §A10, §A15, §A16 and §A21, the shell
//! completions, the rendered skill and the fake peers' documented knobs (`FAKE_OMP_MULTI`,
//! `FAKE_OMP_IDLE_REPLY_RELEASE`, `FAKE_OMP_IDLE_EXIT_RELEASE`, `GLEN_WARM_STALE_MS`). Written
//! without reading the implementation or the Implementer's own suite (`tests/warm.rs`).
//!
//! Everything is observed through the CLI, the files §A6 and §A21 document (`jobs/<id>/…`,
//! `run/locks/<key>.lock`, `run/warm/<key>.{json,release}`) and the fake peer's log. Every glen
//! runs with a cleared environment against a scratch HOME and GLEN_HOME and `fake_omp`, never a
//! live `~/.omp`. `fake_omp` is run with `FAKE_OMP_MULTI=1` so one process takes several prompts,
//! and with `FAKE_OMP_OWNERS_DIR` so it holds its owner lease for its lifetime. Every
//! `FAKE_OMP_*` variable counts toward a job's `env_digest` (§A21), so a test holds a later job
//! by deleting and recreating a knob's file between the jobs, never by changing an env value.
//! Every warm worker is ended (release, expiry, a signal) and every omp group is gone before a
//! test returns.
//!
//! Under `scripts/flake-check.sh` the waits and the upper bounds on the product's timing (the
//! AC-v2-402 500 ms claim, the AC-v2-403 1 s and 4 s bounds) are multiplied by
//! `GLEN_TEST_TIME_SCALE` (`flake::scaled`, exact in a plain run); lower bounds and the windows in
//! which something must not happen are not.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use rustix::process::Signal;
use serde_json::{Value, json};

use crate::flake::{Capture, scaled};
use crate::live_support::*;
use crate::reader::*;
use crate::rpc_support::*;
use crate::support::*;

/// AC-v2-403a: the omp group gone, the lock free and the registry removed within this of the
/// ending (teardown is at most 4 s by the plan's bound).
const FOUR_SECONDS: Duration = Duration::from_secs(4);

/// AC-v2-403b: teardown starts within this of a live endpoint appearing.
const ONE_SECOND: Duration = Duration::from_secs(1);

/// AC-v2-402a: a matching queued job is claimed within this of its creation.
const HALF_SECOND: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------------------------
// Files: the registry, the release request, the lock holder, worker logs.
// ---------------------------------------------------------------------------------------------

fn warm_dir(rb: &RpcBox) -> PathBuf {
    rb.bx.glen_home.join("run/warm")
}

/// `run/warm/<lock_key>.json`; a plain-token session id is its own key (§A6).
fn reg_file(rb: &RpcBox, session: &str) -> PathBuf {
    warm_dir(rb).join(format!("{session}.json"))
}

fn release_file(rb: &RpcBox, session: &str) -> PathBuf {
    warm_dir(rb).join(format!("{session}.release"))
}

fn registry(rb: &RpcBox, session: &str) -> Option<Value> {
    read_json(&reg_file(rb, session))
}

/// Names under `run/warm`, sorted (a missing directory is empty).
fn warm_entries(rb: &RpcBox) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(warm_dir(rb))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// The §A6 lock holder record on the held lock file (`{v,kind,job_id,pid,start}`).
fn holder(rb: &RpcBox, session: &str) -> Option<Value> {
    read_json(&rb.lock_path(session))
}

/// The distinct `warm: cooling (<reason>)` reasons logged to any job's `worker.log` (§A21),
/// sorted. A teardown may log its line to the worker's own log and to an adopted job's log, so
/// the same reason can appear in more than one file.
fn cooling(rb: &RpcBox) -> Vec<String> {
    const MARK: &str = "warm: cooling (";
    let mut out = Vec::new();
    for id in rb.job_ids() {
        let log = fs::read_to_string(rb.job_dir(&id).join("worker.log")).unwrap_or_default();
        for line in log.lines() {
            if let Some(at) = line.find(MARK)
                && let Some(end) = line[at + MARK.len()..].find(')')
            {
                out.push(line[at + MARK.len()..at + MARK.len() + end].to_owned());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn omp_gone(pid: i64, pgid: i64) -> bool {
    !pid_alive(pid) && (pgid == own_pgid() || !group_alive(pgid))
}

/// Signal a pid that may already be gone (never panics).
fn send_signal(pid: i64, sig: Signal) {
    if let Some(p) = i32::try_from(pid)
        .ok()
        .filter(|p| *p > 1)
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process(p, sig);
    }
}

fn millis(v: &Value) -> i64 {
    utc(v).as_millisecond()
}

fn is_hex16(s: &str) -> bool {
    s.len() == 16 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

// ---------------------------------------------------------------------------------------------
// The warm harness: `send` with the fixed fake knobs, one warm cycle, endings.
// ---------------------------------------------------------------------------------------------

/// A file a fake peer polls for (`FAKE_OMP_RUN_RELEASE` and friends): present means released.
struct Gate(PathBuf);

impl Gate {
    fn open(&self) {
        fs::write(&self.0, "").unwrap();
    }

    fn close(&self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// What a test learns about the warm worker of job 1.
struct Cycle {
    job: String,
    pid: i64,
    start: String,
    omp_pid: i64,
    omp_pgid: i64,
}

/// Sends to fixture A with a fixed set of `FAKE_OMP_*` knobs, so two sends share one digest.
struct Wm<'a> {
    rb: &'a RpcBox,
    knobs: Vec<(String, String)>,
}

impl<'a> Wm<'a> {
    fn new(rb: &'a RpcBox) -> Self {
        Wm {
            rb,
            knobs: vec![
                ("FAKE_OMP_MULTI".into(), "1".into()),
                ("FAKE_OMP_OWNERS_DIR".into(), s(&rb.bx.owners())),
            ],
        }
    }

    fn knob(&mut self, key: &str, value: &str) {
        self.knobs.push((key.into(), value.into()));
    }

    /// A file knob (`FAKE_OMP_RUN_RELEASE`, `FAKE_OMP_RESULT_RELEASE`, `FAKE_OMP_IDLE_*`), closed.
    fn gate(&mut self, key: &str, file: &str) -> Gate {
        let path = self.rb.bx.root.join(file);
        self.knob(key, &s(&path));
        Gate(path)
    }

    fn apply(&self, run: Run) -> Run {
        self.knobs.iter().fold(run, |r, (k, v)| r.env(k, v))
    }

    fn run(&self, args: &[&str]) -> Run {
        self.apply(self.rb.glen(args.iter().copied()))
    }

    fn run_in(&self, cwd: &Path, args: &[&str]) -> Run {
        self.apply(self.rb.glen_in(cwd, args.iter().copied()))
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = self.rb.command(args);
        for (k, v) in &self.knobs {
            cmd.env(k, v);
        }
        cmd
    }

    /// `glen send <target> <text> <extra> --wait 20s --json`.
    fn send_args<'b>(target: &'b str, text: &'b str, extra: &[&'b str]) -> Vec<&'b str> {
        let mut a = vec!["send", target, text];
        a.extend_from_slice(extra);
        a.extend(["--wait", "20s", "--json"]);
        a
    }

    fn send(&self, target: &str, text: &str, extra: &[&str]) -> Out {
        self.run(&Self::send_args(target, text, extra)).run()
    }

    /// `--no-wait`: the job id of the running (exit 10) document.
    fn send_nowait(&self, target: &str, text: &str, extra: &[&str]) -> String {
        let mut a = vec!["send", target, text];
        a.extend_from_slice(extra);
        a.extend(["--no-wait", "--json"]);
        job_id(&data_of(&self.run(&a).run(), 10))
    }

    /// A foreground send as a child process, so a test can look while it runs. Its output goes
    /// to files: a pipe can be held open by a process another test spawned meanwhile.
    fn spawn_send(&self, target: &str, text: &str, extra: &[&str]) -> Fg {
        let args = Self::send_args(target, text, extra);
        let mut cmd = self.command(&args);
        let capture = Capture::new();
        capture.attach(&mut cmd);
        Fg {
            child: Some(cmd.spawn().expect("spawn glen send")),
            capture,
            desc: args.join(" "),
        }
    }

    /// `glen wait <job>` to completion (exit 0).
    fn wait_done(&self, id: &str) -> Value {
        let out = self
            .rb
            .glen(["wait", &job_handle(id), "--timeout", "20s", "--json"])
            .run();
        data_of(&out, 0)
    }

    /// Job 1: a foreground send with `--warm <warm>` that completes, then the idle registry.
    fn first(&self, warm: &str) -> Cycle {
        let out = self.send(A.handle, "one", &["--warm", warm]);
        let d = data_of(&out, 0);
        assert_eq!(
            d["job"]["omp_reused"], false,
            "job 1 ran on a fresh omp: {d}"
        );
        self.cycle_of(&job_id(&d))
    }

    /// The registry once it is idle and names `job` as the last job.
    fn cycle_of(&self, job: &str) -> Cycle {
        let rb = self.rb;
        let reg = poll_until("an idle warm registry for the job", scaled(LONG), || {
            registry(rb, A.id).filter(|r| {
                r["state"] == "idle" && (r["job_id"] == job || r["job_id"] == job_handle(job))
            })
        });
        let omp_pid = reg["omp_pid"].as_i64().expect("registry omp_pid");
        let pgid = rb
            .starts()
            .iter()
            .find(|st| st["pid"].as_i64() == Some(omp_pid))
            .and_then(|st| st["pgid"].as_i64())
            .unwrap_or_else(|| panic!("no fake start for omp pid {omp_pid}\n{}", rb.diag()));
        Cycle {
            job: job.to_owned(),
            pid: reg["pid"].as_i64().expect("registry pid"),
            start: reg["start"].as_str().expect("registry start").to_owned(),
            omp_pid,
            omp_pgid: pgid,
        }
    }
}

/// A foreground `glen send` run as a child.
struct Fg {
    child: Option<Child>,
    capture: Capture,
    desc: String,
}

impl Fg {
    fn running(&mut self) -> bool {
        self.child
            .as_mut()
            .expect("child")
            .try_wait()
            .expect("try_wait")
            .is_none()
    }

    /// The send's exit within `timeout` (scaled), and what it wrote.
    fn finish(mut self, timeout: Duration) -> Out {
        let timeout = scaled(timeout);
        let mut child = self.child.take().expect("child");
        let started = Instant::now();
        let status = loop {
            if let Some(st) = child.try_wait().unwrap() {
                break st;
            }
            if started.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                panic!("`glen {}` did not exit within {timeout:?}", self.desc);
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let output = self.capture.output(status);
        Out {
            desc: format!("glen {}", self.desc),
            status,
            stdout: output.stdout,
            stderr: output.stderr,
            elapsed: started.elapsed(),
        }
    }
}

impl Drop for Fg {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// SIGCONT on drop, so a failed test never leaves a stopped process behind.
struct Stopped(i64);

impl Drop for Stopped {
    fn drop(&mut self) {
        send_signal(self.0, Signal::CONT);
    }
}

/// Whether a path ever existed while the guard watched (polled every 2 ms).
struct Appeared {
    seen: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Appeared {
    fn watch(path: PathBuf) -> Self {
        let seen = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, p2) = (seen.clone(), stop.clone());
        std::thread::spawn(move || {
            while !p2.load(Ordering::SeqCst) {
                if path.exists() {
                    s2.store(true, Ordering::SeqCst);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        Appeared { seen, stop }
    }

    fn appeared(&self) -> bool {
        self.seen.load(Ordering::SeqCst)
    }
}

impl Drop for Appeared {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// `release --all` (exit 0) and every worker and omp of the box gone: the end of a test. The
/// worker publishes its registry just after a job's terminal status is durable, so a release sent
/// the instant a `--warm` send returns can meet an empty `run/warm`; wait for the registry (or for
/// the worker to be gone) first.
fn end_warmth(rb: &RpcBox) {
    poll_until("a warm registry, or no live worker", scaled(LONG), || {
        (reg_file(rb, A.id).exists() || rb.live_processes().is_empty()).then_some(())
    });
    let out = rb.glen(["release", "--all", "--json"]).run();
    ok_doc(&out, 0);
    rb.assert_quiet();
}

/// The instant `path` holds a status satisfying `pred` (polled every millisecond).
fn watch_status(
    path: PathBuf,
    pred: impl Fn(&Value) -> bool + Send + 'static,
) -> mpsc::Receiver<Instant> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let deadline = Instant::now() + scaled(Duration::from_secs(30));
        while Instant::now() < deadline {
            if read_json(&path).is_some_and(|st| pred(&st)) {
                let _ = tx.send(Instant::now());
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    rx
}

/// The first new job directory (not in `known`) once `job.json` and `status.json` both exist,
/// and the instant its status first says `omp_reused:true`: (id, created at, claimed at).
fn watch_claim(
    jobs: PathBuf,
    known: BTreeSet<String>,
) -> mpsc::Receiver<(String, Instant, Instant)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let deadline = Instant::now() + scaled(Duration::from_secs(30));
        let mut created: Option<(String, Instant)> = None;
        while Instant::now() < deadline {
            if created.is_none()
                && let Ok(rd) = fs::read_dir(&jobs)
            {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if !known.contains(&name)
                        && read_json(&e.path().join("job.json")).is_some()
                        && read_json(&e.path().join("status.json")).is_some()
                    {
                        created = Some((name, Instant::now()));
                        break;
                    }
                }
            }
            if let Some((id, t0)) = &created
                && read_json(&jobs.join(id).join("status.json"))
                    .is_some_and(|st| st["omp_reused"] == true)
            {
                let _ = tx.send((id.clone(), *t0, Instant::now()));
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    rx
}

/// AC-v2-403a: within `within` of `from`, the omp group is gone, the session lock is free, the
/// registry and the release request are removed, and the teardown logged `warm: cooling (<reason>)`
/// with this reason and no other (AC-v2-403d).
fn assert_cooled(rb: &RpcBox, c: &Cycle, reason: &str, from: Instant, within: Duration) {
    let within = scaled(within);
    let deadline = from + within;
    loop {
        let omp = omp_gone(c.omp_pid, c.omp_pgid);
        let lock = rb.glen_lock_free(A.id);
        let reg = !reg_file(rb, A.id).exists();
        let rel = !release_file(rb, A.id).exists();
        let log = cooling(rb) == [reason.to_owned()];
        if omp && lock && reg && rel && log {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "within {within:?} of the ending, expected omp gone={omp} lock free={lock} \
                 registry removed={reg} release request removed={rel} and only \
                 `warm: cooling ({reason})`={log} (logged: {:?})\n{}",
                cooling(rb),
                rb.diag()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The send's job id whether it succeeded or failed.
fn any_job_id(out: &Out) -> String {
    let env = out.json();
    let job = if env["ok"] == true {
        &env["data"]["job"]
    } else {
        &env["error"]["details"]["job"]
    };
    job["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no job view in {env}"))
        .to_owned()
}

/// A job that is not warm any more: its worker and omp are gone, the lock is free and nothing
/// is registered, quickly after its terminal status (AC-v2-401a, AC-v2-403c).
fn assert_cold_end(rb: &RpcBox, id: &str, what: &str) {
    let worker = rb.status(id)["worker_pid"].as_i64().unwrap_or(0);
    let omps: Vec<(i64, i64)> = rb
        .starts()
        .iter()
        .map(|st| {
            (
                st["pid"].as_i64().unwrap_or(0),
                st["pgid"].as_i64().unwrap_or(0),
            )
        })
        .collect();
    let started = Instant::now();
    loop {
        let worker_gone = worker <= 0 || !pid_alive(worker);
        let omp = omps.iter().all(|(pid, pgid)| omp_gone(*pid, *pgid));
        let lock = rb.glen_lock_free(A.id);
        if worker_gone && omp && lock {
            break;
        }
        if started.elapsed() >= scaled(FOUR_SECONDS) {
            panic!(
                "{what}: worker gone={worker_gone} omp gone={omp} lock free={lock} \
                 {:?} after the terminal status (it stayed warm?)\n{}",
                scaled(FOUR_SECONDS),
                rb.diag()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        warm_entries(rb).is_empty(),
        "{what}: nothing may be registered: {:?}",
        warm_entries(rb)
    );
}

/// `(label, extra send args, extra env)` of one send.
type SendCase = (
    &'static str,
    &'static [&'static str],
    &'static [(&'static str, &'static str)],
);

/// A send case that also names the `warm_ms` it must record.
type MsCase = (
    &'static str,
    &'static [&'static str],
    &'static [(&'static str, &'static str)],
    u64,
);

// ---------------------------------------------------------------------------------------------
// AC-v2-401: off by default, the flag beats the env, 0 is off, 10m is the cap, the recorded
// fields, and the visible surface.
// ---------------------------------------------------------------------------------------------

/// Command names of a `glen --help` page, in order.
fn help_command_names(help: &str) -> Vec<String> {
    help.lines()
        .skip_while(|l| l.trim_end() != "Commands:")
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .filter_map(|l| l.split_whitespace().next().map(str::to_owned))
        .collect()
}

#[test]
fn ac_v2_401_help_completions_and_json_md_expose_warm_and_release() {
    let sb = Sandbox::new();
    let help = sb.glen(["--help"]).run();
    help.assert_exit(0);
    let names = help_command_names(&help.out());
    let pos = |n: &str| {
        names
            .iter()
            .position(|c| c == n)
            .unwrap_or_else(|| panic!("`glen --help` lists no `{n}`: {names:?}"))
    };
    assert!(
        pos("inbox") < pos("release") && pos("release") < pos("watch"),
        "release sits between inbox and watch: {names:?}"
    );
    assert!(
        help.out().contains("End warm workers"),
        "release has a one-line description\n{}",
        help.ctx()
    );

    let send = sb.glen(["send", "--help"]).run();
    send.assert_exit(0);
    let text = send.out();
    assert!(
        text.contains("--warm <DUR>"),
        "send --help lists --warm\n{}",
        send.ctx()
    );
    assert!(
        text.contains("10m"),
        "send --help names the 10m cap\n{}",
        send.ctx()
    );

    let release = sb.glen(["release", "--help"]).run();
    release.assert_exit(0);
    let text = release.out();
    assert!(
        text.contains("Usage:"),
        "release --help prints Usage\n{}",
        release.ctx()
    );
    assert!(text.contains("glen release"), "{}", release.ctx());
    for flag in ["--all", "--force", "--timeout"] {
        assert!(
            text.contains(flag),
            "release --help lists {flag}\n{}",
            release.ctx()
        );
    }

    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let out = sb.glen(["completions", shell]).run();
        out.assert_exit(0);
        let script = out.out();
        assert!(
            script.contains("release"),
            "{shell} completions list release"
        );
        assert!(script.contains("warm"), "{shell} completions list --warm");
    }

    let doc = fs::read_to_string(repo_root().join("docs/json.md")).expect("docs/json.md");
    assert!(
        doc.lines().any(|l| l.trim() == "## `glen release`"),
        "docs/json.md has a `glen release` section"
    );
    for needle in [
        "--warm",
        "GLEN_WARM",
        "warm_ms",
        "env_digest",
        "omp_reused",
        "release_timeout",
    ] {
        assert!(doc.contains(needle), "docs/json.md never says {needle}");
    }
}

#[test]
fn ac_v2_401_without_warm_nothing_is_recorded_and_the_worker_ends_as_before() {
    let cases: [SendCase; 6] = [
        ("no flag, no env", &[], &[]),
        ("--warm 0", &["--warm", "0"], &[]),
        ("GLEN_WARM=0", &[], &[("GLEN_WARM", "0")]),
        ("GLEN_WARM empty", &[], &[("GLEN_WARM", "")]),
        ("GLEN_WARM blank", &[], &[("GLEN_WARM", "   ")]),
        (
            "--warm 0 over GLEN_WARM=5s",
            &["--warm", "0"],
            &[("GLEN_WARM", "5s")],
        ),
    ];
    for (label, extra, env) in cases {
        let rb = RpcBox::new();
        let w = Wm::new(&rb);
        let mut run = w.run(&Wm::send_args(A.handle, "hi", extra));
        for (k, v) in env {
            run = run.env(k, v);
        }
        let out = run.run();
        let d = data_of(&out, 0);
        let id = job_id(&d);
        let job = rb.job_json(&id);
        assert!(job.get("warm_ms").is_none(), "{label}: no warm_ms: {job}");
        let digest = job["env_digest"].as_str().unwrap_or("");
        assert!(
            is_hex16(digest),
            "{label}: env_digest is 16 lowercase hex: {job}"
        );
        assert_eq!(d["job"]["omp_reused"], false, "{label}: {d}");
        rb.wait_terminal(&id);
        assert_cold_end(&rb, &id, label);
        rb.assert_quiet();
    }
}

#[test]
fn ac_v2_401_the_flag_beats_the_environment_and_the_value_is_recorded_in_milliseconds() {
    let cases: [MsCase; 3] = [
        (
            "--warm 1s over GLEN_WARM=5s",
            &["--warm", "1s"],
            &[("GLEN_WARM", "5s")],
            1000,
        ),
        ("GLEN_WARM=3s alone", &[], &[("GLEN_WARM", "3s")], 3000),
        ("--warm 2500ms alone", &["--warm", "2500ms"], &[], 2500),
    ];
    for (label, extra, env, want) in cases {
        let rb = RpcBox::new();
        let w = Wm::new(&rb);
        let mut run = w.run(&Wm::send_args(A.handle, "hi", extra));
        for (k, v) in env {
            run = run.env(k, v);
        }
        let out = run.run();
        let id = job_id(&data_of(&out, 0));
        let job = rb.job_json(&id);
        assert_eq!(
            job["warm_ms"], want,
            "{label}: warm_ms in milliseconds: {job}"
        );
        assert!(
            is_hex16(job["env_digest"].as_str().unwrap_or("")),
            "{label}: {job}"
        );
        end_warmth(&rb);
    }
}

#[test]
fn ac_v2_401_ten_minutes_and_its_spellings_are_accepted() {
    for (flag, want) in [
        ("10m", 600_000u64),
        ("600s", 600_000),
        ("600000ms", 600_000),
        ("5", 5000),
        ("1m", 60_000),
    ] {
        let rb = RpcBox::new();
        let w = Wm::new(&rb);
        let out = w.send(A.handle, "hi", &["--warm", flag]);
        let id = job_id(&data_of(&out, 0));
        assert_eq!(rb.job_json(&id)["warm_ms"], want, "--warm {flag}");
        end_warmth(&rb);
    }
}

#[test]
fn ac_v2_401_a_warm_above_ten_minutes_or_garbage_is_refused_before_a_job_exists() {
    let flags = ["11m", "601s", "600001ms", "abc", "", "1d", "-5s"];
    for bad in flags {
        let rb = RpcBox::new();
        let w = Wm::new(&rb);
        let flag = format!("--warm={bad}");
        let out = w.send(A.handle, "hi", &[flag.as_str()]);
        let err = failed(&out, "usage");
        assert!(
            err["message"]
                .as_str()
                .is_some_and(|m| !m.trim().is_empty()),
            "{flag}: {err}"
        );
        assert!(rb.job_ids().is_empty(), "{flag}: no job was created");
        assert!(rb.starts().is_empty(), "{flag}: no omp was started");
    }
    for bad in ["11m", "abc", "601s", "-1", "1d"] {
        let rb = RpcBox::new();
        let w = Wm::new(&rb);
        let out = w
            .run(&Wm::send_args(A.handle, "hi", &[]))
            .env("GLEN_WARM", bad)
            .run();
        let err = failed(&out, "invalid_env");
        assert!(
            err["message"].as_str().unwrap_or("").contains("GLEN_WARM"),
            "GLEN_WARM={bad}: the message names the variable: {err}"
        );
        assert!(
            rb.job_ids().is_empty(),
            "GLEN_WARM={bad}: no job was created"
        );
        assert!(
            rb.starts().is_empty(),
            "GLEN_WARM={bad}: no omp was started"
        );
    }
}

#[test]
fn ac_v2_401_only_send_reads_glen_warm() {
    let rb = RpcBox::new();
    for args in [
        vec!["jobs", "--json"],
        vec!["ls", "--all", "--json"],
        vec!["release", "--all", "--json"],
        vec!["doctor", "--json"],
    ] {
        let out = rb.glen(&args).env("GLEN_WARM", "abc").run();
        out.assert_no_panic();
        ok_doc(&out, 0);
    }
}

#[test]
fn ac_v2_401_env_digest_ignores_volatile_and_glen_variables_and_tracks_every_other() {
    let rb = RpcBox::new();
    let w = Wm::new(&rb);
    let digest = |extra: &[(&str, &str)]| -> String {
        let mut run = w.run(&Wm::send_args(A.handle, "hi", &[]));
        for (k, v) in extra {
            run = run.env(k, v);
        }
        let id = job_id(&data_of(&run.run(), 0));
        let d = rb.job_json(&id)["env_digest"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        assert!(is_hex16(&d), "env_digest is 16 lowercase hex: {d:?}");
        d
    };
    let base = digest(&[]);
    assert_eq!(digest(&[]), base, "equal environments, equal digests");
    let home2 = rb.bx.root.join("home2");
    fs::create_dir_all(&home2).unwrap();
    let home2 = s(&home2);
    let ignored: [&[(&str, &str)]; 8] = [
        &[("GLEN_JOB_TIMEOUT", "31s")],
        &[("GLEN_WARM", "0")],
        &[("GLEN_SOMETHING_ELSE", "x")],
        &[("PWD", "/somewhere/else")],
        &[("OLDPWD", "/somewhere/else")],
        &[("SHLVL", "7")],
        &[("COLUMNS", "133"), ("LINES", "51")],
        &[("TERM_SESSION_ID", "w0t0p0:abc"), ("_", "/usr/bin/env")],
    ];
    for extra in ignored {
        assert_eq!(digest(extra), base, "{extra:?} is not part of the digest");
    }
    let tracked: [&[(&str, &str)]; 5] = [
        &[("PI_CONFIG_FILES", "/x/y.yml")],
        &[("OPENAI_API_KEY", "k-0")],
        &[("ANTHROPIC_API_KEY", "k-1")],
        &[("PATH", "/usr/bin")],
        &[("HOME", home2.as_str())],
    ];
    let mut seen = BTreeSet::from([base.clone()]);
    for extra in tracked {
        let d = digest(extra);
        assert_ne!(d, base, "{extra:?} changes the digest");
        assert!(seen.insert(d), "{extra:?} gives its own digest");
    }
    rb.assert_quiet();
}

#[test]
fn ac_v2_401_the_live_route_ignores_warm_and_records_neither_field() {
    let lb = LiveBox::new();
    let _ext = lb.start_ext(A.id, "ok", &[], &[]);
    let out = lb
        .send(A.handle, "hi", &["--warm", "5s", "--wait", "20s"])
        .run();
    let env = ok_doc(&out, 0);
    let d = &env["data"];
    assert_eq!(d["job"]["route"], "live", "{d}");
    assert_eq!(
        d["job"]["omp_reused"], false,
        "a live job never reuses an omp: {d}"
    );
    assert!(
        warning_codes(&env).iter().any(|c| c == "ignored_flag"),
        "an explicit --warm is ignored on the live route with `ignored_flag`: {env}"
    );
    let job = lb.job_json(&job_id(d));
    assert!(
        job.get("warm_ms").is_none(),
        "no warm_ms on a live job: {job}"
    );
    assert!(
        job.get("env_digest").is_none(),
        "no env_digest on a live job: {job}"
    );
    assert!(
        warm_entries(&lb).is_empty(),
        "the live route never registers a warm worker"
    );
    lb.assert_no_omp();

    // The env value is ignored silently there.
    let out = lb
        .send(A.handle, "hi again", &["--wait", "20s"])
        .env("GLEN_WARM", "5s")
        .run();
    let env = ok_doc(&out, 0);
    assert_eq!(env["data"]["job"]["route"], "live", "{env}");
    assert!(
        !env["warnings"].to_string().to_lowercase().contains("warm"),
        "GLEN_WARM alone adds no warning on the live route: {env}"
    );
    let job = lb.job_json(&job_id(&env["data"]));
    assert!(
        job.get("warm_ms").is_none() && job.get("env_digest").is_none(),
        "{job}"
    );
    assert!(warm_entries(&lb).is_empty());
}

// ---------------------------------------------------------------------------------------------
// AC-v2-402: steal-from-queue, the claim, the claimant, exactly-once, the job views.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac_v2_402_the_registry_and_the_lock_describe_an_idle_warm_worker() {
    let rb = RpcBox::new();
    let w = Wm::new(&rb);
    let c = w.first("30s");

    let st = rb.status(&c.job);
    assert_eq!(
        st["worker_pid"], c.pid,
        "the registry names the worker that ran job 1: {st}"
    );
    assert_eq!(st["worker_start"], c.start.as_str(), "{st}");

    let reg = registry(&rb, A.id).expect("registry");
    for k in [
        "v",
        "pid",
        "start",
        "session_id",
        "session_handle",
        "job_id",
        "omp_pid",
        "omp_start",
        "warm_ms",
        "expires_at",
        "state",
        "started_at",
    ] {
        assert!(reg.get(k).is_some(), "registry lacks `{k}`: {reg}");
    }
    assert_eq!(reg["v"], 1, "{reg}");
    assert_eq!(reg["session_id"], A.id, "{reg}");
    assert_eq!(reg["session_handle"], A.handle, "{reg}");
    assert_eq!(reg["warm_ms"], 30_000, "{reg}");
    assert_eq!(reg["state"], "idle", "{reg}");
    utc(&reg["expires_at"]);
    utc(&reg["started_at"]);
    assert_eq!(mode(&warm_dir(&rb)), 0o700, "run/warm is private");
    assert_eq!(mode(&reg_file(&rb, A.id)), 0o600, "the registry is private");

    // The registry goes idle first; the lock holder record follows within a moment.
    let h = poll_until(
        "the lock holder to read `warm`",
        scaled(Duration::from_secs(2)),
        || holder(&rb, A.id).filter(|h| h["kind"] == "warm"),
    );
    assert_eq!(h["pid"], c.pid, "{h}");
    assert_eq!(h["start"], c.start.as_str(), "{h}");
    assert!(
        !rb.glen_lock_free(A.id),
        "the warm worker holds glen's session lock"
    );
    assert!(pid_alive(c.pid), "the worker is alive");
    assert!(pid_alive(c.omp_pid), "omp is alive");
    assert_eq!(rb.starts().len(), 1, "one omp start so far");

    // omp's owner lease is held while warm, so the session reads as live (§A21 lease cost).
    let out = rb.glen(["ls", "--all", "--json"]).run();
    let d = data_of(&out, 0);
    let row = arr(&d["sessions"])
        .iter()
        .find(|r| r["id"] == A.id)
        .unwrap_or_else(|| panic!("session A is not in ls: {d}"));
    assert!(
        ["live", "busy"].contains(&row["state"].as_str().unwrap_or("")),
        "the held lease reads as live or busy: {row}"
    );
    end_warmth(&rb);
}

#[test]
fn ac_v2_402_a_second_send_within_the_window_is_stolen_within_500ms_on_the_same_omp() {
    let rb = RpcBox::new();
    let w = Wm::new(&rb);
    let c = w.first("30s");
    let known: BTreeSet<String> = rb.job_ids().into_iter().collect();
    let claim = watch_claim(rb.jobs_dir(), known);
    let id2 = w.send_nowait(A.handle, "two", &["--warm", "30s"]);
    let (claimed_id, created, claimed) = claim
        .recv_timeout(scaled(LONG))
        .unwrap_or_else(|_| panic!("job 2 was never claimed\n{}", rb.diag()));
    assert_eq!(claimed_id, id2);
    let took = claimed.duration_since(created);
    assert!(
        took <= scaled(HALF_SECOND),
        "AC-v2-402a: claimed {took:?} after its creation (limit {:?})\n{}",
        scaled(HALF_SECOND),
        rb.diag()
    );

    let d = w.wait_done(&id2);
    assert_eq!(d["assistant_text"], "pong: two", "{d}");
    assert_eq!(d["job"]["omp_reused"], true, "{d}");
    let st = rb.status(&id2);
    assert_eq!(st["omp_reused"], true, "{st}");
    assert_eq!(st["status"], "completed", "{st}");
    assert_eq!(
        st["worker_pid"], c.pid,
        "AC-v2-402b: the claimant is the warm worker: {st}"
    );
    assert_eq!(st["worker_start"], c.start.as_str(), "{st}");

    let recs = rb.recs();
    assert_eq!(
        starts(&recs).len(),
        1,
        "exactly one omp start\n{}",
        rb.diag()
    );
    let prompts = frames_in(&recs, "in", "prompt");
    assert_eq!(prompts.len(), 2, "exactly two prompt frames\n{}", rb.diag());
    assert_eq!(prompts[0]["message"], "one", "{prompts:?}");
    assert_eq!(prompts[1]["message"], "two", "{prompts:?}");
    end_warmth(&rb);
}

#[test]
fn ac_v2_402_the_claim_names_the_warm_worker_and_the_cold_worker_never_rewrites_the_status() {
    let rb = RpcBox::new();
    let mut w = Wm::new(&rb);
    let run = w.gate("FAKE_OMP_RUN_RELEASE", "run-gate");
    run.open();
    let c = w.first("30s");
    run.close();

    let id2 = w.send_nowait(A.handle, "two", &["--warm", "30s"]);
    let st = rb.wait_status(&id2, "claimed and running", scaled(LONG), |s| {
        s["omp_reused"] == true && s["status"] == "accepted"
    });
    assert_eq!(st["worker_pid"], c.pid, "AC-v2-402b worker_pid: {st}");
    assert_eq!(
        st["worker_start"],
        c.start.as_str(),
        "AC-v2-402b worker_start: {st}"
    );

    // §A21: while the warm worker runs the adopted job, the lock holder is that job and the
    // registry is busy.
    let h = poll_until(
        "the lock holder to name the adopted job",
        scaled(LONG),
        || holder(&rb, A.id).filter(|h| h["kind"] == "job"),
    );
    assert!(h["job_id"] == id2 || h["job_id"] == job_handle(&id2), "{h}");
    assert_eq!(h["pid"], c.pid, "{h}");
    let reg = poll_until("a busy registry", scaled(LONG), || {
        registry(&rb, A.id).filter(|r| r["state"] == "busy")
    });
    assert_eq!(reg["pid"], c.pid, "{reg}");

    // The blocked cold worker exits without writing: for 1.5 s the status keeps naming the
    // claimant, stays reused and never becomes lost.
    let until = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < until {
        let st = rb.status(&id2);
        assert_eq!(
            st["worker_pid"], c.pid,
            "the cold worker rewrote the claim: {st}"
        );
        assert_eq!(st["omp_reused"], true, "{st}");
        assert!(
            ["queued", "accepted"].contains(&st["status"].as_str().unwrap_or("")),
            "a stolen job is neither lost nor finished while it is held: {st}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let wait = rb
        .glen(["wait", &job_handle(&id2), "--timeout", "300ms", "--json"])
        .run();
    let d = data_of(&wait, 10);
    assert_eq!(d["job"]["omp_reused"], true, "{d}");
    assert_ne!(d["job"]["status"], "lost", "{d}");

    run.open();
    let d = w.wait_done(&id2);
    assert_eq!(
        d["assistant_text"], "pong: two",
        "wait returns the warm worker's result: {d}"
    );
    assert_eq!(
        rb.status(&id2)["worker_pid"],
        c.pid,
        "still the claimant after the end"
    );
    end_warmth(&rb);
}

#[test]
fn ac_v2_402_a_foreground_send_that_was_stolen_returns_the_warm_workers_result() {
    let rb = RpcBox::new();
    let mut w = Wm::new(&rb);
    let run = w.gate("FAKE_OMP_RUN_RELEASE", "run-gate");
    run.open();
    w.first("30s");
    run.close();

    let known: BTreeSet<String> = rb.job_ids().into_iter().collect();
    let mut fg = w.spawn_send(A.handle, "two", &["--warm", "30s"]);
    let id2 = poll_until("job 2's directory", scaled(LONG), || {
        rb.job_ids().into_iter().find(|i| !known.contains(i))
    });
    rb.wait_status(&id2, "claimed", scaled(LONG), |s| s["omp_reused"] == true);
    // The cold worker this send spawned is gone within a lock-poll tick or two; the send must
    // keep watching the claimant rather than report the job lost.
    let until = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < until {
        assert!(
            fg.running(),
            "the send gave up while its job was running\n{}",
            rb.diag()
        );
        let st = rb.status(&id2);
        assert_ne!(
            st["status"], "lost",
            "a live claimant's job is never lost: {st}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    run.open();
    let out = fg.finish(LONG);
    let d = data_of(&out, 0);
    assert_eq!(d["status"], "completed", "{d}");
    assert_eq!(d["assistant_text"], "pong: two", "{d}");
    assert_eq!(d["job"]["omp_reused"], true, "{d}");
    end_warmth(&rb);
}

#[test]
fn ac_v2_402_a_stolen_job_is_lost_when_its_claimant_dies() {
    let rb = RpcBox::new();
    let mut w = Wm::new(&rb);
    let run = w.gate("FAKE_OMP_RUN_RELEASE", "run-gate");
    run.open();
    let c = w.first("30s");
    run.close();
    let id2 = w.send_nowait(A.handle, "two", &["--warm", "30s"]);
    rb.wait_status(&id2, "claimed and running", scaled(LONG), |s| {
        s["omp_reused"] == true && s["status"] == "accepted"
    });

    send_signal(c.pid, Signal::KILL);
    signal_group(c.omp_pgid, Signal::KILL);
    let out = rb
        .glen(["wait", &job_handle(&id2), "--timeout", "15s", "--json"])
        .run();
    let err = failed(&out, "job_failed");
    assert_eq!(err["details"]["status"], "lost", "{err}");
    assert_eq!(err["details"]["job"]["status"], "lost", "{err}");
    assert_eq!(rb.status(&id2)["status"], "lost");
    rb.assert_quiet();
}

#[test]
fn ac_v2_402_jobs_queued_behind_a_running_job_are_claimed_in_id_order_right_after_it_settles() {
    let rb = RpcBox::new();
    let mut w = Wm::new(&rb);
    let run = w.gate("FAKE_OMP_RUN_RELEASE", "run-gate");
    let id1 = w.send_nowait(A.handle, "one", &["--warm", "30s"]);
    rb.wait_status(&id1, "accepted", scaled(LONG), |s| {
        s["status"] == "accepted"
    });
    let id_b = w.send_nowait(A.handle, "bee", &["--warm", "30s"]);
    let id_c = w.send_nowait(A.handle, "sea", &["--warm", "30s"]);
    assert!(id_b < id_c, "job ids are time-ordered");
    for id in [&id_b, &id_c] {
        let st = rb.status(id);
        assert_eq!(
            st["status"], "queued",
            "queued behind the running job: {st}"
        );
        assert_ne!(st["omp_reused"], true, "{st}");
    }

    let settled = watch_status(rb.job_dir(&id1).join("status.json"), is_terminal);
    let claimed_b = watch_status(rb.job_dir(&id_b).join("status.json"), |s| {
        s["omp_reused"] == true
    });
    run.open();
    let settled = settled.recv_timeout(scaled(LONG)).expect("job 1 settled");
    let claimed_b = claimed_b
        .recv_timeout(scaled(LONG))
        .expect("job B was claimed");
    assert!(
        claimed_b >= settled || claimed_b.duration_since(settled) == Duration::ZERO,
        "a job behind a running job cannot be claimed before it settles"
    );
    assert!(
        claimed_b.duration_since(settled) <= scaled(HALF_SECOND),
        "a queued job waits at most one claim tick behind the warm worker: {:?}\n{}",
        claimed_b.duration_since(settled),
        rb.diag()
    );

    for id in [&id1, &id_b, &id_c] {
        let st = rb.wait_terminal(id);
        assert_eq!(st["status"], "completed", "{st}");
    }
    assert_ne!(
        rb.status(&id1)["omp_reused"],
        true,
        "job 1 ran on its own omp"
    );
    for (name, id) in [("B", &id_b), ("C", &id_c)] {
        assert_eq!(
            rb.status(id)["omp_reused"],
            true,
            "job {name} was claimed by the warm worker and stays reused\n{}",
            rb.diag()
        );
    }
    let recs = rb.recs();
    assert_eq!(
        starts(&recs).len(),
        1,
        "one omp served all three\n{}",
        rb.diag()
    );
    let texts: Vec<Value> = frames_in(&recs, "in", "prompt")
        .iter()
        .map(|f| f["message"].clone())
        .collect();
    assert_eq!(
        texts,
        [json!("one"), json!("bee"), json!("sea")],
        "oldest first"
    );
    end_warmth(&rb);
}

#[test]
fn ac_v2_402_a_claimed_job_without_warm_ends_warmth_after_it_runs() {
    let rb = RpcBox::new();
    let w = Wm::new(&rb);
    let c = w.first("30s");
    let id2 = w.send_nowait(A.handle, "two", &[]);
    let d = w.wait_done(&id2);
    assert_eq!(
        d["job"]["omp_reused"], true,
        "a queued job with warm absent is still claimed: {d}"
    );
    let from = Instant::now();
    assert_cooled(&rb, &c, "ended", from, FOUR_SECONDS);
    rb.assert_quiet();
    assert_eq!(starts(&rb.recs()).len(), 1);
}

#[test]
fn ac_v2_402_every_job_view_carries_omp_reused_true_only_for_a_reused_omp() {
    let rb = RpcBox::new();
    let w = Wm::new(&rb);
    w.first("30s");
    let out = w.send(A.handle, "two", &["--warm", "30s"]);
    let d2 = data_of(&out, 0);
    let (id1, id2) = (rb.job_ids()[0].clone(), job_id(&d2));
    assert_eq!(d2["job"]["omp_reused"], true, "send: {d2}");

    let wait2 = data_of(&rb.glen(["wait", &job_handle(&id2), "--json"]).run(), 0);
    assert_eq!(wait2["job"]["omp_reused"], true, "wait: {wait2}");
    let wait1 = data_of(&rb.glen(["wait", &job_handle(&id1), "--json"]).run(), 0);
    assert_eq!(
        wait1["job"]["omp_reused"], false,
        "job 1 ran on a fresh omp: {wait1}"
    );

    let jobs = data_of(&rb.glen(["jobs", "--json"]).run(), 0);
    let rows = arr(&jobs["jobs"]);
    assert_eq!(rows.len(), 2, "{jobs}");
    for r in rows {
        let want = r["id"] == id2.as_str();
        assert_eq!(r["omp_reused"], want, "jobs row: {r}");
    }
    end_warmth(&rb);
}

#[test]
fn ac_v2_402_a_failure_view_carries_omp_reused_false() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "error")
        .run();
    let err = failed(&out, "job_failed");
    assert_eq!(err["details"]["job"]["omp_reused"], false, "{err}");
    rb.assert_quiet();
}

#[test]
fn ac_v2_402_each_queued_job_runs_exactly_once_whether_the_warm_or_a_cold_worker_takes_it() {
    let rb = RpcBox::new();
    let w = Wm::new(&rb);
    // A short window with sends spread across it puts claims next to expiries.
    let texts = ["j0", "j1", "j2", "j3", "j4", "j5"];
    let mut ids = Vec::new();
    for (i, text) in texts.iter().enumerate() {
        ids.push(w.send_nowait(A.handle, text, &["--warm", "400ms"]));
        std::thread::sleep(Duration::from_millis(60 + 40 * (i as u64 % 3)));
    }
    let mut reused = 0;
    for (id, text) in ids.iter().zip(texts) {
        let d = w.wait_done(id);
        assert_eq!(d["assistant_text"], format!("pong: {text}"), "{d}");
        reused += usize::from(d["job"]["omp_reused"] == true);
    }
    rb.assert_quiet();
    let recs = rb.recs();
    let prompts = frames_in(&recs, "in", "prompt");
    for text in texts {
        let n = prompts.iter().filter(|f| f["message"] == text).count();
        assert_eq!(n, 1, "`{text}` ran {n} times\n{}", rb.diag());
    }
    assert_eq!(
        starts(&recs).len() + reused,
        texts.len(),
        "every job is either the first of an omp start or reused one (reused: {reused})\n{}",
        rb.diag()
    );
}

#[test]
fn ac_v2_402_a_new_session_goes_warm_keyed_by_its_settled_id_and_a_cwd_through_a_symlink_matches() {
    let rb = RpcBox::new();
    let w = Wm::new(&rb);
    let link = rb.bx.root.join("project-link");
    std::os::unix::fs::symlink(&rb.bx.alpha, &link).unwrap();
    let out = w.send("@new", "one", &["--cwd", &s(&link), "--warm", "30s"]);
    let d = data_of(&out, 0);
    let id1 = job_id(&d);
    assert_eq!(d["job"]["route"], "new", "{d}");
    let job = rb.job_json(&id1);
    assert_eq!(job["warm_ms"], 30_000, "a @new job may stay warm: {job}");
    assert!(is_hex16(job["env_digest"].as_str().unwrap_or("")), "{job}");
    let sid = d["session"]["id"]
        .as_str()
        .expect("settled session id")
        .to_owned();
    let handle = d["session"]["handle"]
        .as_str()
        .expect("session handle")
        .to_owned();

    let reg = poll_until(
        "the registry keyed by the settled session id",
        scaled(LONG),
        || registry(&rb, &sid).filter(|r| r["state"] == "idle"),
    );
    assert_eq!(reg["session_id"], sid.as_str(), "{reg}");

    // The fake peer writes no transcript, so stand in for omp: the session file omp would have
    // written, with the canonical cwd omp reads from getcwd (which differs from the symlinked
    // `--cwd` the first send recorded).
    let canon = fs::canonicalize(&rb.bx.alpha).unwrap();
    rb.bx
        .add(ALPHA, "2026-11-02T10:00:00Z", &sid, Some(&s(&canon)), &[]);
    let out = w.send(&handle, "two", &["--warm", "30s"]);
    let d2 = data_of(&out, 0);
    assert_eq!(
        d2["job"]["omp_reused"], true,
        "the follow-up is stolen by the warm worker: {d2}"
    );
    assert_eq!(d2["assistant_text"], "pong: two", "{d2}");
    let recs = rb.recs();
    assert_eq!(starts(&recs).len(), 1, "one omp start\n{}", rb.diag());
    assert_eq!(
        frames_in(&recs, "in", "prompt").len(),
        2,
        "two prompts\n{}",
        rb.diag()
    );
    let id2 = job_id(&d2);
    poll_until("the worker to be idle again", scaled(LONG), || {
        registry(&rb, &sid).filter(|r| {
            r["state"] == "idle" && (r["job_id"] == id2 || r["job_id"] == job_handle(&id2))
        })
    });
    let out = rb.glen(["release", &handle, "--json"]).run();
    let d = data_of(&out, 0);
    assert_eq!(d["workers"][0]["outcome"], "released", "{d}");
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-v2-403: every ending goes through one teardown.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac_v2_403_the_window_expires_and_everything_is_cleaned_up_within_four_seconds() {
    let rb = RpcBox::new();
    let w = Wm::new(&rb);
    let c = w.first("3s");
    let seen = Instant::now();
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        reg_file(&rb, A.id).exists(),
        "the worker is still warm inside its window"
    );
    assert!(
        pid_alive(c.pid) && pid_alive(c.omp_pid),
        "worker and omp live inside the window"
    );
    // Teardown starts at expiry, at most 3 s after the window opened, and takes at most 4 s.
    assert_cooled(
        &rb,
        &c,
        "expired",
        seen,
        Duration::from_secs(3) + FOUR_SECONDS,
    );
    rb.assert_quiet();
}

#[test]
fn ac_v2_403_glen_release_by_every_target_form_and_all_ends_warmth() {
    let targets = ["handle", "session id", "hex tail", "job handle", "--all"];
    for form in targets {
        let rb = RpcBox::new();
        let w = Wm::new(&rb);
        let c = w.first("30s");
        let target = match form {
            "handle" => A.handle.to_owned(),
            "session id" => A.id.to_owned(),
            "hex tail" => A.id[A.id.len() - 4..].to_owned(),
            "job handle" => job_handle(&c.job),
            _ => "--all".to_owned(),
        };
        let from = Instant::now();
        let out = rb.glen(["release", &target, "--json"]).run();
        let d = data_of(&out, 0);
        assert_eq!(d["total"], 1, "{form}: {d}");
        let wk = &d["workers"][0];
        assert_eq!(wk["outcome"], "released", "{form}: {d}");
        assert_eq!(wk["state"], "idle", "{form}: {d}");
        assert_eq!(wk["pid"], c.pid, "{form}: {d}");
        assert_eq!(wk["session"]["id"], A.id, "{form}: {d}");
        assert_eq!(wk["session"]["handle"], A.handle, "{form}: {d}");
        assert_eq!(wk["job"], job_handle(&c.job), "{form}: {d}");
        // `release` returns once the lock is free and the registry is gone.
        assert!(
            rb.glen_lock_free(A.id),
            "{form}: the lock is free when release returns"
        );
        assert!(
            !reg_file(&rb, A.id).exists(),
            "{form}: the registry is gone when release returns"
        );
        assert_cooled(&rb, &c, "release", from, FOUR_SECONDS);
        rb.assert_quiet();
    }
}

#[test]
fn ac_v2_403_sigterm_and_sighup_end_warmth() {
    for (label, sig) in [("SIGTERM", Signal::TERM), ("SIGHUP", Signal::HUP)] {
        let rb = RpcBox::new();
        let w = Wm::new(&rb);
        let c = w.first("30s");
        let from = Instant::now();
        send_signal(c.pid, sig);
        assert_cooled(&rb, &c, "signal", from, FOUR_SECONDS);
        poll_until(
            &format!("the worker to exit after {label}"),
            scaled(LONG),
            || (!pid_alive(c.pid)).then_some(()),
        );
        rb.assert_quiet();
    }
}

#[test]
fn ac_v2_403_omp_exiting_ends_warmth() {
    let rb = RpcBox::new();
    let mut w = Wm::new(&rb);
    let exit = w.gate("FAKE_OMP_IDLE_EXIT_RELEASE", "idle-exit");
    let c = w.first("30s");
    let from = Instant::now();
    exit.open();
    assert_cooled(&rb, &c, "omp_exited", from, FOUR_SECONDS);
    rb.assert_quiet();
}

#[test]
fn ac_v2_403_a_live_endpoint_for_the_session_ends_warmth_within_one_second() {
    let lb = LiveBox::new();
    let w = Wm::new(&lb);
    let c = w.first("30s");
    let ext = lb.start_ext(A.id, "ok", &[], &[]);
    let from = Instant::now();
    poll_until(
        "teardown to start within a second of the endpoint appearing",
        scaled(ONE_SECOND),
        || cooling(&lb).contains(&"endpoint".to_owned()).then_some(()),
    );
    assert_cooled(&lb, &c, "endpoint", from, FOUR_SECONDS);
    assert!(
        ext.registry.exists(),
        "the endpoint is not the warm worker's to remove"
    );
    lb.assert_quiet();
}

fn never_warm_case(
    label: &str,
    scenario: &str,
    env: &[(&str, &str)],
    exit: i32,
    warm_args: &[&str],
) {
    let rb = RpcBox::new();
    let mut w = Wm::new(&rb);
    w.knob("FAKE_OMP_SCENARIO", scenario);
    let seen = Appeared::watch(reg_file(&rb, A.id));
    let mut run = w.run(&Wm::send_args(A.handle, "hi", warm_args));
    for (k, v) in env {
        run = run.env(k, v);
    }
    let out = run.run();
    out.assert_no_panic();
    out.assert_exit(exit);
    let id = any_job_id(&out);
    rb.wait_terminal(&id);
    assert_cold_end(&rb, &id, label);
    assert!(
        !seen.appeared(),
        "{label}: a registry was published for a job that must not stay warm"
    );
    assert!(
        cooling(&rb).is_empty() || cooling(&rb) == ["ended"],
        "{label}: {:?}",
        cooling(&rb)
    );
    rb.assert_quiet();
}

#[test]
fn ac_v2_403_a_failed_job_never_stays_warm() {
    never_warm_case("failed", "error", &[], 7, &["--warm", "30s"]);
}

#[test]
fn ac_v2_403_an_aborted_job_never_stays_warm() {
    never_warm_case("aborted", "aborted", &[], 7, &["--warm", "30s"]);
}

#[test]
fn ac_v2_403_a_forked_job_never_stays_warm() {
    never_warm_case("forked", "fork", &[], 11, &["--warm", "30s"]);
}

#[test]
fn ac_v2_403_a_completed_but_unsettled_job_never_stays_warm() {
    never_warm_case(
        "unsettled",
        "never_settles",
        &[("GLEN_JOB_TIMEOUT", "2s")],
        0,
        &["--warm", "30s"],
    );
}

#[test]
fn ac_v2_403_a_timed_out_or_cancelled_job_never_stays_warm() {
    for label in ["timeout", "cancelled"] {
        let rb = RpcBox::new();
        let mut w = Wm::new(&rb);
        let _held = w.gate("FAKE_OMP_RUN_RELEASE", "run-gate");
        let seen = Appeared::watch(reg_file(&rb, A.id));
        let timeout = if label == "timeout" { "1s" } else { "30s" };
        let a = [
            "send",
            A.handle,
            "hi",
            "--warm",
            "30s",
            "--no-wait",
            "--json",
        ];
        let out = w.run(&a).env("GLEN_JOB_TIMEOUT", timeout).run();
        let id = job_id(&data_of(&out, 10));
        rb.wait_status(&id, "accepted", scaled(LONG), |s| s["status"] == "accepted");
        if label == "cancelled" {
            let worker = rb.status(&id)["worker_pid"].as_i64().expect("worker pid");
            send_signal(worker, Signal::TERM);
        }
        let st = rb.wait_terminal(&id);
        assert!(
            ["failed", "aborted"].contains(&st["status"].as_str().unwrap_or("")),
            "{label}: {st}"
        );
        assert_cold_end(&rb, &id, label);
        assert!(!seen.appeared(), "{label}: a registry was published");
        rb.assert_quiet();
    }
}

// ---------------------------------------------------------------------------------------------
// AC-v2-404: a job whose key differs is not adopted; the warm worker cools, the job runs cold.
// ---------------------------------------------------------------------------------------------

/// Job 1 is warm; job 2 differs in one key field; job 2 runs cold with its own environment.
/// `setup` prepares the box before either send.
fn mismatch_case(
    label: &str,
    setup: fn(&RpcBox),
    job1_env: &[(&str, &str)],
    job2_args: &[&str],
    job2_env: &[(&str, &str)],
) {
    let rb = RpcBox::new();
    setup(&rb);
    let w = Wm::new(&rb);
    let mut run = w.run(&Wm::send_args(A.handle, "one", &["--warm", "30s"]));
    for (k, v) in job1_env {
        run = run.env(k, v);
    }
    let d1 = data_of(&run.run(), 0);
    let c = w.cycle_of(&job_id(&d1));

    let mut run = w.run(&Wm::send_args(A.handle, "two", job2_args));
    for (k, v) in job2_env {
        run = run.env(k, v);
    }
    let from = Instant::now();
    let out = run.run();
    let d2 = data_of(&out, 0);
    assert_eq!(
        d2["assistant_text"], "pong: two",
        "{label}: the job ends with its normal result: {d2}"
    );
    assert_eq!(d2["job"]["omp_reused"], false, "{label}: not adopted: {d2}");
    assert_ne!(rb.status(&job_id(&d2))["omp_reused"], true, "{label}");

    let recs = rb.recs();
    let all = starts(&recs);
    assert_eq!(all.len(), 2, "{label}: a second omp start\n{}", rb.diag());
    assert_eq!(frames_in(&recs, "in", "prompt").len(), 2, "{label}");
    assert_ne!(all[1]["pid"], all[0]["pid"], "{label}: the job's own omp");
    assert_cooled(&rb, &c, "mismatch", from, FOUR_SECONDS);
    rb.assert_quiet();
    match label {
        "PI_CONFIG_FILES" => {
            assert_eq!(
                all[0]["env"]["PI_CONFIG_FILES"], job1_env[0].1,
                "{}",
                all[0]
            );
            assert_eq!(
                all[1]["env"]["PI_CONFIG_FILES"], job2_env[0].1,
                "{}",
                all[1]
            );
        }
        "thinking" => assert!(
            strings(&all[1]["argv"]).iter().any(|a| a == "high"),
            "the second omp got the job's own --thinking: {}",
            all[1]
        ),
        "profile" => assert!(
            strings(&all[1]["argv"]).iter().any(|a| a == "other"),
            "the second omp got the job's own --profile: {}",
            all[1]
        ),
        _ => {}
    }
}

#[test]
fn ac_v2_404_a_different_thinking_level_cools_and_runs_cold() {
    mismatch_case("thinking", |_| {}, &[], &["--thinking", "high"], &[]);
}

#[test]
fn ac_v2_404_a_different_profile_cools_and_runs_cold() {
    // The profile has its own agent dir; give it a copy of the fixture sessions so the same
    // session resolves under `--profile other`.
    mismatch_case(
        "profile",
        |rb| {
            copy_tree(
                &fixture_agent_dir(),
                &rb.bx.home.join(".omp/profiles/other/agent"),
            )
        },
        &[],
        &["--profile", "other"],
        &[],
    );
}

#[test]
fn ac_v2_404_a_different_environment_cools_and_runs_cold_with_its_own_environment() {
    mismatch_case(
        "PI_CONFIG_FILES",
        |_| {},
        &[("PI_CONFIG_FILES", "/first/config.yml")],
        &[],
        &[("PI_CONFIG_FILES", "/second/config.yml")],
    );
}

#[test]
fn ac_v2_404_a_different_omp_binary_cools_and_runs_cold() {
    let rb = RpcBox::new();
    let copy = rb.bx.root.join("omp-copy");
    fs::copy(&rb.fake, &copy).unwrap();
    let w = Wm::new(&rb);
    let c = w.first("30s");
    let from = Instant::now();
    let out = w
        .run(&Wm::send_args(A.handle, "two", &[]))
        .env("GLEN_OMP_BIN", &copy)
        .run();
    let d2 = data_of(&out, 0);
    assert_eq!(d2["assistant_text"], "pong: two", "{d2}");
    assert_eq!(
        d2["job"]["omp_reused"], false,
        "a different binary is not adopted: {d2}"
    );
    assert_eq!(
        starts(&rb.recs()).len(),
        2,
        "a second omp start\n{}",
        rb.diag()
    );
    assert_cooled(&rb, &c, "mismatch", from, FOUR_SECONDS);
    rb.assert_quiet();
}

#[test]
fn ac_v2_404_a_different_cwd_cools_and_runs_cold() {
    let rb = RpcBox::new();
    let w = Wm::new(&rb);
    let c = w.first("30s");
    let from = Instant::now();
    let out = w
        .run_in(&rb.bx.beta, &Wm::send_args(A.handle, "two", &[]))
        .run();
    let d2 = data_of(&out, 0);
    assert_eq!(d2["assistant_text"], "pong: two", "{d2}");
    assert_eq!(
        d2["job"]["omp_reused"], false,
        "a different cwd is not adopted: {d2}"
    );
    let all = starts(&rb.recs());
    assert_eq!(all.len(), 2, "a second omp start\n{}", rb.diag());
    assert!(
        same_file(Path::new(all[1]["cwd"].as_str().unwrap_or("")), &rb.bx.beta),
        "the second omp runs in the second job's directory: {}",
        all[1]
    );
    assert_cooled(&rb, &c, "mismatch", from, FOUR_SECONDS);
    rb.assert_quiet();
}

#[test]
fn ac_v2_404_a_queued_job_without_an_env_digest_is_never_adopted() {
    let rb = RpcBox::new();
    let w = Wm::new(&rb);
    let c = w.first("30s");

    // A job without an `env_digest` (as a glen from before v0.2 W4 recorded): `rpc`, `queued`.
    // H2: a warm worker adopts only a job its own worker offers (S-03), so the job's worker is
    // started here, with the job on stdin as `send` hands it over.
    let created = SystemTime::now();
    let id = job_uuid(ms_since_epoch(created) + 1, 0x00d1_9e57);
    let dir = rb.job_dir(&id);
    fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    let mut job = rb.job_json(&c.job);
    job["id"] = json!(id);
    job["handle"] = json!(job_handle(&id));
    job["created_at"] = json!(rfc3339(created));
    job["deadline_at"] = json!(rfc3339(created + Duration::from_secs(30)));
    job["text"] = json!("legacy");
    job.as_object_mut().unwrap().remove("env_digest");
    let from = Instant::now();
    write_private(
        &dir.join("job.json"),
        &serde_json::to_vec_pretty(&job).unwrap(),
    );
    let mut st = rb.status(&c.job);
    let fields = st.as_object_mut().unwrap();
    for k in [
        "worker_pid",
        "worker_start",
        "omp_pid",
        "rpc_protocol",
        "request_id",
        "actual_session_id",
        "actual_session_file",
        "delivered_as",
        "waited_ms",
        "omp_reused",
    ] {
        fields.remove(k);
    }
    fields.insert("status".into(), json!("queued"));
    fields.insert("updated_at".into(), json!(rfc3339(SystemTime::now())));
    write_private(
        &dir.join("status.json"),
        &serde_json::to_vec_pretty(&st).unwrap(),
    );
    let mut worker = rb.command(&["__worker", id.as_str()]);
    worker.stdin(std::process::Stdio::piped());
    let mut worker = crate::flake::Captured::spawn(&mut worker).expect("spawn glen __worker");
    {
        use std::io::Write;
        let mut stdin = worker.stdin.take().unwrap();
        stdin
            .write_all(&serde_json::to_vec_pretty(&job).unwrap())
            .unwrap();
    }
    assert_cooled(&rb, &c, "mismatch", from, FOUR_SECONDS);
    let st = rb.wait_status(&id, "completed", scaled(LONG), |s| {
        s["status"] == "completed"
    });
    assert_ne!(
        st["omp_reused"], true,
        "the warm worker did not claim it: {st}"
    );
    assert_eq!(
        starts(&rb.recs()).len(),
        2,
        "it ran cold, on an omp of its own"
    );
    assert!(worker.wait().unwrap().success());
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-v2-405: a stale heartbeat is reported; `release --force` kills after an identity check.
// ---------------------------------------------------------------------------------------------

/// How long a SIGSTOPped worker must stay stopped before its heartbeat is stale: debug builds
/// read `GLEN_WARM_STALE_MS`; release builds use the 5 s bound.
fn stale_wait() -> Duration {
    if cfg!(debug_assertions) {
        Duration::from_millis(1500)
    } else {
        Duration::from_millis(6500)
    }
}

/// The env a reader (doctor, ls) needs to shorten the stale bound (debug builds only).
fn stale_env() -> Vec<(&'static str, &'static str)> {
    if cfg!(debug_assertions) {
        vec![("GLEN_WARM_STALE_MS", "500")]
    } else {
        vec![]
    }
}

fn reader(rb: &RpcBox, args: &[&str]) -> Out {
    let mut run = rb.glen(args.iter().copied());
    for (k, v) in stale_env() {
        run = run.env(k, v);
    }
    run.run()
}

/// A warm worker, stopped with SIGSTOP and stale by `stale_wait`.
fn stopped_worker(rb: &RpcBox) -> (Cycle, Stopped) {
    let w = Wm::new(rb);
    let c = w.first("120s");
    let guard = Stopped(c.pid);
    send_signal(c.pid, Signal::STOP);
    std::thread::sleep(stale_wait());
    (c, guard)
}

#[test]
fn ac_v2_405_a_stopped_worker_is_stale_in_doctor_and_ls() {
    let rb = RpcBox::new();
    let (c, _guard) = stopped_worker(&rb);

    let doc = data_of(&reader(&rb, &["doctor", "--json"]), 0);
    let row = check(&doc, "warm_workers");
    assert_eq!(row["status"], "warn", "a stale heartbeat warns: {row}");
    let value = row["value"].as_str().unwrap_or("");
    assert!(
        value.contains(", 1 stale, 0 dead"),
        "value `<w> warm, <s> stale, <d> dead`: {row}"
    );
    let detail = row["detail"].as_str().unwrap_or("");
    assert!(
        detail.contains(A.handle),
        "the detail names the handle: {row}"
    );
    assert!(
        detail.contains("--force"),
        "the detail names the remedy: {row}"
    );

    let ls = data_of(&reader(&rb, &["ls", "--all", "--json"]), 0);
    let r = arr(&ls["sessions"])
        .iter()
        .find(|r| r["id"] == A.id)
        .unwrap_or_else(|| panic!("session A is not in ls: {ls}"));
    let detail = r["state_detail"].as_str().unwrap_or("");
    assert!(
        detail.starts_with("warm worker, no heartbeat for ") && detail.ends_with('s'),
        "state_detail for a stale worker: {r}"
    );
    let human = reader(&rb, &["ls", "--all", "--human"]);
    human.assert_exit(0);
    assert!(
        human.out().contains("(warm"),
        "the human state cell shows warmth\n{}",
        human.ctx()
    );

    // Hold and release: kill what the test stopped, no matter what.
    drop(_guard);
    send_signal(c.pid, Signal::KILL);
    signal_group(c.omp_pgid, Signal::KILL);
    rb.assert_quiet();
}

#[test]
fn ac_v2_405_release_without_force_times_out_exit_7_and_force_kills_after_the_identity_check() {
    let rb = RpcBox::new();
    let (c, _guard) = stopped_worker(&rb);

    let out = rb
        .glen(["release", A.handle, "--timeout", "1s", "--json"])
        .run();
    let err = failed(&out, "control_failed");
    assert_eq!(err["details"]["reason"], "release_timeout", "{err}");
    assert!(
        err["details"]["workers"]
            .as_array()
            .is_some_and(|w| !w.is_empty()),
        "details.workers: {err}"
    );
    assert!(
        err["hint"].as_str().unwrap_or("").contains("--force"),
        "the hint names --force: {err}"
    );
    assert!(pid_alive(c.pid), "without --force nothing is killed");
    assert!(reg_file(&rb, A.id).exists(), "the registry is kept");

    let out = reader(&rb, &["release", A.handle, "--force", "--json"]);
    let d = data_of(&out, 0);
    assert_eq!(d["total"], 1, "{d}");
    let wk = &d["workers"][0];
    assert_eq!(wk["outcome"], "forced", "{d}");
    assert_eq!(wk["state"], "stale", "{d}");
    assert_eq!(wk["pid"], c.pid, "{d}");
    poll_until("the killed worker to be gone", scaled(LONG), || {
        (!pid_alive(c.pid)).then_some(())
    });
    poll_until("the omp group to be gone", scaled(FOUR_SECONDS), || {
        omp_gone(c.omp_pid, c.omp_pgid).then_some(())
    });
    assert!(!reg_file(&rb, A.id).exists(), "the registry is removed");
    assert!(
        !release_file(&rb, A.id).exists(),
        "so is the release request"
    );
    poll_until("the lock to be free", scaled(FOUR_SECONDS), || {
        rb.glen_lock_free(A.id).then_some(())
    });
    rb.assert_quiet();
}

/// Rewrite the registry's `start` token to another value (the worker keeps running).
fn tamper_start(rb: &RpcBox) {
    let mut reg = registry(rb, A.id).expect("registry");
    let start = reg["start"].as_str().expect("start token").to_owned();
    reg["start"] = json!(format!("{start}9"));
    let tmp = warm_dir(rb).join(".tamper.tmp");
    write_private(&tmp, &serde_json::to_vec(&reg).unwrap());
    fs::rename(&tmp, reg_file(rb, A.id)).unwrap();
}

#[test]
fn ac_v2_405_a_registry_whose_identity_no_longer_matches_is_cleaned_and_never_signalled() {
    for force in [false, true] {
        let rb = RpcBox::new();
        let w = Wm::new(&rb);
        let c = w.first("120s");
        tamper_start(&rb);
        let mut args = vec!["release", A.handle, "--json"];
        if force {
            args.push("--force");
        }
        let out = rb.glen(&args).run();
        let d = data_of(&out, 0);
        let wk = &d["workers"][0];
        assert_eq!(wk["outcome"], "cleaned", "force={force}: {d}");
        assert_eq!(wk["state"], "dead", "force={force}: {d}");
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            pid_alive(c.pid),
            "force={force}: the process behind the registry was never signalled"
        );
        assert!(pid_alive(c.omp_pid), "force={force}: nor its omp");
        assert!(
            !reg_file(&rb, A.id).exists(),
            "force={force}: docs/json.md says an identity-changed registry is removed (`cleaned`)"
        );
        send_signal(c.pid, Signal::TERM);
        poll_until("the untouched worker to end itself", scaled(LONG), || {
            (!pid_alive(c.pid)).then_some(())
        });
        rb.assert_quiet();
    }
}

#[test]
fn ac_v2_405_a_dead_worker_is_counted_by_doctor_and_cleaned_by_release() {
    let rb = RpcBox::new();
    let w = Wm::new(&rb);
    let c = w.first("120s");
    send_signal(c.pid, Signal::KILL);
    signal_group(c.omp_pgid, Signal::KILL);
    poll_until("the worker to be gone", scaled(LONG), || {
        (!pid_alive(c.pid)).then_some(())
    });
    assert!(
        reg_file(&rb, A.id).exists(),
        "a killed worker could not remove its registry"
    );

    let doc = data_of(&rb.glen(["doctor", "--json"]).run(), 0);
    let row = check(&doc, "warm_workers");
    assert!(
        row["value"].as_str().unwrap_or("").contains(", 1 dead"),
        "{row}"
    );
    assert_eq!(
        row["status"], "info",
        "a dead entry alone is not a warning: {row}"
    );

    let ls = data_of(&rb.glen(["ls", "--all", "--json"]).run(), 0);
    let r = arr(&ls["sessions"])
        .iter()
        .find(|r| r["id"] == A.id)
        .expect("row");
    assert!(
        !r["state_detail"]
            .as_str()
            .unwrap_or("")
            .starts_with("warm worker"),
        "a dead entry is not shown as warmth: {r}"
    );

    let out = rb.glen(["release", "--all", "--json"]).run();
    let d = data_of(&out, 0);
    assert_eq!(d["total"], 1, "{d}");
    assert_eq!(d["workers"][0]["outcome"], "cleaned", "{d}");
    assert_eq!(d["workers"][0]["state"], "dead", "{d}");
    assert!(!reg_file(&rb, A.id).exists());
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-v2-406: controls, abort, ls and doctor.
// ---------------------------------------------------------------------------------------------

fn data_keys(v: &Value) -> BTreeSet<String> {
    v.as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// A control on an idle warm session cools the worker, then runs through today's route: the
/// same document shape and the same omp command line as on a session that was never warm.
fn control_case(args: &[&str]) {
    let rb = RpcBox::new();
    let w = Wm::new(&rb);
    let mut with_json = args.to_vec();
    with_json.push("--json");

    // Today's route, on a session that is not warm.
    let base = rb.glen(&with_json).run();
    let base_doc = ok_doc(&base, 0);
    let base_start = starts(&rb.recs());
    assert_eq!(
        base_start.len(),
        1,
        "{args:?}: the control's own omp\n{}",
        rb.diag()
    );
    rb.assert_quiet();

    let c = w.first("120s");
    let before = rb.starts().len();
    let from = Instant::now();
    let out = rb.glen(&with_json).run();
    let doc = ok_doc(&out, 0);
    assert_eq!(
        data_keys(&doc["data"]),
        data_keys(&base_doc["data"]),
        "{args:?}: the document keeps today's shape"
    );
    assert_eq!(doc["data"]["route"], base_doc["data"]["route"], "{args:?}");

    let all = starts(&rb.recs());
    assert_eq!(
        all.len(),
        before + 1,
        "{args:?}: the control ran on its own omp\n{}",
        rb.diag()
    );
    assert_eq!(
        strings(&all[before]["argv"]),
        strings(&base_start[0]["argv"]),
        "{args:?}: the same omp command line as on a session that was never warm"
    );
    assert_cooled(&rb, &c, "release", from, FOUR_SECONDS);
    assert!(
        rb.glen_lock_free(A.id),
        "{args:?}: the control released the lock"
    );
    assert!(
        warm_entries(&rb).is_empty(),
        "{args:?}: {:?}",
        warm_entries(&rb)
    );
    poll_until("the cooled worker to exit", scaled(LONG), || {
        (!pid_alive(c.pid)).then_some(())
    });
    rb.assert_quiet();
}

#[test]
fn ac_v2_406_model_on_an_idle_warm_session_cools_it_then_runs_as_today() {
    control_case(&["model", A.handle, "fake/fake-2"]);
}

#[test]
fn ac_v2_406_thinking_on_an_idle_warm_session_cools_it_then_runs_as_today() {
    control_case(&["thinking", A.handle, "low"]);
}

#[test]
fn ac_v2_406_compact_on_an_idle_warm_session_cools_it_then_runs_as_today() {
    control_case(&["compact", A.handle]);
}

#[test]
fn ac_v2_406_models_for_a_session_on_an_idle_warm_session_cools_it_then_runs_as_today() {
    control_case(&["models", A.handle]);
}

#[test]
fn ac_v2_406_abort_on_an_idle_warm_session_says_idle_and_keeps_it_warm() {
    let rb = RpcBox::new();
    let w = Wm::new(&rb);
    let c = w.first("30s");
    poll_until(
        "the lock holder to read `warm`",
        scaled(Duration::from_secs(2)),
        || holder(&rb, A.id).filter(|h| h["kind"] == "warm"),
    );
    let jobs_before = rb.job_ids();
    let out = rb.glen(["abort", A.handle, "--json"]).run();
    let d = data_of(&out, 0);
    assert_eq!(d["was"], "idle", "nothing to abort: {d}");
    assert_eq!(d["aborted"], false, "{d}");
    assert_eq!(d["session"]["id"], A.id, "{d}");

    assert!(reg_file(&rb, A.id).exists(), "the registry is still there");
    assert_eq!(registry(&rb, A.id).unwrap()["state"], "idle");
    assert!(
        pid_alive(c.pid) && pid_alive(c.omp_pid),
        "worker and omp are alive"
    );
    assert!(!rb.glen_lock_free(A.id), "the lock is still held");
    assert_eq!(holder(&rb, A.id).unwrap()["kind"], "warm");
    assert!(
        cooling(&rb).is_empty(),
        "abort never cools: {:?}",
        cooling(&rb)
    );
    assert_eq!(rb.job_ids(), jobs_before, "abort created no job");
    assert_eq!(rb.starts().len(), 1, "and started no omp");

    // Still warm and functional: the next send is stolen.
    let d2 = data_of(&w.send(A.handle, "two", &["--warm", "30s"]), 0);
    assert_eq!(d2["job"]["omp_reused"], true, "{d2}");
    end_warmth(&rb);
}

#[test]
fn ac_v2_406_ls_shows_idle_and_busy_warmth_and_doctor_counts_it() {
    let rb = RpcBox::new();
    let mut w = Wm::new(&rb);
    let run = w.gate("FAKE_OMP_RUN_RELEASE", "run-gate");
    run.open();
    w.first("60s");

    let row = |rb: &RpcBox| -> Value {
        let d = data_of(&rb.glen(["ls", "--all", "--json"]).run(), 0);
        arr(&d["sessions"])
            .iter()
            .find(|r| r["id"] == A.id)
            .unwrap_or_else(|| panic!("session A is not in ls: {d}"))
            .clone()
    };
    let r = row(&rb);
    let detail = r["state_detail"].as_str().unwrap_or("");
    let left: u64 = detail
        .strip_prefix("warm worker, idle, ")
        .and_then(|t| t.strip_suffix("s left"))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("state_detail `warm worker, idle, <N>s left`: {r}"));
    assert!((40..=60).contains(&left), "about a minute left: {r}");
    assert!(
        ["live", "busy"].contains(&r["state"].as_str().unwrap_or("")),
        "{r}"
    );
    let human = rb.glen(["ls", "--all", "--human"]).run();
    human.assert_exit(0);
    assert!(
        human.out().contains(" (warm)"),
        "the human state cell gains ` (warm)`\n{}",
        human.ctx()
    );

    let doc = data_of(&rb.glen(["doctor", "--json"]).run(), 0);
    let names: Vec<&str> = arr(&doc["checks"])
        .iter()
        .map(|c| c["name"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        names, DOCTOR_CHECKS,
        "15 checks, warm_workers after live_endpoints"
    );
    let row_doc = check(&doc, "warm_workers");
    assert_eq!(row_doc["status"], "info", "{row_doc}");
    assert_eq!(row_doc["value"], "1 warm, 0 stale, 0 dead", "{row_doc}");

    // While an adopted job runs the worker reads busy.
    run.close();
    let id2 = w.send_nowait(A.handle, "two", &["--warm", "60s"]);
    rb.wait_status(&id2, "claimed and running", scaled(LONG), |s| {
        s["omp_reused"] == true && s["status"] == "accepted"
    });
    poll_until("a busy registry", scaled(LONG), || {
        registry(&rb, A.id).filter(|r| r["state"] == "busy")
    });
    let r = row(&rb);
    assert_eq!(r["state_detail"], "warm worker, busy", "{r}");
    let doc = data_of(&rb.glen(["doctor", "--json"]).run(), 0);
    assert_eq!(
        check(&doc, "warm_workers")["status"],
        "info",
        "a busy worker is not stale"
    );
    run.open();
    w.wait_done(&id2);
    end_warmth(&rb);
}

#[test]
fn ac_v2_406_doctor_has_the_warm_workers_row_with_nothing_warm() {
    let rb = RpcBox::new();
    let doc = data_of(&rb.glen(["doctor", "--json"]).run(), 0);
    let names: Vec<&str> = arr(&doc["checks"])
        .iter()
        .map(|c| c["name"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(names, DOCTOR_CHECKS);
    let pos = |n: &str| names.iter().position(|c| *c == n).unwrap();
    assert_eq!(pos("warm_workers"), pos("live_endpoints") + 1);
    let row = check(&doc, "warm_workers");
    assert_eq!(row["status"], "info", "{row}");
    assert_eq!(row["value"], "0 warm, 0 stale, 0 dead", "{row}");
    assert!(
        !rb.bx.glen_home.exists(),
        "doctor creates no GLEN_HOME for it"
    );
}

#[test]
fn ac_v2_406_release_on_a_busy_worker_returns_at_once_and_it_cools_after_the_job() {
    let rb = RpcBox::new();
    let mut w = Wm::new(&rb);
    let run = w.gate("FAKE_OMP_RUN_RELEASE", "run-gate");
    run.open();
    let c = w.first("30s");
    run.close();
    let id2 = w.send_nowait(A.handle, "two", &["--warm", "30s"]);
    rb.wait_status(&id2, "claimed and running", scaled(LONG), |s| {
        s["omp_reused"] == true && s["status"] == "accepted"
    });
    poll_until("a busy registry", scaled(LONG), || {
        registry(&rb, A.id).filter(|r| r["state"] == "busy")
    });

    let started = Instant::now();
    let out = rb.glen(["release", A.handle, "--json"]).run();
    let d = data_of(&out, 0);
    assert!(
        started.elapsed() < scaled(Duration::from_secs(3)),
        "release never waits on a model turn"
    );
    assert_eq!(d["workers"][0]["outcome"], "cooling", "{d}");
    assert_eq!(d["workers"][0]["state"], "busy", "{d}");
    assert!(pid_alive(c.pid), "the worker finishes its job first");
    assert_eq!(
        rb.status(&id2)["status"],
        "accepted",
        "the job is untouched"
    );

    run.open();
    let settled = Instant::now();
    let done = w.wait_done(&id2);
    assert_eq!(done["assistant_text"], "pong: two", "{done}");
    assert_cooled(&rb, &c, "release", settled, FOUR_SECONDS);
    rb.assert_quiet();
}

#[test]
fn ac_v2_406_release_validates_its_arguments_and_matches_only_warm_workers() {
    let rb = RpcBox::new();
    for args in [
        vec!["release", "--json"],
        vec!["release", A.handle, "--all", "--json"],
        vec!["release", "--all", "--timeout", "0", "--json"],
        vec!["release", "--all", "--timeout", "25h", "--json"],
        vec!["release", "--all", "--timeout", "abc", "--json"],
    ] {
        let out = rb.glen(&args).run();
        failed(&out, "usage");
    }
    let out = rb.glen(["release", A.handle, "--json"]).run();
    let err = failed(&out, "not_found");
    assert_eq!(err["details"]["reason"], "not_warm", "{err}");
    assert_eq!(err["details"]["target"], A.handle, "{err}");
    let out = rb.glen(["release", "--all", "--json"]).run();
    let d = data_of(&out, 0);
    assert_eq!(d["total"], 0, "{d}");
    assert_eq!(d["workers"], json!([]), "{d}");
    let human = rb.glen(["release", "--all", "--human"]).run();
    human.assert_exit(0);
    assert!(human.out().contains("no warm workers"), "{}", human.ctx());
    assert!(!rb.bx.glen_home.exists(), "release creates no GLEN_HOME");
    assert!(rb.starts().is_empty());
}

#[test]
fn ac_v2_406_a_release_request_meant_for_another_worker_is_ignored() {
    let rb = RpcBox::new();
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(warm_dir(&rb))
        .unwrap();
    write_private(&release_file(&rb, A.id), b"999999:another-worker");
    let w = Wm::new(&rb);
    let c = w.first("30s");
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        reg_file(&rb, A.id).exists(),
        "a request for another worker did not cool this one"
    );
    assert!(pid_alive(c.pid) && pid_alive(c.omp_pid));
    assert!(!rb.glen_lock_free(A.id));
    assert!(cooling(&rb).is_empty(), "{:?}", cooling(&rb));
    end_warmth(&rb);
}

/// A copy of fixture A under another id, in the same sessions bucket.
fn clone_session(rb: &RpcBox, id: &str) -> PathBuf {
    let text = fs::read_to_string(rb.bx.path(&A)).expect("fixture A");
    let body: Vec<Value> = text
        .lines()
        .skip(2) // the title slot and the session header
        .map(|l| serde_json::from_str(l).expect("fixture entry"))
        .collect();
    rb.bx.add(
        ALPHA,
        "2026-10-01T09:00:00Z",
        id,
        Some(&s(&rb.bx.alpha)),
        &body,
    )
}

#[test]
fn ac_v2_406_release_with_a_tail_that_matches_two_warm_workers_is_ambiguous() {
    let rb = RpcBox::new();
    let (x1, x2) = (
        "01a20001-0000-7000-8000-00000aaa1234",
        "01a20002-0000-7000-8000-00000bbb1234",
    );
    clone_session(&rb, x1);
    clone_session(&rb, x2);
    let w = Wm::new(&rb);
    for id in [x1, x2] {
        data_of(&w.send(id, "hi", &["--warm", "30s"]), 0);
        poll_until("the worker's registry", scaled(LONG), || {
            registry(&rb, id).filter(|r| r["state"] == "idle")
        });
    }
    let out = rb.glen(["release", "1234", "--json"]).run();
    let err = failed(&out, "ambiguous");
    assert!(
        err["message"]
            .as_str()
            .is_some_and(|m| !m.trim().is_empty()),
        "{err}"
    );
    for id in [x1, x2] {
        assert!(
            reg_file(&rb, id).exists(),
            "an ambiguous release ends nothing: {id}"
        );
    }
    let out = rb.glen(["release", "--all", "--json"]).run();
    let d = data_of(&out, 0);
    assert_eq!(d["total"], 2, "{d}");
    assert!(
        arr(&d["workers"])
            .iter()
            .all(|w| w["outcome"] == "released"),
        "{d}"
    );
    rb.assert_quiet();
}

#[test]
fn ac_v2_405_an_unreadable_registry_file_makes_doctor_warn_but_never_fail() {
    let rb = RpcBox::new();
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(warm_dir(&rb))
        .unwrap();
    write_private(&warm_dir(&rb).join("garbage.json"), b"{not json");
    let out = rb.glen(["doctor", "--json"]).run();
    let doc = data_of(&out, 0);
    let row = check(&doc, "warm_workers");
    assert_eq!(
        row["status"], "warn",
        "an unreadable file is a warning: {row}"
    );
}

// ---------------------------------------------------------------------------------------------
// AC-v2-407: replies and the idle clock.
// ---------------------------------------------------------------------------------------------

/// Every file under `dir`, recursively.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn ac_v2_407_an_idle_glen_reply_is_a_tool_error_and_is_attributed_to_no_job() {
    let rb = RpcBox::new();
    let mut w = Wm::new(&rb);
    let idle_reply = w.gate("FAKE_OMP_IDLE_REPLY_RELEASE", "idle-reply");
    let c = w.first("30s");
    let job1_replies = fs::read(rb.job_dir(&c.job).join("replies.ndjson")).unwrap_or_default();

    idle_reply.open();
    let recs = poll_until(
        "omp's idle glen_reply to be answered",
        scaled(Duration::from_secs(3)),
        || {
            let r = rb.recs();
            r.iter()
                .any(|x| is_frame(x, "in", "host_tool_result") && x["frame"]["id"] == "htc_idle")
                .then_some(r)
        },
    );
    let result = recs
        .iter()
        .find(|x| is_frame(x, "in", "host_tool_result") && x["frame"]["id"] == "htc_idle")
        .map(|x| x["frame"].clone())
        .unwrap();
    assert!(
        is_tool_error(&result),
        "the idle call returns a tool error: {result}"
    );

    // Job 2 afterwards: its replies are its own and the idle text is nowhere.
    let d2 = data_of(&w.send(A.handle, "two", &["--warm", "30s"]), 0);
    let id2 = job_id(&d2);
    assert_eq!(d2["job"]["omp_reused"], true, "{d2}");
    assert_eq!(arr(&d2["replies"]).len(), 1, "only job 2's own reply: {d2}");
    assert_eq!(d2["replies"][0]["text"], "reply: two", "{d2}");
    assert_eq!(d2["replies"][0]["job_id"], id2.as_str(), "{d2}");
    for file in files_under(&rb.bx.glen_home) {
        let bytes = fs::read(&file).unwrap_or_default();
        assert!(
            !bytes
                .windows(b"idle reply".len())
                .any(|w| w == b"idle reply"),
            "{} gained the idle reply text",
            file.display()
        );
    }
    assert_eq!(
        fs::read(rb.job_dir(&c.job).join("replies.ndjson")).unwrap_or_default(),
        job1_replies,
        "job 1's replies are unchanged"
    );
    end_warmth(&rb);
}

#[test]
fn ac_v2_407_an_adopted_jobs_replies_carry_its_own_id_and_the_previous_job_is_untouched() {
    let rb = RpcBox::new();
    let mut w = Wm::new(&rb);
    w.knob("FAKE_OMP_SCENARIO", "progress");
    let c = w.first("30s");
    let id1 = c.job.clone();
    let before = fs::read(rb.job_dir(&id1).join("replies.ndjson")).expect("job 1 replies");
    assert_eq!(ndjson(&rb.job_dir(&id1).join("replies.ndjson")).len(), 2);

    let d2 = data_of(&w.send(A.handle, "two", &["--warm", "30s"]), 0);
    let id2 = job_id(&d2);
    assert_eq!(d2["job"]["omp_reused"], true, "{d2}");
    let replies = ndjson(&rb.job_dir(&id2).join("replies.ndjson"));
    assert_eq!(
        replies.len(),
        2,
        "job 2's two progress replies: {replies:?}"
    );
    for r in &replies {
        assert_eq!(
            r["job_id"],
            id2.as_str(),
            "a reply carries its own job's id: {r}"
        );
        assert_eq!(r["session_id"], A.id, "{r}");
    }
    assert_eq!(replies[0]["text"], "progress");
    assert_eq!(replies[1]["text"], "reply: two");
    let result = rb.result(&id2);
    assert_eq!(arr(&result["replies"]).len(), 2, "{result}");
    assert!(
        arr(&result["replies"])
            .iter()
            .all(|r| r["job_id"] == id2.as_str()),
        "{result}"
    );
    assert_eq!(
        fs::read(rb.job_dir(&id1).join("replies.ndjson")).unwrap(),
        before,
        "the previous job's replies.ndjson did not change"
    );
    assert!(
        arr(&rb.result(&id1)["replies"])
            .iter()
            .all(|r| r["job_id"] == id1.as_str()),
        "job 1's result holds only job 1's replies"
    );
    end_warmth(&rb);
}

#[test]
fn ac_v2_407_the_idle_clock_starts_only_after_settlement_and_rearms_per_adopted_job() {
    let rb = RpcBox::new();
    let mut w = Wm::new(&rb);
    let run = w.gate("FAKE_OMP_RUN_RELEASE", "run-gate");
    // Job 1 is held for longer than its whole warm window and still goes warm afterwards.
    let id1 = w.send_nowait(A.handle, "one", &["--warm", "2s"]);
    rb.wait_status(&id1, "accepted", scaled(LONG), |s| {
        s["status"] == "accepted"
    });
    std::thread::sleep(Duration::from_millis(2600));
    assert!(
        !reg_file(&rb, A.id).exists(),
        "nothing is warm while the job runs"
    );
    run.open();
    let c = w.cycle_of(&id1);
    let reg = registry(&rb, A.id).expect("registry");
    let settled = millis(&rb.result(&id1)["settled_at"]);
    assert!(
        millis(&reg["expires_at"]) >= settled + 2000,
        "AC-v2-407c: expires_at is at least warm_ms after settled_at: {reg}"
    );

    // Job 2 carries its own, longer window, held past the first one: busy, never cooled.
    run.close();
    let id2 = w.send_nowait(A.handle, "two", &["--warm", "5s"]);
    rb.wait_status(&id2, "claimed and running", scaled(LONG), |s| {
        s["omp_reused"] == true && s["status"] == "accepted"
    });
    std::thread::sleep(Duration::from_millis(2600));
    let busy = registry(&rb, A.id).expect("registry while job 2 runs");
    assert_eq!(
        busy["state"], "busy",
        "a running adopted job keeps the worker busy past warm_ms: {busy}"
    );
    assert!(pid_alive(c.pid) && pid_alive(c.omp_pid));
    assert_eq!(cooling(&rb), Vec::<String>::new());
    run.open();
    let c2 = w.cycle_of(&id2);
    assert_eq!(c2.pid, c.pid, "the same worker");
    let reg2 = registry(&rb, A.id).expect("registry");
    assert_eq!(
        reg2["warm_ms"], 5000,
        "re-armed with job 2's own warm_ms: {reg2}"
    );
    let settled2 = millis(&rb.result(&id2)["settled_at"]);
    let expires2 = millis(&reg2["expires_at"]);
    assert!(
        expires2 >= settled2 + 5000,
        "expires_at after job 2's settlement: {reg2}"
    );
    assert!(
        expires2 < settled2 + 30_000,
        "not re-armed with an older window: {reg2}"
    );
    end_warmth(&rb);
}

// ---------------------------------------------------------------------------------------------
// Static contracts: §A9, §A15, §A16, §A21, CHANGELOG, the module doc, the skill.
// ---------------------------------------------------------------------------------------------

fn design_section(n: u32) -> String {
    let doc = fs::read_to_string(repo_root().join("docs/design.md")).expect("docs/design.md");
    let head = format!("## §A{n} ");
    let mut lines = doc.lines().skip_while(|l| !l.starts_with(&head));
    assert!(lines.next().is_some(), "docs/design.md has no §A{n}");
    lines
        .take_while(|l| !l.starts_with("## "))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn ac_v2_406_a21_documents_the_lease_cost_the_liveness_decision_and_the_residual_races() {
    let a21 = design_section(21);
    let lower = a21.to_lowercase();
    assert!(
        !lower.contains("stub (v0.2 w0)"),
        "§A21 is no longer a stub"
    );
    let lease = a21
        .lines()
        .find(|l| l.to_lowercase().contains("lease cost"))
        .expect("§A21 has a lease cost paragraph");
    assert!(
        lease.to_lowercase().contains("tui"),
        "the lease cost says the TUI cannot open the session: {lease}"
    );
    assert!(
        lower.contains("queued-job liveness"),
        "§A21 records the queued-job liveness decision"
    );
    assert!(
        lower.contains("two residual races"),
        "§A21 documents two residual races"
    );
    assert!(
        lower.contains("glen_reply"),
        "the second race is about an in-flight glen_reply"
    );
    for reason in [
        "expired",
        "release",
        "endpoint",
        "signal",
        "omp_exited",
        "mismatch",
        "moved",
        "state_error",
        "ended",
    ] {
        assert!(
            a21.contains(&format!("`{reason}`")),
            "§A21 lists the cooling reason `{reason}`"
        );
    }
    assert!(a21.contains("warm: cooling"), "§A21 names the log line");
    for needle in [
        "Default (2)",
        "Default (3)",
        "run/warm",
        "env_digest",
        "omp_reused",
        "10m",
    ] {
        assert!(a21.contains(needle), "§A21 never says {needle}");
    }
}

#[test]
fn ac_v2_406_the_w4_rows_exist_in_a9_a15_a16_and_the_changelog() {
    let a9 = design_section(9);
    let row = a9
        .lines()
        .find(|l| l.starts_with('|') && l.contains("glen release"))
        .expect("§A9 has a `glen release` row");
    assert!(row.contains("v0.2 W4") && row.contains("visible"), "{row}");
    let send = a9
        .lines()
        .find(|l| l.starts_with('|') && l.contains("glen send"))
        .expect("§A9 send row");
    assert!(send.contains("--warm"), "the send row lists --warm: {send}");

    let a15 = design_section(15);
    let w4 = a15
        .lines()
        .find(|l| l.contains("v0.2 W4"))
        .expect("§A15 has a v0.2 W4 bullet");
    for needle in [
        "FAKE_OMP_MULTI",
        "FAKE_OMP_IDLE_REPLY_RELEASE",
        "FAKE_OMP_IDLE_EXIT_RELEASE",
        "GLEN_WARM_STALE_MS",
        "tests/warm.rs",
        "tests/conformance/v2_warm.rs",
    ] {
        assert!(
            w4.contains(needle),
            "the §A15 W4 bullet never says {needle}"
        );
    }

    let a16 = design_section(16);
    let w4 = a16
        .lines()
        .find(|l| l.contains("v0.2 W4"))
        .expect("§A16 has a v0.2 W4 bullet");
    assert!(
        w4.contains("--warm") && w4.contains("glen release") && w4.contains("§A21"),
        "{w4}"
    );

    let changelog = fs::read_to_string(repo_root().join("CHANGELOG.md")).expect("CHANGELOG.md");
    // W7 renamed `## Unreleased` to the `## v0.2.0 — <date>` release heading; a later
    // `## Unreleased` above it holds post-v0.2.0 changes only.
    let unreleased: String = changelog
        .lines()
        .skip_while(|l| !l.starts_with("## v0.2.0"))
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        unreleased.contains("--warm"),
        "CHANGELOG ## Unreleased announces --warm"
    );
    assert!(unreleased.contains("glen release"), "and glen release");
    assert!(unreleased.contains("omp_reused"), "and omp_reused");
    assert!(
        unreleased.contains("warm_workers"),
        "and the warm_workers check"
    );
    assert!(
        unreleased.lines().any(|l| {
            let l = l.to_lowercase();
            l.contains("tui") && l.contains("lease")
        }),
        "one bullet says a warm worker holds the session and omp's lease so the TUI cannot open it"
    );
}

#[test]
fn ac_v2_40x_the_conformance_module_doc_maps_w4_as_implemented() {
    let top = fs::read_to_string(repo_root().join("tests/conformance.rs")).expect("conformance.rs");
    let line = top
        .lines()
        .find(|l| l.contains("v0.2 W4"))
        .expect("a v0.2 W4 line in the module doc");
    assert!(line.contains("AC-v2-4"), "{line}");
    assert!(
        line.contains("v2_warm"),
        "the line names its module: {line}"
    );
    assert!(
        !line.trim_end().ends_with("planned."),
        "W4 is implemented: {line}"
    );
    assert!(line.chars().count() <= 100, "within 100 columns: {line}");
    assert!(
        top.lines().any(|l| l.trim() == "mod v2_warm;"),
        "tests/conformance.rs registers the module"
    );
}

#[test]
fn ac_v2_401_both_rendered_skills_tell_agents_about_warm_and_release() {
    let sb = Sandbox::new();
    sb.glen(["install-skill", "--project", "--json"])
        .run()
        .ok_data();
    let read =
        |rel: &str| fs::read_to_string(sb.cwd.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let claude = read(".claude/skills/glen/SKILL.md");
    let codex = read(".agents/skills/glen/SKILL.md");
    for (who, text) in [("claude", &claude), ("codex", &codex)] {
        assert!(text.contains("--warm"), "the {who} skill mentions --warm");
        assert!(
            text.contains("glen release"),
            "the {who} skill mentions glen release"
        );
        assert!(
            !text.contains("<!-- glen:if"),
            "no unrendered guard in the {who} skill"
        );
        assert!(text.lines().count() <= 160, "{who}: at most 160 lines");
        assert!(text.len() <= 12_288, "{who}: at most 12288 bytes");
    }
}
