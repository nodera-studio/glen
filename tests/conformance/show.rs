//! `glen show` and session resolution (AC-106, AC-110–AC-115, AC-120, AC-121; docs/design.md
//! §A3 parsing, leaf, active branch, rendering and continuations, §A8 resolution, §A10 items).

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::reader::*;
use crate::support::*;

fn show(bx: &OmpBox, args: &[&str]) -> Value {
    let mut full = vec!["show"];
    full.extend_from_slice(args);
    full.push("--json");
    bx.data(&full)
}

fn show_path(bx: &OmpBox, path: &Path, extra: &[&str]) -> Value {
    let p = s(path);
    let mut args = vec![p.as_str()];
    args.extend_from_slice(extra);
    show(bx, &args)
}

fn item<'a>(d: &'a Value, id: &str) -> &'a Value {
    arr(&d["items"])
        .iter()
        .find(|i| i["id"] == id)
        .unwrap_or_else(|| panic!("no item {id}: {d}"))
}

#[test]
fn ac110_handles_ids_suffixes_and_prefixes_resolve() {
    let bx = OmpBox::new();
    let upper_id = A.id.to_uppercase();
    let cases: &[(&str, &Fx)] = &[
        ("e27c0de", &A),
        ("E27C0DE", &A),
        (A.id, &A),
        (upper_id.as_str(), &A),
        ("01a0f6b1", &A),
        ("2026-10-01T09", &A),
        ("a81c0de", &F),
        ("9b3d5a81c0de", &F),
        ("4a07", &G),
        ("01a106bb-4a60", &E),
        ("2026-10-04T11-45-00-000Z_01a106bb", &E),
        ("c4f9a72", &D),
    ];
    for (arg, fx) in cases {
        let d = show(&bx, &[*arg]);
        assert_eq!(d["session"]["id"], fx.id, "show {arg}: {d}");
        assert_eq!(d["continued_from"], json!([]), "show {arg}: {d}");
    }
    bx.assert_no_state();
}

#[test]
fn ac110_ambiguous_arguments_exit_4_with_the_candidates() {
    let bx = OmpBox::new();
    for cmd in ["show", "tail"] {
        let out = bx.glen([cmd, "c0de", "--json"]).run();
        let err = out.error_envelope("ambiguous");
        out.assert_stderr_empty();
        let c = candidates(&err, 2);
        assert!(c.contains(A.id) && c.contains(F.id), "{}", out.ctx());
    }
    // A file-name prefix shared by six October sessions.
    let out = bx.glen(["show", "2026-10-0", "--json"]).run();
    candidates(&out.error_envelope("ambiguous"), 6);

    // Human mode names the candidates.
    let out = bx
        .glen(["show", "c0de", "--human"])
        .env("NO_COLOR", "1")
        .run();
    out.assert_exit(4);
    let all = format!("{}{}", out.out(), out.err());
    assert!(
        all.contains(A.handle) && all.contains(F.handle),
        "human mode lists the candidates\n{}",
        out.ctx()
    );
}

#[test]
fn ac110_unmatched_arguments_are_not_found() {
    let bx = OmpBox::new();
    // `01a0f6b` is 7 hex: not a suffix of any id and too short to be a prefix.
    for arg in [
        "01a0f6b",
        "0de",
        "nope",
        "ffffffff",
        "01a0f6b5",
        "3f7a9c1e5b20",
    ] {
        for cmd in ["show", "tail"] {
            let out = bx.glen([cmd, arg, "--json"]).run();
            out.error_envelope("not_found");
            out.assert_stderr_empty();
        }
    }
}

#[test]
fn ac110_at_latest_is_the_newest_session_in_the_cwd_bucket() {
    let bx = OmpBox::new();
    let d = bx.data_in(&bx.alpha, ["show", "@latest", "--json"]);
    assert_eq!(d["session"]["id"], G.id, "{d}");
    let d = bx.data_in(&bx.beta, ["tail", "@latest", "--json"]);
    assert_eq!(d["session"]["id"], E.id, "{d}");
    let gamma = bx.work("gamma");
    bx.glen_in(&gamma, ["show", "@latest", "--json"])
        .run()
        .error_envelope("not_found");
    // Touching A makes it the newest in alpha.
    set_mtime(&bx.path(&A), std::time::SystemTime::now());
    let d = bx.data_in(&bx.alpha, ["show", "@latest", "--json"]);
    assert_eq!(d["session"]["id"], A.id, "{d}");
}

#[test]
fn ac110_job_handles_and_other_at_forms_are_usage_errors() {
    let bx = OmpBox::new();
    for cmd in ["show", "tail"] {
        let out = bx.glen([cmd, "j1234567", "--json"]).run();
        let err = out.error_envelope("usage");
        let hint = err["hint"].as_str().unwrap_or("");
        assert!(
            hint.contains("glen wait"),
            "hint names `glen wait`\n{}",
            out.ctx()
        );
        for arg in ["@new", "@nope"] {
            bx.glen([cmd, arg, "--json"]).run().error_envelope("usage");
        }
    }
}

#[test]
fn ac110_path_arguments_open_that_file() {
    let bx = OmpBox::new();
    let e = bx.path(&E);
    let d = show_path(&bx, &e, &[]);
    assert_eq!(d["session"]["id"], E.id, "{d}");
    assert_path(&d["session"]["path"], &e);

    // Relative, containing `/`.
    let rel = format!("../../../agent/sessions/{}", E.rel);
    let d = bx.data_in(&bx.beta, ["show", &rel, "--json"]);
    assert_eq!(d["session"]["id"], E.id, "{d}");
    // A bare file name ending `.jsonl`, relative to the working directory.
    let name = Path::new(E.rel).file_name().unwrap().to_str().unwrap();
    let d = bx.data_in(&bx.sessions.join(BETA), ["show", name, "--json"]);
    assert_eq!(d["session"]["id"], E.id, "{d}");

    // A copy outside sessions_dir is that file, not the catalog copy with the same id.
    let outside = bx.root.join("elsewhere");
    fs::create_dir(&outside).unwrap();
    let g_copy = outside.join("g-copy.jsonl");
    fs::copy(bx.path(&G), &g_copy).unwrap();
    let d = show_path(&bx, &g_copy, &[]);
    assert_eq!(d["session"]["id"], G.id, "{d}");
    assert_path(&d["session"]["path"], &g_copy);
    assert_eq!(d["session"]["handle"], G.handle, "{d}");

    // Paths still follow continuations (§A8 step 4).
    let d = show_path(&bx, &bx.path(&B), &[]);
    assert_eq!(d["session"]["id"], C.id, "{d}");
    assert_eq!(d["continued_from"], json!([B.id]), "{d}");

    for bad in [
        s(&fixtures().join("README.md")),
        "./nope.jsonl".to_owned(),
        s(&bx.root.join("missing/x.jsonl")),
    ] {
        bx.glen(["show", &bad, "--json"])
            .run()
            .error_envelope("not_found");
    }
}

