//! `glen mcp` on the real binary with `fake_omp` (v0.2 W5, AC-v2-502 to AC-v2-504; design
//! §A22): a scripted MCP client drives the server over its stdin and stdout.
mod common;
use common::{A_ID, RpcBox, poll};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

const CLAUDE_INIT: &str = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{"elicitation":{}},"clientInfo":{"name":"claude-code","version":"2.1.316"}}}"#;
const CODEX_INIT: &str = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"codex-mcp-client","title":"Codex","version":"0.166.0"}}}"#;

/// One `glen mcp` process and the lines it wrote; dropping it kills it and its children.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Arc<Mutex<Vec<String>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
}
impl Server {
    fn start(mut cmd: Command) -> Self {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let lines = Arc::new(Mutex::new(vec![]));
        let stderr = Arc::new(Mutex::new(vec![]));
        let out = BufReader::new(child.stdout.take().unwrap());
        let shared = lines.clone();
        thread::spawn(move || {
            for line in out.lines() {
                let Ok(line) = line else { break };
                shared.lock().unwrap().push(line);
            }
        });
        let mut err = child.stderr.take().unwrap();
        let shared = stderr.clone();
        thread::spawn(move || {
            let mut buf = vec![];
            let _ = std::io::Read::read_to_end(&mut err, &mut buf);
            shared.lock().unwrap().extend(buf);
        });
        Self {
            stdin: child.stdin.take(),
            child,
            lines,
            stderr,
        }
    }
    fn pid(&self) -> u32 {
        self.child.id()
    }
    fn raw(&mut self, line: &[u8]) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(line).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }
    fn send(&mut self, message: &Value) {
        self.raw(message.to_string().as_bytes());
    }
    fn messages(&self) -> Vec<Value> {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
            .collect()
    }
    fn wait_for(&self, timeout: Duration, mut pred: impl FnMut(&Value) -> bool) -> Value {
        poll(timeout, || self.messages().into_iter().find(|m| pred(m)))
    }
    fn answer(&self, id: &Value) -> Value {
        self.wait_for(Duration::from_secs(30), |m| {
            &m["id"] == id && m.get("method").is_none()
        })
    }
    fn request(&mut self, id: Value, method: &str, params: Value) -> Value {
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        self.answer(&id)
    }
    fn init(&mut self) -> Value {
        self.raw(CLAUDE_INIT.as_bytes());
        let r = self.answer(&json!(0));
        self.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        r
    }
    fn start_call(&mut self, id: Value, name: &str, arguments: Value) {
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
            "params":{"name":name,"arguments":arguments}}));
    }
    /// A finished `tools/call`'s result.
    fn call(&mut self, id: Value, name: &str, arguments: Value) -> Value {
        self.start_call(id.clone(), name, arguments);
        let m = self.answer(&id);
        assert!(m.get("error").is_none(), "{m}");
        m["result"].clone()
    }
    /// Direct children of the server, from `/bin/ps`.
    fn children(&self) -> Vec<u32> {
        children_of(self.pid())
    }
    fn close_stdin(&mut self) {
        self.stdin = None;
    }
    fn exit_within(&mut self, bound: Duration) -> ExitStatus {
        poll(bound, || self.child.try_wait().unwrap())
    }
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        for pid in self.children() {
            signal(pid, rustix::process::Signal::KILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn children_of(parent: u32) -> Vec<u32> {
    let out = Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace().map(|s| s.parse::<u32>().ok());
            let (pid, ppid) = (it.next()??, it.next()??);
            (ppid == parent).then_some(pid)
        })
        .collect()
}
fn signal(pid: u32, sig: rustix::process::Signal) {
    if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
        let _ = rustix::process::kill_process(pid, sig);
    }
}
fn text(result: &Value) -> &str {
    let content = result["content"].as_array().unwrap();
    assert_eq!(content.len(), 1, "{result}");
    assert_eq!(content[0]["type"], "text");
    content[0]["text"].as_str().unwrap()
}
fn envelope(result: &Value) -> Value {
    serde_json::from_str(text(result)).unwrap()
}
fn job_id(doc: &Value) -> String {
    doc["data"]["job"]["id"]
        .as_str()
        .or_else(|| doc["error"]["details"]["job"]["id"].as_str())
        .unwrap()
        .to_owned()
}
fn server(rb: &RpcBox, extra: &[&str], vars: &[(&str, &str)]) -> Server {
    let mut args = extra.to_vec();
    args.push("mcp");
    let mut cmd = rb.glen(&args);
    cmd.envs(vars.iter().copied());
    Server::start(cmd)
}
/// A path whose absence holds `fake_omp` before its reply (create it to release the job).
fn hold_path(rb: &RpcBox) -> String {
    rb.omp
        .tmp
        .path()
        .join("release")
        .to_str()
        .unwrap()
        .to_owned()
}

