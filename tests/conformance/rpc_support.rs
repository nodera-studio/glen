//! Shared harness for the W2 RPC conformance cases (AC-201…AC-227), derived from
//! `.claude/plans/v010-wave-2-rpc.md` and `docs/design.md` §A3–§A11, §A15 without reading the
//! implementation (`src/rpc`, `src/jobs`, `src/commands/{send,wait,jobs}.rs`).
//!
//! `RpcBox` wraps a W1 `OmpBox` (fixture copies, temp HOME, GLEN_HOME that does not exist yet,
//! PI_CODING_AGENT_DIR, TMPDIR outside HOME, TZ=UTC, empty PATH) and points `GLEN_OMP_BIN` at the
//! `fake_omp` cargo example (the plan's test peer, `tests/support/fake_omp.rs`), built on demand.
//! Every fake appends NDJSON records to a log under `<root>/logs/`. Each run also sets
//! `GLEN_JOB_TIMEOUT=30s` (so no stray worker outlives a test for long) and `GLEN_MAX_OUTPUT=0`
//! (no spill unless a test asks for it). Nothing touches a live `~/.omp` or a real omp binary.
//!
//! Detached workers and fake omp processes are waited for by `assert_quiet`; `Drop` kills any
//! leftover (after a failed assertion) before the temp dirs go away.

#![allow(dead_code)]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use rustix::process::{Pid, Signal};
use serde_json::{Map, Value, json};

use crate::reader::*;
use crate::support::*;

/// Generous bound for one asynchronous expectation (job state, log record).
pub const LONG: Duration = Duration::from_secs(20);

/// The §A5 tool description, verbatim.
pub const A5_DESCRIPTION: &str = "Send a message back to the agent that contacted you through glen (Claude Code or Codex). Use it to answer a glen request or report progress.";

/// §A10 job view keys.
pub const JOB_VIEW: [&str; 12] = [
    "id",
    "handle",
    "status",
    "route",
    "deliver_as",
    "delivered_as",
    "session_id",
    "session_handle",
    "actual_session_id",
    "created_at",
    "updated_at",
    "elapsed_ms",
];

/// §A10 completed send/wait data keys (nullable ones stay present).
pub const COMPLETED_KEYS: [&str; 10] = [
    "status",
    "job",
    "session",
    "assistant_text",
    "replies",
    "command_output",
    "stop_reason",
    "session_settled",
    "delivered_as",
    "forked",
];

pub const TERMINAL: [&str; 4] = ["completed", "aborted", "failed", "lost"];

// ---------------------------------------------------------------------------------------------
// The fake peer.
// ---------------------------------------------------------------------------------------------

/// The `fake_omp` example next to the glen binary under test, (re)built once per test process so
/// a filtered `cargo test --test conformance` never runs a stale or missing peer.
pub fn fake_omp_bin() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let exe = Path::new(BIN_PATH);
        let profile_dir = exe.parent().expect("glen binary has a parent dir");
        let target_dir = profile_dir.parent().expect("target dir");
        let profile = profile_dir
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("debug");
        let mut cmd = Command::new(env!("CARGO"));
        cmd.args([
            "build",
            "--quiet",
            "--locked",
            "--offline",
            "--example",
            "fake_omp",
        ])
        .arg("--manifest-path")
        .arg(repo_root().join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir);
        match profile {
            "debug" => {}
            "release" => {
                cmd.arg("--release");
            }
            other => {
                cmd.args(["--profile", other]);
            }
        }
        let out = cmd.output().expect("run cargo build --example fake_omp");
        assert!(
            out.status.success(),
            "cargo build --example fake_omp failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let bin = profile_dir.join("examples/fake_omp");
        assert!(bin.is_file(), "no fake peer at {}", bin.display());
        bin
    })
}

const BIN_PATH: &str = env!("CARGO_BIN_EXE_glen");

// ---------------------------------------------------------------------------------------------
// Ids, handles, timestamps.
// ---------------------------------------------------------------------------------------------

/// A uuidv7-shaped job id whose 48-bit timestamp is `ms` and whose last 12 hex digits are `tail`.
pub fn job_uuid(ms: u64, tail: u64) -> String {
    format!(
        "{:08x}-{:04x}-7000-8000-{:012x}",
        (ms >> 16) & 0xffff_ffff,
        ms & 0xffff,
        tail
    )
}

