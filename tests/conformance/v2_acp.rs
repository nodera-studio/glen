//! v0.2 W3 (the ACP spike, hidden `glen __acp`) conformance: AC-v2-301 … AC-v2-304.
//!
//! Derived from the settled acceptance criteria and "AC details" of
//! `.claude/plans/v020-wave-3-acp-spike.md` (with the binding judge edits in
//! `.claude/plans/v020-wave-3-acp-spike.judge-edits.md`) and from the public contracts they name:
//! `glen --help`, `glen __acp --help`, `docs/json.md`, `docs/design.md` §A2, §A9, §A15, §A16 and
//! §A23, the rendered skill (through `install-skill`), and the `fake_omp acp` scenarios and knobs
//! that §A15 documents. Written without reading `src/acp/`, `src/commands/acp.rs` or
//! `tests/acp.rs`: `glen __acp` is observed only through its exit code, its JSON document and the
//! frames the fake peer logs in `FAKE_OMP_LOG`.
//!
//! Every omp is the `fake_omp` example in `acp` mode (through `GLEN_OMP_BIN`), every `glen` runs with
//! a cleared environment against a scratch HOME, GLEN_HOME and agent directory copied from
//! `tests/fixtures/`, never a live `~/.omp`. The debug-only knobs `GLEN_ACP_SETUP_MS` and
//! `GLEN_ACP_CANCEL_GRACE_MS` shorten two waits; the one case that needs `GLEN_ACP_SETUP_MS` to
//! finish quickly returns early in a release build.
//!
//! The signal cases measure the plan's bounds: `session/cancel` is written within 250 ms of the
//! signal (asserted with 250 ms of scheduling slack, measured from the signal to the moment the
//! frame shows in the fake's log), and the process exits within 11 s (3.5 s before the prompt).
//! Under `scripts/flake-check.sh` these upper bounds and the waits are multiplied by
//! `GLEN_TEST_TIME_SCALE` (`flake::scaled`, exact in a plain run); lower bounds are not.
//! The real-omp experiments E1 to E3 are the orchestrator's and live in evidence; AC-v2-304 is
//! checked here over the documents that record them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use rustix::process::Signal;
use serde_json::{Value, json};

use crate::flake::{Captured, scaled};
use crate::live_support::*;
use crate::reader::*;
use crate::rpc_support::*;
use crate::support::*;

/// The session id `fake_omp acp` reveals on `session/new` (the documented `FAKE_OMP_ACP_SESSION`).
const NEW_ID: &str = "01a1ffff-0000-7000-8000-0123456789ab";
/// The message every run sends; the fake echoes it back as the last assistant message.
const MSG: &str = "conformance ping";
/// `session/cancel` is written within 250 ms of a signal (plus scheduling slack).
const CANCEL_BOUND: Duration = Duration::from_millis(500);

/// The keys of a success document (design §A23 "The document").
const DOC_KEYS: [&str; 14] = [
    "transport",
    "protocol_version",
    "agent",
    "permission",
    "session",
    "task_state",
    "stop_reason",
    "assistant_text",
    "text_truncated",
    "events",
    "events_dropped",
    "updates",
    "usage",
    "cancel",
];

// ---------------------------------------------------------------------------------------------
// Helpers: running `glen __acp`, the fake's log, state on disk.
// ---------------------------------------------------------------------------------------------

