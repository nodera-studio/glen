pub mod envelope;
use crate::{
    cli::{ColorWhen, GlobalArgs},
    env::Env,
    error::{ErrorCode, GlenError, Warning},
    render::palette::{ColorDepth, Theme},
    render::style::Painter,
};
pub use envelope::{Outcome, emit, emit_error};
use std::{
    ffi::OsString,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    process::ExitCode,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Human,
    Json,
    Raw,
}
#[derive(Clone, Copy, Debug)]
pub enum OutputKind {
    Document,
    Raw,
}
pub struct Terminal {
    pub stdout_tty: bool,
    pub stderr_tty: bool,
    pub stdout_cols: Option<u16>,
}
impl Terminal {
    pub fn detect() -> Self {
        Self {
            stdout_tty: io::stdout().is_terminal(),
            stderr_tty: io::stderr().is_terminal(),
            stdout_cols: terminal_size::terminal_size_of(io::stdout()).map(|(w, _)| w.0),
        }
    }
}
pub fn decide_mode(json: bool, human: bool, kind: OutputKind, stdout_tty: bool) -> Mode {
    if json {
        Mode::Json
    } else if human {
        Mode::Human
    } else if matches!(kind, OutputKind::Raw) {
        Mode::Raw
    } else if stdout_tty {
        Mode::Human
    } else {
        Mode::Json
    }
}
pub fn decide_color(flag: ColorWhen, env: &Env, is_tty: bool) -> bool {
    match flag {
        ColorWhen::Always => true,
        ColorWhen::Never => false,
        ColorWhen::Auto => {
            if env.get("NO_COLOR").is_some_and(|v| !v.is_empty()) {
                false
            } else if env
                .get("CLICOLOR_FORCE")
                .is_some_and(|v| !v.is_empty() && v != "0")
            {
                true
            } else if env.get("CLICOLOR") == Some("0") {
                false
            } else {
                is_tty
                    && (env.get("TERM").is_some_and(|v| v != "dumb")
                        || env.get("CLICOLOR").is_some_and(|v| v != "0")
                        || env.get("CI").is_some())
            }
        }
    }
}
pub fn decide_depth(env: &Env) -> ColorDepth {
    if matches!(env.get("COLORTERM"), Some("truecolor" | "24bit")) {
        ColorDepth::TrueColor
    } else if env.get("TERM").is_some_and(|v| v.contains("256color")) {
        ColorDepth::Ansi256
    } else {
        ColorDepth::Ansi16
    }
}
pub fn decide_theme(env: &Env) -> Theme {
    match env.get("GLEN_THEME") {
        Some("light") => return Theme::Light,
        Some("dark") => return Theme::Dark,
        _ => {}
    }
    match env
        .get("COLORFGBG")
        .and_then(|v| v.rsplit(';').next())
        .and_then(|v| v.parse::<u8>().ok())
    {
        Some(7 | 9..=15) => Theme::Light,
        _ => Theme::Dark,
    }
}
pub fn decide_width(env: &Env, term: &Terminal) -> Option<u16> {
    env.get("COLUMNS")
        .and_then(|s| s.parse().ok())
        .filter(|v| (20..=1000).contains(v))
        .or(if term.stdout_tty {
            term.stdout_cols
        } else {
            None
        })
}
pub fn decide_max_output(env: &Env) -> (Option<usize>, Option<Warning>) {
    match env.get("GLEN_MAX_OUTPUT") {
        None => (Some(24000), None),
        Some("0") => (None, None),
        Some(v) => match v.parse::<usize>() {
            Ok(n) => (Some(n), None),
            Err(_) => (
                Some(24000),
                Some(Warning {
                    code: "invalid_env".into(),
                    message: "GLEN_MAX_OUTPUT must be a non-negative integer; using 24000".into(),
                }),
            ),
        },
    }
}
pub struct OutputCtx {
    pub mode: Mode,
    pub stdout_color: bool,
    pub stderr_color: bool,
    pub painter: Painter,
    pub err_painter: Painter,
    pub width: Option<u16>,
    pub stdout_tty: bool,
    pub stderr_tty: bool,
    pub max_output: Option<usize>,
    pub env: Env,
    pub home: Option<PathBuf>,
    pub warnings: Vec<Warning>,
}
impl OutputCtx {
    pub fn new(
        global: &GlobalArgs,
        kind: OutputKind,
        env: &Env,
        term: &Terminal,
    ) -> Result<Self, GlenError> {
        let mode = decide_mode(global.json, global.human, kind, term.stdout_tty);
        let stdout_color = mode == Mode::Human && decide_color(global.color, env, term.stdout_tty);
        let stderr_color = mode != Mode::Json && decide_color(global.color, env, term.stderr_tty);
        let (max_output, warning) = decide_max_output(env);
        let depth = decide_depth(env);
        let theme = decide_theme(env);
        Ok(Self {
            mode,
            stdout_color,
            stderr_color,
            painter: Painter::new(
                if stdout_color {
                    depth
                } else {
                    ColorDepth::None
                },
                theme,
            ),
            err_painter: Painter::new(
                if stderr_color {
                    depth
                } else {
                    ColorDepth::None
                },
                theme,
            ),
            width: decide_width(env, term),
            stdout_tty: term.stdout_tty,
            stderr_tty: term.stderr_tty,
            max_output,
            env: env.clone(),
            home: env.home().map(|p| p.to_path_buf()),
            warnings: warning.into_iter().collect(),
        })
    }
    pub fn fallback(mode: Mode, env: &Env, term: &Terminal) -> Self {
        Self {
            mode,
            stdout_color: false,
            stderr_color: false,
            painter: Painter::new(ColorDepth::None, Theme::Dark),
            err_painter: Painter::new(ColorDepth::None, Theme::Dark),
            width: None,
            stdout_tty: term.stdout_tty,
            stderr_tty: term.stderr_tty,
            max_output: None,
            env: env.clone(),
            home: env.home().map(|p| p.to_path_buf()),
            warnings: vec![],
        }
    }
}
// Inspect only complete flags, consuming global option values and respecting `--`.
fn pre_options(argv: &[OsString]) -> (bool, bool, ColorWhen, OutputKind) {
    let (mut json, mut human, mut color) = (false, false, ColorWhen::Auto);
    let mut kind = OutputKind::Document;
    let mut command_seen = false;
    let mut tokens = argv.iter().skip(1);
    while let Some(token) = tokens.next() {
        let token = token.to_string_lossy();
        match token.as_ref() {
            "--" => break,
            "--json" => json = true,
            "--human" => human = true,
            "--profile" => {
                tokens.next();
            }
            "--color" => {
                color = match tokens.next().and_then(|v| v.to_str()) {
                    Some("always") => ColorWhen::Always,
                    Some("never") => ColorWhen::Never,
                    _ => ColorWhen::Auto,
                };
            }
            value if value.starts_with("--color=") => {
                color = match value.trim_start_matches("--color=") {
                    "always" => ColorWhen::Always,
                    "never" => ColorWhen::Never,
                    _ => ColorWhen::Auto,
                };
            }
            value if !value.starts_with('-') && !command_seen => {
                command_seen = true;
                if value == "completions" {
                    kind = OutputKind::Raw;
                }
            }
            _ => {}
        }
    }
    (json, human, color, kind)
}
pub fn pre_mode(argv: &[OsString], term: &Terminal) -> Mode {
    let (json, human, _, kind) = pre_options(argv);
    decide_mode(json, human, kind, term.stdout_tty)
}
pub fn clap_error(mut e: clap::Error, argv: &[OsString], env: &Env, term: &Terminal) -> ExitCode {
    use clap::{Parser, error::ErrorKind};
    let mode = pre_mode(argv, term);
    let text_only = matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion);
    if mode != Mode::Json || text_only {
        // Rebuild diagnostics from escaped arguments, leaving successful parsing untouched.
        // This keeps clap's trusted layout and styles while escaping only external text.
        let safe_args: Vec<OsString> = argv
            .iter()
            .map(|a| {
                crate::render::sanitize(&a.to_string_lossy())
                    .into_owned()
                    .into()
            })
            .collect();
        e = match crate::cli::Cli::try_parse_from(safe_args) {
            Err(safe) => safe,
            Ok(_) => clap::Error::raw(e.kind(), "invalid arguments"),
        };
        let (_, _, color, _) = pre_options(argv);
        let tty = if e.use_stderr() {
            term.stderr_tty
        } else {
            term.stdout_tty
        };
        let choice = if decide_color(color, env, tty) {
            anstream::ColorChoice::Always
        } else {
            anstream::ColorChoice::Never
        };
        let mut stream = anstream::AutoStream::new(
            if e.use_stderr() {
                Box::new(io::stderr().lock()) as Box<dyn Write>
            } else {
                Box::new(io::stdout().lock()) as Box<dyn Write>
            },
            choice,
        );
        let rendered = e.render().ansi().to_string();
        return ExitCode::from(
            if stream
                .write_all(rendered.as_bytes())
                .and_then(|_| stream.flush())
                .is_ok()
            {
                e.exit_code() as u8
            } else {
                1
            },
        );
    }
    {
        let rendered = e.render().to_string();
        let plain = anstream::adapter::strip_str(&rendered).to_string();
        let message = plain
            .lines()
            .next()
            .unwrap_or("invalid arguments")
            .strip_prefix("error: ")
            .unwrap_or_else(|| plain.lines().next().unwrap_or("invalid arguments"));
        let value = serde_json::json!({"v":1,"ok":false,"error":{"code":"usage","message":message,"details":{"text":plain}}});
        let mut out = anstream::AutoStream::new(io::stdout().lock(), anstream::ColorChoice::Never);
        let result = serde_json::to_vec(&value)
            .map_err(io::Error::other)
            .and_then(|mut bytes| {
                bytes.push(b'\n');
                out.write_all(&bytes)
            })
            .and_then(|_| out.flush());
        ExitCode::from(if result.is_ok() {
            ErrorCode::Usage.exit_code()
        } else {
            1
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    fn env(vars: &[(&str, &str)]) -> Env {
        Env::for_test(Some(Path::new("/h")), Path::new("/w"), vars)
    }
    fn term(tty: bool) -> Terminal {
        Terminal {
            stdout_tty: tty,
            stderr_tty: tty,
            stdout_cols: Some(90),
        }
    }
    #[test]
    fn modes() {
        for tty in [false, true] {
            for kind in [OutputKind::Document, OutputKind::Raw] {
                assert_eq!(decide_mode(true, false, kind, tty), Mode::Json);
                assert_eq!(decide_mode(false, true, kind, tty), Mode::Human);
                assert_eq!(
                    decide_mode(false, false, kind, tty),
                    match kind {
                        OutputKind::Raw => Mode::Raw,
                        _ if tty => Mode::Human,
                        _ => Mode::Json,
                    }
                );
            }
        }
    }
    #[test]
    fn colors() {
        for (vars, tty, want) in [
            (vec![("NO_COLOR", "1"), ("TERM", "xterm")], true, false),
            (vec![("NO_COLOR", ""), ("TERM", "xterm")], true, true),
            (vec![("CLICOLOR_FORCE", "1")], false, true),
            (vec![("CLICOLOR_FORCE", " ")], false, true),
            (vec![("CLICOLOR_FORCE", "0")], false, false),
            (vec![("CLICOLOR_FORCE", "0"), ("TERM", "xterm")], true, true),
            (vec![("CLICOLOR_FORCE", "")], false, false),
            (vec![("CLICOLOR_FORCE", " 0 ")], false, true),
            (
                vec![("NO_COLOR", " "), ("CLICOLOR_FORCE", "1")],
                true,
                false,
            ),
            (
                vec![("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1")],
                true,
                false,
            ),
            (vec![("CLICOLOR", "0"), ("TERM", "xterm")], true, false),
            (vec![("TERM", "dumb")], true, false),
            (vec![], true, false),
            (vec![("CI", "1")], true, true),
            (vec![("CLICOLOR", "1")], true, true),
        ] {
            assert_eq!(
                decide_color(ColorWhen::Auto, &env(&vars), tty),
                want,
                "{vars:?}"
            );
        }
        assert!(decide_color(
            ColorWhen::Always,
            &env(&[("NO_COLOR", "1")]),
            false
        ));
        assert!(!decide_color(
            ColorWhen::Never,
            &env(&[("CLICOLOR_FORCE", "1")]),
            true
        ));
    }
    #[test]
    fn depths() {
        for (vars, want) in [
            (vec![("COLORTERM", "truecolor")], ColorDepth::TrueColor),
            (vec![("COLORTERM", "24bit")], ColorDepth::TrueColor),
            (vec![("TERM", "xterm-256color")], ColorDepth::Ansi256),
            (vec![("TERM", "xterm")], ColorDepth::Ansi16),
        ] {
            assert_eq!(decide_depth(&env(&vars)), want);
        }
    }
    #[test]
    fn themes() {
        for (vars, want) in [
            (vec![("GLEN_THEME", "light")], Theme::Light),
            (vec![("COLORFGBG", "0;15")], Theme::Light),
            (vec![("COLORFGBG", "15;0")], Theme::Dark),
            (vec![], Theme::Dark),
            (
                vec![("GLEN_THEME", "dark"), ("COLORFGBG", "0;15")],
                Theme::Dark,
            ),
        ] {
            assert_eq!(decide_theme(&env(&vars)), want);
        }
    }
    #[test]
    fn widths() {
        assert_eq!(
            decide_width(&env(&[("COLUMNS", "80")]), &term(false)),
            Some(80)
        );
        for v in ["abc", "5", "1001"] {
            assert_eq!(decide_width(&env(&[("COLUMNS", v)]), &term(true)), Some(90));
            assert_eq!(decide_width(&env(&[("COLUMNS", v)]), &term(false)), None);
        }
    }
    #[test]
    fn limits() {
        assert_eq!(decide_max_output(&env(&[])).0, Some(24000));
        assert_eq!(decide_max_output(&env(&[("GLEN_MAX_OUTPUT", "0")])).0, None);
        let (n, w) = decide_max_output(&env(&[("GLEN_MAX_OUTPUT", "abc")]));
        assert_eq!(n, Some(24000));
        assert_eq!(w.unwrap().code, "invalid_env");
    }
    #[test]
    fn early_mode_exact_tokens() {
        for (args, tty, want) in [
            (vec!["glen", "ls", "--json"], true, Mode::Json),
            (vec!["glen", "--human", "x"], false, Mode::Human),
            (vec!["glen", "send", "x", "--", "--json"], true, Mode::Human),
            (vec!["glen", "--jsonx"], true, Mode::Human),
            (vec!["glen", "--json=1"], true, Mode::Human),
            (vec!["glen", "completions", "bad"], false, Mode::Raw),
            (
                vec!["glen", "--profile", "work", "completions"],
                false,
                Mode::Raw,
            ),
            (
                vec!["glen", "--profile=completions", "doctor"],
                false,
                Mode::Json,
            ),
            (vec!["glen", "completions", "--json"], false, Mode::Json),
            (vec!["glen", "--", "completions"], false, Mode::Json),
        ] {
            assert_eq!(
                pre_mode(
                    &args.iter().map(OsString::from).collect::<Vec<_>>(),
                    &term(tty)
                ),
                want
            );
        }
    }
}
