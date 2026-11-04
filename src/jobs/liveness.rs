use super::{JobError, JobId, JobStore, State, Status};
use crate::error::Warning;
use std::time::Duration;
#[derive(Debug)]
pub enum Probe {
    Running(String),
    Gone,
    Unknown(String),
}
#[cfg(target_os = "linux")]
pub fn start_token(pid: u32) -> Probe {
    let text = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Probe::Gone,
        Err(e) => return Probe::Unknown(e.to_string()),
    };
    let Some((_, fields)) = text.rsplit_once(')') else {
        return Probe::Unknown("malformed stat".into());
    };
    let f: Vec<_> = fields.split_whitespace().collect();
    if matches!(f.first(), Some(&"Z" | &"X")) {
        return Probe::Gone;
    }
    match f.get(19) {
        Some(t) => Probe::Running(format!("linux:{t}")),
        None => Probe::Unknown("missing starttime".into()),
    }
}
#[cfg(target_os = "macos")]
pub fn start_token(pid: u32) -> Probe {
    let Ok(pid) = i32::try_from(pid) else {
        return Probe::Unknown("invalid pid".into());
    };
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: info points to writable, correctly sized storage. proc_pidinfo writes at most size bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if n == size {
        // SAFETY: proc_pidinfo returned the full structure size.
        let info = unsafe { info.assume_init() };
        if info.pbi_status == libc::SZOMB {
            return Probe::Gone;
        }
        Probe::Running(format!(
            "macos:{}.{:06}",
            info.pbi_start_tvsec, info.pbi_start_tvusec
        ))
    } else {
        let e = std::io::Error::last_os_error();
        if n <= 0 && e.raw_os_error() == Some(libc::ESRCH) {
            Probe::Gone
        } else {
            Probe::Unknown(e.to_string())
        }
    }
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn start_token(_pid: u32) -> Probe {
    Probe::Unknown("unsupported platform".into())
}
/// Whether a pid gone from this process's view may still run elsewhere (S-04,
/// [`crate::sandbox::pids_hidden`]): set once per process by [`init`].
static PIDS_HIDDEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Records, from the process's own environment, whether pids may be hidden from it. glen's
/// entry point calls this before any command, the hidden worker included.
pub fn init(env: &crate::env::Env) {
    PIDS_HIDDEN.store(
        crate::sandbox::pids_hidden(env, cfg!(target_os = "linux")),
        std::sync::atomic::Ordering::Relaxed,
    );
}
#[cfg(test)]
thread_local! { static FORCE_PIDS_HIDDEN: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) }; }
/// Test-only seam: forces [`init`]'s answer for the current thread, so a unit test can act as a
/// process under a Codex marker on Linux on any host. Call with `None` before the test ends.
#[cfg(test)]
pub(crate) fn force_pids_hidden_for_test(value: Option<bool>) {
    FORCE_PIDS_HIDDEN.with(|f| f.set(value));
}
fn pids_hidden() -> bool {
    #[cfg(test)]
    if let Some(forced) = FORCE_PIDS_HIDDEN.with(|f| f.get()) {
        return forced;
    }
    PIDS_HIDDEN.load(std::sync::atomic::Ordering::Relaxed)
}
/// [`start_token`] as evidence for a liveness decision. Where pids may be hidden (S-04: a Codex
/// sandbox on Linux, possibly in its own pid namespace) a gone pid is `Unknown`, so no live
/// registry or socket is removed, no warm registry is cleaned and no job is persisted `lost`
/// on that alone.
pub fn observe(pid: u32) -> Probe {
    match start_token(pid) {
        Probe::Gone if pids_hidden() => {
            Probe::Unknown("the pid may run in another pid namespace (Codex sandbox)".into())
        }
        probe => probe,
    }
}
pub struct Identity {
    pub pid: u32,
    pub start: String,
}
pub fn current() -> Option<Identity> {
    let pid = std::process::id();
    if let Probe::Running(start) = start_token(pid) {
        Some(Identity { pid, start })
    } else {
        None
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    Dead,
    Unknown,
}
pub fn check(pid: u32, start: &str) -> Liveness {
    match observe(pid) {
        Probe::Running(t) if t == start => Liveness::Alive,
        Probe::Running(_) | Probe::Gone => Liveness::Dead,
        Probe::Unknown(_) => Liveness::Unknown,
    }
}
pub const SPAWN_GRACE: Duration = Duration::from_secs(10);
pub fn is_lost(s: &Status, now: jiff::Timestamp) -> bool {
    !s.status.is_terminal()
        && match (s.worker_pid, s.worker_start.as_deref()) {
            (Some(pid), Some(start)) => check(pid, start) == Liveness::Dead,
            (None, _) => s.updated_at.parse::<jiff::Timestamp>().is_ok_and(|t| {
                now.as_millisecond().saturating_sub(t.as_millisecond())
                    > SPAWN_GRACE.as_millis() as i64
            }),
            _ => false,
        }
}
pub fn mark_lost(store: &JobStore, id: &JobId, s: Status) -> Status {
    report_lost(id, s, try_mark_lost(store, id, false))
}
/// [`mark_lost`] for an observer whose `__worker` child (`child_pid`) exited: under the status
/// lock, a job a warm worker claimed meanwhile (its `worker_pid` names another process) or a
/// terminal one is returned unchanged; only then is `lost` written.
pub fn mark_lost_unless_claimed(store: &JobStore, id: &JobId, child_pid: u32, s: Status) -> Status {
    report_lost(id, s, try_mark_lost_unless_claimed(store, id, child_pid))
}
/// [`mark_lost_unless_claimed`] without the in-memory fallback: `Err` when the status cannot be
/// read or written, for a caller that reports the failure its own way (`glen watch`).
pub(crate) fn try_mark_lost_unless_claimed(
    store: &JobStore,
    id: &JobId,
    child_pid: u32,
) -> Result<Status, crate::error::GlenError> {
    let _lock = store.status_lock(id)?;
    let mut s = store.read_status(id)?;
    if s.status.is_terminal() || s.worker_pid.is_some_and(|p| p != child_pid) {
        return Ok(s);
    }
    s = lost(s);
    s.updated_at = jiff::Timestamp::now().to_string();
    store.write_status_locked(id, &s)?;
    Ok(s)
}
/// Re-read and publish under the worker's status lock; never reports to stderr.
pub(crate) fn try_mark_lost(
    store: &JobStore,
    id: &JobId,
    recheck: bool,
) -> Result<Status, crate::error::GlenError> {
    // The worker uses the same lock for every publication. Re-read while holding it.
    let _lock = store.status_lock(id)?;
    let mut s = store.read_status(id)?;
    if s.status.is_terminal() || (recheck && !is_lost(&s, jiff::Timestamp::now())) {
        return Ok(s);
    }
    s = lost(s);
    s.updated_at = jiff::Timestamp::now().to_string();
    store.write_status_locked(id, &s)?;
    Ok(s)
}
/// The warning code a reader attaches when it saw a dead worker but could not persist `lost`.
pub const LOST_WRITE_FAILED: &str = "lost_write_failed";
fn lost(mut s: Status) -> Status {
    s.status = State::Lost;
    s.error = Some(JobError {
        code: "lost".into(),
        message: "the glen worker exited without reporting a result".into(),
        hint: None,
        details: None,
    });
    s
}
/// A dead worker is reported as `lost` even when the job directory cannot be written (W8-ERR-2):
/// the terminal status lives only in memory and carries a `lost_write_failed` warning for the
/// caller's envelope, never a stderr line.
fn report_lost(
    id: &JobId,
    observed: Status,
    result: Result<Status, crate::error::GlenError>,
) -> Status {
    match result {
        Ok(s) => s,
        Err(e) => {
            let mut s = lost(observed);
            if !s.warnings.iter().any(|w| w.code == LOST_WRITE_FAILED) {
                s.warnings.push(Warning {
                    code: LOST_WRITE_FAILED.into(),
                    message: format!("cannot record job {} as lost: {}", id.handle(), e.message),
                });
            }
            s
        }
    }
}
pub(crate) fn refresh_quiet(
    store: &JobStore,
    id: &JobId,
    s: Status,
) -> Result<Status, crate::error::GlenError> {
    if is_lost(&s, jiff::Timestamp::now()) {
        try_mark_lost(store, id, true)
    } else {
        Ok(s)
    }
}
pub fn refresh(store: &JobStore, id: &JobId, s: Status) -> Status {
    let result = refresh_quiet(store, id, s.clone());
    report_lost(id, s, result)
}

/// A pid that is certainly not running (a reaped child of this test process).
#[cfg(test)]
pub(crate) fn tests_dead_pid() -> u32 {
    let mut c = std::process::Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{process::Command, time::Instant};
    #[test]
    fn own_start_token_stable() {
        let identity = current().unwrap();
        assert_eq!(identity.pid, std::process::id());
        let Probe::Running(token) = start_token(identity.pid) else {
            panic!("own process missing")
        };
        assert_eq!(token, identity.start);
    }
    #[test]
    fn mark_lost_unless_claimed_keeps_a_claimed_job() {
        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        let j = crate::jobs::tests::job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        let mut s = store.read_status(&id).unwrap();
        s.worker_pid = Some(4242);
        store.write_status(&id, &s).unwrap();
        // The claim is already on disk: the exited child (pid 7) is not the job's worker.
        let kept = mark_lost_unless_claimed(&store, &id, 7, s.clone());
        assert_eq!(kept.status, State::Queued);
        assert_eq!(store.read_status(&id).unwrap().status, State::Queued);
        let lost = mark_lost_unless_claimed(&store, &id, 4242, s.clone());
        assert_eq!(lost.status, State::Lost);
        assert_eq!(store.read_status(&id).unwrap().status, State::Lost);
        // A child that died before writing its pid is lost as before.
        let j = crate::jobs::tests::job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        let s = store.read_status(&id).unwrap();
        assert_eq!(
            mark_lost_unless_claimed(&store, &id, 7, s).status,
            State::Lost
        );
    }
    #[test]
    fn mismatched_start_is_dead() {
        let i = current().unwrap();
        assert_eq!(check(i.pid, "bogus start"), Liveness::Dead);
        assert_eq!(check(i.pid, &i.start), Liveness::Alive);
    }
    #[test]
    fn reaped_and_zombie_are_gone() {
        let mut child = Command::new("/bin/sleep").arg("0").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(matches!(start_token(pid), Probe::Gone));
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id();
        let end = Instant::now() + Duration::from_secs(3);
        // Probe without wait()/try_wait(): a zombie must count as gone before reaping.
        while !matches!(start_token(pid), Probe::Gone) {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait().unwrap();
    }
    #[test]
    fn lost_identity_and_spawn_grace_table() {
        let i = current().unwrap();
        let now = jiff::Timestamp::now();
        let mut s = Status {
            v: 1,
            status: State::Accepted,
            updated_at: now.to_string(),
            worker_pid: Some(i.pid),
            worker_start: Some(i.start),
            ..Status::default()
        };
        assert!(!is_lost(&s, now));
        s.worker_start = Some("reused".into());
        assert!(is_lost(&s, now));
        s.worker_pid = None;
        for (age, lost) in [(11, true), (2, false)] {
            s.updated_at = now
                .checked_sub(jiff::SignedDuration::from_secs(age))
                .unwrap()
                .to_string();
            assert_eq!(is_lost(&s, now), lost);
        }
        s.status = State::Completed;
        s.updated_at = "2000-01-01T00:00:00Z".into();
        assert!(!is_lost(&s, now));
        s.status = State::Queued;
        s.worker_pid = Some(i.pid);
        s.worker_start = None;
        assert!(!is_lost(&s, now));
    }
    #[test]
    fn review3_completed_publication_wins_over_stale_lost_reader() {
        let t = tempfile::tempdir().unwrap();
        let store = super::super::tests::store(t.path());
        let j = super::super::tests::job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        let mut stale = store.read_status(&id).unwrap();
        stale.status = State::Accepted;
        stale.worker_pid = Some(std::process::id());
        stale.worker_start = Some("dead identity".into());
        store.write_status(&id, &stale).unwrap();
        let lock = store.status_lock(&id).unwrap();
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| refresh(&store, &id, stale.clone()));
            let mut completed = stale.clone();
            completed.status = State::Completed;
            store.write_status_locked(&id, &completed).unwrap();
            drop(lock);
            assert_eq!(reader.join().unwrap().status, State::Completed);
        });
        assert_eq!(mark_lost(&store, &id, stale).status, State::Completed);
        assert_eq!(store.read_status(&id).unwrap().status, State::Completed);
    }
    #[test]
    fn review15_unknown_start_token_limitation_is_documented() {
        let now = jiff::Timestamp::now();
        let s = Status {
            status: State::Accepted,
            worker_pid: Some(u32::MAX),
            worker_start: None,
            updated_at: "2000-01-01T00:00:00Z".into(),
            ..Status::default()
        };
        assert!(!is_lost(&s, now));
        let design = include_str!("../../docs/design.md");
        assert!(design.contains("Unsupported platforms."));
        assert!(design.contains("readers conservatively cannot mark that job lost"));
    }
    #[test]
    fn refresh_persists_and_preserves_fields() {
        let t = tempfile::tempdir().unwrap();
        let store = super::super::tests::store(t.path());
        let j = super::super::tests::job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        let mut s = store.read_status(&id).unwrap();
        s.worker_pid = Some(std::process::id());
        s.worker_start = Some("wrong".into());
        s.omp_pid = Some(123);
        store.write_status(&id, &s).unwrap();
        let s = refresh(&store, &id, s);
        assert_eq!(s.status, State::Lost);
        assert_eq!(s.omp_pid, Some(123));
        assert_eq!(store.read_status(&id).unwrap().error.unwrap().code, "lost");
    }
}

