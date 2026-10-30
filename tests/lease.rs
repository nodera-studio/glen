#![cfg(unix)]
mod common;
use common::{Holder, OmpBox, data};
use std::{fs, path::PathBuf};
#[test]
#[ignore]
fn lease_holder() {
    common::hold_lease_from_env()
}

fn lock(sb: &OmpBox, key: char) -> PathBuf {
    let s = glen::omp::session::Session::load(&sb.session(key)).unwrap();
    let dir = sb.home.join(".omp/run/session-owners");
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("{}.lock", s.header.id));
    fs::write(&p, b"preserve").unwrap();
    p
}
#[test]
fn ac108_109_cross_process() {
    let sb = OmpBox::new();
    for (key, handle, state) in [('B', "f13a5b6", "busy"), ('A', "e27c0de", "live")] {
        let p = lock(&sb, key);
        let before = fs::metadata(&p).unwrap();
        let mut holder = Holder::new(&p);
        let d = data(&sb.glen(&["ls", "--all", "--json"], &sb.alpha()));
        let rows = d["sessions"].as_array().unwrap();
        for r in rows {
            assert_eq!(
                r["state"],
                if r["handle"] == handle { state } else { "idle" }
            );
        }
        let live = data(&sb.glen(&["ls", "--all", "--live"], &sb.alpha()));
        assert_eq!(live["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(live["sessions"][0]["handle"], handle);
        let after = fs::metadata(&p).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        assert_eq!(before.len(), after.len());
        assert_eq!(fs::read(&p).unwrap(), b"preserve");
        holder.release();
        let d = data(&sb.glen(&["ls", "--all"], &sb.alpha()));
        assert!(
            d["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["state"] == "idle")
        );
    }
    assert!(!sb.glen_home.exists());
}
#[cfg(not(target_os = "linux"))]
#[test]
fn ac108_permission_is_unknown() {
    use std::os::unix::fs::PermissionsExt;
    let sb = OmpBox::new();
    let p = lock(&sb, 'B');
    let dir = p.parent().unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&p).is_ok() {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }
    let o = sb.glen(&["ls", "--all"], &sb.alpha());
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    let d = data(&o);
    let b = d["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["handle"] == "f13a5b6")
        .unwrap();
    assert_eq!(b["state"], "unknown");
    assert!(b["state_detail"].is_string());
}