// ---------------------------------------------------------------------------------------------
// AC-v2-502: the handshake and the JSON-RPC rules over the wire.
// ---------------------------------------------------------------------------------------------

#[test]
fn the_handshake_negotiates_and_lists_the_catalog() {
    let rb = RpcBox::new();
    for (init, version) in [
        (CLAUDE_INIT.to_owned(), "2025-11-25"),
        (CODEX_INIT.to_owned(), "2025-06-18"),
        (
            CLAUDE_INIT.replace("2025-11-25", "2024-11-05"),
            "2025-11-25",
        ),
    ] {
        let mut s = server(&rb, &[], &[]);
        // Before initialize only ping works.
        assert_eq!(
            s.request(json!("p"), "ping", json!({}))["result"],
            json!({})
        );
        let early = s.request(json!(1), "tools/list", json!({}));
        assert_eq!(early["error"]["code"], -32600);
        let message = early["error"]["message"].as_str().unwrap();
        assert!(message.contains("2025-11-25") && message.contains("2025-06-18"));
        assert_eq!(
            s.request(json!(2), "tools/call", json!({"name":"glen_jobs"}))["error"]["code"],
            -32600
        );
        s.raw(init.as_bytes());
        let r = s.answer(&json!(0));
        assert_eq!(
            r["result"],
            json!({"protocolVersion":version,"capabilities":{"tools":{}},
                "serverInfo":{"name":"glen","title":"glen","version":env!("CARGO_PKG_VERSION")}})
        );
        s.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        let again = s.request(json!(3), "initialize", json!({"protocolVersion":version}));
        assert_eq!(again["error"]["code"], -32600);
        let listed = s.request(json!(4), "tools/list", json!({}));
        let catalog = glen::mcp::catalog::load().unwrap();
        let want: Vec<Value> = catalog
            .tools
            .iter()
            .map(
                |t| json!({"name":t.name,"description":t.description,"inputSchema":t.input_schema}),
            )
            .collect();
        assert_eq!(listed["result"], json!({"tools":want}));
        s.close_stdin();
        assert!(s.exit_within(Duration::from_secs(2)).success());
        assert_eq!(
            s.stderr(),
            "",
            "stdout carries protocol only and stderr stays empty"
        );
    }
    assert!(!rb.log.exists(), "no omp started");
}

