//! The real bundled extension, end to end: `glen install-extension` renders it, Node (type
//! stripping, like the plan's TS suite) loads the installed file into a minimal omp extension host
//! written here from omp's documented API (`pi.on`, `pi.registerTool`, `pi.sendUserMessage`,
//! `pi.zod`, `ctx.mode/agent/sessionManager/isIdle/hasPendingMessages/setTimeout/clearTimer/ui`),
//! and the real `glen` binary talks to it (AC-305…AC-313, AC-316, AC-321, AC-322 as observable
//! from outside the extension). `fake_ext` shares glen's Rust registry types; this group is the
//! only check that the TypeScript side and the Rust side agree on the files and the wire.
//!
//! Needs Node ≥ 22.6 on PATH. Without it the tests print a note and pass locally; with `CI` set
//! they fail, as `tests/extension_ts.rs` does.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::live_support::*;
use crate::reader::*;
use crate::rpc_support::*;
use crate::support::*;

/// The host: loads `HOST_EXT`, runs its factory, emits `session_start`, prints `ready`, then
/// simulates omp runs and obeys stdin commands, logging every interaction to `HOST_LOG`.
const HOST_JS: &str = r#"
import { pathToFileURL } from "node:url";
import fs from "node:fs";
import readline from "node:readline";

const E = process.env;
const log = (o) => { try { fs.appendFileSync(E.HOST_LOG, JSON.stringify(o) + "\n"); } catch {} };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const st = { idle: E.HOST_BUSY !== "1", queue: [], id: E.HOST_SESSION_ID, file: E.HOST_SESSION_FILE || undefined, calls: 0 };
const handlers = new Map();
const tools = [];
const chain = () => new Proxy(function () {}, {
  get: (_t, p) => (p === "then" || typeof p === "symbol" ? undefined : () => chain()),
  apply: () => chain(),
});
const fallback = (label, base) => new Proxy(base, {
  get(t, p) {
    if (p in t) return t[p];
    if (typeof p === "symbol" || p === "then") return undefined;
    log({ k: label, p });
    return label === "ctx_unknown" ? undefined : () => undefined;
  },
});
const pi = fallback("pi_unknown", {
  on(name, fn) {
    if (!handlers.has(name)) handlers.set(name, []);
    handlers.get(name).push(fn);
    log({ k: "on", name });
  },
  registerTool(def) {
    tools.push(def);
    log({ k: "registerTool", name: def.name, loadMode: def.loadMode ?? null, approval: def.approval ?? null });
  },
  sendUserMessage(text, opts) {
    log({ k: "sendUserMessage", text, opts: opts ?? null, idle: st.idle });
    if (E.HOST_THROW === "1") throw new Error("host refused the message");
    if (E.HOST_HOLD === "1") return;
    if (st.idle) {
      st.idle = false;
      setImmediate(() => run([text]).catch((e) => log({ k: "run_error", e: String(e) })));
    } else {
      st.queue.push(text);
    }
  },
  zod: chain(),
});
const kind = E.HOST_AGENT === "sub" ? "sub" : "main";
const ctx = fallback("ctx_unknown", {
  mode: E.HOST_MODE || "tui",
  hasUI: true,
  cwd: process.cwd(),
  ui: fallback("ui_unknown", { notify: (msg, level) => log({ k: "notify", msg, level }) }),
  agent: { kind, id: kind === "sub" ? "sub-1" : "main", name: kind === "sub" ? "explore" : "main", depth: kind === "sub" ? 1 : 0 },
  sessionManager: fallback("sm_unknown", { getSessionId: () => st.id, getSessionFile: () => st.file, getCwd: () => process.cwd() }),
  isIdle: () => st.idle,
  hasPendingMessages: () => st.queue.length > 0,
  setTimeout: (fn, ms, ...a) => setTimeout(() => { try { fn(...a); } catch (e) { log({ k: "timer_error", e: String(e) }); } }, ms),
  setInterval: (fn, ms, ...a) => setInterval(() => { try { fn(...a); } catch (e) { log({ k: "timer_error", e: String(e) }); } }, ms),
  clearTimer: (h) => { clearTimeout(h); clearInterval(h); },
});
async function emit(name, payload = {}) {
  log({ k: "emit", name });
  for (const fn of handlers.get(name) ?? []) {
    try { await fn({ type: name, ...payload }, ctx); } catch (e) { log({ k: "handler_error", name, e: String(e?.stack ?? e) }); }
  }
}
async function reply(params) {
  const tool = tools.find((t) => t.name === "glen_reply");
  if (!tool) { log({ k: "no_tool" }); return; }
  const id = "call_" + (++st.calls);
  const r = await tool.execute(id, params, undefined, () => {}, ctx);
  log({ k: "tool_result", id, r });
}
const OLD = { role: "assistant", content: [{ type: "text", text: "OLD CONTEXT" }], stopReason: "stop" };
async function run(texts) {
  st.idle = false;
  await emit("before_agent_start", { reason: "prompt", prompt: texts.join("\n\n") });
  await emit("agent_start", {});
  for (const t of texts) await emit("message_end", { message: { role: "user", content: [{ type: "text", text: t }] } });
  for (const t of texts) await reply({ text: "reply: " + t, final: true });
  const a = { role: "assistant", content: [{ type: "text", text: "pong: " + texts.join(" + ") }], stopReason: "stop" };
  await emit("message_end", { message: a });
  await emit("turn_end", { message: a, toolResults: [] });
  await emit("agent_end", { messages: [OLD, a], willContinue: false });
  await sleep(Number(E.HOST_IDLE_DELAY_MS ?? "200"));
  st.idle = true;
  log({ k: "idle" });
}
async function drain() {
  await reply({ text: "own run reply" });
  const own = { role: "assistant", content: [{ type: "text", text: "own run" }], stopReason: "stop" };
  await emit("message_end", { message: own });
  await emit("turn_end", { message: own, toolResults: [] });
  await emit("agent_end", { messages: [own], willContinue: false });
  await sleep(300);
  const batch = st.queue.splice(0);
  if (batch.length) await run(batch); else st.idle = true;
}
const rl = readline.createInterface({ input: process.stdin });
rl.on("line", async (line) => {
  const [cmd, ...rest] = line.split(" ");
  try {
    if (cmd === "drain") await drain();
    else if (cmd === "talk") await reply({ text: rest.join(" ") });
    else if (cmd === "talk_invalid") await reply({ text: "   " });
    else if (cmd === "shutdown") await emit("session_shutdown", {});
    else if (cmd === "switch") { st.id = rest[0]; await emit("session_switch", { reason: rest[1], previousSessionFile: st.file }); }
    else if (cmd === "exit") { console.log("ok exit"); process.exit(0); }
  } catch (e) { log({ k: "cmd_error", cmd, e: String(e) }); }
  console.log("ok " + cmd);
});
const mod = await import(pathToFileURL(E.HOST_EXT).href);
await mod.default(pi);
log({ k: "factory_done", tools: tools.map((t) => t.name), events: [...handlers.keys()] });
await emit("session_start", {});
console.log("ready");
"#;

