//! `glen tail` and `tail -f` (AC-116–AC-120, AC-124, AC-125; docs/design.md §A3 Follow, §A10
//! NDJSON streams and follow events, §A11 exit codes).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::reader::*;
use crate::support::*;

/// A user entry appended to fixture A's copy (its leaf is 10000009).
fn a_user(id: &str, parent: &str, text: &str) -> Value {
    user(id, Some(parent), "2026-10-01T09:10:00.000Z", text)
}

/// Spawn `tail e27c0de -f --json`, check the `session` line and the five backlog items.
fn follow_a(bx: &OmpBox, extra: &[&str], env: &[(&str, &str)]) -> Stream {
    let mut args = vec!["tail", A.handle, "-f", "--json"];
    args.extend_from_slice(extra);
    let mut st = bx.follow_env(&args, env);
    let ev = st.expect_event("session", T);
    assert_session_object(&ev["session"]);
    assert_eq!(ev["session"]["id"], A.id, "{}", st.ctx());
    assert_eq!(ev["continued_from"], json!([]), "{}", st.ctx());
    assert_eq!(ev["skipped_lines"], 0, "{}", st.ctx());
    let items = st.items(5, T);
    let ids: Vec<&str> = items.iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(ids, A_ITEMS, "backlog\n{}", st.ctx());
    st
}

/// Stop a JSON stream with SIGINT and check the §A10 trailer (AC-124).
fn stop(mut st: Stream) {
    st.interrupt();
    let status = st.wait(T);
    assert_eq!(status.code(), Some(130), "{}", st.ctx());
    assert_eq!(
        st.last_line(),
        json!({"v": 1, "event": "end", "ok": true}),
        "{}",
        st.ctx()
    );
    assert!(
        st.stderr().is_empty(),
        "JSON mode keeps stderr empty\n{}",
        st.ctx()
    );
}

/// A continuation of A (A's entries plus `new_entry`, `continuationOf` A): its final path in
/// A's bucket and its content.
fn continuation_of_a(bx: &OmpBox, new_id: &str, new_entry: &Value) -> (PathBuf, String) {
    let created = "2026-10-01T10:00:00.000Z";
    let path = bx.sessions.join(ALPHA).join(session_name(created, new_id));
    let mut h = header(new_id, Some(created), Some(A.cwd));
    h["parentSession"] = Value::from(A.id);
    h["continuationOf"] = Value::from(A.id);
    let mut text = format!("{h}\n");
    for l in entry_lines(&bx.path(&A)) {
        text.push_str(&l);
        text.push('\n');
    }
    text.push_str(&format!("{new_entry}\n"));
    (path, text)
}

/// Publish a continuation of A in A's bucket, the way omp does (stage a dotfile, rename).
fn continue_a(bx: &OmpBox, new_id: &str, new_entry: &Value) -> PathBuf {
    let (path, text) = continuation_of_a(bx, new_id, new_entry);
    let name = path.file_name().unwrap().to_str().unwrap();
    let stage = path.with_file_name(format!(".{name}.staged.tmp"));
    fs::write(&stage, text).unwrap();
    fs::rename(&stage, &path).unwrap();
    path
}

#[test]
fn ac116_tail_prints_the_last_n_items() {
    let bx = OmpBox::new();
    let d = bx.data(["tail", A.handle, "-n", "2", "--json"]);
    assert_eq!(item_ids(&d), ["10000005", "10000007"], "{d}");
    assert_eq!(kinds(&d), ["assistant", "assistant"], "{d}");
    for k in [
        "session",
        "continued_from",
        "leaf_id",
        "skipped_lines",
        "items",
    ] {
        assert!(d.get(k).is_some(), "tail data lacks `{k}`: {d}");
    }
    assert_session_object(&d["session"]);
    let d = bx.data(["tail", A.handle, "-n", "0", "--json"]);
    assert_eq!(d["items"], json!([]), "{d}");
    let d = bx.data(["tail", A.handle, "-n", "100", "--json"]);
    assert_eq!(item_ids(&d), strs(&A_ITEMS), "{d}");
    let d = bx.data(["tail", A.handle, "--lines", "1", "--json"]);
    assert_eq!(item_ids(&d), ["10000007"], "{d}");
    bx.assert_no_state();
}