#[cfg(test)]
mod quiet_tests {
    use super::*;
    #[test]
    fn quiet_refresh_returns_write_failure_and_preserves_completed_status() {
        let tree = tempfile::tempdir().unwrap();
        let store = super::super::tests::store(tree.path());
        let j = super::super::tests::job();
        let id = JobId::parse(&j.id).unwrap();
        let stale = Status {
            updated_at: "2000-01-01T00:00:00Z".into(),
            ..Status::default()
        };
        let err = refresh_quiet(&store, &id, stale.clone()).unwrap_err();
        assert!(err.message.contains("status.lock"));
        assert!(try_mark_lost(&store, &id, false).is_err());
        store.create(&j).unwrap();
        let mut completed = stale.clone();
        completed.status = State::Completed;
        store.write_status(&id, &completed).unwrap();
        assert_eq!(
            refresh_quiet(&store, &id, stale).unwrap().status,
            State::Completed
        );
        assert_eq!(
            try_mark_lost(&store, &id, false).unwrap().status,
            State::Completed
        );
    }
}

#[cfg(test)]
mod w8_err2_tests {
    use super::*;
    #[test]
    fn unpersistable_lost_is_reported_in_memory_with_one_warning() {
        let tree = tempfile::tempdir().unwrap();
        let store = super::super::tests::store(tree.path());
        let j = super::super::tests::job();
        let id = JobId::parse(&j.id).unwrap();
        // No job directory at all: the status lock cannot be opened, so `lost` cannot be written.
        let observed = Status {
            status: State::Accepted,
            updated_at: "2000-01-01T00:00:00Z".into(),
            worker_pid: Some(std::process::id()),
            worker_start: Some("dead identity".into()),
            omp_pid: Some(42),
            ..Status::default()
        };
        let s = refresh(&store, &id, observed.clone());
        assert_eq!(s.status, State::Lost);
        assert_eq!(s.error.as_ref().unwrap().code, "lost");
        assert_eq!(s.omp_pid, Some(42));
        assert_eq!(s.updated_at, observed.updated_at);
        assert_eq!(s.warnings.len(), 1);
        assert_eq!(s.warnings[0].code, LOST_WRITE_FAILED);
        assert!(
            s.warnings[0].message.contains(&id.handle()),
            "{:?}",
            s.warnings
        );
        assert!(
            s.warnings[0].message.contains("status.lock"),
            "{:?}",
            s.warnings
        );
        // Feeding the in-memory result back in does not stack a second warning.
        let again = mark_lost(&store, &id, s);
        assert_eq!(again.warnings.len(), 1);
    }
}

