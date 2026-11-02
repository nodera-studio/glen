//! Offline harness regressions; no actual omp or coding agent is launched.
use std::{fs, process::Command};
#[test]
fn person_checks_command_output_without_accepting_echoed_nonces() {
    let source = include_str!("acceptance/person.exp");
    let step = source
        .split("proc step ")
        .nth(1)
        .unwrap()
        .split("\nunset -nocomplain env(")
        .next()
        .unwrap()
        .replace("set timeout 150", "set timeout 5");
    let t = tempfile::tempdir().unwrap();
    let script = t.path().join("echo.exp");
    fs::write(
        &script,
        format!(
            r#"
set number 0
set started [clock milliseconds]
proc fail {{message}} {{error $message}}
proc step {step}
spawn /bin/bash --norc --noprofile -i
set timeout 5
expect -re {{[$#] }}
send -- "PS1='acc> '\r"
expect -re {{\r?\n}}
expect -exact "acc> "
set output [step {{printf 'actual-output\n'}} 0 actual-output]
if {{[string match *printf* $output]}} {{error "echo leaked into command output"}}
set rejected [catch {{step {{: requested-NONCE; printf 'wrong\n'}} 0 requested-NONCE}} reason]
if {{!$rejected || ![string match *missing* $reason]}} {{error "echo-only nonce passed: $reason"}}
expect -exact "acc> "
send -- "exit\r"
expect eof
"#
        ),
    )
    .unwrap();
    let out = match Command::new("expect")
        .arg(&script)
        .env("TERM", "dumb")
        .output()
    {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "pty regression not run: optional manual-harness dependency expect is absent"
            );
            return;
        }
        Err(e) => panic!("cannot run expect: {e}"),
    };
    assert!(
        out.status.success(),
        "stdout {} stderr {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
#[test]
fn discovery_prompts_are_uncoached_and_snapshot_prerequisite_is_explicit() {
    let push = include_str!("acceptance/prompts/claude-push.md");
    for text in [
        include_str!("acceptance/prompts/codex.md"),
        include_str!("acceptance/prompts/claude-discover.md"),
        push,
    ] {
        for coaching in [
            "5-second",
            "exit 10",
            "exit 130",
            "Never repeat a send",
            "100 seconds",
        ] {
            assert!(!text.contains(coaching), "coached prompt: {coaching}");
        }
    }
    // AC-v2-206: the push prompt names neither the receiver nor how to run it.
    let lower = push.to_lowercase();
    for word in [
        "background",
        "run_in_background",
        "--for reply",
        "poll",
        "inbox --wait",
        "wait",
    ] {
        assert!(!lower.contains(word), "coached push prompt: {word}");
    }
    assert!(
        push.contains("{{PROJECT}}") && push.contains("{{NONCE_P}}") && push.contains("e27c0de")
    );
    assert!(include_str!("acceptance/README.md").contains("Close other omp sessions first"));
}
#[test]
fn real_omp_legs_run_under_the_glen_harness_profile() {
    let acc = include_str!("acceptance/acc.sh");
    assert!(
        acc.lines().any(|l| l == "PROFILE=glen-harness"),
        "acc.sh must define PROFILE=glen-harness"
    );
    let (mut calls, mut omps, mut installs) = (0, 0, 0);
    for line in acc.lines() {
        if ["doctor", "send", "ls", "inbox"]
            .iter()
            .any(|c| line.contains(&format!("\"$G\" {c} ")))
        {
            calls += 1;
            // Mode p's fake_omp leg: a scratch HOME and agent dir, so no profile is involved.
            let scratch =
                line.contains("HOME=\"$home\"") && line.contains("PI_CODING_AGENT_DIR=\"$S/");
            assert!(
                line.contains("--profile \"$PROFILE\"") || scratch,
                "glen call without --profile: {line}"
            );
        }
        if line.contains("exec omp") {
            omps += 1;
            assert!(
                line.contains("exec omp --profile "),
                "omp without --profile: {line}"
            );
        }
        if line.contains("\"$G\" install-extension") {
            installs += 1;
            // A profile would make it write the profile's real extensions dir (design §A12).
            assert!(
                !line.contains("--profile"),
                "install-extension under a profile: {line}"
            );
            assert!(line.contains("env PI_CODING_AGENT_DIR=\"$S/extension-agent\""));
        }
    }
    assert!(
        calls >= 7 && omps == 2 && installs == 1,
        "{calls} {omps} {installs}"
    );
    // Named profiles ignore PI_CODING_AGENT_DIR; it may only name a scratch dir, never the
    // owner's or the profile's agent dir.
    for (i, _) in acc.match_indices("PI_CODING_AGENT_DIR=") {
        let rest = &acc[i + "PI_CODING_AGENT_DIR=".len()..];
        assert!(
            rest.starts_with("\"$S/"),
            "PI_CODING_AGENT_DIR outside scratch: {rest:.40}"
        );
    }
    let wrapper = include_str!("acceptance/glen-wrapper.sh");
    assert!(!wrapper.contains("PI_CODING_AGENT_DIR="));
    let person = include_str!("acceptance/person.exp");
    assert!(!person.contains("PI_CODING_AGENT_DIR"));
    assert!(person.contains("glen() { command glen --profile glen-harness \"$@\"; }"));
}
