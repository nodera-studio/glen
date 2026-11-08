//! Kernel facts about another process (design §A14): who holds the other end of a socket, how
//! confined it is, which image it runs, its argv and ancestry. A same-uid sandboxed writer can
//! forge any file under GLEN_HOME, but not these.
//!
//! The confinement-ordering rule ([`trust_endpoint`]): glen never hands work to, or accepts
//! work from, a peer more confined than itself. Linux ranks the two confinements; macOS cannot,
//! so there a confined glen allows any peer (it is an add-only principal already). An unknown
//! answer is refused.
use std::{
    ffi::OsString,
    io,
    os::fd::{AsFd, BorrowedFd},
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

/// This process's own confinement, read once (Linux: from `/proc/self`).
pub fn self_confinement() -> &'static Confinement {
    static SELF: OnceLock<Confinement> = OnceLock::new();
    SELF.get_or_init(|| {
        #[cfg(target_os = "linux")]
        {
            facts("/proc/self").map_or_else(Confinement::Unknown, |f| f.absolute())
        }
        #[cfg(not(target_os = "linux"))]
        {
            own(std::process::id())
        }
    })
}

/// The confinement `pid` carries beyond this process's. Linux ranks the two (F5):
/// `Unconfined` means "no more confined than this process", so a containerised glen still
/// refuses a peer that confined itself further. macOS cannot rank, so the answer is the peer's
/// own confinement and [`trust_endpoint`] allows anything when glen itself is confined.
pub fn confinement_of(pid: u32) -> Confinement {
    #[cfg(target_os = "linux")]
    {
        match (facts("/proc/self"), facts(&format!("/proc/{pid}"))) {
            (Ok(me), Ok(peer)) => rank(&me, &peer),
            (Err(why), _) | (_, Err(why)) => Confinement::Unknown(why),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        own(pid)
    }
}

/// What Linux tells about one process's confinement: no_new_privs, the seccomp mode, and its
/// user, pid and mount namespaces. Every unprivileged seccomp, landlock or bwrap sandbox sets
/// no_new_privs; none of these can be undone, and rejoining a namespace needs `CAP_SYS_ADMIN`
/// in it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Facts {
    nnp: bool,
    /// 0 disabled, 1 strict, 2 filter.
    seccomp: u8,
    /// The initial user namespace maps the whole uid range (`0 0 4294967295`).
    initial_userns: bool,
    /// `ns/{user,pid,mnt}` link targets, or why they cannot be read. The links are
    /// ptrace-gated (`status` and `uid_map` are not): EACCES or EPERM for a peer in an
    /// ancestor user namespace, or for one that made itself non-dumpable.
    ns: Result<[PathBuf; 3], NsError>,
}
/// Why a process's `ns` links could not be read.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct NsError {
    /// EACCES or EPERM: the kernel's ptrace access check refused the read.
    denied: bool,
    why: String,
}
#[cfg(target_os = "linux")]
const NAMESPACES: [&str; 3] = ["user", "pid", "mnt"];
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl Facts {
    /// The process's confinement on its own, unranked.
    fn absolute(&self) -> Confinement {
        if self.nnp {
            Confinement::Confined("no_new_privs")
        } else if self.seccomp != 0 {
            Confinement::Confined("seccomp")
        } else if !self.initial_userns {
            // bwrap and rootless containers map only part of the uid range; leaving that
            // namespace needs privilege the process lacks.
            Confinement::Confined("user_namespace")
        } else {
            Confinement::Unconfined
        }
    }
}
/// The facts under `dir` (`/proc/self` or `/proc/<pid>`). The `ns` links can fail with EACCES
/// (a non-dumpable process, or one in an ancestor user namespace): [`rank`] decides what that
/// means.
#[cfg(target_os = "linux")]
fn facts(dir: &str) -> Result<Facts, String> {
    let status = std::fs::read_to_string(format!("{dir}/status"))
        .map_err(|e| format!("cannot read {dir}/status: {e}"))?;
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .map(str::trim)
    };
    let missing = || format!("{dir}/status has no NoNewPrivs or Seccomp field");
    let nnp = match field("NoNewPrivs:") {
        Some("0") => false,
        Some("1") => true,
        _ => return Err(missing()),
    };
    let seccomp = field("Seccomp:")
        .and_then(|s| s.parse().ok())
        .ok_or_else(missing)?;
    let initial_userns = std::fs::read_to_string(format!("{dir}/uid_map")).map_or(true, |m| {
        m.split_whitespace().collect::<Vec<_>>() == ["0", "0", "4294967295"]
    });
    let link = |ns: &str| {
        std::fs::read_link(format!("{dir}/ns/{ns}")).map_err(|e| NsError {
            denied: matches!(e.raw_os_error(), Some(libc::EACCES | libc::EPERM)),
            why: format!("cannot read the {ns} namespace of {dir}: {e}"),
        })
    };
    let ns = (|| {
        Ok([
            link(NAMESPACES[0])?,
            link(NAMESPACES[1])?,
            link(NAMESPACES[2])?,
        ])
    })();
    Ok(Facts {
        nnp,
        seccomp,
        initial_userns,
        ns,
    })
}
/// F5: `peer` ranked against `me`. The peer is no more confined when it sets no_new_privs only
/// if `me` does, its seccomp mode is at most mine, and it shares my user, pid and mount
/// namespaces. A peer in other namespaces passes only when it carries no confinement at all
/// and `me` does: glen runs in the sandbox and the peer on the host (§A13).
///
/// F7: a peer whose `ns` links the kernel refuses to show (EACCES/EPERM) while `me` runs in a
/// user namespace of its own is outside my namespaces (from inside a bwrap user namespace the
/// host's processes are not ptrace-readable); it is ranked on no_new_privs and seccomp alone,
/// which `status` still shows. Anywhere else an unreadable peer stays unknown: from the initial
/// user namespace only a peer that made itself non-dumpable hides them, and it is refused.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rank(me: &Facts, peer: &Facts) -> Confinement {
    if peer.nnp && !me.nnp {
        return Confinement::Confined("no_new_privs");
    }
    if peer.seccomp > me.seccomp {
        return Confinement::Confined("seccomp");
    }
    let (mine, theirs) = match (&me.ns, &peer.ns) {
        (Ok(mine), Ok(theirs)) => (mine, theirs),
        (Ok(_), Err(e)) if e.denied && !me.initial_userns => return Confinement::Unconfined,
        (Err(e), _) | (_, Err(e)) => return Confinement::Unknown(e.why.clone()),
    };
    let host_peer = peer.absolute() == Confinement::Unconfined;
    let caged_me = me.absolute() != Confinement::Unconfined;
    for (i, what) in ["user_namespace", "pid_namespace", "mnt_namespace"]
        .into_iter()
        .enumerate()
    {
        if mine[i] != theirs[i] && !(host_peer && caged_me) {
            return Confinement::Confined(what);
        }
    }
    Confinement::Unconfined
}

/// The confinement a macOS process carries in itself: a Sandbox.kext profile (`sandbox-exec`,
/// seatbelt, App Sandbox), which cannot be removed.
#[cfg(target_os = "macos")]
fn own(pid: u32) -> Confinement {
    sandbox_answer(&format!("sandbox_check({pid})"), sandbox_check(pid))
}
#[cfg(target_os = "macos")]
fn sandbox_answer(call: &str, rc: Option<i32>) -> Confinement {
    match rc {
        Some(0) => Confinement::Unconfined,
        Some(1) => Confinement::Confined("sandbox"),
        Some(rc) => Confinement::Unknown(format!(
            "{call} returned {rc}: {}",
            io::Error::last_os_error()
        )),
        None => Confinement::Unknown(format!("{call} is not available")),
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

/// A macOS audit token (`audit_token_t`): `val[5]` is the pid, `val[7]` the pid version, which
/// a successor that reuses the pid does not share.
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AuditToken {
    val: [u32; 8],
}
/// libSystem's private `int sandbox_check_by_audit_token(audit_token_t, const char *, int,
/// ...)`: like [`sandbox_check`], for the one process generation the token names. A token whose
/// process has exited, or whose pid now belongs to a successor, answers 1 (spike, macOS 27), so
/// a stale token is judged confined and refused.
#[cfg(target_os = "macos")]
fn sandbox_check_by_token(token: AuditToken) -> Option<i32> {
    type ByToken =
        unsafe extern "C" fn(AuditToken, *const libc::c_char, libc::c_int, ...) -> libc::c_int;
    static SYMBOL: OnceLock<Option<ByToken>> = OnceLock::new();
    let f = (*SYMBOL.get_or_init(|| {
        // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name only looks the symbol up.
        let p =
            unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"sandbox_check_by_audit_token".as_ptr()) };
        // SAFETY: a non-null `sandbox_check_by_audit_token` from libSystem has this C signature.
        (!p.is_null()).then(|| unsafe { std::mem::transmute::<*mut libc::c_void, ByToken>(p) })
    }))?;
    // SAFETY: a null operation with type 0 (SANDBOX_FILTER_NONE) takes no variadic arguments.
    Some(unsafe { f(token, std::ptr::null(), 0) })
}

