use super::{JobId, JobStore, Reply, Status, liveness};
use crate::error::GlenError;
use std::{
    collections::HashSet,
    fs,
    process::Child,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
pub enum Waited {
    Terminal(Status),
    Running(Status),
    Interrupted(Status),
    /// `wait --for reply`: unseen replies landed while the job was still queued or accepted.
    Replied(Status, Vec<Reply>),
}
/// Watches `jobs/<id>/replies.ndjson` for replies the caller has not seen (design §A20). Both
/// routes write that file, so one reader serves RPC and live jobs alike.
pub struct ReplyWatch {
    seen: HashSet<String>,
    last: Option<(u64, u64)>,
}
impl ReplyWatch {
    /// The seen set is every reply up to and including `after`, or empty without it. `None`
    /// when `after` names no reply of this job.
    pub fn new(
        store: &JobStore,
        id: &JobId,
        after: Option<&str>,
    ) -> Result<Option<Self>, GlenError> {
        let mut seen = HashSet::new();
        if let Some(after) = after {
            let replies = store.read_replies_complete(id)?;
            let Some(end) = replies.iter().position(|r| r.id == after) else {
                return Ok(None);
            };
            seen.extend(replies[..=end].iter().map(|r| r.id.clone()));
        }
        Ok(Some(Self { seen, last: None }))
    }
    /// Unseen replies in file order. The file is re-read only when its length or inode changed
    /// since the last poll; the key is stored before the read, so an append that races the read
    /// changes the key and is read on the next poll.
    pub fn poll(&mut self, store: &JobStore, id: &JobId) -> Result<Vec<Reply>, GlenError> {
        let path = store.replies_path(id);
        let key = match fs::metadata(&path) {
            Ok(meta) => file_key(&meta),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (0, 0),
            Err(e) => return Err(GlenError::read(&path, e)),
        };
        if self.last == Some(key) {
            return Ok(vec![]);
        }
        self.last = Some(key);
        Ok(self.unseen(store.read_replies_complete(id)?))
    }
    pub fn seen(&self) -> &HashSet<String> {
        &self.seen
    }
    fn unseen(&self, replies: Vec<Reply>) -> Vec<Reply> {
        replies
            .into_iter()
            .filter(|r| !self.seen.contains(&r.id))
            .collect()
    }
}
fn file_key(meta: &fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.len(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        (meta.len(), 0)
    }
}
/// A non-empty reply poll is followed by one more terminal check: a job that ended meanwhile
/// returns its terminal outcome, which then carries every unseen reply.
fn wake_or_terminal(
    store: &JobStore,
    id: &JobId,
    replies: Vec<Reply>,
) -> Result<Waited, GlenError> {
    let s = liveness::refresh(store, id, store.read_status(id)?);
    Ok(if s.status.is_terminal() {
        Waited::Terminal(s)
    } else {
        Waited::Replied(s, replies)
    })
}
pub fn install_interrupt() -> Result<Arc<AtomicBool>, GlenError> {
    let f = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    {
        signal_hook::flag::register_conditional_shutdown(
            signal_hook::consts::SIGINT,
            130,
            f.clone(),
        )?;
        signal_hook::flag::register(signal_hook::consts::SIGINT, f.clone())?;
    }
    Ok(f)
}
/// [`install_interrupt`] plus SIGTERM and SIGHUP on the same flag, for commands that own a
/// short-lived omp process group and must tear it down on any polite stop (W8-ERR-3). The first
/// signal sets the flag and the command returns its interrupted outcome; a repeat of a signal
/// once the flag is set exits at once with 128 plus that signal's number.
pub fn install_interrupt_all() -> Result<Arc<AtomicBool>, GlenError> {
    let f = install_interrupt()?;
    #[cfg(unix)]
    for (signal, code) in [
        (signal_hook::consts::SIGTERM, 143),
        (signal_hook::consts::SIGHUP, 129),
    ] {
        signal_hook::flag::register_conditional_shutdown(signal, code, f.clone())?;
        signal_hook::flag::register(signal, f.clone())?;
    }
    Ok(f)
}
pub fn observe(
    store: &JobStore,
    id: &JobId,
    started: Instant,
    bound: Duration,
    interrupt: &AtomicBool,
    child: Option<&mut Child>,
    on_change: impl FnMut(&Status),
) -> Result<Waited, GlenError> {
    observe_with(store, id, started, bound, interrupt, child, None, on_change)
}
/// [`observe`] plus an optional reply watch. Each tick checks, in order: terminal status, child
/// exit, the SIGINT flag, new replies (with a watch), then the bound. A watch ticks every 100 ms.
#[allow(clippy::too_many_arguments)]
pub fn observe_with(
    store: &JobStore,
    id: &JobId,
    started: Instant,
    bound: Duration,
    interrupt: &AtomicBool,
    mut child: Option<&mut Child>,
    mut watch: Option<&mut ReplyWatch>,
    mut on_change: impl FnMut(&Status),
) -> Result<Waited, GlenError> {
    let mut previous = None;
    loop {
        let mut s = liveness::refresh(store, id, store.read_status(id)?);
        if s.status.is_terminal() {
            return Ok(Waited::Terminal(s));
        }
        let exited = match &mut child {
            Some(c) => c.try_wait()?.map(|_| c.id()),
            None => None,
        };
        if let Some(child_pid) = exited {
            std::thread::sleep(Duration::from_millis(100));
            s = store.read_status(id)?;
            if !s.status.is_terminal() {
                // A warm worker claimed the job (v0.2 W4): its blocked child exits, and the
                // claimant's pid and start token decide liveness from here on.
                if s.worker_pid.is_some_and(|p| p != child_pid) {
                    child = None;
                    continue;
                }
                s = liveness::mark_lost_unless_claimed(store, id, child_pid, s);
                if !s.status.is_terminal() {
                    child = None;
                    continue;
                }
            }
            return Ok(Waited::Terminal(s));
        }
        if interrupt.load(Ordering::SeqCst) {
            return Ok(Waited::Interrupted(s));
        }
        if let Some(w) = watch.as_deref_mut() {
            let replies = w.poll(store, id)?;
            if !replies.is_empty() {
                return wake_or_terminal(store, id, replies);
            }
        }
        if started.elapsed() >= bound {
            return Ok(Waited::Running(s));
        }
        if previous != Some((s.status, s.waiting_for_lock, s.omp_pid)) {
            on_change(&s);
            previous = Some((s.status, s.waiting_for_lock, s.omp_pid));
        }
        std::thread::sleep(
            if watch.is_some() || started.elapsed() < Duration::from_secs(2) {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(250)
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{State, tests};
    #[test]
    fn observer_terminal_bound_interrupt_and_lost() {
        let t = tempfile::tempdir().unwrap();
        let store = tests::store(t.path());
        let j = tests::job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        let mut s = store.read_status(&id).unwrap();
        let flag = AtomicBool::new(false);
        assert!(matches!(
            observe(
                &store,
                &id,
                Instant::now(),
                Duration::ZERO,
                &flag,
                None,
                |_| {}
            )
            .unwrap(),
            Waited::Running(_)
        ));
        flag.store(true, Ordering::SeqCst);
        assert!(matches!(
            observe(
                &store,
                &id,
                Instant::now(),
                Duration::from_secs(1),
                &flag,
                None,
                |_| {}
            )
            .unwrap(),
            Waited::Interrupted(_)
        ));
        s.status = State::Completed;
        store.write_status(&id, &s).unwrap();
        assert!(matches!(
            observe(
                &store,
                &id,
                Instant::now(),
                Duration::from_secs(5),
                &flag,
                None,
                |_| {}
            )
            .unwrap(),
            Waited::Terminal(Status {
                status: State::Completed,
                ..
            })
        ));
        flag.store(false, Ordering::SeqCst);
        s.status = State::Accepted;
        s.worker_pid = Some(std::process::id());
        s.worker_start = Some("wrong".into());
        store.write_status(&id, &s).unwrap();
        assert!(matches!(
            observe(
                &store,
                &id,
                Instant::now(),
                Duration::from_secs(5),
                &flag,
                None,
                |_| {}
            )
            .unwrap(),
            Waited::Terminal(Status {
                status: State::Lost,
                ..
            })
        ));
        assert_eq!(store.read_status(&id).unwrap().status, State::Lost);
    }
    #[test]
    fn observer_detects_child_exit_and_status_change() {
        let t = tempfile::tempdir().unwrap();
        let store = tests::store(t.path());
        let j = tests::job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        let flag = AtomicBool::new(false);
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let mut changes = vec![];
        let result = observe(
            &store,
            &id,
            Instant::now(),
            Duration::from_secs(3),
            &flag,
            Some(&mut child),
            |s| changes.push(s.status),
        )
        .unwrap();
        assert!(matches!(
            result,
            Waited::Terminal(Status {
                status: State::Lost,
                ..
            })
        ));
        assert_eq!(store.read_status(&id).unwrap().status, State::Lost);
        child.wait().unwrap();
    }

    #[test]
    fn observer_follows_a_claimant_after_its_child_exits() {
        let t = tempfile::tempdir().unwrap();
        let store = tests::store(t.path());
        let flag = AtomicBool::new(false);
        let me = liveness::current().unwrap();
        let exited = || {
            let mut c = std::process::Command::new("/bin/sh")
                .args(["-c", "exit 0"])
                .spawn()
                .unwrap();
            while c.try_wait().unwrap().is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
            c
        };
        let job_named = |pid: u32, start: String| {
            let j = tests::job();
            let id = JobId::parse(&j.id).unwrap();
            store.create(&j).unwrap();
            let mut s = store.read_status(&id).unwrap();
            s.worker_pid = Some(pid);
            s.worker_start = Some(start);
            s.omp_reused = true;
            store.write_status(&id, &s).unwrap();
            id
        };
        // A live claimant (this process): the exited child is not a reason to mark it lost.
        let id = job_named(me.pid, me.start.clone());
        let mut child = exited();
        let started = Instant::now();
        let waited = observe(
            &store,
            &id,
            started,
            Duration::from_millis(400),
            &flag,
            Some(&mut child),
            |_| {},
        )
        .unwrap();
        assert!(matches!(waited, Waited::Running(_)));
        assert!(started.elapsed() >= Duration::from_millis(400));
        assert_eq!(store.read_status(&id).unwrap().status, State::Queued);
        // A dead claimant is lost through liveness, not the child.
        let id = job_named(me.pid, "reused".into());
        let mut child = exited();
        assert!(matches!(
            observe(
                &store,
                &id,
                Instant::now(),
                Duration::from_secs(5),
                &flag,
                Some(&mut child),
                |_| {}
            )
            .unwrap(),
            Waited::Terminal(Status {
                status: State::Lost,
                ..
            })
        ));
        // The child itself named as the worker keeps today's lost mapping.
        let mut child = exited();
        let id = job_named(child.id(), "child".into());
        let mut s = store.read_status(&id).unwrap();
        s.worker_start = None;
        store.write_status(&id, &s).unwrap();
        assert!(matches!(
            observe(
                &store,
                &id,
                Instant::now(),
                Duration::from_secs(5),
                &flag,
                Some(&mut child),
                |_| {}
            )
            .unwrap(),
            Waited::Terminal(Status {
                status: State::Lost,
                ..
            })
        ));
    }

    fn reply(id: &str) -> Reply {
        Reply {
            v: 1,
            id: id.into(),
            session_id: "session".into(),
            job_id: None,
            request_id: None,
            tool_call_id: None,
            text: format!("text {id}"),
            is_final: false,
            at: "2026-11-02T00:00:00Z".into(),
        }
    }
    fn line(id: &str) -> Vec<u8> {
        let mut b = serde_json::to_vec(&reply(id)).unwrap();
        b.push(b'\n');
        b
    }
    fn append(store: &JobStore, id: &JobId, bytes: &[u8]) {
        use std::io::Write;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(store.replies_path(id))
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
    fn ids(replies: &[Reply]) -> Vec<&str> {
        replies.iter().map(|r| r.id.as_str()).collect()
    }
    /// An accepted job whose worker is this test process, so `liveness::refresh` keeps it alive.
    fn running_job(store: &JobStore) -> JobId {
        let j = tests::job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        let mut s = store.read_status(&id).unwrap();
        let me = liveness::current().unwrap();
        s.status = State::Accepted;
        s.worker_pid = Some(me.pid);
        s.worker_start = Some(me.start);
        store.write_status(&id, &s).unwrap();
        id
    }
    fn watch_once(store: &JobStore, id: &JobId, watch: &mut ReplyWatch, bound: Duration) -> Waited {
        observe_with(
            store,
            id,
            Instant::now(),
            bound,
            &AtomicBool::new(false),
            None,
            Some(watch),
            |_| {},
        )
        .unwrap()
    }
    #[test]
    fn complete_line_reader_defers_torn_tails_and_skips_bad_lines() {
        let t = tempfile::tempdir().unwrap();
        let store = tests::store(t.path());
        let id = running_job(&store);
        assert!(store.read_replies_complete(&id).unwrap().is_empty());
        assert!(ReplyWatch::new(&store, &id, Some("r1")).unwrap().is_none());
        append(&store, &id, &line("r1"));
        // A torn JSON tail.
        append(&store, &id, br#"{"v":1,"id":"r2","text":"cut"#);
        assert_eq!(ids(&store.read_replies_complete(&id).unwrap()), ["r1"]);
        assert!(ReplyWatch::new(&store, &id, Some("r1")).unwrap().is_some());
        // Terminate it as the next append would, then tear inside a 3-byte character.
        append(&store, &id, b"\n");
        let mut torn = serde_json::to_vec(&Reply {
            text: "caf\u{20ac}".into(),
            ..reply("r3")
        })
        .unwrap();
        let euro = torn
            .windows(3)
            .position(|w| w == "\u{20ac}".as_bytes())
            .unwrap();
        torn.truncate(euro + 1);
        append(&store, &id, &torn);
        assert_eq!(ids(&store.read_replies_complete(&id).unwrap()), ["r1"]);
        assert!(ReplyWatch::new(&store, &id, Some("r1")).unwrap().is_some());
        // A complete malformed line is skipped; a complete object without its newline waits.
        append(&store, &id, b"\nnot json\n");
        append(&store, &id, &serde_json::to_vec(&reply("r4")).unwrap());
        assert_eq!(ids(&store.read_replies_complete(&id).unwrap()), ["r1"]);
        assert!(ReplyWatch::new(&store, &id, Some("r4")).unwrap().is_none());
        let mut watch = ReplyWatch::new(&store, &id, Some("r1")).unwrap().unwrap();
        assert!(watch.poll(&store, &id).unwrap().is_empty());
        append(&store, &id, b"\n");
        assert_eq!(ids(&watch.poll(&store, &id).unwrap()), ["r4"]);
        assert_eq!(
            ids(&store.read_replies_complete(&id).unwrap()),
            ["r1", "r4"]
        );
        assert!(ReplyWatch::new(&store, &id, Some("r4")).unwrap().is_some());
    }
    #[test]
    fn after_skips_earlier_replies_and_unknown_ids_are_none() {
        let t = tempfile::tempdir().unwrap();
        let store = tests::store(t.path());
        let id = running_job(&store);
        for r in ["r1", "r2", "r3"] {
            append(&store, &id, &line(r));
        }
        assert!(
            ReplyWatch::new(&store, &id, Some("nope"))
                .unwrap()
                .is_none()
        );
        let mut all = ReplyWatch::new(&store, &id, None).unwrap().unwrap();
        assert!(all.seen().is_empty());
        assert_eq!(ids(&all.poll(&store, &id).unwrap()), ["r1", "r2", "r3"]);
        let mut after = ReplyWatch::new(&store, &id, Some("r2")).unwrap().unwrap();
        assert_eq!(after.seen().len(), 2);
        assert_eq!(ids(&after.poll(&store, &id).unwrap()), ["r3"]);
        let mut last = ReplyWatch::new(&store, &id, Some("r3")).unwrap().unwrap();
        assert!(last.poll(&store, &id).unwrap().is_empty());
    }
    #[test]
    fn unchanged_file_is_not_reread() {
        let t = tempfile::tempdir().unwrap();
        let store = tests::store(t.path());
        let id = running_job(&store);
        let mut watch = ReplyWatch::new(&store, &id, None).unwrap().unwrap();
        assert!(watch.poll(&store, &id).unwrap().is_empty());
        assert_eq!(watch.last, Some((0, 0)), "a missing file keys as (0, 0)");
        append(&store, &id, &line("r1"));
        assert_eq!(ids(&watch.poll(&store, &id).unwrap()), ["r1"]);
        let key = watch.last.unwrap();
        assert_eq!(key.0, fs::metadata(store.replies_path(&id)).unwrap().len());
        // r1 is still unseen, so only skipping the read keeps this poll empty.
        assert!(watch.poll(&store, &id).unwrap().is_empty());
        assert_eq!(watch.last, Some(key));
    }
    #[test]
    fn appended_reply_wakes_the_observer_promptly() {
        let t = tempfile::tempdir().unwrap();
        let store = tests::store(t.path());
        let id = running_job(&store);
        let mut watch = ReplyWatch::new(&store, &id, None).unwrap().unwrap();
        let (writer_store, writer_id) = (store.clone(), id.clone());
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            append(&writer_store, &writer_id, &line("r1"));
            Instant::now()
        });
        let waited = watch_once(&store, &id, &mut watch, Duration::from_secs(10));
        let woke = Instant::now();
        let appended = writer.join().unwrap();
        let Waited::Replied(s, replies) = waited else {
            panic!("expected a reply wake")
        };
        assert_eq!(s.status, State::Accepted);
        assert_eq!(ids(&replies), ["r1"]);
        assert!(
            woke.duration_since(appended) < Duration::from_millis(500),
            "woke {:?} after the append",
            woke.duration_since(appended)
        );
    }
    #[test]
    fn bound_still_returns_running_with_a_watch() {
        let t = tempfile::tempdir().unwrap();
        let store = tests::store(t.path());
        let id = running_job(&store);
        append(&store, &id, &line("r1"));
        let mut watch = ReplyWatch::new(&store, &id, Some("r1")).unwrap().unwrap();
        let started = Instant::now();
        let waited = watch_once(&store, &id, &mut watch, Duration::from_millis(300));
        assert!(matches!(waited, Waited::Running(_)));
        assert!(started.elapsed() >= Duration::from_millis(300));
    }
    #[test]
    fn terminal_status_wins_over_a_simultaneous_reply() {
        let t = tempfile::tempdir().unwrap();
        let store = tests::store(t.path());
        let id = running_job(&store);
        append(&store, &id, &line("r1"));
        let mut s = store.read_status(&id).unwrap();
        s.status = State::Completed;
        store.write_status(&id, &s).unwrap();
        let mut watch = ReplyWatch::new(&store, &id, None).unwrap().unwrap();
        let waited = watch_once(&store, &id, &mut watch, Duration::from_secs(5));
        assert!(matches!(
            waited,
            Waited::Terminal(Status {
                status: State::Completed,
                ..
            })
        ));
        // The recheck after a non-empty poll: the job ended between the status read and the poll.
        assert!(matches!(
            wake_or_terminal(&store, &id, vec![reply("r1")]).unwrap(),
            Waited::Terminal(Status {
                status: State::Completed,
                ..
            })
        ));
        s.status = State::Accepted;
        store.write_status(&id, &s).unwrap();
        assert!(matches!(
            wake_or_terminal(&store, &id, vec![reply("r1")]).unwrap(),
            Waited::Replied(_, r) if r.len() == 1
        ));
    }
}
