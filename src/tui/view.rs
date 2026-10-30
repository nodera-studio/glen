//! Pure drawing. `prepare` updates only the render cache; `draw` borrows state immutably.
use super::{
    ansi,
    keymap::{self, Action, KeyBinding, KeymapCtx, OverlayCtx},
    layout::{self, Areas, Mode},
    state::*,
    theme, update,
};
use crate::{
    cli::DeliverAs,
    jobs::State as JobState,
    omp::{lease::State, session::TurnStatus},
    render::{
        self as text_render,
        palette::{ColorDepth, Token},
        text, transcript as renderer,
    },
};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyModifiers},
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};
use std::borrow::Cow;
use unicode_segmentation::UnicodeSegmentation;

pub fn prepare(app: &mut App) {
    let (width, height) = layout::viewport(app);
    if layout::mode(app.size.0, app.size.1) != Mode::TooSmall {
        app.list.page = Some(layout::list_page(app));
        update::set_viewport(app, width, height);
    }
}
/// Mutable convenience entry point for the future event loop; no terminal or other I/O.
pub fn render(f: &mut Frame, app: &mut App) {
    prepare(app);
    draw(f, app);
}
pub fn draw(f: &mut Frame, app: &App) {
    if layout::mode(app.size.0, app.size.1) == Mode::TooSmall {
        too_small(f, app);
        return;
    }
    let a = layout::areas(app);
    title_bar(f, app, a.title);
    tabs(f, app, &a);
    if let Some(rect) = a.list {
        list(f, app, rect);
    }
    if let Some(rect) = a.divider {
        for y in rect.y..rect.bottom() {
            paint(
                f,
                app,
                Rect::new(rect.x, y, rect.width, 1),
                vec![styled("│", app.theme.style(Token::Border))],
            );
        }
    }
    if let Some(rect) = a.transcript {
        transcript(f, app, rect);
    }
    strip(f, app, a.strip);
    prompt_rule(f, app, a.rule);
    notice(f, app, a.notice);
    input(f, app, a.input);
    status_bar(f, app, a.status);
    overlay(f, app, &a);
}
pub(super) fn handle(app: &App) -> &str {
    app.list
        .rows
        .iter()
        .find(|r| Some(&r.id) == app.list.selected.as_ref())
        .map(|r| r.handle.as_str())
        .or_else(|| app.transcript.header.as_ref().map(|h| h.handle.as_str()))
        .unwrap_or("?")
}
fn styled(s: impl Into<String>, style: Style) -> Line<'static> {
    Line::from(Span::styled(s.into(), style))
}
fn fit(line: Line<'static>, width: u16) -> Line<'static> {
    let width = usize::from(width);
    if line.width() <= width {
        return line;
    }
    if width == 0 {
        return Line::default();
    }
    let mut spans = vec![];
    let mut used = 0;
    let mut last = Style::default();
    'outer: for span in line.spans {
        let mut content = String::new();
        last = span.style;
        for grapheme in span.content.graphemes(true) {
            let cells = text::width(grapheme);
            if used + cells > width - 1 {
                if !content.is_empty() {
                    spans.push(Span::styled(content, last));
                }
                break 'outer;
            }
            content.push_str(grapheme);
            used += cells;
        }
        if !content.is_empty() {
            spans.push(Span::styled(content, last));
        }
    }
    spans.push(Span::styled("…", last));
    Line::from(spans).style(line.style)
}
fn paint(f: &mut Frame, app: &App, rect: Rect, lines: Vec<Line<'static>>) {
    let rect = rect.intersection(f.area());
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let lines = lines
        .into_iter()
        .take(usize::from(rect.height))
        .map(|line| {
            let mut line = fit(line, rect.width);
            if app.theme.depth == ColorDepth::None {
                line.style = Style::default();
                for span in &mut line.spans {
                    span.style = Style::default();
                }
            }
            line
        })
        .collect::<Vec<_>>();
    f.render_widget(Paragraph::new(lines), rect);
}
fn join(left: Line<'static>, right: Line<'static>, width: u16) -> Line<'static> {
    if right.width() == 0 {
        return fit(left, width);
    }
    if right.width() >= usize::from(width) {
        return fit(right, width);
    }
    let budget = usize::from(width).saturating_sub(right.width() + 1);
    let mut left = fit(left, budget as u16);
    let spaces = usize::from(width).saturating_sub(left.width() + right.width());
    left.spans.push(Span::raw(" ".repeat(spaces)));
    left.spans.extend(right.spans);
    left
}
fn state(state: State) -> (&'static str, &'static str, Token) {
    match state {
        State::Idle => ("○", "idle", Token::Muted),
        State::Live => ("●", "live", Token::Success),
        State::Busy => ("◐", "busy", Token::Focus),
        State::Unknown => ("?", "unknown", Token::Muted),
    }
}
fn mode_name(mode: DeliverAs) -> &'static str {
    match mode {
        DeliverAs::Auto => "auto",
        DeliverAs::Steer => "steer",
        DeliverAs::FollowUp => "follow-up",
    }
}
fn selected(app: &App) -> Option<&SessionRow> {
    app.list
        .rows
        .iter()
        .find(|r| Some(&r.id) == app.list.selected.as_ref())
}
fn session_title(row: &SessionRow, timezone: &jiff::tz::TimeZone) -> String {
    if !row.title.trim().is_empty() {
        return text::single_line(&row.title);
    }
    if let Some(first) = row
        .first_message
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        return text::single_line(first);
    }
    let time = jiff::Timestamp::try_from(row.updated)
        .ok()
        .map(|t| text::local_hm_in(&t, timezone))
        .unwrap_or_else(|| "??:??".into());
    format!("untitled · {time}")
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TitlePart {
    Sessions,
    Live,
    Jobs,
    Unread,
}
/// Binding collapse order: unread, jobs, live, then truncate the session count.
pub fn title_segments(app: &App, width: u16) -> Vec<(TitlePart, String)> {
    let live = app
        .list
        .rows
        .iter()
        .filter(|r| matches!(r.state, State::Live | State::Busy))
        .count();
    let jobs = app.jobs.active_total;
    let unread = app.inbox.unread_by_session.values().copied().sum::<usize>();
    let mut parts = vec![
        (
            TitlePart::Sessions,
            format!("{} sessions", app.list.rows.len()),
        ),
        (TitlePart::Live, format!("{live} live")),
    ];
    if jobs > 0 {
        parts.push((
            TitlePart::Jobs,
            format!("◐ {jobs} {}", if jobs == 1 { "job" } else { "jobs" }),
        ));
    }
    if unread > 0 {
        parts.push((TitlePart::Unread, format!("↩ {unread}")));
    }
    let available = usize::from(width).saturating_sub(text::width("glen watch") + 1);
    let cost = |parts: &[(TitlePart, String)]| {
        parts.iter().map(|(_, s)| text::width(s)).sum::<usize>() + parts.len().saturating_sub(1) * 3
    };
    for drop in [TitlePart::Unread, TitlePart::Jobs, TitlePart::Live] {
        if cost(&parts) <= available {
            break;
        }
        parts.retain(|(name, _)| *name != drop);
    }
    if let Some((_, sessions)) = parts.first_mut() {
        *sessions = text::truncate(sessions, available);
    }
    parts.retain(|(_, s)| !s.is_empty());
    parts
}
fn title_bar(f: &mut Frame, app: &App, rect: Rect) {
    let left = Line::from(vec![
        Span::styled("glen", app.theme.bold()),
        Span::styled(" watch", app.theme.style(Token::Muted)),
    ]);
    let parts = title_segments(app, rect.width);
    let mut spans = vec![];
    for (i, (_, s)) in parts.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", app.theme.style(Token::Muted)));
        }
        spans.push(Span::styled(s, app.theme.style(Token::Muted)));
    }
    paint(
        f,
        app,
        rect,
        vec![join(left, Line::from(spans), rect.width)],
    );
}
fn tab_line(app: &App, title: String, focus: bool, prompt: bool) -> Line<'static> {
    let marker = if focus || prompt {
        theme::FOCUS_BAR
    } else {
        " "
    };
    let marker_style = app.theme.style(if focus {
        Token::Accent
    } else if prompt {
        Token::Border
    } else {
        Token::Muted
    });
    let text_style = if focus {
        app.theme.bold()
    } else if prompt {
        Style::default()
    } else {
        app.theme.style(Token::Muted)
    };
    Line::from(vec![
        Span::styled(marker, marker_style),
        Span::styled(title, text_style),
    ])
}
fn transcript_tab(app: &App, width: u16, narrow: bool) -> Line<'static> {
    let row = selected(app);
    let (glyph, label, token) = state(row.map_or(State::Unknown, |r| r.state));
    let title = app
        .transcript
        .header
        .as_ref()
        .map(|h| text::single_line(&h.title))
        .filter(|s| !s.is_empty())
        .or_else(|| row.map(|row| session_title(row, &app.timezone)))
        .unwrap_or_else(|| "session".into());
    let caption = if narrow {
        format!("{} · ", text::single_line(handle(app)))
    } else {
        format!("{} · {title} · ", text::single_line(handle(app)))
    };
    let mut line = tab_line(
        app,
        caption,
        app.focus == Focus::Transcript,
        app.focus == Focus::Prompt,
    );
    line.spans.push(Span::styled(
        format!("{glyph} {label}"),
        app.theme.style(token),
    ));
    let model = app
        .transcript
        .header
        .as_ref()
        .and_then(|h| h.model.as_deref())
        .or_else(|| row.and_then(|r| r.model.as_deref()))
        .map(|s| text::single_line(s.rsplit('/').next().unwrap_or(s)));
    if !narrow
        && let Some(model) = model
        && line.width() + text::width(&model) + 2 <= usize::from(width)
    {
        line = join(line, styled(model, app.theme.style(Token::Muted)), width);
    }
    fit(line, width)
}
fn tabs(f: &mut Frame, app: &App, a: &Areas) {
    let mode = layout::mode(app.size.0, app.size.1);
    let left_w = match mode {
        Mode::Wide => 34,
        Mode::Medium => 24,
        _ => 12.min(a.tabs.width),
    };
    let list_caption = if mode == Mode::Wide {
        format!(
            "sessions · {}",
            match &app.scope {
                Scope::All => "all projects".into(),
                Scope::Bucket { label, .. } => text::truncate_left(
                    &text::single_line(label),
                    usize::from(left_w).saturating_sub(text::width("▍sessions · ")),
                ),
            }
        )
    } else {
        "sessions".into()
    };
    paint(
        f,
        app,
        Rect::new(a.tabs.x, a.tabs.y, left_w, 1),
        vec![tab_line(
            app,
            list_caption,
            app.focus == Focus::Sessions,
            false,
        )],
    );
    if a.tabs.width > left_w {
        paint(
            f,
            app,
            Rect::new(left_w, a.tabs.y, 1, 1),
            vec![styled("│", app.theme.style(Token::Border))],
        );
        let right = Rect::new(
            left_w + 1,
            a.tabs.y,
            a.tabs.width.saturating_sub(left_w + 1),
            1,
        );
        paint(
            f,
            app,
            right,
            vec![transcript_tab(app, right.width, mode == Mode::Narrow)],
        );
    }
}
fn list(f: &mut Frame, app: &App, rect: Rect) {
    let mode = layout::mode(app.size.0, app.size.1);
    let area = if mode == Mode::Narrow {
        layout::inner(rect)
    } else {
        rect
    };
    let mut lines = vec![];
    if !app.list.loaded {
        lines.push(styled(
            format!("{} reading sessions", theme::spinner(app.frame)),
            app.theme.style(Token::Muted),
        ));
    } else if app.list.rows.is_empty() {
        let mut messages = vec![];
        if let Some(path) = &app.list.sessions_dir_missing {
            messages.push("no omp sessions yet".into());
            messages.push(format!(
                "({} does not exist)",
                text::single_line(&text_render::tilde(path, app.home.as_deref()))
            ));
        } else {
            messages.push(match &app.scope {
                Scope::Bucket { cwd, .. } => format!(
                    "no omp sessions for {}",
                    text::single_line(&text_render::tilde(cwd, app.home.as_deref()))
                ),
                Scope::All => "no omp sessions yet".into(),
            });
            if app.list.total_all > 0 {
                messages.push(format!(
                    "a shows {} sessions in other projects",
                    app.list.total_all
                ));
            }
        }
        lines.extend(
            messages
                .into_iter()
                .map(|message| styled(message, app.theme.style(Token::Muted))),
        );
    }
    let offset = layout::list_offset(app, rect);
    for row in app
        .list
        .rows
        .iter()
        .skip(offset)
        .take(usize::from(rect.height))
    {
        let chosen = Some(&row.id) == app.list.selected.as_ref();
        let marker = if chosen { theme::SELECT_BAR } else { " " };
        let marker_style = if chosen {
            app.theme.style(if app.focus == Focus::Sessions {
                Token::Focus
            } else {
                Token::Border
            })
        } else {
            Style::default()
        };
        let (glyph, _, token) = state(row.state);
        let mut first = Line::from(vec![
            Span::styled(marker, marker_style),
            Span::styled(glyph, app.theme.style(token)),
            Span::raw(" "),
            Span::styled(
                text::single_line(&row.handle),
                app.theme.style(Token::Focus),
            ),
        ]);
        let title = session_title(row, &app.timezone);
        let unread = app
            .inbox
            .unread_by_session
            .get(&row.id)
            .copied()
            .unwrap_or(0);
        let mut right = vec![];
        if unread > 0 {
            right.push(Span::styled(
                format!("↩{unread}  "),
                app.theme.style(Token::Accent2),
            ));
        }
        right.push(Span::styled(
            text::age_in(app.wall, row.updated, &app.timezone),
            app.theme.style(Token::Muted),
        ));
        if mode == Mode::Wide {
            lines.push(join(first, Line::from(right), area.width));
            let second = Line::from(vec![
                Span::styled(marker, marker_style),
                Span::raw("  "),
                Span::styled(
                    title,
                    if chosen {
                        app.theme.bold()
                    } else {
                        Style::default()
                    },
                ),
            ]);
            let turn = match row.last_turn {
                TurnStatus::Pending => Some(("pending", Token::Warning)),
                TurnStatus::Interrupted => Some(("interrupted", Token::Warning)),
                TurnStatus::Aborted => Some(("aborted", Token::Warning)),
                TurnStatus::Error => Some(("error", Token::Danger)),
                _ => None,
            };
            lines.push(if let Some((s, t)) = turn {
                join(second, styled(s, app.theme.style(t)), area.width)
            } else {
                fit(second, area.width)
            });
        } else {
            first
                .spans
                .push(Span::raw(if mode == Mode::Narrow { "  " } else { " " }));
            first.spans.push(Span::styled(
                title,
                if chosen {
                    app.theme.bold()
                } else {
                    Style::default()
                },
            ));
            lines.push(if mode == Mode::Narrow {
                join(first, Line::from(right), area.width)
            } else {
                fit(first, area.width)
            });
        }
    }
    paint(f, app, area, lines);
}
fn reflow(app: &App, width: u16) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut lines = vec![];
    let mut starts = vec![];
    let mut previous = None;
    let painter = app.theme.painter();
    if app.transcript.dropped > 0 {
        lines.push(styled(
            format!(
                "── {} earlier items · glen show {} ──",
                app.transcript.dropped,
                text::single_line(handle(app))
            ),
            app.theme.style(Token::Muted),
        ));
    }
    let style = renderer::Style {
        painter: &painter,
        width: Some(width.max(1)),
        tools: app.transcript.opts.tools,
        thinking: app.transcript.opts.thinking,
    };
    for block in &app.transcript.blocks {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        starts.push(lines.len());
        match block {
            Block::Item(item) => lines.extend(ansi::lines(&renderer::render_item_in(
                item,
                &mut previous,
                &style,
                &app.timezone,
            ))),
            Block::Notice(s) => lines.push(styled(
                text_render::sanitize(s).into_owned(),
                app.theme.style(Token::Muted),
            )),
        }
    }
    (lines, starts)
}
fn empty_transcript_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let muted = Token::Muted;
    let messages = match &app.transcript.status {
        TranscriptStatus::Idle => vec![
            ("select a session".into(), muted),
            ("↑↓ choose · enter write".into(), muted),
        ],
        TranscriptStatus::Opening => vec![(
            format!("{} reading {}", theme::spinner(app.frame), handle(app)),
            muted,
        )],
        TranscriptStatus::NotYetPersisted => vec![
            (
                format!(
                    "{} is live but has not written a transcript yet",
                    handle(app)
                ),
                muted,
            ),
            ("send a message to start it".into(), muted),
        ],
        TranscriptStatus::Ready => vec![("no messages yet".into(), muted)],
        TranscriptStatus::Failed(error) => {
            // Keep the follower/opener's message verbatim, including tail -f's not_found prose.
            let mut messages = vec![(
                format!("✗ {}", text::single_line(&error.message)),
                Token::Danger,
            )];
            if let Some(hint) = &error.hint {
                messages.push((format!("  {}", text::single_line(hint)), muted));
            }
            messages
        }
    };
    messages
        .into_iter()
        .flat_map(|(message, token)| {
            text::wrap(&message, Some(usize::from(width).max(1)))
                .into_iter()
                .map(move |line| styled(line, app.theme.style(token)))
        })
        .collect()
}
fn transcript(f: &mut Frame, app: &App, rect: Rect) {
    let area = layout::inner(rect);
    if matches!(app.transcript.status, TranscriptStatus::Failed(_)) {
        paint(f, app, area, empty_transcript_lines(app, area.width));
        return;
    }
    let scroll = &app.transcript.scroll;
    let height = area.height.saturating_sub(u16::from(!scroll.follow));
    let (lines, starts) = if app.transcript.width == area.width.max(1) {
        (Cow::Borrowed(app.transcript.lines.as_slice()), None)
    } else {
        let (lines, starts) = reflow(app, area.width.max(1));
        (Cow::Owned(lines), Some(starts))
    };
    let mut top = if scroll.follow {
        lines.len().saturating_sub(usize::from(height))
    } else {
        scroll.top
    };
    if !scroll.follow
        && let Some(starts) = starts
    {
        let anchor = app
            .transcript
            .starts
            .partition_point(|start| *start <= top)
            .checked_sub(1);
        top = anchor.and_then(|i| starts.get(i)).copied().unwrap_or(0);
    }
    if lines.is_empty() {
        paint(
            f,
            app,
            Rect::new(area.x, area.y, area.width, height),
            empty_transcript_lines(app, area.width),
        );
    } else {
        paint(
            f,
            app,
            Rect::new(area.x, area.y, area.width, height),
            lines
                .iter()
                .skip(top)
                .take(usize::from(height))
                .cloned()
                .collect(),
        );
    }
    if !scroll.follow && area.height > 0 {
        let (s, t) = if scroll.unseen > 0 {
            (format!("↓ {} new · G follows", scroll.unseen), Token::Focus)
        } else {
            ("G follows".into(), Token::Muted)
        };
        paint(
            f,
            app,
            Rect::new(area.x, area.bottom() - 1, area.width, 1),
            vec![styled(s, app.theme.style(t))],
        );
    }
}
fn elapsed(app: &App, created: &str) -> String {
    let ms = created
        .parse::<jiff::Timestamp>()
        .ok()
        .and_then(|t| app.wall.duration_since(t.into()).ok())
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128);
    crate::commands::wait::duration(ms as i64)
}
struct Activity {
    at: Option<std::time::SystemTime>,
    priority: u8,
    key: String,
    lines: Vec<Line<'static>>,
}
fn date(s: &str) -> Option<std::time::SystemTime> {
    s.parse::<jiff::Timestamp>().ok().map(Into::into)
}
fn instant_date(app: &App, at: std::time::Instant) -> Option<std::time::SystemTime> {
    app.wall.checked_sub(app.now.saturating_duration_since(at))
}
fn activity_groups(app: &App) -> Vec<Activity> {
    let Some(session) = app.list.selected.as_deref() else {
        return vec![];
    };
    let mut groups = vec![];
    for send in app.sends.iter().filter(|s| s.session_id == session) {
        let mut lines = vec![];
        match &send.phase {
            SendPhase::Routing => {
                let text = text::single_line(&send.text);
                let preview = text.graphemes(true).take(40).collect::<String>();
                let dots = if text.graphemes(true).count() > 40 {
                    "…"
                } else {
                    ""
                };
                lines.push(styled(
                    format!(
                        "{} sending \"{preview}{dots}\" · {}",
                        theme::spinner(app.frame),
                        mode_name(send.mode)
                    ),
                    app.theme.style(Token::Focus),
                ));
            }
            _ => continue,
        }
        groups.push(Activity {
            at: instant_date(app, send.started),
            priority: 2,
            key: format!("{:020}", send.local),
            lines,
        });
    }
    let mut replies = std::collections::BTreeMap::new();
    for row in app.jobs.rows.iter().filter(|r| {
        r.session_id.as_deref() == Some(session) || r.actual_session_id.as_deref() == Some(session)
    }) {
        if let Some(result) = &row.result {
            for reply in result.replies.iter().filter(|r| r.session_id == session) {
                replies.insert(reply.id.clone(), reply.clone());
            }
        }
        let mut lines = vec![];
        if !row.status.status.is_terminal() {
            lines.push(styled(
                format!(
                    "{} {} · {} · {}",
                    theme::spinner(app.frame),
                    text::single_line(&row.handle),
                    text::single_line(&row.progress),
                    elapsed(app, &row.created_at)
                ),
                app.theme.style(Token::Focus),
            ));
        } else if row.ours {
            match &row.done {
                Some(Done::Completed(s) | Done::Forked(s)) => lines.push(styled(
                    text::single_line(s),
                    app.theme.style(Token::Success),
                )),
                Some(Done::Failed(err)) => {
                    lines.push(styled(
                        format!(
                            "{} {}",
                            if row.status.status == JobState::Aborted {
                                "!"
                            } else {
                                "✗"
                            },
                            text::single_line(&err.message)
                        ),
                        app.theme.style(if row.status.status == JobState::Aborted {
                            Token::Warning
                        } else {
                            Token::Danger
                        }),
                    ));
                    if let Some(hint) = &err.hint {
                        lines.push(styled(
                            format!("  {}", text::single_line(hint)),
                            app.theme.style(Token::Muted),
                        ));
                    }
                }
                None => lines.push(styled(
                    format!(
                        "! {} · {}",
                        text::single_line(&row.handle),
                        row.status.status.as_str()
                    ),
                    app.theme.style(Token::Warning),
                )),
            }
        }
        if !lines.is_empty() {
            let at = row
                .finished_at
                .and_then(|at| instant_date(app, at))
                .or_else(|| date(&row.status.updated_at))
                .or_else(|| date(&row.created_at));
            groups.push(Activity {
                at,
                priority: 0,
                key: row.id.clone(),
                lines,
            });
        }
    }
    for msg in app.inbox.records.iter().filter(|r| r.session_id == session) {
        replies.insert(
            msg.id.clone(),
            crate::jobs::Reply {
                v: 1,
                id: msg.id.clone(),
                session_id: msg.session_id.clone(),
                job_id: msg.job_id.clone(),
                request_id: None,
                tool_call_id: None,
                text: msg.text.clone(),
                is_final: msg.is_final,
                at: msg.at.clone(),
            },
        );
    }
    let mut replies = replies.into_values().collect::<Vec<_>>();
    replies.sort_by_key(|reply| (date(&reply.at), reply.id.clone()));
    let start = replies.len().saturating_sub(3);
    for reply in replies.into_iter().skip(start) {
        let line = styled(
            format!(
                "↩ {}{}",
                text::single_line(&reply.text),
                if reply.is_final { " ✓" } else { "" }
            ),
            app.theme.style(Token::Muted),
        );
        groups.push(Activity {
            at: date(&reply.at),
            priority: 1,
            key: reply.id,
            lines: vec![line],
        });
    }
    groups.sort_by(|a, b| {
        a.at.cmp(&b.at)
            .then(a.priority.cmp(&b.priority))
            .then(a.key.cmp(&b.key))
    });
    groups
}
pub(super) fn activity_lines(app: &App) -> Vec<Line<'static>> {
    activity_groups(app)
        .into_iter()
        .flat_map(|group| group.lines)
        .collect()
}
fn visible_activity(app: &App, capacity: usize) -> Vec<Line<'static>> {
    let mut selected = vec![];
    let mut left = capacity;
    for group in activity_groups(app).into_iter().rev() {
        if left == 0 {
            break;
        }
        let lines = group.lines.into_iter().take(left).collect::<Vec<_>>();
        left -= lines.len();
        selected.push(lines);
    }
    selected.into_iter().rev().flatten().collect()
}
fn rule_line(app: &App, width: u16, title: &str, style: Style, right: bool) -> Line<'static> {
    let label = text::truncate(title, usize::from(width.saturating_sub(4)));
    let left = if right {
        usize::from(width).saturating_sub(text::width(&label) + 2)
    } else {
        2.min(usize::from(width))
    };
    let rest = usize::from(width).saturating_sub(left + text::width(&label));
    Line::from(vec![
        Span::styled("─".repeat(left), app.theme.style(Token::Border)),
        Span::styled(label, style),
        Span::styled("─".repeat(rest), app.theme.style(Token::Border)),
    ])
}
fn strip(f: &mut Frame, app: &App, rect: Rect) {
    if rect.height == 0 {
        return;
    }
    let area = layout::inner(rect);
    paint(
        f,
        app,
        Rect::new(area.x, area.y, area.width, 1),
        vec![styled(
            "─".repeat(usize::from(area.width)),
            app.theme.style(Token::Border),
        )],
    );
    let count = usize::from(rect.height.saturating_sub(1));
    paint(
        f,
        app,
        Rect::new(
            area.x,
            area.y + 1,
            area.width,
            rect.height.saturating_sub(1),
        ),
        visible_activity(app, count),
    );
}
fn prompt_rule(f: &mut Frame, app: &App, rect: Rect) {
    let (label, token) = if app.prompt.locked.is_some() {
        (
            format!(" {} sending… ", theme::spinner(app.frame)),
            Token::Focus,
        )
    } else {
        (
            format!(" {} ", mode_name(app.prompt.mode)),
            match app.prompt.mode {
                DeliverAs::Auto => Token::Muted,
                DeliverAs::Steer => Token::Warning,
                DeliverAs::FollowUp => Token::Accent,
            },
        )
    };
    paint(
        f,
        app,
        rect,
        vec![rule_line(
            app,
            rect.width,
            &label,
            app.theme.style(token),
            true,
        )],
    );
}
fn notice(f: &mut Frame, app: &App, rect: Rect) {
    let lines = layout::notice_lines(app, rect.width);
    let last = lines.len().saturating_sub(1);
    let has_hint = app.prompt.notice.as_ref().is_some_and(|e| e.hint.is_some());
    paint(
        f,
        app,
        rect,
        lines
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                styled(
                    s,
                    app.theme.style(if has_hint && i == last {
                        Token::Muted
                    } else {
                        Token::Danger
                    }),
                )
            })
            .collect(),
    );
}
fn input(f: &mut Frame, app: &App, mut rect: Rect) {
    if rect.height == 0 || rect.width == 0 {
        return;
    }
    if app.prompt.text.is_empty() {
        let placeholder = if app.list.selected.is_none() {
            "select a session to write to it".into()
        } else if app.focus == Focus::Prompt {
            format!("message {} · enter send", handle(app))
        } else {
            format!("tab to write to {}", handle(app))
        };
        // Keep visual hints above the editable row, so text after the marker is only draft text.
        paint(
            f,
            app,
            Rect::new(rect.x, rect.y, rect.width, 1),
            vec![styled(placeholder, app.theme.style(Token::Muted))],
        );
        if app.list.selected.is_none() {
            if app.focus == Focus::Prompt
                && app.overlay.is_none()
                && f.area().contains((rect.x, rect.y).into())
            {
                f.set_cursor_position((rect.x, rect.y));
            }
            return;
        }
        rect.y = rect.y.saturating_add(1);
        rect.height = rect.height.saturating_sub(1);
        if rect.height == 0 {
            return;
        }
    }
    let (handle, lines, (cx, cy)) = layout::prompt_lines(app, rect.width);
    let prefix_width = text::width(&handle) + 3;
    let start = usize::from(cy).saturating_sub(usize::from(rect.height.saturating_sub(1)));
    let lines = lines
        .into_iter()
        .skip(start)
        .take(usize::from(rect.height))
        .enumerate()
        .map(|(i, s)| {
            let mut spans = if i == 0 {
                vec![
                    Span::styled(handle.clone(), app.theme.style(Token::Accent2)),
                    Span::raw(" "),
                    Span::styled(theme::PROMPT, app.theme.style(Token::Accent)),
                    Span::raw(" "),
                ]
            } else {
                vec![Span::raw(" ".repeat(prefix_width))]
            };
            spans.push(Span::raw(s));
            Line::from(spans)
        })
        .collect();
    paint(f, app, rect, lines);
    if app.focus == Focus::Prompt && app.overlay.is_none() {
        let x = rect
            .x
            .saturating_add(cx)
            .min(rect.right().saturating_sub(1));
        let y = rect
            .y
            .saturating_add(cy.saturating_sub(start as u16))
            .min(rect.bottom().saturating_sub(1));
        if f.area().contains((x, y).into()) {
            f.set_cursor_position((x, y));
        }
    }
}

