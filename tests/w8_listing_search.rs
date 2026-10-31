//! ERR-5 (W8 review): a bucket that can be listed but not searched (mode 0444) must not hide
//! its sessions silently. Each file whose stat fails is reported as `unreadable_session`, and
//! a lookup that misses there is an io error, not not_found.
#![cfg(unix)]
mod common;
use common::*;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn a_listable_but_unsearchable_bucket_reports_its_sessions() {
    let sb = OmpBox::new();
    let beta = sb.agent.join("sessions/-work-beta");
    let files: Vec<_> = fs::read_dir(&beta)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    assert!(!files.is_empty());
    fs::set_permissions(&beta, fs::Permissions::from_mode(0o444)).unwrap();
    if fs::metadata(&files[0]).is_ok() {
        // Running as root: permissions are not enforced.
        fs::set_permissions(&beta, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let ls = json(&sb.glen(&["ls", "--all", "--json"], &sb.alpha()));
    let latest = sb.glen(&["show", "@latest", "--json"], &sb.beta());
    fs::set_permissions(&beta, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(ls["ok"], true, "{ls}");
    let warned: Vec<_> = ls["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"] == "unreadable_session")
        .map(|w| w["message"].as_str().unwrap().to_owned())
        .collect();
    for file in &files {
        assert!(
            warned.iter().any(|m| m.ends_with(file.to_str().unwrap())),
            "{file:?} not reported: {ls}"
        );
    }
    assert_eq!(latest.status.code(), Some(1), "{latest:?}");
    assert_eq!(json(&latest)["error"]["code"], "io");
    sb.clean();
}
