pub mod control;
pub mod reply;
pub mod warm;
pub mod worker;
use crate::{
    env::Env,
    error::Warning,
    glen_home::GlenHome,
    jobs::{Deliver, Job, JobId, Route},
};
use std::{
    fs::OpenOptions,
    io,
    process::{Child, Command, Stdio},
    time::Duration,
};
pub const READY_TIMEOUT: Duration = Duration::from_secs(60);
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);
pub const TICK: Duration = Duration::from_millis(200);
pub const ABORT_GRACE: Duration = Duration::from_secs(10);
pub const KILL_GRACE: Duration = Duration::from_secs(1);
pub const PROMPT_GRACE: Duration = Duration::from_secs(15);
pub const DEFAULT_JOB_TIMEOUT: Duration = Duration::from_secs(3600);
pub const MAX_MESSAGE_BYTES: usize = 524_288;
pub const EVENT_ALLOWLIST: &[&str] = &[
    "agent_start",
    "agent_end",
    "turn_start",
    "turn_end",
    "message_start",
    "message_end",
    "tool_execution_start",
    "tool_execution_end",
    "auto_compaction_start",
    "auto_compaction_end",
    "manual_compaction_start",
    "manual_compaction_end",
    "auto_retry_start",
    "auto_retry_end",
    "model_changed",
    "thinking_level_changed",
    "notice",
    "queue_update",
    "goal_updated",
];
pub fn job_timeout(env: &Env) -> (Duration, Option<Warning>) {
    match env.get("GLEN_JOB_TIMEOUT") {
        None => (DEFAULT_JOB_TIMEOUT, None),
        Some(s) => match crate::cli::parse_duration(s) {
            Ok(d) if !d.is_zero() => (d, None),
            _ => (
                DEFAULT_JOB_TIMEOUT,
                Some(Warning {
                    code: "invalid_env".into(),
                    message: "GLEN_JOB_TIMEOUT must be a duration between 1ms and 24h; using 60m"
                        .into(),
                }),
            ),
        },
    }
}
pub fn streaming_behavior(d: Deliver) -> omp_rpc::StreamingBehavior {
    match d {
        Deliver::Steer => omp_rpc::StreamingBehavior::Steer,
        _ => omp_rpc::StreamingBehavior::FollowUp,
    }
}
pub(crate) fn base_command(
    bin: &std::path::Path,
    profile_flag: Option<&str>,
    cwd: &std::path::Path,
) -> Command {
    let mut command = Command::new(bin);
    command
        .args(["--mode", "rpc", "--no-ui"])
        .env("GLEN_MANAGED", "1")
        .current_dir(cwd);
    if let Some(profile) = profile_flag {
        let profile = profile.trim();
        command
            .args(["--profile", profile])
            .env_remove("OMP_PROFILE")
            .env_remove("PI_PROFILE");
    }
    command
}
/// omp loads project settings, extensions, skills and rules from its cwd with no trust gate
/// (`extensibility/extensions/types.ts:814-820` in the omp clone), so a child that only lists
/// models or probes capabilities (S-02) runs `--no-session` with discovery off
/// (`--no-extensions --no-skills --no-rules`, launch flags omp 18.14.0 parses in every mode,
/// `cli/args.ts:382-387`, and applies in `buildSessionOptions`), in a [`PrivateCwd`]. `send`
/// and resumed controls keep the session's cwd: that directory is the session's by design.
pub(crate) fn listing_command(
    bin: &std::path::Path,
    profile_flag: Option<&str>,
    cwd: &PrivateCwd,
) -> Command {
    let mut command = base_command(bin, profile_flag, &cwd.0);
    // omp falls back to $PWD when it cannot read its cwd (`getProjectDir`): never the caller's.
    command
        .args([
            "--no-session",
            "--no-extensions",
            "--no-skills",
            "--no-rules",
        ])
        .env("PWD", &cwd.0);
    command
}
/// A new, empty 0700 directory under `$TMPDIR` (else `/tmp`) for a listing or probe child
/// (S-02); removed on drop, after the child is gone. Creation fails rather than reuse a path
/// that already exists.
#[derive(Debug)]
pub(crate) struct PrivateCwd(std::path::PathBuf);
impl PrivateCwd {
    pub(crate) fn new(env: &Env) -> io::Result<Self> {
        let temp = env
            .non_empty("TMPDIR")
            .or_else(|| env.non_empty("TMP"))
            .or_else(|| env.non_empty("TEMP"))
            .map(std::path::Path::new)
            .filter(|p| p.is_absolute())
            .unwrap_or(std::path::Path::new("/tmp"));
        let path = temp.join(format!("glen-omp-{}", uuid::Uuid::now_v7().simple()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "cannot create a private directory for omp at {}: {e}",
                    path.display()
                ),
            )
        })?;
        Ok(Self(path))
    }
}
impl Drop for PrivateCwd {
    fn drop(&mut self) {
        // Best effort: a leftover empty directory under $TMPDIR is harmless.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub fn omp_command(job: &Job) -> Command {
    let mut command = base_command(
        job.omp_bin.as_deref().unwrap_or(std::path::Path::new("")),
        job.profile_flag.as_deref(),
        &job.cwd,
    );
    if let Some(t) = &job.thinking_flag {
        command.args(["--thinking", t]);
    }
    if job.route == Route::Rpc
        && let Some(file) = &job.session_file
    {
        command.arg("--resume").arg(file);
    }
    command
}
/// A recorded pid glen may signal or probe (S-30): it fits `pid_t` and is above 1, so a
/// corrupt or hostile record can never turn into `kill(0, _)` (the caller's group), `kill(-1, _)`
/// (every process) or init.
pub(crate) fn signal_pid(pid: u32) -> Option<rustix::process::Pid> {
    i32::try_from(pid)
        .ok()
        .filter(|raw| *raw > 1)
        .and_then(rustix::process::Pid::from_raw)
}
pub(crate) fn kill_group(pid: u32, signal: rustix::process::Signal) -> io::Result<()> {
    let Some(pid) = signal_pid(pid) else {
        return Err(io::Error::other(format!("invalid omp pid {pid}")));
    };
    match rustix::process::kill_process_group(pid, signal) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(e) => Err(io::Error::new(
            io::Error::from(e).kind(),
            format!("cannot signal omp process group {pid}: {e}"),
        )),
    }
}
/// The canonical job bytes `send` hands its worker on stdin (S-13): `job.json`'s own encoding.
pub fn job_bytes(job: &Job) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(job)?;
    bytes.push(b'\n');
    Ok(bytes)
}
/// Spawns `__worker <id>` with `job` on its stdin (S-13): the worker never reads its job from
/// `job.json`, which a writer to GLEN_HOME can rewrite after `send` created it. The bytes are
/// written from a thread that `send` waits for at most [`STDIN_WRITE_BOUND`] (the worker reads
/// stdin first thing), so a worker that never reads cannot hold `send`; a worker that exits
/// first only costs the thread an EPIPE, ignored.
pub fn spawn_worker(home: &GlenHome, id: &JobId, job: &Job) -> io::Result<Child> {
    let bytes = job_bytes(job)?;
    let mut opts = OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // S-12: the worker's stdout and stderr never follow a planted symlink.
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let path = home.root.join("jobs").join(id.as_str()).join("worker.log");
    let log = opts
        .open(&path)
        .map_err(|e| io::Error::new(e.kind(), format!("cannot open {}: {e}", path.display())))?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(["__worker", id.as_str()])
        .stdin(Stdio::piped())
        .stderr(log.try_clone()?)
        .stdout(log)
        .current_dir("/")
        .env("GLEN_HOME", &home.root);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and allocation-free in the forked child.
        unsafe {
            cmd.pre_exec(|| rustix::process::setsid().map(drop).map_err(io::Error::from));
        }
    }
    let mut child = cmd.spawn().map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "cannot spawn worker for {} (log {}): {e}",
                id.handle(),
                path.display()
            ),
        )
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        let (done, written) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::Write;
            // EPIPE: the worker exited before reading; it fails on its own missing job.
            let _ = stdin.write_all(&bytes);
            drop(stdin);
            let _ = done.send(());
        });
        // `send --no-wait` exits right after this, and the thread with it: wait for the write,
        // bounded, since the worker reads its stdin first thing.
        let _ = written.recv_timeout(STDIN_WRITE_BOUND);
    }
    Ok(child)
}
/// The longest `send` waits for its worker to take the job from stdin.
const STDIN_WRITE_BOUND: Duration = Duration::from_secs(5);
/// The last 7 non-hyphen characters of a plain id (`lease_name`'s rule: ASCII letters, digits,
/// `_` and `-`), as omp's UUIDs always are. Any other id is untrusted text that could carry
/// terminal escapes (S-25), so it is never shown: its handle is `h` and 6 hex digits of its
/// hash, which no UUID tail matches.
pub fn session_handle(id: &str) -> String {
    if crate::omp::lease::lease_name(id).is_none() {
        return format!(
            "h{:06x}",
            xxhash_rust::xxh64::xxh64(id.as_bytes(), 0) & 0xff_ffff
        );
    }
    let simple: String = id.chars().filter(|c| *c != '-').collect();
    simple
        .chars()
        .skip(simple.chars().count().saturating_sub(7))
        .collect()
}

