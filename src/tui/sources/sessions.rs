use super::{Ctx, Wake, Worker};
use crate::{
    commands::{ls, open},
    error::GlenError,
    live::{
        client::Hello,
        registry::{self, Registry},
    },
    omp::{
        bucket::bucket_for,
        lease::{self, Lease},
        listing::{self, Catalog, SessionFile, Summary},
        paths,
        session::LoadError,
    },
    render::{sanitize, text},
    tui::state::{ListSnapshot, Msg, Scope, SessionRow},
};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, Metadata},
    path::{Path, PathBuf},
    sync::mpsc::Receiver,
    time::{Duration, Instant, SystemTime},
};
const INTERVAL: Duration = Duration::from_secs(2);
pub enum Command {
    Refresh,
    Scope(Scope),
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: SystemTime,
}
impl Identity {
    fn of(m: &Metadata) -> std::io::Result<Self> {
        #[cfg(unix)]
        let (dev, ino) = {
            use std::os::unix::fs::MetadataExt;
            (m.dev(), m.ino())
        };
        #[cfg(not(unix))]
        let (dev, ino) = (0, 0);
        Ok(Self {
            dev,
            ino,
            size: m.len(),
            mtime: m.modified()?,
        })
    }
}
fn copy_lease(lease: &Lease) -> Lease {
    match lease {
        Lease::Free => Lease::Free,
        Lease::Held => Lease::Held,
        Lease::Unknown { reason, errno } => Lease::Unknown {
            reason: reason.clone(),
            errno: *errno,
        },
    }
}
#[derive(Default)]
struct ProbePolicy(HashMap<PathBuf, (Identity, Lease)>);
impl ProbePolicy {
    fn probe(&mut self, path: &Path, identity: &Identity, probe: impl FnOnce() -> Lease) -> Lease {
        if self
            .0
            .get(path)
            .is_some_and(|(old, lease)| old == identity && matches!(lease, Lease::Free))
        {
            return Lease::Free;
        }
        let lease = probe();
        self.0
            .insert(path.to_path_buf(), (identity.clone(), copy_lease(&lease)));
        lease
    }
}
type HeadCache = HashMap<PathBuf, (u64, SystemTime, Summary)>;
struct Tail {
    identity: Identity,
    raw: Summary,
    refined: Summary,
    held: bool,
}
#[derive(Default)]
struct Reader {
    /// A selected session may live in a bucket copied from another machine/home.
    bucket_override: Option<(PathBuf, String)>,
    heads: HeadCache,
    head_ids: HashMap<PathBuf, (u64, u64)>,
    tails: HashMap<PathBuf, Tail>,
    probes: ProbePolicy,
    #[cfg(test)]
    head_reads: usize,
    #[cfg(test)]
    tail_reads: usize,
}
impl Reader {
    fn refresh(
        &mut self,
        ctx: &Ctx,
        scope: &Scope,
    ) -> Result<(ListSnapshot, Vec<GlenError>), GlenError> {
        let endpoints = registry::states(&ctx.home);
        let registries = registry::index(&ctx.home)
            .into_iter()
            .filter(|r| endpoints.contains_key(&r.session_id))
            .collect();
        let paths = paths::resolve(&ctx.env, ctx.cli.global.profile.as_deref())?;
        self.collect(ctx, scope, endpoints, registries, |s| {
            lease::probe(&paths.session_owners_dir, &s.id)
        })
    }
    fn collect(
        &mut self,
        ctx: &Ctx,
        scope: &Scope,
        endpoints: HashMap<String, Hello>,
        registries: Vec<Registry>,
        mut probe: impl FnMut(&Summary) -> Lease,
    ) -> Result<(ListSnapshot, Vec<GlenError>), GlenError> {
        let paths = paths::resolve(&ctx.env, ctx.cli.global.profile.as_deref())?;
        let files = listing::discover(&paths.sessions_dir)
            .map_err(|e| GlenError::read(&paths.sessions_dir, e))?;
        let mut errors = vec![];
        let mut summaries = vec![];
        let mut identities = HashMap::new();
        let mut seen = HashSet::new();
        for discovered in files {
            if ctx.stopped() {
                break;
            }
            let path = discovered.path;
            seen.insert(path.clone());
            let result = (|| -> Result<Option<(Identity, Summary)>, GlenError> {
                let mut file = File::open(&path).map_err(|e| GlenError::read(&path, e))?;
                let metadata = file.metadata().map_err(|e| GlenError::read(&path, e))?;
                let id = Identity::of(&metadata).map_err(|e| GlenError::read(&path, e))?;
                if let Some((size, mtime, summary)) = self.heads.get(&path)
                    && *size == id.size
                    && *mtime == id.mtime
                    && self.head_ids.get(&path) == Some(&(id.dev, id.ino))
                {
                    return Ok(Some((id, summary.clone())));
                }
                #[cfg(test)]
                {
                    self.head_reads += 1;
                }
                let session_file = SessionFile::from_metadata(&path, &metadata)
                    .map_err(|e| GlenError::read(&path, e))?;
                let summary = match listing::read_head(session_file, &mut file) {
                    Ok(s) => s,
                    Err(LoadError::NotSession) => return Ok(None),
                    Err(e) => return Err(open::load_error(&path, e)),
                };
                self.head_ids.insert(path.clone(), (id.dev, id.ino));
                self.heads
                    .insert(path.clone(), (id.size, id.mtime, summary.clone()));
                Ok(Some((id, summary)))
            })();
            match result {
                Ok(Some((id, summary))) => {
                    identities.insert(path, id);
                    summaries.push(summary);
                }
                Ok(None) => {
                    self.heads.remove(&path);
                    self.head_ids.remove(&path);
                }
                Err(e) => errors.push(e),
            }
        }
        self.heads.retain(|p, _| seen.contains(p));
        self.head_ids.retain(|p, _| seen.contains(p));
        self.tails.retain(|p, _| seen.contains(p));
        self.probes.0.retain(|p, _| seen.contains(p));
        listing::sort_newest_first(&mut summaries);
        let mut catalog = Catalog {
            sessions_dir: paths.sessions_dir.clone(),
            sessions: summaries,
            handles: HashMap::new(),
            unreadable: vec![],
        };
        registry::union(&mut catalog, &ctx.home, &ctx.env, &registries);
        catalog.assign_handles(None);
        let bucket = match scope {
            Scope::Bucket { cwd, .. } => Some(
                self.bucket_override
                    .as_ref()
                    .filter(|(project, _)| project == cwd)
                    .map(|(_, bucket)| bucket.clone())
                    .unwrap_or_else(|| {
                        bucket_for(&paths::absolute(&ctx.env, &cwd.to_string_lossy()), &ctx.env)
                    }),
            ),
            Scope::All => None,
        };
        let mut total_all = catalog.sessions.len();
        let mut rows = vec![];
        for head in &catalog.sessions {
            if ctx.stopped() {
                break;
            }
            if bucket
                .as_ref()
                .is_some_and(|bucket| bucket != &head.file.bucket)
            {
                continue;
            }
            let path = &head.file.path;
            let endpoint = endpoints.get(&head.id);
            let identity = identities.get(path);
            let mut summary = head.clone();
            if let Some(identity) = identity {
                if self
                    .tails
                    .get(path)
                    .is_none_or(|tail| tail.identity != *identity)
                {
                    #[cfg(test)]
                    {
                        self.tail_reads += 1;
                    }
                    if let Err(e) = listing::scan_tail(&mut summary) {
                        if endpoint.is_none() {
                            total_all = total_all.saturating_sub(1);
                            errors.push(GlenError::read(path, e));
                            continue;
                        }
                        errors.push(GlenError::read(path, e));
                    }
                    self.tails.insert(
                        path.clone(),
                        Tail {
                            identity: identity.clone(),
                            raw: summary.clone(),
                            refined: summary.clone(),
                            held: false,
                        },
                    );
                }
                if let Some(tail) = self.tails.get(path) {
                    summary = tail.raw.clone();
                }
            }
            let lease = if endpoint.is_some() {
                self.probes.0.remove(path);
                Lease::Free
            } else if let Some(identity) = identity {
                self.probes.probe(path, identity, || probe(&summary))
            } else {
                probe(&summary)
            };
            // Preserve disk-only last-turn inference when a held lease becomes free.
            if matches!(lease, Lease::Held)
                && let Some(tail) = self.tails.get(path)
                && tail.held
            {
                summary = tail.refined.clone();
            }
            let inferred = ls::infer_row(&mut summary, endpoint, &ctx.env, |_| lease);
            if let Some(tail) = self.tails.get_mut(path) {
                tail.held = matches!(inferred.lease, Lease::Held);
                tail.refined = summary.clone();
            }
            let title = summary
                .title
                .as_deref()
                .or(summary.first_message.as_deref())
                .map(text::single_line)
                .unwrap_or_else(|| {
                    format!(
                        "untitled · {}",
                        summary
                            .created
                            .map(|t| text::local_hm(&t))
                            .unwrap_or_default()
                    )
                });
            rows.push(SessionRow {
                id: summary.id.clone(),
                handle: sanitize(&catalog.handle(&summary.id)).into_owned(),
                path: identity.map(|_| path.clone()),
                title,
                first_message: summary.first_message.as_deref().map(text::single_line),
                cwd: summary.cwd,
                state: inferred.state,
                source: inferred.source,
                last_turn: summary.last_turn,
                updated: summary.file.mtime,
                model: summary.model,
            });
        }
        let sessions_dir_missing = match fs::metadata(&paths.sessions_dir) {
            Ok(_) => None,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(paths.sessions_dir),
            Err(e) => return Err(GlenError::read(&paths.sessions_dir, e)),
        };
        Ok((
            ListSnapshot {
                rows,
                total_all,
                sessions_dir_missing,
            },
            errors,
        ))
    }
}
pub fn spawn(ctx: Ctx, rx_cmd: Receiver<Command>) -> Worker {
    let scope = Scope::Bucket {
        cwd: ctx.env.cwd().into(),
        label: String::new(),
    };
    spawn_scoped(ctx, rx_cmd, scope, None)
}
/// Preserve the selected session's physical bucket, even when its recorded cwd moved.
pub fn spawn_scoped(
    ctx: Ctx,
    rx_cmd: Receiver<Command>,
    mut scope: Scope,
    bucket_override: Option<(PathBuf, String)>,
) -> Worker {
    std::thread::spawn(move || {
        let mut reader = Reader {
            bucket_override,
            ..Reader::default()
        };
        let mut next = Instant::now();
        while !ctx.stopped() {
            if let Ok(command) = rx_cmd.try_recv() {
                for command in std::iter::once(command).chain(rx_cmd.try_iter()) {
                    if let Command::Scope(value) = command {
                        scope = value;
                    }
                }
                next = Instant::now();
            }
            if Instant::now() >= next {
                match reader.refresh(&ctx, &scope) {
                    Ok((snapshot, errors)) => {
                        for error in errors {
                            if !ctx.error("sessions", error) {
                                return;
                            }
                        }
                        if !ctx.send(Msg::Sessions(snapshot)) {
                            return;
                        }
                    }
                    Err(e) => {
                        if !ctx.error("sessions", e) {
                            return;
                        }
                    }
                }
                next = Instant::now() + INTERVAL;
            }
            match ctx.wait(&rx_cmd, next) {
                Wake::Command(command) => {
                    for command in std::iter::once(command).chain(rx_cmd.try_iter()) {
                        if let Command::Scope(value) = command {
                            scope = value;
                        }
                    }
                    next = Instant::now();
                }
                Wake::Tick => (),
                Wake::Stop => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cli::LsArgs,
        output::{Mode, OutputCtx, Terminal},
        tui::sources::tests::Rig,
    };
    fn collect(reader: &mut Reader, ctx: &Ctx, scope: &Scope, count: &mut usize) -> ListSnapshot {
        let (snapshot, errors) = reader
            .collect(ctx, scope, HashMap::new(), vec![], |_| {
                *count += 1;
                Lease::Free
            })
            .unwrap();
        assert!(errors.is_empty());
        snapshot
    }
    #[test]
    fn head_tail_and_free_probe_caches_only_invalidate_the_changed_row() {
        let rig = Rig::new(true);
        let (ctx, _) = rig.ctx();
        let mut reader = Reader::default();
        let mut count = 0;
        let first = collect(&mut reader, &ctx, &Scope::All, &mut count);
        assert_eq!(first.rows.len(), 7);
        assert_eq!((count, reader.head_reads, reader.tail_reads), (7, 7, 7));
        collect(&mut reader, &ctx, &Scope::All, &mut count);
        assert_eq!((count, reader.head_reads, reader.tail_reads), (7, 7, 7));
        let path = rig.path('A');
        let file = File::open(&path).unwrap();
        let mtime = file.metadata().unwrap().modified().unwrap();
        file.set_modified(mtime + Duration::from_secs(1)).unwrap();
        collect(&mut reader, &ctx, &Scope::All, &mut count);
        assert_eq!((count, reader.head_reads, reader.tail_reads), (8, 8, 8));
        let staged = path.with_file_name(".replacement.tmp");
        fs::copy(&path, &staged).unwrap();
        File::open(&staged)
            .unwrap()
            .set_modified(mtime + Duration::from_secs(1))
            .unwrap();
        fs::rename(staged, &path).unwrap();
        collect(&mut reader, &ctx, &Scope::All, &mut count);
        assert_eq!((count, reader.head_reads, reader.tail_reads), (9, 9, 9));
    }
    #[test]
    fn endpoint_rows_are_never_probed_and_held_or_unknown_leases_are_reprobed() {
        let rig = Rig::new(true);
        let (ctx, _) = rig.ctx();
        let mut reader = Reader::default();
        let id = listing::scan_head(SessionFile::at(&rig.path('A')).unwrap())
            .unwrap()
            .id;
        let endpoint = Hello {
            protocol: 1,
            ops: vec!["send".into()],
            ext_version: None,
            omp_version: None,
            pid: std::process::id(),
            session_id: id.clone(),
            session_file: None,
            mode: Some("tui".into()),
            state: "busy".into(),
            busy: true,
            queued: false,
            active_requests: 1,
        };
        let mut calls = 0;
        let (snapshot, errors) = reader
            .collect(
                &ctx,
                &Scope::All,
                HashMap::from([(id.clone(), endpoint)]),
                vec![],
                |_| {
                    calls += 1;
                    Lease::Free
                },
            )
            .unwrap();
        assert!(errors.is_empty());
        assert_eq!(calls, 6);
        assert_eq!(
            snapshot.rows.iter().find(|r| r.id == id).unwrap().source,
            "extension"
        );
        collect(&mut reader, &ctx, &Scope::All, &mut calls);
        assert_eq!(calls, 7);
        let identity = Identity::of(&fs::metadata(rig.path('A')).unwrap()).unwrap();
        let mut policy = ProbePolicy::default();
        let mut n = 0;
        for _ in 0..2 {
            policy.probe(Path::new("held"), &identity, || {
                n += 1;
                Lease::Held
            });
            policy.probe(Path::new("unknown"), &identity, || {
                n += 1;
                Lease::Unknown {
                    reason: "test".into(),
                    errno: None,
                }
            });
        }
        assert_eq!(n, 4);
    }
    #[test]
    fn scoped_snapshot_matches_ls_and_handles_are_assigned_before_filtering() {
        let rig = Rig::new(true);
        let (ctx, _) = rig.ctx();
        let mut reader = Reader::default();
        let scope = Scope::Bucket {
            cwd: ctx.env.cwd().into(),
            label: "alpha".into(),
        };
        let mut calls = 0;
        let snapshot = collect(&mut reader, &ctx, &scope, &mut calls);
        let terminal = Terminal {
            stdout_tty: false,
            stderr_tty: false,
            stdout_cols: None,
        };
        let output = OutputCtx::fallback(Mode::Json, &ctx.env, &terminal);
        let args = LsArgs {
            all: false,
            cwd: Some(ctx.env.cwd().into()),
            limit: 0,
            live: false,
            since: None,
        };
        let cli = ls::run(&ctx.cli, &args, &ctx.env, &output).unwrap();
        let rows = cli.data["sessions"].as_array().unwrap();
        assert_eq!(snapshot.total_all, 7);
        assert_eq!(snapshot.rows.len(), 4);
        assert_eq!(snapshot.rows.len(), rows.len());
        for (actual, expected) in snapshot.rows.iter().zip(rows) {
            assert_eq!(actual.handle, expected["handle"]);
            assert_eq!(
                serde_json::to_value(actual.state).unwrap(),
                expected["state"]
            );
            let title = expected["title"]
                .as_str()
                .or_else(|| expected["first_message"].as_str())
                .unwrap();
            assert_eq!(actual.title, text::single_line(title));
        }
    }
    #[test]
    fn scope_and_refresh_commands_publish_immediately() {
        use crate::tui::sources::tests::{stop, wait};
        use std::sync::mpsc;
        let rig = Rig::new(true);
        let (ctx, rx) = rig.ctx();
        let (tx, commands) = mpsc::channel();
        let worker = spawn(ctx.clone(), commands);
        let initial = wait(&rx, |m| matches!(m, Msg::Sessions(_)));
        assert!(matches!(initial, Msg::Sessions(s) if s.rows.len() == 4));
        let start = Instant::now();
        tx.send(Command::Scope(Scope::All)).unwrap();
        // Refresh must not discard the preceding scope command when coalesced.
        tx.send(Command::Refresh).unwrap();
        let all = wait(&rx, |m| matches!(m, Msg::Sessions(s) if s.rows.len() == 7));
        assert!(matches!(all, Msg::Sessions(s) if s.total_all == 7));
        assert!(start.elapsed() < Duration::from_millis(500));
        stop(&ctx, worker);
    }
    #[test]
    fn initial_bucket_override_survives_all_toggle_and_is_bound_to_its_project() {
        use crate::tui::sources::tests::{stop, wait};
        use std::sync::mpsc;
        let rig = Rig::new(true);
        let (ctx, rx) = rig.ctx();
        let project = PathBuf::from("/old/machine/work/beta");
        let scope = Scope::Bucket {
            cwd: project.clone(),
            label: "beta".into(),
        };
        let (tx, commands) = mpsc::channel();
        let worker = spawn_scoped(
            ctx.clone(),
            commands,
            scope.clone(),
            Some((project, "-work-beta".into())),
        );
        assert!(
            matches!(wait(&rx,|m|matches!(m,Msg::Sessions(_))),Msg::Sessions(s) if s.rows.len() == 3)
        );
        tx.send(Command::Scope(Scope::All)).unwrap();
        assert!(
            matches!(wait(&rx,|m|matches!(m,Msg::Sessions(_))),Msg::Sessions(s) if s.rows.len() == 7)
        );
        tx.send(Command::Scope(scope)).unwrap();
        assert!(
            matches!(wait(&rx,|m|matches!(m,Msg::Sessions(_))),Msg::Sessions(s) if s.rows.len() == 3)
        );
        tx.send(Command::Scope(Scope::Bucket {
            cwd: ctx.env.cwd().into(),
            label: "alpha".into(),
        }))
        .unwrap();
        assert!(
            matches!(wait(&rx,|m|matches!(m,Msg::Sessions(_))),Msg::Sessions(s) if s.rows.len() == 4)
        );
        stop(&ctx, worker);
    }
}