#[cfg(all(test, unix))]
mod s04_tests {
    use super::*;
    /// Resets the seam even when an assertion fails.
    struct Hidden;
    impl Hidden {
        fn on() -> Self {
            force_pids_hidden_for_test(Some(true));
            Hidden
        }
    }
    impl Drop for Hidden {
        fn drop(&mut self) {
            force_pids_hidden_for_test(None);
        }
    }
    /// S-04: where pids may be hidden, a gone pid is unknown: never dead, never lost on disk.
    /// A reused pid (a live process with another start token) is still dead.
    #[test]
    fn a_gone_pid_is_unknown_where_pids_may_be_hidden() {
        let gone = tests_dead_pid();
        force_pids_hidden_for_test(Some(false));
        assert!(matches!(observe(gone), Probe::Gone));
        assert_eq!(check(gone, "linux:1"), Liveness::Dead);
        let _hidden = Hidden::on();
        assert!(matches!(observe(gone), Probe::Unknown(_)));
        assert_eq!(check(gone, "linux:1"), Liveness::Unknown);
        let me = current().unwrap();
        assert_eq!(check(me.pid, &me.start), Liveness::Alive);
        assert_eq!(check(me.pid, "reused"), Liveness::Dead);
        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        let j = crate::jobs::tests::job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        let mut s = store.read_status(&id).unwrap();
        s.status = State::Accepted;
        s.updated_at = "2000-01-01T00:00:00Z".into();
        s.worker_pid = Some(gone);
        s.worker_start = Some("linux:1".into());
        store.write_status(&id, &s).unwrap();
        assert!(!is_lost(&s, jiff::Timestamp::now()));
        assert_eq!(refresh(&store, &id, s.clone()).status, State::Accepted);
        assert_eq!(store.read_status(&id).unwrap().status, State::Accepted);
    }
}