struct NodeEnv {
    node: PathBuf,
    flags: Vec<&'static str>,
}

/// Node ≥ 22.6, with `--experimental-strip-types` when this Node still accepts it.
fn node_env() -> Option<&'static NodeEnv> {
    static ENV: OnceLock<Option<NodeEnv>> = OnceLock::new();
    let env = ENV.get_or_init(|| {
        let path = std::env::var_os("PATH")?;
        let node = std::env::split_paths(&path)
            .map(|d| d.join("node"))
            .find(|p| p.is_file())?;
        let v = Command::new(&node).arg("--version").output().ok()?;
        let text = String::from_utf8_lossy(&v.stdout);
        let nums: Vec<u32> = text
            .trim()
            .trim_start_matches('v')
            .split('.')
            .filter_map(|n| n.parse().ok())
            .collect();
        if nums.len() < 2 || (nums[0], nums[1]) < (22, 6) {
            return None;
        }
        let flagged = Command::new(&node)
            .args(["--experimental-strip-types", "-e", "0"])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        Some(NodeEnv {
            node,
            flags: if flagged {
                vec!["--experimental-strip-types"]
            } else {
                vec![]
            },
        })
    });
    if env.is_none() {
        assert!(
            std::env::var_os("CI").is_none(),
            "Node >= 22.6 is required on CI for the extension end-to-end tests"
        );
        eprintln!("note: Node >= 22.6 not found; skipping the extension end-to-end test");
    }
    env.as_ref()
}

