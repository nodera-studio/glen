use super::{JobId, JobStore, Status, liveness};
use crate::error::GlenError;
use std::{
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
pub fn observe(
    store: &JobStore,
    id: &JobId,
    started: Instant,
    bound: Duration,
    interrupt: &AtomicBool,
    mut child: Option<&mut Child>,
    mut on_change: impl FnMut(&Status),
) -> Result<Waited, GlenError> {
    let mut previous = None;
    loop {
        let mut s = liveness::refresh(store, id, store.read_status(id)?);
        if s.status.is_terminal() {
            return Ok(Waited::Terminal(s));
        }
        if let Some(c) = &mut child
            && c.try_wait()?.is_some()
        {
            std::thread::sleep(Duration::from_millis(100));
            s = store.read_status(id)?;
            if !s.status.is_terminal() {
                s = liveness::mark_lost(store, id, s);
            }
            return Ok(Waited::Terminal(s));
        }
        if interrupt.load(Ordering::SeqCst) {
            return Ok(Waited::Interrupted(s));
        }
        if started.elapsed() >= bound {
            return Ok(Waited::Running(s));
        }
        if previous != Some((s.status, s.waiting_for_lock, s.omp_pid)) {
            on_change(&s);
            previous = Some((s.status, s.waiting_for_lock, s.omp_pid));
        }
        std::thread::sleep(if started.elapsed() < Duration::from_secs(2) {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(250)
        });
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
}
