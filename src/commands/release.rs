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
        let outcome = match release_one(env, &home, target, args, stale, &KERNEL) {
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
        .filter(|(_, _, o)| !matches!(*o, "failed" | "timeout" | UNVERIFIED))
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
    let mut row = json!({
        "session":{"id":e.session_id,"handle":rpc::session_handle(&e.session_id)},
        "job":JobId::parse(&e.job_id).map(|id| id.handle()),
        "pid":e.pid,
        "state":state,
        "outcome":outcome,
    });
    if outcome == UNVERIFIED {
        row["outcome"] = json!("timeout");
        row["reason"] = json!("unverified_process");
    }
    row
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
    facts: &Facts<'_>,
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
        return force(home, t, facts).map_err(io);
    }
    let end = Instant::now() + args.timeout;
    loop {
        if gone(home, e) {
            return Ok(("released", true));
        }
        if Instant::now() >= end {
            return if args.force {
                force(home, t, facts).map_err(io)
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
/// `--force` could not prove a recorded process is the session's glen worker or its omp
/// (S-09): nothing more is signalled and the row is `timeout` with `reason:"unverified_process"`.
const UNVERIFIED: &str = "unverified";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kill {
    /// SIGKILL to the omp process group.
    Group(u32),
    /// SIGKILL to the worker.
    Process(u32),
    /// SIGSTOP to the worker, so it cannot reap omp while omp is proven and signalled (F3).
    Stop(u32),
    /// SIGCONT to a worker stopped for a kill that was then not proven.
    Resume(u32),
}
/// Sends `target`'s signal; a process or group that is already gone is not an error. The
/// worker is signalled through its [`Pinned`] handle, and omp's group through omp's.
fn signal(target: Kill, pinned: Option<&Pinned>) -> std::io::Result<()> {
    use rustix::process::Signal;
    let (pid, sig) = match target {
        Kill::Group(pid) => {
            return match pinned {
                Some(p) => p.send_group(Signal::KILL),
                None => rpc::kill_group(pid, Signal::KILL),
            };
        }
        Kill::Process(pid) => (pid, Signal::KILL),
        Kill::Stop(pid) => (pid, Signal::STOP),
        Kill::Resume(pid) => (pid, Signal::CONT),
    };
    match pinned {
        Some(p) => p.send(sig).map(drop),
        None => Pinned::open(pid)
            .ok_or_else(|| std::io::Error::other(format!("cannot pin worker {pid}")))?
            .send(sig)
            .map(drop),
    }
}
/// A process held for signalling: the worker, or omp as the leader of its group. On Linux it is
/// a pidfd opened before the process's identity is proven, so a signal reaches the proven
/// process or none (AC-H103); a pidfd that cannot be opened leaves the process unverified (F3).
/// On macOS it is the pid, proven again right before each signal (a window of microseconds,
/// §A14).
struct Pinned {
    pid: u32,
    #[cfg(target_os = "linux")]
    fd: std::os::fd::OwnedFd,
}
impl Pinned {
    fn open(pid: u32) -> Option<Self> {
        Some(Self {
            pid,
            #[cfg(target_os = "linux")]
            fd: crate::proc_ident::pidfd(pid)?,
        })
    }
    /// Whether the pinned process has not exited. Linux: its pidfd is not yet readable (it
    /// polls readable once the process exits, a zombie included); an error is an exit (fail
    /// closed). Elsewhere there is nothing to poll, and the caller proves the pid again by its
    /// start token.
    fn alive(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            let mut p = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd, no wait.
            unsafe { libc::poll(&mut p, 1, 0) == 0 }
        }
        #[cfg(not(target_os = "linux"))]
        {
            true
        }
    }
    /// Signals the process group the pinned process leads (F8). Linux 6.9+: through the pidfd
    /// with `PIDFD_SIGNAL_PROCESS_GROUP`, so the signal reaches the group of the pinned
    /// process, never a group that reused its number. On an older kernel (`EINVAL`) and on
    /// macOS: by number, after the caller checked the leader alive and its parent (the worker)
    /// still stopped (§A14 residual). A group already gone is not an error.
    fn send_group(&self, sig: rustix::process::Signal) -> std::io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            const PIDFD_SIGNAL_PROCESS_GROUP: libc::c_uint = 1 << 2;
            // SAFETY: a pidfd we own, a signal number, no siginfo, and a flag word.
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    self.fd.as_raw_fd(),
                    sig.as_raw(),
                    std::ptr::null::<libc::siginfo_t>(),
                    PIDFD_SIGNAL_PROCESS_GROUP,
                )
            };
            if rc == 0 {
                return Ok(());
            }
            let e = std::io::Error::last_os_error();
            match e.raw_os_error() {
                Some(libc::ESRCH) => return Ok(()),
                // Before Linux 6.9 the flag is unknown.
                Some(libc::EINVAL) => (),
                _ => {
                    return Err(std::io::Error::new(
                        e.kind(),
                        format!("cannot signal omp process group {}: {e}", self.pid),
                    ));
                }
            }
        }
        rpc::kill_group(self.pid, sig)
    }
    /// SIGKILL; `Ok(false)` when the process had already exited, so nothing was signalled.
    #[cfg(test)]
    fn kill(&self) -> std::io::Result<bool> {
        self.send(rustix::process::Signal::KILL)
    }
    /// `Ok(false)` when the process had already exited, so nothing was signalled.
    fn send(&self, sig: rustix::process::Signal) -> std::io::Result<bool> {
        use rustix::io::Errno;
        #[cfg(target_os = "linux")]
        let sent = rustix::process::pidfd_send_signal(&self.fd, sig);
        #[cfg(not(target_os = "linux"))]
        let sent = {
            let p = rpc::signal_pid(self.pid)
                .ok_or_else(|| std::io::Error::other(format!("invalid worker pid {}", self.pid)))?;
            rustix::process::kill_process(p, sig)
        };
        match sent {
            Ok(()) => Ok(true),
            Err(Errno::SRCH) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}
/// What `--force` must prove from the kernel before each signal (S-09), a seam for tests.
struct Facts<'a> {
    /// The worker `pid` runs this glen executable as `__worker <origin_job_id>` (AC-H101).
    worker: &'a dyn Fn(u32, &str) -> bool,
    /// The recorded omp `(worker, omp)`: omp's parent is the worker and it leads its own
    /// process group (AC-H102).
    omp: &'a dyn Fn(u32, u32) -> bool,
    /// The worker `pid` is in the stopped state, not merely sent SIGSTOP (F8).
    stopped: &'a dyn Fn(u32) -> bool,
    /// A pinned process has not exited ([`Pinned::alive`]).
    alive: &'a dyn Fn(&Pinned) -> bool,
}
const KERNEL: Facts<'static> = Facts {
    worker: &is_glen_worker,
    omp: &is_workers_omp,
    stopped: &crate::proc_ident::stopped,
    alive: &Pinned::alive,
};
/// How long `--force` waits for a SIGSTOPped worker to be seen stopped (F8).
const STOP_BOUND: Duration = Duration::from_millis(200);
/// Whether the pinned worker is seen stopped within [`STOP_BOUND`].
fn confirmed_stop(facts: &Facts<'_>, worker: &Pinned) -> bool {
    let end = Instant::now() + STOP_BOUND;
    loop {
        if (facts.alive)(worker) && (facts.stopped)(worker.pid) {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn is_glen_worker(pid: u32, origin: &str) -> bool {
    let Some(argv) = crate::proc_ident::argv(pid) else {
        return false;
    };
    let job = |a: &std::ffi::OsString| a.to_str().and_then(JobId::parse);
    crate::proc_ident::runs_current_exe(pid)
        && argv.len() == 3
        && argv[1] == "__worker"
        && job(&argv[2]).is_some_and(|id| Some(id) == JobId::parse(origin))
}
fn is_workers_omp(worker: u32, omp: u32) -> bool {
    crate::proc_ident::ppid(omp) == Some(worker) && crate::proc_ident::pgid(omp) == Some(omp)
}
fn force(home: &GlenHome, t: &Indexed, facts: &Facts<'_>) -> std::io::Result<(&'static str, bool)> {
    force_with(home, &t.entry, facts, &signal, FORCE_BOUND)
}
/// SIGKILL the omp group and the worker, but only when every recorded process is proven by pid
/// and start token (binding J11) and by what it runs (S-09): the worker is this glen executable
/// as `__worker <origin_job_id>`, and omp is the worker's child leading its own group, both
/// proven before the first signal and again right before each one. The order is: stop the
/// proven worker (so omp's pid stays its zombie child at worst), prove omp again and kill its
/// group, then kill the worker (F3). An `Unknown` or unproven
/// process is never signalled, and a failed signal or a worker still alive after `bound` falls
/// to `release_timeout` with the registry kept. `forced` only once the worker is proven dead,
/// the recorded omp group is gone, whether or not its leader was alive to be signalled, and its
/// files are removed (see [`cleaned`]).
fn force_with(
    home: &GlenHome,
    e: &warm::Entry,
    facts: &Facts<'_>,
    kill: &dyn Fn(Kill, Option<&Pinned>) -> std::io::Result<()>,
    bound: Duration,
) -> std::io::Result<(&'static str, bool)> {
    let identity = warm::kill_identity(e);
    match identity.worker {
        Liveness::Dead => return cleaned(home, e, "cleaned"),
        Liveness::Unknown => return Ok(("timeout", false)),
        Liveness::Alive => (),
    }
    // AC-H104: a registry that does not name the worker's argv job cannot be proven.
    let Some(origin) = e.origin_job_id.as_deref() else {
        return Ok((UNVERIFIED, false));
    };
    // AC-H103: pinned before the proof below; on Linux a worker that cannot be pinned is never
    // signalled (F3).
    let Some(worker) = Pinned::open(e.pid) else {
        return Ok((UNVERIFIED, false));
    };
    if !(facts.worker)(e.pid, origin) {
        return Ok((UNVERIFIED, false));
    }
    // Only a proven omp leader's group is signalled: once the leader is gone its pid may be
    // reused, so its group cannot be proven omp's. An unproven leader blocks the kill. The
    // recorded group must be gone either way, since omp's descendants can outlive their leader.
    let (signalled, recorded) = match (e.omp_pid, identity.omp) {
        (_, Some(Liveness::Unknown)) => return Ok(("timeout", false)),
        (Some(pid), Some(Liveness::Alive)) => (Some(pid), Some(pid)),
        (pid, _) => (None, pid),
    };
    // F8: omp is pinned before its proof too, so its group is signalled through it.
    let omp = match signalled {
        Some(pid) => match Pinned::open(pid) {
            Some(p) => Some(p),
            None => return Ok((UNVERIFIED, false)),
        },
        None => None,
    };
    // AC-H102: before the first signal.
    if signalled.is_some_and(|pid| !(facts.omp)(e.pid, pid)) {
        return Ok((UNVERIFIED, false));
    }
    // S-09: each identity is proven again right before its own signal, so a process that ended
    // since the first check (its pid now free for reuse) is never signalled.
    let worker_now = || match warm::kill_identity(e).worker {
        Liveness::Alive if (facts.worker)(e.pid, origin) => Ok(true),
        Liveness::Alive => Err(UNVERIFIED),
        Liveness::Dead => Ok(false),
        Liveness::Unknown => Err("timeout"),
    };
    let mut stopped = false;
    if let Some(pid) = signalled {
        // F3: the worker is stopped first, so it cannot reap omp: omp's pid, and with it the
        // group id, stays pinned (as a zombie at worst) while omp is proven again and its group
        // signalled. A worker gone by now leaves omp unprovable, which the proof below says.
        match worker_now() {
            Ok(true) => {
                if kill(Kill::Stop(e.pid), Some(&worker)).is_err() {
                    return Ok(("timeout", false));
                }
                stopped = true;
            }
            Ok(false) => (),
            Err(outcome) => return Ok((outcome, false)),
        }
        // A stopped worker whose kill is then not proven is resumed, untouched otherwise.
        let give_up = |outcome: &'static str| {
            if stopped {
                let _ = kill(Kill::Resume(e.pid), Some(&worker));
            }
            Ok((outcome, false))
        };
        // F8: SIGSTOP sent is not a worker stopped (it may be resumed by its own uid): unless it
        // is seen stopped, omp's pid is not pinned, so nothing more is signalled.
        if stopped && !confirmed_stop(facts, &worker) {
            return give_up(UNVERIFIED);
        }
        let omp = omp.as_ref().expect("pinned with the first omp proof");
        match warm::kill_identity(e).omp {
            // F8: an omp whose pidfd says it exited is a leader gone, so its group number is
            // not signalled; the recorded group must still be gone below.
            Some(Liveness::Alive) if !(facts.alive)(omp) => (),
            Some(Liveness::Alive) if (facts.omp)(e.pid, pid) => {
                // Right before the group signal, which falls back to the group's number on
                // macOS and before Linux 6.9: the worker must still be stopped, so omp cannot
                // have been reaped and its number reused.
                if stopped && !((facts.alive)(&worker) && (facts.stopped)(e.pid)) {
                    return give_up(UNVERIFIED);
                }
                if kill(Kill::Group(pid), Some(omp)).is_err() {
                    return give_up("timeout");
                }
            }
            Some(Liveness::Alive) => return give_up(UNVERIFIED),
            // A leader gone by then is skipped; its group must still be gone below.
            Some(Liveness::Dead) => (),
            _ => return give_up("timeout"),
        }
    }
    // A worker gone by now needs no kill.
    match worker_now() {
        Ok(true) => {
            if kill(Kill::Process(e.pid), Some(&worker)).is_err() {
                return Ok(("timeout", false));
            }
        }
        Ok(false) => (),
        Err(outcome) => {
            if stopped {
                let _ = kill(Kill::Resume(e.pid), Some(&worker));
            }
            return Ok((outcome, false));
        }
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
    /// Facts that prove any process: these tests are about the pid and start-token proofs.
    const TRUSTING: Facts<'static> = Facts {
        worker: &|_, _| true,
        omp: &|_, _| true,
        stopped: &|_| true,
        alive: &|_| true,
    };
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
            origin_job_id: Some("01a18bcc-6970-7685-8f03-0ecc16977273".into()),
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
        let (outcome, ok) =
            release_one(&env, &h, &entries[0], &args, warm::STALE, &TRUSTING).unwrap();
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
        assert_eq!(force(&h, &indexed, &KERNEL).unwrap(), ("timeout", false));
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
                &TRUSTING,
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
                &TRUSTING,
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
                &TRUSTING,
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
        let never =
            |k: Kill, _: Option<&Pinned>| -> std::io::Result<()> { panic!("signalled {k:?}") };
        assert_eq!(
            force_with(&h, &e, &TRUSTING, &never, Duration::ZERO).unwrap(),
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
        let refused = |_: Kill, _: Option<&Pinned>| -> std::io::Result<()> {
            Err(std::io::ErrorKind::PermissionDenied.into())
        };
        assert_eq!(
            force_with(&h, &e, &TRUSTING, &refused, Duration::ZERO).unwrap(),
            ("timeout", false)
        );
        assert_eq!(warm::read(&h, A).unwrap().entry, e);
        // A signal that reported success but left the worker (this process) running.
        let sent = std::cell::Cell::new(vec![]);
        let ignored = |k: Kill, _: Option<&Pinned>| -> std::io::Result<()> {
            let mut all = sent.take();
            all.push(k);
            sent.set(all);
            Ok(())
        };
        assert_eq!(
            force_with(&h, &e, &TRUSTING, &ignored, Duration::from_millis(100)).unwrap(),
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
            force_with(&h, &e, &TRUSTING, &signal, FORCE_BOUND).unwrap(),
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
        let recorded = |k: Kill, p: Option<&Pinned>| -> std::io::Result<()> {
            let mut all = sent.take();
            all.push(k);
            sent.set(all);
            signal(k, p)
        };
        assert_eq!(
            force_with(&h, &e, &TRUSTING, &recorded, Duration::from_millis(300)).unwrap(),
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
            force_with(&h, &e, &TRUSTING, &signal, FORCE_BOUND).unwrap(),
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
                &TRUSTING,
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
                &TRUSTING,
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
                &TRUSTING,
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
            force_with(&h, &live, &TRUSTING, &signal, FORCE_BOUND).unwrap(),
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
            release_one(
                &env,
                &h,
                &dead_b,
                &args(false, Duration::ZERO),
                warm::STALE,
                &TRUSTING
            )
            .unwrap(),
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
        let kill = |k: Kill, p: Option<&Pinned>| -> std::io::Result<()> {
            sent.borrow_mut().push(k);
            signal(k, p)?;
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
            force_with(&h, &e, &TRUSTING, &kill, FORCE_BOUND).unwrap(),
            ("forced", true)
        );
        assert_eq!(
            *sent.borrow(),
            [Kill::Stop(worker_pid), Kill::Group(omp_pid)],
            "no signal after the group's"
        );
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
                &TRUSTING,
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
            &TRUSTING,
        );
        assert_eq!(got.unwrap(), ("cleaned", true));
        assert!(warm::read(&h, A).is_none());
    }
    /// Records every signal `force_with` asks for, and sends it.
    fn recorder(
        sent: &std::cell::RefCell<Vec<Kill>>,
    ) -> impl Fn(Kill, Option<&Pinned>) -> std::io::Result<()> {
        move |k, p| {
            sent.borrow_mut().push(k);
            signal(k, p)
        }
    }
    /// AC-H101: a live process proven by pid and start token is still never signalled unless
    /// it runs this glen executable as `__worker <origin_job_id>`; the kernel facts reject a
    /// `sleep`, and the row is `timeout` with `reason:"unverified_process"`.
    #[test]
    fn s09_force_never_signals_a_process_that_is_not_the_glen_worker() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let (mut child, e) = sleeper();
        warm::publish(&h, &e).unwrap();
        assert!(!is_glen_worker(e.pid, e.origin_job_id.as_deref().unwrap()));
        let sent = std::cell::RefCell::new(vec![]);
        assert_eq!(
            force_with(&h, &e, &KERNEL, &recorder(&sent), FORCE_BOUND).unwrap(),
            (UNVERIFIED, false)
        );
        assert!(sent.borrow().is_empty(), "nothing signalled");
        assert!(child.try_wait().unwrap().is_none(), "alive");
        assert_eq!(warm::read(&h, A).unwrap().entry, e, "the registry is kept");
        let doc = row(&warm::read(&h, A).unwrap(), "stale", UNVERIFIED);
        assert_eq!(doc["outcome"], "timeout");
        assert_eq!(doc["reason"], "unverified_process");
        // This process does run the glen executable, but not as `__worker <origin>`.
        assert!(!is_glen_worker(
            std::process::id(),
            "01a18bcc-6970-7685-8f03-0ecc16977273"
        ));
        child.kill().unwrap();
        child.wait().unwrap();
    }
    /// AC-H102: the omp group is signalled only once omp is the worker's child and leads its
    /// own group, proven before the first signal: a failed omp proof signals nothing at all,
    /// not even the proven worker.
    #[test]
    fn s09_an_omp_that_is_not_the_workers_child_blocks_every_signal() {
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
        let omp = std::cell::RefCell::new(omp);
        let (mut worker, mut e) = sleeper();
        e.omp_pid = Some(omp_pid);
        e.omp_start = Some(omp_start);
        warm::publish(&h, &e).unwrap();
        // The real omp fact: this "omp" is the test process's child, not the worker's.
        assert!(!is_workers_omp(e.pid, omp_pid));
        assert!(is_workers_omp(std::process::id(), omp_pid));
        let asked = std::cell::RefCell::new(vec![]);
        let facts = Facts {
            worker: &|_, _| true,
            omp: &|w, o| {
                asked.borrow_mut().push((w, o));
                is_workers_omp(w, o)
            },
            ..TRUSTING
        };
        let sent = std::cell::RefCell::new(vec![]);
        assert_eq!(
            force_with(&h, &e, &facts, &recorder(&sent), FORCE_BOUND).unwrap(),
            (UNVERIFIED, false)
        );
        assert_eq!(*asked.borrow(), [(e.pid, omp_pid)]);
        assert!(sent.borrow().is_empty(), "no signal before both proofs");
        assert!(worker.try_wait().unwrap().is_none());
        assert_eq!(warm::read(&h, A).unwrap().entry, e);
        // With both facts proven, the group then the worker are signalled.
        let trusted = Facts {
            stopped: &crate::proc_ident::stopped,
            alive: &Pinned::alive,
            ..TRUSTING
        };
        // The group is gone only once its killed leader (this test's child) is reaped.
        let reaping = |k: Kill, p: Option<&Pinned>| -> std::io::Result<()> {
            recorder(&sent)(k, p)?;
            if k == Kill::Group(omp_pid) {
                omp.borrow_mut().wait()?;
            }
            Ok(())
        };
        assert_eq!(
            force_with(&h, &e, &trusted, &reaping, FORCE_BOUND).unwrap(),
            ("forced", true)
        );
        // F3: the worker is stopped before omp is proven again and its group signalled.
        assert_eq!(
            *sent.borrow(),
            [
                Kill::Stop(e.pid),
                Kill::Group(omp_pid),
                Kill::Process(e.pid)
            ]
        );
        assert!(!worker.wait().unwrap().success());
    }
    /// Whether `pid` reaches the stopped (`ps` state `T`) or running state `stopped` within 5s:
    /// a signal is delivered once its target is scheduled.
    fn reaches(pid: u32, stopped: bool) -> bool {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let o = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            if String::from_utf8_lossy(&o.stdout).contains('T') == stopped {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    /// F3: when omp's second proof (after the worker is stopped) fails, the omp group is never
    /// signalled and the stopped worker is resumed, alive and untouched otherwise.
    #[test]
    fn f3_a_failed_second_omp_proof_resumes_the_stopped_worker() {
        use std::os::unix::process::CommandExt;
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let mut omp = std::process::Command::new("/bin/sleep")
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
        let proofs = std::cell::Cell::new(0);
        let stopped_at_second = std::cell::Cell::new(None);
        let facts = Facts {
            worker: &|_, _| true,
            omp: &|w, _| {
                proofs.set(proofs.get() + 1);
                if proofs.get() == 2 {
                    stopped_at_second.set(Some(reaches(w, true)));
                }
                proofs.get() == 1
            },
            stopped: &crate::proc_ident::stopped,
            alive: &Pinned::alive,
        };
        let sent = std::cell::RefCell::new(vec![]);
        assert_eq!(
            force_with(&h, &e, &facts, &recorder(&sent), FORCE_BOUND).unwrap(),
            (UNVERIFIED, false)
        );
        assert_eq!(
            stopped_at_second.get(),
            Some(true),
            "omp re-proven under a stopped worker"
        );
        assert_eq!(*sent.borrow(), [Kill::Stop(e.pid), Kill::Resume(e.pid)]);
        assert!(worker.try_wait().unwrap().is_none(), "the worker lives");
        assert!(reaches(e.pid, false), "and runs again");
        assert!(omp.try_wait().unwrap().is_none(), "omp was never signalled");
        assert_eq!(warm::read(&h, A).unwrap().entry, e, "the registry is kept");
        worker.kill().unwrap();
        worker.wait().unwrap();
    }
    /// F3 (Linux): a worker that cannot be pinned by a pidfd is never signalled, rather than
    /// signalled by number. A thread id passes the pid and start-token proofs but
    /// `pidfd_open` refuses it.
    #[cfg(target_os = "linux")]
    #[test]
    fn f3_a_worker_without_a_pidfd_is_never_signalled() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let (tid_tx, tid_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            // SAFETY: gettid has no preconditions.
            tid_tx.send(unsafe { libc::gettid() } as u32).unwrap();
            let _ = done_rx.recv();
        });
        let tid = tid_rx.recv().unwrap();
        let crate::jobs::liveness::Probe::Running(start) = crate::jobs::liveness::start_token(tid)
        else {
            panic!("no start token for the thread")
        };
        let e = entry(A, tid, Some(start));
        assert_eq!(warm::kill_identity(&e).worker, Liveness::Alive);
        assert!(Pinned::open(tid).is_none(), "pidfd_open refuses a thread");
        warm::publish(&h, &e).unwrap();
        let never =
            |k: Kill, _: Option<&Pinned>| -> std::io::Result<()> { panic!("signalled {k:?}") };
        assert_eq!(
            force_with(&h, &e, &TRUSTING, &never, FORCE_BOUND).unwrap(),
            (UNVERIFIED, false)
        );
        done_tx.send(()).unwrap();
        thread.join().unwrap();
    }
    /// AC-H104: a registry without `origin_job_id` (written before it existed, or forged) is
    /// never force-killed, whatever else is proven.
    #[test]
    fn s09_a_registry_without_its_origin_job_is_never_force_killed() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let (mut child, mut e) = sleeper();
        e.origin_job_id = None;
        warm::publish(&h, &e).unwrap();
        let sent = std::cell::RefCell::new(vec![]);
        assert_eq!(
            force_with(&h, &e, &TRUSTING, &recorder(&sent), FORCE_BOUND).unwrap(),
            (UNVERIFIED, false)
        );
        assert!(sent.borrow().is_empty());
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }
    /// AC-H103: the worker is pinned before its proof and signalled through that pin. When it
    /// exits (and is reaped, so its pid is free for reuse) between the proof and the signal,
    /// the signal reaches nothing. On Linux the pin is a pidfd, so this holds even if the pid
    /// were reused; elsewhere the pid is proven again right before the signal.
    #[test]
    fn s09_a_worker_that_exits_between_proof_and_signal_is_never_signalled() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let (child, e) = sleeper();
        warm::publish(&h, &e).unwrap();
        #[cfg(target_os = "linux")]
        assert!(Pinned::open(e.pid).is_some(), "a pidfd on Linux");
        let child = std::cell::RefCell::new(child);
        let outcomes = std::cell::RefCell::new(vec![]);
        let exits_first = |k: Kill, p: Option<&Pinned>| -> std::io::Result<()> {
            let mut c = child.borrow_mut();
            c.kill()?;
            c.wait()?;
            assert_eq!(k, Kill::Process(e.pid));
            outcomes
                .borrow_mut()
                .push(p.expect("the pinned worker").kill()?);
            Ok(())
        };
        assert_eq!(
            force_with(&h, &e, &TRUSTING, &exits_first, FORCE_BOUND).unwrap(),
            ("forced", true)
        );
        assert_eq!(*outcomes.borrow(), [false], "the signal reached nothing");
    }
    /// A worker (`sleeper`) and its recorded omp: a `sleep` leading its own process group.
    fn worker_and_omp(h: &GlenHome) -> (std::process::Child, std::process::Child, warm::Entry) {
        use std::os::unix::process::CommandExt;
        let omp = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let crate::jobs::liveness::Probe::Running(omp_start) =
            crate::jobs::liveness::start_token(omp.id())
        else {
            panic!("no start token for omp")
        };
        let (worker, mut e) = sleeper();
        e.omp_pid = Some(omp.id());
        e.omp_start = Some(omp_start);
        warm::publish(h, &e).unwrap();
        (worker, omp, e)
    }
    /// F8: a SIGSTOP the worker is never seen to obey (it could be resumed by its own uid, so
    /// omp's pid is not pinned) signals nothing more: the worker is resumed, omp's group is
    /// never signalled, and the row is `unverified_process`.
    #[test]
    fn f8_an_unconfirmed_stop_resumes_the_worker_and_signals_nothing_more() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let (mut worker, mut omp, e) = worker_and_omp(&h);
        let _cleanup = Group(omp.id());
        let facts = Facts {
            stopped: &|_| false,
            ..TRUSTING
        };
        let sent = std::cell::RefCell::new(vec![]);
        assert_eq!(
            force_with(&h, &e, &facts, &recorder(&sent), FORCE_BOUND).unwrap(),
            (UNVERIFIED, false)
        );
        assert_eq!(*sent.borrow(), [Kill::Stop(e.pid), Kill::Resume(e.pid)]);
        assert!(reaches(e.pid, false), "the worker runs again");
        assert!(omp.try_wait().unwrap().is_none(), "omp was never signalled");
        assert_eq!(warm::read(&h, A).unwrap().entry, e, "the registry is kept");
        worker.kill().unwrap();
        worker.wait().unwrap();
    }
    /// F8: a worker seen stopped, then resumed (by its own uid) before omp's group is signalled,
    /// leaves omp reapable and its group number free for reuse: the group is never signalled.
    #[test]
    fn f8_a_worker_resumed_before_the_group_signal_blocks_it() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let (mut worker, mut omp, e) = worker_and_omp(&h);
        let _cleanup = Group(omp.id());
        let checks = std::cell::Cell::new(0);
        let facts = Facts {
            stopped: &|_| {
                checks.set(checks.get() + 1);
                checks.get() == 1
            },
            ..TRUSTING
        };
        let sent = std::cell::RefCell::new(vec![]);
        assert_eq!(
            force_with(&h, &e, &facts, &recorder(&sent), FORCE_BOUND).unwrap(),
            (UNVERIFIED, false)
        );
        assert_eq!(checks.get(), 2, "seen stopped, then checked again");
        assert_eq!(*sent.borrow(), [Kill::Stop(e.pid), Kill::Resume(e.pid)]);
        assert!(omp.try_wait().unwrap().is_none(), "omp was never signalled");
        worker.kill().unwrap();
        worker.wait().unwrap();
    }
    /// F8: an omp whose pin says it exited (Linux: its pidfd polls readable) is a leader gone:
    /// its group number is never signalled, the worker still is, and the release waits for the
    /// recorded group to be gone (here it is not: `timeout`).
    #[test]
    fn f8_an_omp_whose_pin_says_it_exited_is_never_group_signalled() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let (mut worker, mut omp, e) = worker_and_omp(&h);
        let omp_pid = omp.id();
        let _cleanup = Group(omp_pid);
        let facts = Facts {
            stopped: &crate::proc_ident::stopped,
            alive: &|p: &Pinned| p.pid != omp_pid,
            ..TRUSTING
        };
        let sent = std::cell::RefCell::new(vec![]);
        assert_eq!(
            force_with(&h, &e, &facts, &recorder(&sent), Duration::from_millis(100)).unwrap(),
            ("timeout", false)
        );
        assert_eq!(*sent.borrow(), [Kill::Stop(e.pid), Kill::Process(e.pid)]);
        assert!(omp.try_wait().unwrap().is_none(), "omp was never signalled");
        assert!(!worker.wait().unwrap().success());
    }
    /// F8: omp's group is signalled through omp's pin (Linux 6.9+: `PIDFD_SIGNAL_PROCESS_GROUP`;
    /// older kernels and macOS: by number), and the pin tells a live process from an exited one
    /// (Linux: a zombie's pidfd polls readable).
    #[test]
    fn f8_the_omp_pin_signals_its_group_and_sees_its_exit() {
        use std::os::unix::process::{CommandExt, ExitStatusExt};
        let mut leader = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let _cleanup = Group(leader.id());
        let pin = Pinned::open(leader.id()).expect("pinned");
        assert!(pin.alive());
        pin.send_group(rustix::process::Signal::KILL).unwrap();
        #[cfg(target_os = "linux")]
        {
            let end = Instant::now() + Duration::from_secs(5);
            while pin.alive() {
                assert!(Instant::now() < end, "the pidfd never saw the exit");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        assert_eq!(leader.wait().unwrap().signal(), Some(libc::SIGKILL));
        // A group already gone is not an error.
        pin.send_group(rustix::process::Signal::KILL).unwrap();
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