// These calls must not hold the polling thread (or submission gate) for CALL_TIMEOUT.
macro_rules! polling_command {
    ($name:ident, $inner:ty, $timeout:expr) => {
        #[derive(serde::Serialize)]
        #[serde(transparent)]
        struct $name($inner);
        impl omp_rpc::Command for $name {
            const NAME: &'static str = <$inner as omp_rpc::Command>::NAME;
            const TIMEOUT_MS: Option<u64> = Some($timeout);
            type Output = <$inner as omp_rpc::Command>::Output;
            fn decode(data: Option<serde_json::Value>) -> Result<Self::Output, serde_json::Error> {
                <$inner as omp_rpc::Command>::decode(data)
            }
        }
    };
}
pub(crate) use polling_command;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn omp_command_resume_new_and_profiles() {
        for (route, profile) in [
            (Route::Rpc, None),
            (Route::New, None),
            (Route::Rpc, Some("work")),
            (Route::New, Some("default")),
        ] {
            let mut j = crate::jobs::tests::job();
            j.route = route;
            j.profile_flag = profile.map(str::to_owned);
            let c = omp_command(&j);
            assert_eq!(c.get_program(), j.omp_bin.as_ref().unwrap().as_os_str());
            assert_eq!(c.get_current_dir(), Some(j.cwd.as_path()));
            let args: Vec<_> = c
                .get_args()
                .map(|s| s.to_string_lossy().into_owned())
                .collect();
            let mut expected = vec!["--mode".to_owned(), "rpc".into(), "--no-ui".into()];
            if let Some(p) = profile {
                expected.extend(["--profile".into(), p.into()]);
            }
            if route == Route::Rpc {
                expected.extend(["--resume".into(), "/fixture/session.jsonl".into()]);
            }
            assert_eq!(args, expected);
            let vars: Vec<_> = c.get_envs().collect();
            assert!(vars.contains(&(
                std::ffi::OsStr::new("GLEN_MANAGED"),
                Some(std::ffi::OsStr::new("1"))
            )));
            for key in ["OMP_PROFILE", "PI_PROFILE"] {
                assert_eq!(
                    vars.contains(&(std::ffi::OsStr::new(key), None)),
                    profile.is_some()
                );
            }
        }
    }
    /// S-12: a symlink planted at `worker.log` fails the spawn; its target is never written.
    #[cfg(unix)]
    #[test]
    fn s12_the_worker_log_never_follows_a_symlink() {
        let t = tempfile::tempdir().unwrap();
        let home = GlenHome {
            root: t.path().join("glen"),
        };
        let id = JobId::new();
        let dir = home.ensure_dir(&format!("jobs/{}", id.as_str())).unwrap();
        let victim = t.path().join("victim.jsonl");
        std::fs::write(&victim, b"kept\n").unwrap();
        std::os::unix::fs::symlink(&victim, dir.join("worker.log")).unwrap();
        let e = spawn_worker(&home, &id, &crate::jobs::tests::job()).unwrap_err();
        assert!(e.to_string().contains("worker.log"), "{e}");
        assert_eq!(std::fs::read(&victim).unwrap(), b"kept\n");
    }
    #[test]
    fn s30_only_a_pid_above_one_that_fits_pid_t_is_signalled() {
        for pid in [0, 1, 1 << 31, u32::MAX] {
            assert!(signal_pid(pid).is_none(), "{pid}");
        }
        // A wrapped pid is refused before any signal (SIGCONT, so a regression stays harmless:
        // `pid as i32` once turned u32::MAX into a signal for init).
        for pid in [0, 1 << 31, u32::MAX] {
            let error = kill_group(pid, rustix::process::Signal::CONT).unwrap_err();
            assert!(error.to_string().contains("invalid omp pid"), "{error}");
        }
        assert_eq!(signal_pid(2).map(|p| p.as_raw_nonzero().get()), Some(2));
        let me = std::process::id();
        assert_eq!(
            signal_pid(me).map(|p| p.as_raw_nonzero().get() as u32),
            Some(me)
        );
    }
    #[test]
    fn s25_a_non_plain_session_id_never_reaches_a_handle() {
        assert_eq!(
            session_handle("01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de"),
            "e27c0de"
        );
        assert_eq!(session_handle("abc"), "abc");
        assert_eq!(session_handle("a_b-cdefghij"), "defghij");
        for hostile in [
            "aaaa-zz\x1b[2Jabc",
            "x\u{202e}gpj",
            "tag\u{e0069}",
            "a.b",
            "",
            &"a".repeat(129),
        ] {
            let h = session_handle(hostile);
            assert_eq!(h.len(), 7, "{hostile:?}: {h}");
            assert!(h.starts_with('h'), "{h}");
            assert!(h[1..].bytes().all(|b| b.is_ascii_hexdigit()), "{h}");
            assert_eq!(h, session_handle(hostile), "stable");
        }
        assert_ne!(session_handle("a.b"), session_handle("a.c"));
    }
    #[test]
    fn timeout_env_table() {
        for (raw, expected, warn) in [
            (None, DEFAULT_JOB_TIMEOUT, false),
            (Some("30s"), Duration::from_secs(30), false),
            (Some("1ms"), Duration::from_millis(1), false),
            (Some("24h"), Duration::from_secs(86400), false),
            (Some("0"), DEFAULT_JOB_TIMEOUT, true),
            (Some("abc"), DEFAULT_JOB_TIMEOUT, true),
            (Some("25h"), DEFAULT_JOB_TIMEOUT, true),
        ] {
            let vars: Vec<_> = raw.map(|v| ("GLEN_JOB_TIMEOUT", v)).into_iter().collect();
            let env = Env::for_test(Some(Path::new("/h")), Path::new("/w"), &vars);
            let (d, w) = job_timeout(&env);
            assert_eq!(d, expected);
            assert_eq!(w.is_some(), warn);
            if let Some(w) = w {
                assert_eq!(w.code, "invalid_env");
            }
        }
    }
    #[test]
    fn streaming_and_filter_contract() {
        for (d, s) in [
            (Deliver::Auto, omp_rpc::StreamingBehavior::FollowUp),
            (Deliver::Steer, omp_rpc::StreamingBehavior::Steer),
            (Deliver::FollowUp, omp_rpc::StreamingBehavior::FollowUp),
        ] {
            assert_eq!(streaming_behavior(d), s);
        }
        for ev in ["message_end", "agent_end", "notice"] {
            assert!(EVENT_ALLOWLIST.contains(&ev));
        }
        for ev in [
            "message_update",
            "tool_execution_update",
            "tool_stream_update",
        ] {
            assert!(!EVENT_ALLOWLIST.contains(&ev));
        }
    }
}