/// Install the extension with `glen install-extension` (baked GLEN_HOME = the box's), then copy
/// the installed file next to the host script under a `"type":"module"` package.
fn install(lb: &LiveBox) -> PathBuf {
    let agent = lb.bx.root.join("ext-agent");
    let out = lb
        .glen(["install-extension", "--json"])
        .env("PI_CODING_AGENT_DIR", &agent)
        .run();
    let d = data_of(&out, 0);
    assert_eq!(d["action"], "installed", "{d}");
    let dir = lb.bx.root.join("ext-run");
    fs::create_dir_all(&dir).unwrap();
    fs::copy(agent.join("extensions/glen/index.ts"), dir.join("index.ts")).unwrap();
    fs::write(dir.join("package.json"), r#"{"type":"module"}"#).unwrap();
    fs::write(dir.join("host.mjs"), HOST_JS).unwrap();
    dir
}

struct Host {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    pub pid: u32,
    pub log: PathBuf,
    stderr: PathBuf,
}

impl Host {
    /// Start the host for `session_id` with extra env (`HOST_*`, `GLEN_HOME`, `GLEN_MANAGED`);
    /// waits for `ready`, i.e. until the `session_start` handlers resolved.
    fn start(
        lb: &LiveBox,
        nd: &NodeEnv,
        dir: &Path,
        session_id: &str,
        session_file: Option<&Path>,
        env: &[(&str, &str)],
    ) -> Host {
        let n = fs::read_dir(dir).unwrap().count();
        let log = dir.join(format!("host{n}.ndjson"));
        let stderr = dir.join(format!("host{n}.stderr"));
        let mut cmd = Command::new(&nd.node);
        cmd.env_clear()
            .args(&nd.flags)
            .arg(dir.join("host.mjs"))
            .env("HOME", &lb.bx.home)
            .env("NODE_NO_WARNINGS", "1")
            .env("HOST_EXT", dir.join("index.ts"))
            .env("HOST_LOG", &log)
            .env("HOST_SESSION_ID", session_id)
            .current_dir(&lb.bx.alpha)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(fs::File::create(&stderr).unwrap());
        if let Some(f) = session_file {
            cmd.env("HOST_SESSION_FILE", f);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn node");
        let pid = child.id();
        let stdin = child.stdin.take().unwrap();
        let out = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(l) = line else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let mut h = Host {
            child,
            stdin,
            rx,
            pid,
            log,
            stderr,
        };
        h.expect_line("ready");
        h
    }

    fn expect_line(&mut self, want: &str) {
        let deadline = Instant::now() + LONG;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(l) if l.trim() == want => return,
                Ok(_) => {}
                Err(_) => panic!("host never printed {want:?}\n{}", self.diag()),
            }
        }
    }

    /// Run one stdin command and wait until the host finished it.
    fn cmd(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
        let first = line.split(' ').next().unwrap();
        self.expect_line(&format!("ok {first}"));
    }

    fn records(&self) -> Vec<Value> {
        records(&self.log)
    }

    fn of(&self, k: &str) -> Vec<Value> {
        self.records().into_iter().filter(|r| r["k"] == k).collect()
    }

    fn diag(&self) -> String {
        format!(
            "--- host log ---\n{}\n--- host stderr ---\n{}",
            fs::read_to_string(&self.log).unwrap_or_default(),
            fs::read_to_string(&self.stderr).unwrap_or_default()
        )
    }

    /// No handler, timer or host error was logged.
    fn assert_clean(&self) {
        let bad: Vec<Value> = self
            .records()
            .into_iter()
            .filter(|r| {
                ["handler_error", "timer_error", "run_error", "cmd_error"]
                    .contains(&r["k"].as_str().unwrap_or(""))
            })
            .collect();
        assert!(bad.is_empty(), "host errors: {bad:?}\n{}", self.diag());
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One request on a fresh connection; every frame until the server closes (8 s bound).
fn exchange(sock: &Path, bytes: &[u8]) -> Vec<Value> {
    let mut s = UnixStream::connect(sock).unwrap_or_else(|e| panic!("connect {sock:?}: {e}"));
    s.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
    let _ = s.write_all(bytes);
    let mut buf = Vec::new();
    let r = s.read_to_end(&mut buf);
    assert!(r.is_ok(), "the server did not close the connection: {r:?}");
    String::from_utf8_lossy(&buf)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("frame {l:?}: {e}")))
        .collect()
}

fn one(frames: Vec<Value>) -> Value {
    assert_eq!(frames.len(), 1, "exactly one response frame: {frames:?}");
    frames.into_iter().next().unwrap()
}

fn assert_error(f: &Value, id: Value, code: &str) {
    assert_eq!(f["v"], 1, "{f}");
    assert_eq!(f["id"], id, "{f}");
    assert_eq!(f["ok"], false, "{f}");
    assert_eq!(f["error"]["code"], code, "{f}");
    assert!(f["error"]["message"].is_string(), "{f}");
}

fn socket_of(lb: &LiveBox, h: &Host) -> PathBuf {
    lb.run_live().join(format!("{}.sock", h.pid))
}

// ---------------------------------------------------------------------------------------------
// AC-305, AC-307, AC-308, AC-313 — the endpoint.
// ---------------------------------------------------------------------------------------------

