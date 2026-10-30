mod common;
use common::{OmpBox, data};
use serde_json::{Value, json};
use std::fs;

#[test]
fn show_recovers_invalid_header_by_filename_continuation() {
    let sb = OmpBox::new();
    let path = sb.session('B');
    fs::write(&path, b"garbage\n").unwrap();
    let out = sb.glen(&["show", path.to_str().unwrap(), "--json"], &sb.alpha());
    let d = data(&out);
    assert!(d["session"]["id"].as_str().unwrap().ends_with("b7e2d81"));
    assert_eq!(
        d["continued_from"],
        json!(["01a0fd05-a240-7c41-9d02-7d2e8f13a5b6"])
    );
    sb.clean();
}

#[test]
fn show_and_ls_share_all_title_fallbacks() {
    let sb = OmpBox::new();
    let path = sb.session('A');
    for (slot, header, compact, expected) in [
        (json!("slot"), json!("header"), json!("summary"), "slot"),
        (json!(42), json!("header"), json!("summary"), "header"),
        (json!(""), json!("header"), json!("summary"), "summary"),
        (json!(""), Value::Null, Value::Null, "preview"),
    ] {
        let entries = [
            json!({"type":"title","title":slot}),
            json!({"type":"session","version":3,"id":"title-session","title":header}),
            json!({"type":"message","id":"u","message":{"role":"user","content":"preview"}}),
            json!({"type":"compaction","id":"c","parentId":"u","shortSummary":compact}),
        ];
        fs::write(
            &path,
            entries.iter().map(|e| format!("{e}\n")).collect::<String>(),
        )
        .unwrap();
        let listing = data(&sb.glen(&["ls", "--all", "--json"], &sb.alpha()));
        let row = listing["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == "title-session")
            .unwrap();
        let shown = data(&sb.glen(&["show", path.to_str().unwrap(), "--json"], &sb.alpha()));
        assert_eq!(row["title"], shown["session"]["title"]);
        assert_eq!(row["first_message"], shown["session"]["first_message"]);
        let show = sb.glen(&["show", path.to_str().unwrap(), "--human"], &sb.alpha());
        assert!(show.status.success(), "{show:?}");
        assert_eq!(
            String::from_utf8(show.stdout).unwrap().lines().next(),
            Some(expected)
        );
        let ls = sb.glen(&["ls", "--all", "--human"], &sb.alpha());
        assert!(String::from_utf8(ls.stdout).unwrap().contains(expected));
    }
    sb.clean();
}

#[test]
fn listing_footer_pluralizes_one_and_many() {
    let sb = OmpBox::new();
    let one = sb.agent.join("sessions/one");
    fs::create_dir(&one).unwrap();
    fs::write(
        one.join("one.jsonl"),
        "{\"type\":\"session\",\"id\":\"one\"}\n",
    )
    .unwrap();
    // Point the agent at a separate synthetic catalog with exactly one session.
    let isolated = sb.tmp.path().join("isolated-agent");
    fs::create_dir_all(isolated.join("sessions/one")).unwrap();
    fs::copy(
        one.join("one.jsonl"),
        isolated.join("sessions/one/one.jsonl"),
    )
    .unwrap();
    let out = sb
        .command(&["ls", "--all", "--human"], &sb.alpha())
        .env("PI_CODING_AGENT_DIR", &isolated)
        .output()
        .unwrap();
    assert!(out.status.success());
    let human = String::from_utf8(out.stdout).unwrap();
    assert!(human.contains("1 session · 0 live"), "{human}");
    fs::write(
        isolated.join("sessions/one/two.jsonl"),
        "{\"type\":\"session\",\"id\":\"two\"}\n",
    )
    .unwrap();
    let out = sb
        .command(&["ls", "--all", "--human"], &sb.alpha())
        .env("PI_CODING_AGENT_DIR", &isolated)
        .output()
        .unwrap();
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("2 sessions · 0 live")
    );
    sb.clean();
}

#[test]
fn show_uses_widened_catalog_handle_and_v1_transcript() {
    let sb = OmpBox::new();
    for id in [
        "00000000-0000-0000-0000-00001abcdef0",
        "00000000-0000-0000-0000-00002abcdef0",
    ] {
        let path = sb
            .agent
            .join("sessions/-work-alpha")
            .join(format!("{id}.jsonl"));
        fs::write(
            &path,
            format!("{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\"}}\n"),
        )
        .unwrap();
    }
    for handle in ["1abcdef0", "2abcdef0"] {
        assert_eq!(
            data(&sb.glen(&["show", handle, "--json"], &sb.alpha()))["session"]["handle"],
            handle
        );
    }
    let path = sb.agent.join("sessions/-work-alpha/legacy.jsonl");
    let original = include_bytes!("fixtures/v1-session.jsonl");
    fs::write(&path, original).unwrap();
    let d = data(&sb.glen(&["show", "legacy-", "--json"], &sb.alpha()));
    assert_eq!(d["skipped_lines"], 0);
    assert_eq!(d["items"][0]["text"], "old prompt");
    assert_eq!(d["items"][1]["text"], "old answer");
    assert_eq!(fs::read(&path).unwrap(), original);
    sb.clean();
}
