mod common;
use common::*;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read},
    process::Stdio,
    sync::mpsc,
    time::Duration,
};
#[derive(Debug)]
struct Example {
    ndjson: bool,
    values: Vec<Value>,
}
fn examples() -> BTreeMap<String, Example> {
    let mut map = BTreeMap::new();
    let mut lines = include_str!("../docs/json.md").lines();
    while let Some(line) = lines.next() {
        if let Some(marker) = line
            .strip_prefix("<!-- contract: ")
            .and_then(|s| s.strip_suffix(" -->"))
        {
            let mut parts = marker.split_whitespace();
            let id = parts.next().unwrap();
            let ndjson = parts.next() == Some("ndjson");
            let open = lines.by_ref().find(|l| l.starts_with("```")).unwrap();
            assert_eq!(open, "```json");
            let text = lines
                .by_ref()
                .take_while(|l| *l != "```")
                .collect::<Vec<_>>()
                .join("\n");
            let values = if ndjson {
                text.lines()
                    .map(|l| serde_json::from_str(l).unwrap())
                    .collect()
            } else {
                vec![serde_json::from_str(&text).unwrap()]
            };
            assert!(
                map.insert(id.into(), Example { ndjson, values }).is_none(),
                "duplicate {id}"
            );
        }
    }
    map
}
fn shape_eq(doc: &Value, real: &Value, path: &str) -> Result<(), String> {
    match (doc, real) {
        (Value::Null, _) | (_, Value::Null) => Ok(()),
        (Value::Object(a), Value::Object(b)) => {
            let ak: BTreeSet<_> = a.keys().collect();
            let bk: BTreeSet<_> = b.keys().collect();
            if ak != bk {
                return Err(format!("{path}: keys {ak:?} != {bk:?}"));
            }
            for (key, v) in a {
                shape_eq(v, &b[key], &format!("{path}.{key}"))?;
            }
            Ok(())
        }
        (Value::Array(a), Value::Array(b)) => {
            if let (Some(a), Some(b)) = (a.first(), b.first()) {
                shape_eq(a, b, &format!("{path}[0]"))?;
            }
            Ok(())
        }
        (Value::Bool(_), Value::Bool(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_)) => Ok(()),
        _ => Err(format!("{path}: types differ: {doc} != {real}")),
    }
}
fn terminal_stream(lines: &[Value]) {
    let terminal: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, v)| v["event"] == "end" || v["ok"] == false)
        .collect();
    assert_eq!(terminal.len(), 1, "{lines:?}");
    assert_eq!(terminal[0].0, lines.len() - 1);
}
fn follow(b: &OmpBox, args: &[&str], first: &str) -> Vec<Value> {
    let mut child = b
        .command(args, &b.alpha())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut values = Vec::new();
    loop {
        let line = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        let item = v["event"] == first;
        values.push(v);
        if item {
            break;
        }
    }
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&child),
        rustix::process::Signal::INT,
    )
    .unwrap();
    loop {
        let line = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        let end = v["event"] == "end" || v["ok"] == false;
        values.push(v);
        if end {
            break;
        }
    }
    let status = poll(Duration::from_secs(5), || child.try_wait().unwrap());
    assert_eq!(status.code(), Some(130));
    reader.join().unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stderr.is_empty());
    terminal_stream(&values);
    values
}
fn runners() -> BTreeMap<String, Vec<Value>> {
    let mut all = BTreeMap::new();
    let b = OmpBox::new();
    for (id, args) in [
        ("ls", vec!["ls"]),
        ("show", vec!["show", "e27c0de"]),
        ("show.raw", vec!["show", "e27c0de", "--raw"]),
        ("tail", vec!["tail", "e27c0de", "-n", "1"]),
        ("completions", vec!["completions", "zsh", "--json"]),
        (
            "install-skill",
            vec!["install-skill", "--project", "--dry-run"],
        ),
        ("install-extension", vec!["install-extension", "--dry-run"]),
        ("error.usage", vec!["ls", "--bogus"]),
        ("error.not_found", vec!["show", "fffffff"]),
        ("error.ambiguous", vec!["show", "c0de"]),
        ("error.omp_unavailable", vec!["doctor"]),
    ] {
        let out = b.glen(&args, &b.alpha());
        assert!(out.stderr.is_empty());
        all.insert(id.into(), vec![json(&out)]);
    }
    let out = b
        .command(&["doctor"], &b.alpha())
        .env("GLEN_OMP_BIN", fixture("bin/fake-omp"))
        .output()
        .unwrap();
    assert!(out.status.success());
    all.insert("doctor".into(), vec![json(&out)]);
    all.insert(
        "tail.follow".into(),
        follow(&b, &["tail", "e27c0de", "-n", "1", "-f", "--json"], "item"),
    );
    let inbox = b.glen_home.join("inbox");
    fs::create_dir_all(&inbox).unwrap();
    fs::write(
        inbox.join(format!("{A_ID}.ndjson")),
        format!(
            "{}\n",
            serde_json::json!({"v":1,"id":"01a18bcc-6b8c-74bc-81aa-d8b5134e7534","session_id":A_ID,"job_id":"01a18bcc-6970-7685-8f03-0ecc16977273","request_id":null,"tool_call_id":"call_1","text":"reply: hi","final":true,"at":"2026-10-30T07:53:12.332571Z"})
        ),
    )
    .unwrap();
    all.insert(
        "inbox.follow".into(),
        follow(&b, &["inbox", "-f", "--peek", "--json"], "message"),
    );
    let out = b.glen(&["inbox", "--peek"], &b.alpha());
    assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
    all.insert("inbox".into(), vec![json(&out)]);
    let out = b
        .command(&["show", "e27c0de"], &b.alpha())
        .env("GLEN_MAX_OUTPUT", "200")
        .output()
        .unwrap();
    let stub = json(&out);
    let path = std::path::Path::new(stub["spilled"]["path"].as_str().unwrap());
    let bytes = fs::read(path).unwrap();
    assert_eq!(
        bytes.len() as u64,
        stub["spilled"]["bytes"].as_u64().unwrap()
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let full: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(full["ok"], true);
    shape_eq(&all["show"][0], &full, "spilled.content").unwrap();
    all.insert("spilled".into(), vec![stub]);
    for (id, scenario, exit) in [
        ("send.completed", "ok", 0),
        ("send.forked", "fork", 11),
        ("error.job_failed", "error", 7),
    ] {
        let r = RpcBox::new();
        let out = r
            .glen(&["send", "e27c0de", "hi"])
            .env("FAKE_OMP_SCENARIO", scenario)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(exit), "{out:?}");
        assert!(out.stderr.is_empty());
        let v = json(&out);
        let job = if exit == 7 {
            v["error"]["details"]["job"]["id"].as_str().unwrap()
        } else {
            v["data"]["job"]["id"].as_str().unwrap()
        };
        r.wait_worker_gone(job);
        if id == "send.completed" {
            let out = r.glen(&["jobs"]).output().unwrap();
            let rows = json(&out);
            assert_eq!(rows["data"]["total"], 1);
            assert_eq!(rows["data"]["jobs"].as_array().unwrap().len(), 1);
            assert_eq!(rows["data"]["jobs"][0]["status"], "completed");
            all.insert("jobs".into(), vec![rows]);
        }
        all.insert(id.into(), vec![v]);
    }
    let r = RpcBox::new();
    let out = r
        .glen(&["send", "e27c0de", "hi", "--wait", "100ms"])
        .env("FAKE_OMP_DELAY_MS", "600")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(10));
    let v = json(&out);
    let handle = v["data"]["job"]["handle"].as_str().unwrap();
    let out = r
        .glen(&["wait", handle, "--timeout", "10s"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    all.insert("wait".into(), vec![json(&out)]);
    r.wait_worker_gone(v["data"]["job"]["id"].as_str().unwrap());
    all.insert("send.running".into(), vec![v]);
    let r = RpcBox::new();
    let mut lease = r.hold_lease(A_ID);
    let out = r.glen(&["send", "e27c0de", "hi"]).output().unwrap();
    assert_eq!(out.status.code(), Some(6));
    all.insert("error.no_route".into(), vec![json(&out)]);
    drop(lease.stdin.take());
    poll(Duration::from_secs(5), || lease.try_wait().unwrap());
    let out = r
        .glen(&["send", "e27c0de", "hi"])
        .env("CODEX_SANDBOX", "seatbelt")
        .env("CODEX_SANDBOX_NETWORK_DISABLED", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    all.insert("error.no_route.sandboxed".into(), vec![json(&out)]);
    all
}
#[test]
fn ac412_examples_match_real_envelopes() {
    let docs = examples();
    let real = runners();
    assert_eq!(
        docs.keys().collect::<Vec<_>>(),
        real.keys().collect::<Vec<_>>()
    );
    for (id, example) in docs {
        let actual = &real[&id];
        if example.ndjson {
            terminal_stream(actual);
            terminal_stream(&example.values);
            for value in actual {
                let event = &value["event"];
                let doc = example
                    .values
                    .iter()
                    .find(|v| &v["event"] == event)
                    .unwrap_or_else(|| panic!("{id}: missing event {event}"));
                shape_eq(doc, value, &format!("{id}.{event}")).unwrap();
            }
        } else {
            shape_eq(&example.values[0], &actual[0], &id).unwrap();
        }
    }
}
#[test]
fn ac412_command_sections_and_exit_codes() {
    let text = include_str!("../docs/json.md");
    let mut c = glen::cli::public_command();
    c.build();
    for sub in c.get_subcommands() {
        for name in std::iter::once(sub.get_name()).chain(sub.get_visible_aliases()) {
            assert!(
                text.contains(&format!("## `glen {name}`")),
                "missing section {name}"
            );
        }
    }
    let rows: BTreeMap<u8, String> = text
        .lines()
        .filter_map(|l| {
            let cells: Vec<_> = l.split('|').map(str::trim).collect();
            Some((cells.get(1)?.parse().ok()?, cells.get(3)?.to_string()))
        })
        .collect();
    for code in glen::error::ErrorCode::ALL {
        assert!(
            rows.get(&code.exit_code())
                .is_some_and(|s| s.contains(&format!("`{}`", code.as_str()))),
            "{}",
            code.as_str()
        );
    }
    for exit in [0, 10, 11, 130] {
        assert!(rows.contains_key(&exit));
    }
    assert_eq!(rows.len(), 12);
    assert!(common::exit_code_errors(text, 3).is_empty());
}
#[test]
fn shape_checker_reports_path() {
    let err = shape_eq(
        &serde_json::json!({"data":{"field":1}}),
        &serde_json::json!({"data":{"renamed":1}}),
        "root",
    )
    .unwrap_err();
    assert!(err.contains("root.data") && err.contains("field") && err.contains("renamed"));
}
#[test]
#[ignore]
fn evidence_shapes() {
    let Ok(path) = std::env::var("GLEN_CONTRACT_EVIDENCE") else {
        eprintln!("skipped: GLEN_CONTRACT_EVIDENCE is unset");
        return;
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("skipped: no recording at {path}: {err}");
            return;
        }
    };
    let docs = examples();
    let mut checked = 0;
    for line in text.lines() {
        let call: Value = serde_json::from_str(line).unwrap();
        if call["t"] != "end" {
            continue;
        }
        let args = call["argv"].as_array().unwrap();
        let Some(command) = args
            .iter()
            .filter_map(Value::as_str)
            .find(|s| glen::skill::visible_commands().contains(*s))
        else {
            continue;
        };
        let id = if command == "send" {
            match call["exit"].as_i64() {
                Some(0) => "send.completed",
                Some(10 | 130) => "send.running",
                Some(11) => "send.forked",
                Some(6) if call["stdout"]["error"]["details"]["reason"] == "sandboxed" => {
                    "error.no_route.sandboxed"
                }
                Some(6) => "error.no_route",
                Some(7) => "error.job_failed",
                _ => continue,
            }
        } else {
            command
        };
        let example = docs
            .get(id)
            .unwrap_or_else(|| panic!("no contract for {id}"));
        shape_eq(&example.values[0], &call["stdout"], id).unwrap();
        checked += 1;
    }
    assert!(checked > 0, "no contract calls in evidence");
}

#[test]
fn exit_code_checker_rejects_extra_or_misplaced_codes() {
    let text = include_str!("../docs/json.md");
    for changed in [
        text.replace("`no_route` |", "`no_route`, `io` |"),
        text.replace("`usage`, `invalid_env`", "`usage`"),
        text.replace("| done |", "| `io` |"),
    ] {
        assert!(!common::exit_code_errors(&changed, 3).is_empty());
    }
}
