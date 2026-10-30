use super::{
    palette::Token,
    style::Painter,
    text::{self, clean_lines},
};
use crate::{
    cli::Tools,
    env::Env,
    omp::{
        items::{Item, ToolCall},
        lease::State,
    },
};
use std::{path::Path, time::SystemTime};
pub struct Style<'a> {
    pub painter: &'a Painter,
    pub width: Option<u16>,
    pub tools: Tools,
    pub thinking: bool,
}
fn safe(s: &str) -> String {
    text::single_line(s)
}
fn muted(s: &str, st: &Style<'_>) -> String {
    st.painter.paint(Token::Muted, &super::sanitize(s))
}
fn body(s: &str, token: Option<Token>, st: &Style<'_>) -> String {
    let mut code = false;
    clean_lines(s)
        .into_iter()
        .flat_map(|line| {
            if line.starts_with("```") {
                code = !code;
                return vec![format!("  {}", muted(&line, st))];
            }
            if code {
                return vec![format!(
                    "  {}{}",
                    st.painter.paint(Token::Border, "│ "),
                    line
                )];
            }
            text::wrap(&line, st.width.map(|w| usize::from(w).saturating_sub(2)))
                .into_iter()
                .map(|l| {
                    format!(
                        "  {}",
                        if let Some(t) = token {
                            st.painter.paint(t, &l)
                        } else if l.starts_with('#') {
                            st.painter.bold(&l)
                        } else {
                            l
                        }
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn role(
    name: &str,
    token: Token,
    timestamp: &Option<String>,
    model: Option<&str>,
    st: &Style<'_>,
) -> String {
    let mut s = st.painter.paint(token, &st.painter.bold(name));
    if let Some(m) = model {
        s.push_str(&muted(
            &format!(" · {}", m.rsplit('/').next().unwrap_or(m)),
            st,
        ))
    }
    if let Some(t) = timestamp
        .as_deref()
        .and_then(|s| s.parse::<jiff::Timestamp>().ok())
    {
        s.push_str(&muted(&format!(" · {}", text::local_hm(&t)), st))
    }
    s
}
fn tool(call: &ToolCall, st: &Style<'_>) -> String {
    let p = st.painter;
    let status = if let Some(r) = &call.result {
        let lines = clean_lines(&r.text);
        let first = lines.first().map(String::as_str).unwrap_or("");
        if r.is_error {
            format!("{} {first}", p.fail())
        } else {
            format!(
                "{} {}",
                p.ok(),
                if lines.len() == 1 && text::width(first) <= 40 {
                    first.into()
                } else {
                    format!("{} lines", lines.len())
                }
            )
        }
    } else {
        muted("· no result", st)
    };
    let plain_prefix = format!("↳ {} {}", safe(&call.name), safe(&call.summary));
    let mut s = format!(
        "  ↳ {} {} · {status}",
        p.bold(&safe(&call.name)),
        muted(&safe(&call.summary), st)
    );
    if st.width.is_some_and(|w| {
        text::width(&anstream::adapter::strip_str(&s).to_string()) > usize::from(w)
    }) {
        s = body(
            &format!("{plain_prefix} · {}", anstream::adapter::strip_str(&status)),
            None,
            st,
        )
    }
    if matches!(st.tools, Tools::Full) {
        if let Some(args) = &call.arguments {
            s.push('\n');
            s.push_str(&gutter(
                &serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string()),
                st,
            ));
        }
        if let Some(r) = &call.result {
            s.push('\n');
            s.push_str(&gutter(&r.text, st));
        }
    }
    s
}
fn gutter(text: &str, st: &Style<'_>) -> String {
    clean_lines(text)
        .iter()
        .map(|l| format!("  {}{}", st.painter.paint(Token::Border, "│ "), l))
        .collect::<Vec<_>>()
        .join("\n")
}
fn number(n: u64) -> String {
    let s = n.to_string();
    s.chars()
        .enumerate()
        .map(|(i, c)| {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                format!(",{c}")
            } else {
                c.to_string()
            }
        })
        .collect()
}
fn divider(s: &str, st: &Style<'_>) -> String {
    let s = format!("── {s} ──");
    let w = st.width.map_or(48, usize::from);
    st.painter.paint(
        Token::Border,
        &format!(
            "{}{}",
            text::truncate(&s, w),
            "─".repeat(w.saturating_sub(text::width(&s)))
        ),
    )
}
pub fn render_items(items: &[Item], st: &Style<'_>) -> String {
    let mut prev = None;
    items
        .iter()
        .map(|i| render_item(i, &mut prev, st))
        .collect::<Vec<_>>()
        .join("\n\n")
}
pub fn render_item(item: &Item, prev_model: &mut Option<String>, st: &Style<'_>) -> String {
    let p = st.painter;
    match item {
        Item::User {
            text, timestamp, ..
        } => format!(
            "{}\n{}",
            role("❯ user", Token::Accent, timestamp, None, st),
            body(text, None, st)
        ),
        Item::Assistant {
            text,
            timestamp,
            model,
            has_thinking,
            thinking_words,
            thinking,
            tool_calls,
            error,
            stop_reason,
            ..
        } => {
            let mut s = role(
                "◆ assistant",
                Token::Accent2,
                timestamp,
                model.as_deref(),
                st,
            );
            *prev_model = model.clone();
            if *has_thinking {
                if st.thinking {
                    if let Some(t) = thinking {
                        for (i, l) in clean_lines(t).iter().enumerate() {
                            s.push('\n');
                            s.push_str(&body(
                                &format!("{}{}", if i == 0 { "∴ " } else { "  " }, l),
                                Some(Token::Muted),
                                st,
                            ));
                        }
                    }
                } else {
                    s.push_str(&format!(
                        "\n{}",
                        body(
                            &format!("∴ thinking · {thinking_words} words"),
                            Some(Token::Muted),
                            st
                        )
                    ))
                }
            }
            if !text.is_empty() {
                s.push('\n');
                s.push_str(&body(text, None, st));
            }
            for call in tool_calls {
                s.push('\n');
                s.push_str(&tool(call, st));
            }
            if let Some(e) = error {
                s.push_str(&format!(
                    "\n{}",
                    body(&format!("✗ error: {e}"), Some(Token::Danger), st)
                ))
            }
            let warning = match stop_reason.as_deref() {
                Some("aborted") => Some("! aborted"),
                Some("length") => Some("! cut off (max tokens)"),
                _ => None,
            };
            if let Some(w) = warning {
                s.push_str(&format!("\n{}", body(w, Some(Token::Warning), st)))
            }
            s
        }
        Item::ToolResult {
            tool_name,
            text,
            is_error,
            ..
        } => body(
            &format!(
                "↳ {tool_name} · {} {}",
                if *is_error { "✗" } else { "✓" },
                text
            ),
            None,
            st,
        ),
        Item::Bash {
            command,
            exit_code,
            cancelled,
            output,
            ..
        }
        | Item::Python {
            code: command,
            exit_code,
            cancelled,
            output,
            ..
        } => {
            let prefix = if matches!(item, Item::Bash { .. }) {
                "$"
            } else {
                "»"
            };
            let status = if *cancelled {
                "! cancelled".into()
            } else {
                format!(
                    "{} exit {}",
                    if *exit_code == Some(0) { "✓" } else { "✗" },
                    exit_code.map_or("?".into(), |n| n.to_string())
                )
            };
            let mut s = format!(
                "{prefix} {} · {}",
                safe(command.lines().next().unwrap_or("")),
                p.paint(
                    if *cancelled {
                        Token::Warning
                    } else if *exit_code == Some(0) {
                        Token::Success
                    } else {
                        Token::Danger
                    },
                    &status
                )
            );
            if matches!(st.tools, Tools::Full) {
                s.push('\n');
                s.push_str(&gutter(output, st))
            }
            s
        }
        Item::Developer { text, .. } => format!(
            "{}\n{}",
            muted("◇ developer", st),
            body(text, Some(Token::Muted), st)
        ),
        Item::Custom {
            custom_type, text, ..
        } => format!(
            "{}\n{}",
            p.paint(Token::Focus, &format!("◈ {}", safe(custom_type))),
            body(text, None, st)
        ),
        Item::FileMention { files, .. } => files
            .iter()
            .map(|f| {
                muted(
                    &format!(
                        "@ {}{}",
                        f.path,
                        f.line_count
                            .map(|n| format!(" ({n} lines)"))
                            .unwrap_or_default()
                    ),
                    st,
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Item::Compaction {
            tokens_before,
            tokens_after,
            summary,
            ..
        } => {
            let label = format!(
                "compacted · {} → {} tokens",
                tokens_before.map(number).unwrap_or_else(|| "?".into()),
                tokens_after.map(number).unwrap_or_else(|| "?".into())
            );
            format!(
                "{}{}",
                divider(&label, st),
                summary
                    .as_ref()
                    .map(|s| format!("\n{}", body(s, Some(Token::Muted), st)))
                    .unwrap_or_default()
            )
        }
        Item::Reset { .. } => divider("context cleared", st),
        Item::BranchSummary { summary, .. } => format!(
            "{}\n{}",
            muted("⤷ branch summary", st),
            body(summary, Some(Token::Muted), st)
        ),
        Item::ModelChange { model, .. } => muted(&format!("· model {model}"), st),
        Item::ThinkingLevel { level, .. } => muted(
            &format!("· thinking level {}", level.as_deref().unwrap_or("unknown")),
            st,
        ),
    }
}
pub fn state_text(state: State, p: &Painter) -> String {
    match state {
        State::Idle => format!("{} {}", p.idle(), p.paint(Token::Muted, "idle")),
        State::Live => format!("{} {}", p.live(), p.paint(Token::Success, "live")),
        State::Busy => format!("{} {}", p.busy(), p.paint(Token::Focus, "busy")),
        State::Unknown => format!("{} {}", p.unknown(), p.paint(Token::Muted, "unknown")),
    }
}
pub struct Header<'a> {
    pub title: Option<&'a str>,
    pub handle: &'a str,
    pub cwd: Option<&'a str>,
    pub model: Option<&'a str>,
    pub state: State,
    pub updated: SystemTime,
    pub continued: &'a [String],
}
pub fn render_header(h: &Header<'_>, env: &Env, st: &Style<'_>, short: bool) -> String {
    let title = h.title.map(safe).unwrap_or_else(|| "untitled".into());
    let state = state_text(h.state, st.painter);
    if short {
        return format!("{} · {} · {state}", safe(h.handle), title);
    }
    let title = if h.title.is_some() {
        st.painter.bold(&title)
    } else {
        muted(&title, st)
    };
    let cwd = h
        .cwd
        .map(|s| super::tilde(Path::new(s), env.home()))
        .unwrap_or_default();
    let model = h
        .model
        .map(|m| m.rsplit('/').next().unwrap_or(m))
        .unwrap_or("");
    let mut detail = format!(
        "{} · {cwd} · {} · {state} · updated {}",
        muted(h.handle, st),
        muted(model, st),
        muted(&text::age(SystemTime::now(), h.updated), st)
    );
    if !h.continued.is_empty() {
        detail.push_str(&muted(
            &format!(" · continued from {}", h.continued.join(", ")),
            st,
        ))
    }
    format!("{title}\n{detail}")
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        omp::{
            fixtures,
            items::{self, ItemOpts},
            session::Session,
        },
        render::palette::{ColorDepth, Theme},
    };
    fn render(k: char, depth: ColorDepth, width: Option<u16>) -> String {
        let s = Session::parse(Path::new("fixture"), fixtures::bytes(k)).unwrap();
        let p = Painter::new(depth, Theme::Dark);
        render_items(
            &items::build(
                &s.active_branch(),
                &ItemOpts {
                    thinking: false,
                    tools: Tools::Collapsed,
                },
            ),
            &Style {
                painter: &p,
                width,
                tools: Tools::Collapsed,
                thinking: false,
            },
        )
    }
    #[test]
    fn exact_a() {
        let hm = text::local_hm(&"2026-10-01T09:00:00Z".parse().unwrap());
        assert_eq!(
            render('A', ColorDepth::None, Some(80)),
            format!(
                "· thinking level high\n\n· model anthropic/claude-sonnet-4-5\n\n❯ user · {hm}\n  Why does parser_test flake?\n\n◆ assistant · claude-sonnet-4-5 · {hm}\n  ∴ thinking · 4 words\n  Let me read the test.\n  ↳ read tests/parser_test.rs · ✓ fn parser_test() {{ … }}\n\n◆ assistant · claude-sonnet-4-5 · {hm}\n  The test depends on map order. Fixed."
            )
        );
    }
    #[test]
    fn width_color_and_controls() {
        let plain = render('A', ColorDepth::None, Some(40));
        for line in plain.lines() {
            assert!(text::width(line) <= 40, "{line}")
        }
        let colored = render('A', ColorDepth::TrueColor, Some(40));
        assert!(colored.contains('\x1b'));
        assert_eq!(anstream::adapter::strip_str(&colored).to_string(), plain);
        let e = render('E', ColorDepth::None, Some(80));
        assert!(e.contains("── compacted · 42,000 → 9,000 tokens"));
        assert!(e.contains("$ cargo test · ✓ exit 0"));
        let p = Painter::new(ColorDepth::None, Theme::Dark);
        let st = Style {
            painter: &p,
            width: Some(40),
            thinking: false,
            tools: Tools::Collapsed,
        };
        let i = Item::User {
            id: "x".into(),
            timestamp: None,
            text: "a\tb\n\x1b[31m".into(),
            images: 0,
        };
        assert!(render_item(&i, &mut None, &st).contains("a    b\n  \\x1b[31m"));
    }
}
