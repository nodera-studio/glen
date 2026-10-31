use super::theme::ansi_color;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

fn sgr(params: &str, style: &mut Style) {
    let mut params = params.split(';');
    while let Some(p) = params.next() {
        let n = if p.is_empty() {
            Some(0)
        } else {
            p.parse::<u16>().ok()
        };
        match n {
            Some(0) => *style = Style::default(),
            Some(1) => *style = style.add_modifier(Modifier::BOLD),
            Some(2) => *style = style.add_modifier(Modifier::DIM),
            Some(22) => {
                // Clearing SGR effects restores an unspecified modifier, as in a fresh Style.
                style.add_modifier.remove(Modifier::BOLD | Modifier::DIM);
            }
            Some(n @ 30..=37) => style.fg = Some(ansi_color((n - 30) as u8)),
            Some(n @ 90..=97) => style.fg = Some(ansi_color((n - 90 + 8) as u8)),
            Some(39) => style.fg = None,
            Some(38) => match params.next() {
                Some("5") => {
                    if let Some(n) = params.next().and_then(|s| s.parse::<u8>().ok()) {
                        style.fg = Some(Color::Indexed(n));
                    }
                }
                Some("2") => {
                    let rgb = [params.next(), params.next(), params.next()]
                        .map(|s| s.and_then(|s| s.parse::<u8>().ok()));
                    if let [Some(r), Some(g), Some(b)] = rgb {
                        style.fg = Some(Color::Rgb(r, g, b));
                    }
                }
                _ => (),
            },
            _ => (),
        }
    }
}
/// Converts trusted renderer SGR to cells. Input must come from `render::transcript`
/// or `Painter`, never raw omp text: `sanitize` guarantees untrusted text holds no ESC.
pub fn lines(s: &str) -> Vec<Line<'static>> {
    let mut style = Style::default();
    s.split('\n')
        .map(|line| {
            let bytes = line.as_bytes();
            let mut spans = vec![];
            let mut i = 0;
            let mut start = 0;
            while i < bytes.len() {
                if bytes[i] != 0x1b {
                    i += 1;
                    continue;
                }
                if start < i {
                    spans.push(Span::styled(line[start..i].to_owned(), style));
                }
                i += 1;
                match bytes.get(i) {
                    Some(b'[') => {
                        i += 1;
                        let params = i;
                        while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                            i += 1;
                        }
                        if bytes.get(i) == Some(&b'm') {
                            sgr(&line[params..i], &mut style);
                        }
                        if i < bytes.len() {
                            i += 1;
                        }
                    }
                    Some(b']' | b'P' | b'X' | b'^' | b'_') => {
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == 7 {
                                i += 1;
                                break;
                            }
                            if bytes[i..].starts_with(b"\x1b\\") {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    Some(_) => {
                        while i < bytes.len() && (0x20..=0x2f).contains(&bytes[i]) {
                            i += 1;
                        }
                        if i < bytes.len() {
                            // Consume a whole character even for malformed non-ASCII escapes.
                            i += line[i..].chars().next().map_or(0, char::len_utf8);
                        }
                    }
                    None => (),
                }
                start = i;
            }
            if start < line.len() {
                spans.push(Span::styled(line[start..].to_owned(), style));
            }
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cli::Tools,
        omp::{
            fixtures,
            items::{self, ItemOpts},
            session::Session,
        },
        render::{
            palette::{ColorDepth, Theme, Token},
            transcript,
        },
        tui::theme::TuiTheme,
    };
    use std::path::Path;

    #[test]
    fn every_palette_style_roundtrips() {
        for depth in [
            ColorDepth::None,
            ColorDepth::Ansi16,
            ColorDepth::Ansi256,
            ColorDepth::TrueColor,
        ] {
            for theme in [Theme::Dark, Theme::Light] {
                let theme = TuiTheme { depth, theme };
                for token in [
                    Token::Muted,
                    Token::Border,
                    Token::Accent,
                    Token::Accent2,
                    Token::Success,
                    Token::Warning,
                    Token::Danger,
                    Token::Focus,
                ] {
                    let result = lines(&theme.painter().paint(token, "x"));
                    assert_eq!(result.len(), 1);
                    assert_eq!(
                        result[0].spans,
                        [Span::styled("x", theme.style(token))],
                        "{theme:?} {token:?}"
                    );
                }
                let p = theme.painter();
                assert_eq!(
                    lines(&p.paint(Token::Accent, &p.bold("u")))[0].spans,
                    [Span::styled(
                        "u",
                        theme.style(Token::Accent).patch(theme.bold())
                    )]
                );
                if depth == ColorDepth::None {
                    assert_eq!(theme.bold(), Style::default());
                    assert_eq!(theme.dim(), Style::default());
                }
            }
        }
    }
    #[test]
    fn transcript_fixtures_preserve_every_line() {
        for key in ['A', 'E'] {
            let session = Session::parse(Path::new("fixture"), fixtures::bytes(key)).unwrap();
            for depth in [ColorDepth::TrueColor, ColorDepth::None] {
                let theme = TuiTheme {
                    depth,
                    theme: Theme::Dark,
                };
                let p = theme.painter();
                let items = items::build(
                    &session.active_branch(),
                    &ItemOpts {
                        thinking: true,
                        tools: Tools::Full,
                    },
                );
                let rendered = transcript::render_items(
                    &items,
                    &transcript::Style {
                        painter: &p,
                        width: Some(60),
                        thinking: true,
                        tools: Tools::Full,
                    },
                );
                let parsed = lines(&rendered);
                let text: Vec<String> = parsed
                    .iter()
                    .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                    .collect();
                let stripped = anstream::adapter::strip_str(&rendered).to_string();
                assert_eq!(
                    text,
                    stripped.split('\n').collect::<Vec<_>>(),
                    "fixture {key}"
                );
                if depth == ColorDepth::None {
                    assert!(
                        parsed
                            .iter()
                            .flat_map(|l| &l.spans)
                            .all(|s| s.style == Style::default())
                    );
                }
            }
        }
    }
    #[test]
    fn escapes_resets_and_multiline_style() {
        assert_eq!(
            lines("a\x1b[5;7mb\x1b]0;title\x07c\x1b]0;title\x1b\\d\x1b[2Ke\x1b(Bf")[0].to_string(),
            "abcdef"
        );
        let parsed = lines(
            "\x1b[1;2;31ma\n界\x1b[22;39mb\x1b[38;5;200mc\x1b[38;2;1;2;3md\x1b[0me\x1b[1mf\x1b[mg",
        );
        assert_eq!(
            parsed[0].spans[0].style,
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD | Modifier::DIM)
        );
        assert_eq!(parsed[1].spans[0].style, parsed[0].spans[0].style);
        assert_eq!(parsed[1].spans[1], Span::raw("b"));
        assert_eq!(parsed[1].spans[2].style.fg, Some(Color::Indexed(200)));
        assert_eq!(parsed[1].spans[3].style.fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(parsed[1].spans[4], Span::raw("e"));
        assert_eq!(parsed[1].spans[6], Span::raw("g"));
        assert_eq!(
            lines("a\x1b[38;2;999;2;3mb\x1b[38;5;999mc\x1b[")[0].to_string(),
            "abc"
        );
        for n in 0..16 {
            let sgr = if n < 8 { 30 + n } else { 90 + n - 8 };
            assert_eq!(
                lines(&format!("\x1b[{sgr}mx"))[0].spans[0].style.fg,
                Some(ansi_color(n))
            );
        }
        assert_eq!(lines("a\n\n").len(), 3);
    }
}
