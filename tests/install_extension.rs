mod common;
use common::{OmpBox, data, json};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};
fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}
fn install(b: &OmpBox, flags: &[&str]) -> std::process::Output {
    let mut args = vec!["install-extension", "--json"];
    args.extend_from_slice(flags);
    b.command(&args, &b.alpha()).output().unwrap()
}
fn target(b: &OmpBox) -> std::path::PathBuf {
    b.agent.join("extensions/glen/index.ts")
}
#[test]
fn ac302_install_targets_and_lazy_home() {
    let default = OmpBox::new();
    let d = data(
        &default
            .command(&["install-extension", "--json"], &default.alpha())
            .env_remove("PI_CODING_AGENT_DIR")
            .output()
            .unwrap(),
    );
    assert_eq!(
        d["path"],
        default
            .home
            .join(".omp/agent/extensions/glen/index.ts")
            .to_str()
            .unwrap()
    );
    assert!(!default.glen_home.exists());

    for (flags, vars, suffix) in [
        (vec![], vec![], "agent"),
        (
            vec!["--profile", "work"],
            vec![],
            ".omp/profiles/work/agent",
        ),
        (
            vec![],
            vec![("OMP_PROFILE", "work")],
            ".omp/profiles/work/agent",
        ),
        (
            vec![],
            vec![("PI_PROFILE", "work")],
            ".omp/profiles/work/agent",
        ),
    ] {
        let b = OmpBox::new();
        let mut args = vec!["install-extension", "--json"];
        args.extend(flags);
        let o = b.command(&args, &b.alpha()).envs(vars).output().unwrap();
        let d = data(&o);
        assert_eq!(d["action"], "installed");
        let p = std::path::PathBuf::from(d["path"].as_str().unwrap());
        let expected = if suffix == "agent" {
            target(&b)
        } else {
            b.home.join(suffix).join("extensions/glen/index.ts")
        };
        assert_eq!(p, expected);
        assert_eq!(mode(&p), 0o600);
        assert_eq!(mode(p.parent().unwrap()), 0o700);
        assert!(!b.glen_home.exists());
        let source = fs::read_to_string(&p).unwrap();
        let (marker, body) = source.split_once('\n').unwrap();
        assert_eq!(
            marker,
            format!(
                "// glen-extension version={} protocol=1 digest={:016x}",
                env!("CARGO_PKG_VERSION"),
                xxhash_rust::xxh64::xxh64(body.as_bytes(), 0)
            )
        );
        assert!(body.contains(b.glen_home.to_str().unwrap()));
        assert!(!source.contains("__GLEN_"));
        let files = fs::read_dir(p.parent().unwrap()).unwrap().count();
        assert_eq!(files, 1);
    }
}
#[test]
fn ac303_upgrade_force_and_uninstall_table() {
    let b = OmpBox::new();
    assert_eq!(data(&install(&b, &[]))["action"], "installed");
    let p = target(&b);
    let before = fs::metadata(&p).unwrap();
    assert_eq!(data(&install(&b, &[]))["action"], "unchanged");
    let after = fs::metadata(&p).unwrap();
    assert_eq!(before.ino(), after.ino());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    for (body, action, warning) in [
        (
            glen::extension::render("0.0.1", &b.glen_home),
            "upgraded",
            None,
        ),
        (
            glen::extension::render(env!("CARGO_PKG_VERSION"), Path::new("/another/home")),
            "reinstalled",
            None,
        ),
        (
            glen::extension::render("9.0.0", &b.glen_home),
            "skipped",
            Some("extension_newer"),
        ),
        (
            format!(
                "{}modified",
                glen::extension::render(env!("CARGO_PKG_VERSION"), &b.glen_home)
            ),
            "skipped",
            Some("extension_modified"),
        ),
        ("foreign".into(), "skipped", Some("extension_foreign")),
    ] {
        fs::write(&p, &body).unwrap();
        let o = install(&b, &[]);
        assert_eq!(data(&o)["action"], action);
        if let Some(w) = warning {
            assert_eq!(json(&o)["warnings"][0]["code"], w);
            assert_eq!(fs::read_to_string(&p).unwrap(), body);
            assert_eq!(data(&install(&b, &["--force"]))["action"], "replaced");
        }
    }
    fs::write(&p, "foreign").unwrap();
    assert_eq!(data(&install(&b, &["--uninstall"]))["action"], "skipped");
    assert_eq!(
        data(&install(&b, &["--uninstall", "--force"]))["action"],
        "removed"
    );
    assert!(!p.parent().unwrap().exists());
    assert_eq!(
        data(&install(&b, &["--uninstall"]))["action"],
        "not_installed"
    );
    install(&b, &[]);
    let sibling = p.parent().unwrap().join("owner.txt");
    fs::write(&sibling, "keep").unwrap();
    let o = install(&b, &["--uninstall"]);
    assert_eq!(data(&o)["action"], "removed");
    assert_eq!(json(&o)["warnings"][0]["code"], "extension_dir_not_empty");
    assert_eq!(fs::read_to_string(sibling).unwrap(), "keep");
}
#[test]
fn ac303_dry_run_changes_nothing() {
    let b = OmpBox::new();
    assert_eq!(data(&install(&b, &["--dry-run"]))["action"], "installed");
    assert!(!b.agent.join("extensions").exists());
    install(&b, &[]);
    let p = target(&b);
    let before = fs::metadata(&p).unwrap();
    let bytes = fs::read(&p).unwrap();
    let d = data(&install(&b, &["--uninstall", "--dry-run"]));
    assert_eq!(d["action"], "removed");
    assert_eq!(d["dry_run"], true);
    assert_eq!(fs::read(&p).unwrap(), bytes);
    assert_eq!(
        fs::metadata(p).unwrap().modified().unwrap(),
        before.modified().unwrap()
    );
    assert!(!b.glen_home.exists());
}
#[test]
fn ac304_doctor_checks_and_ac321_warning() {
    let b = OmpBox::new();
    let doctor = || {
        data(
            &b.command(&["doctor", "--json"], &b.alpha())
                .env("GLEN_OMP_BIN", common::fixture("bin/fake-omp"))
                .output()
                .unwrap(),
        )
    };
    assert_eq!(doctor()["extension"]["state"], "not_installed");
    assert!(!b.glen_home.exists());
    install(&b, &[]);
    let d = doctor();
    assert_eq!(d["checks"].as_array().unwrap().len(), 12);
    assert_eq!(d["extension"]["current"], true);
    fs::write(target(&b), glen::extension::render("0.0.1", &b.glen_home)).unwrap();
    assert_eq!(doctor()["checks"][6]["status"], "warn");
    let home = b.tmp.path().join("x".repeat(110));
    let o = b
        .command(&["install-extension", "--json"], &b.alpha())
        .env("GLEN_HOME", &home)
        .output()
        .unwrap();
    assert!(
        json(&o)["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "socket_path_too_long")
    );
    assert!(!home.exists());
}

#[test]
fn installer_rejects_non_utf8_baked_home_without_writing() {
    use std::os::unix::ffi::OsStringExt;
    let b = OmpBox::new();
    let home = b
        .tmp
        .path()
        .join(std::ffi::OsString::from_vec(b"home\xff".to_vec()));
    for mode in ["--json", "--human"] {
        let o = b
            .command(&["install-extension", mode], &b.alpha())
            .env_remove("GLEN_HOME")
            .env("HOME", &home)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{o:?}");
        assert!(!b.agent.join("extensions").exists());
        if mode == "--json" {
            assert_eq!(json(&o)["error"]["code"], "invalid_env");
            assert!(o.stderr.is_empty());
        }
    }
}

#[test]
fn human_install_output_explains_the_tool_and_fits_both_themes() {
    for theme in ["light", "dark"] {
        let b = OmpBox::new();
        let o = b
            .command(
                &["install-extension", "--human", "--color", "always"],
                &b.alpha(),
            )
            .env("GLEN_THEME", theme)
            .env("COLUMNS", "60")
            .output()
            .unwrap();
        assert!(o.status.success(), "{o:?}");
        assert!(o.stderr.is_empty());
        assert!(o.stdout.contains(&27));
        let text = String::from_utf8(o.stdout).unwrap();
        let plain = anstream::adapter::strip_str(&text).to_string();
        assert!(plain.contains("glen_reply"));
        assert!(plain.contains("--uninstall"));
        assert!(plain.contains("restart running omp sessions"));
        assert!(
            plain
                .lines()
                .all(|line| glen::render::text::width(line) <= 60),
            "{plain}"
        );
    }
}

#[test]
fn symlinks_in_extension_path_are_never_installed_or_uninstalled_even_with_force() {
    use std::os::unix::fs::symlink;
    for component in ["extensions", "glen", "index.ts"] {
        for uninstall in [false, true] {
            let b = OmpBox::new();
            let outside = b.tmp.path().join("outside");
            fs::create_dir_all(outside.join("glen")).unwrap();
            let body = glen::extension::render(env!("CARGO_PKG_VERSION"), &b.glen_home);
            fs::write(outside.join("index.ts"), &body).unwrap();
            fs::write(outside.join("glen/index.ts"), &body).unwrap();
            match component {
                "extensions" => symlink(&outside, b.agent.join("extensions")).unwrap(),
                "glen" => {
                    fs::create_dir_all(b.agent.join("extensions")).unwrap();
                    symlink(&outside, b.agent.join("extensions/glen")).unwrap();
                }
                _ => {
                    fs::create_dir_all(b.agent.join("extensions/glen")).unwrap();
                    symlink(outside.join("index.ts"), target(&b)).unwrap();
                }
            }
            let o = install(
                &b,
                if uninstall {
                    &["--uninstall", "--force"]
                } else {
                    &["--force"]
                },
            );
            assert_eq!(data(&o)["action"], "skipped", "{o:?}");
            assert_eq!(json(&o)["warnings"][0]["code"], "unsafe_path");
            assert_eq!(fs::read_to_string(outside.join("index.ts")).unwrap(), body);
            assert_eq!(
                fs::read_to_string(outside.join("glen/index.ts")).unwrap(),
                body
            );
        }
    }
}

#[test]
fn human_dry_run_and_action_specific_tool_explanation() {
    let b = OmpBox::new();
    for flags in [
        vec!["--dry-run"],
        vec![],
        vec![],
        vec!["--uninstall", "--dry-run"],
        vec!["--uninstall"],
        vec!["--uninstall"],
    ] {
        let mut args = vec!["install-extension", "--human"];
        args.extend(&flags);
        let o = b.command(&args, &b.alpha()).output().unwrap();
        assert!(o.status.success(), "{o:?}");
        let text = String::from_utf8(o.stdout).unwrap();
        assert_eq!(
            text.contains("dry run, nothing changed"),
            flags.contains(&"--dry-run")
        );
        let changing_install = text.contains("installed glen") && !text.contains("not_installed");
        assert_eq!(text.contains("adds glen_reply"), changing_install, "{text}");
        assert!(!text.contains("every TUI session"));
    }
}

#[test]
fn doctor_only_suggests_install_when_needed_and_reports_inspection_errors() {
    let b = OmpBox::new();
    let doctor = || {
        data(
            &b.command(&["doctor", "--json"], &b.alpha())
                .env("GLEN_OMP_BIN", common::fixture("bin/fake-omp"))
                .output()
                .unwrap(),
        )
    };
    assert!(
        doctor()["checks"][6]["detail"]
            .as_str()
            .unwrap()
            .contains("install-extension")
    );
    install(&b, &[]);
    let d = doctor();
    assert_eq!(d["extension"]["current"], true);
    assert!(
        !d["checks"][6]["detail"]
            .as_str()
            .unwrap()
            .contains("install-extension")
    );
    fs::remove_file(target(&b)).unwrap();
    fs::create_dir(target(&b)).unwrap();
    let d = doctor();
    assert_eq!(d["extension"]["state"], "unreadable");
    assert_eq!(d["checks"][6]["status"], "warn");
    assert!(
        d["checks"][6]["detail"]
            .as_str()
            .unwrap()
            .contains("cannot inspect extension")
    );
}

#[test]
fn default_profile_is_null_and_named_profile_matches_doctor() {
    let b = OmpBox::new();
    let installed = data(&install(&b, &["--dry-run"]));
    assert!(installed["profile"].is_null());
    let doctor = data(
        &b.command(&["doctor", "--json"], &b.alpha())
            .env("GLEN_OMP_BIN", common::fixture("bin/fake-omp"))
            .output()
            .unwrap(),
    );
    assert_eq!(installed["profile"], doctor["paths"]["profile"]);
    let installed = data(&install(&b, &["--profile", "work", "--dry-run"]));
    let doctor = data(
        &b.command(&["doctor", "--profile", "work", "--json"], &b.alpha())
            .env("GLEN_OMP_BIN", common::fixture("bin/fake-omp"))
            .output()
            .unwrap(),
    );
    assert_eq!(installed["profile"], "work");
    assert_eq!(installed["profile"], doctor["paths"]["profile"]);
}

#[test]
fn doctor_current_is_always_a_boolean() {
    let b = OmpBox::new();
    for present in [false, true] {
        if present {
            install(&b, &[]);
        }
        let doctor = data(
            &b.command(&["doctor", "--json"], &b.alpha())
                .env("GLEN_OMP_BIN", common::fixture("bin/fake-omp"))
                .output()
                .unwrap(),
        );
        assert!(doctor["extension"]["current"].is_boolean());
        assert_eq!(doctor["extension"]["current"], present);
    }
}

#[test]
fn force_replaces_unsafe_contents_and_leaves_current_file_unchanged() {
    let b = OmpBox::new();
    install(&b, &[]);
    for body in [
        glen::extension::render("9.0.0", &b.glen_home),
        format!(
            "{}edited",
            glen::extension::render(env!("CARGO_PKG_VERSION"), &b.glen_home)
        ),
        "foreign".into(),
    ] {
        fs::write(target(&b), body).unwrap();
        let dry = data(&install(&b, &["--force", "--dry-run"]));
        assert_eq!(dry["action"], "replaced");
        assert_eq!(data(&install(&b, &["--force"]))["action"], "replaced");
        let before = fs::metadata(target(&b)).unwrap();
        assert_eq!(data(&install(&b, &["--force"]))["action"], "unchanged");
        let after = fs::metadata(target(&b)).unwrap();
        assert_eq!(before.ino(), after.ino());
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    }
}
