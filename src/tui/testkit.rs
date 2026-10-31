//! Deterministic, fixture-backed scenes. No worker, terminal guard, or live omp access.
use super::{layout, state::*, theme::TuiTheme, update, view};
use crate::{
    cli::DeliverAs,
    commands::wait,
    error::{ErrorCode, GlenError},
    glen_home::GlenHome,
    jobs::{Job, JobError, JobResult, JobStore, Reply, Route, State as JobState, Status},
    omp::{
        fixtures, items,
        lease::State,
        listing::{self, SessionFile},
        session::{Session, TurnStatus},
    },
    render::{
        palette::{ColorDepth, Token},
        text,
    },
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::{Buffer, Cell},
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::{Color, Modifier},
};
use std::{
    fmt::Write as _,
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneKind {
    Main,
    EmptyScope,
    SessionsDirMissing,
    ListLoading,
    Opening,
    NotYetPersisted,
    OpenFailed,
    NoRouteInline,
    OversizeMessage,
    Sending,
    JobCompletedReplies,
    JobFailedNoRoute,
    JobForked,
    InboxOverlay,
    HelpControlsOff,
    HelpControlsOn,
    ConfirmAbort,
    ModelPicker,
    ScrolledWithNew,
    NarrowTranscript,
    TooSmall,
    UntrustedTitle,
    MultilinePrompt,
}
impl SceneKind {
    pub const STATES: [Self; 22] = [
        Self::EmptyScope,
        Self::SessionsDirMissing,
        Self::ListLoading,
        Self::Opening,
        Self::NotYetPersisted,
        Self::OpenFailed,
        Self::NoRouteInline,
        Self::OversizeMessage,
        Self::Sending,
        Self::JobCompletedReplies,
        Self::JobFailedNoRoute,
        Self::JobForked,
        Self::InboxOverlay,
        Self::HelpControlsOff,
        Self::HelpControlsOn,
        Self::ConfirmAbort,
        Self::ModelPicker,
        Self::ScrolledWithNew,
        Self::NarrowTranscript,
        Self::TooSmall,
        Self::UntrustedTitle,
        Self::MultilinePrompt,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::EmptyScope => "empty_scope",
            Self::SessionsDirMissing => "sessions_dir_missing",
            Self::ListLoading => "list_loading",
            Self::Opening => "opening",
            Self::NotYetPersisted => "not_yet_persisted",
            Self::OpenFailed => "open_failed",
            Self::NoRouteInline => "no_route_inline",
            Self::OversizeMessage => "oversize_message",
            Self::Sending => "sending",
            Self::JobCompletedReplies => "job_completed_replies",
            Self::JobFailedNoRoute => "job_failed_no_route",
            Self::JobForked => "job_forked",
            Self::InboxOverlay => "inbox_overlay",
            Self::HelpControlsOff => "help_controls_off",
            Self::HelpControlsOn => "help_controls_on",
            Self::ConfirmAbort => "confirm_abort",
            Self::ModelPicker => "model_picker",
            Self::ScrolledWithNew => "scrolled_with_new",
            Self::NarrowTranscript => "narrow_60x20_transcript_tab",
            Self::TooSmall => "too_small_39x9",
            Self::UntrustedTitle => "untrusted_title",
            Self::MultilinePrompt => "multiline_prompt",
        }
    }
    pub fn size(self) -> (u16, u16) {
        match self {
            Self::NarrowTranscript => (60, 20),
            Self::TooSmall => (39, 9),
            _ => (80, 24),
        }
    }
}
fn key(app: &mut App, code: KeyCode) -> Vec<Cmd> {
    update::update(app, Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}
fn follow(app: &mut App, ev: FollowMsg) {
    let r#gen = app.transcript.r#gen;
    update::update(app, Msg::Follow { r#gen, ev });
}
fn reply(id: &str, text: &str, at: &str, final_: bool, session: &str) -> Reply {
    Reply {
        v: 1,
        id: id.into(),
        session_id: session.into(),
        job_id: Some("00000000-0000-7000-8000-000001234567".into()),
        request_id: None,
        tool_call_id: None,
        text: text.into(),
        is_final: final_,
        at: at.into(),
    }
}
fn job(session: &str) -> Job {
    Job {
        v: 1,
        id: "00000000-0000-7000-8000-000001234567".into(),
        handle: "j1234567".into(),
        created_at: "2026-10-30T11:59:43Z".into(),
        route: Route::Rpc,
        session_id: Some(session.into()),
        session_file: Some("/fixtures/A.jsonl".into()),
        session_handle: Some("e27c0de".into()),
        deliver_as: DeliverAs::Auto.into(),
        text: "run the tests again".into(),
        cwd: "/work/alpha".into(),
        omp_bin: None,
        live_socket: None,
        live_pid: None,
        profile_flag: None,
        owners_dir: "/fixtures/owners".into(),
        job_timeout_ms: 90_000,
        deadline_at: "2026-10-30T12:01:13Z".into(),
    }
}
fn jobs(app: &mut App, kind: SceneKind) {
    let session = app.list.selected.as_deref().unwrap();
    let job = job(session);
    let mut status = Status {
        v: 1,
        status: JobState::Accepted,
        updated_at: "2026-10-30T11:59:50Z".into(),
        ..Status::default()
    };
    let mut result = None;
    match kind {
        SceneKind::JobCompletedReplies | SceneKind::JobForked => {
            status.status = JobState::Completed;
            status.updated_at = "2026-10-30T11:59:57Z".into();
            result = Some(JobResult {
                v: 1,
                prompt_status: Some("completed".into()),
                agent_invoked: true,
                assistant_text: Some("The flaky assertion is fixed.".into()),
                replies: vec![
                    reply(
                        "reply-1",
                        "duplicate result copy",
                        "2026-10-30T11:59:53Z",
                        false,
                        session,
                    ),
                    reply(
                        "reply-2",
                        "All tests passed.",
                        "2026-10-30T11:59:57Z",
                        true,
                        session,
                    ),
                ],
                command_output: vec![],
                stop_reason: Some("stop".into()),
                session_settled: true,
                settled_at: Some(status.updated_at.clone()),
            });
            if kind == SceneKind::JobForked {
                status.forked = Some(crate::jobs::Forked {
                    from: session.into(),
                    to: "00000000-0000-7000-8000-000007654321".into(),
                });
            }
            app.list.rows[0].state = State::Live;
        }
        SceneKind::JobFailedNoRoute => {
            status.status = JobState::Failed;
            status.error = Some(JobError {
                code: "no_route".into(),
                message: "session has no live route".into(),
                hint: Some("run glen install-extension, then restart omp".into()),
                details: None,
            });
        }
        _ => (),
    }
    let done = if status.status == JobState::Completed {
        Some(if let Some(forked) = &status.forked {
            Done::Forked(format!(
                "✓ forked → {}",
                crate::rpc::session_handle(&forked.to)
            ))
        } else {
            Done::Completed(wait::completed_summary(
                &job,
                &status,
                "2026-10-30T12:00:00Z".parse().unwrap(),
            ))
        })
    } else if status.status.is_terminal() {
        // This helper only formats an error; the store constructor does not create/read a directory.
        let store = JobStore::new(GlenHome {
            root: "/fixtures/glen".into(),
        });
        Some(Done::Failed(
            wait::job_error(&store, &job, &status).unwrap().into(),
        ))
    } else {
        None
    };
    let terminal = status.status.is_terminal();
    let row = JobRow {
        id: job.id.clone(),
        handle: job.handle.clone(),
        session_id: job.session_id.clone(),
        actual_session_id: None,
        route: job.route,
        progress: wait::job_progress(&job, &status),
        status,
        created_at: job.created_at,
        result,
        ours: true,
        finished_at: terminal.then_some(app.now - Duration::from_secs(3)),
        done,
    };
    update::update(
        app,
        Msg::Jobs(JobsSnapshot {
            rows: vec![row],
            active_total: usize::from(!terminal),
        }),
    );
}

pub fn scene(kind: SceneKind, size: (u16, u16), theme: TuiTheme) -> App {
    static NOW: OnceLock<Instant> = OnceLock::new();
    let now = *NOW.get_or_init(Instant::now);
    let wall: SystemTime = "2026-10-30T12:00:00Z"
        .parse::<jiff::Timestamp>()
        .unwrap()
        .into();
    let mut app = App::new(
        theme,
        Scope::Bucket {
            cwd: "/work/alpha".into(),
            label: "~/work/alpha".into(),
        },
        size,
        now,
        wall,
    );
    if matches!(
        kind,
        SceneKind::EmptyScope | SceneKind::SessionsDirMissing | SceneKind::ListLoading
    ) {
        if kind != SceneKind::ListLoading {
            update::update(
                &mut app,
                Msg::Sessions(ListSnapshot {
                    rows: vec![],
                    total_all: 0,
                    sessions_dir_missing: (kind == SceneKind::SessionsDirMissing)
                        .then(|| "/fixtures/agent/sessions".into()),
                }),
            );
        }
        return app;
    }
    let mut rows = vec![];
    let mut selected = None;
    for (index, fixture) in ['A', 'B', 'C', 'E'].into_iter().enumerate() {
        let path = format!("/fixtures/{fixture}.jsonl");
        let session = Session::parse(Path::new(&path), fixtures::bytes(fixture)).unwrap();
        let summary = listing::summarize(
            SessionFile {
                path: path.into(),
                bucket: "-work-alpha".into(),
                mtime: wall - Duration::from_secs([180, 420, 3600, 86_400][index]),
                size: fixtures::bytes(fixture).len() as u64,
            },
            &session,
        );
        let branch = session.active_branch();
        let model = branch
            .iter()
            .rev()
            .filter(|entry| entry.kind == "message" && entry.raw["message"]["role"] == "assistant")
            .find_map(|entry| listing::model_of(&entry.raw["message"]))
            .or_else(|| {
                branch
                    .iter()
                    .rev()
                    .filter(|entry| entry.kind == "model_change")
                    .find_map(|entry| entry.raw["model"].as_str().map(str::to_owned))
            });
        rows.push(SessionRow {
            id: summary.id.clone(),
            handle: crate::rpc::session_handle(&summary.id),
            path: Some(summary.file.path.clone()),
            title: text::single_line(
                summary
                    .title
                    .as_deref()
                    .or(summary.first_message.as_deref())
                    .unwrap_or("untitled"),
            ),
            first_message: summary.first_message.as_deref().map(text::single_line),
            cwd: summary.cwd,
            state: [State::Busy, State::Live, State::Idle, State::Unknown][index],
            source: if index < 2 { "extension" } else { "disk" },
            last_turn: TurnStatus::of_branch(&branch),
            updated: summary.file.mtime,
            model: model.clone(),
        });
        if fixture == 'A' {
            selected = Some((session, model));
        }
    }
    update::update(
        &mut app,
        Msg::Sessions(ListSnapshot {
            rows,
            total_all: 4,
            sessions_dir_missing: None,
        }),
    );
    let (session, model) = selected.unwrap();
    let session_id = session.header.id.clone();
    if !matches!(
        kind,
        SceneKind::Opening | SceneKind::NotYetPersisted | SceneKind::OpenFailed
    ) {
        let items = items::build(&session.active_branch(), &app.transcript.opts);
        follow(
            &mut app,
            FollowMsg::Opened {
                header: Header {
                    title: text::single_line(&session.title().unwrap()),
                    handle: "e27c0de".into(),
                    model,
                },
                items,
                dropped: 0,
                not_yet_persisted: false,
            },
        );
    } else if kind == SceneKind::NotYetPersisted {
        follow(&mut app, FollowMsg::NotYetPersisted);
    } else if kind == SceneKind::OpenFailed {
        follow(
            &mut app,
            FollowMsg::Failed(
                GlenError::new(
                    ErrorCode::Io,
                    "cannot read /fixtures/A.jsonl: permission denied",
                )
                .into(),
            ),
        );
    }
    app.focus = Focus::Prompt;
    app.narrow_tab = Pane::Transcript;
    update::update(&mut app, Msg::Paste("run the tests again".into()));
    let record = InboxRecord {
        message: InboxMsg {
            id: "reply-1".into(),
            session_id: session_id.clone(),
            job_id: Some("00000000-0000-7000-8000-000001234567".into()),
            text: "I found the flaky assertion.".into(),
            is_final: false,
            at: "2026-10-30T11:59:53Z".into(),
            unread: true,
        },
        position: InboxPosition { end: 128, ino: 1 },
    };
    update::update(
        &mut app,
        Msg::Inbox(InboxSnapshot {
            records: vec![record],
            ..InboxSnapshot::default()
        }),
    );
    jobs(&mut app, kind);
    match kind {
        SceneKind::Opening
        | SceneKind::NotYetPersisted
        | SceneKind::OpenFailed
        | SceneKind::JobCompletedReplies
        | SceneKind::JobFailedNoRoute
        | SceneKind::JobForked
        | SceneKind::NarrowTranscript
        | SceneKind::TooSmall => {
            key(&mut app, KeyCode::Esc);
        }
        SceneKind::NoRouteInline | SceneKind::Sending => {
            let commands = key(&mut app, KeyCode::Enter);
            let [Cmd::Submit { local, .. }] = commands.as_slice() else {
                panic!("expected submit command");
            };
            if kind == SceneKind::NoRouteInline {
                update::update(
                    &mut app,
                    Msg::Submitted {
                        local: *local,
                        result: Err(GlenError::new(
                            ErrorCode::NoRoute,
                            "session has no live route",
                        )
                        .with_hint("run glen install-extension, then restart omp")
                        .into()),
                    },
                );
            }
        }
        SceneKind::OversizeMessage => {
            app.prompt.text.clear();
            app.prompt.cursor = 0;
            update::update(&mut app, Msg::Paste("x".repeat(524_289)));
            assert!(key(&mut app, KeyCode::Enter).is_empty());
        }
        SceneKind::HelpControlsOn => {
            key(&mut app, KeyCode::Esc);
            key(&mut app, KeyCode::Char('?'));
        }
        SceneKind::ConfirmAbort => {
            update::update(
                &mut app,
                Msg::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)),
            );
        }
        SceneKind::ModelPicker => {
            let mut models: Vec<omp_rpc::ModelInfo> =
                serde_json::from_str(include_str!("../../tests/support/fake_models.json")).unwrap();
            models.sort_by(|a, b| (&a.provider, &a.id).cmp(&(&b.provider, &b.id)));
            key(&mut app, KeyCode::Esc);
            let commands = key(&mut app, KeyCode::Char('m'));
            let [Cmd::Control { local, op, .. }] = commands.as_slice() else {
                panic!("model picker requests models")
            };
            update::update(
                &mut app,
                Msg::ControlDone {
                    local: *local,
                    op: op.clone(),
                    result: Ok(ControlData::Models {
                        models: models
                            .into_iter()
                            .map(|m| ModelChoice {
                                label: format!(
                                    "{}/{} — {}{}",
                                    m.provider,
                                    m.id,
                                    m.name,
                                    if m.provider == "fake" && m.id == "fake-1" {
                                        " (current)"
                                    } else {
                                        ""
                                    }
                                ),
                                current: m.provider == "fake" && m.id == "fake-1",
                                provider: m.provider,
                                id: m.id,
                            })
                            .collect(),
                        thinking_level: Some("medium".into()),
                        warnings: vec![],
                        thinking_levels: vec![
                            "off".into(),
                            "low".into(),
                            "medium".into(),
                            "high".into(),
                        ],
                    }),
                },
            );
        }
        SceneKind::InboxOverlay | SceneKind::HelpControlsOff => {
            if kind == SceneKind::HelpControlsOff {
                app.caps.controls = false;
            }
            key(&mut app, KeyCode::Esc);
            key(
                &mut app,
                KeyCode::Char(if kind == SceneKind::InboxOverlay {
                    'i'
                } else {
                    '?'
                }),
            );
        }
        SceneKind::ScrolledWithNew => {
            key(&mut app, KeyCode::Esc);
            key(&mut app, KeyCode::Home);
            follow(
                &mut app,
                FollowMsg::Items(vec![items::Item::User {
                    id: "new-item".into(),
                    timestamp: None,
                    text: "Please check the remaining test failures.".into(),
                    images: 0,
                }]),
            );
        }
        SceneKind::UntrustedTitle => {
            app.list.rows[0].title = text::single_line("\x1b[31mOops");
            app.transcript.header.as_mut().unwrap().title = app.list.rows[0].title.clone();
            app.focus = Focus::Sessions;
            app.narrow_tab = Pane::Sessions;
        }
        SceneKind::MultilinePrompt => {
            app.prompt.text.clear();
            app.prompt.cursor = 0;
            update::update(&mut app,Msg::Paste("run the tests again\n界 and café\nthen explain the failure\ninclude the exact command".into()));
        }
        _ => (),
    }
    app
}

/// Normalize local HH:MM fields in a copy of the finished grid, preserving every style/column.
/// This avoids process-global TZ changes and still snapshots the timestamp's layout and style.
fn normalize_times(buffer: &mut Buffer) {
    for y in buffer.area.y..buffer.area.bottom() {
        for x in buffer.area.x..buffer.area.right().saturating_sub(4) {
            let s = (x..x + 5)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>();
            let b = s.as_bytes();
            if b.len() == 5
                && b[0].is_ascii_digit()
                && b[1].is_ascii_digit()
                && b[2] == b':'
                && b[3].is_ascii_digit()
                && b[4].is_ascii_digit()
                && &s[..2] <= "23"
                && &s[3..] <= "59"
            {
                for (offset, c) in "12:00".chars().enumerate() {
                    buffer[(x + offset as u16, y)].set_symbol(&c.to_string());
                }
            }
        }
    }
}
fn foreground(color: Color, theme: TuiTheme) -> String {
    let raw = match color {
        Color::Reset => "default".into(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(n) => format!("indexed({n})"),
        c => format!("{c:?}").to_ascii_lowercase(),
    };
    if color == Cell::default().fg {
        return raw;
    }
    for (token, name) in [
        (Token::Muted, "muted"),
        (Token::Border, "border"),
        (Token::Accent, "accent"),
        (Token::Accent2, "accent_2"),
        (Token::Success, "success"),
        (Token::Warning, "warning"),
        (Token::Danger, "danger"),
        (Token::Focus, "focus"),
    ] {
        if theme.style(token).fg == Some(color) {
            return format!("{name}({raw})");
        }
    }
    raw
}
pub fn dump(buffer: &Buffer, theme: TuiTheme) -> String {
    let default = Cell::default();
    assert!(
        buffer.content.iter().all(|cell| cell.bg == default.bg),
        "background fill in scene"
    );
    let mut buffer = buffer.clone();
    normalize_times(&mut buffer);
    let area = buffer.area;
    let mut result = format!("grid {}x{}\n", area.width, area.height);
    for y in area.y..area.bottom() {
        let mut line = String::new();
        let mut x = area.x;
        while x < area.right() {
            let symbol = buffer[(x, y)].symbol();
            line.push_str(symbol);
            x = x.saturating_add(text::width(symbol).max(1) as u16);
        }
        assert_eq!(
            text::width(&line),
            usize::from(area.width),
            "grid row {y} has broken wide cells"
        );
        writeln!(result, "{}", line.trim_end()).unwrap();
    }
    result.push_str("styles (zero-based, half-open columns):\n");
    for y in area.y..area.bottom() {
        let mut start = area.x;
        while start < area.right() {
            let cell = &buffer[(start, y)];
            let mut end = start + 1;
            while end < area.right() && buffer[(end, y)].style() == cell.style() {
                end += 1;
            }
            write!(
                result,
                "r{y} c{start}-{end} fg={}",
                foreground(cell.fg, theme)
            )
            .unwrap();
            if cell.modifier.contains(Modifier::BOLD) {
                result.push_str(" bold");
            }
            if cell.modifier.contains(Modifier::DIM) {
                result.push_str(" dim");
            }
            let other = cell.modifier - Modifier::BOLD - Modifier::DIM;
            if !other.is_empty() {
                write!(result, " {other:?}").unwrap();
            }
            writeln!(result).unwrap();
            start = end;
        }
    }
    result.push_str("bg: none\n");
    result
}
pub fn render(mut app: App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(app.size.0, app.size.1)).unwrap();
    terminal
        .draw(|frame| view::render(frame, &mut app))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let default = Cell::default();
    for cell in &buffer.content {
        assert_eq!(cell.bg, default.bg, "background fill: {cell:?}");
        if app.theme.depth == ColorDepth::None {
            assert_eq!(
                cell.style(),
                default.style(),
                "styled NO_COLOR cell: {cell:?}"
            );
        }
    }
    let cursor = terminal.backend().cursor_visible();
    let expected = app.focus == Focus::Prompt
        && app.overlay.is_none()
        && layout::mode(app.size.0, app.size.1) != layout::Mode::TooSmall;
    assert_eq!(cursor, expected, "cursor visibility for {:?}", app.focus);
    let cursor = if cursor {
        let position = terminal.backend().cursor_position();
        assert!(
            layout::areas(&app).input.contains(position),
            "cursor is outside input"
        );
        format!("{},{}", position.x, position.y)
    } else {
        "hidden".into()
    };
    format!("cursor: {cursor}\n{}", dump(buffer, app.theme))
}
