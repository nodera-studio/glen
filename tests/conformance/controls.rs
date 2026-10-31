//! W3b session controls: `glen abort` (alias `stop`), `models`, `model`, `thinking` and `compact`
//! (`.claude/plans/v010-wave-3b-controls.md`, AC-351…AC-369 and the orchestrator decisions
//! O1–O8; docs/design.md §A3, §A4 "Controls", §A6, §A7, §A9–§A11, §A15; docs/json.md
//! `glen abort|stop|models|model|thinking|compact`). Written from the plan and the docs only,
//! without reading `src/ctl/`, `src/commands/ctl.rs`, the extension source or `tests/controls.rs`.
//!
//! Peers: `fake_omp` for the `rpc` and `job` routes (through `GLEN_OMP_BIN`), `fake_ext` and
//! in-test unix-socket endpoints for the `live` route, and the real bundled extension under Node
//! with a host written here from omp's documented extension API (the `node_*` cases). Every run
//! sets `FAKE_OMP_SERIAL=1`, omp's serial command queue (plan Step 5).
//!
//! Readings chosen where the plan is ambiguous (each is reported with the run):
//! - R1 AC-351 names `install-skill` as hidden, but W4 (merged before this branch was cut) made it
//!   visible and docs/design.md §A9 keeps it visible: the expected list includes it.
//! - R2 O1 adds `stop` as a visible *alias* of `abort`; AC-351's "exactly" list counts commands,
//!   so `stop` is checked as an alias (same envelope), not as a help row.
//! - R3 `models <s>` falling back to the AC-357 listing reports `route:"rpc"` (the listing's route).
//! - R4 `compact --instructions` over 524,288 bytes is an argument error: exit 2 `usage`.
//! - R5 an endpoint that appears while a control waits for a held glen lock is taken once the
//!   lock is free (plan Step 7: wait for the lock, then restart routing at step 1).
//! - R6 `set_thinking_level` with a level outside the seven names is `bad_request` on the socket
//!   (Command contract: `level` ∈ the seven values), not `invalid_level`.
//!
//! Manual (plan Step 12, live omp only; not covered here): real omp writing and restoring the
//! `model_change` / `thinking_level_change` entries across a resume (AC-355 live half), the 7 s
//! abort on a real TUI (AC-356 live half), omp's real `No API key` / `Model not found` texts,
//! real compaction, and omp's global `config.yml` staying unchanged.
//! Not reachable with the fakes: `thinking_clamped` (both fakes accept every level glen lets
//! through), compact refusing a streaming/compacting/queued *RPC* session (the fake never reports
//! one while idle), the lease re-probe immediately before spawning (a race window), and the 13 s
//! escalation against an omp that ignores `abort` (W2 covers escalation with `wedge`).
//!
//! Run: `cargo test --locked --test conformance controls`

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime};

use rustix::process::Signal;
use serde_json::{Value, json};
use xxhash_rust::xxh64::xxh64;

use crate::live_support::*;
use crate::reader::*;
use crate::rpc_support::*;
use crate::support::*;

// ---------------------------------------------------------------------------------------------
// Contract constants.
// ---------------------------------------------------------------------------------------------

/// AC-361: the only levels `thinking` accepts at parse time.
const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// AC-351 (with R1): the visible commands.
const VISIBLE_W3B: [&str; 17] = [
    "ls",
    "show",
    "tail",
    "send",
    "wait",
    "jobs",
    "abort",
    "models",
    "model",
    "thinking",
    "compact",
    "inbox",
    "install-extension",
    "install-skill",
    "doctor",
    "completions",
    "help",
];

/// AC-360 / docs/json.md `glen model`.
const MODEL_KEYS: [&str; 10] = [
    "session",
    "route",
    "model",
    "previous",
    "changed",
    "thinking_level",
    "thinking_levels",
    "busy",
    "scope",
    "forked",
];
/// AC-361 / docs/json.md `glen thinking`.
const THINKING_KEYS: [&str; 11] = [
    "session",
    "route",
    "thinking_level",
    "requested",
    "previous",
    "changed",
    "model",
    "available",
    "busy",
    "scope",
    "forked",
];
/// AC-362 / docs/json.md `glen compact`.
const COMPACT_KEYS: [&str; 6] = [
    "session",
    "route",
    "compacted",
    "tokens_before",
    "tokens_after",
    "forked",
];
/// AC-356 / docs/json.md `glen abort`.
const ABORT_KEYS: [&str; 7] = [
    "session",
    "route",
    "was",
    "aborted",
    "idle_after",
    "queued",
    "queued_jobs",
];
/// AC-357, AC-358 / docs/json.md `glen models`.
const MODELS_KEYS: [&str; 9] = [
    "session",
    "route",
    "current",
    "current_source",
    "thinking_level",
    "thinking_configured",
    "default_model",
    "total",
    "models",
];
/// A `models` row (AC-357).
const ROW_KEYS: [&str; 8] = [
    "provider",
    "id",
    "name",
    "reasoning",
    "thinking_levels",
    "context_window",
    "max_tokens",
    "current",
];
/// The op-level `ModelRow` (Command contract): a row without the CLI's `current` flag.
const OP_ROW_KEYS: [&str; 7] = [
    "provider",
    "id",
    "name",
    "reasoning",
    "thinking_levels",
    "context_window",
    "max_tokens",
];
/// `fake/fake-1`'s levels: `off` plus its efforts.
const FAKE1_LEVELS: [&str; 4] = ["off", "low", "medium", "high"];
/// omp RPC commands that change session or global behaviour settings (AC-355: never sent).
const SETTINGS_COMMANDS: [&str; 6] = [
    "set_auto_compaction",
    "set_steering_mode",
    "set_follow_up_mode",
    "set_interrupt_mode",
    "set_auto_retry",
    "set_fast_mode",
];
/// omp commands that act on the session (AC-354: none before the baseline identity check).
const SESSION_OPS: [&str; 6] = [
    "get_available_models",
    "set_model",
    "get_available_thinking_levels",
    "set_thinking_level",
    "compact",
    "abort",
];
const T0: &str = "2026-10-06T10:00:00.000Z";
const NO_OMP: &str = "/nonexistent/omp";

// ---------------------------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------------------------

/// `glen <args> --json` from `$HOME/work/alpha` with the box environment.
fn ctl(lb: &LiveBox, args: &[&str]) -> Run {
    let mut a: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    a.push("--json".into());
    lb.glen(a).env("FAKE_OMP_SERIAL", "1")
}

/// `glen <args> --json` started in the background (piped stdout/stderr).
fn spawn_ctl(lb: &LiveBox, args: &[&str], env: &[(&str, &str)]) -> Child {
    let mut a: Vec<&str> = args.to_vec();
    a.push("--json");
    let mut cmd = lb.command(&a);
    cmd.env("FAKE_OMP_SERIAL", "1");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.spawn().expect("spawn glen")
}

fn has_keys(v: &Value, keys: &[&str], what: &str) {
    let o = v
        .as_object()
        .unwrap_or_else(|| panic!("{what} is not an object: {v}"));
    for k in keys {
        assert!(o.contains_key(*k), "{what} lacks `{k}`: {v}");
    }
}

/// AC-367: `data.session` is `{id,handle,path}`; `path` is absolute (or null when the session has
/// no file yet).
fn assert_session<P: AsRef<Path>>(d: &Value, id: &str, path: Option<P>) {
    let s = &d["session"];
    has_keys(s, &["id", "handle", "path"], "data.session");
    assert_eq!(s["id"], id, "data.session.id: {d}");
    let h = s["handle"]
        .as_str()
        .unwrap_or_else(|| panic!("data.session.handle is not a string: {d}"))
        .to_lowercase();
    let simple: String = id.chars().filter(|c| *c != '-').collect::<String>();
    assert!(
        h.len() >= 7 && simple.to_lowercase().ends_with(&h),
        "§A8: the handle {h:?} is a tail of {id}: {d}"
    );
    match path {
        Some(p) => assert_path(&s["path"], p.as_ref()),
        None => assert!(
            s["path"].is_null(),
            "an unwritten session has path null: {d}"
        ),
    }
}

/// An error envelope `control_failed` (exit 7) with `details.reason`.
fn control_failed(out: &Out, reason: &str) -> Value {
    let err = failed(out, "control_failed");
    assert_eq!(
        err["details"]["reason"],
        reason,
        "details.reason\n{}",
        out.ctx()
    );
    err
}

fn mref(provider: &str, id: &str) -> Value {
    json!({"provider": provider, "id": id})
}

fn strs_json(v: &[&str]) -> Value {
    json!(v)
}

fn warned(env: &Value, code: &str) -> bool {
    warning_codes(env).iter().any(|c| c == code)
}

fn fake_models() -> Vec<Value> {
    let text = fs::read_to_string(repo_root().join("tests/support/fake_models.json"))
        .expect("tests/support/fake_models.json");
    serde_json::from_str(&text).expect("fake_models.json is JSON")
}

/// The AC-357 rows for the fakes' models, derived from the plan's rule (`thinking_levels` =
/// `["off"]` plus the efforts when `reasoning`), sorted by provider then id; no `current` flag.
fn expected_rows() -> Vec<Value> {
    let mut rows: Vec<Value> = fake_models()
        .iter()
        .map(|m| {
            let mut levels = vec![json!("off")];
            if m["reasoning"] == true {
                levels.extend(
                    m["thinking"]["efforts"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default(),
                );
            }
            json!({
                "provider": m["provider"], "id": m["id"], "name": m["name"],
                "reasoning": m["reasoning"], "thinking_levels": levels,
                "context_window": m["contextWindow"], "max_tokens": m["maxTokens"],
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        let key = |r: &Value| {
            (
                r["provider"].as_str().unwrap().to_owned(),
                r["id"].as_str().unwrap().to_owned(),
            )
        };
        key(a).cmp(&key(b))
    });
    rows
}

/// `data.models` holds exactly the expected rows in order (unknown extra fields allowed), and
/// `current` is true only on `current`. `data.total` counts them.
fn assert_rows(d: &Value, current: Option<(&str, &str)>) {
    let got = arr(&d["models"]);
    let want = expected_rows();
    assert_eq!(got.len(), want.len(), "row count: {d}");
    assert_eq!(d["total"], want.len(), "total: {d}");
    for (g, w) in got.iter().zip(&want) {
        has_keys(g, &ROW_KEYS, "a models row");
        for k in OP_ROW_KEYS {
            assert_eq!(g[k], w[k], "row `{k}` (sorted by provider then id): {g}");
        }
        let is_current = current.is_some_and(|(p, i)| w["provider"] == p && w["id"] == i);
        assert_eq!(g["current"], is_current, "row `current`: {g}");
    }
}

/// `(provider, id)` of every row of `data.models`.
fn pairs(d: &Value) -> Vec<(String, String)> {
    arr(&d["models"])
        .iter()
        .map(|r| {
            (
                r["provider"].as_str().unwrap_or("").to_owned(),
                r["id"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect()
}

/// An assistant message answered by `provider/model`.
fn answered_by(id: &str, parent: Option<&str>, provider: &str, model: &str) -> Value {
    message(
        id,
        parent,
        T0,
        json!({
            "role": "assistant", "content": [{"type": "text", "text": "ok"}],
            "api": "openai-completions", "provider": provider, "model": model,
            "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                      "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}},
            "stopReason": "stop", "timestamp": 0
        }),
    )
}

fn thinking_change(id: &str, parent: Option<&str>, level: &str, configured: &str) -> Value {
    entry(
        "thinking_level_change",
        id,
        parent,
        T0,
        json!({"thinkingLevel": level, "configured": configured}),
    )
}

fn model_change(id: &str, parent: Option<&str>, model: &str, role: Option<&str>) -> Value {
    let mut extra = json!({"model": model});
    if let Some(r) = role {
        extra["role"] = json!(r);
    }
    entry("model_change", id, parent, T0, extra)
}

/// A session in `-work-alpha` with header cwd `cwd`; returns (id, path).
fn add_session(lb: &LiveBox, n: u64, cwd: &Path, body: &[Value]) -> (String, PathBuf) {
    let id = uuid(n);
    let cwd = s(cwd);
    let path = lb.bx.add(ALPHA, T0, &id, Some(cwd.as_str()), body);
    (id, path)
}

/// Session X: thinking `medium` (configured `medium`), default model `fake/fake-1`, one exchange
/// answered by `fake/fake-1`.
fn add_x(lb: &LiveBox, n: u64, cwd: &Path) -> (String, PathBuf) {
    add_session(
        lb,
        n,
        cwd,
        &[
            thinking_change("x0000001", None, "medium", "medium"),
            model_change("x0000002", Some("x0000001"), "fake/fake-1", Some("default")),
            user("x0000003", Some("x0000002"), T0, "hello"),
            answered_by("x0000004", Some("x0000003"), "fake", "fake-1"),
        ],
    )
}

/// The JSON lines appended to `path` after its original bytes `before` (which must be intact).
fn appended(path: &Path, before: &[u8]) -> Vec<Value> {
    let after = fs::read(path).unwrap();
    assert!(
        after.starts_with(before),
        "AC-368: the session file was rewritten, not appended to: {}",
        path.display()
    );
    String::from_utf8_lossy(&after[before.len()..])
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("appended line {l:?}: {e}")))
        .collect()
}

fn argv_of(start: &Value) -> Vec<String> {
    strings(&start["argv"])
}

fn inbound_types(lb: &LiveBox) -> Vec<String> {
    lb.recs()
        .iter()
        .filter(|r| r["event"] == "in")
        .filter_map(|r| r["frame"]["type"].as_str().map(str::to_owned))
        .collect()
}

/// No inbound frame of any of `kinds` reached the fake omp.
fn assert_no_frames(lb: &LiveBox, kinds: &[&str]) {
    let got = inbound_types(lb);
    for k in kinds {
        assert!(
            !got.iter().any(|t| t == k),
            "a `{k}` frame reached omp: {got:?}\n{}",
            lb.diag()
        );
    }
}

/// AC-355: glen never asked omp to change a behaviour setting.
fn assert_no_settings_frames(lb: &LiveBox) {
    assert_no_frames(lb, &SETTINGS_COMMANDS);
    for r in lb.recs().iter().filter(|r| r["event"] == "in") {
        assert!(
            r["frame"].get("persist").is_none(),
            "AC-355: a frame asked omp to persist: {r}"
        );
    }
}

/// The omp process of `start` and its process group have exited.
fn assert_omp_gone(start: &Value) {
    let pid = start["pid"].as_i64().unwrap_or(0);
    let pgid = start["pgid"].as_i64().unwrap_or(0);
    let mine = own_pgid();
    poll_until(
        "the omp process and its group to be gone",
        Duration::from_secs(3),
        || (!pid_alive(pid) && (pgid == mine || !group_alive(pgid))).then_some(()),
    );
}

/// Ops the extension peer received, without the `hello`/`state` handshake.
fn control_ops_in(ext: &Ext) -> Vec<String> {
    ext.ops_in()
        .into_iter()
        .filter(|o| o != "hello" && o != "state")
        .collect()
}

fn ext_frames_of(ext: &Ext, name: &str) -> Vec<Value> {
    ext.frames("in")
        .into_iter()
        .filter(|f| f["op"] == name)
        .collect()
}

fn session_file_arg(path: &Path) -> [String; 2] {
    ["--session-file".into(), s(path)]
}

fn start_ext_for_a(lb: &LiveBox, scenario: &str, env: &[(&str, &str)]) -> Ext {
    let file = lb.bx.path(&A);
    let a = session_file_arg(&file);
    lb.start_ext(A.id, scenario, env, &[a[0].as_str(), a[1].as_str()])
}

/// A job on fixture A whose run is held until `release` exists; returns its id once accepted.
fn held_job(lb: &LiveBox, release: &Path, env: &[(&str, &str)]) -> String {
    let mut run = lb
        .glen(["send", A.handle, "hold", "--no-wait", "--json"])
        .env("FAKE_OMP_SERIAL", "1")
        .env("FAKE_OMP_RUN_RELEASE", release);
    for (k, v) in env {
        run = run.env(k, v);
    }
    let id = job_id(&data_of(&run.run(), 10));
    lb.wait_status(&id, "accepted", LONG, |s| s["status"] == "accepted");
    id
}

fn release(path: &Path) {
    fs::write(path, b"").unwrap();
}

fn now_ms() -> u64 {
    ms_since_epoch(SystemTime::now())
}

fn control_dir(lb: &LiveBox, job: &str) -> PathBuf {
    lb.job_dir(job).join("control")
}

/// Request files (`<name>.json`, not dotfiles, not answers) in a control dir.
fn request_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let n = p.file_name().unwrap().to_string_lossy().into_owned();
                    n.ends_with(".json") && !n.ends_with(".done.json") && !n.starts_with('.')
                })
                .collect()
        })
        .unwrap_or_default()
}

