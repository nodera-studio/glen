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
    for text in [
        include_str!("acceptance/prompts/codex.md"),
        include_str!("acceptance/prompts/claude-discover.md"),
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
    assert!(include_str!("acceptance/README.md").contains("Close other omp sessions first"));
}
