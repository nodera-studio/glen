use std::{path::PathBuf, process::Command};
#[test]
fn extension_unit_tests_under_node() {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let node = std::env::split_paths(&path)
        .map(|p| p.join("node"))
        .find(|p| p.is_file());
    let Some(node) = node else {
        assert!(
            std::env::var_os("CI").is_none(),
            "Node >=22.6 is required in CI"
        );
        eprintln!("skipping TypeScript extension tests: node is not on PATH");
        return;
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut tests = std::fs::read_dir(root.join("extension/glen/test"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".test.ts"))
        .collect::<Vec<_>>();
    tests.sort();
    assert!(!tests.is_empty());
    let run = |flag: bool| {
        let mut c = Command::new(&node);
        c.current_dir(&root).env_clear().env("PATH", &path);
        for key in ["HOME", "TMPDIR"] {
            if let Some(v) = std::env::var_os(key) {
                c.env(key, v);
            }
        }
        if flag {
            c.arg("--experimental-strip-types");
        }
        c.arg("--test").args(&tests).output().unwrap()
    };
    let mut o = run(true);
    if String::from_utf8_lossy(&o.stderr).contains("bad option") {
        o = run(false);
    }
    assert!(
        o.status.success(),
        "Node extension tests failed\n{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}
