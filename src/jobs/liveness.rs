use super::{JobError, JobId, JobStore, State, Status};
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
    match start_token(pid) {
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
    s.status = State::Lost;
    s.updated_at = jiff::Timestamp::now().to_string();
    s.error = Some(JobError {
        code: "lost".into(),
        message: "the glen worker exited without reporting a result".into(),
        hint: None,
        details: None,
    });
    store.write_status_locked(id, &s)?;
    Ok(s)
}
fn report_lost(
    id: &JobId,
    observed: Status,
    result: Result<Status, crate::error::GlenError>,
) -> Status {
    match result {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot mark job {} lost: {e}", id.handle());
            observed
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