fn done_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with(".done.json"))
                .collect()
        })
        .unwrap_or_default()
}

/// Drop control request `<name>.json` for `job` and return its `<name>.done.json` answer (§A7).
fn ask(lb: &LiveBox, job: &str, name: &str, body: &Value) -> Value {
    write_control(lb, job, &format!("{name}.json"), &body.to_string());
    let dir = control_dir(lb, job);
    let done = lb.wait_file(&dir.join(format!("{name}.done.json")), LONG);
    poll_until("the answered request to be removed", LONG, || {
        (!dir.join(format!("{name}.json")).exists()).then_some(())
    });
    assert_eq!(done["v"], 1, "{name}: {done}");
    done
}

/// A job named by handle, id or a job view.
fn names_job(v: &Value, id: &str) -> bool {
    let h = job_handle(id);
    v.as_str() == Some(id)
        || v.as_str() == Some(h.as_str())
        || v["id"].as_str() == Some(id)
        || v["handle"].as_str() == Some(h.as_str())
}

/// `run/locks/<id>.lock` replaced by `content` without touching anyone's flock on it.
fn overwrite_lock_record(lb: &LiveBox, id: &str, content: &str) {
    let f = fs::OpenOptions::new()
        .write(true)
        .open(lb.lock_path(id))
        .expect("the lock file exists");
    f.set_len(0).unwrap();
    f.write_all_at(content.as_bytes(), 0).unwrap();
}

/// Lines of the `Commands:` section of `--help`, first word only.
fn help_commands(text: &str) -> Vec<String> {
    let mut on = false;
    let mut out = Vec::new();
    for l in text.lines() {
        if l.trim_end() == "Commands:" {
            on = true;
            continue;
        }
        if !on {
            continue;
        }
        if l.trim().is_empty() || !l.starts_with("  ") {
            break;
        }
        let rest = &l[2..];
        if rest.starts_with(' ') {
            continue; // a wrapped description line
        }
        out.push(rest.split_whitespace().next().unwrap().to_owned());
    }
    out
}

// ---------------------------------------------------------------------------------------------
// AC-351 — the visible surface; AC-361/AC-363 parse-time checks; §A8 target rules.
// ---------------------------------------------------------------------------------------------

#[test]
// W5 Step 8 made watch visible (spec change to earlier waves); migrated at the W3b→W5 merge
fn ac351_help_lists_exactly_the_visible_commands_and_watch_is_visible_but_usage_gated() {
    let sb = Sandbox::new();
    let out = sb.glen(["--help"]).run();
    out.assert_exit(0);
    let mut got = help_commands(&out.out());
    let mut want: Vec<String> = VISIBLE_W3B.iter().map(|s| (*s).to_owned()).collect();
    want.push("watch".to_owned());
    got.sort();
    want.sort();
    assert_eq!(got, want, "AC-351 (R1, R2)\n{}", out.out());

    let out = sb.glen(["watch", "--json"]).run();
    out.error_envelope("usage");
    out.assert_exit(2).assert_stderr_empty();
    for cmd in ["abort", "stop", "models", "model", "thinking", "compact"] {
        sb.glen([cmd, "--help"]).run().assert_exit(0);
    }
    assert!(!sb.glen_home.exists(), "--help created GLEN_HOME");
}

#[test]
fn ac351_every_completion_script_names_the_five_controls_and_the_seven_levels() {
    let sb = Sandbox::new();
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let out = sb.glen(["completions", shell]).run();
        out.assert_exit(0);
        let script = out.out();
        for cmd in ["abort", "models", "model", "thinking", "compact"] {
            assert!(
                script.contains(cmd),
                "AC-351: the {shell} script lacks `{cmd}`"
            );
        }
        for level in LEVELS {
            assert!(
                script.contains(level),
                "AC-351: the {shell} script lacks the level `{level}`"
            );
        }
    }
}

#[test]
fn ac361_ac363_levels_and_timeouts_are_checked_at_parse_time() {
    let lb = LiveBox::new();
    for level in ["banana", "auto", "inherit", ""] {
        let out = ctl(&lb, &["thinking", A.handle, level]).run();
        failed(&out, "usage");
    }
    let commands: [&[&str]; 5] = [
        &["abort", A.handle],
        &["models"],
        &["model", A.handle, "fake/fake-1"],
        &["thinking", A.handle, "high"],
        &["compact", A.handle],
    ];
    for cmd in commands {
        for t in ["0", "0s", "banana", "25h"] {
            let mut args = cmd.to_vec();
            args.extend(["--timeout", t]);
            let out = ctl(&lb, &args).run();
            failed(&out, "usage");
        }
    }
    lb.assert_no_omp();
    assert!(!lb.glen_home().exists(), "a usage error created GLEN_HOME");
}

#[test]
fn a8_control_targets_resolve_like_every_session_argument() {
    let lb = LiveBox::new();
    let commands: [&[&str]; 5] = [
        &["abort"],
        &["models"],
        &["model"],
        &["thinking"],
        &["compact"],
    ];
    let tail = |cmd: &str| -> Vec<&'static str> {
        match cmd {
            "model" => vec!["fake/fake-1"],
            "thinking" => vec!["high"],
            _ => vec![],
        }
    };
    for cmd in commands {
        let verb = cmd[0];
        let run = |target: &str| {
            let mut args = vec![verb, target];
            args.extend(tail(verb));
            ctl(&lb, &args).run()
        };
        // `@new` is only valid for `send`; a job handle is not a session.
        failed(&run("@new"), "usage");
        failed(&run("j1234567"), "usage");
        failed(&run("fffffff"), "not_found");
        // A and F share the hex tail `c0de` (tests/fixtures/README.md).
        let err = failed(&run("c0de"), "ambiguous");
        candidates(&err, 2);
    }
    lb.assert_no_omp();
    assert!(!lb.glen_home().exists());
}

// ---------------------------------------------------------------------------------------------
// AC-357 — `glen models` without a session.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac357_models_without_a_session_lists_through_a_throwaway_no_session_omp() {
    let lb = LiveBox::new();
    let before = snapshot(&lb.bx.agent);
    let d = data_of(&ctl(&lb, &["models"]).run(), 0);
    has_keys(&d, &MODELS_KEYS, "models data");
    assert!(d["session"].is_null(), "{d}");
    assert_eq!(d["route"], "rpc", "{d}");
    assert!(d["current"].is_null(), "{d}");
    assert!(d["current_source"].is_null(), "{d}");
    assert!(d["thinking_level"].is_null(), "{d}");
    assert_eq!(
        d["default_model"],
        mref("fake", "fake-1"),
        "the in-memory session's model: {d}"
    );
    assert_rows(&d, None);

    let starts = lb.starts();
    assert_eq!(starts.len(), 1, "{}", lb.diag());
    let a = argv_of(&starts[0]);
    assert!(a.iter().any(|x| x == "--no-session"), "argv {a:?}");
    assert!(!a.iter().any(|x| x == "--resume"), "argv {a:?}");
    assert!(a.windows(2).any(|w| w == ["--mode", "rpc"]), "argv {a:?}");
    assert!(a.iter().any(|x| x == "--no-ui"), "argv {a:?}");
    assert_eq!(starts[0]["env"]["GLEN_MANAGED"], "1", "{}", starts[0]);

    assert!(!lb.glen_home().exists(), "AC-357: no GLEN_HOME, no lock");
    assert_eq!(
        snapshot(&lb.bx.agent),
        before,
        "the omp agent dir is untouched"
    );
    assert!(!lb.bx.home.join(".omp").exists(), "nothing under ~/.omp");
    lb.assert_quiet();
}

#[test]
fn ac357_provider_filters_case_insensitively_before_total() {
    let lb = LiveBox::new();
    let cases: [(&str, &[(&str, &str)]); 3] = [
        ("OTHER", &[("other", "fake-2")]),
        ("Fake", &[("fake", "fake-1"), ("fake", "fake-2")]),
        ("nobody", &[]),
    ];
    for (p, want) in cases {
        let d = data_of(&ctl(&lb, &["models", "--provider", p]).run(), 0);
        let want: Vec<(String, String)> = want
            .iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect();
        assert_eq!(pairs(&d), want, "--provider {p}: {d}");
        assert_eq!(d["total"], want.len(), "total after the filter: {d}");
    }
    lb.assert_quiet();
}

#[test]
fn ac357_an_explicit_profile_reaches_omp_and_replaces_the_profile_env() {
    let lb = LiveBox::new();
    let out = ctl(&lb, &["--profile", "work", "models"])
        .env("OMP_PROFILE", "other")
        .env("PI_PROFILE", "other")
        .run();
    data_of(&out, 0);
    let starts = lb.starts();
    assert_eq!(starts.len(), 1);
    let a = argv_of(&starts[0]);
    assert!(
        a.windows(2).any(|w| w == ["--profile", "work"]),
        "argv {a:?}"
    );
    assert!(a.iter().any(|x| x == "--no-session"), "argv {a:?}");
    assert!(starts[0]["env"]["OMP_PROFILE"].is_null(), "{}", starts[0]);
    assert!(starts[0]["env"]["PI_PROFILE"].is_null(), "{}", starts[0]);
    lb.assert_quiet();
}

#[test]
fn ac357_models_without_omp_is_omp_unavailable() {
    let lb = LiveBox::new();
    let out = ctl(&lb, &["models"]).env("GLEN_OMP_BIN", NO_OMP).run();
    failed(&out, "omp_unavailable");
    assert!(!lb.glen_home().exists());
}

// ---------------------------------------------------------------------------------------------
// AC-358 — `glen models <s>`.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac358_a_live_session_lists_through_its_endpoint() {
    let lb = LiveBox::new();
    let file = lb.bx.path(&A);
    let ext = start_ext_for_a(&lb, "ok", &[]);
    let d = data_of(
        &ctl(&lb, &["models", A.handle])
            .env("GLEN_OMP_BIN", NO_OMP)
            .run(),
        0,
    );
    has_keys(&d, &MODELS_KEYS, "models data");
    assert_session(&d, A.id, Some(&file));
    assert_eq!(d["route"], "live", "{d}");
    assert_eq!(d["current_source"], "session", "{d}");
    assert_eq!(d["current"], mref("fake", "fake-1"), "{d}");
    assert_eq!(d["thinking_level"], "medium", "{d}");
    assert_rows(&d, Some(("fake", "fake-1")));
    assert!(
        control_ops_in(&ext).iter().any(|o| o == "list_models"),
        "{:?}",
        ext.ops_in()
    );

    let d = data_of(
        &ctl(&lb, &["models", A.handle, "--provider", "other"])
            .env("GLEN_OMP_BIN", NO_OMP)
            .run(),
        0,
    );
    assert_eq!(
        pairs(&d),
        vec![("other".to_owned(), "fake-2".to_owned())],
        "{d}"
    );
    assert_eq!(d["total"], 1, "{d}");
    assert_eq!(
        d["current"],
        mref("fake", "fake-1"),
        "the filter does not change the selection: {d}"
    );
    lb.assert_no_omp();
}

#[test]
fn ac358_without_an_endpoint_the_selection_comes_from_disk_and_the_session_is_never_resumed() {
    let lb = LiveBox::new();
    let file = lb.bx.path(&A);
    let before = fs::read(&file).unwrap();
    let out = ctl(&lb, &["models", A.handle]).run();
    let env = ok_doc(&out, 0);
    let d = &env["data"];
    has_keys(d, &MODELS_KEYS, "models data");
    assert_session(d, A.id, Some(&file));
    assert_eq!(d["route"], "rpc", "R3: {d}");
    assert_eq!(d["current_source"], "disk", "{d}");
    // Fixture A: thinking_level_change high (configured high), then model_change default.
    assert_eq!(d["current"], mref("anthropic", "claude-sonnet-4-5"), "{d}");
    assert_eq!(d["thinking_level"], "high", "{d}");
    assert_eq!(d["thinking_configured"], "high", "{d}");
    assert_rows(d, None);
    assert!(!warned(&env, "session_unreachable"), "{env}");
    assert!(
        !lb.glen_home().exists(),
        "AC-358: models never creates GLEN_HOME"
    );

    // A held lease is still a listing, with a warning, never exit 6.
    let _lease = hold_lease(&lb.bx.owners(), A.id);
    let env = ok_doc(&ctl(&lb, &["models", A.handle]).run(), 0);
    assert!(warned(&env, "session_unreachable"), "{env}");
    assert_eq!(env["data"]["current_source"], "disk", "{env}");
    assert_eq!(
        env["data"]["current"],
        mref("anthropic", "claude-sonnet-4-5")
    );

    for st in lb.starts() {
        let a = argv_of(&st);
        assert!(
            a.iter().any(|x| x == "--no-session") && !a.iter().any(|x| x == "--resume"),
            "AC-358: models never resumes the session: {a:?}"
        );
    }
    assert_eq!(lb.starts().len(), 2);
    assert_eq!(
        fs::read(&file).unwrap(),
        before,
        "the session file is untouched"
    );
    lb.assert_quiet();
}

#[test]
fn ac358_an_outdated_extension_falls_back_to_disk_with_a_warning() {
    let lb = LiveBox::new();
    let ext = start_ext_for_a(&lb, "ok", &[("FAKE_EXT_OPS", "w3")]);
    let out = ctl(&lb, &["models", A.handle]).run();
    let env = ok_doc(&out, 0);
    assert!(warned(&env, "extension_outdated"), "{env}");
    let d = &env["data"];
    assert_eq!(d["current_source"], "disk", "{d}");
    assert_eq!(d["current"], mref("anthropic", "claude-sonnet-4-5"), "{d}");
    assert_rows(d, None);
    assert!(
        control_ops_in(&ext).is_empty(),
        "AC-353: no op frame for a missing capability: {:?}",
        ext.ops_in()
    );
    lb.assert_quiet();
}

#[test]
fn ac358_a_session_driven_by_a_glen_job_lists_through_that_job() {
    let lb = LiveBox::new();
    let rel = lb.bx.root.join("run-release");
    let j = held_job(&lb, &rel, &[]);
    let d = data_of(&ctl(&lb, &["models", A.handle]).run(), 0);
    assert_eq!(d["route"], "job", "{d}");
    assert_eq!(d["current_source"], "session", "{d}");
    // The job's omp resumed A: an unknown default model leaves the fake on its first model.
    assert_eq!(d["current"], mref("fake", "fake-1"), "{d}");
    assert_eq!(d["thinking_level"], "high", "{d}");
    assert_rows(&d, Some(("fake", "fake-1")));
    assert_eq!(lb.starts().len(), 1, "AC-352: the job route starts no omp");
    assert!(!lb.inbound("get_available_models").is_empty());
    release(&rel);
    assert_eq!(lb.wait_terminal(&j)["status"], "completed");
    lb.assert_quiet();
}

