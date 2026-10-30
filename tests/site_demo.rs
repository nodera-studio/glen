#![cfg(unix)]
mod common;
use common::{A_ID, Holder};
use glen::{
    error::{ErrorCode, GlenError},
    glen_home::GlenHome,
    jobs::{Deliver, Job, JobId, JobResult, JobStore, Reply, Route, State, Status},
};
use serde_json::{Map, Value, json};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime},
};

#[test]
#[ignore]
fn lease_holder() {
    common::hold_lease_from_env()
}

fn invalid(message: impl Into<String>) -> GlenError {
    GlenError::new(ErrorCode::Internal, message)
}

fn copy_demo(from: &Path, to: &Path, home: &Path) -> Result<(), GlenError> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_demo(&entry.path(), &target, home)?;
        } else {
            let content = fs::read_to_string(entry.path())?;
            fs::write(
                target,
                content.replace("/home/fixture", &home.to_string_lossy()),
            )?;
        }
    }
    Ok(())
}

fn guard_paths(stdout: &str, root: &Path) -> Result<(), GlenError> {
    for forbidden in [
        root.to_string_lossy().as_ref(),
        "/Users/",
        "/home/runner",
        "/private/",
    ] {
        if stdout.contains(forbidden) {
            return Err(invalid(format!("demo contains sandbox path: {forbidden}")));
        }
    }
    Ok(())
}

