//! `glen release`: end warm workers (v0.2 W4, design §A21).
use crate::{
    cli::{Cli, ReleaseArgs},
    env::Env,
    error::{ErrorCode, GlenError},
    glen_home::{self, GlenHome},
    jobs::{JobId, JobStore, liveness::Liveness, lock, matches_session},
    output::{Outcome, OutputCtx},
    render::{palette::Token, text},
    rpc::{
        self,
        warm::{self, Class, EntryState, Indexed},
    },
};
use serde_json::{Value, json};
use std::time::{Duration, Instant, SystemTime};

const POLL: Duration = Duration::from_millis(50);

pub fn run(
    _cli: &Cli,
    args: &ReleaseArgs,
    env: &Env,
    ctx: &OutputCtx,
) -> Result<Outcome, GlenError> {
    // Trusted before anything is read or signalled; creates nothing.
    let home = glen_home::resolve_trusted(env, &["run/warm", "run/locks", "jobs"])?;
    let index = warm::index(&home);
    let targets = match &args.target {
        None => index.entries,
        Some(target) => vec![select(&home, index.entries, target)?],
    };
    let stale = warm::stale_bound(env);
    let mut rows = vec![];
    let mut timed_out = false;
    // Tech-debt F3: one worker's I/O error never drops the rows of the others.
    let mut failed = None;
    for target in &targets {
        // The state as found, before this command changed it.
        let state = target
            .entry
            .class(target.mtime, SystemTime::now(), stale)
            .as_str(&target.entry);
        let outcome = match release_one(env, &home, target, args, stale) {
            Ok((outcome, ok)) => {
                timed_out |= !ok;
                outcome
            }
            Err(e) => {
                failed.get_or_insert(e);
                "failed"
            }
        };
        rows.push((target, state, outcome));
    }
    let workers: Vec<Value> = rows
        .iter()
        .map(|(t, state, outcome)| row(t, state, outcome))
        .collect();
    let done = rows
        .iter()
        .filter(|(_, _, o)| !matches!(*o, "failed" | "timeout"))
        .count();
    if let Some(mut e) = failed {
        e.message = format!(
            "{} ({done} of {} warm workers released)",
            e.message,
            rows.len()
        );
        return Err(e.with_detail("workers", workers));
    }
    if timed_out {
        let target = args.target.as_deref().unwrap_or("--all");
        // Tech-debt F5: after `--force` the kill itself could not be proven or finished.
        let hint = if args.force {
            "a process glen cannot prove unchanged is never signalled; check it with `glen doctor`, then retry".to_owned()
        } else {
            format!("wait and retry, or kill it with `glen release {target} --force`")
        };
        return Err(GlenError::new(
            ErrorCode::ControlFailed,
            "a warm worker did not release in time",
        )
        .with_hint(hint)
        .with_details(json!({"reason":"release_timeout","workers":workers})));
    }
    // Tech-debt F4: a registry glen cannot read may still name a running worker, so `--all`
    // never reports success (or "no warm workers") past one.
    if args.target.is_none() && !index.unreadable.is_empty() {
        let paths: Vec<String> = index
            .unreadable
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        return Err(GlenError::new(
            ErrorCode::Io,
            format!(
                "{} warm worker registr{} unreadable: {} ({done} of {} warm workers released)",
                paths.len(),
                if paths.len() == 1 { "y is" } else { "ies are" },
                text::single_line(&paths.join(", ")),
                rows.len()
            ),
        )
        .with_hint(
            "inspect the file; a worker it names may still run (`glen doctor` lists warm workers)",
        )
        .with_details(json!({"workers":workers,"unreadable":paths})));
    }
    let human = if rows.is_empty() {
        ctx.painter.paint(Token::Muted, "no warm workers")
    } else {
        rows.iter()
            .map(|(t, _, outcome)| human_line(ctx, t, outcome))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(Outcome::new(
        json!({"total":workers.len(),"workers":workers}),
        human,
    ))
}

/// One warm worker by session id, a hex tail of 4 or more, or a job handle (`run/warm` only). A
/// job handle names its session's worker: the session it targeted, or for `@new` the one omp
/// created (`actual_session_id`), even after the registry moved on to an adopted job.
fn select(home: &GlenHome, entries: Vec<Indexed>, target: &str) -> Result<Indexed, GlenError> {
    let mut found: Vec<Indexed> = entries
        .iter()
        .filter(|e| matches_session(Some(&e.entry.session_id), target))
        .cloned()
        .collect();
    let store = JobStore::new(home.clone());
    if found.is_empty()
        && (target.starts_with(['j', 'J']) || JobId::parse(target).is_some())
        && let Ok(id) = store.resolve(target)
        && let Ok(job) = store.read_job(&id)
    {
        let actual = store
            .read_status(&id)
            .ok()
            .and_then(|s| s.actual_session_id);
        found = entries
            .into_iter()
            .filter(|e| {
                let session = Some(e.entry.session_id.as_str());
                e.entry.job_id == id.as_str()
                    || job.session_id.as_deref() == session
                    || actual.as_deref() == session
            })
            .collect();
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(
            GlenError::new(ErrorCode::NotFound, format!("no warm worker matches `{target}`"))
                .with_hint("run `glen ls` to see warm sessions")
                .with_details(json!({"reason":"not_warm","target":target})),
        ),
        _ => Err(GlenError::new(
            ErrorCode::Ambiguous,
            format!("several warm workers match `{target}`"),
        )
        .with_details(json!({"candidates":found.iter().map(|e| json!({"id":e.entry.session_id,"handle":rpc::session_handle(&e.entry.session_id)})).collect::<Vec<_>>()}))),
    }
}

fn row(t: &Indexed, state: &str, outcome: &str) -> Value {
    let e = &t.entry;
    json!({
        "session":{"id":e.session_id,"handle":rpc::session_handle(&e.session_id)},
        "job":JobId::parse(&e.job_id).map(|id| id.handle()),
        "pid":e.pid,
        "state":state,
        "outcome":outcome,
    })
}

fn human_line(ctx: &OutputCtx, t: &Indexed, outcome: &str) -> String {
    let p = &ctx.painter;
    // The registry is a file under GLEN_HOME: its ids are untrusted text (S-25).
    let handle = text::single_line(&rpc::session_handle(&t.entry.session_id));
    match outcome {
        "released" => p.paint(
            Token::Success,
            &format!("✓ released {handle} (pid {})", t.entry.pid),
        ),
        "cooling" => p.paint(
            Token::Focus,
            &format!(
                "◐ {handle} cools after {}",
                JobId::parse(&t.entry.job_id)
                    .map_or_else(|| text::single_line(&t.entry.job_id), |id| id.handle())
            ),
        ),
        "forced" => p.paint(Token::Warning, &format!("! killed {handle}")),
        _ => p.paint(
            Token::Muted,
            &format!("· removed a dead entry for {handle}"),
        ),
    }
}

/// The outcome for one worker, and false when it did not release within the timeout.
fn release_one(
    env: &Env,
    home: &GlenHome,
    t: &Indexed,
    args: &ReleaseArgs,
    stale: Duration,
) -> Result<(&'static str, bool), GlenError> {
    let e = &t.entry;
    let io = |err| crate::sandbox::map_home_write(env, "release", err);
    let class = e.class(t.mtime, SystemTime::now(), stale);
    if class == Class::Dead {
        return cleaned(home, e, "cleaned").map_err(io);
    }
    let delivered = warm::request_release(home, &e.session_id, &e.ident()).map_err(io)?;
    // Only a written request makes a busy worker cool after its job; one the metadata lock kept
    // out leaves it to the wait below, as a worker that never answers.
    if delivered && class == Class::Fresh && e.state == EntryState::Busy {
        return Ok(("cooling", true));
    }
    if class == Class::Stale && args.force {
        return force(home, t).map_err(io);
    }
    let end = Instant::now() + args.timeout;
    loop {
        if gone(home, e) {
            return Ok(("released", true));
        }
        if Instant::now() >= end {
            return if args.force {
                force(home, t).map_err(io)
            } else {
                Ok(("timeout", false))
            };
        }
        std::thread::sleep(POLL.min(end.saturating_duration_since(Instant::now())));
    }
}

/// The worker `e` names is gone: the registry is missing or another worker's (pid and start
/// token), and the session lock is free, recorded as another identity's, or `e`'s process is
/// proven dead. A successor that took the session meanwhile does not hold the release up.
fn gone(home: &GlenHome, e: &warm::Entry) -> bool {
    let me = e.ident();
    warm::read(home, &e.session_id).is_none_or(|now| now.entry.ident() != me)
        && (!lock::is_held(home, &e.session_id)
            || lock::holder(home, &e.session_id)
                .is_some_and(|h| warm::ident(h.pid, h.start.as_deref()) != me)
            || warm::kill_identity(e).worker == Liveness::Dead)
}

/// How long `--force` waits for a killed worker and its omp group to be gone.
const FORCE_BOUND: Duration = Duration::from_secs(2);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kill {
    Group(u32),
    Process(u32),
}
/// SIGKILL; a process or group that is already gone is not an error.
fn signal(target: Kill) -> std::io::Result<()> {
    match target {
        Kill::Group(pid) => rpc::kill_group(pid, rustix::process::Signal::KILL),
        Kill::Process(pid) => {
            let p = rpc::signal_pid(pid)
                .ok_or_else(|| std::io::Error::other(format!("invalid worker pid {pid}")))?;
            match rustix::process::kill_process(p, rustix::process::Signal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
    }
}
fn force(home: &GlenHome, t: &Indexed) -> std::io::Result<(&'static str, bool)> {
    force_with(home, &t.entry, &signal, FORCE_BOUND)
}
/// SIGKILL the omp group and the worker, but only when every recorded process is proven by pid
/// and start token (binding J11): an `Unknown` worker or omp is never signalled, and a failed
/// signal or a worker still alive after `bound` falls to `release_timeout` with the registry kept.
/// `forced` only once the worker is proven dead, the recorded omp group is gone, whether or not
/// its leader was alive to be signalled, and its files are removed (see [`cleaned`]).
fn force_with(
    home: &GlenHome,
    e: &warm::Entry,
    kill: &dyn Fn(Kill) -> std::io::Result<()>,
    bound: Duration,
) -> std::io::Result<(&'static str, bool)> {
    let identity = warm::kill_identity(e);
    match identity.worker {
        Liveness::Dead => return cleaned(home, e, "cleaned"),
        Liveness::Unknown => return Ok(("timeout", false)),
        Liveness::Alive => (),
    }
    // Only a proven omp leader's group is signalled: once the leader is gone its pid may be
    // reused, so its group cannot be proven omp's. An unproven leader blocks the kill. The
    // recorded group must be gone either way, since omp's descendants can outlive their leader.
    let (signalled, recorded) = match (e.omp_pid, identity.omp) {
        (_, Some(Liveness::Unknown)) => return Ok(("timeout", false)),
        (Some(pid), Some(Liveness::Alive)) => (Some(pid), Some(pid)),
        (pid, _) => (None, pid),
    };
    // S-09: each identity is proven again right before its own signal, so a process that ended
    // since the first check (its pid now free for reuse) is never signalled. A leader gone by
    // then is skipped (its group must still be gone below); a worker gone by then needs no kill.
    if let Some(pid) = signalled {
        match warm::kill_identity(e).omp {
            Some(Liveness::Alive) => {
                if kill(Kill::Group(pid)).is_err() {
                    return Ok(("timeout", false));
                }
            }
            Some(Liveness::Dead) => (),
            _ => return Ok(("timeout", false)),
        }
    }
    match warm::kill_identity(e).worker {
        Liveness::Alive => {
            if kill(Kill::Process(e.pid)).is_err() {
                return Ok(("timeout", false));
            }
        }
        Liveness::Dead => (),
        Liveness::Unknown => return Ok(("timeout", false)),
    }
    let end = Instant::now() + bound;
    while !(warm::kill_identity(e).worker == Liveness::Dead && recorded.is_none_or(group_gone)) {
        if Instant::now() >= end {
            return Ok(("timeout", false));
        }
        std::thread::sleep(POLL);
    }
    cleaned(home, e, "forced")
}
fn group_gone(pid: u32) -> bool {
    rpc::signal_pid(pid).is_none_or(|pid| {
        rustix::process::test_kill_process_group(pid) == Err(rustix::io::Errno::SRCH)
    })
}

/// `outcome` once `e`'s registry and release request are removed, each only while it still names
/// `e`: a successor's files survive, even ones written as this runs, and sparing them is still
/// success. No session lock is taken, so a registry whose identity is not that of the process
/// holding the lock is removed too. A metadata lock still busy after its bound (a stopped
/// holder) removes nothing more and is `release_timeout`, with the files kept for a later run.
fn cleaned(
    home: &GlenHome,
    e: &warm::Entry,
    outcome: &'static str,
) -> std::io::Result<(&'static str, bool)> {
    match warm::remove_if(home, &e.session_id, &e.ident())
        .and_then(|_| warm::clear_release_if(home, &e.session_id, &e.ident()))
    {
        Ok(_) => Ok((outcome, true)),
        Err(err) if err.kind() == std::io::ErrorKind::TimedOut => Ok(("timeout", false)),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const A: &str = "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de";
    const B: &str = "01a0f6b1-2680-7a3c-8b21-4a1f9e00c0de";
    fn entry(session: &str, pid: u32, start: Option<String>) -> warm::Entry {
        warm::Entry {
            v: 1,
            pid,
            start,
            session_id: session.into(),
            session_handle: None,
            job_id: "01a18bcc-6970-7685-8f03-0ecc16977273".into(),
            omp_pid: None,
            omp_start: None,
            warm_ms: 1000,
            expires_at: "2099-01-01T00:00:00Z".into(),
            state: EntryState::Idle,
            started_at: "2026-11-02T00:00:00Z".into(),
        }
    }
    fn home(t: &tempfile::TempDir) -> GlenHome {
        GlenHome {
            root: t.path().join("glen"),
        }
    }
    #[test]
    fn targets_match_an_id_a_tail_or_a_job_handle() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        for s in [A, B] {
            warm::publish(&h, &entry(s, std::process::id(), None)).unwrap();
        }
        let entries = || warm::index(&h).entries;
        assert_eq!(select(&h, entries(), A).unwrap().entry.session_id, A);
        assert_eq!(
            select(&h, entries(), &A.to_uppercase())
                .unwrap()
                .entry
                .session_id,
            A
        );
        assert_eq!(
            select(&h, entries(), "e27c0de").unwrap().entry.session_id,
            A
        );
        assert_eq!(
            select(&h, entries(), "e00c0de").unwrap().entry.session_id,
            B
        );
        let e = select(&h, entries(), "27b").unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
        assert_eq!(e.details.unwrap()["reason"], "not_warm");
        let e = select(&h, entries(), "c0de").unwrap_err();
        assert_eq!(e.code, ErrorCode::Ambiguous, "a tail both sessions share");
        // A job handle: its session's worker.
        let store = JobStore::new(h.clone());
        let mut j = crate::jobs::tests::job();
        j.session_id = Some(B.into());
        store.create(&j).unwrap();
        let handle = JobId::parse(&j.id).unwrap().handle();
        assert_eq!(select(&h, entries(), &handle).unwrap().entry.session_id, B);
        let e = select(&h, entries(), "jfffffff").unwrap_err();
        assert_eq!(e.details.unwrap()["reason"], "not_warm");
    }
    #[test]
    fn dead_and_mismatched_entries_are_cleaned_never_signalled() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let args = ReleaseArgs {
            target: None,
            all: true,
            force: true,
            timeout: Duration::from_millis(100),
        };
        let env = Env::for_test(None, t.path(), &[]);
        // This test process stands in for a worker whose start token no longer matches.
        warm::publish(&h, &entry(A, std::process::id(), Some("reused".into()))).unwrap();
        let entries = warm::index(&h).entries;
        let (outcome, ok) = release_one(&env, &h, &entries[0], &args, warm::STALE).unwrap();
        assert_eq!((outcome, ok), ("cleaned", true));
        assert!(warm::read(&h, A).is_none());
        assert!(!warm::release_path(&h, A).exists());
        let doc = row(&entries[0], "dead", outcome);
        assert_eq!(doc["state"], "dead");
        assert_eq!(doc["outcome"], "cleaned");
        assert_eq!(doc["session"]["handle"], "e27c0de");
        assert_eq!(doc["job"], "j6977273");
    }
    #[test]
    fn an_unproven_identity_is_never_signalled() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        // A live pid without a start token: `Unknown`, so --force cannot prove it.
        let mut e = entry(A, std::process::id(), None);
        e.state = EntryState::Idle;
        let indexed = Indexed {
            entry: e,
            mtime: SystemTime::now() - Duration::from_secs(60),
        };
        assert_eq!(force(&h, &indexed).unwrap(), ("timeout", false));
    }
    fn me() -> crate::jobs::liveness::Identity {
        crate::jobs::liveness::current().unwrap()
    }
    /// A live worker `glen release` can prove (pid and start token): a `sleep` child.
    fn sleeper() -> (std::process::Child, warm::Entry) {
        let child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let crate::jobs::liveness::Probe::Running(start) =
            crate::jobs::liveness::start_token(child.id())
        else {
            panic!("no start token for the sleeper")
        };
        let e = entry(A, child.id(), Some(start));
        (child, e)
    }
    /// This process as the session's next warm worker: it holds the lock, marked `warm`, and has
    /// published its registry.
    fn successor(h: &GlenHome) -> (lock::SessionLock, warm::Entry) {
        let me = me();
        let held = lock::try_acquire(h, A).unwrap().unwrap();
        lock::mark(
            &held,
            &lock::Holder {
                v: 1,
                kind: lock::HolderKind::Warm,
                job_id: Some(JobId::new().as_str().into()),
                pid: me.pid,
                start: Some(me.start.clone()),
            },
        )
        .unwrap();
        let e = entry(A, me.pid, Some(me.start));
        warm::publish(h, &e).unwrap();
        (held, e)
    }
    fn args(force: bool, timeout: Duration) -> ReleaseArgs {
        ReleaseArgs {
            target: None,
            all: true,
            force,
            timeout,
        }
    }
    #[test]
    fn release_completes_when_a_successor_retook_the_session() {
        for force in [false, true] {
            let t = tempfile::tempdir().unwrap();
            let h = home(&t);
            let env = Env::for_test(None, t.path(), &[]);
            let (mut old, w1) = sleeper();
            // The snapshot `glen release` indexed; then that worker cooled and a successor took
            // the lock and published its own registry.
            let snapshot = Indexed {
                entry: w1,
                mtime: SystemTime::now(),
            };
            let (_held, w2) = successor(&h);
            let got = release_one(
                &env,
                &h,
                &snapshot,
                &args(force, Duration::from_millis(300)),
                warm::STALE,
            )
            .unwrap();
            assert_eq!(got, ("released", true), "force {force}");
            assert_eq!(warm::read(&h, A).unwrap().entry, w2, "force {force}");
            assert!(old.try_wait().unwrap().is_none(), "never signalled");
            old.kill().unwrap();
            old.wait().unwrap();
        }
    }
    #[test]
    fn a_mismatched_entry_is_cleaned_while_a_live_process_holds_the_lock() {
        for force in [false, true] {
            let t = tempfile::tempdir().unwrap();
            let h = home(&t);
            let env = Env::for_test(None, t.path(), &[]);
            // This process holds the session lock as its warm worker, but the registry names a
            // start token that is not its own.
            let (_held, mut e) = successor(&h);
            e.start = Some("reused".into());
            warm::publish(&h, &e).unwrap();
            warm::request_release(&h, A, &e.ident()).unwrap();
            let snapshot = warm::read(&h, A).unwrap();
            let got = release_one(
                &env,
                &h,
                &snapshot,
                &args(force, Duration::from_millis(100)),
                warm::STALE,
            )
            .unwrap();
            assert_eq!(got, ("cleaned", true), "force {force}");
            assert!(warm::read(&h, A).is_none(), "force {force}");
            assert!(!warm::release_path(&h, A).exists(), "force {force}");
            assert!(lock::is_held(&h, A), "the lock stays with its holder");
        }
    }
    #[test]
    fn a_dead_snapshot_never_removes_a_successors_files() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let env = Env::for_test(None, t.path(), &[]);
        let snapshot = Indexed {
            entry: entry(A, crate::jobs::liveness::tests_dead_pid(), None),
            mtime: SystemTime::now(),
        };
        let (held, w2) = successor(&h);
        warm::request_release(&h, A, &w2.ident()).unwrap();
        let release = || {
            release_one(
                &env,
                &h,
                &snapshot,
                &args(true, Duration::from_millis(100)),
                warm::STALE,
            )
            .unwrap()
        };
        assert_eq!(release(), ("cleaned", true));
        assert_eq!(warm::read(&h, A).unwrap().entry, w2);
        assert!(warm::release_requested(&h, A, &w2.ident()));
        // With the lock free too: a file is removed only while it names the dead worker.
        drop(held);
        assert_eq!(release(), ("cleaned", true));
        assert_eq!(warm::read(&h, A).unwrap().entry, w2);
        assert!(warm::release_requested(&h, A, &w2.ident()));
    }
    #[test]
    fn force_never_signals_an_unproven_omp_and_keeps_the_registry() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let me = me();
        // A proven worker whose omp pid is recorded without a start token (`Unknown`).
        let mut e = entry(A, me.pid, Some(me.start));
        e.omp_pid = Some(me.pid);
        warm::publish(&h, &e).unwrap();
        let never = |k: Kill| -> std::io::Result<()> { panic!("signalled {k:?}") };
        assert_eq!(
            force_with(&h, &e, &never, Duration::ZERO).unwrap(),
            ("timeout", false)
        );
        assert_eq!(warm::read(&h, A).unwrap().entry, e, "the registry is kept");
    }
    #[test]
    fn a_failed_signal_or_a_surviving_worker_is_never_forced() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let me = me();
        let e = entry(A, me.pid, Some(me.start));
        warm::publish(&h, &e).unwrap();
        let refused =
            |_: Kill| -> std::io::Result<()> { Err(std::io::ErrorKind::PermissionDenied.into()) };
        assert_eq!(
            force_with(&h, &e, &refused, Duration::ZERO).unwrap(),
            ("timeout", false)
        );
        assert_eq!(warm::read(&h, A).unwrap().entry, e);
        // A signal that reported success but left the worker (this process) running.
        let sent = std::cell::Cell::new(vec![]);
        let ignored = |k: Kill| -> std::io::Result<()> {
            let mut all = sent.take();
            all.push(k);
            sent.set(all);
            Ok(())
        };
        assert_eq!(
            force_with(&h, &e, &ignored, Duration::from_millis(100)).unwrap(),
            ("timeout", false)
        );
        assert_eq!(sent.take(), [Kill::Process(me.pid)]);
        assert_eq!(warm::read(&h, A).unwrap().entry, e);
    }
    #[test]
    fn force_kills_a_proven_worker_then_removes_its_registry() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let (mut child, e) = sleeper();
        warm::publish(&h, &e).unwrap();
        assert_eq!(
            force_with(&h, &e, &signal, FORCE_BOUND).unwrap(),
            ("forced", true)
        );
        assert!(warm::read(&h, A).is_none());
        assert!(!child.wait().unwrap().success(), "killed");
    }
    /// Kills the group on drop, so a failed assertion leaves no `sleep` behind.
    struct Group(u32);
    impl Drop for Group {
        fn drop(&mut self) {
            let _ = rpc::kill_group(self.0, rustix::process::Signal::KILL);
        }
    }
    #[test]
    fn force_is_never_forced_while_a_dead_omp_leaders_group_survives() {
        use std::os::unix::process::CommandExt;
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        // omp's group leader exits at once; a descendant stays in the group.
        let mut leader = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30 & exit 0"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let group = Group(leader.id());
        leader.wait().unwrap();
        assert!(!group_gone(group.0), "the descendant keeps the group");
        let (mut worker, mut e) = sleeper();
        e.omp_pid = Some(group.0);
        e.omp_start = Some("the-exited-leader".into());
        assert_eq!(warm::kill_identity(&e).omp, Some(Liveness::Dead));
        warm::publish(&h, &e).unwrap();
        let sent = std::cell::Cell::new(vec![]);
        let recorded = |k: Kill| -> std::io::Result<()> {
            let mut all = sent.take();
            all.push(k);
            sent.set(all);
            signal(k)
        };
        assert_eq!(
            force_with(&h, &e, &recorded, Duration::from_millis(300)).unwrap(),
            ("timeout", false)
        );
        assert_eq!(
            sent.take(),
            [Kill::Process(e.pid)],
            "a group whose leader is gone is never signalled"
        );
        assert!(
            !worker.wait().unwrap().success(),
            "the proven worker is killed"
        );
        assert!(!group_gone(group.0), "the survivor is left alone");
        assert_eq!(warm::read(&h, A).unwrap().entry, e, "the registry is kept");
        // Once that group is gone, a proven worker with the same omp record is forced.
        rpc::kill_group(group.0, rustix::process::Signal::KILL).unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        while !group_gone(group.0) {
            assert!(Instant::now() < end, "the killed group outlived 10 s");
            std::thread::sleep(POLL);
        }
        let (mut worker, mut e) = sleeper();
        e.omp_pid = Some(group.0);
        e.omp_start = Some("the-exited-leader".into());
        warm::publish(&h, &e).unwrap();
        assert_eq!(
            force_with(&h, &e, &signal, FORCE_BOUND).unwrap(),
            ("forced", true)
        );
        assert!(warm::read(&h, A).is_none());
        assert!(!worker.wait().unwrap().success(), "killed");
    }
    #[test]
    fn a_release_request_that_cannot_be_written_is_never_reported_cooling() {
        for force in [false, true] {
            let t = tempfile::tempdir().unwrap();
            let h = home(&t);
            let env = Env::for_test(None, t.path(), &[]);
            let (mut child, mut e) = sleeper();
            e.state = EntryState::Busy;
            warm::publish(&h, &e).unwrap();
            let snapshot = warm::read(&h, A).unwrap();
            // A holder stopped inside its step keeps the metadata lock past its bound.
            let stopped = warm::hold_meta_lock(&h, A);
            let got = release_one(
                &env,
                &h,
                &snapshot,
                &args(force, Duration::from_millis(100)),
                warm::STALE,
            )
            .unwrap();
            assert_eq!(got, ("timeout", false), "force {force}");
            assert!(!warm::release_path(&h, A).exists(), "force {force}");
            assert_eq!(warm::read(&h, A).unwrap().entry, e, "force {force}");
            drop(stopped);
            if force {
                assert!(!child.wait().unwrap().success(), "killed after the timeout");
                continue;
            }
            assert!(child.try_wait().unwrap().is_none(), "never signalled");
            // Once the lock is free, the request is written and the busy worker cools.
            let got = release_one(
                &env,
                &h,
                &snapshot,
                &args(false, Duration::from_millis(100)),
                warm::STALE,
            )
            .unwrap();
            assert_eq!(got, ("cooling", true));
            assert!(warm::release_requested(&h, A, &e.ident()));
            child.kill().unwrap();
            child.wait().unwrap();
        }
    }
    #[test]
    fn a_removal_the_metadata_lock_blocks_is_a_timeout_and_keeps_the_files() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let env = Env::for_test(None, t.path(), &[]);
        let dead = entry(A, crate::jobs::liveness::tests_dead_pid(), None);
        warm::publish(&h, &dead).unwrap();
        warm::request_release(&h, A, &dead.ident()).unwrap();
        let snapshot = warm::read(&h, A).unwrap();
        let release = |force| {
            release_one(
                &env,
                &h,
                &snapshot,
                &args(force, Duration::from_millis(100)),
                warm::STALE,
            )
            .unwrap()
        };
        let stopped = warm::hold_meta_lock(&h, A);
        for force in [false, true] {
            assert_eq!(release(force), ("timeout", false), "force {force}");
            assert_eq!(warm::read(&h, A).unwrap().entry, dead, "force {force}");
            assert!(warm::release_requested(&h, A, &dead.ident()));
        }
        // `--force` kills a proven worker, but is not `forced` while its files cannot go.
        let (mut child, mut live) = sleeper();
        live.session_id = B.into();
        warm::publish(&h, &live).unwrap();
        let stopped_b = warm::hold_meta_lock(&h, B);
        assert_eq!(
            force_with(&h, &live, &signal, FORCE_BOUND).unwrap(),
            ("timeout", false)
        );
        assert!(!child.wait().unwrap().success(), "killed");
        assert_eq!(
            warm::read(&h, B).unwrap().entry,
            live,
            "the registry is kept"
        );
        // Once the holders let go, the next release cleans.
        drop(stopped);
        drop(stopped_b);
        assert_eq!(release(false), ("cleaned", true));
        assert!(warm::read(&h, A).is_none() && !warm::release_path(&h, A).exists());
        let dead_b = warm::read(&h, B).unwrap();
        assert_eq!(
            release_one(&env, &h, &dead_b, &args(false, Duration::ZERO), warm::STALE).unwrap(),
            ("cleaned", true)
        );
        assert!(warm::read(&h, B).is_none());
    }
    /// S-09: every identity is proven again right before its own signal. Here signalling omp's
    /// group also ends the worker (as a worker exits once its omp dies), so by its own turn the
    /// worker is a zombie whose re-proof says dead: it is never signalled, and the release is
    /// still `forced` once both are gone.
    #[test]
    fn s09_a_worker_that_ended_after_the_first_proof_is_never_signalled() {
        use std::os::unix::process::CommandExt;
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let omp = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let omp_pid = omp.id();
        let _cleanup = Group(omp_pid);
        let crate::jobs::liveness::Probe::Running(omp_start) =
            crate::jobs::liveness::start_token(omp_pid)
        else {
            panic!("no start token for omp")
        };
        let (mut worker, mut e) = sleeper();
        e.omp_pid = Some(omp_pid);
        e.omp_start = Some(omp_start);
        warm::publish(&h, &e).unwrap();
        let worker_pid = worker.id();
        let omp = std::cell::RefCell::new(omp);
        let sent = std::cell::RefCell::new(vec![]);
        let kill = |k: Kill| -> std::io::Result<()> {
            sent.borrow_mut().push(k);
            signal(k)?;
            if k == Kill::Group(omp_pid) {
                omp.borrow_mut().wait()?;
                let pid = rpc::signal_pid(worker_pid).unwrap();
                rustix::process::kill_process(pid, rustix::process::Signal::KILL)?;
                let end = Instant::now() + Duration::from_secs(5);
                while !matches!(
                    crate::jobs::liveness::start_token(worker_pid),
                    crate::jobs::liveness::Probe::Gone
                ) {
                    assert!(Instant::now() < end, "the worker outlived SIGKILL");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            Ok(())
        };
        assert_eq!(
            force_with(&h, &e, &kill, FORCE_BOUND).unwrap(),
            ("forced", true)
        );
        assert_eq!(*sent.borrow(), [Kill::Group(omp_pid)], "no second signal");
        assert!(warm::read(&h, A).is_none());
        assert!(!worker.wait().unwrap().success());
    }
    /// S-04: under a Codex marker on Linux (forced here) a worker whose pid is gone from this
    /// process's view may be alive in the host's pid namespace: its registry is never cleaned
    /// and it is never signalled. Without the marker the same entry is cleaned.
    #[test]
    fn s04_a_gone_worker_is_never_cleaned_where_pids_may_be_hidden() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let env = Env::for_test(None, t.path(), &[]);
        let dead = entry(
            A,
            crate::jobs::liveness::tests_dead_pid(),
            Some("linux:1".into()),
        );
        warm::publish(&h, &dead).unwrap();
        let snapshot = warm::read(&h, A).unwrap();
        crate::jobs::liveness::force_pids_hidden_for_test(Some(true));
        assert_ne!(
            snapshot
                .entry
                .class(snapshot.mtime, SystemTime::now(), warm::STALE),
            Class::Dead
        );
        for force in [false, true] {
            let got = release_one(
                &env,
                &h,
                &snapshot,
                &args(force, Duration::from_millis(100)),
                warm::STALE,
            );
            assert_eq!(got.unwrap(), ("timeout", false), "force {force}");
            assert_eq!(warm::read(&h, A).unwrap().entry, dead, "force {force}");
        }
        crate::jobs::liveness::force_pids_hidden_for_test(None);
        let got = release_one(
            &env,
            &h,
            &snapshot,
            &args(false, Duration::from_millis(100)),
            warm::STALE,
        );
        assert_eq!(got.unwrap(), ("cleaned", true));
        assert!(warm::read(&h, A).is_none());
    }
    #[test]
    fn an_at_new_job_handle_finds_its_worker_after_a_follow_up_was_adopted() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let store = JobStore::new(h.clone());
        let mut first = crate::jobs::tests::job();
        first.route = crate::jobs::Route::New;
        first.session_id = None;
        first.session_file = None;
        first.session_handle = None;
        store.create(&first).unwrap();
        let id = JobId::parse(&first.id).unwrap();
        let mut s = store.read_status(&id).unwrap();
        s.status = crate::jobs::State::Completed;
        s.actual_session_id = Some(A.into());
        store.write_status(&id, &s).unwrap();
        // The registry names the adopted follow-up job now, not the `@new` job.
        let mut e = entry(A, std::process::id(), None);
        e.job_id = JobId::new().as_str().into();
        warm::publish(&h, &e).unwrap();
        warm::publish(&h, &entry(B, std::process::id(), None)).unwrap();
        let found = select(&h, warm::index(&h).entries, &id.handle()).unwrap();
        assert_eq!(found.entry.session_id, A);
    }
}
