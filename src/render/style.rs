use super::palette::{self, ColorDepth, Theme, Token};
#[derive(Clone, Debug)]
pub struct Painter {
    depth: ColorDepth,
    theme: Theme,
}
impl Painter {
    pub fn new(depth: ColorDepth, theme: Theme) -> Self {
        Self { depth, theme }
    }
    pub fn paint(&self, token: Token, text: &str) -> String {
        let s = palette::style(token, self.depth, self.theme);
        if self.depth == ColorDepth::None {
            text.to_string()
        } else {
            format!("{s}{text}{s:#}")
        }
    }
    pub fn bold(&self, text: &str) -> String {
        self.effect(anstyle::Style::new().bold(), text)
    }
    pub fn dim(&self, text: &str) -> String {
        self.effect(anstyle::Style::new().dimmed(), text)
    }
    fn effect(&self, s: anstyle::Style, text: &str) -> String {
        if self.depth == ColorDepth::None {
            text.to_string()
        } else {
            format!("{s}{text}{s:#}")
        }
    }
    pub fn ok(&self) -> String {
        self.paint(Token::Success, "✓")
    }
    pub fn warn(&self) -> String {
        self.paint(Token::Warning, "!")
    }
    pub fn fail(&self) -> String {
        self.paint(Token::Danger, "✗")
    }
    pub fn live(&self) -> String {
        self.paint(Token::Success, "●")
    }
    pub fn idle(&self) -> String {
        self.paint(Token::Muted, "○")
    }
    pub fn busy(&self) -> String {
        self.paint(Token::Focus, "◐")
    }
    pub fn unknown(&self) -> String {
        self.paint(Token::Muted, "?")
    }
    pub fn info(&self) -> String {
        self.paint(Token::Muted, "·")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_color() {
        let p = Painter::new(ColorDepth::None, Theme::Dark);
        assert_eq!(p.paint(Token::Accent, "a\x1b café"), "a\x1b café");
        assert_eq!(p.bold("body"), "body");
    }
}