#[test]
fn a8_two_files_with_one_session_id_are_ambiguous_even_by_full_id() {
    let bx = OmpBox::new();
    let copy = bx.sessions.join(BETA).join("copy-of-a.jsonl");
    fs::copy(bx.path(&A), &copy).unwrap();
    for arg in [A.id, A.handle] {
        let out = bx.glen(["show", arg, "--json"]).run();
        let c = candidates(&out.error_envelope("ambiguous"), 2);
        assert!(c.contains(A.id), "{}", out.ctx());
    }
    let d = show_path(&bx, &copy, &[]);
    assert_eq!(d["session"]["id"], A.id, "{d}");
}

#[test]
fn ac111_continuations_are_followed_and_forks_are_not() {
    let bx = OmpBox::new();
    let d = show(&bx, &[B.handle]);
    assert_eq!(d["session"]["id"], C.id, "{d}");
    assert_path(&d["session"]["path"], &bx.path(&C));
    assert_eq!(d["continued_from"], json!([B.id]), "{d}");
    assert_eq!(item_ids(&d), strs(&C_ITEMS), "{d}");
    assert_eq!(item(&d, "30000006")["text"], "Review complete.", "{d}");

    for (arg, fx) in [(C.handle, &C), (A.handle, &A), (G.handle, &G)] {
        let d = show(&bx, &[arg]);
        assert_eq!(d["session"]["id"], fx.id, "show {arg}: {d}");
        assert_eq!(d["continued_from"], json!([]), "show {arg}: {d}");
    }
    let d = bx.data(["tail", B.handle, "-n", "1", "--json"]);
    assert_eq!(d["session"]["id"], C.id, "{d}");
    assert_eq!(d["continued_from"], json!([B.id]), "{d}");
    assert_eq!(item_ids(&d), ["30000006"], "{d}");
}

#[test]
fn ac111_continuations_are_followed_across_several_hops() {
    let bx = OmpBox::new();
    let c2 = uuid(0x2c2);
    let created = "2026-10-02T16:00:00.000Z";
    let path = bx.sessions.join(ALPHA).join(session_name(created, &c2));
    let mut h = header(&c2, Some(created), Some(C.cwd));
    h["parentSession"] = Value::from(C.id);
    h["continuationOf"] = Value::from(C.id);
    let mut text = format!("{h}\n");
    for l in entry_lines(&bx.path(&C)) {
        text.push_str(&l);
        text.push('\n');
    }
    text.push_str(&jsonl(&[
        user(
            "c2000001",
            Some("30000006"),
            "2026-10-02T16:00:05.000Z",
            "One more pass.",
        ),
        reply(
            "c2000002",
            Some("c2000001"),
            "2026-10-02T16:00:06.000Z",
            "Done again.",
        ),
    ]));
    fs::write(&path, text).unwrap();

    let d = show(&bx, &[B.handle]);
    assert_eq!(d["session"]["id"], c2, "{d}");
    assert_eq!(d["continued_from"], json!([B.id, C.id]), "{d}");
    assert_eq!(item_ids(&d).last().unwrap(), "c2000002", "{d}");
    let d = show(&bx, &[C.handle]);
    assert_eq!(d["session"]["id"], c2, "{d}");
    assert_eq!(d["continued_from"], json!([C.id]), "{d}");
}

#[test]
fn ac112_show_renders_the_active_branch_root_first() {
    let bx = OmpBox::new();
    let d = show(&bx, &[D.handle]);
    assert_eq!(item_ids(&d), strs(&D_ITEMS), "{d}");
    assert_eq!(
        kinds(&d),
        ["user", "assistant", "branch_summary", "user", "assistant"],
        "{d}"
    );
    assert_eq!(d["leaf_id"], "40000008", "{d}");
    let text = d.to_string();
    for abandoned in ["Try the regex.", "Regex failed.", "checkpoint"] {
        assert!(
            !text.contains(abandoned),
            "{abandoned:?} is off the branch: {d}"
        );
    }

    let raw = show(&bx, &[D.handle, "--raw"]);
    assert!(raw.get("items").is_none(), "--raw has no items: {raw}");
    let expected: Vec<Value> = [
        "40000001", "40000002", "40000005", "40000006", "40000007", "40000008",
    ]
    .iter()
    .map(|id| raw_entry(&bx.path(&D), id))
    .collect();
    assert_eq!(arr(&raw["entries"]), &expected, "{raw}");

    let d = bx.data(["tail", D.handle, "-n", "2", "--json"]);
    assert_eq!(item_ids(&d), ["40000006", "40000007"], "{d}");
}