/// Status hints and help use ASCII key spellings at every colour depth.
pub fn key_label(binding: KeyBinding, _depth: ColorDepth) -> String {
    let base = match binding.code {
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => "shift-tab".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pgup".into(),
        KeyCode::PageDown => "pgdn".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::F(n) => format!("f{n}"),
        KeyCode::Char(c) if binding.modifiers.contains(KeyModifiers::CONTROL) => {
            c.to_lowercase().collect()
        }
        KeyCode::Char(c) => c.to_string(),
        _ => "key".into(),
    };
    let mut prefix = String::new();
    for (modifier, ascii) in [
        (KeyModifiers::CONTROL, "^"),
        (KeyModifiers::ALT, "alt-"),
        (KeyModifiers::SHIFT, "shift-"),
    ] {
        if binding.modifiers.contains(modifier) && binding.code != KeyCode::BackTab {
            prefix.push_str(ascii);
        }
    }
    format!("{prefix}{base}")
}
#[derive(Clone, Debug)]
pub struct Hint {
    pub action: Action,
    pub key: String,
    pub label: String,
}
/// Priority order is explicit. Collapsing removes only a suffix of this list.
const PANE_HINT_PRIORITY: &[Action] = &[
    Action::NextFocus,
    Action::Help,
    Action::Quit,
    Action::FocusPrompt,
    Action::Down,
    Action::Up,
    Action::Last,
    Action::Thinking,
    Action::Tools,
    Action::Scope,
    Action::Refresh,
    Action::Inbox,
    Action::Abort,
    Action::Models,
    Action::ThinkingLevels,
    Action::Compact,
    Action::Interrupt,
];
const PROMPT_HINT_PRIORITY: &[Action] = &[
    Action::Send,
    Action::Newline,
    Action::Mode,
    Action::Escape,
    Action::NextFocus,
    Action::Help,
    Action::Interrupt,
    Action::Abort,
];
const OVERLAY_HINT_PRIORITY: &[Action] = &[
    Action::Escape,
    Action::Pick,
    Action::Confirm,
    Action::Cancel,
    Action::Help,
    Action::Interrupt,
];
fn control_action(action: Action) -> bool {
    matches!(
        action,
        Action::Abort | Action::Models | Action::ThinkingLevels | Action::Compact
    )
}
fn all_status_hints(app: &App) -> Vec<Hint> {
    let ctx = KeymapCtx::new(app.overlay.as_ref(), app.focus);
    let table = keymap::bindings(ctx);
    let priority = if app.overlay.is_some() {
        OVERLAY_HINT_PRIORITY
    } else if app.focus == Focus::Prompt {
        PROMPT_HINT_PRIORITY
    } else {
        PANE_HINT_PRIORITY
    };
    priority
        .iter()
        .filter_map(|a| {
            table
                .iter()
                .find(|(_, action, _)| action == a)
                .map(|(binding, action, label)| Hint {
                    action: *action,
                    key: key_label(*binding, app.theme.depth),
                    label: if control_action(*action) && !app.caps.controls {
                        format!("{label} (not in this build)")
                    } else {
                        (*label).into()
                    },
                })
        })
        .collect()
}
fn active_toast(app: &App) -> Option<String> {
    app.toast
        .as_ref()
        .filter(|toast| app.now < toast.until)
        .map(|toast| text::single_line(&toast.text))
}
fn status_budget(app: &App, width: u16) -> usize {
    let toast = active_toast(app).map_or(0, |s| text::width(&s).min(usize::from(width) / 2));
    usize::from(width).saturating_sub(toast + if toast > 0 { 2 } else { 0 })
}
/// Right-to-left priority ladder. Each complete hint has a separate width threshold.
pub fn status_segments(app: &App, width: u16) -> Vec<Hint> {
    let available = status_budget(app, width);
    let mut hints = all_status_hints(app);
    let cost = |hints: &[Hint]| {
        hints
            .iter()
            .map(|h| text::width(&h.key) + 1 + text::width(&h.label))
            .sum::<usize>()
            + hints.len().saturating_sub(1) * 2
    };
    while cost(&hints) > available {
        if hints.pop().is_none() {
            break;
        }
    }
    hints
}
fn confirmation(app: &App, op: &ControlOp, width: u16) -> String {
    let compact = matches!(op, ControlOp::Compact);
    let verb = if compact { "compact" } else { "stop" };
    let handle = text::single_line(handle(app));
    let mut question = if compact {
        format!("compact session {handle}?")
    } else {
        format!("stop the running turn in {handle}?")
    };
    let commands = format!("y {verb}  n cancel");
    let budget = usize::from(width).saturating_sub(text::width(&commands) + 2);
    if text::width(&question) > budget {
        question = if compact {
            format!("compact {handle}?")
        } else {
            format!("stop turn in {handle}?")
        };
    }
    format!("{}  {commands}", text::truncate(&question, budget))
}