#[test]
fn ac358_the_disk_selection_follows_omps_resume_rule_on_the_active_branch() {
    let lb = LiveBox::new();
    type Case<'a> = (
        &'a str,
        Vec<Value>,
        Option<(&'a str, &'a str)>,
        Option<&'a str>,
        Option<&'a str>,
    );
    let cases: Vec<Case> = vec![
        (
            "a default model_change after an assistant wins",
            vec![
                user("c1000001", None, T0, "q"),
                answered_by("c1000002", Some("c1000001"), "anthropic", "claude-x"),
                model_change("c1000003", Some("c1000002"), "fake/fake-2", Some("default")),
            ],
            Some(("fake", "fake-2")),
            None,
            None,
        ),
        (
            "an assistant after a default model_change (role absent) does not override it",
            vec![
                model_change("c2000001", None, "fake/fake-2", None),
                user("c2000002", Some("c2000001"), T0, "q"),
                answered_by("c2000003", Some("c2000002"), "anthropic", "claude-x"),
            ],
            Some(("fake", "fake-2")),
            None,
            None,
        ),
        (
            "a temporary-role change is ignored",
            vec![
                model_change("c3000001", None, "fake/fake-1", Some("default")),
                model_change(
                    "c3000002",
                    Some("c3000001"),
                    "other/fake-2",
                    Some("temporary"),
                ),
            ],
            Some(("fake", "fake-1")),
            None,
            None,
        ),
        (
            "without a model_change the last assistant names the model",
            vec![
                user("c4000001", None, T0, "q"),
                answered_by("c4000002", Some("c4000001"), "other", "fake-2"),
                user("c4000003", Some("c4000002"), T0, "q"),
                answered_by("c4000004", Some("c4000003"), "vendor", "m-3"),
            ],
            Some(("vendor", "m-3")),
            None,
            None,
        ),
        (
            "a <synthetic> assistant is not a selection",
            vec![
                user("c5000001", None, T0, "q"),
                answered_by("c5000002", Some("c5000001"), "fake", "fake-1"),
                user("c5000003", Some("c5000002"), T0, "q"),
                answered_by("c5000004", Some("c5000003"), "anthropic", "<synthetic>"),
            ],
            Some(("fake", "fake-1")),
            None,
            None,
        ),
        (
            "the last thinking change wins and keeps its configured value",
            vec![
                thinking_change("c6000001", None, "low", "low"),
                thinking_change("c6000002", Some("c6000001"), "high", "auto"),
                model_change("c6000003", Some("c6000002"), "fake/fake-1", Some("default")),
            ],
            Some(("fake", "fake-1")),
            Some("high"),
            Some("auto"),
        ),
        (
            "entries off the active branch are ignored",
            vec![
                user("c7000001", None, T0, "q"),
                model_change("c7000002", Some("c7000001"), "fake/fake-2", Some("default")),
                thinking_change("c7000003", Some("c7000002"), "low", "low"),
                answered_by("c7000004", Some("c7000001"), "fake", "fake-1"),
            ],
            Some(("fake", "fake-1")),
            None,
            None,
        ),
    ];
    for (i, (what, body, model, level, configured)) in cases.into_iter().enumerate() {
        let (id, file) = add_session(&lb, 0x3b50 + i as u64, &lb.bx.alpha, &body);
        let d = data_of(&ctl(&lb, &["models", &id]).run(), 0);
        assert_session(&d, &id, Some(&file));
        assert_eq!(d["current_source"], "disk", "{what}: {d}");
        let want_current = model.map_or(Value::Null, |(p, m)| mref(p, m));
        assert_eq!(d["current"], want_current, "{what}: {d}");
        assert_eq!(
            d["thinking_level"],
            level.map_or(Value::Null, Value::from),
            "{what} (no thinking entry → null): {d}"
        );
        assert_eq!(
            d["thinking_configured"],
            configured.map_or(Value::Null, Value::from),
            "{what}: {d}"
        );
        if let Some(cur) = model.filter(|(p, _)| *p != "anthropic") {
            assert_rows(&d, Some(cur));
        }
    }
    assert!(!lb.glen_home().exists());
    lb.assert_quiet();
}

#[test]
fn ac358_ac360_an_endpoint_only_session_has_no_disk_selection_and_warns_not_yet_persisted() {
    let lb = LiveBox::new();
    let id = uuid(0x3b0e);
    let cwd = s(&lb.bx.alpha);
    let mut ext = lb.start_ext(
        &id,
        "ok",
        &[("FAKE_EXT_OPS", "w3"), ("FAKE_EXT_CWD", cwd.as_str())],
        &[],
    );
    let env = ok_doc(&ctl(&lb, &["models", &id]).run(), 0);
    let d = &env["data"];
    assert_session(d, &id, None::<&Path>);
    assert!(d["current"].is_null(), "{d}");
    assert!(d["thinking_level"].is_null(), "{d}");
    assert!(d["current_source"].is_null(), "{d}");
    assert!(warned(&env, "not_yet_persisted"), "{env}");
    assert!(warned(&env, "extension_outdated"), "{env}");
    assert_rows(d, None);
    ext.stop();

    // With a current extension the change goes live and is flagged as not yet on disk.
    let _ext = lb.start_ext(&id, "ok", &[("FAKE_EXT_CWD", cwd.as_str())], &[]);
    let env = ok_doc(
        &ctl(&lb, &["model", &id, "fake/fake-2"])
            .env("GLEN_OMP_BIN", NO_OMP)
            .run(),
        0,
    );
    assert_eq!(env["data"]["route"], "live", "{env}");
    assert_session(&env["data"], &id, None::<&Path>);
    assert!(warned(&env, "not_yet_persisted"), "AC-360: {env}");
    let env = ok_doc(
        &ctl(&lb, &["thinking", &id, "off"])
            .env("GLEN_OMP_BIN", NO_OMP)
            .run(),
        0,
    );
    assert!(warned(&env, "not_yet_persisted"), "AC-360: {env}");
    lb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-359 — model specs.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac359_model_specs_resolve_exactly_first_then_case_insensitively() {
    let lb = LiveBox::new();
    let ext = start_ext_for_a(&lb, "ok", &[]);
    let model = |spec: &str| {
        ctl(&lb, &["model", A.handle, spec])
            .env("GLEN_OMP_BIN", NO_OMP)
            .run()
    };

    type Candidates<'a> = [(&'a str, &'a str, &'a str); 2];
    let ambiguous: [(&str, Candidates); 2] = [
        (
            "fake-2",
            [
                ("fake", "fake-2", "fake fake-2"),
                ("other", "fake-2", "other fake-2"),
            ],
        ),
        // No exact full match, so the case-insensitive pair and the bare id both count.
        (
            "VENDOR/M-3",
            [
                ("router", "vendor/m-3", "router vendor/m-3"),
                ("vendor", "m-3", "vendor m-3"),
            ],
        ),
    ];
    for (spec, want) in ambiguous {
        let err = failed(&model(spec), "ambiguous");
        let mut got: Vec<Value> = arr(&err["details"]["candidates"])
            .iter()
            .map(|c| json!({"provider": c["provider"], "id": c["id"], "name": c["name"]}))
            .collect();
        got.sort_by_key(|c| c.to_string());
        let mut want: Vec<Value> = want
            .iter()
            .map(|(p, i, n)| json!({"provider": p, "id": i, "name": n}))
            .collect();
        want.sort_by_key(|c| c.to_string());
        assert_eq!(got, want, "{spec}: {err}");
    }
    let err = failed(&model("nope"), "not_found");
    assert!(
        plain(&err["hint"]).contains("glen models"),
        "AC-359 hint: {err}"
    );
    assert!(
        ext_frames_of(&ext, "set_model").is_empty(),
        "AC-359: no set_model for an ambiguous or unknown spec"
    );

    let unique = [
        ("FAKE/FAKE-1", ("fake", "fake-1")),
        ("Other/Fake-2", ("other", "fake-2")),
        ("m-3", ("vendor", "m-3")),
        ("vendor/m-3", ("vendor", "m-3")),
        ("router/vendor/m-3", ("router", "vendor/m-3")),
    ];
    for (spec, (p, i)) in unique {
        let d = data_of(&model(spec), 0);
        assert_eq!(d["model"]["provider"], p, "{spec}: {d}");
        assert_eq!(d["model"]["id"], i, "{spec}: {d}");
        let sets = ext_frames_of(&ext, "set_model");
        let last = sets.last().expect("a set_model frame");
        assert_eq!(
            last["args"]["provider"], p,
            "{spec}: exact values sent: {last}"
        );
        assert_eq!(
            last["args"]["model_id"], i,
            "{spec}: exact values sent: {last}"
        );
    }
    lb.assert_no_omp();
}

