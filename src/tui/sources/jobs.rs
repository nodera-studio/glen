use super::{Ctx, Wake, Worker};
use crate::{
    commands::wait,
    error::{ErrorCode, GlenError, Warning},
    jobs::{Job, JobId, JobResult, JobStore, State, Status, liveness},
    tui::state::{Done, ErrView, JobRow, JobsSnapshot, Msg},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    process::Child,
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};
const POLL: Duration = Duration::from_millis(250);
const LIST: Duration = Duration::from_secs(1);
pub enum Command {
    Track { id: JobId, child: Child },
    Refresh,
}
struct Tracked {
    child: Child,
    exited: Option<Instant>,
}
struct Entry {
    job: Job,
    status: Status,
    result: Option<JobResult>,
    result_loaded: bool,
    finished: Option<Instant>,
}
struct Reader {
    store: JobStore,
    entries: BTreeMap<JobId, Entry>,
    children: BTreeMap<JobId, Tracked>,
    ours: BTreeSet<JobId>,
}
impl Reader {
    fn new(ctx: &Ctx) -> Self {
        Self {
            store: JobStore::new(ctx.home.clone()),
            entries: BTreeMap::new(),
            children: BTreeMap::new(),
            ours: BTreeSet::new(),
        }
    }
    fn track(&mut self, id: JobId, child: Child) {
        self.ours.insert(id.clone());
        self.children.insert(
            id,
            Tracked {
                child,
                exited: None,
            },
        );
    }
    fn load(&mut self, id: &JobId) -> Result<(), GlenError> {
        let job = self.store.read_job(id)?;
        if job.id != id.as_str() {
            return Err(GlenError::new(
                ErrorCode::Internal,
                format!(
                    "job id in {} does not match {}",
                    self.store.dir(id).join("job.json").display(),
                    id.as_str()
                ),
            ));
        }
        let status = self.store.read_status(id)?;
        self.entries.insert(
            id.clone(),
            Entry {
                job,
                status,
                result: None,
                result_loaded: false,
                finished: None,
            },
        );
        Ok(())
    }
    fn list(&mut self) -> Vec<GlenError> {
        let mut errors = vec![];
        let root = self.store.home.root.join("jobs");
        match fs::read_dir(&root) {
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.entries.retain(|id, _| self.children.contains_key(id));
                return errors;
            }
            Err(e) => {
                errors.push(GlenError::read(&root, e));
                return errors;
            }
        }
        let ids = self.store.list().into_iter().collect::<BTreeSet<_>>();
        self.entries
            .retain(|id, _| ids.contains(id) || self.children.contains_key(id));
        for id in ids {
            if self
                .entries
                .get(&id)
                .is_some_and(|entry| entry.status.status.is_terminal())
            {
                continue;
            }
            if let Some(entry) = self.entries.get_mut(&id) {
                // job.json is immutable after creation; discovered active jobs enter the fast status loop.
                match self.store.read_job(&id) {
                    Ok(job) if job.id == id.as_str() => entry.job = job,
                    Ok(_) => errors.push(GlenError::new(
                        ErrorCode::Internal,
                        format!("invalid stored job id at {}", self.store.dir(&id).display()),
                    )),
                    Err(e) => errors.push(e),
                }
            } else if let Err(e) = self.load(&id) {
                errors.push(e);
            }
        }
        errors
    }
    fn poll(&mut self, now: Instant) -> Vec<GlenError> {
        let mut errors = vec![];
        let missing = self
            .children
            .keys()
            .filter(|id| !self.entries.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        for id in missing {
            if let Err(e) = self.load(&id) {
                errors.push(e);
            }
        }
        let mut child_errors = vec![];
        for (id, tracked) in &mut self.children {
            match tracked.child.try_wait() {
                Ok(Some(_)) => {
                    tracked.exited.get_or_insert(now);
                }
                Ok(None) => (),
                Err(e) => {
                    let error = GlenError::new(
                        ErrorCode::Io,
                        format!("cannot observe worker for job {}: {e}", id.handle()),
                    );
                    child_errors.push((id.clone(), error));
                }
            }
        }
        for (id, entry) in &mut self.entries {
            if !entry.status.status.is_terminal() {
                let status = match self.store.read_status(id) {
                    Ok(status) => status,
                    Err(e) => {
                        warn(&mut entry.status, "status_read_failed", e);
                        continue;
                    }
                };
                entry.status = match liveness::refresh_quiet(&self.store, id, status.clone()) {
                    Ok(status) => status,
                    Err(e) => {
                        let mut status = status;
                        warn(&mut status, "lost_write_failed", e);
                        status
                    }
                };
                if !entry.status.status.is_terminal()
                    && self
                        .children
                        .get(id)
                        .and_then(|child| child.exited)
                        .is_some_and(|at| {
                            now.saturating_duration_since(at) >= Duration::from_millis(100)
                        })
                {
                    match liveness::try_mark_lost(&self.store, id, false) {
                        Ok(status) => entry.status = status,
                        Err(e) => warn(&mut entry.status, "lost_write_failed", e),
                    }
                }
            }
            if entry.status.status.is_terminal() {
                entry.finished.get_or_insert(now);
                if !entry.result_loaded {
                    match self.store.read_result(id) {
                        Ok(mut result) => {
                            let mut replies = self
                                .store
                                .read_replies(id)
                                .into_iter()
                                .map(|r| (r.id.clone(), r))
                                .collect::<BTreeMap<_, _>>();
                            for reply in result.replies {
                                replies.insert(reply.id.clone(), reply);
                            }
                            result.replies = replies.into_values().collect();
                            result.replies.sort_by(|a, b| {
                                a.at.parse::<jiff::Timestamp>()
                                    .ok()
                                    .cmp(&b.at.parse::<jiff::Timestamp>().ok())
                                    .then(a.id.cmp(&b.id))
                            });
                            entry.result = Some(result);
                            entry.result_loaded = true;
                        }
                        Err(e)
                            if e.code == ErrorCode::NotFound
                                && entry.status.status != State::Completed =>
                        {
                            entry.result_loaded = true
                        }
                        Err(e) => warn(&mut entry.status, "result_read_failed", e),
                    }
                }
            }
        }
        // Apply observation errors after reading status, so a fresh status cannot erase them.
        for (id, error) in child_errors {
            if let Some(entry) = self.entries.get_mut(&id) {
                warn(&mut entry.status, "child_observation_failed", error);
            } else {
                errors.push(error);
            }
        }
        // Already-reaped children can be dropped. Still-running workers are never signalled.
        self.children.retain(|id, child| {
            child.exited.is_none()
                || self
                    .entries
                    .get(id)
                    .is_none_or(|entry| !entry.status.status.is_terminal())
        });
        errors
    }
    fn snapshot(&self) -> JobsSnapshot {
        let now = jiff::Timestamp::now();
        let mut rows = vec![];
        for (id, entry) in self.entries.iter().rev() {
            let status = &entry.status;
            let done = if !status.status.is_terminal() {
                None
            } else if status.status == State::Completed {
                Some(if let Some(forked) = &status.forked {
                    Done::Forked(format!(
                        "✓ forked → {}",
                        crate::rpc::session_handle(&forked.to)
                    ))
                } else {
                    Done::Completed(wait::completed_summary(&entry.job, status, now))
                })
            } else {
                Some(Done::Failed(
                    match wait::job_error(&self.store, &entry.job, status) {
                        Ok(error) | Err(error) => ErrView::from(error),
                    },
                ))
            };
            rows.push(JobRow {
                id: id.as_str().into(),
                handle: entry.job.handle.clone(),
                session_id: entry.job.session_id.clone(),
                actual_session_id: status.actual_session_id.clone(),
                route: entry.job.route,
                status: status.clone(),
                created_at: entry.job.created_at.clone(),
                result: entry.result.clone(),
                ours: self.ours.contains(id),
                finished_at: entry.finished,
                progress: wait::job_progress(&entry.job, status),
                done,
            });
        }
        let active_total = rows
            .iter()
            .filter(|row| !row.status.status.is_terminal())
            .count();
        JobsSnapshot { rows, active_total }
    }
}
fn warn(status: &mut Status, code: &str, error: GlenError) {
    let message = format!(
        "{}{}",
        error.message,
        error
            .hint
            .map(|hint| format!(" · {hint}"))
            .unwrap_or_default()
    );
    if !status
        .warnings
        .iter()
        .any(|warning| warning.code == code && warning.message == message)
    {
        status.warnings.push(Warning {
            code: code.into(),
            message,
        });
    }
}
pub fn spawn(ctx: Ctx, rx_cmd: Receiver<Command>) -> Worker {
    std::thread::spawn(move || {
        let mut reader = Reader::new(&ctx);
        let mut next_list = Instant::now();
        let mut next_poll = Instant::now();
        while !ctx.stopped() {
            while let Ok(command) = rx_cmd.try_recv() {
                if let Command::Track { id, child } = command {
                    reader.track(id, child);
                }
                next_list = Instant::now();
                next_poll = Instant::now();
            }
            if Instant::now() >= next_list {
                for error in reader.list() {
                    if !ctx.error("jobs", error) {
                        return;
                    }
                }
                next_list = Instant::now() + LIST;
            }
            if Instant::now() >= next_poll {
                for error in reader.poll(Instant::now()) {
                    if !ctx.error("jobs", error) {
                        return;
                    }
                }
                if !ctx.send(Msg::Jobs(reader.snapshot())) {
                    return;
                }
                next_poll = Instant::now() + POLL;
            }
            match ctx.wait(&rx_cmd, next_list.min(next_poll)) {
                Wake::Command(command) => {
                    if let Command::Track { id, child } = command {
                        reader.track(id, child);
                    }
                    next_list = Instant::now();
                    next_poll = Instant::now();
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
        jobs::{Reply, tests::job},
        tui::sources::tests::{Rig, stop, wait},
    };
    use std::{
        process::{Command as Process, Stdio},
        sync::mpsc,
    };
    fn setup(rig: &Rig) -> (Reader, JobId) {
        let (ctx, _) = rig.ctx();
        let reader = Reader::new(&ctx);
        let job = job();
        let id = JobId::parse(&job.id).unwrap();
        reader.store.create(&job).unwrap();
        (reader, id)
    }
    fn result() -> JobResult {
        JobResult {
            v: 1,
            prompt_status: Some("completed".into()),
            agent_invoked: true,
            assistant_text: Some("done".into()),
            replies: vec![reply("same")],
            command_output: vec![],
            stop_reason: Some("stop".into()),
            session_settled: true,
            settled_at: Some(jiff::Timestamp::now().to_string()),
        }
    }
    fn reply(id: &str) -> Reply {
        Reply {
            v: 1,
            id: id.into(),
            session_id: "session-a".into(),
            job_id: None,
            request_id: None,
            tool_call_id: None,
            text: id.into(),
            is_final: true,
            at: "2026-10-30T00:00:00Z".into(),
        }
    }
    fn status(store: &JobStore, id: &JobId, state: State) {
        let mut status = store.read_status(id).unwrap();
        status.status = state;
        status.updated_at = jiff::Timestamp::now().to_string();
        store.write_status(id, &status).unwrap();
    }
    fn exited_child() -> Child {
        let mut child = Process::new("/bin/sh")
            .args(["-c", "exit 0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(child.wait().unwrap().success());
        child
    }
    #[test]
    fn terminal_results_merge_replies_and_cache_job_and_status() {
        let rig = Rig::new(false);
        let (mut reader, id) = setup(&rig);
        assert!(reader.list().is_empty());
        assert!(reader.poll(Instant::now()).is_empty());
        assert_eq!(reader.snapshot().active_total, 1);
        assert_eq!(reader.snapshot().rows[0].progress, "starting omp");
        status(&reader.store, &id, State::Accepted);
        assert!(reader.poll(Instant::now()).is_empty());
        assert_eq!(reader.snapshot().rows[0].status.status, State::Accepted);
        reader.store.append_reply(&id, &reply("same")).unwrap();
        reader.store.append_reply(&id, &reply("log-only")).unwrap();
        reader.store.write_result(&id, &result()).unwrap();
        status(&reader.store, &id, State::Completed);
        assert!(reader.poll(Instant::now()).is_empty());
        let snapshot = reader.snapshot();
        assert_eq!(snapshot.active_total, 0);
        assert_eq!(snapshot.rows[0].result.as_ref().unwrap().replies.len(), 2);
        assert!(snapshot.rows[0].finished_at.is_some());
        assert!(
            matches!(&snapshot.rows[0].done, Some(Done::Completed(s)) if s.starts_with("✓ completed"))
        );
        fs::write(reader.store.dir(&id).join("job.json"), "broken").unwrap();
        fs::write(reader.store.dir(&id).join("status.json"), "broken").unwrap();
        assert!(reader.list().is_empty());
        assert!(reader.poll(Instant::now()).is_empty());
        assert_eq!(reader.snapshot().rows[0].status.status, State::Completed);
    }
    #[test]
    fn exited_child_gets_existing_lost_rule_after_the_result_grace() {
        let rig = Rig::new(false);
        let (mut reader, id) = setup(&rig);
        reader.track(id.clone(), exited_child());
        let now = Instant::now();
        assert!(reader.poll(now).is_empty());
        assert_eq!(reader.snapshot().rows[0].status.status, State::Queued);
        assert!(reader.poll(now + POLL).is_empty());
        let row = reader.snapshot().rows.remove(0);
        assert_eq!(row.status.status, State::Lost);
        assert!(row.ours);
        assert!(reader.children.is_empty());
        assert!(
            matches!(row.done, Some(Done::Failed(e)) if e.code == "job_failed" && e.message.contains("exited without reporting"))
        );
        assert_eq!(reader.store.read_status(&id).unwrap().status, State::Lost);
    }
    #[test]
    fn lost_write_failure_is_row_warning_not_output_or_a_lost_snapshot() {
        let rig = Rig::new(false);
        let (mut reader, id) = setup(&rig);
        let mut status = reader.store.read_status(&id).unwrap();
        status.updated_at = "2000-01-01T00:00:00Z".into();
        reader.store.write_status(&id, &status).unwrap();
        let lock = reader.store.dir(&id).join("status.lock");
        if lock.exists() {
            fs::remove_file(&lock).unwrap();
        }
        fs::create_dir(&lock).unwrap();
        assert!(reader.list().is_empty());
        assert!(reader.poll(Instant::now()).is_empty());
        let row = reader.snapshot().rows.remove(0);
        assert_eq!(row.status.status, State::Queued);
        assert!(
            row.status
                .warnings
                .iter()
                .any(|w| w.code == "lost_write_failed" && w.message.contains("status.lock"))
        );
        assert_eq!(reader.store.read_status(&id).unwrap().status, State::Queued);
    }
    #[test]
    fn foreign_jobs_are_discovered_then_polled_at_the_fast_interval() {
        let rig = Rig::new(false);
        let (ctx, rx) = rig.ctx();
        let (tx, commands) = mpsc::channel();
        let worker = spawn(ctx.clone(), commands);
        wait(&rx, |m| matches!(m, Msg::Jobs(s) if s.rows.is_empty()));
        let store = JobStore::new(rig.home.clone());
        let job = job();
        let id = JobId::parse(&job.id).unwrap();
        store.create(&job).unwrap();
        let created = Instant::now();
        let discovered = wait(
            &rx,
            |m| matches!(m, Msg::Jobs(s) if s.rows.iter().any(|r| r.id == id.as_str())),
        );
        assert!(created.elapsed() < Duration::from_millis(1500));
        assert!(matches!(discovered, Msg::Jobs(s) if !s.rows[0].ours && s.active_total == 1));
        store.write_result(&id, &result()).unwrap();
        status(&store, &id, State::Completed);
        let completed = Instant::now();
        let snapshot = wait(
            &rx,
            |m| matches!(m, Msg::Jobs(s) if s.rows.iter().any(|r| r.status.status == State::Completed)),
        );
        assert!(completed.elapsed() < Duration::from_millis(500));
        assert!(matches!(snapshot, Msg::Jobs(s) if s.rows[0].result.is_some()));
        stop(&ctx, worker);
        drop(tx);
    }
    #[test]
    fn stopping_reader_drops_but_never_kills_a_running_child() {
        let rig = Rig::new(false);
        let (ctx, rx) = rig.ctx();
        let (reader, id) = setup(&rig);
        drop(reader);
        let (tx, commands) = mpsc::channel();
        let child = Process::new("/bin/sleep")
            .arg("1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        tx.send(Command::Track { id, child }).unwrap();
        let worker = spawn(ctx.clone(), commands);
        wait(
            &rx,
            |m| matches!(m, Msg::Jobs(s) if s.rows.iter().any(|r| r.ours)),
        );
        stop(&ctx, worker);
        assert!(matches!(
            liveness::start_token(pid),
            liveness::Probe::Running(_)
        ));
        let pid = rustix::process::Pid::from_raw(pid as i32).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(Instant::now() < deadline, "child failed to exit naturally");
            if let Some((_, status)) =
                rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::NOHANG).unwrap()
            {
                assert_eq!(status.exit_status(), Some(0));
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[test]
    fn child_observation_error_survives_the_status_refresh() {
        let rig = Rig::new(false);
        let (mut reader, id) = setup(&rig);
        let child = Process::new("/bin/sh")
            .args(["-c", "exit 0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // Simulate another observer reaping the worker without updating this Child handle.
        let pid = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(Instant::now() < deadline);
            if rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::NOHANG)
                .unwrap()
                .is_some()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        reader.track(id.clone(), child);
        assert!(reader.poll(Instant::now()).is_empty());
        assert!(
            reader.snapshot().rows[0]
                .status
                .warnings
                .iter()
                .any(|w| w.code == "child_observation_failed" && w.message.contains(&id.handle()))
        );
        assert_eq!(reader.store.read_status(&id).unwrap().status, State::Queued);
    }
}
