//! `glen inbox` and `glen inbox -f` over crafted §A5 inbox files, plus the reply-record null
//! contract on the RPC route (AC-319, AC-320, AC-324, AC-325; docs/design.md §A5, §A6, §A9, §A10).

use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

use crate::live_support::*;
use crate::reader::*;
use crate::rpc_support::*;
use crate::support::*;

/// §A5 record keys.
const RECORD_KEYS: [&str; 9] = [
    "v",
    "id",
    "session_id",
    "job_id",
    "request_id",
    "tool_call_id",
    "text",
    "final",
    "at",
];

/// One poll interval is 250 ms; a new line must show up well within this.
const FOLLOW: Duration = Duration::from_millis(1500);

fn inbox_file(home: &Path, session: &str) -> PathBuf {
    home.join(format!("inbox/{session}.ndjson"))
}

fn cursor(home: &Path, session: &str) -> PathBuf {
    home.join(format!("inbox/.read/{session}"))
}

fn job(tail: u64) -> String {
    job_uuid(1_793_000_000_000, tail)
}

/// `glen inbox <args> --json` → data (exit 0, one envelope, empty stderr, no ANSI).
fn inbox(sb: &Sandbox, args: &[&str]) -> Value {
    let mut a = vec!["inbox"];
    a.extend_from_slice(args);
    a.push("--json");
    data_of(&sb.glen(a).run(), 0)
}

fn texts(d: &Value) -> Vec<String> {
    arr(&d["messages"])
        .iter()
        .map(|m| m["text"].as_str().unwrap().to_owned())
        .collect()
}

/// A message is the record plus `session_handle` and `job_handle`.
fn assert_message(m: &Value, rec: &Value) {
    for k in RECORD_KEYS {
        assert_eq!(m[k], rec[k], "message `{k}`: {m}");
    }
    assert_eq!(
        m["session_handle"],
        tail7(rec["session_id"].as_str().unwrap()),
        "{m}"
    );
    match rec["job_id"].as_str() {
        Some(j) => assert_eq!(m["job_handle"], job_handle(j), "{m}"),
        None => assert!(m["job_handle"].is_null(), "{m}"),
    }
}

/// `glen inbox <args>` from the sandbox, for streams and raw pipes.
fn command(sb: &Sandbox, args: &[&str]) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.env_clear()
        .args(args)
        .env("HOME", &sb.home)
        .env("GLEN_HOME", &sb.glen_home)
        .env("PATH", &sb.empty_bin)
        .current_dir(&sb.cwd)
        .stdin(Stdio::null());
    cmd
}

fn follow(sb: &Sandbox, args: &[&str]) -> Stream {
    Stream::spawn(command(sb, args), args.join(" "))
}

/// Records in two sessions whose parsed `at` order differs from both file order and string
/// order: r2 is 09:30Z written with a +02:00 offset.
fn two_sessions(home: &Path) -> Vec<Value> {
    let j = job(0xabc1234);
    let r1 = inbox_record(A.id, Some(&j), "first", "2026-10-30T10:00:00.000Z");
    let r2 = inbox_record(D.id, None, "second", "2026-10-30T11:30:00+02:00");
    let r3 = inbox_record(A.id, None, "third", "2026-10-30T10:00:00.5Z");
    let r4 = inbox_record(D.id, None, "fourth", "2026-10-30T10:00:00.250000001Z");
    append_record(&inbox_file(home, A.id), &r1);
    append_record(&inbox_file(home, A.id), &r3);
    append_record(&inbox_file(home, D.id), &r2);
    append_record(&inbox_file(home, D.id), &r4);
    vec![r2, r1, r4, r3]
}