/// Whether two confinements can be ranked here (Linux, F5). Where they cannot (macOS), a
/// confined glen allows any peer: it is an add-only principal already.
const RANKS: bool = cfg!(target_os = "linux");

/// The confinement-ordering rule: `Ok` when `peer` may exchange work with this process,
/// otherwise why not. `of` answers the peer's confinement relative to this process, or why the
/// peer is not the process it claims to be.
fn may_trust_with(
    peer: &Peer,
    me: &Confinement,
    ranks: bool,
    of: impl FnOnce(u32) -> Result<Confinement, String>,
) -> Result<(), String> {
    let euid = rustix::process::geteuid().as_raw();
    if peer.uid != euid {
        return Err(format!(
            "the endpoint runs as uid {}, not as this user ({euid})",
            peer.uid
        ));
    }
    let confined = matches!(me, Confinement::Confined(_));
    let Some(pid) = peer.pid else {
        // An unmappable pid: glen runs in a child pid namespace, the peer outside it (§A13).
        return if confined {
            Ok(())
        } else {
            Err("the endpoint process is outside this process's pid namespace".into())
        };
    };
    if confined && !ranks {
        return Ok(());
    }
    match of(pid)? {
        Confinement::Unconfined => Ok(()),
        Confinement::Confined(what) => Err(format!(
            "the endpoint process {pid} runs more confined ({what}) than glen"
        )),
        Confinement::Unknown(why) => Err(format!(
            "cannot tell whether the endpoint process {pid} runs more confined than glen ({why})"
        )),
    }
}
/// [`trust_endpoint`] for a process known only by pid (no socket): its uid is not checked, and
/// nothing binds the answer to a process generation.
pub fn may_trust_pid(pid: u32) -> Result<(), String> {
    let peer = Peer {
        pid: Some(pid),
        uid: rustix::process::geteuid().as_raw(),
    };
    may_trust_with(&peer, self_confinement(), RANKS, |pid| {
        Ok(confinement_of(pid))
    })
}

/// S-18, for every live connection: `fd`, connected to `path`, is held by the registered
/// endpoint process `pid`, and that process may receive work under the confinement-ordering
/// rule. The kernel records the listener's pid at `listen()`, so the number alone may since
/// name a successor (a confined listener forks a child that keeps the socket, then exits). The
/// confinement is therefore judged only for the process generation that holds the socket:
///
/// - Linux: a `SO_PEERPIDFD` pidfd (6.5+) pins the listening process; it must still be `pid`
///   (and a thread-group leader) before and after its confinement is read. On an older kernel,
///   or when the pidfd cannot be made (the listener has exited), `/proc/<pid>/fd` must hold the
///   one listening socket bound to `path`, before and after.
/// - macOS: the `LOCAL_PEERTOKEN` audit token must name `pid` and fails once the listener has
///   exited, but after pid reuse it names the successor (spike), so `pid` must also hold a
///   listening socket bound to `path` and, once it has accepted, the server end of this very
///   connection (F10: `PROC_PIDFDSOCKETINFO`, its `soi_so` equal to this socket's
///   `unsi_conn_so`), waited for until `end`. The confinement is judged by token
///   (`sandbox_check_by_audit_token`), which answers "confined" for a token whose process is
///   gone.
pub fn trust_endpoint(
    fd: BorrowedFd<'_>,
    path: &Path,
    pid: u32,
    end: std::time::Instant,
) -> Result<(), Refusal> {
    trust_endpoint_with(fd, path, pid, true, end)
}
/// Why [`trust_endpoint`] refused a connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The endpoint is not the registered process, or may not receive work, and why.
    Untrusted(String),
    /// macOS: the registered process did not accept the connection by the deadline, so whether
    /// it holds the connection is not known yet (F10).
    NotAccepted,
}
/// [`trust_endpoint`]; `pidfd` false forces the Linux fallback (tests).
fn trust_endpoint_with(
    fd: BorrowedFd<'_>,
    path: &Path,
    pid: u32,
    pidfd: bool,
    end: std::time::Instant,
) -> Result<(), Refusal> {
    let peer = peer_of(fd).map_err(|e| {
        Refusal::Untrusted(format!("cannot read the endpoint's peer credentials: {e}"))
    })?;
    if let Some(got) = peer.pid
        && got != pid
    {
        return Err(Refusal::Untrusted(format!(
            "the socket is held by pid {got}, not by the registered pid {pid}"
        )));
    }
    let late = std::cell::Cell::new(false);
    may_trust_with(&peer, self_confinement(), RANKS, |pid| {
        bound_confinement(fd, path, pid, pidfd, end).map_err(|r| match r {
            Refusal::Untrusted(why) => why,
            Refusal::NotAccepted => {
                late.set(true);
                "the endpoint did not accept the connection in time".into()
            }
        })
    })
    .map_err(|why| {
        if late.get() {
            Refusal::NotAccepted
        } else {
            Refusal::Untrusted(why)
        }
    })
}