#[test]
fn the_error_table_over_the_wire() {
    let rb = RpcBox::new();
    let mut s = server(&rb, &[], &[]);
    s.init();
    s.raw(b"{not json");
    s.raw(br#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#);
    let errors = s.wait_for(Duration::from_secs(5), |m| m["id"].is_null());
    assert_eq!(errors["error"]["code"], -32700);
    let batch = poll(Duration::from_secs(5), || s.messages().get(2).cloned());
    assert_eq!(
        (batch["id"].clone(), batch["error"]["code"].clone()),
        (Value::Null, json!(-32600))
    );
    for method in ["server/discover", "resources/list", "prompts/list"] {
        let r = s.request(json!(method), method, json!({}));
        assert_eq!(r["error"]["code"], -32601, "{method}");
    }
    let unknown = s.request(
        json!(5),
        "tools/call",
        json!({"name":"glen_nope","arguments":{}}),
    );
    assert_eq!(unknown["error"]["code"], -32602);
    let mut long = br#"{"jsonrpc":"2.0","id":6,"method":"ping","pad":""#.to_vec();
    long.resize(long.len() + 4 * 1024 * 1024 + 1, b'x');
    long.extend_from_slice(br#""}"#);
    s.raw(&long);
    let after = s.request(json!(7), "ping", json!({}));
    assert_eq!(after["result"], json!({}));
    let all = s.messages();
    let oversize: Vec<&Value> = all
        .iter()
        .filter(|m| m["id"].is_null() && m["error"]["code"] == -32700)
        .collect();
    assert_eq!(oversize.len(), 2, "the junk line and the 4 MiB + 1 line");
    assert!(all.iter().all(|m| m["id"] != 6));
    s.send(&json!({"jsonrpc":"2.0","method":"notifications/whatever"}));
    s.send(&json!({"jsonrpc":"2.0","id":8,"result":{}}));
    assert_eq!(s.request(json!(9), "ping", json!({}))["result"], json!({}));
    assert!(s.messages().iter().all(|m| m["id"] != 8));
}

// ---------------------------------------------------------------------------------------------
// AC-v2-503: a result's text is the CLI's own envelope.
// ---------------------------------------------------------------------------------------------

#[test]
fn deterministic_results_are_byte_equal_to_a_direct_run() {
    let rb = RpcBox::new();
    // `age_ms` counts from the file's mtime; one in the future pins it at 0 for both runs.
    std::fs::File::options()
        .write(true)
        .open(rb.omp.session('A'))
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(3600))
        .unwrap();
    let mut s = server(&rb, &[], &[]);
    s.init();
    for (i, (name, arguments, argv)) in [
        (
            "glen_show",
            json!({"session":"e27c0de"}),
            vec!["--json", "show", "e27c0de"],
        ),
        (
            "glen_show",
            json!({"session":"e27c0de","tools":"full","thinking":true}),
            vec!["--json", "show", "e27c0de", "--tools", "full", "--thinking"],
        ),
        (
            "glen_tail",
            json!({"session":"e27c0de","lines":3}),
            vec!["--json", "tail", "e27c0de", "-n", "3"],
        ),
        ("glen_jobs", json!({}), vec!["--json", "jobs"]),
        (
            "glen_inbox",
            json!({"peek":true}),
            vec!["--json", "inbox", "--peek"],
        ),
        ("glen_doctor", json!({}), vec!["--json", "doctor"]),
        (
            "glen_show",
            json!({"session":"nope"}),
            vec!["--json", "show", "nope"],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let result = s.call(json!(10 + i), name, arguments);
        let direct = rb.glen(&argv).output().unwrap();
        let direct = String::from_utf8(direct.stdout).unwrap();
        assert!(
            text(&result) == direct,
            "{name} {argv:?}\n{}\n{direct}",
            text(&result)
        );
        assert!(text(&result).ends_with('\n'));
        let ok = envelope(&result)["ok"] == true;
        assert_eq!(result["isError"], !ok, "{name}");
        assert!(result.get("structuredContent").is_none());
        assert_eq!(result.as_object().unwrap().len(), 2, "{result}");
    }
    assert_eq!(
        s.messages().last().unwrap()["result"]["isError"],
        true,
        "unknown handle"
    );
}

#[test]
fn is_error_follows_ok_and_exits_10_11_and_130_are_results() {
    let rb = RpcBox::new();
    // A completed send.
    let mut s = server(&rb, &[], &[]);
    s.init();
    let done = s.call(
        json!(1),
        "glen_send",
        json!({"session":"e27c0de","message":"hi"}),
    );
    assert_eq!(done["isError"], false, "{done}");
    assert_eq!(envelope(&done)["data"]["status"], "completed");
    rb.wait_worker_gone(&job_id(&envelope(&done)));
    let missing = s.call(json!(2), "glen_wait", json!({"job":"j0000000"}));
    assert_eq!(missing["isError"], true);
    assert_eq!(envelope(&missing)["error"]["code"], "not_found");
    drop(s);

    // A send held past its wait: the exit-10 running envelope.
    let hold = hold_path(&rb);
    let mut s = server(&rb, &[], &[("FAKE_OMP_RUN_RELEASE", &hold)]);
    s.init();
    let running = s.call(
        json!(1),
        "glen_send",
        json!({"session":"e27c0de","message":"held","wait":"1s"}),
    );
    assert_eq!(running["isError"], false, "{running}");
    let doc = envelope(&running);
    assert_eq!(doc["data"]["status"], "running");
    let job = job_id(&doc);
    // A SIGINT-interrupted observer: the exit-130 document.
    s.start_call(json!(2), "glen_wait", json!({"job":job,"timeout":"60s"}));
    let observer = poll(Duration::from_secs(10), || s.children().first().copied());
    thread::sleep(Duration::from_millis(300));
    signal(observer, rustix::process::Signal::INT);
    let interrupted = s.answer(&json!(2))["result"].clone();
    assert_eq!(interrupted["isError"], false, "{interrupted}");
    assert_eq!(
        envelope(&interrupted)["data"]["interrupted"],
        true,
        "{interrupted}"
    );
    std::fs::write(&hold, b"go").unwrap();
    rb.wait_worker_gone(&job);
    drop(s);

    // A forked job (exit 11) and a failed job.
    for (scenario, is_error) in [("fork", false), ("error", true)] {
        let mut s = server(&rb, &[], &[("FAKE_OMP_SCENARIO", scenario)]);
        s.init();
        let r = s.call(
            json!(1),
            "glen_send",
            json!({"session":"e27c0de","message":scenario}),
        );
        assert_eq!(r["isError"], is_error, "{scenario}: {r}");
        let doc = envelope(&r);
        if scenario == "fork" {
            assert!(doc["data"]["forked"].is_object(), "{doc}");
        } else {
            assert_eq!(doc["error"]["code"], "job_failed", "{doc}");
        }
        rb.wait_worker_gone(&job_id(&doc));
    }
}

#[test]
fn argv_reaches_glen_verbatim_without_a_shell() {
    let rb = RpcBox::new();
    let marker = rb.omp.tmp.path().join("pwned");
    let message = format!(
        "-v is broken; echo `id` $(touch {}) > x && \"quoted\"  'single'",
        marker.display()
    );
    let mut s = server(&rb, &[], &[]);
    s.init();
    let r = s.call(
        json!(1),
        "glen_send",
        json!({"session":"e27c0de","message":message}),
    );
    assert_eq!(r["isError"], false, "{r}");
    rb.wait_worker_gone(&job_id(&envelope(&r)));
    let prompts = rb.frames_in("prompt");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0]["message"], message);
    assert!(!marker.exists(), "no shell ran the message");
    assert!(!rb.omp.alpha().join("x").exists());
    // `-` reads the child's /dev/null stdin: the CLI's own usage error.
    let dash = s.call(
        json!(2),
        "glen_send",
        json!({"session":"e27c0de","message":"-"}),
    );
    assert_eq!(dash["isError"], true);
    let doc = envelope(&dash);
    assert_eq!(doc["error"]["code"], "usage", "{doc}");
    assert_ne!(doc["error"]["details"]["reason"], "mcp_arguments", "{doc}");
    assert_eq!(rb.frames_in("prompt").len(), 1);
}