#[test]
fn ac112_leaf_resolution_edge_cases() {
    let bx = OmpBox::new();
    let gamma = bx.bucket("-work-gamma");
    let a_text = fs::read_to_string(bx.path(&A)).unwrap();
    let t = "2026-10-01T09:10:00.000Z";
    let with_tail = |name: &str, extra: Value| {
        let p = gamma.join(name);
        fs::write(&p, format!("{a_text}{extra}\n")).unwrap();
        p
    };
    let label = |leaf: Value| {
        entry(
            "label",
            "10000010",
            Some("10000009"),
            t,
            json!({"targetId": "10000003", "label": "x", "activeLeafId": leaf}),
        )
    };

    // `activeLeafId: null` → an empty branch.
    let p = with_tail("null-leaf.jsonl", label(Value::Null));
    let d = show_path(&bx, &p, &[]);
    assert_eq!(d["leaf_id"], Value::Null, "{d}");
    assert_eq!(d["items"], json!([]), "{d}");

    // A captured leaf that no longer exists → the newest entry that selects itself.
    let p = with_tail("missing-leaf.jsonl", label(Value::from("ffffffff")));
    let d = show_path(&bx, &p, &[]);
    assert_eq!(d["leaf_id"], "10000009", "{d}");
    assert_eq!(item_ids(&d), strs(&A_ITEMS), "{d}");

    // A captured leaf earlier in the file.
    let p = with_tail("early-leaf.jsonl", label(Value::from("10000003")));
    let d = show_path(&bx, &p, &[]);
    assert_eq!(d["leaf_id"], "10000003", "{d}");
    assert_eq!(item_ids(&d), ["10000001", "10000002", "10000003"], "{d}");

    // A parent cycle terminates.
    let id = uuid(0x3c1);
    let ts = "2026-10-06T10:00:01.000Z";
    let p = bx.add(
        "-work-gamma",
        "2026-10-06T10:00:00.000Z",
        &id,
        None,
        &[
            user("aaaa0001", Some("aaaa0002"), ts, "one"),
            reply("aaaa0002", Some("aaaa0001"), ts, "two"),
        ],
    );
    let d = show_path(&bx, &p, &[]);
    let got = item_ids(&d);
    assert!(
        !got.is_empty() && got.len() <= 2 && got.iter().all(|i| i.starts_with("aaaa000")),
        "{d}"
    );

    // A header-only session.
    let id = uuid(0x3c2);
    let p = bx.add("-work-gamma", "2026-10-06T11:00:00.000Z", &id, None, &[]);
    let d = show_path(&bx, &p, &[]);
    assert_eq!(d["items"], json!([]), "{d}");
    assert_eq!(d["leaf_id"], Value::Null, "{d}");
    assert_eq!(d["skipped_lines"], 0, "{d}");
}

#[test]
fn ac113_malformed_lines_are_counted_and_unknown_types_keep_the_chain() {
    let bx = OmpBox::new();
    let d = show(&bx, &[E.handle]);
    assert_eq!(d["skipped_lines"], 2, "{d}");
    assert_eq!(item_ids(&d), strs(&E_ITEMS), "{d}");
    assert_eq!(
        kinds(&d),
        [
            "user",
            "assistant",
            "user",
            "assistant",
            "compaction",
            "user",
            "assistant",
            "user",
            "bash",
            "assistant"
        ],
        "{d}"
    );
    assert_eq!(item(&d, "50000009")["text"], "Unknown fields.", "{d}");

    let raw = show(&bx, &[E.handle, "--raw"]);
    assert_eq!(raw["skipped_lines"], 2, "{raw}");
    let raw_ids: Vec<&str> = arr(&raw["entries"])
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        raw_ids,
        [
            "50000001", "50000002", "50000003", "50000004", "50000005", "50000006", "50000007",
            "50000008", "50000009", "5000000a", "5000000b"
        ],
        "{raw}"
    );
    let future = &arr(&raw["entries"])[7];
    assert_eq!(future, &raw_entry(&bx.path(&E), "50000008"), "{raw}");

    let d = bx.data(["tail", E.handle, "-n", "3", "--json"]);
    assert_eq!(item_ids(&d), ["50000009", "5000000a", "5000000b"], "{d}");
    assert_eq!(d["skipped_lines"], 2, "{d}");
}

#[test]
fn ac113_an_invalid_utf8_line_costs_one_skipped_line() {
    let bx = OmpBox::new();
    let src = fs::read(bx.path(&A)).unwrap();
    let mut out = Vec::new();
    for line in src.split_inclusive(|b| *b == b'\n') {
        out.extend_from_slice(line);
        if line.windows(10).any(|w| w == b"\"10000003\"")
            && line.starts_with(b"{\"type\":\"message\"")
        {
            out.extend_from_slice(
                b"{\"type\":\"custom\",\"id\":\"\xff\xfe\",\"parentId\":\"10000003\"}\n",
            );
        }
    }
    let p = bx.bucket("-work-gamma").join("bad-utf8.jsonl");
    fs::write(&p, out).unwrap();
    let d = show_path(&bx, &p, &[]);
    assert_eq!(d["skipped_lines"], 1, "{d}");
    assert_eq!(item_ids(&d), strs(&A_ITEMS), "{d}");
}

#[test]
fn ac114_default_show_hides_thinking_and_collapses_tool_calls() {
    let bx = OmpBox::new();
    let out = bx.glen(["show", A.handle, "--json"]).run();
    let d = out.ok_data();
    assert!(
        !out.out().contains("Check the fixture loader."),
        "thinking text is hidden by default\n{}",
        out.ctx()
    );
    assert_eq!(item_ids(&d), strs(&A_ITEMS), "{d}");
    assert_eq!(
        kinds(&d),
        [
            "thinking_level",
            "model_change",
            "user",
            "assistant",
            "assistant"
        ],
        "{d}"
    );
    assert_eq!(d["items"][0]["level"], "high", "{d}");
    assert_eq!(d["items"][1]["model"], MODEL, "{d}");
    let u = &d["items"][2];
    assert_eq!(u["text"], "Why does parser_test flake?", "{u}");
    assert_eq!(u["images"], 0, "{u}");

    let a = &d["items"][3];
    assert_eq!(a["text"], "Let me read the test.", "{a}");
    assert_eq!(a["model"], MODEL, "{a}");
    assert_eq!(a["stop_reason"], "toolUse", "{a}");
    assert_eq!(a["error"], Value::Null, "{a}");
    assert_eq!(a["has_thinking"], true, "{a}");
    assert_eq!(a["thinking_words"], 4, "{a}");
    assert!(a.get("thinking").is_none_or(Value::is_null), "{a}");
    let calls = arr(&a["tool_calls"]);
    assert_eq!(calls.len(), 1, "{a}");
    let call = &calls[0];
    assert_eq!(call["id"], "toolu_01", "{call}");
    assert_eq!(call["name"], "read", "{call}");
    assert!(
        call["summary"]
            .as_str()
            .unwrap_or("")
            .contains("tests/parser_test.rs"),
        "{call}"
    );
    assert!(
        call.get("arguments").is_none(),
        "collapsed calls carry no arguments: {call}"
    );
    let result_text = "fn parser_test() { … }";
    assert_eq!(
        call["result"],
        json!({
            "is_error": false, "text": result_text, "chars": result_text.chars().count(),
            "truncated": false, "images": 0
        }),
        "{call}"
    );
    assert!(
        !kinds(&d).contains(&"tool_result".to_owned()),
        "the result is folded: {d}"
    );

    let last = &d["items"][4];
    assert_eq!(
        last["text"], "The test depends on map order. Fixed.",
        "{last}"
    );
    assert_eq!(last["stop_reason"], "stop", "{last}");
    assert_eq!(last["has_thinking"], false, "{last}");
}