/// [`confinement_of`] `pid`, proven to be the process that holds the listening socket behind
/// `fd` before and after it is read.
#[cfg(target_os = "linux")]
fn bound_confinement(
    fd: BorrowedFd<'_>,
    path: &Path,
    pid: u32,
    pidfd: bool,
    _end: std::time::Instant,
) -> Result<Confinement, Refusal> {
    let held = holder(fd, path, pid, pidfd).map_err(Refusal::Untrusted)?;
    held().map_err(Refusal::Untrusted)?;
    let confinement = confinement_of(pid);
    held().map_err(Refusal::Untrusted)?;
    Ok(confinement)
}
/// A check that `pid` is still the process holding the listener: through a pidfd when the
/// kernel gives one, else through `/proc/<pid>/fd`.
#[cfg(target_os = "linux")]
fn holder(
    fd: BorrowedFd<'_>,
    path: &Path,
    pid: u32,
    pidfd: bool,
) -> Result<Box<dyn Fn() -> Result<(), String>>, String> {
    if pidfd {
        match peer_pidfd(fd) {
            Ok(pinned) => {
                return Ok(Box::new(move || {
                    pidfd_names(&pinned, pid)?;
                    leader(pid)
                }));
            }
            // ENOPROTOOPT: before Linux 6.5. EINVAL: the listener has exited (6.5 to 6.8).
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOPROTOOPT | libc::EINVAL)) => (),
            Err(e) => return Err(format!("cannot pin the endpoint process: {e}")),
        }
    }
    let inode = listening_inode(path)?;
    Ok(Box::new(move || {
        holds_socket(pid, inode)?;
        leader(pid)
    }))
}
/// `SO_PEERPIDFD` (Linux 6.5+): a pidfd for the process that `listen()`ed.
#[cfg(target_os = "linux")]
fn peer_pidfd(fd: BorrowedFd<'_>) -> io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd, FromRawFd};
    const SO_PEERPIDFD: libc::c_int = 77;
    let mut pidfd: libc::c_int = -1;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: pidfd and len point to writable storage of the size passed.
    let rc = unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            SO_PEERPIDFD,
            (&raw mut pidfd).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success the kernel installed a new pidfd that nothing else owns.
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(pidfd) })
}
/// The pidfd still names a live `pid`: `/proc/self/fdinfo/<fd>` shows `Pid: -1` once the process
/// has exited and `0` when it is outside this pid namespace.
#[cfg(target_os = "linux")]
fn pidfd_names(pidfd: &std::os::fd::OwnedFd, pid: u32) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", pidfd.as_raw_fd()))
        .map_err(|e| format!("cannot read the endpoint pidfd: {e}"))?;
    let got = info
        .lines()
        .find_map(|l| l.strip_prefix("Pid:"))
        .and_then(|v| v.trim().parse::<i64>().ok());
    match got {
        Some(n) if n == i64::from(pid) => Ok(()),
        Some(-1) => Err("the process that listened on the socket has exited".into()),
        Some(0) => {
            Err("the process that listened on the socket is outside this pid namespace".into())
        }
        Some(n) => Err(format!(
            "the socket was bound by pid {n}, not by the registered pid {pid}"
        )),
        None => Err("the endpoint pidfd names no pid".into()),
    }
}
/// `pid` is a thread-group leader: a recycled number held by a thread is not a process.
#[cfg(target_os = "linux")]
fn leader(pid: u32) -> Result<(), String> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .map_err(|e| format!("cannot read /proc/{pid}/status: {e}"))?;
    let tgid = status
        .lines()
        .find_map(|l| l.strip_prefix("Tgid:"))
        .and_then(|v| v.trim().parse::<u32>().ok());
    if tgid == Some(pid) {
        Ok(())
    } else {
        Err(format!("pid {pid} is not a process (Tgid {tgid:?})"))
    }
}
/// The socket inode of the one listening unix socket whose bound file is the file now at
/// `path` (F9). The kernel is asked through `NETLINK_SOCK_DIAG` (`unix_diag`, this network
/// namespace's listeners with their `UNIX_DIAG_VFS` device and inode), and the listener is
/// selected by `lstat(path)`'s device and inode, never by its name: a bound name is not
/// escaped in `/proc/net/unix` and need not be the file's current name (a socket renamed over
/// `path` keeps the name it was bound under). No match, or more than one, is refused, and so
/// is any malformed, truncated or error reply.
#[cfg(target_os = "linux")]
fn listening_inode(path: &Path) -> Result<u64, String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("cannot lstat {}: {e}", path.display()))?;
    if !meta.file_type().is_socket() {
        return Err(format!("{} is not a socket", path.display()));
    }
    let want = (vfs_dev(meta.dev()), meta.ino());
    let listeners = unix_listeners()?;
    let found: Vec<u32> = listeners
        .iter()
        .filter(|l| {
            l.vfs
                .is_some_and(|(dev, ino)| (dev, u64::from(ino)) == (want.0, want.1 & 0xffff_ffff))
        })
        .map(|l| l.inode)
        .collect();
    match found[..] {
        [inode] => Ok(u64::from(inode)),
        [] => Err(format!(
            "no listening socket is bound to {}",
            path.display()
        )),
        _ => Err(format!(
            "{} listening sockets are bound to {}",
            found.len(),
            path.display()
        )),
    }
}
/// `st_dev` (the kernel's `new_encode_dev` of the superblock's device) in the kernel's internal
/// `dev_t` encoding (`MAJOR << 20 | MINOR`), which `UNIX_DIAG_VFS` reports (`s_dev`).
#[cfg(target_os = "linux")]
fn vfs_dev(st_dev: u64) -> u32 {
    let (major, minor) = (libc::major(st_dev), libc::minor(st_dev));
    (major << 20) | (minor & 0xf_ffff)
}
/// A listening unix socket as `unix_diag` reports it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Listener {
    /// The socket's own inode (`socket:[inode]` in an fd table).
    inode: u32,
    /// The bound file's device (kernel encoding) and inode (low 32 bits), when bound to a path.
    vfs: Option<(u32, u32)>,
}
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod diag {
    pub const NETLINK_SOCK_DIAG: libc::c_int = 4;
    pub const SOCK_DIAG_BY_FAMILY: u16 = 20;
    pub const NLMSG_ERROR: u16 = 2;
    pub const NLMSG_DONE: u16 = 3;
    pub const NLM_F_REQUEST: u16 = 0x1;
    pub const NLM_F_DUMP: u16 = 0x300;
    pub const TCP_LISTEN: u8 = 10;
    pub const UDIAG_SHOW_VFS: u32 = 0x2;
    pub const UNIX_DIAG_VFS: u16 = 1;
    /// `struct nlmsghdr`.
    pub const NLMSG_HDR: usize = 16;
    /// `struct unix_diag_req`.
    pub const REQ: usize = 24;
    /// `struct unix_diag_msg`.
    pub const MSG: usize = 16;
    /// One receive: larger than any dump datagram the kernel builds (at most 32 KiB).
    pub const RECV: usize = 64 * 1024;
    /// The whole reply: far more listeners than a host has, so a reply this large is refused.
    pub const TOTAL: usize = 16 * 1024 * 1024;
    pub const fn align4(n: usize) -> usize {
        (n + 3) & !3
    }
}
/// Every listening unix socket in this network namespace, from one `unix_diag` dump.
#[cfg(target_os = "linux")]
fn unix_listeners() -> Result<Vec<Listener>, String> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let fail = |what: &str| format!("unix_diag: {what}: {}", io::Error::last_os_error());
    // SAFETY: socket(2) with constant arguments; the result is checked.
    let raw = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            diag::NETLINK_SOCK_DIAG,
        )
    };
    if raw < 0 {
        return Err(fail("socket"));
    }
    // SAFETY: a new fd nothing else owns.
    let sock = unsafe { OwnedFd::from_raw_fd(raw) };
    let timeout = libc::timeval {
        tv_sec: 2,
        tv_usec: 0,
    };
    // SAFETY: a valid timeval of the size passed.
    if unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&raw const timeout).cast(),
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        )
    } != 0
    {
        return Err(fail("SO_RCVTIMEO"));
    }
    const SEQ: u32 = 0x676c_656e;
    let mut req = [0u8; diag::NLMSG_HDR + diag::REQ];
    req[0..4].copy_from_slice(&((diag::NLMSG_HDR + diag::REQ) as u32).to_ne_bytes());
    req[4..6].copy_from_slice(&diag::SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    req[6..8].copy_from_slice(&(diag::NLM_F_REQUEST | diag::NLM_F_DUMP).to_ne_bytes());
    req[8..12].copy_from_slice(&SEQ.to_ne_bytes());
    // unix_diag_req: family, protocol, pad, states, ino, show, cookie[2].
    let body = diag::NLMSG_HDR;
    req[body] = libc::AF_UNIX as u8;
    req[body + 4..body + 8].copy_from_slice(&(1u32 << diag::TCP_LISTEN).to_ne_bytes());
    req[body + 12..body + 16].copy_from_slice(&diag::UDIAG_SHOW_VFS.to_ne_bytes());
    // SAFETY: zeroed sockaddr_nl is valid; nl_pid 0 is the kernel.
    let mut kernel: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    kernel.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    // SAFETY: req and kernel are valid for the lengths passed.
    let sent = unsafe {
        libc::sendto(
            sock.as_raw_fd(),
            req.as_ptr().cast(),
            req.len(),
            0,
            (&raw const kernel).cast(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if sent != req.len() as isize {
        return Err(fail("sendto"));
    }
    let mut buf = vec![0u8; diag::RECV];
    let mut listeners = vec![];
    let mut total = 0usize;
    loop {
        // SAFETY: zeroed sockaddr_nl is valid storage for the sender's address.
        let mut from: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        let mut from_len = std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t;
        // SAFETY: buf and from are writable for the lengths passed. MSG_TRUNC makes the
        // return value the datagram's real length, so a truncated one is detected.
        let n = unsafe {
            libc::recvfrom(
                sock.as_raw_fd(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                libc::MSG_TRUNC,
                (&raw mut from).cast(),
                &mut from_len,
            )
        };
        if n < 0 {
            return Err(fail("recv"));
        }
        let n = n as usize;
        if n > buf.len() {
            return Err("unix_diag: a reply datagram was truncated".into());
        }
        if from.nl_pid != 0 {
            return Err("unix_diag: a reply came from outside the kernel".into());
        }
        total += n;
        if total > diag::TOTAL {
            return Err("unix_diag: the reply is too large".into());
        }
        if parse_diag(&buf[..n], SEQ, &mut listeners)? {
            return Ok(listeners);
        }
    }
}
/// Parses one `unix_diag` reply datagram into `out`; `Ok(true)` once the dump is done. Fails
/// on anything malformed, an error message, a foreign sequence number or an unexpected type.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_diag(mut data: &[u8], seq: u32, out: &mut Vec<Listener>) -> Result<bool, String> {
    let malformed = |what: &str| Err(format!("unix_diag: malformed reply ({what})"));
    let u16_at = |b: &[u8], at: usize| u16::from_ne_bytes([b[at], b[at + 1]]);
    let u32_at = |b: &[u8], at: usize| u32::from_ne_bytes(b[at..at + 4].try_into().expect("4"));
    if data.is_empty() {
        return malformed("empty datagram");
    }
    while !data.is_empty() {
        if data.len() < diag::NLMSG_HDR {
            return malformed("short header");
        }
        let len = u32_at(data, 0) as usize;
        if len < diag::NLMSG_HDR || len > data.len() {
            return malformed("message length");
        }
        if u32_at(data, 8) != seq {
            return malformed("sequence number");
        }
        let payload = &data[diag::NLMSG_HDR..len];
        match u16_at(data, 4) {
            diag::NLMSG_DONE => {
                // The dump's final status (0, or a negative errno).
                if payload.len() >= 4 && u32_at(payload, 0) != 0 {
                    return Err(format!(
                        "unix_diag: the dump failed ({})",
                        io::Error::from_raw_os_error(-(u32_at(payload, 0) as i32))
                    ));
                }
                return if data.len() == diag::align4(len).min(data.len()) {
                    Ok(true)
                } else {
                    malformed("data after the end of the dump")
                };
            }
            diag::NLMSG_ERROR => {
                let errno = if payload.len() >= 4 {
                    -(u32_at(payload, 0) as i32)
                } else {
                    0
                };
                return Err(format!(
                    "unix_diag: the kernel refused the request ({})",
                    io::Error::from_raw_os_error(errno)
                ));
            }
            diag::SOCK_DIAG_BY_FAMILY => out.push(parse_listener(payload)?),
            other => return malformed(&format!("message type {other}")),
        }
        data = &data[diag::align4(len).min(data.len())..];
    }
    Ok(false)
}
/// One `unix_diag_msg` and its attributes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_listener(msg: &[u8]) -> Result<Listener, String> {
    let malformed = |what: &str| Err(format!("unix_diag: malformed socket record ({what})"));
    let u16_at = |b: &[u8], at: usize| u16::from_ne_bytes([b[at], b[at + 1]]);
    let u32_at = |b: &[u8], at: usize| u32::from_ne_bytes(b[at..at + 4].try_into().expect("4"));
    if msg.len() < diag::MSG {
        return malformed("short record");
    }
    // unix_diag_msg: family, type, state, pad, ino, cookie[2].
    if msg[0] != libc::AF_UNIX as u8 {
        return malformed("family");
    }
    if msg[2] != diag::TCP_LISTEN {
        return malformed("a socket that is not listening");
    }
    let inode = u32_at(msg, 4);
    let mut vfs = None;
    let mut attrs = &msg[diag::MSG..];
    while !attrs.is_empty() {
        if attrs.len() < 4 {
            return malformed("short attribute");
        }
        let len = usize::from(u16_at(attrs, 0));
        if len < 4 || len > attrs.len() {
            return malformed("attribute length");
        }
        if u16_at(attrs, 2) == diag::UNIX_DIAG_VFS {
            if len != 4 + 8 || vfs.is_some() {
                return malformed("UNIX_DIAG_VFS");
            }
            // unix_diag_vfs: ino, then dev.
            vfs = Some((u32_at(attrs, 8), u32_at(attrs, 4)));
        }
        attrs = &attrs[diag::align4(len).min(attrs.len())..];
    }
    Ok(Listener { inode, vfs })
}
/// `pid`'s fd table holds the socket `inode`.
#[cfg(target_os = "linux")]
fn holds_socket(pid: u32, inode: u64) -> Result<(), String> {
    let want = std::ffi::OsString::from(format!("socket:[{inode}]"));
    let dir = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .map_err(|e| format!("cannot read /proc/{pid}/fd: {e}"))?;
    if dir
        .flatten()
        .any(|e| std::fs::read_link(e.path()).is_ok_and(|l| l.as_os_str() == want))
    {
        Ok(())
    } else {
        Err(format!(
            "process {pid} does not hold the endpoint's listening socket"
        ))
    }
}

#[cfg(target_os = "macos")]
fn bound_confinement(
    fd: BorrowedFd<'_>,
    path: &Path,
    pid: u32,
    _pidfd: bool,
    end: std::time::Instant,
) -> Result<Confinement, Refusal> {
    let token = peer_token(fd).map_err(|e| {
        Refusal::Untrusted(if e.raw_os_error() == Some(libc::EINVAL) {
            "the process that listened on the socket has exited".to_owned()
        } else {
            format!("cannot read the endpoint's audit token: {e}")
        })
    })?;
    if token.val[5] != pid {
        return Err(Refusal::Untrusted(format!(
            "the socket's audit token names pid {}, not the registered pid {pid}",
            token.val[5]
        )));
    }
    holds_listener(pid, path).map_err(Refusal::Untrusted)?;
    // F10: a name proves nothing about which socket this connection reached (a listener kept
    // by a departed process's child, renamed over the path of a TUI that reused that pid). The
    // server end of this very connection must be in `pid`'s fd table, before and after the
    // confinement is read.
    let server = server_end(fd).map_err(Refusal::Untrusted)?;
    accepted(pid, server, end)?;
    let confinement = sandbox_answer(
        "sandbox_check_by_audit_token",
        sandbox_check_by_token(token),
    );
    accepted(pid, server, end)?;
    Ok(confinement)
}
/// The kernel's (hashed, stable) id of the server-side socket of the connected unix socket
/// `fd` (`unsi_conn_so`). Before `accept` it is the socket queued on the listener; `accept`
/// hands that same socket to the accepting process.
#[cfg(target_os = "macos")]
fn server_end(fd: BorrowedFd<'_>) -> Result<u64, String> {
    use std::os::fd::AsRawFd;
    let info = socket_info(std::process::id(), fd.as_raw_fd()).ok_or_else(|| {
        format!(
            "cannot read this connection: {}",
            io::Error::last_os_error()
        )
    })?;
    match (info.kind(), info.conn_so()) {
        (SOCKINFO_UN, so) if so != 0 => Ok(so),
        _ => Err("this connection has no unix peer socket".into()),
    }
}
/// Waits until `pid` holds the socket `server` (it accepted this connection), or `end`.
#[cfg(target_os = "macos")]
fn accepted(pid: u32, server: u64, end: std::time::Instant) -> Result<(), Refusal> {
    loop {
        if any_socket(pid, |s| s.so() == server).map_err(Refusal::Untrusted)? {
            return Ok(());
        }
        if std::time::Instant::now() >= end {
            return Err(Refusal::NotAccepted);
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
/// `LOCAL_PEERTOKEN`: the audit token of the process that `listen()`ed.
#[cfg(target_os = "macos")]
fn peer_token(fd: BorrowedFd<'_>) -> io::Result<AuditToken> {
    use std::os::fd::AsRawFd;
    let mut token = AuditToken { val: [0; 8] };
    let mut len = std::mem::size_of::<AuditToken>() as libc::socklen_t;
    // SAFETY: token and len point to writable storage of the size passed.
    let rc = unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            (&raw mut token).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(token)
}
/// `pid` holds a listening unix socket bound to `path` (`PROC_PIDLISTFDS`, then
/// `PROC_PIDFDSOCKETINFO` on each socket fd).
#[cfg(target_os = "macos")]
fn holds_listener(pid: u32, path: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    let found = any_socket(pid, |s| {
        s.kind() == SOCKINFO_UN
            && i32::from(s.options()) & libc::SO_ACCEPTCONN != 0
            && same_socket_path(Path::new(std::ffi::OsStr::from_bytes(s.unix_path())), path)
    })?;
    if found {
        Ok(())
    } else {
        Err(format!(
            "process {pid} does not hold a listening socket bound to {}",
            path.display()
        ))
    }
}
/// `soi_kind` of a unix socket.
#[cfg(target_os = "macos")]
const SOCKINFO_UN: i32 = 3;
/// A `struct socket_fdinfo` (<sys/proc_info.h>, 792 bytes): `psi.soi_so` (u64) at 160,
/// `psi.soi_options` (short) at 188, `psi.soi_kind` (int) at 256, then
/// `psi.soi_proto.pri_un`: `unsi_conn_so` (u64) at 264 and `unsi_addr` (sockaddr_un) at 280.
#[cfg(target_os = "macos")]
struct SocketInfo([u8; 792]);
#[cfg(target_os = "macos")]
impl SocketInfo {
    fn so(&self) -> u64 {
        u64::from_ne_bytes(self.0[160..168].try_into().expect("8 bytes"))
    }
    fn options(&self) -> i16 {
        i16::from_ne_bytes(self.0[188..190].try_into().expect("2 bytes"))
    }
    fn kind(&self) -> i32 {
        i32::from_ne_bytes(self.0[256..260].try_into().expect("4 bytes"))
    }
    fn conn_so(&self) -> u64 {
        u64::from_ne_bytes(self.0[264..272].try_into().expect("8 bytes"))
    }
    /// sockaddr_un: sun_len, sun_family, then sun_path[104], NUL-terminated.
    fn unix_path(&self) -> &[u8] {
        let sun_path = &self.0[282..282 + 104];
        &sun_path[..sun_path.iter().position(|b| *b == 0).unwrap_or(104)]
    }
}
/// `pid`'s socket fd `fd`, or `None` when it cannot be read (not a socket, gone).
#[cfg(target_os = "macos")]
fn socket_info(pid: u32, fd: i32) -> Option<SocketInfo> {
    const PROC_PIDFDSOCKETINFO: libc::c_int = 3;
    let mut info = SocketInfo([0; 792]);
    // SAFETY: info is writable for its full size.
    let got = unsafe {
        libc::proc_pidfdinfo(
            i32::try_from(pid).ok()?,
            fd,
            PROC_PIDFDSOCKETINFO,
            info.0.as_mut_ptr().cast(),
            info.0.len() as i32,
        )
    };
    (got == info.0.len() as i32).then_some(info)
}
/// Whether any socket in `pid`'s fd table (`PROC_PIDLISTFDS`) satisfies `want`.
#[cfg(target_os = "macos")]
fn any_socket(pid: u32, want: impl Fn(&SocketInfo) -> bool) -> Result<bool, String> {
    let raw = i32::try_from(pid).map_err(|_| format!("invalid pid {pid}"))?;
    let unreadable = || {
        format!(
            "cannot list the fds of process {pid}: {}",
            io::Error::last_os_error()
        )
    };
    // SAFETY: a null buffer asks for the size only.
    let n = unsafe { libc::proc_pidinfo(raw, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    if n <= 0 {
        return Err(unreadable());
    }
    let each = std::mem::size_of::<libc::proc_fdinfo>();
    // Room for fds opened since the size was asked.
    let mut fds = vec![
        libc::proc_fdinfo {
            proc_fd: 0,
            proc_fdtype: 0
        };
        n as usize / each + 16
    ];
    // SAFETY: fds is writable for the byte length passed.
    let n = unsafe {
        libc::proc_pidinfo(
            raw,
            libc::PROC_PIDLISTFDS,
            0,
            fds.as_mut_ptr().cast(),
            (fds.len() * each) as i32,
        )
    };
    if n <= 0 {
        return Err(unreadable());
    }
    fds.truncate(n as usize / each);
    Ok(fds
        .iter()
        .filter(|f| f.proc_fdtype == libc::PROX_FDTYPE_SOCKET as u32)
        .filter_map(|f| socket_info(pid, f.proc_fd))
        .any(|s| want(&s)))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn bound_confinement(
    _fd: BorrowedFd<'_>,
    _path: &Path,
    _pid: u32,
    _pidfd: bool,
    _end: std::time::Instant,
) -> Result<Confinement, Refusal> {
    Ok(Confinement::Unknown(
        "socket ownership is not provable on this platform".into(),
    ))
}

/// Whether the socket name `bound` (as the listener spelled it) is `path`: equal, or the same
/// file name in the same real directory (the extension may spell GLEN_HOME differently).
#[cfg(target_os = "macos")]
fn same_socket_path(bound: &Path, path: &Path) -> bool {
    bound == path
        || (bound.file_name().is_some() && bound.file_name() == path.file_name())
            && matches!(
                (
                    bound.parent().map(std::fs::canonicalize),
                    path.parent().map(std::fs::canonicalize)
                ),
                (Some(Ok(a)), Some(Ok(b))) if a == b
            )
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

/// Whether `pid` runs the same executable file as this process, compared by the identity of the
/// file the kernel mapped, never by path (F4): a path replaced since (by a symlink to glen, say)
/// proves nothing.
///
/// - Linux: `stat` through `/proc/<pid>/exe` and `/proc/self/exe` reaches the mapped inode even
///   when it was deleted or replaced; device and inode must match.
/// - macOS: device and inode of the first file-backed memory region, the main image
///   (`PROC_PIDREGIONPATHINFO`), for both processes. Where that cannot be read, the canonical
///   `proc_pidpath` paths are compared instead; that proves nothing more than a path, but only
///   a process the caller could already signal can pass.
///
/// A worker started by another glen binary (one replaced by an upgrade since) is not proven.
pub fn runs_current_exe(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        match (
            std::fs::metadata(format!("/proc/{pid}/exe")),
            std::fs::metadata("/proc/self/exe"),
        ) {
            (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
            _ => false,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        #[cfg(target_os = "macos")]
        if let (Some(theirs), Some(mine)) = (main_image(pid), main_image(std::process::id())) {
            return theirs == mine;
        }
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
        match (image(pid), std::env::current_exe()) {
            (Some(theirs), Ok(mine)) => canon(&theirs) == canon(&mine),
            _ => false,
        }
    }
}
/// Device and inode of `pid`'s main image: its first file-backed region.
#[cfg(target_os = "macos")]
fn main_image(pid: u32) -> Option<(u32, u64)> {
    // <sys/proc_info.h>: struct proc_regionwithpathinfo is 1272 bytes; prp_prinfo.pri_address
    // at 80, pri_size at 88, prp_vip.vip_vi.vi_stat.vst_dev at 96, vst_ino at 104, vip_path at
    // 248.
    const PROC_PIDREGIONPATHINFO: libc::c_int = 8;
    const SIZE: usize = 1272;
    let raw = i32::try_from(pid).ok()?;
    let mut info = [0u8; SIZE];
    let mut address = 0u64;
    let u64_at = |b: &[u8], at: usize| u64::from_ne_bytes(b[at..at + 8].try_into().expect("8"));
    for _ in 0..4096 {
        // SAFETY: info is writable for SIZE bytes.
        let n = unsafe {
            libc::proc_pidinfo(
                raw,
                PROC_PIDREGIONPATHINFO,
                address,
                info.as_mut_ptr().cast(),
                SIZE as i32,
            )
        };
        if n != SIZE as i32 {
            return None;
        }
        if info[248] != 0 {
            let dev = u32::from_ne_bytes(info[96..100].try_into().expect("4"));
            return Some((dev, u64_at(&info, 104)));
        }
        let next = u64_at(&info, 80).checked_add(u64_at(&info, 88))?;
        if next <= address {
            return None;
        }
        address = next;
    }
    None
}

/// `pid`'s argv as the kernel holds it (`/proc/<pid>/cmdline`, `KERN_PROCARGS2`); `None` when
/// unreadable or empty.
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
    // F1: a process between exec and its argument setup reads as empty.
    if bytes.is_empty() {
        return None;
    }
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

/// Whether `pid` is in the stopped state (Linux `/proc/<pid>/stat` state `T`, macOS
/// `pbi_status == SSTOP`): a SIGSTOP submitted is not yet a process stopped.
pub fn stopped(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(')')
                .and_then(|(_, f)| f.split_whitespace().next())
                == Some("T")
        })
    }
    #[cfg(target_os = "macos")]
    {
        let Ok(raw) = i32::try_from(pid) else {
            return false;
        };
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: info is writable and correctly sized; proc_pidinfo writes at most size bytes.
        let n = unsafe {
            libc::proc_pidinfo(
                raw,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        // SAFETY: proc_pidinfo returned the full structure.
        n == size && unsafe { info.assume_init() }.pbi_status == libc::SSTOP
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        false
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

    /// Skips a test this process cannot stand in for, saying why; under CI (`CI` or
    /// `GITHUB_ACTIONS` set) it fails instead, so a confined runner never passes silently.
    fn skip(why: &str) {
        let ci = ["CI", "GITHUB_ACTIONS"]
            .iter()
            .any(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty()));
        assert!(!ci, "cannot run under CI: {why}");
        eprintln!("skipped: {why}");
    }
    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done() {
            assert!(std::time::Instant::now() < end, "{what}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

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
        let of = |c: Confinement| move |_| Ok(c);
        for ranks in [false, true] {
            assert_eq!(
                may_trust_with(&peer(Some(7)), &free, ranks, of(free.clone())),
                Ok(())
            );
            for other in [caged.clone(), Confinement::Unknown("eacces".into())] {
                assert!(may_trust_with(&peer(Some(7)), &free, ranks, of(other.clone())).is_err());
                // A confined glen allows any peer only where sandboxes cannot be ranked.
                let got = may_trust_with(&peer(Some(7)), &caged, ranks, of(other));
                assert_eq!(got.is_ok(), !ranks, "ranks {ranks}: {got:?}");
            }
            // An unknown self is refused like an unconfined one.
            let unknown = Confinement::Unknown("no /proc".into());
            assert!(may_trust_with(&peer(Some(7)), &unknown, ranks, of(caged.clone())).is_err());
            // An unmappable peer pid (0) only when this process is confined.
            assert!(may_trust_with(&peer(None), &free, ranks, of(free.clone())).is_err());
            assert_eq!(
                may_trust_with(&peer(None), &caged, ranks, of(free.clone())),
                Ok(())
            );
            // A peer that is not the process it claims is refused, whatever its confinement.
            assert_eq!(
                may_trust_with(&peer(Some(7)), &free, ranks, |_| Err("gone".into())),
                Err("gone".into())
            );
            // Another uid never.
            let foreign = Peer {
                pid: Some(7),
                uid: me.wrapping_add(1),
            };
            assert!(may_trust_with(&foreign, &caged, ranks, of(free.clone())).is_err());
        }
    }

    /// F5: Linux ranks a peer against glen's own confinement, so a containerised glen still
    /// refuses a peer confined further, and a sandboxed glen still reaches the owner's TUI.
    #[test]
    fn f5_confinements_are_ranked_relative_to_glen() {
        let host_ns = [
            PathBuf::from("user:[1]"),
            PathBuf::from("pid:[2]"),
            PathBuf::from("mnt:[3]"),
        ];
        let facts = |nnp, seccomp, initial_userns, ns: &[PathBuf; 3]| Facts {
            nnp,
            seccomp,
            initial_userns,
            ns: Ok(ns.clone()),
        };
        let host = facts(false, 0, true, &host_ns);
        // Docker: a seccomp filter in its own pid and mount namespaces, no no_new_privs.
        let docker_ns = [
            host_ns[0].clone(),
            PathBuf::from("pid:[20]"),
            PathBuf::from("mnt:[30]"),
        ];
        let docker = facts(false, 2, true, &docker_ns);
        let nested_nnp = facts(true, 2, true, &docker_ns);
        let nested_plain = facts(false, 2, true, &docker_ns);
        // Rootless podman / toolbox: its own user namespace too.
        let podman_ns = [
            PathBuf::from("user:[10]"),
            docker_ns[1].clone(),
            docker_ns[2].clone(),
        ];
        let podman = facts(false, 2, false, &podman_ns);
        // Codex on Linux: landlock and seccomp (so no_new_privs), maybe its own mount namespace.
        let codex = facts(true, 2, true, &host_ns);
        let codex_bwrap = facts(true, 2, true, &docker_ns);
        let free = Confinement::Unconfined;
        let cases = [
            ("docker self, nested nnp peer", &docker, &nested_nnp, false),
            (
                "docker self, same-container peer",
                &docker,
                &nested_plain,
                true,
            ),
            (
                "podman self, nested nnp peer",
                &podman,
                &facts(true, 2, false, &podman_ns),
                false,
            ),
            (
                "podman self, same-container peer",
                &podman,
                &podman.clone(),
                true,
            ),
            ("codex self, owner TUI", &codex, &host, true),
            ("codex in bwrap, owner TUI", &codex_bwrap, &host, true),
            (
                "codex self, another sandbox",
                &codex,
                &facts(true, 2, true, &docker_ns),
                false,
            ),
            ("host self, owner TUI", &host, &host, true),
            (
                "host self, nnp peer",
                &host,
                &facts(true, 0, true, &host_ns),
                false,
            ),
            (
                "host self, seccomp peer",
                &host,
                &facts(false, 2, true, &host_ns),
                false,
            ),
            ("host self, container peer", &host, &nested_plain, false),
            (
                "host self, peer in another mount ns",
                &host,
                &facts(false, 0, true, &docker_ns),
                false,
            ),
            (
                "host self, user-ns peer",
                &host,
                &facts(false, 0, false, &podman_ns),
                false,
            ),
            (
                "strict seccomp self, filter peer",
                &facts(false, 1, true, &host_ns),
                &facts(false, 2, true, &host_ns),
                false,
            ),
        ];
        for (name, me, peer, allowed) in cases {
            assert_eq!(
                rank(me, peer) == free,
                allowed,
                "{name}: {:?}",
                rank(me, peer)
            );
        }
    }

    /// F7: a peer whose `ns` links the kernel will not show (EACCES/EPERM: ptrace-gated) is
    /// ranked on no_new_privs and seccomp only when glen runs in a user namespace of its own
    /// (bwrap; the peer is then the host's omp), and stays unknown everywhere else, since from
    /// the initial user namespace only a non-dumpable process hides them.
    #[test]
    fn f7_a_peer_with_unreadable_namespaces_is_ranked_only_from_a_user_namespace() {
        let host_ns = [
            PathBuf::from("user:[1]"),
            PathBuf::from("pid:[2]"),
            PathBuf::from("mnt:[3]"),
        ];
        let bwrap_ns = [
            PathBuf::from("user:[10]"),
            host_ns[1].clone(),
            PathBuf::from("mnt:[30]"),
        ];
        let denied = |denied| {
            Err(NsError {
                denied,
                why: "cannot read the user namespace of /proc/7: Permission denied".into(),
            })
        };
        let facts = |nnp, seccomp, initial_userns, ns| Facts {
            nnp,
            seccomp,
            initial_userns,
            ns,
        };
        // glen inside bwrap: no_new_privs, its own user and mount namespaces.
        let bwrap = facts(true, 0, false, Ok(bwrap_ns.clone()));
        let host = facts(false, 0, true, Ok(host_ns.clone()));
        let free = Confinement::Unconfined;
        let cases = [
            (
                "userns self, unreadable unconfined peer",
                &bwrap,
                facts(false, 0, true, denied(true)),
                true,
            ),
            (
                "userns self, unreadable peer with no_new_privs as glen",
                &bwrap,
                facts(true, 0, true, denied(true)),
                true,
            ),
            (
                "userns self, unreadable seccomp peer",
                &bwrap,
                facts(false, 2, true, denied(true)),
                false,
            ),
            (
                "userns self without no_new_privs, unreadable nnp peer",
                &facts(false, 0, false, Ok(bwrap_ns.clone())),
                facts(true, 0, true, denied(true)),
                false,
            ),
            (
                "userns self, namespaces unreadable for another reason",
                &bwrap,
                facts(false, 0, true, denied(false)),
                false,
            ),
            (
                "initial-userns self, unreadable (non-dumpable) peer",
                &host,
                facts(false, 0, true, denied(true)),
                false,
            ),
            (
                "confined initial-userns self, unreadable peer",
                &facts(true, 2, true, Ok(host_ns.clone())),
                facts(false, 0, true, denied(true)),
                false,
            ),
        ];
        for (name, me, peer, allowed) in cases {
            let got = rank(me, &peer);
            assert_eq!(got == free, allowed, "{name}: {got:?}");
            if !allowed && !(peer.nnp && !me.nnp) && peer.seccomp <= me.seccomp {
                assert!(matches!(got, Confinement::Unknown(_)), "{name}: {got:?}");
            }
        }
    }

    /// F1: argv is read only once the child has exec'd and set up its arguments; a process
    /// between the two reads as `None`, never as `[""]`.
    #[test]
    fn image_argv_and_ancestry_of_a_child() {
        use std::os::unix::process::CommandExt;
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        wait_until("sleep never exec'd", || {
            image(pid).is_some_and(|p| p.ends_with("sleep")) && argv(pid).is_some()
        });
        let args = argv(pid).unwrap();
        assert_eq!(args.len(), 2, "{args:?}");
        assert_eq!(args[1], "30");
        assert_eq!(ppid(pid), Some(std::process::id()));
        assert_eq!(pgid(pid), Some(pid));
        assert!(!runs_current_exe(pid));
        assert!(runs_current_exe(std::process::id()));
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(argv(pid), None, "a reaped process has no argv");
    }

    /// F8: a SIGSTOPped process is seen stopped, a running one is not.
    #[test]
    fn a_stopped_process_is_seen_stopped() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(!stopped(pid));
        let raw = pid as libc::pid_t;
        // SAFETY: signals to this test's own child.
        unsafe { libc::kill(raw, libc::SIGSTOP) };
        wait_until("never seen stopped", || stopped(pid));
        // SAFETY: as above.
        unsafe { libc::kill(raw, libc::SIGCONT) };
        wait_until("never seen running again", || !stopped(pid));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!stopped(pid), "a reaped process is not stopped");
    }

    /// F4: the image is proven by the mapped file, not by a path. A symlink named after glen's
    /// executable that points elsewhere, and a copy of glen's executable, are both other files.
    #[test]
    fn f4_the_image_is_proven_by_the_mapped_file_not_its_path() {
        let t = tempfile::tempdir().unwrap();
        let copy = t.path().join("glen-copy");
        std::fs::copy("/bin/sleep", &copy).unwrap();
        let mut child = std::process::Command::new(&copy).arg("30").spawn().unwrap();
        let pid = child.id();
        wait_until("the copy never exec'd", || {
            image(pid).is_some_and(|p| p.ends_with("glen-copy"))
        });
        // Replace the path the child ran from with a symlink to glen's own executable: a path
        // compare after canonicalising would now call the child glen.
        std::fs::remove_file(&copy).unwrap();
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &copy).unwrap();
        assert!(!runs_current_exe(pid), "a replaced path proves nothing");
        child.kill().unwrap();
        child.wait().unwrap();
    }

    /// A listener bound to `path` by a forked child A, held by A's child B after A exits and is
    /// reaped. Returns (A, B); the caller kills B.
    fn orphaned_listener(path: &Path) -> (u32, u32) {
        orphaned_listener_serving(path, false)
    }
    /// [`orphaned_listener`]; when `serve`, B accepts every connection and keeps it open.
    fn orphaned_listener_serving(path: &Path, serve: bool) -> (u32, u32) {
        use std::os::unix::ffi::OsStrExt;
        // SAFETY: zeroed sockaddr_un is a valid value.
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as _;
        let bytes = path.as_os_str().as_bytes();
        assert!(bytes.len() < addr.sun_path.len());
        for (d, s) in addr.sun_path.iter_mut().zip(bytes) {
            *d = *s as _;
        }
        let mut pipe = [0; 2];
        // SAFETY: pipe writes two fds into the array.
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        // SAFETY: the child runs only async-signal-safe system calls, then `_exit`s or pauses.
        let a = unsafe { libc::fork() };
        assert!(a >= 0);
        if a == 0 {
            // SAFETY: plain system calls on local storage.
            unsafe {
                let s = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
                let len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
                if s < 0
                    || libc::bind(s, (&raw const addr).cast(), len) != 0
                    || libc::listen(s, 8) != 0
                {
                    libc::_exit(1);
                }
                let b = libc::fork();
                if b == 0 {
                    loop {
                        if serve {
                            libc::accept(s, std::ptr::null_mut(), std::ptr::null_mut());
                        } else {
                            libc::pause();
                        }
                    }
                }
                libc::write(
                    pipe[1],
                    (&raw const b).cast(),
                    std::mem::size_of::<libc::pid_t>(),
                );
                libc::_exit(0);
            }
        }
        let mut b: libc::pid_t = 0;
        // SAFETY: reads one pid_t into b; waitpid reaps A.
        unsafe {
            assert_eq!(
                libc::read(
                    pipe[0],
                    (&raw mut b).cast(),
                    std::mem::size_of::<libc::pid_t>()
                ),
                std::mem::size_of::<libc::pid_t>() as isize
            );
            libc::close(pipe[0]);
            libc::close(pipe[1]);
            let mut status = 0;
            assert_eq!(libc::waitpid(a, &mut status, 0), a);
            assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
        }
        assert!(b > 0);
        (a as u32, b as u32)
    }

    /// F2: an endpoint is trusted for the process generation that holds its listener, and only
    /// then. Here the listener's process A exited after forking B, which keeps the socket: the
    /// kernel still reports A's pid for it. Claiming A is refused (A is gone); claiming a live
    /// unconfined pid (this process) is refused (it is not the listener); and the ownership proof
    /// that defeats a successor reusing A's pid holds for B, never for another live process.
    #[test]
    fn f2_a_listener_whose_process_is_gone_is_refused() {
        if !RANKS && *self_confinement() != Confinement::Unconfined {
            return skip(&format!("this test process is {:?}", self_confinement()));
        }
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("l.sock");
        let (a, b) = orphaned_listener(&path);
        let _kill = Kill(b);
        let stream = UnixStream::connect(&path).unwrap();
        // The pid fixed at listen(); Linux may report 0 for a reaped one, refused all the same.
        let reported = peer_of(&stream).unwrap().pid;
        assert!(
            reported == Some(a) || (cfg!(target_os = "linux") && reported.is_none()),
            "{reported:?}"
        );
        let me = std::process::id();
        for pidfd in [true, false] {
            let got = trust_endpoint_with(stream.as_fd(), &path, a, pidfd, soon());
            assert!(got.is_err(), "pidfd {pidfd}: {got:?}");
            let got = trust_endpoint_with(stream.as_fd(), &path, me, pidfd, soon());
            assert!(got.is_err(), "pidfd {pidfd}: {got:?}");
        }
        #[cfg(target_os = "linux")]
        {
            let inode = listening_inode(&path).unwrap();
            assert_eq!(holds_socket(b, inode), Ok(()), "B holds the listener");
            assert!(
                holds_socket(me, inode).is_err(),
                "a successor at A's pid would not"
            );
            assert!(holds_socket(a, inode).is_err());
        }
        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                peer_token(stream.as_fd()).map_err(|e| e.raw_os_error()),
                Err(Some(libc::EINVAL))
            );
            assert_eq!(holds_listener(b, &path), Ok(()), "B holds the listener");
            assert!(
                holds_listener(me, &path).is_err(),
                "a successor at A's pid would not"
            );
        }
    }
    /// One netlink message of type `ty` around `payload`, padded to 4 bytes.
    fn nlmsg(ty: u16, seq: u32, payload: &[u8]) -> Vec<u8> {
        let len = diag::NLMSG_HDR + payload.len();
        let mut m = vec![];
        m.extend((len as u32).to_ne_bytes());
        m.extend(ty.to_ne_bytes());
        m.extend(0x2u16.to_ne_bytes()); // NLM_F_MULTI
        m.extend(seq.to_ne_bytes());
        m.extend(0u32.to_ne_bytes());
        m.extend(payload);
        m.resize(diag::align4(len), 0);
        m
    }
    /// A `unix_diag_msg` for a listener with socket inode `ino`, and its `UNIX_DIAG_VFS`.
    fn record(state: u8, ino: u32, vfs: Option<(u32, u32)>) -> Vec<u8> {
        let mut r = vec![libc::AF_UNIX as u8, libc::SOCK_STREAM as u8, state, 0];
        r.extend(ino.to_ne_bytes());
        r.extend([0u8; 8]);
        if let Some((dev, vino)) = vfs {
            r.extend(12u16.to_ne_bytes());
            r.extend(diag::UNIX_DIAG_VFS.to_ne_bytes());
            r.extend(vino.to_ne_bytes());
            r.extend(dev.to_ne_bytes());
        }
        r
    }

    /// F9: the `unix_diag` reply parser takes well-formed records and fails closed on anything
    /// else: an error, a failed dump, a foreign sequence, an unknown type, truncation, bad
    /// attribute lengths, a duplicated VFS attribute, a socket that is not a listener, and data
    /// after the end of the dump.
    #[test]
    fn f9_the_unix_diag_parser_fails_closed() {
        const SEQ: u32 = 9;
        let by = diag::SOCK_DIAG_BY_FAMILY;
        let done = nlmsg(diag::NLMSG_DONE, SEQ, &0i32.to_ne_bytes());
        let a = nlmsg(
            by,
            SEQ,
            &record(diag::TCP_LISTEN, 11, Some((0x80_0001, 77))),
        );
        let b = nlmsg(by, SEQ, &record(diag::TCP_LISTEN, 12, None));
        let mut out = vec![];
        assert_eq!(
            parse_diag(&[a.clone(), b.clone()].concat(), SEQ, &mut out),
            Ok(false)
        );
        assert_eq!(parse_diag(&done, SEQ, &mut out), Ok(true));
        assert_eq!(
            out,
            [
                Listener {
                    inode: 11,
                    vfs: Some((0x80_0001, 77))
                },
                Listener {
                    inode: 12,
                    vfs: None
                }
            ]
        );
        let mut out = vec![];
        assert_eq!(
            parse_diag(&[a.clone(), done.clone()].concat(), SEQ, &mut out),
            Ok(true)
        );
        assert_eq!(out.len(), 1);
        let bad: Vec<(&str, Vec<u8>)> = vec![
            ("empty", vec![]),
            ("short header", a[..10].to_vec()),
            ("cut message", a[..a.len() - 4].to_vec()),
            (
                "foreign sequence",
                nlmsg(by, SEQ + 1, &record(diag::TCP_LISTEN, 1, None)),
            ),
            (
                "error",
                nlmsg(diag::NLMSG_ERROR, SEQ, &(-libc::EINVAL).to_ne_bytes()),
            ),
            (
                "failed dump",
                nlmsg(diag::NLMSG_DONE, SEQ, &(-libc::ENOBUFS).to_ne_bytes()),
            ),
            ("unknown type", nlmsg(1, SEQ, &[])),
            ("short record", nlmsg(by, SEQ, &[1, 1, 10, 0])),
            ("not listening", nlmsg(by, SEQ, &record(1, 1, None))),
            ("data after done", [done.clone(), a.clone()].concat()),
            ("length below a header", {
                let mut m = a.clone();
                m[0..4].copy_from_slice(&8u32.to_ne_bytes());
                m
            }),
            ("attribute length", {
                let mut r = record(diag::TCP_LISTEN, 1, Some((1, 2)));
                let at = diag::MSG;
                r[at..at + 2].copy_from_slice(&40u16.to_ne_bytes());
                nlmsg(by, SEQ, &r)
            }),
            ("short VFS attribute", {
                let mut r = record(diag::TCP_LISTEN, 1, Some((1, 2)));
                let at = diag::MSG;
                r[at..at + 2].copy_from_slice(&8u16.to_ne_bytes());
                nlmsg(by, SEQ, &r)
            }),
            ("duplicated VFS attribute", {
                let mut r = record(diag::TCP_LISTEN, 1, Some((1, 2)));
                let attr = r[diag::MSG..].to_vec();
                r.extend(attr);
                nlmsg(by, SEQ, &r)
            }),
            ("foreign family", {
                let mut r = record(diag::TCP_LISTEN, 1, None);
                r[0] = libc::AF_INET as u8;
                nlmsg(by, SEQ, &r)
            }),
        ];
        for (name, data) in bad {
            assert!(parse_diag(&data, SEQ, &mut vec![]).is_err(), "{name}");
        }
    }

    /// F9: a listener bound here is found by its file's device and inode (the kernel's `s_dev`
    /// encoding matches `st_dev` converted back), and this process holds its socket inode.
    #[cfg(target_os = "linux")]
    #[test]
    fn f9_a_bound_listener_is_found_by_its_file() {
        use std::os::unix::fs::MetadataExt;
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("l.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let meta = std::fs::symlink_metadata(&path).unwrap();
        let want = (vfs_dev(meta.dev()), meta.ino() as u32);
        let all = unix_listeners().unwrap();
        let mine: Vec<_> = all.iter().filter(|l| l.vfs == Some(want)).collect();
        assert_eq!(mine.len(), 1, "{want:?} in {all:?}");
        let inode = listening_inode(&path).unwrap();
        assert_eq!(inode, u64::from(mine[0].inode));
        // The socket inode is the one the fd table names.
        let fd = std::os::fd::AsRawFd::as_raw_fd(&listener);
        assert_eq!(
            std::fs::read_link(format!("/proc/self/fd/{fd}")).unwrap(),
            PathBuf::from(format!("socket:[{inode}]"))
        );
        assert_eq!(holds_socket(std::process::id(), inode), Ok(()));
        drop(listener);
        std::fs::remove_file(&path).unwrap();
        assert!(
            listening_inode(&path).is_err(),
            "nothing is bound there now"
        );
    }

    /// F9: another process's listener renamed over the registered path is the socket at that
    /// path; the registered process (this one, still listening on the socket it bound under that
    /// same name) does not hold it, so the endpoint is refused. Selecting by bound name, as
    /// `/proc/net/unix` would, picks this process's own listener instead.
    #[cfg(target_os = "linux")]
    #[test]
    fn f9_a_listener_renamed_over_the_path_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("l.sock");
        let _mine = UnixListener::bind(&path).unwrap();
        let other = t.path().join("o.sock");
        let (_a, b) = orphaned_listener(&other);
        let _kill = Kill(b);
        std::fs::rename(&other, &path).unwrap();
        let theirs = listening_inode(&path).unwrap();
        assert_eq!(
            holds_socket(b, theirs),
            Ok(()),
            "B holds the socket at the path"
        );
        assert!(holds_socket(std::process::id(), theirs).is_err());
        let stream = UnixStream::connect(&path).unwrap();
        let got = trust_endpoint_with(stream.as_fd(), &path, std::process::id(), false, soon());
        assert!(got.is_err(), "{got:?}");
    }

    /// F9: `st_dev` back to the kernel's `MAJOR << 20 | MINOR`.
    #[cfg(target_os = "linux")]
    #[test]
    fn f9_st_dev_converts_to_the_kernel_encoding() {
        assert_eq!(vfs_dev(libc::makedev(8, 1)), (8 << 20) | 1);
        assert_eq!(vfs_dev(libc::makedev(0, 300)), 300);
        assert_eq!(vfs_dev(libc::makedev(259, 70_000)), (259 << 20) | 70_000);
    }

    /// A deadline for the macOS accepted-connection proof (F10).
    fn soon() -> std::time::Instant {
        std::time::Instant::now() + std::time::Duration::from_millis(200)
    }

    /// F10 (macOS): a listening socket bound under the endpoint's name proves nothing about the
    /// connection. Here the registered endpoint (this process) still holds a listener bound to
    /// `path`, so the name proof passes, but the socket at `path` is another process's (B, which
    /// renamed its own over it) and B accepted the connection: the endpoint never holds the
    /// connection's server end, so it is refused once the deadline passes. When the endpoint
    /// itself accepts, it is found.
    #[cfg(target_os = "macos")]
    #[test]
    fn f10_the_endpoint_must_hold_the_accepted_connection() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("l.sock");
        let mine = UnixListener::bind(&path).unwrap();
        let other = t.path().join("o.sock");
        let (_a, b) = orphaned_listener_serving(&other, true);
        let _kill = Kill(b);
        std::fs::rename(&other, &path).unwrap();
        let me = std::process::id();
        assert_eq!(
            holds_listener(me, &path),
            Ok(()),
            "the name proof is fooled"
        );
        let stream = UnixStream::connect(&path).unwrap();
        let server = server_end(stream.as_fd()).unwrap();
        wait_until("B never accepted", || {
            any_socket(b, |s| s.so() == server) == Ok(true)
        });
        let start = std::time::Instant::now();
        assert_eq!(accepted(me, server, soon()), Err(Refusal::NotAccepted));
        assert!(start.elapsed() >= std::time::Duration::from_millis(150));
        // The registered endpoint accepting its own connection is found at once.
        std::fs::remove_file(&path).unwrap();
        drop(mine);
        let listener = UnixListener::bind(&path).unwrap();
        let stream = UnixStream::connect(&path).unwrap();
        let server = server_end(stream.as_fd()).unwrap();
        assert_eq!(
            accepted(me, server, std::time::Instant::now()),
            Err(Refusal::NotAccepted),
            "not accepted yet"
        );
        let _accepted = listener.accept().unwrap();
        assert_eq!(accepted(me, server, std::time::Instant::now()), Ok(()));
        assert_eq!(
            trust_endpoint_with(stream.as_fd(), &path, me, true, soon()),
            Ok(())
        );
    }

    struct Kill(u32);
    impl Drop for Kill {
        fn drop(&mut self) {
            // SAFETY: SIGKILL to a pid this test forked.
            unsafe { libc::kill(self.0 as libc::pid_t, libc::SIGKILL) };
        }
    }

    /// F2: the listening process itself is trusted, through the pidfd and through the Linux
    /// fallback, and a wrong pid is refused.
    #[test]
    fn f2_the_process_holding_the_listener_is_trusted() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("l.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let stream = UnixStream::connect(&path).unwrap();
        // macOS proves the connection by its accepted end in the endpoint's fd table (F10).
        let _accepted = listener.accept().unwrap();
        let me = std::process::id();
        for pidfd in [true, false] {
            assert_eq!(
                trust_endpoint_with(stream.as_fd(), &path, me, pidfd, soon()),
                Ok(())
            );
            assert!(trust_endpoint_with(stream.as_fd(), &path, me + 1, pidfd, soon()).is_err());
        }
        #[cfg(target_os = "linux")]
        {
            let pinned = peer_pidfd(stream.as_fd());
            // CI runs Linux 6.8, which has SO_PEERPIDFD; older kernels take the fallback.
            if let Ok(p) = &pinned {
                assert_eq!(pidfd_names(p, me), Ok(()));
                assert!(pidfd_names(p, me + 1).is_err());
            } else {
                eprintln!("no SO_PEERPIDFD here: {pinned:?}");
            }
            assert_eq!(leader(me), Ok(()));
        }
    }

    /// AC-H110: the macOS binding answers for this process and a `sandbox-exec` child; a token
    /// whose pid version is not the live process's (a successor at that pid) answers confined.
    #[cfg(target_os = "macos")]
    #[test]
    fn sandbox_check_tells_a_sandboxed_child_from_this_process() {
        assert!(sandbox_check(std::process::id()).is_some(), "symbol found");
        // -1 (an error) and a missing symbol are Unknown, which an unconfined glen refuses.
        for rc in [Some(-1), None] {
            assert!(matches!(sandbox_answer("x", rc), Confinement::Unknown(_)));
        }
        if *self_confinement() != Confinement::Unconfined {
            return skip(&format!("this test process is {:?}", self_confinement()));
        }
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("t.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let stream = UnixStream::connect(&path).unwrap();
        let token = peer_token(stream.as_fd()).unwrap();
        assert_eq!(token.val[5], std::process::id());
        assert_eq!(sandbox_check_by_token(token), Some(0));
        let mut stale = token;
        stale.val[7] = stale.val[7].wrapping_add(1000);
        assert_eq!(
            sandbox_check_by_token(stale),
            Some(1),
            "a stale token fails closed"
        );
        let mut caged = std::process::Command::new("/usr/bin/sandbox-exec")
            .args(["-p", "(version 1)(allow default)", "/bin/sleep", "30"])
            .spawn()
            .unwrap();
        let mut free = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        // sandbox-exec applies the profile, then execs: wait until the image is sleep.
        wait_until("sandbox-exec never exec'd", || {
            image(caged.id()).is_some_and(|p| p.ends_with("sleep"))
        });
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
            return skip(&format!("this test process is {:?}", self_confinement()));
        }
        let mut caged = std::process::Command::new("setpriv")
            .args(["--no-new-privs", "/bin/sleep", "30"])
            .spawn();
        let Ok(caged) = caged.as_mut() else {
            return skip("setpriv is not installed");
        };
        wait_until("setpriv never exec'd", || {
            image(caged.id()).is_some_and(|p| p.ends_with("sleep"))
        });
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
