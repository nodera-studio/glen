//! The opt-in warm worker (v0.2 W4, design §A21): settings, the match key, the on-disk
//! registry and release request, the claim, the idle loop and its one teardown.
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
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
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

/// xxh64 (seed 0) of the sorted `KEY=VALUE\n` lines of `env`, without [`VOLATILE_ENV`] and
/// `GLEN_` variables, as 16 lowercase hex digits. No value is ever recorded, only this digest.
pub fn env_digest(env: &Env) -> String {
    let mut bytes = Vec::new();
    for (k, v) in env.iter() {
        if VOLATILE_ENV.contains(&k) || k.starts_with("GLEN_") {
            continue;
        }
        bytes.extend_from_slice(k.as_bytes());
        bytes.push(b'=');
        bytes.extend_from_slice(v.as_bytes());
        bytes.push(b'\n');
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
            None => matches!(liveness::start_token(self.pid), liveness::Probe::Gone),
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
/// Atomic 0600 write in the 0700 `run/warm` directory.
pub fn publish(home: &GlenHome, entry: &Entry) -> io::Result<()> {
    home.ensure_dir("run/warm")?;
    write_private_atomic(&path(home, &entry.session_id), &serde_json::to_vec(entry)?)
}
/// The heartbeat: moves the registry's mtime without rewriting it. A missing file or a
/// symlink (never followed) is ignored.
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
/// Removes the registry; a missing one is fine.
pub fn remove(home: &GlenHome, session_id: &str) -> io::Result<()> {
    remove_file(&path(home, session_id))
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
    let entry: Entry = serde_json::from_slice(&bytes).ok()?;
    (entry.v == 1 && entry.pid > 0 && entry.session_id == session_id)
        .then_some(Indexed { entry, mtime })
}
/// Every readable registry, plus the count of files that are not (torn, foreign, a symlink).
/// Release requests and atomic-write temporaries are not registries.
pub fn index(home: &GlenHome) -> (Vec<Indexed>, usize) {
    let mut entries = vec![];
    let mut unreadable = 0;
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
            Some(i) => entries.push(i),
            None => unreadable += 1,
        }
    }
    entries.sort_by(|a, b| a.entry.session_id.cmp(&b.entry.session_id));
    (entries, unreadable)
}
/// Asks the worker named by `ident` (see [`ident`]) to cool. Rewriting refreshes the request.
pub fn request_release(home: &GlenHome, session_id: &str, ident: &str) -> io::Result<()> {
    home.ensure_dir("run/warm")?;
    write_private_atomic(&release_path(home, session_id), ident.as_bytes())
}
/// True only for a request addressed to this worker's own identity: a request meant for an
/// earlier worker of the session, or a malformed one, is ignored.
pub fn release_requested(home: &GlenHome, session_id: &str, me: &str) -> bool {
    read_regular(&release_path(home, session_id)).is_some_and(|(bytes, _)| bytes == me.as_bytes())
}
pub fn clear_release(home: &GlenHome, session_id: &str) -> io::Result<()> {
    remove_file(&release_path(home, session_id))
}
/// What `glen release --force` may signal: only a process whose pid and start token are proven
/// unchanged (`Alive`). `Unknown` never signals.
#[derive(Debug, PartialEq, Eq)]
pub struct KillIdentity {
    pub worker: Liveness,
    /// `None` when the entry records no omp, or no omp start token to prove it by.
    pub omp: Option<Liveness>,
}
pub fn kill_identity(entry: &Entry) -> KillIdentity {
    KillIdentity {
        worker: match &entry.start {
            Some(start) => liveness::check(entry.pid, start),
            None if matches!(liveness::start_token(entry.pid), liveness::Probe::Gone) => {
                Liveness::Dead
            }
            None => Liveness::Unknown,
        },
        omp: entry
            .omp_pid
            .zip(entry.omp_start.as_deref())
            .map(|(pid, start)| liveness::check(pid, start)),
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
        assert_eq!(index(&h).0.len(), 0);
        assert!(!h.root.exists(), "readers create nothing");
        let e = me(EntryState::Idle);
        publish(&h, &e).unwrap();
        assert_eq!(read(&h, ID).unwrap().entry, e);
        let (all, unreadable) = index(&h);
        assert_eq!((all.len(), unreadable), (1, 0));
        assert_eq!(all[0].entry, e);
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path(&h, ID)), 0o600);
        assert_eq!(mode(&h.root.join("run/warm")), 0o700);
        // A release request and a torn or foreign file are not registries.
        request_release(&h, ID, &e.ident()).unwrap();
        assert_eq!(mode(&release_path(&h, ID)), 0o600);
        fs::write(dir(&h).join("torn.json"), b"{\"v\":1,").unwrap();
        fs::write(dir(&h).join(".x.json.1.0.tmp"), b"{}").unwrap();
        let (all, unreadable) = index(&h);
        assert_eq!((all.len(), unreadable), (1, 1));
        remove(&h, ID).unwrap();
        remove(&h, ID).unwrap();
        assert!(read(&h, ID).is_none());
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
        let (all, unreadable) = index(&h);
        assert_eq!((all.len(), unreadable), (0, 1));
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
    fn kill_identity_requires_proof() {
        let mut e = me(EntryState::Idle);
        assert_eq!(kill_identity(&e).worker, Liveness::Alive);
        assert_eq!(kill_identity(&e).omp, None);
        e.omp_pid = Some(std::process::id());
        assert_eq!(
            kill_identity(&e).omp,
            None,
            "no omp start token proves nothing"
        );
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
