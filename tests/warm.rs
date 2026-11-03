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
        for e in warm::index(&self.home()).entries {
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
    /// The box's one warm registry, whatever its session.
    fn registry(&self) -> Option<warm::Indexed> {
        warm::index(&self.home()).entries.into_iter().next()
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
                && !lock::is_held(&self.home(), &entry.session_id)
                && gone(entry.pid)
                && entry.omp_pid.is_none_or(group_gone))
            .then_some(())
        });
        started.elapsed()
    }
    /// [`Self::wait_cold`] for the worker of job `id`, by the pids in its status, for a window
    /// that may have ended before any registry was read.
    fn wait_job_cold(&self, id: &str, within: Duration) {
        let s = self.job(id).1;
        let pid = s["worker_pid"].as_u64().unwrap() as u32;
        let omp = s["omp_pid"].as_u64().map(|p| p as u32);
        poll(within, || {
            (self.registry().is_none() && !self.held() && gone(pid) && omp.is_none_or(group_gone))
                .then_some(())
        });
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
/// `settled_at` of a job's `result.json`.
fn settled_at(b: &WarmBox, id: &str) -> jiff::Timestamp {
    let result: Value =
        serde_json::from_slice(&std::fs::read(b.job_dir(id).join("result.json")).unwrap()).unwrap();
    result["settled_at"].as_str().unwrap().parse().unwrap()
}
/// Milliseconds from a job's settlement to the end of the registry's window.
fn window_ms(b: &WarmBox, id: &str, reg: &warm::Indexed) -> i64 {
    let expires: jiff::Timestamp = reg.entry.expires_at.parse().unwrap();
    expires.as_millisecond() - settled_at(b, id).as_millisecond()
}

#[test]
fn ac_v2_403_a_warm_worker_holds_the_session_then_expires() {
    let b = WarmBox::new();
    // The hold, on a window no process start-up can outlast; the registry is observed first.
    let first = b.send(&["--warm", "60s"]);
    assert_eq!(first["job"]["status"], "completed", "{first}");
    let id = job_id(&first);
    let reg = b.wait_registry();
    assert_eq!(reg.entry.job_id, id);
    assert!(b.held());
    // The registry is published just before the holder flips to `warm`.
    let holder = poll(Duration::from_secs(2), || {
        b.holder().filter(|h| h.kind == lock::HolderKind::Warm)
    });
    assert_eq!(holder.pid, reg.entry.pid);
    let omp = reg.entry.omp_pid.unwrap();
    assert!(!group_gone(omp), "omp stays alive while warm");
    let (job, _) = b.job(&id);
    assert_eq!(job["warm_ms"], 60_000);
    assert!(
        window_ms(&b, &id, &reg) >= 60_000,
        "{}",
        reg.entry.expires_at
    );
    release_all(&b);
    // Expiry, on a 1 s window nothing has to catch while it is open: the worker ends on its own.
    let second = b.send(&["--warm", "1s"]);
    assert_eq!(second["job"]["status"], "completed", "{second}");
    let two = job_id(&second);
    poll(Duration::from_secs(10), || {
        b.log(&two)
            .contains("warm: cooling (expired)")
            .then_some(())
    });
    b.wait_job_cold(&two, Duration::from_secs(6));
    assert!(!warm::release_path(&b.home(), A_ID).exists());
    assert_eq!(b.starts(), 2, "one omp per warm worker");
}

#[test]
fn ac_v2_402_a_second_send_is_stolen_and_runs_on_the_same_omp() {
    let b = WarmBox::new();
    // Windows no process start-up can outlast: the second send always finds the worker warm.
    let first = b.send(&["--warm", "30s"]);
    let one = job_id(&first);
    let reg = b.wait_registry();
    let queued = b.no_wait(&["again", "--warm", "30s"]);
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
    // The window re-arms after the adopted job, with that job's own duration.
    assert!(
        window_ms(&b, &two, &reg) >= 30_000,
        "{}",
        reg.entry.expires_at
    );
    release_all(&b);
    assert!(b.log(&one).contains("warm: cooling (release)"));
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

/// `pred` once it holds, or `None` once `child` exited without it.
fn until_or_exit<T>(child: &mut std::process::Child, pred: impl Fn() -> Option<T>) -> Option<T> {
    poll(Duration::from_secs(10), || match pred() {
        Some(v) => Some(Some(v)),
        None => child.try_wait().unwrap().map(|_| None),
    })
}

#[test]
fn ac_v2_406_models_asks_the_job_a_cooling_warm_worker_adopted() {
    use glen::jobs::{Deliver, Job, JobId, JobStore, Route, State};
    let b = WarmBox::new();
    let home = b.home();
    // This test process plays an idle warm worker: it holds the session, marked `warm`.
    let me = liveness::current().unwrap();
    let held = lock::try_acquire(&home, A_ID).unwrap().unwrap();
    let warm_holder = lock::Holder {
        v: 1,
        kind: lock::HolderKind::Warm,
        job_id: Some(JobId::new().as_str().into()),
        pid: me.pid,
        start: Some(me.start.clone()),
    };
    lock::mark(&held, &warm_holder).unwrap();
    let mut models = b
        .cmd(&["models", "e27c0de", "--timeout", "10s", "--json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let ident = warm::ident(me.pid, Some(&me.start));
    let asked = until_or_exit(&mut models, || {
        warm::release_requested(&home, A_ID, &ident).then_some(())
    });
    assert!(
        asked.is_some(),
        "models never asked the warm worker to cool"
    );
    // Before it cools, the worker adopts a queued job, the way a claim does: the job's status
    // names this worker first, then the session's holder says `job`.
    let id = JobId::new();
    let store = JobStore::new(home.clone());
    let now = jiff::Timestamp::now().to_string();
    store
        .create(&Job {
            v: 1,
            id: id.as_str().into(),
            handle: id.handle(),
            created_at: now.clone(),
            route: Route::Rpc,
            session_id: Some(A_ID.into()),
            session_file: Some(b.omp.session('A')),
            session_handle: Some("e27c0de".into()),
            deliver_as: Deliver::Auto,
            text: "adopted".into(),
            cwd: b.omp.alpha(),
            omp_bin: Some(b.fake.clone()),
            live_socket: None,
            live_pid: None,
            profile_flag: None,
            thinking_flag: None,
            owners_dir: b.owners(),
            job_timeout_ms: 30_000,
            deadline_at: now,
            warm_ms: Some(30_000),
            env_digest: None,
        })
        .unwrap();
    let mut status = store.read_status(&id).unwrap();
    status.status = State::Accepted;
    status.worker_pid = Some(me.pid);
    status.worker_start = Some(me.start.clone());
    status.omp_reused = true;
    store.write_status(&id, &status).unwrap();
    lock::mark(
        &held,
        &lock::Holder {
            kind: lock::HolderKind::Job,
            job_id: Some(id.as_str().into()),
            ..warm_holder
        },
    )
    .unwrap();
    // The adopted job answers the control from its inbox, as its worker would.
    let control = b.job_dir(id.as_str()).join("control");
    let request = until_or_exit(&mut models, || {
        std::fs::read_dir(&control).ok()?.flatten().find_map(|e| {
            let name = e.file_name().into_string().ok()?;
            (!name.starts_with('.') && name.ends_with(".json") && !name.ends_with(".done.json"))
                .then(|| e.path())
        })
    });
    if let Some(request) = request {
        let body: Value = serde_json::from_slice(&std::fs::read(&request).unwrap()).unwrap();
        assert_eq!(body["op"], "list_models", "{body}");
        let answer = serde_json::json!({"v":1,"ok":true,"data":{
            "session_id":A_ID,
            "current":{"provider":"fake","id":"adopted"},
            "thinking_level":"high",
            "models":[{"provider":"fake","id":"adopted","name":"Adopted","reasoning":true,
                "thinking_levels":["off","high"],"context_window":1000,"max_tokens":100}],
        }});
        write_control(
            &control,
            &request
                .with_extension("done.json")
                .file_name()
                .unwrap()
                .to_string_lossy(),
            answer.to_string(),
        );
    }
    let doc = data(&models.wait_with_output().unwrap());
    assert_eq!(doc["route"], "job", "{doc}");
    assert_eq!(doc["current_source"], "session", "{doc}");
    assert_eq!(doc["current"]["id"], "adopted", "{doc}");
    assert_eq!(doc["thinking_level"], "high", "{doc}");
    assert_eq!(doc["total"], 1, "{doc}");
    assert_eq!(b.starts(), 0, "no omp of its own: the job answered");
    drop(held);
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

/// A stale idle registry for session A naming `pid` with its real start token, as a forger
/// reads both from `ps`.
fn forged_entry(pid: u32) -> warm::Entry {
    let Probe::Running(start) = liveness::start_token(pid) else {
        panic!("no start token for {pid}")
    };
    warm::Entry {
        v: 1,
        pid,
        start: Some(start),
        session_id: A_ID.into(),
        session_handle: None,
        job_id: "01a18bcc-6970-7685-8f03-0ecc16977273".into(),
        omp_pid: None,
        omp_start: None,
        warm_ms: 1000,
        expires_at: "2099-01-01T00:00:00Z".into(),
        state: warm::EntryState::Idle,
        started_at: "2020-01-01T00:00:00Z".into(),
    }
}
fn age(path: &std::path::Path) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000))
        .unwrap();
}

/// S-09, the audit's reproduction: a registry forged under a name that is not its session's
/// key (`forged.json`), naming a live process by its real pid and start token, is never a
/// release target. `release --all --force` signals nothing and deletes nothing; doctor names
/// the file in a warning.
#[test]
fn s09_a_misnamed_registry_is_never_signalled_or_removed() {
    let b = WarmBox::new();
    let mut sleeper = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    let dir = b.home().ensure_dir("run/warm").unwrap();
    let forged = dir.join("forged.json");
    std::fs::write(
        &forged,
        serde_json::to_vec(&forged_entry(sleeper.id())).unwrap(),
    )
    .unwrap();
    age(&forged);
    let out = data(&b.run(&["release", "--all", "--force", "--json"]));
    assert_eq!(out["total"], 0, "{out}");
    assert!(sleeper.try_wait().unwrap().is_none(), "never signalled");
    assert!(forged.exists(), "never deleted");
    let doctor = data(&b.run(&["doctor", "--json"]));
    let row = check(&doctor, "warm_workers");
    assert_eq!(row["status"], "warn", "{row}");
    assert_eq!(row["value"], "0 warm, 0 stale, 0 dead");
    let detail = row["detail"].as_str().unwrap();
    assert!(
        detail.contains("forged.json") && detail.contains("ignored"),
        "{detail}"
    );
    sleeper.kill().unwrap();
    sleeper.wait().unwrap();
}

/// Tech-debt F4: a torn registry may still name a running worker, so `release --all` releases
/// the readable ones and then fails (exit 1, `io`) naming the torn file; it never says "no warm
/// workers" with exit 0, and it removes nothing it cannot read.
#[test]
fn f4_release_all_never_succeeds_past_an_unreadable_registry() {
    let b = WarmBox::new();
    let dir = b.home().ensure_dir("run/warm").unwrap();
    let torn = dir.join("01a0f6b1-2680-7a3c-8b21-4a1f9e00c0de.json");
    std::fs::write(&torn, b"{\"v\":1,\"pid\":").unwrap();
    // A dead registry next to it is still cleaned and reported (never this test process: the
    // box's drop kills every pid left in a registry).
    let mut stand_in = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    let mut dead = forged_entry(stand_in.id());
    dead.start = Some("reused".into());
    warm::publish(&b.home(), &dead).unwrap();
    let out = b.run(&["release", "--all", "--json"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let e = &json(&out)["error"];
    assert_eq!(e["code"], "io", "{e}");
    assert!(e["message"].as_str().unwrap().contains("unreadable"), "{e}");
    assert_eq!(e["details"]["unreadable"][0], torn.to_str().unwrap());
    assert_eq!(e["details"]["workers"][0]["outcome"], "cleaned", "{e}");
    assert!(torn.exists());
    assert!(b.registry().is_none());
    let human = b.run(&["release", "--all", "--human"]);
    assert_eq!(human.status.code(), Some(1), "{human:?}");
    assert!(!String::from_utf8_lossy(&human.stdout).contains("no warm workers"));
    stand_in.kill().unwrap();
    stand_in.wait().unwrap();
}

/// Tech-debt F3: one worker's I/O error never drops the other rows. The first worker (by
/// session id) cannot be cleaned (its metadata lock path is a directory); the second still is,
/// and the error carries both rows.
#[test]
fn f3_release_all_keeps_every_row_past_one_workers_io_error() {
    let b = WarmBox::new();
    // Never this test process: the box's drop kills every pid left in a registry.
    let mut stand_in = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    let first = "01a0f6b1-2680-7a3c-8b21-4a1f9e00c0de";
    for id in [first, A_ID] {
        let mut dead = forged_entry(stand_in.id());
        dead.session_id = id.into();
        dead.start = Some("reused".into());
        warm::publish(&b.home(), &dead).unwrap();
    }
    let lock = b.home().root.join("run/warm").join(format!("{first}.lock"));
    std::fs::remove_file(&lock).unwrap();
    std::fs::create_dir(&lock).unwrap();
    let out = b.run(&["release", "--all", "--json"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let e = &json(&out)["error"];
    assert_eq!(e["code"], "io", "{e}");
    let message = e["message"].as_str().unwrap();
    assert!(
        message.contains(".lock") && message.ends_with("(1 of 2 warm workers released)"),
        "{message}"
    );
    let rows = e["details"]["workers"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{e}");
    assert_eq!(
        (&rows[0]["session"]["id"], &rows[0]["outcome"]),
        (&Value::from(first), &Value::from("failed"))
    );
    assert_eq!(
        (&rows[1]["session"]["id"], &rows[1]["outcome"]),
        (&Value::from(A_ID), &Value::from("cleaned"))
    );
    assert!(warm::read(&b.home(), A_ID).is_none());
    assert!(warm::read(&b.home(), first).is_some());
    assert!(stand_in.try_wait().unwrap().is_none(), "never signalled");
    stand_in.kill().unwrap();
    stand_in.wait().unwrap();
}

/// Tech-debt F5: when `--force` itself could not prove the process, the hint never suggests
/// `--force` again; without it, it does.
#[test]
fn f5_the_release_timeout_hint_names_force_only_without_it() {
    let b = WarmBox::new();
    // A live process with no start token: unprovable, so never signalled.
    let mut stand_in = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    let mut unproven = forged_entry(stand_in.id());
    unproven.start = None;
    warm::publish(&b.home(), &unproven).unwrap();
    for (force, names_force) in [(true, false), (false, true)] {
        let mut args = vec!["release", "e27c0de", "--timeout", "100ms", "--json"];
        if force {
            args.push("--force");
        }
        let out = b.run(&args);
        assert_eq!(out.status.code(), Some(7), "{out:?}");
        let e = &json(&out)["error"];
        assert_eq!(e["details"]["reason"], "release_timeout");
        let hint = e["hint"].as_str().unwrap();
        assert_eq!(
            hint.contains("--force"),
            names_force,
            "force {force}: {hint}"
        );
    }
    assert_eq!(warm::read(&b.home(), A_ID).unwrap().entry, unproven);
    assert!(stand_in.try_wait().unwrap().is_none(), "never signalled");
    stand_in.kill().unwrap();
    stand_in.wait().unwrap();
}

/// S-27: a job whose final session identity omp never confirmed (every get_state from the
/// third on answers after 4 s, past its 1 s budget, so the retry fails too) completes with the
/// `session_identity_unverified` warning and does not stay warm.
#[test]
fn s27_an_unverified_identity_never_stays_warm() {
    let b = WarmBox::with(&[
        ("FAKE_OMP_STATE_DELAY_AT", "3+"),
        ("FAKE_OMP_PREFLIGHT_DELAY_MS", "4000"),
    ]);
    let out = b.run(&["send", "e27c0de", "hi", "--warm", "30s", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let v = json(&out);
    let codes: Vec<&str> = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| w["code"].as_str())
        .collect();
    assert_eq!(codes, ["session_identity_unverified"], "{v}");
    b.wait_worker_gone(&job_id(&v["data"]));
    assert!(b.registry().is_none(), "no warm registry");
    assert!(!b.held(), "the session lock is free");
}

/// A flock on session A's metadata lock file from this process, as a holder stopped inside its
/// step keeps it: glen's own acquisitions give up after their bound.
fn hold_meta_lock(b: &WarmBox) -> std::fs::File {
    let path = b
        .home()
        .root
        .join("run/warm")
        .join(format!("{}.lock", lock::lock_key(A_ID)));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .unwrap();
    file.lock().unwrap();
    file
}

#[test]
fn ac_v2_405_a_blocked_metadata_lock_is_never_reported_cooling_or_cleaned() {
    let b = WarmBox::new();
    std::fs::create_dir_all(&b.omp.glen_home).unwrap();
    // A busy worker glen can prove by pid and start token.
    let mut sleeper = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    let Probe::Running(start) = liveness::start_token(sleeper.id()) else {
        panic!("no start token for the sleeper")
    };
    let busy = warm::Entry {
        v: 1,
        pid: sleeper.id(),
        start: Some(start),
        session_id: A_ID.into(),
        session_handle: None,
        job_id: "01a18bcc-6970-7685-8f03-0ecc16977273".into(),
        omp_pid: None,
        omp_start: None,
        warm_ms: 1000,
        expires_at: "2099-01-01T00:00:00Z".into(),
        state: warm::EntryState::Busy,
        started_at: "2026-11-02T00:00:00Z".into(),
    };
    warm::publish(&b.home(), &busy).unwrap();
    let release_file = warm::release_path(&b.home(), A_ID);
    let timed_out = |out: &Output, state: &str| {
        assert_eq!(out.status.code(), Some(7), "{out:?}");
        let e = json(out);
        assert_eq!(e["error"]["code"], "control_failed", "{e}");
        assert_eq!(e["error"]["details"]["reason"], "release_timeout", "{e}");
        let w = &e["error"]["details"]["workers"][0];
        assert_eq!(
            (w["state"].as_str(), w["outcome"].as_str()),
            (Some(state), Some("timeout")),
            "{e}"
        );
    };
    // The request cannot be written: the busy worker is waited for, never reported `cooling`.
    let held = hold_meta_lock(&b);
    timed_out(
        &b.run(&["release", "e27c0de", "--timeout", "200ms", "--json"]),
        "busy",
    );
    assert!(!release_file.exists());
    assert!(sleeper.try_wait().unwrap().is_none(), "never signalled");
    drop(held);
    let out = data(&b.run(&["release", "e27c0de", "--json"]));
    assert_eq!(out["workers"][0]["outcome"], "cooling", "{out}");
    assert!(release_file.exists());
    // Its worker is gone now: a dead registry whose removal the lock blocks is kept.
    sleeper.kill().unwrap();
    sleeper.wait().unwrap();
    let held = hold_meta_lock(&b);
    timed_out(&b.run(&["release", "e27c0de", "--json"]), "dead");
    assert_eq!(b.registry().unwrap().entry, busy, "the registry is kept");
    assert!(release_file.exists());
    drop(held);
    let out = data(&b.run(&["release", "e27c0de", "--json"]));
    assert_eq!(out["workers"][0]["outcome"], "cleaned", "{out}");
    assert!(b.registry().is_none() && !release_file.exists());
}

fn no_job_created(b: &WarmBox) -> bool {
    std::fs::read_dir(b.omp.glen_home.join("jobs")).map_or(true, |d| d.count() == 0)
}
fn warm_ms_of(b: &WarmBox, doc: &Value) -> Value {
    b.job(&job_id(doc)).0["warm_ms"].clone()
}
fn release_all(b: &WarmBox) {
    let entry = b.registry().map(|r| r.entry);
    assert!(b.run(&["release", "--all", "--json"]).status.success());
    if let Some(e) = entry {
        b.wait_cold(&e, Duration::from_secs(4));
    }
}

#[test]
fn ac_v2_401_off_by_default_flag_beats_env_and_zero_is_off() {
    let b = WarmBox::new();
    let cold = b.send(&[]);
    let (job, _) = b.job(&job_id(&cold));
    assert!(job.get("warm_ms").is_none(), "{job}");
    let digest = job["env_digest"].as_str().unwrap();
    assert!(
        digest.len() == 16
            && digest
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    b.wait_worker_gone(&job_id(&cold));
    assert!(b.registry().is_none() && !b.held());
    for (flag, var, want) in [
        (Some("2s"), None, Some(2000)),
        (None, Some("3s"), Some(3000)),
        (Some("1s"), Some("5s"), Some(1000)),
        (Some("0"), Some("5s"), None),
        (None, Some("0"), None),
        (Some("10m"), None, Some(600_000)),
    ] {
        let mut args = vec!["send", "e27c0de", "hi", "--json"];
        args.extend(flag.map(|f| ["--warm", f]).into_iter().flatten());
        let mut c = b.cmd(&args);
        c.envs(var.map(|v| ("GLEN_WARM", v)));
        let doc = data(&c.output().unwrap());
        assert_eq!(
            warm_ms_of(&b, &doc),
            want.map_or(Value::Null, Value::from),
            "{flag:?} {var:?}"
        );
        if want.is_some() {
            // Warm, or already cooled on its own (the 1s row): end it either way, never by
            // catching a window that may have closed.
            let id = job_id(&doc);
            poll(Duration::from_secs(10), || {
                (b.registry().is_some() || b.log(&id).contains("warm: cooling")).then_some(())
            });
            release_all(&b);
            b.wait_job_cold(&id, Duration::from_secs(6));
        } else {
            b.wait_worker_gone(&job_id(&doc));
            assert!(b.registry().is_none(), "{flag:?} {var:?}");
        }
    }
}

#[test]
fn ac_v2_401_bad_values_fail_before_any_job_exists() {
    let b = WarmBox::new();
    for (flag, var, code) in [
        (Some("11m"), None, "usage"),
        (Some("soon"), None, "usage"),
        (None, Some("11m"), "invalid_env"),
        (None, Some("abc"), "invalid_env"),
    ] {
        let mut args = vec!["send", "e27c0de", "hi", "--json"];
        args.extend(flag.map(|f| ["--warm", f]).into_iter().flatten());
        let mut c = b.cmd(&args);
        c.envs(var.map(|v| ("GLEN_WARM", v)));
        let out = c.output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{flag:?} {var:?}: {out:?}");
        if code == "invalid_env" {
            let e = json(&out);
            assert_eq!(e["error"]["code"], code);
            assert!(
                e["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("GLEN_WARM")
            );
        }
        assert!(no_job_created(&b), "{flag:?} {var:?}");
    }
    // GLEN_WARM is read only by send.
    let mut c = b.cmd(&["jobs", "--json"]);
    c.env("GLEN_WARM", "abc");
    assert!(c.output().unwrap().status.success());
}

#[test]
fn ac_v2_401_new_session_stays_warm_and_a_send_through_a_symlinked_cwd_is_stolen() {
    let b = WarmBox::new();
    let link = b.omp.tmp.path().join("alpha-link");
    std::os::unix::fs::symlink(b.omp.alpha(), &link).unwrap();
    let first = b.run(&[
        "send",
        "@new",
        "hi",
        "--cwd",
        link.to_str().unwrap(),
        "--warm",
        "5s",
        "--json",
    ]);
    let first = data(&first);
    let reg = b.wait_registry();
    let id = reg.entry.session_id.clone();
    assert_eq!(first["session"]["id"], id.as_str());
    // omp would have persisted the session; the fake does not, so the test writes its header.
    let file = b
        .omp
        .agent
        .join("sessions/-work-alpha")
        .join(format!("2026-11-02T00-00-00-000Z_{id}.jsonl"));
    std::fs::write(
        &file,
        format!(
            "{}\n",
            serde_json::json!({"type":"session","version":3,"id":id,"cwd":b.omp.alpha(),"timestamp":"2026-11-02T00:00:00Z"})
        ),
    )
    .unwrap();
    let handle = glen::rpc::session_handle(&id);
    let second = data(&b.run(&["send", &handle, "again", "--warm", "5s", "--json"]));
    assert_eq!(second["job"]["omp_reused"], true, "{second}");
    assert_eq!(b.starts(), 1);
    release_all(&b);
}

#[test]
fn ac_v2_401_live_route_ignores_warm_with_a_warning() {
    let mut lb = LiveBox::new();
    lb.start_ext(A_ID, "ok", &[]);
    let out = lb
        .rpc
        .glen(&["send", "e27c0de", "hi", "--warm", "5s", "--json"])
        .output()
        .unwrap();
    let v = json(&out);
    let id = v["data"]["job"]["id"].as_str().unwrap();
    assert_eq!(v["data"]["job"]["route"], "live", "{v}");
    let (job, _) = lb.rpc.job(id);
    assert!(
        job.get("warm_ms").is_none() && job.get("env_digest").is_none(),
        "{job}"
    );
    let warnings = v["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w["code"] == "ignored_flag"
                && w["message"].as_str().unwrap().contains("--warm")),
        "{v}"
    );
    let home = GlenHome {
        root: lb.rpc.omp.glen_home.clone(),
    };
    assert!(warm::read(&home, A_ID).is_none());
}

fn status_of(b: &WarmBox, id: &str) -> Value {
    b.job(id).1
}

#[test]
fn ac_v2_402_a_stolen_job_is_the_claimants_never_lost_while_it_lives() {
    let (_dir, run) = gate_path();
    let b = WarmBox::with(&[("FAKE_OMP_RUN_RELEASE", run.to_str().unwrap())]);
    std::fs::write(&run, b"go").unwrap();
    b.send(&["--warm", "5s"]);
    let reg = b.wait_registry();
    // Hold job 2 in omp by deleting the knob's file (never by changing an env value).
    std::fs::remove_file(&run).unwrap();
    let two = job_id(&b.no_wait(&["held", "--warm", "5s"]));
    let claimed = poll(Duration::from_secs(5), || {
        let s = status_of(&b, &two);
        (s["omp_reused"] == true).then_some(s)
    });
    assert_eq!(claimed["worker_pid"], reg.entry.pid);
    // The blocked cold worker exits on the steal without writing the status.
    poll(Duration::from_secs(5), || {
        b.log(&two)
            .contains("claimed by another worker, exiting")
            .then_some(())
    });
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(1500) {
        let s = status_of(&b, &two);
        assert!(
            matches!(s["status"].as_str(), Some("queued" | "accepted")),
            "{s}"
        );
        assert_eq!(s["worker_pid"], reg.entry.pid);
        std::thread::sleep(Duration::from_millis(100));
    }
    let waited = b.run(&["wait", &two, "--timeout", "500ms", "--json"]);
    assert_eq!(waited.status.code(), Some(10), "{waited:?}");
    assert_eq!(json(&waited)["data"]["job"]["task_state"], "working");
    std::fs::write(&run, b"go").unwrap();
    let done = data(&b.run(&["wait", &two, "--json"]));
    assert_eq!(done["job"]["status"], "completed");
    assert_eq!(done["assistant_text"], "pong: held");
    // A claimant that dies mid-job: its job is lost through liveness.
    let reg = b.wait_registry();
    std::fs::remove_file(&run).unwrap();
    let three = job_id(&b.no_wait(&["doomed", "--warm", "5s"]));
    poll(Duration::from_secs(5), || {
        (status_of(&b, &three)["status"] == "accepted").then_some(())
    });
    let pid = rustix::process::Pid::from_raw(reg.entry.pid as i32).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::KILL).unwrap();
    let lost = b.run(&["wait", &three, "--json"]);
    assert_eq!(
        json(&lost)["error"]["details"]["status"],
        "lost",
        "{lost:?}"
    );
    // The killed worker never tore down: end its omp group here, then drop its registry so the
    // box's guard never signals the dead worker's pid (which the OS may reuse).
    let omp = reg.entry.omp_pid.unwrap();
    let group = rustix::process::Pid::from_raw(omp as i32).unwrap();
    match rustix::process::kill_process_group(group, rustix::process::Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => (),
        Err(e) => panic!("cannot kill omp group {omp}: {e}"),
    }
    poll(Duration::from_secs(5), || group_gone(omp).then_some(()));
    warm::remove(&b.home(), A_ID).unwrap();
}
fn gate_path() -> (tempfile::TempDir, std::path::PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("release");
    (t, p)
}

#[test]
fn ac_v2_402_a_job_queued_behind_the_first_is_claimed_once_it_settles() {
    let (_dir, result) = gate_path();
    let b = WarmBox::with(&[("FAKE_OMP_RESULT_RELEASE", result.to_str().unwrap())]);
    let one = job_id(&b.no_wait(&["first", "--warm", "5s"]));
    poll(Duration::from_secs(5), || {
        (status_of(&b, &one)["status"] == "accepted").then_some(())
    });
    // Queued while job 1 runs: no warm duration, so warmth ends after it.
    let two = job_id(&b.no_wait(&["second"]));
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(&result, b"go").unwrap();
    let settled = poll(Duration::from_secs(10), || {
        let s = status_of(&b, &one);
        (s["status"] == "completed").then(Instant::now)
    });
    poll(Duration::from_secs(5), || {
        (status_of(&b, &two)["omp_reused"] == true).then_some(())
    });
    assert!(
        settled.elapsed() < Duration::from_millis(500),
        "{:?}",
        settled.elapsed()
    );
    let done = data(&b.run(&["wait", &two, "--json"]));
    assert_eq!(done["job"]["omp_reused"], true);
    poll(Duration::from_secs(6), || {
        (b.registry().is_none() && !b.held()).then_some(())
    });
    assert!(
        b.log(&one).contains("warm: cooling (ended)"),
        "{}",
        b.log(&one)
    );
    assert_eq!(b.starts(), 1);
}

fn contains(path: &std::path::Path, text: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|s| s.contains(text))
}

#[test]
fn ac_v2_407_an_idle_glen_reply_is_rejected_and_attributed_nowhere() {
    let (_dir, idle) = gate_path();
    let b = WarmBox::with(&[
        ("FAKE_OMP_IDLE_REPLY_RELEASE", idle.to_str().unwrap()),
        ("FAKE_OMP_SCENARIO", "progress"),
    ]);
    let first = b.send(&["--warm", "5s"]);
    let one = job_id(&first);
    let reg = b.wait_registry();
    let replies_one = std::fs::read(b.job_dir(&one).join("replies.ndjson")).unwrap();
    std::fs::write(&idle, b"go").unwrap();
    let result = poll(Duration::from_secs(5), || {
        b.frames_in("host_tool_result")
            .into_iter()
            .find(|f| f["id"] == "htc_idle")
    });
    assert_eq!(result["isError"], true, "{result}");
    // The next job's replies are its own; the first job's file is unchanged.
    let second = b.send(&["--warm", "5s"]);
    let two = job_id(&second);
    assert_eq!(second["job"]["omp_reused"], true);
    let texts: Vec<_> = second["replies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(texts, ["progress", "reply: hi"], "{second}");
    for r in second["replies"].as_array().unwrap() {
        assert_eq!(r["job_id"], two.as_str());
    }
    assert_eq!(
        std::fs::read(b.job_dir(&one).join("replies.ndjson")).unwrap(),
        replies_one
    );
    for job in [&one, &two] {
        for name in ["replies.ndjson", "result.json"] {
            assert!(
                !contains(&b.job_dir(job).join(name), "idle reply"),
                "{job} {name}"
            );
        }
    }
    for e in std::fs::read_dir(b.omp.glen_home.join("inbox")).unwrap() {
        assert!(!contains(&e.unwrap().path(), "idle reply"));
    }
    release_all(&b);
    let _ = reg;
}

#[test]
fn ac_v2_407_the_window_opens_after_settlement_and_a_long_job_stays_busy() {
    let (_run_dir, run) = gate_path();
    let (_result_dir, result) = gate_path();
    let b = WarmBox::with(&[
        ("FAKE_OMP_RUN_RELEASE", run.to_str().unwrap()),
        ("FAKE_OMP_RESULT_RELEASE", result.to_str().unwrap()),
    ]);
    std::fs::write(&run, b"go").unwrap();
    // Job 1 is held before its result, and job 2 queues behind it: the worker adopts job 2 on
    // its first idle tick after job 1 settles, so no process start-up races job 1's window.
    let one = job_id(&b.no_wait(&["first", "--warm", "2s"]));
    poll(Duration::from_secs(10), || {
        contains(&b.job_dir(&one).join("replies.ndjson"), "reply: first").then_some(())
    });
    let two = job_id(&b.no_wait(&["long", "--warm", "30s"]));
    // Job 1 is past the run gate (its reply is in): closing it holds only job 2.
    std::fs::remove_file(&run).unwrap();
    std::fs::write(&result, b"go").unwrap();
    let busy = poll(Duration::from_secs(10), || {
        b.registry()
            .filter(|r| r.entry.state == warm::EntryState::Busy && r.entry.job_id == two)
    });
    // Job 1's window opened at its settlement and runs its full 2 s.
    assert!(
        window_ms(&b, &one, &busy) >= 2000,
        "{}",
        busy.entry.expires_at
    );
    // Job 2 held in omp past that window keeps the worker busy, never expired.
    let expires: jiff::Timestamp = busy.entry.expires_at.parse().unwrap();
    let past = expires.as_millisecond() + 500 - jiff::Timestamp::now().as_millisecond();
    std::thread::sleep(Duration::from_millis(past.max(0) as u64));
    let still = b.registry().unwrap();
    assert_eq!(still.entry.state, warm::EntryState::Busy);
    assert_eq!(still.entry.job_id, two);
    assert!(!b.log(&one).contains("warm: cooling"), "{}", b.log(&one));
    std::fs::write(&run, b"go").unwrap();
    let done = data(&b.run(&["wait", &two, "--json"]));
    assert_eq!(done["job"]["omp_reused"], true, "{done}");
    // The window re-opens after job 2's own settlement, for job 2's own duration.
    let idle = b.wait_registry();
    assert_eq!(idle.entry.job_id, two);
    assert!(
        window_ms(&b, &two, &idle) >= 30_000,
        "{}",
        idle.entry.expires_at
    );
    release_all(&b);
    assert!(b.log(&one).contains("warm: cooling (release)"));
    assert_eq!(b.starts(), 1);
}

#[test]
fn ac_v2_407_an_unsettled_completed_job_does_not_stay_warm() {
    let b = WarmBox::with(&[("FAKE_OMP_NEVER_SETTLE", "1")]);
    let one = b.send(&["--warm", "5s"]);
    assert_eq!(one["job"]["status"], "completed");
    b.wait_worker_gone(&job_id(&one));
    assert!(b.registry().is_none() && !b.held());
    assert!(!b.log(&job_id(&one)).contains("warm:"));
}

fn signal(pid: u32, sig: rustix::process::Signal) {
    rustix::process::kill_process(rustix::process::Pid::from_raw(pid as i32).unwrap(), sig)
        .unwrap();
}

#[test]
fn ac_v2_403_every_ending_goes_through_one_teardown() {
    for ending in ["release-all", "sigterm", "sighup", "omp-exit"] {
        let (_dir, exit) = gate_path();
        let b = WarmBox::with(&[("FAKE_OMP_IDLE_EXIT_RELEASE", exit.to_str().unwrap())]);
        let one = job_id(&b.send(&["--warm", "60s"]));
        let reg = b.wait_registry();
        let reason = match ending {
            "release-all" => {
                assert!(b.run(&["release", "--all", "--json"]).status.success());
                "release"
            }
            "sigterm" => {
                signal(reg.entry.pid, rustix::process::Signal::TERM);
                "signal"
            }
            "sighup" => {
                signal(reg.entry.pid, rustix::process::Signal::HUP);
                "signal"
            }
            _ => {
                std::fs::write(&exit, b"go").unwrap();
                "omp_exited"
            }
        };
        let took = b.wait_cold(&reg.entry, Duration::from_secs(4));
        assert!(took < Duration::from_secs(4), "{ending}: {took:?}");
        assert!(!warm::release_path(&b.home(), A_ID).exists(), "{ending}");
        assert!(
            b.log(&one).contains(&format!("warm: cooling ({reason})")),
            "{ending}: {}",
            b.log(&one)
        );
    }
}

#[test]
fn ac_v2_403_a_live_endpoint_for_the_session_ends_warmth_within_a_second() {
    let mut lb = LiveBox::new();
    let owners = lb.rpc.owners();
    let send = lb
        .rpc
        .glen(&["send", "e27c0de", "hi", "--warm", "60s", "--json"])
        .env("FAKE_OMP_MULTI", "1")
        .env("FAKE_OMP_OWNERS_DIR", &owners)
        .output()
        .unwrap();
    let one = data(&send)["job"]["id"].as_str().unwrap().to_owned();
    let home = GlenHome {
        root: lb.rpc.omp.glen_home.clone(),
    };
    let reg = poll(Duration::from_secs(10), || warm::read(&home, A_ID));
    lb.start_ext(A_ID, "ok", &[]);
    let appeared = Instant::now();
    let omp = reg.entry.omp_pid.unwrap();
    poll(Duration::from_secs(1), || group_gone(omp).then_some(()));
    assert!(
        appeared.elapsed() < Duration::from_secs(1),
        "{:?}",
        appeared.elapsed()
    );
    poll(Duration::from_secs(4), || {
        (warm::read(&home, A_ID).is_none() && !lock::is_held(&home, A_ID) && gone(reg.entry.pid))
            .then_some(())
    });
    let log = std::fs::read_to_string(lb.rpc.job_dir(&one).join("worker.log")).unwrap();
    assert!(log.contains("warm: cooling (endpoint)"), "{log}");
}

#[test]
fn ac_v2_403_failed_aborted_and_forked_jobs_never_stay_warm() {
    for scenario in ["error", "aborted", "fork"] {
        let b = WarmBox::with(&[("FAKE_OMP_SCENARIO", scenario)]);
        let out = b.run(&["send", "e27c0de", "hi", "--warm", "5s", "--json"]);
        let v = json(&out);
        let id = v["data"]["job"]["id"]
            .as_str()
            .or_else(|| v["error"]["details"]["job"]["id"].as_str())
            .unwrap()
            .to_owned();
        b.wait_worker_gone(&id);
        assert!(b.registry().is_none() && !b.held(), "{scenario}");
        assert!(!b.log(&id).contains("warm:"), "{scenario}: {}", b.log(&id));
    }
}

#[test]
fn ac_v2_403_a_stray_control_for_the_last_job_is_answered_job_ended() {
    let b = WarmBox::new();
    let one = job_id(&b.send(&["--warm", "30s"]));
    b.wait_registry();
    let dir = b.job_dir(&one).join("control");
    write_control(&dir, "1.json", br#"{"v":1,"op":"list_models"}"#);
    let done = poll(Duration::from_secs(3), || {
        std::fs::read(dir.join("1.done.json"))
            .ok()
            .map(|bytes| serde_json::from_slice::<Value>(&bytes).unwrap())
    });
    assert_eq!(done["error"]["code"], "job_ended", "{done}");
    assert!(b.registry().is_some(), "still warm");
    release_all(&b);
}

#[test]
fn ac_v2_404_a_job_with_another_key_cools_the_worker_and_runs_cold() {
    let copy = {
        let b = WarmBox::new();
        let p = b.omp.tmp.path().join("fake_omp_copy");
        std::fs::copy(&b.fake, &p).unwrap();
        (b, p)
    };
    let (b, copied) = copy;
    for (name, extra, vars) in [
        ("thinking", vec!["--thinking", "high"], vec![]),
        (
            "env",
            vec![],
            vec![("PI_CONFIG_FILES", "/other/config.yml")],
        ),
        (
            "binary",
            vec![],
            vec![("GLEN_OMP_BIN", copied.to_str().unwrap())],
        ),
    ] {
        let starts = b.starts();
        b.send(&["--warm", "30s"]);
        let reg = b.wait_registry();
        let mut args = vec!["send", "e27c0de", "cold", "--json"];
        args.extend(&extra);
        let mut c = b.cmd(&args);
        c.envs(vars.iter().copied());
        let two = data(&c.output().unwrap());
        assert_eq!(two["job"]["status"], "completed", "{name}");
        assert_eq!(two["job"]["omp_reused"], false, "{name}");
        assert_eq!(b.starts(), starts + 2, "{name}: job 2 started its own omp");
        assert!(
            b.log(&reg.entry.job_id)
                .contains("warm: cooling (mismatch)"),
            "{name}"
        );
        if name == "env" {
            let last = b
                .fake_log()
                .into_iter()
                .rfind(|v| v["event"] == "start")
                .unwrap();
            assert_eq!(last["env"]["PI_CONFIG_FILES"], "/other/config.yml");
        }
        b.wait_worker_gone(&job_id(&two));
        assert!(b.registry().is_none(), "{name}");
    }
}
