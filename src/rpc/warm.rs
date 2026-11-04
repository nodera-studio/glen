//! The opt-in warm worker (v0.2 W4, design §A21): settings, the match key, the on-disk
//! registry and release request, the claim, the idle loop and its one teardown.
use super::{
    reply::PendingReply,
    worker::{Failure, Worker},
};
use crate::{
    env::Env,
    error::{ErrorCode, GlenError},
    glen_home::{GlenHome, write_private_atomic},
    jobs::{
        Job, JobId, JobStore, State, Status,
        liveness::{self, Liveness},
        lock::lock_key,
    },
};
use omp_rpc::{Client, Event};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering, mpsc},
    time::{Duration, Instant, SystemTime},
};

/// The longest a worker may stay warm after a job (`--warm`, `GLEN_WARM`).
pub const MAX_WARM: Duration = Duration::from_secs(600);
/// One idle-loop tick: a queued job waits at most this long behind an idle warm worker.
pub const CLAIM_TICK: Duration = Duration::from_millis(100);
/// How often an idle warm worker touches its registry (the heartbeat is the file's mtime).
pub const HEARTBEAT: Duration = Duration::from_secs(1);
/// An idle registry whose mtime is older than this has a stale heartbeat.
pub const STALE: Duration = Duration::from_secs(5);
/// The longest teardown waits for `Client::close` before it frees the session anyway.
pub const TEARDOWN_BOUND: Duration = Duration::from_millis(3500);
/// Teardown's one deadline, from its start to the session lock's release: the close gets at most
/// [`TEARDOWN_BOUND`] of it and the metadata cleanup the rest (never more than its own bound).
pub const TEARDOWN_DEADLINE: Duration = Duration::from_millis(3600);
/// Variables that differ between otherwise equal environments; `env_digest` skips them, and
/// every `GLEN_` variable.
pub const VOLATILE_ENV: &[&str] = &[
    "_",
    "PWD",
    "OLDPWD",
    "SHLVL",
    "COLUMNS",
    "LINES",
    "TERM_SESSION_ID",
];

/// `--warm`'s value parser: a `parse_duration` value of at most 10m (`0` means off).
pub fn parse_warm(raw: &str) -> Result<Duration, String> {
    let d = crate::cli::parse_duration(raw)?;
    if d > MAX_WARM {
        Err("warm duration must be at most 10m (0 = off)".into())
    } else {
        Ok(d)
    }
}

/// The effective warm duration of a send: the flag if given (even `0`), else `GLEN_WARM`, else
/// off. `None` means off. A bad or over-cap `GLEN_WARM` is `invalid_env`.
pub fn resolve_warm(flag: Option<Duration>, env: &Env) -> Result<Option<Duration>, GlenError> {
    let d = match flag {
        Some(d) => d,
        None => match env.non_empty("GLEN_WARM") {
            None => return Ok(None),
            Some(raw) => parse_warm(raw.trim()).map_err(|e| {
                GlenError::new(ErrorCode::InvalidEnv, format!("GLEN_WARM: {e}"))
                    .with_hint("set GLEN_WARM to a duration such as 5m (at most 10m), or unset it")
                    .with_detail("variable", "GLEN_WARM")
            })?,
        },
    };
    Ok((!d.is_zero()).then_some(d))
}

/// A warm duration as `job.json` records it, from a `send`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setting {
    pub duration: Duration,
    /// Given as `--warm`, not through `GLEN_WARM`: only an explicit flag warns on the live route.
    pub explicit: bool,
}

/// xxh64 (seed 0) of `env`'s variables sorted by name, without [`VOLATILE_ENV`] and `GLEN_`
/// variables, as 16 lowercase hex digits. Each name and each value is length-prefixed (u64
/// little-endian), so no value can mimic another variable. No value is ever recorded, only
/// this digest.
pub fn env_digest(env: &Env) -> String {
    let mut bytes = Vec::new();
    for (k, v) in env.iter() {
        if VOLATILE_ENV.contains(&k) || k.starts_with("GLEN_") {
            continue;
        }
        for part in [k, v] {
            bytes.extend_from_slice(&(part.len() as u64).to_le_bytes());
            bytes.extend_from_slice(part.as_bytes());
        }
    }
    format!("{:016x}", xxhash_rust::xxh64::xxh64(&bytes, 0))
}

/// What a queued job must share with the warm worker's own job to run on its omp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub omp_bin: Option<std::path::PathBuf>,
    pub profile_flag: Option<String>,
    pub thinking_flag: Option<String>,
    pub cwd: std::path::PathBuf,
    pub env_digest: String,
}
impl Key {
    /// `None` for a job without an `env_digest` (written before v0.2 W4): it never goes warm.
    pub fn of(job: &Job) -> Option<Self> {
        Some(Self {
            omp_bin: job.omp_bin.clone(),
            profile_flag: job.profile_flag.clone(),
            thinking_flag: job.thinking_flag.clone(),
            cwd: job.cwd.clone(),
            env_digest: job.env_digest.clone()?,
        })
    }
    /// Exact on every field but `cwd`, which compares as a canonical path (a `@new --cwd` through
    /// a symlink and the session header's cwd name one directory).
    pub fn matches(&self, job: &Job) -> bool {
        job.env_digest.as_deref() == Some(self.env_digest.as_str())
            && job.omp_bin == self.omp_bin
            && job.profile_flag == self.profile_flag
            && job.thinking_flag == self.thinking_flag
            && same_dir(&job.cwd, &self.cwd)
    }
}
fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The stale bound readers use; debug builds read `GLEN_WARM_STALE_MS` (100 to 30000) so tests
/// need not wait 5 s.
pub fn stale_bound(env: &Env) -> Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = env
        .get("GLEN_WARM_STALE_MS")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| (100..=30_000).contains(ms))
    {
        return Duration::from_millis(ms);
    }
    let _ = env;
    STALE
}

