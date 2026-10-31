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

/// W8-ERR-1 re-derived this test: an unwritable cursor is now caught before any output, so
/// the failure is the one JSON error envelope on stdout (stderr stays empty), not `ok:true`
/// followed by a stderr line.
#[test]
fn mark_read_failure_is_an_error_envelope_and_leaves_messages_unread() {
    let b = OmpBox::new();
    append(&b, A, &record(A, "keep", "2026-10-30T00:00:00Z"));
    fs::write(b.glen_home.join("inbox/.read"), "blocked").unwrap();
    let o = inbox(&b, &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(o.stderr.is_empty(), "{o:?}");
    let v = common::json(&o);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "io");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&format!("cannot mark inbox {A} read")),
        "{v}"
    );
    assert_eq!(data(&inbox(&b, &["--peek"]))["total"], 1);
}

/// W8-ERR-1: inside Codex's sandbox an unwritable GLEN_HOME is exit 6 `sandboxed`, and the
/// messages stay unread. `.read` does not exist yet, so the probe must catch a read-only
/// `inbox/` where `mark` would create it.
#[test]
fn sandboxed_mark_read_failure_is_exit_six_and_leaves_messages_unread() {
    if rustix::process::geteuid().is_root() {
        return; // root ignores the read-only mode this test relies on
    }
    let b = OmpBox::new();
    append(&b, A, &record(A, "keep", "2026-10-30T00:00:00Z"));
    let dir = b.glen_home.join("inbox");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let o = b
        .command(&["inbox", "--json"], &b.alpha())
        .env("CODEX_SANDBOX", "seatbelt")
        .output()
        .unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(o.status.code(), Some(6), "{o:?}");
    assert!(o.stderr.is_empty(), "{o:?}");
    let v = common::json(&o);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["details"]["reason"], "sandboxed", "{v}");
    assert_eq!(v["error"]["details"]["glen_home_writable"], false, "{v}");
    assert!(!dir.join(".read").exists());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no probe file left");
    let d = data(&inbox(&b, &["--peek"]));
    assert_eq!(d["total"], 1);
    assert_eq!(d["messages"][0]["text"], "keep");
}