#[test]
fn ac359_an_ambiguous_spec_on_the_rpc_route_never_reaches_omp() {
    let lb = LiveBox::new();
    let file = lb.bx.path(&A);
    let before = fs::read(&file).unwrap();
    let err = failed(&ctl(&lb, &["model", A.handle, "fake-2"]).run(), "ambiguous");
    assert_eq!(arr(&err["details"]["candidates"]).len(), 2, "{err}");
    assert_no_frames(&lb, &["set_model"]);
    assert_eq!(fs::read(&file).unwrap(), before);
    lb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-354, AC-355, AC-360, AC-361, AC-362 — the short-lived RPC client.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac354_ac355_ac360_model_on_an_unowned_session_runs_a_short_lived_omp_that_writes_the_change() {
    let lb = LiveBox::new();
    let (id, file) = add_x(&lb, 0x3b01, &lb.bx.beta);
    let before = fs::read(&file).unwrap();
    let out = ctl(&lb, &["model", &id, "fake/fake-2"]).run();
    let env = ok_doc(&out, 0);
    let d = &env["data"];
    has_keys(d, &MODEL_KEYS, "model data");
    assert_session(d, &id, Some(&file));
    assert_eq!(d["route"], "rpc", "{d}");
    assert_eq!(
        d["model"],
        json!({"provider": "fake", "id": "fake-2", "name": "fake fake-2"}),
        "{d}"
    );
    assert_eq!(d["previous"], mref("fake", "fake-1"), "{d}");
    assert_eq!(d["changed"], true, "{d}");
    assert_eq!(d["thinking_level"], "off", "{d}");
    assert_eq!(d["thinking_levels"], json!(["off"]), "{d}");
    assert_eq!(d["busy"], false, "{d}");
    assert_eq!(d["scope"], "session", "{d}");
    assert!(d["forked"].is_null(), "{d}");
    assert!(
        warned(&env, "thinking_changed"),
        "medium → off on a non-reasoning model: {env}"
    );

    // AC-354: one omp, resumed, managed, in the session's own cwd.
    let starts = lb.starts();
    assert_eq!(starts.len(), 1, "{}", lb.diag());
    assert_omp_argv(&starts[0]["argv"], Some(file.as_path()), None);
    assert_eq!(starts[0]["env"]["GLEN_MANAGED"], "1", "{}", starts[0]);
    assert!(
        same_file(
            Path::new(starts[0]["cwd"].as_str().unwrap_or("")),
            &lb.bx.beta
        ),
        "the W2 worker cwd (the header's existing cwd): {}",
        starts[0]
    );
    let got = lb.recs();
    let first_state = index_of(&got, |r| is_frame(r, "in", "get_state"));
    let first_op = index_of(&got, |r| {
        r["event"] == "in" && SESSION_OPS.contains(&r["frame"]["type"].as_str().unwrap_or(""))
    });
    assert!(
        first_state.is_some() && first_state < first_op,
        "AC-354: get_state is read before any op\n{}",
        lb.diag()
    );

    // AC-355: omp's own entries, appended; nothing else changed.
    let new = appended(&file, &before);
    assert_eq!(new.len(), 2, "{new:?}");
    assert_eq!(new[0]["type"], "model_change", "{new:?}");
    assert_eq!(new[0]["model"], "fake/fake-2", "{new:?}");
    assert_eq!(new[0]["role"], "default", "{new:?}");
    assert_eq!(new[1]["type"], "thinking_level_change", "{new:?}");
    assert_eq!(new[1]["thinkingLevel"], "off", "{new:?}");
    assert_no_settings_frames(&lb);
    assert!(lb.glen_lock_free(&id), "AC-354: the lock is released");
    assert_omp_gone(&starts[0]);

    // AC-358: the next listing reads the change back from disk.
    let d = data_of(&ctl(&lb, &["models", &id]).run(), 0);
    assert_eq!(d["current"], mref("fake", "fake-2"), "{d}");
    assert_eq!(d["thinking_level"], "off", "{d}");
    assert_eq!(d["current_source"], "disk", "{d}");
    assert_rows(&d, Some(("fake", "fake-2")));

    // The same model again: unchanged, no thinking warning.
    let env = ok_doc(&ctl(&lb, &["model", &id, "fake/fake-2"]).run(), 0);
    assert_eq!(env["data"]["changed"], false, "{env}");
    assert_eq!(env["data"]["previous"], mref("fake", "fake-2"), "{env}");
    assert!(!warned(&env, "thinking_changed"), "{env}");
    lb.assert_quiet();
}

#[test]
fn ac355_ac361_thinking_on_an_unowned_session_is_checked_first_and_written_only_on_change() {
    let lb = LiveBox::new();
    let (id, file) = add_x(&lb, 0x3b02, &lb.bx.alpha);
    let before = fs::read(&file).unwrap();

    let err = failed(&ctl(&lb, &["thinking", &id, "xhigh"]).run(), "usage");
    assert_eq!(err["details"]["reason"], "invalid_level", "{err}");
    assert_eq!(
        err["details"]["available"],
        strs_json(&FAKE1_LEVELS),
        "{err}"
    );
    assert_eq!(err["details"]["model"], mref("fake", "fake-1"), "{err}");
    assert_no_frames(&lb, &["set_thinking_level"]);
    assert_eq!(fs::read(&file).unwrap(), before, "nothing written");

    let env = ok_doc(&ctl(&lb, &["thinking", &id, "high"]).run(), 0);
    let d = &env["data"];
    has_keys(d, &THINKING_KEYS, "thinking data");
    assert_session(d, &id, Some(&file));
    assert_eq!(d["route"], "rpc", "{d}");
    assert_eq!(d["thinking_level"], "high", "{d}");
    assert_eq!(d["requested"], "high", "{d}");
    assert_eq!(d["previous"], "medium", "{d}");
    assert_eq!(d["changed"], true, "{d}");
    assert_eq!(d["model"], mref("fake", "fake-1"), "{d}");
    assert_eq!(d["available"], strs_json(&FAKE1_LEVELS), "{d}");
    assert_eq!(d["busy"], false, "{d}");
    assert_eq!(d["scope"], "session", "{d}");
    assert!(d["forked"].is_null(), "{d}");
    assert!(!warned(&env, "thinking_clamped"), "{env}");
    let new = appended(&file, &before);
    assert_eq!(new.len(), 1, "{new:?}");
    assert_eq!(new[0]["type"], "thinking_level_change", "{new:?}");
    assert_eq!(new[0]["thinkingLevel"], "high", "{new:?}");

    let now = fs::read(&file).unwrap();
    let d = data_of(&ctl(&lb, &["thinking", &id, "high"]).run(), 0);
    assert_eq!(d["changed"], false, "{d}");
    assert_eq!(d["previous"], "high", "{d}");
    assert_eq!(
        fs::read(&file).unwrap(),
        now,
        "AC-355: no entry when the level is unchanged"
    );

    let d = data_of(&ctl(&lb, &["models", &id]).run(), 0);
    assert_eq!(d["thinking_level"], "high", "{d}");
    assert_no_settings_frames(&lb);
    lb.assert_quiet();
}

#[test]
fn ac360_live_and_rpc_model_documents_agree_apart_from_route_and_session() {
    let lb = LiveBox::new();
    let (id, _file) = add_x(&lb, 0x3b03, &lb.bx.alpha);
    let _ext = start_ext_for_a(&lb, "ok", &[]);
    let live = ok_doc(&ctl(&lb, &["model", A.handle, "fake/fake-2"]).run(), 0);
    let rpc = ok_doc(&ctl(&lb, &["model", &id, "fake/fake-2"]).run(), 0);
    assert_eq!(live["data"]["route"], "live", "{live}");
    assert_eq!(rpc["data"]["route"], "rpc", "{rpc}");
    let strip = |env: &Value| {
        let mut d = env["data"].clone();
        let o = d.as_object_mut().unwrap();
        o.remove("route");
        o.remove("session");
        d
    };
    assert_eq!(
        strip(&live),
        strip(&rpc),
        "AC-360: the same document on both routes"
    );
    assert_eq!(
        warning_codes(&live),
        warning_codes(&rpc),
        "the same warnings on both routes"
    );
    assert_eq!(lb.starts().len(), 1, "only the rpc call started omp");
    lb.assert_quiet();
}

#[test]
fn ac360_missing_credentials_are_control_failed_no_auth_on_every_route() {
    let lb = LiveBox::new();
    let (id, file) = add_x(&lb, 0x3b04, &lb.bx.alpha);
    let before = fs::read(&file).unwrap();
    let err = control_failed(
        &ctl(&lb, &["model", &id, "nokey/locked-1"]).run(),
        "no_auth",
    );
    assert!(
        plain(&err["hint"]).to_lowercase().contains("log"),
        "AC-360: a hint to log in to the provider: {err}"
    );
    assert_eq!(fs::read(&file).unwrap(), before, "nothing written");

    let mut ext = start_ext_for_a(&lb, "ok", &[]);
    let out = ctl(&lb, &["model", A.handle, "nokey/locked-1"])
        .env("GLEN_OMP_BIN", NO_OMP)
        .run();
    control_failed(&out, "no_auth");
    ext.stop();
    let _ext = start_ext_for_a(&lb, "ok", &[("FAKE_EXT_NO_AUTH", "other")]);
    let out = ctl(&lb, &["model", A.handle, "other/fake-2"])
        .env("GLEN_OMP_BIN", NO_OMP)
        .run();
    control_failed(&out, "no_auth");
    lb.assert_quiet();
}

#[test]
fn ac360_a_busy_session_takes_the_model_for_its_next_request() {
    let lb = LiveBox::new();
    let _ext = start_ext_for_a(&lb, "busy", &[]);
    let d = data_of(
        &ctl(&lb, &["model", A.handle, "fake/fake-2"])
            .env("GLEN_OMP_BIN", NO_OMP)
            .run(),
        0,
    );
    assert_eq!(d["busy"], true, "{d}");
    assert_eq!(d["route"], "live", "{d}");
    let out = lb
        .glen(["model", A.handle, "other/fake-2", "--human"])
        .env("GLEN_OMP_BIN", NO_OMP)
        .run();
    out.assert_exit(0);
    let text = strip_ansi(&out.out());
    for needle in ["other/fake-2", "thinking", "live", "next model request"] {
        assert!(
            text.contains(needle),
            "AC-360 human text lacks {needle:?}:\n{text}"
        );
    }
}

#[test]
fn ac361_thinking_on_the_live_route() {
    let lb = LiveBox::new();
    let file = lb.bx.path(&A);
    let ext = start_ext_for_a(&lb, "ok", &[]);
    let run = |level: &str| {
        ctl(&lb, &["thinking", A.handle, level])
            .env("GLEN_OMP_BIN", NO_OMP)
            .run()
    };
    let err = failed(&run("xhigh"), "usage");
    assert_eq!(err["details"]["reason"], "invalid_level", "{err}");
    assert_eq!(
        err["details"]["available"],
        strs_json(&FAKE1_LEVELS),
        "{err}"
    );
    assert_eq!(err["details"]["model"], mref("fake", "fake-1"), "{err}");

    let d = data_of(&run("high"), 0);
    has_keys(&d, &THINKING_KEYS, "thinking data");
    assert_session(&d, A.id, Some(&file));
    assert_eq!(d["route"], "live", "{d}");
    assert_eq!(d["thinking_level"], "high", "{d}");
    assert_eq!(d["requested"], "high", "{d}");
    assert_eq!(d["previous"], "medium", "{d}");
    assert_eq!(d["changed"], true, "{d}");
    assert_eq!(d["model"], mref("fake", "fake-1"), "{d}");
    assert_eq!(d["available"], strs_json(&FAKE1_LEVELS), "{d}");
    assert_eq!(d["scope"], "session", "{d}");
    assert!(d["forked"].is_null(), "{d}");
    let sets = ext_frames_of(&ext, "set_thinking_level");
    assert_eq!(sets.last().unwrap()["args"]["level"], "high");

    let out = lb
        .glen(["thinking", A.handle, "low", "--human"])
        .env("GLEN_OMP_BIN", NO_OMP)
        .run();
    out.assert_exit(0);
    let text = strip_ansi(&out.out());
    for needle in ["low", "fake/fake-1", "live"] {
        assert!(
            text.contains(needle),
            "human text lacks {needle:?}:\n{text}"
        );
    }
    lb.assert_no_omp();
}

#[test]
fn ac362_compact_on_an_idle_unowned_session_passes_the_instructions_and_reports_tokens() {
    let lb = LiveBox::new();
    let (id, file) = add_x(&lb, 0x3b05, &lb.bx.alpha);
    let before = fs::read(&file).unwrap();
    let out = ctl(
        &lb,
        &["compact", &id, "--instructions", "keep the decisions"],
    )
    .run();
    let d = data_of(&out, 0);
    has_keys(&d, &COMPACT_KEYS, "compact data");
    assert_session(&d, &id, Some(&file));
    assert_eq!(d["route"], "rpc", "{d}");
    assert_eq!(d["compacted"], true, "{d}");
    assert_eq!(
        d["tokens_before"], 1234,
        "omp's CompactionResult.tokensBefore: {d}"
    );
    assert_eq!(
        d["tokens_after"], 123,
        "omp's context usage after the compaction: {d}"
    );
    assert!(d["forked"].is_null(), "{d}");
    let compacts = lb.inbound("compact");
    assert_eq!(compacts.len(), 1, "{}", lb.diag());
    assert_eq!(
        compacts[0]["customInstructions"], "keep the decisions",
        "the instructions are passed through: {}",
        compacts[0]
    );
    let new = appended(&file, &before);
    assert_eq!(new.len(), 1, "{new:?}");
    assert_eq!(new[0]["type"], "compaction", "omp's own entry: {new:?}");
    lb.assert_quiet();
}

// Linux MAX_ARG_STRLEN (131072 bytes) prevents this argv boundary test; see
// commands::ctl::tests::instructions_are_bounded_at_524288_bytes for direct coverage.
#[cfg(not(target_os = "linux"))]
#[test]
fn ac362_r4_instructions_are_bounded_at_524288_bytes() {
    let lb = LiveBox::new();
    let (id, file) = add_x(&lb, 0x3b06, &lb.bx.alpha);
    let before = fs::read(&file).unwrap();
    let over = "x".repeat(524_289);
    let out = ctl(&lb, &["compact", &id, "--instructions", &over]).run();
    failed(&out, "usage");
    assert_no_frames(&lb, &["compact"]);
    assert_eq!(fs::read(&file).unwrap(), before);

    let at = "y".repeat(524_288);
    let d = data_of(&ctl(&lb, &["compact", &id, "--instructions", &at]).run(), 0);
    assert_eq!(d["compacted"], true, "{d}");
    lb.assert_quiet();
}

#[test]
fn ac362_omp_failing_the_compaction_is_omp_error_with_its_message() {
    let lb = LiveBox::new();
    let (id, _file) = add_x(&lb, 0x3b07, &lb.bx.alpha);
    let out = ctl(&lb, &["compact", &id])
        .env("FAKE_OMP_SCENARIO", "compact_error")
        .run();
    let err = control_failed(&out, "omp_error");
    assert!(
        format!("{err}").contains("Nothing to compact"),
        "AC-362: omp's message: {err}"
    );
    lb.assert_quiet();
}

#[test]
fn ac362_compact_refuses_a_busy_live_session_and_compacts_an_idle_one() {
    let lb = LiveBox::new();
    let mut ext = start_ext_for_a(&lb, "busy", &[]);
    let out = ctl(&lb, &["compact", A.handle])
        .env("GLEN_OMP_BIN", NO_OMP)
        .run();
    let err = control_failed(&out, "busy");
    assert!(
        plain(&err["hint"]).contains("glen abort"),
        "AC-362: a hint to wait or `glen abort` first: {err}"
    );
    ext.stop();

    let _ext = start_ext_for_a(&lb, "ok", &[]);
    let d = data_of(
        &ctl(&lb, &["compact", A.handle])
            .env("GLEN_OMP_BIN", NO_OMP)
            .run(),
        0,
    );
    assert_eq!(d["route"], "live", "{d}");
    assert_eq!(d["compacted"], true, "{d}");
    assert_eq!(d["tokens_before"], 1234, "{d}");
    assert_eq!(d["tokens_after"], 123, "{d}");
    lb.assert_no_omp();
}

#[test]
fn ac362_compact_on_the_job_route_is_busy_and_compacts_nothing() {
    let lb = LiveBox::new();
    let rel = lb.bx.root.join("run-release");
    let j = held_job(&lb, &rel, &[]);
    let err = control_failed(&ctl(&lb, &["compact", A.handle]).run(), "busy");
    assert!(plain(&err["hint"]).contains("glen abort"), "{err}");
    assert_no_frames(&lb, &["compact"]);
    release(&rel);
    lb.wait_terminal(&j);
    lb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-354 — safety of the short-lived client.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac354_a_session_that_resumes_under_another_id_is_refused_before_any_op() {
    let lb = LiveBox::new();
    let file = lb.bx.path(&A);
    let before = fs::read(&file).unwrap();
    let moved = format!("{}beef", &A.id[..A.id.len() - 4]);
    let cases: [&[&str]; 3] = [
        &["model", A.handle, "fake/fake-2"],
        &["thinking", A.handle, "low"],
        &["compact", A.handle],
    ];
    for args in cases {
        let out = ctl(&lb, args).env("FAKE_OMP_SCENARIO", "moved").run();
        let err = control_failed(&out, "session_moved");
        assert_eq!(err["details"]["from"], A.id, "{args:?}: {err}");
        assert_eq!(err["details"]["to"], moved.as_str(), "{args:?}: {err}");
    }
    assert_no_frames(&lb, &SESSION_OPS);
    assert_eq!(fs::read(&file).unwrap(), before);
    lb.assert_quiet();
}

#[test]
fn ac354_a_fork_after_the_op_exits_11_with_the_new_handle() {
    let lb = LiveBox::new();
    let to = moved_id(A.id);
    let cases: [&[&str]; 3] = [
        &["thinking", A.handle, "low"],
        &["compact", A.handle],
        &["model", A.handle, "fake/fake-2"],
    ];
    for args in cases {
        let out = ctl(&lb, args)
            .env("FAKE_OMP_SCENARIO", "fork_control")
            .run();
        let env = ok_doc(&out, 11);
        assert_eq!(
            env["data"]["forked"],
            json!({"from": A.id, "to": to, "to_handle": tail7(&to)}),
            "{args:?}: {env}"
        );
    }
    lb.assert_quiet();
}

#[test]
fn ac354_ac365_the_short_lived_client_holds_the_glen_lock_with_a_control_record() {
    let lb = LiveBox::new();
    let (id, _file) = add_x(&lb, 0x3b08, &lb.bx.alpha);
    let child = spawn_ctl(
        &lb,
        &["compact", &id],
        &[("FAKE_OMP_COMPACT_DELAY_MS", "3000")],
    );
    let pid = child.id();
    let rec = poll_until("a control holder record", LONG, || {
        read_json(&lb.lock_path(&id)).filter(|r| r["kind"] == "control")
    });
    assert_eq!(rec["v"], 1, "{rec}");
    assert!(rec["job_id"].is_null(), "{rec}");
    assert_eq!(rec["pid"], pid, "the holder is this glen process: {rec}");
    assert!(
        rec["start"].is_null() || rec["start"].as_str().is_some_and(|s| !s.is_empty()),
        "{rec}"
    );
    assert!(
        !lb.glen_lock_free(&id),
        "AC-354: the glen lock is held during the run"
    );
    let out = finish(child, "compact", Duration::from_secs(30));
    assert_eq!(data_of(&out, 0)["compacted"], true);
    assert!(lb.glen_lock_free(&id), "released after omp closed");
    assert_omp_gone(&lb.starts()[0]);
    lb.assert_quiet();
}

#[test]
fn ac354_r5_an_endpoint_that_appears_while_waiting_for_the_lock_is_honoured() {
    let lb = LiveBox::new();
    let lock = lb.hold_glen_lock(A.id);
    let child = spawn_ctl(&lb, &["model", A.handle, "fake/fake-2"], &[]);
    std::thread::sleep(Duration::from_millis(1500));
    let ext = start_ext_for_a(&lb, "ok", &[]);
    drop(lock);
    let out = finish(child, "model", Duration::from_secs(30));
    let d = data_of(&out, 0);
    assert_eq!(d["route"], "live", "{d}");
    lb.assert_no_omp();
    assert_eq!(
        ext_frames_of(&ext, "set_model").len(),
        1,
        "{:?}",
        ext.ops_in()
    );
}

#[test]
fn ac354_continuations_are_followed_to_the_session_that_continues() {
    let lb = LiveBox::new();
    let d = data_of(&ctl(&lb, &["model", B.handle, "fake/fake-2"]).run(), 0);
    assert_session(&d, C.id, Some(&lb.bx.path(&C)));
    assert_eq!(d["route"], "rpc", "{d}");
    let starts = lb.starts();
    assert_eq!(starts.len(), 1);
    assert_omp_argv(&starts[0]["argv"], Some(lb.bx.path(&C).as_path()), None);
    lb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-352, AC-353 — routing order and refusals.
// ---------------------------------------------------------------------------------------------

/// The four control commands against fixture A.
/// Ordered so each op stays valid after the ones before it (`fake-2` has only `off`).
const CONTROLS_ON_A: [&[&str]; 4] = [
    &["abort", "e27c0de"],
    &["thinking", "e27c0de", "high"],
    &["compact", "e27c0de"],
    &["model", "e27c0de", "fake/fake-2"],
];

#[test]
fn ac352_a_held_lease_without_an_endpoint_refuses_every_control_before_anything_starts() {
    let lb = LiveBox::new();
    let _lease = hold_lease(&lb.bx.owners(), A.id);
    for args in CONTROLS_ON_A {
        let err = failed(&ctl(&lb, args).run(), "no_route");
        assert!(
            plain(&err["hint"]).contains("glen install-extension"),
            "AC-352/AC-315: the install hint `send` gives: {args:?}: {err}"
        );
    }
    let ext_file = lb.bx.agent.join("extensions/glen/index.ts");
    fs::create_dir_all(ext_file.parent().unwrap()).unwrap();
    fs::write(&ext_file, "// installed\n").unwrap();
    for args in CONTROLS_ON_A {
        let err = failed(&ctl(&lb, args).run(), "no_route");
        assert!(
            plain(&err["hint"]).contains("restart"),
            "AC-352/AC-315: the restart hint once the extension is installed: {args:?}: {err}"
        );
    }
    lb.assert_no_omp();
    assert!(
        lb.job_ids().is_empty(),
        "no control file or job was written"
    );
}

#[test]
fn ac352_an_endpoint_while_the_glen_lock_is_held_refuses() {
    let lb = LiveBox::new();
    let ext = start_ext_for_a(&lb, "ok", &[]);
    let _lock = lb.hold_glen_lock(A.id);
    for args in CONTROLS_ON_A {
        failed(&ctl(&lb, args).run(), "no_route");
    }
    assert!(
        control_ops_in(&ext).is_empty(),
        "no op reached the endpoint: {:?}",
        ext.ops_in()
    );
    lb.assert_no_omp();
}

#[test]
fn ac352_an_unhealthy_endpoint_refuses_quickly_and_models_still_lists() {
    let lb = LiveBox::new();
    for scenario in ["hang", "v2"] {
        let mut ext = start_ext_for_a(&lb, scenario, &[]);
        let n = lb.starts().len();
        for args in CONTROLS_ON_A {
            let out = ctl(&lb, args).run();
            failed(&out, "no_route");
            assert!(
                out.elapsed < Duration::from_secs(3),
                "{scenario} {args:?}: hello is bounded at 2 s, took {:?}",
                out.elapsed
            );
        }
        assert_eq!(lb.starts().len(), n, "{scenario}: a refusal starts no omp");
        // AC-358: models never exits 6.
        let env = ok_doc(&ctl(&lb, &["models", A.handle]).run(), 0);
        assert_eq!(env["data"]["current_source"], "disk", "{scenario}: {env}");
        ext.stop();
    }
    lb.assert_quiet();
}

#[test]
fn ac352_each_route_reports_itself_and_only_rpc_starts_omp() {
    let lb = LiveBox::new();
    // live
    let mut ext = start_ext_for_a(&lb, "ok", &[]);
    for args in CONTROLS_ON_A {
        let d = data_of(&ctl(&lb, args).env("GLEN_OMP_BIN", NO_OMP).run(), 0);
        assert_eq!(d["route"], "live", "{args:?}: {d}");
    }
    lb.assert_no_omp();
    ext.stop();
    // none: an idle, unowned abort
    let d = data_of(&ctl(&lb, &["abort", A.handle]).run(), 0);
    assert_eq!(d["route"], "none", "{d}");
    lb.assert_no_omp();
    // rpc
    for args in &CONTROLS_ON_A[1..] {
        let d = data_of(&ctl(&lb, args).run(), 0);
        assert_eq!(d["route"], "rpc", "{args:?}: {d}");
    }
    assert_eq!(lb.starts().len(), 3, "one short-lived omp per rpc control");
    for st in lb.starts() {
        assert_omp_argv(&st["argv"], Some(lb.bx.path(&A).as_path()), None);
        assert_eq!(st["env"]["GLEN_MANAGED"], "1");
    }
    // job
    let rel = lb.bx.root.join("run-release");
    let j = held_job(&lb, &rel, &[]);
    let n = lb.starts().len();
    for args in [
        &["model", "e27c0de", "fake/fake-2"][..],
        &["thinking", "e27c0de", "off"][..],
    ] {
        let d = data_of(&ctl(&lb, args).run(), 0);
        assert_eq!(d["route"], "job", "{args:?}: {d}");
    }
    assert_eq!(lb.starts().len(), n, "AC-352: the job route starts no omp");
    let sets: Vec<Value> = lb
        .recs()
        .into_iter()
        .filter(|r| is_frame(r, "in", "set_model"))
        .collect();
    assert_eq!(
        sets.len(),
        2,
        "one set_model on the rpc route, one on the job's omp: {}",
        lb.diag()
    );
    release(&rel);
    lb.wait_terminal(&j);
    lb.assert_quiet();
}

#[test]
fn ac353_an_extension_without_the_op_gets_no_op_frame_and_an_install_hint() {
    let lb = LiveBox::new();
    let ext = start_ext_for_a(&lb, "ok", &[("FAKE_EXT_OPS", "w3")]);
    for args in CONTROLS_ON_A {
        let err = failed(
            &ctl(&lb, args).env("GLEN_OMP_BIN", NO_OMP).run(),
            "no_route",
        );
        assert_eq!(
            err["details"]["reason"], "extension_outdated",
            "{args:?}: {err}"
        );
        assert_eq!(
            err["details"]["ext_version"], PKG_VERSION,
            "{args:?}: {err}"
        );
        let hint = plain(&err["hint"]);
        assert!(
            hint.contains("glen install-extension") && hint.contains("restart"),
            "{args:?}: {err}"
        );
    }
    assert!(
        control_ops_in(&ext).is_empty(),
        "AC-353: no op frame: {:?}",
        ext.ops_in()
    );
    assert!(ext.ops_in().iter().any(|o| o == "hello"));
    lb.assert_no_omp();
    assert!(lb.job_ids().is_empty());
}

#[test]
fn ac353_an_unsupported_op_answer_maps_to_extension_outdated() {
    let lb = LiveBox::new();
    let _peer = Peer::start(&lb, A.id, |_, req, s| {
        if !answer_unary(A.id, req, s) {
            write_frame(
                s,
                &json!({"v":1,"id":req["id"],"ok":false,"error":{"code":"unsupported_op","message":"not here",
                        "details":{"op":req["op"],"ops":["hello","state","send"],"reserved":false}}}),
            );
        }
    });
    for args in CONTROLS_ON_A {
        let err = failed(
            &ctl(&lb, args).env("GLEN_OMP_BIN", NO_OMP).run(),
            "no_route",
        );
        assert_eq!(
            err["details"]["reason"], "extension_outdated",
            "{args:?}: {err}"
        );
        assert!(
            plain(&err["hint"]).contains("glen install-extension"),
            "{args:?}: {err}"
        );
    }
    lb.assert_no_omp();
}

#[test]
fn ac352_ac358_an_unknown_lease_refuses_controls_and_models_warns() {
    let lb = LiveBox::new();
    // §A3: an id that is not a plain token probes as `unknown` (glen does not port wyhash).
    let id = "legacy.session.3b0f";
    let cwd = s(&lb.bx.alpha);
    lb.bx.add(
        ALPHA,
        T0,
        id,
        Some(cwd.as_str()),
        &[
            model_change("u0000001", None, "fake/fake-1", Some("default")),
            user("u0000002", Some("u0000001"), T0, "hello"),
        ],
    );
    let controls: [&[&str]; 4] = [
        &["abort", id],
        &["thinking", id, "high"],
        &["compact", id],
        &["model", id, "fake/fake-2"],
    ];
    for args in controls {
        failed(&ctl(&lb, args).run(), "no_route");
    }
    lb.assert_no_omp();
    let env = ok_doc(&ctl(&lb, &["models", id]).run(), 0);
    assert!(warned(&env, "session_unreachable"), "AC-358: {env}");
    assert_eq!(env["data"]["current_source"], "disk", "{env}");
    assert_eq!(env["data"]["current"], mref("fake", "fake-1"), "{env}");
    lb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-356 — abort.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac356_an_idle_unowned_session_has_nothing_to_abort_and_nothing_is_created() {
    let lb = LiveBox::new();
    let before = snapshot(&lb.bx.sessions);
    for verb in ["abort", "stop"] {
        let env = ok_doc(&ctl(&lb, &[verb, A.handle]).run(), 0);
        let d = &env["data"];
        has_keys(d, &ABORT_KEYS, "abort data");
        assert_session(d, A.id, Some(&lb.bx.path(&A)));
        assert_eq!(d["route"], "none", "{verb}: {d}");
        assert_eq!(d["was"], "idle", "{verb}: {d}");
        assert_eq!(d["aborted"], false, "{verb}: {d}");
        assert_eq!(d["queued_jobs"], json!([]), "{verb}: {d}");
        assert!(!warned(&env, "jobs_queued") && !warned(&env, "abort_pending"));
    }
    let out = lb.glen(["abort", A.handle, "--human"]).run();
    out.assert_exit(0);
    assert!(
        strip_ansi(&out.out()).contains("nothing to abort"),
        "AC-356 human text:\n{}",
        out.out()
    );
    lb.assert_no_omp();
    assert!(!lb.glen_home().exists(), "AC-356: no GLEN_HOME");
    assert_eq!(snapshot(&lb.bx.sessions), before);
}

#[test]
fn ac356_aborting_a_busy_live_session_stops_the_run_and_its_glen_job_ends_aborted() {
    let lb = LiveBox::new();
    let ext = start_ext_for_a(&lb, "never_settles", &[]);
    let j = job_id(&data_of(
        &lb.send(A.handle, "hold", &["--no-wait"]).run(),
        10,
    ));
    lb.wait_status(&j, "accepted", LONG, |s| s["status"] == "accepted");
    let out = ctl(&lb, &["abort", A.handle])
        .env("GLEN_OMP_BIN", NO_OMP)
        .run();
    assert!(
        out.elapsed < Duration::from_secs(7),
        "AC-356: within 7 s, took {:?}",
        out.elapsed
    );
    let env = ok_doc(&out, 0);
    let d = &env["data"];
    has_keys(d, &ABORT_KEYS, "abort data");
    assert_eq!(d["route"], "live", "{d}");
    assert_eq!(d["was"], "busy", "{d}");
    assert_eq!(d["aborted"], true, "{d}");
    assert_eq!(d["idle_after"], true, "{d}");
    assert!(!warned(&env, "abort_pending"), "{env}");
    assert_eq!(ext_frames_of(&ext, "abort").len(), 1, "{:?}", ext.ops_in());

    let err = failed(
        &lb.glen(["wait", &job_handle(&j), "--timeout", "20s", "--json"])
            .run(),
        "job_failed",
    );
    assert_job_failed(&err, "aborted", "aborted");
    lb.assert_no_omp();
    lb.assert_quiet();
}

#[test]
fn ac356_an_abort_that_does_not_settle_warns_abort_pending() {
    let lb = LiveBox::new();
    let _ext = start_ext_for_a(&lb, "busy", &[("FAKE_EXT_ABORT_STUCK", "1")]);
    let out = ctl(&lb, &["abort", A.handle])
        .env("GLEN_OMP_BIN", NO_OMP)
        .run();
    assert!(
        out.elapsed < Duration::from_secs(7),
        "took {:?}",
        out.elapsed
    );
    let env = ok_doc(&out, 0);
    assert_eq!(env["data"]["aborted"], true, "{env}");
    assert_eq!(env["data"]["idle_after"], false, "{env}");
    assert!(warned(&env, "abort_pending"), "{env}");
}

#[test]
fn ac356_ac364_aborting_a_session_driven_by_a_glen_job_cancels_it_and_reports_the_queue() {
    let lb = LiveBox::new();
    let j1 = job_id(&data_of(
        &lb.glen(["send", A.handle, "first", "--no-wait", "--json"])
            .env("FAKE_OMP_SCENARIO", "hang")
            .env("FAKE_OMP_SERIAL", "1")
            .run(),
        10,
    ));
    lb.wait_status(&j1, "accepted", LONG, |s| s["status"] == "accepted");
    let j2 = job_id(&data_of(
        &lb.glen(["send", A.handle, "second", "--no-wait", "--json"])
            .run(),
        10,
    ));
    lb.wait_status(&j2, "queued behind j1", LONG, |s| {
        s["status"] == "queued" && s["worker_pid"].is_u64()
    });

    let out = ctl(&lb, &["abort", A.handle]).run();
    assert!(
        out.elapsed < Duration::from_secs(3),
        "AC-356: the job acknowledgement within 3 s, took {:?}",
        out.elapsed
    );
    let env = ok_doc(&out, 0);
    let d = &env["data"];
    has_keys(d, &ABORT_KEYS, "abort data");
    has_keys(d, &["job", "job_status"], "abort data on the job route");
    assert_eq!(d["route"], "job", "{d}");
    assert!(
        names_job(&d["job"], &j1),
        "data.job names the running job: {d}"
    );
    assert_eq!(d["job_status"], "aborted", "{d}");
    assert_eq!(d["aborted"], true, "aborted = (job_status == aborted): {d}");
    let queued = arr(&d["queued_jobs"]);
    assert_eq!(queued.len(), 1, "{d}");
    assert!(names_job(&queued[0], &j2), "AC-356: the queued job: {d}");
    assert!(warned(&env, "jobs_queued"), "{env}");
    assert_eq!(lb.status(&j1)["status"], "aborted");

    assert_eq!(
        lb.wait_terminal(&j2)["status"],
        "completed",
        "the queued job starts next"
    );
    assert_eq!(lb.starts().len(), 2, "abort started no omp of its own");
    lb.assert_quiet();
}

#[test]
fn ac356_ac365_a_glen_lock_held_without_a_job_makes_abort_busy() {
    let lb = LiveBox::new();
    let lock = lb.hold_glen_lock(A.id);
    let out = ctl(&lb, &["abort", A.handle]).run();
    control_failed(&out, "busy");
    // A control holder record, as a short-lived client writes it.
    lock.set_len(0).unwrap();
    let rec = json!({"v":1,"kind":"control","job_id":null,"pid":std::process::id(),"start":null});
    lock.write_all_at(rec.to_string().as_bytes(), 0).unwrap();
    control_failed(&ctl(&lb, &["abort", A.handle]).run(), "busy");
    lb.assert_no_omp();
    drop(lock);
}

// ---------------------------------------------------------------------------------------------
// AC-363 — deadlines and SIGINT.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac363_an_rpc_deadline_stops_the_omp_group_without_an_abort_frame() {
    let lb = LiveBox::new();
    let (id, file) = add_x(&lb, 0x3b09, &lb.bx.alpha);
    let before = fs::read(&file).unwrap();
    let out = ctl(&lb, &["compact", &id, "--timeout", "1s"])
        .env("FAKE_OMP_COMPACT_DELAY_MS", "8000")
        .run();
    control_failed(&out, "timeout");
    assert!(
        out.elapsed < Duration::from_secs(6),
        "AC-363: exit within 5 s of the 1 s deadline, took {:?}",
        out.elapsed
    );
    assert_eq!(lb.inbound("compact").len(), 1, "{}", lb.diag());
    assert_no_frames(&lb, &["abort"]);
    assert_omp_gone(&lb.starts()[0]);
    assert_eq!(fs::read(&file).unwrap(), before, "nothing compacted");
    assert!(lb.glen_lock_free(&id));
}

#[test]
fn ac363_sigint_on_the_rpc_route_exits_130_with_an_interrupted_document() {
    let lb = LiveBox::new();
    let (id, file) = add_x(&lb, 0x3b0a, &lb.bx.alpha);
    let before = fs::read(&file).unwrap();
    let child = spawn_ctl(
        &lb,
        &["compact", &id],
        &[("FAKE_OMP_COMPACT_DELAY_MS", "8000")],
    );
    let pid = child.id();
    lb.wait_log("the compact frame", |r| {
        r.iter().any(|x| is_frame(x, "in", "compact"))
    });
    let t = Instant::now();
    signal(pid as i64, Signal::INT);
    let out = finish(child, "compact (SIGINT)", Duration::from_secs(20));
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "AC-363: exit within 5 s of SIGINT, took {:?}",
        t.elapsed()
    );
    let env = ok_doc(&out, 130);
    let d = &env["data"];
    assert_eq!(d["status"], "interrupted", "{d}");
    assert_eq!(d["interrupted"], true, "{d}");
    assert_eq!(d["route"], "rpc", "{d}");
    assert_session(d, &id, Some(&file));
    assert!(d["hint"].as_str().is_some_and(|h| !h.is_empty()), "{d}");
    assert_no_frames(&lb, &["abort"]);
    assert_omp_gone(&lb.starts()[0]);
    assert_eq!(fs::read(&file).unwrap(), before, "nothing compacted");
}

#[test]
fn ac363_an_omp_that_never_becomes_ready_is_bounded_by_the_deadline() {
    let lb = LiveBox::new();
    let never = lb.bx.root.join("never-ready");
    let out = ctl(&lb, &["model", A.handle, "fake/fake-2", "--timeout", "2s"])
        .env("FAKE_OMP_READY_RELEASE", &never)
        .run();
    control_failed(&out, "timeout");
    assert!(
        out.elapsed < Duration::from_secs(7),
        "AC-363: within 5 s of the deadline, took {:?}",
        out.elapsed
    );
    assert_eq!(lb.starts().len(), 1);
    assert_omp_gone(&lb.starts()[0]);
    assert_no_frames(&lb, &["set_model"]);
    assert!(lb.glen_lock_free(A.id));
}

#[test]
fn ac363_a_live_deadline_closes_the_socket_with_a_hint() {
    let lb = LiveBox::new();
    let _ext = start_ext_for_a(&lb, "ok", &[("FAKE_EXT_COMPACT_DELAY_MS", "5000")]);
    let out = ctl(&lb, &["compact", A.handle, "--timeout", "1s"])
        .env("GLEN_OMP_BIN", NO_OMP)
        .run();
    let err = control_failed(&out, "timeout");
    assert!(
        err["hint"].as_str().is_some_and(|h| !h.is_empty()),
        "AC-363: a hint that the op may still finish: {err}"
    );
    assert!(
        out.elapsed < Duration::from_secs(4),
        "the live deadline bounds the command, took {:?}",
        out.elapsed
    );
}

#[test]
fn ac363_ac364_a_job_route_deadline_leaves_an_unknown_outcome_and_the_request_never_runs() {
    let lb = LiveBox::new();
    let ready = lb.bx.root.join("ready");
    let j = job_id(&data_of(
        &lb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .env("FAKE_OMP_SERIAL", "1")
            .env("FAKE_OMP_READY_RELEASE", &ready)
            .run(),
        10,
    ));
    lb.wait_log("the job's omp start", |r| !starts(r).is_empty());

    let sent = now_ms();
    let out = ctl(&lb, &["model", A.handle, "fake/fake-2", "--timeout", "2s"]).run();
    let err = control_failed(&out, "timeout");
    assert_eq!(err["details"]["outcome"], "unknown", "AC-363: {err}");
    assert!(
        format!("{err}").contains(&job_handle(&j)),
        "AC-363: the hint names the job: {err}"
    );
    assert!(
        out.elapsed < Duration::from_secs(4),
        "took {:?}",
        out.elapsed
    );
    let dir = control_dir(&lb, &j);
    let reqs = request_files(&dir);
    assert_eq!(reqs.len(), 1, "the request stays in place for the worker");
    // AC-363/AC-364: `{"v":1,"op","args","deadline_ms"}` carrying the command deadline.
    let req = read_json(&reqs[0]).expect("the request is JSON");
    assert_eq!(req["v"], 1, "{req}");
    assert_eq!(
        req["op"], "list_models",
        "the model list comes first (AC-359): {req}"
    );
    assert!(req["args"].is_object(), "{req}");
    let deadline = req["deadline_ms"]
        .as_u64()
        .unwrap_or_else(|| panic!("deadline_ms is Unix ms: {req}"));
    assert!(
        deadline > sent && deadline <= sent + 2_000 + 500,
        "deadline_ms is the 2 s command deadline: {deadline} vs sent {sent}"
    );

    release(&ready);
    let done = poll_until("the worker's answer to the expired request", LONG, || {
        done_files(&dir).into_iter().find_map(|p| read_json(&p))
    });
    assert_eq!(done["ok"], false, "{done}");
    assert_eq!(
        done["error"]["code"], "timeout",
        "AC-364: expired requests are answered timeout: {done}"
    );
    assert_eq!(lb.wait_terminal(&j)["status"], "completed");
    assert_no_frames(&lb, &["get_available_models", "set_model"]);
    lb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-364 — controls executed by a running glen RPC job.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac364_a_running_job_executes_control_files_on_its_own_omp() {
    let lb = LiveBox::new();
    let rel = lb.bx.root.join("run-release");
    let j = held_job(&lb, &rel, &[]);
    let far = now_ms() + 60_000;

    let done = ask(
        &lb,
        &j,
        "a",
        &json!({"v":1,"op":"list_models","args":{},"deadline_ms":far}),
    );
    assert_eq!(done["ok"], true, "{done}");
    let d = &done["data"];
    assert_eq!(d["current"], mref("fake", "fake-1"), "{done}");
    assert_eq!(d["thinking_level"], "high", "{done}");
    assert_eq!(d["session_id"], A.id, "{done}");
    let rows = arr(&d["models"]);
    assert_eq!(rows.len(), 6, "{done}");
    for r in rows {
        has_keys(r, &OP_ROW_KEYS, "a ModelRow");
    }

    let done = ask(
        &lb,
        &j,
        "b",
        &json!({"v":1,"op":"set_thinking_level","args":{"level":"xhigh"},"deadline_ms":far}),
    );
    assert_eq!(done["ok"], false, "{done}");
    assert_eq!(done["error"]["code"], "invalid_level", "{done}");
    assert_eq!(
        done["error"]["details"]["available"],
        strs_json(&FAKE1_LEVELS),
        "{done}"
    );
    assert_no_frames(&lb, &["set_thinking_level"]);

    let done = ask(
        &lb,
        &j,
        "c",
        &json!({"v":1,"op":"compact","args":{},"deadline_ms":far}),
    );
    assert_eq!(done["error"]["code"], "busy", "{done}");
    assert_no_frames(&lb, &["compact"]);

    let past = now_ms() - 1_000;
    let done = ask(
        &lb,
        &j,
        "d",
        &json!({"v":1,"op":"set_model","args":{"provider":"fake","model_id":"fake-2"},"deadline_ms":past}),
    );
    assert_eq!(done["error"]["code"], "timeout", "{done}");
    assert_no_frames(&lb, &["set_model"]);

    let done = ask(
        &lb,
        &j,
        "e",
        &json!({"v":1,"op":"set_model","args":{"provider":"fake","model_id":"fake-2"},"deadline_ms":far}),
    );
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(
        done["data"]["model"],
        json!({"provider":"fake","id":"fake-2","name":"fake fake-2"}),
        "{done}"
    );
    assert_eq!(done["data"]["previous"], mref("fake", "fake-1"), "{done}");
    assert_eq!(
        done["data"]["busy"], true,
        "the job's prompt is running: {done}"
    );
    assert_eq!(lb.inbound("set_model").len(), 1);

    let done = ask(
        &lb,
        &j,
        "f",
        &json!({"v":1,"op":"set_thinking_level","args":{"level":"banana"},"deadline_ms":far}),
    );
    assert_eq!(done["error"]["code"], "bad_request", "{done}");

    let t = Instant::now();
    write_control(&lb, &j, "z.json", r#"{"v":1,"op":"abort"}"#);
    let st = lb.wait_terminal(&j);
    assert_eq!(st["status"], "aborted", "{st}");
    assert!(
        t.elapsed() <= Duration::from_millis(1500),
        "AC-364: W2's 1 s abort bound with no control in flight, took {:?}",
        t.elapsed()
    );
    let done = lb.wait_file(&control_dir(&lb, &j).join("z.done.json"), LONG);
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(done["data"]["job_status"], "aborted", "{done}");
    assert_eq!(lb.starts().len(), 1);
    lb.assert_quiet();
}

#[test]
fn ac364_ops_written_while_omp_starts_wait_for_the_client_and_are_answered_before_the_job_ends() {
    let lb = LiveBox::new();
    let ready = lb.bx.root.join("ready");
    let rel = lb.bx.root.join("run-release");
    let j = job_id(&data_of(
        &lb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .env("FAKE_OMP_SERIAL", "1")
            .env("FAKE_OMP_READY_RELEASE", &ready)
            .env("FAKE_OMP_RUN_RELEASE", &rel)
            .run(),
        10,
    ));
    lb.wait_log("the job's omp start", |r| !starts(r).is_empty());
    let far = now_ms() + 60_000;
    write_control(
        &lb,
        &j,
        "a.json",
        &json!({"v":1,"op":"list_models","args":{},"deadline_ms":far}).to_string(),
    );
    std::thread::sleep(Duration::from_millis(600));
    let dir = control_dir(&lb, &j);
    assert!(
        !dir.join("a.done.json").exists(),
        "AC-364: no answer before the client is ready"
    );
    assert!(dir.join("a.json").exists(), "the request stays in place");

    release(&ready);
    let done = lb.wait_file(&dir.join("a.done.json"), LONG);
    assert_eq!(done["ok"], true, "{done}");
    assert!(
        !is_terminal(&lb.status(&j)),
        "answered before the job ended"
    );
    release(&rel);
    assert_eq!(lb.wait_terminal(&j)["status"], "completed");
    lb.assert_quiet();
}

#[test]
fn ac364_after_an_abort_no_further_control_op_is_dispatched() {
    let lb = LiveBox::new();
    let j = job_id(&data_of(
        &lb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .env("FAKE_OMP_SCENARIO", "hang")
            .env("FAKE_OMP_SERIAL", "1")
            .env("FAKE_OMP_STATE_DELAY_MS", "3000")
            .run(),
        10,
    ));
    lb.wait_status(&j, "accepted", LONG, |s| s["status"] == "accepted");
    let far = now_ms() + 60_000;
    write_control(
        &lb,
        &j,
        "a.json",
        &json!({"v":1,"op":"list_models","args":{},"deadline_ms":far}).to_string(),
    );
    lb.wait_log("list_models in flight on omp", |r| {
        index_of(r, |x| is_frame(x, "in", "get_available_models"))
            .is_some_and(|i| r[i..].iter().any(|x| is_frame(x, "in", "get_state")))
    });
    let t = Instant::now();
    write_control(&lb, &j, "b.json", r#"{"v":1,"op":"abort"}"#);
    write_control(
        &lb,
        &j,
        "c.json",
        &json!({"v":1,"op":"set_model","args":{"provider":"fake","model_id":"fake-2"},"deadline_ms":far})
            .to_string(),
    );
    let st = lb.wait_terminal(&j);
    assert_eq!(st["status"], "aborted", "{st}");
    assert!(
        t.elapsed() <= Duration::from_secs(13),
        "AC-364: the 13 s escalation bound, took {:?}",
        t.elapsed()
    );
    let dir = control_dir(&lb, &j);
    let c = lb.wait_file(&dir.join("c.done.json"), LONG);
    assert_eq!(c["ok"], false, "{c}");
    assert_eq!(
        c["error"]["code"], "job_ended",
        "AC-364: unanswered at the end: {c}"
    );
    assert_no_frames(&lb, &["set_model"]);
    let a = lb.wait_file(&dir.join("a.done.json"), LONG);
    assert!(a["ok"].is_boolean(), "the in-flight op is answered: {a}");
    let b = lb.wait_file(&dir.join("b.done.json"), LONG);
    assert_eq!(b["data"]["job_status"], "aborted", "{b}");
    lb.assert_quiet();
}

#[test]
fn ac364_a_control_deferred_in_startup_is_answered_job_ended_and_the_cli_reroutes_once() {
    let lb = LiveBox::new();
    let ready = lb.bx.root.join("ready");
    let j = job_id(&data_of(
        &lb.glen(["send", A.handle, "hi", "--no-wait", "--json"])
            .env("FAKE_OMP_SERIAL", "1")
            .env("FAKE_OMP_READY_RELEASE", &ready)
            .run(),
        10,
    ));
    lb.wait_log("the job's omp start", |r| !starts(r).is_empty());
    let child = spawn_ctl(&lb, &["model", A.handle, "fake/fake-2"], &[]);
    let dir = control_dir(&lb, &j);
    poll_until("the CLI's control request", LONG, || {
        (!request_files(&dir).is_empty()).then_some(())
    });
    write_control(&lb, &j, "0-abort.json", r#"{"v":1,"op":"abort"}"#);
    let out = finish(child, "model (job_ended)", Duration::from_secs(40));
    let d = data_of(&out, 0);
    assert_eq!(
        d["route"], "rpc",
        "AC-364: re-routed once after job_ended: {d}"
    );
    assert_eq!(d["model"]["id"], "fake-2", "{d}");
    assert!(is_terminal(&lb.status(&j)), "{}", lb.diag());
    let st = lb.starts();
    assert_eq!(st.len(), 2, "{}", lb.diag());
    assert_omp_argv(&st[1]["argv"], Some(lb.bx.path(&A).as_path()), None);
    lb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-365 — lock holder records.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac365_a_running_job_records_itself_in_the_lock_and_a_finished_jobs_record_is_ignored() {
    let lb = LiveBox::new();
    let rel = lb.bx.root.join("run-release");
    let j = held_job(&lb, &rel, &[]);
    let st = lb.status(&j);
    let rec = read_json(&lb.lock_path(A.id)).expect("AC-365: a holder record in the lock file");
    assert_eq!(rec["v"], 1, "{rec}");
    assert_eq!(rec["kind"], "job", "{rec}");
    assert_eq!(rec["job_id"], j.as_str(), "{rec}");
    assert_eq!(rec["pid"], st["worker_pid"], "{rec} vs {st}");
    assert!(
        rec["start"].is_null() || rec["start"].as_str().is_some_and(|s| !s.is_empty()),
        "{rec}"
    );
    release(&rel);
    lb.wait_terminal(&j);
    lb.assert_quiet();

    let d = data_of(&ctl(&lb, &["model", A.handle, "fake/fake-2"]).run(), 0);
    assert_eq!(
        d["route"], "rpc",
        "a finished job's record is not trusted: {d}"
    );
    lb.assert_quiet();
}

#[test]
fn ac365_without_a_trusted_record_the_single_active_job_of_the_session_is_used() {
    let lb = LiveBox::new();
    let rel = lb.bx.root.join("run-release");
    let j = held_job(&lb, &rel, &[]);
    let dead = json!({"v":1,"kind":"job","job_id":"01a20000-0000-7000-8000-00000000dead",
                      "pid":dead_pid(),"start":null})
    .to_string();
    for content in ["", dead.as_str()] {
        overwrite_lock_record(&lb, A.id, content);
        let d = data_of(&ctl(&lb, &["thinking", A.handle, "high"]).run(), 0);
        assert_eq!(
            d["route"], "job",
            "AC-365 fallback with record {content:?}: {d}"
        );
    }
    assert!(!lb.glen_lock_free(A.id), "the worker still holds its lock");
    release(&rel);
    lb.wait_terminal(&j);
    lb.assert_quiet();
}

#[test]
fn ac365_a_lock_held_by_nobody_known_makes_controls_wait_for_their_deadline() {
    let lb = LiveBox::new();
    let _lock = lb.hold_glen_lock(A.id);
    let out = ctl(&lb, &["model", A.handle, "fake/fake-2", "--timeout", "2s"]).run();
    control_failed(&out, "timeout");
    assert!(
        out.elapsed >= Duration::from_millis(1500) && out.elapsed < Duration::from_secs(5),
        "waited for the lock until the deadline: {:?}",
        out.elapsed
    );
    lb.assert_no_omp();
    assert!(lb.job_ids().is_empty());
}

// ---------------------------------------------------------------------------------------------
// AC-366 (CLI half) — a post-op identity change on the live route.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac366_a_post_op_identity_change_on_the_live_route_exits_11() {
    let lb = LiveBox::new();
    let _ext = start_ext_for_a(&lb, "ok", &[("FAKE_EXT_POST_ID", "1")]);
    let to = moved_id(A.id);
    let cases: [&[&str]; 3] = [
        &["thinking", A.handle, "low"],
        &["compact", A.handle],
        &["model", A.handle, "fake/fake-2"],
    ];
    for args in cases {
        let env = ok_doc(&ctl(&lb, args).env("GLEN_OMP_BIN", NO_OMP).run(), 11);
        assert_eq!(
            env["data"]["forked"],
            json!({"from": A.id, "to": to, "to_handle": tail7(&to)}),
            "{args:?}: {env}"
        );
    }
    lb.assert_no_omp();
}

// ---------------------------------------------------------------------------------------------
// AC-367, AC-368 — output and writes.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac367_the_human_models_table_fits_columns_and_keeps_every_model_name() {
    let lb = LiveBox::new();
    let out = lb
        .glen(["models", "--human"])
        .env("COLUMNS", "60")
        .env("FAKE_OMP_SERIAL", "1")
        .run();
    out.assert_exit(0);
    let text = out.out();
    for l in text.lines() {
        assert!(
            display_width(l) <= 60,
            "AC-367: a line wider than COLUMNS=60: {l:?}"
        );
    }
    for r in expected_rows() {
        let pair = format!(
            "{}/{}",
            r["provider"].as_str().unwrap(),
            r["id"].as_str().unwrap()
        );
        assert!(
            strip_ansi(&text).contains(&pair),
            "MODEL keeps `{pair}` (NAME truncates first):\n{text}"
        );
    }
    lb.assert_quiet();
}

#[test]
fn ac368_on_the_rpc_route_only_omp_writes_the_session_and_glen_writes_only_its_home() {
    let lb = LiveBox::new();
    let (id, file) = add_x(&lb, 0x3b0b, &lb.bx.alpha);
    let before_bytes = fs::read(&file).unwrap();
    let rel_file = s(file.strip_prefix(&lb.bx.root).unwrap());
    let outside = |lb: &LiveBox| -> Vec<(String, u64, SystemTime)> {
        snapshot(&lb.bx.root)
            .into_iter()
            .filter(|(p, _, _)| {
                !p.starts_with("logs") && !p.starts_with("extlogs") && *p != rel_file
            })
            .collect()
    };
    let before = outside(&lb);
    let runs: [&[&str]; 5] = [
        &["models", &id],
        &["model", &id, "other/fake-2"],
        &["thinking", &id, "xhigh"],
        &["compact", &id],
        &["abort", &id],
    ];
    for args in runs {
        data_of(&ctl(&lb, args).run(), 0);
    }
    lb.assert_quiet();
    assert_eq!(
        outside(&lb),
        before,
        "AC-368: nothing outside GLEN_HOME changed except the session file"
    );
    let kinds: Vec<String> = appended(&file, &before_bytes)
        .iter()
        .map(|e| e["type"].as_str().unwrap_or("?").to_owned())
        .collect();
    assert_eq!(
        kinds,
        ["model_change", "thinking_level_change", "compaction"],
        "only omp's own entries were appended"
    );
    assert!(
        lb.glen_home().is_dir(),
        "glen's own state lives in GLEN_HOME"
    );
}

// ---------------------------------------------------------------------------------------------
// AC-369 — a same-version, older extension build.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac369_a_same_version_older_build_is_outdated_upgraded_and_warned() {
    let sb = Sandbox::new();
    let agent = sb.dir("agent");
    let file = agent.join("extensions/glen/index.ts");
    let install = |args: &[&str]| -> Value {
        let mut a = vec!["install-extension"];
        a.extend_from_slice(args);
        a.push("--json");
        data_of(&sb.glen(a).env("PI_CODING_AGENT_DIR", &agent).run(), 0)
    };
    assert_eq!(install(&[])["action"], "installed");
    let current = fs::read(&file).unwrap();
    let nl = current.iter().position(|b| *b == b'\n').unwrap();
    let mut body = current[nl + 1..].to_vec();
    body.extend_from_slice(b"// an older build of the same version\n");
    let mut older = format!(
        "// glen-extension version={PKG_VERSION} protocol=1 digest={:016x}\n",
        xxh64(&body, 0)
    )
    .into_bytes();
    older.extend_from_slice(&body);
    fs::write(&file, &older).unwrap();

    let doctor = data_of(
        &sb.glen(["doctor", "--json"])
            .fake_omp()
            .env("PI_CODING_AGENT_DIR", &agent)
            .run(),
        0,
    );
    assert_eq!(check(&doctor, "extension")["status"], "warn", "{doctor}");
    assert_eq!(doctor["extension"]["state"], "outdated", "{doctor}");
    assert_eq!(doctor["extension"]["current"], false, "{doctor}");

    let d = install(&["--dry-run"]);
    assert_eq!(d["action"], "upgraded", "{d}");
    assert_eq!(fs::read(&file).unwrap(), older, "--dry-run writes nothing");
    let d = install(&[]);
    assert_eq!(d["action"], "upgraded", "AC-369: {d}");
    assert_eq!(fs::read(&file).unwrap(), current, "the current rendering");
    assert_eq!(install(&[])["action"], "unchanged");
}

// =============================================================================================
// The real bundled extension under Node (AC-353, AC-356, AC-362, AC-366, AC-368 TS half).
//
// The host below implements omp's extension API as the plan cites it (`ctx.abort`, `ctx.isIdle`,
// `ctx.hasPendingMessages`, `ctx.compact`, `ctx.getContextUsage`, `ctx.model`,
// `ctx.models.list()/current()`, `pi.setModel`, `pi.getThinkingLevel`, `pi.setThinkingLevel`,
// `ctx.ui.notify` with `ctx.hasUI`) on top of the W3 surface (`pi.on`, `pi.registerTool`,
// `ctx.sessionManager`, managed timers). Needs Node ≥ 22.6; without it the cases pass with a note
// locally and fail on CI, like `tests/conformance/ext_node.rs`.
// =============================================================================================

const CTL_HOST_JS: &str = r#"
import { pathToFileURL } from "node:url";
import fs from "node:fs";
import readline from "node:readline";

const E = process.env;
const log = (o) => { try { fs.appendFileSync(E.HOST_LOG, JSON.stringify(o) + "\n"); } catch {} };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const MODELS = JSON.parse(fs.readFileSync(E.HOST_MODELS, "utf8"));
const byPair = (pair) => MODELS.find((m) => `${m.provider}/${m.id}` === pair);
const st = {
  idle: E.HOST_BUSY !== "1",
  queue: [],
  id: E.HOST_SESSION_ID,
  file: E.HOST_SESSION_FILE || undefined,
  model: byPair(E.HOST_MODEL || "fake/fake-1"),
  thinking: E.HOST_THINKING || "medium",
  tokens: 1234,
  entries: [],
  compacting: null,
};
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
  on(name, fn) { if (!handlers.has(name)) handlers.set(name, []); handlers.get(name).push(fn); },
  registerTool(def) { tools.push(def); },
  sendUserMessage(text, opts) { log({ k: "sendUserMessage", text, opts: opts ?? null }); },
  async setModel(model) {
    log({ k: "setModel", provider: model?.provider ?? null, id: model?.id ?? null });
    await sleep(20);
    if (E.HOST_SWITCH_ON_SET_MODEL) st.id = E.HOST_SWITCH_ON_SET_MODEL;
    if (E.HOST_SET_MODEL_THROWS) throw new Error(E.HOST_SET_MODEL_THROWS);
    if (E.HOST_NO_AUTH && model?.provider === E.HOST_NO_AUTH) return false;
    st.model = model;
    if (!model.reasoning) st.thinking = "off";
    return true;
  },
  getThinkingLevel() { return st.thinking; },
  setThinkingLevel(level) { log({ k: "setThinkingLevel", level }); st.thinking = level; },
  zod: chain(),
});
const ctx = fallback("ctx_unknown", {
  mode: "tui",
  hasUI: E.HOST_NO_UI !== "1",
  cwd: process.cwd(),
  ui: fallback("ui_unknown", { notify: (msg, level) => log({ k: "notify", msg, level: level ?? null }) }),
  agent: { kind: "main", id: "main", name: "main", depth: 0 },
  sessionManager: fallback("sm_unknown", {
    getSessionId: () => st.id, getSessionFile: () => st.file, getCwd: () => process.cwd(),
    getEntries: () => st.entries,
  }),
  isIdle: () => st.idle,
  hasPendingMessages: () => st.queue.length > 0 || E.HOST_PENDING === "1",
  setTimeout: (fn, ms, ...a) => setTimeout(() => { try { fn(...a); } catch (e) { log({ k: "timer_error", e: String(e) }); } }, ms),
  setInterval: (fn, ms, ...a) => setInterval(() => { try { fn(...a); } catch (e) { log({ k: "timer_error", e: String(e) }); } }, ms),
  clearTimer: (h) => { clearTimeout(h); clearInterval(h); },
  get model() { return st.model; },
  models: { list: () => MODELS.map((m) => ({ ...m })), current: () => st.model },
  getContextUsage: () => ({ tokens: st.tokens, contextWindow: 200000, percent: st.tokens / 2000 }),
  abort() {
    log({ k: "abort" });
    if (st.compacting) { const c = st.compacting; st.compacting = null; c.reject(new Error("compaction cancelled")); }
    if (E.HOST_ABORT_IDLES !== "0") setTimeout(() => { st.idle = true; log({ k: "idle" }); }, 150);
  },
  compact(instructions) {
    log({ k: "compact", instructions: instructions ?? null });
    if (E.HOST_COMPACT_REJECT) return Promise.reject(new Error(E.HOST_COMPACT_REJECT));
    return new Promise((resolve, reject) => {
      const c = { reject };
      st.compacting = c;
      setTimeout(() => {
        if (st.compacting === c) {
          st.compacting = null;
          st.entries.push({ type: "compaction", id: `compact-${st.entries.length}` });
          st.tokens = 123; resolve();
        }
      }, Number(E.HOST_COMPACT_MS ?? "0"));
    });
  },
});
async function emit(name, payload = {}) {
  for (const fn of handlers.get(name) ?? []) {
    try { await fn({ type: name, ...payload }, ctx); } catch (e) { log({ k: "handler_error", name, e: String(e?.stack ?? e) }); }
  }
}
const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const [cmd] = line.split(" ");
  if (cmd === "busy") st.idle = false;
  else if (cmd === "idle") st.idle = true;
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
            "Node >= 22.6 is required on CI for the extension control tests"
        );
        eprintln!("note: Node >= 22.6 not found; skipping the extension control test");
    }
    env.as_ref()
}