/// `j` + the last 7 hex digits of the dash-free job id (§A7, §A8).
pub fn job_handle(id: &str) -> String {
    format!("j{}", tail7(id))
}

/// The last 7 hex digits of the dash-free id (§A8 session handle, `to_handle`).
pub fn tail7(id: &str) -> String {
    let simple: String = id.chars().filter(|c| *c != '-').collect();
    simple[simple.len() - 7..].to_lowercase()
}

/// A lowercase hyphenated uuid with version nibble 7 and an RFC 4122 variant.
pub fn assert_uuid_v7(id: &str) {
    let b = id.as_bytes();
    let shape = b.len() == 36
        && [8, 13, 18, 23].iter().all(|&i| b[i] == b'-')
        && id
            .chars()
            .enumerate()
            .all(|(i, c)| [8, 13, 18, 23].contains(&i) || matches!(c, '0'..='9' | 'a'..='f'));
    assert!(shape, "job id {id:?} is not a lowercase hyphenated uuid");
    assert_eq!(b[14], b'7', "job id {id:?} is not a uuidv7");
    assert!(
        matches!(b[19], b'8' | b'9' | b'a' | b'b'),
        "job id {id:?} has no RFC 4122 variant"
    );
}

pub fn rfc3339(t: SystemTime) -> String {
    jiff::Timestamp::try_from(t).unwrap().to_string()
}

