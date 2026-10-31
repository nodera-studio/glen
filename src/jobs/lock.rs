use crate::{glen_home::GlenHome, omp::lease};
use rustix::{
    fs::{FlockOperation, Mode, OFlags, flock, open},
    io::Errno,
};
use serde::{Deserialize, Serialize};
use std::{
    io,
    os::fd::OwnedFd,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HolderKind {
    Job,
    Control,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Holder {
    pub v: u32,
    pub kind: HolderKind,
    pub job_id: Option<String>,
    pub pid: u32,
    pub start: Option<String>,
}
/// Record ownership without replacing the inode protected by flock.
pub fn mark(lock: &SessionLock, h: &Holder) -> io::Result<()> {
    let bytes = serde_json::to_vec(h)?;
    let contextual = |e: rustix::io::Errno| {
        io::Error::new(
            io::Error::from(e).kind(),
            format!("cannot mark holder in {}: {e}", lock.path.display()),
        )
    };
    rustix::fs::ftruncate(&lock.fd, 0).map_err(contextual)?;
    let mut offset = 0;
    while offset < bytes.len() {
        let n =
            rustix::io::pwrite(&lock.fd, &bytes[offset..], offset as u64).map_err(contextual)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("cannot write holder to {}", lock.path.display()),
            ));
        }
        offset += n;
    }
    Ok(())
}
/// Read only. The caller must separately verify that the lock and identity are live.
pub fn holder(home: &GlenHome, id: &str) -> Option<Holder> {
    use std::io::Read;
    let file = std::fs::File::open(lock_path(home, id)).ok()?;
    let mut bytes = Vec::new();
    file.take(16_385).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 16_384 {
        return None;
    }
    let h: Holder = serde_json::from_slice(&bytes).ok()?;
    (h.v == 1 && h.pid > 0).then_some(h)
}
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
/// A child spawned on another thread copies the fd table and shares this open file
/// description until its exec closes the O_CLOEXEC copy, so closing our fd alone can leave
/// the flock held. LOCK_UN releases it for every copy at once.
impl Drop for SessionLock {
    fn drop(&mut self) {
        let _ = flock(&self.fd, FlockOperation::Unlock);
    }
}
pub fn try_acquire(home: &GlenHome, id: &str) -> io::Result<Option<SessionLock>> {
    home.ensure_dir("run/locks")
        .map_err(|e| match crate::glen_home::take_refusal(e) {
            // SEC-3: keep the invalid_env refusal intact for `sandbox::map_home_write`.
            Ok(refusal) => io::Error::other(refusal),
            Err(e) => io::Error::new(
                e.kind(),
                format!(
                    "cannot create {}: {e}",
                    home.root.join("run/locks").display()
                ),
            ),
        })?;
    let path = lock_path(home, id);
    // SEC-3: never follow a planted symlink, and only truncate a regular file we own.
    let fd = open(
        &path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::CREATE | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|e| {
        io::Error::new(
            io::Error::from(e).kind(),
            format!("cannot open lock {}: {e}", path.display()),
        )
    })?;
    let stat = rustix::fs::fstat(&fd).map_err(|e| {
        io::Error::new(
            io::Error::from(e).kind(),
            format!("cannot stat lock {}: {e}", path.display()),
        )
    })?;
    let euid = rustix::process::geteuid().as_raw();
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
        || stat.st_uid != euid
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "refusing lock {}: not a regular file owned by the current uid {euid}",
                path.display()
            ),
        ));
    }
    match flock(&fd, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Some(SessionLock { fd, path })),
        Err(Errno::WOULDBLOCK) => Ok(None),
        Err(e) => Err(io::Error::other(format!(
            "cannot lock {}: {e}",
            path.display()
        ))),
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
    mut on_blocked: impl FnMut() -> io::Result<()>,
) -> io::Result<Acquired> {
    let started = Instant::now();
    let mut blocked = false;
    loop {
        if cancel() {
            return Ok(Acquired::Cancelled);
        }
        if let Some(lock) = try_acquire(home, id)? {
            return Ok(Acquired::Locked {
                lock,
                waited: started.elapsed(),
            });
        }
        if !blocked {
            on_blocked()?;
            blocked = true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(Acquired::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(200).min(left));
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
    match flock(&fd, FlockOperation::NonBlockingLockExclusive) {
        // Release the probe explicitly, as SessionLock's Drop does: a concurrently spawned
        // child may share this description until its exec.
        Ok(()) => {
            let _ = flock(&fd, FlockOperation::Unlock);
            false
        }
        Err(e) => e == Errno::WOULDBLOCK,
    }
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
            std::fs::metadata(&lock.path).unwrap().permissions().mode() & 0o777,
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
            acquire(&h, "abc", Instant::now(), &|| false, || Ok(())).unwrap(),
            Acquired::TimedOut
        ));
        assert!(matches!(
            acquire(
                &h,
                "abc",
                Instant::now() + Duration::from_secs(30),
                &|| true,
                || Ok(())
            )
            .unwrap(),
            Acquired::Cancelled
        ));
        drop(lock);
        assert!(!is_held(&h, "abc"));
        assert!(matches!(
            acquire(
                &h,
                "abc",
                Instant::now() + Duration::from_secs(30),
                &|| { false },
                || Ok(())
            )
            .unwrap(),
            Acquired::Locked { .. }
        ));
    }
    #[test]
    fn cycle3_blocked_callback_tracks_real_acquire_not_a_probe() {
        use std::cell::{Cell, RefCell};
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        assert!(!is_held(&h, "race"));
        let held = RefCell::new(None);
        let blocked = Cell::new(0);
        // Take the competing lock after any pre-check, just before the actual acquire.
        let result = acquire(
            &h,
            "race",
            Instant::now(),
            &|| {
                if held.borrow().is_none() {
                    *held.borrow_mut() = try_acquire(&h, "race").unwrap();
                }
                false
            },
            || {
                blocked.set(blocked.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        assert!(matches!(result, Acquired::TimedOut));
        assert_eq!(
            blocked.get(),
            1,
            "publish only on the first actual contention"
        );
        assert!(is_held(&h, "race"));
        drop(held);
        // A pre-check could have seen held, but the real acquire is now uncontended.
        let result = acquire(&h, "race", Instant::now(), &|| false, || {
            blocked.set(blocked.get() + 1);
            Ok(())
        })
        .unwrap();
        assert!(matches!(result, Acquired::Locked { .. }));
        assert_eq!(blocked.get(), 1, "no false queued marker after release");
    }
    #[test]
    fn passed_deadline_still_takes_an_uncontended_lock() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let result = acquire(&h, "free", Instant::now(), &|| false, || {
            panic!("an uncontended lock must not publish a blocked marker")
        })
        .unwrap();
        assert!(matches!(result, Acquired::Locked { .. }));
        assert!(is_held(&h, "free"));
        // Cancellation still wins over an uncontended lock and an expired deadline.
        assert!(matches!(
            acquire(&h, "other", Instant::now(), &|| true, || {
                panic!("cancelled acquire must not publish a blocked marker")
            })
            .unwrap(),
            Acquired::Cancelled
        ));
        assert!(!lock_path(&h, "other").exists());
    }
    #[test]
    fn sec3_symlinked_lock_is_refused_and_its_target_unchanged() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        h.ensure_dir("run/locks").unwrap();
        let victim = t.path().join("victim.txt");
        std::fs::write(&victim, b"VICTIM DATA").unwrap();
        std::os::unix::fs::symlink(&victim, lock_path(&h, "s")).unwrap();
        assert!(try_acquire(&h, "s").is_err());
        assert!(acquire(&h, "s", Instant::now(), &|| false, || Ok(())).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"VICTIM DATA");
        // A dangling symlink is not followed into a new file either.
        let absent = t.path().join("absent.txt");
        std::os::unix::fs::symlink(&absent, lock_path(&h, "d")).unwrap();
        assert!(try_acquire(&h, "d").is_err());
        assert!(!absent.exists());
    }
    #[test]
    fn sec3_non_regular_lock_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        h.ensure_dir("run/locks").unwrap();
        use std::os::unix::ffi::OsStrExt;
        let fifo = std::ffi::CString::new(lock_path(&h, "f").as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo` is a valid NUL-terminated path for the duration of the call.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let err = try_acquire(&h, "f").err().unwrap();
        assert!(err.to_string().contains("not a regular file"), "{err}");
    }
    #[test]
    fn sec3_untrusted_locks_dir_keeps_the_invalid_env_refusal() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        h.ensure_dir("run/locks").unwrap();
        std::fs::set_permissions(
            h.root.join("run/locks"),
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        let err = crate::glen_home::take_refusal(try_acquire(&h, "s").err().unwrap()).unwrap();
        assert_eq!(err.code, crate::error::ErrorCode::InvalidEnv);
        assert!(!lock_path(&h, "s").exists());
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

#[cfg(test)]
mod holder_tests {
    use super::*;
    #[test]
    fn holder_is_lazy_and_mark_keeps_the_locked_inode() {
        let t = tempfile::tempdir().unwrap();
        let home = GlenHome {
            root: t.path().join("glen"),
        };
        assert_eq!(holder(&home, "s"), None);
        assert!(!home.root.exists());
        let lock = try_acquire(&home, "s").unwrap().unwrap();
        let identity = crate::jobs::liveness::current().unwrap();
        for (kind, job_id) in [
            (HolderKind::Job, Some("job".into())),
            (HolderKind::Control, None),
        ] {
            let h = Holder {
                v: 1,
                kind,
                job_id,
                pid: identity.pid,
                start: Some(identity.start.clone()),
            };
            mark(&lock, &h).unwrap();
            assert_eq!(holder(&home, "s"), Some(h));
            assert!(is_held(&home, "s"));
            assert!(try_acquire(&home, "s").unwrap().is_none());
        }
        drop(lock);
        assert!(!is_held(&home, "s"));
        // A stale holder is deliberately retained; it is not evidence of ownership.
        assert!(holder(&home, "s").is_some());
        for bytes in ["bad", r#"{"v":2,"kind":"control","pid":1}"#] {
            std::fs::write(lock_path(&home, "s"), bytes).unwrap();
            assert_eq!(holder(&home, "s"), None);
        }
    }
    #[test]
    fn f9_release_does_not_wait_for_a_shared_descriptions_last_close() {
        // A child spawned on another thread holds a copy of the fd until its exec; a dup
        // shares the open file description the same way, deterministically.
        let t = tempfile::tempdir().unwrap();
        let home = GlenHome {
            root: t.path().join("glen"),
        };
        let lock = try_acquire(&home, "s").unwrap().unwrap();
        let child_copy = rustix::io::dup(&lock.fd).unwrap();
        drop(lock);
        assert!(!is_held(&home, "s"));
        let again = try_acquire(&home, "s").unwrap();
        assert!(again.is_some());
        drop(child_copy);
    }
}
