//! Shared harness for the W1 reader conformance cases (AC-101…AC-126), derived from
//! `.claude/plans/v010-wave-1-reader.md` and `docs/design.md` §A3, §A8–§A11 without reading the
//! implementation.
//!
//! `OmpBox` copies `tests/fixtures/omp/agent` into a temp dir outside HOME and gives the copies the
//! plan's deterministic mtimes (Step 13): G now−1m, E −2m, D −3m, C −4m, B −5m, A −6m, F −30d.
//! Every `glen` run clears the environment and sets HOME, GLEN_HOME (never pre-created),
//! PI_CODING_AGENT_DIR, TMPDIR (outside HOME), TZ=UTC and an empty PATH on its own `Command`.
//! The checked-in fixtures are only read, and nothing touches a live `~/.omp`.

#![allow(dead_code)]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use crate::support::*;

pub const ALPHA: &str = "-work-alpha";
pub const BETA: &str = "-work-beta";
/// The nested subagent transcript (fixture S): never discovered.
pub const S_ID: &str = "01a0f6b5-ba60-7012-8f3b-3f7a9c1e5b20";
pub const MODEL: &str = "anthropic/claude-sonnet-4-5";
/// Generous bound for one streamed expectation (plan Step 13 uses 10 s).
pub const T: Duration = Duration::from_secs(10);

/// A discoverable fixture session (tests/fixtures/README.md).
pub struct Fx {
    pub rel: &'static str,
    pub id: &'static str,
    pub handle: &'static str,
    pub title: &'static str,
    pub bucket: &'static str,
    pub cwd: &'static str,
    pub created: &'static str,
    pub first_message: &'static str,
    /// Seconds before `OmpBox::t0` that the copy's mtime is set to.
    pub age_s: u64,
}

pub const A: Fx = Fx {
    rel: "-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl",
    id: "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
    handle: "e27c0de",
    title: "Fix the flaky parser test",
    bucket: ALPHA,
    cwd: "/home/fixture/work/alpha",
    created: "2026-10-01T09:00:00Z",
    first_message: "Why does parser_test flake?",
    age_s: 360,
};
pub const B: Fx = Fx {
    rel: "-work-alpha/2026-10-02T14-30-00-000Z_01a0fd05-a240-7c41-9d02-7d2e8f13a5b6.jsonl",
    id: "01a0fd05-a240-7c41-9d02-7d2e8f13a5b6",
    handle: "f13a5b6",
    title: "Continue the fixture review",
    bucket: ALPHA,
    cwd: "/home/fixture/work/alpha",
    created: "2026-10-02T14:30:00Z",
    first_message: "Review the first fixture.",
    age_s: 300,
};
pub const C: Fx = Fx {
    rel: "-work-alpha/2026-10-02T15-10-00-000Z_01a0fd2a-4140-7e05-a3f4-0c9a4b7e2d81.jsonl",
    id: "01a0fd2a-4140-7e05-a3f4-0c9a4b7e2d81",
    handle: "b7e2d81",
    title: "Continue the fixture review",
    bucket: ALPHA,
    cwd: "/home/fixture/work/alpha",
    created: "2026-10-02T15:10:00Z",
    first_message: "Review the first fixture.",
    age_s: 240,
};
pub const D: Fx = Fx {
    rel: "-work-beta/2026-10-03T08-00-00-000Z_01a100c6-f000-7b18-8e6a-5e8b1c4f9a72.jsonl",
    id: "01a100c6-f000-7b18-8e6a-5e8b1c4f9a72",
    handle: "c4f9a72",
    title: "Try another approach",
    bucket: BETA,
    cwd: "/home/fixture/work/beta",
    created: "2026-10-03T08:00:00Z",
    first_message: "Try a parser.",
    age_s: 180,
};
pub const E: Fx = Fx {
    rel: "-work-beta/2026-10-04T11-45-00-000Z_01a106bb-4a60-7f92-b0c3-b2d6e0a3f417.jsonl",
    id: "01a106bb-4a60-7f92-b0c3-b2d6e0a3f417",
    handle: "0a3f417",
    title: "Compact placeholder history",
    bucket: BETA,
    cwd: "/home/fixture/work/beta",
    created: "2026-10-04T11:45:00Z",
    first_message: "Start.",
    age_s: 120,
};
pub const F: Fx = Fx {
    rel: "-work-beta/2026-09-15T07-20-00-000Z_01a0a3ef-d900-7d6e-9a57-9b3d5a81c0de.jsonl",
    id: "01a0a3ef-d900-7d6e-9a57-9b3d5a81c0de",
    handle: "a81c0de",
    title: "Legacy title",
    bucket: BETA,
    cwd: "/home/fixture/work/beta",
    created: "2026-09-15T07:20:00Z",
    first_message: "Placeholder request.",
    age_s: 30 * 86_400,
};
pub const G: Fx = Fx {
    rel: "-work-alpha/2026-10-05T16-20-00-000Z_01a10cdd-6b80-7a29-9c5e-6d1f3b8e4a07.jsonl",
    id: "01a10cdd-6b80-7a29-9c5e-6d1f3b8e4a07",
    handle: "b8e4a07",
    title: "Fork a placeholder session",
    bucket: ALPHA,
    cwd: "/home/fixture/work/alpha",
    created: "2026-10-05T16:20:00Z",
    first_message: "Placeholder request.",
    age_s: 60,
};

