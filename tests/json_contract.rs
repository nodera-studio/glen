mod common;
use common::*;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read, Write},
    process::Stdio,
    sync::mpsc,
    time::Duration,
};
#[derive(Debug)]
struct Example {
    ndjson: bool,
    values: Vec<Value>,
}
// libc supplies POSIX regexes on both supported OSes, without a new dev dependency.
struct SourcePattern(libc::regex_t);
impl SourcePattern {
    fn new(pattern: &str) -> Self {
        let pattern = std::ffi::CString::new(pattern).expect("source pattern has no NUL");
        let mut compiled = std::mem::MaybeUninit::uninit();
        // SAFETY: regcomp initializes the valid, writable regex_t slot on success;
        // pattern is a live NUL-terminated string, and we check the return code.
        let result = unsafe {
            libc::regcomp(
                compiled.as_mut_ptr(),
                pattern.as_ptr(),
                libc::REG_EXTENDED | libc::REG_NOSUB,
            )
        };
        assert_eq!(result, 0, "cannot compile source pattern {pattern:?}");
        // SAFETY: successful regcomp initialized this regex_t.
        Self(unsafe { compiled.assume_init() })
    }
    fn is_match(&self, source: &str) -> bool {
        let source = std::ffi::CString::new(source).expect("Rust source has no NUL");
        // SAFETY: the compiled regex and source are live. REG_NOSUB needs no match
        // slots, so a null pointer with a zero match count is valid.
        let result = unsafe { libc::regexec(&self.0, source.as_ptr(), 0, std::ptr::null_mut(), 0) };
        assert!(
            result == 0 || result == libc::REG_NOMATCH,
            "source regex failed: {result}"
        );
        result == 0
    }
}
impl Drop for SourcePattern {
    fn drop(&mut self) {
        // SAFETY: this value owns a successfully compiled regex and frees it once.
        unsafe { libc::regfree(&mut self.0) };
    }
}
#[test]
fn every_no_route_site_names_a_reason() {
    let constructors = SourcePattern::new(
        r#"(GlenError::new|Failure::new)\([[:space:]]*(ErrorCode::NoRoute|"no_route")"#,
    );
    let literals = SourcePattern::new(r#"code:[[:space:]]*"no_route""#);
    for source in [
        "GlenError::new(\n ErrorCode::NoRoute,",
        "Failure::new(\n \"no_route\",",
    ] {
        assert!(
            constructors.is_match(source),
            "scan must recognize multiline constructors"
        );
    }
    assert!(literals.is_match("code:\n \"no_route\""));
    assert!(!constructors.is_match("GlenError::no_route(reason, message)"));
    let mut pending = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    let mut inspected = 0;
    while let Some(dir) = pending.pop() {
        for entry in
            fs::read_dir(&dir).unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        {
            let path = entry.expect("source directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let source = fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
                assert!(
                    !constructors.is_match(&source) && !literals.is_match(&source),
                    "{} constructs no_route without its typed reason",
                    path.display()
                );
                inspected += 1;
            }
        }
    }
    assert!(inspected > 0);
    let reasons = include_str!("../docs/json.md")
        .lines()
        .find_map(|line| line.strip_prefix("Allowed `no_route` reasons: "))
        .expect("Error details documents the allowed reasons")
        .split('`')
        .skip(1)
        .step_by(2)
        .collect::<Vec<_>>();
    assert_eq!(reasons.len(), glen::error::NoRouteReason::ALL.len());
    assert_eq!(
        reasons.into_iter().collect::<BTreeSet<_>>(),
        glen::error::NoRouteReason::ALL
            .into_iter()
            .map(|reason| reason.as_str())
            .collect::<BTreeSet<_>>()
    );
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
fn shape_eq(doc: &Value, real: &Value, lenient_errors: bool, path: &str) -> Result<(), String> {
    match (doc, real) {
        (Value::Null, _) | (_, Value::Null) => Ok(()),
        (Value::Object(a), Value::Object(b)) => {
            if path.ends_with(".error") {
                for object in [a, b] {
                    for key in ["code", "message"] {
                        if !object.get(key).is_some_and(Value::is_string) {
                            return Err(format!("{path}.{key}: required string"));
                        }
                    }
                    for (key, value) in object {
                        let valid = match key.as_str() {
                            "code" | "message" | "hint" => value.is_string(),
                            "details" => value.is_object(),
                            _ => false,
                        };
                        if !valid {
                            return Err(format!("{path}.{key}: unexpected key or type"));
                        }
                    }
                }
                if lenient_errors {
                    for (key, value) in a {
                        if let Some(other) = b.get(key) {
                            shape_eq(value, other, lenient_errors, &format!("{path}.{key}"))?;
                        }
                    }
                    return Ok(());
                }
            }
            let ak: BTreeSet<_> = a.keys().collect();
            let bk: BTreeSet<_> = b.keys().collect();
            if ak != bk {
                return Err(format!("{path}: keys {ak:?} != {bk:?}"));
            }
            for (key, v) in a {
                shape_eq(v, &b[key], lenient_errors, &format!("{path}.{key}"))?;
            }
            Ok(())
        }
        (Value::Array(a), Value::Array(b)) => {
            // Tail documents variants by kind and key set; other arrays keep their existing checks.
            if path == "tail.data.items" && a.iter().chain(b).all(|item| item.get("kind").is_some())
            {
                for (index, item) in b.iter().enumerate() {
                    let keys: BTreeSet<_> = item.as_object().unwrap().keys().collect();
                    let example = a
                        .iter()
                        .find(|example| {
                            example["kind"] == item["kind"]
                                && example.as_object().unwrap().keys().collect::<BTreeSet<_>>()
                                    == keys
                        })
                        .ok_or_else(|| {
                            let variants: Vec<_> = a
                                .iter()
                                .filter(|example| example["kind"] == item["kind"])
                                .map(|example| {
                                    example.as_object().unwrap().keys().collect::<BTreeSet<_>>()
                                })
                                .collect();
                            format!(
                                "{path}[{index}]: undocumented kind {} or key set {keys:?}; expected {variants:?}",
                                item["kind"]
                            )
                        })?;
                    shape_eq(example, item, lenient_errors, &format!("{path}[{index}]"))?;
                }
            } else if let (Some(a), Some(b)) = (a.first(), b.first()) {
                shape_eq(a, b, lenient_errors, &format!("{path}[0]"))?;
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
    shape_eq(&all["show"][0], &full, false, "spilled.content").unwrap();
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
    for (id, args, scenario, exit) in [
        ("models", vec!["models", "--json"], "ok", 0),
        (
            "models.session",
            vec!["models", "e27c0de", "--json"],
            "ok",
            0,
        ),
        (
            "model",
            vec!["model", "e27c0de", "fake/fake-2", "--json"],
            "ok",
            0,
        ),
        (
            "thinking",
            vec!["thinking", "e27c0de", "high", "--json"],
            "ok",
            0,
        ),
        ("abort.idle", vec!["abort", "e27c0de", "--json"], "ok", 0),
        (
            "compact",
            vec![
                "compact",
                "e27c0de",
                "--instructions",
                "keep exact facts",
                "--json",
            ],
            "ok",
            0,
        ),
        (
            "error.control_failed",
            vec!["compact", "e27c0de", "--json"],
            "compact_error",
            7,
        ),
    ] {
        let r = RpcBox::new();
        let out = r
            .glen(&args)
            .env("FAKE_OMP_SERIAL", "1")
            .env("FAKE_OMP_SCENARIO", scenario)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(exit), "{id}: {out:?}");
        assert!(out.stderr.is_empty(), "{id}: {out:?}");
        let v = json(&out);
        assert_eq!(v["ok"], exit == 0, "{id}: {v}");
        if exit == 7 {
            assert_eq!(v["error"]["code"], "control_failed");
            assert_eq!(v["error"]["details"]["reason"], "omp_error");
        } else {
            assert_eq!(
                v["data"]["route"],
                if id == "abort.idle" { "none" } else { "rpc" }
            );
        }
        all.insert(id.into(), vec![v]);
    }
    let r = RpcBox::new();
    let out = r
        .glen(&["send", "e27c0de", "hold", "--no-wait", "--json"])
        .env("FAKE_OMP_SERIAL", "1")
        .env("FAKE_OMP_RUN_RELEASE", r.omp.tmp.path().join("release"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(10), "{out:?}");
    assert!(out.stderr.is_empty());
    let launched = json(&out);
    let job = launched["data"]["job"]["id"].as_str().unwrap();
    poll(Duration::from_secs(5), || {
        (r.job(job).1["status"] == "accepted").then_some(())
    });
    let out = r
        .glen(&["abort", "e27c0de", "--timeout", "15s", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stderr.is_empty());
    let v = json(&out);
    assert_eq!(v["data"]["route"], "job");
    assert_eq!(v["data"]["aborted"], true);
    assert_eq!(v["data"]["job_status"], "aborted");
    r.wait_worker_gone(job);
    all.insert("abort.aborted".into(), vec![v]);
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
    let mut sandboxed = json(&out);
    // W10 fixes2 decision 1 + cycle-3 Codex P2 (design.md:449): on Linux the refusal adds
    // exactly `lease:"unknown"` to the documented details.
    if cfg!(target_os = "linux") {
        let details = sandboxed["error"]["details"].as_object_mut().unwrap();
        assert_eq!(details.remove("lease"), Some(serde_json::json!("unknown")));
    }
    all.insert("error.no_route.sandboxed".into(), vec![sandboxed]);
    let scoped = OmpBox::new();
    let out = scoped.glen(
        &[
            "show",
            "e27c0de",
            "--cwd",
            scoped.beta().to_str().unwrap(),
            "--json",
        ],
        &scoped.alpha(),
    );
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(json(&out)["error"]["details"]["reason"], "outside_cwd");
    all.insert("error.not_found.outside_cwd".into(), vec![json(&out)]);
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
                shape_eq(doc, value, false, &format!("{id}.{event}")).unwrap();
            }
        } else {
            shape_eq(&example.values[0], &actual[0], false, &id)
                .unwrap_or_else(|error| panic!("{error}; actual envelope: {}", actual[0]));
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
fn tail_full_fixture_matches_documented_item_variants() {
    let b = OmpBox::new();
    let args = ["tail", "e27c0de", "-n", "1000"];
    let out = b.glen(&args, &b.alpha());
    assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
    let original = json(&out);
    // Extend only the temporary copy of A; the shared catalog keeps its entry counts.
    let mut session = fs::OpenOptions::new()
        .append(true)
        .open(original["data"]["session"]["path"].as_str().unwrap())
        .unwrap();
    session
        .write_all(include_bytes!("fixtures/tail-items.jsonl"))
        .unwrap();
    let out = b.glen(&args, &b.alpha());
    assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
    let actual = json(&out);
    assert_eq!(actual["data"]["leaf_id"], "1000000f");
    assert_eq!(actual["data"]["skipped_lines"], 0);
    let items = actual["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 11);
    for kind in [
        "tool_result",
        "python",
        "developer",
        "custom",
        "file_mention",
        "reset",
    ] {
        assert!(
            items.iter().any(|item| item["kind"] == kind),
            "missing {kind}"
        );
    }
    for has_tools in [false, true] {
        assert!(items.iter().any(
            |item| item["kind"] == "assistant" && item.get("tool_calls").is_some() == has_tools
        ));
    }
    let docs = examples();
    shape_eq(&docs["tail"].values[0], &actual, false, "tail").unwrap();
}
#[test]
fn shape_checker_reports_path() {
    let err = shape_eq(
        &serde_json::json!({"data":{"field":1}}),
        &serde_json::json!({"data":{"renamed":1}}),
        false,
        "root",
    )
    .unwrap_err();
    assert!(err.contains("root.data") && err.contains("field") && err.contains("renamed"));
}
#[test]
fn tail_shape_checker_matches_every_item_by_kind() {
    let docs = examples();
    let items = &docs["tail"].values[0]["data"]["items"];
    let mut reordered = items.as_array().unwrap().clone();
    reordered.reverse();
    reordered.push(reordered[0].clone());
    assert!(
        shape_eq(
            items,
            &serde_json::json!(reordered),
            false,
            "tail.data.items"
        )
        .is_ok()
    );
    assert!(shape_eq(items, &serde_json::json!([]), false, "tail.data.items").is_ok());

    // A later malformed item must fail even when the first item matches.
    let mut malformed = items.clone();
    malformed[1].as_object_mut().unwrap().remove("id");
    let err = shape_eq(items, &malformed, false, "tail.data.items").unwrap_err();
    assert!(err.contains("tail.data.items[1]") && err.contains("id"));
    let mut malformed = items.clone();
    malformed[1]["extra"] = serde_json::json!(true);
    assert!(shape_eq(items, &malformed, false, "tail.data.items").is_err());
    let mut malformed = items.clone();
    malformed[1]["id"] = serde_json::json!(42);
    assert!(shape_eq(items, &malformed, false, "tail.data.items").is_err());
    let mut malformed = items.clone();
    malformed[1]["tool_calls"][0]["result"]["chars"] = serde_json::json!("wrong");
    let err = shape_eq(items, &malformed, false, "tail.data.items").unwrap_err();
    assert!(err.contains("tail.data.items[1].tool_calls[0].result.chars"));

    let mut unknown = items.clone();
    unknown[1]["kind"] = serde_json::json!("undocumented_item");
    let err = shape_eq(items, &unknown, false, "tail.data.items").unwrap_err();
    assert!(err.contains("tail.data.items[1]") && err.contains("undocumented_item"));
    assert!(shape_eq(&serde_json::json!([]), &unknown, false, "tail.data.items").is_err());
}
#[test]
fn shape_checker_untagged_arrays_keep_first_item_behavior() {
    for (doc, real) in [
        (serde_json::json!([1]), serde_json::json!([2, "ignored"])),
        (
            serde_json::json!([{"id": "example"}]),
            serde_json::json!([{"id": "real"}, {"different": true}]),
        ),
    ] {
        assert!(shape_eq(&doc, &real, false, "tail.data.items").is_ok());
    }
    assert!(
        shape_eq(
            &serde_json::json!([1]),
            &serde_json::json!(["wrong"]),
            false,
            "tail.data.items"
        )
        .is_err()
    );
}
#[test]
fn evidence_error_shapes() {
    let docs = examples();
    let example = &docs["error.usage"].values[0];
    let call = serde_json::json!({
        "t": "end",
        "id": "usage",
        "argv": ["glen", "tail", "--json", "--cwd"],
        "exit": 2,
        "ms": 1,
        "stdout": {
            "v": 1,
            "ok": false,
            "error": {
                "code": "usage",
                "message": "unexpected argument '--cwd' found",
                "details": {"text": "Usage: glen tail [OPTIONS] <SESSION>"}
            }
        }
    });
    assert!(shape_eq(example, &call["stdout"], false, "error.usage").is_ok());
    let mut missing_code = call["stdout"].clone();
    missing_code["error"]
        .as_object_mut()
        .unwrap()
        .remove("code");
    assert!(shape_eq(example, &missing_code, false, "error.usage").is_err());
    let mut extra_field = call["stdout"].clone();
    extra_field["error"]["extra"] = serde_json::json!(true);
    assert!(shape_eq(example, &extra_field, false, "error.usage").is_err());
    let mut invalid_details = call["stdout"].clone();
    invalid_details["error"]["details"] = serde_json::json!("not an object");
    assert!(shape_eq(example, &invalid_details, false, "error.usage").is_err());
}
#[test]
fn error_shape_optional_fields_are_validated_on_both_sides() {
    let required = serde_json::json!({"code": "usage", "message": "example"});
    let optional = serde_json::json!({
        "code": "usage",
        "message": "example",
        "hint": "try --help",
        "details": {"text": "usage"}
    });
    let mut hint_only = optional.clone();
    hint_only.as_object_mut().unwrap().remove("details");
    let mut details_only = optional.clone();
    details_only.as_object_mut().unwrap().remove("hint");
    for fields in [&hint_only, &details_only, &optional] {
        for (doc, real) in [(&required, fields), (fields, &required)] {
            assert!(shape_eq(doc, real, true, "error.usage.error").is_ok());
            assert!(shape_eq(doc, real, false, "error.usage.error").is_err());
            assert!(shape_eq(doc, real, true, "other").is_err());
            assert!(shape_eq(doc, real, false, "other").is_err());
        }
    }
    for (key, value) in [
        ("code", serde_json::json!(42)),
        ("message", Value::Null),
        ("hint", Value::Null),
        ("details", serde_json::json!("not an object")),
        ("extra", serde_json::json!(true)),
    ] {
        let mut invalid = optional.clone();
        invalid[key] = value;
        for lenient_errors in [false, true] {
            assert!(shape_eq(&optional, &invalid, lenient_errors, "error.usage.error").is_err());
            assert!(shape_eq(&invalid, &optional, lenient_errors, "error.usage.error").is_err());
        }
    }
    let mut mismatched = optional.clone();
    mismatched["details"]["text"] = serde_json::json!(42);
    for lenient_errors in [false, true] {
        assert!(shape_eq(&optional, &optional, lenient_errors, "error.usage.error").is_ok());
        assert!(shape_eq(&optional, &mismatched, lenient_errors, "error.usage.error").is_err());
    }
}
fn is_help_invocation(call: &Value) -> bool {
    call["exit"] == 0
        && call["stdout"].is_string()
        && call["argv"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .take_while(|arg| *arg != "--")
            .any(|arg| {
                matches!(arg, "--help" | "--version" | "-V")
                    || arg
                        .strip_prefix('-')
                        .is_some_and(|flags| flags.split('-').next().unwrap().contains('h'))
            })
}
#[test]
fn help_invocations_respect_argument_separator() {
    for (call, expected) in [
        (
            serde_json::json!({"argv":["inbox","--help","--"],"exit":0,"stdout":"help"}),
            true,
        ),
        (
            serde_json::json!({"argv":["inbox","-h","--"],"exit":0,"stdout":"help"}),
            true,
        ),
        (
            serde_json::json!({"argv":["tail","X","-fh","--"],"exit":0,"stdout":"help"}),
            true,
        ),
        (
            serde_json::json!({"argv":["glen","--version","--"],"exit":0,"stdout":"version"}),
            true,
        ),
        (
            serde_json::json!({"argv":["glen","-V","--"],"exit":0,"stdout":"version"}),
            true,
        ),
        (
            serde_json::json!({"argv":["send","--","--help"],"exit":0,"stdout":"help"}),
            false,
        ),
        (
            serde_json::json!({"argv":["send","--","-h"],"exit":0,"stdout":"help"}),
            false,
        ),
        (
            serde_json::json!({"argv":["send","--","-fh"],"exit":0,"stdout":"help"}),
            false,
        ),
        (
            serde_json::json!({"argv":["send","--","--version"],"exit":0,"stdout":"version"}),
            false,
        ),
        (
            serde_json::json!({"argv":["send","--","-V"],"exit":0,"stdout":"version"}),
            false,
        ),
        (
            serde_json::json!({"argv":["inbox","--json"],"exit":0,"stdout":"output"}),
            false,
        ),
        (
            serde_json::json!({"argv":["tail","-f-h"],"exit":0,"stdout":"output"}),
            false,
        ),
    ] {
        assert_eq!(is_help_invocation(&call), expected, "{call}");
    }
}
#[test]
fn evidence_help_is_skipped_while_contract_is_checked() {
    let docs = examples();
    let example = &docs["error.usage"].values[0];
    let calls = [
        serde_json::json!({
            "t": "end",
            "id": "help",
            "argv": ["glen", "inbox", "--help"],
            "exit": 0,
            "ms": 1,
            "stdout": "Usage: glen inbox [OPTIONS]"
        }),
        serde_json::json!({
            "t": "end",
            "id": "cluster-help",
            "argv": ["glen", "tail", "X", "-fh"],
            "exit": 0,
            "ms": 1,
            "stdout": "Usage: glen tail [OPTIONS]"
        }),
        serde_json::json!({
            "t": "end",
            "id": "failed-help",
            "argv": ["glen", "send", "--help"],
            "exit": 1,
            "ms": 1,
            "stdout": example
        }),
        serde_json::json!({
            "t": "end",
            "id": "object-help",
            "argv": ["glen", "send", "--help"],
            "exit": 0,
            "ms": 1,
            "stdout": example
        }),
        serde_json::json!({
            "t": "end",
            "id": "usage",
            "argv": ["glen", "tail", "--json", "--cwd"],
            "exit": 2,
            "ms": 1,
            "stdout": example
        }),
    ];
    let mut checked = 0;
    for call in calls {
        if is_help_invocation(&call) {
            continue;
        }
        shape_eq(example, &call["stdout"], true, "error.usage").unwrap();
        checked += 1;
    }
    assert_eq!(checked, 3);
}
#[test]
fn evidence_failed_help_rejects_malformed_error() {
    let docs = examples();
    let example = &docs["error.usage"].values[0];
    let mut call = serde_json::json!({
        "t": "end",
        "argv": ["glen", "send", "--help"],
        "exit": 1,
        "stdout": example
    });
    call["stdout"]["error"]
        .as_object_mut()
        .unwrap()
        .remove("message");
    assert!(!is_help_invocation(&call));
    let err = shape_eq(example, &call["stdout"], true, "error.usage").unwrap_err();
    assert!(err.contains("error.usage.error.message"));
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
            panic!("no recording at {path}: setup did not produce calls.ndjson: {err}");
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
        if is_help_invocation(&call) {
            continue;
        }
        let Some(command) = args
            .iter()
            .filter_map(Value::as_str)
            .find(|s| glen::skill::visible_commands().contains(*s))
        else {
            continue;
        };
        let error_id = format!(
            "error.{}{}",
            call["stdout"]["error"]["code"].as_str().unwrap_or_default(),
            if call["stdout"]["error"]["code"] == "no_route"
                && call["stdout"]["error"]["details"]["reason"] == "sandboxed"
            {
                ".sandboxed"
            } else {
                ""
            }
        );
        let id = if call["stdout"]["ok"] == false
            || !matches!(call["exit"].as_i64(), Some(0 | 10 | 11 | 130))
        {
            error_id.as_str()
        } else if command == "send" {
            match call["exit"].as_i64() {
                Some(0) => "send.completed",
                Some(10 | 130) => "send.running",
                Some(11) => "send.forked",
                _ => continue,
            }
        } else {
            command
        };
        let example = docs
            .get(id)
            .unwrap_or_else(|| panic!("no contract for {id}"));
        shape_eq(&example.values[0], &call["stdout"], true, id).unwrap();
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
