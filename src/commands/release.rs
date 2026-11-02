//! `glen release`: end warm workers (v0.2 W4, design §A21).
use crate::{
    cli::{Cli, ReleaseArgs},
    env::Env,
    error::{ErrorCode, GlenError},
    glen_home::{self, GlenHome},
    jobs::{JobId, JobStore, liveness::Liveness, lock, matches_session},
    output::{Outcome, OutputCtx},
    render::palette::Token,
    rpc::{
        self,
        warm::{self, Class, EntryState, Indexed},
    },
};
use serde_json::{Value, json};
use std::time::{Duration, Instant, SystemTime};

const POLL: Duration = Duration::from_millis(50);

pub fn run(
    _cli: &Cli,
    args: &ReleaseArgs,
    env: &Env,
    ctx: &OutputCtx,
) -> Result<Outcome, GlenError> {
    // Trusted before anything is read or signalled; creates nothing.
    let home = glen_home::resolve_trusted(env, &["run/warm", "run/locks", "jobs"])?;
    let (entries, _) = warm::index(&home);
    let targets = match &args.target {
        None => entries,
        Some(target) => vec![select(&home, entries, target)?],
    };
    let stale = warm::stale_bound(env);
    let mut rows = vec![];
    let mut timed_out = false;
    for target in &targets {
        // The state as found, before this command changed it.
        let state = target
            .entry
            .class(target.mtime, SystemTime::now(), stale)
            .as_str(&target.entry);
        let (outcome, ok) = release_one(env, &home, target, args, stale)?;
        timed_out |= !ok;
        rows.push((target, state, outcome));
    }
    let workers: Vec<Value> = rows
        .iter()
        .map(|(t, state, outcome)| row(t, state, outcome))
        .collect();
    if timed_out {
        return Err(GlenError::new(
            ErrorCode::ControlFailed,
            "a warm worker did not release in time",
        )
        .with_hint(format!(
            "wait and retry, or kill it with `glen release {} --force`",
            args.target.as_deref().unwrap_or("--all")
        ))
        .with_details(json!({"reason":"release_timeout","workers":workers})));
    }
    let human = if rows.is_empty() {
        ctx.painter.paint(Token::Muted, "no warm workers")
    } else {
        rows.iter()
            .map(|(t, _, outcome)| human_line(ctx, t, outcome))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(Outcome::new(
        json!({"total":workers.len(),"workers":workers}),
        human,
    ))
}

/// One warm worker by session id, a hex tail of 4 or more, or a job handle (`run/warm` only).
fn select(home: &GlenHome, entries: Vec<Indexed>, target: &str) -> Result<Indexed, GlenError> {
    let mut found: Vec<Indexed> = entries
        .iter()
        .filter(|e| matches_session(Some(&e.entry.session_id), target))
        .cloned()
        .collect();
    if found.is_empty()
        && (target.starts_with(['j', 'J']) || JobId::parse(target).is_some())
        && let Ok(id) = JobStore::new(home.clone()).resolve(target)
        && let Ok(job) = JobStore::new(home.clone()).read_job(&id)
    {
        found = entries
            .into_iter()
            .filter(|e| {
                e.entry.job_id == id.as_str()
                    || job.session_id.as_deref() == Some(e.entry.session_id.as_str())
            })
            .collect();
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(
            GlenError::new(ErrorCode::NotFound, format!("no warm worker matches `{target}`"))
                .with_hint("run `glen ls` to see warm sessions")
                .with_details(json!({"reason":"not_warm","target":target})),
        ),
        _ => Err(GlenError::new(
            ErrorCode::Ambiguous,
            format!("several warm workers match `{target}`"),
        )
        .with_details(json!({"candidates":found.iter().map(|e| json!({"id":e.entry.session_id,"handle":rpc::session_handle(&e.entry.session_id)})).collect::<Vec<_>>()}))),
    }
}

fn row(t: &Indexed, state: &str, outcome: &str) -> Value {
    let e = &t.entry;
    json!({
        "session":{"id":e.session_id,"handle":rpc::session_handle(&e.session_id)},
        "job":JobId::parse(&e.job_id).map(|id| id.handle()),
        "pid":e.pid,
        "state":state,
        "outcome":outcome,
    })
}

fn human_line(ctx: &OutputCtx, t: &Indexed, outcome: &str) -> String {
    let p = &ctx.painter;
    let handle = rpc::session_handle(&t.entry.session_id);
    match outcome {
        "released" => p.paint(
            Token::Success,
            &format!("✓ released {handle} (pid {})", t.entry.pid),
        ),
        "cooling" => p.paint(
            Token::Focus,
            &format!(
                "◐ {handle} cools after {}",
                JobId::parse(&t.entry.job_id)
                    .map_or_else(|| t.entry.job_id.clone(), |id| id.handle())
            ),
        ),
        "forced" => p.paint(Token::Warning, &format!("! killed {handle}")),
        _ => p.paint(
            Token::Muted,
            &format!("· removed a dead entry for {handle}"),
        ),
    }
}

/// The outcome for one worker, and false when it did not release within the timeout.
fn release_one(
    env: &Env,
    home: &GlenHome,
    t: &Indexed,
    args: &ReleaseArgs,
    stale: Duration,
) -> Result<(&'static str, bool), GlenError> {
    let e = &t.entry;
    let class = e.class(t.mtime, SystemTime::now(), stale);
    if class == Class::Dead {
        cleanup(home, &e.session_id);
        return Ok(("cleaned", true));
    }
    warm::request_release(home, &e.session_id, &e.ident())
        .map_err(|err| crate::sandbox::map_home_write(env, "release", err))?;
    if class == Class::Fresh && e.state == EntryState::Busy {
        return Ok(("cooling", true));
    }
    if class == Class::Stale && args.force {
        return Ok(force(home, t));
    }
    let end = Instant::now() + args.timeout;
    loop {
        if warm::read(home, &e.session_id).is_none_or(|now| now.entry.pid != e.pid)
            && !lock::is_held(home, &e.session_id)
        {
            return Ok(("released", true));
        }
        if Instant::now() >= end {
            return Ok(if args.force {
                force(home, t)
            } else {
                ("timeout", false)
            });
        }
        std::thread::sleep(POLL.min(end.saturating_duration_since(Instant::now())));
    }
}

/// SIGKILL the omp group and the worker, but only processes whose pid and start token are proven
/// unchanged (an `Unknown` identity is never signalled and falls to `release_timeout`).
fn force(home: &GlenHome, t: &Indexed) -> (&'static str, bool) {
    let e = &t.entry;
    let identity = warm::kill_identity(e);
    match identity.worker {
        Liveness::Dead => {
            cleanup(home, &e.session_id);
            ("cleaned", true)
        }
        Liveness::Unknown => ("timeout", false),
        Liveness::Alive => {
            if identity.omp == Some(Liveness::Alive)
                && let Some(omp) = e.omp_pid
            {
                let _ = rpc::kill_group(omp, rustix::process::Signal::KILL);
            }
            if let Some(pid) = rustix::process::Pid::from_raw(e.pid as i32) {
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            }
            cleanup(home, &e.session_id);
            // The kernel frees the flock with the worker; wait a moment for it.
            let end = Instant::now() + Duration::from_secs(2);
            while lock::is_held(home, &e.session_id) && Instant::now() < end {
                std::thread::sleep(POLL);
            }
            ("forced", true)
        }
    }
}

fn cleanup(home: &GlenHome, session_id: &str) {
    let _ = warm::remove(home, session_id);
    let _ = warm::clear_release(home, session_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    const A: &str = "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de";
    const B: &str = "01a0f6b1-2680-7a3c-8b21-4a1f9e00c0de";
    fn entry(session: &str, pid: u32, start: Option<String>) -> warm::Entry {
        warm::Entry {
            v: 1,
            pid,
            start,
            session_id: session.into(),
            session_handle: None,
            job_id: "01a18bcc-6970-7685-8f03-0ecc16977273".into(),
            omp_pid: None,
            omp_start: None,
            warm_ms: 1000,
            expires_at: "2099-01-01T00:00:00Z".into(),
            state: EntryState::Idle,
            started_at: "2026-11-02T00:00:00Z".into(),
        }
    }
    fn home(t: &tempfile::TempDir) -> GlenHome {
        GlenHome {
            root: t.path().join("glen"),
        }
    }
    #[test]
    fn targets_match_an_id_a_tail_or_a_job_handle() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        for s in [A, B] {
            warm::publish(&h, &entry(s, std::process::id(), None)).unwrap();
        }
        let entries = || warm::index(&h).0;
        assert_eq!(select(&h, entries(), A).unwrap().entry.session_id, A);
        assert_eq!(
            select(&h, entries(), &A.to_uppercase())
                .unwrap()
                .entry
                .session_id,
            A
        );
        assert_eq!(
            select(&h, entries(), "e27c0de").unwrap().entry.session_id,
            A
        );
        assert_eq!(
            select(&h, entries(), "e00c0de").unwrap().entry.session_id,
            B
        );
        let e = select(&h, entries(), "27b").unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
        assert_eq!(e.details.unwrap()["reason"], "not_warm");
        let e = select(&h, entries(), "c0de").unwrap_err();
        assert_eq!(e.code, ErrorCode::Ambiguous, "a tail both sessions share");
        // A job handle: its session's worker.
        let store = JobStore::new(h.clone());
        let mut j = crate::jobs::tests::job();
        j.session_id = Some(B.into());
        store.create(&j).unwrap();
        let handle = JobId::parse(&j.id).unwrap().handle();
        assert_eq!(select(&h, entries(), &handle).unwrap().entry.session_id, B);
        let e = select(&h, entries(), "jfffffff").unwrap_err();
        assert_eq!(e.details.unwrap()["reason"], "not_warm");
    }
    #[test]
    fn dead_and_mismatched_entries_are_cleaned_never_signalled() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let args = ReleaseArgs {
            target: None,
            all: true,
            force: true,
            timeout: Duration::from_millis(100),
        };
        let env = Env::for_test(None, t.path(), &[]);
        // This test process stands in for a worker whose start token no longer matches.
        warm::publish(&h, &entry(A, std::process::id(), Some("reused".into()))).unwrap();
        let (entries, _) = warm::index(&h);
        let (outcome, ok) = release_one(&env, &h, &entries[0], &args, warm::STALE).unwrap();
        assert_eq!((outcome, ok), ("cleaned", true));
        assert!(warm::read(&h, A).is_none());
        assert!(!warm::release_path(&h, A).exists());
        let doc = row(&entries[0], "dead", outcome);
        assert_eq!(doc["state"], "dead");
        assert_eq!(doc["outcome"], "cleaned");
        assert_eq!(doc["session"]["handle"], "e27c0de");
        assert_eq!(doc["job"], "j6977273");
    }
    #[test]
    fn an_unproven_identity_is_never_signalled() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        // A live pid without a start token: `Unknown`, so --force cannot prove it.
        let mut e = entry(A, std::process::id(), None);
        e.state = EntryState::Idle;
        let indexed = Indexed {
            entry: e,
            mtime: SystemTime::now() - Duration::from_secs(60),
        };
        assert_eq!(force(&h, &indexed), ("timeout", false));
    }
}