/// `run/warm/<lock_key>.json` (design §A21): written atomically at each transition; its mtime is
/// the heartbeat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub v: u32,
    pub pid: u32,
    #[serde(default)]
    pub start: Option<String>,
    pub session_id: String,
    #[serde(default)]
    pub session_handle: Option<String>,
    /// The job that made the worker warm, or the job it runs or ran last.
    pub job_id: String,
    #[serde(default)]
    pub omp_pid: Option<u32>,
    #[serde(default)]
    pub omp_start: Option<String>,
    pub warm_ms: u64,
    pub expires_at: String,
    pub state: EntryState,
    pub started_at: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    Idle,
    Busy,
}
/// How a reader sees an entry: `Dead` when the worker's pid or start token no longer match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Fresh,
    Stale,
    Dead,
}
impl Class {
    pub fn as_str(self, entry: &Entry) -> &'static str {
        match (self, entry.state) {
            (Self::Dead, _) => "dead",
            (Self::Stale, _) => "stale",
            (Self::Fresh, EntryState::Idle) => "idle",
            (Self::Fresh, EntryState::Busy) => "busy",
        }
    }
}
/// A registry and its heartbeat (the file's mtime).
#[derive(Clone, Debug)]
pub struct Indexed {
    pub entry: Entry,
    pub mtime: SystemTime,
}
impl Entry {
    /// `Unknown` liveness counts as alive, as everywhere else in glen; only a proven change is
    /// `Dead`. A busy worker is never stale: it heartbeats only while idle.
    pub fn class(&self, mtime: SystemTime, now: SystemTime, stale: Duration) -> Class {
        let dead = match &self.start {
            Some(start) => liveness::check(self.pid, start) == Liveness::Dead,
            None => matches!(liveness::observe(self.pid), liveness::Probe::Gone),
        };
        if dead {
            Class::Dead
        } else if self.state == EntryState::Idle
            && now.duration_since(mtime).unwrap_or_default() > stale
        {
            Class::Stale
        } else {
            Class::Fresh
        }
    }
    /// The `glen ls` `state_detail` for a live entry; `None` for a dead one.
    pub fn detail(&self, class: Class, mtime: SystemTime, now: SystemTime) -> Option<String> {
        match (class, self.state) {
            (Class::Dead, _) => None,
            (Class::Stale, _) => Some(format!(
                "warm worker, no heartbeat for {}s",
                now.duration_since(mtime).unwrap_or_default().as_secs()
            )),
            (Class::Fresh, EntryState::Busy) => Some("warm worker, busy".into()),
            (Class::Fresh, EntryState::Idle) => {
                let left = self
                    .expires_at
                    .parse::<jiff::Timestamp>()
                    .map(|t| {
                        let ms = t.as_millisecond().saturating_sub(
                            jiff::Timestamp::try_from(now)
                                .map(|n| n.as_millisecond())
                                .unwrap_or(0),
                        );
                        (ms.max(0) as u64).div_ceil(1000)
                    })
                    .unwrap_or(0);
                Some(format!("warm worker, idle, {left}s left"))
            }
        }
    }
    /// The identity a release request must carry to reach this worker.
    pub fn ident(&self) -> String {
        ident(self.pid, self.start.as_deref())
    }
}
/// `<pid>:<start token>`, the body of a release request (an unknown token is empty).
pub fn ident(pid: u32, start: Option<&str>) -> String {
    format!("{pid}:{}", start.unwrap_or(""))
}
fn dir(home: &GlenHome) -> PathBuf {
    home.root.join("run/warm")
}
pub fn path(home: &GlenHome, session_id: &str) -> PathBuf {
    dir(home).join(format!("{}.json", lock_key(session_id)))
}
pub fn release_path(home: &GlenHome, session_id: &str) -> PathBuf {
    dir(home).join(format!("{}.release", lock_key(session_id)))
}
/// `run/warm/<lock_key>.lock`, the session's metadata lock file.
fn meta_path(home: &GlenHome, session_id: &str) -> PathBuf {
    dir(home).join(format!("{}.lock", lock_key(session_id)))
}
/// The longest a caller waits for the metadata lock: every holder keeps it for one read, compare
/// and write or unlink, so only a stopped holder outlasts this.
const META_BOUND: Duration = Duration::from_secs(1);
/// The session's metadata lock (design §A21): a flock independent of the session lock, taken by
/// every publication and removal of the registry and the release request and held only for that
/// one step, never across a wait. A conditional removal therefore unlinks exactly the file it
/// compared: no publish or other removal can land in between.
pub(crate) struct MetaLock {
    fd: std::os::fd::OwnedFd,
    path: PathBuf,
    /// The registry and the release request it guards.
    guards: [PathBuf; 2],
}
/// The lock file lives only while a file it guards does: the holder that leaves neither unlinks
/// it before it unlocks, and [`meta_lock`] retries a lock taken on an unlinked file. Unlocks
/// explicitly, as `jobs::lock::SessionLock` does: a child forked on another thread shares the
/// description until its exec.
impl Drop for MetaLock {
    fn drop(&mut self) {
        let gone = |p: &PathBuf| matches!(fs::symlink_metadata(p), Err(e) if e.kind() == io::ErrorKind::NotFound);
        if self.guards.iter().all(gone) {
            let _ = fs::remove_file(&self.path);
        }
        let _ = rustix::fs::flock(&self.fd, rustix::fs::FlockOperation::Unlock);
    }
}
/// Takes the metadata lock, waiting at most [`META_BOUND`]. `None` when `run/warm` does not
/// exist: there is nothing to remove, and a publish fails on its own write.
fn meta_lock(home: &GlenHome, session_id: &str) -> io::Result<Option<MetaLock>> {
    meta_lock_until(home, session_id, Instant::now() + META_BOUND)
}
/// [`meta_lock`], waiting until `end` (one attempt even when `end` has passed). A lock still busy
/// then is a `TimedOut` error.
fn meta_lock_until(
    home: &GlenHome,
    session_id: &str,
    end: Instant,
) -> io::Result<Option<MetaLock>> {
    use rustix::{
        fs::{FlockOperation, Mode, OFlags, flock, fstat, lstat, open},
        io::Errno,
    };
    let file = meta_path(home, session_id);
    let context = |what: &str, e: Errno| {
        io::Error::new(
            io::Error::from(e).kind(),
            format!("cannot {what} {}: {e}", file.display()),
        )
    };
    let started = Instant::now();
    loop {
        let fd = match open(
            &file,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        ) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Ok(None),
            Err(e) => return Err(context("open", e)),
        };
        #[cfg(test)]
        if let Some(hook) = BETWEEN_OPEN_AND_FLOCK.take() {
            hook();
        }
        match flock(&fd, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {
                // The holder before this one may have unlinked the file this fd locks.
                let linked = match (fstat(&fd), lstat(&file)) {
                    (Ok(held), Ok(now)) => (held.st_dev, held.st_ino) == (now.st_dev, now.st_ino),
                    _ => false,
                };
                if linked {
                    return Ok(Some(MetaLock {
                        fd,
                        path: file.clone(),
                        guards: [path(home, session_id), release_path(home, session_id)],
                    }));
                }
                let _ = flock(&fd, FlockOperation::Unlock);
            }
            Err(Errno::WOULDBLOCK) => std::thread::sleep(Duration::from_millis(1)),
            Err(e) => return Err(context("lock", e)),
        }
        if Instant::now() >= end {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "cannot lock {}: still held after {}ms",
                    file.display(),
                    started.elapsed().as_millis()
                ),
            ));
        }
    }
}
#[cfg(test)]
thread_local! {
    /// Test-only seam: runs once, in this thread's next [`meta_lock`] attempt, between its open
    /// and its flock, so a test can replace the file the attempt just opened.
    static BETWEEN_OPEN_AND_FLOCK: std::cell::Cell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::Cell::new(None) };
}
/// Test-only: the session's metadata lock, held until dropped, as by a holder stopped inside its
/// step (another fd's flock, so this process's own callers contend for it too).
#[cfg(test)]
pub(crate) fn hold_meta_lock(home: &GlenHome, session_id: &str) -> MetaLock {
    home.ensure_dir("run/warm").unwrap();
    meta_lock(home, session_id).unwrap().unwrap()
}
/// Atomic 0600 write in the 0700 `run/warm` directory, under the metadata lock.
pub fn publish(home: &GlenHome, entry: &Entry) -> io::Result<()> {
    home.ensure_dir("run/warm")?;
    let _meta = meta_lock(home, &entry.session_id)?;
    write_private_atomic(&path(home, &entry.session_id), &serde_json::to_vec(entry)?)
}
/// The heartbeat: moves the registry's mtime without rewriting it. A missing file or a
/// symlink (never followed) is ignored. It takes no metadata lock: it never creates the file, so
/// at worst it moves the mtime of a registry a removal just unlinked.
pub fn touch(home: &GlenHome, session_id: &str) {
    use rustix::fs::{Mode, OFlags, open};
    let Ok(fd) = open(
        path(home, session_id),
        OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return;
    };
    let file = fs::File::from(fd);
    if file.metadata().is_ok_and(|m| m.is_file()) {
        let _ = file.set_modified(SystemTime::now());
    }
}
fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}
/// Removes the registry under the metadata lock; a missing one is fine.
pub fn remove(home: &GlenHome, session_id: &str) -> io::Result<()> {
    let Some(_meta) = meta_lock(home, session_id)? else {
        return Ok(());
    };
    remove_file(&path(home, session_id))
}
/// Removes the registry only while it still names the worker `ident` (see [`ident`]); true when
/// it did. It needs no session lock: the read, the compare and the unlink all run under the
/// metadata lock, so a successor's registry is never removed.
pub fn remove_if(home: &GlenHome, session_id: &str, ident: &str) -> io::Result<bool> {
    let Some(_meta) = meta_lock(home, session_id)? else {
        return Ok(false);
    };
    if read(home, session_id).is_none_or(|i| i.entry.ident() != ident) {
        return Ok(false);
    }
    remove_file(&path(home, session_id)).map(|()| true)
}
fn read_regular(path: &Path) -> Option<(Vec<u8>, SystemTime)> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    Some((bytes, meta.modified().ok()?))
}
pub fn read(home: &GlenHome, session_id: &str) -> Option<Indexed> {
    let (bytes, mtime) = read_regular(&path(home, session_id))?;
    parse(&bytes, session_id).map(|entry| Indexed { entry, mtime })
}
fn parse(bytes: &[u8], session_id: &str) -> Option<Entry> {
    serde_json::from_slice::<Entry>(bytes)
        .ok()
        .filter(|e| e.v == 1 && e.pid > 0 && e.session_id == session_id)
}
/// What [`index`] found in `run/warm`.
#[derive(Debug, Default)]
pub struct Index {
    /// Registries named for their own session (`<lock_key(session_id)>.json`), by session id.
    pub entries: Vec<Indexed>,
    /// Registry files that cannot be read or parsed (torn, foreign, a symlink): a worker may
    /// still run behind one, so callers surface them and never treat them as absent.
    pub unreadable: Vec<PathBuf>,
    /// Parsable registries under another session's name (S-09): never a release or kill
    /// target, never deleted; `doctor` reports them.
    pub misnamed: Vec<PathBuf>,
}
/// Every registry in `run/warm`, sorted out. Release requests, metadata lock files and
/// atomic-write temporaries are not registries. A forged file can name any pid, so only one
/// at its session's own path counts (the path every glen writer uses).
pub fn index(home: &GlenHome) -> Index {
    let mut index = Index::default();
    for e in fs::read_dir(dir(home)).into_iter().flatten().flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        match read_regular(&e.path()).and_then(|(bytes, mtime)| {
            serde_json::from_slice::<Entry>(&bytes)
                .ok()
                .filter(|entry| entry.v == 1 && entry.pid > 0)
                .map(|entry| Indexed { entry, mtime })
        }) {
            Some(i) if name == format!("{}.json", lock_key(&i.entry.session_id)) => {
                index.entries.push(i)
            }
            Some(_) => index.misnamed.push(e.path()),
            None => index.unreadable.push(e.path()),
        }
    }
    index
        .entries
        .sort_by(|a, b| a.entry.session_id.cmp(&b.entry.session_id));
    index.unreadable.sort();
    index.misnamed.sort();
    index
}
/// Asks the worker named by `ident` (see [`ident`]) to cool, under the metadata lock; true once
/// the request is written. Rewriting refreshes the request. A lock still busy after
/// [`META_BOUND`] has a stopped holder (such as a worker SIGSTOPped as it published): nothing is
/// written and the result is false, so the caller never reports the request delivered; its own
/// wait, or `glen release --force`, decides as for a worker that never answers.
pub fn request_release(home: &GlenHome, session_id: &str, ident: &str) -> io::Result<bool> {
    home.ensure_dir("run/warm")?;
    let _meta = match meta_lock(home, session_id) {
        Err(e) if e.kind() == io::ErrorKind::TimedOut => return Ok(false),
        held => held?,
    };
    write_private_atomic(&release_path(home, session_id), ident.as_bytes()).map(|()| true)
}
/// True only for a request addressed to this worker's own identity: a request meant for an
/// earlier worker of the session, or a malformed one, is ignored.
pub fn release_requested(home: &GlenHome, session_id: &str, me: &str) -> bool {
    read_regular(&release_path(home, session_id)).is_some_and(|(bytes, _)| bytes == me.as_bytes())
}
/// Removes the release request under the metadata lock; a missing one is fine.
pub fn clear_release(home: &GlenHome, session_id: &str) -> io::Result<()> {
    let Some(_meta) = meta_lock(home, session_id)? else {
        return Ok(());
    };
    remove_file(&release_path(home, session_id))
}
/// Removes the release request only while it is addressed to `ident`; true when it did. The read,
/// the compare and the unlink run under the metadata lock, so another `glen release`'s request is
/// never removed.
pub fn clear_release_if(home: &GlenHome, session_id: &str, ident: &str) -> io::Result<bool> {
    let Some(_meta) = meta_lock(home, session_id)? else {
        return Ok(false);
    };
    if !release_requested(home, session_id, ident) {
        return Ok(false);
    }
    remove_file(&release_path(home, session_id)).map(|()| true)
}
/// Teardown's cleanup: removes the registry and the release request under one acquisition of the
/// metadata lock, waited for until `end`.
fn remove_both(home: &GlenHome, session_id: &str, end: Instant) -> io::Result<()> {
    let Some(_meta) = meta_lock_until(home, session_id, end)? else {
        return Ok(());
    };
    remove_file(&path(home, session_id))?;
    remove_file(&release_path(home, session_id))
}
/// What `glen release --force` may signal: only a process whose pid and start token are proven
/// unchanged (`Alive`). `Unknown` never signals.
#[derive(Debug, PartialEq, Eq)]
pub struct KillIdentity {
    pub worker: Liveness,
    /// `None` when the entry records no omp. A recorded omp without a start token is `Unknown`
    /// unless its pid is gone.
    pub omp: Option<Liveness>,
}
pub fn kill_identity(entry: &Entry) -> KillIdentity {
    KillIdentity {
        worker: proven(entry.pid, entry.start.as_deref()),
        omp: entry
            .omp_pid
            .map(|pid| proven(pid, entry.omp_start.as_deref())),
    }
}
/// A pid's liveness against its recorded start token; without one only a gone pid is proven
/// (`Dead`), anything else is `Unknown`.
fn proven(pid: u32, start: Option<&str>) -> Liveness {
    match start {
        Some(start) => liveness::check(pid, start),
        None if matches!(liveness::observe(pid), liveness::Probe::Gone) => Liveness::Dead,
        None => Liveness::Unknown,
    }
}

