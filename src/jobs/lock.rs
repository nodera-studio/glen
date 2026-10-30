use crate::{glen_home::GlenHome, omp::lease};
use rustix::{
    fs::{FlockOperation, Mode, OFlags, flock, open},
    io::Errno,
};
use std::{
    io,
    os::fd::OwnedFd,
    path::PathBuf,
    time::{Duration, Instant},
};
pub fn lock_key(id: &str) -> String {
    lease::lease_name(id)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("h-{:016x}", xxhash_rust::xxh64::xxh64(id.as_bytes(), 0)))
}
pub fn lock_path(home: &GlenHome, id: &str) -> PathBuf {
    home.root
        .join("run/locks")
        .join(format!("{}.lock", lock_key(id)))
}
pub struct SessionLock {
    pub fd: OwnedFd,
    pub path: PathBuf,
}
pub fn try_acquire(home: &GlenHome, id: &str) -> io::Result<Option<SessionLock>> {
    home.ensure_dir("run/locks")?;
    let path = lock_path(home, id);
    let fd = open(
        &path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::CREATE,
        Mode::from_raw_mode(0o600),
    )?;
    match flock(&fd, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Some(SessionLock { fd, path })),
        Err(Errno::WOULDBLOCK) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub enum Acquired {
    Locked { lock: SessionLock, waited: Duration },
    TimedOut,
    Cancelled,
}
pub fn acquire(
    home: &GlenHome,
    id: &str,
    deadline: Instant,
    cancel: &dyn Fn() -> bool,
) -> io::Result<Acquired> {
    let started = Instant::now();
    loop {
        if cancel() {
            return Ok(Acquired::Cancelled);
        }
        if Instant::now() >= deadline {
            return Ok(Acquired::TimedOut);
        }
        if let Some(lock) = try_acquire(home, id)? {
            return Ok(Acquired::Locked {
                lock,
                waited: started.elapsed(),
            });
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
pub fn is_held(home: &GlenHome, id: &str) -> bool {
    let Ok(fd) = open(
        lock_path(home, id),
        OFlags::RDWR | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return false;
    };
    matches!(
        flock(&fd, FlockOperation::NonBlockingLockExclusive),
        Err(Errno::WOULDBLOCK)
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn home(t: &tempfile::TempDir) -> GlenHome {
        GlenHome {
            root: t.path().join("glen"),
        }
    }
    #[test]
    fn lazy_probe_keys_and_modes() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        assert!(!is_held(&h, "abc"));
        assert!(!h.root.exists());
        assert!(lock_key("a/b").starts_with("h-"));
        let id = "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de";
        assert_eq!(lock_key(id), id);
        let lock = try_acquire(&h, id).unwrap().unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(lock.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[test]
    fn exclusive_timeout_cancel_and_release() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let lock = try_acquire(&h, "abc").unwrap().unwrap();
        assert!(is_held(&h, "abc"));
        assert!(try_acquire(&h, "abc").unwrap().is_none());
        assert!(matches!(
            acquire(
                &h,
                "abc",
                Instant::now() + Duration::from_millis(300),
                &|| false
            )
            .unwrap(),
            Acquired::TimedOut
        ));
        assert!(matches!(
            acquire(&h, "abc", Instant::now() + Duration::from_secs(1), &|| true).unwrap(),
            Acquired::Cancelled
        ));
        drop(lock);
        assert!(!is_held(&h, "abc"));
        assert!(matches!(
            acquire(&h, "abc", Instant::now() + Duration::from_secs(1), &|| {
                false
            })
            .unwrap(),
            Acquired::Locked { .. }
        ));
    }
    #[test]
    fn lock_not_inherited_by_child() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let lock = try_acquire(&h, "abc").unwrap().unwrap();
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("2")
            .spawn()
            .unwrap();
        drop(lock);
        assert!(!is_held(&h, "abc"));
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
