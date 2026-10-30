//! Terminal ownership and effect execution. Readers only communicate through owned messages.
use super::{
    controls,
    sources::{self, Ctx},
    state::{App, Cmd, ErrView, JobRef, Msg, Scope, Target},
    update, view,
};
use crate::{
    cli::{Cli, DeliverAs},
    commands::{
        open::{self, Opened},
        send,
    },
    env::Env,
    error::{ErrorCode, GlenError},
    glen_home::GlenHome,
    omp::items::ItemOpts,
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    crossterm::{
        cursor::{Hide, Show},
        event::{self, DisableBracketedPaste, EnableBracketedPaste, Event},
        execute,
        terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
    },
};
use std::{
    io, panic,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};

static ACTIVE: AtomicBool = AtomicBool::new(false);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalStep {
    Raw,
    Alternate,
    Paste,
    Hide,
    NoPaste,
    Leave,
    Show,
    NoRaw,
}
const ENTER: [TerminalStep; 4] = [
    TerminalStep::Raw,
    TerminalStep::Alternate,
    TerminalStep::Paste,
    TerminalStep::Hide,
];
const LEAVE: [TerminalStep; 4] = [
    TerminalStep::NoPaste,
    TerminalStep::Leave,
    TerminalStep::Show,
    TerminalStep::NoRaw,
];
fn terminal_step(step: TerminalStep) -> io::Result<()> {
    match step {
        TerminalStep::Raw => terminal::enable_raw_mode(),
        TerminalStep::Alternate => execute!(io::stdout(), EnterAlternateScreen),
        TerminalStep::Paste => execute!(io::stdout(), EnableBracketedPaste),
        TerminalStep::Hide => execute!(io::stdout(), Hide),
        TerminalStep::NoPaste => execute!(io::stdout(), DisableBracketedPaste),
        TerminalStep::Leave => execute!(io::stdout(), LeaveAlternateScreen),
        TerminalStep::Show => execute!(io::stdout(), Show),
        TerminalStep::NoRaw => terminal::disable_raw_mode(),
    }
}
fn restore_with(
    active: &AtomicBool,
    mut step: impl FnMut(TerminalStep) -> io::Result<()>,
) -> io::Result<()> {
    if !active.swap(false, Ordering::AcqRel) {
        return Ok(());
    }
    let mut first = None;
    for action in LEAVE {
        if let Err(error) = step(action) {
            first.get_or_insert(error);
        }
    }
    first.map_or(Ok(()), Err)
}
fn enter_with(
    active: &AtomicBool,
    mut step: impl FnMut(TerminalStep) -> io::Result<()>,
) -> io::Result<()> {
    // Set before raw mode: failures at any stage still run every teardown step.
    active
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "watch terminal is already active",
            )
        })?;
    for action in ENTER {
        if let Err(first) = step(action) {
            return match restore_with(active, &mut step) {
                Ok(()) | Err(_) => Err(first),
            };
        }
    }
    Ok(())
}
fn restore_terminal() -> io::Result<()> {
    restore_with(&ACTIVE, terminal_step)
}
struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enter_with(&ACTIVE, terminal_step)?;
        Ok(Self)
    }
    fn restore(&self) -> io::Result<()> {
        restore_terminal()
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Normal exits collect the error explicitly; Drop and panic paths must never panic.
        if let Err(_error) = self.restore() {}
    }
}
type Hook = dyn Fn(&panic::PanicHookInfo<'_>) + Send + Sync + 'static;
struct PanicHook(Arc<Hook>);
impl PanicHook {
    fn install() -> Self {
        let previous: Arc<Hook> = Arc::from(panic::take_hook());
        let hook = previous.clone();
        panic::set_hook(Box::new(move |info| {
            if let Err(_error) = restore_terminal() {}
            hook(info);
            std::process::exit(101);
        }));
        Self(previous)
    }
}
impl Drop for PanicHook {
    fn drop(&mut self) {
        let previous = self.0.clone();
        panic::set_hook(Box::new(move |info| previous(info)));
    }
}
fn io_error(action: &str, error: io::Error) -> GlenError {
    GlenError::new(
        ErrorCode::Io,
        format!("cannot {action} for glen watch: {error}"),
    )
}

#[cfg(unix)]
struct Signals(signal_hook::iterator::Handle);
#[cfg(unix)]
impl Signals {
    fn start(
        ctx: &Ctx,
        hup: Arc<AtomicBool>,
        quitting: Arc<AtomicBool>,
    ) -> Result<Self, GlenError> {
        use signal_hook::{
            consts::{SIGHUP, SIGINT, SIGTERM},
            iterator::Signals,
        };
        let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP])
            .map_err(|e| io_error("install signal reader", e))?;
        let handle = Self(signals.handle());
        let ctx = ctx.clone();
        let thread = thread::Builder::new()
            .name("watch-signals".into())
            .spawn(move || {
                for signal in signals.forever() {
                    if quitting.load(Ordering::Acquire) {
                        if let Err(_error) = restore_terminal() {}
                        std::process::exit(128 + signal);
                    }
                    if signal == SIGHUP {
                        hup.store(true, Ordering::Release);
                    }
                    if !ctx.send(Msg::Signal(signal)) {
                        break;
                    }
                }
            })
            .map_err(|e| io_error("start signal reader", e))?;
        drop(thread); // Closing the iterator wakes it; never wait for a reader at quit.
        Ok(handle)
    }
}
#[cfg(unix)]
impl Drop for Signals {
    fn drop(&mut self) {
        self.0.close();
    }
}
fn input_msg(event: Event) -> Option<Msg> {
    match event {
        Event::Key(key) => Some(Msg::Key(key)),
        Event::Paste(text) => Some(Msg::Paste(text)),
        Event::Resize(w, h) => Some(Msg::Resize(w, h)),
        _ => None,
    }
}
const INPUT: &str = "terminal input";
fn start_input(ctx: &Ctx) -> Result<(), GlenError> {
    let ctx = ctx.clone();
    let reader = thread::Builder::new()
        .name("watch-input".into())
        .spawn(move || {
            while !ctx.stopped() {
                let result = event::poll(Duration::from_millis(50)).and_then(|ready| {
                    if ready {
                        event::read().map(input_msg)
                    } else {
                        Ok(None)
                    }
                });
                match result {
                    Ok(Some(msg)) => {
                        if !ctx.send(msg) {
                            break;
                        }
                    }
                    Ok(None) => (),
                    Err(error) => {
                        // Wake the loop with an owned error; only it restores the terminal and reports.
                        ctx.error(INPUT, io_error("read terminal input", error));
                        break;
                    }
                }
            }
        })
        .map_err(|e| io_error("start input reader", e))?;
    drop(reader);
    Ok(())
}
struct Effects {
    ctx: Ctx,
    sessions: Sender<sources::sessions::Command>,
    follow: Sender<sources::follow::Command>,
    jobs: Sender<sources::jobs::Command>,
    inbox: Sender<sources::inbox::Command>,
    tasks: Vec<JoinHandle<()>>,
}
fn command_send<T>(tx: &Sender<T>, command: T, source: &str) -> Result<(), GlenError> {
    tx.send(command).map_err(|_| {
        GlenError::new(
            ErrorCode::Internal,
            format!("glen watch {source} worker disconnected"),
        )
    })
}
pub(crate) fn initial_bucket(
    cli: &Cli,
    app: &App,
    opened: Option<&Opened>,
) -> Option<(std::path::PathBuf, String)> {
    let crate::cli::Command::Watch(args) = &cli.command else {
        return None;
    };
    if args.cwd.is_some() {
        return None;
    }
    let Scope::Bucket { cwd, .. } = app.bucket_scope.as_ref()? else {
        return None;
    };
    Some((cwd.clone(), opened?.summary.file.bucket.clone()))
}
impl Effects {
    fn start(ctx: Ctx, app: &App, opened: Option<Opened>) -> Self {
        let (sessions, s_rx) = mpsc::channel();
        let (follow, f_rx) = mpsc::channel();
        let (jobs, j_rx) = mpsc::channel();
        let (inbox, i_rx) = mpsc::channel();
        let bucket_override = initial_bucket(&ctx.cli, app, opened.as_ref());
        let session_reader = if bucket_override.is_none()
            && matches!(&app.scope, Scope::Bucket {cwd,..} if cwd == ctx.env.cwd())
        {
            sources::sessions::spawn(ctx.clone(), s_rx)
        } else {
            sources::sessions::spawn_scoped(ctx.clone(), s_rx, app.scope.clone(), bucket_override)
        };
        drop(session_reader);
        let follower = if let (Some(opened), Some(target)) = (opened, app.transcript.target.clone())
        {
            sources::follow::spawn_seeded(
                ctx.clone(),
                f_rx,
                sources::follow::Command {
                    r#gen: app.transcript.r#gen,
                    target,
                    opts: ItemOpts {
                        thinking: app.transcript.opts.thinking,
                        tools: app.transcript.opts.tools,
                    },
                },
                opened,
            )
        } else {
            sources::follow::spawn(ctx.clone(), f_rx)
        };
        drop(follower);
        drop(sources::jobs::spawn(ctx.clone(), j_rx));
        drop(sources::inbox::spawn(ctx.clone(), i_rx));
        Self {
            ctx,
            sessions,
            follow,
            jobs,
            inbox,
            tasks: vec![],
        }
    }
    fn execute(&mut self, command: Cmd) -> Result<Option<i32>, GlenError> {
        match command {
            Cmd::Follow {
                r#gen,
                target,
                opts,
            } => command_send(
                &self.follow,
                sources::follow::Command {
                    r#gen,
                    target,
                    opts,
                },
                "follow",
            )?,
            Cmd::ListScope(scope) => command_send(
                &self.sessions,
                sources::sessions::Command::Scope(scope),
                "sessions",
            )?,
            Cmd::Refresh => {
                command_send(
                    &self.sessions,
                    sources::sessions::Command::Refresh,
                    "sessions",
                )?;
                command_send(&self.jobs, sources::jobs::Command::Refresh, "jobs")?;
                command_send(&self.inbox, sources::inbox::Command::Refresh, "inbox")?;
            }
            Cmd::Submit {
                local,
                target,
                text,
                mode,
            } => {
                let ctx = self.ctx.clone();
                let jobs = self.jobs.clone();
                let task = thread::Builder::new()
                    .name("watch-submit".into())
                    .spawn(move || {
                        let result = submit(&ctx, target, text, mode)
                            .map(|submitted| {
                                let job = JobRef {
                                    id: submitted.id.as_str().into(),
                                    handle: submitted.job.handle,
                                };
                                // Dropping a child after shutdown never kills the durable worker.
                                if let Err(_closed) = jobs.send(sources::jobs::Command::Track {
                                    id: submitted.id,
                                    child: submitted.child,
                                }) {
                                    ctx.error("jobs", GlenError::new(ErrorCode::Io,
                                        format!("job {} was submitted but its watch tracker disconnected",job.handle))
                                        .with_hint(format!("the job continues; run `glen wait {}`",job.handle)));
                                }
                                job
                            })
                            .map_err(ErrView::from);
                        ctx.send(Msg::Submitted { local, result });
                    })
                    .map_err(|e| io_error("start submit thread", e))?;
                self.tasks.push(task);
            }
            Cmd::Control { local, target, op } => {
                let ctx = self.ctx.clone();
                let task = thread::Builder::new()
                    .name("watch-control".into())
                    .spawn(move || {
                        let result = controls::run(&ctx.cli, &ctx.env, &target, &op, &ctx.stop)
                            .map_err(ErrView::from);
                        ctx.send(Msg::ControlDone { local, op, result });
                    })
                    .map_err(|e| io_error("start control thread", e))?;
                self.tasks.push(task);
            }
            Cmd::Quit(code) => return Ok(Some(code)),
        }
        Ok(None)
    }
    fn apply(&mut self, app: &mut App, msg: Msg) -> Result<Option<i32>, GlenError> {
        // Fatal input errors cannot be retried by typing. Data-source errors remain ordinary toasts.
        if let Msg::SourceError {
            source: INPUT,
            error,
        } = msg
        {
            let mut fatal = GlenError::new(ErrorCode::Io, error.message);
            fatal.hint = error.hint;
            return Err(fatal);
        }
        for command in update::update(app, msg) {
            if let Some(code) = self.execute(command)? {
                return Ok(Some(code));
            }
        }
        Ok(None)
    }
}
fn submit(
    ctx: &Ctx,
    target: Target,
    text: String,
    mode: DeliverAs,
) -> Result<send::Submitted, GlenError> {
    let cancelled = || {
        GlenError::new(
            ErrorCode::Io,
            "glen watch stopped before submitting the message",
        )
    };
    if ctx.stopped() {
        return Err(cancelled());
    }
    let arg = target
        .path
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(target.id);
    // The real send opener is unprobed. send::submit performs live-route checks before probing.
    let opened = open::open_unprobed(&ctx.cli, &ctx.env, &arg)?;
    let cwd = send::send_cwd(Some(&opened), None, &ctx.env)?;
    if ctx.stopped() {
        return Err(cancelled());
    }
    send::submit(&ctx.cli, &ctx.env, Some(&opened), cwd, text, mode)
}
fn reap(tasks: &mut Vec<JoinHandle<()>>) -> Result<(), GlenError> {
    let mut index = 0;
    let mut first = None;
    while index < tasks.len() {
        if tasks[index].is_finished() {
            if tasks.swap_remove(index).join().is_err() {
                first.get_or_insert_with(|| {
                    GlenError::new(ErrorCode::Internal, "glen watch action thread panicked")
                });
            }
        } else {
            index += 1;
        }
    }
    first.map_or(Ok(()), Err)
}
fn finish_tasks(tasks: &mut Vec<JoinHandle<()>>, bound: Duration) -> Result<(), GlenError> {
    let deadline = Instant::now() + bound;
    let mut first = None;
    loop {
        if let Err(error) = reap(tasks) {
            first.get_or_insert(error);
        }
        if tasks.is_empty() || Instant::now() >= deadline {
            break;
        }
        thread::sleep(
            Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    // An unfinished submit may still be in Catalog::read. Detach after the shared total deadline.
    tasks.clear();
    first.map_or(Ok(()), Err)
}
#[cfg(debug_assertions)]
#[derive(Clone, Copy)]
struct PanicTest {
    worker: bool,
    after: Duration,
}
#[cfg(debug_assertions)]
impl PanicTest {
    fn read(env: &Env) -> Option<Self> {
        let (where_, ms) = env.get("GLEN_WATCH_TEST_PANIC")?.split_once(':')?;
        Some(Self {
            worker: match where_ {
                "ui" => false,
                "worker" => true,
                _ => return None,
            },
            after: Duration::from_millis(ms.parse().ok()?),
        })
    }
    fn worker(self, ctx: &Ctx) -> Result<(), GlenError> {
        if !self.worker {
            return Ok(());
        }
        let ctx = ctx.clone();
        drop(
            thread::Builder::new()
                .name("watch-test-panic".into())
                .spawn(move || {
                    let start = Instant::now();
                    while !ctx.stopped() {
                        if start.elapsed() >= self.after {
                            panic!("GLEN_WATCH_TEST_PANIC worker");
                        }
                        thread::sleep(
                            Duration::from_millis(10)
                                .min(self.after.saturating_sub(start.elapsed())),
                        );
                    }
                })
                .map_err(|e| io_error("start debug panic thread", e))?,
        );
        Ok(())
    }
}
/// Incoming snapshots must not postpone animation, toast expiry, or finished-row expiry.
fn clock_tick(app: &App, deadline: &mut Instant, now: Instant, wall: SystemTime) -> Option<Msg> {
    let next = now + update::tick_interval(app);
    if now >= *deadline {
        *deadline = next;
        Some(Msg::Tick { now, wall })
    } else {
        *deadline = (*deadline).min(next);
        None
    }
}
fn terminal_error(hup: &AtomicBool, error: GlenError) -> Result<i32, GlenError> {
    if hup.load(Ordering::Acquire) {
        Ok(129)
    } else {
        Err(error)
    }
}
type WatchTerminal = Terminal<CrosstermBackend<Box<dyn io::Write>>>;
fn event_loop(
    app: &mut App,
    rx: &Receiver<Msg>,
    effects: &mut Effects,
    terminal: &mut WatchTerminal,
    hup: &AtomicBool,
) -> Result<i32, GlenError> {
    let size = terminal
        .size()
        .map_err(|e| io_error("read terminal size", e))?;
    app.size = (size.width, size.height);
    let mut deadline = Instant::now() + update::tick_interval(app);
    #[cfg(debug_assertions)]
    let panic_test = PanicTest::read(&effects.ctx.env);
    #[cfg(debug_assertions)]
    let started = Instant::now();
    loop {
        #[cfg(debug_assertions)]
        if panic_test.is_some_and(|test| !test.worker && started.elapsed() >= test.after) {
            panic!("GLEN_WATCH_TEST_PANIC ui");
        }
        // Every iteration follows input/data or Tick; the first renders the initial state.
        view::prepare(app);
        if let Err(error) = terminal.draw(|f| view::draw(f, app)) {
            return terminal_error(hup, io_error("draw terminal frame", error));
        }
        reap(&mut effects.tasks)?;
        let msg = match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(msg) => Some(msg),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(GlenError::new(
                    ErrorCode::Internal,
                    "glen watch event channel disconnected",
                ));
            }
        };
        for msg in msg.into_iter().chain(rx.try_iter()).take(256) {
            if matches!(msg, Msg::Signal(1)) {
                hup.store(true, Ordering::Release);
            }
            if let Some(code) = effects.apply(app, msg)? {
                return Ok(code);
            }
        }
        if let Some(tick) = clock_tick(app, &mut deadline, Instant::now(), SystemTime::now())
            && let Some(code) = effects.apply(app, tick)?
        {
            return Ok(code);
        }
    }
}
pub(crate) fn run(
    cli: Arc<Cli>,
    env: Env,
    home: GlenHome,
    mut app: App,
    opened: Option<Opened>,
) -> Result<i32, GlenError> {
    app.timezone = jiff::tz::TimeZone::system();
    let _hook = PanicHook::install();
    let (tx, rx) = mpsc::channel();
    let ctx = Ctx {
        cli,
        env,
        home,
        stop: Arc::new(AtomicBool::new(false)),
        tx,
    };
    let hup = Arc::new(AtomicBool::new(false));
    let quitting = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    let signals = Signals::start(&ctx, hup.clone(), quitting.clone())?;
    let guard = TerminalGuard::enter().map_err(|e| io_error("enter terminal", e))?;
    let mut effects = Effects::start(ctx.clone(), &app, opened);
    let result = (|| {
        let backend = CrosstermBackend::new(Box::new(io::stdout()) as Box<dyn io::Write>);
        let mut terminal =
            Terminal::new(backend).map_err(|e| io_error("initialise terminal", e))?;
        let result = (|| {
            start_input(&ctx)?;
            effects.execute(Cmd::Refresh)?;
            #[cfg(debug_assertions)]
            if let Some(test) = PanicTest::read(&ctx.env) {
                test.worker(&ctx)?;
            }
            event_loop(&mut app, &rx, &mut effects, &mut terminal, &hup)
        })();
        // ratatui-core 0.1.2's Terminal::drop uses an error-print macro when showing
        // the cursor fails, which panics if stderr is also dead. TerminalGuard owns all real
        // teardown; detach the failed/live output before Drop and retain the first error.
        *terminal.backend_mut() = CrosstermBackend::new(Box::new(io::sink()));
        result
    })();
    quitting.store(true, Ordering::Release);
    ctx.stop.store(true, Ordering::Release);
    let restored = guard.restore().map_err(|e| io_error("restore terminal", e));
    let finished = finish_tasks(&mut effects.tasks, Duration::from_secs(6));
    #[cfg(unix)]
    drop(signals);
    match result {
        Err(error) => terminal_error(&hup, error),
        Ok(code) if code != 0 => Ok(code), // Preserve signal/draw exit codes over secondary teardown errors.
        Ok(_) => {
            restored?;
            finished?;
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cli::Tools,
        jobs::JobStore,
        render::palette::{ColorDepth, Theme},
        tui::{
            sources::tests::Rig,
            state::{ControlOp, Scope},
            theme::TuiTheme,
        },
    };
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    fn app() -> App {
        App::new(
            TuiTheme {
                depth: ColorDepth::None,
                theme: Theme::Dark,
            },
            Scope::All,
            (100, 30),
            Instant::now(),
            SystemTime::now(),
        )
    }
    type Receivers = (
        Receiver<sources::sessions::Command>,
        Receiver<sources::follow::Command>,
        Receiver<sources::jobs::Command>,
        Receiver<sources::inbox::Command>,
    );
    fn effects(ctx: Ctx) -> (Effects, Receivers) {
        let (sessions, s) = mpsc::channel();
        let (follow, f) = mpsc::channel();
        let (jobs, j) = mpsc::channel();
        let (inbox, i) = mpsc::channel();
        (
            Effects {
                ctx,
                sessions,
                follow,
                jobs,
                inbox,
                tasks: vec![],
            },
            (s, f, j, i),
        )
    }
    #[test]
    fn partial_enter_restores_every_step_and_keeps_the_original_error() {
        for failed in ENTER {
            let active = AtomicBool::new(false);
            let mut seen = vec![];
            let result = enter_with(&active, |step| {
                if step == TerminalStep::Raw {
                    assert!(active.load(Ordering::Acquire));
                }
                seen.push(step);
                if step == failed {
                    Err(io::Error::other("enter failed"))
                } else if LEAVE.contains(&step) {
                    Err(io::Error::other("cleanup failed"))
                } else {
                    Ok(())
                }
            });
            assert_eq!(result.unwrap_err().to_string(), "enter failed");
            assert!(!active.load(Ordering::Acquire));
            assert_eq!(&seen[seen.len() - 4..], &LEAVE);
        }
    }
    #[test]
    fn restore_is_idempotent_attempts_all_actions_and_returns_only_the_first_error() {
        let active = AtomicBool::new(true);
        let mut seen = vec![];
        let error = restore_with(&active, |step| {
            seen.push(step);
            Err(io::Error::other(format!("{step:?}")))
        })
        .unwrap_err();
        assert_eq!(seen, LEAVE);
        assert_eq!(error.to_string(), "NoPaste");
        restore_with(&active, |_| panic!("restored twice")).unwrap();
        assert!(!active.load(Ordering::Acquire));
        active.store(true, Ordering::Release);
        assert_eq!(
            enter_with(&active, |_| panic!("already entered"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(active.load(Ordering::Acquire));
    }
    #[test]
    fn input_adapter_forwards_only_keys_paste_and_resize() {
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(matches!(input_msg(Event::Key(key)),Some(Msg::Key(k)) if k == key));
        assert!(
            matches!(input_msg(Event::Paste("a\r\nb".into())),Some(Msg::Paste(s)) if s == "a\r\nb")
        );
        assert!(matches!(
            input_msg(Event::Resize(72, 20)),
            Some(Msg::Resize(72, 20))
        ));
        assert!(input_msg(Event::FocusGained).is_none());
        assert!(input_msg(Event::FocusLost).is_none());
    }
    #[test]
    fn commands_route_to_their_workers_and_quit_returns_without_waiting() {
        let rig = Rig::new(false);
        let (ctx, _) = rig.ctx();
        let (mut effects, (s, f, j, i)) = effects(ctx);
        let target = Target {
            id: "session".into(),
            path: None,
        };
        effects
            .execute(Cmd::Follow {
                r#gen: 9,
                target: target.clone(),
                opts: ItemOpts {
                    thinking: true,
                    tools: Tools::Full,
                },
            })
            .unwrap();
        let command = f.try_recv().unwrap();
        assert_eq!(command.r#gen, 9);
        assert_eq!(command.target, target);
        assert!(command.opts.thinking);
        assert!(matches!(command.opts.tools, Tools::Full));
        effects.execute(Cmd::ListScope(Scope::All)).unwrap();
        assert!(matches!(
            s.try_recv().unwrap(),
            sources::sessions::Command::Scope(Scope::All)
        ));
        effects.execute(Cmd::Refresh).unwrap();
        assert!(matches!(
            s.try_recv().unwrap(),
            sources::sessions::Command::Refresh
        ));
        assert!(matches!(
            j.try_recv().unwrap(),
            sources::jobs::Command::Refresh
        ));
        assert!(matches!(
            i.try_recv().unwrap(),
            sources::inbox::Command::Refresh
        ));
        assert_eq!(effects.execute(Cmd::Quit(143)).unwrap(), Some(143));
        let mut state = app();
        assert_eq!(
            effects.apply(&mut state, Msg::Signal(1)).unwrap(),
            Some(129)
        );
        assert!(!rig.home.root.exists());
    }
    #[test]
    fn control_and_submit_refusals_arrive_as_owned_results_without_creating_home() {
        let rig = Rig::new(false);
        let (ctx, rx) = rig.ctx();
        let (mut effects, _readers) = effects(ctx);
        let target = Target {
            id: "missing".into(),
            path: Some(rig.temp.path().join("missing.jsonl")),
        };
        effects
            .execute(Cmd::Control {
                local: 3,
                target: target.clone(),
                op: ControlOp::Abort,
            })
            .unwrap();
        assert!(
            matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(),Msg::ControlDone {local:3,op:ControlOp::Abort,result:Err(e)} if e.code == "not_implemented")
        );
        effects
            .execute(Cmd::Submit {
                local: 4,
                target,
                text: "-".into(),
                mode: DeliverAs::Auto,
            })
            .unwrap();
        assert!(
            matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(),Msg::Submitted {local:4,result:Err(e)} if e.code == "not_found")
        );
        finish_tasks(&mut effects.tasks, Duration::from_secs(1)).unwrap();
        assert!(!rig.home.root.exists());
    }
    #[test]
    fn successful_submit_tracks_the_child_before_reporting_the_literal_hyphen() {
        let rig = Rig::new(true);
        let (mut ctx, rx) = rig.ctx();
        let bin =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bin/fake-omp");
        ctx.env = Env::for_test(
            Some(rig.temp.path()),
            rig.env.cwd(),
            &[
                ("PI_CODING_AGENT_DIR", rig.agent.to_str().unwrap()),
                ("GLEN_HOME", rig.home.root.to_str().unwrap()),
                ("GLEN_OMP_BIN", bin.to_str().unwrap()),
            ],
        );
        let (mut effects, (_s, _f, j, _i)) = effects(ctx);
        effects
            .execute(Cmd::Submit {
                local: 5,
                target: Target {
                    id: "a".into(),
                    path: Some(rig.path('A')),
                },
                text: "-".into(),
                mode: DeliverAs::FollowUp,
            })
            .unwrap();
        let reported = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let sources::jobs::Command::Track { id, mut child } = j.try_recv().unwrap() else {
            panic!("expected child tracking");
        };
        assert!(
            matches!(reported,Msg::Submitted {local:5,result:Ok(job)} if job.id == id.as_str() && job.handle == id.handle())
        );
        let stored = JobStore::new(rig.home.clone()).read_job(&id).unwrap();
        assert_eq!(stored.text, "-");
        assert_eq!(stored.deliver_as, crate::jobs::Deliver::FollowUp);
        let deadline = Instant::now() + Duration::from_secs(3);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        finish_tasks(&mut effects.tasks, Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn cancelled_submit_never_opens_the_target_and_input_failures_are_fatal() {
        let rig = Rig::new(false);
        let (ctx, _) = rig.ctx();
        ctx.stop.store(true, Ordering::Release);
        let error = submit(
            &ctx,
            Target {
                id: "missing".into(),
                path: None,
            },
            "x".into(),
            DeliverAs::Auto,
        )
        .err()
        .unwrap();
        assert_eq!(error.code, ErrorCode::Io);
        assert!(error.message.contains("stopped before submitting"));
        let (mut effects, _readers) = effects(ctx);
        let error = effects
            .apply(
                &mut app(),
                Msg::SourceError {
                    source: INPUT,
                    error: io_error("read terminal input", io::Error::other("failed")).into(),
                },
            )
            .unwrap_err();
        assert!(error.message.contains("read terminal input"));
        assert!(!rig.home.root.exists());
    }
    #[test]
    fn action_deadline_is_shared_and_readers_are_not_part_of_the_join_set() {
        let (tx, rx) = mpsc::channel();
        let mut tasks = (0..3)
            .map(|_| {
                let tx = tx.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(100));
                    tx.send(()).unwrap();
                })
            })
            .collect::<Vec<_>>();
        let start = Instant::now();
        finish_tasks(&mut tasks, Duration::from_millis(5)).unwrap();
        assert!(start.elapsed() < Duration::from_millis(50));
        assert!(tasks.is_empty());
        for _ in 0..3 {
            rx.recv_timeout(Duration::from_secs(1)).unwrap();
        }
        let start = Instant::now();
        finish_tasks(&mut vec![], Duration::from_secs(6)).unwrap();
        assert!(start.elapsed() < Duration::from_millis(50));
    }
    #[cfg(debug_assertions)]
    #[test]
    fn panic_knob_is_parsed_only_for_known_debug_modes() {
        for (value, expected) in [
            ("ui:0", Some(false)),
            ("worker:20", Some(true)),
            ("other:1", None),
            ("ui:bad", None),
        ] {
            let env = Env::for_test(
                None,
                std::path::Path::new("/fixture"),
                &[("GLEN_WATCH_TEST_PANIC", value)],
            );
            assert_eq!(PanicTest::read(&env).map(|v| v.worker), expected);
        }
    }
    #[test]
    fn steady_message_traffic_does_not_starve_clock_or_toast_expiry() {
        let mut app = app();
        let start = app.now;
        app.toast = Some(super::super::state::Toast {
            text: "notice".into(),
            until: start + Duration::from_secs(3),
        });
        let mut deadline = start + update::tick_interval(&app);
        for step in 1..=80 {
            // An ordinary stream arriving faster than recv_timeout's idle interval.
            update::update(
                &mut app,
                Msg::Jobs(super::super::state::JobsSnapshot {
                    rows: vec![],
                    active_total: 0,
                }),
            );
            let now = start + Duration::from_millis(50 * step);
            if let Some(tick) = clock_tick(&app, &mut deadline, now, SystemTime::UNIX_EPOCH) {
                update::update(&mut app, tick);
            }
            if step >= 60 {
                assert!(app.toast.is_none());
                break;
            }
        }
        assert!(app.now >= start + Duration::from_secs(3));
        assert!(app.frame >= 12);
    }
    #[test]
    fn closed_command_channel_returns_a_contextual_error() {
        let (tx, rx) = mpsc::channel::<()>();
        drop(rx);
        let error = command_send(&tx, (), "follow").unwrap_err();
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(error.message.contains("watch follow worker disconnected"));
    }
    #[test]
    fn a_hangup_observed_by_the_signal_reader_preserves_exit_129_on_io_failure() {
        let hup = AtomicBool::new(false);
        let error = || io_error("draw terminal frame", io::Error::other("closed terminal"));
        assert_eq!(
            terminal_error(&hup, error()).unwrap_err().code,
            ErrorCode::Io
        );
        // The iterator records SIGHUP before queuing Msg::Signal, independently of UI draining.
        hup.store(true, Ordering::Release);
        assert_eq!(terminal_error(&hup, error()).unwrap(), 129);
    }
}