/// Compare-and-claim (design §A21): only a `queued`, not yet reused job is claimed. The claim
/// names this worker as the job's worker in the same status write, so the job's liveness
/// passes from its blocked cold worker to the claimant at once. `None` when refused.
pub fn claim(
    store: &JobStore,
    id: &JobId,
    pid: u32,
    start: Option<&str>,
) -> Result<Option<Status>, GlenError> {
    let (status, wrote) = store.update_status_if(id, |s| {
        if s.status != State::Queued || s.omp_reused {
            return false;
        }
        s.worker_pid = Some(pid);
        s.worker_start = start.map(str::to_owned);
        s.omp_reused = true;
        s.waiting_for_lock = false;
        s.updated_at = jiff::Timestamp::now().to_string();
        true
    })?;
    Ok(wrote.then_some(status))
}

/// Why warmth ended; logged as `warm: cooling (<reason>)` by the one teardown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    Expired,
    Release,
    Endpoint,
    Signal,
    OmpExited,
    Mismatch,
    Moved,
    StateError,
    Ended,
}
impl Reason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::Release => "release",
            Self::Endpoint => "endpoint",
            Self::Signal => "signal",
            Self::OmpExited => "omp_exited",
            Self::Mismatch => "mismatch",
            Self::Moved => "moved",
            Self::StateError => "state_error",
            Self::Ended => "ended",
        }
    }
}
/// What one idle tick observed, in the order the loop checks it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Tick {
    pub cancelled: bool,
    pub omp_exited: bool,
    pub release: bool,
    pub endpoint: bool,
    pub expired: bool,
}
/// The first ending a tick observed: a signal, omp exiting, a release request, a live endpoint
/// for the session, expiry. `None` stays warm (and scans for a queued job).
pub(crate) fn decide(t: &Tick) -> Option<Reason> {
    [
        (t.cancelled, Reason::Signal),
        (t.omp_exited, Reason::OmpExited),
        (t.release, Reason::Release),
        (t.endpoint, Reason::Endpoint),
        (t.expired, Reason::Expired),
    ]
    .into_iter()
    .find_map(|(seen, reason)| seen.then_some(reason))
}
/// What an idle warm worker keeps from its last job: omp, its receivers and the session lock.
pub(crate) struct Warm {
    pub client: Client,
    pub events: mpsc::Receiver<Event>,
    pub replies: mpsc::Receiver<PendingReply>,
    pub lock: crate::jobs::lock::SessionLock,
    pub session_id: String,
    /// The match key of the job that made the worker warm.
    pub key: Key,
}
/// Drains omp's events; true once the channel is closed (omp exited).
fn drain_events(events: &mpsc::Receiver<Event>) -> bool {
    loop {
        match events.try_recv() {
            Ok(_) => (),
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => return true,
        }
    }
}
/// Fails every `glen_reply` that arrived with no job running: it is attributed to nothing.
fn reject_replies(replies: &mpsc::Receiver<PendingReply>) {
    for reply in replies.try_iter() {
        reply.discard(GlenError::new(
            ErrorCode::JobFailed,
            "glen_reply: no glen job is running on this session",
        ));
    }
}
/// A live endpoint registry for the session: a stat, then a read. A live pid or an unreadable
/// registry yields; a dead one is ignored. No hello probe (it alone could take 1 s).
fn endpoint_present(home: &GlenHome, session_id: &str) -> bool {
    use crate::live::registry;
    if !registry::path(home, session_id).exists() {
        return false;
    }
    match registry::read(home, session_id) {
        registry::Read::Missing => false,
        registry::Read::Unreadable(_) => true,
        registry::Read::Ok(r) => !registry::pid_dead(r.pid),
    }
}
fn timestamp_after(d: Duration) -> String {
    jiff::Timestamp::now()
        .checked_add(jiff::SignedDuration::from_millis(
            d.as_millis().min(i64::MAX as u128) as i64,
        ))
        .unwrap_or_else(|_| jiff::Timestamp::now())
        .to_string()
}
/// Appends one line to a job's own `worker.log` (the process stderr is the first job's log).
fn append_log(store: &JobStore, id: &JobId, handle: &str, line: &str) {
    use std::io::Write;
    let path = store.dir(id).join("worker.log");
    let mut opts = fs::OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // S-12: never append through a planted symlink.
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    if let Err(e) = opts
        .open(&path)
        .and_then(|mut f| writeln!(f, "{} {handle} {line}", jiff::Timestamp::now()))
    {
        eprintln!("cannot write {}: {e}", path.display());
    }
}
/// Records this process as the lock's holder: `Warm` while idle, `Job` for an adopted job (so a
/// control reaches that job through its control inbox).
fn hold(
    lock: &crate::jobs::lock::SessionLock,
    kind: crate::jobs::lock::HolderKind,
    job_id: &JobId,
    start: Option<&str>,
) -> io::Result<()> {
    crate::jobs::lock::mark(
        lock,
        &crate::jobs::lock::Holder {
            v: 1,
            kind,
            job_id: Some(job_id.as_str().into()),
            pid: std::process::id(),
            start: start.map(str::to_owned),
        },
    )
}
/// The steal scan's outcome: the oldest queued RPC job of the warm session, if any.
#[derive(Debug)]
pub(crate) enum Found {
    Nothing,
    Adopt(JobId, Box<Job>),
    /// The oldest candidate differs in its match key: the worker cools and it runs cold.
    Mismatch(JobId),
}
enum Seen {
    Candidate(Box<Job>),
    Irrelevant,
    Retry,
}
/// The steal scan with its cache (design §A21): ids classified for good (terminal, not queued,
/// reused, another route or session) are skipped, so an idle tick costs one `read_dir`. A read
/// error is retried on the next tick, at most [`Scan::RETRIES`] times: a job directory is
/// visible before `create` wrote both of its files.
#[derive(Default)]
pub(crate) struct Scan {
    settled: BTreeSet<JobId>,
    errors: BTreeMap<JobId, u32>,
}
impl Scan {
    pub(crate) const RETRIES: u32 = 20;
    pub(crate) fn next(&mut self, store: &JobStore, session_id: &str, key: &Key) -> Found {
        let mut oldest: Option<(JobId, Box<Job>)> = None;
        for id in store.list() {
            if self.settled.contains(&id) {
                continue;
            }
            match seen(store, &id, session_id) {
                Seen::Candidate(job) => {
                    if oldest.as_ref().is_none_or(|(o, _)| id < *o) {
                        oldest = Some((id, job));
                    }
                }
                Seen::Irrelevant => {
                    self.errors.remove(&id);
                    self.settled.insert(id);
                }
                Seen::Retry => {
                    let n = self.errors.entry(id.clone()).or_default();
                    *n += 1;
                    if *n >= Self::RETRIES {
                        self.errors.remove(&id);
                        self.settled.insert(id);
                    }
                }
            }
        }
        match oldest {
            None => Found::Nothing,
            Some((id, job)) if key.matches(&job) => Found::Adopt(id, job),
            Some((id, _)) => Found::Mismatch(id),
        }
    }
}
fn seen(store: &JobStore, id: &JobId, session_id: &str) -> Seen {
    let Ok(s) = store.read_status(id) else {
        return Seen::Retry;
    };
    if s.status != State::Queued || s.omp_reused {
        return Seen::Irrelevant;
    }
    let Ok(job) = store.read_job(id) else {
        return Seen::Retry;
    };
    if job.route != crate::jobs::Route::Rpc || job.session_id.as_deref() != Some(session_id) {
        return Seen::Irrelevant;
    }
    Seen::Candidate(Box::new(job))
}
/// The idle phase after a job that stays warm (design §A21). It never writes the status of a
/// job it is not running: every failure here ends warmth through [`teardown`]. `Err` only when
/// an adopted job panicked, so the worker exits non-zero.
pub(crate) fn idle(first: &Worker, warm: Warm) -> Result<(), Failure> {
    let me = liveness::current();
    let mut idler = Idler {
        first,
        home: first.store.home.clone(),
        start: me.as_ref().map(|i| i.start.clone()),
        my_ident: ident(std::process::id(), me.as_ref().map(|i| i.start.as_str())),
        entry: Entry {
            v: 1,
            pid: std::process::id(),
            start: me.map(|i| i.start),
            session_id: warm.session_id.clone(),
            session_handle: Some(super::session_handle(&warm.session_id)),
            job_id: first.id.as_str().into(),
            omp_pid: warm.client.pid(),
            omp_start: warm
                .client
                .pid()
                .and_then(|p| match liveness::start_token(p) {
                    liveness::Probe::Running(t) => Some(t),
                    _ => None,
                }),
            warm_ms: first.job.warm_ms.unwrap_or(0),
            expires_at: String::new(),
            state: EntryState::Idle,
            started_at: jiff::Timestamp::now().to_string(),
        },
        last: None,
        panicked: false,
    };
    let reason = idler.run(&warm);
    let Idler {
        home,
        last,
        panicked,
        ..
    } = idler;
    let Warm {
        client,
        lock,
        session_id,
        ..
    } = warm;
    let log = |line: &str| {
        first.log("warm", line);
        if let Some(w) = &last {
            append_log(&w.store, &w.id, &w.job.handle, &format!("warm: {line}"));
        }
    };
    teardown(
        &home,
        &session_id,
        reason,
        move || client.close().err().map(|e| e.to_string()),
        move || drop(lock),
        &log,
    );
    if panicked {
        Err(Failure::new(
            "internal",
            "a job the warm worker adopted panicked",
        ))
    } else {
        Ok(())
    }
}
struct Idler<'a> {
    first: &'a Worker,
    home: GlenHome,
    start: Option<String>,
    my_ident: String,
    entry: Entry,
    /// The last job the worker adopted; its control inbox is answered while idle.
    last: Option<Worker>,
    panicked: bool,
}
impl Idler<'_> {
    fn log(&self, line: &str) {
        self.first.log("warm", line);
    }
    fn last(&self) -> &Worker {
        self.last.as_ref().unwrap_or(self.first)
    }
    /// Publishes the idle registry, then flips the holder to `warm` (a reader that sees holder
    /// `warm` finds the entry). The window opens now, after the last job's terminal status.
    fn go_idle(&mut self, warm: &Warm) -> Result<Instant, Reason> {
        let (job_id, warm_ms) = {
            let last = self.last();
            (last.id.as_str().to_owned(), last.job.warm_ms.unwrap_or(0))
        };
        self.entry.job_id = job_id;
        self.entry.warm_ms = warm_ms;
        let window = Duration::from_millis(self.entry.warm_ms);
        self.entry.expires_at = timestamp_after(window);
        self.entry.state = EntryState::Idle;
        let expires = Instant::now() + window;
        if let Err(e) = publish(&self.home, &self.entry) {
            self.log(&format!(
                "cannot publish {}: {e}",
                path(&self.home, &self.entry.session_id).display()
            ));
            return Err(Reason::StateError);
        }
        let id = self.last().id.clone();
        if let Err(e) = hold(
            &warm.lock,
            crate::jobs::lock::HolderKind::Warm,
            &id,
            self.start.as_deref(),
        ) {
            self.log(&format!("cannot mark the warm holder: {e}"));
            return Err(Reason::StateError);
        }
        // Settlement barrier: nothing the last job left behind reaches the next one.
        drain_events(&warm.events);
        reject_replies(&warm.replies);
        self.log(&format!(
            "idle for {}ms on {}",
            self.entry.warm_ms, self.entry.session_id
        ));
        Ok(expires)
    }
    fn run(&mut self, warm: &Warm) -> Reason {
        let mut expires = match self.go_idle(warm) {
            Ok(t) => t,
            Err(reason) => return reason,
        };
        let mut beat = Instant::now();
        let mut scan = Scan::default();
        loop {
            let tick = Tick {
                cancelled: self.first.cancel.load(Ordering::SeqCst),
                omp_exited: drain_events(&warm.events),
                release: {
                    reject_replies(&warm.replies);
                    release_requested(&self.home, &warm.session_id, &self.my_ident)
                },
                endpoint: endpoint_present(&self.home, &warm.session_id),
                expired: Instant::now() >= expires,
            };
            // A control written for the last job just before it ended is answered `job_ended`.
            self.last().respond_pending();
            if let Some(reason) = decide(&tick) {
                return reason;
            }
            if beat.elapsed() >= HEARTBEAT {
                touch(&self.home, &warm.session_id);
                beat = Instant::now();
            }
            match scan.next(&self.first.store, &warm.session_id, &warm.key) {
                Found::Nothing => std::thread::sleep(CLAIM_TICK),
                Found::Mismatch(id) => {
                    self.log(&format!("{} differs from the warm omp", id.handle()));
                    return Reason::Mismatch;
                }
                Found::Adopt(id, job) => match self.adopt(warm, id, *job) {
                    Adopted::End(reason) => return reason,
                    // Nothing ran: the window and its expiry stay as they were.
                    Adopted::Refused => (),
                    Adopted::Ran => {
                        expires = match self.go_idle(warm) {
                            Ok(t) => t,
                            Err(reason) => return reason,
                        };
                        beat = Instant::now();
                    }
                },
            }
        }
    }
    /// Claims and runs one queued job on the warm omp: `Ran` when the worker stays warm after
    /// it, `Refused` when the claim lost (nothing ran; the scan moves on next tick).
    fn adopt(&mut self, warm: &Warm, id: JobId, job: Job) -> Adopted {
        // (1) The omp process still drives the warm session.
        let state = match super::worker::query_state(&warm.client, Duration::from_secs(1)) {
            Ok(s) if s.session_id == warm.session_id => s,
            Ok(s) => {
                self.log(&format!("omp moved to session {}", s.session_id));
                return Adopted::End(Reason::Moved);
            }
            Err(e) => {
                self.log(&format!("cannot check the warm session: {e}"));
                return Adopted::End(Reason::StateError);
            }
        };
        // (2) The holder names the job before the claim, so a control never sees `warm` while a
        // claimed job is about to run; (3) the claim itself.
        let warm_holder = |this: &Self| {
            hold(
                &warm.lock,
                crate::jobs::lock::HolderKind::Warm,
                &this.last().id,
                this.start.as_deref(),
            )
        };
        if let Err(e) = hold(
            &warm.lock,
            crate::jobs::lock::HolderKind::Job,
            &id,
            self.start.as_deref(),
        ) {
            self.log(&format!("cannot mark the holder for {}: {e}", id.handle()));
            return Adopted::End(Reason::StateError);
        }
        let status = match claim(
            &self.first.store,
            &id,
            std::process::id(),
            self.start.as_deref(),
        ) {
            Ok(Some(status)) => status,
            Ok(None) => {
                return match warm_holder(self) {
                    Ok(()) => Adopted::Refused,
                    Err(e) => {
                        self.log(&format!("cannot mark the warm holder: {e}"));
                        Adopted::End(Reason::StateError)
                    }
                };
            }
            Err(e) => {
                self.log(&format!("cannot claim {}: {e}", id.handle()));
                return Adopted::End(Reason::StateError);
            }
        };
        // (4) Busy: committed to the job from here; any failure ends it through its own path.
        self.entry.state = EntryState::Busy;
        self.entry.job_id = id.as_str().into();
        if let Err(e) = publish(&self.home, &self.entry) {
            self.log(&format!("cannot publish the busy registry: {e}"));
        }
        let (worker, panicked) = run_adopted(self.first, warm, id, job, status, &state);
        let keep = worker.keep_warm();
        self.last = Some(worker);
        if panicked {
            self.panicked = true;
            return Adopted::End(Reason::StateError);
        }
        if self.first.cancel.load(Ordering::SeqCst) {
            return Adopted::End(Reason::Signal);
        }
        if keep {
            Adopted::Ran
        } else {
            Adopted::End(Reason::Ended)
        }
    }
}
/// What [`Idler::adopt`] did with a found job.
enum Adopted {
    /// The job ran and the worker stays warm: the window re-arms after it.
    Ran,
    /// Another worker claimed the job first; nothing ran.
    Refused,
    /// Warmth ends.
    End(Reason),
}
/// Runs a claimed job on the warm omp with its own `Worker`, sink and deadline. A panic fails
/// that job, never the one that made the worker warm.
fn run_adopted(
    first: &Worker,
    warm: &Warm,
    id: JobId,
    job: Job,
    status: Status,
    state: &omp_rpc::SessionState,
) -> (Worker, bool) {
    let now = jiff::Timestamp::now();
    let left = job
        .deadline_at
        .parse::<jiff::Timestamp>()
        .map(|d| {
            d.as_millisecond()
                .saturating_sub(now.as_millisecond())
                .max(0) as u64
        })
        .unwrap_or(0);
    let waited = job
        .created_at
        .parse::<jiff::Timestamp>()
        .map(|c| {
            now.as_millisecond()
                .saturating_sub(c.as_millisecond())
                .max(0) as u64
        })
        .unwrap_or(0);
    let mut w = Worker {
        store: first.store.clone(),
        id,
        job,
        status,
        cancel: Arc::clone(&first.cancel),
        deadline: Instant::now() + Duration::from_millis(left),
        control_in_flight: None,
        omp_queued: state.queued_message_count > 0,
        settled: false,
    };
    append_log(
        &w.store,
        &w.id,
        &w.job.handle,
        &format!(
            "warm: adopted {} (warm worker pid {})",
            w.job.handle,
            std::process::id()
        ),
    );
    w.log("warm", "adopted on the warm omp");
    w.status.omp_pid = warm.client.pid();
    w.status.rpc_protocol = Some(warm.client.protocol_version());
    w.status.waited_ms = Some(waited);
    if w.job.deliver_as == crate::jobs::Deliver::Steer
        && Duration::from_millis(waited) >= super::TICK
    {
        w.status.warnings.push(crate::error::Warning {
            code: "steer_queued".into(),
            message: "another glen job held the session, so the steer message waited and was sent as a prompt".into(),
        });
    }
    // Barrier: frames and replies from before this job are never its own.
    drain_events(&warm.events);
    reject_replies(&warm.replies);
    let sink = super::reply::ReplySink::new(w.store.clone(), w.store.home.clone(), w.id.clone());
    guarded(w, |w| {
        w.save()?;
        w.execute(
            &warm.client,
            &warm.events,
            &warm.replies,
            state,
            &sink,
            &mut None,
        )
    })
}
/// Runs `body` for an adopted job under its own `catch_unwind`: a failure or a panic is that
/// job's own (`finish_failure`); true when it panicked.
fn guarded(mut w: Worker, body: impl FnOnce(&mut Worker) -> Result<(), Failure>) -> (Worker, bool) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&mut w)));
    let (failure, panicked) = match result {
        Ok(Ok(())) => return (w, false),
        Ok(Err(f)) => (f, false),
        Err(payload) => (
            Failure::new(
                "internal",
                format!(
                    "worker panicked: {}",
                    super::worker::panic_message(payload.as_ref())
                ),
            ),
            true,
        ),
    };
    w.log("failed", &failure.to_string());
    if let Err(e) = w.finish_failure(failure) {
        w.log("failed", &format!("cannot persist the failure: {e}"));
    }
    (w, panicked)
}
/// The one teardown (design §A21), within [`TEARDOWN_DEADLINE`]: close omp (bounded by
/// [`TEARDOWN_BOUND`]), remove the registry and any release request under one acquisition of the
/// metadata lock (so a cleanup's removal never interleaves with them; waited for at most
/// [`META_BOUND`] and never past the deadline), then free the session lock. Normally the registry
/// goes before the lock, so a reader never sees a free lock with a live registry; only a holder
/// that keeps the metadata lock past that wait (a stopped process) leaves both files behind,
/// logged, and the lock is freed anyway within the deadline.
pub(crate) fn teardown(
    home: &GlenHome,
    session_id: &str,
    reason: Reason,
    close: impl FnOnce() -> Option<String> + Send + 'static,
    release_lock: impl FnOnce(),
    log: &dyn Fn(&str),
) {
    let deadline = Instant::now() + TEARDOWN_DEADLINE;
    log(&format!("cooling ({})", reason.as_str()));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(close());
    });
    match rx.recv_timeout(TEARDOWN_BOUND) {
        Ok(None) => (),
        Ok(Some(e)) => log(&format!("close: {e}")),
        Err(_) => log("close exceeded 3.5s (omp group may survive)"),
    }
    if let Err(e) = remove_both(home, session_id, deadline.min(Instant::now() + META_BOUND)) {
        log(&format!(
            "cannot remove {} and {}: {e} (the session lock is freed anyway)",
            path(home, session_id).display(),
            release_path(home, session_id).display()
        ));
    }
    release_lock();
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    fn env(vars: &[(&str, &str)]) -> Env {
        Env::for_test(Some(Path::new("/h")), Path::new("/w"), vars)
    }
    #[test]
    fn parse_warm_table() {
        for (raw, ms) in [
            ("0", 0),
            ("1500ms", 1500),
            ("2s", 2000),
            ("5", 5000),
            ("10m", 600_000),
        ] {
            assert_eq!(parse_warm(raw), Ok(Duration::from_millis(ms)), "{raw}");
        }
        for raw in ["11m", "600001ms", "1h", "abc", "", "-1", "2d"] {
            assert!(parse_warm(raw).is_err(), "{raw}");
        }
        assert!(parse_warm("11m").unwrap_err().contains("10m"));
    }
    #[test]
    fn resolve_warm_table() {
        let s = |n| Some(Duration::from_secs(n));
        for (flag, var, want) in [
            (None, None, None),
            (s(2), None, s(2)),
            (s(1), Some("5s"), s(1)),
            (Some(Duration::ZERO), Some("5s"), None),
            (None, Some("3s"), s(3)),
            (None, Some("0"), None),
            (None, Some(""), None),
            (None, Some("  "), None),
            (None, Some(" 4s "), s(4)),
            (s(600), None, s(600)),
        ] {
            let vars: Vec<_> = var.map(|v| ("GLEN_WARM", v)).into_iter().collect();
            assert_eq!(
                resolve_warm(flag, &env(&vars)).unwrap(),
                want,
                "{flag:?} {var:?}"
            );
        }
        for bad in ["11m", "abc", "1h"] {
            let e = resolve_warm(None, &env(&[("GLEN_WARM", bad)])).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidEnv, "{bad}");
            assert!(e.message.contains("GLEN_WARM"), "{}", e.message);
            // A valid flag wins over a bad variable.
            assert_eq!(
                resolve_warm(s(2), &env(&[("GLEN_WARM", bad)])).unwrap(),
                s(2)
            );
        }
    }
    #[test]
    fn env_digest_is_stable_and_skips_volatile_and_glen_variables() {
        let base = [
            ("HOME", "/h"),
            ("PATH", "/bin"),
            ("PI_CONFIG_FILES", "a"),
            ("OMP_PROFILE", "p"),
            ("PROVIDER_API_KEY", "k"),
        ];
        let digest = env_digest(&env(&base));
        assert_eq!(digest.len(), 16);
        assert!(
            digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_eq!(env_digest(&env(&base)), digest);
        for (k, v) in [
            ("GLEN_JOB_TIMEOUT", "30s"),
            ("GLEN_WARM", "5m"),
            ("GLEN_HOME", "/g"),
            ("PWD", "/x"),
            ("SHLVL", "3"),
            ("_", "/usr/bin/glen"),
            ("TERM_SESSION_ID", "w0t0p0"),
        ] {
            let mut vars = base.to_vec();
            vars.push((k, v));
            assert_eq!(env_digest(&env(&vars)), digest, "{k}");
        }
        for (i, changed) in [
            ("PI_CONFIG_FILES", "b"),
            ("OMP_PROFILE", "q"),
            ("HOME", "/other"),
            ("PATH", "/usr/bin"),
            ("PROVIDER_API_KEY", "other"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut vars = base.to_vec();
            vars[i] = changed;
            assert_ne!(env_digest(&env(&vars)), digest, "{changed:?}");
        }
        let mut vars = base.to_vec();
        vars.push(("OTHER_API_KEY", "new"));
        assert_ne!(env_digest(&env(&vars)), digest);
    }
    #[test]
    fn env_digest_encoding_is_unambiguous() {
        // A value holding the separators of a `KEY=VALUE\n` encoding is not a second variable.
        assert_ne!(
            env_digest(&env(&[("A", "x\nB=y")])),
            env_digest(&env(&[("A", "x"), ("B", "y")]))
        );
        assert_ne!(
            env_digest(&env(&[("A=B", "c")])),
            env_digest(&env(&[("A", "B=c")]))
        );
    }
    #[test]
    fn key_matches_field_by_field_and_cwd_canonically() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("project");
        let other = t.path().join("other");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut warm = crate::jobs::tests::job();
        warm.cwd = link.clone();
        warm.env_digest = Some("0123456789abcdef".into());
        let key = Key::of(&warm).unwrap();
        let mut job = warm.clone();
        job.cwd = real.clone();
        assert!(key.matches(&job), "a symlinked and a canonical path match");
        job.cwd = real.join(".");
        assert!(key.matches(&job));
        type Change = fn(&mut Job, &Path);
        let cases: [(&str, Change); 6] = [
            ("omp_bin", |j, _| j.omp_bin = Some("/bin/other_omp".into())),
            ("profile", |j, _| j.profile_flag = Some("other".into())),
            ("thinking", |j, _| j.thinking_flag = Some("high".into())),
            ("cwd", |j, other| j.cwd = other.to_path_buf()),
            ("digest", |j, _| {
                j.env_digest = Some("fedcba9876543210".into())
            }),
            ("no digest", |j, _| j.env_digest = None),
        ];
        for (name, change) in cases {
            let mut job = warm.clone();
            change(&mut job, &other);
            assert!(!key.matches(&job), "{name}");
        }
        // A path that cannot be canonicalised compares lexically.
        let mut missing = warm.clone();
        missing.cwd = "/nonexistent/glen-warm".into();
        let key = Key::of(&missing).unwrap();
        assert!(key.matches(&missing));
        let mut job = missing.clone();
        job.cwd = "/nonexistent/glen-warm-other".into();
        assert!(!key.matches(&job));
        missing.env_digest = None;
        assert!(Key::of(&missing).is_none());
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    const ID: &str = "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de";
    fn home(t: &tempfile::TempDir) -> GlenHome {
        GlenHome {
            root: t.path().join("glen"),
        }
    }
    fn entry(pid: u32, start: Option<String>, state: EntryState) -> Entry {
        Entry {
            v: 1,
            pid,
            start,
            session_id: ID.into(),
            session_handle: Some("e27c0de".into()),
            job_id: "01a18bcc-6970-7685-8f03-0ecc16977273".into(),
            omp_pid: None,
            omp_start: None,
            warm_ms: 2000,
            expires_at: "2026-11-02T00:00:10Z".into(),
            state,
            started_at: "2026-11-02T00:00:00Z".into(),
        }
    }
    fn me(state: EntryState) -> Entry {
        let i = liveness::current().unwrap();
        entry(i.pid, Some(i.start), state)
    }
    #[test]
    fn publish_read_index_round_trip_private_and_lazy() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        assert!(read(&h, ID).is_none());
        assert_eq!(index(&h).entries.len(), 0);
        assert!(!h.root.exists(), "readers create nothing");
        let e = me(EntryState::Idle);
        publish(&h, &e).unwrap();
        assert_eq!(read(&h, ID).unwrap().entry, e);
        let found = index(&h);
        assert_eq!((found.entries.len(), found.unreadable.len()), (1, 0));
        assert_eq!(found.entries[0].entry, e);
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path(&h, ID)), 0o600);
        assert_eq!(mode(&h.root.join("run/warm")), 0o700);
        // The metadata lock file the publish created is not a registry either.
        assert_eq!(mode(&meta_path(&h, ID)), 0o600);
        // A release request and a torn or foreign file are not registries.
        request_release(&h, ID, &e.ident()).unwrap();
        assert_eq!(mode(&release_path(&h, ID)), 0o600);
        fs::write(dir(&h).join("torn.json"), b"{\"v\":1,").unwrap();
        fs::write(dir(&h).join(".x.json.1.0.tmp"), b"{}").unwrap();
        let found = index(&h);
        assert_eq!((found.entries.len(), found.unreadable.len()), (1, 1));
        assert_eq!(found.unreadable, [dir(&h).join("torn.json")]);
        remove(&h, ID).unwrap();
        remove(&h, ID).unwrap();
        assert!(read(&h, ID).is_none());
    }
    /// S-09: only a registry at its session's own path is indexed; a parsable one under another
    /// name (another session's key, or any name) is reported as misnamed and left in place.
    #[test]
    fn s09_index_takes_only_registries_named_for_their_session() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let e = me(EntryState::Idle);
        publish(&h, &e).unwrap();
        let bytes = serde_json::to_vec(&e).unwrap();
        let other = path(&h, "01a0f6b1-2680-7a3c-8b21-4a1f9e00c0de");
        let forged = dir(&h).join("forged.json");
        for p in [&other, &forged] {
            fs::write(p, &bytes).unwrap();
        }
        let found = index(&h);
        assert_eq!(found.entries.len(), 1);
        assert_eq!(found.entries[0].entry, e);
        assert_eq!(found.misnamed, [other.clone(), forged.clone()]);
        assert!(found.unreadable.is_empty());
        assert!(other.exists() && forged.exists(), "never deleted");
    }
    #[test]
    fn symlinks_are_never_read_or_touched() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        h.ensure_dir("run/warm").unwrap();
        let victim = t.path().join("victim.json");
        fs::write(&victim, serde_json::to_vec(&me(EntryState::Idle)).unwrap()).unwrap();
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        fs::File::options()
            .write(true)
            .open(&victim)
            .unwrap()
            .set_modified(old)
            .unwrap();
        std::os::unix::fs::symlink(&victim, path(&h, ID)).unwrap();
        assert!(read(&h, ID).is_none());
        let found = index(&h);
        assert_eq!((found.entries.len(), found.unreadable.len()), (0, 1));
        touch(&h, ID);
        assert_eq!(fs::metadata(&victim).unwrap().modified().unwrap(), old);
        std::os::unix::fs::symlink(&victim, release_path(&h, ID)).unwrap();
        assert!(!release_requested(&h, ID, &me(EntryState::Idle).ident()));
    }
    #[test]
    fn touch_moves_the_mtime_without_rewriting() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        touch(&h, ID);
        assert!(!h.root.exists());
        publish(&h, &me(EntryState::Idle)).unwrap();
        let p = path(&h, ID);
        let old = SystemTime::now() - Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let body = fs::read(&p).unwrap();
        touch(&h, ID);
        let after = read(&h, ID).unwrap().mtime;
        assert!(after > old + Duration::from_secs(30), "{after:?}");
        assert_eq!(fs::read(&p).unwrap(), body);
    }
    #[test]
    fn class_table() {
        let now = SystemTime::now();
        let fresh = now - Duration::from_secs(1);
        let old = now - Duration::from_secs(6);
        let idle = me(EntryState::Idle);
        let busy = me(EntryState::Busy);
        assert_eq!(idle.class(fresh, now, STALE), Class::Fresh);
        assert_eq!(idle.class(old, now, STALE), Class::Stale);
        assert_eq!(
            busy.class(old, now, STALE),
            Class::Fresh,
            "busy is never stale"
        );
        assert_eq!(
            idle.class(fresh, now, Duration::from_millis(500)),
            Class::Stale
        );
        let mut reused = idle.clone();
        reused.start = Some("reused".into());
        assert_eq!(reused.class(fresh, now, STALE), Class::Dead);
        let dead = entry(
            crate::jobs::liveness::tests_dead_pid(),
            None,
            EntryState::Idle,
        );
        assert_eq!(dead.class(fresh, now, STALE), Class::Dead);
        assert_eq!(Class::Fresh.as_str(&idle), "idle");
        assert_eq!(Class::Fresh.as_str(&busy), "busy");
        assert_eq!(Class::Stale.as_str(&idle), "stale");
        assert_eq!(Class::Dead.as_str(&idle), "dead");
    }
    #[test]
    fn detail_strings() {
        let now = SystemTime::now();
        let mut idle = me(EntryState::Idle);
        idle.expires_at = (jiff::Timestamp::try_from(now).unwrap()
            + jiff::SignedDuration::from_millis(4200))
        .to_string();
        assert_eq!(
            idle.detail(Class::Fresh, now, now).as_deref(),
            Some("warm worker, idle, 5s left")
        );
        idle.expires_at = "2000-01-01T00:00:00Z".into();
        assert_eq!(
            idle.detail(Class::Fresh, now, now).as_deref(),
            Some("warm worker, idle, 0s left")
        );
        assert_eq!(
            me(EntryState::Busy)
                .detail(Class::Fresh, now, now)
                .as_deref(),
            Some("warm worker, busy")
        );
        assert_eq!(
            idle.detail(Class::Stale, now - Duration::from_secs(7), now)
                .as_deref(),
            Some("warm worker, no heartbeat for 7s")
        );
        assert_eq!(idle.detail(Class::Dead, now, now), None);
    }
    #[test]
    fn release_requests_reach_only_the_named_worker() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let mine = me(EntryState::Idle).ident();
        assert!(!release_requested(&h, ID, &mine));
        request_release(&h, ID, "1:other-start").unwrap();
        assert!(
            !release_requested(&h, ID, &mine),
            "an earlier worker's request"
        );
        fs::write(release_path(&h, ID), b"garbage").unwrap();
        assert!(!release_requested(&h, ID, &mine), "malformed");
        request_release(&h, ID, &mine).unwrap();
        assert!(release_requested(&h, ID, &mine));
        clear_release(&h, ID).unwrap();
        clear_release(&h, ID).unwrap();
        assert!(!release_requested(&h, ID, &mine));
        assert_eq!(ident(12, None), "12:");
        assert_eq!(ident(12, Some("t")), "12:t");
    }
    #[test]
    fn conditional_removal_spares_another_workers_files() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        assert!(!remove_if(&h, ID, "1:old").unwrap(), "nothing to remove");
        assert!(!clear_release_if(&h, ID, "1:old").unwrap());
        let successor = me(EntryState::Idle);
        publish(&h, &successor).unwrap();
        request_release(&h, ID, &successor.ident()).unwrap();
        // A snapshot of an earlier worker of the session removes nothing of its successor's.
        assert!(!remove_if(&h, ID, "1:old").unwrap());
        assert!(!clear_release_if(&h, ID, "1:old").unwrap());
        assert_eq!(read(&h, ID).unwrap().entry, successor);
        assert!(release_requested(&h, ID, &successor.ident()));
        assert!(remove_if(&h, ID, &successor.ident()).unwrap());
        assert!(clear_release_if(&h, ID, &successor.ident()).unwrap());
        assert!(read(&h, ID).is_none() && !release_path(&h, ID).exists());
    }
    /// The names in `run/warm`, sorted.
    fn files(h: &GlenHome) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir(h))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
    /// How long a test holds the metadata lock to prove that another party waits for it: well
    /// under [`META_BOUND`], so the waiter never gives up.
    const HELD: Duration = Duration::from_millis(100);
    #[test]
    fn overlapping_removers_wait_their_turn_and_the_newest_files_survive() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let key = lock_key(ID);
        // The old worker `old`'s files, which two overlapping `glen release` cleanups remove.
        let old = entry(1, Some("old".into()), EntryState::Idle);
        let newest = me(EntryState::Idle);
        publish(&h, &old).unwrap();
        request_release(&h, ID, &old.ident()).unwrap();
        // The test publishes both files under the metadata lock, as a successor does; both
        // cleanups start while it holds the lock.
        let meta = meta_lock(&h, ID).unwrap().unwrap();
        std::thread::scope(|s| {
            let removers: Vec<_> = (0..2)
                .map(|_| {
                    s.spawn(|| {
                        (
                            remove_if(&h, ID, &old.ident()).unwrap(),
                            clear_release_if(&h, ID, &old.ident()).unwrap(),
                        )
                    })
                })
                .collect();
            std::thread::sleep(HELD);
            assert_eq!(
                read(&h, ID).unwrap().entry,
                old,
                "no removal lands while another holder has the lock"
            );
            assert!(release_requested(&h, ID, &old.ident()));
            write_private_atomic(&path(&h, ID), &serde_json::to_vec(&newest).unwrap()).unwrap();
            write_private_atomic(&release_path(&h, ID), newest.ident().as_bytes()).unwrap();
            drop(meta);
            for r in removers {
                assert_eq!(r.join().unwrap(), (false, false), "only `old`'s files go");
            }
        });
        assert_eq!(read(&h, ID).unwrap().entry, newest);
        assert!(release_requested(&h, ID, &newest.ident()));
        assert_eq!(
            files(&h),
            [
                format!("{key}.json"),
                format!("{key}.lock"),
                format!("{key}.release")
            ],
            "no copy left aside"
        );
    }
    #[test]
    fn teardown_between_a_cleanups_compare_and_unlink_resurrects_nothing() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let worker = me(EntryState::Idle);
        let session = crate::jobs::lock::try_acquire(&h, ID).unwrap().unwrap();
        publish(&h, &worker).unwrap();
        request_release(&h, ID, &worker.ident()).unwrap();
        // A `glen release` cleanup for `worker` holds the metadata lock: it has read the registry
        // and compared its identity, and has not yet unlinked it. The worker tears down meanwhile.
        let cleanup = meta_lock(&h, ID).unwrap().unwrap();
        assert!(read(&h, ID).is_some_and(|i| i.entry.ident() == worker.ident()));
        std::thread::scope(|s| {
            let down = s.spawn(|| {
                teardown(
                    &h,
                    ID,
                    Reason::Release,
                    || None,
                    move || drop(session),
                    &|_: &str| (),
                )
            });
            std::thread::sleep(HELD);
            assert!(
                crate::jobs::lock::is_held(&h, ID),
                "teardown's removal waits for the cleanup, so the session lock stays held"
            );
            // The cleanup's unlink, then its unlock.
            remove_file(&path(&h, ID)).unwrap();
            drop(cleanup);
            down.join().unwrap();
        });
        assert!(!crate::jobs::lock::is_held(&h, ID));
        assert!(files(&h).is_empty(), "{:?}", files(&h));
        // A late cleanup for the same worker finds nothing and puts nothing back.
        assert!(!remove_if(&h, ID, &worker.ident()).unwrap());
        assert!(!clear_release_if(&h, ID, &worker.ident()).unwrap());
        assert!(files(&h).is_empty(), "{:?}", files(&h));
        // Nor does it touch the next worker's registry.
        let next = entry(1, Some("next".into()), EntryState::Idle);
        publish(&h, &next).unwrap();
        assert!(!remove_if(&h, ID, &worker.ident()).unwrap());
        assert_eq!(read(&h, ID).unwrap().entry, next);
    }
    #[test]
    fn a_mismatched_entry_is_removed_while_its_session_lock_is_held() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        // A live process holds the session lock, but the registry names a start token that is not
        // its own: cleanup removes it without the session lock, which stays with its holder.
        let _session = crate::jobs::lock::try_acquire(&h, ID).unwrap().unwrap();
        let mut reused = me(EntryState::Idle);
        reused.start = Some("reused".into());
        publish(&h, &reused).unwrap();
        request_release(&h, ID, &reused.ident()).unwrap();
        assert!(remove_if(&h, ID, &reused.ident()).unwrap());
        assert!(clear_release_if(&h, ID, &reused.ident()).unwrap());
        assert!(crate::jobs::lock::is_held(&h, ID));
        assert!(files(&h).is_empty(), "{:?}", files(&h));
    }
    #[test]
    fn the_lock_file_lives_only_while_a_file_it_guards_does() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let e = me(EntryState::Idle);
        publish(&h, &e).unwrap();
        request_release(&h, ID, &e.ident()).unwrap();
        remove(&h, ID).unwrap();
        assert!(
            meta_path(&h, ID).exists(),
            "the release request still needs it"
        );
        clear_release(&h, ID).unwrap();
        assert!(files(&h).is_empty(), "{:?}", files(&h));
        // A waiter that locks the file its holder unlinked holds nothing: the next acquire locks
        // the file now at the path, which is the one every later caller contends for.
        let held = meta_lock(&h, ID).unwrap().unwrap();
        let stale = fs::File::open(meta_path(&h, ID)).unwrap();
        drop(held);
        assert!(!meta_path(&h, ID).exists());
        stale.try_lock().unwrap();
        let now = meta_lock(&h, ID).unwrap().unwrap();
        assert!(meta_path(&h, ID).exists());
        assert!(
            fs::File::open(meta_path(&h, ID))
                .unwrap()
                .try_lock()
                .is_err(),
            "the linked file is the one held"
        );
        drop(now);
    }
    #[test]
    fn an_acquisition_whose_file_was_replaced_retries_on_the_file_at_the_path() {
        use std::{cell::Cell, rc::Rc};
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        publish(&h, &me(EntryState::Idle)).unwrap();
        let lock_file = meta_path(&h, ID);
        let replaced = Rc::new(Cell::new(false));
        // Between this acquisition's open and its flock, the previous holder unlinks the file it
        // opened and the next caller creates a new one at the path.
        BETWEEN_OPEN_AND_FLOCK.set(Some(Box::new({
            let (lock_file, replaced) = (lock_file.clone(), Rc::clone(&replaced));
            move || {
                fs::remove_file(&lock_file).unwrap();
                fs::File::create(&lock_file).unwrap();
                replaced.set(true);
            }
        })));
        let held = meta_lock(&h, ID).unwrap().unwrap();
        assert!(
            replaced.get(),
            "the seam ran between the open and the flock"
        );
        assert!(
            fs::File::open(&lock_file).unwrap().try_lock().is_err(),
            "the lock is held on the file now at the path, not on the unlinked one opened first"
        );
        drop(held);
        assert!(fs::File::open(&lock_file).unwrap().try_lock().is_ok());
    }
    #[test]
    fn a_stopped_holder_bounds_every_wait_and_a_release_request_is_not_written() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let e = me(EntryState::Idle);
        publish(&h, &e).unwrap();
        // A holder that never lets go, as a worker stopped inside its publish.
        let stopped = meta_lock(&h, ID).unwrap().unwrap();
        let started = Instant::now();
        assert!(
            !request_release(&h, ID, &e.ident()).unwrap(),
            "reported as not written"
        );
        assert!(started.elapsed() >= META_BOUND, "{:?}", started.elapsed());
        assert!(
            !release_path(&h, ID).exists(),
            "not written without the lock"
        );
        let err = remove_if(&h, ID, &e.ident()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert_eq!(
            read(&h, ID).unwrap().entry,
            e,
            "nothing removed without the lock"
        );
        drop(stopped);
        assert!(remove_if(&h, ID, &e.ident()).unwrap());
    }
    #[test]
    fn kill_identity_requires_proof() {
        let mut e = me(EntryState::Idle);
        assert_eq!(kill_identity(&e).worker, Liveness::Alive);
        assert_eq!(kill_identity(&e).omp, None);
        e.omp_pid = Some(std::process::id());
        assert_eq!(
            kill_identity(&e).omp,
            Some(Liveness::Unknown),
            "no omp start token proves nothing"
        );
        e.omp_pid = Some(crate::jobs::liveness::tests_dead_pid());
        assert_eq!(
            kill_identity(&e).omp,
            Some(Liveness::Dead),
            "a gone omp pid"
        );
        e.omp_pid = Some(std::process::id());
        e.omp_start = Some("reused".into());
        assert_eq!(kill_identity(&e).omp, Some(Liveness::Dead));
        e.omp_start = e.start.clone();
        assert_eq!(kill_identity(&e).omp, Some(Liveness::Alive));
        e.start = Some("reused".into());
        assert_eq!(kill_identity(&e).worker, Liveness::Dead);
        e.start = None;
        assert_eq!(kill_identity(&e).worker, Liveness::Unknown);
        e.pid = crate::jobs::liveness::tests_dead_pid();
        assert_eq!(kill_identity(&e).worker, Liveness::Dead);
    }
    #[cfg(debug_assertions)]
    #[test]
    fn stale_bound_reads_the_debug_knob_in_range() {
        let env = |v: &str| Env::for_test(None, Path::new("/"), &[("GLEN_WARM_STALE_MS", v)]);
        assert_eq!(stale_bound(&env("500")), Duration::from_millis(500));
        for bad in ["99", "30001", "x", ""] {
            assert_eq!(stale_bound(&env(bad)), STALE, "{bad}");
        }
        assert_eq!(
            stale_bound(&Env::for_test(None, Path::new("/"), &[])),
            STALE
        );
    }
}

