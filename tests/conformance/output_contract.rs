//! Output modes, the JSON envelope, usage errors and exit codes
//! (AC-003, AC-004, AC-005, AC-006, AC-010, AC-021; §A10, §A11).

use crate::support::*;
use serde_json::Value;

#[test]
fn ac003_piped_document_command_prints_one_compact_envelope() {
    let sb = Sandbox::new();
    for out in [sb.glen(["doctor"]).fake_omp().run(), sb.glen(["ls"]).run()] {
        let env = out.json();
        assert_eq!(env["v"], Value::from(1), "{}", out.ctx());
        out.assert_stderr_empty();
        let text = out.out();
        assert!(
            text.ends_with('\n'),
            "envelope ends with a newline\n{}",
            out.ctx()
        );
        assert_eq!(
            text.trim_end_matches('\n').lines().count(),
            1,
            "non-TTY JSON must be compact (one line)\n{}",
            out.ctx()
        );
    }
}

#[test]
fn a10_success_envelope_shape() {
    let sb = Sandbox::new();
    let out = sb.glen(["doctor", "--json"]).fake_omp().run();
    let env = out.ok_envelope();
    let keys: Vec<&str> = env
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["data", "ok", "v"],
        "no warnings key when there are none\n{}",
        out.ctx()
    );
    assert!(env["v"].is_u64(), "v is the integer 1\n{}", out.ctx());
}

#[test]
fn ac010_error_envelope_carries_string_code_and_message() {
    let sb = Sandbox::new();
    let out = sb.glen(["watch"]).run();
    let err = out.error_envelope("usage");
    assert!(err["code"].is_string() && err["message"].is_string());
}

#[test]
fn ac004_human_flag_prints_human_output_on_a_pipe() {
    let sb = Sandbox::new();
    let out = sb.glen(["doctor", "--human"]).fake_omp().run();
    out.assert_exit(0).assert_no_ansi();
    assert!(!out.is_json(), "--human must not print JSON\n{}", out.ctx());
    let text = out.out();
    assert!(text.contains("doctor"), "{}", out.ctx());
    assert!(text.contains(TESTED_OMP), "{}", out.ctx());
    // §A10: paths under HOME render as `~/…` in human output.
    assert!(text.contains("~/.omp/agent/sessions"), "{}", out.ctx());
    assert!(
        out.stderr.is_empty(),
        "no warnings, no spinner\n{}",
        out.ctx()
    );
}

#[test]
fn ac005_json_and_human_together_is_a_usage_error() {
    let sb = Sandbox::new();
    for args in [
        ["--json", "--human", "doctor"],
        ["doctor", "--json", "--human"],
    ] {
        let out = sb.glen(args).fake_omp().run();
        out.error_envelope("usage");
        out.assert_stderr_empty();
    }
    // Reversed order: the mode is open to interpretation, the exit code is not.
    let out = sb.glen(["--human", "--json", "doctor"]).fake_omp().run();
    out.assert_exit(2);
    if !out.stdout.is_empty() {
        out.error_envelope("usage");
    }
}

#[test]
fn ac006_usage_errors_in_json_mode_are_envelopes_on_stdout() {
    let sb = Sandbox::new();
    let cases: &[&[&str]] = &[
        &["bogus"],
        &["--json", "bogus"],
        &["doctor", "--bogus-flag"],
        &["--json", "doctor", "--bogus-flag"],
        &["--color", "sometimes", "doctor"],
        &["--json", "completions", "notashell"],
        &["completions", "notashell", "--json"],
        &["show"],
    ];
    for args in cases {
        let out = sb.glen(*args).run();
        let err = out.error_envelope("usage");
        out.assert_stderr_empty().assert_no_ansi();
        let msg = err["message"].as_str().unwrap();
        assert!(
            !msg.starts_with("error:"),
            "message should not repeat the `error:` prefix\n{}",
            out.ctx()
        );
    }
}

#[test]
fn a10_raw_completions_errors_without_json_go_to_stderr_only() {
    // Clarified §A10: a raw-output command without `--json` renders every error as human text on
    // stderr with an empty stdout and the normal exit code (the `--json` forms are covered in
    // ac006_usage_errors_in_json_mode_are_envelopes_on_stdout).
    let sb = Sandbox::new();
    for args in [vec!["completions", "notashell"], vec!["completions"]] {
        let out = sb.glen(&args).run();
        out.assert_exit(exit_for("usage")).assert_no_ansi();
        assert!(
            out.stdout.is_empty(),
            "no envelope on stdout without --json\n{}",
            out.ctx()
        );
        assert!(!out.stderr.is_empty(), "{}", out.ctx());
    }
}

#[test]
fn usage_errors_in_human_mode_stay_off_stdout() {
    let sb = Sandbox::new();
    let cases: &[&[&str]] = &[
        &["--human", "bogus"],
        // A token after `--` is not a flag.
        &["--human", "bogus", "--", "--json"],
        // Only the whole token `--json` selects JSON.
        &["--jsonx", "--human"],
        &["--json=1", "--human", "doctor"],
    ];
    for args in cases {
        let out = sb.glen(*args).run();
        out.assert_exit(2);
        assert!(
            out.stdout.is_empty(),
            "human-mode usage errors print nothing on stdout\n{}",
            out.ctx()
        );
        assert!(!out.stderr.is_empty(), "{}", out.ctx());
    }
}

#[test]
fn ac010_exit_codes_follow_the_design_table() {
    let sb = Sandbox::new();
    // error_envelope() checks each exit code against docs/design.md §A11.
    sb.glen(["tail"]).run().error_envelope("usage");
    sb.glen(["watch"]).run().error_envelope("usage");
    sb.glen(["doctor"])
        .fake_omp()
        .env("OMP_PROFILE", "Bad Name")
        .run()
        .error_envelope("invalid_env");
    sb.glen(["doctor"]).run().error_envelope("omp_unavailable");
}

#[test]
fn ac021_no_spinner_when_stderr_is_not_a_terminal() {
    let sb = Sandbox::new();
    let tools = sb.dir("tools");
    // A slow omp gives a spinner time to draw if one were (wrongly) enabled.
    let slow = write_script(&tools, "slow-omp", "/bin/sleep 1\necho omp/18.14.0");
    for args in [vec!["doctor", "--human"], vec!["doctor"]] {
        let out = sb.glen(&args).env("GLEN_OMP_BIN", &slow).run();
        out.assert_exit(0);
        assert!(
            out.stderr.is_empty(),
            "no spinner on a non-TTY stderr or in JSON mode\n{}",
            out.ctx()
        );
        let braille = out
            .out()
            .chars()
            .any(|c| ('\u{2800}'..='\u{28ff}').contains(&c));
        assert!(!braille, "spinner frames on stdout\n{}", out.ctx());
    }
}
