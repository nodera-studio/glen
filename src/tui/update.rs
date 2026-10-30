//! Pure state transitions. Commands describe effects; workers execute them later.
use super::{
    ansi,
    keymap::{self, Action, KeymapCtx},
    prompt::Draft,
    state::*,
};
use crate::{
    cli::DeliverAs,
    error::{ErrorCode, GlenError},
    render::{self, palette::Token, text, transcript},
};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    text::Line,
};
use std::time::Duration;

pub fn update(app: &mut App, msg: Msg) -> Vec<Cmd> {
    if app.exit.is_some() {
        return vec![];
    }
    app.dirty |= match &msg {
        Msg::Tick { .. } => false,
        Msg::Key(key) => key.kind != KeyEventKind::Release,
        Msg::Resize(w, h) => app.size != (*w, *h),
        Msg::Follow { r#gen, .. } => *r#gen == app.transcript.r#gen,
        _ => true,
    };
    let commands = match msg {
        Msg::SourceError { source, error } => {
            toast(
                app,
                format!(
                    "{source}: {}{}",
                    error.message,
                    error
                        .hint
                        .map(|hint| format!(" · {hint}"))
                        .unwrap_or_default()
                ),
            );
            vec![]
        }
        Msg::InboxReset(sessions) => {
            app.inbox
                .records
                .retain(|r| !sessions.contains(&r.session_id));
            app.inbox
                .positions
                .retain(|(session, _), _| !sessions.contains(session));
            for session in sessions {
                app.inbox.cursors.remove(&session);
                app.inbox.unread_by_session.remove(&session);
            }
            app.inbox.scroll = app
                .inbox
                .scroll
                .min(app.inbox.records.len().saturating_sub(1));
            vec![]
        }
        Msg::Key(key) => key_update(app, key),
        Msg::Paste(s) => {
            if let Some(Overlay::Picker(p)) = &mut app.overlay {
                p.filter.push_str(&text::single_line(&s));
                p.selected = 0;
            } else if app.overlay.is_none() && app.focus == Focus::Prompt {
                app.prompt.insert_paste(&s);
            }
            vec![]
        }
        Msg::Resize(w, h) => {
            app.size = (w, h);
            sync_tab(app);
            vec![]
        }
        Msg::Tick { now, wall } => {
            let animates = tick_interval(app) == Duration::from_millis(80) || !app.list.loaded;
            let age_changed = app.list.rows.iter().any(|row| {
                text::age_in(app.wall, row.updated, &app.timezone)
                    != text::age_in(wall, row.updated, &app.timezone)
            }) || matches!(app.overlay, Some(Overlay::Inbox))
                && app.inbox.records.iter().any(|record| {
                    record.at.parse::<jiff::Timestamp>().ok().is_some_and(|at| {
                        text::age_in(app.wall, at.into(), &app.timezone)
                            != text::age_in(wall, at.into(), &app.timezone)
                    })
                });
            app.now = now;
            app.wall = wall;
            app.frame = app.frame.wrapping_add(1);
            if app.toast.as_ref().is_some_and(|t| now >= t.until) {
                app.toast = None;
                app.dirty = true;
            }
            let rows = app.jobs.rows.len();
            expire_jobs(app, false);
            app.dirty |= animates || age_changed || rows != app.jobs.rows.len();
            vec![]
        }
        Msg::Signal(n) => quit(app, 128i32.saturating_add(n)),
        Msg::Sessions(snapshot) => sessions(app, snapshot),
        Msg::Follow { r#gen, ev } => {
            if r#gen == app.transcript.r#gen {
                followed(app, ev);
            }
            vec![]
        }
        Msg::Submitted { local, result } => {
            submitted(app, local, result);
            vec![]
        }
        Msg::Jobs(snapshot) => {
            jobs(app, snapshot);
            vec![]
        }
        Msg::Inbox(snapshot) => {
            inbox(app, snapshot);
            vec![]
        }
        Msg::ControlDone { local, op, result } => {
            control_done(app, local, op, result);
            vec![]
        }
    };
    save_draft(app);
    follow_bottom(app);
    commands
}
fn quit(app: &mut App, code: i32) -> Vec<Cmd> {
    app.exit = Some(code);
    vec![Cmd::Quit(code)]
}
fn toast(app: &mut App, s: impl Into<String>) {
    app.toast = Some(Toast {
        text: s.into(),
        until: app.now + Duration::from_secs(3),
    });
}
fn save_draft(app: &mut App) {
    if let Some(id) = &app.list.selected {
        app.drafts.insert(id.clone(), app.prompt.draft());
    }
}
fn switch_draft(app: &mut App, id: String) {
    save_draft(app);
    let draft = app.drafts.get(&id).cloned().unwrap_or_default();
    app.list.selected = Some(id);
    app.prompt.load(draft, &app.history);
}
fn sync_tab(app: &mut App) {
    match app.focus {
        Focus::Sessions => app.narrow_tab = Pane::Sessions,
        Focus::Transcript | Focus::Prompt => app.narrow_tab = Pane::Transcript,
    }
}
fn focus(app: &mut App, next: bool) {
    app.focus = match (app.focus, next) {
        (Focus::Sessions, true) | (Focus::Prompt, false) => Focus::Transcript,
        (Focus::Transcript, true) | (Focus::Sessions, false) => Focus::Prompt,
        _ => Focus::Sessions,
    };
    sync_tab(app);
}
fn selected_target(app: &App) -> Option<Target> {
    app.list
        .rows
        .iter()
        .find(|r| Some(&r.id) == app.list.selected.as_ref())
        .map(|r| Target {
            id: r.id.clone(),
            path: r.path.clone(),
        })
}
fn follow_command(app: &mut App, clear: bool) -> Vec<Cmd> {
    let Some(target) = selected_target(app).or_else(|| app.transcript.target.clone()) else {
        return vec![];
    };
    app.transcript.r#gen = app.transcript.r#gen.wrapping_add(1);
    app.transcript.status = TranscriptStatus::Opening;
    app.transcript.target = Some(target.clone());
    if clear {
        app.transcript.blocks.clear();
        app.transcript.lines.clear();
        app.transcript.starts.clear();
        app.transcript.prev_model = None;
        app.transcript.header = None;
        app.transcript.dropped = 0;
        app.transcript.scroll = Scroll {
            follow: true,
            top: 0,
            unseen: 0,
        };
        app.pending_control = None;
    }
    vec![Cmd::Follow {
        r#gen: app.transcript.r#gen,
        target,
        opts: crate::omp::items::ItemOpts {
            thinking: app.transcript.opts.thinking,
            tools: app.transcript.opts.tools,
        },
    }]
}
fn select(app: &mut App, index: usize) -> Vec<Cmd> {
    let Some(row) = app.list.rows.get(index) else {
        return vec![];
    };
    if app.list.selected.as_deref() == Some(&row.id) {
        return vec![];
    }
    switch_draft(app, row.id.clone());
    let page = app.list_page();
    if index < app.list.offset {
        app.list.offset = index;
    } else if index >= app.list.offset.saturating_add(page) {
        app.list.offset = index + 1 - page;
    }
    follow_command(app, true)
}
fn sessions(app: &mut App, snapshot: ListSnapshot) -> Vec<Cmd> {
    app.list.loaded = true;
    app.list.rows = snapshot.rows;
    app.list.total_all = snapshot.total_all;
    app.list.sessions_dir_missing = snapshot.sessions_dir_missing;
    app.list.offset = app
        .list
        .offset
        .min(app.list.rows.len().saturating_sub(app.list_page()));
    if app.list.selected.is_none() && !app.list.rows.is_empty() {
        return select(app, 0);
    }
    if let Some(index) = app
        .list
        .rows
        .iter()
        .position(|r| Some(&r.id) == app.list.selected.as_ref())
    {
        if index < app.list.offset {
            app.list.offset = index;
        } else if index >= app.list.offset + app.list_page() {
            app.list.offset = index + 1 - app.list_page();
        }
        if app.transcript.target.is_none() {
            return follow_command(app, true);
        }
    }
    vec![]
}
fn movement(app: &mut App, action: Action) -> Vec<Cmd> {
    let page = if app.focus == Focus::Sessions {
        app.list_page()
    } else {
        app.page()
    };
    if app.focus == Focus::Sessions {
        if app.list.rows.is_empty() {
            return vec![];
        }
        let current = app
            .list
            .rows
            .iter()
            .position(|r| Some(&r.id) == app.list.selected.as_ref())
            .unwrap_or(0);
        let index = match action {
            Action::Up => current.saturating_sub(1),
            Action::Down => current.saturating_add(1),
            Action::PageUp => current.saturating_sub(page),
            Action::PageDown => current.saturating_add(page),
            Action::First => 0,
            Action::Last => app.list.rows.len() - 1,
            _ => current,
        }
        .min(app.list.rows.len() - 1);
        select(app, index)
    } else {
        let bottom = app.transcript.lines.len().saturating_sub(page);
        let scroll = &mut app.transcript.scroll;
        if action == Action::Last {
            scroll.follow = true;
            scroll.unseen = 0;
            scroll.top = bottom;
            return vec![];
        }
        scroll.follow = false;
        scroll.top = match action {
            Action::Up => scroll.top.saturating_sub(1),
            Action::Down => scroll.top.saturating_add(1),
            Action::PageUp => scroll.top.saturating_sub(page),
            Action::PageDown => scroll.top.saturating_add(page),
            Action::HalfUp => scroll.top.saturating_sub((page / 2).max(1)),
            Action::HalfDown => scroll.top.saturating_add((page / 2).max(1)),
            Action::First => 0,
            _ => scroll.top,
        }
        .min(bottom);
        vec![]
    }
}
fn key_update(app: &mut App, key: KeyEvent) -> Vec<Cmd> {
    let ctx = KeymapCtx::new(app.overlay.as_ref(), app.focus);
    if let Some(action) = keymap::action(ctx, key) {
        return dispatch(app, action);
    }
    if key.kind == KeyEventKind::Release {
        return vec![];
    }
    if matches!(app.overlay, Some(Overlay::Help)) && key.modifiers.is_empty() {
        let (page, limit) = super::view::help_scroll_bounds(app);
        let top = app.help_scroll.min(limit);
        app.help_scroll = match key.code {
            KeyCode::Up | KeyCode::Char('k') => top.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => top.saturating_add(1).min(limit),
            KeyCode::PageUp => top.saturating_sub(page),
            KeyCode::PageDown => top.saturating_add(page).min(limit),
            KeyCode::Home => 0,
            KeyCode::End => limit,
            _ => top,
        };
        return vec![];
    }
    if let Some(overlay) = &mut app.overlay {
        if let Overlay::Picker(p) = overlay {
            if key.modifiers.is_empty() && matches!(key.code, KeyCode::Up | KeyCode::Down) {
                p.selected = if key.code == KeyCode::Down {
                    p.selected
                        .saturating_add(1)
                        .min(p.visible_entries().len().saturating_sub(1))
                } else {
                    p.selected.saturating_sub(1)
                };
            } else if key.kind == KeyEventKind::Press
                && !key.modifiers.intersects(
                    KeyModifiers::CONTROL
                        | KeyModifiers::ALT
                        | KeyModifiers::SUPER
                        | KeyModifiers::HYPER
                        | KeyModifiers::META,
                )
            {
                match key.code {
                    KeyCode::Char(c) if !c.is_control() => {
                        p.filter.push(c);
                        p.selected = 0;
                    }
                    KeyCode::Backspace => {
                        p.filter.pop();
                        p.selected = 0;
                    }
                    _ => (),
                }
            }
        } else if let Overlay::Inbox = overlay
            && key.modifiers.is_empty()
        {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    app.inbox.scroll = app.inbox.scroll.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    app.inbox.scroll = app
                        .inbox
                        .scroll
                        .saturating_add(1)
                        .min(app.inbox.records.len().saturating_sub(1))
                }
                _ => (),
            }
        }
        return vec![];
    }
    if app.focus == Focus::Prompt
        && key.kind == KeyEventKind::Press
        && !key.modifiers.intersects(
            KeyModifiers::CONTROL
                | KeyModifiers::ALT
                | KeyModifiers::SUPER
                | KeyModifiers::HYPER
                | KeyModifiers::META,
        )
        && let KeyCode::Char(c) = key.code
        && !c.is_control()
    {
        app.prompt.insert(&c.to_string());
    }
    vec![]
}
fn dispatch(app: &mut App, action: Action) -> Vec<Cmd> {
    use Action::*;
    match action {
        NextFocus => focus(app, true),
        PrevFocus => focus(app, false),
        Up | Down | PageUp | PageDown | HalfUp | HalfDown | First | Last => {
            return movement(app, action);
        }
        FocusPrompt => {
            if app.list.selected.is_some() {
                app.focus = Focus::Prompt;
                sync_tab(app);
            }
        }
        Thinking | Tools => {
            if action == Thinking {
                app.transcript.opts.thinking = !app.transcript.opts.thinking;
            } else {
                app.transcript.opts.tools = match app.transcript.opts.tools {
                    crate::cli::Tools::Collapsed => crate::cli::Tools::Full,
                    crate::cli::Tools::Full => crate::cli::Tools::None,
                    crate::cli::Tools::None => crate::cli::Tools::Collapsed,
                };
            }
            return follow_command(app, false);
        }
        Scope => {
            app.scope = match &app.scope {
                super::state::Scope::Bucket { .. } => {
                    app.bucket_scope = Some(app.scope.clone());
                    super::state::Scope::All
                }
                super::state::Scope::All => {
                    let Some(bucket) = &app.bucket_scope else {
                        return vec![];
                    };
                    bucket.clone()
                }
            };
            return vec![Cmd::ListScope(app.scope.clone())];
        }
        Refresh => return vec![Cmd::Refresh],
        Inbox => app.overlay = Some(Overlay::Inbox),
        Help => {
            app.help_scroll = 0;
            app.overlay = Some(Overlay::Help);
        }
        Quit => return quit(app, 0),
        Send => return submit(app),
        Newline => app.prompt.insert("\n"),
        Mode => app.prompt.cycle_mode(),
        Edit(e) => app.prompt.edit(e),
        Escape => {
            if app.overlay.take().is_none() {
                if app.prompt.notice.take().is_none() {
                    app.focus = Focus::Transcript;
                    sync_tab(app);
                }
            } else {
                app.pending_control = None;
            }
        }
        Interrupt => {
            if app.prompt.text.is_empty() {
                return quit(app, 0);
            }
            app.prompt.clear();
        }
        Eof => {
            if app.prompt.text.is_empty() {
                return quit(app, 0);
            }
        }
        Abort | Models | ThinkingLevels | Compact => return control_start(app, action),
        Cancel => {
            app.overlay = None;
            app.pending_control = None;
        }
        Confirm => {
            if let Some(Overlay::Confirm(op)) = app.overlay.take() {
                return control(app, op, None);
            }
        }
        Pick => {
            let op = if let Some(Overlay::Picker(p)) = &app.overlay {
                (app.transcript.target.as_ref() == Some(&p.target))
                    .then(|| {
                        p.visible_entries()
                            .get(p.selected)
                            .map(|entry| entry.op.clone())
                    })
                    .flatten()
            } else {
                None
            };
            if let Some(op) = op {
                app.overlay = None;
                return control(app, op, None);
            }
        }
    }
    vec![]
}
fn submit(app: &mut App) -> Vec<Cmd> {
    if app.prompt.text.trim().is_empty() || app.prompt.locked.is_some() {
        return vec![];
    }
    let Some(target) = selected_target(app) else {
        return vec![];
    };
    if app
        .sends
        .iter()
        .any(|s| s.session_id == target.id && matches!(s.phase, SendPhase::Routing))
    {
        return vec![];
    }
    if app.prompt.text.len() > 524_288 {
        app.prompt.notice =
            Some(GlenError::new(ErrorCode::Usage, "message is larger than 512 KiB").into());
        return vec![];
    }
    expire_jobs(app, true);
    app.sends
        .retain(|s| !matches!(s.phase, SendPhase::Refused(_)));
    let local = app.local();
    let text = app.prompt.text.clone();
    let mode = app.prompt.mode;
    let handle = app
        .list
        .rows
        .iter()
        .find(|r| r.id == target.id)
        .map(|r| r.handle.clone())
        .unwrap_or_default();
    app.sends.push(SendTrack {
        local,
        session_id: target.id.clone(),
        handle,
        text: text.clone(),
        mode,
        phase: SendPhase::Routing,
        started: app.now,
    });
    app.prompt.locked = Some(local);
    app.prompt.notice = None;
    vec![Cmd::Submit {
        local,
        target,
        text,
        mode,
    }]
}
fn submitted(app: &mut App, local: u64, result: Result<JobRef, ErrView>) {
    let Some(index) = app
        .sends
        .iter()
        .position(|s| s.local == local && matches!(s.phase, SendPhase::Routing))
    else {
        return;
    };
    save_draft(app);
    let track = &mut app.sends[index];
    let draft = app
        .drafts
        .entry(track.session_id.clone())
        .or_insert_with(|| Draft {
            text: track.text.clone(),
            cursor: track.text.len(),
            mode: track.mode,
            locked: Some(local),
            notice: None,
        });
    if draft.locked != Some(local) {
        return;
    }
    draft.locked = None;
    match result {
        Ok(job) => {
            draft.text.clear();
            draft.cursor = 0;
            draft.mode = DeliverAs::Auto;
            draft.notice = None;
            app.history.push(track.text.clone());
            track.phase = SendPhase::Job {
                id: job.id,
                handle: job.handle,
            };
        }
        Err(err) => {
            draft.notice = Some(err.clone());
            track.phase = SendPhase::Refused(err);
        }
    }
    app.prompt.history = app.history.clone();
    if app.list.selected.as_deref() == Some(&track.session_id) {
        app.prompt.load(draft.clone(), &app.history);
    }
}
fn control_start(app: &mut App, action: Action) -> Vec<Cmd> {
    if !app.caps.controls {
        toast(app, "session controls are not in this build");
        return vec![];
    }
    if app.transcript.target.is_none() {
        return vec![];
    }
    match action {
        Action::Abort => app.overlay = Some(Overlay::Confirm(ControlOp::Abort)),
        Action::Compact => app.overlay = Some(Overlay::Confirm(ControlOp::Compact)),
        Action::Models => return control(app, ControlOp::ListModels, Some(PickerKind::Model)),
        Action::ThinkingLevels => {
            return control(app, ControlOp::ListModels, Some(PickerKind::Thinking));
        }
        _ => (),
    }
    vec![]
}
fn control(app: &mut App, op: ControlOp, picker: Option<PickerKind>) -> Vec<Cmd> {
    if !app.caps.controls {
        toast(app, "session controls are not in this build");
        return vec![];
    }
    let Some(target) = app.transcript.target.clone() else {
        return vec![];
    };
    let local = app.local();
    app.pending_control = Some(PendingControl {
        local,
        target: target.clone(),
        op: op.clone(),
        picker,
    });
    vec![Cmd::Control { local, target, op }]
}
fn control_done(app: &mut App, local: u64, op: ControlOp, result: Result<ControlData, ErrView>) {
    let Some(pending) = app.pending_control.as_ref() else {
        return;
    };
    if pending.local != local
        || pending.op != op
        || app.transcript.target.as_ref() != Some(&pending.target)
    {
        return;
    }
    let Some(pending) = app.pending_control.take() else {
        return;
    };
    match result {
        Err(err) => app.prompt.notice = Some(err),
        Ok(ControlData::Done(s)) => toast(app, s),
        Ok(ControlData::Models {
            models,
            thinking_levels,
        }) => {
            let Some(kind) = pending.picker else {
                return;
            };
            let entries = match kind {
                PickerKind::Model => models
                    .into_iter()
                    .map(|m| PickerEntry {
                        label: text::single_line(&m.label),
                        op: ControlOp::SetModel {
                            provider: m.provider,
                            id: m.id,
                        },
                    })
                    .collect::<Vec<_>>(),
                PickerKind::Thinking => thinking_levels
                    .into_iter()
                    .map(|level| PickerEntry {
                        label: text::single_line(&level),
                        op: ControlOp::SetThinking(level),
                    })
                    .collect(),
            };
            if entries.is_empty() {
                toast(app, "no session control choices available");
            } else {
                app.overlay = Some(Overlay::Picker(Picker {
                    filter: String::new(),
                    kind,
                    entries,
                    selected: 0,
                    target: pending.target,
                }));
            }
        }
    }
}
fn followed(app: &mut App, ev: FollowMsg) {
    match ev {
        FollowMsg::Opened {
            header,
            items,
            dropped,
            not_yet_persisted,
        } => {
            app.transcript.header = Some(header);
            app.transcript.blocks.clear();
            app.transcript.dropped = dropped;
            app.transcript
                .blocks
                .extend(items.into_iter().map(Block::Item));
            trim_blocks(app);
            rerender(app);
            app.transcript.status = if not_yet_persisted {
                TranscriptStatus::NotYetPersisted
            } else {
                TranscriptStatus::Ready
            };
        }
        FollowMsg::NotYetPersisted => app.transcript.status = TranscriptStatus::NotYetPersisted,
        FollowMsg::Items(items) => {
            app.transcript.status = TranscriptStatus::Ready;
            let n = items.len();
            let start = app.transcript.blocks.len();
            app.transcript
                .blocks
                .extend(items.into_iter().map(Block::Item));
            if !app.transcript.scroll.follow {
                app.transcript.scroll.unseen = app.transcript.scroll.unseen.saturating_add(n);
            }
            render_new(app, start);
            let removed = trim_blocks(app);
            if removed > 0 {
                trim_rendered(app, removed);
            }
        }
        FollowMsg::Notice(s) => {
            let start = app.transcript.blocks.len();
            app.transcript.blocks.push_back(Block::Notice(s));
            render_new(app, start);
        }
        FollowMsg::Continued {
            to_id,
            to_path,
            handle,
        } => {
            switch_draft(app, to_id.clone());
            app.transcript.target = Some(Target {
                id: to_id,
                path: to_path,
            });
            if let Some(header) = &mut app.transcript.header {
                header.handle = handle.clone();
            }
            let start = app.transcript.blocks.len();
            app.transcript.blocks.push_back(Block::Notice(format!(
                "── continued in {} ──",
                text::single_line(&handle)
            )));
            render_new(app, start);
            app.pending_control = None;
            if matches!(app.overlay, Some(Overlay::Confirm(_) | Overlay::Picker(_))) {
                app.overlay = None;
            }
        }
        FollowMsg::Failed(err) => app.transcript.status = TranscriptStatus::Failed(err),
    }
}
fn trim_blocks(app: &mut App) -> usize {
    let mut items = app
        .transcript
        .blocks
        .iter()
        .filter(|b| matches!(b, Block::Item(_)))
        .count();
    let mut removed = 0;
    while items > 1000 {
        if let Some(b) = app.transcript.blocks.pop_front() {
            if matches!(b, Block::Item(_)) {
                items -= 1;
                app.transcript.dropped = app.transcript.dropped.saturating_add(1);
            }
            removed += 1;
        } else {
            break;
        }
    }
    removed
}
/// Drop cached rows with their blocks, retaining every surviving span and the model context.
fn trim_rendered(app: &mut App, removed: usize) {
    let mut banner = dropped_banner(app);
    banner.push(Line::default());
    let prefix = banner.len();
    let t = &mut app.transcript;
    let cut = t.starts[removed];
    t.lines.drain(..cut);
    t.lines.splice(0..0, banner);
    t.starts.drain(..removed);
    for start in &mut t.starts {
        *start = start.saturating_sub(cut).saturating_add(prefix);
    }
    t.scroll.top = t.scroll.top.saturating_sub(cut).saturating_add(prefix);
}
#[cfg(test)]
thread_local! {
    static RENDERED_BLOCKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
fn render_new(app: &mut App, start: usize) {
    let p = app.theme.painter();
    let t = &mut app.transcript;
    let st = transcript::Style {
        painter: &p,
        width: Some(t.width.max(1)),
        tools: t.opts.tools,
        thinking: t.opts.thinking,
    };
    for block in t.blocks.iter().skip(start) {
        #[cfg(test)]
        RENDERED_BLOCKS.with(|count| count.set(count.get() + 1));
        if !t.lines.is_empty() {
            t.lines.push(Line::default());
        }
        t.starts.push(t.lines.len());
        let rendered = match block {
            Block::Item(item) => {
                transcript::render_item_in(item, &mut t.prev_model, &st, &app.timezone)
            }
            Block::Notice(s) => p.paint(Token::Muted, &render::sanitize(s)),
        };
        t.lines.extend(ansi::lines(&rendered));
    }
}
fn dropped_banner(app: &App) -> Vec<Line<'static>> {
    let t = &app.transcript;
    if t.dropped > 0 {
        let handle = t
            .header
            .as_ref()
            .map(|h| h.handle.as_str())
            .unwrap_or("<session>");
        let banner = format!(
            "── {} earlier items · glen show {} ──",
            t.dropped,
            text::single_line(handle)
        );
        return ansi::lines(&app.theme.painter().paint(Token::Muted, &banner));
    }
    vec![]
}
fn rerender(app: &mut App) {
    let banner = dropped_banner(app);
    let t = &mut app.transcript;
    t.lines.clear();
    t.lines.extend(banner);
    t.starts.clear();
    t.prev_model = None;
    render_new(app, 0);
}
fn follow_bottom(app: &mut App) {
    if app.transcript.scroll.follow {
        app.transcript.scroll.top = app.transcript.lines.len().saturating_sub(app.page());
    }
}
/// Apply the viewport supplied by the view; a width change reflows the cache once.
pub fn set_viewport(app: &mut App, width: u16, height: u16) {
    app.dirty |= app.transcript.width != width.max(1) || app.transcript.height != height.max(1);
    let anchor = app
        .transcript
        .starts
        .partition_point(|start| *start <= app.transcript.scroll.top)
        .checked_sub(1);
    let width = width.max(1);
    app.transcript.height = height.max(1);
    if width != app.transcript.width {
        app.transcript.width = width;
        rerender(app);
        if !app.transcript.scroll.follow {
            app.transcript.scroll.top = anchor
                .and_then(|i| app.transcript.starts.get(i))
                .copied()
                .unwrap_or(0);
        }
    }
    follow_bottom(app);
}
fn expire_jobs(app: &mut App, all: bool) {
    app.jobs.rows.retain(|row| {
        let expired = row.status.status.is_terminal()
            && (all
                || row.finished_at.is_some_and(|at| {
                    app.now.saturating_duration_since(at) >= Duration::from_secs(60)
                }));
        if expired {
            app.jobs.dismissed.insert(row.id.clone());
        }
        !expired
    });
}
fn jobs(app: &mut App, mut snapshot: JobsSnapshot) {
    for row in &mut snapshot.rows {
        if row.status.status.is_terminal() {
            row.finished_at = app
                .jobs
                .rows
                .iter()
                .find(|old| old.id == row.id)
                .and_then(|old| old.finished_at)
                .or(Some(app.now));
        } else {
            row.finished_at = None;
        }
        row.ours |= app
            .sends
            .iter()
            .any(|s| matches!(&s.phase,SendPhase::Job{id,..} if id==&row.id));
    }
    // A worker may stop reporting finished jobs; retain their strip rows for the grace period.
    for old in &app.jobs.rows {
        if old.status.status.is_terminal() && !snapshot.rows.iter().any(|r| r.id == old.id) {
            snapshot.rows.push(old.clone());
        }
    }
    snapshot
        .rows
        .retain(|row| !row.status.status.is_terminal() || !app.jobs.dismissed.contains(&row.id));
    app.jobs.rows = snapshot.rows;
    app.jobs.active_total = snapshot.active_total;
    expire_jobs(app, false);
}
fn inbox(app: &mut App, snapshot: InboxSnapshot) {
    app.inbox.cursors.extend(snapshot.cursors);
    for record in snapshot.records {
        let msg = record.message;
        app.inbox
            .positions
            .insert((msg.session_id.clone(), msg.id.clone()), record.position);
        if let Some(old) = app
            .inbox
            .records
            .iter_mut()
            .find(|r| r.id == msg.id && r.session_id == msg.session_id)
        {
            *old = msg;
        } else {
            app.inbox.records.push(msg);
        }
    }
    app.inbox
        .records
        .sort_by(|a, b| timestamp_cmp(&a.at, &b.at));
    let excess = app.inbox.records.len().saturating_sub(500);
    app.inbox.records.drain(..excess);
    app.inbox.unread_by_session.clear();
    for msg in &mut app.inbox.records {
        if let Some(position) = app
            .inbox
            .positions
            .get(&(msg.session_id.clone(), msg.id.clone()))
        {
            let cursor = app
                .inbox
                .cursors
                .get(&msg.session_id)
                .copied()
                .unwrap_or_default();
            msg.unread = cursor.ino != position.ino || position.end > cursor.offset;
        }
        if msg.unread {
            *app.inbox
                .unread_by_session
                .entry(msg.session_id.clone())
                .or_default() += 1;
        }
    }
    app.inbox.positions.retain(|(session, id), _| {
        app.inbox
            .records
            .iter()
            .any(|r| &r.id == id && &r.session_id == session)
    });
    app.inbox.scroll = app
        .inbox
        .scroll
        .min(app.inbox.records.len().saturating_sub(1));
}
fn timestamp_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    // Malformed timestamps sort before dated records. Using one total ordering
    // also keeps mixed timezone offsets and malformed input safe for sort_by.
    a.parse::<jiff::Timestamp>()
        .ok()
        .cmp(&b.parse::<jiff::Timestamp>().ok())
        .then_with(|| a.cmp(b))
}

