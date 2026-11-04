//! Process and timing helpers that hold under `scripts/flake-check.sh`, which repeats the suite
//! beside CPU burners. Shared by the `common` tests and the conformance suite.
#![allow(dead_code)]
use std::{
    fs::File,
    io::{Read, Seek},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::OnceLock,
    time::Duration,
};

/// `GLEN_TEST_TIME_SCALE`, a whole number from 1 that `scripts/flake-check.sh` exports for its
/// loaded runs; 1 when unset.
pub fn time_scale() -> u32 {
    static SCALE: OnceLock<u32> = OnceLock::new();
    *SCALE.get_or_init(|| match std::env::var("GLEN_TEST_TIME_SCALE") {
        Err(std::env::VarError::NotPresent) => 1,
        value => value
            .as_deref()
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|n| *n >= 1)
            .unwrap_or_else(|| {
                panic!("GLEN_TEST_TIME_SCALE must be a whole number from 1: {value:?}")
            }),
    })
}

/// A test's wait, or its upper bound on how long the product takes, times [`time_scale`]: exactly
/// `d` in a normal run, so no product bound is ever widened there. A lower bound is never scaled.
pub fn scaled(d: Duration) -> Duration {
    d * time_scale()
}

/// A child's stdout and stderr, captured in unlinked files instead of pipes. On macOS std makes a
/// pipe with `pipe()` and sets `FD_CLOEXEC` after it, so a process another test thread spawns in
/// between keeps the write end for its whole life (a `glen send --warm` passes it on to its warm
/// worker and omp, for the window). Reading a pipe to end of file, as `Command::output()` does,
/// then waits for that unrelated process. A file has no end of file to wait for: what the child
/// wrote is all there once it has exited.
pub struct Capture {
    stdout: File,
    stderr: File,
}

impl Capture {
    pub fn new() -> Self {
        Self {
            stdout: tempfile::tempfile().unwrap(),
            stderr: tempfile::tempfile().unwrap(),
        }
    }

    /// Points `cmd`'s stdout and stderr at the capture.
    pub fn attach(&self, cmd: &mut Command) {
        cmd.stdout(self.stdout.try_clone().unwrap());
        self.attach_stderr(cmd);
    }

    /// Points only `cmd`'s stderr at the capture (its stdout goes where the caller says).
    pub fn attach_stderr(&self, cmd: &mut Command) {
        cmd.stderr(self.stderr.try_clone().unwrap());
    }

    /// What the child wrote, as an `Output` with its exit `status`; read once it has exited.
    pub fn output(&self, status: ExitStatus) -> Output {
        let read = |mut f: &File| {
            f.rewind().unwrap();
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes).unwrap();
            bytes
        };
        Output {
            status,
            stdout: read(&self.stdout),
            stderr: read(&self.stderr),
        }
    }
}

/// `cmd.output()` through a [`Capture`]: it returns once the child has exited, whoever else holds
/// a copy of its output. Stdin is null, as `output()` makes it.
pub fn output(cmd: &mut Command) -> Output {
    try_output(cmd).unwrap()
}

/// [`output`] that returns a spawn or wait error instead of panicking, as `cmd.output()` does.
pub fn try_output(cmd: &mut Command) -> std::io::Result<Output> {
    let capture = Capture::new();
    capture.attach(cmd.stdin(Stdio::null()));
    let status = cmd.status()?;
    Ok(capture.output(status))
}

/// Where tests run the binaries cargo builds: `target/<profile>/glen-test-bin/`. Each cargo run
/// that builds a binary, or finds it fresh, puts it in `target/<profile>` again, and on macOS it
/// does so by removing the file there and copying the new one in (it never hard-links there,
/// rust-lang/cargo#10060). A test that runs cargo (to build a fake peer, or another test's
/// executable) so makes that path vanish for a moment, and a concurrent exec of it fails with
/// ENOENT or is killed (flake-proof run 41761493795). Cargo never touches this directory:
/// [`publish`] puts a binary here by an atomic rename, and every cargo run a test makes and every
/// publish hold [`locked`]'s lock, so no publish reads a binary that cargo is replacing.
fn bin_dir() -> PathBuf {
    profile_dir().join("glen-test-bin")
}

/// `target/<profile>`, the directory of the glen binary under test.
fn profile_dir() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_glen")).parent().unwrap()
}

/// Runs `f` holding the lock of [`bin_dir`], across processes.
fn locked<T>(f: impl FnOnce() -> T) -> T {
    let dir = bin_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".lock"))
        .unwrap();
    lock.lock().unwrap();
    let result = f();
    lock.unlock().unwrap();
    result
}

/// `cargo <args> --locked --offline` on this package, for the target dir and profile of the glen
/// binary under test.
fn cargo_command(args: &[&str]) -> Command {
    let profile = profile_dir();
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(args)
        .args(["--locked", "--offline", "--manifest-path"])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .arg("--target-dir")
        .arg(profile.parent().unwrap());
    match profile.file_name().unwrap().to_str().unwrap() {
        "debug" => {}
        "release" => {
            cmd.arg("--release");
        }
        other => {
            cmd.args(["--profile", other]);
        }
    }
    cmd
}

