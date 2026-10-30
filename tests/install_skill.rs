mod common;
use common::*;
use glen::skill::{self, Agent};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::Path,
};
fn install(b: &OmpBox, args: &[&str]) -> Value {
    data(&with_umask(b, args, "022").output().unwrap())
}
fn with_umask(b: &OmpBox, args: &[&str], mask: &str) -> std::process::Command {
    let base = b.command(&[], &b.alpha());
    let mut command = std::process::Command::new("/bin/sh");
    command
        .env_clear()
        .envs(base.get_envs().filter_map(|(k, v)| v.map(|v| (k, v))))
        .current_dir(b.alpha())
        .args([
            "-c",
            &format!("umask {mask}; exec \"$@\""),
            "glen-test",
            env!("CARGO_BIN_EXE_glen"),
        ])
        .args(args);
    command
}
fn user(b: &OmpBox, a: Agent) -> std::path::PathBuf {
    b.home
        .join(if a == Agent::Claude {
            ".claude"
        } else {
            ".agents"
        })
        .join("skills/glen/SKILL.md")
}
fn action(v: &Value, index: usize, want: &str) {
    assert_eq!(v["targets"][index]["action"], want, "{v}");
}
#[test]
fn ac405_targets() {
    let b = OmpBox::new();
    let v = install(&b, &["install-skill"]);
    assert_eq!(v["scope"], "user");
    for a in Agent::ALL {
        let p = user(&b, a);
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            skill::render_current(a).unwrap()
        );
        assert_eq!(fs::metadata(&p).unwrap().mode() & 0o777, 0o644);
        assert_eq!(
            fs::metadata(p.parent().unwrap()).unwrap().mode() & 0o777,
            0o755
        );
    }
    assert!(!b.glen_home.exists());
    assert!(!b.home.join(".codex").exists());
    for (flag, a) in [("--claude", Agent::Claude), ("--codex", Agent::Codex)] {
        let v = install(&b, &["install-skill", flag]);
        assert_eq!(v["targets"].as_array().unwrap().len(), 1);
        assert_eq!(v["targets"][0]["agent"], a.as_str());
    }
    for value in [
        b.tmp.path().join("config"),
        std::path::PathBuf::from("config-relative"),
    ] {
        let out = b
            .command(&["install-skill", "--claude"], &b.alpha())
            .env("CLAUDE_CONFIG_DIR", &value)
            .output()
            .unwrap();
        let v = data(&out);
        let expected = if value.is_absolute() {
            value
        } else {
            b.alpha().join(value)
        }
        .join("skills/glen/SKILL.md");
        assert_eq!(v["targets"][0]["path"], expected.to_str().unwrap());
        assert!(expected.is_file());
    }
    let v = install(&b, &["install-skill", "--project"]);
    assert_eq!(v["scope"], "project");
    for a in Agent::ALL {
        assert!(skill::project_target(&b.alpha(), a).is_file());
    }
}
#[test]
fn ac406_state_table() {
    let b = OmpBox::new();
    install(&b, &["install-skill", "--claude"]);
    let p = user(&b, Agent::Claude);
    let meta = fs::metadata(&p).unwrap();
    action(&install(&b, &["install-skill", "--claude"]), 0, "unchanged");
    let after = fs::metadata(&p).unwrap();
    assert_eq!(meta.ino(), after.ino());
    assert_eq!(meta.modified().unwrap(), after.modified().unwrap());
    let visible = skill::visible_commands();
    let mut different = visible.clone();
    different.insert("inbox".into());
    for text in [
        skill::render(Agent::Claude, "0.0.1", &visible).unwrap(),
        skill::render(Agent::Claude, env!("CARGO_PKG_VERSION"), &different).unwrap(),
    ] {
        fs::write(&p, text).unwrap();
        let v = install(&b, &["install-skill", "--claude"]);
        action(&v, 0, "upgraded");
        assert_eq!(v["targets"][0]["state"], "outdated");
    }
    let current = skill::render_current(Agent::Claude).unwrap();
    let cases = [
        (
            skill::render(Agent::Claude, "9.9.9", &visible).unwrap(),
            "skill_newer",
        ),
        (format!("{current}\nedit"), "skill_modified"),
        (String::from("foreign"), "skill_foreign"),
        (
            skill::render_current(Agent::Codex).unwrap(),
            "skill_foreign",
        ),
    ];
    for (text, code) in &cases {
        fs::write(&p, text).unwrap();
        let out = b.glen(&["install-skill", "--claude"], &b.alpha());
        action(&data(&out), 0, "skipped");
        assert_eq!(json(&out)["warnings"][0]["code"], *code);
        assert_eq!(fs::read_to_string(&p).unwrap(), *text);
        action(
            &install(&b, &["install-skill", "--claude", "--force"]),
            0,
            "reinstalled",
        );
    }
    for (text, code) in &cases {
        fs::write(&p, text).unwrap();
        let out = b.glen(&["install-skill", "--claude", "--uninstall"], &b.alpha());
        if *code == "skill_newer" {
            action(&data(&out), 0, "removed");
        } else {
            action(&data(&out), 0, "skipped");
            assert_eq!(json(&out)["warnings"][0]["code"], *code);
            assert!(
                json(&out)["warnings"][0]["message"]
                    .as_str()
                    .unwrap()
                    .contains("--force removes it")
            );
            action(
                &install(&b, &["install-skill", "--claude", "--force", "--uninstall"]),
                0,
                "removed",
            );
        }
        install(&b, &["install-skill", "--claude"]);
    }
    fs::write(p.parent().unwrap().join("keep"), "sibling").unwrap();
    let out = b.glen(&["install-skill", "--claude", "--uninstall"], &b.alpha());
    action(&data(&out), 0, "removed");
    assert_eq!(json(&out)["warnings"][0]["code"], "skill_dir_not_empty");
    assert!(p.parent().unwrap().join("keep").is_file());
    action(
        &install(&b, &["install-skill", "--claude", "--uninstall"]),
        0,
        "not_installed",
    );
}
#[test]
fn ac406_symlinks_and_dry_runs() {
    let b = OmpBox::new();
    let p = user(&b, Agent::Claude);
    let external = b.tmp.path().join("external");
    fs::create_dir(&external).unwrap();
    fs::create_dir_all(p.parent().unwrap().parent().unwrap()).unwrap();
    symlink(&external, p.parent().unwrap()).unwrap();
    for flags in [vec![], vec!["--force"], vec!["--force", "--uninstall"]] {
        let mut args = vec!["install-skill", "--claude"];
        args.extend(flags);
        let out = b.glen(&args, &b.alpha());
        action(&data(&out), 0, "skipped");
        assert!(
            json(&out)["warnings"][0]["message"]
                .as_str()
                .unwrap()
                .contains("symlink")
        );
        assert_eq!(fs::read_dir(&external).unwrap().count(), 0);
    }
    let alpha = b.alpha();
    fs::create_dir(alpha.join(".agents")).unwrap();
    symlink(&external, alpha.join(".agents/skills")).unwrap();
    action(
        &install(&b, &["install-skill", "--codex", "--project", "--force"]),
        0,
        "skipped",
    );
    let c = OmpBox::new();
    let dotfiles = c.tmp.path().join("dotfiles");
    fs::create_dir(&dotfiles).unwrap();
    symlink(&dotfiles, c.home.join(".claude")).unwrap();
    action(&install(&c, &["install-skill", "--claude"]), 0, "installed");
    let p = user(&c, Agent::Claude);
    let target = c.tmp.path().join("other.md");
    fs::write(&target, "untouched").unwrap();
    fs::remove_file(&p).unwrap();
    symlink(&target, &p).unwrap();
    action(&install(&c, &["install-skill", "--claude"]), 0, "skipped");
    action(
        &install(&c, &["install-skill", "--claude", "--force"]),
        0,
        "reinstalled",
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
    assert!(!fs::symlink_metadata(&p).unwrap().is_symlink());
    fs::remove_file(&p).unwrap();
    symlink(&target, &p).unwrap();
    action(
        &install(&c, &["install-skill", "--claude", "--force", "--uninstall"]),
        0,
        "removed",
    );
    assert!(target.exists());
    let d = OmpBox::new();
    let before = tree(&d.home);
    install(&d, &["install-skill", "--dry-run"]);
    install(&d, &["install-skill", "--project", "--dry-run"]);
    assert_eq!(tree(&d.home), before);
    install(&d, &["install-skill"]);
    let before = tree(&d.home);
    install(&d, &["install-skill", "--dry-run", "--uninstall"]);
    assert_eq!(tree(&d.home), before);
    assert!(!d.glen_home.exists());
}
fn tree(path: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut result = vec![];
    for e in fs::read_dir(path).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            result.push((p.clone(), vec![]));
            result.extend(tree(&p));
        } else {
            result.push((p.clone(), fs::read(p).unwrap()));
        }
    }
    result.sort();
    result
}
#[test]
fn ac407_output_and_preflight() {
    let b = OmpBox::new();
    let out = b.glen(&["install-skill", "--dry-run"], &b.alpha());
    assert!(out.stderr.is_empty());
    let v = data(&out);
    assert_eq!(v["targets"][0]["installed_version"], Value::Null);
    assert_eq!(v["targets"][0]["state"], "not_installed");
    assert_eq!(v["dry_run"], true);
    let out = b.glen(&["install-skill", "--human", "--dry-run"], &b.alpha());
    assert!(String::from_utf8_lossy(&out.stdout).contains("~/.claude/skills/glen/SKILL.md"));
    assert!(String::from_utf8_lossy(&out.stdout).contains("$glen"));
    let out = b
        .command(&["install-skill"], &b.alpha())
        .env_remove("HOME")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out)["error"]["code"], "invalid_env");
    let out = b
        .command(&["install-skill"], &b.alpha())
        .env("HOME", " \t ")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out)["error"]["code"], "invalid_env");
    let parent = user(&b, Agent::Codex).parent().unwrap().to_path_buf();
    fs::create_dir_all(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o000)).unwrap();
    let out = b.glen(&["install-skill"], &b.alpha());
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!user(&b, Agent::Claude).exists());
    assert_eq!(
        json(&out)["error"]["details"]["done"],
        serde_json::json!([])
    );
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();
    let out = b.glen(&["install-skill"], &b.alpha());
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        json(&out)["error"]["details"]["done"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn ac408_doctor_skill_checks() {
    let b = OmpBox::new();
    let doctor = || {
        data(
            &b.command(&["doctor"], &b.alpha())
                .env("GLEN_OMP_BIN", fixture("bin/fake-omp"))
                .output()
                .unwrap(),
        )
    };
    let before = tree(&b.home);
    let v = doctor();
    assert_eq!(v["skills"]["codex"]["project"], Value::Null);
    assert_eq!(v["checks"][7]["status"], "info");
    assert_eq!(tree(&b.home), before);
    install(&b, &["install-skill"]);
    let v = doctor();
    assert_eq!(v["checks"][7]["status"], "ok");
    assert_eq!(v["skills"]["claude"]["current"], true);
    let p = user(&b, Agent::Claude);
    fs::write(&p, format!("{}edited", fs::read_to_string(&p).unwrap())).unwrap();
    assert_eq!(doctor()["checks"][7]["status"], "warn");
    let project = skill::project_target(&b.alpha(), Agent::Codex);
    fs::create_dir_all(project.parent().unwrap()).unwrap();
    fs::write(
        &project,
        skill::render(Agent::Codex, "0.0.1", &skill::visible_commands()).unwrap(),
    )
    .unwrap();
    let v = doctor();
    assert_eq!(v["checks"][8]["status"], "warn");
    assert!(
        v["checks"][8]["detail"]
            .as_str()
            .unwrap()
            .contains("--project")
    );
    assert_eq!(v["skills"]["codex"]["project"]["state"], "outdated");
}
#[test]
fn ac409_env_files() {
    let b = OmpBox::new();
    fs::write(
        b.home.join(".env"),
        "export XDG_STATE_HOME=/x\nPI_CODING_AGENT_DIR=/x\n# XDG_DATA_HOME=/ignored",
    )
    .unwrap();
    let doctor = |set: bool| {
        let mut c = b.command(&["doctor"], &b.alpha());
        c.env("GLEN_OMP_BIN", fixture("bin/fake-omp"));
        if set {
            c.env("XDG_STATE_HOME", "/x");
        }
        data(&c.output().unwrap())
    };
    let v = doctor(false);
    assert_eq!(v["checks"][6]["status"], "warn");
    assert_eq!(
        v["checks"][6]["value"],
        serde_json::json!([b.home.join(".env")])
    );
    assert!(
        v["checks"][6]["detail"]
            .as_str()
            .unwrap()
            .contains("XDG_STATE_HOME")
    );
    assert_eq!(doctor(true)["checks"][6]["status"], "ok");
    // PI_CODING_AGENT_DIR is already explicitly set by OmpBox. Alias maps to that key.
    fs::write(b.alpha().join(".env"), "OMP_CODING_AGENT_DIR=/x").unwrap();
    assert_eq!(doctor(true)["checks"][6]["status"], "ok");
    let out = b
        .command(&["doctor"], &b.alpha())
        .env_remove("PI_CODING_AGENT_DIR")
        .env("XDG_STATE_HOME", "/x")
        .env("GLEN_OMP_BIN", fixture("bin/fake-omp"))
        .output()
        .unwrap();
    assert_eq!(data(&out)["checks"][6]["status"], "warn");
    fs::set_permissions(b.home.join(".env"), fs::Permissions::from_mode(0o000)).unwrap();
    fs::set_permissions(b.alpha().join(".env"), fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(doctor(false)["checks"][6]["status"], "ok");
}
#[test]
fn ac410_sandbox() {
    let b = RpcBox::new();
    for network in [false, true] {
        for target in ["e27c0de", "@new"] {
            let mut c = b.glen(&["send", target, "hi"]);
            c.env("CODEX_SANDBOX", "seatbelt");
            if !network {
                c.env("CODEX_SANDBOX_NETWORK_DISABLED", "1");
            }
            let start = std::time::Instant::now();
            let out = c.output().unwrap();
            assert_eq!(out.status.code(), Some(6), "{out:?}");
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            let v = json(&out);
            assert_eq!(v["error"]["details"]["reason"], "sandboxed");
            assert_eq!(v["error"]["details"]["network"], network);
            assert!(b.fake_log().is_empty());
            assert!(!b.omp.glen_home.exists());
        }
        for args in [
            vec!["ls"],
            vec!["show", "e27c0de"],
            vec!["tail", "e27c0de"],
            vec!["jobs"],
        ] {
            let mut c = b.glen(&args);
            c.env("CODEX_SANDBOX", "seatbelt");
            if !network {
                c.env("CODEX_SANDBOX_NETWORK_DISABLED", "1");
            }
            data(&c.output().unwrap());
        }
    }
    let out = b
        .glen(&["doctor"])
        .env("CODEX_SANDBOX", "seatbelt")
        .output()
        .unwrap();
    assert_eq!(data(&out)["checks"][9]["status"], "warn");
    fs::create_dir(&b.omp.glen_home).unwrap();
    fs::set_permissions(&b.omp.glen_home, fs::Permissions::from_mode(0o500)).unwrap();
    let out = b
        .glen(&["show", "e27c0de"])
        .env("GLEN_MAX_OUTPUT", "200")
        .env("CODEX_SANDBOX", "seatbelt")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    assert_eq!(json(&out)["error"]["details"]["glen_home_writable"], false);
    assert!(
        json(&out)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("$TMPDIR")
    );
    let out = b
        .glen(&["show", "e27c0de"])
        .env("GLEN_MAX_OUTPUT", "200")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    fs::set_permissions(&b.omp.glen_home, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn cycle1_project_without_home_whitespace_config_and_umask() {
    let b = OmpBox::new();
    let out = b
        .command(&["install-skill", "--project"], &b.alpha())
        .env_remove("HOME")
        .output()
        .unwrap();
    assert_eq!(data(&out)["scope"], "project");
    let out = with_umask(&b, &["install-skill", "--claude"], "077")
        .env("CLAUDE_CONFIG_DIR", "  ")
        .output()
        .unwrap();
    assert_eq!(
        data(&out)["targets"][0]["path"],
        user(&b, Agent::Claude).to_str().unwrap()
    );
    assert_eq!(
        fs::metadata(user(&b, Agent::Claude)).unwrap().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(user(&b, Agent::Claude).parent().unwrap())
            .unwrap()
            .mode()
            & 0o777,
        0o700
    );
}
#[test]
fn cycle1_modified_older_foreign_agent_and_non_utf8_skip_safely() {
    let b = OmpBox::new();
    install(&b, &["install-skill", "--claude"]);
    let path = user(&b, Agent::Claude);
    let old = skill::render(Agent::Claude, "0.0.1", &skill::visible_commands()).unwrap();
    for (bytes, state, version) in [
        (
            format!("{old}edited").into_bytes(),
            "modified",
            serde_json::json!("0.0.1"),
        ),
        (
            skill::render_current(Agent::Codex).unwrap().into_bytes(),
            "foreign",
            Value::Null,
        ),
        (vec![0xff, 0xfe], "foreign", Value::Null),
    ] {
        fs::write(&path, &bytes).unwrap();
        for flags in [vec![], vec!["--uninstall"]] {
            let mut args = vec!["install-skill", "--claude"];
            args.extend(flags);
            let out = b.glen(&args, &b.alpha());
            let v = data(&out);
            action(&v, 0, "skipped");
            assert_eq!(v["targets"][0]["state"], state);
            assert_eq!(v["targets"][0]["installed_version"], version);
            assert_eq!(json(&out)["warnings"][0]["code"], format!("skill_{state}"));
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }
}
#[test]
fn cycle1_dry_run_never_spills_or_prunes_and_human_verbs_fit() {
    let b = OmpBox::new();
    for human in [false, true] {
        let mut args = vec!["install-skill", "--dry-run"];
        if human {
            args.push("--human");
        }
        let out = b
            .command(&args, &b.alpha())
            .env("GLEN_MAX_OUTPUT", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(!b.glen_home.exists());
    }
    let outdir = b.glen_home.join("out");
    fs::create_dir_all(&outdir).unwrap();
    let retained = outdir.join("old.json");
    fs::write(&retained, b"retain").unwrap();
    fs::File::open(&retained)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(9 * 86400))
        .unwrap();
    let before = tree(&b.glen_home);
    let out = b
        .command(&["install-skill", "--dry-run"], &b.alpha())
        .env("GLEN_MAX_OUTPUT", "1")
        .output()
        .unwrap();
    data(&out);
    assert_eq!(tree(&b.glen_home), before);
    for width in ["40", "60"] {
        let out = b
            .command(&["install-skill", "--human", "--dry-run"], &b.alpha())
            .env("COLUMNS", width)
            .output()
            .unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("would install"));
        assert!(!text.contains("would installed"));
        assert!(
            text.lines()
                .all(|line| glen::render::text::width(line) <= width.parse::<usize>().unwrap()),
            "{text}"
        );
    }
    install(&b, &["install-skill"]);
    let out = b.glen(
        &["install-skill", "--human", "--uninstall", "--dry-run"],
        &b.alpha(),
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("would remove"));
    let footer = text.lines().last().unwrap();
    assert!(!footer.contains("/glen") && !footer.contains("$glen"));
}
#[test]
fn cycle1_doctor_home_symlink_is_user_copy_and_project_detail_has_no_leading_separator() {
    let b = OmpBox::new();
    let config = b.tmp.path().join("config");
    fs::create_dir(&config).unwrap();
    symlink(config, b.home.join(".claude")).unwrap();
    install(&b, &["install-skill"]);
    let out = b
        .command(&["doctor"], &b.home)
        .env("GLEN_OMP_BIN", fixture("bin/fake-omp"))
        .output()
        .unwrap();
    let v = data(&out);
    assert_eq!(v["skills"]["claude"]["project"], Value::Null);
    assert_eq!(v["checks"][7]["status"], "ok");
    let project = skill::project_target(&b.alpha(), Agent::Claude);
    fs::create_dir_all(project.parent().unwrap()).unwrap();
    fs::write(&project, "foreign").unwrap();
    let out = b
        .command(&["doctor"], &b.alpha())
        .env("GLEN_OMP_BIN", fixture("bin/fake-omp"))
        .output()
        .unwrap();
    let v = data(&out);
    let detail = v["checks"][7]["detail"].as_str().unwrap();
    assert!(detail.starts_with("project copy"), "{detail}");
}
#[test]
fn cycle1_uninstall_error_keeps_removed_target_and_earlier_warnings() {
    let b = OmpBox::new();
    install(&b, &["install-skill"]);
    fs::write(user(&b, Agent::Claude), "foreign").unwrap();
    let skills = user(&b, Agent::Codex)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    fs::set_permissions(&skills, fs::Permissions::from_mode(0o555)).unwrap();
    let out = b.glen(&["install-skill", "--uninstall"], &b.alpha());
    fs::set_permissions(&skills, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let v = json(&out);
    assert_eq!(v["error"]["details"]["done"][1]["action"], "removed");
    assert_eq!(
        v["error"]["details"]["warnings"][0]["code"],
        "skill_foreign"
    );
    assert!(!user(&b, Agent::Codex).exists());
}
#[test]
fn cycle1_network_disabled_only_allows_send_and_maps_actual_job_write_failure() {
    let b = RpcBox::new();
    let out = b
        .glen(&["send", "e27c0de", "hi"])
        .env("CODEX_SANDBOX", "")
        .env("CODEX_SANDBOX_NETWORK_DISABLED", "1")
        .output()
        .unwrap();
    let v = data(&out);
    assert_eq!(v["status"], "completed");
    b.wait_worker_gone(v["job"]["id"].as_str().unwrap());
    let out = b
        .glen(&["doctor"])
        .env("CODEX_SANDBOX_NETWORK_DISABLED", "1")
        .output()
        .unwrap();
    let v = data(&out);
    assert_eq!(v["checks"][9]["status"], "info");
    assert_eq!(
        v["checks"][9]["detail"],
        "network disabled by the host; escalated commands still work"
    );
    fs::set_permissions(&b.omp.glen_home, fs::Permissions::from_mode(0o500)).unwrap();
    // A fresh home child makes the denied operation occur during job creation.
    let out = b
        .glen(&["send", "@new", "hi"])
        .env("GLEN_HOME", b.omp.glen_home.join("blocked"))
        .env("CODEX_SANDBOX_NETWORK_DISABLED", "1")
        .output()
        .unwrap();
    fs::set_permissions(&b.omp.glen_home, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(out.status.code(), Some(6));
    assert_eq!(json(&out)["error"]["details"]["reason"], "sandboxed");
    assert_eq!(json(&out)["error"]["details"]["glen_home_writable"], false);
}

#[test]
fn cycle2_doctor_absent_symlink_scopes_and_foreign_file_symlink() {
    let b = OmpBox::new();
    let home_link = b.tmp.path().join("home-link");
    symlink(&b.home, &home_link).unwrap();
    let out = b
        .command(&["doctor"], &b.home)
        .env("HOME", &home_link)
        .env("GLEN_OMP_BIN", fixture("bin/fake-omp"))
        .output()
        .unwrap();
    let v = data(&out);
    for a in Agent::ALL {
        assert_eq!(v["skills"][a.as_str()]["project"], Value::Null);
    }
    assert!(
        !v["checks"][7]["detail"]
            .as_str()
            .unwrap()
            .contains("foreign")
    );
    let shared = b.tmp.path().join("shared-claude");
    fs::create_dir(&shared).unwrap();
    symlink(&shared, b.alpha().join(".claude")).unwrap();
    let doctor = || {
        data(
            &b.command(&["doctor"], &b.alpha())
                .env("GLEN_OMP_BIN", fixture("bin/fake-omp"))
                .output()
                .unwrap(),
        )
    };
    assert_eq!(doctor()["skills"]["claude"]["project"], Value::Null);
    fs::create_dir_all(shared.join("skills/glen")).unwrap();
    fs::write(
        shared.join("skills/glen/SKILL.md"),
        "divergent real content",
    )
    .unwrap();
    let v = doctor();
    assert_eq!(v["skills"]["claude"]["project"]["state"], "foreign");
    assert!(
        v["checks"][7]["detail"]
            .as_str()
            .unwrap()
            .contains("foreign")
    );
    // A final-component symlink to the installed user file is still a foreign project entry.
    install(&b, &["install-skill", "--codex"]);
    let project = skill::project_target(&b.alpha(), Agent::Codex);
    fs::create_dir_all(project.parent().unwrap()).unwrap();
    symlink(user(&b, Agent::Codex), &project).unwrap();
    let v = doctor();
    assert_eq!(v["skills"]["codex"]["project"]["state"], "foreign");
    assert!(
        v["checks"][8]["detail"]
            .as_str()
            .unwrap()
            .contains("foreign")
    );
}

#[test]
fn cycle2_newer_edit_wins() {
    let b = OmpBox::new();
    install(&b, &["install-skill", "--claude"]);
    let path = user(&b, Agent::Claude);
    let edited = format!(
        "{}user edit",
        skill::render(Agent::Claude, "99.0.0", &skill::visible_commands()).unwrap()
    );
    fs::write(&path, &edited).unwrap();
    let out = b.glen(&["install-skill", "--claude"], &b.alpha());
    let v = data(&out);
    assert_eq!(v["targets"][0]["state"], "modified");
    action(&v, 0, "skipped");
    assert_eq!(json(&out)["warnings"][0]["code"], "skill_modified");
    assert_eq!(fs::read_to_string(&path).unwrap(), edited);
}
#[test]
fn cycle2_foreign_doctor_version_is_null() {
    let b = OmpBox::new();
    install(&b, &["install-skill", "--claude"]);
    let path = user(&b, Agent::Claude);
    fs::write(&path, skill::render_current(Agent::Codex).unwrap()).unwrap();
    let out = b
        .command(&["doctor"], &b.alpha())
        .env("GLEN_OMP_BIN", fixture("bin/fake-omp"))
        .output()
        .unwrap();
    let v = data(&out);
    assert_eq!(v["skills"]["claude"]["state"], "foreign");
    assert_eq!(v["skills"]["claude"]["version"], Value::Null);
}
#[test]
fn cycle2_alias_process_environment_resolves_and_suppresses_dotenv_warning() {
    let b = OmpBox::new();
    fs::write(
        b.alpha().join(".env"),
        "PI_CODING_AGENT_DIR=/elsewhere\nOMP_CODING_AGENT_DIR=/elsewhere",
    )
    .unwrap();
    let alias = b.tmp.path().join("alias-agent");
    let out = b
        .command(&["doctor"], &b.alpha())
        .env_remove("PI_CODING_AGENT_DIR")
        .env("OMP_CODING_AGENT_DIR", &alias)
        .env("GLEN_OMP_BIN", fixture("bin/fake-omp"))
        .output()
        .unwrap();
    let v = data(&out);
    assert_eq!(v["paths"]["agent_dir"], alias.to_str().unwrap());
    assert_eq!(v["paths"]["agent_dir_source"], "env");
    assert_eq!(v["checks"][6]["status"], "ok");
}
#[test]
fn cycle2_refusal_precedes_missing_omp_lookup() {
    let b = OmpBox::new();
    let out = b
        .command(&["send", "@new", "hi"], &b.alpha())
        .env("CODEX_SANDBOX", "seatbelt")
        .env("GLEN_OMP_BIN", "/missing/omp")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    assert_eq!(json(&out)["error"]["details"]["reason"], "sandboxed");
    assert!(!b.glen_home.exists());
}
#[test]
fn cycle2_footer_dry_run_and_line_boundaries_at_40_and_60() {
    let b = OmpBox::new();
    install(&b, &["install-skill"]);
    for width in ["40", "60"] {
        let out = b
            .command(&["install-skill", "--human", "--dry-run"], &b.alpha())
            .env("COLUMNS", width)
            .output()
            .unwrap();
        assert!(out.status.success());
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("session\nClaude Code:"), "{text}");
        let out = b
            .command(
                &["install-skill", "--human", "--uninstall", "--dry-run"],
                &b.alpha(),
            )
            .env("COLUMNS", width)
            .output()
            .unwrap();
        assert!(out.status.success());
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("would uninstall"), "{text}");
        assert!(!text.contains("uninstall complete"));
        assert!(user(&b, Agent::Claude).is_file());
    }
}
#[test]
fn cycle2_marker_only_spill_and_job_write_errors_are_truthful() {
    let b = RpcBox::new();
    fs::create_dir(&b.omp.glen_home).unwrap();
    fs::set_permissions(&b.omp.glen_home, fs::Permissions::from_mode(0o500)).unwrap();
    for args in [vec!["ls", "--all"], vec!["send", "@new", "hi"]] {
        let out = b
            .glen(&args)
            .env("GLEN_HOME", b.omp.glen_home.join("blocked"))
            .env("GLEN_MAX_OUTPUT", "1")
            .env("CODEX_SANDBOX_NETWORK_DISABLED", "1")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(6));
        let v = json(&out);
        let message = v["error"]["message"].as_str().unwrap();
        assert!(message.starts_with("cannot write GLEN_HOME"), "{v}");
        assert!(
            !message.contains("job j")
                && !message.contains("inside Codex")
                && !message.contains("cannot reach omp"),
            "{v}"
        );
        assert_eq!(v["error"]["details"]["reason"], "sandboxed");
    }
    fs::set_permissions(&b.omp.glen_home, fs::Permissions::from_mode(0o700)).unwrap();
}
#[test]
fn cycle2_worker_network_failures_reach_cli_as_sandboxed() {
    for scenario in ["network_refused", "network_dns", "network_denied"] {
        let b = RpcBox::new();
        let out = b
            .glen(&["send", "@new", "hi"])
            .env("CODEX_SANDBOX_NETWORK_DISABLED", "1")
            .env("FAKE_OMP_SCENARIO", scenario)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(6), "{out:?}");
        let v = json(&out);
        // wait's envelope nests the persisted worker error.
        assert_eq!(v["error"]["code"], "no_route");
        assert_eq!(
            v["error"]["details"]["error"]["details"]["reason"], "sandboxed",
            "{v}"
        );
        b.wait_worker_gone(v["error"]["details"]["job"]["id"].as_str().unwrap());
    }
}
#[test]
fn cycle2_changed_warning_is_in_json_contract() {
    assert!(
        include_str!("../docs/json.md").contains("| `changed_during_install` | install-skill |")
    );
}

#[test]
fn cycle2_worker_spawn_permission_failure_is_sandboxed() {
    let b = RpcBox::new();
    let interpreter = b.omp.tmp.path().join("denied-interpreter");
    fs::write(&interpreter, "not executable").unwrap();
    fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o600)).unwrap();
    let executable = b.omp.tmp.path().join("spawn-omp");
    fs::write(&executable, format!("#!{}\n", interpreter.display())).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let out = b
        .glen(&["send", "@new", "hi"])
        .env("GLEN_OMP_BIN", &executable)
        .env("CODEX_SANDBOX_NETWORK_DISABLED", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6), "{out:?}");
    let v = json(&out);
    assert_eq!(
        v["error"]["details"]["error"]["details"]["reason"],
        "sandboxed"
    );
    b.wait_worker_gone(v["error"]["details"]["job"]["id"].as_str().unwrap());
}