// ---------------------------------------------------------------------------------------------
// AC-319 — unread, cursors, --peek, --all, --session.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac319_unread_records_are_listed_in_parsed_at_order_then_marked_read() {
    let sb = Sandbox::new();
    let want = two_sessions(&sb.glen_home);
    let d = inbox(&sb, &[]);
    assert_eq!(d["total"], 4, "{d}");
    assert_eq!(d["marked_read"], true, "{d}");
    assert_eq!(d["skipped_lines"], 0, "{d}");
    assert_eq!(texts(&d), ["second", "first", "fourth", "third"], "{d}");
    for (m, rec) in arr(&d["messages"]).iter().zip(&want) {
        assert_message(m, rec);
    }

    // Cursors: `inbox/.read/<key>` 0600 in a 0700 dir, at the end of the last complete line.
    assert_eq!(mode(&sb.glen_home.join("inbox/.read")), 0o700);
    for id in [A.id, D.id] {
        let c = cursor(&sb.glen_home, id);
        assert_eq!(mode(&c), 0o600, "{}", c.display());
        let v: Value = serde_json::from_slice(&fs::read(&c).unwrap()).unwrap();
        let md = fs::metadata(inbox_file(&sb.glen_home, id)).unwrap();
        assert_eq!(v["v"], 1, "{v}");
        assert_eq!(v["offset"], md.len(), "{v}");
        assert_eq!(v["ino"], md.ino(), "{v}");
    }

    let d = inbox(&sb, &[]);
    assert_eq!(d["total"], 0, "everything was read: {d}");
    assert_eq!(d["messages"], json!([]), "{d}");

    let d = inbox(&sb, &["--all"]);
    assert_eq!(d["total"], 4, "--all includes read records: {d}");
    assert_eq!(texts(&d), ["second", "first", "fourth", "third"], "{d}");
}

#[test]
fn ac319_peek_leaves_every_cursor_untouched() {
    let sb = Sandbox::new();
    two_sessions(&sb.glen_home);
    let d = inbox(&sb, &["--peek"]);
    assert_eq!(d["total"], 4, "{d}");
    assert_eq!(d["marked_read"], false, "{d}");
    assert!(
        !sb.glen_home.join("inbox/.read").exists(),
        "--peek writes no cursor"
    );
    assert_eq!(inbox(&sb, &["--peek", "--all"])["total"], 4);

    assert_eq!(inbox(&sb, &[])["total"], 4, "still unread after --peek");
    let snap = snapshot(&sb.glen_home.join("inbox/.read"));
    append_record(
        &inbox_file(&sb.glen_home, A.id),
        &inbox_record(A.id, None, "late", "2026-10-30T12:00:00Z"),
    );
    let d = inbox(&sb, &["--peek"]);
    assert_eq!(texts(&d), ["late"], "{d}");
    assert_eq!(
        snapshot(&sb.glen_home.join("inbox/.read")),
        snap,
        "--peek leaves existing cursors as they were"
    );
}

#[test]
fn ac319_session_filter_matches_a_full_id_or_a_hex_tail() {
    let sb = Sandbox::new();
    two_sessions(&sb.glen_home);
    let d = inbox(&sb, &["--session", A.id, "--peek"]);
    assert_eq!(texts(&d), ["first", "third"], "{d}");
    // D ends …9a72: a 4-hex tail, matched case-insensitively.
    let d = inbox(&sb, &["--session", "9A72", "--peek"]);
    assert_eq!(texts(&d), ["second", "fourth"], "{d}");
    let d = inbox(&sb, &["--session", &tail7(D.id)]);
    assert_eq!(texts(&d), ["second", "fourth"], "{d}");
    assert!(
        cursor(&sb.glen_home, D.id).is_file(),
        "the filtered session is marked"
    );
    assert!(
        !cursor(&sb.glen_home, A.id).exists(),
        "other sessions are not marked"
    );
    assert_eq!(texts(&inbox(&sb, &[])), ["first", "third"]);
}

#[test]
fn ac319_a_partial_last_line_is_neither_shown_nor_consumed() {
    let sb = Sandbox::new();
    let f = inbox_file(&sb.glen_home, A.id);
    let r1 = inbox_record(A.id, None, "whole", "2026-10-30T10:00:00Z");
    let r2 = inbox_record(A.id, None, "split", "2026-10-30T10:00:01Z");
    append_record(&f, &r1);
    let line = r2.to_string();
    let (head, rest) = line.split_at(line.len() / 2);
    append_bytes(&f, head.as_bytes());
    let d = inbox(&sb, &[]);
    assert_eq!(texts(&d), ["whole"], "{d}");
    assert_eq!(
        d["skipped_lines"], 0,
        "a partial line is not malformed: {d}"
    );
    assert_eq!(inbox(&sb, &[])["total"], 0);

    append_bytes(&f, format!("{rest}\n").as_bytes());
    assert_eq!(
        texts(&inbox(&sb, &[])),
        ["split"],
        "completed line, consumed once"
    );
    assert_eq!(inbox(&sb, &[])["total"], 0);
}

