mod common;
use common::{OmpBox, data, json};
use serde_json::{Value, json as j};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    process::{Child, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};
struct Following {
    child: Child,
    lines: Receiver<String>,
}
impl Following {
    fn start(sb: &OmpBox, args: &[&str]) -> Self {
        let mut child = sb
            .command(args, &sb.alpha())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pipe = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for l in BufReader::new(pipe).lines() {
                let Ok(l) = l else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        Self { child, lines: rx }
    }
    fn line(&self) -> String {
        self.lines
            .recv_timeout(Duration::from_secs(10))
            .expect("stream line within 10s")
    }
    fn event(&self) -> Value {
        let line = self.line();
        assert!(!line.contains('\x1b'));
        serde_json::from_str(&line).unwrap()
    }
    fn backlog(&self) {
        assert_eq!(self.event()["event"], "session");
        for _ in 0..5 {
            assert_eq!(self.event()["event"], "item");
        }
    }
    fn wait(&mut self) -> std::process::ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return s;
            }
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "child failed to exit"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Following {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            self.child.kill().unwrap();
            self.wait();
        }
    }
}
fn message(id: &str, parent: &str, text: &str) -> String {
    format!(
        "{}\n",
        j!({"type":"message","id":id,"parentId":parent,"timestamp":"2026-10-10T00:00:00Z","message":{"role":"user","content":text}})
    )
}
fn append(p: &std::path::Path, s: &str) {
    let mut f = OpenOptions::new().append(true).open(p).unwrap();
    f.write_all(s.as_bytes()).unwrap();
    f.flush().unwrap();
}
#[test]
fn tail_follow_with_a_closed_stdout_exits_0_silently() {
    for close_after_backlog in [false, true] {
        let sb = OmpBox::new();
        let mut command = sb.command(&["tail", "e27c0de", "-f", "--json"], &sb.alpha());
        if close_after_backlog {
            command.stdout(Stdio::piped());
        } else {
            let (reader, writer) = std::io::pipe().unwrap();
            drop(reader);
            command.stdout(Stdio::from(writer));
        }
        let mut child = command.stderr(Stdio::piped()).spawn().unwrap();
        if close_after_backlog {
            let stdout = child.stdout.take().unwrap();
            let (tx, rx) = mpsc::channel();
            let reader = thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                for index in 0..6 {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let event: Value = serde_json::from_str(&line).unwrap();
                    assert_eq!(event["event"], if index == 0 { "session" } else { "item" });
                }
                drop(reader);
                tx.send(()).unwrap();
            });
            if let Err(error) = rx.recv_timeout(Duration::from_secs(10)) {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("tail backlog did not arrive: {error}: {output:?}");
            }
            reader.join().unwrap();
            append(
                &sb.session('A'),
                &message("10000010", "10000009", "after reader closed"),
            );
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("tail did not stop after EPIPE: {output:?}");
            }
            thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}
