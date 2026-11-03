//! Process and timing helpers that hold under `scripts/flake-check.sh`, which repeats the suite
//! beside CPU burners. Shared by the `common` tests and the conformance suite.
#![allow(dead_code)]
use std::{
    fs::File,
    io::{Read, Seek},
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

/// A file every write to which fails with EPIPE, for a child's stdout: the write end of a FIFO
/// whose only reader has closed. A pipe from `std::io::pipe()` with its reader dropped is not
/// enough on macOS: std sets the pipe's `FD_CLOEXEC` after `pipe()`, so a process another test
/// thread spawns in between keeps the read end for its life, and the child's writes succeed. Both
/// FIFO ends are opened with `O_CLOEXEC`, so a process forked meanwhile holds the reader only until
/// its exec; this returns once a probe write fails with EPIPE, the FIFO's path already removed, so
/// from then on no process holds or can open a reader.
pub fn closed_stdout() -> File {
    use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
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
    drop(reader);
    drop(dir);
    // A probe byte lands only while a forked child still holds the reader (it never reads it).
    let deadline = std::time::Instant::now() + scaled(Duration::from_secs(10));
    loop {
        match std::io::Write::write(&mut &writer, b"\0") {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return writer,
            Ok(_) => assert!(
                std::time::Instant::now() < deadline,
                "a reader of the closed stdout outlived its holder's exec"
            ),
            Err(e) => panic!("probe the closed stdout: {e}"),
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
