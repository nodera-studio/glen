//! The command surface: visible vs hidden subcommands, help/version, the §A9 argument
//! surface of the commands, and completions (AC-001, AC-002, AC-017; §A9, §A10).

use crate::support::*;
use serde_json::Value;

/// Subcommand names listed under the help "Commands:" heading.
fn listed_commands(help: &str) -> Vec<String> {
    let mut lines = help.lines().skip_while(|l| l.trim() != "Commands:");
    assert!(
        lines.next().is_some(),
        "help has no `Commands:` section:\n{help}"
    );
    let mut names: Vec<String> = lines
        .take_while(|l| !l.trim().is_empty())
        .filter(|l| l.starts_with(' '))
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    names.sort();
    names
}

/// Whole-word match, where `_` counts as a separator so `glen__ls` matches `ls`.
fn contains_command_word(hay: &str, word: &str) -> bool {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '-';
    hay.match_indices(word).any(|(i, _)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + word.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

fn assert_no_hidden_commands(script: &str, what: &str) {
    assert!(!script.contains("__worker"), "{what} exposes __worker");
    for name in HIDDEN {
        assert!(
            !contains_command_word(script, name),
            "{what} mentions hidden subcommand `{name}`:\n{script}"
        );
    }
}

#[test]
fn ac001_help_lists_exactly_the_visible_commands() {
    let sb = Sandbox::new();
    for args in [vec!["--help"], vec!["-h"], vec!["help"]] {
        let out = sb.glen(&args).run();
        out.assert_exit(0);
        assert_eq!(
            listed_commands(&out.out()),
            [
                "completions",
                "doctor",
                "help",
                "inbox",
                "install-extension",
                "install-skill",
                "jobs",
                "ls",
                "send",
                "show",
                "tail",
                "wait",
                "watch"
            ],
            "{}",
            out.ctx()
        );
    }
}

#[test]
fn a10_help_and_version_are_text_even_with_json() {
    let sb = Sandbox::new();
    let help = sb.glen(["--json", "--help"]).run();
    help.assert_exit(0);
    assert!(!help.is_json(), "--help must be text\n{}", help.ctx());
    assert!(help.out().contains("doctor"), "{}", help.ctx());

    let ver = sb.glen(["--version", "--json"]).run();
    ver.assert_exit(0);
    assert!(!ver.is_json(), "--version must be text\n{}", ver.ctx());
}

#[test]
fn version_prints_glen_and_crate_version() {
    let sb = Sandbox::new();
    for flag in ["--version", "-V"] {
        let out = sb.glen([flag]).run();
        out.assert_exit(0);
        assert_eq!(
            out.out().trim(),
            format!("glen {PKG_VERSION}"),
            "{}",
            out.ctx()
        );
    }
}

#[test]
fn a9_every_command_accepts_help_and_version() {
    let sb = Sandbox::new();
    for sub in [
        "ls",
        "show",
        "tail",
        "send",
        "wait",
        "jobs",
        "inbox",
        "watch",
        "install-extension",
        "install-skill",
        "doctor",
        "completions",
    ]
    .into_iter()
    .chain(HIDDEN)
    {
        let help = sb.glen([sub, "--help"]).run();
        help.assert_exit(0);
        assert!(help.out().contains("Usage"), "{}", help.ctx());

        let ver = sb.glen([sub, "--version"]).run();
        ver.assert_exit(0);
        assert!(ver.out().contains(PKG_VERSION), "{}", ver.ctx());
    }
}

#[test]
fn hidden_commands_still_have_help_pages() {
    let sb = Sandbox::new();
    for sub in HIDDEN {
        let out = sb.glen(["help", sub]).run();
        out.assert_exit(0);
        assert!(
            out.out().contains("Usage") && out.out().contains(sub),
            "{}",
            out.ctx()
        );
    }
}

#[test]
fn ac002_watch_requires_an_interactive_terminal() {
    let sb = Sandbox::new();
    let invocations: [&[&str]; 1] = [&["watch"]];
    for args in invocations {
        let out = sb.glen(args).run();
        let err = out.error_envelope("usage");
        out.assert_stderr_empty();
        assert!(
            err["message"].as_str().unwrap().contains(args[0]),
            "message should name the command\n{}",
            out.ctx()
        );
    }
}

#[test]
fn ac002_watch_in_human_mode_reports_on_stderr() {
    let sb = Sandbox::new();
    let out = sb.glen(["watch", "--human"]).run();
    out.assert_exit(2).assert_no_ansi();
    assert!(out.stdout.is_empty(), "stdout must be empty\n{}", out.ctx());
    assert!(out.err().starts_with("error:"), "{}", out.ctx());
}

/// Watch's declared arguments parse; its non-TTY gate runs before session lookup.
#[test]
fn a9_watch_arguments_parse_before_the_terminal_gate() {
    let sb = Sandbox::new();
    let valid: &[&[&str]] = &[&["watch", "abc1234", "--all"]];
    for args in valid {
        let err = sb.glen(*args).run().error_envelope("usage");
        assert!(
            err["message"]
                .as_str()
                .unwrap()
                .contains("interactive terminal")
        );
    }
}

#[test]
fn a9_rpc_arguments_parse() {
    let sb = Sandbox::new();
    let valid: &[&[&str]] = &[
        &["send", "@latest", "hi", "--as", "steer", "--cwd", "/tmp"],
        &["send", "@new", "hi", "--as", "follow-up", "--wait", "2m"],
        &["send", "abc1234", "hi", "--as", "auto", "--no-wait"],
        &["send", "abc1234", "-"],
        &["send", "abc1234", "hi", "--wait", "90s"],
        &["send", "abc1234", "hi", "--wait", "1500ms"],
        &["send", "abc1234", "hi", "--wait", "1h"],
        &["send", "abc1234", "hi", "--wait", "45"],
        &["send", "abc1234", "hi", "--wait", "0"],
        &["send", "abc1234", "hi", "--wait", "24h"],
        &["send", "abc1234", "hi", "--wait", "86400"],
        &["wait", "j1234567", "--timeout", "30s"],
        &["jobs", "--session", "abc1234", "--active", "-n", "3"],
        &["jobs", "--limit", "3"],
    ];
    for args in valid {
        let out = sb.glen(*args).run();
        if args[0] == "jobs" {
            out.ok_data();
        } else if args.contains(&"@new") {
            out.error_envelope("omp_unavailable");
        } else {
            out.error_envelope("not_found");
        }
    }
}

/// Malformed arguments are usage errors (exit 2), including the §A9 duration rules.
#[test]
fn a9_invalid_arguments_are_usage_errors() {
    let sb = Sandbox::new();
    let invalid: &[&[&str]] = &[
        &["send", "abc1234", "hi", "--steer", "--follow-up"],
        &["send", "abc1234", "hi", "--as", "steer", "--steer"],
        &["send", "abc1234", "hi", "--cwd", "/tmp"],
        &["show"],
        &["tail"],
        &["send", "abc1234"],
        &["wait"],
        &["show", "abc1234", "--tools", "bogus"],
        &["send", "abc1234", "hi", "--as", "bogus"],
        &["send", "abc1234", "hi", "--wait", "abc"],
        &["send", "abc1234", "hi", "--wait", "25h"],
        &["send", "abc1234", "hi", "--wait", "86401"],
        &["send", "abc1234", "hi", "--wait", "1441m"],
        &["send", "abc1234", "hi", "--wait", "1h30m"],
        &["send", "abc1234", "hi", "--wait=-1"],
        &["send", "abc1234", "hi", "--wait", "10s", "--no-wait"],
        &["wait", "j1234567", "--timeout", "25h"],
        &["ls", "-n", "abc"],
        &["ls", "--all", "--cwd", "/tmp"],
        &["ls", "--since", "1y"],
        &["--json", "completions"],
        &["--json", "completions", "notashell"],
        &["doctor", "unexpected-positional"],
    ];
    for args in invalid {
        sb.glen(*args).run().error_envelope("usage");
    }
}

#[test]
fn bare_glen_prints_usage_and_exits_2() {
    let sb = Sandbox::new();
    let out = sb.glen::<[&str; 0], &str>([]).run();
    out.assert_exit(2);
    assert!(
        format!("{}{}", out.out(), out.err()).contains("Usage"),
        "{}",
        out.ctx()
    );
}

#[test]
fn ac017_completions_are_raw_scripts_without_hidden_commands() {
    let sb = Sandbox::new();
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let out = sb.glen(["completions", shell]).run();
        out.assert_exit(0).assert_no_ansi();
        assert!(out.stderr.is_empty(), "{}", out.ctx());
        let script = out.out();
        assert!(
            !script.trim_start().starts_with('{') && !out.is_json(),
            "completions must be raw, not JSON\n{}",
            out.ctx()
        );
        assert!(
            contains_command_word(&script, "doctor")
                && contains_command_word(&script, "completions"),
            "{shell} script lacks the visible commands\n{}",
            out.ctx()
        );
        for name in [
            "ls",
            "show",
            "tail",
            "send",
            "wait",
            "jobs",
            "inbox",
            "watch",
            "install-extension",
            "install-skill",
        ] {
            assert!(contains_command_word(&script, name), "{shell} lacks {name}");
        }
        assert_no_hidden_commands(&script, shell);
    }
}

#[test]
fn ac017_zsh_completion_is_a_compdef_script() {
    let sb = Sandbox::new();
    let out = sb.glen(["completions", "zsh"]).run();
    out.assert_exit(0);
    assert_eq!(
        out.out().lines().next(),
        Some("#compdef glen"),
        "{}",
        out.ctx()
    );
}

#[test]
fn ac017_completions_stay_uncoloured_when_colour_is_forced() {
    let sb = Sandbox::new();
    sb.glen(["completions", "bash"])
        .env("CLICOLOR_FORCE", "1")
        .run()
        .assert_exit(0)
        .assert_no_ansi();
    sb.glen(["--color", "always", "completions", "fish"])
        .run()
        .assert_exit(0)
        .assert_no_ansi();
}

#[test]
fn a10_explicit_json_completions_carry_shell_and_script() {
    let sb = Sandbox::new();
    for (args, shell) in [
        (vec!["completions", "zsh", "--json"], "zsh"),
        (vec!["--json", "completions", "bash"], "bash"),
    ] {
        let out = sb.glen(&args).run();
        let data = out.ok_data_or_spilled(&sb.glen_home);
        out.assert_stderr_empty();
        assert_eq!(data["shell"], Value::from(shell), "{}", out.ctx());
        let script = data["script"].as_str().expect("data.script is a string");
        assert!(!script.trim().is_empty(), "{}", out.ctx());
        assert!(contains_command_word(script, "doctor"), "{}", out.ctx());
        assert_no_hidden_commands(script, shell);
        if shell == "zsh" {
            assert!(script.starts_with("#compdef glen"), "{}", out.ctx());
        }
    }
}

#[test]
fn a9_reader_arguments_parse() {
    let sb = Sandbox::new();
    let valid: &[&[&str]] = &[
        &["ls", "--all", "-n", "5", "--live"],
        &["ls", "--cwd", "/tmp", "-n", "5", "--live"],
        &["ls", "--limit", "3"],
        &["show", "abc1234", "--thinking", "--tools", "full", "--raw"],
        &["show", "abc1234", "--tools", "none"],
        &["show", "abc1234", "--tools", "collapsed"],
        &["tail", "abc1234", "-n", "3", "-f", "--thinking"],
        &["tail", "abc1234", "--lines", "3", "--follow"],
        &["--profile", "work", "ls"],
        &["ls", "--color", "never", "--json"],
    ];
    for args in valid {
        let out = sb.glen(*args).run();
        if args.contains(&"show") || args.contains(&"tail") {
            out.error_envelope("not_found");
        } else {
            out.ok_data();
        }
    }
}

#[test]
fn a9_live_arguments_parse() {
    let sb = Sandbox::new();
    for args in [
        vec!["inbox", "--session", "abc1234", "--peek", "--all"],
        vec!["install-extension", "--dry-run", "--force", "--uninstall"],
    ] {
        sb.glen(args).run().ok_data();
    }
}

#[test]
fn a9_install_skill_arguments_parse() {
    let sb = Sandbox::new();
    for args in [
        vec![
            "install-skill",
            "--claude",
            "--codex",
            "--project",
            "--dry-run",
            "--force",
            "--uninstall",
        ],
        vec!["install-skill", "--dry-run"],
    ] {
        sb.glen(args).run().ok_data();
    }
}
