//! Shared harness for the W3 live-route conformance cases (AC-301…AC-327), derived from
//! `.claude/plans/v010-wave-3-live.md` (acceptance criteria and orchestrator decisions O1–O8)
//! and `docs/design.md` §A3–§A11, §A15 without reading the implementation (`src/live/`,
//! `src/extension.rs`, `src/inbox.rs`, `src/ops.rs`, `extension/glen/index.ts`).
//!
//! `LiveBox` is an `RpcBox` whose GLEN_HOME lives under a short `/tmp` path (unix socket paths
//! stay far below the `sun_path` limit; macOS `$TMPDIR` alone is ~50 bytes). It starts two kinds
//! of endpoint:
//! - `Ext`: the plan's `fake_ext` cargo example (`tests/support/fake_ext.rs`), a separate process
//!   with a real pid, scripted through `FAKE_EXT_SCENARIO` and logging frames to `FAKE_EXT_LOG`;
//! - `Peer`: an in-test unix-socket server (pid = this test process) for protocol answers the
//!   fake has no scenario for. Its registry is written here, from the §A3 field list.
//!
//! Nothing touches a live `~/.omp`, a real omp or the real extension.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::ops::Deref;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustix::process::Signal;
use serde_json::{Value, json};

use crate::rpc_support::*;
use crate::support::*;

/// `sun_path` capacity counted in bytes (AC-321): a socket path of this many bytes or more is
/// refused.
#[cfg(target_os = "macos")]
pub const SUN_LIMIT: usize = 104;
#[cfg(not(target_os = "macos"))]
pub const SUN_LIMIT: usize = 108;

/// The reserved W3b ops (AC-307, §A3).
pub const RESERVED_OPS: [&str; 5] = [
    "abort",
    "list_models",
    "set_model",
    "set_thinking_level",
    "compact",
];

/// The W3 socket ops (AC-307/308).
pub const OPS: [&str; 3] = ["hello", "state", "send"];

/// `id` with its last four hex digits replaced by `f0f0` (the fake's moved identity).
pub fn moved_id(id: &str) -> String {
    format!("{}f0f0", &id[..id.len() - 4])
}

/// Hint text without markdown backticks (the plan quotes hints with inline code spans).
pub fn plain(v: &Value) -> String {
    v.as_str().unwrap_or("").replace('`', "")
}

