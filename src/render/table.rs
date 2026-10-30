use super::{palette::Token, style::Painter};
use comfy_table::{Cell, CellAlignment, ContentArrangement};
pub struct Table {
    inner: comfy_table::Table,
    painter: Painter,
}
impl Table {
    pub fn new(painter: &Painter, width: Option<u16>) -> Self {
        let mut inner = comfy_table::Table::new();
        inner
            .load_style(comfy_table::presets::NOTHING)
            .force_no_tty();
        if let Some(w) = width {
            inner
                .set_content_arrangement(ContentArrangement::Dynamic)
                .set_width(w);
        } else {
            inner.set_content_arrangement(ContentArrangement::Disabled);
        }
        Self {
            inner,
            painter: painter.clone(),
        }
    }
    pub fn header(&mut self, cells: &[&str]) {
        self.inner.set_header(
            cells
                .iter()
                .map(|s| Cell::new(self.painter.paint(Token::Muted, &self.painter.bold(s)))),
        );
    }
    pub fn row(&mut self, cells: Vec<String>) {
        self.inner.add_row(cells);
    }
    pub fn align_right(&mut self, col: usize) {
        if let Some(c) = self.inner.column_mut(col) {
            c.set_cell_alignment(CellAlignment::Right);
        }
    }
    pub fn render(&mut self) -> String {
        let last = self.inner.column_count().saturating_sub(1);
        for (i, c) in self.inner.column_iter_mut().enumerate() {
            c.set_padding(if i == last { (0, 0) } else { (0, 2) });
        }
        self.inner
            .to_string()
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::palette::{ColorDepth, Theme};
    #[test]
    fn gutters() {
        let p = Painter::new(ColorDepth::None, Theme::Dark);
        let mut t = Table::new(&p, None);
        t.row(vec!["a".into(), "b".into()]);
        assert_eq!(t.render(), "a  b");
    }
    #[test]
    fn bounded_width() {
        let p = Painter::new(ColorDepth::Ansi256, Theme::Dark);
        let mut t = Table::new(&p, Some(40));
        t.header(&["Name", "Description"]);
        t.row(vec![
            p.paint(Token::Accent, "a long unbrokenpathwithoutanyspacehere"),
            "a lengthy description that wraps to several lines".into(),
        ]);
        let text = t.render();
        for line in anstream::adapter::strip_str(&text).to_string().lines() {
            assert!(line.chars().count() <= 40, "{line:?}");
            assert_eq!(line, line.trim_end());
        }
    }
    #[test]
    fn painted_alignment() {
        let p = Painter::new(ColorDepth::Ansi16, Theme::Dark);
        let mut t = Table::new(&p, None);
        t.row(vec![p.paint(Token::Accent, "same"), "x".into()]);
        t.row(vec!["same".into(), "x".into()]);
        let rendered = t.render();
        let plain = anstream::adapter::strip_str(&rendered).to_string();
        assert_eq!(
            plain.lines().collect::<Vec<_>>(),
            vec!["same  x", "same  x"]
        );
    }
}
