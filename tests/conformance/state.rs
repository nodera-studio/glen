//! glen state under GLEN_HOME: spill, modes, retention and lazy creation
//! (AC-018, AC-019, AC-027; §A1, §A6, §A10).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::support::*;
use serde_json::Value;

/// An absolute agent dir long enough that doctor's JSON exceeds 24000 bytes
/// (agent_dir, sessions_dir and extensions_dir each repeat it).
fn long_agent_dir() -> String {
    let mut p = String::from("/nonexistent-glen-conformance");
    while p.len() < 9000 {
        p.push('/');
        p.push_str(&"d".repeat(200));
    }
    p
}

/// Parse a spilled stub `{"v":1,"ok":true,"spilled":{…}}`; returns (path, bytes, format).
fn spilled(out: &Out) -> (PathBuf, u64, String) {
    let env = out.json();
    out.assert_exit(0).assert_stderr_empty();
    let keys: Vec<&str> = env
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .filter(|k| *k != "warnings")
        .collect();
    assert_eq!(
        keys,
        ["ok", "spilled", "v"],
        "spilled stub shape\n{}",
        out.ctx()
    );
    assert_eq!(env["v"], Value::from(1));
    assert_eq!(env["ok"], Value::Bool(true));
    let sp = &env["spilled"];
    let path = PathBuf::from(sp["path"].as_str().expect("spilled.path"));
    let bytes = sp["bytes"].as_u64().expect("spilled.bytes is an integer");
    let format = sp["format"].as_str().expect("spilled.format").to_owned();
    (path, bytes, format)
}

fn assert_spill_file(sb: &Sandbox, path: &Path, ext: &str) -> Vec<u8> {
    let out_dir = sb.glen_home.join("out");
    assert!(
        path.is_absolute(),
        "spill path is absolute: {}",
        path.display()
    );
    assert_eq!(
        path.parent(),
        Some(out_dir.as_path()),
        "spill lives in $GLEN_HOME/out"
    );
    let name = path.file_name().unwrap().to_str().unwrap();
    // §A6: out/<unix-ms>-<command>-<pid>.{json,txt}
    let stem = name
        .strip_suffix(&format!(".{ext}"))
        .unwrap_or_else(|| panic!("{name} should end in .{ext}"));
    let parts: Vec<&str> = stem.split('-').collect();
    assert_eq!(parts.len(), 3, "file name {name}");
    assert!(
        parts[0].parse::<u64>().is_ok() && parts[0].len() >= 13,
        "unix ms in {name}"
    );
    assert_eq!(parts[1], "doctor", "command in {name}");
    assert!(parts[2].parse::<u32>().is_ok(), "pid in {name}");
    assert_eq!(mode(path), 0o600, "spill file mode");
    let entries: Vec<_> = fs::read_dir(&out_dir).unwrap().flatten().collect();
    assert_eq!(
        entries.len(),
        1,
        "out/ holds only the spill (no temp files left): {:?}",
        list_tree(&out_dir)
    );
    fs::read(path).unwrap()
}

#[test]
fn ac018_json_output_over_the_limit_is_spilled() {
    let sb = Sandbox::new();
    let out = sb
        .glen(["doctor"])
        .fake_omp()
        .env("GLEN_MAX_OUTPUT", "100")
        .run();
    let (path, bytes, format) = spilled(&out);
    assert_eq!(format, "json");
    assert!(bytes > 100, "{}", out.ctx());
    let content = assert_spill_file(&sb, &path, "json");
    assert_eq!(
        content.len() as u64,
        bytes,
        "spilled.bytes is the file size"
    );
    let full = json_docs(&content).expect("spill holds the full JSON document");
    assert_eq!(full.len(), 1);
    assert_eq!(full[0]["v"], Value::from(1));
    assert_eq!(full[0]["ok"], Value::Bool(true));
    assert!(full[0]["data"]["checks"].is_array());
    // AC-019: directories glen creates are 0700.
    assert_eq!(mode(&sb.glen_home), 0o700);
    assert_eq!(mode(&sb.glen_home.join("out")), 0o700);
}

