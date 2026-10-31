//! Saturating geometry and pre-fitted content shared by layout and drawing.
use super::{
    state::{App, Pane},
    view,
};
use crate::render::text;
use ratatui::layout::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    TooSmall,
    Narrow,
    Medium,
    Wide,
}
pub fn mode(w: u16, h: u16) -> Mode {
    if w < 40 || h < 10 {
        Mode::TooSmall
    } else if w >= 100 {
        Mode::Wide
    } else if w >= 72 {
        Mode::Medium
    } else {
        Mode::Narrow
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Areas {
    pub title: Rect,
    pub tabs: Rect,
    pub list: Option<Rect>,
    pub divider: Option<Rect>,
    pub transcript: Option<Rect>,
    pub strip: Rect,
    pub rule: Rect,
    pub notice: Rect,
    pub input: Rect,
    pub status: Rect,
}
/// Handle and prompt body are measured together, including the real handle prefix.
pub fn prompt_lines(app: &App, width: u16) -> (String, Vec<String>, (u16, u16)) {
    let handle = text::truncate(
        &text::single_line(view::handle(app)),
        usize::from(width / 3),
    );
    let handle_width = text::width(&handle)
        .saturating_add(1)
        .min(usize::from(width));
    let (lines, (x, y)) = app
        .prompt
        .display(width.saturating_sub(handle_width as u16));
    (
        handle,
        lines,
        (
            x.saturating_add(handle_width as u16)
                .min(width.saturating_sub(1)),
            y,
        ),
    )
}
pub fn notice_lines(app: &App, width: u16) -> Vec<String> {
    let Some(err) = &app.prompt.notice else {
        return vec![];
    };
    let w = usize::from(width);
    if w == 0 {
        return vec![];
    }
    let marker = if err.is_warning() { "!" } else { "✗" };
    let wrapped = text::wrap(
        &format!("{marker} {}", text::single_line(&err.message)),
        Some(w),
    );
    let cut = wrapped.len() > 2;
    let mut lines = wrapped
        .into_iter()
        .take(2)
        .map(|s| text::truncate(&s, w))
        .collect::<Vec<_>>();
    if cut && let Some(last) = lines.last_mut() {
        *last = text::truncate(&format!("{last}…"), w);
    }
    if let Some(hint) = &err.hint {
        lines.push(text::truncate(&format!("  {}", text::single_line(hint)), w));
    }
    lines
}
pub fn areas(app: &App) -> Areas {
    let (w, h) = app.size;
    let mode = mode(w, h);
    if mode == Mode::TooSmall {
        return Areas {
            title: Rect::new(0, 0, w, u16::from(h > 0)),
            ..Areas::default()
        };
    }
    let input_h = if app.prompt.text.is_empty() && app.list.selected.is_some() {
        2 // Keep the placeholder above the empty editable row.
    } else {
        (prompt_lines(app, w)
            .1
            .len()
            .min(if h >= 40 { 6 } else { 3 }) as u16)
            .max(1)
    };
    let notice_h = notice_lines(app, w).len() as u16;
    let body_h = h
        .saturating_sub(4)
        .saturating_sub(input_h)
        .saturating_sub(notice_h);
    let body = Rect::new(0, 2, w, body_h);
    let mut a = Areas {
        title: Rect::new(0, 0, w, 1),
        tabs: Rect::new(0, 1, w, 1),
        rule: Rect::new(0, body.bottom(), w, 1),
        notice: Rect::new(0, body.bottom().saturating_add(1), w, notice_h),
        input: Rect::new(
            0,
            body.bottom().saturating_add(1).saturating_add(notice_h),
            w,
            input_h,
        ),
        status: Rect::new(0, h.saturating_sub(1), w, 1),
        ..Areas::default()
    };
    let pane = match mode {
        Mode::Wide | Mode::Medium => {
            let list_w = if mode == Mode::Wide { 34 } else { 24 };
            a.list = Some(Rect::new(0, body.y, list_w, body.height));
            a.divider = Some(Rect::new(list_w, body.y, 1, body.height));
            Rect::new(
                list_w + 1,
                body.y,
                w.saturating_sub(list_w + 1),
                body.height,
            )
        }
        Mode::Narrow => {
            if app.narrow_tab == Pane::Sessions {
                a.list = Some(body);
                return a;
            }
            body
        }
        Mode::TooSmall => return a,
    };
    let rows = view::activity_lines(app).len().min(3) as u16;
    let max_strip = body_h / 4;
    let strip_h = if rows == 0 || max_strip < 2 {
        0
    } else {
        (1 + rows).min(max_strip)
    };
    a.transcript = Some(Rect::new(
        pane.x,
        pane.y,
        pane.width,
        pane.height.saturating_sub(strip_h),
    ));
    a.strip = Rect::new(
        pane.x,
        pane.bottom().saturating_sub(strip_h),
        pane.width,
        strip_h,
    );
    a
}
pub fn transcript_width(app: &App) -> u16 {
    let pane_width = match mode(app.size.0, app.size.1) {
        Mode::TooSmall => return 0,
        Mode::Narrow => app.size.0,
        Mode::Medium => app.size.0.saturating_sub(25),
        Mode::Wide => app.size.0.saturating_sub(35),
    };
    pane_width.saturating_sub(2)
}
pub fn inner(rect: Rect) -> Rect {
    Rect::new(
        rect.x.saturating_add(u16::from(rect.width > 0)),
        rect.y,
        rect.width.saturating_sub(2),
        rect.height,
    )
}
/// The hidden transcript keeps the geometry it will have when its narrow tab opens.
pub fn viewport(app: &App) -> (u16, u16) {
    let a = areas(app);
    let height = if let Some(transcript) = a.transcript {
        transcript.height
    } else if let Some(list) = a.list {
        let max = list.height / 4;
        let rows = view::activity_lines(app).len().min(3) as u16;
        list.height.saturating_sub(if max >= 2 && rows > 0 {
            (1 + rows).min(max)
        } else {
            0
        })
    } else {
        0
    };
    (
        transcript_width(app),
        height.saturating_sub(u16::from(!app.transcript.scroll.follow)),
    )
}
pub fn list_page(app: &App) -> usize {
    let a = areas(app);
    let height = a.rule.y.saturating_sub(a.tabs.bottom());
    let stride = if mode(app.size.0, app.size.1) == Mode::Wide {
        2
    } else {
        1
    };
    usize::from(height / stride).max(1)
}
/// The selected row can be scrolled into view without mutating the reducer's offset.
pub fn list_offset(app: &App, area: Rect) -> usize {
    let stride = if mode(app.size.0, app.size.1) == Mode::Wide {
        2
    } else {
        1
    };
    let page = usize::from(area.height / stride).max(1);
    let offset = app
        .list
        .offset
        .min(app.list.rows.len().saturating_sub(page));
    match app
        .list
        .rows
        .iter()
        .position(|r| Some(&r.id) == app.list.selected.as_ref())
    {
        Some(i) if i < offset => i,
        Some(i) if i >= offset + page => i + 1 - page,
        _ => offset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::palette::{ColorDepth, Theme};
    use crate::tui::{theme::TuiTheme, view::tests::scene};
    fn app() -> App {
        scene(TuiTheme {
            depth: ColorDepth::None,
            theme: Theme::Dark,
        })
    }
    #[test]
    fn breakpoints_and_inner_widths_are_exact() {
        for (w, h, expected) in [
            (39, 100, Mode::TooSmall),
            (100, 9, Mode::TooSmall),
            (40, 10, Mode::Narrow),
            (71, 20, Mode::Narrow),
            (72, 20, Mode::Medium),
            (99, 20, Mode::Medium),
            (100, 20, Mode::Wide),
        ] {
            assert_eq!(mode(w, h), expected);
        }
        let mut a = app();
        for (w, expected) in [
            (40, 38),
            (71, 69),
            (72, 45),
            (99, 72),
            (100, 63),
            (300, 263),
        ] {
            a.size = (w, 40);
            assert_eq!(transcript_width(&a), expected);
            assert_eq!(a.narrow(), w < 72);
        }
    }
    #[test]
    fn areas_partition_rows_and_keep_the_strip_within_a_quarter_of_the_body() {
        let mut a = app();
        for w in (1..=300).step_by(7) {
            for h in (1..=100).step_by(5) {
                a.size = (w, h);
                let areas = areas(&a);
                let frame = Rect::new(0, 0, w, h);
                for rect in [
                    areas.title,
                    areas.tabs,
                    areas.strip,
                    areas.rule,
                    areas.notice,
                    areas.input,
                    areas.status,
                ]
                .into_iter()
                .chain(areas.list)
                .chain(areas.divider)
                .chain(areas.transcript)
                {
                    assert!(
                        rect.right() <= frame.right() && rect.bottom() <= frame.bottom(),
                        "{w}x{h}: {rect:?}"
                    );
                }
                if mode(w, h) == Mode::TooSmall {
                    assert!(areas.list.is_none());
                    assert_eq!(areas.title.height, 1);
                    continue;
                }
                assert_eq!(areas.title.height, 1);
                assert_eq!(areas.tabs.y, 1);
                assert_eq!(areas.rule.bottom(), areas.notice.y);
                assert_eq!(areas.notice.bottom(), areas.input.y);
                assert_eq!(areas.input.bottom(), areas.status.y);
                assert_eq!(areas.status.bottom(), h);
                let body = areas.rule.y.saturating_sub(2);
                assert!(areas.strip.height <= body / 4);
                assert!(areas.strip.height == 0 || (2..=4).contains(&areas.strip.height));
                assert!((1..=if h >= 40 { 6 } else { 3 }).contains(&areas.input.height));
                assert!(areas.notice.height <= 3);
            }
        }
    }
    #[test]
    fn narrow_only_allocates_the_active_pane_and_input_reserves_the_handle() {
        let mut a = app();
        a.size = (60, 40);
        a.narrow_tab = Pane::Sessions;
        let left = areas(&a);
        assert!(left.list.is_some());
        assert!(left.transcript.is_none());
        assert_eq!(left.strip.height, 0);
        a.narrow_tab = Pane::Transcript;
        let right = areas(&a);
        assert!(right.list.is_none());
        assert!(right.transcript.is_some());
        assert!(right.divider.is_none());
        let (handle, lines, (x, y)) = prompt_lines(&a, 60);
        assert_eq!(handle, "e27c0de");
        assert!(lines.len() > 1);
        assert!(usize::from(x) >= text::width(&handle) + 3);
        assert!(usize::from(y) < lines.len());
        assert!(
            notice_lines(&a, 40)
                .iter()
                .all(|line| text::width(line) <= 40)
        );
    }
}

#[cfg(test)]
mod paging_tests {
    use super::*;
    use crate::{
        render::palette::{ColorDepth, Theme},
        tui::{
            state::{Focus, Msg},
            theme::TuiTheme,
            update, view,
        },
    };
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    #[test]
    fn wide_list_page_keys_move_one_visible_page_of_two_line_rows() {
        let mut app = view::tests::scene(TuiTheme {
            depth: ColorDepth::None,
            theme: Theme::Dark,
        });
        let row = app.list.rows[0].clone();
        app.list.rows = (0..50)
            .map(|n| {
                let mut r = row.clone();
                r.id = format!("row{n}");
                r
            })
            .collect();
        app.list.selected = Some("row0".into());
        app.focus = Focus::Sessions;
        view::prepare(&mut app);
        let page = usize::from(areas(&app).list.unwrap().height / 2).max(1);
        assert_eq!(app.list_page(), page);
        let cmds = update::update(
            &mut app,
            Msg::Key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)),
        );
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            app.list.selected.as_deref(),
            Some(format!("row{page}").as_str())
        );
        assert_eq!(app.list.offset, 1);
    }
}
