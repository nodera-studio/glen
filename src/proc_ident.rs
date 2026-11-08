//! Kernel facts about another process (design §A14): who holds the other end of a socket, how
//! confined it is, which image it runs, its argv and ancestry. A same-uid sandboxed writer can
//! forge any file under GLEN_HOME, but not these.
//!
//! The confinement-ordering rule ([`may_trust`]): an unconfined glen never hands work to, or
//! accepts work from, a peer more confined than itself. A confined glen allows it (it is an
//! add-only principal already, and two sandboxes cannot be ranked). An unknown answer on an
//! unconfined glen is refused.
use std::{
    ffi::OsString,
    io,
    os::fd::AsFd,
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// The kernel's account of a connected unix socket's peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    /// `None` when the peer's pid cannot be mapped into this process's pid namespace (the
    /// kernel reports 0).
    pub pid: Option<u32>,
    pub uid: u32,
}

/// The peer of a connected unix socket (Linux `SO_PEERCRED`, macOS `LOCAL_PEERPID` and
/// `getpeereid`). For a client socket the peer is the listener that `bind`/`listen`ed, even
/// before it `accept`s the connection.
pub fn peer_of(fd: impl AsFd) -> io::Result<Peer> {
    use std::os::fd::AsRawFd;
    let raw = fd.as_fd().as_raw_fd();
    #[cfg(target_os = "linux")]
    {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: cred and len point to writable storage of the size passed.
        let rc = unsafe {
            libc::getsockopt(
                raw,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&raw mut cred).cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Peer {
            pid: u32::try_from(cred.pid).ok().filter(|p| *p > 0),
            uid: cred.uid,
        })
    }
    #[cfg(target_os = "macos")]
    {
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        // SAFETY: pid and len point to writable storage of the size passed.
        let rc = unsafe {
            libc::getsockopt(
                raw,
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                (&raw mut pid).cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        let (mut uid, mut gid) = (0, 0);
        // SAFETY: uid and gid point to writable storage.
        if unsafe { libc::getpeereid(raw, &mut uid, &mut gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Peer {
            pid: u32::try_from(pid).ok().filter(|p| *p > 0),
            uid,
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = raw;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "peer credentials are not supported on this platform",
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confinement {
    Unconfined,
    /// Which confinement the process carries.
    Confined(&'static str),
    /// Why glen cannot tell.
    Unknown(String),
}

/// This process's own confinement, read once.
pub fn self_confinement() -> &'static Confinement {
    static SELF: OnceLock<Confinement> = OnceLock::new();
    SELF.get_or_init(|| own(std::process::id()))
}

/// The confinement `pid` carries beyond this process's. When this process is confined the
/// answer is only informative: [`may_trust`] does not rank two sandboxes.
pub fn confinement_of(pid: u32) -> Confinement {
    #[cfg(target_os = "linux")]
    {
        let base = own(pid);
        if base != Confinement::Unconfined {
            return base;
        }
        // Namespaces only rejoin with CAP_SYS_ADMIN in the target namespace: a peer in another
        // user, pid or mount namespace than this process is confined relative to it.
        for ns in ["user", "pid", "mnt"] {
            let mine = std::fs::read_link(format!("/proc/self/ns/{ns}"));
            let theirs = std::fs::read_link(format!("/proc/{pid}/ns/{ns}"));
            match (mine, theirs) {
                (Ok(a), Ok(b)) if a == b => (),
                (Ok(_), Ok(_)) => return Confinement::Confined("namespace"),
                (Err(e), _) | (_, Err(e)) => {
                    return Confinement::Unknown(format!("cannot read the {ns} namespace: {e}"));
                }
            }
        }
        Confinement::Unconfined
    }
    #[cfg(not(target_os = "linux"))]
    {
        own(pid)
    }
}

/// The confinement a process carries in itself: Linux no_new_privs or a seccomp filter (every
/// unprivileged seccomp, landlock or bwrap sandbox sets no_new_privs), macOS a Sandbox.kext
/// profile (`sandbox-exec`, seatbelt, App Sandbox). None of them can be removed.
#[cfg(target_os = "linux")]
fn own(pid: u32) -> Confinement {
    let status = match std::fs::read_to_string(format!("/proc/{pid}/status")) {
        Ok(s) => s,
        Err(e) => return Confinement::Unknown(format!("cannot read /proc/{pid}/status: {e}")),
    };
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .map(str::trim)
    };
    match (field("NoNewPrivs:"), field("Seccomp:")) {
        // A user namespace below the initial one (bwrap, rootless containers) maps only part of
        // the uid range; leaving it needs privilege the process lacks.
        (Some("0"), Some("0")) if !initial_user_ns(pid) => Confinement::Confined("user_namespace"),
        (Some("0"), Some("0")) => Confinement::Unconfined,
        (Some("1"), _) => Confinement::Confined("no_new_privs"),
        (Some(_), Some(s)) if s != "0" => Confinement::Confined("seccomp"),
        _ => Confinement::Unknown(format!(
            "/proc/{pid}/status has no NoNewPrivs or Seccomp field"
        )),
    }
}
/// Whether `pid` runs in the initial user namespace: its `uid_map` maps the full range
/// identically (`0 0 4294967295`). An unreadable map counts as initial: the namespace
/// comparison in [`confinement_of`] still applies to a peer.
#[cfg(target_os = "linux")]
fn initial_user_ns(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/uid_map")).map_or(true, |m| {
        m.split_whitespace().collect::<Vec<_>>() == ["0", "0", "4294967295"]
    })
}
#[cfg(target_os = "macos")]
fn own(pid: u32) -> Confinement {
    sandbox_answer(pid, sandbox_check(pid))
}
#[cfg(target_os = "macos")]
fn sandbox_answer(pid: u32, rc: Option<i32>) -> Confinement {
    match rc {
        Some(0) => Confinement::Unconfined,
        Some(1) => Confinement::Confined("sandbox"),
        Some(rc) => Confinement::Unknown(format!(
            "sandbox_check({pid}) returned {rc}: {}",
            io::Error::last_os_error()
        )),
        None => Confinement::Unknown("sandbox_check is not available".into()),
    }
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn own(_pid: u32) -> Confinement {
    Confinement::Unknown("confinement is not detectable on this platform".into())
}

/// libSystem's private `int sandbox_check(pid_t, const char *operation, int type, ...)`, looked
/// up at run time so a macOS without it makes glen answer `Unknown`, never fail to load. With
/// no operation it answers whether the process has a sandbox profile at all: 0 no, 1 yes (also
/// for a pid that no longer exists), -1 on error.
#[cfg(target_os = "macos")]
fn sandbox_check(pid: u32) -> Option<i32> {
    type SandboxCheck =
        unsafe extern "C" fn(libc::pid_t, *const libc::c_char, libc::c_int, ...) -> libc::c_int;
    static SYMBOL: OnceLock<Option<SandboxCheck>> = OnceLock::new();
    let f = (*SYMBOL.get_or_init(|| {
        // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name only looks the symbol up.
        let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"sandbox_check".as_ptr()) };
        // SAFETY: a non-null `sandbox_check` from libSystem has this C signature.
        (!p.is_null()).then(|| unsafe { std::mem::transmute::<*mut libc::c_void, SandboxCheck>(p) })
    }))?;
    let pid = libc::pid_t::try_from(pid).ok()?;
    // SAFETY: a null operation with type 0 (SANDBOX_FILTER_NONE) takes no variadic arguments.
    Some(unsafe { f(pid, std::ptr::null(), 0) })
}

/// The confinement-ordering rule for a socket peer: `Ok` when an endpoint run by `peer` may
/// receive work from this process, otherwise why not.
pub fn may_trust(peer: &Peer) -> Result<(), String> {
    may_trust_with(peer, self_confinement(), confinement_of)
}
fn may_trust_with(
    peer: &Peer,
    me: &Confinement,
    of: impl Fn(u32) -> Confinement,
) -> Result<(), String> {
    let euid = rustix::process::geteuid().as_raw();
    if peer.uid != euid {
        return Err(format!(
            "the endpoint runs as uid {}, not as this user ({euid})",
            peer.uid
        ));
    }
    if matches!(me, Confinement::Confined(_)) {
        return Ok(());
    }
    let Some(pid) = peer.pid else {
        return Err("the endpoint process is outside this process's pid namespace".into());
    };
    match of(pid) {
        Confinement::Unconfined => Ok(()),
        Confinement::Confined(what) => Err(format!(
            "the endpoint process {pid} runs confined ({what}) and glen does not"
        )),
        Confinement::Unknown(why) => Err(format!(
            "cannot tell whether the endpoint process {pid} runs confined ({why})"
        )),
    }
}
/// [`may_trust`] for a process known only by pid (no socket): its uid is not checked.
pub fn may_trust_pid(pid: u32) -> Result<(), String> {
    let me = self_confinement();
    let peer = Peer {
        pid: Some(pid),
        uid: rustix::process::geteuid().as_raw(),
    };
    may_trust_with(&peer, me, confinement_of)
}

/// The executable the kernel mapped for `pid` (`/proc/<pid>/exe` without a ` (deleted)` suffix,
/// or `proc_pidpath`).
pub fn image(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let link = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        let s = link.into_os_string().into_string().ok()?;
        Some(PathBuf::from(s.strip_suffix(" (deleted)").unwrap_or(&s)))
    }
    #[cfg(target_os = "macos")]
    {
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: buf is writable for the length passed.
        let n = unsafe {
            libc::proc_pidpath(
                i32::try_from(pid).ok()?,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
            )
        };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(OsString::from_vec(buf)))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

/// Whether `pid` runs the same executable as this process: both resolved to their real paths.
pub fn runs_current_exe(pid: u32) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    match (image(pid), std::env::current_exe()) {
        (Some(theirs), Ok(mine)) => canon(&theirs) == canon(&mine),
        _ => false,
    }
}

/// `pid`'s argv as the kernel holds it (`/proc/<pid>/cmdline`, `KERN_PROCARGS2`).
pub fn argv(pid: u32) -> Option<Vec<OsString>> {
    use std::os::unix::ffi::OsStringExt;
    #[cfg(target_os = "linux")]
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    #[cfg(target_os = "macos")]
    let bytes = {
        let mut mib = [
            libc::CTL_KERN,
            libc::KERN_PROCARGS2,
            i32::try_from(pid).ok()?,
        ];
        let mut size: libc::size_t = 0;
        // SAFETY: a null buffer asks for the size only.
        if unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } != 0
        {
            return None;
        }
        let mut buf = vec![0u8; size];
        // SAFETY: buf is writable for `size` bytes, which sysctl updates to the bytes written.
        if unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buf.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } != 0
        {
            return None;
        }
        buf.truncate(size);
        // argc, the exec path, NUL padding, then argv[0..argc] NUL-terminated.
        let argc = usize::try_from(i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?)).ok()?;
        let rest = buf.get(4..)?;
        let path_end = rest.iter().position(|b| *b == 0)?;
        let start = path_end + rest[path_end..].iter().position(|b| *b != 0)?;
        let args: Vec<&[u8]> = rest[start..].split(|b| *b == 0).take(argc).collect();
        if args.len() != argc {
            return None;
        }
        args.join(&0u8)
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let bytes: Vec<u8> = {
        let _ = pid;
        return None;
    };
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(&bytes);
    Some(
        bytes
            .split(|b| *b == 0)
            .map(|a| OsString::from_vec(a.to_vec()))
            .collect(),
    )
}