#[test]
fn ac018_human_output_over_the_limit_is_spilled_without_ansi() {
    let sb = Sandbox::new();
    let out = sb
        .glen(["doctor", "--human"])
        .fake_omp()
        .env("GLEN_MAX_OUTPUT", "100")
        .env("CLICOLOR_FORCE", "1")
        .run();
    out.assert_exit(0);
    let text = strip_ansi(&out.out());
    assert_eq!(
        text.trim_end().lines().count(),
        1,
        "one line\n{}",
        out.ctx()
    );
    assert!(text.contains("KB"), "names the size\n{}", out.ctx());
    let out_dir = s(&sb.glen_home.join("out"));
    let start = text
        .find(&out_dir)
        .unwrap_or_else(|| panic!("names the spill path\n{}", out.ctx()));
    let path: String = text[start..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    let content = assert_spill_file(&sb, Path::new(&path), "txt");
    assert!(!has_esc(&content), "spilled text has no ANSI");
    let content = String::from_utf8(content).unwrap();
    assert!(content.contains("doctor") && content.contains(TESTED_OMP));
    assert_eq!(mode(&sb.glen_home), 0o700);
}

#[test]
fn ac018_spill_threshold_boundary() {
    let sb = Sandbox::new();
    let base = sb
        .glen(["doctor"])
        .fake_omp()
        .env("GLEN_MAX_OUTPUT", "0")
        .run();
    base.ok_data();
    let n = base.stdout.len();

    let same = sb
        .glen(["doctor"])
        .fake_omp()
        .env("GLEN_MAX_OUTPUT", n.to_string())
        .run();
    same.ok_data();
    assert_eq!(
        same.stdout, base.stdout,
        "output at the limit is not spilled"
    );
    assert!(!sb.glen_home.exists());

    let over = sb
        .glen(["doctor"])
        .fake_omp()
        .env("GLEN_MAX_OUTPUT", (n - 2).to_string())
        .run();
    let (path, bytes, _) = spilled(&over);
    assert!(
        bytes == n as u64 || bytes == n as u64 - 1,
        "spilled.bytes {bytes} vs rendered {n}"
    );
    assert_eq!(fs::metadata(path).unwrap().len(), bytes);
}

#[test]
fn ac018_default_limit_is_24000_and_zero_disables() {
    let sb = Sandbox::new();
    let long = long_agent_dir();

    let out = sb
        .glen(["doctor"])
        .fake_omp()
        .env("PI_CODING_AGENT_DIR", &long)
        .run();
    let (path, bytes, format) = spilled(&out);
    assert_eq!(format, "json");
    assert!(bytes > 24000);
    let content = assert_spill_file(&sb, &path, "json");
    let full = json_docs(&content).unwrap();
    assert_eq!(
        full[0]["data"]["paths"]["agent_dir"],
        Value::from(long.as_str())
    );

    let sb = Sandbox::new();
    let out = sb
        .glen(["doctor"])
        .fake_omp()
        .env("PI_CODING_AGENT_DIR", &long)
        .env("GLEN_MAX_OUTPUT", "0")
        .run();
    let data = out.ok_data();
    assert!(out.stdout.len() > 24000);
    assert_eq!(data["paths"]["agent_dir"], Value::from(long.as_str()));
    assert!(!sb.glen_home.exists(), "nothing spilled, nothing created");
}

#[test]
fn ac018_invalid_limit_warns_and_uses_the_default() {
    let sb = Sandbox::new();
    for value in ["abc", "-5", "1.5"] {
        let out = sb
            .glen(["doctor"])
            .fake_omp()
            .env("GLEN_MAX_OUTPUT", value)
            .run();
        let env = out.ok_envelope();
        assert!(
            env["data"].is_object(),
            "small output is not spilled\n{}",
            out.ctx()
        );
        let codes: Vec<&str> = env["warnings"]
            .as_array()
            .unwrap_or_else(|| panic!("warning expected\n{}", out.ctx()))
            .iter()
            .filter_map(|w| w["code"].as_str())
            .collect();
        assert!(codes.contains(&"invalid_env"), "{}", out.ctx());
    }
    assert!(!sb.glen_home.exists());

    let out = sb
        .glen(["doctor"])
        .fake_omp()
        .env("PI_CODING_AGENT_DIR", long_agent_dir())
        .env("GLEN_MAX_OUTPUT", "abc")
        .run();
    spilled(&out);
}

#[test]
fn ac018_error_envelopes_and_raw_output_are_never_spilled() {
    let sb = Sandbox::new();
    sb.glen(["bogus"])
        .env("GLEN_MAX_OUTPUT", "10")
        .run()
        .error_envelope("usage");

    let out = sb.glen(["doctor"]).env("GLEN_MAX_OUTPUT", "10").run();
    let err = out.error_envelope("omp_unavailable");
    assert!(err["details"]["report"].is_object());

    let out = sb
        .glen(["doctor"])
        .env("PI_CODING_AGENT_DIR", long_agent_dir())
        .run();
    out.error_envelope("omp_unavailable");
    assert!(out.stdout.len() > 24000);

    let out = sb
        .glen(["completions", "bash"])
        .env("GLEN_MAX_OUTPUT", "10")
        .run();
    out.assert_exit(0);
    assert!(out.stdout.len() > 10 && !out.is_json(), "{}", out.ctx());

    assert!(!sb.glen_home.exists(), "{:?}", list_tree(&sb.root));
}

#[test]
fn ac019_spill_keeps_the_mode_of_an_existing_glen_home() {
    let sb = Sandbox::new();
    fs::create_dir(&sb.glen_home).unwrap();
    set_mode(&sb.glen_home, 0o755);
    let out = sb
        .glen(["doctor"])
        .fake_omp()
        .env("GLEN_MAX_OUTPUT", "100")
        .run();
    let (path, _, _) = spilled(&out);
    assert_eq!(
        mode(&sb.glen_home),
        0o755,
        "existing GLEN_HOME is not chmodded"
    );
    assert_eq!(mode(&sb.glen_home.join("out")), 0o700);
    assert_eq!(mode(&path), 0o600);

    // An existing out/ keeps its mode too.
    let sb = Sandbox::new();
    fs::create_dir_all(sb.glen_home.join("out")).unwrap();
    set_mode(&sb.glen_home.join("out"), 0o750);
    let out = sb
        .glen(["doctor"])
        .fake_omp()
        .env("GLEN_MAX_OUTPUT", "100")
        .run();
    spilled(&out);
    assert_eq!(mode(&sb.glen_home.join("out")), 0o750);
}

#[test]
fn a6_spill_prunes_outputs_older_than_seven_days() {
    let sb = Sandbox::new();
    let out_dir = sb.glen_home.join("out");
    fs::create_dir_all(&out_dir).unwrap();
    set_mode(&sb.glen_home, 0o700);
    set_mode(&out_dir, 0o700);
    let now = SystemTime::now();
    let old = out_dir.join("1000000000000-doctor-1.json");
    let recent = out_dir.join("1000000000001-doctor-2.json");
    for (p, age_days) in [(&old, 8), (&recent, 1)] {
        fs::write(p, "{}\n").unwrap();
        let f = fs::File::options().write(true).open(p).unwrap();
        f.set_modified(now - Duration::from_secs(age_days * 86_400))
            .unwrap();
    }
    let out = sb
        .glen(["doctor"])
        .fake_omp()
        .env("GLEN_MAX_OUTPUT", "100")
        .run();
    let (path, _, _) = spilled(&out);
    assert!(path.exists());
    assert!(!old.exists(), "a week-old spill is pruned");
    assert!(recent.exists(), "a recent spill is kept");
}

#[test]
fn ac027_commands_do_not_create_glen_home_or_omp_dirs() {
    let invocations: &[(&[&str], bool)] = &[
        (&["--help"], false),
        (&["--version"], false),
        (&[], false),
        (&["help", "ls"], false),
        (&["doctor"], true),
        (&["doctor", "--human"], true),
        (&["doctor", "--profile", "work"], true),
        (&["doctor"], false),
        (&["doctor", "--human"], false),
        (&["ls"], false),
        (&["jobs"], false),
        (&["wait", "j1234567"], false),
        (&["show", "abc1234"], false),
        (&["send", "abc1234", "hi"], false),
        (&["install-extension", "--dry-run"], false),
        (&["inbox"], false),
        (&["inbox", "--peek"], false),
        (&["install-skill", "--dry-run"], false),
        (&["install-skill", "--project", "--dry-run"], false),
        (&["completions", "zsh"], false),
        (&["bogus"], false),
    ];
    for (args, with_omp) in invocations {
        for unset_glen_home in [false, true] {
            let sb = Sandbox::new();
            let mut run = sb.glen(*args);
            if *with_omp {
                run = run.fake_omp();
            }
            if unset_glen_home {
                run = run.env_remove("GLEN_HOME");
            }
            let out = run.run();
            assert!(
                !sb.glen_home.exists() && is_empty_dir(&sb.home) && is_empty_dir(&sb.cwd),
                "state created: home={:?} cwd={:?}\n{}",
                list_tree(&sb.home),
                list_tree(&sb.cwd),
                out.ctx()
            );
        }
    }
}

#[test]
fn a1_doctor_never_writes_into_omp_directories() {
    fn snapshot(root: &Path) -> Vec<(String, u64, SystemTime)> {
        list_tree(root)
            .into_iter()
            .map(|rel| {
                let md = fs::symlink_metadata(root.join(&rel)).unwrap();
                (rel, md.len(), md.modified().unwrap())
            })
            .collect()
    }
    let sb = Sandbox::new();
    let before = snapshot(&fixture_agent_dir());
    sb.glen(["doctor"])
        .fake_omp()
        .env("PI_CODING_AGENT_DIR", fixture_agent_dir())
        .run()
        .ok_data();
    assert_eq!(snapshot(&fixture_agent_dir()), before);

    // XDG roots are only probed, never populated.
    let data = sb.dir("data");
    let state = sb.dir("state");
    fs::create_dir(data.join("omp")).unwrap();
    fs::create_dir(state.join("omp")).unwrap();
    sb.glen(["doctor"])
        .fake_omp()
        .env("XDG_DATA_HOME", &data)
        .env("XDG_STATE_HOME", &state)
        .run()
        .ok_data();
    assert!(is_empty_dir(&data.join("omp")) && is_empty_dir(&state.join("omp")));
    assert!(is_empty_dir(&sb.home));
}
