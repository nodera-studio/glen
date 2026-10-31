mod common;
use common::{OmpBox, data, poll};
use serde_json::{Value, json as j};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    process::Stdio,
    time::Duration,
};
fn closed_stdout() -> Stdio {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    Stdio::from(writer)
}
const A: &str = common::A_ID;
const B: &str = "01a0f6b1-2680-7a3c-8b21-4a1f9e2fffff";
fn record(id: &str, text: &str, at: &str) -> Value {
    j!({"v":1,"id":format!("r-{text}"),"session_id":id,"job_id":null,"request_id":null,"tool_call_id":null,"text":text,"final":true,"at":at})
}
fn path(b: &OmpBox, id: &str) -> std::path::PathBuf {
    b.glen_home.join("inbox").join(format!("{id}.ndjson"))
}
fn append(b: &OmpBox, id: &str, r: &Value) {
    let p = path(b, id);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    writeln!(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .unwrap(),
        "{r}"
    )
    .unwrap();
}
fn inbox(b: &OmpBox, flags: &[&str]) -> std::process::Output {
    let mut a = vec!["inbox", "--json"];
    a.extend_from_slice(flags);
    b.command(&a, &b.alpha()).output().unwrap()
}
#[test]
fn ac319_unread_all_peek_filters_and_order() {
    let b = OmpBox::new();
    append(&b, A, &record(A, "later", "2026-10-30T01:00:00.001Z"));
    append(&b, B, &record(B, "first", "2026-10-30T01:00:00.000000001Z"));
    let d = data(&inbox(&b, &["--peek"]));
    assert_eq!(d["total"], 2);
    assert_eq!(d["messages"][0]["text"], "first");
    assert_eq!(d["messages"][0]["job_handle"], Value::Null);
    assert!(!b.glen_home.join("inbox/.read").exists());
    let d = data(&inbox(&b, &[]));
    assert_eq!(d["marked_read"], true);
    assert_eq!(data(&inbox(&b, &[]))["total"], 0);
    assert_eq!(data(&inbox(&b, &["--all"]))["total"], 2);
    assert_eq!(data(&inbox(&b, &["--all", "--session", A]))["total"], 1);
    assert_eq!(
        data(&inbox(&b, &["--all", "--session", "ffff"]))["messages"][0]["text"],
        "first"
    );
    let cursor = b.glen_home.join("inbox/.read").join(A);
    assert_eq!(
        fs::metadata(cursor).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(b.glen_home.join("inbox/.read"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}
#[test]
fn ac319_partial_malformed_truncate_and_inode() {
    let b = OmpBox::new();
    append(&b, A, &record(A, "one", "invalid"));
    let p = path(&b, A);
    let mut f = fs::OpenOptions::new().append(true).open(&p).unwrap();
    write!(f, "bad\n{}", record(A, "partial", "2026-10-30T01:00:00Z")).unwrap();
    let d = data(&inbox(&b, &[]));
    assert_eq!(d["total"], 1);
    assert_eq!(d["skipped_lines"], 1);
    let cursor = b.glen_home.join("inbox/.read").join(A);
    let before: Value = serde_json::from_slice(&fs::read(&cursor).unwrap()).unwrap();
    assert!(before["offset"].as_u64().unwrap() < fs::metadata(&p).unwrap().len());
    writeln!(f).unwrap();
    assert_eq!(data(&inbox(&b, &[]))["messages"][0]["text"], "partial");
    fs::write(
        &p,
        format!("{}\n", record(A, "new", "2026-10-30T00:00:00Z")),
    )
    .unwrap();
    assert_eq!(data(&inbox(&b, &[]))["messages"][0]["text"], "new");
    let replacement = p.with_extension("tmp");
    fs::write(
        &replacement,
        format!("{}\n", record(A, "replace", "2026-10-30T00:00:00Z")),
    )
    .unwrap();
    fs::rename(replacement, p).unwrap();
    assert_eq!(data(&inbox(&b, &[]))["messages"][0]["text"], "replace");
}
#[test]
fn ac319_missing_home_is_lazy_and_human_is_sanitized() {
    let b = OmpBox::new();
    assert_eq!(data(&inbox(&b, &[]))["total"], 0);
    assert!(!b.glen_home.exists());
    append(&b, A, &record(A, "text\u{1b}[31m", "2026-10-30T00:00:00Z"));
    let o = b
        .command(&["inbox", "--human", "--peek"], &b.alpha())
        .output()
        .unwrap();
    assert!(o.status.success());
    assert!(!o.stdout.contains(&27));
    let h = String::from_utf8(o.stdout).unwrap();
    assert!(h.contains("↩ e27c0de"));
    assert!(h.contains("1 message · 1 session · peek"));
}
#[test]
fn ac319_output_failure_keeps_cursor() {
    let b = OmpBox::new();
    for i in 0..300 {
        append(
            &b,
            A,
            &record(
                A,
                &format!("{i}{}", "x".repeat(500)),
                "2026-10-30T00:00:00Z",
            ),
        );
    }
    let o = b
        .command(&["inbox", "--json"], &b.alpha())
        .env("GLEN_MAX_OUTPUT", "0")
        .stdout(closed_stdout())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stderr.is_empty());
    assert!(!b.glen_home.join("inbox/.read").exists());
}
#[test]
fn ac320_follow_backlog_new_files_peek_and_sigint() {
    for peek in [false, true] {
        let b = OmpBox::new();
        append(&b, A, &record(A, "backlog", "2026-10-30T00:00:00Z"));
        let mut args = vec!["inbox", "-f", "--json"];
        if peek {
            args.push("--peek");
        }
        let mut c = b
            .command(&args, &b.alpha())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = c.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                let _ = tx.send(serde_json::from_str::<Value>(&line).unwrap());
            }
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(3)).unwrap()["message"]["text"],
            "backlog"
        );
        append(&b, A, &record(A, "appended", "2026-10-30T00:00:01Z"));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap()["message"]["text"],
            "appended"
        );
        append(&b, B, &record(B, "new session", "2026-10-30T00:00:02Z"));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap()["message"]["text"],
            "new session"
        );
        let pid = rustix::process::Pid::from_raw(c.id() as i32).unwrap();
        rustix::process::kill_process(pid, rustix::process::Signal::INT).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            j!({"v":1,"event":"end","ok":true})
        );
        let status = poll(Duration::from_secs(2), || c.try_wait().unwrap());
        assert_eq!(status.code(), Some(130));
        assert_eq!(b.glen_home.join("inbox/.read").exists(), !peek);
    }
}
#[test]
fn ac325_rpc_reply_nulls() {
    let b = common::RpcBox::new();
    let o = b
        .glen(&["send", "e27c0de", "hi", "--json"])
        .env("FAKE_OMP_SCENARIO", "reply")
        .output()
        .unwrap();
    let d = data(&o);
    let id = d["job"]["id"].as_str().unwrap();
    b.wait_worker_gone(id);
    let lines = fs::read_to_string(path(&b.omp, A)).unwrap();
    let r: Value = serde_json::from_str(lines.lines().next().unwrap()).unwrap();
    assert!(r.get("request_id").unwrap().is_null());
}