#[test]
fn argument_errors_are_usage_envelopes_and_start_nothing() {
    let rb = RpcBox::new();
    let mut s = server(&rb, &[], &[]);
    s.init();
    for (i, (name, arguments, argument)) in [
        (
            "glen_send",
            json!({"session":"e27c0de","message":"m","bogus":1}),
            "bogus",
        ),
        (
            "glen_send",
            json!({"session":"e27c0de","message":7}),
            "message",
        ),
        ("glen_send", json!({"session":"e27c0de"}), "message"),
        (
            "glen_send",
            json!({"session":null,"message":"m"}),
            "session",
        ),
        (
            "glen_send",
            json!({"session":"e27c0de","message":"m","as":"shout"}),
            "as",
        ),
        (
            "glen_send",
            json!({"session":"e27c0de","message":"a\u{0}b"}),
            "message",
        ),
        (
            "glen_send",
            json!({"session":"e27c0de","message":"m","no_wait":"yes"}),
            "no_wait",
        ),
        ("glen_jobs", json!({"limit":-1}), "limit"),
        (
            "glen_tail",
            json!({"session":"e27c0de","follow":true}),
            "follow",
        ),
        ("glen_inbox", json!({"follow":true}), "follow"),
        ("glen_jobs", json!({"json":true}), "json"),
        ("glen_jobs", json!({"profile":"p"}), "profile"),
    ]
    .into_iter()
    .enumerate()
    {
        let r = s.call(json!(10 + i), name, arguments.clone());
        assert_eq!(r["isError"], true, "{arguments}");
        let doc = envelope(&r);
        assert_eq!(doc["v"], 1);
        assert_eq!(doc["ok"], false);
        assert_eq!(doc["error"]["code"], "usage", "{doc}");
        assert_eq!(doc["error"]["details"]["reason"], "mcp_arguments", "{doc}");
        assert_eq!(doc["error"]["details"]["tool"], name);
        assert_eq!(doc["error"]["details"]["argument"], argument, "{doc}");
        assert!(
            doc["error"]["message"].as_str().unwrap().contains(argument),
            "{doc}"
        );
    }
    assert!(s.children().is_empty(), "no child started");
    // Absent or null arguments, and null for an optional property, are unset.
    for (i, params) in [
        json!({"name":"glen_jobs"}),
        json!({"name":"glen_jobs","arguments":null}),
        json!({"name":"glen_jobs","arguments":{"session":null,"limit":null,"active":null}}),
    ]
    .into_iter()
    .enumerate()
    {
        let r = s.request(json!(100 + i), "tools/call", params);
        assert_eq!(r["result"]["isError"], false, "{r}");
        assert_eq!(envelope(&r["result"])["data"]["total"], 0);
    }
    // The server passes exactly what was supplied: a host that fills `wait`'s default beside
    // `no_wait` gets the CLI's own conflict, and nothing is sent.
    let both = s.call(
        json!(200),
        "glen_send",
        json!({"session":"e27c0de","message":"m","no_wait":true,"wait":"90s"}),
    );
    assert_eq!(both["isError"], true);
    let doc = envelope(&both);
    assert_eq!(doc["error"]["code"], "usage", "{doc}");
    assert!(
        doc["error"]["details"]
            .get("reason")
            .is_none_or(|r| r != "mcp_arguments")
    );
    assert!(!rb.log.exists(), "the fake omp never started");
}

#[test]
fn the_profile_and_cwd_of_the_server_reach_every_child() {
    let rb = RpcBox::new();
    let mut s = server(&rb, &["--profile", "p"], &[]);
    s.init();
    let doctor = envelope(&s.call(json!(1), "glen_doctor", json!({})));
    assert_eq!(doctor["data"]["paths"]["profile"], "p", "{doctor}");
    drop(s);
    let mut s = server(&rb, &[], &[]);
    s.init();
    let listed = envelope(&s.call(json!(1), "glen_ls", json!({})));
    let direct: Value =
        serde_json::from_slice(&rb.glen(&["--json", "ls"]).output().unwrap().stdout).unwrap();
    let ids = |doc: &Value| -> Vec<Value> {
        doc["data"]["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["id"].clone())
            .collect()
    };
    assert!(!ids(&listed).is_empty());
    assert_eq!(
        ids(&listed),
        ids(&direct),
        "the child ran in the server's cwd"
    );
    assert!(ids(&listed).contains(&json!(A_ID)));
}
