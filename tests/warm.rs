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

fn check<'a>(doctor: &'a Value, name: &str) -> &'a Value {
    doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()
}
fn ls_row(b: &WarmBox) -> Value {
    let ls = data(&b.run(&["ls", "--json"]));
    ls["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == A_ID)
        .unwrap()
        .clone()
}

#[test]
fn ac_v2_406_abort_keeps_an_idle_warm_session_and_ls_and_doctor_show_it() {
    let b = WarmBox::new();
    b.send(&["--warm", "30s"]);
    let reg = b.wait_registry();
    let abort = data(&b.run(&["abort", "e27c0de", "--json"]));
    assert_eq!(abort["was"], "idle", "{abort}");
    assert_eq!(abort["aborted"], false);
    assert!(b.registry().is_some() && b.held() && !group_gone(reg.entry.omp_pid.unwrap()));
    let row = ls_row(&b);
    let detail = row["state_detail"].as_str().unwrap();
    assert!(
        detail.starts_with("warm worker, idle, ") && detail.ends_with("s left"),
        "{detail}"
    );
    let human = b.run(&["ls", "--human"]);
    assert!(
        String::from_utf8_lossy(&human.stdout).contains("live (warm)"),
        "{human:?}"
    );
    let doctor = json(&b.run(&["doctor", "--json"]));
    let names: Vec<_> = doctor["data"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    let at = names.iter().position(|n| *n == "warm_workers").unwrap();
    assert_eq!(names[at - 1], "live_endpoints");
    assert_eq!(names.len(), 14);
    let row = check(&doctor["data"], "warm_workers");
    assert_eq!(row["status"], "info");
    assert_eq!(row["value"], "1 warm, 0 stale, 0 dead");
    let released = data(&b.run(&["release", "e27c0de", "--json"]));
    assert_eq!(released["total"], 1);
    assert_eq!(released["workers"][0]["outcome"], "released");
    assert_eq!(released["workers"][0]["session"]["handle"], "e27c0de");
    assert!(b.registry().is_none() && !b.held());
    b.wait_cold(&reg.entry, Duration::from_secs(4));
    assert!(b.log(&reg.entry.job_id).contains("warm: cooling (release)"));
    let none = data(&b.run(&["release", "--all", "--json"]));
    assert_eq!(none["total"], 0);
    let missing = b.run(&["release", "fffffff", "--json"]);
    assert_eq!(missing.status.code(), Some(3), "{missing:?}");
    assert_eq!(json(&missing)["error"]["details"]["reason"], "not_warm");
}

#[test]
fn ac_v2_406_controls_cool_an_idle_warm_session_then_run_as_today() {
    for args in [
        vec!["thinking", "e27c0de", "high", "--json"],
        vec!["models", "e27c0de", "--json"],
        vec!["compact", "e27c0de", "--json"],
    ] {
        let b = WarmBox::new();
        b.send(&["--warm", "30s"]);
        let reg = b.wait_registry();
        let out = b.run(&args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        b.wait_cold(&reg.entry, Duration::from_secs(4));
        assert_eq!(b.starts(), 2, "{args:?}: the control's own omp");
        assert!(b.log(&reg.entry.job_id).contains("warm: cooling (release)"));
    }
}

#[test]
fn ac_v2_405_a_stopped_worker_is_stale_and_release_force_kills_it() {
    let b = WarmBox::with(&[("GLEN_WARM_STALE_MS", "500")]);
    b.send(&["--warm", "60s"]);
    let reg = b.wait_registry();
    let pid = rustix::process::Pid::from_raw(reg.entry.pid as i32).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::STOP).unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    let doctor = data(&b.run(&["doctor", "--json"]));
    let row = check(&doctor, "warm_workers");
    assert_eq!(row["status"], "warn", "{row}");
    assert!(row["detail"].as_str().unwrap().contains("e27c0de"), "{row}");
    let detail = ls_row(&b)["state_detail"].as_str().unwrap().to_owned();
    assert!(
        detail.starts_with("warm worker, no heartbeat for "),
        "{detail}"
    );
    let out = b.run(&["release", "e27c0de", "--timeout", "1s", "--json"]);
    assert_eq!(out.status.code(), Some(7), "{out:?}");
    let e = json(&out);
    assert_eq!(e["error"]["code"], "control_failed");
    assert_eq!(e["error"]["details"]["reason"], "release_timeout");
    assert!(e["error"]["hint"].as_str().unwrap().contains("--force"));
    let forced = data(&b.run(&["release", "e27c0de", "--force", "--json"]));
    assert_eq!(forced["workers"][0]["outcome"], "forced", "{forced}");
    assert_eq!(forced["workers"][0]["state"], "stale");
    b.wait_cold(&reg.entry, Duration::from_secs(4));
}

#[test]
fn ac_v2_405_a_mismatched_registry_is_cleaned_and_never_signalled() {
    let b = WarmBox::new();
    let mut sleeper = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    let entry = warm::Entry {
        v: 1,
        pid: sleeper.id(),
        start: Some("not-its-start".into()),
        session_id: A_ID.into(),
        session_handle: None,
        job_id: "01a18bcc-6970-7685-8f03-0ecc16977273".into(),
        omp_pid: None,
        omp_start: None,
        warm_ms: 1000,
        expires_at: "2099-01-01T00:00:00Z".into(),
        state: warm::EntryState::Idle,
        started_at: "2026-11-02T00:00:00Z".into(),
    };
    std::fs::create_dir_all(&b.omp.glen_home).unwrap();
    warm::publish(&b.home(), &entry).unwrap();
    let out = data(&b.run(&["release", "--all", "--force", "--json"]));
    assert_eq!(out["workers"][0]["outcome"], "cleaned", "{out}");
    assert_eq!(out["workers"][0]["state"], "dead");
    assert!(b.registry().is_none());
    assert!(
        sleeper.try_wait().unwrap().is_none(),
        "the pid was never signalled"
    );
    sleeper.kill().unwrap();
    sleeper.wait().unwrap();
}
