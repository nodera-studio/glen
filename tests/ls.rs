mod common;
use common::{OmpBox, data, json};
use std::{
    fs,
    time::{Duration, SystemTime},
};
#[test]
fn ac102_108_catalog_scope_filters() {
    let sb = OmpBox::new();
    let d = data(&sb.glen(&["ls", "--all", "--json"], &sb.alpha()));
    let rows = d["sessions"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|s| s["handle"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "b8e4a07", "0a3f417", "c4f9a72", "b7e2d81", "f13a5b6", "e27c0de", "a81c0de"
        ]
    );
    assert_eq!(d["total_all"], 7);
    for s in rows {
        assert_eq!(s["state"], "idle");
        assert_eq!(s["state_source"], "disk");
        for key in [
            "handle",
            "id",
            "path",
            "bucket",
            "cwd",
            "title",
            "first_message",
            "created_at",
            "updated_at",
            "age_ms",
            "size_bytes",
            "model",
            "last_turn",
            "state",
            "state_source",
            "state_detail",
            "continuation_of",
            "parent_session",
        ] {
            assert!(s.get(key).is_some(), "{key}")
        }
        assert!(std::path::Path::new(s["path"].as_str().unwrap()).is_absolute());
        for k in ["created_at", "updated_at"] {
            s[k].as_str().unwrap().parse::<jiff::Timestamp>().unwrap();
        }
    }
    assert_eq!(rows[4]["last_turn"], "pending");
    let d = data(&sb.glen(&["ls", "--json"], &sb.alpha()));
    assert_eq!(d["scope"]["bucket"], "-work-alpha");
    assert_eq!(
        d["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["handle"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["b8e4a07", "b7e2d81", "f13a5b6", "e27c0de"]
    );
    let d = data(&sb.glen(&["ls", "--cwd", sb.beta().to_str().unwrap()], &sb.alpha()));
    assert_eq!(d["sessions"].as_array().unwrap().len(), 3);
    assert_eq!(d["scope"]["bucket"], "-work-beta");
    for (args, n, total) in [
        (vec!["ls", "-n", "2"], 2, 4),
        (vec!["ls", "-n", "0"], 4, 4),
        (vec!["ls", "--all", "--since", "10m"], 6, 6),
        (vec!["ls", "--all", "--live"], 0, 0),
    ] {
        let d = data(&sb.glen(&args, &sb.alpha()));
        assert_eq!(d["sessions"].as_array().unwrap().len(), n);
        assert_eq!(d["total"], total);
        sb.clean();
    }
    sb.clean();
}
#[test]
fn ac103_122_missing_and_usage() {
    let sb = OmpBox::new();
    let o = sb.glen(&["ls", "--all", "--cwd", "x"], &sb.alpha());
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(json(&o)["error"]["code"], "usage");
    let o = sb
        .command(&["ls"], &sb.alpha())
        .env("PI_CODING_AGENT_DIR", sb.tmp.path().join("missing"))
        .output()
        .unwrap();
    assert_eq!(data(&o)["sessions"], serde_json::json!([]));
    sb.clean();
}
#[test]
fn ac121_123_width_and_sanitizing() {
    let sb = OmpBox::new();
    let a = sb.session('A');
    let content = fs::read_to_string(&a).unwrap().replace(
        "Fix the flaky parser test",
        &("Fix the flaky parser test ".to_owned() + &"long title ".repeat(10)),
    );
    fs::write(a, content).unwrap();
    for width in [20, 40, 60, 80] {
        let o = sb
            .command(&["ls", "--human", "--all"], &sb.alpha())
            .env("COLUMNS", width.to_string())
            .output()
            .unwrap();
        assert!(o.status.success());
        let s = String::from_utf8(o.stdout).unwrap();
        for l in s.lines() {
            assert!(
                unicode_width::UnicodeWidthStr::width(l) <= width,
                "{width}: {l:?}"
            )
        }
        if width == 60 {
            assert!(s.starts_with("handle"));
            assert!(s.contains('…'));
        }
    }
    let o = sb
        .command(&["ls", "--human"], &sb.alpha())
        .env("NO_COLOR", "1")
        .env("CLICOLOR_FORCE", "1")
        .output()
        .unwrap();
    assert!(!o.stdout.contains(&27));
    let p = sb.session('A');
    let b = fs::read_to_string(&p)
        .unwrap()
        .replace("Fix the flaky parser test", "evil\\u001b[31m");
    fs::write(p, b).unwrap();
    let o = sb.glen(&["ls", "--human"], &sb.alpha());
    let s = String::from_utf8(o.stdout).unwrap();
    assert!(s.contains("\\x1b"));
    assert!(!s.contains('\x1b'));
    sb.clean();
}
#[test]
fn ac126_future_mtime() {
    let sb = OmpBox::new();
    fs::File::open(sb.session('A'))
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(86400))
        .unwrap();
    let d = data(&sb.glen(&["ls"], &sb.alpha()));
    assert_eq!(d["sessions"][0]["handle"], "e27c0de");
    assert_eq!(d["sessions"][0]["age_ms"], 0);
    let o = sb.glen(&["ls", "--human"], &sb.alpha());
    assert!(String::from_utf8(o.stdout).unwrap().contains("just now"));
    sb.clean();
}

#[test]
fn session_text_is_raw_in_json_and_escaped_on_single_human_lines() {
    let sb = OmpBox::new();
    let title = "  alpha\t beta\n\x1b[31m gamma\x07  ";
    let preview = "  preview\t text\n\x1b[31m next  ";
    let path = sb.session('A');
    let mut entries = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    entries[0]["title"] = serde_json::json!(title);
    let user = entries
        .iter_mut()
        .find(|entry| entry["message"]["role"] == "user")
        .unwrap();
    user["message"]["content"] = serde_json::json!(preview);
    let content = entries
        .iter()
        .map(|entry| format!("{entry}\n"))
        .collect::<String>();
    fs::write(path, content).unwrap();
    let d = data(&sb.glen(&["ls", "--all", "--json"], &sb.alpha()));
    let row = d["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["handle"] == "e27c0de")
        .unwrap();
    assert_eq!(row["title"], title);
    assert_eq!(row["first_message"], preview);
    for args in [
        vec!["ls", "--human"],
        vec!["show", "e27c0de", "--human"],
        vec!["tail", "e27c0de", "-n", "1", "--human"],
    ] {
        let o = sb.glen(&args, &sb.alpha());
        assert!(o.status.success());
        let text = String::from_utf8(o.stdout).unwrap();
        assert!(text.contains("alpha beta \\x1b[31m gamma\\x07"), "{text}");
        assert!(!text.contains('\x1b'));
    }
    sb.clean();
}