/// `glen install-extension` into a scratch agent dir (baked GLEN_HOME = the box's), then the
/// installed file next to the host script under a `"type":"module"` package.
fn install_for_node(lb: &LiveBox) -> PathBuf {
    let agent = lb.bx.root.join("ext-agent");
    let d = data_of(
        &lb.glen(["install-extension", "--json"])
            .env("PI_CODING_AGENT_DIR", &agent)
            .run(),
        0,
    );
    assert_eq!(d["action"], "installed", "{d}");
    let dir = lb.bx.root.join("ext-run");
    fs::create_dir_all(&dir).unwrap();
    fs::copy(agent.join("extensions/glen/index.ts"), dir.join("index.ts")).unwrap();
    fs::write(dir.join("package.json"), r#"{"type":"module"}"#).unwrap();
    fs::write(dir.join("host.mjs"), CTL_HOST_JS).unwrap();
    dir
}

struct CtlHost {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    pid: u32,
    log: PathBuf,
    stderr: PathBuf,
}

impl CtlHost {
    fn start(lb: &LiveBox, nd: &NodeEnv, dir: &Path, env: &[(&str, &str)]) -> CtlHost {
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
            .env(
                "HOST_MODELS",
                repo_root().join("tests/support/fake_models.json"),
            )
            .env("HOST_SESSION_ID", A.id)
            .env("HOST_SESSION_FILE", lb.bx.path(&A))
            .current_dir(&lb.bx.alpha)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(fs::File::create(&stderr).unwrap());
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
        let mut h = CtlHost {
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

    fn cmd(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
        let first = line.split(' ').next().unwrap();
        self.expect_line(&format!("ok {first}"));
    }

    fn of(&self, k: &str) -> Vec<Value> {
        records(&self.log)
            .into_iter()
            .filter(|r| r["k"] == k)
            .collect()
    }

    fn count(&self, k: &str) -> usize {
        self.of(k).len()
    }

    fn socket(&self, lb: &LiveBox) -> PathBuf {
        lb.run_live().join(format!("{}.sock", self.pid))
    }

    fn diag(&self) -> String {
        format!(
            "--- host log ---\n{}\n--- host stderr ---\n{}",
            fs::read_to_string(&self.log).unwrap_or_default(),
            fs::read_to_string(&self.stderr).unwrap_or_default()
        )
    }

    /// No handler or timer error was logged.
    fn assert_clean(&self) {
        let bad: Vec<Value> = records(&self.log)
            .into_iter()
            .filter(|r| ["handler_error", "timer_error"].contains(&r["k"].as_str().unwrap_or("")))
            .collect();
        assert!(bad.is_empty(), "host errors: {bad:?}\n{}", self.diag());
    }
}

impl Drop for CtlHost {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("{}", self.diag());
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One request on a fresh connection; every frame until the server closes (12 s bound). Reading
/// to EOF is what proves "exactly one response frame" (AC-366).
fn exchange(sock: &Path, req: &Value) -> Vec<Value> {
    let mut s = UnixStream::connect(sock).unwrap_or_else(|e| panic!("connect {sock:?}: {e}"));
    s.set_read_timeout(Some(Duration::from_secs(12))).unwrap();
    let _ = s.write_all(format!("{req}\n").as_bytes());
    let mut buf = Vec::new();
    let r = s.read_to_end(&mut buf);
    if let Err(e) = r {
        if buf.ends_with(b"\n") {
            panic!(
                "the extension answered {req} but did not close the connection within 12 s \
                 (plan Step 4: one response frame, then close; defect D1): {e}; received {:?}",
                String::from_utf8_lossy(&buf)
            );
        }
        panic!(
            "no complete answer to {req} within 12 s ({e}); received {:?}",
            String::from_utf8_lossy(&buf)
        );
    }
    String::from_utf8_lossy(&buf)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("frame {l:?}: {e}")))
        .collect()
}

/// AC-366: exactly one response frame, echoing the request id.
fn op(sock: &Path, id: &str, name: &str, args: Value) -> Value {
    let frames = exchange(sock, &json!({"v":1,"id":id,"op":name,"args":args}));
    assert_eq!(
        frames.len(),
        1,
        "AC-366: exactly one response frame for {name}: {frames:?}"
    );
    let f = frames.into_iter().next().unwrap();
    assert_eq!(f["v"], 1, "{f}");
    assert_eq!(f["id"], id, "{f}");
    f
}

fn op_ok(sock: &Path, id: &str, name: &str, args: Value) -> Value {
    let f = op(sock, id, name, args);
    assert_eq!(f["ok"], true, "{name}: {f}");
    f["data"].clone()
}

fn op_err(sock: &Path, id: &str, name: &str, args: Value, code: &str) -> Value {
    let f = op(sock, id, name, args);
    assert_eq!(f["ok"], false, "{name}: {f}");
    assert_eq!(f["error"]["code"], code, "{name}: {f}");
    assert!(f["error"]["message"].is_string(), "{f}");
    f["error"].clone()
}

/// Calls the host saw that change the session (setModel, setThinkingLevel, abort, compact).
fn effects(h: &CtlHost) -> [usize; 4] {
    [
        h.count("setModel"),
        h.count("setThinkingLevel"),
        h.count("abort"),
        h.count("compact"),
    ]
}

#[test]
fn node_ac353_ac366_hello_advertises_exactly_the_eight_ops_and_list_models_reads_the_host() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install_for_node(&lb);
    let h = CtlHost::start(&lb, nd, &dir, &[]);
    let sock = h.socket(&lb);
    let d = op_ok(
        &sock,
        "h1",
        "hello",
        json!({"client":"conformance","client_version":"0","protocols":[1]}),
    );
    assert_eq!(d["ops"], json!(OPS), "AC-353: exactly the eight ops: {d}");

    let d = op_ok(&sock, "m1", "list_models", json!({}));
    assert_eq!(d["current"], mref("fake", "fake-1"), "{d}");
    assert_eq!(d["thinking_level"], "medium", "{d}");
    assert_eq!(d["session_id"], A.id, "{d}");
    let mut got: Vec<Value> = arr(&d["models"])
        .iter()
        .map(|r| {
            has_keys(r, &OP_ROW_KEYS, "a ModelRow");
            let mut o = serde_json::Map::new();
            for k in OP_ROW_KEYS {
                o.insert(k.to_owned(), r[k].clone());
            }
            Value::Object(o)
        })
        .collect();
    got.sort_by_key(|r| (r["provider"].to_string(), r["id"].to_string()));
    let mut want = expected_rows();
    want.sort_by_key(|r| (r["provider"].to_string(), r["id"].to_string()));
    assert_eq!(got, want, "AC-366: rows from ctx.models.list()");
    assert_eq!(effects(&h), [0, 0, 0, 0], "{}", h.diag());
    h.assert_clean();
}

#[test]
fn node_ac366_set_model_takes_exact_pairs_and_maps_auth_and_omp_errors() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install_for_node(&lb);

    let h = CtlHost::start(&lb, nd, &dir, &[("HOST_NO_AUTH", "nokey")]);
    let sock = h.socket(&lb);
    op_err(
        &sock,
        "s1",
        "set_model",
        json!({"provider":"fake","model_id":"FAKE-2"}),
        "model_not_found",
    );
    op_err(
        &sock,
        "s2",
        "set_model",
        json!({"provider":"nope","model_id":"fake-1"}),
        "model_not_found",
    );
    assert_eq!(h.count("setModel"), 0, "no lookup outside exact pairs");
    op_err(
        &sock,
        "s3",
        "set_model",
        json!({"provider":"nokey","model_id":"locked-1"}),
        "no_auth",
    );
    let notes = h.count("notify");
    let d = op_ok(
        &sock,
        "s4",
        "set_model",
        json!({"provider":"fake","model_id":"fake-2"}),
    );
    assert_eq!(
        d["model"],
        json!({"provider":"fake","id":"fake-2","name":"fake fake-2"}),
        "{d}"
    );
    assert_eq!(d["previous"], mref("fake", "fake-1"), "{d}");
    assert_eq!(d["thinking_level"], "off", "{d}");
    assert_eq!(d["thinking_levels"], json!(["off"]), "{d}");
    assert_eq!(d["busy"], false, "{d}");
    assert_eq!(d["session_id"], A.id, "{d}");
    let calls = h.of("setModel");
    let last = calls.last().unwrap();
    assert_eq!(
        (&last["provider"], &last["id"]),
        (&json!("fake"), &json!("fake-2"))
    );
    assert_eq!(
        h.count("notify"),
        notes + 1,
        "AC-366: one notice per applied op"
    );
    h.assert_clean();
    drop(h);

    let h = CtlHost::start(
        &lb,
        nd,
        &dir,
        &[("HOST_SET_MODEL_THROWS", "registry exploded")],
    );
    let err = op_err(
        &h.socket(&lb),
        "s5",
        "set_model",
        json!({"provider":"fake","model_id":"fake-2"}),
        "omp_error",
    );
    assert!(
        err["message"]
            .as_str()
            .unwrap_or("")
            .contains("registry exploded"),
        "AC-366: the thrown message: {err}"
    );
    drop(h);

    let h = CtlHost::start(&lb, nd, &dir, &[("HOST_BUSY", "1")]);
    let d = op_ok(
        &h.socket(&lb),
        "s6",
        "set_model",
        json!({"provider":"other","model_id":"fake-2"}),
    );
    assert_eq!(d["busy"], true, "{d}");
    assert_eq!(h.count("abort"), 0, "a model switch never stops the run");
}