#[test]
fn node_ac305_ac308_a_tui_session_start_publishes_a_private_endpoint() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install(&lb);
    let file = lb.bx.path(&A);
    let h = Host::start(&lb, nd, &dir, A.id, Some(&file), &[]);
    let sock = socket_of(&lb, &h);

    // Published before session_start resolved, with the baked GLEN_HOME (no GLEN_HOME env).
    let reg_path = lb.registry_path(A.id);
    let reg = read_json(&reg_path)
        .unwrap_or_else(|| panic!("AC-305: no registry at ready\n{}", h.diag()));
    assert_eq!(mode(&lb.run_live()), 0o700, "run/live is 0700 when created");
    assert_eq!(mode(&reg_path), 0o600);
    let md = fs::symlink_metadata(&sock).expect("the socket exists");
    assert!(md.file_type().is_socket(), "{}", sock.display());
    assert_eq!(mode(&sock), 0o600);
    assert_eq!(reg["v"], 1, "{reg}");
    assert_eq!(reg["pid"], h.pid, "{reg}");
    assert_eq!(reg["session_id"], A.id, "{reg}");
    assert_path(&reg["session_file"], &file);
    assert_path(&reg["socket"], &sock);
    assert_eq!(reg["ext_version"], PKG_VERSION, "{reg}");
    assert!(
        reg["omp_version"].is_null() || reg["omp_version"].is_string(),
        "{reg}"
    );
    assert_eq!(reg["protocol"], 1, "{reg}");
    assert_eq!(reg["mode"], "tui", "{reg}");
    utc(&reg["started_at"]);

    // hello and state (AC-308); unknown request fields are ignored (AC-307).
    let f = one(exchange(
        &sock,
        br#"{"v":1,"id":"h1","op":"hello","args":{"client":"conformance","client_version":"0","protocols":[1]},"zzz":1}
"#,
    ));
    assert_eq!(f["v"], 1, "{f}");
    assert_eq!(f["id"], "h1", "{f}");
    assert_eq!(f["ok"], true, "{f}");
    let mut want = json!({
        "protocol": 1, "protocols": [1], "ops": ["hello", "state", "send"],
        "ext_version": PKG_VERSION, "pid": h.pid, "session_id": A.id,
        "mode": "tui", "state": "idle", "busy": false, "queued": false, "active_requests": 0,
    });
    for (k, v) in want.as_object().unwrap() {
        assert_eq!(&f["data"][k], v, "hello data `{k}`: {f}");
    }
    assert_path(&f["data"]["session_file"], &file);
    assert!(f["data"].get("omp_version").is_some(), "{f}");

    let f = one(exchange(
        &sock,
        b"{\"v\":1,\"id\":\"s1\",\"op\":\"state\"}\n",
    ));
    assert_eq!(f["ok"], true, "{f}");
    for k in ["protocol", "protocols", "ops"] {
        assert!(f["data"].get(k).is_none(), "state has no `{k}`: {f}");
        want.as_object_mut().unwrap().remove(k);
    }
    for (k, v) in want.as_object().unwrap() {
        assert_eq!(&f["data"][k], v, "state data `{k}`: {f}");
    }

    // glen reads the same endpoint (AC-318) …
    let d = data_of(&lb.glen(["ls", "--json"]).run(), 0);
    let a = row(&d, A.id);
    assert_eq!(a["state"], "live", "{a}");
    assert_eq!(a["state_source"], "extension", "{a}");
    assert_eq!(a["state_detail"], "tui", "{a}");
    h.assert_clean();
}

#[test]
fn node_ac307_protocol_errors_and_reserved_ops() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install(&lb);
    let h = Host::start(&lb, nd, &dir, A.id, None, &[]);
    let sock = socket_of(&lb, &h);

    let f = one(exchange(
        &sock,
        b"{\"v\":2,\"id\":\"x\",\"op\":\"hello\"}\n",
    ));
    assert_error(&f, json!("x"), "unsupported_version");
    assert_eq!(f["error"]["details"]["supported"], json!([1]), "{f}");

    for op in RESERVED_OPS {
        let f = one(exchange(
            &sock,
            format!("{{\"v\":1,\"id\":\"r\",\"op\":\"{op}\",\"args\":{{}}}}\n").as_bytes(),
        ));
        assert_error(&f, json!("r"), "unsupported_op");
        assert_eq!(
            f["error"]["details"],
            json!({"op": op, "ops": OPS, "reserved": true}),
            "{f}"
        );
    }
    let f = one(exchange(
        &sock,
        b"{\"v\":1,\"id\":\"u\",\"op\":\"frobnicate\"}\n",
    ));
    assert_error(&f, json!("u"), "unsupported_op");
    assert_eq!(
        f["error"]["details"],
        json!({"op": "frobnicate", "ops": OPS, "reserved": false}),
        "{f}"
    );

    let f = one(exchange(&sock, b"{this is not json\n"));
    assert_error(&f, Value::Null, "bad_request");
    let f = one(exchange(&sock, b"{\"v\":1,\"op\":\"hello\"}\n"));
    assert_error(&f, Value::Null, "bad_request");
    let f = one(exchange(&sock, b"{\"v\":1,\"id\":7,\"op\":\"hello\"}\n"));
    assert_eq!(f["error"]["code"], "bad_request", "a non-string id: {f}");

    // A line over 4 MiB.
    let mut big = b"{\"v\":1,\"id\":\"big\",\"op\":\"hello\",\"pad\":\"".to_vec();
    big.extend(std::iter::repeat_n(b'x', 4 * 1024 * 1024 + 1));
    big.extend_from_slice(b"\"}\n");
    let f = one(exchange(&sock, &big));
    assert_eq!(f["error"]["code"], "bad_request", "{f}");

    // A connection with no complete line is closed after about 5 s.
    let mut s = UnixStream::connect(&sock).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(9))).unwrap();
    s.write_all(b"{\"v\":1,").unwrap();
    let t = Instant::now();
    let mut buf = Vec::new();
    assert!(
        s.read_to_end(&mut buf).is_ok(),
        "AC-307: idle connection closed"
    );
    assert!(
        t.elapsed() >= Duration::from_secs(4) && t.elapsed() < Duration::from_secs(8),
        "closed after {:?}",
        t.elapsed()
    );

    // The endpoint still serves after all of that.
    let f = one(exchange(
        &sock,
        b"{\"v\":1,\"id\":\"ok\",\"op\":\"state\"}\n",
    ));
    assert_eq!(f["ok"], true, "{f}");
    h.assert_clean();
}