/// The seven discoverable fixtures, newest `OmpBox` mtime first.
pub const ALL: [&Fx; 7] = [&G, &E, &D, &C, &B, &A, &F];

/// Visible transcript items (AC-112–AC-114): entry ids on the active branch that render.
pub const A_ITEMS: [&str; 5] = ["10000001", "10000002", "10000003", "10000005", "10000007"];
pub const C_ITEMS: [&str; 6] = [
    "20000001", "20000002", "20000003", "30000004", "30000005", "30000006",
];
pub const D_ITEMS: [&str; 5] = ["40000001", "40000002", "40000005", "40000006", "40000007"];
pub const E_ITEMS: [&str; 10] = [
    "50000001", "50000002", "50000003", "50000004", "50000005", "50000006", "50000007", "50000009",
    "5000000a", "5000000b",
];

pub fn want(fxs: &[&Fx]) -> Vec<String> {
    fxs.iter().map(|f| f.id.to_owned()).collect()
}

pub fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

// ---------------------------------------------------------------------------------------------
// omp JSONL v3 builders (shapes from omp v18.14.0 and tests/fixtures).
// ---------------------------------------------------------------------------------------------

/// A uuidv7-shaped id whose last 12 hex digits are `n`.
pub fn uuid(n: u64) -> String {
    format!("01a20000-0000-7000-8000-{n:012x}")
}

/// `2026-10-06T10:00:00.000Z` + id → `2026-10-06T10-00-00-000Z_<id>.jsonl`.
pub fn session_name(created: &str, id: &str) -> String {
    format!("{}_{id}.jsonl", created.replace([':', '.'], "-"))
}

pub fn header(id: &str, created: Option<&str>, cwd: Option<&str>) -> Value {
    let mut h = json!({"type": "session", "version": 3, "id": id});
    if let Some(t) = created {
        h["timestamp"] = Value::from(t);
    }
    if let Some(c) = cwd {
        h["cwd"] = Value::from(c);
    }
    h
}

pub fn title_slot(title: &str) -> Value {
    json!({"type": "title", "v": 1, "title": title, "source": "auto"})
}

pub fn entry(kind: &str, id: &str, parent: Option<&str>, ts: &str, extra: Value) -> Value {
    let mut e = json!({"type": kind, "id": id, "parentId": parent, "timestamp": ts});
    if let Value::Object(m) = extra {
        for (k, v) in m {
            e[k] = v;
        }
    }
    e
}

pub fn message(id: &str, parent: Option<&str>, ts: &str, msg: Value) -> Value {
    entry("message", id, parent, ts, json!({"message": msg}))
}

pub fn user(id: &str, parent: Option<&str>, ts: &str, text: &str) -> Value {
    message(
        id,
        parent,
        ts,
        json!({"role": "user", "content": [{"type": "text", "text": text}], "timestamp": 0, "attribution": "user"}),
    )
}