#[test]
fn ac319_malformed_lines_are_skipped_and_counted() {
    let sb = Sandbox::new();
    let f = inbox_file(&sb.glen_home, A.id);
    append_record(&f, &inbox_record(A.id, None, "one", "2026-10-30T10:00:00Z"));
    append_bytes(&f, b"this is not json\n");
    append_record(&f, &inbox_record(A.id, None, "two", "2026-10-30T10:00:01Z"));
    let d = inbox(&sb, &[]);
    assert_eq!(texts(&d), ["one", "two"], "{d}");
    assert_eq!(d["skipped_lines"], 1, "{d}");
    assert_eq!(d["total"], 2, "{d}");
}

#[test]
fn ac319_a_truncated_or_replaced_file_restarts_at_zero() {
    let sb = Sandbox::new();
    let f = inbox_file(&sb.glen_home, A.id);
    append_record(
        &f,
        &inbox_record(
            A.id,
            None,
            "a long first reply text",
            "2026-10-30T10:00:00Z",
        ),
    );
    append_record(
        &f,
        &inbox_record(
            A.id,
            None,
            "a long second reply text",
            "2026-10-30T10:00:01Z",
        ),
    );
    assert_eq!(inbox(&sb, &[])["total"], 2);

    // Same inode, shorter content: the cursor offset is beyond the file size.
    let ino = fs::metadata(&f).unwrap().ino();
    fs::write(
        &f,
        format!(
            "{}\n",
            inbox_record(A.id, None, "x", "2026-10-30T10:00:02Z")
        ),
    )
    .unwrap();
    assert_eq!(fs::metadata(&f).unwrap().ino(), ino);
    assert_eq!(texts(&inbox(&sb, &[])), ["x"], "truncation restarts at 0");

    // A new inode with longer content.
    let tmp = sb.glen_home.join("inbox/.replace.tmp");
    let mut body = String::new();
    for (i, t) in ["p", "q", "r"].iter().enumerate() {
        body.push_str(&format!(
            "{}\n",
            inbox_record(A.id, None, t, &format!("2026-10-30T11:00:0{i}Z"))
        ));
    }
    fs::write(&tmp, body).unwrap();
    fs::rename(&tmp, &f).unwrap();
    assert_ne!(fs::metadata(&f).unwrap().ino(), ino);
    assert_eq!(
        texts(&inbox(&sb, &[])),
        ["p", "q", "r"],
        "a new inode restarts at 0"
    );
}

#[test]
fn ac319_a_missing_glen_home_or_inbox_is_empty_and_creates_nothing() {
    let sb = Sandbox::new();
    for args in [
        &[][..],
        &["--peek"][..],
        &["--all"][..],
        &["--session", A.id][..],
    ] {
        let d = inbox(&sb, args);
        assert_eq!(
            d,
            json!({"total": 0, "marked_read": false, "skipped_lines": 0, "messages": []}),
            "{args:?}"
        );
        assert!(!sb.glen_home.exists(), "{args:?}: GLEN_HOME was created");
    }
    mkdir_private(&sb.glen_home);
    assert_eq!(inbox(&sb, &[])["total"], 0);
    assert!(
        list_tree(&sb.glen_home).is_empty(),
        "{:?}",
        list_tree(&sb.glen_home)
    );
}

#[test]
fn ac319_a_failed_write_advances_no_cursor() {
    let sb = Sandbox::new();
    two_sessions(&sb.glen_home);
    for mode_flag in ["--json", "--human"] {
        let out = sb.glen(["inbox", mode_flag]).stdout(closed_stdout()).run();
        out.assert_no_panic();
        assert_eq!(
            out.code(),
            1,
            "§A10: a closed stdout exits 1\n{}",
            out.ctx()
        );
        assert!(
            !sb.glen_home.join("inbox/.read").exists(),
            "{mode_flag}: a broken pipe marks nothing"
        );
    }
    assert_eq!(inbox(&sb, &[])["total"], 4, "everything is still unread");
}

