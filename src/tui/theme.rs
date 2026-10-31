use crate::{
    env::Env,
    output::{OutputCtx, decide_depth, decide_theme},
    render::{
        palette::{self, ColorDepth, Theme, Token},
        style::Painter,
    },
};
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TuiTheme {
    pub depth: ColorDepth,
    pub theme: Theme,
}
impl TuiTheme {
    pub fn from_ctx(ctx: &OutputCtx, env: &Env) -> Self {
        Self {
            depth: if ctx.stdout_color {
                decide_depth(env)
            } else {
                ColorDepth::None
            },
            theme: decide_theme(env),
        }
    }
    pub fn painter(&self) -> Painter {
        Painter::new(self.depth, self.theme)
    }
    pub fn style(&self, t: Token) -> Style {
        convert(palette::style(t, self.depth, self.theme))
    }
    pub fn bold(&self) -> Style {
        if self.depth == ColorDepth::None {
            Style::default()
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        }
    }
    pub fn dim(&self) -> Style {
        if self.depth == ColorDepth::None {
            Style::default()
        } else {
            Style::default().add_modifier(Modifier::DIM)
        }
    }
}
pub(super) fn ansi_color(index: u8) -> Color {
    match index {
        4 => Color::Blue,
        6 => Color::Cyan,
        2 => Color::Green,
        3 => Color::Yellow,
        1 => Color::Red,
        8 => Color::DarkGray,
        12 => Color::LightBlue,
        n => Color::Indexed(n),
    }
}
fn convert(s: anstyle::Style) -> Style {
    let mut style = Style {
        fg: s.get_fg_color().map(|c| match c {
            anstyle::Color::Ansi(c) => ansi_color(c as u8),
            anstyle::Color::Ansi256(c) => Color::Indexed(c.0),
            anstyle::Color::Rgb(c) => Color::Rgb(c.0, c.1, c.2),
        }),
        ..Style::default()
    };
    if s.get_effects().contains(anstyle::Effects::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if s.get_effects().contains(anstyle::Effects::DIMMED) {
        style = style.add_modifier(Modifier::DIM);
    }
    style
}
pub const SPINNER: [&str; 29] = [
    "⠁", "⠁", "⠉", "⠙", "⠚", "⠒", "⠂", "⠂", "⠒", "⠲", "⠴", "⠤", "⠄", "⠄", "⠤", "⠠", "⠠", "⠤", "⠦",
    "⠖", "⠒", "⠐", "⠐", "⠒", "⠓", "⠋", "⠉", "⠈", "⠈",
];
/// Indicatif's default spinner, advanced every 80 ms.
pub fn spinner(frame: usize) -> &'static str {
    SPINNER[frame % SPINNER.len()]
}
pub const FOCUS_BAR: &str = "▍";
pub const SELECT_BAR: &str = "▌";
pub const PROMPT: &str = "❯";
pub const REPLY: &str = "↩";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cli::{ColorWhen, GlobalArgs},
        output::{OutputKind, Terminal},
    };
    #[test]
    fn context_controls_color_and_theme() {
        let root = std::path::Path::new("/fixture");
        for (vars, flag, tty, depth) in [
            (
                vec![("NO_COLOR", "1"), ("COLORTERM", "truecolor")],
                ColorWhen::Auto,
                true,
                ColorDepth::None,
            ),
            (
                vec![("CLICOLOR_FORCE", "1"), ("TERM", "xterm-256color")],
                ColorWhen::Auto,
                false,
                ColorDepth::Ansi256,
            ),
            (
                vec![("COLORTERM", "truecolor")],
                ColorWhen::Always,
                false,
                ColorDepth::TrueColor,
            ),
            (
                vec![("CLICOLOR_FORCE", "1")],
                ColorWhen::Never,
                true,
                ColorDepth::None,
            ),
            (
                vec![("TERM", "xterm")],
                ColorWhen::Auto,
                true,
                ColorDepth::Ansi16,
            ),
            (
                vec![("TERM", "xterm")],
                ColorWhen::Auto,
                false,
                ColorDepth::None,
            ),
        ] {
            let mut vars = vars;
            vars.push(("GLEN_THEME", "light"));
            let env = Env::for_test(Some(root), root, &vars);
            let ctx = OutputCtx::new(
                &GlobalArgs {
                    human: true,
                    color: flag,
                    ..GlobalArgs::default()
                },
                OutputKind::Document,
                &env,
                &Terminal {
                    stdout_tty: tty,
                    stderr_tty: true,
                    stdout_cols: None,
                },
            )
            .unwrap();
            assert_eq!(
                TuiTheme::from_ctx(&ctx, &env),
                TuiTheme {
                    depth,
                    theme: Theme::Light
                }
            );
        }
        assert_eq!(SPINNER.concat(), "⠁⠁⠉⠙⠚⠒⠂⠂⠒⠲⠴⠤⠄⠄⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈⠈");
        assert_eq!(spinner(29), spinner(0));
        use anstyle::AnsiColor::*;
        for (index, ansi) in [
            Black,
            Red,
            Green,
            Yellow,
            Blue,
            Magenta,
            Cyan,
            White,
            BrightBlack,
            BrightRed,
            BrightGreen,
            BrightYellow,
            BrightBlue,
            BrightMagenta,
            BrightCyan,
            BrightWhite,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(convert(ansi.on_default()).fg, Some(ansi_color(index as u8)));
        }
    }
}
