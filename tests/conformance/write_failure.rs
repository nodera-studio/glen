//! A closed or failing stdout: no panic, no second document, exit 1 (AC-029; §A10).

use std::process::Stdio;

use crate::support::*;

/// A pipe whose read end is already closed: every write gets EPIPE.
fn closed_stdout() -> Stdio {
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    Stdio::from(writer)
}

fn assert_failed_write(out: &Out) {
    out.assert_no_panic();
    out.assert_exit(1);
    let err = out.err();
    assert!(
        !err.contains("\"v\"") && !err.trim_start().starts_with('{'),
        "no second JSON document on stderr\n{}",
        out.ctx()
    );
}

#[test]
fn ac029_closed_stdout_in_json_mode() {
    let sb = Sandbox::new();
    let out = sb
        .glen(["doctor", "--json"])
        .fake_omp()
        .stdout(closed_stdout())
        .run();
    assert_failed_write(&out);
    assert!(
        out.stderr.is_empty(),
        "JSON mode keeps stderr empty\n{}",
        out.ctx()
    );
}

#[test]
fn ac029_closed_stdout_in_human_mode() {
    let sb = Sandbox::new();
    let out = sb
        .glen(["doctor", "--human"])
        .fake_omp()
        .stdout(closed_stdout())
        .run();
    assert_failed_write(&out);
}

#[test]
fn ac029_closed_stdout_while_reporting_an_error() {
    let sb = Sandbox::new();
    for args in [vec!["jobs"], vec!["doctor"]] {
        let out = sb.glen(&args).stdout(closed_stdout()).run();
        assert_failed_write(&out);
    }
}

#[test]
fn ac029_closed_stdout_for_raw_completions() {
    let sb = Sandbox::new();
    let out = sb
        .glen(["completions", "bash"])
        .stdout(closed_stdout())
        .run();
    assert_failed_write(&out);
}

#[test]
fn ac029_closed_stdout_for_help_usage_and_version_does_not_panic() {
    let sb = Sandbox::new();
    for args in [vec!["--help"], vec!["--version"], vec!["bogus"]] {
        let out = sb.glen(&args).stdout(closed_stdout()).run();
        out.assert_no_panic();
    }
}

/// AC-029 is unconditional ("IF writing to stdout fails … SHALL exit 1"); this pins it for
/// clap-rendered help, version and usage output too.
#[test]
fn ac029_closed_stdout_for_help_usage_and_version_exits_1() {
    let sb = Sandbox::new();
    for args in [vec!["--help"], vec!["--version"], vec!["bogus"]] {
        let out = sb.glen(&args).stdout(closed_stdout()).run();
        out.assert_no_panic();
        out.assert_exit(1);
    }
}

#[test]
fn ac029_closed_stdout_for_ls() {
    let sb = Sandbox::new();
    let out = sb.glen(["ls"]).stdout(closed_stdout()).run();
    assert_failed_write(&out);
}