#[test]
fn ac116_tail_defaults_to_ten_items_with_collapsed_tools() {
    let bx = OmpBox::new();
    let id = uuid(0x116);
    let mut body = Vec::new();
    let mut parent: Option<String> = None;
    let mut all_ids = Vec::new();
    for n in 1..=12u32 {
        let eid = format!("{n:08x}");
        let ts = format!("2026-10-06T10:00:{n:02}.000Z");
        let e = if n % 2 == 1 {
            user(&eid, parent.as_deref(), &ts, "question")
        } else {
            reply(&eid, parent.as_deref(), &ts, "answer")
        };
        body.push(e);
        all_ids.push(eid.clone());
        parent = Some(eid);
    }
    let p = bx.add("-work-gamma", "2026-10-06T10:00:00.000Z", &id, None, &body);
    let d = bx.data(["tail", &s(&p), "--json"]);
    assert_eq!(item_ids(&d), all_ids[2..], "default -n is 10: {d}");

    let d = bx.data(["tail", A.handle, "--json"]);
    assert_eq!(item_ids(&d), strs(&A_ITEMS), "{d}");
    let call = &d["items"][3]["tool_calls"][0];
    assert!(
        call.get("arguments").is_none(),
        "tail collapses tool calls: {call}"
    );
    assert!(!d.to_string().contains("Check the fixture loader."), "{d}");
}

#[test]
fn ac116_tail_human_renders_the_last_items() {
    let bx = OmpBox::new();
    let out = bx
        .glen(["tail", A.handle, "-n", "1", "--human"])
        .env("NO_COLOR", "1")
        .run();
    out.assert_exit(0).assert_no_ansi();
    let text = out.out();
    assert!(text.contains(A.handle), "{}", out.ctx());
    assert!(
        text.contains("The test depends on map order. Fixed."),
        "{}",
        out.ctx()
    );
    assert!(
        !text.contains("Why does parser_test flake?"),
        "{}",
        out.ctx()
    );
}

#[test]
fn ac117_follow_streams_the_backlog_then_appended_entries() {
    let bx = OmpBox::new();
    let mut st = follow_a(&bx, &[], &[]);
    let a = bx.path(&A);

    let appended = Instant::now();
    append_line(&a, &a_user("10000010", "10000009", "more"));
    let ev = st.expect_event("item", T);
    let latency = appended.elapsed();
    assert_eq!(ev["item"]["id"], "10000010", "{}", st.ctx());
    assert_eq!(ev["item"]["kind"], "user", "{}", st.ctx());
    assert_eq!(ev["item"]["text"], "more", "{}", st.ctx());
    assert!(
        latency <= Duration::from_millis(2500),
        "an appended entry is written within 2 s (took {latency:?})\n{}",
        st.ctx()
    );

    append_line(
        &a,
        &reply(
            "10000011",
            Some("10000010"),
            "2026-10-01T09:10:01.000Z",
            "noted",
        ),
    );
    let ev = st.expect_event("item", T);
    assert_eq!(ev["item"]["id"], "10000011", "{}", st.ctx());
    assert_eq!(ev["item"]["kind"], "assistant", "{}", st.ctx());
    stop(st);
    bx.assert_no_state();
}