/// [`cargo_command`] run to its exit under [`locked`]'s lock (see [`bin_dir`]); output as
/// [`output`] gives it.
pub fn cargo(args: &[&str]) -> Output {
    locked(|| output(&mut cargo_command(args)))
}

/// `cargo build --example <name>`, then the example [`publish`]ed: the path a test runs it from.
/// Called once per test process, so the fake peers match this source.
pub fn example(name: &str) -> PathBuf {
    locked(|| {
        let out = output(&mut cargo_command(&["build", "--quiet", "--example", name]));
        assert!(
            out.status.success(),
            "cargo build --example {name} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        publish(&profile_dir().join("examples").join(name))
    })
}

/// The glen binary under test, [`publish`]ed once per process, for a suite that runs [`cargo`]
/// while it runs glen.
pub fn glen() -> &'static Path {
    static GLEN: OnceLock<PathBuf> = OnceLock::new();
    GLEN.get_or_init(|| locked(|| publish(Path::new(env!("CARGO_BIN_EXE_glen")))))
}

/// `built` at `<bin_dir>/<its name>`, by a hard link renamed into place, so the path always
/// holds a whole binary. No copy is written, so no exec finds one open for writing (ETXTBSY on
/// Linux); on macOS the link is to cargo's copy, not to the linker's output that
/// rust-lang/cargo#10060 is about. A path that already holds `built` is left alone, so on Linux
/// (where cargo hard-links) a running binary's own path does not turn `(deleted)`. Called under
/// [`locked`]'s lock.
fn publish(built: &Path) -> PathBuf {
    use std::os::unix::fs::MetadataExt;
    let name = built.file_name().unwrap().to_str().unwrap();
    let dst = bin_dir().join(name);
    let id = |p: &Path| std::fs::metadata(p).map(|m| (m.dev(), m.ino())).ok();
    let built_id = id(built);
    if built_id.is_some() && built_id == id(&dst) {
        return dst;
    }
    let tmp = bin_dir().join(format!(".{name}.tmp"));
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => panic!("remove {tmp:?}: {e}"),
        _ => {}
    }
    std::fs::hard_link(built, &tmp)
        .or_else(|_| std::fs::copy(built, &tmp).map(drop))
        .unwrap_or_else(|e| panic!("publish {built:?}: {e}"));
    std::fs::rename(&tmp, &dst).unwrap();
    dst
}

/// [`output`] and [`try_output`] as methods, for a `Command` built in a chain: `cmd.captured()`
/// in place of `cmd.output().unwrap()`.
pub trait CaptureExt {
    /// [`output`]: what the child wrote, read from files once it has exited.
    fn captured(&mut self) -> Output;
    /// [`try_output`]: the same, with a spawn error returned.
    fn try_captured(&mut self) -> std::io::Result<Output>;
}

impl CaptureExt for Command {
    fn captured(&mut self) -> Output {
        output(self)
    }
    fn try_captured(&mut self) -> std::io::Result<Output> {
        try_output(self)
    }
}

/// A spawned child whose stdout and stderr go to a [`Capture`]; it derefs to its [`Child`].
/// Stdin is left as the command sets it.
pub struct Captured {
    child: Child,
    capture: Capture,
}

impl Captured {
    pub fn spawn(cmd: &mut Command) -> std::io::Result<Self> {
        let capture = Capture::new();
        capture.attach(cmd);
        Ok(Self {
            child: cmd.spawn()?,
            capture,
        })
    }

    /// `Child::wait_with_output` that returns at the child's exit, not at end of file on a pipe:
    /// it closes the child's stdin, waits, and reads the capture.
    pub fn wait_with_output(mut self) -> Output {
        drop(self.child.stdin.take());
        let status = self.child.wait().unwrap();
        self.capture.output(status)
    }

    /// What the child wrote, once it has exited with `status` (reaped by the caller).
    pub fn output(&self, status: ExitStatus) -> Output {
        self.capture.output(status)
    }
}

impl std::ops::Deref for Captured {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}

impl std::ops::DerefMut for Captured {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}