#[test]
fn node_ac313_bind_sweeps_dead_leftovers_and_shutdown_or_exit_removes_the_files() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install(&lb);
    let dead = dead_pid();
    mkdir_private(&lb.run_live());
    let dead_sock = lb.run_live().join(format!("{dead}.sock"));
    stale_socket(&dead_sock);
    let dead_reg = write_registry_doc(&lb, D.id, &registry_doc(D.id, dead, &dead_sock));
    let keep_sock = lb.run_live().join(format!("{}.sock", std::process::id()));
    stale_socket(&keep_sock);
    let keep_reg = write_registry_doc(
        &lb,
        C.id,
        &registry_doc(C.id, std::process::id(), &keep_sock),
    );

    let mut h = Host::start(&lb, nd, &dir, A.id, None, &[]);
    assert!(
        !dead_reg.exists() && !dead_sock.exists(),
        "AC-313: dead-pid files swept"
    );
    assert!(
        keep_reg.exists() && keep_sock.exists(),
        "a live pid's files are kept"
    );
    let sock = socket_of(&lb, &h);
    let reg = lb.registry_path(A.id);
    assert!(sock.exists() && reg.exists());
    h.cmd("shutdown");
    assert!(
        !sock.exists() && !reg.exists(),
        "AC-313: shutdown unlinks socket and registry"
    );
    h.assert_clean();

    // The process `exit` backstop, without session_shutdown.
    let mut h = Host::start(&lb, nd, &dir, B.id, None, &[]);
    let (sock, reg) = (socket_of(&lb, &h), lb.registry_path(B.id));
    assert!(sock.exists() && reg.exists());
    h.cmd("exit");
    poll_until("the host to exit", LONG, || h.child.try_wait().unwrap());
    assert!(
        !sock.exists() && !reg.exists(),
        "AC-313: the exit handler unlinks too"
    );
}

// ---------------------------------------------------------------------------------------------
// AC-306, AC-305, AC-321 — where no endpoint is bound.
// ---------------------------------------------------------------------------------------------

#[test]
fn node_ac306_managed_processes_register_nothing() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install(&lb);
    let mut h = Host::start(&lb, nd, &dir, A.id, None, &[("GLEN_MANAGED", "1")]);
    let done = h.of("factory_done");
    assert_eq!(done[0]["tools"], json!([]), "no tool\n{}", h.diag());
    assert_eq!(done[0]["events"], json!([]), "no handler\n{}", h.diag());
    h.cmd("talk hello");
    assert!(!h.of("no_tool").is_empty());
    assert!(!lb.glen_home().exists(), "AC-306: no file under GLEN_HOME");
}

#[test]
fn node_ac305_no_endpoint_outside_a_main_tui_session_but_the_tool_stays() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install(&lb);
    for env in [
        vec![("HOST_MODE", "rpc")],
        vec![("HOST_MODE", "print")],
        vec![("HOST_AGENT", "sub")],
    ] {
        let h = Host::start(&lb, nd, &dir, A.id, None, &env);
        let done = h.of("factory_done");
        assert_eq!(
            done[0]["tools"],
            json!(["glen_reply"]),
            "{env:?}\n{}",
            h.diag()
        );
        assert!(
            list_tree(&lb.run_live()).is_empty(),
            "{env:?}: no endpoint: {:?}",
            list_tree(&lb.run_live())
        );
        h.assert_clean();
    }
    // A session id that is not a plain token.
    let h = Host::start(&lb, nd, &dir, "not a token!", None, &[]);
    assert!(
        list_tree(&lb.run_live()).is_empty(),
        "{:?}",
        list_tree(&lb.run_live())
    );
    drop(h);
    // A relative GLEN_HOME binds nothing and creates nothing relative to the cwd.
    let before = list_tree(&lb.bx.alpha);
    let h = Host::start(&lb, nd, &dir, A.id, None, &[("GLEN_HOME", "rel/home")]);
    assert_eq!(list_tree(&lb.bx.alpha), before);
    assert!(list_tree(&lb.run_live()).is_empty());
    drop(h);
}

