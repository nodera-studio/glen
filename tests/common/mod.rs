#![allow(dead_code)]
use std::{
    path::PathBuf,
    process::{Command, Output},
};
pub fn glen(args: &[&str], vars: &[(&str, &str)]) -> Output {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let path = tmp.path().join("bin");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&path).unwrap();
    Command::new(env!("CARGO_BIN_EXE_glen"))
        .args(args)
        .env_clear()
        .env("HOME", &home)
        .env("GLEN_HOME", tmp.path().join("glen"))
        .env("PATH", path)
        .envs(vars.iter().copied())
        .output()
        .unwrap()
}
pub fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path)
}
pub fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON: {e}; stdout {:?}, stderr {:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

pub struct OmpBox {
    pub tmp: tempfile::TempDir,
    pub home: PathBuf,
    pub agent: PathBuf,
    pub tmpdir: PathBuf,
    pub glen_home: PathBuf,
}
impl OmpBox {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let agent = root.join("agent");
        let tmpdir = root.join("tmpdir");
        let glen_home = root.join("glen");
        for p in [
            home.join("work/alpha"),
            home.join("work/beta"),
            agent.clone(),
            tmpdir.clone(),
        ] {
            std::fs::create_dir_all(p).unwrap();
        }
        copy_tree(&fixture("omp/agent"), &agent);
        let sb = Self {
            tmp,
            home,
            agent,
            tmpdir,
            glen_home,
        };
        let now = std::time::SystemTime::now();
        for (k, seconds) in [
            ('G', 60),
            ('E', 120),
            ('D', 180),
            ('C', 240),
            ('B', 300),
            ('A', 360),
            ('F', 30 * 86400),
        ] {
            std::fs::File::open(sb.session(k))
                .unwrap()
                .set_modified(now - std::time::Duration::from_secs(seconds))
                .unwrap();
        }
        sb
    }
    pub fn alpha(&self) -> PathBuf {
        self.home.join("work/alpha")
    }
    pub fn beta(&self) -> PathBuf {
        self.home.join("work/beta")
    }
    pub fn session(&self, key: char) -> PathBuf {
        let tail = match key {
            'A' => "e27c0de",
            'B' => "f13a5b6",
            'C' => "b7e2d81",
            'D' => "c4f9a72",
            'E' => "0a3f417",
            'F' => "a81c0de",
            'G' => "b8e4a07",
            _ => panic!("unknown fixture"),
        };
        for bucket in ["-work-alpha", "-work-beta"] {
            for e in std::fs::read_dir(self.agent.join("sessions").join(bucket)).unwrap() {
                let e = e.unwrap();
                if e.file_name()
                    .to_string_lossy()
                    .ends_with(&format!("{tail}.jsonl"))
                {
                    return e.path();
                }
            }
        }
        panic!("fixture missing")
    }
    pub fn command(&self, args: &[&str], cwd: &std::path::Path) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_glen"));
        c.args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("GLEN_HOME", &self.glen_home)
            .env("PI_CODING_AGENT_DIR", &self.agent)
            .env("TMPDIR", &self.tmpdir)
            .env("TZ", "UTC")
            .env("PATH", "")
            .current_dir(cwd);
        c
    }
    pub fn glen(&self, args: &[&str], cwd: &std::path::Path) -> Output {
        self.command(args, cwd).output().unwrap()
    }
    pub fn clean(&self) {
        assert!(!self.home.join(".omp").exists());
        assert!(!self.glen_home.exists());
    }
}
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            std::fs::create_dir(&target).unwrap();
            copy_tree(&e.path(), &target)
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}
pub fn data(out: &Output) -> serde_json::Value {
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    assert!(!out.stdout.contains(&27));
    let v = json(out);
    assert_eq!(v["ok"], true);
    v["data"].clone()
}