#[test]
fn mark_read_failure_is_reported_on_stderr_and_leaves_messages_unread() {
    let b = OmpBox::new();
    append(&b, A, &record(A, "keep", "2026-10-30T00:00:00Z"));
    fs::write(b.glen_home.join("inbox/.read"), "blocked").unwrap();
    let o = inbox(&b, &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains(&format!("cannot mark inbox {A} read")));
    assert_eq!(data(&inbox(&b, &["--peek"]))["total"], 1);
}

#[test]
fn footer_counts_only_sessions_with_displayed_messages() {
    let b = OmpBox::new();
    append(&b, A, &record(A, "one", "2026-10-30T00:00:00Z"));
    append(&b, B, &record(B, "read", "2026-10-30T00:00:00Z"));
    inbox(&b, &["--session", B]);
    let o = b
        .command(&["inbox", "--human", "--peek"], &b.alpha())
        .output()
        .unwrap();
    assert!(
        String::from_utf8(o.stdout)
            .unwrap()
            .contains("1 message · 1 session · peek")
    );
    append(&b, B, &record(B, "two", "2026-10-30T00:00:01Z"));
    let o = b
        .command(&["inbox", "--human", "--peek"], &b.alpha())
        .output()
        .unwrap();
    assert!(
        String::from_utf8(o.stdout)
            .unwrap()
            .contains("2 messages · 2 sessions · peek")
    );
}

#[test]
fn closed_stdout_does_not_mark_inbox_read() {
    let b = OmpBox::new();
    append(&b, A, &record(A, "keep", "2026-10-30T00:00:00Z"));
    let o = b
        .command(&["inbox", "--json"], &b.alpha())
        .stdout(closed_stdout())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert!(!b.glen_home.join("inbox/.read").exists());
    assert!(o.stderr.is_empty());
}

#[test]
fn inbox_with_a_closed_stdout_keeps_messages_unread() {
    for follow in [false, true] {
        let b = OmpBox::new();
        append(&b, A, &record(A, "keep", "2026-10-30T00:00:00Z"));
        let args = if follow {
            vec!["inbox", "--json", "-f"]
        } else {
            vec!["inbox", "--json"]
        };
        let mut child = b
            .command(&args, &b.alpha())
            .stdout(closed_stdout())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("inbox did not stop after EPIPE: {output:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let o = child.wait_with_output().unwrap();
        assert_eq!(o.status.code(), Some(0), "{o:?}");
        assert!(o.stderr.is_empty(), "{o:?}");
        assert!(!b.glen_home.join("inbox/.read").exists());
        let remaining = data(&inbox(&b, &["--peek"]));
        assert_eq!(remaining["total"], 1);
        assert_eq!(remaining["messages"][0]["text"], "keep");
        assert!(!b.glen_home.join("inbox/.read").exists());
    }
}

#[test]
fn inbox_rejects_hex_suffixes_shorter_than_four_digits() {
    let b = OmpBox::new();
    for suffix in ["a", "AB", "abc"] {
        for mode in ["--peek", "--follow"] {
            let o = inbox(&b, &["--session", suffix, mode]);
            assert_eq!(o.status.code(), Some(2), "{o:?}");
            assert_eq!(common::json(&o)["error"]["code"], "usage");
            assert!(o.stderr.is_empty());
        }
    }
    assert_eq!(data(&inbox(&b, &["--session", "abcd"]))["total"], 0);
    assert!(!b.glen_home.exists());
}