fn read(rel: &str) -> String {
    fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// `glen <args>` from the box's alpha directory with the fake in `scenario`; `args` are complete
/// (they include `__acp`).
fn glen_acp(rb: &RpcBox, scenario: &str, args: &[&str], env: &[(&str, &str)]) -> (Command, String) {
    let mut cmd = rb.command(args);
    cmd.env("FAKE_OMP_ACP", scenario)
        .env("FAKE_OMP_ACP_SESSION", NEW_ID)
        .env("FAKE_OMP_VERSION", TESTED_OMP);
    for (k, v) in env {
        cmd.env(k, v);
    }
    (cmd, args.join(" "))
}

/// `glen __acp <session> <message> <extra…> --json`.
fn acp_with(rb: &RpcBox, scenario: &str, session: &str, message: &str, extra: &[&str]) -> Run {
    let mut args = vec!["__acp", session, message];
    args.extend_from_slice(extra);
    args.push("--json");
    let (cmd, desc) = glen_acp(rb, scenario, &args, &[]);
    Run::from_command(cmd, desc)
}

fn acp(rb: &RpcBox, scenario: &str, session: &str, extra: &[&str]) -> Run {
    acp_with(rb, scenario, session, MSG, extra)
}

/// A background `glen __acp <session> MSG --json`, its stdout and stderr captured in files.
fn spawn_acp(rb: &RpcBox, scenario: &str, session: &str, env: &[(&str, &str)]) -> Captured {
    let (mut cmd, desc) = glen_acp(rb, scenario, &["__acp", session, MSG, "--json"], env);
    Captured::spawn(&mut cmd).unwrap_or_else(|e| panic!("spawn glen {desc}: {e}"))
}

/// Frames in direction `dir` (`"in"` = glen → omp, `"out"` = omp → glen) of a fake log.
fn frames(recs: &[Value], dir: &str) -> Vec<Value> {
    recs.iter()
        .filter(|r| r["event"] == dir && r.get("frame").is_some())
        .map(|r| r["frame"].clone())
        .collect()
}

/// The `method` of every frame glen wrote, in order (`<response>` for an answer to the agent).
fn methods(recs: &[Value]) -> Vec<String> {
    frames(recs, "in")
        .iter()
        .map(|f| f["method"].as_str().unwrap_or("<response>").to_owned())
        .collect()
}

/// The first frame glen wrote for `method`.
fn sent(recs: &[Value], method: &str) -> Value {
    frames(recs, "in")
        .into_iter()
        .find(|f| f["method"] == method)
        .unwrap_or_else(|| panic!("glen never sent `{method}`: {:?}", methods(recs)))
}

fn is_cancel(r: &Value) -> bool {
    r["event"] == "in" && r["frame"]["method"] == "session/cancel"
}

/// An `agent_message_chunk` the fake wrote (the turn is under way).
/// The turn's first agent chunk: one logged after the `session/prompt` frame. `session/load`
/// replays an existing session's history as agent chunks too, and a signal sent on one of those
/// lands before the prompt, when there is no turn to cancel.
fn first_chunk_logged(recs: &[Value]) -> bool {
    recs.iter()
        .skip_while(|r| !(r["event"] == "in" && r["frame"]["method"] == "session/prompt"))
        .any(|r| {
            r["event"] == "out"
                && r["frame"]["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
        })
}

#[test]
fn the_first_chunk_is_the_turns_never_a_session_load_replay() {
    let chunk = |text: &str| {
        json!({"event":"out","frame":{"method":"session/update","params":{"update":{
            "sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}}})
    };
    let method = |m: &str| json!({"event":"in","frame":{"method":m}});
    // Run 41621490685's log at the signal: a load whose replay holds an agent chunk.
    let mut recs = vec![
        method("initialize"),
        method("session/load"),
        chunk("REPLAYED ANSWER"),
    ];
    assert!(!first_chunk_logged(&recs));
    recs.push(method("session/prompt"));
    assert!(!first_chunk_logged(&recs));
    recs.push(chunk("Hel"));
    assert!(first_chunk_logged(&recs));
}

fn initialize_logged(recs: &[Value]) -> bool {
    recs.iter()
        .any(|r| r["event"] == "in" && r["frame"]["method"] == "initialize")
}

/// The instant a `session/cancel` frame shows in the fake's log (polled every 2 ms).
fn wait_for_cancel_frame(rb: &RpcBox, timeout: Duration) -> Option<Instant> {
    let deadline = Instant::now() + timeout;
    loop {
        if rb.recs().iter().any(is_cancel) {
            return Some(Instant::now());
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Every omp the box started is gone, process and group, within 3 s.
fn assert_omp_gone(rb: &RpcBox) {
    let mine = own_pgid();
    let all = rb.starts();
    assert!(!all.is_empty(), "no omp was started\n{}", rb.diag());
    for st in all {
        let pid = st["pid"].as_i64().unwrap_or(0);
        let pgid = st["pgid"].as_i64().unwrap_or(0);
        poll_until(
            "the omp process and its group to be gone",
            scaled(Duration::from_secs(3)),
            || (!pid_alive(pid) && (pgid == mine || !group_alive(pgid))).then_some(()),
        );
    }
}

/// omp's `stderr` detail as one string (an array of lines or a string).
fn stderr_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        other => panic!("details.stderr is neither a string nor an array: {other}"),
    }
}

/// No `jobs/` and no `inbox/` under GLEN_HOME and no job directory at all (AC-v2-303).
fn assert_no_durable_state(rb: &RpcBox) {
    for dir in ["jobs", "inbox"] {
        assert!(
            !rb.bx.glen_home.join(dir).exists(),
            "GLEN_HOME gained `{dir}/`: {:?}",
            list_tree(&rb.bx.glen_home)
        );
    }
    assert!(rb.job_ids().is_empty(), "a job exists: {:?}", rb.job_ids());
}

/// The sessions tree (names, sizes, mtimes) and every `.jsonl` file under the box.
struct Tree {
    sessions: Vec<(String, u64, SystemTime)>,
    jsonl: Vec<String>,
}

fn tree(rb: &RpcBox) -> Tree {
    Tree {
        sessions: snapshot(&rb.bx.sessions),
        jsonl: list_tree(&rb.bx.root)
            .into_iter()
            .filter(|p| p.ends_with(".jsonl"))
            .collect(),
    }
}

/// AC-v2-303: no `.jsonl` was written and the sessions tree is unchanged byte for byte.
fn assert_untouched(rb: &RpcBox, before: &Tree) {
    let after = tree(rb);
    assert_eq!(after.jsonl, before.jsonl, "a .jsonl file appeared or went");
    assert_eq!(
        after.sessions, before.sessions,
        "the sessions tree changed (names, sizes or mtimes)"
    );
}

/// The one `ok:true` document of a successful run; the success keys all present.
fn success(out: &Out) -> Value {
    let d = data_of(out, 0);
    for k in DOC_KEYS {
        assert!(d.get(k).is_some(), "success document lacks `{k}`: {d}");
    }
    d
}

fn sum_of(kinds: &Value) -> u64 {
    kinds
        .as_object()
        .unwrap_or_else(|| panic!("updates.kinds is not an object: {kinds}"))
        .values()
        .map(|v| v.as_u64().expect("a count"))
        .sum()
}

// ---------------------------------------------------------------------------------------------
// AC-v2-301: hidden from help, completions, the skill, docs/json.md and the MCP catalog.
// ---------------------------------------------------------------------------------------------

/// The `(command)` names under `Commands:` in `glen --help`.
fn help_commands(help: &str) -> Vec<String> {
    let mut in_commands = false;
    let mut out = Vec::new();
    for line in help.lines() {
        if line.trim_end() == "Commands:" {
            in_commands = true;
            continue;
        }
        if !in_commands {
            continue;
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("  ")
            && !rest.starts_with(' ')
            && let Some(name) = rest.split_whitespace().next()
        {
            out.push(name.to_owned());
        }
    }
    out
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[test]
fn ac_v2_301_acp_is_hidden_and_help_lists_exactly_the_visible_commands() {
    assert!(
        HIDDEN.contains(&"__acp"),
        "the conformance HIDDEN list names __acp"
    );
    assert_eq!(
        VISIBLE.len(),
        20,
        "VISIBLE lists 20 commands (v0.2 W5 adds mcp)"
    );
    assert!(!VISIBLE.contains(&"__acp"));
    let sb = Sandbox::new();
    let mut want: Vec<String> = VISIBLE.iter().map(|s| (*s).to_owned()).collect();
    want.sort();
    for args in [vec!["--help"], vec!["-h"], vec!["help"]] {
        let out = sb.glen(&args).run();
        out.assert_exit(0);
        let mut got = help_commands(&out.out());
        got.sort();
        assert_eq!(got, want, "{}", out.ctx());
    }
    let mut pages: Vec<Vec<&str>> = vec![vec!["--help"], vec!["-h"], vec!["help"]];
    for c in VISIBLE.iter().filter(|c| **c != "help") {
        pages.push(vec![*c, "--help"]);
        pages.push(vec!["help", *c]);
    }
    for args in &pages {
        let out = sb.glen(args).run();
        out.assert_exit(0);
        assert!(
            !out.out().contains("__acp") && !out.err().contains("__acp"),
            "{}",
            out.ctx()
        );
    }
    assert!(!sb.glen_home.exists(), "help pages create no GLEN_HOME");
}

#[test]
fn ac_v2_301_acp_has_a_help_page_that_names_permission() {
    let sb = Sandbox::new();
    for args in [vec!["__acp", "--help"], vec!["help", "__acp"]] {
        let out = sb.glen(&args).run();
        out.assert_exit(0);
        let text = out.out();
        for needle in ["Usage", "__acp", "--permission", "allow", "deny"] {
            assert!(
                text.contains(needle),
                "`glen {}` lacks `{needle}`\n{}",
                args.join(" "),
                out.ctx()
            );
        }
    }
    assert!(!sb.glen_home.exists());
}

#[test]
fn ac_v2_301_acp_is_absent_from_every_completion_script() {
    let sb = Sandbox::new();
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let raw = sb.glen(["completions", shell]).run();
        raw.assert_exit(0);
        assert!(
            !raw.out().contains("__acp"),
            "{shell} completions expose __acp"
        );
        let data = sb.glen(["completions", shell, "--json"]).run().ok_data();
        assert!(
            !data["script"]
                .as_str()
                .expect("a script string")
                .contains("__acp"),
            "{shell} completions (--json) expose __acp"
        );
    }
}

#[test]
fn ac_v2_301_acp_is_absent_from_the_skill_json_contract_and_mcp_catalog() {
    let sb = Sandbox::new();
    sb.glen(["install-skill", "--project", "--json"])
        .run()
        .ok_data();
    for rel in [
        ".claude/skills/glen/SKILL.md",
        ".agents/skills/glen/SKILL.md",
    ] {
        let text = fs::read_to_string(sb.cwd.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert!(!text.contains("__acp"), "the rendered {rel} names __acp");
    }
    let mut files = Vec::new();
    walk_files(&repo_root().join("skill"), &mut files);
    walk_files(&repo_root().join("src/mcp"), &mut files);
    files.push(repo_root().join("docs/json.md"));
    for f in files {
        if let Ok(text) = fs::read_to_string(&f) {
            assert!(!text.contains("__acp"), "{} names __acp", f.display());
        }
    }
}

#[test]
fn ac_v2_301_no_hidden_entry_answers_not_implemented() {
    let rb = RpcBox::new();
    for name in HIDDEN {
        let (cmd, desc) = glen_acp(&rb, "ok", &[name, "@new", MSG, "--json"], &[]);
        let out = Run::from_command(cmd, desc).run();
        assert_ne!(out.code(), 8, "{name} is not implemented\n{}", out.ctx());
        assert!(
            !out.out().contains("not_implemented"),
            "{name} answers not_implemented\n{}",
            out.ctx()
        );
        out.assert_exit(0);
    }
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-v2-302: usage and setup failures (nothing is spawned for a usage error).
// ---------------------------------------------------------------------------------------------

#[test]
fn ac_v2_302_usage_errors_start_no_omp_and_create_no_state() {
    let rb = RpcBox::new();
    for session in ["@new", A.handle] {
        let out = acp_with(&rb, "ok", session, "", &[]).run();
        failed(&out, "usage");
    }
    // A missing message and an unknown --permission value are clap usage errors (exit 2).
    let (cmd, desc) = glen_acp(&rb, "ok", &["__acp", "@new", "--json"], &[]);
    Run::from_command(cmd, desc).run().assert_exit(2);
    let out = acp(&rb, "ok", "@new", &["--permission", "bogus"]).run();
    out.assert_exit(2);
    assert!(
        !out.out().contains("\"ok\":true"),
        "a bad --permission must not succeed\n{}",
        out.ctx()
    );
    // An unknown session handle is not_found, before any omp.
    let out = acp(&rb, "ok", "deadbee", &[]).run();
    failed(&out, "not_found");
    assert!(rb.starts().is_empty(), "omp started\n{}", rb.diag());
    assert_no_durable_state(&rb);
}

#[test]
fn ac_v2_302_an_initialize_that_is_not_protocol_v1_exits_5_acp_protocol() {
    for scenario in ["proto2", "proto_missing", "init_error"] {
        for session in ["@new", A.handle] {
            let rb = RpcBox::new();
            let before = tree(&rb);
            let out = acp(&rb, scenario, session, &[]).run();
            let err = failed(&out, "omp_unavailable");
            let det = &err["details"];
            assert_eq!(det["reason"], "acp_protocol", "{scenario} {session}: {err}");
            assert_eq!(det["requested"], 1, "{scenario} {session}: {err}");
            if scenario == "proto2" {
                assert_eq!(det["got"], 2, "{scenario} {session}: {err}");
            }
            assert_eq!(
                methods(&rb.recs()),
                ["initialize"],
                "{scenario} {session}: nothing but initialize may be sent\n{}",
                rb.diag()
            );
            assert_omp_gone(&rb);
            assert!(rb.glen_lock_free(A.id) && rb.glen_lock_free(NEW_ID));
            assert_no_durable_state(&rb);
            assert_untouched(&rb, &before);
        }
    }
}

#[test]
fn ac_v2_302_an_agent_without_load_session_fails_only_an_existing_session() {
    let rb = RpcBox::new();
    let out = acp(&rb, "no_load", A.handle, &[]).run();
    let err = failed(&out, "omp_unavailable");
    assert_eq!(err["details"]["reason"], "acp_capability", "{err}");
    assert_eq!(err["details"]["capability"], "loadSession", "{err}");
    assert_eq!(methods(&rb.recs()), ["initialize"], "{}", rb.diag());
    assert_omp_gone(&rb);
    assert!(rb.glen_lock_free(A.id));

    // @new needs no session/load; omp advertised no sessionCapabilities, so no session/close.
    let log = rb.log_path("new");
    let out = acp(&rb, "no_load", "@new", &[])
        .env("FAKE_OMP_LOG", &log)
        .run();
    let d = success(&out);
    assert_eq!(d["task_state"], "completed", "{d}");
    assert_eq!(
        methods(&records(&log)),
        ["initialize", "session/new", "session/prompt"],
        "session/close is sent only when omp advertises it"
    );
    rb.assert_quiet();
}

#[test]
fn ac_v2_302_an_omp_that_exits_before_initialize_is_acp_exited_with_its_stderr() {
    let rb = RpcBox::new();
    let out = acp(&rb, "startup_exit", "@new", &[])
        .env("FAKE_OMP_STARTUP_STDERR", "fake omp refuses to start\n")
        .run();
    let err = failed(&out, "omp_unavailable");
    assert_eq!(err["details"]["reason"], "acp_exited", "{err}");
    assert!(
        stderr_text(&err["details"]["stderr"]).contains("fake omp refuses to start"),
        "details.stderr carries omp's stderr: {err}"
    );
    assert_omp_gone(&rb);
    assert_no_durable_state(&rb);
}

#[test]
fn ac_v2_302_an_unanswered_initialize_times_out_acp_timeout() {
    if !cfg!(debug_assertions) {
        eprintln!("skipped: GLEN_ACP_SETUP_MS is read by debug builds only");
        return;
    }
    let rb = RpcBox::new();
    let out = acp(&rb, "init_hang", "@new", &[])
        .env("GLEN_ACP_SETUP_MS", "300")
        .run();
    let err = failed(&out, "omp_unavailable");
    assert_eq!(err["details"]["reason"], "acp_timeout", "{err}");
    assert!(
        out.elapsed < scaled(Duration::from_secs(15)),
        "the setup bound is 300 ms plus teardown, took {:?}",
        out.elapsed
    );
    assert_omp_gone(&rb);
    assert_no_durable_state(&rb);
}

#[test]
fn ac_v2_302_a_bad_frame_after_the_initialize_result_is_job_failed_acp_malformed() {
    let rb = RpcBox::new();
    let out = acp(&rb, "noise", "@new", &[]).run();
    let err = failed(&out, "job_failed");
    assert_eq!(err["details"]["reason"], "acp_malformed", "{err}");
    assert_omp_gone(&rb);
    assert_no_durable_state(&rb);
}

#[test]
fn ac_v2_302_a_missing_omp_is_omp_unavailable_acp_spawn() {
    let rb = RpcBox::new();
    for session in ["@new", A.handle] {
        let out = acp(&rb, "ok", session, &[])
            .env("GLEN_OMP_BIN", "/nonexistent/omp")
            .run();
        let err = failed(&out, "omp_unavailable");
        assert_eq!(err["details"]["reason"], "acp_spawn", "{session}: {err}");
        assert!(
            err["hint"]
                .as_str()
                .is_some_and(|h| h.contains("GLEN_OMP_BIN")),
            "the hint names GLEN_OMP_BIN: {err}"
        );
    }
    assert!(rb.starts().is_empty());
    assert!(rb.glen_lock_free(A.id));
    assert_no_durable_state(&rb);
}

// ---------------------------------------------------------------------------------------------
// AC-v2-302: the happy paths, text rebuilt from `session/update`.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac_v2_302_a_new_session_rebuilds_the_last_assistant_message_from_updates() {
    let rb = RpcBox::new();
    let before = tree(&rb);
    let out = acp(&rb, "ok", "@new", &[]).run();
    let d = success(&out);
    assert_eq!(d["transport"], "acp", "{d}");
    assert_eq!(d["protocol_version"], 1, "{d}");
    assert_eq!(d["agent"]["name"], "omp", "{d}");
    assert_eq!(d["agent"]["version"], TESTED_OMP, "{d}");
    assert_eq!(d["permission"], "deny", "the default policy is deny: {d}");
    let s = &d["session"];
    assert_eq!(s["id"], NEW_ID, "{d}");
    assert_eq!(s["handle"], tail7(NEW_ID), "{d}");
    assert!(s["path"].is_null(), "@new has no session file: {d}");
    assert_eq!(s["created"], true, "{d}");
    assert_eq!(d["task_state"], "completed", "{d}");
    assert_eq!(d["stop_reason"], "end_turn", "{d}");
    // The last non-empty message group, not the whole stream: "Hel" + "lo " (m1) are dropped.
    assert_eq!(d["assistant_text"], format!("reply: {MSG}"), "{d}");
    assert_eq!(d["text_truncated"], false, "{d}");
    assert_eq!(d["events"], json!([]), "{d}");
    assert_eq!(d["events_dropped"], 0, "{d}");
    assert!(d["usage"].is_object(), "usage is kept: {d}");
    assert!(d["cancel"].is_null(), "no cancel: {d}");
    let u = &d["updates"];
    assert_eq!(u["replayed"], 0, "a new session replays nothing: {d}");
    assert_eq!(
        u["kinds"],
        json!({"agent_thought_chunk":1,"agent_message_chunk":4,"tool_call":1,"tool_call_update":1}),
        "{d}"
    );
    assert_eq!(u["during_turn"], sum_of(&u["kinds"]), "{d}");
    assert_eq!(u["during_turn"], 7, "{d}");
    assert_eq!(u["messages"], 2, "two non-empty assistant groups: {d}");

    assert_omp_gone(&rb);
    assert!(rb.glen_lock_free(NEW_ID), "the glen lock is released");
    assert_no_durable_state(&rb);
    assert_untouched(&rb, &before);
    assert!(rb.recs().iter().all(|r| r["event"] != "permission"));
    rb.assert_quiet();
}

#[test]
fn ac_v2_302_the_wire_follows_acp_v1_initialize_new_prompt_close() {
    let rb = RpcBox::new();
    success(&acp(&rb, "ok", "@new", &[]).run());
    let recs = rb.recs();
    assert_eq!(
        methods(&recs),
        [
            "initialize",
            "session/new",
            "session/prompt",
            "session/close"
        ],
        "{}",
        rb.diag()
    );
    let wire = frames(&recs, "in");
    let mut last = 0;
    for f in &wire {
        assert_eq!(f["jsonrpc"], "2.0", "{f}");
        let id = &f["id"];
        assert!(
            !id.is_null(),
            "every frame glen wrote here is a request: {f}"
        );
        let n = id
            .as_u64()
            .unwrap_or_else(|| panic!("request ids are integers: {f}"));
        assert!(n > last, "ids ascend from 1: {wire:?}");
        last = n;
    }
    assert_eq!(wire[0]["id"], 1, "ids start at 1");
    let init = &wire[0]["params"];
    assert_eq!(init["protocolVersion"], 1, "{init}");
    assert_eq!(
        init["clientCapabilities"],
        json!({"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}),
        "{init}"
    );
    assert_eq!(init["clientInfo"]["name"], "glen", "{init}");
    assert_eq!(init["clientInfo"]["title"], "glen", "{init}");
    assert_eq!(init["clientInfo"]["version"], PKG_VERSION, "{init}");
    let new = &wire[1]["params"];
    assert_eq!(new["mcpServers"], json!([]), "{new}");
    assert_cwd(&new["cwd"], &rb.bx.alpha);
    let prompt = &wire[2]["params"];
    assert_eq!(prompt["sessionId"], NEW_ID, "{prompt}");
    assert_eq!(
        prompt["prompt"],
        json!([{"type":"text","text":MSG}]),
        "{prompt}"
    );
    assert_eq!(wire[3]["params"]["sessionId"], NEW_ID, "{}", wire[3]);
    rb.assert_quiet();
}

/// A JSON cwd that is absolute and names `dir`.
fn assert_cwd(v: &Value, dir: &Path) {
    let p = v
        .as_str()
        .unwrap_or_else(|| panic!("cwd is not a string: {v}"));
    assert!(Path::new(p).is_absolute(), "cwd {p} is absolute");
    assert!(
        same_file(Path::new(p), dir),
        "cwd {p} is not {}",
        dir.display()
    );
}

#[test]
fn ac_v2_302_omp_runs_in_its_own_group_with_glen_managed_and_the_process_cwd() {
    let rb = RpcBox::new();
    success(&acp(&rb, "ok", "@new", &[]).run());
    let st = &rb.starts()[0];
    assert_eq!(st["env"]["GLEN_MANAGED"], "1", "{st}");
    assert_eq!(st["argv"], json!(["acp"]), "{st}");
    assert_cwd(&st["cwd"], &rb.bx.alpha);
    let (pid, pgid) = (st["pid"].as_i64().unwrap(), st["pgid"].as_i64().unwrap());
    assert_eq!(pgid, pid, "omp leads its own process group: {st}");
    assert_ne!(pgid, own_pgid());
    rb.assert_quiet();
}

#[test]
fn ac_v2_302_an_explicit_profile_is_passed_to_omp_acp_and_the_profile_env_is_dropped() {
    let rb = RpcBox::new();
    let (cmd, desc) = glen_acp(
        &rb,
        "ok",
        &["--profile", "work", "__acp", "@new", MSG, "--json"],
        &[("OMP_PROFILE", "other"), ("PI_PROFILE", "other")],
    );
    success(&Run::from_command(cmd, desc).run());
    let st = &rb.starts()[0];
    assert_eq!(st["argv"], json!(["acp", "--profile", "work"]), "{st}");
    assert!(st["env"]["OMP_PROFILE"].is_null(), "{st}");
    assert!(st["env"]["PI_PROFILE"].is_null(), "{st}");
    assert_eq!(st["env"]["GLEN_MANAGED"], "1", "{st}");
    rb.assert_quiet();
}

#[test]
fn ac_v2_302_an_existing_session_is_loaded_and_its_replay_is_not_the_answer() {
    let rb = RpcBox::new();
    let before = tree(&rb);
    let out = acp(&rb, "ok", A.handle, &[])
        .env("FAKE_OMP_ACP_REPLAY", "3")
        .run();
    let d = success(&out);
    let s = &d["session"];
    assert_eq!(s["id"], A.id, "{d}");
    assert_eq!(s["handle"], A.handle, "{d}");
    assert_eq!(s["created"], false, "{d}");
    assert_path(&s["path"], &rb.bx.path(&A));
    assert_eq!(d["task_state"], "completed", "{d}");
    assert_eq!(d["stop_reason"], "end_turn", "{d}");
    assert_eq!(
        d["assistant_text"],
        format!("reply: {MSG}"),
        "the replayed history is not part of the answer: {d}"
    );
    assert!(
        !d["assistant_text"]
            .as_str()
            .unwrap_or("")
            .contains("REPLAYED"),
        "{d}"
    );
    let u = &d["updates"];
    assert!(
        u["replayed"].as_u64().unwrap_or(0) >= 6,
        "three replayed pairs count as replayed: {d}"
    );
    assert_eq!(
        u["kinds"]["agent_message_chunk"], 4,
        "only the turn's own chunks are counted by kind: {d}"
    );
    assert_eq!(u["messages"], 2, "{d}");

    let recs = rb.recs();
    let m = methods(&recs);
    assert_eq!(&m[..3], ["initialize", "session/load", "session/prompt"]);
    assert!(!m.iter().any(|x| x == "session/new"), "{m:?}");
    let load = sent(&recs, "session/load");
    assert_eq!(load["params"]["sessionId"], A.id, "{load}");
    assert_eq!(load["params"]["mcpServers"], json!([]), "{load}");
    // The fixture's recorded cwd is not a directory here, so omp runs in glen's own cwd.
    assert_cwd(&load["params"]["cwd"], &rb.bx.alpha);
    assert_eq!(sent(&recs, "session/prompt")["params"]["sessionId"], A.id);
    assert_eq!(rb.starts()[0]["env"]["GLEN_MANAGED"], "1");
    assert_omp_gone(&rb);
    assert!(rb.glen_lock_free(A.id));
    assert_no_durable_state(&rb);
    assert_untouched(&rb, &before);
    rb.assert_quiet();
}

#[test]
fn ac_v2_302_bootstrap_updates_after_the_load_do_not_change_the_answer() {
    // The fake emits non-text bootstrap updates 100 ms after the load answer; whenever they land,
    // the text and the stop reason stay those of the turn.
    let rb = RpcBox::new();
    for _ in 0..3 {
        let d = success(&acp(&rb, "ok", A.handle, &[]).run());
        assert_eq!(d["assistant_text"], format!("reply: {MSG}"), "{d}");
        assert_eq!(d["stop_reason"], "end_turn", "{d}");
        assert_eq!(d["updates"]["kinds"]["agent_message_chunk"], 4, "{d}");
    }
    rb.assert_quiet();
}

#[test]
fn ac_v2_302_a_session_is_addressed_by_handle_id_or_latest() {
    let rb = RpcBox::new();
    for (session, want) in [(A.handle, A.id), (A.id, A.id), ("@latest", G.id)] {
        let d = success(&acp(&rb, "ok", session, &[]).run());
        assert_eq!(d["session"]["id"], want, "{session}: {d}");
        assert_eq!(d["session"]["handle"], tail7(want), "{session}: {d}");
        assert_eq!(d["session"]["created"], false, "{session}: {d}");
    }
    rb.assert_quiet();
}

#[test]
fn ac_v2_302_the_last_message_group_is_the_text_and_stop_reasons_map() {
    // `stop` streams "Hel" "lo " (m1) and then `stopped: <reason>` (m2).
    let rb = RpcBox::new();
    let d = success(
        &acp(&rb, "stop", "@new", &[])
            .env("FAKE_OMP_ACP_STOP", "end_turn")
            .run(),
    );
    assert_eq!(d["assistant_text"], "stopped: end_turn", "{d}");
    assert_eq!(d["task_state"], "completed", "{d}");
    assert_eq!(d["stop_reason"], "end_turn", "{d}");

    for (stop, state) in [
        ("cancelled", "canceled"),
        ("refusal", "rejected"),
        ("max_tokens", "failed"),
        ("max_turn_requests", "failed"),
        ("some_future_reason", "failed"),
    ] {
        let out = acp(&rb, "stop", "@new", &[])
            .env("FAKE_OMP_ACP_STOP", stop)
            .run();
        let err = failed(&out, "job_failed");
        let det = &err["details"];
        assert_eq!(det["reason"], "acp_stop_reason", "{stop}: {err}");
        assert_eq!(det["stop_reason"], stop, "{stop}: {err}");
        assert_eq!(det["task_state"], state, "{stop}: {err}");
        assert_eq!(
            det["assistant_text"],
            format!("stopped: {stop}"),
            "the partial text rides on the error: {err}"
        );
        assert_eq!(det["session"]["id"], NEW_ID, "{stop}: {err}");
    }
    assert_omp_gone(&rb);
    rb.assert_quiet();
}

#[test]
fn ac_v2_302_human_output_is_sanitized_and_json_keeps_raw_text() {
    let rb = RpcBox::new();
    let nasty = "red \u{1b}[31mtext\u{1b}[0m";
    let (cmd, desc) = glen_acp(&rb, "ok", &["__acp", "@new", nasty, "--human"], &[]);
    let out = Run::from_command(cmd, desc).run();
    out.assert_exit(0).assert_no_ansi();
    assert!(out.out().contains("red "), "{}", out.ctx());
    assert!(out.out().contains(&tail7(NEW_ID)), "{}", out.ctx());

    let d = success(&acp_with(&rb, "ok", "@new", nasty, &[]).run());
    assert_eq!(
        d["assistant_text"],
        format!("reply: {nasty}"),
        "JSON keeps the raw string: {d}"
    );
    rb.assert_quiet();
}

#[test]
fn ac_v2_302_omp_stderr_is_not_relayed_on_success() {
    let rb = RpcBox::new();
    let out = acp(&rb, "ok", "@new", &[])
        .env("FAKE_OMP_STARTUP_STDERR", "chatty startup banner\n")
        .run();
    let d = success(&out);
    assert!(d.get("stderr").is_none(), "{d}");
    assert!(
        !out.out().contains("chatty startup banner"),
        "stderr only rides on failures\n{}",
        out.ctx()
    );
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-v2-302: `--permission allow|deny`, reported as `input_required`.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac_v2_302_permission_requests_are_answered_by_policy_and_reported_as_input_required() {
    // (--permission flag, offered options, the option id the policy must select). The policy is
    // the flag (deny by default); an unavailable wanted kind is answered `cancelled`.
    let cases: [(Option<&str>, &str, Option<&str>); 7] = [
        (None, "all", Some("reject_once")),
        (Some("deny"), "all", Some("reject_once")),
        (Some("allow"), "all", Some("allow_once")),
        (Some("deny"), "reject_only", Some("reject_once")),
        (Some("allow"), "allow_only", Some("allow_once")),
        (Some("deny"), "allow_only", None),
        (Some("allow"), "reject_only", None),
    ];
    let rb = RpcBox::new();
    for (n, (flag, options, option)) in cases.into_iter().enumerate() {
        let policy = flag.unwrap_or("deny");
        let answer = option.unwrap_or("cancelled");
        let log = rb.log_path(&format!("permission{n}"));
        let mut extra = vec![];
        if let Some(f) = flag {
            extra.extend(["--permission", f]);
        }
        let out = acp(&rb, "permission", "@new", &extra)
            .env("FAKE_OMP_ACP_OPTIONS", options)
            .env("FAKE_OMP_LOG", &log)
            .run();
        let what = format!("--permission {flag:?} with {options} options");
        let d = success(&out);
        assert_eq!(d["permission"], policy, "{what}: {d}");
        assert_eq!(d["task_state"], "completed", "{what}: {d}");
        assert_eq!(d["stop_reason"], "end_turn", "{what}: {d}");
        let events = d["events"].as_array().expect("events");
        assert_eq!(events.len(), 1, "{what}: one request, one event: {d}");
        assert_eq!(d["events_dropped"], 0, "{what}: {d}");
        let ev = &events[0];
        assert_eq!(ev["event"], "task_state", "{what}: {ev}");
        assert_eq!(ev["task_state"], "input_required", "{what}: {ev}");
        assert_eq!(ev["tool_call_id"], "t1", "{what}: {ev}");
        assert_eq!(ev["title"], "Delete e2-target.txt", "{what}: {ev}");
        assert_eq!(ev["kind"], "execute", "{what}: {ev}");
        assert_eq!(ev["policy"], policy, "{what}: {ev}");
        assert_eq!(ev["answer"], answer, "{what}: {ev}");
        assert_eq!(ev["option_id"], json!(option), "{what}: {ev}");

        // The wire carries the same answer, and never an `*_always` option.
        let recs = records(&log);
        let reply = recs
            .iter()
            .find(|r| r["event"] == "permission")
            .unwrap_or_else(|| panic!("{what}: the fake saw no permission answer\n{}", rb.diag()))
            ["response"]
            .clone();
        assert_eq!(reply["id"], 100, "{what}: {reply}");
        let want = match option {
            Some(o) => json!({"outcome":"selected","optionId":o}),
            None => json!({"outcome":"cancelled"}),
        };
        assert_eq!(reply["result"]["outcome"], want, "{what}: {reply}");
        assert_eq!(
            d["assistant_text"],
            format!("permission:{}", option.unwrap_or("cancelled")),
            "{what}: the fake reports what it received: {d}"
        );
        assert!(
            !reply.to_string().contains("always"),
            "{what}: an *_always option was answered: {reply}"
        );
    }
    rb.assert_quiet();
}

#[test]
fn ac_v2_302_every_other_agent_request_is_answered_method_not_found() {
    let rb = RpcBox::new();
    let d = success(&acp(&rb, "agent_request", "@new", &[]).run());
    assert_eq!(
        d["assistant_text"], "agent-request:-32601",
        "fs/read_text_file is answered -32601: {d}"
    );
    assert_eq!(d["stop_reason"], "end_turn", "{d}");
    assert_eq!(
        d["events"],
        json!([]),
        "only permission requests are events: {d}"
    );
    let answers: Vec<Value> = frames(&rb.recs(), "in")
        .into_iter()
        .filter(|f| f["id"] == 101)
        .collect();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0]["error"]["code"], -32601, "{}", answers[0]);
    assert!(answers[0].get("method").is_none() && answers[0].get("result").is_none());
    rb.assert_quiet();
}

// ---------------------------------------------------------------------------------------------
// AC-v2-302: SIGINT, SIGTERM and SIGHUP send `session/cancel`.
// ---------------------------------------------------------------------------------------------

/// Signal a turn under way (`cancel` scenario: omp answers `cancelled` once asked).
fn cancel_flow(sig: Signal) {
    let rb = RpcBox::new();
    let before = tree(&rb);
    let child = spawn_acp(&rb, "cancel", "@new", &[]);
    let pid = i64::from(child.id());
    rb.wait_log("the first agent chunk", first_chunk_logged);
    let t = Instant::now();
    signal(pid, sig);
    let seen = wait_for_cancel_frame(&rb, scaled(Duration::from_secs(10)))
        .unwrap_or_else(|| panic!("no session/cancel after {sig:?}\n{}", rb.diag()));
    let latency = seen.duration_since(t);
    assert!(
        latency < scaled(CANCEL_BOUND),
        "session/cancel took {latency:?} after {sig:?} (bound 250 ms)"
    );
    let out = finish(
        child,
        "__acp @new (signalled)",
        scaled(Duration::from_secs(30)),
    );
    assert!(
        t.elapsed() < scaled(Duration::from_secs(11)),
        "exit within 11 s of the signal, took {:?}",
        t.elapsed()
    );
    let env = ok_doc(&out, 130);
    let d = &env["data"];
    assert_eq!(d["task_state"], "canceled", "{sig:?}: {d}");
    assert_eq!(d["cancel"]["sent"], true, "{sig:?}: {d}");
    assert_eq!(d["cancel"]["confirmed"], true, "{sig:?}: {d}");
    assert_eq!(d["cancel"]["reason"], "interrupt", "{sig:?}: {d}");
    assert_eq!(d["stop_reason"], "cancelled", "{sig:?}: {d}");
    assert_eq!(d["session"]["id"], NEW_ID, "{sig:?}: {d}");
    assert_eq!(d["assistant_text"], "Hel", "the partial text survives: {d}");

    let recs = rb.recs();
    let cancels: Vec<Value> = frames(&recs, "in")
        .into_iter()
        .filter(|f| f["method"] == "session/cancel")
        .collect();
    assert_eq!(cancels.len(), 1, "exactly one session/cancel: {cancels:?}");
    assert!(
        cancels[0].get("id").is_none(),
        "session/cancel is a notification: {}",
        cancels[0]
    );
    assert_eq!(cancels[0]["jsonrpc"], "2.0");
    assert_eq!(cancels[0]["params"]["sessionId"], NEW_ID);
    assert!(
        !methods(&recs).iter().any(|m| m == "session/close"),
        "session/close is skipped after an interrupt: {:?}",
        methods(&recs)
    );
    assert_omp_gone(&rb);
    assert!(rb.glen_lock_free(NEW_ID), "the lock is released at exit");
    assert_no_durable_state(&rb);
    assert_untouched(&rb, &before);
    rb.assert_quiet();
}

#[test]
fn ac_v2_302_sigint_sends_session_cancel_and_exits_130_canceled() {
    cancel_flow(Signal::INT);
}

#[test]
fn ac_v2_302_sigterm_sends_session_cancel_and_exits_130_canceled() {
    cancel_flow(Signal::TERM);
}

#[test]
fn ac_v2_302_sighup_sends_session_cancel_and_exits_130_canceled() {
    cancel_flow(Signal::HUP);
}

#[test]
fn ac_v2_302_an_unconfirmed_cancel_still_exits_130_within_the_bound() {
    let rb = RpcBox::new();
    let child = spawn_acp(
        &rb,
        "cancel_ignored",
        A.handle,
        &[("GLEN_ACP_CANCEL_GRACE_MS", "300")],
    );
    let pid = i64::from(child.id());
    rb.wait_log("the first agent chunk", first_chunk_logged);
    let t = Instant::now();
    signal(pid, Signal::INT);
    let seen = wait_for_cancel_frame(&rb, scaled(Duration::from_secs(10)))
        .unwrap_or_else(|| panic!("no session/cancel\n{}", rb.diag()));
    assert!(seen.duration_since(t) < scaled(CANCEL_BOUND));
    let out = finish(
        child,
        "__acp A (cancel ignored)",
        scaled(Duration::from_secs(30)),
    );
    assert!(
        t.elapsed() < scaled(Duration::from_secs(9)),
        "an unconfirmed cancel exits within 8.3 s, took {:?}",
        t.elapsed()
    );
    let env = ok_doc(&out, 130);
    let d = &env["data"];
    assert_eq!(d["task_state"], "canceled", "{d}");
    assert_eq!(d["cancel"]["sent"], true, "{d}");
    assert_eq!(d["cancel"]["confirmed"], false, "{d}");
    assert_eq!(d["cancel"]["reason"], "interrupt", "{d}");
    assert!(d["stop_reason"].is_null(), "omp never answered: {d}");
    assert_eq!(d["session"]["id"], A.id, "{d}");
    assert_omp_gone(&rb);
    assert!(rb.glen_lock_free(A.id));
    rb.assert_quiet();
}

#[test]
fn ac_v2_302_a_signal_before_the_prompt_sends_no_cancel_and_exits_fast() {
    for sig in [Signal::INT, Signal::TERM] {
        let rb = RpcBox::new();
        let child = spawn_acp(&rb, "init_hang", "@new", &[]);
        let pid = i64::from(child.id());
        rb.wait_log("the initialize request", initialize_logged);
        let t = Instant::now();
        signal(pid, sig);
        let out = finish(
            child,
            "__acp @new (before the prompt)",
            scaled(Duration::from_secs(30)),
        );
        assert!(
            t.elapsed() < scaled(Duration::from_millis(3500)),
            "exit within 3.5 s of {sig:?} before the prompt, took {:?}",
            t.elapsed()
        );
        let env = ok_doc(&out, 130);
        let d = &env["data"];
        assert_eq!(d["task_state"], "canceled", "{sig:?}: {d}");
        assert_eq!(d["cancel"]["sent"], false, "{sig:?}: {d}");
        assert_eq!(d["cancel"]["confirmed"], false, "{sig:?}: {d}");
        assert_eq!(d["cancel"]["reason"], "interrupt", "{sig:?}: {d}");
        assert!(d["stop_reason"].is_null(), "{sig:?}: {d}");
        let m = methods(&rb.recs());
        assert_eq!(m, ["initialize"], "no cancel and no prompt: {m:?}");
        assert_omp_gone(&rb);
        assert_no_durable_state(&rb);
        rb.assert_quiet();
    }
}

// ---------------------------------------------------------------------------------------------
// AC-v2-302: failures after the `initialize` result.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac_v2_302_an_omp_that_dies_mid_turn_is_job_failed_acp_exited_with_the_partial_text() {
    let rb = RpcBox::new();
    let before = tree(&rb);
    let out = acp(&rb, "crash", "@new", &[]).run();
    let err = failed(&out, "job_failed");
    let det = &err["details"];
    assert_eq!(det["reason"], "acp_exited", "{err}");
    assert_eq!(det["assistant_text"], "Hel", "{err}");
    assert_eq!(det["session"]["id"], NEW_ID, "{err}");
    assert_omp_gone(&rb);
    assert!(rb.glen_lock_free(NEW_ID));
    assert_no_durable_state(&rb);
    assert_untouched(&rb, &before);
}

#[test]
fn ac_v2_302_a_json_rpc_error_on_load_or_prompt_is_job_failed_acp_rpc_error() {
    let rb = RpcBox::new();
    let before = tree(&rb);
    let log = rb.log_path("load");
    let out = acp(&rb, "rpc_error_load", A.handle, &[])
        .env("FAKE_OMP_LOG", &log)
        .run();
    let err = failed(&out, "job_failed");
    let det = &err["details"];
    assert_eq!(det["reason"], "acp_rpc_error", "{err}");
    assert_eq!(det["method"], "session/load", "{err}");
    assert_eq!(det["code"], -32603, "{err}");
    assert!(
        det["message"]
            .as_str()
            .is_some_and(|m| m.contains(&format!("ACP session not found: {}", A.id))),
        "omp's data.details wins for the message: {err}"
    );
    assert!(
        !methods(&records(&log))
            .iter()
            .any(|m| m == "session/prompt"),
        "no prompt after a failed load"
    );

    let out = acp(&rb, "rpc_error_prompt", "@new", &[]).run();
    let err = failed(&out, "job_failed");
    let det = &err["details"];
    assert_eq!(det["reason"], "acp_rpc_error", "{err}");
    assert_eq!(det["method"], "session/prompt", "{err}");
    assert_eq!(det["code"], -32603, "{err}");
    assert!(
        det["message"]
            .as_str()
            .is_some_and(|m| m.contains("ACP session not found")),
        "{err}"
    );
    assert_eq!(det["session"]["id"], NEW_ID, "{err}");
    assert_omp_gone(&rb);
    assert!(rb.glen_lock_free(A.id) && rb.glen_lock_free(NEW_ID));
    assert_no_durable_state(&rb);
    assert_untouched(&rb, &before);
}

#[test]
fn ac_v2_302_the_job_deadline_cancels_then_fails_acp_timeout() {
    let rb = RpcBox::new();
    let out = acp(&rb, "hang", A.handle, &[])
        .env("GLEN_JOB_TIMEOUT", "2s")
        .env("GLEN_ACP_CANCEL_GRACE_MS", "300")
        .run();
    let err = failed(&out, "job_failed");
    let det = &err["details"];
    assert_eq!(det["reason"], "acp_timeout", "{err}");
    assert_eq!(det["cancel"]["reason"], "deadline", "{err}");
    assert_eq!(det["cancel"]["sent"], true, "{err}");
    assert_eq!(det["assistant_text"], "Hel", "{err}");
    assert!(
        out.elapsed >= Duration::from_millis(1800),
        "the deadline is GLEN_JOB_TIMEOUT, took {:?}",
        out.elapsed
    );
    let m = methods(&rb.recs());
    let prompt = m
        .iter()
        .position(|x| x == "session/prompt")
        .expect("prompt");
    let cancel = m
        .iter()
        .position(|x| x == "session/cancel")
        .expect("cancel");
    assert!(prompt < cancel, "glen cancels the prompt it sent: {m:?}");
    assert!(!m.iter().any(|x| x == "session/close"), "{m:?}");
    assert_omp_gone(&rb);
    assert!(rb.glen_lock_free(A.id));
    assert_no_durable_state(&rb);
}

// ---------------------------------------------------------------------------------------------
// AC-v2-303: the glen lock, the endpoint and lease probes, no omp before a refusal.
// ---------------------------------------------------------------------------------------------

/// A refusal: exit 6 `no_route` with `reason`, no omp, no job, no `.jsonl` touched.
fn refused(rb: &RpcBox, out: &Out, reason: &str, before: &Tree) -> Value {
    let err = failed(out, "no_route");
    assert_eq!(err["details"]["reason"], reason, "{err}");
    assert!(
        err["hint"].as_str().is_some_and(|h| !h.trim().is_empty()),
        "a refusal carries a hint: {err}"
    );
    assert!(rb.starts().is_empty(), "omp was started\n{}", rb.diag());
    assert!(
        !rb.log.exists() || rb.recs().is_empty(),
        "the fake saw frames\n{}",
        rb.diag()
    );
    assert_no_durable_state(rb);
    assert_untouched(rb, before);
    err
}

#[test]
fn ac_v2_303_a_held_owner_lease_is_refused_live_owner_before_any_omp() {
    let rb = RpcBox::new();
    let before = tree(&rb);
    let _lease = hold_lease(&rb.bx.owners(), A.id);
    let out = acp(&rb, "ok", A.handle, &[]).run();
    refused(&rb, &out, "live_owner", &before);
    assert!(
        rb.glen_lock_free(A.id),
        "the lock is released after a refusal"
    );
}

#[test]
fn ac_v2_303_a_held_glen_lock_is_refused_rpc_job_running_before_any_omp() {
    let rb = RpcBox::new();
    let before = tree(&rb);
    let _held = rb.hold_glen_lock(A.id);
    let out = acp(&rb, "ok", A.handle, &[]).run();
    let err = refused(&rb, &out, "rpc_job_running", &before);
    assert!(
        err["hint"].as_str().unwrap_or("").contains("glen jobs"),
        "the hint says to wait for `glen jobs --active --session <handle>`: {err}"
    );
}

#[test]
fn ac_v2_303_a_live_endpoint_is_refused_live_owner_with_the_send_hint() {
    let lb = LiveBox::new();
    let before = tree(&lb);
    let file = s(&lb.bx.path(&A));
    let mut ext = lb.start_ext(A.id, "ok", &[], &["--session-file", &file]);
    let out = acp(&lb, "ok", A.handle, &[]).run();
    let err = refused(&lb, &out, "live_owner", &before);
    assert!(
        err["hint"].as_str().unwrap_or("").contains("glen send"),
        "the hint points at the live route: {err}"
    );
    lb.assert_no_omp();
    ext.stop();
}

#[test]
fn ac_v2_303_an_unhealthy_endpoint_is_refused_unhealthy_endpoint() {
    let lb = LiveBox::new();
    let before = tree(&lb);
    let file = s(&lb.bx.path(&A));
    let mut ext = lb.start_ext(A.id, "hang", &[], &["--session-file", &file]);
    let out = acp(&lb, "ok", A.handle, &[]).run();
    refused(&lb, &out, "unhealthy_endpoint", &before);
    lb.assert_no_omp();
    ext.stop();
}

#[test]
fn ac_v2_303_a_free_lease_and_no_endpoint_proceeds_and_the_lock_is_free_after() {
    let rb = RpcBox::new();
    let d = success(&acp(&rb, "ok", A.handle, &[]).run());
    assert_eq!(d["session"]["id"], A.id, "{d}");
    assert!(rb.glen_lock_free(A.id));
    rb.assert_quiet();
}

#[test]
fn ac_v2_303_the_session_lock_is_held_for_the_whole_turn_existing_and_new() {
    let rb = RpcBox::new();
    let before = tree(&rb);
    for session in [A.handle, "@new"] {
        let id = if session == "@new" { NEW_ID } else { A.id };
        let log = rb.log_path(&format!("lock-{id}"));
        let child = spawn_acp(
            &rb,
            "cancel",
            session,
            &[("FAKE_OMP_LOG", s(&log).as_str())],
        );
        let pid = i64::from(child.id());
        poll_until("the first agent chunk", scaled(LONG), || {
            first_chunk_logged(&records(&log)).then_some(())
        });
        assert!(
            !rb.glen_lock_free(id),
            "{session}: the glen lock is held mid-turn"
        );
        if session == A.handle {
            // A second __acp on the same session waits for no one: it is refused.
            let second = rb.log_path("second");
            let out = acp(&rb, "ok", A.handle, &[])
                .env("FAKE_OMP_LOG", &second)
                .run();
            let err = failed(&out, "no_route");
            assert_eq!(err["details"]["reason"], "rpc_job_running", "{err}");
            assert!(records(&second).is_empty(), "the refused run started omp");
        }
        signal(pid, Signal::INT);
        let out = finish(child, "__acp (lock held)", scaled(Duration::from_secs(30)));
        ok_doc(&out, 130);
        assert!(
            rb.glen_lock_free(id),
            "{session}: the lock is free after exit"
        );
    }
    assert_no_durable_state(&rb);
    assert_untouched(&rb, &before);
    rb.assert_quiet();
}

#[test]
fn ac_v2_303_a_new_sessions_revealed_id_that_is_already_locked_never_prompts() {
    let rb = RpcBox::new();
    let before = tree(&rb);
    let _held = rb.hold_glen_lock(NEW_ID);
    let out = acp(&rb, "ok", "@new", &[]).run();
    assert_eq!(out.code(), 1, "an io-class internal error\n{}", out.ctx());
    let env = out.json();
    assert_eq!(env["ok"], false, "{env}");
    assert!(
        ["io", "internal"].contains(&env["error"]["code"].as_str().unwrap_or("")),
        "{env}"
    );
    assert_ne!(env["error"]["details"]["reason"], "acp_rpc_error", "{env}");
    let m = methods(&rb.recs());
    assert!(!m.iter().any(|x| x == "session/prompt"), "{m:?}");
    assert_omp_gone(&rb);
    assert_no_durable_state(&rb);
    assert_untouched(&rb, &before);
}

// ---------------------------------------------------------------------------------------------
// AC-v2-304: the experiments' record (static, over the docs).
// ---------------------------------------------------------------------------------------------

fn design() -> String {
    read("docs/design.md")
}

/// Body lines of design section `## §A<n> …` (up to the next `## ` heading), fenced code dropped.
fn section(doc: &str, n: u32) -> Vec<String> {
    let mut in_fence = false;
    let mut lines = Vec::new();
    for l in doc.lines() {
        if l.trim_start().starts_with("```") {
            in_fence = !in_fence;
        } else if !in_fence {
            lines.push(l.to_owned());
        }
    }
    let start = lines
        .iter()
        .position(|l| l.starts_with(&format!("## §A{n} ")))
        .unwrap_or_else(|| panic!("docs/design.md has no §A{n}"));
    lines[start + 1..]
        .iter()
        .take_while(|l| !l.starts_with("## "))
        .cloned()
        .collect()
}

/// Whole-word match of `word` in `line` (letters, digits and `_`, `-` count as word characters).
fn has_word(line: &str, word: &str) -> bool {
    line.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .any(|w| w == word)
}

#[test]
fn ac_v2_304_design_a23_records_the_experiments_the_verdict_and_the_reply_loss() {
    let a23 = section(&design(), 23);
    let text = a23.join("\n");
    for e in ["E1", "E2", "E3"] {
        assert!(a23.iter().any(|l| has_word(l, e)), "§A23 does not name {e}");
    }
    let verdict = a23.iter().find(|l| {
        let low = l.to_lowercase();
        (low.contains("result") || low.contains("verdict"))
            && ["GO", "NO-GO", "inconclusive"]
                .iter()
                .any(|w| has_word(l, w))
    });
    let verdict =
        verdict.unwrap_or_else(|| panic!("§A23 states no GO / NO-GO / inconclusive verdict"));
    if has_word(verdict, "inconclusive") {
        assert!(
            text.to_lowercase().contains("blocked"),
            "an inconclusive verdict records AC-v2-304 as blocked"
        );
    }
    for g in ["G1", "G2", "G3"] {
        assert!(
            a23.iter().any(|l| has_word(l, g)),
            "§A23 lacks the pre-registered rule's {g}"
        );
    }
    assert!(
        a23.iter()
            .any(|l| l.contains("glen_reply") && l.to_lowercase().contains("attribution")),
        "§A23 names the loss of glen_reply attribution under ACP"
    );
    assert!(
        text.contains("input_required"),
        "§A23 says where input_required appears"
    );
    assert!(
        text.contains("W5") && text.to_lowercase().contains("mcp"),
        "§A23 records the W5 MCP carry-over"
    );
}

#[test]
fn ac_v2_304_design_a2_has_an_acp_row_naming_protocol_version() {
    let a2 = section(&design(), 2);
    assert!(
        a2.iter().any(|l| l.trim_start().starts_with('|')
            && l.contains("ACP")
            && l.contains("PROTOCOL_VERSION")),
        "§A2 has no ACP table row naming PROTOCOL_VERSION"
    );
}

#[test]
fn ac_v2_304_design_a9_lists_acp_as_an_internal_command() {
    let a9 = section(&design(), 9);
    assert!(
        a9.iter()
            .any(|l| l.contains("Internal") && l.contains("__acp")),
        "§A9's Internal bullet does not list __acp"
    );
}

#[test]
fn ac_v2_304_design_a15_documents_every_fake_acp_scenario_and_knob_the_suite_uses() {
    let a15 = section(&design(), 15).join("\n");
    for needle in [
        "fake_omp acp",
        "FAKE_OMP_ACP",
        "FAKE_OMP_ACP_SESSION",
        "FAKE_OMP_ACP_REPLAY",
        "FAKE_OMP_ACP_OPTIONS",
        "FAKE_OMP_ACP_STOP",
        "FAKE_OMP_STARTUP_STDERR",
        "FAKE_OMP_VERSION",
        "GLEN_ACP_SETUP_MS",
        "GLEN_ACP_CANCEL_GRACE_MS",
    ] {
        assert!(a15.contains(needle), "§A15 does not document `{needle}`");
    }
    for scenario in [
        "ok",
        "proto2",
        "proto_missing",
        "init_error",
        "no_load",
        "init_hang",
        "startup_exit",
        "noise",
        "rpc_error_load",
        "rpc_error_prompt",
        "permission",
        "agent_request",
        "cancel",
        "cancel_ignored",
        "hang",
        "crash",
        "stop",
    ] {
        assert!(
            a15.contains(&format!("`{scenario}`")),
            "§A15 does not document the `{scenario}` scenario"
        );
    }
}

#[test]
fn ac_v2_304_the_w3_wave_bullet_names_acp_and_the_changelog_mentions_the_command() {
    let a16 = section(&design(), 16);
    assert!(
        a16.iter().any(|l| l.contains("W3") && l.contains("ACP")),
        "§A16's W3 wave bullet no longer names ACP"
    );
    let log = read("CHANGELOG.md");
    let cut = log
        .find("\n## v0.1.1")
        .expect("CHANGELOG.md has a `## v0.1.1` section");
    assert!(
        log[..cut].contains("__acp"),
        "the unreleased CHANGELOG section does not mention __acp"
    );
}

#[test]
fn ac_v2_304_the_conformance_module_doc_maps_w3_to_this_module() {
    let src = read("tests/conformance.rs");
    let doc: Vec<&str> = src.lines().take_while(|l| l.starts_with("//!")).collect();
    let line = doc
        .iter()
        .find(|l| l.contains("- v0.2 W3"))
        .expect("the module doc has a `v0.2 W3` line");
    assert!(line.contains("AC-v2-3"), "{line}");
    assert!(line.contains("v2_acp"), "{line}");
    assert!(!line.trim_end().ends_with("planned."), "{line}");
    for l in &doc {
        assert!(
            l.chars().count() <= 100,
            "module-doc line over 100 columns: {l}"
        );
    }
    assert!(
        src.contains("mod v2_acp;"),
        "tests/conformance.rs does not register v2_acp"
    );
}

#[test]
fn ac_v2_304_no_acp_crate_joined_the_lockfile() {
    let lock = read("Cargo.lock");
    assert!(
        !lock.contains("name = \"agent-client-protocol\""),
        "the spike is hand-rolled; the agent-client-protocol crate is rejected in the ADR"
    );
}
