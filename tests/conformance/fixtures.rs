//! The omp session fixtures, checked against the plan's Step 9 specification (AC-023).
//! Read-only: these tests never write under tests/fixtures/.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use crate::support::*;
use serde_json::{Value, json};

const ALPHA: &str = "-work-alpha";
const BETA: &str = "-work-beta";

const A: &str = "-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl";
const B: &str = "-work-alpha/2026-10-02T14-30-00-000Z_01a0fd05-a240-7c41-9d02-7d2e8f13a5b6.jsonl";
const C: &str = "-work-alpha/2026-10-02T15-10-00-000Z_01a0fd2a-4140-7e05-a3f4-0c9a4b7e2d81.jsonl";
const D: &str = "-work-beta/2026-10-03T08-00-00-000Z_01a100c6-f000-7b18-8e6a-5e8b1c4f9a72.jsonl";
const E: &str = "-work-beta/2026-10-04T11-45-00-000Z_01a106bb-4a60-7f92-b0c3-b2d6e0a3f417.jsonl";
const F: &str = "-work-beta/2026-09-15T07-20-00-000Z_01a0a3ef-d900-7d6e-9a57-9b3d5a81c0de.jsonl";
const G: &str = "-work-alpha/2026-10-05T16-20-00-000Z_01a10cdd-6b80-7a29-9c5e-6d1f3b8e4a07.jsonl";
const S: &str =
    "-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de/subagent-01.jsonl";
const S_ID: &str = "01a0f6b5-ba60-7012-8f3b-3f7a9c1e5b20";
const LOCK: &str =
    "-work-alpha/.2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl.lock.os";

const TOP_LEVEL: [&str; 7] = [A, B, C, D, E, F, G];
const WITH_SLOT: [&str; 6] = [A, B, C, D, E, G];

fn sessions() -> PathBuf {
    fixture_agent_dir().join("sessions")
}

/// The uuid in a session file name.
fn file_id(rel: &str) -> &str {
    let name = rel.rsplit('/').next().unwrap();
    let stem = name.strip_suffix(".jsonl").unwrap();
    stem.split_once('_').unwrap().1
}

fn hex_id(id: &str) -> String {
    id.replace('-', "")
}

struct Session {
    raw: String,
    /// Every physical line, with `None` for lines that are not JSON.
    lines: Vec<(String, Option<Value>)>,
}

impl Session {
    fn load(rel: &str) -> Self {
        let raw = fs::read_to_string(sessions().join(rel))
            .unwrap_or_else(|e| panic!("read fixture {rel}: {e}"));
        let lines = raw
            .split_inclusive('\n')
            .map(|l| {
                let text = l.strip_suffix('\n').unwrap_or(l).to_owned();
                let json = serde_json::from_str::<Value>(&text).ok();
                (text, json)
            })
            .collect();
        Session { raw, lines }
    }

    fn header(&self) -> &Value {
        self.lines
            .iter()
            .filter_map(|(_, j)| j.as_ref())
            .find(|j| j["type"] == "session")
            .expect("a session header")
    }

    /// Parsed tree entries: everything except the title slot and the header.
    fn entries(&self) -> Vec<&Value> {
        self.lines
            .iter()
            .filter_map(|(_, j)| j.as_ref())
            .filter(|j| j["type"] != "title" && j["type"] != "session")
            .collect()
    }

    fn malformed(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter(|(_, j)| j.is_none())
            .map(|(t, _)| t.as_str())
            .collect()
    }
}

/// `type`, or `message/<role>` for message entries.
fn kind(e: &Value) -> String {
    match e["type"].as_str().unwrap_or("?") {
        "message" => format!("message/{}", e["message"]["role"].as_str().unwrap_or("?")),
        t => t.to_owned(),
    }
}

fn kinds(entries: &[&Value]) -> Vec<String> {
    entries.iter().map(|e| kind(e)).collect()
}

fn id(e: &Value) -> &str {
    e["id"].as_str().expect("entry id")
}