pub fn ms_since_epoch(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------------------------
// Processes.
// ---------------------------------------------------------------------------------------------

fn pid_of(raw: i64) -> Option<Pid> {
    i32::try_from(raw)
        .ok()
        .filter(|p| *p > 1)
        .and_then(Pid::from_raw)
}

/// True while a process with this pid exists and belongs to us (zombies count as existing).
pub fn pid_alive(raw: i64) -> bool {
    pid_of(raw).is_some_and(|p| rustix::process::test_kill_process(p).is_ok())
}

/// True while any process of this process group exists.
pub fn group_alive(raw: i64) -> bool {
    pid_of(raw).is_some_and(|p| rustix::process::test_kill_process_group(p).is_ok())
}

pub fn own_pgid() -> i64 {
    rustix::process::getpgrp().as_raw_nonzero().get() as i64
}

pub fn signal(raw: i64, sig: Signal) {
    let pid = pid_of(raw).unwrap_or_else(|| panic!("bad pid {raw}"));
    rustix::process::kill_process(pid, sig).unwrap_or_else(|e| panic!("kill {raw}: {e}"));
}

/// Signal a whole process group, the way a terminal delivers Ctrl-C to its foreground job.
pub fn signal_group(raw: i64, sig: Signal) {
    assert_ne!(raw, own_pgid(), "refusing to signal the test's own group");
    let pid = pid_of(raw).unwrap_or_else(|| panic!("bad pgid {raw}"));
    rustix::process::kill_process_group(pid, sig).unwrap_or_else(|e| panic!("killpg {raw}: {e}"));
}

/// A pid that existed and is gone (spawned and reaped).
pub fn dead_pid() -> u32 {
    let mut c = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

pub fn poll_until<T>(what: &str, timeout: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return v;
        }
        if Instant::now() >= deadline {
            panic!("timed out after {timeout:?} waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// What a file descriptor of another process points at (Linux `/proc`, else `lsof`).
#[cfg(target_os = "linux")]
pub fn fd_target(pid: i64, fd: u32) -> Option<String> {
    fs::read_link(format!("/proc/{pid}/fd/{fd}"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "linux"))]
pub fn fd_target(pid: i64, fd: u32) -> Option<String> {
    let lsof = if Path::new("/usr/sbin/lsof").exists() {
        "/usr/sbin/lsof"
    } else {
        "lsof"
    };
    let out = Command::new(lsof)
        .args(["-a", "-p", &pid.to_string(), "-d", &fd.to_string(), "-Fn"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix('n'))
        .map(str::to_owned)
}

/// Collect a spawned child's output with a bound, as an `Out`.
pub fn finish(mut child: Child, desc: &str, timeout: Duration) -> Out {
    let started = Instant::now();
    let mut so = child.stdout.take().expect("piped stdout");
    let mut se = child.stderr.take().expect("piped stderr");
    let to = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let te = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
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
    Out {
        desc: format!("glen {desc}"),
        status,
        stdout: to.join().unwrap(),
        stderr: te.join().unwrap(),
        elapsed: started.elapsed(),
    }
}

// ---------------------------------------------------------------------------------------------
// Fake log records (`FAKE_OMP_LOG`).
// ---------------------------------------------------------------------------------------------

/// Every complete record of a fake log, in append order (missing log → empty).
pub fn records(path: &Path) -> Vec<Value> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return vec![],
        Err(error) => panic!("cannot read fake records {}: {error}", path.display()),
    };
    let mut records = vec![];
    let mut segments = bytes.split(|byte| *byte == b'\n').peekable();
    while let Some(line) = segments.next() {
        let trailing = segments.peek().is_none();
        if trailing && line.is_empty() {
            break;
        }
        match serde_json::from_slice(line) {
            Ok(record) => records.push(record),
            Err(_) if trailing => (), // An unfinished trailing write may not yet be JSON.
            Err(error) => panic!(
                "malformed complete fake record in {}: {error}; line: {:?}",
                path.display(),
                String::from_utf8_lossy(line)
            ),
        }
    }
    records
}

/// Frames of `kind` that crossed the wire in direction `dir` (`"in"` = glen → omp).
pub fn frames_in(recs: &[Value], dir: &str, kind: &str) -> Vec<Value> {
    recs.iter()
        .filter(|r| r["event"] == dir && r["frame"]["type"] == kind)
        .map(|r| r["frame"].clone())
        .collect()
}

/// Index of the first record matching `pred`.
pub fn index_of(recs: &[Value], pred: impl Fn(&Value) -> bool) -> Option<usize> {
    recs.iter().position(pred)
}

pub fn is_frame(r: &Value, dir: &str, kind: &str) -> bool {
    r["event"] == dir && r["frame"]["type"] == kind
}

pub fn starts(recs: &[Value]) -> Vec<Value> {
    recs.iter()
        .filter(|r| r["event"] == "start")
        .cloned()
        .collect()
}

/// The `sessionId` omp reported in its `get_state` responses.
pub fn reported_session_ids(recs: &[Value]) -> Vec<String> {
    frames_in(recs, "out", "response")
        .into_iter()
        .filter(|f| f["command"] == "get_state" && f["success"] == true)
        .filter_map(|f| f["data"]["sessionId"].as_str().map(str::to_owned))
        .collect()
}

/// `host_tool_result` reports a tool error (top-level or payload `isError`).
pub fn is_tool_error(frame: &Value) -> bool {
    frame["isError"] == true || frame["result"]["isError"] == true
}

/// The text blocks of a `host_tool_result`, joined.
pub fn tool_text(frame: &Value) -> String {
    frame["result"]["content"]
        .as_array()
        .map(|c| {
            c.iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

pub fn strings(v: &Value) -> Vec<String> {
    arr(v)
        .iter()
        .map(|s| s.as_str().expect("string").to_owned())
        .collect()
}

/// The omp command line (AC-202/209): `--mode rpc`, `--no-ui`, then optional `--profile <p>` and
/// `--resume <absolute file>`, and nothing else (order-independent).
pub fn assert_omp_argv(argv: &Value, resume: Option<&Path>, profile: Option<&str>) {
    let a = strings(argv);
    let value_of = |flag: &str| -> Option<&String> {
        a.iter().position(|x| x == flag).and_then(|i| a.get(i + 1))
    };
    assert_eq!(
        value_of("--mode").map(String::as_str),
        Some("rpc"),
        "argv {a:?}"
    );
    assert_eq!(
        a.iter().filter(|x| *x == "--no-ui").count(),
        1,
        "argv {a:?}"
    );
    let mut len = 3;
    match resume {
        Some(p) => {
            let r = value_of("--resume").unwrap_or_else(|| panic!("no --resume in {a:?}"));
            assert!(Path::new(r).is_absolute(), "--resume is absolute: {a:?}");
            assert!(
                same_file(Path::new(r), p),
                "--resume {r} is not {}",
                p.display()
            );
            len += 2;
        }
        None => assert!(!a.iter().any(|x| x == "--resume"), "argv {a:?}"),
    }
    match profile {
        Some(p) => {
            assert_eq!(
                value_of("--profile").map(String::as_str),
                Some(p),
                "argv {a:?}"
            );
            len += 2;
        }
        None => assert!(!a.iter().any(|x| x == "--profile"), "argv {a:?}"),
    }
    assert_eq!(a.len(), len, "unexpected omp arguments: {a:?}");
}

// ---------------------------------------------------------------------------------------------
// Documents.
// ---------------------------------------------------------------------------------------------

/// An `ok:true` envelope with exit `exit` (0, 10, 11 or 130); JSON mode means one document, an
/// empty stderr and no ANSI (AC-227). Returns the envelope.
pub fn ok_doc(out: &Out, exit: i32) -> Value {
    out.assert_no_panic();
    let env = out.json();
    assert_eq!(out.code(), exit, "unexpected exit code\n{}", out.ctx());
    assert_eq!(env["v"], 1, "envelope v\n{}", out.ctx());
    assert_eq!(env["ok"], true, "envelope ok\n{}", out.ctx());
    for k in env.as_object().unwrap().keys() {
        assert!(
            ["v", "ok", "data", "warnings", "spilled"].contains(&k.as_str()),
            "unexpected top-level key {k:?}\n{}",
            out.ctx()
        );
    }
    out.assert_stderr_empty().assert_no_ansi();
    env
}

/// The `data` of an `ok:true` envelope with exit `exit`.
pub fn data_of(out: &Out, exit: i32) -> Value {
    let env = ok_doc(out, exit);
    assert!(env["data"].is_object(), "no data object\n{}", out.ctx());
    env["data"].clone()
}

/// The warning codes of an envelope.
pub fn warning_codes(env: &Value) -> Vec<String> {
    env["warnings"]
        .as_array()
        .map(|w| {
            w.iter()
                .filter_map(|x| x["code"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// An error envelope with `code` and its §A11 exit; JSON mode keeps stderr empty and ANSI-free.
pub fn failed(out: &Out, code: &str) -> Value {
    out.assert_no_panic();
    let err = out.error_envelope(code);
    out.assert_stderr_empty().assert_no_ansi();
    err
}

pub fn job_id(data: &Value) -> String {
    data["job"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("data.job.id is not a string: {data}"))
        .to_owned()
}

/// §A10 job view: every key present, with the documented shapes.
pub fn assert_job_view(job: &Value) {
    let obj = job
        .as_object()
        .unwrap_or_else(|| panic!("job view is not an object: {job}"));
    for k in JOB_VIEW {
        assert!(obj.contains_key(k), "job view lacks `{k}`: {job}");
    }
    let id = job["id"].as_str().expect("job.id");
    assert_uuid_v7(id);
    assert_eq!(job["handle"], job_handle(id), "job.handle: {job}");
    let status = job["status"].as_str().unwrap_or("");
    assert!(
        [
            "queued",
            "accepted",
            "completed",
            "aborted",
            "failed",
            "lost"
        ]
        .contains(&status),
        "job.status {status:?}: {job}"
    );
    // §A7 (W3): routes are `rpc|new|live`.
    assert!(
        ["rpc", "new", "live"].contains(&job["route"].as_str().unwrap_or("")),
        "{job}"
    );
    assert!(
        ["auto", "steer", "follow_up"].contains(&job["deliver_as"].as_str().unwrap_or("")),
        "{job}"
    );
    utc(&job["created_at"]);
    utc(&job["updated_at"]);
    assert!(job["elapsed_ms"].is_u64(), "elapsed_ms: {job}");
}

/// `job_failed` details: `{status, job, error, forked}` (AC-205), with the job error `code`.
pub fn assert_job_failed(err: &Value, status: &str, code: &str) {
    let d = &err["details"];
    for k in ["status", "job", "error", "forked"] {
        assert!(d.get(k).is_some(), "details lack `{k}`: {err}");
    }
    assert_eq!(d["status"], status, "details.status: {err}");
    assert_eq!(d["error"]["code"], code, "details.error.code: {err}");
    assert!(
        d["error"]["message"]
            .as_str()
            .is_some_and(|m| !m.trim().is_empty()),
        "details.error.message: {err}"
    );
    assert_job_view(&d["job"]);
    assert_eq!(d["job"]["status"], status, "details.job.status: {err}");
}

/// A §A5 reply record: every field present (absent values are `null`); the RPC route has no
/// request id.
pub fn assert_reply(r: &Value, session: &str, job: &str, call: &str, text: &str, fin: bool) {
    let o = r
        .as_object()
        .unwrap_or_else(|| panic!("reply is not an object: {r}"));
    for k in [
        "v",
        "id",
        "session_id",
        "job_id",
        "request_id",
        "tool_call_id",
        "text",
        "final",
        "at",
    ] {
        assert!(
            o.contains_key(k),
            "reply lacks `{k}` (nulls are never omitted): {r}"
        );
    }
    assert_eq!(r["v"], 1, "{r}");
    assert!(r["id"].as_str().is_some_and(|s| !s.is_empty()), "{r}");
    assert_eq!(r["session_id"], session, "{r}");
    assert_eq!(r["job_id"], job, "{r}");
    assert!(r["request_id"].is_null(), "{r}");
    assert_eq!(r["tool_call_id"], call, "{r}");
    assert_eq!(r["text"], text, "{r}");
    assert_eq!(r["final"], fin, "{r}");
    utc(&r["at"]);
}

pub fn ndjson(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{}: {e}: {l}", path.display())))
        .collect()
}

/// §A6: every directory under `root` is 0700 and every file 0600.
pub fn assert_private_tree(root: &Path) {
    assert_eq!(mode(root), 0o700, "{}", root.display());
    for rel in list_tree(root) {
        let p = root.join(&rel);
        let md = fs::symlink_metadata(&p).unwrap();
        let want = if md.is_dir() { 0o700 } else { 0o600 };
        assert_eq!(mode(&p), want, "{rel} has mode {:o}", mode(&p));
    }
}

pub fn read_json(path: &Path) -> Option<Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

// ---------------------------------------------------------------------------------------------
// The sandbox.
// ---------------------------------------------------------------------------------------------

pub struct RpcBox {
    pub bx: OmpBox,
    pub fake: PathBuf,
    /// The default `FAKE_OMP_LOG`, shared by every fake of this box unless a run overrides it.
    pub log: PathBuf,
}

impl RpcBox {
    pub fn new() -> Self {
        let fake = fake_omp_bin().to_path_buf();
        let bx = OmpBox::new();
        let logs = bx.root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        RpcBox {
            log: logs.join("fake.ndjson"),
            fake,
            bx,
        }
    }

    /// Another fake log path (for runs whose frames must not mix with the default log).
    pub fn log_path(&self, name: &str) -> PathBuf {
        self.bx.root.join("logs").join(format!("{name}.ndjson"))
    }

    /// A `glen` command with the box environment, from `cwd`, stdin `/dev/null`.
    pub fn command_in(&self, cwd: &Path, args: &[OsString]) -> Command {
        let mut cmd = Command::new(BIN);
        cmd.env_clear()
            .args(args)
            .env("HOME", &self.bx.home)
            .env("GLEN_HOME", &self.bx.glen_home)
            .env("PI_CODING_AGENT_DIR", &self.bx.agent)
            .env("TMPDIR", &self.bx.tmpdir)
            .env("TZ", "UTC")
            .env("PATH", &self.bx.empty_bin)
            .env("GLEN_OMP_BIN", &self.fake)
            .env("FAKE_OMP_LOG", &self.log)
            .env("GLEN_JOB_TIMEOUT", "30s")
            .env("GLEN_MAX_OUTPUT", "0")
            .current_dir(cwd)
            .stdin(Stdio::null());
        cmd
    }

    pub fn command<S: AsRef<OsStr>>(&self, args: &[S]) -> Command {
        let args: Vec<OsString> = args.iter().map(|a| a.as_ref().to_owned()).collect();
        self.command_in(&self.bx.alpha, &args)
    }

    /// `glen <args>` from `$HOME/work/alpha`.
    pub fn glen<I, S>(&self, args: I) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.glen_in(&self.bx.alpha.clone(), args)
    }

    pub fn glen_in<I, S>(&self, cwd: &Path, args: I) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
        let desc = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        Run::from_command(self.command_in(cwd, &args), desc)
    }

    /// Run with `input` on stdin.
    pub fn glen_stdin(&self, args: &[&str], env: &[(&str, &str)], input: &[u8]) -> Out {
        let mut cmd = self.command(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn glen");
        let mut stdin = child.stdin.take().unwrap();
        let bytes = input.to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes);
        });
        finish(child, &args.join(" "), Duration::from_secs(60))
    }

    /// Spawn `glen` as the leader of its own process group (a terminal's foreground job), with
    /// piped stdout and stderr.
    pub fn spawn_group(&self, args: &[&str], env: &[(&str, &str)]) -> Child {
        let mut cmd = self.command(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.spawn().expect("spawn glen")
    }

    // -- jobs on disk -------------------------------------------------------------------------

    pub fn jobs_dir(&self) -> PathBuf {
        self.bx.glen_home.join("jobs")
    }

    pub fn job_dir(&self, id: &str) -> PathBuf {
        self.jobs_dir().join(id)
    }

    /// Names of the directories under `$GLEN_HOME/jobs`, sorted.
    pub fn job_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.jobs_dir())
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    pub fn job_json(&self, id: &str) -> Value {
        let p = self.job_dir(id).join("job.json");
        read_json(&p).unwrap_or_else(|| panic!("{} is missing or not JSON", p.display()))
    }

    pub fn try_status(&self, id: &str) -> Option<Value> {
        read_json(&self.job_dir(id).join("status.json"))
    }

    pub fn status(&self, id: &str) -> Value {
        self.try_status(id)
            .unwrap_or_else(|| panic!("job {id} has no readable status.json"))
    }

    pub fn result(&self, id: &str) -> Value {
        let p = self.job_dir(id).join("result.json");
        read_json(&p).unwrap_or_else(|| panic!("{} is missing or not JSON", p.display()))
    }

    pub fn wait_status(
        &self,
        id: &str,
        what: &str,
        timeout: Duration,
        pred: impl Fn(&Value) -> bool,
    ) -> Value {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(st) = self.try_status(id)
                && pred(&st)
            {
                return st;
            }
            if Instant::now() >= deadline {
                panic!(
                    "job {id}: no `{what}` status within {timeout:?}; last status: {:?}\n{}",
                    self.try_status(id),
                    self.diag()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn wait_terminal(&self, id: &str) -> Value {
        self.wait_status(id, "terminal", LONG, is_terminal)
    }

    /// The only job directory, once it exists.
    pub fn wait_one_job(&self) -> String {
        poll_until("a job directory", LONG, || {
            let ids = self.job_ids();
            (ids.len() == 1).then(|| ids[0].clone())
        })
    }

    pub fn wait_file(&self, path: &Path, timeout: Duration) -> Value {
        poll_until(&format!("{}", path.display()), timeout, || read_json(path))
    }

    // -- fake log ------------------------------------------------------------------------------

    pub fn recs(&self) -> Vec<Value> {
        records(&self.log)
    }

    pub fn inbound(&self, kind: &str) -> Vec<Value> {
        frames_in(&self.recs(), "in", kind)
    }

    pub fn starts(&self) -> Vec<Value> {
        starts(&self.recs())
    }

    pub fn wait_log(&self, what: &str, pred: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        poll_until(what, LONG, || {
            let r = self.recs();
            pred(&r).then_some(r)
        })
    }

    fn log_files(&self) -> Vec<PathBuf> {
        fs::read_dir(self.bx.root.join("logs"))
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default()
    }

    // -- locks ---------------------------------------------------------------------------------

    /// `$GLEN_HOME/run/locks/<key>.lock`; a plain token id is its own key (§A6).
    pub fn lock_path(&self, session_id: &str) -> PathBuf {
        self.bx
            .glen_home
            .join("run/locks")
            .join(format!("{session_id}.lock"))
    }

    /// Hold glen's per-session lock the way a running worker does (exclusive flock).
    pub fn hold_glen_lock(&self, session_id: &str) -> fs::File {
        let locks = self.bx.glen_home.join("run/locks");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&locks)
            .unwrap();
        let f = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.lock_path(session_id))
            .unwrap();
        f.try_lock().expect("flock the glen session lock");
        f
    }

    /// The glen session lock is not held by anyone (a missing file is free).
    pub fn glen_lock_free(&self, session_id: &str) -> bool {
        let Ok(f) = fs::File::open(self.lock_path(session_id)) else {
            return true;
        };
        match f.try_lock() {
            Ok(()) => {
                let _ = f.unlock();
                true
            }
            Err(fs::TryLockError::WouldBlock) => false,
            Err(fs::TryLockError::Error(e)) => panic!("flock probe: {e}"),
        }
    }

    // -- crafted jobs --------------------------------------------------------------------------

    /// Send to fixture A and wait for completion; returns the job id (a template for crafting).
    pub fn completed_job(&self, text: &str) -> String {
        let out = self
            .glen(["send", A.handle, text, "--wait", "20s", "--json"])
            .run();
        let d = data_of(&out, 0);
        let id = job_id(&d);
        self.wait_terminal(&id);
        id
    }

    /// Copy a real job directory under `new_id`: `job.json` gets the new `id`, `handle` and
    /// `created_at` (plus `job_patch`); `status_patch` is merged into `status.json` (a `null`
    /// value removes the key) and the file's mtime is set to its `updated_at`. Only regular files
    /// are copied; `result.json` and `replies.ndjson` only when `keep_result`.
    pub fn clone_job(
        &self,
        template: &str,
        new_id: &str,
        created: SystemTime,
        status_patch: Value,
        keep_result: bool,
    ) -> PathBuf {
        let src = self.job_dir(template);
        let dst = self.job_dir(new_id);
        fs::DirBuilder::new().mode(0o700).create(&dst).unwrap();
        for e in fs::read_dir(&src).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !e.path().is_file()
                || name == "job.json"
                || name == "status.json"
                || (!keep_result && (name == "result.json" || name == "replies.ndjson"))
            {
                continue;
            }
            write_private(&dst.join(&name), &fs::read(e.path()).unwrap());
        }
        let mut job = self.job_json(template);
        job["id"] = Value::from(new_id);
        job["handle"] = Value::from(job_handle(new_id));
        job["created_at"] = Value::from(rfc3339(created));
        if let Some(ms) = job["job_timeout_ms"].as_u64() {
            job["deadline_at"] = Value::from(rfc3339(created + Duration::from_millis(ms)));
        }
        write_private(
            &dst.join("job.json"),
            &serde_json::to_vec_pretty(&job).unwrap(),
        );
        let mut st = self.status(template);
        merge(&mut st, &status_patch);
        let status_path = dst.join("status.json");
        write_private(&status_path, &serde_json::to_vec_pretty(&st).unwrap());
        if let Some(t) = st["updated_at"].as_str() {
            let ts: jiff::Timestamp = t.parse().unwrap();
            set_mtime(&status_path, SystemTime::from(ts));
        }
        dst
    }

    // -- teardown ------------------------------------------------------------------------------

    /// Worker and fake omp processes of this box that still exist.
    pub fn live_processes(&self) -> Vec<String> {
        let me = std::process::id() as i64;
        let mine = own_pgid();
        let mut v = Vec::new();
        for id in self.job_ids() {
            if let Some(pid) = self.try_status(&id).and_then(|s| s["worker_pid"].as_i64())
                && pid != me
                && pid_alive(pid)
            {
                v.push(format!("worker {pid} (job {id})"));
            }
        }
        for log in self.log_files() {
            for s in starts(&records(&log)) {
                let pid = s["pid"].as_i64().unwrap_or(0);
                let pgid = s["pgid"].as_i64().unwrap_or(0);
                if pid_alive(pid) {
                    v.push(format!("fake omp {pid}"));
                }
                if pgid != mine && pgid != me && group_alive(pgid) {
                    v.push(format!("fake omp group {pgid}"));
                }
            }
        }
        v
    }

    /// Every worker and fake omp process has exited (bounded at 20 s).
    pub fn assert_quiet(&self) {
        let deadline = Instant::now() + LONG;
        loop {
            let live = self.live_processes();
            if live.is_empty() {
                return;
            }
            if Instant::now() >= deadline {
                panic!("still running {LONG:?} later: {live:?}\n{}", self.diag());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Job statuses and the fake log, for failure messages.
    pub fn diag(&self) -> String {
        let mut s = String::new();
        for id in self.job_ids() {
            s.push_str(&format!(
                "--- job {id} status: {:?}\n",
                self.try_status(&id)
            ));
            if let Ok(log) = fs::read_to_string(self.job_dir(&id).join("worker.log")) {
                let tail: Vec<&str> = log.lines().rev().take(15).collect();
                s.push_str("    worker.log tail:\n");
                for l in tail.into_iter().rev() {
                    s.push_str(&format!("      {l}\n"));
                }
            }
        }
        for log in self.log_files() {
            let recs = records(&log);
            s.push_str(&format!("--- {} ({} records)\n", log.display(), recs.len()));
            for r in recs
                .iter()
                .rev()
                .take(25)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                let mut line = r.to_string();
                line.truncate(300);
                s.push_str(&format!("    {line}\n"));
            }
        }
        s
    }
}

impl Drop for RpcBox {
    fn drop(&mut self) {
        // Best effort, never panics: a failed test must not leave a worker or omp running.
        let me = std::process::id() as i64;
        let mine = own_pgid();
        for id in self.job_ids() {
            if let Some(pid) = self.try_status(&id).and_then(|s| s["worker_pid"].as_i64())
                && pid != me
                && let Some(p) = pid_of(pid)
            {
                let _ = rustix::process::kill_process(p, Signal::KILL);
            }
        }
        for log in self.log_files() {
            for s in starts(&records(&log)) {
                let pgid = s["pgid"].as_i64().unwrap_or(0);
                if pgid != mine
                    && pgid != me
                    && let Some(p) = pid_of(pgid)
                {
                    let _ = rustix::process::kill_process_group(p, Signal::KILL);
                }
                if let Some(p) = pid_of(s["pid"].as_i64().unwrap_or(0)) {
                    let _ = rustix::process::kill_process(p, Signal::KILL);
                }
            }
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.live_processes().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

pub fn is_terminal(st: &Value) -> bool {
    TERMINAL.contains(&st["status"].as_str().unwrap_or(""))
}

fn merge(base: &mut Value, patch: &Value) {
    let (Some(b), Some(p)) = (base.as_object_mut(), patch.as_object()) else {
        return;
    };
    for (k, v) in p {
        if v.is_null() {
            b.remove(k);
        } else {
            b.insert(k.clone(), v.clone());
        }
    }
}

pub fn write_private(path: &Path, bytes: &[u8]) {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    f.write_all(bytes).unwrap();
}

/// Drop a control request atomically (temp dotfile in the same dir, then rename), as §A7 says.
pub fn write_control(rb: &RpcBox, id: &str, name: &str, body: &str) {
    let dir = rb.job_dir(id).join("control");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .unwrap();
    let tmp = dir.join(format!(".{name}.staging"));
    write_private(&tmp, body.as_bytes());
    fs::rename(&tmp, dir.join(name)).unwrap();
}

/// A JSON object from key/value pairs (for status patches).
pub fn obj(pairs: &[(&str, Value)]) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert((*k).to_owned(), v.clone());
    }
    Value::Object(m)
}

pub fn jstr(s: &str) -> Value {
    json!(s)
}

#[cfg(test)]
mod record_tests {
    use super::*;

    #[test]
    fn complete_records_survive_an_unfinished_trailing_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fake.ndjson");
        fs::write(&path, b"{\"n\":1}\n{\"n\":2}\n{\"n\":").unwrap();
        assert_eq!(records(&path), vec![json!({"n":1}), json!({"n":2})]);
        fs::write(&path, b"{\"n\":1}\n{\"n\":2}").unwrap();
        assert_eq!(records(&path), vec![json!({"n":1}), json!({"n":2})]);
        fs::write(&path, b"{\"n\":1}\n{\"text\":\"\xf0\x9f").unwrap();
        assert_eq!(records(&path), vec![json!({"n":1})]);
        fs::write(&path, b"").unwrap();
        assert!(records(&path).is_empty());
        assert!(records(&temp.path().join("absent")).is_empty());
    }

    #[test]
    fn malformed_complete_records_panic_with_the_path_and_line() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("torn.ndjson");
        for bytes in [b"{\"n\":1}\nTORN\n".as_slice(), b"TORN\n{\"n\":"] {
            fs::write(&path, bytes).unwrap();
            let panic = std::panic::catch_unwind(|| records(&path)).unwrap_err();
            let message = panic.downcast_ref::<String>().unwrap();
            assert!(
                message.contains("malformed complete fake record"),
                "{message}"
            );
            assert!(message.contains(path.to_str().unwrap()), "{message}");
            assert!(message.contains("TORN"), "{message}");
        }
    }
}