#[test]
fn ac114_thinking_and_tools_flags_change_the_items() {
    let bx = OmpBox::new();
    let d = show(&bx, &[A.handle, "--thinking"]);
    assert_eq!(
        d["items"][3]["thinking"], "Check the fixture loader.",
        "{d}"
    );

    let d = show(&bx, &[A.handle, "--tools", "full"]);
    let call = &d["items"][3]["tool_calls"][0];
    assert_eq!(
        call["arguments"],
        json!({"path": "tests/parser_test.rs"}),
        "{call}"
    );
    assert_eq!(call["result"]["text"], "fn parser_test() { … }", "{call}");

    for (fx, n) in [(&A, 5), (&E, 9)] {
        let d = show(&bx, &[fx.handle, "--tools", "none"]);
        assert_eq!(arr(&d["items"]).len(), n, "{d}");
        for it in arr(&d["items"]) {
            assert!(
                !["tool_result", "bash", "python"].contains(&it["kind"].as_str().unwrap()),
                "{it}"
            );
            assert!(
                it.get("tool_calls")
                    .is_none_or(|c| c.as_array().is_some_and(Vec::is_empty)),
                "{it}"
            );
        }
    }

    // `tail` keeps tool calls collapsed and accepts --thinking.
    let d = bx.data(["tail", A.handle, "-n", "2", "--thinking", "--json"]);
    assert_eq!(
        d["items"][0]["thinking"], "Check the fixture loader.",
        "{d}"
    );
    assert!(
        d["items"][0]["tool_calls"][0].get("arguments").is_none(),
        "{d}"
    );
}

#[test]
fn ac114_collapsed_results_and_outputs_are_cut_to_500_characters() {
    let bx = OmpBox::new();
    let big: String = "aé".repeat(300);
    let long_cmd = format!("echo {}", "x".repeat(300));
    let t = "2026-10-06T10:00:01.000Z";
    let id = uuid(0x500);
    let p = bx.add(
        "-work-gamma",
        "2026-10-06T10:00:00.000Z",
        &id,
        None,
        &[
            user("b0000001", None, t, "go"),
            assistant(
                "b0000002",
                Some("b0000001"),
                t,
                json!([tool_call("t1", "bash", json!({"command": long_cmd}))]),
                "toolUse",
            ),
            tool_result("b0000003", Some("b0000002"), t, "t1", "bash", &big, false),
            tool_result(
                "b0000004",
                Some("b0000003"),
                t,
                "t-none",
                "grep",
                &big,
                true,
            ),
            message(
                "b0000005",
                Some("b0000004"),
                t,
                json!({"role": "bashExecution", "command": "make", "output": big, "exitCode": 0,
                       "cancelled": false, "truncated": false, "timestamp": 0}),
            ),
        ],
    );
    let d = show_path(&bx, &p, &[]);
    let call = &item(&d, "b0000002")["tool_calls"][0];
    let summary = call["summary"].as_str().unwrap();
    assert!(
        summary.chars().count() <= 120,
        "§A10 summary limit: {summary:?}"
    );
    let r = &call["result"];
    let text = r["text"].as_str().unwrap();
    assert_eq!(text.chars().count(), 500, "{r}");
    assert!(big.starts_with(text), "{r}");
    assert_eq!(r["truncated"], true, "{r}");
    assert_eq!(r["chars"], 600, "{r}");

    let standalone = item(&d, "b0000004");
    assert_eq!(standalone["kind"], "tool_result", "{standalone}");
    assert_eq!(standalone["tool_call_id"], "t-none", "{standalone}");
    assert_eq!(standalone["tool_name"], "grep", "{standalone}");
    assert_eq!(standalone["is_error"], true, "{standalone}");
    assert_eq!(
        standalone["text"].as_str().unwrap().chars().count(),
        500,
        "{standalone}"
    );
    assert_eq!(standalone["truncated"], true, "{standalone}");
    assert_eq!(standalone["chars"], 600, "{standalone}");

    let bash = item(&d, "b0000005");
    assert_eq!(
        bash["output"].as_str().unwrap().chars().count(),
        500,
        "{bash}"
    );
    assert_eq!(bash["truncated"], true, "{bash}");
    assert_eq!(bash["chars"], 600, "{bash}");

    let d = show_path(&bx, &p, &["--tools", "full"]);
    let r = &item(&d, "b0000002")["tool_calls"][0]["result"];
    assert_eq!(
        r["text"],
        big.as_str(),
        "--tools full keeps the whole result: {r}"
    );
    assert_eq!(r["truncated"], false, "{r}");
    assert_eq!(item(&d, "b0000005")["output"], big.as_str(), "{d}");
}

#[test]
fn ac114_meta_dividers_and_unknown_blocks() {
    let bx = OmpBox::new();
    let d = show(&bx, &[E.handle]);
    let c = item(&d, "50000005");
    assert_eq!(c["kind"], "compaction", "{c}");
    assert_eq!(c["tokens_before"], 42000, "{c}");
    assert_eq!(c["tokens_after"], 9000, "{c}");
    assert!(c["summary"].is_string(), "{c}");
    let bash = item(&d, "5000000a");
    assert_eq!(bash["command"], "cargo test", "{bash}");
    assert_eq!(bash["exit_code"], 0, "{bash}");
    assert_eq!(bash["cancelled"], false, "{bash}");
    assert_eq!(bash["output"], "ok", "{bash}");
    let last = item(&d, "5000000b")["text"].as_str().unwrap();
    assert!(
        last.contains("[future_block]") && last.contains("Complete."),
        "{last:?}"
    );

    let d = show(&bx, &[D.handle]);
    assert_eq!(
        item(&d, "40000005")["summary"],
        "Abandoned the regex approach.",
        "{d}"
    );

    let out = bx
        .glen(["show", E.handle, "--human"])
        .env("NO_COLOR", "1")
        .run();
    out.assert_exit(0);
    let text = out.out();
    assert!(text.contains("[future_block]"), "{}", out.ctx());
    assert!(text.contains("cargo test"), "{}", out.ctx());
    assert!(text.contains("2 malformed lines skipped"), "{}", out.ctx());
}

