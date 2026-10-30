mod common;
use common::{OmpBox, data, json};
use serde_json::json as j;
#[test]
fn ac110_115_active_transcript() {
    let sb = OmpBox::new();
    let d = data(&sb.glen(&["show", "e27c0de", "--json"], &sb.alpha()));
    assert_eq!(
        d["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "thinking_level",
            "model_change",
            "user",
            "assistant",
            "assistant"
        ]
    );
    assert!(d["items"][3].get("thinking").is_none());
    assert!(d["items"][3]["tool_calls"][0]["result"].is_object());
    assert_eq!(
        data(&sb.glen(&["show", "e27c0de", "--thinking"], &sb.alpha()))["items"][3]["thinking"],
        "Check the fixture loader."
    );
    assert!(data(&sb.glen(&["show","e27c0de","--tools","full"],&sb.alpha()))["items"][3]["tool_calls"][0]["arguments"].is_object());
    assert!(
        data(&sb.glen(&["show", "e27c0de", "--tools", "none"], &sb.alpha()))["items"][3]
            .get("tool_calls")
            .is_none()
    );
    let b = data(&sb.glen(&["show", "f13a5b6"], &sb.alpha()));
    assert!(b["session"]["id"].as_str().unwrap().ends_with("b7e2d81"));
    assert_eq!(
        b["continued_from"],
        j!(["01a0fd05-a240-7c41-9d02-7d2e8f13a5b6"])
    );
    for h in ["e27c0de", "b8e4a07"] {
        let d = data(&sb.glen(&["show", h], &sb.alpha()));
        assert_eq!(d["session"]["handle"], h);
        assert_eq!(d["continued_from"], j!([]));
    }
    let d = data(&sb.glen(&["show", "c4f9a72"], &sb.alpha()));
    assert_eq!(
        d["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["40000001", "40000002", "40000005", "40000006", "40000007"]
    );
    let d = data(&sb.glen(&["show", "c4f9a72", "--raw"], &sb.alpha()));
    assert!(d.get("items").is_none());
    assert_eq!(
        d["entries"].as_array().unwrap().last().unwrap()["id"],
        "40000008"
    );
    let d = data(&sb.glen(&["show", "0a3f417"], &sb.alpha()));
    assert_eq!(d["skipped_lines"], 2);
    assert_eq!(d["items"].as_array().unwrap().len(), 10);
    let raw = data(&sb.glen(&["show", "0a3f417", "--raw"], &sb.alpha()));
    assert!(
        raw["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "future_kind")
    );
    sb.clean();
}
#[test]
fn ac110_resolution_errors_and_paths() {
    let sb = OmpBox::new();
    for (arg, code, kind) in [
        ("c0de", 4, "ambiguous"),
        ("01a0f6b", 3, "not_found"),
        ("j1234567", 2, "usage"),
        ("@new", 2, "usage"),
    ] {
        let o = sb.glen(&["show", arg], &sb.alpha());
        assert_eq!(o.status.code(), Some(code));
        let e = json(&o);
        assert_eq!(e["error"]["code"], kind);
        if code == 4 {
            assert_eq!(
                e["error"]["details"]["candidates"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            )
        }
    }
    assert_eq!(
        data(&sb.glen(&["show", "@latest"], &sb.alpha()))["session"]["handle"],
        "b8e4a07"
    );
    assert_eq!(
        data(&sb.glen(&["show", sb.session('E').to_str().unwrap()], &sb.alpha()))["skipped_lines"],
        2
    );
    let o = sb.glen(
        &["show", common::fixture("README.md").to_str().unwrap()],
        &sb.alpha(),
    );
    assert_eq!(o.status.code(), Some(3));
    let o = sb.glen(&["show", "c0de", "--human"], &sb.alpha());
    let s = String::from_utf8(o.stdout).unwrap();
    assert!(s.contains("e27c0de") && s.contains("a81c0de"));
    sb.clean();
}
#[test]
fn ac120_spill() {
    let sb = OmpBox::new();
    let o = sb
        .command(&["show", "0a3f417", "--json"], &sb.alpha())
        .env("GLEN_MAX_OUTPUT", "200")
        .output()
        .unwrap();
    assert!(o.status.success());
    let v = json(&o);
    let path = std::path::Path::new(v["spilled"]["path"].as_str().unwrap());
    assert!(path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let full: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(full["data"]["items"].as_array().unwrap().len(), 10);
    let o = sb
        .command(&["show", "0a3f417", "--human"], &sb.alpha())
        .env("GLEN_MAX_OUTPUT", "200")
        .output()
        .unwrap();
    assert!(
        String::from_utf8(o.stdout)
            .unwrap()
            .contains("full output in")
    );
    assert!(!sb.home.join(".omp").exists());
}
#[test]
fn ac121_human() {
    let sb = OmpBox::new();
    let o = sb
        .command(&["show", "e27c0de", "--human"], &sb.alpha())
        .env("COLUMNS", "80")
        .output()
        .unwrap();
    assert!(o.status.success());
    let s = String::from_utf8(o.stdout).unwrap();
    for part in [
        "❯ user · 09:00",
        "◆ assistant",
        "↳ read tests/parser_test.rs",
        "∴ thinking",
    ] {
        assert!(s.contains(part), "{s}")
    }
    for l in s.lines() {
        assert!(unicode_width::UnicodeWidthStr::width(l) <= 80, "{l}")
    }
    sb.clean();
}