/// Joins a thread that reads an exited child's stdout pipe to end of file, if it finishes within
/// 5 s (scaled): its result, else `None`. A process another test thread spawned can hold the
/// pipe's write end for its life (see [`Capture`]), so end of file may not come; the lines a test
/// waits for have arrived by then.
pub fn join_reader<T>(reader: std::thread::JoinHandle<T>) -> Option<T> {
    let deadline = std::time::Instant::now() + scaled(Duration::from_secs(5));
    while !reader.is_finished() {
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Some(reader.join().unwrap())
}

/// A file every write to which fails with EPIPE, for a child's stdout: the write end of a FIFO
/// whose only reader has closed. A pipe from `std::io::pipe()` with its reader dropped is not
/// enough on macOS: std sets the pipe's `FD_CLOEXEC` after `pipe()`, so a process another test
/// thread spawns in between keeps the read end for its life, and the child's writes succeed.
pub fn closed_stdout() -> File {
    let (reader, writer) = fifo();
    drop(reader);
    await_no_reader(&writer);
    writer
}

/// The read and write ends of a FIFO, both blocking, for a child's stdout that the test reads and
/// later closes. Both ends are opened with `O_CLOEXEC` and the path is removed before this
/// returns, so no process can open another reader, and one forked meanwhile holds the reader only
/// until its exec ([`await_no_reader`] waits that out).
pub fn fifo() -> (File, File) {
    use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawFd};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stdout");
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: `c_path` is a NUL-terminated path that outlives the call.
    let made = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    assert_eq!(made, 0, "mkfifo: {}", std::io::Error::last_os_error());
    let reader = File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&path)
        .unwrap();
    // A reader exists, so this blocking open returns at once.
    let writer = File::options().write(true).open(&path).unwrap();
    // SAFETY: F_GETFL and F_SETFL on a descriptor `reader` owns.
    let unblocked = unsafe {
        let flags = libc::fcntl(reader.as_raw_fd(), libc::F_GETFL);
        flags >= 0 && libc::fcntl(reader.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) == 0
    };
    assert!(unblocked, "fcntl: {}", std::io::Error::last_os_error());
    (reader, writer)
}

/// Returns once a write to `writer`, a [`fifo`]'s write end, fails with EPIPE: its reader has
/// closed, and so has every copy a process forked meanwhile held until its exec (a probe byte
/// lands only while one is left, and nothing reads it). Bounded at 10 s, scaled.
pub fn await_no_reader(writer: &File) {
    let deadline = std::time::Instant::now() + scaled(Duration::from_secs(10));
    loop {
        match std::io::Write::write(&mut &*writer, b"\0") {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return,
            Ok(_) => assert!(
                std::time::Instant::now() < deadline,
                "a reader of the FIFO outlived its holder's exec"
            ),
            Err(e) => panic!("probe the FIFO: {e}"),
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Lets `settle` pass (a window in which the caller may check that nothing happened), then waits
/// until process `pid` catches SIGINT, so a SIGINT sent next reaches its handler instead of the
/// default or inherited disposition. Linux shows the handler in `/proc/<pid>/status` (`SigCgt`).
/// macOS has no such view, so there the wait is `settle` times [`time_scale`]. A process that has
/// exited ends the wait (the caller sees its exit).
#[cfg(target_os = "linux")]
pub fn await_sigint_handler(pid: u32, settle: Duration) {
    std::thread::sleep(settle);
    let deadline = std::time::Instant::now() + scaled(Duration::from_secs(20));
    loop {
        let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
            return;
        };
        let field = |name: &str| status.lines().find_map(|l| l.strip_prefix(name));
        if field("State:").is_some_and(|s| s.trim_start().starts_with('Z')) {
            return;
        }
        let caught = field("SigCgt:")
            .and_then(|mask| u64::from_str_radix(mask.trim(), 16).ok())
            .is_some_and(|mask| mask & (1u64 << (libc::SIGINT - 1)) != 0);
        if caught {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "process {pid} never installed a SIGINT handler"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// See the Linux version: macOS cannot show a process's handlers, so this sleeps `settle` scaled.
#[cfg(target_os = "macos")]
pub fn await_sigint_handler(_pid: u32, settle: Duration) {
    std::thread::sleep(scaled(settle));
}

/// The live direct children of `parent` (zombies left out), read from the kernel: no `/bin/ps`,
/// whose spawn per poll is slow under load.
#[cfg(target_os = "linux")]
pub fn children_of(parent: u32) -> Vec<u32> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    dir.filter_map(|entry| {
        let pid: u32 = entry.ok()?.file_name().to_str()?.parse().ok()?;
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // `pid (comm) state ppid …`: comm may hold spaces and parentheses.
        let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
        let (state, ppid) = (fields.next()?, fields.next()?.parse::<u32>().ok()?);
        (ppid == parent && !matches!(state, "Z" | "X")).then_some(pid)
    })
    .collect()
}

/// The live direct children of `parent` (zombies left out), read from the kernel: no `/bin/ps`,
/// whose spawn per poll is slow under load.
#[cfg(target_os = "macos")]
pub fn children_of(parent: u32) -> Vec<u32> {
    let mut pids = [0 as libc::pid_t; 1024];
    let bytes = libc::c_int::try_from(std::mem::size_of_val(&pids)).unwrap();
    // SAFETY: `pids` is writable storage of `bytes` bytes, the most the call writes; it returns
    // the number of pids written (zombies included).
    let n =
        unsafe { libc::proc_listchildpids(parent as libc::pid_t, pids.as_mut_ptr().cast(), bytes) };
    pids[..usize::try_from(n).unwrap_or(0).min(pids.len())]
        .iter()
        .filter(|&&pid| live(pid))
        .map(|&pid| pid as u32)
        .collect()
}

/// Whether `pid` runs: it exists and is not a zombie.
#[cfg(target_os = "macos")]
fn live(pid: libc::pid_t) -> bool {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).unwrap();
    // SAFETY: `info` is writable storage of `size` bytes, the most the call writes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    // SAFETY: a return of `size` means the call filled the whole structure.
    n == size && unsafe { info.assume_init() }.pbi_status != libc::SZOMB
}