pub fn assistant(id: &str, parent: Option<&str>, ts: &str, content: Value, stop: &str) -> Value {
    message(
        id,
        parent,
        ts,
        json!({
            "role": "assistant", "content": content, "api": "anthropic-messages",
            "provider": "anthropic", "model": "claude-sonnet-4-5",
            "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                      "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}},
            "stopReason": stop, "timestamp": 0
        }),
    )
}

pub fn reply(id: &str, parent: Option<&str>, ts: &str, text: &str) -> Value {
    assistant(
        id,
        parent,
        ts,
        json!([{"type": "text", "text": text}]),
        "stop",
    )
}

pub fn tool_call(call_id: &str, name: &str, args: Value) -> Value {
    json!({"type": "toolCall", "id": call_id, "name": name, "arguments": args})
}

pub fn tool_result(
    id: &str,
    parent: Option<&str>,
    ts: &str,
    call_id: &str,
    name: &str,
    text: &str,
    is_error: bool,
) -> Value {
    message(
        id,
        parent,
        ts,
        json!({"role": "toolResult", "toolCallId": call_id, "toolName": name,
               "content": [{"type": "text", "text": text}], "isError": is_error, "timestamp": 0}),
    )
}

pub fn jsonl(lines: &[Value]) -> String {
    lines.iter().map(|v| format!("{v}\n")).collect()
}

pub fn write_lines(path: &Path, lines: &[Value]) {
    fs::write(path, jsonl(lines)).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

pub fn append_line(path: &Path, v: &Value) {
    let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(format!("{v}\n").as_bytes()).unwrap();
    f.flush().unwrap();
}

/// The raw text lines of a session file after its title slot and header.
pub fn entry_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| {
            let v: Value = serde_json::from_str(l).unwrap_or(Value::Null);
            v["type"] != "title" && v["type"] != "session"
        })
        .map(str::to_owned)
        .collect()
}

/// Parsed entries of a fixture copy (title slot and header excluded), keyed by id.
pub fn raw_entry(path: &Path, id: &str) -> Value {
    entry_lines(path)
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["id"] == id)
        .unwrap_or_else(|| panic!("no entry {id} in {}", path.display()))
}

pub fn set_mtime(p: &Path, t: SystemTime) {
    let f = fs::File::options()
        .write(true)
        .open(p)
        .unwrap_or_else(|e| panic!("open {}: {e}", p.display()));
    f.set_modified(t).unwrap();
}

pub fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &dst);
        } else {
            fs::copy(e.path(), &dst).unwrap();
        }
    }
}