#[test]
fn ac117_follow_waits_for_a_complete_line() {
    let bx = OmpBox::new();
    let mut st = follow_a(&bx, &[], &[]);
    let a = bx.path(&A);
    let line = format!("{}\n", a_user("10000010", "10000009", "split write"));
    let (head, rest) = line.split_at(line.len() / 2);
    let mut f = fs::OpenOptions::new().append(true).open(&a).unwrap();
    f.write_all(head.as_bytes()).unwrap();
    f.flush().unwrap();
    assert!(
        st.next_line(Duration::from_millis(700)).is_none(),
        "nothing is written for a partial line\n{}",
        st.ctx()
    );
    f.write_all(rest.as_bytes()).unwrap();
    f.flush().unwrap();
    let ev = st.expect_event("item", T);
    assert_eq!(ev["item"]["id"], "10000010", "{}", st.ctx());
    assert_eq!(ev["item"]["text"], "split write", "{}", st.ctx());
    append_line(&a, &a_user("10000011", "10000010", "sentinel"));
    let ev = st.expect_event("item", T);
    assert_eq!(
        ev["item"]["id"],
        "10000011",
        "exactly one item per line\n{}",
        st.ctx()
    );
    stop(st);
}

#[test]
fn ac117_follow_with_n_0_streams_only_new_items_as_ndjson_by_default() {
    let bx = OmpBox::new();
    // No --json: a piped stdout selects JSON, so the stream is NDJSON.
    let mut st = bx.follow(&["tail", A.handle, "-f", "-n", "0"]);
    let ev = st.expect_event("session", T);
    assert_eq!(ev["session"]["id"], A.id, "{}", st.ctx());
    append_line(&bx.path(&A), &a_user("10000010", "10000009", "fresh"));
    let ev = st.expect_event("item", T);
    assert_eq!(
        ev["item"]["id"],
        "10000010",
        "no backlog with -n 0\n{}",
        st.ctx()
    );
    stop(st);
}

#[test]
fn ac117_follow_is_never_spilled_or_coloured() {
    let bx = OmpBox::new();
    let mut st = follow_a(
        &bx,
        &["--color", "always"],
        &[
            ("GLEN_MAX_OUTPUT", "10"),
            ("CLICOLOR_FORCE", "1"),
            ("COLORTERM", "truecolor"),
        ],
    );
    append_line(&bx.path(&A), &a_user("10000010", "10000009", "more"));
    st.expect_event("item", T);
    stop(st);
    assert!(!bx.glen_home.exists(), "a stream never spills");
}

#[test]
fn ac117_follow_in_human_mode_renders_text() {
    let bx = OmpBox::new();
    let mut st = bx.follow_env(&["tail", A.handle, "-f", "--human"], &[("NO_COLOR", "1")]);
    st.wait_for_text("The test depends on map order. Fixed.", T);
    append_line(
        &bx.path(&A),
        &a_user("10000010", "10000009", "human follow text"),
    );
    st.wait_for_text("human follow text", T);
    st.interrupt();
    let status = st.wait(T);
    assert_eq!(status.code(), Some(130), "{}", st.ctx());
    for l in &st.lines {
        assert!(!l.contains('\u{1b}'), "{}", st.ctx());
        let json_event =
            serde_json::from_str::<Value>(l.trim()).is_ok_and(|v| v.get("event").is_some());
        assert!(!json_event, "human mode prints no NDJSON\n{}", st.ctx());
    }
}

#[test]
fn ac118_follow_switches_to_a_continuation_without_repeating_items() {
    let bx = OmpBox::new();
    let mut st = follow_a(&bx, &[], &[]);
    let new_id = uuid(0x118);
    let next = user(
        "10000010",
        Some("10000009"),
        "2026-10-01T10:00:01.000Z",
        "continued here",
    );
    let path = continue_a(&bx, &new_id, &next);

    let ev = st.expect_event("continued", T);
    assert_eq!(ev["from"], A.id, "{}", st.ctx());
    assert_eq!(ev["to"], new_id.as_str(), "{}", st.ctx());
    assert_path(&ev["path"], &path);
    let ev = st.expect_event("item", T);
    assert_eq!(
        ev["item"]["id"],
        "10000010",
        "only unseen items\n{}",
        st.ctx()
    );

    append_line(
        &path,
        &reply(
            "10000011",
            Some("10000010"),
            "2026-10-01T10:00:02.000Z",
            "in the new file",
        ),
    );
    let ev = st.expect_event("item", T);
    assert_eq!(
        ev["item"]["id"],
        "10000011",
        "follows the new file\n{}",
        st.ctx()
    );
    stop(st);
}

