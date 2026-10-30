//! A byte-indexed, grapheme-boundary editor. Text remains raw until display.
use super::state::ErrView;
use crate::{
    cli::DeliverAs,
    render::{sanitize, text},
};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug)]
pub struct Prompt {
    pub text: String,
    pub cursor: usize,
    pub mode: DeliverAs,
    pub history: Vec<String>,
    pub hist: Option<usize>,
    pub locked: Option<u64>,
    pub notice: Option<ErrView>,
    /// Restore the unsent input after navigating past the most recent history entry.
    history_draft: Option<(String, usize)>,
}
impl Default for Prompt {
    fn default() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            mode: DeliverAs::Auto,
            history: vec![],
            hist: None,
            locked: None,
            notice: None,
            history_draft: None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Draft {
    pub text: String,
    pub cursor: usize,
    pub mode: DeliverAs,
    pub locked: Option<u64>,
    pub notice: Option<ErrView>,
}
impl Default for Draft {
    fn default() -> Self {
        Prompt::default().draft()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Left,
    Right,
    Home,
    End,
    Backspace,
    Delete,
    WordBefore,
    LineBefore,
    LineAfter,
    HistoryUp,
    HistoryDown,
}
impl Prompt {
    pub fn draft(&self) -> Draft {
        Draft {
            text: self.text.clone(),
            cursor: self.cursor,
            mode: self.mode,
            locked: self.locked,
            notice: self.notice.clone(),
        }
    }
    pub fn load(&mut self, draft: Draft, history: &[String]) {
        self.text = draft.text;
        self.cursor = draft.cursor;
        self.mode = draft.mode;
        self.locked = draft.locked;
        self.notice = draft.notice;
        self.history = history.to_vec();
        self.hist = None;
        self.history_draft = None;
        self.boundary();
    }
    fn boundary(&mut self) {
        self.cursor = self.cursor_boundary();
    }
    fn cursor_boundary(&self) -> usize {
        self.text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(std::iter::once(self.text.len()))
            .take_while(|i| *i <= self.cursor)
            .last()
            .unwrap_or(0)
    }
    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }
    fn next(&self) -> usize {
        self.cursor
            + self.text[self.cursor..]
                .graphemes(true)
                .next()
                .map_or(0, str::len)
    }
    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }
    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i)
    }
    fn changed(&mut self) {
        self.hist = None;
        self.history_draft = None;
        self.notice = None;
    }
    pub fn insert(&mut self, s: &str) {
        if self.locked.is_some() {
            return;
        }
        self.boundary();
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.changed();
    }
    pub fn insert_paste(&mut self, s: &str) {
        self.insert(&s.replace("\r\n", "\n").replace('\r', "\n"));
    }
    pub fn clear(&mut self) {
        if self.locked.is_some() {
            return;
        }
        self.text.clear();
        self.cursor = 0;
        self.changed();
    }
    pub fn cycle_mode(&mut self) {
        if self.locked.is_none() {
            self.mode = match self.mode {
                DeliverAs::Auto => DeliverAs::Steer,
                DeliverAs::Steer => DeliverAs::FollowUp,
                DeliverAs::FollowUp => DeliverAs::Auto,
            };
        }
    }
    pub fn edit(&mut self, edit: Edit) {
        if self.locked.is_some() {
            return;
        }
        self.boundary();
        match edit {
            Edit::Left => self.cursor = self.previous(),
            Edit::Right => self.cursor = self.next(),
            Edit::Home => self.cursor = self.line_start(),
            Edit::End => self.cursor = self.line_end(),
            Edit::Backspace => {
                let i = self.previous();
                self.text.drain(i..self.cursor);
                self.cursor = i;
                self.changed();
            }
            Edit::Delete => {
                self.text.drain(self.cursor..self.next());
                self.changed();
            }
            Edit::LineBefore => {
                let i = self.line_start();
                self.text.drain(i..self.cursor);
                self.cursor = i;
                self.changed();
            }
            Edit::LineAfter => {
                self.text.drain(self.cursor..self.line_end());
                self.changed();
            }
            Edit::WordBefore => {
                let mut start = self.cursor;
                let mut chars = self.text[..self.cursor].char_indices().rev().peekable();
                while let Some(&(i, c)) = chars.peek() {
                    if !c.is_whitespace() {
                        break;
                    }
                    start = i;
                    chars.next();
                }
                for (i, c) in chars {
                    if c.is_whitespace() {
                        break;
                    }
                    start = i;
                }
                self.text.drain(start..self.cursor);
                self.cursor = start;
                self.changed();
            }
            Edit::HistoryUp | Edit::HistoryDown => self.history_move(edit == Edit::HistoryUp),
        }
    }
    fn history_move(&mut self, up: bool) {
        if self.text.contains('\n') || self.history.is_empty() {
            return;
        }
        if up {
            if self.hist.is_none() {
                self.history_draft = Some((self.text.clone(), self.cursor));
            }
            let i = self
                .hist
                .map_or(self.history.len() - 1, |i| i.saturating_sub(1));
            self.hist = Some(i);
            self.text = self.history[i].clone();
            self.cursor = self.text.len();
        } else if let Some(i) = self.hist {
            if i + 1 < self.history.len() {
                self.hist = Some(i + 1);
                self.text = self.history[i + 1].clone();
                self.cursor = self.text.len();
            } else {
                self.hist = None;
                if let Some((text, cursor)) = self.history_draft.take() {
                    self.text = text;
                    self.cursor = cursor;
                }
            }
        }
    }
    /// Visual body lines and cursor (column, row), reserving the `❯ ` prefix.
    pub fn display(&self, width: u16) -> (Vec<String>, (u16, u16)) {
        let prefix = text::width("❯ ").min(usize::from(width));
        let available = usize::from(width).saturating_sub(prefix).max(1);
        let mut lines = vec![String::new()];
        let mut col = 0usize;
        let mut row = 0usize;
        let mut cursor = (0, 0);
        let point = self.cursor_boundary();
        for (i, raw) in self
            .text
            .grapheme_indices(true)
            .chain(std::iter::once((self.text.len(), "")))
        {
            let newline = raw.ends_with('\n');
            let shown = sanitize(raw.strip_suffix('\n').unwrap_or(raw));
            for (part, glyph) in shown.graphemes(true).enumerate() {
                let cells = text::width(glyph);
                if col > 0 && col + cells > available {
                    lines.push(String::new());
                    row += 1;
                    col = 0;
                }
                if i == point && part == 0 {
                    cursor = (
                        (prefix + col).min(usize::from(width.saturating_sub(1))) as u16,
                        row.min(u16::MAX as usize) as u16,
                    );
                }
                lines[row].push_str(glyph);
                col += cells;
            }
            if i == self.text.len() || newline {
                if i == point && shown.is_empty() {
                    let (x, y) = if col >= available {
                        (prefix, row + 1)
                    } else {
                        (prefix + col, row)
                    };
                    cursor = (
                        x.min(usize::from(width.saturating_sub(1))) as u16,
                        y.min(u16::MAX as usize) as u16,
                    );
                    if i == self.text.len() && y > row {
                        lines.push(String::new());
                    }
                }
                if newline {
                    lines.push(String::new());
                    row += 1;
                    col = 0;
                }
            }
        }
        (lines, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paste_literal_crlf_is_one_newline() {
        let mut p = Prompt::default();
        p.insert_paste("a\r\nb");
        assert_eq!(p.text, "a\nb");
        assert_eq!(p.cursor, 3);
    }
    #[test]
    fn grapheme_wrap_cursor_and_edits_match_rendered_clusters() {
        let mut p = Prompt::default();
        p.insert("👩‍💻你好世界");
        let (lines, cursor) = p.display(8);
        assert_eq!(lines, ["👩‍💻你好", "世界"]);
        assert_eq!(cursor, (6, 1));
        assert_eq!(ratatui::text::Span::raw(&lines[0]).width(), 6);
        assert_eq!(
            ratatui::text::Span::raw(&lines[1]).width() + 2,
            usize::from(cursor.0)
        );
        p.clear();
        p.insert("👩‍💻e\u{301}你");
        assert_eq!(p.display(20).1, (7, 0));
        p.edit(Edit::Left);
        p.edit(Edit::Backspace);
        assert_eq!(p.text, "👩‍💻你");
        assert_eq!(p.cursor, "👩‍💻".len());
        p.edit(Edit::Home);
        p.edit(Edit::Delete);
        assert_eq!(p.text, "你");
        p.clear();
        p.insert("🇷🇴x");
        assert_eq!(p.display(20).1, (5, 0));
    }
    #[test]
    fn paste_mixed_newlines_and_controls_remains_raw_until_display() {
        let mut p = Prompt::default();
        p.insert_paste("a\r\nb\rc\n\x1b\t");
        assert_eq!(p.text, "a\nb\nc\n\x1b\t");
        let (lines, cursor) = p.display(30);
        assert_eq!(lines, ["a", "b", "c", r"\x1b\t"]);
        assert_eq!(cursor, (8, 3));
        assert!(lines.iter().all(|l| !l.chars().any(char::is_control)));
    }
    #[test]
    fn editing_respects_utf8_boundaries() {
        let mut p = Prompt::default();
        p.insert("aé界z");
        assert_eq!(p.cursor, 7);
        p.edit(Edit::Left);
        assert_eq!(p.cursor, 6);
        p.edit(Edit::Left);
        assert_eq!(p.cursor, 3);
        p.edit(Edit::Delete);
        assert_eq!(p.text, "aéz");
        p.edit(Edit::Backspace);
        assert_eq!(p.text, "az");
        assert_eq!(p.cursor, 1);
        p.edit(Edit::Home);
        p.edit(Edit::Backspace);
        assert_eq!(p.text, "az");
        p.edit(Edit::End);
        p.edit(Edit::Delete);
        assert_eq!(p.text, "az");
    }
    #[test]
    fn word_and_line_deletes_preserve_other_lines() {
        let mut p = Prompt::default();
        p.insert("first\né界 words  ");
        p.edit(Edit::WordBefore);
        assert_eq!(p.text, "first\né界 ");
        p.edit(Edit::WordBefore);
        assert_eq!(p.text, "first\n");
        p.insert("left right");
        p.edit(Edit::Left);
        p.edit(Edit::Left);
        p.edit(Edit::LineBefore);
        assert_eq!(p.text, "first\nht");
        assert_eq!(p.cursor, 6);
        p.edit(Edit::LineAfter);
        assert_eq!(p.text, "first\n");
        p.edit(Edit::Home);
        assert_eq!(p.cursor, 6);
        p.edit(Edit::Left);
        p.edit(Edit::Home);
        assert_eq!(p.cursor, 0);
        p.edit(Edit::End);
        assert_eq!(p.cursor, 5);
    }
    #[test]
    fn history_restores_the_unsent_draft_and_cursor() {
        let mut p = Prompt {
            history: vec!["one".into(), "two".into()],
            ..Prompt::default()
        };
        p.insert("draft");
        p.edit(Edit::Left);
        p.edit(Edit::HistoryUp);
        assert_eq!(p.text, "two");
        p.edit(Edit::HistoryUp);
        assert_eq!(p.text, "one");
        p.edit(Edit::HistoryUp);
        assert_eq!(p.text, "one");
        p.edit(Edit::HistoryDown);
        assert_eq!(p.text, "two");
        p.edit(Edit::HistoryDown);
        assert_eq!(p.text, "draft");
        assert_eq!(p.cursor, 4);
        assert_eq!(p.hist, None);
        p.insert("\n");
        let text = p.text.clone();
        p.edit(Edit::HistoryUp);
        assert_eq!(p.text, text);
    }
    #[test]
    fn locked_editor_cannot_modify_a_routing_draft() {
        let mut p = Prompt::default();
        p.insert("é");
        p.locked = Some(4);
        p.insert_paste("x");
        p.edit(Edit::Backspace);
        p.edit(Edit::Left);
        p.clear();
        p.cycle_mode();
        assert_eq!(p.text, "é");
        assert_eq!(p.cursor, 2);
        assert_eq!(p.mode, DeliverAs::Auto);
    }
    #[test]
    fn display_uses_cell_width_and_wraps_with_the_prompt_prefix() {
        let mut p = Prompt::default();
        p.insert("é界ab");
        let (lines, cursor) = p.display(6);
        assert_eq!(lines, ["é界a", "b"]);
        assert_eq!(cursor, (3, 1));
        p.cursor = 2;
        assert_eq!(p.display(6).1, (3, 0));
        p.cursor = 5;
        assert_eq!(p.display(6).1, (5, 0));
        p.cursor = 6;
        assert_eq!(p.display(6).1, (2, 1));
        p.insert("\n");
        assert_eq!(p.display(6).0, ["é界a", "b"]);
        for width in [0, 1, 2, 3, 4] {
            let _ = p.display(width);
        }
    }
    #[test]
    fn exact_full_line_has_a_visible_insertion_point() {
        let mut p = Prompt::default();
        p.insert("abcd");
        assert_eq!(p.display(6), (vec!["abcd".into(), "".into()], (2, 1)));
    }
    #[test]
    fn draft_loading_clamps_invalid_byte_cursors() {
        let mut p = Prompt::default();
        p.load(
            Draft {
                text: "界".into(),
                cursor: 2,
                ..Draft::default()
            },
            &[],
        );
        assert_eq!(p.cursor, 0);
        p.edit(Edit::Right);
        assert_eq!(p.cursor, 3);
    }
}

#[cfg(test)]
mod display_regressions {
    use super::*;
    #[test]
    fn escaped_controls_wrap_without_exceeding_the_body_width() {
        let mut p = Prompt::default();
        p.insert("\x1b");
        assert_eq!(
            p.display(4),
            (vec!["\\x".into(), "1b".into(), "".into()], (2, 2))
        );
        assert!(p.display(4).0.iter().all(|line| text::width(line) <= 2));
        p.clear();
        p.insert("a\n\nb");
        assert_eq!(p.display(20).0, ["a", "", "b"]);
    }
}