#[cfg(test)]
mod claim_tests {
    use super::*;
    fn queued(store: &JobStore) -> JobId {
        let j = crate::jobs::tests::job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        id
    }
    #[test]
    fn claim_wins_once_and_only_on_a_queued_unreused_job() {
        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        let id = queued(&store);
        let mut s = store.read_status(&id).unwrap();
        s.worker_pid = Some(1);
        s.worker_start = Some("cold".into());
        s.waiting_for_lock = true;
        store.write_status(&id, &s).unwrap();
        let won = claim(&store, &id, 42, Some("warm")).unwrap().unwrap();
        assert_eq!(won.worker_pid, Some(42));
        assert_eq!(won.worker_start.as_deref(), Some("warm"));
        assert!(won.omp_reused && !won.waiting_for_lock);
        assert_eq!(won.status, State::Queued, "the first omp event accepts it");
        let disk = store.read_status(&id).unwrap();
        assert_eq!(disk.worker_pid, Some(42));
        assert!(
            claim(&store, &id, 43, None).unwrap().is_none(),
            "second claim"
        );
        assert_eq!(store.read_status(&id).unwrap().worker_pid, Some(42));
        for state in [State::Accepted, State::Completed, State::Lost] {
            let id = queued(&store);
            let mut s = store.read_status(&id).unwrap();
            s.status = state;
            store.write_status(&id, &s).unwrap();
            assert!(claim(&store, &id, 42, None).unwrap().is_none(), "{state:?}");
        }
        // Without a start token only the pid is written, as the cold worker does.
        let id = queued(&store);
        let s = claim(&store, &id, 42, None).unwrap().unwrap();
        assert_eq!((s.worker_pid, s.worker_start), (Some(42), None));
    }
    #[test]
    fn concurrent_claims_have_exactly_one_winner() {
        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        for _ in 0..10 {
            let id = queued(&store);
            let winners: usize = std::thread::scope(|scope| {
                let threads: Vec<_> = (0..4u32)
                    .map(|n| {
                        let (store, id) = (&store, &id);
                        scope.spawn(move || claim(store, id, 100 + n, None).unwrap().is_some())
                    })
                    .collect();
                threads
                    .into_iter()
                    .map(|t| usize::from(t.join().unwrap()))
                    .sum()
            });
            assert_eq!(winners, 1);
        }
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;
    use crate::mcp::test_support::scaled;
    const ID: &str = "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de";
    #[test]
    fn decide_checks_in_the_documented_order() {
        let t = |cancelled, omp_exited, release, endpoint, expired| Tick {
            cancelled,
            omp_exited,
            release,
            endpoint,
            expired,
        };
        for (tick, want) in [
            (t(false, false, false, false, false), None),
            (t(true, true, true, true, true), Some(Reason::Signal)),
            (t(true, false, true, false, true), Some(Reason::Signal)),
            (t(false, true, true, true, true), Some(Reason::OmpExited)),
            (t(false, false, true, true, true), Some(Reason::Release)),
            (t(false, false, true, false, true), Some(Reason::Release)),
            (t(false, false, false, true, true), Some(Reason::Endpoint)),
            (t(false, false, false, false, true), Some(Reason::Expired)),
        ] {
            assert_eq!(decide(&tick), want, "{tick:?}");
        }
        assert_eq!(Reason::OmpExited.as_str(), "omp_exited");
        assert_eq!(Reason::StateError.as_str(), "state_error");
    }
    #[test]
    fn omp_exit_is_a_closed_event_channel() {
        let (tx, rx) = mpsc::channel::<Event>();
        assert!(!drain_events(&rx));
        drop(tx);
        assert!(drain_events(&rx));
    }
    #[test]
    fn endpoint_check_is_a_stat_then_a_pid_check() {
        let t = tempfile::tempdir().unwrap();
        let home = GlenHome {
            root: t.path().join("glen"),
        };
        assert!(!endpoint_present(&home, ID));
        home.ensure_dir("run/live").unwrap();
        let write = |pid: u32| {
            let r =
                serde_json::json!({"v":1,"pid":pid,"session_id":ID,"socket":"/tmp/glen-x.sock"});
            fs::write(crate::live::registry::path(&home, ID), r.to_string()).unwrap();
        };
        write(std::process::id());
        assert!(endpoint_present(&home, ID));
        write(liveness::tests_dead_pid());
        assert!(
            !endpoint_present(&home, ID),
            "a dead TUI's registry is ignored"
        );
        fs::write(crate::live::registry::path(&home, ID), b"torn").unwrap();
        assert!(endpoint_present(&home, ID), "unreadable yields");
    }
    #[test]
    fn teardown_is_bounded_and_frees_the_lock_after_the_registry() {
        let t = tempfile::tempdir().unwrap();
        let home = GlenHome {
            root: t.path().join("glen"),
        };
        let lock = crate::jobs::lock::try_acquire(&home, ID).unwrap().unwrap();
        let me = liveness::current().unwrap();
        publish(
            &home,
            &Entry {
                v: 1,
                pid: me.pid,
                start: Some(me.start.clone()),
                session_id: ID.into(),
                session_handle: None,
                job_id: "j".into(),
                omp_pid: None,
                omp_start: None,
                warm_ms: 1000,
                expires_at: timestamp_after(Duration::from_secs(1)),
                state: EntryState::Idle,
                started_at: jiff::Timestamp::now().to_string(),
            },
        )
        .unwrap();
        request_release(&home, ID, &ident(me.pid, Some(&me.start))).unwrap();
        let lines = std::sync::Mutex::new(vec![]);
        let order = std::cell::Cell::new(None);
        let started = Instant::now();
        teardown(
            &home,
            ID,
            Reason::Release,
            || {
                // An omp that never dies: teardown must not wait for it.
                std::thread::sleep(Duration::from_secs(10));
                None
            },
            || {
                order.set(Some((
                    path(&home, ID).exists(),
                    release_path(&home, ID).exists(),
                )));
                drop(lock);
            },
            &|l: &str| lines.lock().unwrap().push(l.to_owned()),
        );
        assert!(
            started.elapsed() < scaled(Duration::from_secs(4)),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            order.get(),
            Some((false, false)),
            "registry gone before the lock"
        );
        assert!(!crate::jobs::lock::is_held(&home, ID));
        let lines = lines.into_inner().unwrap();
        assert_eq!(lines[0], "cooling (release)");
        assert!(
            lines.iter().any(|l| l.contains("exceeded 3.5s")),
            "{lines:?}"
        );
        // A prompt close logs its error only.
        let lines = std::sync::Mutex::new(vec![]);
        teardown(
            &home,
            ID,
            Reason::Expired,
            || Some("group survived".into()),
            || (),
            &|l: &str| lines.lock().unwrap().push(l.to_owned()),
        );
        assert_eq!(
            lines.into_inner().unwrap(),
            ["cooling (expired)", "close: group survived"]
        );
    }
    #[test]
    fn a_stopped_metadata_holder_costs_teardown_one_wait_within_its_deadline() {
        let t = tempfile::tempdir().unwrap();
        let home = GlenHome {
            root: t.path().join("glen"),
        };
        let me = liveness::current().unwrap();
        let mine = ident(me.pid, Some(&me.start));
        publish(
            &home,
            &Entry {
                v: 1,
                pid: me.pid,
                start: Some(me.start.clone()),
                session_id: ID.into(),
                session_handle: None,
                job_id: "j".into(),
                omp_pid: None,
                omp_start: None,
                warm_ms: 1000,
                expires_at: timestamp_after(Duration::from_secs(1)),
                state: EntryState::Idle,
                started_at: jiff::Timestamp::now().to_string(),
            },
        )
        .unwrap();
        assert!(request_release(&home, ID, &mine).unwrap());
        // Another process keeps the metadata lock, as one stopped inside its step would.
        let stopped = hold_meta_lock(&home, ID);
        let files = || (path(&home, ID).exists(), release_path(&home, ID).exists());
        // A prompt close: one bounded acquisition for both files, never one per file.
        let lock = crate::jobs::lock::try_acquire(&home, ID).unwrap().unwrap();
        let lines = std::sync::Mutex::new(vec![]);
        let seen = std::cell::Cell::new(None);
        let started = Instant::now();
        teardown(
            &home,
            ID,
            Reason::Release,
            || None,
            || {
                seen.set(Some(files()));
                drop(lock);
            },
            &|l: &str| lines.lock().unwrap().push(l.to_owned()),
        );
        let took = started.elapsed();
        assert!(
            took >= META_BOUND && took < scaled(META_BOUND + META_BOUND / 2),
            "one wait of the metadata bound: {took:?}"
        );
        assert!(!crate::jobs::lock::is_held(&home, ID));
        assert_eq!(
            seen.get(),
            Some((true, true)),
            "the documented failure path: both files outlive the session lock"
        );
        let lines = lines.into_inner().unwrap();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[1].starts_with("cannot remove") && lines[1].contains("freed anyway"),
            "{lines:?}"
        );
        // A close that uses its whole bound leaves the cleanup only what remains of the deadline.
        let lock = crate::jobs::lock::try_acquire(&home, ID).unwrap().unwrap();
        let started = Instant::now();
        teardown(
            &home,
            ID,
            Reason::Expired,
            || {
                std::thread::sleep(Duration::from_secs(10));
                None
            },
            move || drop(lock),
            &|_: &str| (),
        );
        let took = started.elapsed();
        assert!(
            took >= TEARDOWN_BOUND && took < scaled(TEARDOWN_DEADLINE + Duration::from_millis(250)),
            "teardown keeps its deadline: {took:?}"
        );
        assert!(!crate::jobs::lock::is_held(&home, ID));
        // Once the holder lets go, the next teardown removes both before the lock.
        drop(stopped);
        let lock = crate::jobs::lock::try_acquire(&home, ID).unwrap().unwrap();
        let seen = std::cell::Cell::new(None);
        teardown(
            &home,
            ID,
            Reason::Release,
            || None,
            || {
                seen.set(Some(files()));
                drop(lock);
            },
            &|_: &str| (),
        );
        assert_eq!(seen.get(), Some((false, false)));
    }
}

