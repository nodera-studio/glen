mod common;
use glen::{
    cli,
    error::ErrorCode,
    skill::{self, Agent},
};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug)]
struct Word {
    text: String,
    quoted: bool,
}
fn words(s: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut quote = None;
    let mut quoted = false;
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            text.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                text.push(c);
            }
            continue;
        }
        if matches!(c, '\'' | '"') {
            quote = Some(c);
            quoted = true;
        } else if c.is_whitespace() {
            if !text.is_empty() {
                out.push(Word {
                    text: std::mem::take(&mut text),
                    quoted,
                });
                quoted = false;
            }
        } else {
            text.push(c);
        }
    }
    if !text.is_empty() {
        out.push(Word { text, quoted });
    }
    out
}
fn spans(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fence = false;
    for line in s.lines() {
        if line.starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence {
            out.push(line.into());
        } else {
            out.extend(
                line.split('`')
                    .enumerate()
                    .filter(|(i, _)| i % 2 == 1)
                    .map(|(_, s)| s.to_owned()),
            );
        }
    }
    out
}
fn invoked(s: &str) -> Vec<Vec<Word>> {
    let mut result = Vec::new();
    for span in spans(s) {
        let mut iter = words(&span).into_iter();
        while let Some(w) = iter.next() {
            if w.text == "glen" {
                let mut invocation = Vec::new();
                for word in iter.by_ref() {
                    if matches!(word.text.as_str(), "|" | "<" | ">" | "&&" | ";" | "||") {
                        break;
                    }
                    invocation.push(word);
                }
                if !invocation.is_empty() {
                    result.push(invocation);
                }
            }
        }
    }
    result
}
fn subcommand_index(tokens: &[Word], cmd: &clap::Command) -> Option<usize> {
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i].text;
        if !t.starts_with('-') {
            return Some(i);
        }
        let name = t.split('=').next().unwrap();
        if let Some(a) = cmd.get_arguments().find(|a| {
            a.get_long().is_some_and(|n| name == format!("--{n}"))
                || a.get_short().is_some_and(|n| name == format!("-{n}"))
        }) && a.get_action().takes_values()
            && !t.contains('=')
        {
            i += 1;
        }
        i += 1;
    }
    None
}
fn exit_errors(s: &str) -> Vec<String> {
    let rows: BTreeMap<u8, String> = s
        .lines()
        .filter_map(|l| {
            let cells: Vec<_> = l.split('|').map(str::trim).collect();
            Some((cells.get(1)?.parse().ok()?, cells.get(2)?.to_string()))
        })
        .collect();
    let mut errors = Vec::new();
    for code in ErrorCode::ALL {
        if !rows
            .get(&code.exit_code())
            .is_some_and(|r| r.contains(&format!("`{}`", code.as_str())))
        {
            errors.push(format!(
                "exit {} missing {}",
                code.exit_code(),
                code.as_str()
            ));
        }
    }
    for exit in [0, 10, 11, 130] {
        if !rows.contains_key(&exit) {
            errors.push(format!("missing exit {exit}"));
        }
    }
    for exit in rows.keys() {
        if ![0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 130].contains(exit) {
            errors.push(format!("unknown exit {exit}"));
        }
    }
    errors.extend(common::exit_code_errors(s, 2));
    errors
}
fn check_options(tokens: &[Word], args: &[&clap::Arg], context: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let mut j = 0;
    while j < tokens.len() {
        let w = &tokens[j];
        if w.text == "--" {
            break;
        }
        if w.text.starts_with('-') && w.text != "-" && w.text != "---" && w.text != "-->" {
            let (flag, inline) = w
                .text
                .split_once('=')
                .map_or((w.text.as_str(), None), |(f, v)| (f, Some(v)));
            let arg = args.iter().find(|a| {
                !a.is_hide_set()
                    && (a.get_long().is_some_and(|n| flag == format!("--{n}"))
                        || a.get_short().is_some_and(|n| flag == format!("-{n}")))
            });
            if let Some(a) = arg {
                let possible = a
                    .get_value_parser()
                    .possible_values()
                    .map(|v| v.map(|v| v.get_name().to_owned()).collect::<Vec<_>>())
                    .unwrap_or_default();
                let value = inline.or_else(|| tokens.get(j + 1).map(|w| w.text.as_str()));
                if a.get_action().takes_values()
                    && !possible.is_empty()
                    && let Some(value) = value
                    && !value.starts_with('<')
                    && !value.starts_with('-')
                    && !possible.iter().any(|v| v == value)
                {
                    errors.push(format!("invalid {flag} value {value} on {context}"));
                }
                if a.get_action().takes_values() && inline.is_none() {
                    j += 1;
                }
            } else {
                errors.push(format!("unknown flag {flag} on {context}"));
            }
        }
        j += 1;
    }
    errors
}
/// Shared plain checker: mutation tests exercise exactly the same checks as the real skill.
fn check_skill(rendered: &str, cmd: &clap::Command) -> Vec<String> {
    let mut errors = exit_errors(rendered);
    let mut seen = BTreeSet::new();
    for tokens in invoked(rendered) {
        let Some(i) = subcommand_index(&tokens, cmd) else {
            errors.extend(check_options(
                &tokens,
                &cmd.get_arguments().collect::<Vec<_>>(),
                "glen",
            ));
            continue;
        };
        errors.extend(check_options(
            &tokens[..i],
            &cmd.get_arguments().collect::<Vec<_>>(),
            "glen",
        ));
        let name = &tokens[i].text;
        if name.starts_with('<') {
            continue;
        }
        let Some(sub) = cmd.find_subcommand(name) else {
            errors.push(format!("unknown command {name}"));
            continue;
        };
        seen.insert(name.to_owned());
        let mut j = i + 1;
        while j < tokens.len() {
            let w = &tokens[j];
            if w.text == "--" {
                break;
            }
            if w.text.starts_with('-') && w.text != "-" {
                let (flag, inline) = w
                    .text
                    .split_once('=')
                    .map_or((w.text.as_str(), None), |(f, v)| (f, Some(v)));
                if matches!(flag, "--help" | "-h" | "--version" | "-V") {
                    j += 1;
                    continue;
                }
                let arg = sub
                    .get_arguments()
                    .chain(cmd.get_arguments().filter(|a| a.is_global_set()))
                    .filter(|a| !a.is_hide_set())
                    .find(|a| {
                        a.get_long().is_some_and(|n| flag == format!("--{n}"))
                            || a.get_short().is_some_and(|n| flag == format!("-{n}"))
                    });
                if let Some(a) = arg {
                    let possible = a
                        .get_value_parser()
                        .possible_values()
                        .map(|v| v.map(|v| v.get_name().to_owned()).collect::<Vec<_>>())
                        .unwrap_or_default();
                    let value = inline
                        .map(|v| (v, false))
                        .or_else(|| tokens.get(j + 1).map(|w| (w.text.as_str(), w.quoted)));
                    if !possible.is_empty()
                        && let Some((value, quoted)) = value
                        && !quoted
                        && !value.starts_with('<')
                        && !possible.iter().any(|v| v == value)
                    {
                        errors.push(format!("invalid {flag} value {value}"));
                    }
                    if a.get_action().takes_values() && inline.is_none() {
                        j += 1;
                    }
                } else {
                    errors.push(format!("unknown flag {flag} on {name}"));
                }
            }
            j += 1;
        }
    }
    let all_args: Vec<_> = cmd
        .get_arguments()
        .chain(cmd.get_subcommands().flat_map(|s| s.get_arguments()))
        .collect();
    let mut section_anchor: Option<String> = None;
    let mut anchor: Option<String> = None;
    let mut fence = false;
    for line in rendered.lines() {
        let heading = line.starts_with('#');
        let table_row = line.starts_with('|');
        if table_row {
            anchor = section_anchor.clone();
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            anchor = section_anchor.clone();
        }
        let list_item = trimmed.starts_with("- ")
            || trimmed
                .split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()));
        if heading {
            section_anchor = None;
            anchor = None;
        } else if list_item {
            anchor = section_anchor.clone();
        }
        if line.starts_with("```") {
            fence = !fence;
            continue;
        }
        for (index, span) in line.split('`').enumerate() {
            if index % 2 == 1 || fence || heading {
                let invocations = invoked(&format!("`{span}`"));
                if !invocations.is_empty() {
                    for invocation in invocations {
                        if let Some(i) = subcommand_index(&invocation, cmd)
                            && cmd.find_subcommand(&invocation[i].text).is_some()
                        {
                            if table_row {
                                continue;
                            }
                            anchor = Some(invocation[i].text.clone());
                            if heading {
                                section_anchor = anchor.clone();
                            }
                        }
                    }
                    continue;
                }
            }
            let scoped_args;
            let args = if let Some(sub) = anchor.as_deref().and_then(|a| cmd.find_subcommand(a)) {
                scoped_args = sub
                    .get_arguments()
                    .chain(cmd.get_arguments().filter(|a| a.is_global_set()))
                    .collect::<Vec<_>>();
                &scoped_args
            } else {
                &all_args
            };
            let normalized = if index % 2 == 0 && !fence {
                span.split_whitespace()
                    .map(|w| w.trim_matches(['(', ')', ',', ';', '.', ':']))
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                span.to_owned()
            };
            let context = anchor.as_deref().unwrap_or("skill prose");
            errors.extend(check_options(&words(&normalized), args, context));
        }
    }
    for sub in cmd
        .get_subcommands()
        .filter(|c| !c.is_hide_set() && c.get_name() != "help")
    {
        for name in std::iter::once(sub.get_name()).chain(sub.get_visible_aliases()) {
            if !seen.contains(name) {
                errors.push(format!("missing command reference {name}"));
            }
        }
    }
    errors
}
fn command() -> clap::Command {
    let mut c = cli::public_command();
    c.build();
    c
}
#[test]
fn ac404_invocations_exist_in_help_and_visible_commands_covered() {
    for a in Agent::ALL {
        assert_eq!(
            check_skill(&skill::render_current(a).unwrap(), &command()),
            Vec::<String>::new()
        );
    }
}
#[test]
fn ac404_checker_mutations() {
    let text = skill::render_current(Agent::Claude).unwrap();
    let c = command();
    assert!(check_skill(&text, &c).is_empty());
    for (mutation, token) in [
        (format!("{text}\n`glen frobnicate`"), "frobnicate"),
        (format!("{text}\n`glen ls --bogus`"), "--bogus"),
        (format!("{text}\n`glen --bogus ls`"), "--bogus"),
        (format!("{text}\n`glen --color=bogus ls`"), "bogus"),
        (format!("{text}\n`--dir DIR`"), "--dir"),
        (format!("{text}\n`--tools fuller`"), "fuller"),
        (format!("{text}\n`--as automatic`"), "automatic"),
        (format!("{text}\n`--nowait`"), "--nowait"),
        (format!("{text}\nUse --dir DIR here."), "--dir"),
        (
            text.replace("`no_route` |", "`no_route`, `io` |"),
            "no_route",
        ),
        (format!("{text}\n`glen jobs --follow-up`"), "--follow-up"),
        (text.replace("glen jobs", "glen ls"), "jobs"),
        (
            text.replace("| 6 | `no_route`", "| 6 | `job_failed`"),
            "no_route",
        ),
        (
            format!("{text}\n`glen show <handle> --tools bogus`"),
            "bogus",
        ),
    ] {
        let errors = check_skill(&mutation, &c);
        assert!(
            errors.iter().any(|e| e.contains(token)),
            "{token}: {errors:?}"
        );
    }
}
#[test]
fn ac404_unguarded_template_mentions() {
    let visible = skill::visible_commands();
    let c = command();
    for segment in skill::parse_template(skill::TEMPLATE).unwrap() {
        for tokens in invoked(&segment.text) {
            if let Some(i) = subcommand_index(&tokens, &c) {
                let name = &tokens[i].text;
                if !name.starts_with('<') {
                    assert!(
                        visible.contains(name) || segment.guards.contains(name),
                        "unguarded {name}"
                    );
                }
            }
        }
    }
}
#[test]
fn ac404_exit_table() {
    assert!(exit_errors(&skill::render_current(Agent::Claude).unwrap()).is_empty());
}
#[test]
fn ac404_help_matches_tree() {
    let out = common::glen(&["--help"], &[]);
    let help = String::from_utf8(out.stdout).unwrap();
    let names: BTreeSet<_> = help
        .split("Commands:\n")
        .nth(1)
        .unwrap()
        .split("\nOptions:")
        .next()
        .unwrap()
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    let c = command();
    assert_eq!(
        names,
        c.get_subcommands()
            .map(|c| c.get_name().to_owned())
            .collect()
    );
}
#[test]
fn ac402_frontmatter_and_marker_ac403_no_guards_ac413_size() {
    for a in Agent::ALL {
        let text = skill::render_current(a).unwrap();
        assert!(text.starts_with("---\nname: glen\n"));
        assert!(
            !text.contains("glen:if") && !text.contains("glen:end") && !text.contains("\n\n\n")
        );
        assert!(text.lines().count() <= 160 && text.len() <= 12 * 1024);
        for heading in [
            "The loop",
            "Exit codes",
            "Output",
            "Sending well",
            "Shell timeouts and sandboxes",
            "Rules",
        ] {
            assert!(text.contains(&format!("## {heading}")));
        }
        for token in [
            "glen_reply",
            "--steer",
            "--follow-up",
            "glen wait",
            "data.forked.to_handle",
            "spilled",
        ] {
            assert!(text.contains(token));
        }
    }
}
#[test]
fn ac404_numeric_defaults() {
    let c = command();
    let text = skill::render_current(Agent::Claude).unwrap();
    for (sub, arg, value, prose) in [
        ("send", "wait", "90s", "default to 90 s"),
        ("wait", "timeout", "90s", "default to 90 s"),
        ("ls", "limit", "20", "default 20"),
        ("jobs", "limit", "20", "default 20 rows"),
        ("tail", "lines", "10", "default 10"),
    ] {
        let a = c
            .find_subcommand(sub)
            .unwrap()
            .get_arguments()
            .find(|a| a.get_id() == arg)
            .unwrap();
        assert_eq!(a.get_default_values(), [std::ffi::OsStr::new(value)]);
        assert!(text.contains(prose), "{prose}");
    }
}