#[test]
fn node_ac366_set_thinking_level_is_checked_against_the_current_model() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install_for_node(&lb);
    let h = CtlHost::start(&lb, nd, &dir, &[]);
    let sock = h.socket(&lb);
    let err = op_err(
        &sock,
        "t1",
        "set_thinking_level",
        json!({"level":"xhigh"}),
        "invalid_level",
    );
    assert_eq!(
        err["details"]["available"],
        strs_json(&FAKE1_LEVELS),
        "{err}"
    );
    assert_eq!(err["details"]["model"], mref("fake", "fake-1"), "{err}");
    assert_eq!(
        h.count("setThinkingLevel"),
        0,
        "no setter call for an invalid level"
    );

    let notes = h.count("notify");
    let d = op_ok(&sock, "t2", "set_thinking_level", json!({"level":"high"}));
    assert_eq!(d["thinking_level"], "high", "{d}");
    assert_eq!(d["previous"], "medium", "{d}");
    assert_eq!(d["model"], mref("fake", "fake-1"), "{d}");
    assert_eq!(d["available"], strs_json(&FAKE1_LEVELS), "{d}");
    assert_eq!(d["busy"], false, "{d}");
    assert_eq!(d["session_id"], A.id, "{d}");
    assert_eq!(h.of("setThinkingLevel")[0]["level"], "high");
    assert_eq!(h.count("notify"), notes + 1, "one notice per applied op");

    op_ok(
        &sock,
        "t3",
        "set_model",
        json!({"provider":"fake","model_id":"fake-2"}),
    );
    let err = op_err(
        &sock,
        "t4",
        "set_thinking_level",
        json!({"level":"low"}),
        "invalid_level",
    );
    assert_eq!(
        err["details"]["available"],
        json!(["off"]),
        "a non-reasoning model: {err}"
    );
    assert_eq!(h.count("setThinkingLevel"), 1);
    h.assert_clean();
}

