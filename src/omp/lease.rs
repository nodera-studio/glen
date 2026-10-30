use serde::Serialize;
use std::path::{Path, PathBuf};
#[derive(Debug)]
pub enum Lease {
    Free,
    Held,
    Unknown(String),
}
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Idle,
    Live,
    Busy,
    Unknown,
}
pub fn lease_name(id: &str) -> Option<&str> {
    (!id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)))
    .then_some(id)
}
pub fn lock_path(owners: &Path, id: &str) -> Option<PathBuf> {
    Some(owners.join(format!("{}.lock", lease_name(id)?)))
}
pub fn memory_lock_name(path: &str) -> String {
    use xxhash_rust::xxh64::xxh64;
    format!(
        "omp-file-lock-{:016x}{:016x}",
        xxh64(path.as_bytes(), 0x4f4d_502d_4c4f_434b),
        xxh64(path.as_bytes(), 0x5049_2d46_494c_454c)
    )
}
#[cfg(unix)]
pub fn probe_flock(lock: &Path) -> Lease {
    use rustix::{
        fs::{FileType, FlockOperation, Mode, OFlags, flock, fstat, open},
        io::Errno,
    };
    let fd = match open(
        lock,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK | OFlags::NOCTTY,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(Errno::NOENT) => return Lease::Free,
        Err(e) => return Lease::Unknown(e.to_string()),
    };
    match fstat(&fd) {
        Ok(m) if FileType::from_raw_mode(m.st_mode) == FileType::RegularFile => {}
        Ok(_) => return Lease::Unknown("not a regular file".into()),
        Err(e) => return Lease::Unknown(e.to_string()),
    }
    match flock(&fd, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => match flock(&fd, FlockOperation::Unlock) {
            Ok(()) => Lease::Free,
            Err(e) => Lease::Unknown(e.to_string()),
        },
        Err(Errno::WOULDBLOCK) => Lease::Held,
        Err(e) => Lease::Unknown(e.to_string()),
    }
}
#[cfg(target_os = "linux")]
pub fn probe_abstract(path: &str) -> Lease {
    use rustix::{
        io::Errno,
        net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType, bind, socket_with},
    };
    let fd = match socket_with(
        AddressFamily::UNIX,
        SocketType::DGRAM,
        SocketFlags::CLOEXEC,
        None,
    ) {
        Ok(fd) => fd,
        Err(e) => return Lease::Unknown(e.to_string()),
    };
    let name = memory_lock_name(path);
    let addr = match SocketAddrUnix::new_abstract_name(name.as_bytes()) {
        Ok(a) => a,
        Err(e) => return Lease::Unknown(e.to_string()),
    };
    match bind(&fd, &addr) {
        Ok(()) => Lease::Free,
        Err(Errno::ADDRINUSE) => Lease::Held,
        Err(e) => Lease::Unknown(e.to_string()),
    }
}
pub fn probe(owners: &Path, id: &str) -> Lease {
    let Some(path) = lock_path(owners, id) else {
        return Lease::Unknown("session id is not a plain token".into());
    };
    #[cfg(target_os = "linux")]
    {
        match path.to_str() {
            Some(s) => probe_abstract(s),
            None => Lease::Unknown("non-UTF-8 lock path".into()),
        }
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        probe_flock(&path)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Lease::Unknown("unsupported platform".into())
    }
}
pub fn infer_state(lease: &Lease, in_flight: bool) -> State {
    match lease {
        Lease::Free => State::Idle,
        Lease::Held if in_flight => State::Busy,
        Lease::Held => State::Live,
        Lease::Unknown(_) => State::Unknown,
    }
}
pub fn state_detail(lease: &Lease) -> Option<String> {
    if let Lease::Unknown(s) = lease {
        Some(s.clone())
    } else {
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn golden_lock_names() {
        assert_eq!(
            memory_lock_name(
                "/home/fixture/.omp/run/session-owners/01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.lock"
            ),
            "omp-file-lock-fdf8456569e488c6109108ed93e83ae3"
        );
        assert_eq!(
            memory_lock_name("/x.lock"),
            "omp-file-lock-414147c7ad2644ab5392fc41faa97fcf"
        );
    }
    #[test]
    fn names_and_states() {
        for id in ["", "a/b", "..", &"a".repeat(129)] {
            assert!(lease_name(id).is_none())
        }
        assert_eq!(
            lock_path(Path::new("/owners"), "abc").unwrap(),
            Path::new("/owners/abc.lock")
        );
        for (l, b, w) in [
            (Lease::Free, true, State::Idle),
            (Lease::Held, true, State::Busy),
            (Lease::Held, false, State::Live),
            (Lease::Unknown("x".into()), false, State::Unknown),
        ] {
            assert_eq!(infer_state(&l, b), w)
        }
    }
    #[cfg(unix)]
    #[test]
    fn flock_is_read_only() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("x.lock");
        assert!(matches!(probe_flock(&p), Lease::Free));
        assert!(!p.exists());
        std::fs::write(&p, b"unchanged").unwrap();
        let f = std::fs::File::open(&p).unwrap();
        f.lock().unwrap();
        assert!(matches!(probe_flock(&p), Lease::Held));
        drop(f);
        assert!(matches!(probe_flock(&p), Lease::Free));
        assert_eq!(std::fs::read(&p).unwrap(), b"unchanged");
        assert!(matches!(probe_flock(t.path()), Lease::Unknown(_)));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn abstract_probe() {
        use std::os::linux::net::SocketAddrExt;
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("x.lock");
        let p = p.to_str().unwrap();
        let a = std::os::unix::net::SocketAddr::from_abstract_name(memory_lock_name(p)).unwrap();
        let s = std::os::unix::net::UnixDatagram::bind_addr(&a).unwrap();
        assert!(matches!(probe_abstract(p), Lease::Held));
        drop(s);
        assert!(matches!(probe_abstract(p), Lease::Free));
    }
}