fn status_bar(f: &mut Frame, app: &App, rect: Rect) {
    if let Some(Overlay::Confirm(op)) = &app.overlay {
        paint(
            f,
            app,
            rect,
            vec![styled(
                confirmation(app, op, rect.width),
                app.theme.style(Token::Warning),
            )],
        );
        return;
    }
    let hints = status_segments(app, rect.width);
    let mut spans = vec![];
    for (i, hint) in hints.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::raw(hint.key));
        spans.push(Span::styled(
            format!(" {}", hint.label),
            app.theme.style(Token::Muted),
        ));
    }
    let left = Line::from(spans);
    let line = if let Some(toast) = active_toast(app) {
        join(
            left,
            styled(
                text::truncate(&toast, usize::from(rect.width) / 2),
                app.theme.style(Token::Muted),
            ),
            rect.width,
        )
    } else {
        left
    };
    paint(f, app, rect, vec![line]);
}
fn overlay_area(a: &Areas) -> Option<Rect> {
    a.transcript
        .map(|rect| {
            Rect::new(
                rect.x,
                rect.y,
                rect.width,
                rect.height.saturating_add(a.strip.height),
            )
        })
        .or(a.list)
}
fn help_rows(app: &App) -> Vec<(String, String, bool)> {
    let mut rows = vec![];
    for (title, bindings) in keymap::help_groups() {
        rows.push((String::new(), title.into(), false));
        let mut actions = vec![];
        let mut group: Vec<(String, String, bool)> = vec![];
        for (binding, action, label) in bindings {
            let key = key_label(binding, app.theme.depth);
            if let Some(index) = actions.iter().position(|a| *a == action) {
                let current = &mut group[index].0;
                if !current.split(" / ").any(|s| s == key) {
                    current.push_str(" / ");
                    current.push_str(&key);
                }
            } else {
                actions.push(action);
                let unavailable = control_action(action) && !app.caps.controls;
                group.push((
                    key,
                    if unavailable {
                        format!("{label} (not in this build)")
                    } else {
                        label.into()
                    },
                    unavailable,
                ));
            }
        }
        rows.extend(group);
    }
    rows
}
fn help_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let rows = help_rows(app);
    let key_w = rows
        .iter()
        .map(|(key, _, _)| text::width(key))
        .max()
        .unwrap_or(0)
        .min(usize::from(width).saturating_sub(3))
        .max(1);
    let mut lines = vec![];
    for (key, label, unavailable) in rows {
        if key.is_empty() {
            lines.push(styled(
                label,
                app.theme.style(Token::Muted).patch(app.theme.bold()),
            ));
            continue;
        }
        let key = text::truncate(&key, key_w);
        let padding = key_w.saturating_sub(text::width(&key)) + 2;
        let label_width = usize::from(width).saturating_sub(key_w + 2).max(1);
        let labels = if unavailable && text::width(&label) > label_width {
            let mut lines = text::wrap(
                label.strip_suffix(" (not in this build)").unwrap_or(&label),
                Some(label_width),
            );
            lines.push("(not in this build)".into());
            lines
        } else {
            text::wrap(&label, Some(label_width))
        };
        for (index, label) in labels.into_iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(
                    if index == 0 {
                        format!("{key}{}", " ".repeat(padding))
                    } else {
                        " ".repeat(key_w + 2)
                    },
                    app.theme.style(if unavailable {
                        Token::Muted
                    } else {
                        Token::Focus
                    }),
                ),
                Span::styled(
                    label,
                    if unavailable {
                        app.theme.style(Token::Muted)
                    } else {
                        Style::default()
                    },
                ),
            ]));
        }
    }
    lines
}
pub(super) fn help_scroll_bounds(app: &App) -> (usize, usize) {
    let area = overlay_area(&layout::areas(app))
        .map(layout::inner)
        .unwrap_or_default();
    let page = usize::from(area.height.saturating_sub(2)).max(1);
    (page, help_lines(app, area.width).len().saturating_sub(page))
}
fn overlay(f: &mut Frame, app: &App, a: &Areas) {
    let Some(overlay) = &app.overlay else {
        return;
    };
    let Some(rect) = overlay_area(a) else {
        return;
    };
    let rect = rect.intersection(f.area());
    f.render_widget(Clear, rect);
    let area = layout::inner(rect);
    if area.height == 0 || area.width == 0 {
        return;
    }
    let title = match overlay {
        Overlay::Help => " help ".into(),
        Overlay::Inbox => " inbox ".into(),
        Overlay::Confirm(_) => " confirm ".into(),
        Overlay::Picker(p) => format!(
            " {} · filter: {} ",
            if p.kind == PickerKind::Model {
                "model"
            } else {
                "thinking level"
            },
            text::single_line(&p.filter)
        ),
    };
    paint(
        f,
        app,
        Rect::new(area.x, area.y, area.width, 1),
        vec![rule_line(app, area.width, &title, app.theme.bold(), false)],
    );
    if area.height < 2 {
        return;
    }
    let content = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(2),
    );
    let footer = Rect::new(area.x, area.bottom() - 1, area.width, 1);
    match overlay {
        Overlay::Help => {
            let lines = help_lines(app, content.width);
            let offset = app
                .help_scroll
                .min(lines.len().saturating_sub(usize::from(content.height)));
            paint(
                f,
                app,
                content,
                lines
                    .into_iter()
                    .skip(offset)
                    .take(usize::from(content.height))
                    .collect(),
            );
            paint(
                f,
                app,
                footer,
                vec![styled(
                    "up/down scroll · esc closes",
                    app.theme.style(Token::Muted),
                )],
            );
        }
        Overlay::Inbox => {
            let mut lines = vec![];
            for record in app.inbox.records.iter().skip(app.inbox.scroll) {
                let handle = app
                    .list
                    .rows
                    .iter()
                    .find(|r| r.id == record.session_id)
                    .map(|r| r.handle.clone())
                    .unwrap_or_else(|| crate::rpc::session_handle(&record.session_id));
                let age = record
                    .at
                    .parse::<jiff::Timestamp>()
                    .ok()
                    .map(|t| text::age_in(app.wall, t.into(), &app.timezone))
                    .unwrap_or_else(|| "unknown age".into());
                let job = record
                    .job_id
                    .as_deref()
                    .map(|id| {
                        crate::jobs::JobId::parse(id)
                            .map(|j| j.handle())
                            .unwrap_or_else(|| text::single_line(id))
                    })
                    .unwrap_or_else(|| "tui".into());
                lines.push(Line::from(vec![
                    Span::styled(
                        if record.unread { "● " } else { "  " },
                        app.theme.style(Token::Focus),
                    ),
                    Span::styled(
                        format!(
                            "↩ {} · {age} · {job}{}",
                            text::single_line(&handle),
                            if record.is_final { " · ✓" } else { "" }
                        ),
                        app.theme.style(Token::Muted),
                    ),
                ]));
                for body in text::clean_lines(&record.text) {
                    for line in text::wrap(
                        &body,
                        Some(usize::from(content.width.saturating_sub(2)).max(1)),
                    ) {
                        lines.push(Line::from(format!("  {line}")));
                    }
                }
                lines.push(Line::default());
                if lines.len() >= usize::from(content.height) {
                    break;
                }
            }
            if lines.is_empty() {
                lines.push(styled(
                    "no messages from omp yet",
                    app.theme.style(Token::Muted),
                ));
            }
            paint(f, app, content, lines);
            paint(
                f,
                app,
                footer,
                vec![styled(
                    "peek only · glen inbox marks these read",
                    app.theme.style(Token::Muted),
                )],
            );
        }
        Overlay::Picker(p) => {
            let entries = p.visible_entries();
            let page = usize::from(content.height).max(1);
            let offset = p.selected.saturating_sub(page.saturating_sub(1));
            let lines = entries
                .iter()
                .enumerate()
                .skip(offset)
                .take(page)
                .map(|(i, entry)| {
                    Line::from(vec![
                        Span::styled(
                            if i == p.selected { "● " } else { "  " },
                            app.theme.style(Token::Focus),
                        ),
                        Span::styled(
                            text::single_line(&entry.label),
                            if i == p.selected {
                                app.theme.bold()
                            } else {
                                Style::default()
                            },
                        ),
                    ])
                })
                .collect::<Vec<_>>();
            paint(
                f,
                app,
                content,
                if lines.is_empty() {
                    vec![styled("no matching choices", app.theme.style(Token::Muted))]
                } else {
                    lines
                },
            );
            let table = keymap::bindings(KeymapCtx {
                overlay: OverlayCtx::Picker,
                focus: app.focus,
            });
            let enter = table
                .iter()
                .find(|(_, a, _)| *a == Action::Pick)
                .map(|(k, _, _)| key_label(*k, app.theme.depth))
                .unwrap_or_default();
            let arrows = if app.theme.depth == ColorDepth::None {
                "Up/Down"
            } else {
                "↑↓"
            };
            paint(
                f,
                app,
                footer,
                vec![styled(
                    format!("{arrows} move · {enter} picks · type to filter · Esc closes"),
                    app.theme.style(Token::Muted),
                )],
            );
        }
        Overlay::Confirm(op) => {
            paint(
                f,
                app,
                content,
                vec![styled(
                    confirmation(app, op, content.width),
                    app.theme.style(Token::Warning),
                )],
            );
            paint(
                f,
                app,
                footer,
                vec![styled("Esc cancels", app.theme.style(Token::Muted))],
            );
        }
    }
}
fn too_small(f: &mut Frame, app: &App) {
    let message = format!(
        "glen watch needs at least 40×10 · now {}×{}",
        app.size.0, app.size.1
    );
    let lines = text::wrap(&message, Some(usize::from(app.size.0).max(1)))
        .into_iter()
        .map(|line| styled(line, app.theme.style(Token::Muted)))
        .collect();
    paint(f, app, Rect::new(0, 0, app.size.0, app.size.1), lines);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        error::{ErrorCode, GlenError},
        jobs::{Route, Status},
        omp::{items::Item, session::TurnStatus},
        render::palette::Theme,
        tui::theme::TuiTheme,
    };
    use ratatui::{
        Terminal,
        backend::TestBackend,
        buffer::{Buffer, Cell},
    };
    use std::{
        collections::HashSet,
        time::{Duration, Instant, SystemTime},
    };
    pub(crate) fn scene(theme: TuiTheme) -> App {
        let now = Instant::now();
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(2000);
        let mut app = App::new(
            theme,
            Scope::Bucket {
                cwd: "/work/alpha".into(),
                label: "~/work/alpha".into(),
            },
            (120, 30),
            now,
            wall,
        );
        app.caps.controls = false;
        let mut rows = vec![];
        for n in 0..8 {
            rows.push(SessionRow {
                id: format!("session-{n}"),
                handle: if n == 0 {
                    "e27c0de".into()
                } else {
                    format!("handle{n}")
                },
                path: None,
                title: format!("Fix the interface 界 and prompt editor {n}"),
                first_message: None,
                cwd: Some("/work/alpha".into()),
                state: [State::Busy, State::Live, State::Idle, State::Unknown][n % 4],
                source: "disk",
                last_turn: [
                    TurnStatus::Pending,
                    TurnStatus::Error,
                    TurnStatus::Complete,
                    TurnStatus::Interrupted,
                ][n % 4],
                updated: wall - Duration::from_secs(180),
                model: Some("provider/model-test".into()),
            });
        }
        update::update(
            &mut app,
            Msg::Sessions(ListSnapshot {
                rows,
                total_all: 8,
                sessions_dir_missing: None,
            }),
        );
        let r#gen = app.transcript.r#gen;
        update::update(&mut app,Msg::Follow{r#gen,ev:FollowMsg::Opened{header:Header{title:"Fix the interface 界 and prompt editor".into(),handle:"e27c0de".into(),model:Some("provider/model-test".into())},items:(0..8).map(|n|Item::User{id:n.to_string(),timestamp:None,text:format!("Message {n} with wide 界 text, a long line that wraps inside the transcript.\nSecond line with \x1b[31m escaped input."),images:0}).collect(),dropped:2,not_yet_persisted:false}});
        app.prompt.insert_paste(
            "A prompt with 界 and é.\nsecond line\nthird line\nfourth line\nfifth line",
        );
        app.focus = Focus::Prompt;
        app.narrow_tab = Pane::Transcript;
        app.prompt.notice=Some(GlenError::new(ErrorCode::NoRoute,"This deliberately long message explains why delivery failed and wraps onto two rows in a narrow viewport without spilling over the layout.").with_hint("run glen install-extension, then restart the omp session; safe \x1b").into());
        app.jobs.active_total = 2;
        app.jobs.rows.push(JobRow {
            id: "job".into(),
            handle: "j1234567".into(),
            session_id: Some("session-0".into()),
            actual_session_id: None,
            route: Route::Rpc,
            status: Status {
                status: JobState::Accepted,
                ..Status::default()
            },
            created_at: "1970-01-01T00:30:00Z".into(),
            result: None,
            ours: true,
            finished_at: None,
            progress: "waiting for the reply".into(),
            done: None,
        });
        app.sends.push(SendTrack {
            local: 1,
            session_id: "session-0".into(),
            handle: "e27c0de".into(),
            text: "A longer prompt with more than forty characters for the sending preview".into(),
            mode: DeliverAs::Steer,
            phase: SendPhase::Routing,
            started: now,
        });
        for n in 0..2 {
            app.inbox.records.push(InboxMsg {
                id: format!("reply{n}"),
                session_id: "session-0".into(),
                job_id: None,
                text: format!("A reply {n} with 界 and a newline\nMore reply text."),
                is_final: n == 1,
                at: "1970-01-01T00:31:00Z".into(),
                unread: true,
            });
        }
        app.inbox.unread_by_session.insert("session-0".into(), 2);
        app.toast = Some(Toast {
            text: "refreshed sessions".into(),
            until: now + Duration::from_secs(3),
        });
        app
    }
    fn palette(depth: ColorDepth, theme: Theme) -> TuiTheme {
        TuiTheme { depth, theme }
    }
    fn screenshot(app: &mut App) -> (Buffer, bool, ratatui::layout::Position) {
        let mut terminal = Terminal::new(TestBackend::new(app.size.0, app.size.1)).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
        (
            terminal.backend().buffer().clone(),
            terminal.backend().cursor_visible(),
            terminal.backend().cursor_position(),
        )
    }
    fn row_text(buffer: &Buffer, y: u16) -> String {
        (buffer.area.x..buffer.area.right())
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }
    fn assert_styles(buffer: &Buffer, depth: ColorDepth) {
        let default = Cell::default();
        for cell in &buffer.content {
            assert_eq!(cell.bg, default.bg);
            if depth == ColorDepth::None {
                assert_eq!(cell.fg, default.fg);
                assert!(cell.modifier.is_empty(), "{cell:?}");
            }
        }
    }
    fn picker(app: &App) -> Overlay {
        Overlay::Picker(Picker {
            filter: String::new(),
            kind: PickerKind::Model,
            entries: vec![
                PickerEntry {
                    label: "model-test".into(),
                    op: ControlOp::SetModel {
                        provider: "provider".into(),
                        id: "model-test".into(),
                    },
                },
                PickerEntry {
                    label: "Other model 界".into(),
                    op: ControlOp::SetThinking("high".into()),
                },
            ],
            selected: 0,
            target: app.transcript.target.clone().unwrap(),
        })
    }
    #[test]
    fn populated_size_sweep_and_breakpoints_draw_without_panics() {
        let mut app = scene(palette(ColorDepth::None, Theme::Dark));
        let mut sizes = vec![];
        for w in (1..=300).step_by(7) {
            for h in (1..=100).step_by(5) {
                sizes.push((w, h));
            }
        }
        for w in [39, 40, 41, 71, 72, 73, 99, 100, 101, 300] {
            for h in [9, 10, 11, 39, 40, 41, 99, 100] {
                sizes.push((w, h));
            }
        }
        sizes.push((0, 0));
        for (i, size) in sizes.into_iter().enumerate() {
            app.size = size;
            app.narrow_tab = if i % 2 == 0 {
                Pane::Sessions
            } else {
                Pane::Transcript
            };
            app.overlay = match i % 5 {
                0 => None,
                1 => Some(Overlay::Help),
                2 => Some(Overlay::Inbox),
                3 => Some(picker(&app)),
                _ => Some(Overlay::Confirm(ControlOp::Abort)),
            };
            let (buffer, cursor, position) = screenshot(&mut app);
            assert_styles(&buffer, ColorDepth::None);
            if cursor {
                assert!(buffer.area.contains(position));
                assert!(app.overlay.is_none());
                assert_eq!(app.focus, Focus::Prompt);
            }
        }
    }
    #[test]
    fn every_theme_has_default_background_and_none_has_no_foreground_or_modifiers() {
        for depth in [
            ColorDepth::None,
            ColorDepth::Ansi16,
            ColorDepth::Ansi256,
            ColorDepth::TrueColor,
        ] {
            for theme in [Theme::Dark, Theme::Light] {
                let mut app = scene(palette(depth, theme));
                for size in [
                    (40, 10),
                    (71, 30),
                    (72, 30),
                    (99, 40),
                    (100, 30),
                    (180, 60),
                    (300, 100),
                ] {
                    app.size = size;
                    for overlay in [
                        None,
                        Some(Overlay::Help),
                        Some(Overlay::Inbox),
                        Some(picker(&app)),
                        Some(Overlay::Confirm(ControlOp::Compact)),
                    ] {
                        app.overlay = overlay;
                        let (buffer, _, _) = screenshot(&mut app);
                        assert_styles(&buffer, depth);
                    }
                }
            }
        }
    }
    #[test]
    fn each_region_keeps_text_and_wide_cells_inside_its_rect() {
        for size in [(40, 10), (60, 30), (72, 40), (99, 30), (100, 40), (180, 60)] {
            let mut app = scene(palette(ColorDepth::TrueColor, Theme::Dark));
            app.size = size;
            prepare(&mut app);
            let a = layout::areas(&app);
            let mut regions: Vec<(Rect, u8)> = vec![
                (a.title, 0),
                (a.tabs, 1),
                (a.rule, 2),
                (a.notice, 3),
                (a.input, 4),
                (a.status, 5),
                (a.strip, 6),
            ];
            if let Some(r) = a.list {
                regions.push((r, 7));
            }
            if let Some(r) = a.transcript {
                regions.push((r, 8));
            }
            if let Some(r) = overlay_area(&a) {
                regions.push((r, 9));
            }
            for (rect, kind) in regions {
                if kind == 9 {
                    app.overlay = Some(Overlay::Help);
                }
                let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
                terminal
                    .draw(|f| match kind {
                        0 => title_bar(f, &app, rect),
                        1 => tabs(f, &app, &a),
                        2 => prompt_rule(f, &app, rect),
                        3 => notice(f, &app, rect),
                        4 => input(f, &app, rect),
                        5 => status_bar(f, &app, rect),
                        6 => strip(f, &app, rect),
                        7 => list(f, &app, rect),
                        8 => transcript(f, &app, rect),
                        _ => overlay(f, &app, &a),
                    })
                    .unwrap();
                let buffer = terminal.backend().buffer();
                for y in 0..size.1 {
                    for x in 0..size.0 {
                        let symbol = buffer[(x, y)].symbol();
                        if symbol.trim().is_empty() {
                            continue;
                        }
                        assert!(
                            rect.contains((x, y).into()),
                            "region {kind} {size:?}: {x},{y} {symbol:?} outside {rect:?}"
                        );
                        assert!(
                            usize::from(x) + text::width(symbol) <= usize::from(rect.right()),
                            "wide glyph crosses region"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn collapse_sweep_is_monotonic_and_hint_drops_have_stable_widths_between_them() {
        for depth in [ColorDepth::None, ColorDepth::TrueColor] {
            let mut app = scene(palette(depth, Theme::Dark));
            let mut dropped = HashSet::new();
            let mut prior = title_segments(&app, 300)
                .iter()
                .map(|(name, _)| *name)
                .collect::<HashSet<_>>();
            for width in (0..300).rev() {
                let current = title_segments(&app, width)
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<HashSet<_>>();
                for missing in prior.difference(&current) {
                    dropped.insert(*missing);
                }
                assert!(dropped.is_disjoint(&current));
                assert!(current.contains(&TitlePart::Sessions) || current.is_empty());
                if !current.contains(&TitlePart::Live) {
                    assert!(!current.contains(&TitlePart::Jobs));
                }
                if !current.contains(&TitlePart::Jobs) {
                    assert!(!current.contains(&TitlePart::Unread));
                }
                prior = current;
            }
            for focus in [Focus::Sessions, Focus::Transcript, Focus::Prompt] {
                app.focus = focus;
                for toast in [false, true] {
                    if !toast {
                        app.toast = None;
                    } else {
                        app.toast = Some(Toast {
                            text: "a long toast to reserve space".into(),
                            until: app.now + Duration::from_secs(3),
                        });
                    }
                    let mut dropped: Vec<Action> = vec![];
                    let mut prior = status_segments(&app, 600);
                    let mut last_drop = None;
                    for width in (0..600).rev() {
                        let current = status_segments(&app, width);
                        let missing = prior
                            .iter()
                            .filter(|old| !current.iter().any(|new| new.action == old.action))
                            .collect::<Vec<_>>();
                        assert!(
                            missing.len() <= 1,
                            "multiple hints dropped at width {width}"
                        );
                        if !missing.is_empty() {
                            if let Some(previous) = last_drop {
                                assert!(previous - width >= 2, "adjacent collapse steps");
                            }
                            last_drop = Some(width);
                            dropped.extend(missing.into_iter().map(|h| h.action));
                        }
                        assert!(current.iter().all(|h| !dropped.contains(&h.action)));
                        prior = current;
                    }
                }
            }
        }
    }
    #[test]
    fn key_register_is_consistent_in_status_help_and_picker_rows() {
        for depth in [
            ColorDepth::None,
            ColorDepth::Ansi16,
            ColorDepth::Ansi256,
            ColorDepth::TrueColor,
        ] {
            let mut app = scene(palette(depth, Theme::Dark));
            for focus in [Focus::Sessions, Focus::Transcript, Focus::Prompt] {
                app.focus = focus;
                let mut rows = help_rows(&app)
                    .into_iter()
                    .map(|(key, label, _)| format!("{key} {label}"))
                    .collect::<Vec<_>>();
                rows.push(
                    status_segments(&app, 600)
                        .iter()
                        .map(|h| format!("{} {}", h.key, h.label))
                        .collect::<Vec<_>>()
                        .join("  "),
                );
                for row in rows {
                    let unicode = row.chars().any(|c| "⇥⏎⌥⌃⇧⌫↑↓←→".contains(c));
                    let ascii = row.contains("Ctrl-")
                        || row.contains("Alt-")
                        || row.contains("Shift-")
                        || row.contains('^');
                    assert!(!(unicode && ascii), "mixed register: {row}");
                    if depth == ColorDepth::None {
                        assert!(!unicode, "ASCII register: {row}");
                    }
                    assert!(row.is_ascii(), "hints must use ASCII at every depth: {row}");
                }
            }
        }
    }
    #[test]
    fn ambiguous_terminal_glyphs_use_the_cli_single_cell_convention() {
        for glyph in ["●", "◐", "○", "◆"] {
            assert_eq!(text::width(glyph), 1);
            assert_eq!(
                unicode_width::UnicodeWidthChar::width(glyph.chars().next().unwrap()),
                Some(1)
            );
        }
    }
    #[test]
    fn list_tab_preserves_the_tail_of_a_long_project_path() {
        let mut app = scene(palette(ColorDepth::TrueColor, Theme::Dark));
        app.scope = Scope::Bucket {
            cwd: "/home/test/work/a-very-long-parent-directory/你好项目".into(),
            label: "~/work/a-very-long-parent-directory/你好项目".into(),
        };
        let (buffer, _, _) = screenshot(&mut app);
        let mut shown = String::new();
        let mut x = 0;
        while x < 34 {
            let symbol = buffer[(x, 1)].symbol();
            shown.push_str(symbol);
            x += text::width(symbol).max(1) as u16;
        }
        let shown = shown.trim_end();
        assert!(shown.starts_with(" sessions · …"), "{shown}");
        assert!(shown.ends_with("/你好项目"), "{shown}");
        assert!(text::width(shown) <= 34);
    }
    #[test]
    fn grapheme_fitting_and_prompt_cursor_match_the_test_backend() {
        for (value, width, expected) in [
            ("👩‍💻ab", 3, "👩‍💻…"),
            ("你好世界", 5, "你好…"),
            ("e\u{301}xy", 2, "e\u{301}…"),
            ("🇷🇴ab", 3, "🇷🇴…"),
        ] {
            let fitted = fit(Line::raw(value), width);
            assert_eq!(fitted.to_string(), expected);
            assert!(fitted.width() <= usize::from(width));
            let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
            terminal
                .draw(|frame| frame.render_widget(Paragraph::new(fitted), frame.area()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(
                buffer[(0, 0)].symbol(),
                value.graphemes(true).next().unwrap()
            );
            assert_eq!(buffer[(width - 1, 0)].symbol(), "…");
        }
        let mut app = scene(palette(ColorDepth::TrueColor, Theme::Dark));
        app.size = (60, 20);
        app.prompt.clear();
        app.prompt.notice = None;
        app.prompt.insert("👩‍💻你好世界e\u{301}");
        let (buffer, visible, cursor) = screenshot(&mut app);
        let area = layout::areas(&app).input;
        let prefix = text::width("e27c0de ❯ ") as u16;
        assert!(visible);
        assert_eq!(buffer[(prefix, area.y)].symbol(), "👩‍💻");
        assert_eq!(buffer[(prefix + 2, area.y)].symbol(), "你");
        assert_eq!(cursor.x, prefix + 11);
        assert_eq!(cursor.y, area.y);
    }
    #[test]
    fn view_and_transcript_cache_use_the_supplied_timezone() {
        let mut app = scene(palette(ColorDepth::TrueColor, Theme::Dark));
        app.timezone = jiff::tz::TimeZone::fixed(jiff::tz::Offset::from_hours(9).unwrap());
        let timestamp: jiff::Timestamp = "2026-10-30T23:30:00Z".parse().unwrap();
        app.wall = "2026-11-10T00:00:00Z"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .into();
        app.list.rows[0].title.clear();
        app.list.rows[0].first_message = None;
        app.list.rows[0].updated = timestamp.into();
        assert_eq!(
            session_title(&app.list.rows[0], &app.timezone),
            "untitled · 08:30"
        );
        let r#gen = app.transcript.r#gen;
        update::update(
            &mut app,
            Msg::Follow {
                r#gen,
                ev: FollowMsg::Opened {
                    header: Header {
                        title: "".into(),
                        handle: "e27c0de".into(),
                        model: None,
                    },
                    items: vec![Item::User {
                        id: "timezone".into(),
                        timestamp: Some(timestamp.to_string()),
                        text: "timezone body".into(),
                        images: 0,
                    }],
                    dropped: 0,
                    not_yet_persisted: false,
                },
            },
        );
        let cached = app
            .transcript
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(cached.contains("08:30"));
        assert!(!cached.contains("23:30"));
        app.size = (100, 40);
        assert_ne!(app.transcript.width, layout::transcript_width(&app));
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let shown = region_text(buffer, buffer.area).join("\n");
        assert!(shown.contains("untitled · 08:30"));
        assert!(shown.contains("2026-10-31"));
        assert!(shown.contains("❯ user · 08:30"));
        assert!(!shown.contains("23:30"));
    }
    #[test]
    fn ascii_key_spellings_and_pane_hint_priority_are_depth_independent() {
        for depth in [
            ColorDepth::None,
            ColorDepth::Ansi16,
            ColorDepth::Ansi256,
            ColorDepth::TrueColor,
        ] {
            for (code, modifiers, expected) in [
                (KeyCode::Tab, KeyModifiers::NONE, "tab"),
                (KeyCode::Enter, KeyModifiers::NONE, "enter"),
                (KeyCode::Esc, KeyModifiers::NONE, "esc"),
                (KeyCode::Enter, KeyModifiers::ALT, "alt-enter"),
                (KeyCode::Char('t'), KeyModifiers::CONTROL, "^t"),
                (KeyCode::Char('c'), KeyModifiers::CONTROL, "^c"),
                (KeyCode::Char('x'), KeyModifiers::CONTROL, "^x"),
            ] {
                assert_eq!(
                    key_label(
                        KeyBinding {
                            code,
                            modifiers,
                            label: "test"
                        },
                        depth
                    ),
                    expected
                );
            }
            let mut app = scene(palette(depth, Theme::Dark));
            app.toast = None;
            for focus in [Focus::Sessions, Focus::Transcript] {
                app.focus = focus;
                let hints = all_status_hints(&app);
                assert_eq!(
                    hints[..3]
                        .iter()
                        .map(|hint| hint.action)
                        .collect::<Vec<_>>(),
                    [Action::NextFocus, Action::Help, Action::Quit]
                );
                let narrow = status_segments(&app, 40);
                assert!(narrow.iter().any(|hint| hint.action == Action::Help));
                assert!(narrow.iter().any(|hint| hint.action == Action::Quit));
            }
        }
    }
    fn region_text(buffer: &Buffer, area: Rect) -> Vec<String> {
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .into()
            })
            .collect()
    }
    #[test]
    fn empty_states_use_the_table_text_and_the_owned_home_path() {
        let mut app = App::new(
            palette(ColorDepth::TrueColor, Theme::Dark),
            Scope::Bucket {
                cwd: "/home/test/work/gamma".into(),
                label: "gamma".into(),
            },
            (120, 40),
            Instant::now(),
            SystemTime::UNIX_EPOCH,
        );
        app.home = Some("/home/test".into());
        app.list.loaded = true;
        app.list.sessions_dir_missing = Some("/home/test/ae/sessions".into());
        let (buffer, _, _) = screenshot(&mut app);
        let areas = layout::areas(&app);
        let lines = region_text(&buffer, areas.list.unwrap());
        assert_eq!(
            &lines[..2],
            ["no omp sessions yet", "(~/ae/sessions does not exist)"]
        );
        let lines = region_text(&buffer, layout::inner(areas.transcript.unwrap()));
        assert_eq!(&lines[..2], ["select a session", "↑↓ choose · enter write"]);
        assert!(
            region_text(&buffer, areas.input)
                .join("\n")
                .contains("select a session to write to it")
        );
        app.list.sessions_dir_missing = None;
        app.list.total_all = 7;
        let (buffer, _, _) = screenshot(&mut app);
        let lines = region_text(&buffer, layout::areas(&app).list.unwrap());
        assert_eq!(lines[0], "no omp sessions for ~/work/gamma");
        assert!(lines[1].starts_with("a shows 7 sessions in other"));
        assert!(lines[1].ends_with('…'));
        app.list.total_all = 0;
        let (buffer, _, _) = screenshot(&mut app);
        assert!(region_text(&buffer, layout::areas(&app).list.unwrap())[1].is_empty());
    }
    #[test]
    fn transcript_interim_states_preserve_error_prose_style_and_hint() {
        let mut app = scene(palette(ColorDepth::TrueColor, Theme::Dark));
        let strings = |lines: Vec<Line<'static>>| {
            lines
                .into_iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
        };
        app.transcript.status = TranscriptStatus::Ready;
        assert_eq!(
            strings(empty_transcript_lines(&app, 100)),
            ["no messages yet"]
        );
        app.transcript.status = TranscriptStatus::Opening;
        assert_eq!(
            strings(empty_transcript_lines(&app, 100)),
            [format!("{} reading e27c0de", theme::spinner(app.frame))]
        );
        app.transcript.status = TranscriptStatus::NotYetPersisted;
        assert_eq!(
            strings(empty_transcript_lines(&app, 100)),
            [
                "e27c0de is live but has not written a transcript yet",
                "send a message to start it"
            ]
        );
        let message = "session file disappeared or was replaced: /fixtures/A.jsonl";
        app.transcript.status = TranscriptStatus::Failed(
            crate::error::GlenError::new(crate::error::ErrorCode::NotFound, message)
                .with_hint("select another session")
                .into(),
        );
        let lines = empty_transcript_lines(&app, 100);
        assert_eq!(lines[0].spans[0].style, app.theme.style(Token::Danger));
        assert_eq!(lines[1].spans[0].style, app.theme.style(Token::Muted));
        assert_eq!(
            strings(lines),
            [format!("✗ {message}"), "  select another session".into()]
        );
        app.transcript.dropped = 0;
        app.transcript.blocks.clear();
        app.transcript.lines.clear();
        let (buffer, _, _) = screenshot(&mut app);
        let area = layout::inner(layout::areas(&app).transcript.unwrap());
        assert_eq!(
            buffer[(area.x, area.y)].fg,
            app.theme.style(Token::Danger).fg.unwrap()
        );
    }
    #[test]
    fn follow_failure_overrides_cached_and_reflowed_transcript_content() {
        for width in [60, 120] {
            for following in [false, true] {
                let mut app = scene(palette(ColorDepth::TrueColor, Theme::Dark));
                prepare(&mut app);
                assert!(!app.transcript.lines.is_empty());
                assert!(!app.transcript.blocks.is_empty());
                let cached = app.transcript.lines.clone();
                app.size.0 = width;
                app.transcript.scroll.follow = following;
                app.transcript.scroll.unseen = 3;
                let r#gen = app.transcript.r#gen;
                update::update(
                    &mut app,
                    Msg::Follow {
                        r#gen,
                        ev: FollowMsg::Failed(
                            GlenError::new(
                                ErrorCode::NotFound,
                                "session file disappeared or was replaced: /fixtures/A.jsonl",
                            )
                            .with_hint("select another session")
                            .into(),
                        ),
                    },
                );
                assert_eq!(app.transcript.lines, cached);
                // Draw without preparing: exercise both the cached-width and reflow paths.
                let mut terminal = Terminal::new(TestBackend::new(width, app.size.1)).unwrap();
                terminal.draw(|frame| draw(frame, &app)).unwrap();
                let area = layout::inner(layout::areas(&app).transcript.unwrap());
                let buffer = terminal.backend().buffer();
                let lines = region_text(buffer, area);
                assert!(lines[0].starts_with("✗ session file disappeared or was replaced:"));
                let shown = lines.join("\n");
                assert!(shown.contains("select another session"));
                assert!(!shown.contains("Message"));
                assert!(!shown.contains("earlier items"));
                assert!(!shown.contains("G follows"));
                let danger = app.theme.style(Token::Danger).fg.unwrap();
                for x in area.x
                    ..area.x + text::width("✗ session file disappeared or was replaced:") as u16
                {
                    assert_eq!(buffer[(x, area.y)].fg, danger);
                }
            }
        }
    }
    #[test]
    fn submit_refusal_is_inline_only_and_stays_dismissed() {
        let mut app = scene(palette(ColorDepth::TrueColor, Theme::Dark));
        app.jobs.rows.clear();
        app.inbox.records.clear();
        let error: ErrView = GlenError::new(ErrorCode::NoRoute, "no route for this session")
            .with_hint("install the extension")
            .into();
        app.sends[0].phase = SendPhase::Refused(error.clone());
        app.prompt.notice = Some(error);
        assert!(activity_lines(&app).is_empty());
        let (buffer, _, _) = screenshot(&mut app);
        let areas = layout::areas(&app);
        assert_eq!(areas.strip.height, 0);
        let lines = region_text(&buffer, areas.notice);
        assert_eq!(
            lines,
            ["✗ no route for this session", "  install the extension"]
        );
        let shown = region_text(&buffer, buffer.area).join("\n");
        assert_eq!(shown.matches("no route for this session").count(), 1);
        update::update(
            &mut app,
            Msg::Key(ratatui::crossterm::event::KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            )),
        );
        assert!(app.prompt.notice.is_none());
        assert!(activity_lines(&app).is_empty());
        let (buffer, _, _) = screenshot(&mut app);
        assert!(
            !region_text(&buffer, buffer.area)
                .join("\n")
                .contains("no route for this session")
        );
    }
    #[test]
    fn prompt_and_inbox_empty_placeholders_are_visible_and_muted() {
        let mut app = scene(palette(ColorDepth::TrueColor, Theme::Dark));
        app.size = (60, 20);
        app.prompt.text.clear();
        app.prompt.cursor = 0;
        for (focus, expected) in [
            (Focus::Prompt, "message e27c0de · enter send"),
            (Focus::Sessions, "tab to write to e27c0de"),
        ] {
            app.focus = focus;
            let (buffer, cursor, position) = screenshot(&mut app);
            let area = layout::areas(&app).input;
            let lines = region_text(&buffer, area);
            assert_eq!(area.height, 2);
            assert_eq!(lines[0], expected);
            assert_eq!(lines[1], "e27c0de ❯");
            assert!(app.prompt.text.is_empty());
            assert_eq!(app.prompt.cursor, 0);
            assert_eq!(cursor, focus == Focus::Prompt);
            if cursor {
                assert_eq!(position.y, area.y + 1);
                assert_eq!(position.x, area.x + text::width("e27c0de ❯ ") as u16);
            }
            assert_eq!(
                buffer[(area.x + 10, area.y)].fg,
                app.theme.style(Token::Muted).fg.unwrap()
            );
        }
        app.inbox.records.clear();
        app.overlay = Some(Overlay::Inbox);
        let (buffer, _, _) = screenshot(&mut app);
        assert!(
            (0..buffer.area.height)
                .any(|y| row_text(&buffer, y).contains("no messages from omp yet"))
        );
    }
    #[test]
    fn help_covers_every_context_binding_without_truncation_and_scrolls() {
        let mut app = scene(palette(ColorDepth::TrueColor, Theme::Dark));
        app.size = (140, 70);
        app.overlay = Some(Overlay::Help);
        let mut required = vec![];
        for overlay in [
            OverlayCtx::None,
            OverlayCtx::Help,
            OverlayCtx::Inbox,
            OverlayCtx::Confirm,
            OverlayCtx::Picker,
        ] {
            for focus in [Focus::Sessions, Focus::Transcript, Focus::Prompt] {
                for (binding, action, label) in keymap::bindings(KeymapCtx { overlay, focus }) {
                    required.push((key_label(*binding, app.theme.depth), *action, *label));
                }
            }
        }
        for focus in [Focus::Sessions, Focus::Transcript, Focus::Prompt] {
            app.focus = focus;
            let rows = help_rows(&app);
            let (buffer, _, _) = screenshot(&mut app);
            let shown = (0..buffer.area.height)
                .map(|y| row_text(&buffer, y))
                .collect::<Vec<_>>()
                .join("\n");
            for (key, action, label) in &required {
                assert!(
                    rows.iter()
                        .any(|(keys, text, _)| keys.split(" / ").any(|k| k == key)
                            && text.starts_with(label)),
                    "missing {action:?}: {key} {label}"
                );
                assert!(shown.contains(key), "key absent from rendered help: {key}");
                assert!(
                    shown.contains(label),
                    "action absent from rendered help: {action:?} {label}"
                );
            }
            for (key, label, unavailable) in rows {
                if unavailable {
                    assert!(
                        shown
                            .lines()
                            .any(|line| line.contains(&key) && line.contains(&label)),
                        "truncated unavailable control: {key} {label}"
                    );
                }
            }
        }
        let narrow_lines = help_lines(&app, 38)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            narrow_lines
                .iter()
                .filter(|line| line.contains("(not in this build)"))
                .count(),
            help_rows(&app)
                .iter()
                .filter(|(_, _, unavailable)| *unavailable)
                .count(),
            "every unavailable control keeps its full suffix at narrow widths"
        );
        for (keys, _, unavailable) in help_rows(&app) {
            if !keys.is_empty() {
                assert!(
                    narrow_lines.iter().any(|line| line.starts_with(&keys)),
                    "truncated aliases: {keys}"
                );
            }
            if unavailable {
                assert!(
                    narrow_lines
                        .iter()
                        .any(|line| line.contains("(not in this build)"))
                );
            }
        }
        app.size = (80, 24);
        app.help_scroll = 0;
        let (before, _, _) = screenshot(&mut app);
        let key = |code| {
            Msg::Key(ratatui::crossterm::event::KeyEvent::new(
                code,
                KeyModifiers::NONE,
            ))
        };
        assert!(update::update(&mut app, key(KeyCode::PageDown)).is_empty());
        assert!(app.help_scroll > 0);
        let (after, _, _) = screenshot(&mut app);
        assert_ne!(before, after);
        update::update(&mut app, key(KeyCode::End));
        let (_, limit) = help_scroll_bounds(&app);
        assert_eq!(app.help_scroll, limit);
        update::update(&mut app, key(KeyCode::Down));
        assert_eq!(app.help_scroll, limit);
        update::update(&mut app, key(KeyCode::Home));
        assert_eq!(app.help_scroll, 0);
        update::update(&mut app, key(KeyCode::Esc));
        assert!(app.overlay.is_none());
    }
    #[test]
    fn too_small_reports_the_full_current_dimensions_and_nothing_else() {
        let mut app = scene(palette(ColorDepth::None, Theme::Dark));
        for size in [(60, 9), (30, 8), (39, 10)] {
            app.size = size;
            let (buffer, cursor, _) = screenshot(&mut app);
            let shown = (0..size.1)
                .map(|y| row_text(&buffer, y))
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(
                shown,
                format!(
                    "glen watch needs at least 40×10 · now {}×{}",
                    size.0, size.1
                )
            );
            assert!(!cursor);
        }
    }
    #[test]
    fn cursor_tracks_wrapped_prompt_only_without_an_overlay() {
        let mut app = scene(palette(ColorDepth::None, Theme::Dark));
        app.size = (60, 30);
        let (buffer, visible, position) = screenshot(&mut app);
        assert!(visible);
        let a = layout::areas(&app);
        assert!(a.input.contains(position));
        assert!(row_text(&buffer, a.input.y).contains("e27c0de ❯"));
        app.overlay = Some(Overlay::Help);
        assert!(!screenshot(&mut app).1);
        app.overlay = None;
        app.focus = Focus::Sessions;
        assert!(!screenshot(&mut app).1);
        app.focus = Focus::Prompt;
        app.size = (39, 30);
        assert!(!screenshot(&mut app).1);
    }
    #[test]
    fn readonly_draw_reflows_without_mutation_and_none_discards_stale_colour() {
        let mut app = scene(palette(ColorDepth::TrueColor, Theme::Dark));
        prepare(&mut app);
        let width = app.transcript.width;
        let old = app.transcript.lines.clone();
        app.theme.depth = ColorDepth::None;
        let mut same_width = Terminal::new(TestBackend::new(app.size.0, app.size.1)).unwrap();
        same_width.draw(|f| draw(f, &app)).unwrap();
        assert_styles(same_width.backend().buffer(), ColorDepth::None);
        app.size = (80, 30);
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        assert_eq!(app.transcript.width, width);
        assert_eq!(app.transcript.lines, old);
        assert_styles(terminal.backend().buffer(), ColorDepth::None);
        prepare(&mut app);
        assert_eq!(app.transcript.width, layout::transcript_width(&app));
        assert_eq!(app.transcript.height, layout::viewport(&app).1.max(1));
    }
    #[test]
    fn title_fallback_strip_and_visible_frame_have_the_expected_content() {
        let mut app = scene(palette(ColorDepth::None, Theme::Dark));
        app.size = (160, 40);
        app.focus = Focus::Sessions;
        app.prompt.notice = None;
        app.prompt.clear();
        app.toast = None;
        app.list.rows[1].title.clear();
        app.list.rows[1].first_message = Some("First message\nwith control \x1b".into());
        assert_eq!(
            session_title(&app.list.rows[1], &app.timezone),
            r"First message with control \x1b"
        );
        app.list.rows[1].first_message = None;
        assert!(session_title(&app.list.rows[1], &app.timezone).starts_with("untitled · "));
        let (buffer, _, _) = screenshot(&mut app);
        assert!(row_text(&buffer, 0).starts_with("glen watch"));
        assert!(row_text(&buffer, 0).contains("8 sessions"));
        assert!(row_text(&buffer, 1).contains("model-test"));
        let activity = activity_lines(&app)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert!(
            activity
                .iter()
                .any(|s| s.contains("sending \"A longer prompt"))
        );
        assert!(
            activity
                .iter()
                .any(|s| s.contains("j1234567 · waiting for the reply"))
        );
        assert!(activity.iter().any(|s| s.starts_with("↩ ")));
    }
    #[test]
    fn inbox_is_peek_only_and_help_marks_controls_unavailable() {
        let mut app = scene(palette(ColorDepth::TrueColor, Theme::Dark));
        app.size = (160, 50);
        app.overlay = Some(Overlay::Inbox);
        let before = app
            .inbox
            .records
            .iter()
            .map(|r| r.unread)
            .collect::<Vec<_>>();
        let (buffer, _, _) = screenshot(&mut app);
        let dump = (0..50)
            .map(|y| row_text(&buffer, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(dump.contains("peek only · glen inbox marks these read"));
        assert!(dump.contains("↩ e27c0de"));
        assert_eq!(
            before,
            app.inbox
                .records
                .iter()
                .map(|r| r.unread)
                .collect::<Vec<_>>()
        );
        app.overlay = Some(Overlay::Help);
        let (buffer, _, _) = screenshot(&mut app);
        let dump = (0..50)
            .map(|y| row_text(&buffer, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(dump.contains("not in this build"));
    }
    #[test]
    fn picker_filters_selects_the_filtered_op_and_toasts_expire_at_three_seconds() {
        let mut app = scene(palette(ColorDepth::None, Theme::Dark));
        app.caps.controls = true;
        app.overlay = Some(picker(&app));
        for c in "other".chars() {
            update::update(
                &mut app,
                Msg::Key(ratatui::crossterm::event::KeyEvent::new(
                    KeyCode::Char(c),
                    KeyModifiers::NONE,
                )),
            );
        }
        let Some(Overlay::Picker(p)) = &app.overlay else {
            panic!("picker")
        };
        assert_eq!(p.filter, "other");
        assert_eq!(p.visible_entries().len(), 1);
        let cmds = update::update(
            &mut app,
            Msg::Key(ratatui::crossterm::event::KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            )),
        );
        assert!(
            matches!(&cmds[..],[Cmd::Control{op:ControlOp::SetThinking(level),..}] if level=="high")
        );
        app.pending_control = None;
        app.caps.controls = false;
        update::update(
            &mut app,
            Msg::Key(ratatui::crossterm::event::KeyEvent::new(
                KeyCode::Char('x'),
                KeyModifiers::CONTROL,
            )),
        );
        let now = app.now;
        let wall = app.wall;
        assert_eq!(
            app.toast.as_ref().unwrap().until,
            now + Duration::from_secs(3)
        );
        update::update(
            &mut app,
            Msg::Tick {
                now: now + Duration::from_millis(2999),
                wall,
            },
        );
        assert!(app.toast.is_some());
        update::update(
            &mut app,
            Msg::Tick {
                now: now + Duration::from_secs(3),
                wall,
            },
        );
        assert!(app.toast.is_none());
    }
}

#[cfg(test)]
mod refinements {
    use super::*;
    use crate::{
        error::{ErrorCode, GlenError},
        render::palette::Theme,
        tui::theme::TuiTheme,
    };
    use ratatui::{Terminal, backend::TestBackend};
    use std::time::Duration;
    #[test]
    fn newer_submit_stays_last_and_a_failure_hint_never_replaces_its_headline() {
        let mut app = super::tests::scene(TuiTheme {
            depth: ColorDepth::None,
            theme: Theme::Dark,
        });
        let lines = visible_activity(&app, 3)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert!(lines.last().unwrap().contains("sending \""));
        // Refused submits belong inline; failed jobs still need a strip headline and hint.
        app.sends.clear();
        app.jobs.rows[0].status.status = JobState::Failed;
        app.jobs.rows[0].finished_at = Some(app.now);
        app.jobs.rows[0].done = Some(Done::Failed(
            GlenError::new(ErrorCode::NoRoute, "no route")
                .with_hint("a hint")
                .into(),
        ));
        let lines = visible_activity(&app, 1)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(lines, ["✗ no route"]);
        let lines = visible_activity(&app, 2)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(lines, ["✗ no route", "  a hint"]);
        app.jobs.rows[0].finished_at = Some(app.now - Duration::from_secs(200));
        assert!(
            !visible_activity(&app, 1)[0]
                .to_string()
                .contains("no route")
        );
    }
    #[test]
    fn unavailable_controls_keep_their_suffix_at_medium_widths() {
        let mut app = super::tests::scene(TuiTheme {
            depth: ColorDepth::None,
            theme: Theme::Dark,
        });
        app.size = (72, 50);
        app.overlay = Some(Overlay::Help);
        let mut terminal = Terminal::new(TestBackend::new(72, 50)).unwrap();
        terminal.draw(|f| render(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let a = layout::areas(&app);
        let pane = a.transcript.unwrap();
        let lines = (pane.y..pane.bottom())
            .map(|y| {
                (pane.x..pane.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(lines.iter().any(|s| s.contains("(not in this build)")));
    }
    #[test]
    fn picker_filter_paste_backspace_and_repeat_remain_isolated_from_the_draft() {
        use ratatui::crossterm::event::{KeyEvent, KeyEventKind};
        let mut app = super::tests::scene(TuiTheme {
            depth: ColorDepth::None,
            theme: Theme::Dark,
        });
        let original = app.prompt.text.clone();
        app.overlay = Some(Overlay::Picker(Picker {
            filter: String::new(),
            kind: PickerKind::Thinking,
            entries: vec![
                PickerEntry {
                    label: "high".into(),
                    op: ControlOp::SetThinking("high".into()),
                },
                PickerEntry {
                    label: "low".into(),
                    op: ControlOp::SetThinking("low".into()),
                },
            ],
            selected: 0,
            target: app.transcript.target.clone().unwrap(),
        }));
        update::update(&mut app, Msg::Paste("HI".into()));
        let mut key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Repeat;
        update::update(&mut app, Msg::Key(key));
        let Some(Overlay::Picker(p)) = &app.overlay else {
            panic!("picker")
        };
        assert_eq!(p.filter, "HI");
        assert_eq!(p.visible_entries().len(), 1);
        update::update(
            &mut app,
            Msg::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
        );
        let Some(Overlay::Picker(p)) = &app.overlay else {
            panic!("picker")
        };
        assert_eq!(p.filter, "H");
        assert_eq!(app.prompt.text, original);
        update::update(&mut app, Msg::Paste("no-match".into()));
        let cmds = update::update(
            &mut app,
            Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        );
        assert!(cmds.is_empty());
        assert!(matches!(app.overlay, Some(Overlay::Picker(_))));
    }
}

#[cfg(test)]
mod confirm_tests {
    use super::*;
    use crate::{render::palette::Theme, tui::theme::TuiTheme};
    #[test]
    fn minimum_width_keeps_the_target_and_both_confirmation_keys_visible() {
        let app = super::tests::scene(TuiTheme {
            depth: ColorDepth::None,
            theme: Theme::Dark,
        });
        for op in [ControlOp::Abort, ControlOp::Compact] {
            let line = confirmation(&app, &op, 40);
            assert!(text::width(&line) <= 40);
            assert!(line.contains("e27c0de"));
            assert!(line.contains("y "));
            assert!(line.contains("n cancel"));
        }
        assert_eq!(
            confirmation(&app, &ControlOp::Abort, 80),
            "stop the running turn in e27c0de?  y stop  n cancel"
        );
    }
}

#[cfg(test)]
mod snapshots {
    use crate::{
        render::palette::{ColorDepth, Theme},
        tui::{
            testkit::{self, SceneKind},
            theme::TuiTheme,
        },
    };
    fn pin(name: &str, kind: SceneKind, size: (u16, u16), theme: TuiTheme) {
        let value = testkit::render(testkit::scene(kind, size, theme));
        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_path("snapshots");
        settings.set_prepend_module_to_snapshot(false);
        settings.set_omit_expression(true);
        settings.set_description(format!(
            "{} · {}x{} · {:?} {:?}",
            kind.name(),
            size.0,
            size.1,
            theme.theme,
            theme.depth
        ));
        settings.bind(|| insta::assert_snapshot!(name, value));
    }
    macro_rules! snapshot {
        ($name:ident, $kind:ident, $size:expr, $depth:ident, $theme:ident) => {
            #[test]
            fn $name() {
                pin(
                    stringify!($name),
                    SceneKind::$kind,
                    $size,
                    TuiTheme {
                        depth: ColorDepth::$depth,
                        theme: Theme::$theme,
                    },
                );
            }
        };
    }
    snapshot!(main_80x24_dark_truecolor, Main, (80, 24), TrueColor, Dark);
    snapshot!(main_80x24_light_truecolor, Main, (80, 24), TrueColor, Light);
    snapshot!(main_80x24_no_color, Main, (80, 24), None, Dark);
    snapshot!(main_120x40_dark_truecolor, Main, (120, 40), TrueColor, Dark);
    snapshot!(
        main_120x40_light_truecolor,
        Main,
        (120, 40),
        TrueColor,
        Light
    );
    snapshot!(main_120x40_no_color, Main, (120, 40), None, Dark);
    snapshot!(main_60x20_dark_truecolor, Main, (60, 20), TrueColor, Dark);
    snapshot!(main_60x20_light_truecolor, Main, (60, 20), TrueColor, Light);
    snapshot!(main_60x20_no_color, Main, (60, 20), None, Dark);
    snapshot!(main_80x24_dark_ansi256, Main, (80, 24), Ansi256, Dark);
    snapshot!(main_80x24_dark_ansi16, Main, (80, 24), Ansi16, Dark);
    snapshot!(state_empty_scope, EmptyScope, (80, 24), TrueColor, Dark);
    snapshot!(
        state_sessions_dir_missing,
        SessionsDirMissing,
        (80, 24),
        TrueColor,
        Dark
    );
    snapshot!(state_list_loading, ListLoading, (80, 24), TrueColor, Dark);
    snapshot!(state_opening, Opening, (80, 24), TrueColor, Dark);
    snapshot!(
        state_not_yet_persisted,
        NotYetPersisted,
        (80, 24),
        TrueColor,
        Dark
    );
    snapshot!(state_open_failed, OpenFailed, (80, 24), TrueColor, Dark);
    snapshot!(
        state_no_route_inline,
        NoRouteInline,
        (80, 24),
        TrueColor,
        Dark
    );
    snapshot!(
        state_oversize_message,
        OversizeMessage,
        (80, 24),
        TrueColor,
        Dark
    );
    snapshot!(state_sending, Sending, (80, 24), TrueColor, Dark);
    snapshot!(
        state_job_completed_replies,
        JobCompletedReplies,
        (80, 24),
        TrueColor,
        Dark
    );
    snapshot!(
        state_job_failed_no_route,
        JobFailedNoRoute,
        (80, 24),
        TrueColor,
        Dark
    );
    snapshot!(state_job_forked, JobForked, (80, 24), TrueColor, Dark);
    snapshot!(state_inbox_overlay, InboxOverlay, (80, 24), TrueColor, Dark);
    snapshot!(
        state_help_controls_off,
        HelpControlsOff,
        (80, 24),
        TrueColor,
        Dark
    );
    snapshot!(
        state_help_controls_on,
        HelpControlsOn,
        (80, 24),
        TrueColor,
        Dark
    );
    snapshot!(state_confirm_abort, ConfirmAbort, (80, 24), TrueColor, Dark);
    snapshot!(state_model_picker, ModelPicker, (80, 24), TrueColor, Dark);
    snapshot!(
        state_scrolled_with_new,
        ScrolledWithNew,
        (80, 24),
        TrueColor,
        Dark
    );
    snapshot!(
        state_narrow_60x20_transcript_tab,
        NarrowTranscript,
        (60, 20),
        TrueColor,
        Dark
    );
    snapshot!(state_too_small_39x9, TooSmall, (39, 9), TrueColor, Dark);
    snapshot!(
        state_untrusted_title,
        UntrustedTitle,
        (80, 24),
        TrueColor,
        Dark
    );
    snapshot!(
        state_multiline_prompt,
        MultilinePrompt,
        (80, 24),
        TrueColor,
        Dark
    );
    #[test]
    fn every_state_is_unstyled_in_both_no_color_themes_and_deterministic() {
        for kind in std::iter::once(SceneKind::Main).chain(SceneKind::STATES) {
            for theme in [Theme::Dark, Theme::Light] {
                let theme = TuiTheme {
                    depth: ColorDepth::None,
                    theme,
                };
                let first = testkit::render(testkit::scene(kind, kind.size(), theme));
                assert_eq!(
                    first,
                    testkit::render(testkit::scene(kind, kind.size(), theme)),
                    "non-deterministic {}",
                    kind.name()
                );
            }
        }
    }
}
