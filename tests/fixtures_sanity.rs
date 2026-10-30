use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};
const FILES: [(&str, &str, &str); 8] = [
    (
        "A",
        "-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl",
        "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
    ),
    (
        "B",
        "-work-alpha/2026-10-02T14-30-00-000Z_01a0fd05-a240-7c41-9d02-7d2e8f13a5b6.jsonl",
        "01a0fd05-a240-7c41-9d02-7d2e8f13a5b6",
    ),
    (
        "C",
        "-work-alpha/2026-10-02T15-10-00-000Z_01a0fd2a-4140-7e05-a3f4-0c9a4b7e2d81.jsonl",
        "01a0fd2a-4140-7e05-a3f4-0c9a4b7e2d81",
    ),
    (
        "D",
        "-work-beta/2026-10-03T08-00-00-000Z_01a100c6-f000-7b18-8e6a-5e8b1c4f9a72.jsonl",
        "01a100c6-f000-7b18-8e6a-5e8b1c4f9a72",
    ),
    (
        "E",
        "-work-beta/2026-10-04T11-45-00-000Z_01a106bb-4a60-7f92-b0c3-b2d6e0a3f417.jsonl",
        "01a106bb-4a60-7f92-b0c3-b2d6e0a3f417",
    ),
    (
        "F",
        "-work-beta/2026-09-15T07-20-00-000Z_01a0a3ef-d900-7d6e-9a57-9b3d5a81c0de.jsonl",
        "01a0a3ef-d900-7d6e-9a57-9b3d5a81c0de",
    ),
    (
        "G",
        "-work-alpha/2026-10-05T16-20-00-000Z_01a10cdd-6b80-7a29-9c5e-6d1f3b8e4a07.jsonl",
        "01a10cdd-6b80-7a29-9c5e-6d1f3b8e4a07",
    ),
    (
        "S",
        "-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de/subagent-01.jsonl",
        "01a0f6b5-ba60-7012-8f3b-3f7a9c1e5b20",
    ),
];
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/omp/agent/sessions")
}
fn bytes(key: &str) -> Vec<u8> {
    fs::read(root().join(FILES.iter().find(|f| f.0 == key).unwrap().1)).unwrap()
}
fn objects(key: &str) -> Vec<Value> {
    String::from_utf8(bytes(key))
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}
fn header(key: &str) -> Value {
    objects(key)
        .into_iter()
        .find(|v| v["type"] == "session")
        .unwrap()
}
#[test]
fn slots_headers_runtime_and_entry_shape() {
    for (key, file, id) in FILES {
        let b = bytes(key);
        let first = b.split_inclusive(|c| *c == b'\n').next().unwrap();
        let first: Value = serde_json::from_slice(first).unwrap();
        if key == "F" || key == "S" {
            assert_eq!(first["type"], "session");
        } else {
            assert_eq!(
                b.split_inclusive(|c| *c == b'\n').next().unwrap().len(),
                256,
                "{key}"
            );
            assert_eq!(first["type"], "title");
            assert_eq!(first["v"], 1);
        }
        let h = header(key);
        assert_eq!(h["version"], 3);
        assert_eq!(h["id"], id);
        if key != "S" {
            assert!(file.ends_with(&format!("{id}.jsonl")));
        }
        assert_eq!(&id[14..15], "7");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
        let mut previous = "".to_string();
        let mut ids = BTreeSet::new();
        for v in objects(key) {
            assert_eq!(v["runtime"]["version"], "18.14.0");
            if v["type"] == "session" || v["type"] == "title" {
                continue;
            }
            let id = v["id"].as_str().unwrap();
            assert!(
                id.len() == 8
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            );
            assert!(ids.insert(id.to_string()));
            let ts = v["timestamp"].as_str().unwrap();
            assert!(ts >= previous.as_str(), "{key}: {ts} < {previous}");
            previous = ts.into();
        }
    }
}
#[test]
fn malformed_lines_are_deliberate() {
    for (key, _, _) in FILES {
        let text = String::from_utf8(bytes(key)).unwrap();
        let malformed = text
            .lines()
            .filter(|l| serde_json::from_str::<Value>(l).is_err())
            .collect::<Vec<_>>();
        if key == "E" {
            assert_eq!(
                malformed,
                [
                    "not json at all",
                    r#"{"type":"message","id":"deadbeef","parentId":"#
                ]
            );
            assert!(!text.ends_with('\n'));
        } else {
            assert!(malformed.is_empty(), "{key}");
        }
    }
}
#[test]
fn continuation_copies_and_fork() {
    assert_eq!(header("C")["continuationOf"], FILES[1].2);
    assert_eq!(header("C")["parentSession"], FILES[1].2);
    let b = objects("B")
        .into_iter()
        .filter(|v| v["type"] == "message")
        .collect::<Vec<_>>();
    assert_eq!(b.len(), 3);
    let c = objects("C");
    for entry in b {
        assert!(c.contains(&entry));
    }
    assert_eq!(header("G")["parentSession"], FILES[0].2);
    assert!(header("G").get("continuationOf").is_none());
}
#[test]
fn active_leaf_branch() {
    let d = objects("D");
    let last = d.last().unwrap();
    assert_eq!(last["activeLeafId"], "40000008");
    assert_ne!(last["id"], last["activeLeafId"]);
    let entries = d
        .iter()
        .filter_map(|v| v["id"].as_str().map(|id| (id, v)))
        .collect::<BTreeMap<_, _>>();
    let mut at = Some("40000008");
    let mut chain = Vec::new();
    while let Some(id) = at {
        assert!(!chain.contains(&id));
        chain.push(id);
        at = entries[id]["parentId"].as_str();
    }
    chain.reverse();
    assert_eq!(
        chain,
        [
            "40000001", "40000002", "40000005", "40000006", "40000007", "40000008"
        ]
    );
}
#[test]
fn unknowns_and_message_shapes() {
    let e = objects("E");
    assert_eq!(e.iter().filter(|v| v["type"] == "future_kind").count(), 1);
    assert!(
        e.iter()
            .any(|v| v["futureField"]["nested"] == true && v["message"]["newMessageField"] == 1)
    );
    assert!(e.iter().any(|v| {
        v["message"]["content"]
            .as_array()
            .is_some_and(|b| b.iter().any(|x| x["type"] == "future_block"))
    }));
    assert!(e.iter().any(|v| v["type"] == "compaction"));
    let mut string_users = 0;
    let mut roles = BTreeSet::new();
    let mut blocks = BTreeSet::new();
    let mut kinds = BTreeSet::new();
    for (key, _, _) in FILES {
        for v in objects(key) {
            kinds.insert(v["type"].as_str().unwrap().to_string());
            if let Some(role) = v["message"]["role"].as_str() {
                roles.insert(role.to_string());
                if role == "user" {
                    let content = &v["message"]["content"];
                    if content.is_string() {
                        assert!(key == "B" || key == "C");
                        assert_eq!(content, "And the second fixture?");
                        string_users += 1;
                    } else {
                        assert!(content.is_array());
                    }
                }
                if let Some(content) = v["message"]["content"].as_array() {
                    for block in content {
                        blocks.insert(block["type"].as_str().unwrap().to_string());
                    }
                }
            }
        }
    }
    assert_eq!(string_users, 2);
    for role in ["user", "assistant", "toolResult", "bashExecution"] {
        assert!(roles.contains(role));
    }
    for block in ["thinking", "toolCall", "text"] {
        assert!(blocks.contains(block));
    }
    for kind in [
        "model_change",
        "thinking_level_change",
        "compaction",
        "future_kind",
    ] {
        assert!(kinds.contains(kind));
    }
}
#[test]
fn layout_and_handle_edges() {
    assert!(root().join("-work-alpha").is_dir());
    assert!(root().join("-work-beta").is_dir());
    let lock = root().join(
        "-work-alpha/.2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl.lock.os",
    );
    assert_eq!(fs::metadata(lock).unwrap().len(), 0);
    assert!(root().join(FILES[7].1).is_file());
    let a = FILES[0].2;
    let f = FILES[5].2;
    assert_eq!(&a[a.len() - 4..], &f[f.len() - 4..]);
    assert_ne!(&a[a.len() - 7..], &f[f.len() - 7..]);
    assert_eq!(&a[..7], &FILES[7].2[..7]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bin/fake-omp");
        assert_eq!(
            fs::metadata(bin).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}