#[cfg(test)]
mod adopt_tests {
    use super::*;
    const SESSION: &str = "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de";
    fn key() -> Key {
        let mut j = crate::jobs::tests::job();
        j.env_digest = Some("0123456789abcdef".into());
        Key::of(&j).unwrap()
    }
    fn queued(store: &JobStore, edit: impl FnOnce(&mut Job)) -> JobId {
        let mut j = crate::jobs::tests::job();
        j.env_digest = Some("0123456789abcdef".into());
        edit(&mut j);
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        id
    }
    #[test]
    fn scan_takes_the_oldest_matching_queued_job_of_the_session() {
        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        let mut scan = Scan::default();
        assert!(matches!(scan.next(&store, SESSION, &key()), Found::Nothing));
        // Not candidates, cached for good: another session, the live route, a reused job.
        queued(&store, |j| j.session_id = Some("other".into()));
        queued(&store, |j| j.route = crate::jobs::Route::Live);
        let reused = queued(&store, |_| ());
        store
            .update_status_if(&reused, |s| {
                s.omp_reused = true;
                true
            })
            .unwrap();
        assert!(matches!(scan.next(&store, SESSION, &key()), Found::Nothing));
        assert_eq!(scan.settled.len(), 3);
        let first = queued(&store, |_| ());
        std::thread::sleep(Duration::from_millis(2));
        queued(&store, |_| ());
        match scan.next(&store, SESSION, &key()) {
            Found::Adopt(id, _) => assert_eq!(id, first, "the oldest first"),
            other => panic!("{other:?}"),
        }
        let mut s = store.read_status(&first).unwrap();
        s.status = State::Completed;
        store.write_status(&first, &s).unwrap();
        assert!(matches!(
            scan.next(&store, SESSION, &key()),
            Found::Adopt(..)
        ));
        assert!(scan.settled.contains(&first));
    }
    #[test]
    fn scan_cools_on_a_mismatching_oldest_job() {
        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        let other = queued(&store, |j| j.thinking_flag = Some("high".into()));
        std::thread::sleep(Duration::from_millis(2));
        queued(&store, |_| ());
        match Scan::default().next(&store, SESSION, &key()) {
            Found::Mismatch(id) => assert_eq!(id, other),
            found => panic!("{found:?}"),
        }
        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        queued(&store, |j| j.env_digest = None);
        assert!(matches!(
            Scan::default().next(&store, SESSION, &key()),
            Found::Mismatch(_)
        ));
    }
    #[test]
    fn scan_retries_a_job_seen_before_its_files_exist() {
        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        let j = crate::jobs::tests::job();
        let id = JobId::parse(&j.id).unwrap();
        // Tick 1: the directory exists, neither file yet (`create` is mid-way).
        store
            .home
            .ensure_dir(&format!("jobs/{}", id.as_str()))
            .unwrap();
        let mut scan = Scan::default();
        assert!(matches!(scan.next(&store, SESSION, &key()), Found::Nothing));
        assert!(!scan.settled.contains(&id), "a read error is never cached");
        // Tick 2: both files.
        std::fs::remove_dir(store.dir(&id)).unwrap();
        queued(&store, |job| {
            *job = Job {
                env_digest: Some("0123456789abcdef".into()),
                ..j.clone()
            }
        });
        match scan.next(&store, SESSION, &key()) {
            Found::Adopt(found, _) => assert_eq!(found, id),
            other => panic!("{other:?}"),
        }
        // A directory that never becomes a job is given up after the retry bound.
        let broken = JobId::new();
        store
            .home
            .ensure_dir(&format!("jobs/{}", broken.as_str()))
            .unwrap();
        let mut scan = Scan::default();
        for _ in 0..Scan::RETRIES {
            scan.next(&store, "none", &key());
        }
        assert!(scan.settled.contains(&broken));
    }
    #[test]
    fn a_panic_in_an_adopted_job_fails_that_job_only() {
        let t = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(t.path());
        let one = queued(&store, |_| ());
        let mut s = store.read_status(&one).unwrap();
        s.status = State::Completed;
        s.worker_pid = Some(std::process::id());
        store.write_status(&one, &s).unwrap();
        let before = std::fs::read(store.dir(&one).join("status.json")).unwrap();
        let two = queued(&store, |_| ());
        let me = liveness::current().unwrap();
        let claimed = claim(&store, &two, me.pid, Some(&me.start))
            .unwrap()
            .unwrap();
        let w = Worker {
            store: store.clone(),
            id: two.clone(),
            job: store.read_job(&two).unwrap(),
            status: claimed,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            deadline: Instant::now() + Duration::from_secs(10),
            control_in_flight: None,
            omp_queued: false,
            settled: false,
        };
        let (w, panicked) = guarded(w, |_| panic!("boom"));
        assert!(panicked);
        assert!(!w.keep_warm());
        let failed = store.read_status(&two).unwrap();
        assert_eq!(failed.status, State::Failed);
        assert_eq!(failed.error.unwrap().message, "worker panicked: boom");
        assert_eq!(
            std::fs::read(store.dir(&one).join("status.json")).unwrap(),
            before
        );
        // An ordinary failure is the adopted job's own too, without the panic flag.
        let three = queued(&store, |_| ());
        let claimed = claim(&store, &three, me.pid, Some(&me.start))
            .unwrap()
            .unwrap();
        let w = Worker {
            id: three.clone(),
            job: store.read_job(&three).unwrap(),
            status: claimed,
            ..w
        };
        let (_, panicked) = guarded(w, |_| Err(Failure::new("omp_exited", "gone")));
        assert!(!panicked);
        assert_eq!(store.read_status(&three).unwrap().status, State::Failed);
    }
}