pub fn tick_interval(app: &App) -> Duration {
    let animates = matches!(
        app.transcript.status,
        TranscriptStatus::Opening | TranscriptStatus::NotYetPersisted
    ) || app.pending_control.is_some()
        || app.jobs.active_total > 0
        || app.jobs.rows.iter().any(|r| !r.status.status.is_terminal())
        || app
            .sends
            .iter()
            .any(|s| matches!(s.phase, SendPhase::Routing));
    Duration::from_millis(if animates { 80 } else { 250 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cli::Tools,
        jobs::{JobResult, Reply, Route, State as JobState, Status},
        omp::{items::Item, lease::State as SessionState, session::TurnStatus},
        render::palette::{ColorDepth, Theme},
        tui::{
            keymap::{Action, OverlayCtx},
            theme::TuiTheme,
        },
    };
    use std::{
        collections::HashMap,
        path::PathBuf,
        time::{Instant, SystemTime},
    };
    fn row(id: &str) -> SessionRow {
        SessionRow {
            id: id.into(),
            handle: format!("h{id}"),
            path: Some(PathBuf::from(format!("/fixture/{id}.jsonl"))),
            title: id.into(),
            first_message: None,
            cwd: None,
            state: SessionState::Idle,
            source: "disk",
            last_turn: TurnStatus::Unknown,
            updated: SystemTime::UNIX_EPOCH,
            model: None,
        }
    }
    fn app() -> App {
        let mut app = App::new(
            TuiTheme {
                depth: ColorDepth::None,
                theme: Theme::Dark,
            },
            Scope::Bucket {
                cwd: "/fixture".into(),
                label: "fixture".into(),
            },
            (100, 12),
            Instant::now(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(100),
        );
        let cmds = update(
            &mut app,
            Msg::Sessions(ListSnapshot {
                rows: vec![row("a"), row("b"), row("c")],
                total_all: 3,
                sessions_dir_missing: None,
            }),
        );
        assert_eq!(cmds.len(), 1);
        app
    }
    fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Vec<Cmd> {
        update(app, Msg::Key(KeyEvent::new(code, modifiers)))
    }
    fn plain(app: &mut App, code: KeyCode) -> Vec<Cmd> {
        key(app, code, KeyModifiers::NONE)
    }
    fn item(n: usize) -> Item {
        Item::User {
            id: n.to_string(),
            timestamp: None,
            text: format!("{n}: a transcript line that wraps across the viewport"),
            images: 0,
        }
    }
    fn items(app: &mut App, values: Vec<Item>) {
        let r#gen = app.transcript.r#gen;
        assert!(
            update(
                app,
                Msg::Follow {
                    r#gen,
                    ev: FollowMsg::Items(values)
                }
            )
            .is_empty()
        );
    }
    fn open(app: &mut App, values: Vec<Item>) {
        let r#gen = app.transcript.r#gen;
        update(
            app,
            Msg::Follow {
                r#gen,
                ev: FollowMsg::Opened {
                    header: Header {
                        title: "title".into(),
                        handle: "ha".into(),
                        model: None,
                    },
                    items: values,
                    dropped: 0,
                    not_yet_persisted: false,
                },
            },
        );
    }
    fn type_text(app: &mut App, text: &str) {
        app.focus = Focus::Prompt;
        update(app, Msg::Paste(text.into()));
    }
    fn local(cmds: Vec<Cmd>) -> u64 {
        assert_eq!(cmds.len(), 1);
        match cmds.into_iter().next() {
            Some(Cmd::Submit { local, .. }) | Some(Cmd::Control { local, .. }) => local,
            _ => panic!("expected submit/control"),
        }
    }
    fn error() -> ErrView {
        GlenError::new(
            ErrorCode::NoRoute,
            "job j1234567 failed: session is open in another omp process",
        )
        .with_hint("install the extension")
        .into()
    }

    #[test]
    fn focus_cycles_wide_medium_and_narrow_and_overlays_capture() {
        for width in [140, 100, 60] {
            let mut a = app();
            update(&mut a, Msg::Resize(width, 20));
            for expected in [Focus::Transcript, Focus::Prompt, Focus::Sessions] {
                plain(&mut a, KeyCode::Tab);
                assert_eq!(a.focus, expected);
                if a.narrow() {
                    assert_eq!(
                        a.narrow_tab,
                        if expected == Focus::Sessions {
                            Pane::Sessions
                        } else {
                            Pane::Transcript
                        }
                    );
                }
            }
            key(&mut a, KeyCode::BackTab, KeyModifiers::SHIFT);
            assert_eq!(a.focus, Focus::Prompt);
            a.overlay = Some(Overlay::Help);
            plain(&mut a, KeyCode::Tab);
            plain(&mut a, KeyCode::Char('q'));
            assert_eq!(a.focus, Focus::Prompt);
            assert!(a.exit.is_none());
            plain(&mut a, KeyCode::Esc);
            assert!(a.overlay.is_none());
        }
    }
    #[test]
    fn selection_is_clamped_paged_and_kept_by_id_including_vanished_ids() {
        let mut a = app();
        set_viewport(&mut a, 40, 1);
        assert!(plain(&mut a, KeyCode::Up).is_empty());
        let before = a.transcript.r#gen;
        let cmds = plain(&mut a, KeyCode::PageDown);
        assert!(
            matches!(&cmds[..],[Cmd::Follow{r#gen,target,..}] if *r#gen==before+1 && target.id=="b")
        );
        assert_eq!(a.list.offset, 1);
        plain(&mut a, KeyCode::End);
        assert_eq!(a.list.selected.as_deref(), Some("c"));
        assert!(plain(&mut a, KeyCode::Down).is_empty());
        let generation = a.transcript.r#gen;
        assert!(
            update(
                &mut a,
                Msg::Sessions(ListSnapshot {
                    rows: vec![row("c"), row("b"), row("a")],
                    total_all: 3,
                    sessions_dir_missing: None
                })
            )
            .is_empty()
        );
        assert_eq!(a.list.selected.as_deref(), Some("c"));
        assert_eq!(a.transcript.r#gen, generation);
        update(
            &mut a,
            Msg::Sessions(ListSnapshot {
                rows: vec![row("b")],
                total_all: 1,
                sessions_dir_missing: None,
            }),
        );
        assert_eq!(a.list.selected.as_deref(), Some("c"));
        plain(&mut a, KeyCode::Home);
        assert_eq!(a.list.selected.as_deref(), Some("b"));
    }
    #[test]
    fn stale_follow_events_are_ignored_and_continuation_moves_selection() {
        let mut a = app();
        let old = a.transcript.r#gen;
        plain(&mut a, KeyCode::Down);
        update(
            &mut a,
            Msg::Follow {
                r#gen: old,
                ev: FollowMsg::Items(vec![item(0)]),
            },
        );
        assert!(a.transcript.blocks.is_empty());
        let r#gen = a.transcript.r#gen;
        update(
            &mut a,
            Msg::Follow {
                r#gen,
                ev: FollowMsg::Continued {
                    to_id: "next".into(),
                    to_path: Some("/next.jsonl".into()),
                    handle: "next-handle".into(),
                },
            },
        );
        assert_eq!(a.list.selected.as_deref(), Some("next"));
        assert_eq!(a.transcript.target.as_ref().unwrap().id, "next");
        assert!(
            matches!(a.transcript.blocks.back(),Some(Block::Notice(s)) if s=="── continued in next-handle ──")
        );
    }
    #[test]
    fn append_follow_scroll_unseen_and_resume() {
        let mut a = app();
        a.focus = Focus::Transcript;
        set_viewport(&mut a, 35, 3);
        open(&mut a, (0..5).map(item).collect());
        assert_eq!(a.transcript.scroll.top, a.transcript.lines.len() - 3);
        plain(&mut a, KeyCode::Up);
        let top = a.transcript.scroll.top;
        items(&mut a, vec![item(5), item(6)]);
        assert_eq!(a.transcript.scroll.top, top);
        assert_eq!(a.transcript.scroll.unseen, 2);
        assert!(!a.transcript.scroll.follow);
        plain(&mut a, KeyCode::Char('G'));
        assert!(a.transcript.scroll.follow);
        assert_eq!(a.transcript.scroll.unseen, 0);
        assert_eq!(a.transcript.scroll.top, a.transcript.lines.len() - 3);
        items(&mut a, vec![item(7)]);
        assert_eq!(a.transcript.scroll.top, a.transcript.lines.len() - 3);
    }
    #[test]
    fn follow_invariant_tracks_bottom_after_every_append() {
        let mut a = app();
        set_viewport(&mut a, 40, 6);
        open(&mut a, vec![]);
        for n in 0..80 {
            if n % 7 == 0 {
                set_viewport(&mut a, 15 + n as u16, 1 + n as u16 % 10);
            }
            if n % 5 == 0 {
                let r#gen = a.transcript.r#gen;
                update(
                    &mut a,
                    Msg::Follow {
                        r#gen,
                        ev: FollowMsg::Notice("── switched branch ──".into()),
                    },
                );
            }
            // Model a stale top after an external viewport/cache adjustment.
            a.transcript.scroll.top = 0;
            items(&mut a, if n % 3 == 0 { vec![] } else { vec![item(n)] });
            assert!(a.transcript.scroll.follow);
            assert_eq!(
                a.transcript.scroll.top,
                a.transcript.lines.len().saturating_sub(a.page()),
                "append {n}"
            );
        }
    }
    #[test]
    fn width_change_maps_top_to_the_same_block_and_append_cache_matches_renderer() {
        let mut a = app();
        set_viewport(&mut a, 40, 3);
        let values = (0..7).map(item).collect::<Vec<_>>();
        open(&mut a, values[..4].to_vec());
        items(&mut a, values[4..].to_vec());
        let p = a.theme.painter();
        let expected = transcript::render_items(
            &values,
            &transcript::Style {
                painter: &p,
                width: Some(40),
                tools: Tools::Collapsed,
                thinking: false,
            },
        );
        assert_eq!(
            a.transcript
                .lines
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            expected.split('\n').collect::<Vec<_>>()
        );
        a.transcript.scroll.follow = false;
        a.transcript.scroll.top = a.transcript.starts[3] + 1;
        set_viewport(&mut a, 18, 3);
        assert_eq!(a.transcript.scroll.top, a.transcript.starts[3]);
    }
    #[test]
    fn model_context_is_continued_on_incremental_render_and_reset_on_reflow() {
        let mut a = app();
        let assistant = |id: &str| Item::Assistant {
            id: id.into(),
            timestamp: None,
            text: "answer".into(),
            model: Some("provider/model".into()),
            stop_reason: None,
            error: None,
            has_thinking: false,
            thinking_words: 0,
            thinking: None,
            tool_calls: vec![],
        };
        let values = [assistant("one"), assistant("two")];
        open(&mut a, vec![values[0].clone()]);
        items(&mut a, vec![values[1].clone()]);
        let prose = a
            .transcript
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(prose.matches("model").count(), 1);
        set_viewport(&mut a, 60, 4);
        let prose = a
            .transcript
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(prose.matches("model").count(), 1);
    }
    #[test]
    fn transcript_item_cap_retains_notices_and_shows_the_exact_dropped_banner() {
        let mut a = app();
        open(&mut a, (0..1005).map(item).collect());
        assert_eq!(
            a.transcript
                .blocks
                .iter()
                .filter(|b| matches!(b, Block::Item(_)))
                .count(),
            1000
        );
        assert_eq!(a.transcript.dropped, 5);
        assert_eq!(
            a.transcript.lines[0].to_string(),
            "── 5 earlier items · glen show ha ──"
        );
        assert_eq!(a.transcript.starts.len(), 1000);
        items(&mut a, vec![item(1005)]);
        assert_eq!(a.transcript.dropped, 6);
        a.transcript.scroll.follow = false;
        a.transcript.scroll.top = 0;
        set_viewport(&mut a, 45, 4);
        assert_eq!(a.transcript.scroll.top, 0);
    }
    #[test]
    fn cap_trimming_reuses_surviving_lines_and_renders_only_appended_blocks() {
        for notice in [false, true] {
            let mut a = app();
            set_viewport(&mut a, 40, 6);
            open(&mut a, vec![item(0)]);
            if notice {
                let r#gen = a.transcript.r#gen;
                update(
                    &mut a,
                    Msg::Follow {
                        r#gen,
                        ev: FollowMsg::Notice("retained notice".into()),
                    },
                );
            }
            items(&mut a, (1..1000).map(item).collect());
            a.transcript.scroll.follow = false;
            a.transcript.scroll.top = a.transcript.starts[500] + 1;
            for (id, removed) in [(1000, 1), (1001, if notice { 2 } else { 1 })] {
                let cut = a.transcript.starts[removed];
                let retained = a.transcript.lines[cut..].to_vec();
                let starts = a.transcript.starts[removed..]
                    .iter()
                    .map(|s| s - cut + 2)
                    .collect::<Vec<_>>();
                let top = a.transcript.scroll.top;
                RENDERED_BLOCKS.with(|count| count.set(0));
                items(&mut a, vec![item(id)]);
                RENDERED_BLOCKS
                    .with(|count| assert_eq!(count.get(), 1, "trim re-rendered retained blocks"));
                assert_eq!(&a.transcript.lines[2..2 + retained.len()], retained);
                assert_eq!(&a.transcript.starts[..starts.len()], starts);
                assert_eq!(a.transcript.scroll.top, top.saturating_sub(cut) + 2);
                assert_eq!(a.transcript.starts[0], 2);
                assert_eq!(a.transcript.dropped, id - 999);
                assert_eq!(
                    a.transcript.lines[0].to_string(),
                    format!("── {} earlier items · glen show ha ──", id - 999)
                );
            }
        }
    }
    #[test]
    fn dirty_tracks_visual_ticks_and_resize_defers_the_viewport_to_prepare() {
        let mut a = app();
        open(&mut a, vec![item(0)]);
        assert!(a.dirty);
        a.dirty = false; // A completed draw consumes the flag.
        let now = a.now + Duration::from_millis(250);
        let wall = a.wall + Duration::from_millis(250);
        update(&mut a, Msg::Tick { now, wall });
        assert!(!a.dirty, "an idle tick must not request a draw");
        let mut release = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        update(&mut a, Msg::Key(release));
        assert!(!a.dirty);
        let stale = a.transcript.r#gen.wrapping_sub(1);
        update(
            &mut a,
            Msg::Follow {
                r#gen: stale,
                ev: FollowMsg::Items(vec![item(1)]),
            },
        );
        assert!(!a.dirty);
        let width = a.transcript.width;
        RENDERED_BLOCKS.with(|count| count.set(0));
        update(&mut a, Msg::Resize(120, 30));
        assert!(a.dirty);
        assert_eq!(a.transcript.width, width);
        RENDERED_BLOCKS.with(|count| assert_eq!(count.get(), 0));
        crate::tui::view::prepare(&mut a);
        assert_eq!(a.transcript.width, 83);
        RENDERED_BLOCKS.with(|count| assert_eq!(count.get(), 1));
        a.dirty = false;
        a.transcript.status = TranscriptStatus::Opening;
        let now = a.now + Duration::from_millis(80);
        update(&mut a, Msg::Tick { now, wall });
        assert!(a.dirty, "spinner frames must request a draw");
        a.transcript.status = TranscriptStatus::Ready;
        toast(&mut a, "expires");
        a.dirty = false;
        let now = a.now + Duration::from_secs(3);
        update(&mut a, Msg::Tick { now, wall });
        assert!(a.dirty);
        assert!(a.toast.is_none());
        a.dirty = false;
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(121);
        update(&mut a, Msg::Tick { now, wall });
        assert!(a.dirty, "a changed session-age label must request a draw");
    }
    #[test]
    fn thinking_tools_and_scope_emit_only_the_expected_commands() {
        let mut a = app();
        let generation = a.transcript.r#gen;
        let cmds = plain(&mut a, KeyCode::Char('t'));
        assert!(
            matches!(&cmds[..],[Cmd::Follow{r#gen,opts,..}] if *r#gen==generation+1&&opts.thinking)
        );
        for tools in [Tools::Full, Tools::None, Tools::Collapsed] {
            let cmds = plain(&mut a, KeyCode::Char('o'));
            assert!(
                matches!(&cmds[..],[Cmd::Follow{opts,..}] if std::mem::discriminant(&opts.tools)==std::mem::discriminant(&tools))
            );
        }
        assert!(matches!(
            &plain(&mut a, KeyCode::Char('a'))[..],
            [Cmd::ListScope(Scope::All)]
        ));
        assert!(
            matches!(&plain(&mut a,KeyCode::Char('a'))[..],[Cmd::ListScope(Scope::Bucket{cwd,..})] if cwd==&PathBuf::from("/fixture"))
        );
        assert!(matches!(
            &plain(&mut a, KeyCode::Char('r'))[..],
            [Cmd::Refresh]
        ));
    }
    #[test]
    fn enter_focuses_prompt_and_q_types_in_it() {
        let mut a = app();
        plain(&mut a, KeyCode::Enter);
        assert_eq!(a.focus, Focus::Prompt);
        plain(&mut a, KeyCode::Char('q'));
        assert_eq!(a.prompt.text, "q");
        assert!(a.exit.is_none());
        key(&mut a, KeyCode::Enter, KeyModifiers::ALT);
        key(&mut a, KeyCode::Char('j'), KeyModifiers::CONTROL);
        plain(&mut a, KeyCode::Char('\n'));
        assert_eq!(a.prompt.text, "q\n\n\n");
    }
    #[test]
    fn submit_validates_bytes_and_whitespace_and_sends_a_literal_hyphen() {
        for text in ["", " \n ", &"x".repeat(524_289)] {
            let mut a = app();
            type_text(&mut a, text);
            assert!(plain(&mut a, KeyCode::Enter).is_empty());
            assert!(a.sends.is_empty());
            if text.len() > 524_288 {
                assert_eq!(
                    a.prompt.notice.as_ref().unwrap().message,
                    "message is larger than 512 KiB"
                );
            }
        }
        for text in ["-".to_string(), "x".repeat(524_288)] {
            let mut a = app();
            type_text(&mut a, &text);
            let cmds = plain(&mut a, KeyCode::Enter);
            assert!(
                matches!(&cmds[..],[Cmd::Submit{target,text:actual,mode:DeliverAs::Auto,..}] if target.id=="a"&&actual==&text)
            );
            assert!(a.prompt.locked.is_some());
            assert_eq!(a.sends.len(), 1);
            assert!(plain(&mut a, KeyCode::Enter).is_empty());
        }
    }
    #[test]
    fn repeated_send_and_released_keys_have_no_effect() {
        let mut a = app();
        type_text(&mut a, "text");
        let mut ev = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        ev.kind = KeyEventKind::Repeat;
        assert!(update(&mut a, Msg::Key(ev)).is_empty());
        ev.kind = KeyEventKind::Release;
        assert!(update(&mut a, Msg::Key(ev)).is_empty());
        ev = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        ev.kind = KeyEventKind::Repeat;
        update(&mut a, Msg::Key(ev));
        assert_eq!(a.prompt.text, "text");
        ev = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        ev.kind = KeyEventKind::Repeat;
        update(&mut a, Msg::Key(ev));
        assert_eq!(a.prompt.cursor, 3);
    }
    #[test]
    fn submitted_failure_preserves_text_hint_and_mode_success_adds_global_history() {
        let mut a = app();
        type_text(&mut a, "message");
        key(&mut a, KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert_eq!(a.prompt.mode, DeliverAs::Steer);
        let id = local(plain(&mut a, KeyCode::Enter));
        update(
            &mut a,
            Msg::Submitted {
                local: id,
                result: Err(error()),
            },
        );
        assert_eq!(a.prompt.text, "message");
        assert_eq!(
            a.prompt.notice.as_ref().unwrap().hint.as_deref(),
            Some("install the extension")
        );
        assert!(a.prompt.locked.is_none());
        assert!(matches!(a.sends[0].phase, SendPhase::Refused(_)));
        let id = local(plain(&mut a, KeyCode::Enter));
        update(
            &mut a,
            Msg::Submitted {
                local: id,
                result: Ok(JobRef {
                    id: "job".into(),
                    handle: "j1234567".into(),
                }),
            },
        );
        assert!(a.prompt.text.is_empty());
        assert!(a.prompt.notice.is_none());
        assert_eq!(a.prompt.mode, DeliverAs::Auto);
        assert_eq!(a.history, ["message"]);
        assert_eq!(a.prompt.history, a.history);
        update(
            &mut a,
            Msg::Submitted {
                local: id,
                result: Ok(JobRef {
                    id: "duplicate".into(),
                    handle: "duplicate".into(),
                }),
            },
        );
        assert_eq!(a.history.len(), 1);
    }
    #[test]
    fn mode_cycles_and_escape_dismisses_error_before_switching_focus() {
        let mut a = app();
        a.focus = Focus::Prompt;
        for mode in [DeliverAs::Steer, DeliverAs::FollowUp, DeliverAs::Auto] {
            key(&mut a, KeyCode::Char('t'), KeyModifiers::CONTROL);
            assert_eq!(a.prompt.mode, mode);
        }
        a.prompt.notice = Some(error());
        plain(&mut a, KeyCode::Esc);
        assert_eq!(a.focus, Focus::Prompt);
        assert!(a.prompt.notice.is_none());
        plain(&mut a, KeyCode::Esc);
        assert_eq!(a.focus, Focus::Transcript);
    }
    #[test]
    fn selection_switches_drafts_without_losing_text_cursor_or_mode() {
        let mut a = app();
        type_text(&mut a, "A界");
        plain(&mut a, KeyCode::Left);
        key(&mut a, KeyCode::Char('t'), KeyModifiers::CONTROL);
        a.focus = Focus::Sessions;
        plain(&mut a, KeyCode::Down);
        type_text(&mut a, "B");
        a.focus = Focus::Sessions;
        plain(&mut a, KeyCode::Up);
        assert_eq!(a.prompt.text, "A界");
        assert_eq!(a.prompt.cursor, 1);
        assert_eq!(a.prompt.mode, DeliverAs::Steer);
    }
    #[test]
    fn inactive_submission_refusal_and_success_update_only_the_owning_draft() {
        for success in [false, true] {
            let mut a = app();
            type_text(&mut a, "A draft");
            let id = local(plain(&mut a, KeyCode::Enter));
            a.focus = Focus::Sessions;
            plain(&mut a, KeyCode::Down);
            type_text(&mut a, "B draft");
            plain(&mut a, KeyCode::Left);
            let cursor = a.prompt.cursor;
            update(
                &mut a,
                Msg::Submitted {
                    local: id,
                    result: if success {
                        Ok(JobRef {
                            id: "job".into(),
                            handle: "job".into(),
                        })
                    } else {
                        Err(error())
                    },
                },
            );
            assert_eq!(a.prompt.text, "B draft");
            assert_eq!(a.prompt.cursor, cursor);
            assert!(a.prompt.notice.is_none());
            let draft = &a.drafts["a"];
            assert!(draft.locked.is_none());
            assert_eq!(draft.text, if success { "" } else { "A draft" });
            assert_eq!(draft.notice.is_some(), !success);
            a.focus = Focus::Sessions;
            plain(&mut a, KeyCode::Up);
            assert_eq!(a.prompt.text, if success { "" } else { "A draft" });
            assert_eq!(a.prompt.notice.is_some(), !success);
        }
    }
    fn job(state: JobState) -> JobRow {
        JobRow {
            id: "job".into(),
            handle: "j1234567".into(),
            session_id: Some("a".into()),
            actual_session_id: None,
            route: Route::Rpc,
            status: Status {
                status: state,
                ..Status::default()
            },
            created_at: "1970-01-01T00:00:00Z".into(),
            result: None,
            ours: true,
            finished_at: None,
            progress: match state {
                JobState::Queued => "starting omp",
                JobState::Accepted => "waiting for the reply",
                _ => "completed",
            }
            .into(),
            done: if state == JobState::Completed {
                Some(Done::Completed(
                    "✓ completed · j1234567 · ha · 1m 40s".into(),
                ))
            } else {
                None
            },
        }
    }
    fn publish_job(a: &mut App, row: JobRow) {
        let active_total = usize::from(!row.status.status.is_terminal());
        update(
            a,
            Msg::Jobs(JobsSnapshot {
                rows: vec![row],
                active_total,
            }),
        );
    }
    fn reply(id: &str, at: u64) -> Reply {
        Reply {
            v: 1,
            id: id.into(),
            session_id: "a".into(),
            job_id: Some("job".into()),
            request_id: None,
            tool_call_id: None,
            text: id.into(),
            is_final: true,
            at: format!("1970-01-01T00:00:{at:02}Z"),
        }
    }
    fn result(replies: Vec<Reply>) -> JobResult {
        JobResult {
            v: 1,
            prompt_status: None,
            agent_invoked: true,
            assistant_text: None,
            replies,
            command_output: vec![],
            stop_reason: None,
            session_settled: false,
            settled_at: None,
        }
    }
    #[test]
    fn job_progress_and_precomputed_terminal_messages_and_errors() {
        let mut a = app();
        for (state, expected) in [
            (JobState::Queued, "starting omp"),
            (JobState::Accepted, "waiting for the reply"),
            (JobState::Completed, "✓ completed"),
        ] {
            publish_job(&mut a, job(state));
            let lines = crate::tui::view::activity_lines(&a);
            assert!(lines[0].to_string().contains(expected));
            assert_eq!(a.jobs.rows[0].finished_at.is_some(), state.is_terminal());
        }
        let mut failed = job(JobState::Failed);
        failed.done = Some(Done::Failed(error()));
        publish_job(&mut a, failed);
        let lines = crate::tui::view::activity_lines(&a);
        assert_eq!(lines[0].to_string(), format!("✗ {}", error().message));
        assert_eq!(lines[1].to_string(), format!("  {}", error().hint.unwrap()));
        let mut forked = job(JobState::Completed);
        forked.done = Some(Done::Forked("✓ forked → new-handle".into()));
        publish_job(&mut a, forked);
        assert_eq!(
            crate::tui::view::activity_lines(&a)[0].to_string(),
            "✓ forked → new-handle"
        );
    }
    #[test]
    fn final_reply_does_not_finish_an_accepted_job_and_newest_three_are_deduplicated() {
        let mut a = app();
        let mut row = job(JobState::Accepted);
        row.result = Some(result(vec![
            reply("one", 1),
            reply("two", 2),
            reply("three", 3),
            reply("four", 4),
        ]));
        publish_job(&mut a, row);
        let inbox = InboxSnapshot {
            records: vec![InboxRecord {
                message: InboxMsg {
                    id: "four".into(),
                    session_id: "a".into(),
                    job_id: Some("job".into()),
                    text: "four from inbox".into(),
                    is_final: true,
                    at: reply("four", 4).at,
                    unread: true,
                },
                position: InboxPosition { end: 10, ino: 1 },
            }],
            ..InboxSnapshot::default()
        };
        update(&mut a, Msg::Inbox(inbox));
        let lines = crate::tui::view::activity_lines(&a);
        assert_eq!(lines.len(), 4);
        assert!(lines[0].to_string().contains("waiting for the reply"));
        assert!(!lines.iter().any(|l| l.to_string().contains("✓ completed")));
        assert!(!lines.iter().any(|l| l.to_string() == "↩ one ✓"));
        assert_eq!(lines[3].to_string(), "↩ four from inbox ✓");
    }
    #[test]
    fn terminal_rows_expire_on_time_or_next_submit_and_cannot_resurrect() {
        let mut a = app();
        let row = job(JobState::Completed);
        publish_job(&mut a, row.clone());
        let finish = a.jobs.rows[0].finished_at;
        let now = a.now + Duration::from_secs(59);
        let wall = a.wall;
        update(&mut a, Msg::Tick { now, wall });
        publish_job(&mut a, row.clone());
        assert_eq!(a.jobs.rows[0].finished_at, finish);
        let now = a.now + Duration::from_secs(1);
        update(&mut a, Msg::Tick { now, wall });
        assert!(a.jobs.rows.is_empty());
        publish_job(&mut a, row.clone());
        assert!(a.jobs.rows.is_empty());
        let mut a = app();
        publish_job(&mut a, row.clone());
        type_text(&mut a, "next");
        plain(&mut a, KeyCode::Enter);
        assert!(a.jobs.rows.is_empty());
        publish_job(&mut a, row);
        assert!(a.jobs.rows.is_empty());
    }
    #[test]
    fn unread_cursor_advances_lower_counts_and_merges_are_capped() {
        let mut a = app();
        let records = (0..510)
            .map(|n| InboxRecord {
                message: InboxMsg {
                    id: n.to_string(),
                    session_id: if n % 2 == 0 { "a" } else { "b" }.into(),
                    job_id: None,
                    text: n.to_string(),
                    is_final: false,
                    at: format!("{n:04}"),
                    unread: false,
                },
                position: InboxPosition { end: n + 1, ino: 4 },
            })
            .collect();
        update(
            &mut a,
            Msg::Inbox(InboxSnapshot {
                records,
                cursors: HashMap::new(),
            }),
        );
        assert_eq!(a.inbox.records.len(), 500);
        assert_eq!(a.inbox.unread_by_session["a"], 250);
        assert_eq!(a.inbox.unread_by_session["b"], 250);
        let mut cursors = HashMap::new();
        cursors.insert(
            "a".into(),
            crate::inbox::Cursor {
                offset: 500,
                ino: 4,
            },
        );
        update(
            &mut a,
            Msg::Inbox(InboxSnapshot {
                records: vec![],
                cursors,
            }),
        );
        assert_eq!(a.inbox.unread_by_session["a"], 5);
        assert_eq!(a.inbox.unread_by_session["b"], 250);
        assert_eq!(a.inbox.positions.len(), 500);
    }
    #[test]
    fn disabled_controls_toast_without_commands_from_panes_or_prompt() {
        for focus in [Focus::Sessions, Focus::Transcript, Focus::Prompt] {
            let mut a = app();
            a.focus = focus;
            assert!(key(&mut a, KeyCode::Char('x'), KeyModifiers::CONTROL).is_empty());
            assert_eq!(
                a.toast.as_ref().unwrap().text,
                "session controls are not in this build"
            );
            assert!(a.overlay.is_none());
        }
        for code in ['m', 'T', 'C'] {
            let mut a = app();
            assert!(plain(&mut a, KeyCode::Char(code)).is_empty());
            assert_eq!(
                a.toast.as_ref().unwrap().text,
                "session controls are not in this build"
            );
        }
    }
    #[test]
    fn enabled_confirm_controls_require_y_and_cancel_with_n_or_escape() {
        for cancel in [KeyCode::Char('n'), KeyCode::Esc] {
            let mut a = app();
            a.caps.controls = true;
            key(&mut a, KeyCode::Char('x'), KeyModifiers::CONTROL);
            assert!(matches!(
                a.overlay,
                Some(Overlay::Confirm(ControlOp::Abort))
            ));
            assert!(plain(&mut a, cancel).is_empty());
            assert!(a.overlay.is_none());
        }
        for (code, mods, op) in [
            (KeyCode::Char('x'), KeyModifiers::CONTROL, ControlOp::Abort),
            (KeyCode::Char('C'), KeyModifiers::NONE, ControlOp::Compact),
        ] {
            let mut a = app();
            a.caps.controls = true;
            key(&mut a, code, mods);
            let cmds = plain(&mut a, KeyCode::Char('y'));
            assert!(
                matches!(&cmds[..],[Cmd::Control{target,op:actual,..}] if target.id=="a"&&actual==&op)
            );
            let id = local(cmds);
            update(
                &mut a,
                Msg::ControlDone {
                    local: id,
                    op: op.clone(),
                    result: Ok(ControlData::Done("done".into())),
                },
            );
            assert_eq!(a.toast.as_ref().unwrap().text, "done");
        }
    }
    #[test]
    fn model_and_thinking_picker_flows_and_errors_are_correlated() {
        for (code, expected) in [
            (
                'm',
                ControlOp::SetModel {
                    provider: "provider".into(),
                    id: "model".into(),
                },
            ),
            ('T', ControlOp::SetThinking("high".into())),
        ] {
            let mut a = app();
            a.caps.controls = true;
            let cmds = plain(&mut a, KeyCode::Char(code));
            assert!(matches!(
                &cmds[..],
                [Cmd::Control {
                    op: ControlOp::ListModels,
                    ..
                }]
            ));
            let id = local(cmds);
            update(
                &mut a,
                Msg::ControlDone {
                    local: id + 1,
                    op: ControlOp::ListModels,
                    result: Err(error()),
                },
            );
            assert!(a.prompt.notice.is_none());
            update(
                &mut a,
                Msg::ControlDone {
                    local: id,
                    op: ControlOp::ListModels,
                    result: Ok(ControlData::Models {
                        models: vec![ModelChoice {
                            provider: "provider".into(),
                            id: "model".into(),
                            label: "Model".into(),
                        }],
                        thinking_levels: vec!["high".into()],
                    }),
                },
            );
            assert!(matches!(a.overlay, Some(Overlay::Picker(_))));
            let cmds = plain(&mut a, KeyCode::Enter);
            assert!(matches!(&cmds[..],[Cmd::Control{op,..}] if op==&expected));
            let id = local(cmds);
            update(
                &mut a,
                Msg::ControlDone {
                    local: id,
                    op: expected,
                    result: Err(error()),
                },
            );
            assert_eq!(a.prompt.notice.as_ref(), Some(&error()));
        }
    }
    #[test]
    fn quit_keys_signals_and_interrupt_clear() {
        for (focus, code, mods) in [
            (Focus::Sessions, KeyCode::Char('q'), KeyModifiers::NONE),
            (Focus::Sessions, KeyCode::Char('c'), KeyModifiers::CONTROL),
            (Focus::Prompt, KeyCode::Char('d'), KeyModifiers::CONTROL),
        ] {
            let mut a = app();
            a.focus = focus;
            assert!(matches!(&key(&mut a, code, mods)[..], [Cmd::Quit(0)]));
            assert_eq!(a.exit, Some(0));
            assert!(plain(&mut a, KeyCode::Char('q')).is_empty());
        }
        let mut a = app();
        type_text(&mut a, "text");
        assert!(key(&mut a, KeyCode::Char('c'), KeyModifiers::CONTROL).is_empty());
        assert!(a.prompt.text.is_empty());
        assert!(a.exit.is_none());
        assert!(matches!(
            &update(&mut a, Msg::Signal(15))[..],
            [Cmd::Quit(143)]
        ));
        let mut a = app();
        type_text(&mut a, "text");
        assert!(key(&mut a, KeyCode::Char('d'), KeyModifiers::CONTROL).is_empty());
        assert!(a.exit.is_none());
    }
    #[test]
    fn tick_updates_clocks_expires_toasts_and_uses_animation_interval() {
        let mut a = app();
        assert_eq!(tick_interval(&a), Duration::from_millis(80));
        open(&mut a, vec![]);
        assert_eq!(tick_interval(&a), Duration::from_millis(250));
        toast(&mut a, "temporary");
        let now = a.now + Duration::from_secs(5);
        let wall = a.wall + Duration::from_secs(5);
        assert!(update(&mut a, Msg::Tick { now, wall }).is_empty());
        assert_eq!(a.frame, 1);
        assert_eq!(a.now, now);
        assert_eq!(a.wall, wall);
        assert!(a.toast.is_none());
        publish_job(&mut a, job(JobState::Accepted));
        assert_eq!(tick_interval(&a), Duration::from_millis(80));
    }
    fn command_signature(cmds: Vec<Cmd>) -> Vec<String> {
        cmds.into_iter()
            .map(|c| match c {
                Cmd::Follow {
                    r#gen,
                    target,
                    opts,
                } => format!("follow {gen} {target:?} {} {:?}", opts.thinking, opts.tools),
                Cmd::ListScope(scope) => format!("scope {scope:?}"),
                Cmd::Refresh => "refresh".into(),
                Cmd::Submit {
                    local,
                    target,
                    text,
                    mode,
                } => format!("submit {local} {target:?} {text} {mode:?}"),
                Cmd::Control { local, target, op } => format!("control {local} {target:?} {op:?}"),
                Cmd::Quit(n) => format!("quit {n}"),
            })
            .collect()
    }
    fn fingerprint(a: &App) -> String {
        format!(
            "{:?} {:?} {:?} {:?} {:?} {} {} {:?} {:?} {:?} {:?} {:?} {} {:?}",
            a.focus,
            a.narrow_tab,
            a.overlay,
            a.scope,
            a.list.selected,
            a.prompt.text,
            a.prompt.cursor,
            a.prompt.mode,
            a.prompt.notice,
            a.toast.as_ref().map(|t| &t.text),
            a.exit,
            a.pending_control.as_ref().map(|p| &p.op),
            a.transcript.scroll.top,
            a.transcript.scroll.follow
        )
    }
    fn context_app(overlay: OverlayCtx, focus: Focus) -> App {
        let mut a = app();
        a.caps.controls = true;
        a.focus = focus;
        open(&mut a, (0..4).map(item).collect());
        a.overlay = match overlay {
            OverlayCtx::None => None,
            OverlayCtx::Help => Some(Overlay::Help),
            OverlayCtx::Inbox => Some(Overlay::Inbox),
            OverlayCtx::Confirm => Some(Overlay::Confirm(ControlOp::Abort)),
            OverlayCtx::Picker => Some(Overlay::Picker(Picker {
                filter: String::new(),
                kind: PickerKind::Thinking,
                entries: vec![PickerEntry {
                    label: "high".into(),
                    op: ControlOp::SetThinking("high".into()),
                }],
                selected: 0,
                target: a.transcript.target.clone().unwrap(),
            })),
        };
        a
    }
    #[test]
    fn dispatch_consumes_the_shared_keymap_table_in_every_context() {
        for overlay in [
            OverlayCtx::None,
            OverlayCtx::Help,
            OverlayCtx::Inbox,
            OverlayCtx::Confirm,
            OverlayCtx::Picker,
        ] {
            for focus in [Focus::Sessions, Focus::Transcript, Focus::Prompt] {
                let ctx = KeymapCtx { overlay, focus };
                for (binding, action, _) in keymap::bindings(ctx) {
                    let mut actual = context_app(overlay, focus);
                    let mut expected = context_app(overlay, focus);
                    let cmd_actual = update(&mut actual, Msg::Key(binding.event()));
                    let cmd_expected = dispatch(&mut expected, *action);
                    save_draft(&mut expected);
                    follow_bottom(&mut expected);
                    assert_eq!(
                        command_signature(cmd_actual),
                        command_signature(cmd_expected),
                        "{} {ctx:?} {action:?}",
                        binding.label
                    );
                    assert_eq!(
                        fingerprint(&actual),
                        fingerprint(&expected),
                        "{} {ctx:?}",
                        binding.label
                    );
                }
            }
        }
        assert_eq!(
            keymap::action(
                KeymapCtx {
                    overlay: OverlayCtx::None,
                    focus: Focus::Prompt
                },
                KeyEvent::new(KeyCode::Char('\n'), KeyModifiers::NONE)
            ),
            Some(Action::Newline)
        );
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use crate::{
        render::palette::{ColorDepth, Theme},
        tui::theme::TuiTheme,
    };
    use std::collections::HashMap;
    use std::{
        path::PathBuf,
        time::{Instant, SystemTime},
    };
    fn empty() -> App {
        App::new(
            TuiTheme {
                depth: ColorDepth::None,
                theme: Theme::Dark,
            },
            Scope::Bucket {
                cwd: PathBuf::from("/fixture"),
                label: "fixture".into(),
            },
            (90, 20),
            Instant::now(),
            SystemTime::UNIX_EPOCH,
        )
    }
    #[test]
    fn no_selected_row_cannot_submit_and_paste_still_normalizes_crlf() {
        let mut a = empty();
        a.focus = Focus::Prompt;
        update(&mut a, Msg::Paste("a\r\nb".into()));
        assert_eq!(a.prompt.text, "a\nb");
        assert!(
            update(
                &mut a,
                Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            )
            .is_empty()
        );
        assert!(a.sends.is_empty());
    }
    #[test]
    fn pending_confirmation_cannot_apply_to_a_new_continuation() {
        let mut a = empty();
        a.caps.controls = true;
        a.transcript.target = Some(Target {
            id: "old".into(),
            path: None,
        });
        a.list.selected = Some("old".into());
        update(
            &mut a,
            Msg::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)),
        );
        assert!(matches!(
            a.overlay,
            Some(Overlay::Confirm(ControlOp::Abort))
        ));
        update(
            &mut a,
            Msg::Follow {
                r#gen: 0,
                ev: FollowMsg::Continued {
                    to_id: "new".into(),
                    to_path: None,
                    handle: "new".into(),
                },
            },
        );
        assert!(a.overlay.is_none());
        assert!(
            update(
                &mut a,
                Msg::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
            )
            .is_empty()
        );
    }
    #[test]
    fn picker_escape_cancels_and_stale_control_completion_cannot_reopen_it() {
        let mut a = empty();
        a.caps.controls = true;
        a.transcript.target = Some(Target {
            id: "a".into(),
            path: None,
        });
        let cmds = control(&mut a, ControlOp::ListModels, Some(PickerKind::Model));
        let Some(Cmd::Control { local, .. }) = cmds.first() else {
            panic!("control")
        };
        let local = *local;
        let data = ControlData::Models {
            models: vec![ModelChoice {
                provider: "p".into(),
                id: "m".into(),
                label: "M".into(),
            }],
            thinking_levels: vec![],
        };
        update(
            &mut a,
            Msg::ControlDone {
                local,
                op: ControlOp::ListModels,
                result: Ok(data.clone()),
            },
        );
        assert!(
            update(
                &mut a,
                Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            )
            .is_empty()
        );
        assert!(a.overlay.is_none());
        update(
            &mut a,
            Msg::ControlDone {
                local,
                op: ControlOp::ListModels,
                result: Ok(data),
            },
        );
        assert!(a.overlay.is_none());
    }
    #[test]
    fn persisted_follow_status_and_notice_are_kept_separate_from_items() {
        let mut a = empty();
        update(
            &mut a,
            Msg::Follow {
                r#gen: 0,
                ev: FollowMsg::NotYetPersisted,
            },
        );
        assert!(matches!(
            a.transcript.status,
            TranscriptStatus::NotYetPersisted
        ));
        assert_eq!(tick_interval(&a), Duration::from_millis(80));
        update(
            &mut a,
            Msg::Follow {
                r#gen: 0,
                ev: FollowMsg::Notice("── session file rewritten ──".into()),
            },
        );
        assert_eq!(
            a.transcript.lines[0].to_string(),
            "── session file rewritten ──"
        );
        let err: ErrView = GlenError::new(ErrorCode::NotFound, "session disappeared")
            .with_hint("refresh")
            .into();
        update(
            &mut a,
            Msg::Follow {
                r#gen: 0,
                ev: FollowMsg::Failed(err.clone()),
            },
        );
        assert!(matches!(&a.transcript.status,TranscriptStatus::Failed(actual) if actual==&err));
        assert_eq!(tick_interval(&a), Duration::from_millis(250));
    }
    #[test]
    fn an_inbox_cursor_is_retained_when_only_other_files_have_updates() {
        let mut a = empty();
        let mut cursors = HashMap::new();
        cursors.insert("a".into(), crate::inbox::Cursor { offset: 10, ino: 3 });
        let records = vec![InboxRecord {
            message: InboxMsg {
                id: "read".into(),
                session_id: "a".into(),
                job_id: None,
                text: "body".into(),
                is_final: false,
                at: "1970-01-01T00:00:00Z".into(),
                unread: true,
            },
            position: InboxPosition { end: 10, ino: 3 },
        }];
        update(&mut a, Msg::Inbox(InboxSnapshot { records, cursors }));
        assert!(a.inbox.unread_by_session.is_empty());
        update(&mut a, Msg::Inbox(InboxSnapshot::default()));
        assert!(a.inbox.unread_by_session.is_empty());
        assert!(!a.inbox.records[0].unread);
    }
}

#[cfg(test)]
mod timestamp_tests {
    use super::*;
    #[test]
    fn mixed_offsets_and_malformed_timestamps_have_a_total_chronological_order() {
        let invalid = "2026-10-30T00:00 invalid";
        let earlier = "2026-10-30T01:00:00+02:00";
        let later = "2026-10-29T23:30:00Z";
        let mut timestamps = [later, invalid, earlier];
        timestamps.sort_by(|a, b| timestamp_cmp(a, b));
        assert_eq!(timestamps, [invalid, earlier, later]);
        for a in timestamps {
            for b in timestamps {
                for c in timestamps {
                    if timestamp_cmp(a, b).is_le() && timestamp_cmp(b, c).is_le() {
                        assert!(timestamp_cmp(a, c).is_le());
                    }
                }
            }
        }
    }
}
