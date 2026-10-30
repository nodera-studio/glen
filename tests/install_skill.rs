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
    data(&b.glen(args, &b.alpha()))
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
    assert!(v["checks"][6]["value"].as_str().unwrap().contains("~/.env"));
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
