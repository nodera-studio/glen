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
    assert_eq!(
        commands,
        [
            "ls",
            "show",
            "tail",
            "send",
            "wait",
            "jobs",
            "inbox",
            "install-extension",
            "install-skill",
            "doctor",
            "completions",
            "help"
        ]
    );
}
#[test]
fn send_help_documents_thinking_flag() {
    let o = glen(&["send", "--help"], &[]);
    assert!(o.status.success());
    let s = String::from_utf8(o.stdout).unwrap();
    assert!(s.contains("--thinking <LEVEL>"));
    assert!(s.contains("omp's startup reasoning effort"));
}
#[test]
fn hidden_commands() {
    // `watch` is the only command still hidden after W3 and W4.
    let o = glen(&["watch"], &[]);
    assert_eq!(o.status.code(), Some(8));
    assert_eq!(json(&o)["error"]["code"], "not_implemented");
    assert_eq!(json(&o)["v"], 1);
    assert!(o.stderr.is_empty());
    let o = glen(&["watch", "--human"], &[]);
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
    let v = json(&o);
    let check = &v["data"]["checks"][1];
    assert_eq!(check["status"], "warn");
    assert_eq!(check["value"], "18.0.9");
    assert_eq!(check["detail"], "untested");
}
#[test]
fn doctor_unavailable() {
    for vars in [vec![], vec![("GLEN_OMP_BIN", "/nonexistent")]] {
        let o = glen(&["doctor"], &vars);
        assert_eq!(o.status.code(), Some(5));
        let v = json(&o);
        assert_eq!(v["error"]["code"], "omp_unavailable");
        assert!(v["error"]["details"]["report"]["checks"].is_array());
        let skipped = &v["error"]["details"]["report"]["checks"][1];
        assert_eq!(skipped["status"], "info");
        assert_eq!(skipped["value"], "skipped");
        assert_eq!(skipped["detail"], "omp binary is missing");
        assert!(
            v["error"]["details"]["report"]["checks"][0]["detail"]
                .as_str()
                .unwrap()
                .contains(if vars.is_empty() {
                    "PATH"
                } else {
                    "/nonexistent"
                })
        );
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
        for visible in [
            "doctor",
            "ls",
            "show",
            "tail",
            "send",
            "wait",
            "jobs",
            "inbox",
            "install-extension",
            "install-skill",
        ] {
            assert!(s.contains(visible));
        }
        for hidden in ["__worker", "watch"] {
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

#[test]
fn help_and_usage_follow_glen_color_policy() {
    for args in [vec!["--help"], vec!["--human", "bogus"]] {
        for (flags, vars, colored) in [
            (
                vec!["--color", "never"],
                vec![("CLICOLOR_FORCE", "1")],
                false,
            ),
            (vec!["--color=always"], vec![("NO_COLOR", "1")], true),
            (vec![], vec![("CLICOLOR_FORCE", "1")], true),
            (
                vec![],
                vec![("NO_COLOR", " "), ("CLICOLOR_FORCE", "1")],
                false,
            ),
            (vec![], vec![("CLICOLOR_FORCE", " ")], true),
            (vec![], vec![("CLICOLOR_FORCE", "0")], false),
        ] {
            let mut command = flags;
            command.extend_from_slice(&args);
            let o = glen(&command, &vars);
            let (text, other) = if args.contains(&"--help") {
                assert!(o.status.success());
                (&o.stdout, &o.stderr)
            } else {
                assert_eq!(o.status.code(), Some(2));
                (&o.stderr, &o.stdout)
            };
            assert_eq!(text.contains(&27), colored, "{command:?} {vars:?}");
            assert!(other.is_empty());
        }
    }
    let help = glen(&["--help"], &[]);
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("Print a JSON envelope"));
    assert!(help.contains("Print human-readable output"));
}

#[test]
fn clap_external_argument_controls_are_escaped() {
    let bad = "bad\nforged\r\u{9b}\x1b\u{202e}\u{2066}";
    for args in [
        vec!["--human", bad],
        vec!["--human", "doctor", bad],
        vec!["completions", bad],
        vec!["--human", "--color", bad, "doctor"],
    ] {
        let o = glen(&args, &[]);
        assert_eq!(o.status.code(), Some(2));
        assert!(o.stdout.is_empty());
        let text = String::from_utf8(o.stderr).unwrap();
        assert!(
            text.contains(r"bad\nforged\r\u{9b}\x1b\u{202e}\u{2066}"),
            "{text:?}"
        );
        for c in ['\r', '\u{9b}', '\x1b', '\u{202e}', '\u{2066}'] {
            assert!(!text.contains(c), "{text:?}");
        }
        assert!(!text.lines().any(|line| line.starts_with("forged")));
    }
}

#[test]
fn bare_glen_is_a_json_usage_error() {
    let o = glen(&[], &[]);
    assert_eq!(o.status.code(), Some(2));
    assert!(o.stderr.is_empty());
    let v = json(&o);
    assert_eq!(v["error"]["code"], "usage");
    assert_eq!(v["error"]["message"], "a subcommand is required");
    let help = v["error"]["details"]["text"].as_str().unwrap();
    assert!(help.contains("See and talk to omp sessions"));
    assert!(help.contains("Usage:"));
    assert!(help.contains("Commands:"));

    let o = glen(&["--json"], &[]);
    assert_eq!(o.status.code(), Some(2));
    let v = json(&o);
    assert_eq!(v["error"]["code"], "usage");
    assert_eq!(
        v["error"]["message"],
        "'glen' requires a subcommand but one was not provided"
    );
    assert!(o.stderr.is_empty());
}

#[test]
fn raw_errors_are_human_and_a_relative_glen_home_is_invalid_env_for_completions_too() {
    for args in [
        vec!["completions", "bad"],
        vec!["completions"],
        vec!["completions", "bash", "--bad"],
    ] {
        let o = glen(&args, &[("GLEN_HOME", "relative")]);
        assert_eq!(o.status.code(), Some(2));
        assert!(o.stdout.is_empty());
        assert!(o.stderr.starts_with(b"error:"));
        let mut args = args;
        args.push("--json");
        let o = glen(&args, &[]);
        assert_eq!(o.status.code(), Some(2));
        assert_eq!(json(&o)["error"]["code"], "usage");
        assert!(o.stderr.is_empty());
    }
    // AC-020 has no trigger restriction: a relative GLEN_HOME is resolved (and rejected) while
    // building the run, before dispatch, so it is `invalid_env` for `completions` too. An error
    // is never a script (LOW-004): without `--json`, stdout must stay empty so
    // `glen completions zsh >> ~/.zshrc` can never append an envelope, and the error renders as
    // human text on stderr instead.
    let o = glen(&["completions", "bash"], &[("GLEN_HOME", "relative")]);
    assert_eq!(o.status.code(), Some(2));
    assert!(o.stdout.is_empty());
    let text = String::from_utf8(o.stderr).unwrap();
    assert!(text.starts_with("error:"));
    assert!(text.contains("invalid_env") || text.contains("GLEN_HOME"));

    let o = glen(
        &["completions", "bash", "--json"],
        &[("GLEN_HOME", "relative")],
    );
    assert_eq!(o.status.code(), Some(2));
    assert!(o.stderr.is_empty());
    let v = json(&o);
    assert_eq!(v["error"]["code"], "invalid_env");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("GLEN_HOME")
    );
}

#[test]
fn doctor_json_values_keep_raw_paths_and_details_keep_reasons() {
    let t = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let home = t.path().to_str().unwrap();
    let bin = t.path().join("omp\x1b\u{202e}");
    fs::copy(fake(), &bin).unwrap();
    let o = glen(
        &["doctor"],
        &[
            ("HOME", home),
            ("GLEN_OMP_BIN", bin.to_str().unwrap()),
            ("GLEN_HOME", home),
        ],
    );
    assert!(o.status.success());
    let v = json(&o);
    for (name, expected, reason) in [
        ("omp_binary", bin, ""),
        (
            "agent_dir",
            t.path().join(".omp/agent"),
            "omp has not run with this profile yet",
        ),
        (
            "sessions_dir",
            t.path().join(".omp/agent/sessions"),
            "no sessions directory yet",
        ),
        ("glen_home", t.path().to_path_buf(), ""),
    ] {
        let check = v["data"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap();
        assert_eq!(check["value"], expected.to_str().unwrap());
        assert_eq!(check["detail"], reason);
    }
    let o = glen(
        &["doctor", "--human"],
        &[
            ("HOME", home),
            ("GLEN_OMP_BIN", &fake()),
            ("GLEN_HOME", home),
        ],
    );
    assert!(o.status.success());
    assert!(
        String::from_utf8(o.stdout)
            .unwrap()
            .contains("~/.omp/agent")
    );
}

#[test]
fn doctor_colored_rows_have_no_empty_spans_or_trailing_padding() {
    let agent = fixture("omp/agent");
    let o = glen(
        &["doctor", "--human", "--color", "always"],
        &[
            ("GLEN_OMP_BIN", &fake()),
            ("PI_CODING_AGENT_DIR", agent.to_str().unwrap()),
        ],
    );
    assert!(o.status.success());
    let text = String::from_utf8(o.stdout).unwrap();
    assert!(text.contains('\x1b'));
    assert!(!text.contains("m\x1b[0m"), "{text:?}");
    let plain = anstream::adapter::strip_str(&text).to_string();
    for line in plain.lines() {
        assert_eq!(line, line.trim_end(), "{line:?}");
    }
}

#[cfg(unix)]
#[test]
fn profile_flag_prevents_inherited_profile_conflicts_in_omp() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let bin = t.path().join("omp");
    fs::write(&bin, "#!/bin/sh\nif [ \"${OMP_PROFILE+x}\" = x ] || [ \"${PI_PROFILE+x}\" = x ]; then exit 64; fi\necho omp/18.14.0\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    for profile in ["work", "default", "  work  "] {
        let o = glen(
            &["--profile", profile, "doctor"],
            &[
                ("GLEN_OMP_BIN", bin.to_str().unwrap()),
                ("OMP_PROFILE", "Bad Name"),
                ("PI_PROFILE", "../bad"),
            ],
        );
        assert!(o.status.success(), "{o:?}");
        assert_eq!(json(&o)["data"]["omp"]["version"], "18.14.0");
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_home_is_an_error_envelope_without_panic() {
    use std::os::unix::ffi::OsStringExt;
    let t = tempfile::tempdir().unwrap();
    let home = t
        .path()
        .join(std::ffi::OsString::from_vec(b"home\xff".to_vec()));
    let o = Command::new(env!("CARGO_BIN_EXE_glen"))
        .arg("doctor")
        .env_clear()
        .env("HOME", &home)
        .env("GLEN_OMP_BIN", fake())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2), "{o:?}");
    assert_eq!(json(&o)["error"]["code"], "invalid_env");
    assert!(o.stderr.is_empty());
}

#[cfg(unix)]
#[test]
fn non_utf8_profile_keeps_original_clap_diagnostic() {
    use std::os::unix::ffi::OsStringExt;
    let t = tempfile::tempdir().unwrap();
    for mode in ["--human", "--json"] {
        let o = Command::new(env!("CARGO_BIN_EXE_glen"))
            .args([mode, "--profile"])
            .arg(std::ffi::OsString::from_vec(b"bad\xff\x1b\r".to_vec()))
            .arg("doctor")
            .env_clear()
            .env("HOME", t.path())
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{o:?}");
        if mode == "--human" {
            assert!(o.stdout.is_empty());
            let text = String::from_utf8(o.stderr).unwrap();
            assert!(text.contains("invalid UTF-8"), "{text:?}");
            assert!(text.contains("Usage:"), "{text:?}");
            assert!(text.ends_with('\n'), "{text:?}");
            assert!(!text.contains(['\x1b', '\r']), "{text:?}");
        } else {
            assert!(o.stderr.is_empty());
            let v = json(&o);
            assert_eq!(v["error"]["code"], "usage");
            assert!(
                v["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("invalid UTF-8")
            );
            assert!(
                v["error"]["details"]["text"]
                    .as_str()
                    .unwrap()
                    .contains("Usage:")
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_home_allows_a_full_human_doctor_report() {
    use std::os::unix::ffi::OsStringExt;
    let t = tempfile::tempdir().unwrap();
    let home = t
        .path()
        .join(std::ffi::OsString::from_vec(b"home\xff".to_vec()));
    for (bin, code) in [(fake(), 0), ("/nonexistent".into(), 5)] {
        let o = Command::new(env!("CARGO_BIN_EXE_glen"))
            .args(["doctor", "--human"])
            .env_clear()
            .env("HOME", &home)
            .env("GLEN_OMP_BIN", bin)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(code), "{o:?}");
        let text = String::from_utf8(o.stdout).unwrap();
        for row in [
            "glen doctor",
            "omp binary",
            "omp version",
            "profile",
            "agent dir",
            "sessions dir",
            "glen home",
            "tested against omp",
        ] {
            assert!(text.contains(row), "{text:?}");
        }
        assert!(text.contains(home.to_string_lossy().as_ref()), "{text:?}");
        assert!(!String::from_utf8_lossy(&o.stderr).contains("cannot serialize"));
    }
    assert!(!home.exists());
}

#[cfg(unix)]
#[test]
fn doctor_json_explains_glen_home_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let o = glen(
        &["doctor"],
        &[
            ("GLEN_OMP_BIN", &fake()),
            ("GLEN_HOME", t.path().to_str().unwrap()),
        ],
    );
    assert!(o.status.success());
    let v = json(&o);
    let check = &v["data"]["checks"][5];
    assert_eq!(check["status"], "warn");
    assert_eq!(check["value"], t.path().to_str().unwrap());
    assert_eq!(check["detail"], "group/other permission bits are set");
    assert_eq!(
        fs::metadata(t.path()).unwrap().permissions().mode() & 0o777,
        0o755
    );
}