#[test]
fn node_ac321_a_glen_home_at_the_socket_limit_binds_nothing_and_notifies_once() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install(&lb);
    let probe = lb.bx.root.join("h");
    // Counted with a one-digit pid, so the real `<pid>.sock` is at or over the limit for any pid;
    // two-byte padding keeps the character count below it (AC-321: bytes, not characters).
    let base = s(&probe).len() + "/run/live/".len() + 1 + ".sock".len();
    let pad = "é".repeat(SUN_LIMIT.saturating_sub(base) / 2 + 1);
    let home = format!("{}{pad}", s(&probe));
    let h = Host::start(&lb, nd, &dir, A.id, None, &[("GLEN_HOME", &home)]);
    let socks: Vec<String> = list_tree(Path::new(&home))
        .into_iter()
        .filter(|p| p.ends_with(".sock") || p.ends_with(".json"))
        .collect();
    assert!(socks.is_empty(), "AC-321: nothing bound: {socks:?}");
    assert_eq!(h.of("notify").len(), 1, "AC-321: one notice\n{}", h.diag());
    h.assert_clean();
}

// ---------------------------------------------------------------------------------------------
// AC-309, AC-310, AC-311, AC-316 — glen send into the real extension.
// ---------------------------------------------------------------------------------------------

#[test]
fn node_ac309_ac310_ac316_an_idle_session_gets_a_prompt_and_settles_only_once_idle() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install(&lb);
    let h = Host::start(
        &lb,
        nd,
        &dir,
        A.id,
        Some(&lb.bx.path(&A)),
        &[("HOST_IDLE_DELAY_MS", "1500")],
    );
    let out = lb
        .send(A.handle, "hi", &["--wait", "20s"])
        .env("GLEN_OMP_BIN", "/nonexistent/omp")
        .run();
    let d = data_of(&out, 0);
    let id = job_id(&d);
    assert_eq!(d["job"]["route"], "live", "{d}");
    assert_eq!(d["delivered_as"], "prompt", "{d}");
    assert_eq!(
        d["assistant_text"], "pong: hi",
        "AC-310: from the run's message_end, never agent_end.messages: {d}"
    );
    assert_eq!(d["stop_reason"], "stop", "{d}");
    assert_eq!(d["session_settled"], true, "{d}");
    assert!(
        out.elapsed >= Duration::from_millis(1500),
        "AC-310: settled only after isIdle() turned true ({:?})",
        out.elapsed
    );

    // AC-309: no options when idle.
    let sends = h.of("sendUserMessage");
    assert_eq!(sends.len(), 1, "{}", h.diag());
    assert_eq!(sends[0]["text"], "hi");
    assert!(sends[0]["opts"].is_null(), "{}", sends[0]);

    // AC-311: the tool, its record and its attribution.
    let tool = &h.of("registerTool")[0];
    assert_eq!(tool["name"], "glen_reply");
    assert_eq!(tool["loadMode"], "essential", "{tool}");
    assert_eq!(tool["approval"], "read", "{tool}");
    let r = &h.of("tool_result")[0]["r"];
    assert_eq!(r["content"][0]["text"], "delivered", "{r}");
    assert!(r["isError"].is_null() || r["isError"] == false, "{r}");
    let replies = arr(&d["replies"]);
    assert_eq!(replies.len(), 1, "{d}");
    let st = lb.status(&id);
    let rec = &replies[0];
    for k in [
        "v",
        "id",
        "session_id",
        "job_id",
        "request_id",
        "tool_call_id",
        "text",
        "final",
        "at",
    ] {
        assert!(rec.get(k).is_some(), "record lacks `{k}`: {rec}");
    }
    assert_eq!(rec["session_id"], A.id, "{rec}");
    assert_eq!(rec["job_id"], id.as_str(), "{rec}");
    assert_eq!(rec["request_id"], st["request_id"], "{rec}");
    assert_eq!(rec["tool_call_id"], "call_1", "{rec}");
    assert_eq!(rec["text"], "reply: hi", "{rec}");
    assert_eq!(rec["final"], true, "{rec}");
    utc(&rec["at"]);
    let inbox = ndjson(&lb.inbox_file(A.id));
    assert_eq!(
        inbox,
        std::slice::from_ref(rec),
        "the extension's inbox line, once"
    );
    assert_eq!(mode(&lb.inbox_file(A.id)), 0o600);
    lb.assert_quiet();
    lb.assert_no_omp();
    h.assert_clean();
}