#[test]
fn ac116_tail_documents() {
    let sb = OmpBox::new();
    let d = data(&sb.glen(&["tail", "e27c0de", "-n", "2"], &sb.alpha()));
    assert_eq!(
        d["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["assistant", "assistant"]
    );
    assert_eq!(
        data(&sb.glen(&["tail", "e27c0de", "-n", "0"], &sb.alpha()))["items"],
        j!([])
    );
    sb.clean();
}
#[test]
fn ac117_118_append_partial_continuation() {
    let sb = OmpBox::new();
    let p = sb.session('A');
    let f = Following::start(&sb, &["tail", p.to_str().unwrap(), "-f", "--json"]);
    f.backlog();
    append(&p, &message("10000010", "10000009", "more"));
    assert_eq!(f.event()["item"]["id"], "10000010");
    let line = message("10000011", "10000010", "partial");
    let half = line.len() / 2;
    append(&p, &line[..half]);
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    append(&p, &line[half..]);
    assert_eq!(f.event()["item"]["id"], "10000011");
    let old = "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de";
    let new = "01a0f6b2-2680-7a3c-8b21-4a1f9e27c0df";
    let content = fs::read_to_string(&p).unwrap().replace(
        &format!("\"id\":\"{old}\""),
        &format!("\"id\":\"{new}\",\"continuationOf\":\"{old}\""),
    ) + &message("10000012", "10000011", "continued")
        // omp keeps the starting copy on equal activity timestamps. This child
        // must contain a newer turn to exercise continuation following.
        .replace("2026-10-10T00:00:00Z", "2026-10-10T01:00:00Z");
    let next = p
        .parent()
        .unwrap()
        .join(format!("2026-10-10T01-00-00-000Z_{new}.jsonl"));
    fs::write(&next, content).unwrap();
    let e = f.event();
    assert_eq!(e["event"], "continued");
    assert_eq!(e["from"], old);
    assert_eq!(e["to"], new);
    assert_eq!(f.event()["item"]["id"], "10000012");
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    sb.clean();
}
#[test]
fn ac125_rename_replace() {
    let sb = OmpBox::new();
    let p = sb.session('A');
    let f = Following::start(&sb, &["tail", p.to_str().unwrap(), "-f", "--json"]);
    f.backlog();
    let content = fs::read_to_string(&p).unwrap()
        + &message("10000010", "10000009", "one")
        + &message("10000011", "10000010", "two");
    let stage = p.with_file_name(".rewrite.tmp");
    fs::write(&stage, content).unwrap();
    fs::rename(stage, &p).unwrap();
    assert_eq!(f.event()["event"], "rewritten");
    for id in ["10000010", "10000011"] {
        assert_eq!(f.event()["item"]["id"], id)
    }
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    sb.clean();
}
#[test]
fn same_inode_rewrites_at_or_above_the_old_length_emit_rewritten_then_items() {
    for grows in [false, true] {
        let sb = OmpBox::new();
        let path = sb.session('A');
        let f = Following::start(&sb, &["tail", path.to_str().unwrap(), "-f", "--json"]);
        f.backlog();
        let before = fs::metadata(&path).unwrap();
        let mut content = fs::read_to_string(&path)
            .unwrap()
            .replace("10000007", "90000007");
        if grows {
            content.push_str(&message("10000010", "10000009", "after in-place rewrite"));
        }
        assert!(content.len() as u64 >= before.len());
        fs::write(&path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(before.ino(), fs::metadata(&path).unwrap().ino());
        }
        assert_eq!(f.event()["event"], "rewritten");
        assert_eq!(f.event()["item"]["id"], "90000007");
        if grows {
            assert_eq!(f.event()["item"]["id"], "10000010");
        }
        assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
        sb.clean();
    }
}
#[test]
fn equal_size_early_prefix_rewrite_is_not_hidden_by_the_unchanged_tail_window() {
    let sb = OmpBox::new();
    let path = sb.session('A');
    let original = fs::read_to_string(&path).unwrap()
        + &format!(
            "{}\n",
            j!({"type":"future", "id":"padding", "parentId":"10000009", "data":"x".repeat(9000)})
        );
    fs::write(&path, &original).unwrap();
    let f = Following::start(&sb, &["tail", path.to_str().unwrap(), "-f", "--json"]);
    f.backlog();
    let before = fs::metadata(&path).unwrap();
    let rewritten = original
        .replace("10000003", "90000003")
        .replace("Why does parser_test flake?", "prefix rewrite detected    ");
    assert_eq!(original.len(), rewritten.len());
    assert_eq!(
        &original.as_bytes()[original.len() - 4096..],
        &rewritten.as_bytes()[rewritten.len() - 4096..]
    );
    fs::write(&path, rewritten).unwrap();
    fs::File::open(&path)
        .unwrap()
        .set_modified(before.modified().unwrap() + Duration::from_secs(1))
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(before.ino(), fs::metadata(&path).unwrap().ino());
    }
    assert_eq!(f.event()["event"], "rewritten");
    let item = f.event();
    assert_eq!(item["item"]["id"], "90000003");
    assert_eq!(item["item"]["text"], "prefix rewrite detected    ");
    append(
        &path,
        &message("after-prefix", "padding", "normal append after rewrite"),
    );
    assert_eq!(f.event()["item"]["id"], "after-prefix");
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    drop(f);
    sb.clean();
}
#[test]
fn v1_rewrite_removes_stale_occurrences_before_a_genuinely_new_duplicate() {
    let sb = OmpBox::new();
    let path = sb.alpha().join("legacy.jsonl");
    let header = include_str!("fixtures/v1-session.jsonl")
        .lines()
        .next()
        .unwrap();
    let a = format!(
        "{}\n",
        j!({"type":"message", "message":{"role":"user", "content":"old prompt"}})
    );
    let b = a.replace("old prompt", "another prompt");
    fs::write(&path, format!("{header}\n{a}{a}{b}")).unwrap();
    let f = Following::start(&sb, &["tail", path.to_str().unwrap(), "-f", "--json"]);
    assert_eq!(f.event()["event"], "session");
    for text in ["old prompt", "old prompt", "another prompt"] {
        assert_eq!(f.event()["item"]["text"], text);
    }
    fs::write(&path, format!("{header}\n{a}{b}")).unwrap();
    assert_eq!(f.event()["event"], "rewritten");
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    append(&path, &a);
    assert_eq!(f.event()["item"]["text"], "old prompt");
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    drop(f);
    sb.clean();
}