/// (relative path, size for files, mtime) for every entry under `root`, without following links.
pub fn snapshot(root: &Path) -> Vec<(String, u64, SystemTime)> {
    list_tree(root)
        .into_iter()
        .map(|rel| {
            let md = fs::symlink_metadata(root.join(&rel)).unwrap();
            let len = if md.is_dir() { 0 } else { md.len() };
            (rel, len, md.modified().unwrap())
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The sandbox.
// ---------------------------------------------------------------------------------------------

pub struct OmpBox {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    /// Copy of tests/fixtures/omp/agent, outside HOME (PI_CODING_AGENT_DIR).
    pub agent: PathBuf,
    pub sessions: PathBuf,
    /// TMPDIR for glen: outside HOME, so HOME paths never map to `-tmp-…` buckets.
    pub tmpdir: PathBuf,
    /// GLEN_HOME; does not exist until glen spills.
    pub glen_home: PathBuf,
    pub empty_bin: PathBuf,
    /// `$HOME/work/alpha` and `$HOME/work/beta` (buckets `-work-alpha`, `-work-beta`).
    pub alpha: PathBuf,
    pub beta: PathBuf,
    /// When the fixture mtimes were set.
    pub t0: SystemTime,
}

impl OmpBox {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("glen-w1-")
            .tempdir()
            .expect("tempdir");
        let root = dir.path().to_path_buf();
        let home = root.join("home");
        let tmpdir = root.join("tmp");
        let empty_bin = root.join("emptybin");
        let alpha = home.join("work/alpha");
        let beta = home.join("work/beta");
        for d in [&tmpdir, &empty_bin, &alpha, &beta] {
            fs::create_dir_all(d).unwrap();
        }
        let agent = root.join("agent");
        copy_tree(&fixture_agent_dir(), &agent);
        let sessions = agent.join("sessions");
        let t0 = SystemTime::now();
        for fx in ALL {
            set_mtime(&sessions.join(fx.rel), t0 - Duration::from_secs(fx.age_s));
        }
        OmpBox {
            _dir: dir,
            glen_home: root.join("glen"),
            root,
            home,
            agent,
            sessions,
            tmpdir,
            empty_bin,
            alpha,
            beta,
            t0,
        }
    }

    pub fn path(&self, fx: &Fx) -> PathBuf {
        self.sessions.join(fx.rel)
    }

    /// `<base>/run/session-owners` for the default base `$HOME/.omp` (§A12 rule 7).
    pub fn owners(&self) -> PathBuf {
        self.home.join(".omp/run/session-owners")
    }

    /// A bucket directory under the sandbox `sessions_dir` (created).
    pub fn bucket(&self, name: &str) -> PathBuf {
        let p = self.sessions.join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// `$HOME/work/<name>` (created).
    pub fn work(&self, name: &str) -> PathBuf {
        let p = self.home.join("work").join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// Write `header(id, created, cwd)` + `body` as `<bucket>/<created>_<id>.jsonl`.
    pub fn add(
        &self,
        bucket: &str,
        created: &str,
        id: &str,
        cwd: Option<&str>,
        body: &[Value],
    ) -> PathBuf {
        let path = self.bucket(bucket).join(session_name(created, id));
        let mut lines = vec![header(id, Some(created), cwd)];
        lines.extend_from_slice(body);
        write_lines(&path, &lines);
        path
    }

    fn command(&self, cwd: &Path, args: &[OsString]) -> (Command, String) {
        let mut cmd = Command::new(BIN);
        cmd.env_clear()
            .args(args)
            .env("HOME", &self.home)
            .env("GLEN_HOME", &self.glen_home)
            .env("PI_CODING_AGENT_DIR", &self.agent)
            .env("TMPDIR", &self.tmpdir)
            .env("TZ", "UTC")
            .env("PATH", &self.empty_bin)
            .current_dir(cwd)
            .stdin(Stdio::null());
        let desc = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        (cmd, desc)
    }

    /// `glen <args>` from `$HOME/work/alpha`.
    pub fn glen<I, S>(&self, args: I) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.glen_in(&self.alpha, args)
    }

    pub fn glen_in<I, S>(&self, cwd: &Path, args: I) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
        let (cmd, desc) = self.command(cwd, &args);
        Run::from_command(cmd, desc)
    }

    /// A successful JSON document's `data`; JSON mode keeps stderr empty and ANSI-free.
    pub fn data<I, S>(&self, args: I) -> Value
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.data_in(&self.alpha, args)
    }

    pub fn data_in<I, S>(&self, cwd: &Path, args: I) -> Value
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.glen_in(cwd, args).run();
        let data = out.ok_data();
        out.assert_stderr_empty().assert_no_ansi();
        data
    }

    /// Spawn a long-running `glen` (e.g. `tail -f`) from `$HOME/work/alpha`.
    pub fn follow(&self, args: &[&str]) -> Stream {
        self.follow_env(args, &[])
    }

    pub fn follow_env(&self, args: &[&str], env: &[(&str, &str)]) -> Stream {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let (mut cmd, desc) = self.command(&self.alpha, &args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        Stream::spawn(cmd, desc)
    }

    /// AC-122: no GLEN_HOME and nothing under the omp config root `$HOME/.omp`.
    pub fn assert_no_state(&self) {
        assert!(
            !self.glen_home.exists(),
            "GLEN_HOME was created: {:?}",
            list_tree(&self.glen_home)
        );
        assert!(
            !self.home.join(".omp").exists(),
            "something was created under $HOME/.omp: {:?}",
            list_tree(&self.home.join(".omp"))
        );
    }
}

// ---------------------------------------------------------------------------------------------
// JSON helpers.
// ---------------------------------------------------------------------------------------------

pub fn arr(v: &Value) -> &Vec<Value> {
    v.as_array()
        .unwrap_or_else(|| panic!("expected an array, got {v}"))
}

/// `data.sessions[].id`.
pub fn ids(data: &Value) -> Vec<String> {
    arr(&data["sessions"])
        .iter()
        .map(|s| s["id"].as_str().expect("session id").to_owned())
        .collect()
}

/// The `ls` row for `id` (exactly one).
pub fn row<'a>(data: &'a Value, id: &str) -> &'a Value {
    let rows: Vec<&Value> = arr(&data["sessions"])
        .iter()
        .filter(|s| s["id"] == id)
        .collect();
    assert_eq!(rows.len(), 1, "expected one row for {id}: {data}");
    rows[0]
}