#[test]
fn cycle1_skill_sandbox_guard_exit_guidance_and_plain_command_example() {
    let text = skill::render_current(Agent::Claude).unwrap();
    assert!(!text.contains("support live sessions"));
    assert!(text.contains("Only non-empty `CODEX_SANDBOX` marks a sandbox"));
    assert!(text.contains("`CODEX_SANDBOX_NETWORK_DISABLED=1` alone allows sends"));
    assert!(text.contains("done; read `data`, then use `data.forked.to_handle`"));
    assert!(text.contains("send/wait: `glen wait <job>`; tail -f: stream ended"));
    assert!(text.contains("glen send <handle> -- \"-v is broken\""));
    assert!(!text.contains("< file.md"));
    let visible = skill::visible_commands()
        .into_iter()
        .chain(["install-extension".into()])
        .collect();
    let future = skill::render(Agent::Claude, "0.1.0", &visible).unwrap();
    assert!(future.contains("support live sessions"));
    let faq = include_str!("../site/src/components/FAQ.astro");
    assert!(faq.contains("CODEX_SANDBOX_NETWORK_DISABLED=1</code> alone allows sending"));
}

#[test]
fn cycle2_option_only_and_nearest_prose_anchor_mutations() {
    let text = skill::render_current(Agent::Claude).unwrap();
    let c = command();
    assert!(check_skill(&text, &c).is_empty());
    for mutation in [
        "`glen --bogus`",
        "`glen --color=bogus`",
        "## `glen jobs`\nUse `--tools full`.",
        "## glen jobs\nUse `--tools full`.",
        "- `glen send` then `glen jobs`; use --follow-up.",
        "- `glen jobs` takes `--as auto`.",
    ] {
        let errors = check_skill(&format!("{text}\n{mutation}"), &c);
        assert!(!errors.is_empty(), "{mutation}");
    }
    for valid in [
        "`glen --json`",
        "`glen --color=auto`",
        "## `glen show`\nUse `--tools full`.",
        "- `glen send` then `glen jobs`; use --active.",
        "## Options\nUse `--tools full`.",
    ] {
        assert!(
            check_skill(&format!("{text}\n{valid}"), &c).is_empty(),
            "{valid}"
        );
    }
}