#[test]
fn node_ac309_ac311_busy_sends_queue_and_replies_follow_consumption() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install(&lb);
    let mut h = Host::start(&lb, nd, &dir, A.id, None, &[("HOST_BUSY", "1")]);
    let d = data_of(&lb.glen(["ls", "--json"]).run(), 0);
    assert_eq!(
        row(&d, A.id)["state"],
        "busy",
        "AC-318 from the real extension"
    );

    let j1 = job_id(&data_of(
        &lb.send(A.handle, "one", &["--no-wait"]).run(),
        10,
    ));
    lb.wait_status(&j1, "accepted", LONG, |s| s["status"] == "accepted");
    let j2 = job_id(&data_of(
        &lb.send(A.handle, "two", &["--steer", "--no-wait"]).run(),
        10,
    ));
    lb.wait_status(&j2, "accepted", LONG, |s| s["status"] == "accepted");
    assert_eq!(lb.status(&j1)["delivered_as"], "follow_up");
    assert_eq!(lb.status(&j2)["delivered_as"], "steer");
    let sends = h.of("sendUserMessage");
    assert_eq!(
        sends[0]["opts"],
        json!({"deliverAs": "followUp"}),
        "{}",
        h.diag()
    );
    assert_eq!(
        sends[1]["opts"],
        json!({"deliverAs": "steer"}),
        "{}",
        h.diag()
    );

    // The TUI user's own run ends (with its own glen_reply), then the queued batch runs.
    h.cmd("drain");
    for (j, text) in [(&j1, "one"), (&j2, "two")] {
        let w = data_of(
            &lb.glen(["wait", &job_handle(j), "--timeout", "20s", "--json"])
                .run(),
            0,
        );
        assert_eq!(w["assistant_text"], "pong: one + two", "{text}: {w}");
    }
    let inbox = ndjson(&lb.inbox_file(A.id));
    let by_text = |t: &str| {
        inbox
            .iter()
            .find(|r| r["text"] == t)
            .unwrap_or_else(|| panic!("no inbox record {t:?}: {inbox:?}"))
            .clone()
    };
    let own = by_text("own run reply");
    assert!(
        own["job_id"].is_null() && own["request_id"].is_null(),
        "AC-311: a queued (unconsumed) request never borrows the user's reply: {own}"
    );
    for t in ["reply: one", "reply: two"] {
        assert_eq!(
            by_text(t)["job_id"],
            j2.as_str(),
            "AC-311: the newest consumed unsettled request"
        );
    }
    lb.assert_quiet();
    h.assert_clean();
}

#[test]
fn node_ac311_talk_back_from_the_tui_reaches_glen_inbox_with_null_ids() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install(&lb);
    let mut h = Host::start(&lb, nd, &dir, A.id, None, &[]);
    h.cmd("talk hello from the TUI");
    h.cmd("talk_invalid");
    let results = h.of("tool_result");
    assert_eq!(
        results[0]["r"]["content"][0]["text"], "delivered",
        "{results:?}"
    );
    assert_eq!(
        results[1]["r"]["isError"], true,
        "invalid args: {results:?}"
    );
    let lines = ndjson(&lb.inbox_file(A.id));
    assert_eq!(lines.len(), 1, "invalid calls record nothing: {lines:?}");
    assert_eq!(mode(&lb.glen_home().join("inbox")), 0o700);
    let d = data_of(&lb.glen(["inbox", "--json"]).run(), 0);
    let m = &arr(&d["messages"])[0];
    assert_eq!(m["text"], "hello from the TUI", "{m}");
    assert_eq!(m["session_id"], A.id, "{m}");
    assert!(m["job_id"].is_null() && m["request_id"].is_null(), "{m}");
    assert!(m["job_handle"].is_null(), "{m}");
    assert_eq!(m["session_handle"], A.handle, "{m}");
    h.assert_clean();
}

// ---------------------------------------------------------------------------------------------
// AC-312, AC-313, AC-316 — identity changes and shutdown with a request in flight.
// ---------------------------------------------------------------------------------------------

#[test]
fn node_ac313_ac316_shutdown_fails_the_active_job_omp_exited() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install(&lb);
    let mut h = Host::start(&lb, nd, &dir, A.id, None, &[("HOST_HOLD", "1")]);
    let j = job_id(&data_of(&lb.send(A.handle, "hi", &["--no-wait"]).run(), 10));
    lb.wait_status(&j, "accepted", LONG, |s| s["status"] == "accepted");
    h.cmd("shutdown");
    let st = lb.wait_terminal(&j);
    assert_eq!(st["status"], "failed", "{st}");
    assert_eq!(st["error"]["code"], "omp_exited", "{st}");
    assert!(lb.job_dir(&j).join("result.json").is_file());
    lb.assert_quiet();
    h.assert_clean();
}

#[test]
fn node_ac312_a_new_session_rekeys_the_registry_and_ends_the_request() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install(&lb);
    let mut h = Host::start(&lb, nd, &dir, A.id, None, &[("HOST_HOLD", "1")]);
    let j = job_id(&data_of(&lb.send(A.handle, "hi", &["--no-wait"]).run(), 10));
    lb.wait_status(&j, "accepted", LONG, |s| s["status"] == "accepted");
    let new_id = "01a2ffff-0312-7000-8000-0000000a3120";
    h.cmd(&format!("switch {new_id} new"));
    let st = lb.wait_terminal(&j);
    assert_eq!(st["status"], "aborted", "{st}");
    assert_eq!(st["error"]["code"], "session_switched", "{st}");
    let reg = read_json(&lb.registry_path(new_id)).expect("AC-312: the new registry");
    assert_eq!(reg["pid"], h.pid, "{reg}");
    assert_eq!(reg["session_id"], new_id, "{reg}");
    assert!(
        !lb.registry_path(A.id).exists(),
        "AC-312: the old registry is removed"
    );
    let f = one(exchange(
        &socket_of(&lb, &h),
        b"{\"v\":1,\"id\":\"s\",\"op\":\"state\"}\n",
    ));
    assert_eq!(f["data"]["session_id"], new_id, "{f}");
    lb.assert_quiet();
    h.assert_clean();
}