pub fn item_ids(data: &Value) -> Vec<String> {
    arr(&data["items"])
        .iter()
        .map(|i| i["id"].as_str().expect("item id").to_owned())
        .collect()
}

pub fn kinds(data: &Value) -> Vec<String> {
    arr(&data["items"])
        .iter()
        .map(|i| i["kind"].as_str().expect("item kind").to_owned())
        .collect()
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// A JSON path field: absolute and naming `expected` on disk.
pub fn assert_path(v: &Value, expected: &Path) {
    let p = Path::new(
        v.as_str()
            .unwrap_or_else(|| panic!("path is not a string: {v}")),
    );
    assert!(p.is_absolute(), "JSON paths are absolute: {}", p.display());
    assert!(
        same_file(p, expected),
        "{} is not {}",
        p.display(),
        expected.display()
    );
}

/// Parse an RFC 3339 timestamp and require UTC (`Z` or `+00:00`).
pub fn utc(v: &Value) -> jiff::Timestamp {
    let s = v
        .as_str()
        .unwrap_or_else(|| panic!("timestamp is not a string: {v}"));
    assert!(
        s.ends_with('Z') || s.ends_with("+00:00"),
        "timestamp {s:?} is not RFC 3339 UTC"
    );
    s.parse::<jiff::Timestamp>()
        .unwrap_or_else(|e| panic!("timestamp {s:?} does not parse: {e}"))
}

pub fn instant(s: &str) -> jiff::Timestamp {
    s.parse().unwrap()
}

pub fn system(t: SystemTime) -> jiff::Timestamp {
    jiff::Timestamp::try_from(t).unwrap()
}

/// The AC-106 session object fields.
pub const SESSION_FIELDS: [&str; 18] = [
    "handle",
    "id",
    "path",
    "bucket",
    "cwd",
    "title",
    "first_message",
    "created_at",
    "updated_at",
    "age_ms",
    "size_bytes",
    "model",
    "last_turn",
    "state",
    "state_source",
    "state_detail",
    "continuation_of",
    "parent_session",
];

/// AC-106: every field present (nullable ones as `null`), with the documented types and values.
pub fn assert_session_object(s: &Value) {
    let obj = s
        .as_object()
        .unwrap_or_else(|| panic!("session is not an object: {s}"));
    for k in SESSION_FIELDS {
        assert!(obj.contains_key(k), "session object lacks `{k}`: {s}");
    }
    for k in ["handle", "id", "path", "bucket"] {
        assert!(s[k].is_string(), "`{k}` is not a string: {s}");
    }
    assert!(Path::new(s["path"].as_str().unwrap()).is_absolute(), "{s}");
    for k in [
        "cwd",
        "title",
        "first_message",
        "model",
        "state_detail",
        "continuation_of",
        "parent_session",
    ] {
        assert!(
            s[k].is_null() || s[k].is_string(),
            "`{k}` is neither null nor a string: {s}"
        );
    }
    if !s["created_at"].is_null() {
        utc(&s["created_at"]);
    }
    utc(&s["updated_at"]);
    assert!(
        s["age_ms"].is_u64(),
        "age_ms is a non-negative integer: {s}"
    );
    assert!(s["size_bytes"].is_u64(), "size_bytes is an integer: {s}");
    let turn = s["last_turn"].as_str().unwrap_or("");
    assert!(
        [
            "complete",
            "interrupted",
            "aborted",
            "error",
            "pending",
            "unknown"
        ]
        .contains(&turn),
        "last_turn {turn:?}: {s}"
    );
    let state = s["state"].as_str().unwrap_or("");
    assert!(
        ["idle", "live", "busy", "unknown"].contains(&state),
        "state {state:?}: {s}"
    );
    assert_eq!(s["state_source"], "disk", "W1 state_source: {s}");
}

/// The candidates of an `ambiguous` error, as text, after checking the count.
pub fn candidates(err: &Value, n: usize) -> String {
    let c = arr(&err["details"]["candidates"]);
    assert_eq!(c.len(), n, "candidate count: {err}");
    serde_json::to_string(c).unwrap()
}

/// Raw C0/C1 controls other than newline (glen must escape them in human output).
pub fn raw_controls(text: &str) -> Vec<char> {
    text.chars()
        .filter(|&c| (c < ' ' && c != '\n') || c == '\u{7f}' || ('\u{80}'..='\u{9f}').contains(&c))
        .collect()
}

pub fn has_esc_seq(b: &[u8]) -> bool {
    b.windows(2).any(|w| w == b"\x1b[")
}

pub fn display_width(line: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(strip_ansi(line).as_str())
}

/// A pipe whose read end is already closed: every write gets EPIPE.
pub fn closed_stdout() -> Stdio {
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    Stdio::from(writer)
}

// ---------------------------------------------------------------------------------------------
// Streams (`tail -f`).
// ---------------------------------------------------------------------------------------------

/// A running `glen` whose stdout lines arrive on a channel.
pub struct Stream {
    child: Child,
    rx: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<u8>>>,
    /// Every stdout line received so far, newline included.
    pub lines: Vec<String>,
    desc: String,
    status: Option<ExitStatus>,
}

impl Stream {
    fn spawn(mut cmd: Command, desc: String) -> Stream {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .unwrap_or_else(|e| panic!("spawn glen {desc}: {e}"));
        let stdout = child.stdout.take().unwrap();
        let mut err_pipe = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                let mut buf = Vec::new();
                match r.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(String::from_utf8_lossy(&buf).into_owned()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&stderr);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = err_pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&chunk[..n]);
            }
        });
        Stream {
            child,
            rx,
            stderr,
            lines: Vec::new(),
            desc,
            status: None,
        }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned()
    }

    pub fn ctx(&self) -> String {
        format!(
            "`glen {}` (exit {:?})\n--- stdout so far ---\n{}--- stderr ---\n{}",
            self.desc,
            self.status,
            self.lines.concat(),
            self.stderr()
        )
    }

    /// The next stdout line, or `None` on timeout or end of stream.
    pub fn next_line(&mut self, timeout: Duration) -> Option<String> {
        match self.rx.recv_timeout(timeout) {
            Ok(l) => {
                self.lines.push(l.clone());
                Some(l)
            }
            Err(_) => None,
        }
    }

    /// The next line as one complete, compact, ANSI-free JSON object with `v:1` (AC-117).
    pub fn next_json(&mut self, timeout: Duration) -> Value {
        let line = self
            .next_line(timeout)
            .unwrap_or_else(|| panic!("no stream line within {timeout:?}\n{}", self.ctx()));
        parse_stream_line(&line, &self.ctx())
    }

    /// The next line, which must be an event of `kind`.
    pub fn expect_event(&mut self, kind: &str, timeout: Duration) -> Value {
        let ev = self.next_json(timeout);
        assert_eq!(
            ev["event"],
            Value::from(kind),
            "expected a `{kind}` event\n{}",
            self.ctx()
        );
        ev
    }

    /// The next `n` lines, which must all be `item` events; returns the items.
    pub fn items(&mut self, n: usize, timeout: Duration) -> Vec<Value> {
        (0..n)
            .map(|_| {
                let ev = self.expect_event("item", timeout);
                assert!(
                    ev["item"].is_object(),
                    "item event without an item\n{}",
                    self.ctx()
                );
                ev["item"].clone()
            })
            .collect()
    }

    /// Read human lines until one contains `needle`.
    pub fn wait_for_text(&mut self, needle: &str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.next_line(left) {
                Some(l) if l.contains(needle) => return,
                Some(_) => {}
                None => panic!(
                    "no line containing {needle:?} within {timeout:?}\n{}",
                    self.ctx()
                ),
            }
        }
    }

    pub fn interrupt(&self) {
        let ok = Command::new("/bin/kill")
            .args(["-INT", &self.pid().to_string()])
            .status()
            .expect("run /bin/kill")
            .success();
        assert!(ok, "kill -INT failed");
    }

    /// Wait for the process to exit, then collect the rest of stdout.
    pub fn wait(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                break st;
            }
            if Instant::now() >= deadline {
                panic!("glen did not exit within {timeout:?}\n{}", self.ctx());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        self.status = Some(status);
        while let Ok(l) = self.rx.recv_timeout(Duration::from_secs(5)) {
            self.lines.push(l);
        }
        status
    }

    pub fn last_line(&self) -> Value {
        let line = self
            .lines
            .last()
            .unwrap_or_else(|| panic!("no stdout\n{}", self.ctx()));
        parse_stream_line(line, &self.ctx())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        if self.status.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// One NDJSON line: newline-terminated, one compact JSON object, no ANSI, `v:1`, and either an
/// `event` string or an error envelope.
pub fn parse_stream_line(line: &str, ctx: &str) -> Value {
    let body = line
        .strip_suffix('\n')
        .unwrap_or_else(|| panic!("stream line without a newline: {line:?}\n{ctx}"));
    assert!(
        !body.contains('\u{1b}'),
        "ANSI in a stream line: {body:?}\n{ctx}"
    );
    let v: Value = serde_json::from_str(body)
        .unwrap_or_else(|e| panic!("stream line is not one JSON object ({e}): {body:?}\n{ctx}"));
    assert!(v.is_object(), "stream line is not an object: {body}\n{ctx}");
    assert_eq!(v["v"], Value::from(1), "stream line v: {body}\n{ctx}");
    assert!(
        v["event"].is_string() || v["ok"] == Value::Bool(false),
        "stream line is neither an event nor an error envelope: {body}\n{ctx}"
    );
    v
}

// ---------------------------------------------------------------------------------------------
// Owner leases (AC-108, AC-109; §A3 Liveness).
// ---------------------------------------------------------------------------------------------

/// The Linux abstract-socket name for a lock path (AC-109 formula).
pub fn lock_name(lock_path: &str) -> String {
    use xxhash_rust::xxh64::xxh64;
    let b = lock_path.as_bytes();
    format!(
        "omp-file-lock-{:016x}{:016x}",
        xxh64(b, 0x4f4d_502d_4c4f_434b),
        xxh64(b, 0x5049_2d46_494c_454c)
    )
}

/// Holds an omp owner lease for `<owners>/<id>.lock` until dropped, the way omp does: an
/// abstract datagram bind on Linux, an exclusive `flock` on the created lock file elsewhere.
pub struct Held(Box<dyn std::any::Any>);

#[cfg(target_os = "linux")]
pub fn hold_lease(owners: &Path, id: &str) -> Held {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};
    let lock = owners.join(format!("{id}.lock"));
    let name = lock_name(lock.to_str().expect("UTF-8 lock path"));
    let addr = SocketAddr::from_abstract_name(name.as_bytes()).expect("abstract name");
    let sock = UnixDatagram::bind_addr(&addr).expect("bind the lease name");
    Held(Box::new(sock))
}

#[cfg(not(target_os = "linux"))]
pub fn hold_lease(owners: &Path, id: &str) -> Held {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(owners)
        .unwrap();
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(owners.join(format!("{id}.lock")))
        .unwrap();
    f.try_lock().expect("flock the lease file");
    Held(Box::new(f))
}

/// Restores a directory's mode on drop (permission tests).
pub struct RestoreMode(pub PathBuf, pub u32);

impl Drop for RestoreMode {
    fn drop(&mut self) {
        let _ = fs::set_permissions(
            &self.0,
            std::os::unix::fs::PermissionsExt::from_mode(self.1),
        );
    }
}
