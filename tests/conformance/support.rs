//! Shared harness for the W0 conformance suite.
//!
//! Every run goes through the real `glen` binary with a cleared environment: `HOME` is a fresh
//! temp dir, `GLEN_HOME` points at a path that does not exist yet, `PATH` is an empty dir and the
//! working directory is an empty temp dir. Env is set per `Command`, never process-wide.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

pub const BIN: &str = env!("CARGO_BIN_EXE_glen");
pub const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const TESTED_OMP: &str = "18.14.0";

/// The one command still hidden after W3 and W4 (AC-301, AC-401).
pub const HIDDEN: [&str; 1] = ["watch"];

/// Doctor check names, in the order docs/design.md §A10 lists them.
pub const DOCTOR_CHECKS: [&str; 12] = [
    "omp_binary",
    "omp_version",
    "profile",
    "agent_dir",
    "sessions_dir",
    "glen_home",
    "extension",
    "live_endpoints",
    "omp_env_files",
    "skill_claude",
    "skill_codex",
    "sandbox",
];

const RUN_TIMEOUT: Duration = Duration::from_secs(60);

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn fixtures() -> PathBuf {
    repo_root().join("tests/fixtures")
}

pub fn fake_omp() -> PathBuf {
    fixtures().join("bin/fake-omp")
}

pub fn fixture_agent_dir() -> PathBuf {
    fixtures().join("omp/agent")
}

pub fn s(p: &Path) -> String {
    p.to_str().expect("test paths are UTF-8").to_owned()
}

pub fn mode(p: &Path) -> u32 {
    fs::metadata(p)
        .unwrap_or_else(|e| panic!("stat {}: {e}", p.display()))
        .permissions()
        .mode()
        & 0o777
}

pub fn set_mode(p: &Path, m: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
}

/// Write an executable `/bin/sh` script.
pub fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    set_mode(&path, 0o755);
    path
}

/// True if the directory is missing or has no entries.
pub fn is_empty_dir(p: &Path) -> bool {
    match fs::read_dir(p) {
        Ok(mut it) => it.next().is_none(),
        Err(_) => true,
    }
}

