use anstyle::{AnsiColor, Color, Effects, Style};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorDepth {
    None,
    Ansi16,
    Ansi256,
    TrueColor,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}
#[derive(Clone, Copy, Debug)]
pub enum Token {
    Muted,
    Border,
    Accent,
    Accent2,
    Success,
    Warning,
    Danger,
    Focus,
}
pub const USER: Token = Token::Accent;
pub const ASSISTANT: Token = Token::Accent2;
pub const TOOL: Token = Token::Muted;
pub fn style(token: Token, depth: ColorDepth, theme: Theme) -> Style {
    if depth == ColorDepth::None {
        return Style::new();
    }
    let light = theme == Theme::Light;
    let (rgb, idx, ansi) = match token {
        Token::Muted => (
            if light { (80, 80, 80) } else { (180, 181, 189) },
            if light { 239 } else { 250 },
            None,
        ),
        Token::Border => (
            if light { (218, 218, 218) } else { (69, 71, 77) },
            if light { 253 } else { 238 },
            Some(AnsiColor::BrightBlack),
        ),
        Token::Accent => (
            if light {
                (94, 106, 210)
            } else {
                (113, 112, 255)
            },
            if light { 62 } else { 63 },
            Some(AnsiColor::Blue),
        ),
        Token::Accent2 => ((18, 165, 148), 36, Some(AnsiColor::Cyan)),
        Token::Success => (
            if light { (0, 127, 61) } else { (0, 202, 80) },
            if light { 29 } else { 41 },
            Some(AnsiColor::Green),
        ),
        Token::Warning => (
            if light { (166, 99, 0) } else { (255, 147, 0) },
            if light { 130 } else { 208 },
            Some(AnsiColor::Yellow),
        ),
        Token::Danger => (
            if light { (220, 38, 38) } else { (255, 86, 95) },
            if light { 160 } else { 203 },
            Some(AnsiColor::Red),
        ),
        Token::Focus => (
            if light {
                (64, 105, 253)
            } else {
                (148, 173, 255)
            },
            if light { 12 } else { 111 },
            Some(AnsiColor::BrightBlue),
        ),
    };
    match depth {
        ColorDepth::None => Style::new(),
        ColorDepth::Ansi16 => ansi.map_or_else(
            || Style::new().effects(Effects::DIMMED),
            |c| Style::new().fg_color(Some(Color::Ansi(c))),
        ),
        ColorDepth::Ansi256 => {
            Style::new().fg_color(Some(Color::Ansi256(anstyle::Ansi256Color(idx))))
        }
        ColorDepth::TrueColor => {
            Style::new().fg_color(Some(Color::Rgb(anstyle::RgbColor(rgb.0, rgb.1, rgb.2))))
        }
    }
}
pub fn clap_styles() -> clap::builder::Styles {
    clap::builder::Styles::styled()
        .header(Style::new().bold())
        .usage(Style::new().bold())
        .literal(AnsiColor::Blue.on_default())
        .placeholder(Style::new().dimmed())
        .error(AnsiColor::Red.on_default().bold())
        .valid(AnsiColor::Green.on_default())
        .invalid(AnsiColor::Yellow.on_default())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_contract() {
        let rows = [
            (Token::Muted, (80, 80, 80), (180, 181, 189), 239, 250, None),
            (
                Token::Border,
                (218, 218, 218),
                (69, 71, 77),
                253,
                238,
                Some(AnsiColor::BrightBlack),
            ),
            (
                Token::Accent,
                (94, 106, 210),
                (113, 112, 255),
                62,
                63,
                Some(AnsiColor::Blue),
            ),
            (
                Token::Accent2,
                (18, 165, 148),
                (18, 165, 148),
                36,
                36,
                Some(AnsiColor::Cyan),
            ),
            (
                Token::Success,
                (0, 127, 61),
                (0, 202, 80),
                29,
                41,
                Some(AnsiColor::Green),
            ),
            (
                Token::Warning,
                (166, 99, 0),
                (255, 147, 0),
                130,
                208,
                Some(AnsiColor::Yellow),
            ),
            (
                Token::Danger,
                (220, 38, 38),
                (255, 86, 95),
                160,
                203,
                Some(AnsiColor::Red),
            ),
            (
                Token::Focus,
                (64, 105, 253),
                (148, 173, 255),
                12,
                111,
                Some(AnsiColor::BrightBlue),
            ),
        ];
        for (token, light, dark, li, di, ansi) in rows {
            for (theme, rgb, idx) in [(Theme::Light, light, li), (Theme::Dark, dark, di)] {
                assert_eq!(
                    style(token, ColorDepth::TrueColor, theme).get_fg_color(),
                    Some(Color::Rgb(anstyle::RgbColor(rgb.0, rgb.1, rgb.2)))
                );
                assert_eq!(
                    style(token, ColorDepth::Ansi256, theme).get_fg_color(),
                    Some(Color::Ansi256(anstyle::Ansi256Color(idx)))
                );
                let s = style(token, ColorDepth::Ansi16, theme);
                assert_eq!(s.get_fg_color(), ansi.map(Color::Ansi));
                if ansi.is_none() {
                    assert!(s.get_effects().contains(Effects::DIMMED));
                }
                assert_eq!(style(token, ColorDepth::None, theme), Style::new());
            }
        }
    }
}
