//! v0.2 W4 warm worker (AC-v2-401…AC-v2-407, design §A21): the Implementer's own tests, on the
//! real binary with `fake_omp` (never a live omp). Every test ends warmth (release, expiry, a
//! signal) and a drop guard kills any survivor, so no worker outlives its box.
#![cfg(unix)]
mod common;
use common::*;
use glen::{
    glen_home::GlenHome,
    jobs::{
        liveness::{self, Probe},
        lock,
    },
    rpc::warm,
};
use serde_json::Value;
use std::{
    process::{Command, Output},
    time::{Duration, Instant},
};

/// An `RpcBox` whose drop kills every warm worker and omp group it left behind.
struct WarmBox {
    b: RpcBox,
    extra: Vec<(String, String)>,
}
impl std::ops::Deref for WarmBox {
    type Target = RpcBox;
    fn deref(&self) -> &RpcBox {
        &self.b
    }
}
impl Drop for WarmBox {
    fn drop(&mut self) {
        let (entries, _) = warm::index(&self.home());
        for e in entries {
            for pid in [Some(e.entry.pid), e.entry.omp_pid].into_iter().flatten() {
                if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::CONT);
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
                }
            }
            if let Some(pid) = e
                .entry
                .omp_pid
                .and_then(|p| rustix::process::Pid::from_raw(p as i32))
            {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
        }
    }
}
impl WarmBox {
    /// Every command of the box shares the fake's knobs, so the env digest matches (a knob
    /// changed between two sends makes the second run cold, AC-v2-404).
    fn new() -> Self {
        Self::with(&[])
    }
    fn with(vars: &[(&str, &str)]) -> Self {
        let mut extra = vec![("FAKE_OMP_MULTI".to_owned(), "1".to_owned())];
        extra.extend(vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())));
        Self {
            b: RpcBox::new(),
            extra,
        }
    }
    fn home(&self) -> GlenHome {
        GlenHome {
            root: self.b.omp.glen_home.clone(),
        }
    }
    /// The box's glen command, with the fake's lease held in the box's owners directory so a
    /// warm omp visibly keeps the session.
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = self.b.glen(args);
        c.env("FAKE_OMP_OWNERS_DIR", self.b.owners());
        c.envs(self.extra.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }
    fn send(&self, extra: &[&str]) -> Value {
        let mut args = vec!["send", "e27c0de", "hi", "--json"];
        args.extend(extra);
        data(&self.run(&args))
    }
    /// `send e27c0de <text> --no-wait --json`: exit 10 while the job runs; its `data`.
    fn no_wait(&self, args: &[&str]) -> Value {
        let mut all = vec!["send", "e27c0de"];
        all.extend(args);
        all.extend(["--no-wait", "--json"]);
        let out = self.run(&all);
        assert_eq!(out.status.code(), Some(10), "{out:?}");
        json(&out)["data"].clone()
    }
    fn registry(&self) -> Option<warm::Indexed> {
        warm::read(&self.home(), A_ID)
    }
    fn wait_registry(&self) -> warm::Indexed {
        poll(Duration::from_secs(10), || {
            self.registry()
                .filter(|r| r.entry.state == warm::EntryState::Idle)
        })
    }
    fn holder(&self) -> Option<lock::Holder> {
        lock::holder(&self.home(), A_ID)
    }
    fn held(&self) -> bool {
        lock::is_held(&self.home(), A_ID)
    }
    fn log(&self, job: &str) -> String {
        std::fs::read_to_string(self.b.job_dir(job).join("worker.log")).unwrap_or_default()
    }
    fn starts(&self) -> usize {
        self.b
            .fake_log()
            .iter()
            .filter(|v| v["event"] == "start")
            .count()
    }
    /// Waits until warmth is over: registry gone, lock free, worker and omp group gone.
    fn wait_cold(&self, entry: &warm::Entry, within: Duration) -> Duration {
        let started = Instant::now();
        poll(within, || {
            (self.registry().is_none()
                && !self.held()
                && gone(entry.pid)
                && entry.omp_pid.is_none_or(group_gone))
            .then_some(())
        });
        started.elapsed()
    }
}
fn gone(pid: u32) -> bool {
    matches!(liveness::start_token(pid), Probe::Gone)
}
fn group_gone(pid: u32) -> bool {
    rustix::process::Pid::from_raw(pid as i32).is_some_and(|pid| {
        rustix::process::test_kill_process_group(pid) == Err(rustix::io::Errno::SRCH)
    })
}
fn job_id(doc: &Value) -> String {
    doc["job"]["id"]
        .as_str()
        .or_else(|| doc["id"].as_str())
        .unwrap()
        .to_owned()
}
#[test]
fn ac_v2_403_a_warm_worker_holds_the_session_then_expires() {
    let b = WarmBox::new();
    let first = b.send(&["--warm", "1s"]);
    assert_eq!(first["job"]["status"], "completed", "{first}");
    let id = job_id(&first);
    let (job, _) = b.job(&id);
    assert_eq!(job["warm_ms"], 1000);
    let reg = b.wait_registry();
    assert_eq!(reg.entry.job_id, id);
    assert!(b.held());
    let holder = b.holder().unwrap();
    assert_eq!(holder.kind, lock::HolderKind::Warm);
    assert_eq!(holder.pid, reg.entry.pid);
    let omp = reg.entry.omp_pid.unwrap();
    assert!(!group_gone(omp), "omp stays alive while warm");
    b.wait_cold(&reg.entry, Duration::from_secs(6));
    assert!(
        b.log(&id).contains("warm: cooling (expired)"),
        "{}",
        b.log(&id)
    );
    assert!(!warm::release_path(&b.home(), A_ID).exists());
    assert_eq!(b.starts(), 1);
}

#[test]
fn ac_v2_402_a_second_send_is_stolen_and_runs_on_the_same_omp() {
    let b = WarmBox::new();
    let first = b.send(&["--warm", "2s"]);
    let one = job_id(&first);
    let reg = b.wait_registry();
    let queued = b.no_wait(&["again", "--warm", "2s"]);
    let sent = Instant::now();
    let two = job_id(&queued);
    let claimed = poll(Duration::from_secs(5), || {
        let s = b.job(&two).1;
        (s["omp_reused"] == true).then_some(s)
    });
    assert!(
        sent.elapsed() < Duration::from_millis(500),
        "{:?}",
        sent.elapsed()
    );
    assert_eq!(claimed["worker_pid"], reg.entry.pid);
    assert_eq!(claimed["worker_start"], reg.entry.start.clone().unwrap());
    let done = data(&b.run(&["wait", &two, "--json"]));
    assert_eq!(done["job"]["status"], "completed", "{done}");
    assert_eq!(done["job"]["omp_reused"], true);
    assert_eq!(done["assistant_text"], "pong: again");
    assert_eq!(b.starts(), 1);
    assert_eq!(b.frames_in("prompt").len(), 2);
    let reg = b.wait_registry();
    assert_eq!(reg.entry.job_id, two);
    b.wait_cold(&reg.entry, Duration::from_secs(8));
    assert!(b.log(&one).contains("warm: cooling (expired)"));
    assert!(b.log(&two).contains("warm: adopted"), "{}", b.log(&two));
}