#[test]
fn node_ac356_ac366_abort_idle_busy_and_a_run_that_never_stops() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install_for_node(&lb);

    let h = CtlHost::start(&lb, nd, &dir, &[("HOST_PENDING", "1")]);
    let d = op_ok(&h.socket(&lb), "a1", "abort", json!({}));
    assert_eq!(d["was"], "idle", "{d}");
    assert_eq!(d["aborted"], false, "{d}");
    assert_eq!(d["idle_after"], true, "{d}");
    assert_eq!(d["queued"], true, "queued messages are reported: {d}");
    assert_eq!(d["session_id"], A.id, "{d}");
    assert_eq!(h.count("abort"), 0, "AC-366: idle → no ctx.abort call");
    drop(h);

    let h = CtlHost::start(&lb, nd, &dir, &[("HOST_BUSY", "1")]);
    let notes = h.count("notify");
    let t = Instant::now();
    let d = op_ok(&h.socket(&lb), "a2", "abort", json!({}));
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert_eq!(d["was"], "busy", "{d}");
    assert_eq!(d["aborted"], true, "{d}");
    assert_eq!(d["idle_after"], true, "{d}");
    assert_eq!(d["queued"], false, "{d}");
    assert_eq!(
        h.count("abort"),
        1,
        "AC-356: ctx.abort() once\n{}",
        h.diag()
    );
    assert_eq!(h.count("notify"), notes + 1, "one notice");
    h.assert_clean();
    drop(h);

    let h = CtlHost::start(
        &lb,
        nd,
        &dir,
        &[("HOST_BUSY", "1"), ("HOST_ABORT_IDLES", "0")],
    );
    let t = Instant::now();
    let d = op_ok(&h.socket(&lb), "a3", "abort", json!({}));
    let took = t.elapsed();
    assert!(
        took >= Duration::from_millis(2500) && took < Duration::from_secs(5),
        "AC-356: answers after the 3 s settle wait, took {took:?}"
    );
    assert_eq!(d["aborted"], true, "{d}");
    assert_eq!(d["idle_after"], false, "{d}");
    assert_eq!(h.count("abort"), 1, "still exactly one ctx.abort()");
    h.assert_clean();
}