/// One session with every documented item kind (§A10 transcript items table).
#[test]
fn ac114_every_item_kind_carries_its_documented_fields() {
    let bx = OmpBox::new();
    let t = |n: u32| format!("2026-10-06T10:00:{n:02}.000Z");
    let id = uuid(0x51);
    let img = json!({"type": "image", "data": "AAAA", "mimeType": "image/png"});
    let lines = vec![
        entry(
            "thinking_level_change",
            "k0000001",
            None,
            &t(1),
            json!({"thinkingLevel": "low"}),
        ),
        entry(
            "model_change",
            "k0000002",
            Some("k0000001"),
            &t(2),
            json!({"model": "openai/gpt-5"}),
        ),
        message(
            "k0000003",
            Some("k0000002"),
            &t(3),
            json!({"role": "user", "content": [{"type": "text", "text": "look"}, img], "timestamp": 0}),
        ),
        message(
            "k0000004",
            Some("k0000003"),
            &t(4),
            json!({"role": "developer", "content": "dev note", "timestamp": 0}),
        ),
        message(
            "k0000005",
            Some("k0000004"),
            &t(5),
            json!({"role": "custom", "customType": "reminder", "content": "remember", "display": true, "timestamp": 0}),
        ),
        message(
            "k0000006",
            Some("k0000005"),
            &t(6),
            json!({"role": "custom", "customType": "secret", "content": "hidden custom", "display": false, "timestamp": 0}),
        ),
        entry(
            "custom_message",
            "k0000007",
            Some("k0000006"),
            &t(7),
            json!({"customType": "ext", "content": "shown ext", "display": true}),
        ),
        entry(
            "custom_message",
            "k0000008",
            Some("k0000007"),
            &t(8),
            json!({"customType": "ext", "content": "hidden ext", "display": false}),
        ),
        message(
            "k0000009",
            Some("k0000008"),
            &t(9),
            json!({"role": "fileMention", "files": [{"path": "src/a.rs", "content": "x", "lineCount": 3}], "timestamp": 0}),
        ),
        assistant(
            "k0000010",
            Some("k0000009"),
            &t(10),
            json!([
                {"type": "redactedThinking", "data": "opaque"},
                {"type": "text", "text": "answer"},
                {"type": "unknown_block", "x": 1},
                tool_call("t1", "bash", json!({"command": "ls -la"}))
            ]),
            "toolUse",
        ),
        tool_result(
            "k0000011",
            Some("k0000010"),
            &t(11),
            "t1",
            "bash",
            "files",
            false,
        ),
        tool_result(
            "k0000012",
            Some("k0000011"),
            &t(12),
            "t-missing",
            "grep",
            "orphan",
            true,
        ),
        message(
            "k0000013",
            Some("k0000012"),
            &t(13),
            json!({"role": "bashExecution", "command": "make", "output": "built", "exitCode": 2, "cancelled": false, "truncated": false, "timestamp": 0}),
        ),
        message(
            "k0000014",
            Some("k0000013"),
            &t(14),
            json!({"role": "pythonExecution", "code": "print(1)", "output": "1", "exitCode": 0, "cancelled": false, "truncated": false, "timestamp": 0}),
        ),
        entry(
            "compaction",
            "k0000015",
            Some("k0000014"),
            &t(15),
            json!({"summary": "Long summary.", "shortSummary": "Short.", "firstKeptEntryId": "k0000003", "tokensBefore": 100, "tokensAfter": 10}),
        ),
        entry(
            "reset_boundary",
            "k0000016",
            Some("k0000015"),
            &t(16),
            json!({}),
        ),
        entry(
            "branch_summary",
            "k0000017",
            Some("k0000016"),
            &t(17),
            json!({"fromId": "k0000003", "summary": "Went back."}),
        ),
        entry(
            "label",
            "k0000018",
            Some("k0000017"),
            &t(18),
            json!({"targetId": "k0000003", "label": "mark"}),
        ),
        entry(
            "custom",
            "k0000019",
            Some("k0000018"),
            &t(19),
            json!({"customType": "state", "data": {}}),
        ),
        entry(
            "title_change",
            "k0000020",
            Some("k0000019"),
            &t(20),
            json!({"title": "T", "source": "auto"}),
        ),
        entry(
            "brand_new_kind",
            "k0000021",
            Some("k0000020"),
            &t(21),
            json!({"payload": 1}),
        ),
        message(
            "k0000022",
            Some("k0000021"),
            &t(22),
            json!({"role": "hookMessage", "customType": "hook", "content": "hooked", "display": true, "timestamp": 0}),
        ),
        reply("k0000023", Some("k0000022"), &t(23), "bye"),
    ];
    let p = bx.add("-work-gamma", &t(0), &id, None, &lines);
    let d = show_path(&bx, &p, &[]);
    let expected = [
        ("k0000001", "thinking_level"),
        ("k0000002", "model_change"),
        ("k0000003", "user"),
        ("k0000004", "developer"),
        ("k0000005", "custom"),
        ("k0000007", "custom"),
        ("k0000009", "file_mention"),
        ("k0000010", "assistant"),
        ("k0000012", "tool_result"),
        ("k0000013", "bash"),
        ("k0000014", "python"),
        ("k0000015", "compaction"),
        ("k0000016", "reset"),
        ("k0000017", "branch_summary"),
        ("k0000022", "custom"),
        ("k0000023", "assistant"),
    ];
    let got: Vec<(String, String)> = arr(&d["items"])
        .iter()
        .map(|i| {
            (
                i["id"].as_str().unwrap().to_owned(),
                i["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let want_pairs: Vec<(String, String)> = expected
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect();
    assert_eq!(got, want_pairs, "{d}");

    let fields: &[(&str, &[&str])] = &[
        ("user", &["text", "images"]),
        (
            "assistant",
            &[
                "text",
                "model",
                "stop_reason",
                "error",
                "has_thinking",
                "thinking_words",
            ],
        ),
        (
            "tool_result",
            &[
                "tool_call_id",
                "tool_name",
                "is_error",
                "text",
                "chars",
                "truncated",
                "images",
            ],
        ),
        (
            "bash",
            &[
                "command",
                "exit_code",
                "cancelled",
                "output",
                "chars",
                "truncated",
            ],
        ),
        (
            "python",
            &[
                "code",
                "exit_code",
                "cancelled",
                "output",
                "chars",
                "truncated",
            ],
        ),
        ("developer", &["text"]),
        ("custom", &["custom_type", "text"]),
        ("file_mention", &["files"]),
        ("compaction", &["tokens_before", "tokens_after", "summary"]),
        ("reset", &[]),
        ("branch_summary", &["summary"]),
        ("model_change", &["model"]),
        ("thinking_level", &["level"]),
    ];
    for it in arr(&d["items"]) {
        let kind = it["kind"].as_str().unwrap();
        assert!(
            it["id"].is_string() && it.get("timestamp").is_some(),
            "{it}"
        );
        let (_, keys) = fields.iter().find(|(k, _)| *k == kind).unwrap();
        for k in *keys {
            assert!(it.get(*k).is_some(), "{kind} item lacks `{k}`: {it}");
        }
    }
    assert_eq!(item(&d, "k0000001")["level"], "low", "{d}");
    assert_eq!(item(&d, "k0000002")["model"], "openai/gpt-5", "{d}");
    assert_eq!(item(&d, "k0000003")["text"], "look", "{d}");
    assert_eq!(item(&d, "k0000003")["images"], 1, "{d}");
    assert_eq!(item(&d, "k0000004")["text"], "dev note", "{d}");
    assert_eq!(item(&d, "k0000005")["custom_type"], "reminder", "{d}");
    assert_eq!(item(&d, "k0000005")["text"], "remember", "{d}");
    assert_eq!(item(&d, "k0000007")["text"], "shown ext", "{d}");
    assert_eq!(item(&d, "k0000009")["files"][0]["path"], "src/a.rs", "{d}");
    assert_eq!(item(&d, "k0000009")["files"][0]["line_count"], 3, "{d}");
    let a = item(&d, "k0000010");
    assert_eq!(a["has_thinking"], true, "{a}");
    assert!(a["text"].as_str().unwrap().contains("answer"), "{a}");
    assert!(
        a["text"].as_str().unwrap().contains("[unknown_block]"),
        "{a}"
    );
    assert_eq!(a["tool_calls"][0]["name"], "bash", "{a}");
    assert_eq!(a["tool_calls"][0]["result"]["text"], "files", "{a}");
    let tr = item(&d, "k0000012");
    assert_eq!(
        (
            tr["tool_call_id"].clone(),
            tr["tool_name"].clone(),
            tr["is_error"].clone(),
            tr["text"].clone()
        ),
        (
            json!("t-missing"),
            json!("grep"),
            json!(true),
            json!("orphan")
        ),
        "{tr}"
    );
    assert_eq!(item(&d, "k0000013")["exit_code"], 2, "{d}");
    assert_eq!(item(&d, "k0000014")["code"], "print(1)", "{d}");
    assert_eq!(item(&d, "k0000015")["tokens_before"], 100, "{d}");
    assert_eq!(item(&d, "k0000017")["summary"], "Went back.", "{d}");
    assert_eq!(item(&d, "k0000022")["custom_type"], "hook", "{d}");
    let text = d.to_string();
    for hidden in ["hidden custom", "hidden ext", "\"mark\""] {
        assert!(!text.contains(hidden), "{hidden} must be hidden: {d}");
    }

    let raw = show_path(&bx, &p, &["--raw"]);
    assert_eq!(arr(&raw["entries"]).len(), lines.len(), "{raw}");
    assert_eq!(
        arr(&raw["entries"])[20],
        lines[20],
        "unknown kinds pass through --raw"
    );

    let none = show_path(&bx, &p, &["--tools", "none"]);
    let none_kinds = kinds(&none);
    for k in ["tool_result", "bash", "python"] {
        assert!(!none_kinds.contains(&k.to_owned()), "{none}");
    }
    let full = show_path(&bx, &p, &["--tools", "full"]);
    assert_eq!(
        item(&full, "k0000010")["tool_calls"][0]["arguments"],
        json!({"command": "ls -la"}),
        "{full}"
    );
}

#[test]
fn ac114_human_transcript_for_a_matches_the_plan_layout() {
    let bx = OmpBox::new();
    let out = bx
        .glen(["show", A.handle, "--human"])
        .env("NO_COLOR", "1")
        .env("COLUMNS", "80")
        .run();
    out.assert_exit(0).assert_no_ansi();
    let lines: Vec<String> = out.out().lines().map(|l| l.trim_end().to_owned()).collect();
    let block = [
        "· thinking level high",
        "",
        "· model anthropic/claude-sonnet-4-5",
        "",
        "❯ user · 09:00",
        "  Why does parser_test flake?",
        "",
        "◆ assistant · claude-sonnet-4-5 · 09:00",
        "  ∴ thinking · 4 words",
        "  Let me read the test.",
        "  ↳ read tests/parser_test.rs · ✓ fn parser_test() { … }",
        "",
        "◆ assistant · 09:00",
        "  The test depends on map order. Fixed.",
    ];
    let found = lines
        .windows(block.len())
        .any(|w| w.iter().zip(block.iter()).all(|(a, b)| a == b));
    assert!(
        found,
        "plan Step 7 verbatim layout not found\n{}",
        out.ctx()
    );
    let header = lines.iter().take(3).cloned().collect::<Vec<_>>().join("\n");
    assert!(
        header.contains(A.title) && header.contains(A.handle),
        "{header}"
    );
    assert!(header.contains("idle"), "{header}");
}

#[test]
fn ac114_human_flags_change_what_is_shown() {
    let bx = OmpBox::new();
    let human = |args: &[&str]| {
        let out = bx.glen(args).env("NO_COLOR", "1").run();
        out.assert_exit(0);
        out.out()
    };
    let default = human(&["show", A.handle, "--human"]);
    assert!(!default.contains("Check the fixture loader."), "{default}");
    assert!(
        default.contains("tests/parser_test.rs") && !default.contains("\"path\""),
        "{default}"
    );
    let thinking = human(&["show", A.handle, "--human", "--thinking"]);
    assert!(thinking.contains("Check the fixture loader."), "{thinking}");
    let full = human(&["show", A.handle, "--human", "--tools", "full"]);
    assert!(
        full.contains("\"path\"") && full.contains("fn parser_test()"),
        "{full}"
    );
    let none = human(&["show", A.handle, "--human", "--tools", "none"]);
    assert!(!none.contains("parser_test.rs"), "{none}");
    let none_e = human(&["show", E.handle, "--human", "--tools", "none"]);
    assert!(!none_e.contains("cargo test"), "{none_e}");
    let d = human(&["show", D.handle, "--human"]);
    assert!(
        d.contains("Abandoned the regex approach.") && !d.contains("Regex failed."),
        "{d}"
    );
    assert!(!d.contains("checkpoint"), "labels are hidden: {d}");
    let cont = human(&["show", B.handle, "--human"]);
    assert!(cont.contains("continued from"), "{cont}");
}

#[test]
fn ac115_show_json_document_shape() {
    let bx = OmpBox::new();
    for cmd in ["show", "tail"] {
        let d = bx.data([cmd, A.handle, "--json"]);
        for k in [
            "session",
            "continued_from",
            "leaf_id",
            "skipped_lines",
            "items",
        ] {
            assert!(d.get(k).is_some(), "{cmd} data lacks `{k}`: {d}");
        }
        assert!(d.get("entries").is_none(), "{d}");
        assert_eq!(d["continued_from"], json!([]), "{d}");
        assert_eq!(d["leaf_id"], "10000009", "{d}");
        assert_eq!(d["skipped_lines"], 0, "{d}");
        let s = &d["session"];
        assert_session_object(s);
        assert_eq!(s["id"], A.id, "{s}");
        assert_eq!(s["handle"], A.handle, "{s}");
        assert_eq!(s["title"], A.title, "{s}");
        assert_eq!(s["first_message"], A.first_message, "{s}");
        assert_eq!(s["cwd"], A.cwd, "{s}");
        assert_eq!(s["bucket"], ALPHA, "{s}");
        assert_eq!(s["model"], MODEL, "{s}");
        assert_eq!(s["last_turn"], "complete", "{s}");
        assert_eq!(s["state"], "idle", "{s}");
        assert_eq!(utc(&s["created_at"]), instant(A.created), "{s}");
        assert_path(&s["path"], &bx.path(&A));
        for it in arr(&d["items"]) {
            assert!(it["kind"].is_string() && it["id"].is_string(), "{it}");
            assert!(it.get("timestamp").is_some(), "{it}");
        }
    }
    let d = show(&bx, &[B.handle]);
    assert_eq!(
        d["session"]["last_turn"], "complete",
        "C's branch ends with a reply: {d}"
    );
    assert_eq!(d["session"]["continuation_of"], B.id, "{d}");
}

#[test]
fn ac115_raw_lists_the_active_branch_entries_verbatim() {
    let bx = OmpBox::new();
    let raw = show(&bx, &[A.handle, "--raw"]);
    for k in [
        "session",
        "continued_from",
        "leaf_id",
        "skipped_lines",
        "entries",
    ] {
        assert!(raw.get(k).is_some(), "--raw data lacks `{k}`: {raw}");
    }
    assert!(raw.get("items").is_none(), "{raw}");
    let expected: Vec<Value> = entry_lines(&bx.path(&A))
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(expected.len(), 9);
    assert_eq!(arr(&raw["entries"]), &expected, "{raw}");

    let out = bx
        .glen(["show", A.handle, "--raw", "--human"])
        .env("NO_COLOR", "1")
        .run();
    out.assert_exit(0);
    assert!(out.out().contains("system-prompt-digest"), "{}", out.ctx());
}

#[test]
fn ac115_item_timestamps_are_utc_or_null() {
    let bx = OmpBox::new();
    let d = show(&bx, &[A.handle]);
    for (it, sec) in arr(&d["items"]).iter().zip([1, 2, 3, 5, 7]) {
        let expected = instant(&format!("2026-10-01T09:00:0{sec}Z"));
        assert_eq!(utc(&it["timestamp"]), expected, "{it}");
    }
    let id = uuid(0x7a);
    let mut no_ts = user("f0000002", Some("f0000001"), "", "no time");
    no_ts.as_object_mut().unwrap().remove("timestamp");
    let p = bx.add(
        "-work-gamma",
        "2026-10-06T10:00:00.000Z",
        &id,
        None,
        &[user("f0000001", None, "not-a-time", "bad time"), no_ts],
    );
    let d = show_path(&bx, &p, &[]);
    assert_eq!(item(&d, "f0000001")["timestamp"], Value::Null, "{d}");
    assert_eq!(item(&d, "f0000002")["timestamp"], Value::Null, "{d}");
}

fn assert_spill_file(bx: &OmpBox, path: &Path, command: &str, ext: &str) -> Vec<u8> {
    let out_dir = bx.glen_home.join("out");
    assert_eq!(path.parent(), Some(out_dir.as_path()), "{}", path.display());
    let name = path.file_name().unwrap().to_str().unwrap();
    let stem = name
        .strip_suffix(&format!(".{ext}"))
        .unwrap_or_else(|| panic!("{name} should end in .{ext}"));
    let parts: Vec<&str> = stem.split('-').collect();
    assert_eq!(parts.len(), 3, "§A6 out/<unix-ms>-<command>-<pid>: {name}");
    assert_eq!(parts[1], command, "{name}");
    assert_eq!(mode(path), 0o600, "{name}");
    assert_eq!(mode(&bx.glen_home), 0o700);
    fs::read(path).unwrap()
}

#[test]
fn ac120_large_show_output_spills_to_glen_home() {
    let bx = OmpBox::new();
    let out = bx
        .glen(["show", E.handle, "--json"])
        .env("GLEN_MAX_OUTPUT", "200")
        .run();
    let env = out.json();
    out.assert_exit(0).assert_stderr_empty();
    assert_eq!(env["ok"], true, "{}", out.ctx());
    assert!(
        env.get("data").is_none(),
        "only the spilled envelope: {}",
        out.ctx()
    );
    let sp = &env["spilled"];
    assert_eq!(sp["format"], "json", "{}", out.ctx());
    let path = Path::new(sp["path"].as_str().unwrap()).to_path_buf();
    let content = assert_spill_file(&bx, &path, "show", "json");
    assert_eq!(sp["bytes"], content.len(), "{}", out.ctx());
    let full: Value = serde_json::from_slice(&content).expect("the spill is one JSON document");
    assert_eq!(item_ids(&full["data"]), strs(&E_ITEMS));

    let bx = OmpBox::new();
    let out = bx
        .glen(["show", E.handle, "--human"])
        .env("GLEN_MAX_OUTPUT", "200")
        .env("CLICOLOR_FORCE", "1")
        .run();
    out.assert_exit(0);
    let text = strip_ansi(&out.out());
    assert_eq!(text.trim_end().lines().count(), 1, "{}", out.ctx());
    assert!(
        text.contains("KB") && text.contains("full output in"),
        "{}",
        out.ctx()
    );
    let out_dir = s(&bx.glen_home.join("out"));
    let start = text
        .find(&out_dir)
        .unwrap_or_else(|| panic!("{}", out.ctx()));
    let path: String = text[start..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    let content = assert_spill_file(&bx, Path::new(&path), "show", "txt");
    assert!(!has_esc(&content), "spilled text has no ANSI");
    assert!(String::from_utf8_lossy(&content).contains("Started."));

    let bx = OmpBox::new();
    let d = {
        let out = bx
            .glen(["show", E.handle, "--json"])
            .env("GLEN_MAX_OUTPUT", "0")
            .run();
        out.ok_data()
    };
    assert_eq!(item_ids(&d), strs(&E_ITEMS));
    bx.assert_no_state();
}

#[test]
fn ac121_show_human_escapes_untrusted_text() {
    let bx = OmpBox::new();
    let id = uuid(0x121);
    let t = "2026-10-06T10:00:01.000Z";
    let mut h = header(
        &id,
        Some("2026-10-06T10:00:00.000Z"),
        Some("/home/\u{1b}[2Jcwd"),
    );
    h["title"] = Value::from("title \u{1b}[31mred");
    let p = bx
        .bucket("-work-gamma")
        .join(session_name("2026-10-06T10:00:00.000Z", &id));
    let msg = "hello \u{1b}[31mred\u{7} there";
    write_lines(
        &p,
        &[
            h,
            user("c1000001", None, t, msg),
            assistant(
                "c1000002",
                Some("c1000001"),
                t,
                json!([{"type": "text", "text": "ok"}, tool_call("t1", "read", json!({"path": "src/\u{1b}[1mbold.rs"}))]),
                "toolUse",
            ),
            tool_result(
                "c1000003",
                Some("c1000002"),
                t,
                "t1",
                "read",
                "out \u{1b}[2Jput",
                false,
            ),
        ],
    );
    let ps = s(&p);
    for flags in [vec![], vec!["--tools", "full"], vec!["--raw"]] {
        let mut args = vec!["show", ps.as_str(), "--human"];
        args.extend(flags);
        let out = bx.glen(&args).env("NO_COLOR", "1").run();
        out.assert_exit(0);
        let text = out.out();
        assert_eq!(raw_controls(&text), [] as [char; 0], "{}", out.ctx());
        assert!(text.contains(r"\x1b[31m"), "{}", out.ctx());

        let out = bx.glen(&args).env("CLICOLOR_FORCE", "1").run();
        let text = out.out();
        for raw in ["\u{1b}[31mred", "\u{1b}[2J", "\u{1b}[1mbold", "\u{7}"] {
            assert!(
                !text.contains(raw),
                "raw {raw:?} from session data\n{}",
                out.ctx()
            );
        }
    }
    let d = show_path(&bx, &p, &[]);
    assert_eq!(
        item(&d, "c1000001")["text"],
        msg,
        "JSON keeps the raw text: {d}"
    );
}

#[test]
fn ac121_show_human_prose_wraps_to_columns() {
    let bx = OmpBox::new();
    let words: Vec<String> = (0..60).map(|i| format!("lorem{i}")).collect();
    let text = words.join(" ");
    let id = uuid(0x122);
    let t = "2026-10-06T10:00:01.000Z";
    let p = bx
        .bucket("-work-gamma")
        .join(session_name("2026-10-06T10:00:00.000Z", &id));
    write_lines(
        &p,
        &[
            title_slot("Wrap test"),
            header(&id, Some("2026-10-06T10:00:00.000Z"), None),
            user("c2000001", None, t, &text),
            reply("c2000002", Some("c2000001"), t, &text),
        ],
    );
    for cols in [40usize, 72] {
        let out = bx
            .glen(["show", &s(&p), "--human"])
            .env("COLUMNS", cols.to_string())
            .env("CLICOLOR_FORCE", "1")
            .run();
        out.assert_exit(0);
        let mut seen = Vec::new();
        for line in out.out().lines() {
            let plain = strip_ansi(line);
            if plain.contains("lorem") {
                assert!(
                    display_width(line) <= cols,
                    "prose wider than COLUMNS={cols}: {plain:?}\n{}",
                    out.ctx()
                );
                seen.extend(
                    plain
                        .split_whitespace()
                        .filter(|w| w.starts_with("lorem"))
                        .map(str::to_owned),
                );
            }
        }
        let mut expected = words.clone();
        expected.extend(words.iter().cloned());
        assert_eq!(
            seen,
            expected,
            "every word is kept, in order\n{}",
            out.ctx()
        );
    }
}

#[test]
fn a10_show_and_tail_on_a_closed_stdout_exit_0() {
    let bx = OmpBox::new();
    for args in [
        vec!["show", A.handle, "--json"],
        vec!["show", A.handle, "--human"],
        vec!["tail", A.handle, "--json"],
    ] {
        let out = bx.glen(&args).stdout(closed_stdout()).run();
        out.assert_no_panic();
        out.assert_exit(0);
    }
}
