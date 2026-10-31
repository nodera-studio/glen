//! I/O stays on these workers; the UI receives owned messages only.
pub mod follow;
pub mod inbox;
pub mod jobs;
pub mod sessions;
use crate::{cli::Cli, env::Env, error::GlenError, glen_home::GlenHome, tui::state::Msg};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError, Sender},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
#[derive(Clone)]
pub struct Ctx {
    pub cli: Arc<Cli>,
    pub env: Env,
    pub home: GlenHome,
    pub stop: Arc<AtomicBool>,
    pub tx: Sender<Msg>,
}
pub(super) enum Wake<T> {
    Command(T),
    Tick,
    Stop,
}
impl Ctx {
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
    pub fn send(&self, msg: Msg) -> bool {
        !self.stopped() && self.tx.send(msg).is_ok()
    }
    pub fn error(&self, source: &'static str, error: GlenError) -> bool {
        self.send(Msg::SourceError {
            source,
            error: error.into(),
        })
    }
    /// Command waits double as interruptible sleeps, capped at 100 ms per slice.
    pub(super) fn wait<T>(&self, rx: &Receiver<T>, deadline: Instant) -> Wake<T> {
        loop {
            if self.stopped() {
                return Wake::Stop;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Wake::Tick;
            }
            match rx.recv_timeout(remaining.min(Duration::from_millis(100))) {
                Ok(command) => return Wake::Command(command),
                Err(RecvTimeoutError::Disconnected) => return Wake::Stop,
                Err(RecvTimeoutError::Timeout) => (),
            }
        }
    }
}
pub(super) fn latest<T>(rx: &Receiver<T>, mut command: T) -> T {
    while let Ok(next) = rx.try_recv() {
        command = next;
    }
    command
}
/// Retain ownership explicitly: joining readers never implies terminating submitted jobs.
pub type Worker = JoinHandle<()>;

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::{
        omp::{fixtures, listing},
        tui::{state::*, update},
    };
    use clap::Parser;
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::mpsc,
    };
    pub struct Rig {
        pub temp: tempfile::TempDir,
        pub agent: PathBuf,
        pub env: Env,
        pub home: GlenHome,
    }
    impl Rig {
        pub fn new(copy: bool) -> Self {
            let temp = tempfile::Builder::new()
                .prefix("gsrc-")
                .tempdir_in("/tmp")
                .unwrap();
            let agent = temp.path().join("agent");
            let home = GlenHome {
                root: temp.path().join("glen"),
            };
            if copy {
                let tree = fixtures::tree();
                copy_tree(tree.path(), &agent.join("sessions"));
            }
            let cwd = temp.path().join("work/alpha");
            fs::create_dir_all(&cwd).unwrap();
            let fixture_tmp = temp.path().join("scratch");
            fs::create_dir_all(&fixture_tmp).unwrap();
            let env = Env::for_test(
                Some(temp.path()),
                &cwd,
                &[
                    ("TMPDIR", fixture_tmp.to_str().unwrap()),
                    ("PI_CODING_AGENT_DIR", agent.to_str().unwrap()),
                    ("GLEN_HOME", home.root.to_str().unwrap()),
                ],
            );
            Self {
                temp,
                agent,
                env,
                home,
            }
        }
        pub fn ctx(&self) -> (Ctx, mpsc::Receiver<Msg>) {
            let (tx, rx) = mpsc::channel();
            let cli = Cli::try_parse_from(["glen", "watch"]).unwrap();
            (
                Ctx {
                    cli: Arc::new(cli),
                    env: self.env.clone(),
                    home: self.home.clone(),
                    stop: Arc::new(AtomicBool::new(false)),
                    tx,
                },
                rx,
            )
        }
        pub fn path(&self, key: char) -> PathBuf {
            let id =
                crate::omp::session::Session::parse(Path::new("fixture"), fixtures::bytes(key))
                    .unwrap()
                    .header
                    .id;
            listing::discover(&self.agent.join("sessions"))
                .unwrap()
                .into_iter()
                .find(|file| listing::scan_head(file.clone()).unwrap().id == id)
                .unwrap()
                .path
        }
    }
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &to.join(entry.file_name()));
            } else {
                fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
            }
        }
    }
    pub fn wait(rx: &mpsc::Receiver<Msg>, predicate: impl Fn(&Msg) -> bool) -> Msg {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            assert!(Instant::now() < deadline, "worker message timeout");
            let msg = match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(msg) => msg,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(error) => panic!("worker disconnected: {error}"),
            };
            if let Msg::SourceError { source, error } = &msg {
                panic!("{source}: {}", error.message);
            }
            if predicate(&msg) {
                return msg;
            }
        }
    }
    pub fn stop(ctx: &Ctx, worker: Worker) {
        ctx.stop.store(true, Ordering::Release);
        assert!(worker.join().is_ok());
    }
    #[test]
    fn absent_home_stays_absent_across_all_reader_cycles_and_shutdown_is_prompt() {
        let rig = Rig::new(false);
        let (ctx, rx) = rig.ctx();
        let (s_tx, s_rx) = mpsc::channel();
        let (j_tx, j_rx) = mpsc::channel();
        let (i_tx, i_rx) = mpsc::channel();
        let readers = [
            sessions::spawn(ctx.clone(), s_rx),
            jobs::spawn(ctx.clone(), j_rx),
            inbox::spawn(ctx.clone(), i_rx),
        ];
        let mut seen = [false; 3];
        while !seen.iter().all(|seen| *seen) {
            match rx.recv_timeout(Duration::from_secs(3)).unwrap() {
                Msg::Sessions(s) => {
                    assert!(s.rows.is_empty());
                    assert!(s.sessions_dir_missing.is_some());
                    seen[0] = true;
                }
                Msg::Jobs(s) => {
                    assert!(s.rows.is_empty());
                    seen[1] = true;
                }
                Msg::Inbox(s) => {
                    assert!(s.records.is_empty());
                    seen[2] = true;
                }
                Msg::SourceError { error, .. } => panic!("{}", error.message),
                _ => (),
            }
        }
        assert!(!rig.home.root.exists());
        let start = Instant::now();
        ctx.stop.store(true, Ordering::Release);
        for reader in readers {
            assert!(reader.join().is_ok());
        }
        assert!(start.elapsed() < Duration::from_millis(500));
        assert!(!rig.home.root.exists());
        drop((s_tx, j_tx, i_tx));
    }
    #[test]
    fn source_errors_and_inbox_epoch_resets_are_reducer_messages() {
        use crate::{
            error::{ErrorCode, GlenError},
            render::palette::{ColorDepth, Theme},
            tui::theme::TuiTheme,
        };
        let mut app = App::new(
            TuiTheme {
                depth: ColorDepth::None,
                theme: Theme::Dark,
            },
            Scope::All,
            (100, 30),
            Instant::now(),
            std::time::SystemTime::UNIX_EPOCH,
        );
        update::update(
            &mut app,
            Msg::SourceError {
                source: "sessions",
                error: GlenError::new(ErrorCode::Io, "cannot read catalog")
                    .with_hint("check permissions")
                    .into(),
            },
        );
        assert!(
            app.toast
                .as_ref()
                .unwrap()
                .text
                .contains("check permissions")
        );
        app.inbox.records.push(InboxMsg {
            id: "one".into(),
            session_id: "old".into(),
            job_id: None,
            text: "body".into(),
            is_final: false,
            at: String::new(),
            unread: true,
        });
        app.inbox.unread_by_session.insert("old".into(), 1);
        update::update(&mut app, Msg::InboxReset(vec!["old".into()]));
        assert!(app.inbox.records.is_empty());
        assert!(app.inbox.unread_by_session.is_empty());
    }
    #[test]
    fn tui_and_shared_reader_paths_have_no_direct_output_macros() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut pending = vec![base.join("src/tui")];
        let mut files = vec![
            base.join("src/commands/follow.rs"),
            base.join("src/inbox.rs"),
        ];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    pending.push(entry.path());
                } else if entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "rs")
                {
                    files.push(entry.path());
                }
            }
        }
        let forbidden = [
            concat!("e", "println", "!"),
            concat!("print", "ln", "!"),
            concat!("print", "!("),
        ];
        for file in files {
            let source = fs::read_to_string(&file).unwrap();
            for pattern in forbidden {
                assert!(
                    !source.contains(pattern),
                    "direct output in {}",
                    file.display()
                );
            }
        }
        let source = fs::read_to_string(base.join("src/tui/sources/inbox.rs")).unwrap();
        assert!(!source.contains(&["inbox", "::", "mark", "("].concat()));
    }
    #[test]
    fn unreadable_roots_are_channel_errors_and_follow_waits_for_a_new_command() {
        let rig = Rig::new(false);
        fs::create_dir_all(&rig.home.root).unwrap();
        fs::write(rig.home.root.join("jobs"), "not a directory").unwrap();
        fs::write(rig.home.root.join("inbox"), "not a directory").unwrap();
        fs::create_dir_all(&rig.agent).unwrap();
        fs::write(rig.agent.join("sessions"), "not a directory").unwrap();
        let (ctx, rx) = rig.ctx();
        let (s_tx, s_rx) = mpsc::channel();
        let (j_tx, j_rx) = mpsc::channel();
        let (i_tx, i_rx) = mpsc::channel();
        let (f_tx, f_rx) = mpsc::channel();
        let readers = [
            sessions::spawn(ctx.clone(), s_rx),
            jobs::spawn(ctx.clone(), j_rx),
            inbox::spawn(ctx.clone(), i_rx),
            follow::spawn(ctx.clone(), f_rx),
        ];
        f_tx.send(follow::Command {
            r#gen: 1,
            target: Target {
                id: "absent".into(),
                path: Some(rig.temp.path().join("absent.jsonl")),
            },
            opts: crate::omp::items::ItemOpts {
                thinking: false,
                tools: crate::cli::Tools::Collapsed,
            },
        })
        .unwrap();
        let mut errors = std::collections::HashSet::new();
        let mut failed = false;
        let deadline = Instant::now() + Duration::from_secs(3);
        while errors.len() < 3 || !failed {
            assert!(
                Instant::now() < deadline,
                "worker failed to report I/O error"
            );
            match rx.recv_timeout(Duration::from_millis(500)).unwrap() {
                Msg::SourceError { source, error } => {
                    assert!(!error.message.is_empty());
                    errors.insert(source);
                }
                Msg::Follow {
                    ev: FollowMsg::Failed(error),
                    ..
                } => {
                    assert!(!failed);
                    failed = true;
                    assert!(!error.message.is_empty());
                }
                _ => (),
            }
        }
        ctx.stop.store(true, Ordering::Release);
        for reader in readers {
            assert!(reader.join().is_ok());
        }
        drop((s_tx, j_tx, i_tx, f_tx));
    }
}
