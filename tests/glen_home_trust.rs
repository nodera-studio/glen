//! SEC-3 (W8 G3): glen refuses to write into a GLEN_HOME another user could write to.
#![cfg(unix)]
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn glen(home: &Path, glen_home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_glen"))
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", home)
        .env("GLEN_HOME", glen_home)
        .env("GLEN_MAX_OUTPUT", "10")
        .output()
        .unwrap()
}

#[test]
fn a_world_writable_glen_home_is_refused_before_anything_is_written() {
    let t = tempfile::tempdir().unwrap();
    let shared = t.path().join("shared");
    fs::create_dir_all(shared.join("out")).unwrap();
    let planted = shared.join("out/notes.txt");
    fs::write(&planted, b"keep me").unwrap();
    for dir in [&shared, &shared.join("out")] {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o777)).unwrap();
    }
    // GLEN_MAX_OUTPUT forces the oversized envelope to spill into `$GLEN_HOME/out`.
    let out = glen(t.path(), &shared, &["ls", "--json"]);
    assert_ne!(out.status.code(), Some(0), "{out:?}");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("refusing GLEN_HOME directory"), "{text}");
    let names: Vec<_> = fs::read_dir(shared.join("out"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["notes.txt"], "no spill file was written");
    assert_eq!(fs::read(&planted).unwrap(), b"keep me");
    assert_eq!(
        fs::metadata(&shared).unwrap().permissions().mode() & 0o777,
        0o777,
        "glen never chmods an existing home"
    );
    // The same home made private is trusted again.
    for dir in [&shared, &shared.join("out")] {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let out = glen(t.path(), &shared, &["ls", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}