#[test]
fn ac319_human_output_shows_the_replies_sanitized_and_marks_them_read() {
    let sb = Sandbox::new();
    let f = inbox_file(&sb.glen_home, A.id);
    append_record(
        &f,
        &inbox_record(A.id, None, "plain words", "2026-10-30T10:00:00Z"),
    );
    append_record(
        &f,
        &inbox_record(A.id, None, "red \u{1b}[31mALERT", "2026-10-30T10:00:01Z"),
    );
    let out = sb.glen(["inbox", "--human"]).env("COLUMNS", "100").run();
    out.assert_exit(0).assert_no_ansi();
    assert!(!out.is_json(), "{}", out.ctx());
    let text = out.out();
    assert!(text.contains("plain words"), "{}", out.ctx());
    assert!(text.contains("ALERT"), "{}", out.ctx());
    assert_eq!(inbox(&sb, &[])["total"], 0, "human mode marks read too");
}

// ---------------------------------------------------------------------------------------------
// AC-320 — `glen inbox -f`.
// ---------------------------------------------------------------------------------------------

fn expect_message(st: &mut Stream, text: &str, timeout: Duration) -> Value {
    let ev = st.expect_event("message", timeout);
    assert_eq!(ev["message"]["text"], text, "{}", st.ctx());
    for k in RECORD_KEYS {
        assert!(ev["message"].get(k).is_some(), "message lacks `{k}`: {ev}");
    }
    ev
}

#[test]
fn ac320_follow_streams_the_backlog_then_new_lines_and_new_sessions() {
    let sb = Sandbox::new();
    let fa = inbox_file(&sb.glen_home, A.id);
    append_record(&fa, &inbox_record(A.id, None, "b1", "2026-10-30T10:00:00Z"));
    append_record(&fa, &inbox_record(A.id, None, "b2", "2026-10-30T10:00:01Z"));
    let mut st = follow(&sb, &["inbox", "-f", "--json"]);
    expect_message(&mut st, "b1", T);
    expect_message(&mut st, "b2", T);

    append_record(&fa, &inbox_record(A.id, None, "n1", "2026-10-30T10:00:02Z"));
    expect_message(&mut st, "n1", FOLLOW);
    let fd = inbox_file(&sb.glen_home, D.id);
    append_record(&fd, &inbox_record(D.id, None, "d1", "2026-10-30T10:00:03Z"));
    let ev = expect_message(&mut st, "d1", FOLLOW);
    assert_eq!(ev["message"]["session_id"], D.id, "{ev}");

    st.interrupt();
    let status = st.wait(T);
    assert_eq!(status.code(), Some(130), "{}", st.ctx());
    assert_eq!(st.last_line(), json!({"v": 1, "event": "end", "ok": true}));
    assert!(st.stderr().is_empty(), "AC-324: {}", st.ctx());
    assert_eq!(
        inbox(&sb, &[])["total"],
        0,
        "each written line was marked read"
    );
}

#[test]
fn ac320_follow_peek_marks_nothing() {
    let sb = Sandbox::new();
    two_sessions(&sb.glen_home);
    let mut st = follow(&sb, &["inbox", "-f", "--peek", "--json"]);
    for t in ["second", "first", "fourth", "third"] {
        expect_message(&mut st, t, T);
    }
    st.interrupt();
    assert_eq!(st.wait(T).code(), Some(130));
    assert!(!sb.glen_home.join("inbox/.read").exists());
    assert_eq!(inbox(&sb, &[])["total"], 4);
}

#[test]
fn ac320_follow_all_replays_read_records() {
    let sb = Sandbox::new();
    two_sessions(&sb.glen_home);
    assert_eq!(inbox(&sb, &[])["total"], 4);
    let mut st = follow(&sb, &["inbox", "-f", "--all", "--json"]);
    for t in ["second", "first", "fourth", "third"] {
        expect_message(&mut st, t, T);
    }
    st.interrupt();
    assert_eq!(st.wait(T).code(), Some(130));
}

#[test]
fn ac320_a_broken_pipe_stops_following_and_marks_nothing_further() {
    let sb = Sandbox::new();
    let f = inbox_file(&sb.glen_home, A.id);
    append_record(
        &f,
        &inbox_record(A.id, None, "seen", "2026-10-30T10:00:00Z"),
    );
    let mut cmd = command(&sb, &["inbox", "-f", "--json"]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    assert!(first.contains("\"seen\""), "{first}");
    drop(reader);
    append_record(
        &f,
        &inbox_record(A.id, None, "unseen", "2026-10-30T10:00:01Z"),
    );
    let status = poll_until("inbox -f to stop after the broken pipe", LONG, || {
        child.try_wait().unwrap()
    });
    let mut err = String::new();
    let _ = std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err);
    assert!(!err.contains("panicked"), "{err}");
    assert_eq!(
        status.code(),
        Some(1),
        "§A10: a closed stdout ends the stream with exit 1; stderr: {err}"
    );
    let d = inbox(&sb, &["--peek"]);
    assert_eq!(
        texts(&d),
        ["unseen"],
        "the written line was marked, the failed one was not: {d}"
    );
}

