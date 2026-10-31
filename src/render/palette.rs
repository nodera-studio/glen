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
        Token::Accent2 => (
            if light {
                (14, 124, 112)
            } else {
                (18, 165, 148)
            },
            if light { 23 } else { 36 },
            Some(AnsiColor::Cyan),
        ),
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
    #[derive(serde::Deserialize)]
    struct PaletteFile {
        tokens: Vec<Row>,
    }
    #[derive(serde::Deserialize)]
    struct Row {
        name: String,
        light: Shade,
        dark: Shade,
        ansi16: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Shade {
        hex: String,
        xterm: u8,
    }
    fn token(name: &str) -> Token {
        match name {
            "muted" => Token::Muted,
            "border" => Token::Border,
            "accent" => Token::Accent,
            "accent_2" => Token::Accent2,
            "success" => Token::Success,
            "warning" => Token::Warning,
            "danger" => Token::Danger,
            "focus" => Token::Focus,
            _ => panic!("unknown palette token: {name}"),
        }
    }
    fn ansi(name: &str) -> AnsiColor {
        match name {
            "bright_black" => AnsiColor::BrightBlack,
            "blue" => AnsiColor::Blue,
            "cyan" => AnsiColor::Cyan,
            "green" => AnsiColor::Green,
            "yellow" => AnsiColor::Yellow,
            "red" => AnsiColor::Red,
            "bright_blue" => AnsiColor::BrightBlue,
            _ => panic!("unknown ANSI colour: {name}"),
        }
    }
    fn rgb(hex: &str) -> (u8, u8, u8) {
        assert!(
            hex.len() == 7
                && hex.starts_with('#')
                && hex[1..].bytes().all(|c| c.is_ascii_hexdigit())
        );
        let channel = |i| u8::from_str_radix(&hex[i..i + 2], 16).expect("validated hex");
        (channel(1), channel(3), channel(5))
    }
    #[test]
    fn palette_contract() {
        let file: PaletteFile =
            serde_json::from_str(include_str!("palette.json")).expect("valid palette fixture");
        let mut names: Vec<_> = file.tokens.iter().map(|r| r.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "accent", "accent_2", "border", "danger", "focus", "muted", "success", "warning"
            ]
        );
        let rows = file.tokens.iter().map(|r| {
            (
                token(&r.name),
                rgb(&r.light.hex),
                rgb(&r.dark.hex),
                r.light.xterm,
                r.dark.xterm,
                r.ansi16.as_deref().map(ansi),
            )
        });
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
    /// WCAG relative luminance (sRGB) and contrast ratio against white.
    fn contrast_on_white(rgb: (u8, u8, u8)) -> f64 {
        fn channel(c: u8) -> f64 {
            let c = c as f64 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        let l = 0.2126 * channel(rgb.0) + 0.7152 * channel(rgb.1) + 0.0722 * channel(rgb.2);
        (1.0 + 0.05) / (l + 0.05)
    }
    #[test]
    fn light_text_tokens_meet_wcag_aa_on_white() {
        let file: PaletteFile =
            serde_json::from_str(include_str!("palette.json")).expect("valid palette fixture");
        for row in &file.tokens {
            if row.name == "border" {
                continue; // a border, not text; not held to a text contrast bound
            }
            let hex = rgb(&row.light.hex);
            let ratio = contrast_on_white(hex);
            assert!(
                ratio >= 4.5,
                "{}: {:?} is only {ratio:.2}:1 on white",
                row.name,
                row.light.hex
            );
            if row.light.xterm >= 16 {
                // Cube/greyscale indices map to a fixed RGB; terminal-defined indices < 16
                // vary by terminal and are skipped.
                let idx_rgb = xterm_rgb(row.light.xterm);
                assert!(
                    contrast_on_white(idx_rgb) >= 4.5,
                    "{}: xterm {} is only {:.2}:1 on white",
                    row.name,
                    row.light.xterm,
                    contrast_on_white(idx_rgb)
                );
            }
        }
    }
    /// The xterm-256 palette's cube (16-231) and greyscale (232-255) RGB values.
    fn xterm_rgb(idx: u8) -> (u8, u8, u8) {
        if idx >= 232 {
            let v = 8 + (idx - 232) as u32 * 10;
            return (v as u8, v as u8, v as u8);
        }
        let n = idx - 16;
        let levels = [0u32, 95, 135, 175, 215, 255];
        let r = levels[(n / 36) as usize];
        let g = levels[((n / 6) % 6) as usize];
        let b = levels[(n % 6) as usize];
        (r as u8, g as u8, b as u8)
    }
}
