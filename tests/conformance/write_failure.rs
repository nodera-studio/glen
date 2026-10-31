//! W10 AC-1001/AC-1002 amend AC-029: EPIPE keeps the outcome's exit code;
//! other stdout failures and failed stderr writes exit 1 without a panic or second document.

use std::{
    fs::File,
    process::{Command, Stdio},
};

use crate::support::*;

/// A pipe whose read end is already closed: every write gets EPIPE.
fn closed_stdout() -> Stdio {
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    Stdio::from(writer)
}

fn assert_silent_exit(out: &Out, expected: i32) {
    out.assert_no_panic();
    out.assert_exit(expected);
    assert!(
        out.stderr.is_empty(),
        "EPIPE must not produce stderr text or a second JSON document\n{}",
        out.ctx()
    );
}

#[test]
fn ac1001_closed_stdout_in_json_and_human_modes_keeps_success() {
    let sb = Sandbox::new();
    for mode in ["--json", "--human"] {
        let out = sb
            .glen(["doctor", mode])
            .fake_omp()
            .stdout(closed_stdout())
            .run();
        assert_silent_exit(&out, 0);
    }
}

#[test]
fn ac1001_closed_stdout_for_raw_completions_keeps_success() {
    let sb = Sandbox::new();
    let out = sb
        .glen(["completions", "bash"])
        .stdout(closed_stdout())
        .run();
    assert_silent_exit(&out, 0);
}

#[test]
fn ac1001_closed_stdout_for_help_version_and_ls_keeps_success() {
    let sb = Sandbox::new();
    for args in [vec!["--help"], vec!["--version"], vec!["ls"]] {
        let out = sb.glen(&args).stdout(closed_stdout()).run();
        assert_silent_exit(&out, 0);
    }
}

#[test]
fn ac1001_closed_stdout_keeps_failure_outcomes() {
    let sb = Sandbox::new();
    // PATH is empty: doctor has no omp. Non-interactive watch is a usage failure.
    for (args, expected) in [(vec!["bogus"], 2), (vec!["doctor"], 5), (vec!["watch"], 2)] {
        let out = sb.glen(&args).stdout(closed_stdout()).run();
        assert_silent_exit(&out, expected);
    }
}

#[test]
fn ac1002_readonly_stdout_exits_one_without_a_second_document() {
    let sb = Sandbox::new();
    for args in [vec!["doctor", "--json"], vec!["ls"]] {
        let file = tempfile::NamedTempFile::new().expect("stdout file");
        // File::open is read-only: writes fail EBADF on both Linux and macOS.
        let stdout = File::open(file.path()).expect("open read-only stdout");
        let out = sb.glen(&args).fake_omp().stdout(Stdio::from(stdout)).run();
        out.assert_no_panic();
        out.assert_exit(1);
        out.assert_stderr_empty();
        assert_eq!(
            file.as_file().metadata().expect("stat stdout file").len(),
            0
        );
    }
}

#[test]
fn ac1002_human_usage_keeps_two_when_stderr_succeeds_despite_closed_stdout() {
    let sb = Sandbox::new();
    let out = sb.glen(["bogus", "--human"]).stdout(closed_stdout()).run();
    out.assert_no_panic();
    out.assert_exit(2);
    let err = out.err();
    assert!(
        err.starts_with("error:"),
        "human usage on stderr\n{}",
        out.ctx()
    );
    assert!(
        err.contains("Usage:"),
        "usage text on stderr\n{}",
        out.ctx()
    );
}

#[test]
fn ac1002_human_usage_with_both_pipes_closed_exits_one() {
    let sb = Sandbox::new();
    // Run captures stderr itself. Redirect it to the closed stdout in the shell,
    // then exec glen, retaining the harness's bounded wait and isolated environment.
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "exec \"$1\" bogus --human 2>&1", "closed-stderr", BIN])
        .env_clear()
        .env("HOME", &sb.home)
        .env("GLEN_HOME", &sb.glen_home)
        .env("PATH", &sb.empty_bin)
        .current_dir(&sb.cwd)
        .stdin(Stdio::null());
    let out = Run::from_command(command, "bogus --human (both pipes closed)".into())
        .stdout(closed_stdout())
        .run();
    out.assert_no_panic();
    out.assert_exit(1);
    assert!(out.stderr.is_empty(), "{}", out.ctx());
}