#[test]
fn ac320_follow_with_no_inbox_creates_nothing_and_picks_up_a_new_file() {
    let sb = Sandbox::new();
    let mut st = follow(&sb, &["inbox", "-f", "--json"]);
    assert!(
        st.next_line(Duration::from_millis(700)).is_none(),
        "{}",
        st.ctx()
    );
    assert!(!sb.glen_home.exists(), "polling creates nothing");
    append_record(
        &inbox_file(&sb.glen_home, A.id),
        &inbox_record(A.id, None, "arrived", "2026-10-30T10:00:00Z"),
    );
    expect_message(&mut st, "arrived", FOLLOW);
    st.interrupt();
    assert_eq!(st.wait(T).code(), Some(130));
}

// ---------------------------------------------------------------------------------------------
// AC-325 — reply ids are always present (null when absent) and readers accept them missing.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac325_rpc_replies_reach_the_inbox_with_explicit_nulls() {
    let rb = RpcBox::new();
    let d = data_of(
        &rb.glen(["send", A.handle, "hello", "--wait", "20s", "--json"])
            .run(),
        0,
    );
    let id = job_id(&d);
    rb.assert_quiet();
    let path = rb.bx.glen_home.join(format!("inbox/{}.ndjson", A.id));
    let raw = fs::read_to_string(&path).unwrap();
    let line = raw.lines().next().unwrap();
    assert!(
        line.contains("\"request_id\":null"),
        "§A5 explicit null: {line}"
    );
    let rec: Value = serde_json::from_str(line).unwrap();
    for k in RECORD_KEYS {
        assert!(rec.get(k).is_some(), "inbox record lacks `{k}`: {rec}");
    }
    assert_eq!(rec["job_id"], id.as_str());

    let inb = data_of(&rb.glen(["inbox", "--json"]).run(), 0);
    let m = &arr(&inb["messages"])[0];
    assert_eq!(m["job_handle"], job_handle(&id), "{m}");
    assert_eq!(m["session_handle"], A.handle, "{m}");
    assert_eq!(m["text"], "reply: hello", "{m}");
}

#[test]
fn ac325_records_missing_the_optional_ids_are_read_as_nulls() {
    let sb = Sandbox::new();
    let rec = json!({"v":1,"id":"r-lean","session_id":A.id,"text":"lean","final":true,"at":"2026-10-30T10:00:00Z"});
    append_record(&inbox_file(&sb.glen_home, A.id), &rec);
    let d = inbox(&sb, &[]);
    assert_eq!(
        d["skipped_lines"], 0,
        "a record without the ids is not malformed: {d}"
    );
    let m = &arr(&d["messages"])[0];
    for k in ["job_id", "request_id", "tool_call_id", "job_handle"] {
        assert!(
            m.get(k).is_some_and(Value::is_null),
            "`{k}` is present as null: {m}"
        );
    }
    assert_eq!(m["text"], "lean", "{m}");
}

#[test]
fn ac319_a_session_tail_shorter_than_four_hex_digits_is_usage() {
    let sb = Sandbox::new();
    two_sessions(&sb.glen_home);
    for follow in [false, true] {
        let mut args = vec!["inbox", "--session", "9a7"];
        if follow {
            args.push("-f");
        }
        args.push("--json");
        let out = sb.glen(&args).run();
        failed(&out, "usage");
        assert!(
            !sb.glen_home.join("inbox/.read").exists(),
            "follow={follow}: nothing was read or marked"
        );
    }
    // Four digits and full ids stay accepted.
    assert_eq!(
        texts(&inbox(&sb, &["--session", "9a72", "--peek"])),
        ["second", "fourth"]
    );
    assert_eq!(
        texts(&inbox(&sb, &["--session", D.id, "--peek"])),
        ["second", "fourth"]
    );
}