pub fn fake_omp_bin() -> PathBuf {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let bin = PathBuf::from(env!("CARGO_BIN_EXE_glen"));
            let dir = bin.parent().unwrap();
            let fake = dir.join("examples/fake_omp");
            // Rebuild once per integration-test process so fake-peer knobs match this source.
            let mut cmd = Command::new(env!("CARGO"));
            cmd.current_dir(env!("CARGO_MANIFEST_DIR")).args([
                "build",
                "--example",
                "fake_omp",
                "--offline",
                "--locked",
            ]);
            if dir.file_name().unwrap() == "release" {
                cmd.arg("--release");
            }
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            fake
        })
        .clone()
}
pub struct RpcBox {
    pub omp: OmpBox,
    pub fake: PathBuf,
    pub log: PathBuf,
}
impl RpcBox {
    pub fn new() -> Self {
        let omp = OmpBox::new();
        let fake = fake_omp_bin();
        let log = omp.tmp.path().join("fake.ndjson");
        Self { omp, fake, log }
    }
    pub fn glen(&self, args: &[&str]) -> Command {
        let mut c = self.omp.command(args, &self.omp.alpha());
        c.env("GLEN_OMP_BIN", &self.fake)
            .env("FAKE_OMP_LOG", &self.log)
            .env("GLEN_JOB_TIMEOUT", "30s");
        c
    }
    pub fn fake_log(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|s| serde_json::from_str(s).ok())
            .collect()
    }
    pub fn frames_in(&self, kind: &str) -> Vec<serde_json::Value> {
        self.fake_log()
            .into_iter()
            .filter(|v| v["event"] == "in" && v["frame"]["type"] == kind)
            .map(|v| v["frame"].clone())
            .collect()
    }
    pub fn job_dir(&self, id: &str) -> PathBuf {
        self.omp.glen_home.join("jobs").join(id)
    }
    pub fn job(&self, id: &str) -> (serde_json::Value, serde_json::Value) {
        let dir = self.job_dir(id);
        // The directory becomes visible before create() has atomically published both files.
        poll(std::time::Duration::from_secs(3), || {
            let read = |name| match std::fs::read(dir.join(name)) {
                Ok(bytes) => Some(serde_json::from_slice(&bytes).unwrap()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => panic!("cannot read {name}: {e}"),
            };
            Some((read("job.json")?, read("status.json")?))
        })
    }
    pub fn wait_terminal(&self, id: &str, timeout: std::time::Duration) -> serde_json::Value {
        poll(timeout, || {
            let s = self.job(id).1;
            matches!(
                s["status"].as_str(),
                Some("completed" | "failed" | "aborted" | "lost")
            )
            .then_some(s)
        })
    }
    pub fn wait_worker_gone(&self, id: &str) {
        let s = self.wait_terminal(id, std::time::Duration::from_secs(25));
        poll(std::time::Duration::from_secs(10), || {
            let dead = s["worker_pid"].as_u64().is_none_or(|pid| {
                matches!(
                    glen::jobs::liveness::start_token(pid as u32),
                    glen::jobs::liveness::Probe::Gone
                )
            });
            let gone = s["omp_pid"].as_u64().is_none_or(|pid| {
                rustix::process::Pid::from_raw(pid as i32).is_some_and(|pid| {
                    rustix::process::test_kill_process_group(pid) == Err(rustix::io::Errno::SRCH)
                })
            });
            (dead && gone).then_some(())
        });
    }
    pub fn hold_glen_lock(&self, id: &str) -> glen::jobs::lock::SessionLock {
        glen::jobs::lock::try_acquire(
            &glen::glen_home::GlenHome {
                root: self.omp.glen_home.clone(),
            },
            id,
        )
        .unwrap()
        .unwrap()
    }
    pub fn owners(&self) -> PathBuf {
        self.omp.home.join(".omp/run/session-owners")
    }
    pub fn hold_lease(&self, id: &str) -> std::process::Child {
        use std::io::BufRead;
        std::fs::create_dir_all(self.owners()).unwrap();
        let mut child = Command::new(&self.fake)
            .arg("hold-lease")
            .arg(self.owners().join(format!("{id}.lock")))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line, "held\n");
        child
    }
}
pub fn poll<T>(timeout: std::time::Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let start = std::time::Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(
            start.elapsed() < timeout,
            "poll timed out after {timeout:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
pub const A_ID: &str = "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de";

pub fn fake_ext_bin() -> PathBuf {
    static BIN: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BIN.get_or_init(|| {
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_glen"));
        let dir = bin.parent().unwrap();
        let mut c = Command::new(env!("CARGO"));
        c.current_dir(env!("CARGO_MANIFEST_DIR")).args([
            "build",
            "--example",
            "fake_ext",
            "--offline",
            "--locked",
        ]);
        if dir.file_name().unwrap() == "release" {
            c.arg("--release");
        }
        let out = c.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        dir.join("examples/fake_ext")
    })
    .clone()
}
pub struct LiveBox {
    pub rpc: RpcBox,
    ext: Vec<std::process::Child>,
    pub log: PathBuf,
    _short: tempfile::TempDir,
}
impl LiveBox {
    pub fn new() -> Self {
        let mut rpc = RpcBox::new();
        let short = tempfile::Builder::new()
            .prefix("gle-")
            .tempdir_in("/tmp")
            .unwrap();
        rpc.omp.glen_home = short.path().join("g");
        let log = short.path().join("ext.ndjson");
        Self {
            rpc,
            ext: vec![],
            log,
            _short: short,
        }
    }
    pub fn start_ext(&mut self, id: &str, scenario: &str, vars: &[(&str, &str)]) -> u32 {
        use std::io::BufRead;
        let mut c = Command::new(fake_ext_bin());
        c.args([
            "--home",
            self.rpc.omp.glen_home.to_str().unwrap(),
            "--session-id",
            id,
        ])
        .env_clear()
        .env("FAKE_EXT_SCENARIO", scenario)
        .env("FAKE_EXT_LOG", &self.log)
        .env("FAKE_EXT_CWD", self.rpc.omp.alpha())
        .envs(vars.iter().copied())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
        if id == A_ID {
            c.arg("--session-file").arg(self.rpc.omp.session('A'));
        }
        let mut child = c.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = std::io::BufReader::new(stdout)
                .read_line(&mut line)
                .map(|_| line);
            let _ = tx.send(result);
        });
        let ready = rx.recv_timeout(std::time::Duration::from_secs(10));
        if !matches!(&ready,Ok(Ok(line)) if line=="ready\n") {
            let _ = child.kill();
            let out = child.wait_with_output().unwrap();
            panic!(
                "fake_ext not ready: {ready:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let pid = child.id();
        self.ext.push(child);
        pid
    }
    pub fn registry(&self, id: &str) -> PathBuf {
        glen::live::registry::path(
            &glen::glen_home::GlenHome {
                root: self.rpc.omp.glen_home.clone(),
            },
            id,
        )
    }
    pub fn kill_ext(&mut self, signal: i32) {
        for child in &mut self.ext {
            if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
                let sig = rustix::process::Signal::from_named_raw(signal).unwrap();
                let _ = rustix::process::kill_process(pid, sig);
            }
            let _ = child.wait();
        }
        self.ext.clear();
    }
    pub fn ext_log(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|s| serde_json::from_str(s).ok())
            .collect()
    }
    pub fn write_registry(&self, id: &str, pid: u32, socket: &std::path::Path) {
        let r = serde_json::json!({"v":1,"pid":pid,"session_id":id,"socket":socket,"session_file":self.rpc.omp.session('A'),"protocol":1,"mode":"tui","started_at":"2026-10-30T00:00:00Z"});
        let path = self.registry(id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, r.to_string()).unwrap();
    }
}
impl Drop for LiveBox {
    fn drop(&mut self) {
        for c in &mut self.ext {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
pub fn dead_pid() -> u32 {
    let mut c = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

/// Only backticked error codes count; statuses in successful rows are prose.
pub fn exit_code_errors(text: &str, column: usize) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut expected: BTreeMap<u8, BTreeSet<&str>> = BTreeMap::new();
    for exit in [0, 10, 11, 130] {
        expected.insert(exit, BTreeSet::new());
    }
    for code in glen::error::ErrorCode::ALL {
        expected
            .entry(code.exit_code())
            .or_default()
            .insert(code.as_str());
    }
    let rows: BTreeMap<u8, BTreeSet<&str>> = text
        .lines()
        .filter_map(|line| {
            let cells: Vec<_> = line.split('|').map(str::trim).collect();
            let exit = cells.get(1)?.parse().ok()?;
            let codes = cells
                .get(column)?
                .split('`')
                .enumerate()
                .filter(|(i, _)| i % 2 == 1)
                .map(|(_, s)| s)
                .collect();
            Some((exit, codes))
        })
        .collect();
    expected
        .iter()
        .filter(|(exit, want)| rows.get(exit) != Some(*want))
        .map(|(exit, want)| format!("exit {exit}: expected {want:?}, found {:?}", rows.get(exit)))
        .chain(
            rows.keys()
                .filter(|exit| !expected.contains_key(exit))
                .map(|exit| format!("unknown exit {exit}")),
        )
        .collect()
}

#[cfg(unix)]
pub fn hold_lease_from_env() {
    use std::{
        io::{Read, Write},
        path::PathBuf,
    };
    let Some(path) = std::env::var_os("GLEN_TEST_HOLD_LOCK") else {
        return;
    };
    let path = PathBuf::from(path);
    #[cfg(not(target_os = "linux"))]
    let _held = {
        let f = std::fs::File::options()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        f.lock().unwrap();
        f
    };
    #[cfg(target_os = "linux")]
    let _held = {
        use std::os::linux::net::SocketAddrExt;
        let name = glen::omp::lease::memory_lock_name(path.to_str().unwrap());
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name).unwrap();
        std::os::unix::net::UnixDatagram::bind_addr(&addr).unwrap()
    };
    println!("held");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
}
#[cfg(unix)]
pub struct Holder(std::process::Child);
#[cfg(unix)]
impl Holder {
    pub fn new(path: &std::path::Path) -> Self {
        use std::{
            io::{BufRead, BufReader},
            process::Stdio,
            sync::mpsc,
            thread,
            time::Duration,
        };
        let mut c = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lease_holder", "--ignored", "--nocapture"])
            .env_clear()
            .env("GLEN_TEST_HOLD_LOCK", path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = c.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut ready = Some(tx);
            for l in BufReader::new(out).lines() {
                if l.unwrap() == "held"
                    && let Some(tx) = ready.take()
                {
                    tx.send(()).unwrap();
                }
            }
        });
        let holder = Self(c);
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        holder
    }
    pub fn release(&mut self) {
        use std::{
            thread,
            time::{Duration, Instant},
        };
        drop(self.0.stdin.take());
        let start = Instant::now();
        loop {
            if let Some(s) = self.0.try_wait().unwrap() {
                assert!(s.success());
                return;
            }
            assert!(start.elapsed() < Duration::from_secs(3));
            thread::sleep(Duration::from_millis(20));
        }
    }
}
#[cfg(unix)]
impl Drop for Holder {
    fn drop(&mut self) {
        use std::{
            thread,
            time::{Duration, Instant},
        };
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            let start = Instant::now();
            while self.0.try_wait().unwrap().is_none() {
                assert!(start.elapsed() < Duration::from_secs(3));
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

pub fn fake_models() -> Vec<serde_json::Value> {
    serde_json::from_str(include_str!("../support/fake_models.json")).unwrap()
}
pub fn session_lines(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
pub fn hold_glen_lock_as(
    home: &glen::glen_home::GlenHome,
    id: &str,
    holder: Option<glen::jobs::lock::Holder>,
) -> glen::jobs::lock::SessionLock {
    let lock = glen::jobs::lock::try_acquire(home, id).unwrap().unwrap();
    if let Some(holder) = holder {
        glen::jobs::lock::mark(&lock, &holder).unwrap();
    }
    lock
}
