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
}
pub fn path(home: &GlenHome, id: &str) -> PathBuf {
    super::run_dir(home).join(format!("{}.json", lock_key(id)))
}
pub enum Read {
    Missing,
    Unreadable(String),
    Ok(Box<Registry>),
}
pub fn read(home: &GlenHome, id: &str) -> Read {
    let p = path(home, id);
    match fs::read(&p) {
        Ok(b) => match serde_json::from_slice::<Registry>(&b) {
            Ok(r) if r.v == 1 && r.pid > 0 && r.session_id == id && r.socket.is_absolute() => {
                Read::Ok(Box::new(r))
            }
            Ok(_) => Read::Unreadable(format!("invalid registry {}", p.display())),
            Err(e) => Read::Unreadable(format!("{}: {e}", p.display())),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Missing,
        Err(e) => Read::Unreadable(format!("{}: {e}", p.display())),
    }
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
        .filter(|r| r.v == 1 && r.pid > 0)
        .collect()
}
pub fn pid_dead(pid: u32) -> bool {
    matches!(liveness::start_token(pid), Probe::Gone)
}
pub fn remove_if_unchanged(
    home: &GlenHome,
    reg: &Registry,
    socket_too: bool,
) -> Result<bool, crate::error::GlenError> {
    let target = path(home, &reg.session_id);
    let tombstone = super::run_dir(home).join(format!(
        ".{}.{}.stale",
        lock_key(&reg.session_id),
        uuid::Uuid::now_v7()
    ));
    let io = |e| {
        crate::error::GlenError::new(
            crate::error::ErrorCode::Io,
            format!(
                "cannot clean live registry {} for session {}: {e}",
                target.display(),
                reg.session_id
            ),
        )
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
    if socket_too && reg.socket == owned_socket && pid_dead(reg.pid) {
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
        if pid_dead(r.pid) || catalog.sessions.iter().any(|s| s.id == r.session_id) {
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
    let (tx, rx) = std::sync::mpsc::channel();
    let registries = index(home);
    let count = registries.len();
    for r in registries {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let result = super::client::state(&r.socket, super::STATE_LS)
                .ok()
                .filter(|s| s.session_id == r.session_id);
            let _ = tx.send(result);
        });
    }
    drop(tx);
    let end = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let mut states = std::collections::HashMap::new();
    for _ in 0..count {
        match rx.recv_timeout(end.saturating_duration_since(std::time::Instant::now())) {
            Ok(Some(s)) => {
                states.insert(s.session_id.clone(), s);
            }
            Ok(None) => (),
            Err(_) => break,
        }
    }
    states
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
        let value = serde_json::json!({"v":1,"pid":std::process::id(),"session_id":id,"socket":"/tmp/socket","started_at":"2026-10-30T00:00:00Z","future":true});
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