pub fn list_tree(root: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            out.push(s(p.strip_prefix(base).unwrap()));
            if p.is_dir() {
                walk(base, &p, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// An isolated filesystem for one test.
pub struct Sandbox {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    /// Default `GLEN_HOME`; does not exist until glen creates it.
    pub glen_home: PathBuf,
    pub empty_bin: PathBuf,
    pub cwd: PathBuf,
}

impl Sandbox {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("glen-conf-")
            .tempdir()
            .expect("tempdir");
        let root = dir.path().to_path_buf();
        let home = root.join("home");
        let empty_bin = root.join("emptybin");
        let cwd = root.join("work");
        for d in [&home, &empty_bin, &cwd] {
            fs::create_dir(d).unwrap();
        }
        Sandbox {
            _dir: dir,
            glen_home: root.join("glen"),
            root,
            home,
            empty_bin,
            cwd,
        }
    }

    /// Make a fresh directory under the sandbox root (not under HOME).
    pub fn dir(&self, rel: &str) -> PathBuf {
        let p = self.root.join(rel);
        fs::create_dir_all(&p).unwrap();
        p
    }

    pub fn glen<I, S>(&self, args: I) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
        let mut cmd = Command::new(BIN);
        cmd.env_clear()
            .args(&args)
            .env("HOME", &self.home)
            .env("GLEN_HOME", &self.glen_home)
            .env("PATH", &self.empty_bin)
            .current_dir(&self.cwd)
            .stdin(Stdio::null());
        let desc = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        Run {
            cmd,
            desc,
            env_desc: Vec::new(),
            stdout: None,
        }
    }
}

pub struct Run {
    cmd: Command,
    desc: String,
    env_desc: Vec<String>,
    stdout: Option<Stdio>,
}

impl Run {
    /// Wrap a prepared `Command` whose environment the caller has already set (W1 `OmpBox`).
    pub fn from_command(cmd: Command, desc: String) -> Self {
        Run {
            cmd,
            desc,
            env_desc: Vec::new(),
            stdout: None,
        }
    }

    pub fn env(mut self, k: impl AsRef<OsStr>, v: impl AsRef<OsStr>) -> Self {
        self.env_desc.push(format!(
            "{}={:?}",
            k.as_ref().to_string_lossy(),
            v.as_ref().to_string_lossy()
        ));
        self.cmd.env(k, v);
        self
    }

    pub fn env_remove(mut self, k: impl AsRef<OsStr>) -> Self {
        self.env_desc
            .push(format!("unset {}", k.as_ref().to_string_lossy()));
        self.cmd.env_remove(k);
        self
    }

    /// `GLEN_OMP_BIN` = the fixture fake omp (reports 18.14.0 unless `FAKE_OMP_VERSION` is set).
    pub fn fake_omp(self) -> Self {
        self.env("GLEN_OMP_BIN", fake_omp())
    }

    pub fn stdout(mut self, s: Stdio) -> Self {
        self.stdout = Some(s);
        self
    }

    pub fn run(mut self) -> Out {
        self.cmd
            .stdout(self.stdout.take().unwrap_or_else(Stdio::piped))
            .stderr(Stdio::piped());
        let started = Instant::now();
        let child = self
            .cmd
            .spawn()
            .unwrap_or_else(|e| panic!("spawn glen {}: {e}", self.desc));
        let pid = child.id();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        let output = match rx.recv_timeout(RUN_TIMEOUT) {
            Ok(r) => r.unwrap_or_else(|e| panic!("wait glen {}: {e}", self.desc)),
            Err(_) => {
                let _ = Command::new("/bin/kill")
                    .args(["-9", &pid.to_string()])
                    .status();
                panic!("`glen {}` did not finish within {RUN_TIMEOUT:?}", self.desc);
            }
        };
        Out {
            desc: format!("glen {}  [{}]", self.desc, self.env_desc.join(" ")),
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
            elapsed: started.elapsed(),
        }
    }
}

pub struct Out {
    pub desc: String,
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
}

impl Out {
    pub fn ctx(&self) -> String {
        format!(
            "`{}` exited {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.desc,
            self.status,
            String::from_utf8_lossy(&self.stdout),
            String::from_utf8_lossy(&self.stderr)
        )
    }

    pub fn code(&self) -> i32 {
        self.status
            .code()
            .unwrap_or_else(|| panic!("killed by a signal\n{}", self.ctx()))
    }

    pub fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn err(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    pub fn assert_exit(&self, expected: i32) -> &Self {
        assert_eq!(
            self.code(),
            expected,
            "unexpected exit code\n{}",
            self.ctx()
        );
        self
    }

    pub fn assert_stderr_empty(&self) -> &Self {
        assert!(
            self.stderr.is_empty(),
            "stderr must be empty in JSON mode\n{}",
            self.ctx()
        );
        self
    }

    pub fn assert_no_ansi(&self) -> &Self {
        assert!(!has_esc(&self.stdout), "ESC byte on stdout\n{}", self.ctx());
        assert!(!has_esc(&self.stderr), "ESC byte on stderr\n{}", self.ctx());
        self
    }

    pub fn assert_no_panic(&self) -> &Self {
        assert!(
            !self.err().contains("panicked") && self.status.code() != Some(101),
            "glen panicked\n{}",
            self.ctx()
        );
        self
    }

    /// stdout holds exactly one JSON document (plus trailing whitespace).
    pub fn json(&self) -> Value {
        match json_docs(&self.stdout) {
            Ok(mut docs) if docs.len() == 1 => docs.remove(0),
            Ok(docs) => panic!(
                "expected exactly one JSON document on stdout, got {}\n{}",
                docs.len(),
                self.ctx()
            ),
            Err(e) => panic!("stdout is not a JSON document ({e})\n{}", self.ctx()),
        }
    }

    pub fn is_json(&self) -> bool {
        matches!(json_docs(&self.stdout), Ok(d) if !d.is_empty())
    }

    /// A `{"v":1,"ok":true,"data":…}` envelope with exit 0. Returns the envelope.
    pub fn ok_envelope(&self) -> Value {
        let env = self.json();
        assert_eq!(env["v"], Value::from(1), "envelope v\n{}", self.ctx());
        assert_eq!(env["ok"], Value::Bool(true), "envelope ok\n{}", self.ctx());
        self.assert_exit(0);
        let obj = env.as_object().unwrap();
        for k in obj.keys() {
            assert!(
                ["v", "ok", "data", "warnings", "spilled"].contains(&k.as_str()),
                "unexpected top-level key {k:?}\n{}",
                self.ctx()
            );
        }
        if let Some(w) = obj.get("warnings") {
            let arr = w.as_array().expect("warnings is an array");
            assert!(
                !arr.is_empty(),
                "warnings present but empty\n{}",
                self.ctx()
            );
            for item in arr {
                assert!(item["code"].is_string() && item["message"].is_string());
            }
        }
        env
    }

    /// The `data` of a successful, non-spilled envelope.
    pub fn ok_data(&self) -> Value {
        let env = self.ok_envelope();
        assert!(
            env["data"].is_object(),
            "envelope has no data object\n{}",
            self.ctx()
        );
        env["data"].clone()
    }

    /// An error envelope with `code`, whose exit code matches docs/design.md §A11 (AC-010).
    /// Returns the `error` object.
    pub fn error_envelope(&self, code: &str) -> Value {
        let env = self.json();
        assert_eq!(env["v"], Value::from(1), "envelope v\n{}", self.ctx());
        assert_eq!(env["ok"], Value::Bool(false), "envelope ok\n{}", self.ctx());
        for k in env.as_object().unwrap().keys() {
            assert!(
                ["v", "ok", "error", "warnings"].contains(&k.as_str()),
                "unexpected top-level key {k:?}\n{}",
                self.ctx()
            );
        }
        let err = &env["error"];
        assert!(err.is_object(), "error is not an object\n{}", self.ctx());
        for k in err.as_object().unwrap().keys() {
            assert!(
                ["code", "message", "hint", "details"].contains(&k.as_str()),
                "unexpected error key {k:?}\n{}",
                self.ctx()
            );
        }
        assert_eq!(err["code"], Value::from(code), "error.code\n{}", self.ctx());
        let msg = err["message"]
            .as_str()
            .unwrap_or_else(|| panic!("error.message is not a string\n{}", self.ctx()));
        assert!(
            !msg.trim().is_empty(),
            "empty error.message\n{}",
            self.ctx()
        );
        if let Some(h) = err.get("hint") {
            assert!(h.is_string(), "error.hint is not a string\n{}", self.ctx());
        }
        self.assert_exit(exit_for(code));
        err.clone()
    }
}

pub fn has_esc(b: &[u8]) -> bool {
    b.contains(&0x1b)
}

pub fn json_docs(bytes: &[u8]) -> Result<Vec<Value>, String> {
    let mut docs = Vec::new();
    let mut it = serde_json::Deserializer::from_slice(bytes).into_iter::<Value>();
    for d in it.by_ref() {
        docs.push(d.map_err(|e| e.to_string())?);
    }
    if docs.is_empty() {
        return Err("no JSON document".into());
    }
    Ok(docs)
}

/// Remove ANSI escape sequences (CSI, OSC and two-byte escapes).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Find a doctor check by name in `data.checks`.
pub fn check<'a>(data: &'a Value, name: &str) -> &'a Value {
    data["checks"]
        .as_array()
        .unwrap_or_else(|| panic!("checks is not an array: {data}"))
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no check named {name}: {data}"))
}