#[test]
fn node_ac362_ac366_compact_refuses_busy_and_runs_one_at_a_time() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install_for_node(&lb);

    for knob in [("HOST_BUSY", "1"), ("HOST_PENDING", "1")] {
        let h = CtlHost::start(&lb, nd, &dir, &[knob]);
        op_err(&h.socket(&lb), "c1", "compact", json!({}), "busy");
        assert_eq!(h.count("compact"), 0, "{knob:?}: never compacts while busy");
        assert_eq!(h.count("abort"), 0);
    }

    let h = CtlHost::start(&lb, nd, &dir, &[]);
    let notes = h.count("notify");
    let d = op_ok(
        &h.socket(&lb),
        "c2",
        "compact",
        json!({"instructions":"keep decisions"}),
    );
    assert_eq!(d["tokens_before"], 1234, "{d}");
    assert_eq!(d["tokens_after"], 123, "{d}");
    assert_eq!(d["session_id"], A.id, "{d}");
    assert_eq!(h.of("compact")[0]["instructions"], "keep decisions");
    assert!(h.count("notify") > notes, "a notice for the compaction");
    drop(h);

    let h = CtlHost::start(
        &lb,
        nd,
        &dir,
        &[("HOST_COMPACT_REJECT", "Nothing to compact")],
    );
    let err = op_err(&h.socket(&lb), "c3", "compact", json!({}), "omp_error");
    assert!(
        err["message"]
            .as_str()
            .unwrap_or("")
            .contains("Nothing to compact"),
        "{err}"
    );
    drop(h);

    // A compaction it started is busy: a second compact is refused and abort cancels the first.
    let h = CtlHost::start(&lb, nd, &dir, &[("HOST_COMPACT_MS", "6000")]);
    let sock = h.socket(&lb);
    let first = {
        let sock = sock.clone();
        std::thread::spawn(move || op(&sock, "c4", "compact", json!({})))
    };
    poll_until("the first ctx.compact call", LONG, || {
        (h.count("compact") == 1).then_some(())
    });
    op_err(&sock, "c5", "compact", json!({}), "busy");
    assert_eq!(h.count("compact"), 1, "one compaction at a time");
    let d = op_ok(&sock, "c6", "abort", json!({}));
    assert_eq!(
        d["was"], "busy",
        "AC-366: a compaction it started counts as busy: {d}"
    );
    assert_eq!(d["aborted"], true, "{d}");
    assert_eq!(h.count("abort"), 1);
    let f = first.join().unwrap();
    assert_eq!(f["ok"], false, "{f}");
    assert_eq!(
        f["error"]["code"], "omp_error",
        "the cancelled compaction: {f}"
    );
    h.assert_clean();
}

#[test]
fn node_ac366_a_different_expected_session_or_bad_args_change_nothing() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install_for_node(&lb);
    let h = CtlHost::start(&lb, nd, &dir, &[("HOST_BUSY", "1")]);
    let sock = h.socket(&lb);
    let other = uuid(0xdead);
    let mismatched = [
        ("abort", json!({"expect_session_id": other})),
        ("list_models", json!({"expect_session_id": other})),
        (
            "set_model",
            json!({"provider":"fake","model_id":"fake-2","expect_session_id": other}),
        ),
        (
            "set_thinking_level",
            json!({"level":"low","expect_session_id": other}),
        ),
        ("compact", json!({"expect_session_id": other})),
    ];
    for (i, (name, args)) in mismatched.into_iter().enumerate() {
        let err = op_err(&sock, &format!("x{i}"), name, args, "session_mismatch");
        assert_eq!(err["details"]["session_id"], A.id, "{name}: {err}");
    }
    let bad = [
        ("set_model", json!({})),
        ("set_model", json!({"provider":"fake"})),
        ("set_model", json!({"provider":"  ","model_id":"fake-1"})),
        ("set_thinking_level", json!({})),
        ("set_thinking_level", json!({"level":"banana"})),
        ("compact", json!({"instructions": 5})),
        ("compact", json!({"instructions": "x".repeat(524_289)})),
    ];
    for (i, (name, args)) in bad.into_iter().enumerate() {
        op_err(&sock, &format!("b{i}"), name, args, "bad_request");
    }
    assert_eq!(effects(&h), [0, 0, 0, 0], "no side effect\n{}", h.diag());
    assert_eq!(h.count("notify"), 0, "nothing applied, nothing notified");
    h.assert_clean();
}

#[test]
fn node_ac366_the_post_op_identity_is_reported_and_glen_exits_11() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install_for_node(&lb);
    let to = moved_id(A.id);
    let h = CtlHost::start(&lb, nd, &dir, &[("HOST_SWITCH_ON_SET_MODEL", to.as_str())]);
    let d = op_ok(
        &h.socket(&lb),
        "p1",
        "set_model",
        json!({"provider":"fake","model_id":"fake-2"}),
    );
    assert_eq!(
        d["session_id"],
        to.as_str(),
        "AC-366: re-read after the await: {d}"
    );
    drop(h);

    let _h = CtlHost::start(&lb, nd, &dir, &[("HOST_SWITCH_ON_SET_MODEL", to.as_str())]);
    let env = ok_doc(
        &ctl(&lb, &["model", A.handle, "other/fake-2"])
            .env("GLEN_OMP_BIN", NO_OMP)
            .run(),
        11,
    );
    assert_eq!(
        env["data"]["forked"],
        json!({"from": A.id, "to": to, "to_handle": tail7(&to)}),
        "{env}"
    );
    lb.assert_no_omp();
}

#[test]
fn node_ac366_without_a_ui_nothing_is_notified() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install_for_node(&lb);
    let h = CtlHost::start(&lb, nd, &dir, &[("HOST_NO_UI", "1")]);
    let sock = h.socket(&lb);
    op_ok(
        &sock,
        "n1",
        "set_model",
        json!({"provider":"other","model_id":"fake-2"}),
    );
    op_ok(&sock, "n2", "set_thinking_level", json!({"level":"max"}));
    op_ok(&sock, "n3", "compact", json!({}));
    assert_eq!(h.count("notify"), 0, "{}", h.diag());
    h.assert_clean();
}

#[test]
fn node_ac352_ac368_glen_controls_through_the_real_extension() {
    let Some(nd) = node_env() else { return };
    let lb = LiveBox::new();
    let dir = install_for_node(&lb);
    let mut h = CtlHost::start(&lb, nd, &dir, &[]);
    let sessions = snapshot(&lb.bx.sessions);
    let home = list_tree(lb.glen_home());
    let run = |args: &[&str]| ctl(&lb, args).env("GLEN_OMP_BIN", NO_OMP).run();

    let d = data_of(&run(&["models", A.handle]), 0);
    assert_eq!(d["route"], "live", "{d}");
    assert_eq!(d["current_source"], "session", "{d}");
    assert_eq!(d["current"], mref("fake", "fake-1"), "{d}");
    assert_rows(&d, Some(("fake", "fake-1")));

    let d = data_of(&run(&["model", A.handle, "other/fake-2"]), 0);
    assert_eq!(d["route"], "live", "{d}");
    assert_eq!(d["model"]["provider"], "other", "{d}");
    assert_eq!(d["previous"], mref("fake", "fake-1"), "{d}");

    let d = data_of(&run(&["thinking", A.handle, "xhigh"]), 0);
    assert_eq!(d["thinking_level"], "xhigh", "{d}");
    assert_eq!(d["model"], mref("other", "fake-2"), "{d}");

    let d = data_of(
        &run(&["compact", A.handle, "--instructions", "keep decisions"]),
        0,
    );
    assert_eq!(d["compacted"], true, "{d}");

    let d = data_of(&run(&["abort", A.handle]), 0);
    assert_eq!(d["route"], "live", "{d}");
    assert_eq!(d["was"], "idle", "{d}");
    assert_eq!(d["aborted"], false, "{d}");

    h.cmd("busy");
    let d = data_of(&run(&["abort", A.handle]), 0);
    assert_eq!(d["was"], "busy", "{d}");
    assert_eq!(d["aborted"], true, "{d}");

    assert_eq!(h.of("setModel")[0]["provider"], "other");
    assert_eq!(h.of("setThinkingLevel")[0]["level"], "xhigh");
    assert_eq!(h.of("compact")[0]["instructions"], "keep decisions");
    assert_eq!(h.count("abort"), 1);
    lb.assert_no_omp();
    assert_eq!(
        snapshot(&lb.bx.sessions),
        sessions,
        "AC-368: no omp session file was touched"
    );
    assert_eq!(
        list_tree(lb.glen_home()),
        home,
        "AC-368: the controls wrote nothing under GLEN_HOME"
    );
    h.assert_clean();
}
