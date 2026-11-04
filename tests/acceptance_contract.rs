//! Offline harness regressions; no actual omp or coding agent is launched.
#[path = "common/flake.rs"]
mod flake;
use flake::CaptureExt;
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
        .try_captured()
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
    let mcp = [
        include_str!("acceptance/prompts/claude-mcp.md"),
        include_str!("acceptance/prompts/codex-mcp.md"),
    ];
    for text in [
        include_str!("acceptance/prompts/codex.md"),
        include_str!("acceptance/prompts/claude-discover.md"),
        push,
        mcp[0],
        mcp[1],
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
    // AC-v2-505 (v0.2 W5): the MCP prompts name neither the protocol, the tools nor a shell.
    for text in mcp {
        let lower = text.to_lowercase();
        for word in [
            "mcp",
            "glen_",
            "inbox",
            "--for reply",
            "background",
            "poll",
            "bash",
            "shell",
        ] {
            assert!(!lower.contains(word), "coached MCP prompt: {word}");
        }
        assert!(
            text.contains("{{PROJECT}}")
                && text.contains("{{NONCE_M}}")
                && text.contains("e27c0de")
        );
    }
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
#[test]
fn mcp_mode_runs_no_omp_and_its_launcher_closes_the_environment() {
    let acc = include_str!("acceptance/acc.sh");
    assert_eq!(
        acc.matches("[[ $mode =~ ^(a|b|b2|b3|c1|c2|m|p)$ ]]")
            .count(),
        2,
        "mode m must be in setup's and check's mode regex"
    );
    let setup_m = acc
        .split("\nsetup_m() {\n")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("acc.sh defines setup_m");
    // No real omp, no profile, no consent flag and no Codex config in mode m (J3, J5).
    for banned in [
        "need omp",
        "omp --",
        "exec omp",
        "--profile",
        "ACC_",
        ".codex",
    ] {
        assert!(!setup_m.contains(banned), "setup_m: {banned}");
    }
    assert!(setup_m.contains("acceptance m needs W5"));
    assert!(
        setup_m.contains("^(HOME|PI_CODING_AGENT_DIR|GLEN_OMP_BIN|GLEN_HOME|FAKE_OMP_[A-Z0-9_]+)$"),
        "mcp.json's env repeats only the launcher's names"
    );
    assert!(acc.contains("exec env -i PATH=/usr/bin:/bin "));
    assert!(acc.contains("^(HOME|PI_CODING_AGENT_DIR|GLEN_OMP_BIN|FAKE_OMP_[A-Z0-9_]+)$"));
    // Generate the launcher from a manifest that carries names it must drop.
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().canonicalize().unwrap();
    let dir = dir.to_str().unwrap();
    fs::create_dir(t.path().join("bin")).unwrap();
    let manifest = serde_json::json!({"env": {
        "HOME": format!("{dir}/omp-home"), "PI_CODING_AGENT_DIR": format!("{dir}/omp-agent"),
        "GLEN_OMP_BIN": format!("{dir}/fake-bin/fake_omp"), "GLEN_HOME": "/elsewhere",
        "FAKE_OMP_SCENARIO": "progress", "FAKE_OMP_DELAY_MS": "8000", "OMP_PROFILE": "default",
        "PI_PROFILE": "default", "PI_CONFIG_DIR": ".omp-owner", "CLAUDE_CONFIG_DIR": "/c",
        "CODEX_HOME": "/x", "LD_PRELOAD": "/p.so"}});
    fs::write(t.path().join("manifest.json"), manifest.to_string()).unwrap();
    let acc_path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/acceptance/acc.sh");
    let out = match Command::new("bash")
        .args([
            "-c",
            r#"source "$1" && write_mcp_launcher "$2""#,
            "_",
            acc_path,
            dir,
        ])
        .try_captured()
    {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("launcher generation not run: bash is absent");
            return;
        }
        Err(e) => panic!("cannot run bash: {e}"),
    };
    if !out.status.success() && Command::new("jq").arg("--version").try_captured().is_err() {
        eprintln!("launcher generation not run: optional harness dependency jq is absent");
        return;
    }
    assert!(
        out.status.success(),
        "write_mcp_launcher: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let launcher = fs::read_to_string(t.path().join("bin/glen-mcp")).unwrap();
    let mut lines = launcher.lines();
    assert_eq!(lines.next(), Some("#!/usr/bin/env bash"));
    let exec = lines.next().unwrap();
    assert_eq!(lines.next(), None, "{launcher}");
    let rest = exec
        .strip_prefix("exec env -i PATH=/usr/bin:/bin ")
        .unwrap_or_else(|| panic!("launcher must close the environment: {exec}"));
    for banned in [
        "--profile",
        "OMP_PROFILE",
        "PI_PROFILE",
        "PI_CONFIG_DIR",
        "CLAUDE_",
        "CODEX_",
        "LD_PRELOAD",
        "elsewhere",
    ] {
        assert!(!exec.contains(banned), "launcher passes {banned}: {exec}");
    }
    // Only the allowlisted names, a fixed scratch GLEN_HOME, and glen itself, never omp.
    let (assignments, command) = rest
        .split_once(&format!("{dir}/glen-bin/glen mcp "))
        .unwrap();
    let mut names: Vec<_> = assignments
        .split_whitespace()
        .map(|a| a.split_once('=').unwrap().0)
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "FAKE_OMP_DELAY_MS",
            "FAKE_OMP_SCENARIO",
            "GLEN_HOME",
            "GLEN_OMP_BIN",
            "HOME",
            "PI_CODING_AGENT_DIR"
        ],
        "{exec}"
    );
    assert!(assignments.contains(&format!("GLEN_HOME='{dir}/home'")));
    assert!(!exec.split_whitespace().any(|w| {
        let w = w.trim_matches(['\'', '"']);
        w == "omp" || w.ends_with("/omp")
    }));
    // The wire log is protocol messages only: two tees, no environment dump.
    assert_eq!(
        command,
        format!("< <(tee -a {dir}/mcp-in.ndjson) > >(tee -a {dir}/mcp-out.ndjson)"),
        "{exec}"
    );
}