/// `pid`'s parent pid.
pub fn ppid(pid: u32) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (_, fields) = stat.rsplit_once(')')?;
        fields.split_whitespace().nth(1)?.parse().ok()
    }
    #[cfg(target_os = "macos")]
    {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: info is writable and correctly sized; proc_pidinfo writes at most size bytes.
        let n = unsafe {
            libc::proc_pidinfo(
                i32::try_from(pid).ok()?,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        // SAFETY: proc_pidinfo returned the full structure.
        (n == size).then(|| unsafe { info.assume_init() }.pbi_ppid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

/// `pid`'s process group.
pub fn pgid(pid: u32) -> Option<u32> {
    let pid = rustix::process::Pid::from_raw(i32::try_from(pid).ok()?)?;
    rustix::process::getpgid(Some(pid))
        .ok()
        .map(|p| p.as_raw_nonzero().get() as u32)
}

/// A pidfd for `pid`: signals sent through it reach that process or none, never a successor
/// that reused the pid.
#[cfg(target_os = "linux")]
pub fn pidfd(pid: u32) -> Option<std::os::fd::OwnedFd> {
    let pid = rustix::process::Pid::from_raw(i32::try_from(pid).ok()?)?;
    rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};

    #[test]
    fn a_connected_socket_names_the_listener_before_it_accepts() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("s.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let stream = UnixStream::connect(&path).unwrap();
        let peer = peer_of(&stream).unwrap();
        assert_eq!(peer.pid, Some(std::process::id()));
        assert_eq!(peer.uid, rustix::process::geteuid().as_raw());
    }

    #[test]
    fn the_ordering_rule() {
        let me = rustix::process::geteuid().as_raw();
        let peer = |pid| Peer { pid, uid: me };
        let free = Confinement::Unconfined;
        let caged = Confinement::Confined("sandbox");
        let of = |c: Confinement| move |_| c.clone();
        assert_eq!(
            may_trust_with(&peer(Some(7)), &free, of(free.clone())),
            Ok(())
        );
        for other in [caged.clone(), Confinement::Unknown("eacces".into())] {
            assert!(may_trust_with(&peer(Some(7)), &free, of(other.clone())).is_err());
            // A confined glen does not rank sandboxes.
            assert_eq!(may_trust_with(&peer(Some(7)), &caged, of(other)), Ok(()));
        }
        // An unknown self is refused like an unconfined one.
        let unknown = Confinement::Unknown("no /proc".into());
        assert!(may_trust_with(&peer(Some(7)), &unknown, of(caged.clone())).is_err());
        // An unmappable peer pid (0) only when this process is confined.
        assert!(may_trust_with(&peer(None), &free, of(free.clone())).is_err());
        assert_eq!(
            may_trust_with(&peer(None), &caged, of(free.clone())),
            Ok(())
        );
        // Another uid never.
        let foreign = Peer {
            pid: Some(7),
            uid: me.wrapping_add(1),
        };
        assert!(may_trust_with(&foreign, &caged, of(free)).is_err());
    }

    #[test]
    fn image_argv_and_ancestry_of_a_child() {
        use std::os::unix::process::CommandExt;
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        let args = argv(pid).unwrap();
        assert_eq!(args.len(), 2, "{args:?}");
        assert_eq!(args[1], "30");
        assert_eq!(ppid(pid), Some(std::process::id()));
        assert_eq!(pgid(pid), Some(pid));
        assert!(image(pid).unwrap().ends_with("sleep"));
        assert!(!runs_current_exe(pid));
        assert!(runs_current_exe(std::process::id()));
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(argv(pid), None, "a reaped process has no argv");
    }

    /// AC-H110: the macOS binding answers for this process and a `sandbox-exec` child. Skips,
    /// saying why, when this test process is itself confined (nested profiles are refused).
    #[cfg(target_os = "macos")]
    #[test]
    fn sandbox_check_tells_a_sandboxed_child_from_this_process() {
        assert!(sandbox_check(std::process::id()).is_some(), "symbol found");
        // -1 (an error) and a missing symbol are Unknown, which an unconfined glen refuses.
        for rc in [Some(-1), None] {
            assert!(matches!(sandbox_answer(1, rc), Confinement::Unknown(_)));
        }
        if let Confinement::Confined(_) | Confinement::Unknown(_) = self_confinement() {
            eprintln!("skipped: this test process is {:?}", self_confinement());
            return;
        }
        let mut caged = std::process::Command::new("/usr/bin/sandbox-exec")
            .args(["-p", "(version 1)(allow default)", "/bin/sleep", "30"])
            .spawn()
            .unwrap();
        let mut free = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        // sandbox-exec applies the profile, then execs: wait until the image is sleep.
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !image(caged.id()).is_some_and(|p| p.ends_with("sleep")) {
            assert!(std::time::Instant::now() < end, "sandbox-exec never exec'd");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(confinement_of(caged.id()), Confinement::Confined("sandbox"));
        assert_eq!(confinement_of(free.id()), Confinement::Unconfined);
        assert!(may_trust_pid(caged.id()).is_err());
        assert_eq!(may_trust_pid(free.id()), Ok(()));
        for c in [&mut caged, &mut free] {
            c.kill().unwrap();
            c.wait().unwrap();
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn no_new_privs_is_confinement() {
        if *self_confinement() != Confinement::Unconfined {
            eprintln!("skipped: this test process is {:?}", self_confinement());
            return;
        }
        let mut caged = std::process::Command::new("setpriv")
            .args(["--no-new-privs", "/bin/sleep", "30"])
            .spawn();
        let Ok(caged) = caged.as_mut() else {
            eprintln!("skipped: setpriv is not installed");
            return;
        };
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !image(caged.id()).is_some_and(|p| p.ends_with("sleep")) {
            assert!(std::time::Instant::now() < end, "setpriv never exec'd");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            confinement_of(caged.id()),
            Confinement::Confined("no_new_privs")
        );
        assert!(may_trust_pid(caged.id()).is_err());
        assert_eq!(confinement_of(std::process::id()), Confinement::Unconfined);
        caged.kill().unwrap();
        caged.wait().unwrap();
    }
}