#[test]
fn v1_rewrite_with_shifted_ordinals_emits_only_new_content() {
    let sb = OmpBox::new();
    let path = sb.alpha().join("legacy.jsonl");
    let original = include_str!("fixtures/v1-session.jsonl");
    fs::write(&path, original).unwrap();
    let f = Following::start(&sb, &["tail", path.to_str().unwrap(), "-f", "--json"]);
    assert_eq!(f.event()["event"], "session");
    assert_eq!(f.event()["item"]["text"], "old prompt");
    assert_eq!(f.event()["item"]["text"], "old answer");
    let (header, entries) = original.split_once('\n').unwrap();
    let fresh = "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"fresh after rewrite\"}}\n";
    let stage = path.with_extension("staged");
    fs::write(
        &stage,
        format!("{header}\n{{\"type\":\"future_metadata\"}}\n{entries}{fresh}"),
    )
    .unwrap();
    fs::rename(stage, &path).unwrap();
    assert_eq!(f.event()["event"], "rewritten");
    assert_eq!(f.event()["item"]["text"], "fresh after rewrite");
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    append(&path, &(original.lines().nth(1).unwrap().to_owned() + "\n"));
    assert_eq!(f.event()["item"]["text"], "old prompt");
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    sb.clean();
}
#[cfg(unix)]
#[test]
fn ac124_sigint_trailer() {
    let sb = OmpBox::new();
    let mut f = Following::start(&sb, &["tail", "e27c0de", "-f", "--json"]);
    f.backlog();
    let start = Instant::now();
    assert!(
        std::process::Command::new("/bin/kill")
            .args(["-INT", &f.child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(f.event(), j!({"v":1,"event":"end","ok":true}));
    assert_eq!(f.wait().code(), Some(130));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(f.lines.recv_timeout(Duration::from_millis(500)).is_err());
    sb.clean();
}
#[test]
fn ac119_resolution_and_disappearance() {
    let sb = OmpBox::new();
    let o = sb.glen(&["tail", "nope", "-f"], &sb.alpha());
    assert_eq!(o.status.code(), Some(3));
    assert_eq!(json(&o)["error"]["code"], "not_found");
    assert_eq!(String::from_utf8(o.stdout).unwrap().lines().count(), 1);
    let p = sb.session('A');
    let mut f = Following::start(&sb, &["tail", p.to_str().unwrap(), "-f", "--json"]);
    f.backlog();
    fs::remove_file(p).unwrap();
    let e = f.event();
    assert_eq!(e["ok"], false);
    assert_eq!(e["error"]["code"], "not_found");
    assert_eq!(f.wait().code(), Some(3));
    sb.clean();
}
#[test]
fn human_follow_and_zero_backlog() {
    let sb = OmpBox::new();
    let p = sb.session('A');
    let f = Following::start(
        &sb,
        &["tail", p.to_str().unwrap(), "-n", "0", "-f", "--human"],
    );
    assert!(f.line().contains("e27c0de"));
    append(&p, &message("10000010", "10000009", "human more"));
    let start = Instant::now();
    loop {
        let l = f.line();
        if l.contains("❯ user") {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
    }
    assert_eq!(f.line(), "  human more");
    sb.clean();
}
#[test]
fn branch_switch_and_shrink() {
    let sb = OmpBox::new();
    let p = sb.session('A');
    let f = Following::start(&sb, &["tail", p.to_str().unwrap(), "-f", "--json"]);
    f.backlog();
    append(&p, &message("new-branch", "10000003", "different"));
    assert_eq!(f.event()["event"], "branch_switch");
    assert_eq!(f.event()["item"]["id"], "new-branch");
    let session = fs::read_to_string(&p).unwrap();
    let header = session.lines().take(2).collect::<Vec<_>>().join("\n") + "\n";
    fs::write(&p, header + &message("after-shrink", "", "fresh")).unwrap();
    assert_eq!(f.event()["event"], "rewritten");
    assert_eq!(f.event()["item"]["id"], "after-shrink");
    sb.clean();
}
#[test]
fn follow_never_spills_and_broken_pipe() {
    let sb = OmpBox::new();
    let mut c = sb.command(&["tail", "e27c0de", "-f", "--json"], &sb.alpha());
    c.env("GLEN_MAX_OUTPUT", "1");
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let mut child = c.stdout(Stdio::from(writer)).spawn().unwrap();
    let start = Instant::now();
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            assert_eq!(s.code(), Some(0));
            break;
        }
        if start.elapsed() > Duration::from_secs(3) {
            child.kill().unwrap();
            panic!("follower did not stop on broken pipe")
        }
        thread::sleep(Duration::from_millis(20));
    }
    sb.clean();
}

#[test]
fn snapshots_wait_for_newlines_at_start_reload_and_continuation() {
    let sb = OmpBox::new();
    let path = sb.session('A');
    let base = fs::read_to_string(&path).unwrap();
    let pending = message("startup-partial", "10000009", "startup");
    fs::write(&path, base.clone() + pending.trim_end_matches('\n')).unwrap();
    // Single-document parsing remains lenient.
    let doc = data(&sb.glen(&["show", path.to_str().unwrap()], &sb.alpha()));
    assert_eq!(
        doc["items"].as_array().unwrap().last().unwrap()["id"],
        "startup-partial"
    );
    let f = Following::start(&sb, &["tail", path.to_str().unwrap(), "-f", "--json"]);
    f.backlog();
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    append(&path, "\n");
    assert_eq!(f.event()["item"]["id"], "startup-partial");

    let partial = message("reload-partial", "startup-partial", "rewrite");
    let staged = path.with_file_name(".partial-rewrite.tmp");
    let content = fs::read_to_string(&path).unwrap() + partial.trim_end_matches('\n');
    fs::write(&staged, &content).unwrap();
    fs::rename(staged, &path).unwrap();
    assert_eq!(f.event()["event"], "rewritten");
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    append(&path, "\n");
    assert_eq!(f.event()["item"]["id"], "reload-partial");

    let old = "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de";
    let new = "01a0f6b2-2680-7a3c-8b21-4a1f9e27c0df";
    let partial = message("continue-partial", "reload-partial", "continued")
        .replace("2026-10-10T00:00:00Z", "2026-10-11T00:00:00Z");
    let content = fs::read_to_string(&path).unwrap().replace(
        &format!("\"id\":\"{old}\""),
        &format!("\"id\":\"{new}\",\"continuationOf\":\"{old}\""),
    ) + partial.trim_end_matches('\n');
    let next = path
        .parent()
        .unwrap()
        .join(format!("2026-10-11T00-00-00-000Z_{new}.jsonl"));
    fs::write(&next, content).unwrap();
    assert_eq!(f.event()["event"], "continued");
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    append(&next, "\n");
    assert_eq!(f.event()["item"]["id"], "continue-partial");
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
}

#[test]
fn replaced_header_and_garbage_recover_original_continuation() {
    for garbage in [false, true] {
        let sb = OmpBox::new();
        let path = sb.session('A');
        let original = fs::read_to_string(&path).unwrap();
        let f = Following::start(&sb, &["tail", path.to_str().unwrap(), "-f", "--json"]);
        f.backlog();
        let staged = path.with_file_name(".replacement.tmp");
        fs::write(
            &staged,
            if garbage {
                "garbage\n".into()
            } else {
                original.replace("01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de", "unrelated-session")
                    + &message("unrelated-entry", "10000009", "must never emit")
            },
        )
        .unwrap();
        fs::rename(staged, &path).unwrap();
        assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
        let old = "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de";
        let new = "01a0f6b2-2680-7a3c-8b21-4a1f9e27c0df";
        let content = original.replace(
            &format!("\"id\":\"{old}\""),
            &format!("\"id\":\"{new}\",\"continuationOf\":\"{old}\""),
        ) + &message("recovered-entry", "10000009", "recovered");
        let next = path
            .parent()
            .unwrap()
            .join(format!("2026-10-10T00-00-00-000Z_{new}.jsonl"));
        fs::write(next, content).unwrap();
        let e = f.event();
        assert_eq!(e["event"], "continued");
        assert_eq!(e["from"], old);
        assert_eq!(e["to"], new);
        assert_eq!(f.event()["item"]["id"], "recovered-entry");
        assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    }
}

#[test]
fn replacement_without_continuation_observes_five_second_grace() {
    for garbage in [false, true] {
        let sb = OmpBox::new();
        let path = sb.session('A');
        let mut f = Following::start(&sb, &["tail", path.to_str().unwrap(), "-f", "--json"]);
        f.backlog();
        let staged = path.with_file_name(".replacement.tmp");
        fs::write(
            &staged,
            if garbage {
                "garbage\n"
            } else {
                "{\"type\":\"session\",\"id\":\"different\"}\n"
            },
        )
        .unwrap();
        let start = Instant::now();
        fs::rename(staged, &path).unwrap();
        let e = f.event();
        assert!(start.elapsed() >= Duration::from_secs(5));
        assert_eq!(e["error"]["code"], "not_found");
        assert!(
            e["error"]["message"]
                .as_str()
                .unwrap()
                .contains(path.to_str().unwrap())
        );
        assert_eq!(f.wait().code(), Some(3));
    }
}

#[test]
fn removed_bucket_uses_disappearance_grace_and_path_error() {
    let sb = OmpBox::new();
    let path = sb.session('A');
    let mut f = Following::start(&sb, &["tail", path.to_str().unwrap(), "-f", "--json"]);
    f.backlog();
    let bucket = path.parent().unwrap();
    let start = Instant::now();
    fs::rename(bucket, bucket.with_file_name("removed-bucket")).unwrap();
    let e = f.event();
    assert!(start.elapsed() >= Duration::from_secs(5));
    assert_eq!(e["error"]["code"], "not_found");
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap()
            .contains(path.to_str().unwrap())
    );
    assert_eq!(f.wait().code(), Some(3));
}

#[test]
fn session_precedes_environment_warnings() {
    let sb = OmpBox::new();
    let mut child = sb
        .command(&["tail", "e27c0de", "-f", "--json", "-n", "0"], &sb.alpha())
        .env("GLEN_MAX_OUTPUT", "invalid")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pipe = child.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    thread::spawn(move || {
        for l in BufReader::new(pipe).lines() {
            let Ok(l) = l else { break };
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    let f = Following { child, lines };
    assert_eq!(f.event()["event"], "session");
    let warning = f.event();
    assert_eq!(warning["event"], "warning");
    assert_eq!(warning["code"], "invalid_env");
}

#[test]
fn rejected_in_place_reload_stays_rejected_after_regrowth() {
    let sb = OmpBox::new();
    let path = sb.session('A');
    let original = fs::read_to_string(&path).unwrap();
    let f = Following::start(&sb, &["tail", path.to_str().unwrap(), "-f", "--json"]);
    f.backlog();
    fs::write(&path, "garbage\n").unwrap();
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    let unrelated = original.replace(
        "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
        "00000000-0000-0000-0000-000000000000",
    ) + &message("unrelated-regrown", "10000009", "must never emit");
    fs::write(&path, unrelated).unwrap();
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
    fs::write(
        &path,
        original + &message("original-restored", "10000009", "safe"),
    )
    .unwrap();
    assert_eq!(f.event()["event"], "rewritten");
    assert_eq!(f.event()["item"]["id"], "original-restored");
    assert!(f.lines.recv_timeout(Duration::from_millis(600)).is_err());
}

#[test]
fn cwd_scopes_latest_and_follow_but_never_rebases_explicit_paths() {
    let sb = OmpBox::new();
    let alpha = sb.alpha();
    let beta = sb.beta();
    let foreign = beta.join("nested");
    fs::create_dir(&foreign).unwrap();
    let a = sb.session('A');
    let relative = format!(
        "../../../agent/sessions/-work-alpha/{}",
        a.file_name().unwrap().to_str().unwrap()
    );
    for command in ["show", "tail"] {
        let latest = sb.glen(
            &[
                command,
                "@latest",
                "--cwd",
                beta.to_str().unwrap(),
                "--json",
            ],
            &alpha,
        );
        assert_eq!(
            data(&latest)["session"]["path"],
            sb.session('E').to_str().unwrap()
        );
        for arg in [a.to_str().unwrap(), &relative] {
            let out = sb.glen(
                &[command, arg, "--cwd", foreign.to_str().unwrap(), "--json"],
                &alpha,
            );
            assert_eq!(data(&out)["session"]["id"], common::A_ID);
        }
    }
    let outside = sb.glen(
        &[
            "tail",
            "0a3f417",
            "-f",
            "--cwd",
            alpha.to_str().unwrap(),
            "--json",
        ],
        &beta,
    );
    assert_eq!(outside.status.code(), Some(3));
    assert_eq!(json(&outside)["error"]["details"]["reason"], "outside_cwd");
    let mut following = Following::start(
        &sb,
        &[
            "tail",
            "@latest",
            "-f",
            "--cwd",
            beta.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(
        following.event()["session"]["path"],
        sb.session('E').to_str().unwrap()
    );
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(following.child.id() as i32).unwrap(),
        rustix::process::Signal::INT,
    )
    .unwrap();
    assert_eq!(
        common::poll(Duration::from_secs(10), || following
            .child
            .try_wait()
            .unwrap())
        .code(),
        Some(130)
    );
    sb.clean();
}