#[test]
fn ac119_resolution_errors_before_a_follow_are_one_envelope() {
    let bx = OmpBox::new();
    for (arg, code) in [
        ("nope", "not_found"),
        ("c0de", "ambiguous"),
        ("j1234567", "usage"),
    ] {
        for json_flag in [true, false] {
            let mut args = vec!["tail", arg, "-f"];
            if json_flag {
                args.push("--json");
            }
            let out = bx.glen(&args).run();
            let err = out.error_envelope(code);
            out.assert_stderr_empty();
            assert!(err.get("event").is_none(), "{}", out.ctx());
            assert!(
                !out.out().contains("\"event\""),
                "not NDJSON\n{}",
                out.ctx()
            );
        }
    }
}

#[test]
fn ac119_a_vanished_file_ends_the_stream_with_not_found() {
    let bx = OmpBox::new();
    let mut st = follow_a(&bx, &[], &[]);
    let removed = Instant::now();
    fs::remove_file(bx.path(&A)).unwrap();
    let status = st.wait(Duration::from_secs(30));
    let took = removed.elapsed();
    assert_eq!(status.code(), Some(3), "{}", st.ctx());
    let last = st.last_line();
    assert_eq!(last["ok"], false, "{}", st.ctx());
    assert_eq!(last["error"]["code"], "not_found", "{}", st.ctx());
    assert!(
        took >= Duration::from_millis(4500) && took <= Duration::from_secs(15),
        "ends about 5 s after the file disappears (took {took:?})\n{}",
        st.ctx()
    );
    assert!(
        !st.lines.iter().any(|l| l.contains("\"event\":\"end\"")),
        "an error stream has no `end` line\n{}",
        st.ctx()
    );
}

#[test]
fn ac119_a_vanished_file_whose_continuation_appears_keeps_following() {
    let bx = OmpBox::new();
    let mut st = follow_a(&bx, &[], &[]);
    let new_id = uuid(0x119);
    let next = user(
        "10000010",
        Some("10000009"),
        "2026-10-01T10:00:01.000Z",
        "moved on",
    );
    // Build the continuation from A's content before A disappears, outside sessions_dir.
    let (path, text) = continuation_of_a(&bx, &new_id, &next);
    let staged = bx.root.join("staged-continuation");
    fs::write(&staged, text).unwrap();
    fs::remove_file(bx.path(&A)).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    fs::rename(&staged, &path).unwrap();

    let ev = st.expect_event("continued", T);
    assert_eq!(ev["to"], new_id.as_str(), "{}", st.ctx());
    let ev = st.expect_event("item", T);
    assert_eq!(ev["item"]["id"], "10000010", "{}", st.ctx());
    stop(st);
}

#[test]
fn ac124_sigint_ends_a_json_stream_with_end_and_exit_130() {
    let bx = OmpBox::new();
    let mut st = follow_a(&bx, &[], &[]);
    let sent = Instant::now();
    st.interrupt();
    let status = st.wait(T);
    let took = sent.elapsed();
    assert_eq!(status.code(), Some(130), "{}", st.ctx());
    assert_eq!(
        st.last_line(),
        json!({"v": 1, "event": "end", "ok": true}),
        "{}",
        st.ctx()
    );
    assert!(
        took <= Duration::from_secs(2),
        "stops within a poll interval (took {took:?})\n{}",
        st.ctx()
    );
}

#[test]
fn ac124_sigint_ends_a_human_stream_with_exit_130() {
    let bx = OmpBox::new();
    let mut st = bx.follow_env(&["tail", A.handle, "-f", "--human"], &[("NO_COLOR", "1")]);
    st.wait_for_text("The test depends on map order. Fixed.", T);
    st.interrupt();
    let status = st.wait(T);
    assert_eq!(status.code(), Some(130), "{}", st.ctx());
}