// Keep all stdout, including the header already read before SIGINT. Bound both readiness and exit.
fn follow(command: &mut Command) -> Result<String, GlenError> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid("missing tail stdout"))?;
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || -> Result<String, GlenError> {
        let mut all = String::new();
        let mut notified = false;
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            all.push_str(&line);
            if !notified && !line.trim().is_empty() {
                tx.send(()).map_err(|e| invalid(e.to_string()))?;
                notified = true;
            }
        }
        Ok(all)
    });
    let result = (|| {
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|e| invalid(format!("tail header: {e}")))?;
        let signal = Command::new("/bin/kill")
            .args(["-INT", &child.id().to_string()])
            .status()?;
        if !signal.success() {
            return Err(invalid("could not interrupt tail"));
        }
        let start = Instant::now();
        while child.try_wait()?.is_none() {
            if start.elapsed() >= Duration::from_secs(10) {
                return Err(invalid("tail did not exit after SIGINT"));
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    })();
    if result.is_err() && child.try_wait()?.is_none() {
        child.kill()?;
    }
    let output = child.wait_with_output()?;
    let stdout = reader
        .join()
        .map_err(|_| invalid("tail reader panicked"))??;
    result?;
    assert_eq!(output.status.code(), Some(130), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    Ok(stdout)
}

#[test]
fn demo_capture_matches() -> Result<(), GlenError> {
    let tmp = tempfile::tempdir()?;
    let root = fs::canonicalize(tmp.path())?;
    let home = root.join("home");
    let agent = root.join("agent");
    let glen_home = root.join("glen");
    let tmpdir = root.join("tmpdir");
    let cwd = home.join("work/alpha");
    for path in [&cwd, &agent, &glen_home, &tmpdir] {
        fs::create_dir_all(path)?;
    }
    copy_demo(&common::fixture("demo/agent"), &agent, &home)?;
    let sessions = agent.join("sessions/-work-alpha");
    let owners = home.join(".omp/run/session-owners");
    fs::create_dir_all(&owners)?;
    let lock = owners.join(format!("{A_ID}.lock"));
    fs::write(&lock, b"preserve")?;
    let _lease = Holder::new(&lock);

    let store = JobStore::new(GlenHome {
        root: glen_home.clone(),
    });
    let id = JobId::parse("019a3c00-0000-7000-8000-00000a3c9f41")
        .ok_or_else(|| invalid("invalid demo job id"))?;
    let created = "2026-10-29T12:04:00.000Z";
    let updated = "2026-10-29T12:04:41.207Z";
    let prompt = "Fix 0012 and tell me when it passes.";
    let session_file = sessions.join(format!("2026-10-29T12-00-00-000Z_{A_ID}.jsonl"));
    store.create(&Job {
        v: 1,
        id: id.as_str().into(),
        handle: id.handle(),
        created_at: created.into(),
        route: Route::Rpc,
        session_id: Some(A_ID.into()),
        session_file: Some(session_file),
        session_handle: Some("e27c0de".into()),
        deliver_as: Deliver::Auto,
        text: prompt.into(),
        cwd: cwd.clone(),
        omp_bin: "omp".into(),
        profile_flag: None,
        owners_dir: owners,
        job_timeout_ms: 30000,
        deadline_at: updated.into(),
    })?;
    store.write_result(
        &id,
        &JobResult {
            v: 1,
            prompt_status: Some("completed".into()),
            agent_invoked: true,
            assistant_text: Some(
                "Added the index. All 12 tests pass. Migration 0012 is ready.".into(),
            ),
            replies: vec![Reply {
                v: 1,
                id: "demo-reply".into(),
                session_id: A_ID.into(),
                job_id: Some(id.as_str().into()),
                request_id: None,
                tool_call_id: Some("reply-0012".into()),
                text: "Added the index. All 12 tests pass.".into(),
                is_final: true,
                at: updated.into(),
            }],
            command_output: vec![],
            stop_reason: Some("stop".into()),
            session_settled: true,
            settled_at: Some(updated.into()),
        },
    )?;
    store.write_status(
        &id,
        &Status {
            v: 1,
            status: State::Completed,
            updated_at: updated.into(),
            ..Status::default()
        },
    )?;

    let mut captures = Map::new();
    for (name, widths, args) in [
        ("ls", &[76, 44][..], vec!["ls"]),
        ("show", &[76, 64, 44][..], vec!["show", "e27c0de"]),
        (
            "tail-f",
            &[64, 44][..],
            vec!["tail", "e27c0de", "-n", "0", "-f"],
        ),
        ("wait", &[76, 44][..], vec!["wait", "ja3c9f41"]),
    ] {
        for width in widths {
            let mut command = Command::new(env!("CARGO_BIN_EXE_glen"));
            command
                .args(["--human", "--color", "always"])
                .args(&args)
                .env_clear()
                .env("HOME", &home)
                .env("GLEN_HOME", &glen_home)
                .env("PI_CODING_AGENT_DIR", &agent)
                .env("TMPDIR", &tmpdir)
                .env("TZ", "UTC")
                .env("COLUMNS", width.to_string())
                .env("COLORTERM", "truecolor")
                .env("GLEN_THEME", "dark")
                .env("PATH", "")
                .current_dir(&cwd);
            let now = SystemTime::now();
            for entry in fs::read_dir(&sessions)? {
                let path = entry?.path();
                let file = path
                    .file_name()
                    .ok_or_else(|| invalid("missing fixture filename"))?
                    .to_string_lossy();
                let age = if file.contains("e27c0de") {
                    75
                } else if file.contains("b7e2d81") {
                    210
                } else {
                    5400
                };
                fs::File::open(&path)?.set_modified(now - Duration::from_secs(age))?;
            }
            let stdout = if name == "tail-f" {
                follow(&mut command)?
            } else {
                let output = command.output()?;
                assert!(output.status.success(), "{output:?}");
                assert!(output.stderr.is_empty(), "{output:?}");
                String::from_utf8(output.stdout).map_err(|e| invalid(e.to_string()))?
            };
            guard_paths(&stdout, &root)?;
            captures.insert(
                format!("{name}@{width}"),
                json!({"args":args,"columns":width,"stdout":stdout}),
            );
        }
    }
    let plain_ls = anstream::adapter::strip_str(
        captures["ls@76"]["stdout"]
            .as_str()
            .ok_or_else(|| invalid("missing ls stdout"))?,
    )
    .to_string();
    assert!(plain_ls.contains("● live"), "{plain_ls}");
    assert!(
        plain_ls.contains("1m ago") && plain_ls.contains("3m ago") && plain_ls.contains("1h ago"),
        "{plain_ls}"
    );
    assert!(plain_ls.contains("3 sessions · 1 live"), "{plain_ls}");
    let plain_wait = anstream::adapter::strip_str(
        captures["wait@76"]["stdout"]
            .as_str()
            .ok_or_else(|| invalid("missing wait stdout"))?,
    )
    .to_string();
    assert!(
        plain_wait.starts_with("✓ completed · ja3c9f41 · e27c0de · 41s"),
        "{plain_wait}"
    );
    let golden = json!({
        "comment":"Generated by tests/site_demo.rs from tests/fixtures/demo. Do not edit; run: npm --prefix site run demo:refresh",
        "captures":captures
    });
    let fresh = serde_json::to_string_pretty(&golden).map_err(|e| invalid(e.to_string()))? + "\n";
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/site/src/data/demo-capture.json"
    ));
    if std::env::var_os("GLEN_UPDATE_DEMO").is_some() {
        fs::write(path, fresh)?;
    } else {
        let saved = fs::read_to_string(path)?;
        if saved != fresh {
            let previous: Value =
                serde_json::from_str(&saved).map_err(|e| invalid(e.to_string()))?;
            let key = captures
                .keys()
                .chain(
                    previous["captures"]
                        .as_object()
                        .into_iter()
                        .flat_map(|m| m.keys()),
                )
                .find(|key| previous["captures"][*key] != captures[*key]);
            panic!(
                "demo capture differs at {}: saved {:?}, fresh {:?}; run: npm --prefix site run demo:refresh (GLEN_UPDATE_DEMO=1 cargo test --locked --test site_demo) and commit site/src/data/demo-capture.json",
                key.map(String::as_str).unwrap_or("comment/format"),
                key.map(|k| &previous["captures"][k]),
                key.map(|k| &captures[k])
            );
        }
    }
    Ok(())
}

#[test]
fn path_guard_rejects_sandbox_paths() {
    let root = Path::new("/tmp/demo-sandbox");
    for path in [
        "/tmp/demo-sandbox/home",
        "/Users/fixture",
        "/home/runner/work",
        "/private/tmp/demo",
    ] {
        assert!(guard_paths(path, root).is_err(), "{path}");
    }
    assert!(guard_paths("~/work/alpha · e27c0de", root).is_ok());
}