/// W8-DOC-7: `marked_read` is true only when a cursor moves; `--all` after everything was
/// read returns the old messages without moving anything.
#[test]
fn marked_read_is_false_when_no_cursor_moves() {
    let b = OmpBox::new();
    append(&b, A, &record(A, "one", "2026-10-30T00:00:00Z"));
    append(&b, B, &record(B, "two", "2026-10-30T00:00:01Z"));
    let d = data(&inbox(&b, &[]));
    assert_eq!(d["marked_read"], true);
    let cursor = b.glen_home.join("inbox/.read").join(A);
    let before = fs::read(&cursor).unwrap();
    let d = data(&inbox(&b, &["--all"]));
    assert_eq!(d["total"], 2);
    assert_eq!(d["marked_read"], false);
    assert_eq!(fs::read(&cursor).unwrap(), before);
    let d = data(&inbox(&b, &[]));
    assert_eq!(
        (d["total"].clone(), d["marked_read"].clone()),
        (j!(0), j!(false))
    );
    append(&b, B, &record(B, "three", "2026-10-30T00:00:02Z"));
    let d = data(&inbox(&b, &["--all"]));
    assert_eq!(d["total"], 3);
    assert_eq!(d["marked_read"], true);
    assert_eq!(data(&inbox(&b, &[]))["total"], 0);
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

fn fixture_session_id(b: &OmpBox, key: char) -> String {
    glen::omp::session::Session::load(&b.session(key))
        .unwrap()
        .header
        .id
}

#[test]
fn cwd_inbox_intersects_session_filters_and_leaves_foreign_and_orphan_replies_unread() {
    let b = OmpBox::new();
    let beta = fixture_session_id(&b, 'E');
    let registry_alpha = "registry-alpha";
    let registry_unknown = "registry-unknown";
    for (id, text) in [
        (A, "alpha"),
        (beta.as_str(), "beta"),
        (B, "orphan"),
        (registry_alpha, "live alpha"),
        (registry_unknown, "unknown project"),
    ] {
        append(&b, id, &record(id, text, "2026-10-30T00:00:00Z"));
    }
    let home = glen::glen_home::GlenHome {
        root: b.glen_home.clone(),
    };
    fs::create_dir_all(glen::live::run_dir(&home)).unwrap();
    for (id, cwd) in [(registry_alpha, Some(b.alpha())), (registry_unknown, None)] {
        fs::write(glen::live::registry::path(&home, id), j!({"v":1,"pid":std::process::id(),"session_id":id,"socket":glen::live::run_dir(&home).join(format!("{id}.sock")),"cwd":cwd}).to_string()).unwrap();
    }
    let alpha = b.alpha();
    let flags = ["--cwd", alpha.to_str().unwrap()];
    let prefix = inbox(
        &b,
        &[
            "--cwd",
            alpha.to_str().unwrap(),
            "--session",
            "01a0f6b1",
            "--peek",
        ],
    );
    assert_eq!(data(&prefix)["total"], 1);
    assert_eq!(data(&prefix)["messages"][0]["session_id"], A);
    let legacy = inbox(
        &b,
        &[
            "--cwd",
            alpha.to_str().unwrap(),
            "--session",
            "registr",
            "--peek",
        ],
    );
    assert_eq!(data(&legacy)["total"], 1);
    assert_eq!(data(&legacy)["messages"][0]["session_id"], registry_alpha);
    let selected = data(&inbox(&b, &["--cwd", alpha.to_str().unwrap(), "--peek"]));
    assert_eq!(selected["total"], 2);
    assert!(
        selected["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message["session_id"] == A || message["session_id"] == registry_alpha)
    );
    assert_eq!(data(&inbox(&b, &flags))["total"], 2);
    for id in [&beta, B, registry_unknown] {
        assert!(!b.glen_home.join("inbox/.read").join(id).exists());
    }
    let remaining = data(&inbox(&b, &["--peek"]));
    assert_eq!(remaining["total"], 3);
    let outside = inbox(&b, &["--cwd", alpha.to_str().unwrap(), "--session", &beta]);
    assert_eq!(data(&outside)["total"], 0);
    assert_eq!(common::json(&outside)["warnings"][0]["code"], "outside_cwd");
    let included = inbox(
        &b,
        &[
            "--cwd",
            alpha.to_str().unwrap(),
            "--session",
            A,
            "--all",
            "--peek",
        ],
    );
    assert_eq!(data(&included)["total"], 1);
    assert!(common::json(&included).get("warnings").is_none());
}

struct ScopedInboxChild(std::process::Child);
impl Drop for ScopedInboxChild {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}

#[test]
fn cwd_inbox_follow_discovers_new_sessions_and_reclassifies_an_orphan_within_ten_seconds() {
    let b = OmpBox::new();
    let beta = fixture_session_id(&b, 'E');
    for (id, text) in [(A, "alpha"), (beta.as_str(), "beta"), (B, "orphan")] {
        append(&b, id, &record(id, text, "2026-10-30T00:00:00Z"));
    }
    let mut child = ScopedInboxChild(
        b.command(
            &[
                "inbox",
                "-f",
                "--cwd",
                b.alpha().to_str().unwrap(),
                "--json",
            ],
            &b.alpha(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let event: Value = serde_json::from_str(&line.unwrap()).unwrap();
            if tx.send(event).is_err() {
                break;
            }
        }
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap()["message"]["text"],
        "alpha"
    );
    assert!(matches!(
        rx.recv_timeout(Duration::from_millis(300)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    let new = "00000000-0000-7000-8000-000009876abc";
    append(&b, new, &record(new, "new alpha", "2026-10-30T00:00:01Z"));
    assert!(matches!(
        rx.recv_timeout(Duration::from_millis(300)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(!b.glen_home.join("inbox/.read").join(new).exists());
    fs::write(
        b.agent
            .join("sessions/-work-alpha")
            .join(format!("{new}.jsonl")),
        format!(
            "{}\n",
            j!({"type":"session","version":3,"id":new,"cwd":b.alpha()})
        ),
    )
    .unwrap();
    let message = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(message["event"], "message");
    assert_eq!(message["message"]["session_id"], new);
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(child.0.id() as i32).unwrap(),
        rustix::process::Signal::INT,
    )
    .unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap()["event"],
        "end"
    );
    assert_eq!(
        poll(Duration::from_secs(10), || child.0.try_wait().unwrap()).code(),
        Some(130)
    );
    reader.join().unwrap();
    let mut err = String::new();
    std::io::Read::read_to_string(&mut child.0.stderr.take().unwrap(), &mut err).unwrap();
    assert!(err.is_empty(), "{err}");
    assert!(b.glen_home.join("inbox/.read").join(A).exists());
    assert!(b.glen_home.join("inbox/.read").join(new).exists());
    assert!(!b.glen_home.join("inbox/.read").join(&beta).exists());
    assert!(!b.glen_home.join("inbox/.read").join(B).exists());
    assert_eq!(data(&inbox(&b, &["--peek"]))["total"], 2);
}