// ---------------------------------------------------------------------------------------------
// AC-309 — the send op on the wire.
// ---------------------------------------------------------------------------------------------

/// Write one request and read only the first frame (a `send` connection stays open for events).
fn first_frame(sock: &Path, req: &Value) -> (Value, UnixStream) {
    let mut s = UnixStream::connect(sock).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
    s.write_all(format!("{req}\n").as_bytes()).unwrap();
    let mut line = String::new();
    BufReader::new(s.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    let v = serde_json::from_str(&line).unwrap_or_else(|e| panic!("first frame {line:?}: {e}"));
    (v, s)
}

fn send_req(id: &str, text: &str, deliver_as: &str, expect: Option<&str>) -> Value {
    json!({"v":1,"id":id,"op":"send","args":{"text":text,"deliver_as":deliver_as,"job_id":null,"expect_session_id":expect}})
}

#[test]
fn node_ac309_an_idle_session_takes_every_mode_as_a_prompt() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install(&lb);
    let h = Host::start(&lb, nd, &dir, A.id, None, &[]);
    for flags in [&["--steer"][..], &["--follow-up"][..]] {
        let mut args = vec!["--wait", "20s"];
        args.extend_from_slice(flags);
        let d = data_of(&lb.send(A.handle, "idle", &args).run(), 0);
        assert_eq!(d["delivered_as"], "prompt", "{flags:?}: {d}");
    }
    let sends = h.of("sendUserMessage");
    assert_eq!(sends.len(), 2, "{}", h.diag());
    for s in &sends {
        assert!(s["opts"].is_null(), "AC-309: no options while idle: {s}");
    }
    lb.assert_quiet();
    h.assert_clean();
}

#[test]
fn node_ac309_send_validation_mismatch_and_duplicates_never_inject() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install(&lb);
    let h = Host::start(&lb, nd, &dir, A.id, None, &[("HOST_HOLD", "1")]);
    let sock = socket_of(&lb, &h);

    let (f, _) = first_frame(&sock, &send_req("m1", "hi", "auto", Some(B.id)));
    assert_error(&f, json!("m1"), "session_mismatch");
    assert_eq!(f["error"]["details"]["session_id"], A.id, "{f}");
    let big = "x".repeat(524_289);
    for (id, req) in [
        ("b1", send_req("b1", "   ", "auto", Some(A.id))),
        ("b2", send_req("b2", "hi", "sideways", Some(A.id))),
        ("b3", send_req("b3", &big, "auto", Some(A.id))),
    ] {
        let (f, _) = first_frame(&sock, &req);
        assert_error(&f, json!(id), "bad_request");
    }
    assert!(
        h.of("sendUserMessage").is_empty(),
        "nothing injected\n{}",
        h.diag()
    );

    // A valid send: the ack is the first frame; the same id again is refused.
    let (ack, _conn) = first_frame(&sock, &send_req("d1", "hi", "auto", Some(A.id)));
    assert_eq!(ack["v"], 1, "{ack}");
    assert_eq!(ack["id"], "d1", "{ack}");
    assert_eq!(ack["ok"], true, "{ack}");
    assert_eq!(ack["data"]["delivered_as"], "prompt", "{ack}");
    assert_eq!(ack["data"]["session_id"], A.id, "{ack}");
    assert_eq!(ack["data"]["request_id"], "d1", "{ack}");
    let (f, _) = first_frame(&sock, &send_req("d1", "hi again", "auto", Some(A.id)));
    assert_error(&f, json!("d1"), "bad_request");
    assert_eq!(
        h.of("sendUserMessage").len(),
        1,
        "one injection\n{}",
        h.diag()
    );
    let st = one(exchange(
        &sock,
        b"{\"v\":1,\"id\":\"s\",\"op\":\"state\"}\n",
    ));
    assert_eq!(st["data"]["active_requests"], 1, "{st}");
    h.assert_clean();

    // sendUserMessage throwing → send_failed.
    let h2 = Host::start(&lb, nd, &dir, B.id, None, &[("HOST_THROW", "1")]);
    let (f, _) = first_frame(
        &socket_of(&lb, &h2),
        &send_req("t1", "hi", "auto", Some(B.id)),
    );
    assert_error(&f, json!("t1"), "send_failed");
}