/// Warning codes of an envelope or of `status.json` (`[{code,message}]` or bare strings).
pub fn codes_of(warnings: &Value) -> Vec<String> {
    warnings
        .as_array()
        .map(|w| {
            w.iter()
                .filter_map(|x| x["code"].as_str().or(x.as_str()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// The fake extension peer.
// ---------------------------------------------------------------------------------------------

/// The `fake_ext` example next to the glen binary under test, (re)built once per test process.
pub fn fake_ext_bin() -> &'static Path {
    static EXT: OnceLock<PathBuf> = OnceLock::new();
    EXT.get_or_init(|| {
        let exe = Path::new(BIN);
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
            "fake_ext",
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
        let out = cmd.output().expect("run cargo build --example fake_ext");
        assert!(
            out.status.success(),
            "cargo build --example fake_ext failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let bin = profile_dir.join("examples/fake_ext");
        assert!(bin.is_file(), "no fake extension at {}", bin.display());
        bin
    })
}

/// A running `fake_ext` process (killed on drop).
pub struct Ext {
    child: Option<Child>,
    pub pid: u32,
    pub session_id: String,
    pub log: PathBuf,
    pub registry: PathBuf,
    pub socket: PathBuf,
}

impl Ext {
    /// Frames the fake logged in direction `dir` (`"in"` = glen → extension).
    pub fn frames(&self, dir: &str) -> Vec<Value> {
        records(&self.log)
            .into_iter()
            .filter(|r| r["dir"] == dir)
            .map(|r| r["frame"].clone())
            .collect()
    }

    /// Inbound request ops, in order.
    pub fn ops_in(&self) -> Vec<String> {
        self.frames("in")
            .iter()
            .map(|f| f["op"].as_str().unwrap_or("?").to_owned())
            .collect()
    }

    pub fn alive(&mut self) -> bool {
        match &mut self.child {
            Some(c) => c.try_wait().ok().flatten().is_none(),
            None => false,
        }
    }

    pub fn signal(&self, sig: Signal) {
        signal(self.pid as i64, sig);
    }

    /// SIGTERM and wait for exit (the fake removes its files unless `FAKE_EXT_KEEP_FILES=1`).
    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            self.signal(Signal::TERM);
            let deadline = Instant::now() + LONG;
            while c.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Ext {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The sandbox.
// ---------------------------------------------------------------------------------------------

pub struct LiveBox {
    pub rb: RpcBox,
    _short: tempfile::TempDir,
    ext_seq: Mutex<u32>,
}

impl Deref for LiveBox {
    type Target = RpcBox;
    fn deref(&self) -> &RpcBox {
        &self.rb
    }
}

impl LiveBox {
    pub fn new() -> Self {
        fake_ext_bin();
        let short = tempfile::Builder::new()
            .prefix("gc")
            .tempdir_in("/tmp")
            .expect("short tempdir under /tmp");
        let mut rb = RpcBox::new();
        rb.bx.glen_home = short.path().join("g");
        fs::create_dir_all(rb.bx.root.join("extlogs")).unwrap();
        LiveBox {
            rb,
            _short: short,
            ext_seq: Mutex::new(0),
        }
    }

    pub fn glen_home(&self) -> &Path {
        &self.rb.bx.glen_home
    }

    pub fn run_live(&self) -> PathBuf {
        self.glen_home().join("run/live")
    }

    /// `run/live/<lock_key(id)>.json`; a plain-token id is its own key (§A6).
    pub fn registry_path(&self, session_id: &str) -> PathBuf {
        self.run_live().join(format!("{session_id}.json"))
    }

    pub fn inbox_file(&self, session_id: &str) -> PathBuf {
        self.glen_home().join(format!("inbox/{session_id}.ndjson"))
    }

    /// Start `fake_ext` for `session_id` with `scenario`; `env` adds `FAKE_EXT_*` knobs and
    /// `args` extra command-line flags (`--session-file`, `--mode`). Waits for `ready`.
    pub fn start_ext(
        &self,
        session_id: &str,
        scenario: &str,
        env: &[(&str, &str)],
        args: &[&str],
    ) -> Ext {
        let n = {
            let mut s = self.ext_seq.lock().unwrap();
            *s += 1;
            *s
        };
        let log = self
            .rb
            .bx
            .root
            .join("extlogs")
            .join(format!("ext{n}.ndjson"));
        let mut cmd = Command::new(fake_ext_bin());
        cmd.env_clear()
            .arg("--home")
            .arg(self.glen_home())
            .arg("--session-id")
            .arg(session_id)
            .args(args)
            .env("FAKE_EXT_SCENARIO", scenario)
            .env("FAKE_EXT_LOG", &log)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn fake_ext");
        let pid = child.id();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let ready = rx.recv_timeout(Duration::from_secs(10)).unwrap_or_default();
        assert_eq!(ready.trim(), "ready", "fake_ext did not report ready");
        Ext {
            child: Some(child),
            pid,
            session_id: session_id.to_owned(),
            log,
            registry: self.registry_path(session_id),
            socket: self.run_live().join(format!("{pid}.sock")),
        }
    }

    /// `glen send <session> <text> --json` plus `extra`, run to completion.
    pub fn send(&self, session: &str, text: &str, extra: &[&str]) -> Run {
        let mut args = vec!["send", session, text];
        args.extend_from_slice(extra);
        args.push("--json");
        self.rb.glen(args)
    }

    /// The fake omp was never started by any run of this box.
    pub fn assert_no_omp(&self) {
        assert!(
            self.rb.starts().is_empty(),
            "an omp process was started\n{}",
            self.rb.diag()
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Registries and sockets written by the test.
// ---------------------------------------------------------------------------------------------

pub fn mkdir_private(p: &Path) {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(p)
        .unwrap();
}

/// The §A3 registry document.
pub fn registry_doc(session_id: &str, pid: u32, socket: &Path) -> Value {
    json!({
        "v": 1,
        "pid": pid,
        "session_id": session_id,
        "session_file": null,
        "socket": socket,
        "ext_version": PKG_VERSION,
        "omp_version": "18.14.0",
        "protocol": 1,
        "mode": "tui",
        "started_at": jiff::Timestamp::now().to_string(),
        "cwd": null,
    })
}

/// Write `run/live/<id>.json` (0600, dirs 0700) with `doc`.
pub fn write_registry_doc(lb: &LiveBox, session_id: &str, doc: &Value) -> PathBuf {
    mkdir_private(&lb.run_live());
    let p = lb.registry_path(session_id);
    write_private(&p, &serde_json::to_vec(doc).unwrap());
    p
}

/// A socket file nobody listens on (bind, then drop the listener): connecting is refused.
pub fn stale_socket(path: &Path) {
    let l = UnixListener::bind(path).expect("bind a throwaway socket");
    drop(l);
    assert!(
        path.exists(),
        "the socket file stays after the listener is dropped"
    );
}

// ---------------------------------------------------------------------------------------------
// The in-test peer.
// ---------------------------------------------------------------------------------------------

/// `hello` data per AC-308 (`state` drops `protocol`, `protocols` and `ops`).
pub fn hello_data(session_id: &str) -> Value {
    json!({
        "protocol": 1,
        "protocols": [1],
        "ops": OPS,
        "ext_version": PKG_VERSION,
        "omp_version": "18.14.0",
        "pid": std::process::id(),
        "session_id": session_id,
        "session_file": null,
        "mode": "tui",
        "state": "idle",
        "busy": false,
        "queued": false,
        "active_requests": 0,
    })
}

pub fn write_frame(s: &mut UnixStream, v: &Value) {
    let _ = s.write_all(format!("{v}\n").as_bytes());
    let _ = s.flush();
}

/// What the peer does with one connection: `(connection index, parsed request, stream)`.
pub type Behave = dyn Fn(usize, &Value, &mut UnixStream) + Send + Sync + 'static;

/// An in-process endpoint: registry pid = this test process, socket `run/live/<pid>.sock`.
pub struct Peer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub socket: PathBuf,
    pub registry: PathBuf,
    /// Every request line received, parsed (`null` when not JSON).
    pub requests: Arc<Mutex<Vec<Value>>>,
}

impl Peer {
    pub fn start(
        lb: &LiveBox,
        session_id: &str,
        behave: impl Fn(usize, &Value, &mut UnixStream) + Send + Sync + 'static,
    ) -> Peer {
        mkdir_private(&lb.run_live());
        let socket = lb.run_live().join(format!("{}.sock", std::process::id()));
        let listener = UnixListener::bind(&socket).expect("bind the peer socket");
        set_mode(&socket, 0o600);
        listener.set_nonblocking(true).unwrap();
        let registry = write_registry_doc(
            lb,
            session_id,
            &registry_doc(session_id, std::process::id(), &socket),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let behave: Arc<Behave> = Arc::new(behave);
        let (st, rq) = (Arc::clone(&stop), Arc::clone(&requests));
        let thread = std::thread::spawn(move || {
            let mut n = 0usize;
            let mut conns = Vec::new();
            while !st.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut s, _)) => {
                        let idx = n;
                        n += 1;
                        let (b, rq) = (Arc::clone(&behave), Arc::clone(&rq));
                        conns.push(std::thread::spawn(move || {
                            let _ = s.set_nonblocking(false);
                            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                            let mut line = Vec::new();
                            let _ = BufReader::new(s.try_clone().unwrap())
                                .take(1 << 20)
                                .read_until(b'\n', &mut line);
                            let req: Value = serde_json::from_slice(&line).unwrap_or(Value::Null);
                            rq.lock().unwrap().push(req.clone());
                            b(idx, &req, &mut s);
                        }));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
            for c in conns {
                let _ = c.join();
            }
        });
        Peer {
            stop,
            thread: Some(thread),
            socket,
            registry,
            requests,
        }
    }

    pub fn ops_in(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r["op"].as_str().unwrap_or("?").to_owned())
            .collect()
    }

    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The standard unary answers: `hello`/`state` per AC-308 for `session_id`.
pub fn answer_unary(session_id: &str, req: &Value, s: &mut UnixStream) -> bool {
    let id = req["id"].clone();
    match req["op"].as_str() {
        Some("hello") => {
            write_frame(
                s,
                &json!({"v":1,"id":id,"ok":true,"data":hello_data(session_id)}),
            );
            true
        }
        Some("state") => {
            let mut d = hello_data(session_id);
            for k in ["protocol", "protocols", "ops"] {
                d.as_object_mut().unwrap().remove(k);
            }
            write_frame(s, &json!({"v":1,"id":id,"ok":true,"data":d}));
            true
        }
        _ => false,
    }
}

/// A §A5 reply record as the extension streams it.
pub fn reply_record(session_id: &str, req: &Value, text: &str, fin: bool) -> Value {
    json!({
        "v": 1,
        "id": uuid::Uuid::now_v7().to_string(),
        "session_id": session_id,
        "job_id": req["args"]["job_id"],
        "request_id": req["id"],
        "tool_call_id": "call_peer",
        "text": text,
        "final": fin,
        "at": jiff::Timestamp::now().to_string(),
    })
}

// ---------------------------------------------------------------------------------------------
// The inbox (§A5, §A6).
// ---------------------------------------------------------------------------------------------

/// A §A5 inbox record with every key present.
pub fn inbox_record(session_id: &str, job_id: Option<&str>, text: &str, at: &str) -> Value {
    json!({
        "v": 1,
        "id": uuid::Uuid::now_v7().to_string(),
        "session_id": session_id,
        "job_id": job_id,
        "request_id": job_id.map(|_| uuid::Uuid::now_v7().to_string()),
        "tool_call_id": "call_1",
        "text": text,
        "final": false,
        "at": at,
    })
}

/// Append raw bytes to a file (created 0600 under a 0700 dir).
pub fn append_bytes(path: &Path, bytes: &[u8]) {
    mkdir_private(path.parent().unwrap());
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    set_mode(path, 0o600);
    f.write_all(bytes).unwrap();
    f.flush().unwrap();
}

pub fn append_record(path: &Path, rec: &Value) {
    append_bytes(path, format!("{rec}\n").as_bytes());
}
