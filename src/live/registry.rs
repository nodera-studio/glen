use crate::{
    glen_home::GlenHome,
    jobs::{
        liveness::{self, Probe},
        lock::lock_key,
    },
};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Registry {
    pub v: u32,
    pub pid: u32,
    pub session_id: String,
    #[serde(default)]
    pub session_file: Option<PathBuf>,
    pub socket: PathBuf,
    #[serde(default)]
    pub ext_version: Option<String>,
    #[serde(default)]
    pub omp_version: Option<String>,
    #[serde(default)]
    pub protocol: Option<u32>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// The endpoint's pid namespace inode (Linux, S-04/S-10): `pid` is numbered there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pidns: Option<u64>,
    /// The kernel boot id when the endpoint registered (Linux).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
}
pub fn path(home: &GlenHome, id: &str) -> PathBuf {
    super::run_dir(home).join(format!("{}.json", lock_key(id)))
}
pub enum Read {
    Missing,
    Unreadable(String),
    /// A well-formed registry whose socket glen will not connect to (S-18, [`check_socket`]).
    Untrusted(Box<Registry>, String),
    Ok(Box<Registry>),
}
pub fn read(home: &GlenHome, id: &str) -> Read {
    let p = path(home, id);
    match fs::read(&p) {
        Ok(b) => match serde_json::from_slice::<Registry>(&b) {
            Ok(r) if r.v == 1 && r.pid > 0 && r.session_id == id && r.socket.is_absolute() => {
                match check_socket(home, &r) {
                    Ok(()) => Read::Ok(Box::new(r)),
                    Err(reason) => Read::Untrusted(Box::new(r), reason),
                }
            }
            Ok(_) => Read::Unreadable(format!("invalid registry {}", p.display())),
            Err(e) => Read::Unreadable(format!("{}: {e}", p.display())),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Missing,
        Err(e) => Read::Unreadable(format!("{}: {e}", p.display())),
    }
}
/// S-18 hygiene, before any connection: the registry's socket is exactly
/// `run/live/<pid>.sock` (the directory compared by its real path, as the extension may spell
/// GLEN_HOME differently), and once it exists it is a socket owned by this user with no group or
/// other permission bits. A socket not bound yet, or that glen cannot stat, passes: connecting
/// reports it. The
/// kernel peer check on every connection (`client`) is what binds the socket to `pid`.
pub fn check_socket(home: &GlenHome, r: &Registry) -> Result<(), String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let named = r
        .socket
        .file_name()
        .is_some_and(|n| *n == *format!("{}.sock", r.pid))
        && r.socket.parent().is_some_and(|dir| {
            dir == super::run_dir(home)
                || matches!(
                    (fs::canonicalize(dir), fs::canonicalize(super::run_dir(home))),
                    (Ok(a), Ok(b)) if a == b
                )
        });
    if !named {
        return Err(format!(
            "registry socket {} is not {}/{}.sock",
            r.socket.display(),
            super::run_dir(home).display(),
            r.pid
        ));
    }
    // A socket glen cannot inspect is left to connect(2), which fails the same way or reaches
    // the kernel peer check.
    let Ok(meta) = fs::symlink_metadata(&r.socket) else {
        return Ok(());
    };
    let euid = rustix::process::geteuid().as_raw();
    if !meta.file_type().is_socket() || meta.uid() != euid || meta.mode() & 0o077 != 0 {
        return Err(format!(
            "{} is not a private socket owned by this user",
            r.socket.display()
        ));
    }
    Ok(())
}
pub fn files(home: &GlenHome) -> Vec<PathBuf> {
    fs::read_dir(super::run_dir(home))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| !s.starts_with('.') && s.ends_with(".json"))
        })
        .collect()
}
pub fn index(home: &GlenHome) -> Vec<Registry> {
    files(home)
        .into_iter()
        .filter_map(|p| {
            fs::read(p)
                .ok()
                .and_then(|b| serde_json::from_slice::<Registry>(&b).ok())
        })
        .filter(|r| r.v == 1 && r.pid > 0 && check_socket(home, r).is_ok())
        .collect()
}
/// Only a pid proven gone (`liveness::observe`: never under S-04's hidden pids) is dead, or one
/// recorded at another boot. A registry recording another pid namespace (S-04/S-10: an
/// endpoint glen's own pid namespace cannot see) is never dead; one recording neither is
/// judged on its pid, as before.
pub fn pid_dead(r: &Registry) -> bool {
    match liveness::scope(r.boot_id.as_deref(), r.pidns) {
        liveness::Scope::OtherBoot => true,
        liveness::Scope::Elsewhere => false,
        liveness::Scope::Here => matches!(liveness::observe(r.pid), Probe::Gone),
    }
}
/// Whether `run/live/<pid>.sock` of a dead registry may go. The pid is probed in this
/// process's pid namespace, so it must be gone here and numbered here: a registry from another
/// boot is stale, but `<pid>.sock` is a current endpoint's unless the record names this pid
/// namespace (the initial one keeps its inode across boots). And no current registry of another
/// pid namespace may name the same pid, as in the extension's sweep (S-04/S-10).
fn socket_dead(home: &GlenHome, reg: &Registry) -> bool {
    !numbered_elsewhere(reg)
        && matches!(liveness::observe(reg.pid), Probe::Gone)
        && !files(home).into_iter().any(|p| {
            fs::read(p)
                .ok()
                .and_then(|b| serde_json::from_slice::<Registry>(&b).ok())
                .is_some_and(|r| r.pid == reg.pid && numbered_elsewhere(&r))
        })
}
/// Whether a registry's pid is numbered in a pid namespace other than this process's: another
/// namespace, a place this process cannot compare, or another boot whose record does not name
/// this namespace.
fn numbered_elsewhere(r: &Registry) -> bool {
    match liveness::scope(r.boot_id.as_deref(), r.pidns) {
        liveness::Scope::Here => false,
        liveness::Scope::OtherBoot => {
            r.pidns.is_none() || r.pidns != liveness::here().map(|h| h.pidns)
        }
        liveness::Scope::Elsewhere => true,
    }
}
/// A failed stale-registry cleanup, carrying the raw I/O error alongside the
/// formatted message so callers can tell a sandbox permission denial from any
/// other cleanup failure.
#[derive(Debug)]
pub struct CleanupError {
    pub message: String,
    pub io: std::io::Error,
}
impl std::fmt::Display for CleanupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CleanupError {}
pub fn remove_if_unchanged(
    home: &GlenHome,
    reg: &Registry,
    socket_too: bool,
) -> Result<bool, CleanupError> {
    let target = path(home, &reg.session_id);
    let tombstone = super::run_dir(home).join(format!(
        ".{}.{}.stale",
        lock_key(&reg.session_id),
        uuid::Uuid::now_v7()
    ));
    let io = |e: std::io::Error| CleanupError {
        message: format!(
            "cannot clean live registry {} for session {}: {e}",
            target.display(),
            reg.session_id
        ),
        io: e,
    };
    match fs::rename(&target, &tombstone) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(io(e)),
    }
    let bytes = fs::read(&tombstone).map_err(io)?;
    let unchanged = serde_json::from_slice::<Registry>(&bytes).is_ok_and(|now| now == *reg);
    if !unchanged {
        // Restore without clobbering a concurrent atomic publisher. Keep an
        // unexpected tombstone when the target is occupied by an even newer one.
        match fs::hard_link(&tombstone, &target) {
            Ok(()) => fs::remove_file(&tombstone).map_err(io)?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(io(e)),
        }
        return Ok(false);
    }
    fs::remove_file(&tombstone).map_err(io)?;
    let owned_socket = super::run_dir(home).join(format!("{}.sock", reg.pid));
    if socket_too && reg.socket == owned_socket && socket_dead(home, reg) {
        match fs::remove_file(&owned_socket) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(io(e)),
        }
    }
    Ok(true)
}
/// Add unpublished TUI sessions to the same handle namespace as disk sessions.
/// No session bytes are invented on disk; the synthetic header is read-only.
pub fn union(
    catalog: &mut crate::omp::listing::Catalog,
    home: &GlenHome,
    env: &crate::env::Env,
    registries: &[Registry],
) {
    use crate::omp::{
        bucket::bucket_for,
        listing::{SessionFile, sort_newest_first, summarize},
        session::Session,
    };
    for r in registries {
        // AC-H109: a registry naming a process more confined than glen never lends a session
        // its cwd or title (the confinement-ordering rule, without a connection).
        if pid_dead(r)
            || catalog.sessions.iter().any(|s| s.id == r.session_id)
            || crate::proc_ident::may_trust_pid(r.pid).is_err()
        {
            continue;
        }
        let file = r.session_file.clone().unwrap_or_else(|| {
            super::run_dir(home).join(format!("{}.jsonl", lock_key(&r.session_id)))
        });
        if file.exists() {
            continue;
        }
        let header = serde_json::json!({"type":"session","version":3,"id":r.session_id,"cwd":r.cwd,"timestamp":r.started_at,"title":r.title});
        if let Ok(session) = Session::parse(&file, format!("{header}\n").as_bytes()) {
            let mtime = r
                .started_at
                .as_deref()
                .and_then(|s| s.parse::<jiff::Timestamp>().ok())
                .map(std::time::SystemTime::from)
                .unwrap_or(std::time::UNIX_EPOCH);
            let mut s = summarize(
                SessionFile {
                    path: file,
                    bucket: bucket_for(
                        r.cwd
                            .as_deref()
                            .map(std::path::Path::new)
                            .unwrap_or(env.cwd()),
                        env,
                    ),
                    mtime,
                    size: 0,
                },
                &session,
            );
            s.title = r.title.clone();
            catalog.sessions.push(s);
        }
    }
    sort_newest_first(&mut catalog.sessions);
}
pub fn states(home: &GlenHome) -> std::collections::HashMap<String, super::client::Hello> {
    probe(home).0
}
/// Every registered endpoint's `state`, within one second: the answers of trusted endpoints by
/// session id, and the session ids whose endpoint glen does not trust (S-18: its socket, kernel
/// peer or reported pid is not the registered process, or it runs more confined than glen).
pub fn probe(
    home: &GlenHome,
) -> (
    std::collections::HashMap<String, super::client::Hello>,
    std::collections::HashSet<String>,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    let all = files(home)
        .into_iter()
        .filter_map(|p| {
            fs::read(p)
                .ok()
                .and_then(|b| serde_json::from_slice::<Registry>(&b).ok())
        })
        .filter(|r| r.v == 1 && r.pid > 0)
        .filter_map(|r| match check_socket(home, &r) {
            Ok(()) => Some(Ok(r)),
            // A dead pid's registry is stale, not untrusted.
            Err(_) if pid_dead(&r) => None,
            Err(_) => Some(Err(r.session_id)),
        })
        .collect::<Vec<_>>();
    let mut untrusted = std::collections::HashSet::new();
    let mut count = 0;
    for r in all {
        let r = match r {
            Ok(r) => r,
            Err(id) => {
                untrusted.insert(id);
                continue;
            }
        };
        count += 1;
        let tx = tx.clone();
        std::thread::spawn(move || {
            let result = match super::client::state(&r.socket, r.pid, super::STATE_LS) {
                Err(super::client::ClientError::Untrusted(_)) => Err(r.session_id),
                Ok(s) if s.pid != r.pid => Err(r.session_id),
                Ok(s) if s.session_id == r.session_id => Ok(Some(s)),
                _ => Ok(None),
            };
            let _ = tx.send(result);
        });
    }
    drop(tx);
    let end = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let mut states = std::collections::HashMap::new();
    for _ in 0..count {
        match rx.recv_timeout(end.saturating_duration_since(std::time::Instant::now())) {
            Ok(Ok(Some(s))) => {
                states.insert(s.session_id.clone(), s);
            }
            Ok(Ok(None)) => (),
            Ok(Err(id)) => {
                untrusted.insert(id);
            }
            Err(_) => break,
        }
    }
    (states, untrusted)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_lenient_compare_delete_and_endpoint_only_union() {
        let t = tempfile::tempdir().unwrap();
        let home = GlenHome {
            root: t.path().join("g"),
        };
        let id = "00000000-0000-7000-8000-000001234567";
        assert!(matches!(read(&home, id), Read::Missing));
        assert!(index(&home).is_empty());
        assert!(!home.root.exists());
        let dir = home.ensure_dir("run/live").unwrap();
        let value = serde_json::json!({"v":1,"pid":std::process::id(),"session_id":id,"socket":super::super::run_dir(&home).join(format!("{}.sock", std::process::id())),"started_at":"2026-10-30T00:00:00Z","future":true});
        std::fs::write(path(&home, id), value.to_string()).unwrap();
        let Read::Ok(reg) = read(&home, id) else {
            panic!("registry")
        };
        let mut changed = (*reg).clone();
        changed.started_at = Some("new".into());
        std::fs::write(path(&home, id), serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(!remove_if_unchanged(&home, &reg, false).unwrap());
        let Read::Ok(restored) = read(&home, id) else {
            panic!("fresh registry must be restored")
        };
        assert_eq!(*restored, changed);
        assert_eq!(files(&home).len(), 1);
        let env = crate::env::Env::for_test(Some(t.path()), t.path(), &[]);
        let mut catalog = crate::omp::listing::Catalog::scan(&t.path().join("sessions")).unwrap();
        union(&mut catalog, &home, &env, &[(*reg).clone()]);
        catalog.assign_handles(None);
        assert_eq!(catalog.sessions.len(), 1);
        assert_eq!(
            crate::omp::handle::resolve("1234567", &catalog, &env).unwrap(),
            catalog.sessions[0].file.path
        );
        assert!(!catalog.sessions[0].file.path.exists());
        remove_if_unchanged(&home, &changed, false).unwrap();
        assert!(!path(&home, id).exists());
        std::fs::write(dir.join(format!("{id}.json")), "bad").unwrap();
        assert!(matches!(read(&home, id), Read::Unreadable(_)));
        assert!(index(&home).is_empty());
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;
    #[test]
    fn stale_cleanup_only_unlinks_the_owned_pid_socket() {
        let t = tempfile::tempdir().unwrap();
        let home = GlenHome {
            root: t.path().join("g"),
        };
        home.ensure_dir("run/live").unwrap();
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        let protected = t.path().join("protected.jsonl");
        std::fs::write(&protected, "protected").unwrap();
        let mut registry = Registry {
            v: 1,
            pid,
            session_id: "stale".into(),
            session_file: None,
            socket: protected.clone(),
            ext_version: None,
            omp_version: None,
            protocol: Some(1),
            mode: Some("tui".into()),
            started_at: Some("2026-10-30T00:00:00Z".into()),
            cwd: None,
            title: None,
            pidns: None,
            boot_id: None,
        };
        std::fs::write(path(&home, "stale"), serde_json::to_vec(&registry).unwrap()).unwrap();
        remove_if_unchanged(&home, &registry, true).unwrap();
        assert_eq!(std::fs::read_to_string(&protected).unwrap(), "protected");
        assert!(!path(&home, "stale").exists());
        registry.socket = super::super::run_dir(&home).join(format!("{pid}.sock"));
        std::fs::write(&registry.socket, "leftover").unwrap();
        std::fs::write(path(&home, "stale"), serde_json::to_vec(&registry).unwrap()).unwrap();
        remove_if_unchanged(&home, &registry, true).unwrap();
        assert!(!registry.socket.exists());
    }
}

#[cfg(test)]
mod h3_tests {
    use super::*;
    /// S-04/S-10 (AC-H302): a registry recording another pid namespace is never dead, so it and
    /// its socket survive cleanup; another boot is dead; a legacy registry is judged on its pid.
    #[test]
    fn pid_dead_consults_the_recorded_namespace() {
        let t = tempfile::tempdir().unwrap();
        let home = GlenHome {
            root: t.path().join("g"),
        };
        home.ensure_dir("run/live").unwrap();
        let pid = crate::jobs::liveness::tests_dead_pid();
        let here = liveness::here();
        let mut r = Registry {
            v: 1,
            pid,
            session_id: "s".into(),
            session_file: None,
            socket: super::super::run_dir(&home).join(format!("{pid}.sock")),
            ext_version: None,
            omp_version: None,
            protocol: Some(1),
            mode: Some("tui".into()),
            started_at: None,
            cwd: None,
            title: None,
            pidns: None,
            boot_id: None,
        };
        assert!(pid_dead(&r), "legacy: a gone pid is dead");
        r.pidns = Some(here.map_or(7, |h| h.pidns + 1));
        r.boot_id = here.map(|h| h.boot_id.clone());
        assert!(!pid_dead(&r), "another pid namespace is never dead");
        std::fs::write(path(&home, "s"), serde_json::to_vec(&r).unwrap()).unwrap();
        std::fs::write(&r.socket, "endpoint").unwrap();
        assert!(remove_if_unchanged(&home, &r, true).unwrap());
        assert!(r.socket.exists(), "the socket of a foreign namespace stays");
        if let Some(h) = here {
            r.pidns = Some(h.pidns);
            assert!(pid_dead(&r), "this namespace: a gone pid is dead");
            r.pidns = Some(h.pidns + 1);
            r.boot_id = Some("another-boot".into());
            assert!(pid_dead(&r), "another boot is dead");
            r.pidns = Some(h.pidns);
            r.boot_id = Some(h.boot_id.clone());
            r.pid = std::process::id();
            assert!(!pid_dead(&r), "a live pid here");
        }
        // The fields are omitted when unknown and read back when present.
        let legacy = serde_json::to_value(Registry {
            pidns: None,
            boot_id: None,
            ..r.clone()
        })
        .unwrap();
        assert!(legacy.get("pidns").is_none() && legacy.get("boot_id").is_none());
        let back: Registry =
            serde_json::from_value(serde_json::json!({"v":1,"pid":5,"session_id":"s","socket":"/x/5.sock","pidns":42,"boot_id":"b"})).unwrap();
        assert_eq!((back.pidns, back.boot_id.as_deref()), (Some(42), Some("b")));
    }
}

#[cfg(test)]
mod h3_fix1_tests {
    use super::*;
    fn reg(home: &GlenHome, id: &str, pid: u32) -> Registry {
        Registry {
            v: 1,
            pid,
            session_id: id.into(),
            session_file: None,
            socket: super::super::run_dir(home).join(format!("{pid}.sock")),
            ext_version: None,
            omp_version: None,
            protocol: Some(1),
            mode: Some("tui".into()),
            started_at: None,
            cwd: None,
            title: None,
            pidns: None,
            boot_id: None,
        }
    }
    fn publish(home: &GlenHome, r: &Registry) {
        std::fs::write(path(home, &r.session_id), serde_json::to_vec(r).unwrap()).unwrap();
    }
    /// MEDIUM-001: a stale registry from another boot and another pid namespace names pid P,
    /// whose number this process probes in its own namespace: `P.sock` may be a live endpoint's
    /// now, so cleanup removes the registry and keeps the socket.
    #[test]
    fn an_other_boot_registry_of_another_namespace_keeps_the_socket() {
        let t = tempfile::tempdir().unwrap();
        let home = GlenHome {
            root: t.path().join("g"),
        };
        home.ensure_dir("run/live").unwrap();
        let pid = liveness::tests_dead_pid();
        let mut r = reg(&home, "old", pid);
        r.boot_id = Some("another-boot".into());
        r.pidns = Some(liveness::here().map_or(7, |h| h.pidns + 1));
        publish(&home, &r);
        std::fs::write(&r.socket, "a live endpoint's socket").unwrap();
        remove_if_unchanged(&home, &r, true).unwrap();
        assert!(r.socket.exists(), "the socket is kept");
        // The same boot record in this pid namespace: the pid is dead here, the socket goes.
        if let Some(h) = liveness::here() {
            r.pidns = Some(h.pidns);
            publish(&home, &r);
            assert!(remove_if_unchanged(&home, &r, true).unwrap());
            assert!(!path(&home, "old").exists());
            assert!(
                !r.socket.exists(),
                "this namespace's dead pid: the socket goes"
            );
        }
    }
    /// MEDIUM-001, as the extension's sweep does: a socket whose pid a current registry of
    /// another pid namespace names is never unlinked, whatever the stale registry says.
    #[test]
    fn a_pid_named_by_another_namespace_keeps_its_socket() {
        let t = tempfile::tempdir().unwrap();
        let home = GlenHome {
            root: t.path().join("g"),
        };
        home.ensure_dir("run/live").unwrap();
        let pid = liveness::tests_dead_pid();
        let mut live = reg(&home, "live", pid);
        live.pidns = Some(liveness::here().map_or(7, |h| h.pidns + 1));
        live.boot_id = liveness::here().map(|h| h.boot_id.clone());
        publish(&home, &live);
        let stale = reg(&home, "stale", pid);
        publish(&home, &stale);
        std::fs::write(&stale.socket, "the other namespace's endpoint").unwrap();
        assert!(pid_dead(&stale));
        assert!(remove_if_unchanged(&home, &stale, true).unwrap());
        assert!(!path(&home, "stale").exists());
        assert!(stale.socket.exists(), "the socket is kept");
        assert!(path(&home, "live").exists());
    }
}