#[test]
fn ac125_a_rename_replace_emits_rewritten_then_only_new_items() {
    let bx = OmpBox::new();
    let mut st = follow_a(&bx, &[], &[]);
    let a = bx.path(&A);
    let mut text = fs::read_to_string(&a).unwrap();
    text.push_str(&jsonl(&[
        a_user("10000010", "10000009", "after the rewrite"),
        reply(
            "10000011",
            Some("10000010"),
            "2026-10-01T09:10:01.000Z",
            "second new",
        ),
    ]));
    let stage = a.with_file_name(format!(
        ".{}.4f2a.tmp",
        a.file_name().unwrap().to_str().unwrap()
    ));
    fs::write(&stage, text).unwrap();
    fs::rename(&stage, &a).unwrap();

    let ev = st.expect_event("rewritten", T);
    assert_path(&ev["path"], &a);
    let items = st.items(2, T);
    let ids: Vec<&str> = items.iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["10000010", "10000011"], "{}", st.ctx());

    append_line(&a, &a_user("10000012", "10000011", "sentinel"));
    let ev = st.expect_event("item", T);
    assert_eq!(ev["item"]["id"], "10000012", "{}", st.ctx());
    stop(st);
}

#[test]
fn ac125_a_shrunk_file_emits_rewritten_then_only_new_items() {
    let bx = OmpBox::new();
    let mut st = follow_a(&bx, &[], &[]);
    let a = bx.path(&A);
    let old_len = fs::metadata(&a).unwrap().len();
    // Drop the 256-byte title slot and add one short entry: the file shrinks in place.
    let short = json!({"type": "message", "id": "10000010", "parentId": "10000009",
                       "timestamp": "2026-10-01T09:10:00.000Z",
                       "message": {"role": "user", "content": "shrunk"}});
    let mut text: String = fs::read_to_string(&a)
        .unwrap()
        .split_inclusive('\n')
        .skip(1)
        .collect();
    text.push_str(&format!("{short}\n"));
    assert!((text.len() as u64) < old_len, "the rewrite must be shorter");
    let mut f = fs::OpenOptions::new().write(true).open(&a).unwrap();
    f.write_all(text.as_bytes()).unwrap();
    f.set_len(text.len() as u64).unwrap();
    f.flush().unwrap();
    drop(f);

    let ev = st.expect_event("rewritten", T);
    assert!(ev.get("path").is_some(), "{}", st.ctx());
    let mut ev = st.next_json(T);
    if ev["event"] == "branch_switch" {
        ev = st.next_json(T);
    }
    assert_eq!(ev["event"], "item", "{}", st.ctx());
    assert_eq!(
        ev["item"]["id"],
        "10000010",
        "only the unseen entry\n{}",
        st.ctx()
    );

    append_line(&a, &a_user("10000011", "10000010", "sentinel"));
    let ev = st.expect_event("item", T);
    assert_eq!(ev["item"]["id"], "10000011", "{}", st.ctx());
    stop(st);
}

#[test]
fn a10_follow_on_a_closed_stdout_exits_1() {
    let bx = OmpBox::new();
    let out = bx
        .glen(["tail", A.handle, "-f", "--json"])
        .stdout(closed_stdout())
        .run();
    out.assert_no_panic();
    out.assert_exit(1);
}

#[test]
fn ac120_a_large_tail_document_spills_but_a_stream_does_not() {
    let bx = OmpBox::new();
    let out = bx
        .glen(["tail", E.handle, "--json"])
        .env("GLEN_MAX_OUTPUT", "200")
        .run();
    let env = out.json();
    out.assert_exit(0).assert_stderr_empty();
    let path = Path::new(
        env["spilled"]["path"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a spilled envelope\n{}", out.ctx())),
    )
    .to_path_buf();
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(name.contains("-tail-") && name.ends_with(".json"), "{name}");
    assert_eq!(mode(&path), 0o600);
    let full: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(item_ids(&full["data"]), strs(&E_ITEMS));
}
