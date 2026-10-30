pub mod control;
pub mod reply;
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
pub fn omp_command(job: &Job) -> Command {
    let mut c = Command::new(&job.omp_bin);
    c.args(["--mode", "rpc", "--no-ui"]);
    if let Some(p) = &job.profile_flag {
        c.args(["--profile", p])
            .env_remove("OMP_PROFILE")
            .env_remove("PI_PROFILE");
    }
    if job.route == Route::Rpc
        && let Some(file) = &job.session_file
    {
        c.arg("--resume").arg(file);
    }
    c.env("GLEN_MANAGED", "1").current_dir(&job.cwd);
    c
}
pub fn spawn_worker(home: &GlenHome, id: &JobId) -> io::Result<Child> {
    let mut opts = OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let path = home.root.join("jobs").join(id.as_str()).join("worker.log");
    let log = opts
        .open(&path)
        .map_err(|e| io::Error::new(e.kind(), format!("cannot open {}: {e}", path.display())))?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(["__worker", id.as_str()])
        .stdin(Stdio::null())
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
    cmd.spawn().map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "cannot spawn worker for {} (log {}): {e}",
                id.handle(),
                path.display()
            ),
        )
    })
}
pub fn session_handle(id: &str) -> String {
    let simple: String = id.chars().filter(|c| *c != '-').collect();
    simple
        .chars()
        .skip(simple.chars().count().saturating_sub(7))
        .collect()
}

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
            assert_eq!(c.get_program(), j.omp_bin.as_os_str());
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