fn parent(e: &Value) -> &Value {
    &e["parentId"]
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `2026-10-01T09-00-00-000Z` → unix ms.
fn file_timestamp_ms(rel: &str) -> u64 {
    let name = rel.rsplit('/').next().unwrap();
    let ts = name.split_once('_').unwrap().0;
    let n: Vec<i64> = ts
        .trim_end_matches('Z')
        .split(['-', 'T'])
        .map(|p| p.parse().unwrap())
        .collect();
    let days = days_from_civil(n[0], n[1], n[2]);
    let secs = days * 86_400 + n[3] * 3600 + n[4] * 60 + n[5];
    (secs * 1000 + n[6]) as u64
}

#[test]
fn layout_buckets_artifacts_and_lock_sibling() {
    for bucket in [ALPHA, BETA] {
        assert!(sessions().join(bucket).is_dir(), "bucket {bucket}");
    }
    for rel in TOP_LEVEL.iter().chain([&S]) {
        assert!(sessions().join(rel).is_file(), "missing fixture {rel}");
    }
    let artifact_dir = sessions().join(A.strip_suffix(".jsonl").unwrap());
    assert!(artifact_dir.is_dir(), "A's subagent artifact directory");
    assert!(sessions().join(S).starts_with(&artifact_dir));
    let lock = fs::metadata(sessions().join(LOCK)).expect(".lock.os sibling exists");
    assert!(lock.is_file());
    assert_eq!(lock.len(), 0, ".lock.os is empty");
}

#[test]
fn title_slots_are_exactly_256_bytes() {
    for rel in WITH_SLOT {
        let sess = Session::load(rel);
        let first = sess.raw.split_inclusive('\n').next().unwrap();
        assert_eq!(first.len(), 256, "{rel}: title slot incl. newline");
        let slot = sess.lines[0].1.as_ref().expect("title slot is JSON");
        assert_eq!(slot["type"], "title", "{rel}");
        assert_eq!(slot["v"], 1, "{rel}");
        assert!(
            slot["title"].as_str().is_some_and(|t| !t.is_empty()),
            "{rel}"
        );
        assert_eq!(
            sess.lines[1].1.as_ref().unwrap()["type"],
            "session",
            "{rel}: header on line 2"
        );
    }
    let a = Session::load(A);
    let slot = a.lines[0].1.as_ref().unwrap();
    assert_eq!(slot["title"], "Fix the flaky parser test");
    assert_eq!(slot["source"], "auto");
    assert!(slot["updatedAt"].is_string() || slot["updatedAt"].is_number());
    assert!(
        slot["pad"]
            .as_str()
            .is_some_and(|p| p.chars().all(|c| c == ' ')),
        "pad is spaces"
    );
}

#[test]
fn legacy_file_has_no_title_slot() {
    let f = Session::load(F);
    let first = f.lines[0].1.as_ref().expect("F line 1 is JSON");
    assert_eq!(first["type"], "session", "F starts with its header");
    assert_eq!(first["version"], 3);
    assert_eq!(first["title"], "Legacy title");
    assert_eq!(kinds(&f.entries()), ["message/user", "message/assistant"]);
}

#[test]
fn headers_match_file_names_and_buckets() {
    for rel in TOP_LEVEL {
        let sess = Session::load(rel);
        let h = sess.header();
        assert_eq!(h["version"], 3, "{rel}");
        assert_eq!(h["id"], file_id(rel), "{rel}: header id = file-name uuid");
        let cwd = if rel.starts_with(ALPHA) {
            "/home/fixture/work/alpha"
        } else {
            "/home/fixture/work/beta"
        };
        assert_eq!(h["cwd"], cwd, "{rel}");
    }
    assert_eq!(Session::load(S).header()["id"], S_ID);
}

#[test]
fn ids_are_uuidv7_with_the_file_timestamp() {
    for rel in TOP_LEVEL {
        let id = file_id(rel);
        let hex = hex_id(id);
        assert_eq!(hex.len(), 32, "{id}");
        assert_eq!(&hex[12..13], "7", "{id}: version nibble");
        assert!("89ab".contains(&hex[16..17]), "{id}: variant");
        let ms = u64::from_str_radix(&hex[..12], 16).unwrap();
        assert_eq!(
            ms,
            file_timestamp_ms(rel),
            "{id}: leading 48 bits = file timestamp"
        );
    }
    let hex = hex_id(S_ID);
    assert_eq!(&hex[12..13], "7");
    assert!("89ab".contains(&hex[16..17]));
}

#[test]
fn only_e_has_malformed_lines() {
    for rel in TOP_LEVEL.iter().chain([&S]) {
        let sess = Session::load(rel);
        if *rel == E {
            assert_eq!(
                sess.malformed(),
                [
                    "not json at all",
                    r#"{"type":"message","id":"deadbeef","parentId":"#
                ],
                "E's two deliberate malformed lines"
            );
            assert!(
                !sess.raw.ends_with('\n'),
                "E ends mid-write, without a newline"
            );
        } else {
            assert_eq!(sess.malformed(), [] as [&str; 0], "{rel}");
            assert!(sess.raw.ends_with('\n'), "{rel} ends with a newline");
        }
    }
}

#[test]
fn entries_form_a_valid_tree() {
    for rel in TOP_LEVEL.iter().chain([&S]) {
        let sess = Session::load(rel);
        let mut seen = HashSet::new();
        let mut last_ts = String::new();
        for e in sess.entries() {
            let eid = id(e);
            assert!(
                eid.len() == 8
                    && eid
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{rel}: entry id {eid:?} is 8 lowercase hex"
            );
            match parent(e) {
                Value::Null => {}
                Value::String(p) => assert!(
                    seen.contains(p.as_str()),
                    "{rel}: {eid} parent {p} precedes it"
                ),
                other => panic!("{rel}: {eid} parentId {other}"),
            }
            assert!(seen.insert(eid.to_owned()), "{rel}: duplicate id {eid}");
            let ts = e["timestamp"]
                .as_str()
                .unwrap_or_else(|| panic!("{rel}: {eid} timestamp"));
            assert!(
                ts.ends_with('Z') && ts.contains('T'),
                "{rel}: {ts} is ISO-8601 UTC"
            );
            assert!(
                ts >= last_ts.as_str(),
                "{rel}: timestamps increase ({last_ts} then {ts})"
            );
            last_ts = ts.to_owned();
            if e["type"] != "future_kind" {
                assert_eq!(
                    e["runtime"]["version"], TESTED_OMP,
                    "{rel}: {eid} carries runtime"
                );
            }
        }
    }
}

#[test]
fn message_shapes_follow_omp() {
    let b_second_user = id(Session::load(B).entries()[2]).to_owned();
    for rel in TOP_LEVEL.iter().chain([&S]) {
        let sess = Session::load(rel);
        for e in sess
            .entries()
            .into_iter()
            .filter(|e| e["type"] == "message")
        {
            let m = &e["message"];
            match m["role"].as_str() {
                Some("user") => {
                    assert_eq!(m["attribution"], "user", "{rel}: {}", id(e));
                    assert!(m.get("timestamp").is_some(), "{rel}: {}", id(e));
                    if id(e) == b_second_user {
                        assert_eq!(m["content"], "And the second fixture?");
                    } else {
                        let blocks = m["content"].as_array().unwrap_or_else(|| {
                            panic!("{rel}: user {} uses the block-array form", id(e))
                        });
                        assert!(
                            blocks
                                .iter()
                                .all(|b| b["type"] == "text" && b["text"].is_string())
                        );
                    }
                }
                Some("assistant") => {
                    for k in [
                        "api",
                        "provider",
                        "model",
                        "content",
                        "usage",
                        "stopReason",
                        "timestamp",
                        "responseId",
                        "duration",
                        "ttft",
                    ] {
                        assert!(m.get(k).is_some(), "{rel}: assistant {} lacks {k}", id(e));
                    }
                    for k in ["input", "output", "cacheRead", "cacheWrite", "totalTokens"] {
                        assert!(m["usage"][k].is_number(), "{rel}: usage.{k}");
                    }
                    for k in ["input", "output", "cacheRead", "cacheWrite", "total"] {
                        assert!(m["usage"]["cost"][k].is_number(), "{rel}: usage.cost.{k}");
                    }
                }
                _ => {}
            }
        }
    }
}

#[test]
fn a_is_a_basic_transcript() {
    let a = Session::load(A);
    let e = a.entries();
    assert_eq!(
        kinds(&e),
        [
            "thinking_level_change",
            "model_change",
            "message/user",
            "custom",
            "message/assistant",
            "message/toolResult",
            "message/assistant",
            "model_usage",
            "title_change",
        ]
    );
    assert_eq!(parent(e[0]), &Value::Null);
    assert_eq!(e[0]["thinkingLevel"], "high");
    assert_eq!(e[0]["configured"], "high");
    assert_eq!(e[1]["model"], "anthropic/claude-sonnet-4-5");
    assert_eq!(e[1]["role"], "default");
    assert_eq!(
        e[2]["message"]["content"],
        json!([{"type": "text", "text": "Why does parser_test flake?"}])
    );
    assert_eq!(e[3]["customType"], "system-prompt-digest");
    let first = &e[4]["message"];
    let blocks = first["content"].as_array().unwrap();
    let types: Vec<&str> = blocks.iter().map(|b| b["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["thinking", "text", "toolCall"]);
    assert_eq!(blocks[0]["thinkingSignature"], "sig-placeholder");
    assert_eq!(blocks[2]["id"], "toolu_01");
    assert_eq!(blocks[2]["name"], "read");
    assert_eq!(blocks[2]["arguments"]["path"], "tests/parser_test.rs");
    assert_eq!(first["stopReason"], "toolUse");
    let result = &e[5]["message"];
    assert_eq!(result["toolCallId"], "toolu_01");
    assert_eq!(result["toolName"], "read");
    assert_eq!(result["isError"], false);
    assert!(result["content"].is_array());
    assert_eq!(e[6]["message"]["stopReason"], "stop");
    assert_eq!(e[7]["purpose"], "title");
    assert_eq!(e[8]["source"], "auto");
    assert!(e[8]["title"].is_string());
}

#[test]
fn b_and_c_are_a_continuation_pair() {
    let b = Session::load(B);
    let be = b.entries();
    assert_eq!(
        kinds(&be),
        ["message/user", "message/assistant", "message/user"]
    );
    assert_eq!(be[1]["message"]["stopReason"], "stop");
    assert_eq!(be[2]["message"]["content"], "And the second fixture?");

    let c = Session::load(C);
    let b_id = file_id(B);
    assert_eq!(c.header()["parentSession"], b_id);
    assert_eq!(c.header()["continuationOf"], b_id);
    let ce = c.entries();
    assert_eq!(ce.len(), 6, "B's three entries plus three new ones");
    for i in 0..3 {
        assert_eq!(ce[i], be[i], "C republishes B's entry {i} verbatim");
    }
    assert_eq!(
        kinds(&ce[3..]),
        ["message/assistant", "message/user", "message/assistant"]
    );
    assert_eq!(
        parent(ce[3]),
        id(be[2]),
        "C's first new reply follows B's last user entry"
    );
}

#[test]
fn d_is_branched_with_an_active_leaf() {
    let d = Session::load(D);
    let e = d.entries();
    assert_eq!(
        kinds(&e),
        [
            "message/user",      // u1
            "message/assistant", // a1
            "message/user",      // u2
            "message/assistant", // a2
            "branch_summary",    // bs1
            "message/user",      // u3
            "message/assistant", // a3
            "label",             // lb1
            "model_usage",       // mu1
        ]
    );
    let (u1, a1, u2, a2, bs1, u3, a3, lb1, mu1) =
        (e[0], e[1], e[2], e[3], e[4], e[5], e[6], e[7], e[8]);
    assert_eq!(parent(u1), &Value::Null);
    assert_eq!(parent(a1), id(u1));
    assert_eq!(parent(u2), id(a1));
    assert_eq!(parent(a2), id(u2));
    assert_eq!(parent(bs1), id(a1));
    assert_eq!(bs1["fromId"], id(a2));
    assert_eq!(bs1["summary"], "Abandoned the regex approach.");
    assert_eq!(parent(u3), id(bs1));
    assert_eq!(parent(a3), id(u3));
    assert_eq!(parent(lb1), id(a3));
    assert_eq!(lb1["targetId"], id(a1));
    assert_eq!(lb1["label"], "checkpoint");
    assert_eq!(parent(mu1), id(a2));
    assert_eq!(mu1["activeLeafId"], id(lb1));
    assert_ne!(id(mu1), id(lb1));

    // Walk parentId from the active leaf.
    let by_id: HashMap<&str, &Value> = e.iter().map(|x| (id(x), *x)).collect();
    let mut path = Vec::new();
    let mut cur = mu1["activeLeafId"].as_str();
    while let Some(i) = cur {
        let node = by_id[i];
        path.push(kind(node));
        cur = parent(node).as_str();
    }
    path.reverse();
    assert_eq!(
        path,
        [
            "message/user",
            "message/assistant",
            "branch_summary",
            "message/user",
            "message/assistant",
            "label"
        ]
    );
}

#[test]
fn e_has_compaction_unknowns_and_a_torn_tail() {
    let sess = Session::load(E);
    let e = sess.entries();
    assert_eq!(
        kinds(&e),
        [
            "message/user",          // u1
            "message/assistant",     // a1
            "message/user",          // u2
            "message/assistant",     // a2
            "compaction",            // c1
            "message/user",          // u3
            "message/assistant",     // a3
            "future_kind",           // x1
            "message/user",          // u4
            "message/bashExecution", // bash
            "message/assistant",     // a4
        ]
    );
    let (u2, a2, c1, u3, a3, x1, u4, bash, a4) =
        (e[2], e[3], e[4], e[5], e[6], e[7], e[8], e[9], e[10]);
    assert_eq!(parent(c1), id(a2));
    assert!(c1["summary"].is_string() && c1["shortSummary"].is_string());
    assert_eq!(c1["firstKeptEntryId"], id(u2));
    assert_eq!(c1["tokensBefore"], 42000);
    assert_eq!(c1["tokensAfter"], 9000);
    assert_eq!(parent(u3), id(c1));
    assert_eq!(parent(a3), id(u3));
    assert_eq!(
        parent(x1),
        id(a3),
        "the unknown entry sits on the active path"
    );
    assert_eq!(x1["payload"], json!({"x": 1}));
    assert_eq!(parent(u4), id(x1));
    assert_eq!(u4["futureField"], json!({"nested": true}));
    assert_eq!(u4["message"]["newMessageField"], 1);
    let b = &bash["message"];
    assert_eq!(b["command"], "cargo test");
    assert_eq!(b["output"], "ok");
    assert_eq!(b["exitCode"], 0);
    assert_eq!(b["cancelled"], false);
    assert_eq!(b["truncated"], false);
    assert!(b.get("timestamp").is_some());
    let blocks = a4["message"]["content"].as_array().unwrap();
    assert!(blocks.contains(&json!({"type": "future_block", "data": "x"})));
    assert!(blocks.iter().any(|b| b["type"] == "text"));

    // Physical order: the `not json at all` line sits between a3 and x1.
    let order: Vec<&str> = sess.lines.iter().map(|(t, _)| t.as_str()).collect();
    let bad = order.iter().position(|l| *l == "not json at all").unwrap();
    let x1_line = order
        .iter()
        .position(|l| l.contains("\"future_kind\""))
        .unwrap();
    assert_eq!(bad + 1, x1_line);
}

#[test]
fn g_is_a_fork_not_a_continuation() {
    let g = Session::load(G);
    let h = g.header();
    assert_eq!(h["parentSession"], file_id(A));
    assert!(
        h.get("continuationOf").is_none(),
        "a fork has no continuationOf key"
    );
    let ge = g.entries();
    assert_eq!(kinds(&ge), ["message/user", "message/assistant"]);
    assert_eq!(parent(ge[0]), &Value::Null);
    assert_eq!(parent(ge[1]), id(ge[0]));
    let a = Session::load(A);
    let a_ids: HashSet<&str> = a.entries().into_iter().map(id).collect();
    assert!(
        ge.iter().all(|e| !a_ids.contains(id(e))),
        "G uses new entry ids"
    );
}

#[test]
fn s_is_a_minimal_subagent_transcript() {
    let sess = Session::load(S);
    assert_eq!(kinds(&sess.entries()), ["message/user"]);
}

#[test]
fn handles_are_unique_while_short_suffixes_collide() {
    let tail = |rel: &str, n: usize| {
        let h = hex_id(file_id(rel));
        h[h.len() - n..].to_owned()
    };
    assert_eq!(tail(A, 4), tail(F, 4), "A and F share the last 4 hex");
    assert_eq!(tail(A, 4), "c0de");
    let handles: HashSet<String> = TOP_LEVEL.iter().map(|r| tail(r, 7)).collect();
    assert_eq!(handles.len(), TOP_LEVEL.len(), "7-hex handles are unique");
    assert_eq!(tail(A, 7), "e27c0de");
    assert_eq!(
        &hex_id(file_id(A))[..7],
        &hex_id(S_ID)[..7],
        "A and S share a prefix"
    );
}

#[test]
fn fixtures_avoid_secret_shaped_fields() {
    for rel in TOP_LEVEL.iter().chain([&S]) {
        let raw = fs::read_to_string(sessions().join(rel)).unwrap();
        for bad in [
            "credential_pin",
            "providerPromptCacheKey",
            "taskPromptCacheKey",
        ] {
            assert!(!raw.contains(bad), "{rel} contains {bad}");
        }
    }
}

#[test]
fn fake_omp_reports_a_version_and_rejects_everything_else() {
    let bin = fake_omp();
    let m = fs::metadata(&bin).unwrap().permissions().mode();
    assert_ne!(m & 0o111, 0, "fake-omp is executable");

    let run = |args: &[&str], env: &[(&str, &str)]| {
        let mut cmd = Command::new(&bin);
        cmd.env_clear().args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    };
    let out = run(&["--version"], &[]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "omp/18.14.0");
    let out = run(&["--version"], &[("FAKE_OMP_VERSION", "1.2.3")]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "omp/1.2.3");
    let out = run(&["--mode", "rpc"], &[]);
    assert_eq!(out.status.code(), Some(64));
    assert!(out.stdout.is_empty() && !out.stderr.is_empty());
}
