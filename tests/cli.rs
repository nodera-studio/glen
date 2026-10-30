mod common;
use common::{fixture, glen, json};
use std::{
    fs,
    process::{Command, Stdio},
};
fn fake() -> String {
    fixture("bin/fake-omp").to_str().unwrap().into()
}
#[test]
fn public_help() {
    let o = glen(&["--help"], &[]);
    assert!(o.status.success());
    let s = String::from_utf8(o.stdout).unwrap();
    let commands = s
        .split("Commands:\n")
        .nth(1)
        .unwrap()
        .split("\nOptions:")
        .next()
        .unwrap()
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect::<Vec<_>>();
    assert_eq!(commands, ["doctor", "completions", "help"]);
}
#[test]
fn hidden_commands() {
    for args in [
        vec!["ls"],
        vec!["show", "a"],
        vec!["tail", "a"],
        vec!["send", "@new", "hello"],
        vec!["wait", "j1234567"],
        vec!["jobs"],
        vec!["inbox"],
        vec!["watch"],
        vec!["install-extension"],
        vec!["install-skill"],
    ] {
        let o = glen(&args, &[]);
        assert_eq!(o.status.code(), Some(8), "{args:?}");
        assert_eq!(json(&o)["error"]["code"], "not_implemented");
        assert_eq!(json(&o)["v"], 1);
        assert!(o.stderr.is_empty());
    }
    let o = glen(&["ls", "--human"], &[]);
    assert_eq!(o.status.code(), Some(8));
    assert!(o.stdout.is_empty());
    assert!(o.stderr.starts_with(b"error:"));
    assert!(!o.stderr.contains(&27));
}
#[test]
fn usage_envelopes() {
    for args in [
        vec!["--json", "--human", "doctor"],
        vec!["bogus"],
        vec!["doctor", "--bad"],
        vec!["send", "a", "b", "--wait", "25h"],
    ] {
        let o = glen(&args, &[]);
        assert_eq!(o.status.code(), Some(2));
        assert_eq!(json(&o)["error"]["code"], "usage");
        assert!(o.stderr.is_empty());
    }
}
#[test]
fn doctor_success() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let o = glen(
        &["doctor"],
        &[("GLEN_OMP_BIN", &fake()), ("HOME", home.to_str().unwrap())],
    );
    assert!(o.status.success(), "{o:?}");
    let v = json(&o);
    assert_eq!(v["data"]["omp"]["version"], "18.14.0");
    assert_eq!(v["data"]["omp"]["tested_version"], "18.14.0");
    assert_eq!(
        v["data"]["paths"]["sessions_dir"],
        home.join(".omp/agent/sessions").to_str().unwrap()
    );
    assert!(
        v["data"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "omp_version" && c["status"] == "ok")
    );
    assert!(v.get("warnings").is_none());
    assert_eq!(o.stdout.iter().filter(|b| **b == b'\n').count(), 1);
    assert!(o.stderr.is_empty());
}
#[test]
fn untested_version() {
    let o = glen(
        &["doctor"],
        &[("GLEN_OMP_BIN", &fake()), ("FAKE_OMP_VERSION", "18.0.9")],
    );
    assert!(o.status.success());
    assert!(
        json(&o)["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "omp_version_untested")
    );
}
#[test]
fn doctor_unavailable() {
    for vars in [vec![], vec![("GLEN_OMP_BIN", "/nonexistent")]] {
        let o = glen(&["doctor"], &vars);
        assert_eq!(o.status.code(), Some(5));
        let v = json(&o);
        assert_eq!(v["error"]["code"], "omp_unavailable");
        assert!(v["error"]["details"]["report"]["checks"].is_array());
        assert!(o.stderr.is_empty());
    }
    for version in ["bad", "18.14", "18.14.0\u{1b}[31m"] {
        let o = glen(
            &["doctor"],
            &[("GLEN_OMP_BIN", &fake()), ("FAKE_OMP_VERSION", version)],
        );
        assert_eq!(o.status.code(), Some(5));
        assert_eq!(json(&o)["error"]["code"], "omp_unavailable");
    }
}
#[test]
fn fixture_paths() {
    let agent = fixture("omp/agent");
    let o = glen(
        &["doctor"],
        &[
            ("GLEN_OMP_BIN", &fake()),
            ("PI_CODING_AGENT_DIR", agent.to_str().unwrap()),
        ],
    );
    assert!(o.status.success());
    let v = json(&o);
    assert_eq!(
        v["data"]["paths"]["sessions_dir"],
        agent.join("sessions").to_str().unwrap()
    );
    assert!(
        v["data"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "sessions_dir" && c["status"] == "ok")
    );
}
#[test]
fn invalid_environment() {
    for vars in [
        vec![("OMP_PROFILE", "Bad Name")],
        vec![("PI_PROFILE", "Bad Name")],
        vec![("GLEN_HOME", "rel")],
    ] {
        let o = glen(&["doctor"], &vars);
        assert_eq!(o.status.code(), Some(2));
        assert_eq!(json(&o)["error"]["code"], "invalid_env");
    }
}
#[test]
fn human_color_controls() {
    let fake = fake();
    let o = glen(
        &["doctor", "--human", "--color", "always"],
        &[("GLEN_OMP_BIN", &fake)],
    );
    assert!(o.stdout.windows(2).any(|b| b == b"\x1b["));
    let o = glen(
        &["doctor", "--human"],
        &[
            ("GLEN_OMP_BIN", &fake),
            ("NO_COLOR", "1"),
            ("CLICOLOR_FORCE", "1"),
        ],
    );
    assert!(!o.stdout.contains(&27));
    assert!(!o.stderr.contains(&27));
    let o = glen(
        &["doctor", "--human"],
        &[("GLEN_OMP_BIN", &fake), ("CLICOLOR_FORCE", "1")],
    );
    assert!(o.stdout.contains(&27));
    let o = glen(
        &["doctor", "--json", "--color", "always"],
        &[("GLEN_OMP_BIN", &fake), ("CLICOLOR_FORCE", "1")],
    );
    assert!(!o.stdout.contains(&27));
    assert!(o.stderr.is_empty());
}
#[test]
fn human_table_width() {
    let o = glen(
        &["doctor", "--human"],
        &[("GLEN_OMP_BIN", &fake()), ("COLUMNS", "40")],
    );
    assert!(o.status.success());
    for line in String::from_utf8(o.stdout).unwrap().lines() {
        assert!(line.chars().count() <= 40, "{line:?}");
    }
}
#[test]
fn completions() {
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let o = glen(&["completions", shell], &[]);
        assert!(o.status.success(), "{o:?}");
        assert!(!o.stdout.starts_with(b"{"));
        let s = String::from_utf8(o.stdout).unwrap();
        assert!(s.contains("doctor"));
        for hidden in [
            "glen,ls)",
            "send",
            "install-skill",
            "install-extension",
            "inbox",
            "watch",
        ] {
            assert!(!s.contains(hidden), "{shell}: {hidden}");
        }
    }
    let o = glen(&["completions", "zsh", "--json"], &[]);
    assert!(o.status.success());
    let v = json(&o);
    assert_eq!(v["data"]["shell"], "zsh");
    assert!(!v["data"]["script"].as_str().unwrap().is_empty());
}
#[test]
fn version() {
    let o = glen(&["--version"], &[]);
    assert!(o.status.success());
    assert_eq!(o.stdout, b"glen 0.1.0\n");
}
#[test]
fn lazy_state() {
    let t = tempfile::tempdir().unwrap();
    let h = t.path().join("glen");
    for args in [
        vec!["--help"],
        vec!["--version"],
        vec!["doctor"],
        vec!["ls"],
    ] {
        for omp in [&fake(), "/nonexistent"] {
            glen(
                &args,
                &[("GLEN_HOME", h.to_str().unwrap()), ("GLEN_OMP_BIN", omp)],
            );
            assert!(!h.exists());
        }
    }
}
#[test]
fn closed_stdout_pipe() {
    let t = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_glen"))
        .args(["doctor", "--human"])
        .env_clear()
        .env("HOME", t.path())
        .env("GLEN_OMP_BIN", fake())
        .env("PATH", t.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&o.stderr).contains("panicked"));
}
#[test]
fn external_controls_escaped() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("agent\x1b[31m\x07\u{9b}");
    fs::create_dir(&path).unwrap();
    let o = glen(
        &["doctor", "--human"],
        &[
            ("GLEN_OMP_BIN", &fake()),
            ("PI_CODING_AGENT_DIR", path.to_str().unwrap()),
        ],
    );
    assert!(o.status.success());
    let text = String::from_utf8(o.stdout).unwrap();
    assert!(!text.contains('\x1b'));
    assert!(!text.contains('\x07'));
    assert!(!text.contains('\u{9b}'));
    assert!(text.contains(r"\x1b[31m\x07\u{9b}"));
}