/// Exit code for an error code, parsed from the §A11 table in docs/design.md (AC-010).
pub fn exit_for(code: &str) -> i32 {
    *design_exit_table()
        .get(code)
        .unwrap_or_else(|| panic!("code {code:?} is not in docs/design.md §A11"))
}

pub fn design_exit_table() -> &'static BTreeMap<String, i32> {
    static TABLE: OnceLock<BTreeMap<String, i32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let doc = fs::read_to_string(repo_root().join("docs/design.md"))
            .expect("docs/design.md exists (AC-024)");
        let mut in_section = false;
        let mut table = BTreeMap::new();
        for line in doc.lines() {
            if line.starts_with("## ") {
                in_section = line.starts_with("## §A11");
                continue;
            }
            if !in_section || !line.starts_with('|') {
                continue;
            }
            let cols: Vec<&str> = line.split('|').map(str::trim).collect();
            // ["", exit, codes, meaning, ""]
            let Some(Ok(exit)) = cols.get(1).map(|c| c.parse::<i32>()) else {
                continue;
            };
            let codes = cols.get(2).copied().unwrap_or("");
            for (i, part) in codes.split('`').enumerate() {
                if i % 2 == 1 && !part.is_empty() {
                    table.insert(part.to_owned(), exit);
                }
            }
        }
        assert!(
            !table.is_empty(),
            "could not parse the §A11 exit-code table in docs/design.md"
        );
        table
    })
}

/// Lexically normalise an absolute path (resolve `.` and `..` without touching the filesystem).
pub fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::from("/");
    for c in p.components() {
        match c {
            std::path::Component::RootDir | std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(n) => out.push(n),
            std::path::Component::Prefix(_) => unreachable!(),
        }
    }
    out
}
