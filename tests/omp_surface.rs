//! The omp surface contract (AC-v2-105): what glen needs from omp's CLI and RPC, asserted once
//! and run against `fake_omp` in CI and against a real omp when `GLEN_OMP_SURFACE_BIN` names
//! one (the nightly `omp-latest` workflow and the local evidence run).
//!
//! A real omp always runs with `env_clear()`, a fresh `tempfile` HOME holding only the loopback
//! stub provider (`tests/fixtures/omp-surface/models.yml`) and that HOME as its cwd, so the
//! owner's `~/.omp` and the repo's project config are unreachable. No prompt, steer, follow-up
//! or model change is ever sent.
mod common;
use common::{fake_omp_bin, fixture};
use omp_rpc::{
    Client, ClientOptions, GetAvailableModelsCommand, GetAvailableThinkingLevelsCommand,
    GetStateCommand, HostTool, SetEventFilterCommand,
};
use serde_json::{Map, json};
use std::{
    io::Read,
    path::Path,
    process::{Child, ChildStderr, Command, Stdio},
    thread::JoinHandle,
    time::{Duration, Instant},
};
const FLAGS: [&str; 8] = [
    "--mode",
    "--no-ui",
    "--no-session",
    "--resume",
    "--profile",
    "--thinking",
    "--extension",
    "--no-extensions",
];
/// What one surface run saw, for the nightly log.
struct Seen {
    version: String,
    protocol: u32,
    models: usize,
}
fn command(bin: &Path, home: &Path, env: &[(&str, &str)]) -> Command {
    let mut c = Command::new(bin);
    c.env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .envs(env.iter().copied())
        .current_dir(home);
    c
}
/// `--flag` appears as a word of the help text, alone or as `--flag=<value>`.
fn lists_flag(help: &str, flag: &str) -> bool {
    help.split(|c: char| c.is_whitespace() || matches!(c, ',' | '[' | ']'))
        .any(|word| word.split('=').next() == Some(flag))
}
/// glen's `glen_reply` definition, rebuilt here because `rpc::reply::glen_reply_tool` is
/// crate-private. The handler is never called: no prompt is sent.
fn glen_reply() -> HostTool {
    let schema = Map::from_iter([
        ("type".into(), json!("object")),
        (
            "properties".into(),
            json!({"text":{"type":"string","description":"The message for the calling agent."},"final":{"type":"boolean","description":"true when this is your final answer to the request."}}),
        ),
        ("required".into(), json!(["text"])),
        ("additionalProperties".into(), json!(false)),
    ]);
    HostTool::new(
        "glen_reply",
        "Send a message back to the agent that contacted you through glen.",
        schema,
        |_, _| Err("the surface contract never prompts".into()),
    )
}
fn kill_group(pid: u32) {
    if let Some(pid) = rustix::process::Pid::from_raw(pid as i32)
        && let Err(e) = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL)
        && e != rustix::io::Errno::SRCH
    {
        eprintln!("omp_surface: cannot kill omp group {pid:?}: {e}");
    }
}
fn group_gone(pid: u32) -> bool {
    rustix::process::Pid::from_raw(pid as i32).is_none_or(|group| {
        rustix::process::test_kill_process_group(group) == Err(rustix::io::Errno::SRCH)
    })
}
/// Waits up to `grace` for the leader to exit and its process group to empty, then SIGKILLs
/// what is left of the group, so no descendant outlives the run or holds omp's stderr. Returns
/// whether the leader exited by itself within `grace`.
fn end_group(child: &mut Child, grace: Duration) -> bool {
    let pid = child.id();
    let end = Instant::now() + grace;
    let mut exited = false;
    loop {
        // Reaping the leader first keeps a zombie from counting as a live member.
        exited = exited || child.try_wait().unwrap().is_some();
        if exited && group_gone(pid) {
            return true;
        }
        if Instant::now() >= end {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    kill_group(pid);
    let _ = child.wait();
    let end = Instant::now() + Duration::from_secs(5);
    while !group_gone(pid) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    exited
}
fn read_stderr(mut stderr: ChildStderr) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    })
}
/// omp's stderr, or a note when something outside the group still holds the pipe at `timeout`.
fn join_stderr(reader: JoinHandle<String>, timeout: Duration) -> String {
    let end = Instant::now() + timeout;
    while !reader.is_finished() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    if reader.is_finished() {
        reader.join().unwrap_or_default()
    } else {
        format!("<omp stderr still open {timeout:?} after teardown>")
    }
}
fn assert_surface(bin: &Path, home: &Path, env: &[(&str, &str)], real: bool) -> Seen {
    let who = if real { "real omp" } else { "fake_omp" };
    // (1) `--version` normalizes through glen's own parser.
    let out = command(bin, home, env).arg("--version").output().unwrap();
    assert!(out.status.success(), "{who} --version: {out:?}");
    let line = String::from_utf8(out.stdout).unwrap();
    let line = line.lines().next().unwrap_or("");
    let version = glen::version::parse_omp_line(line)
        .unwrap_or_else(|| panic!("{who} --version is not parseable: {line:?}"))
        .text;
    // (2) `--help` lists every flag glen passes.
    let out = command(bin, home, env).arg("--help").output().unwrap();
    assert!(out.status.success(), "{who} --help: {out:?}");
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in FLAGS {
        assert!(
            lists_flag(&help, flag),
            "{who} --help lacks {flag}:\n{help}"
        );
    }
    // (3) RPC mode in its own process group, the way glen starts it.
    let mut c = command(bin, home, env);
    c.args(["--mode", "rpc", "--no-ui", "--no-session"])
        .env("GLEN_MANAGED", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    std::os::unix::process::CommandExt::process_group(&mut c, 0);
    let mut child = c.spawn().unwrap();
    let stderr = read_stderr(child.stderr.take().unwrap());
    let connected = Client::from_io(
        child.stdout.take().unwrap(),
        child.stdin.take().unwrap(),
        ClientOptions {
            tools: vec![],
            uris: vec![],
            ready_timeout: Duration::from_secs(30),
            default_timeout: Duration::from_secs(10),
        },
    );
    let (client, _events) = match connected {
        Ok(c) => c,
        Err(e) => {
            end_group(&mut child, Duration::ZERO);
            panic!(
                "{who} did not reach ready: {e}\nstderr: {}",
                join_stderr(stderr, Duration::from_secs(5))
            );
        }
    };
    let checks = (|| -> Result<(u32, usize), String> {
        // (4) Protocol 1 or 2, and 2 whenever ready advertises v2 with the standard limits.
        let protocol = client.protocol_version();
        let ready = client.ready();
        let v2 = ready
            .supported_protocol_versions
            .as_ref()
            .is_some_and(|v| v.contains(&2))
            && ready.max_frame_bytes == Some(1_048_576)
            && ready.max_reassembled_frame_bytes == Some(67_108_864);
        if !matches!(protocol, 1 | 2) || (v2 && protocol != 2) {
            return Err(format!("protocol {protocol} for ready {ready:?}"));
        }
        // (5) glen_reply registers.
        let tools = client
            .set_custom_tools(vec![glen_reply()])
            .map_err(|e| format!("set_host_tools: {e}"))?;
        if !tools.iter().any(|t| t == "glen_reply") {
            return Err(format!("glen_reply not registered: {tools:?}"));
        }
        // (6) The worker's event allowlist is accepted.
        client
            .call(&SetEventFilterCommand {
                events: Some(
                    glen::rpc::EVENT_ALLOWLIST
                        .iter()
                        .map(|s| (*s).into())
                        .collect(),
                ),
            })
            .map_err(|e| format!("set_event_filter: {e}"))?;
        // (7) get_state decodes into the pinned SessionState.
        let state = client
            .call(&GetStateCommand {})
            .map_err(|e| format!("get_state: {e}"))?;
        if state.session_id.is_empty() || state.is_streaming {
            return Err(format!("unexpected state: {state:?}"));
        }
        // (8) Models and thinking levels decode.
        let models = client
            .call(&GetAvailableModelsCommand {})
            .map_err(|e| format!("get_available_models: {e}"))?;
        if models.is_empty() {
            return Err("get_available_models returned no model".into());
        }
        client
            .call(&GetAvailableThinkingLevelsCommand {})
            .map_err(|e| format!("get_available_thinking_levels: {e}"))?;
        Ok((protocol, models.len()))
    })();
    // (9) Closing stdin ends omp within 10 s; otherwise the test fails. Either way whatever is
    // left of the group is killed and the stderr join is bounded.
    let closed = client.close();
    let exited = end_group(&mut child, Duration::from_secs(10));
    let stderr = join_stderr(stderr, Duration::from_secs(5));
    let (protocol, models) = checks.unwrap_or_else(|e| panic!("{who}: {e}\nstderr: {stderr}"));
    assert!(closed.is_ok(), "{who}: close: {closed:?}");
    assert!(
        exited,
        "{who} did not exit within 10 s of stdin EOF\nstderr: {stderr}"
    );
    Seen {
        version,
        protocol,
        models,
    }
}
#[test]
fn lists_flag_matches_words_only() {
    let help = "  --mode=<value>  Output\n  -r, --resume=<value>\n  --no-session  x\n  [--profile]";
    for flag in ["--mode", "--resume", "--no-session", "--profile"] {
        assert!(lists_flag(help, flag), "{flag}");
    }
    for flag in ["--no-ui", "--mod", "--session"] {
        assert!(!lists_flag(help, flag), "{flag}");
    }
}
#[test]
fn teardown_kills_descendants_and_bounds_the_stderr_join() {
    for (case, script, leader_exits) in [
        // The leader exits at once; a descendant keeps the inherited stderr pipe open.
        ("inherited pipe", "sleep 30 & exit 0", true),
        // The descendant closed stderr, so the reader ends while the group lives on.
        ("closed pipe", "sleep 30 2>&- & exit 0", true),
        // A wedged leader that ignores stdin EOF.
        ("wedged leader", "exec sleep 30", false),
    ] {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", script])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        let mut child = c.spawn().unwrap();
        let pid = child.id();
        let reader = read_stderr(child.stderr.take().unwrap());
        let start = Instant::now();
        assert_eq!(
            end_group(&mut child, Duration::from_millis(500)),
            leader_exits,
            "{case}"
        );
        assert!(group_gone(pid), "{case}: the group survived teardown");
        assert_eq!(join_stderr(reader, Duration::from_secs(2)), "", "{case}");
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_secs(5), "{case}: {elapsed:?}");
    }
}
#[test]
fn surface_against_fake_omp() {
    for (env, protocol) in [(vec![], 1), (vec![("FAKE_OMP_V2", "1")], 2)] {
        let home = tempfile::tempdir().unwrap();
        let seen = assert_surface(&fake_omp_bin(), home.path(), &env, false);
        assert_eq!(seen.version, "18.14.0");
        assert_eq!(seen.protocol, protocol, "{env:?}");
        assert!(seen.models >= 1);
    }
}
#[test]
fn surface_against_real_omp() {
    let Some(bin) = std::env::var_os("GLEN_OMP_SURFACE_BIN") else {
        eprintln!("omp_surface: GLEN_OMP_SURFACE_BIN is unset; skipping the real-omp run");
        return;
    };
    let bin = Path::new(&bin);
    assert!(
        bin.is_absolute(),
        "GLEN_OMP_SURFACE_BIN must be an absolute path: {}",
        bin.display()
    );
    let meta = std::fs::metadata(bin)
        .unwrap_or_else(|e| panic!("GLEN_OMP_SURFACE_BIN {}: {e}", bin.display()));
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(
            meta.is_file() && meta.permissions().mode() & 0o111 != 0,
            "GLEN_OMP_SURFACE_BIN is not an executable file: {}",
            bin.display()
        );
    }
    // Never a home from the environment: a fresh scratch HOME with only the stub provider.
    let home = tempfile::tempdir().unwrap();
    let agent = home.path().join(".omp/agent");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::copy(fixture("omp-surface/models.yml"), agent.join("models.yml")).unwrap();
    let seen = assert_surface(bin, home.path(), &[], true);
    println!(
        "omp_surface: version={} protocol={} models={}",
        seen.version, seen.protocol, seen.models
    );
}
