//! bwrap-based simulation of the Linux Codex sandbox: a read-only root plus, for the
//! network-isolation scenarios, a separate network namespace (`--unshare-net`). Proves the
//! EROFS/sandboxed mapping (item 5/9) and the Linux lease rule (item 18, AC-1028: any Codex
//! marker hides the lease) against a real kernel, never the actual Codex sandbox. Run only by the `linux-sandbox` CI
//! job (`cargo test --locked --test linux_sandbox -- --ignored --test-threads=1`); every test
//! is `#[ignore]` so a normal `cargo test` never needs bwrap.
#![cfg(target_os = "linux")]
mod common;

use common::{A_ID, LiveBox, RpcBox};
use std::{
    process::{Command, Output, Stdio},
    sync::{OnceLock, mpsc},
    time::Duration,
};

/// `bwrap --ro-bind / / --dev /dev --proc /proc --die-with-parent [--unshare-net] [--bind W W]…
/// -- <program> <args>`. Carries the harness command's own program/args/envs (via
/// `env_clear()` + `get_envs()`), plus `PATH=/usr/bin:/bin` so `bash`/coreutils resolve. No
/// `--unshare-pid`: glen's live-endpoint liveness check uses the host pid
/// (`src/live/registry.rs`), which a pid namespace would hide; `bwrap_pid` adds it for
/// scenarios that never reach a live endpoint.
fn bwrap(cmd: &Command, net: bool, writable: &[&std::path::Path]) -> Output {
    bwrap_pid(cmd, net, false, writable)
}
/// `bwrap`, plus `--unshare-pid` when `pid` (Codex's own bwrap shape, where PID 1 is bwrap).
fn bwrap_pid(cmd: &Command, net: bool, pid: bool, writable: &[&std::path::Path]) -> Output {
    preflight();
    let mut c = Command::new("bwrap");
    c.arg("--ro-bind")
        .arg("/")
        .arg("/")
        .arg("--dev")
        .arg("/dev")
        .arg("--proc")
        .arg("/proc")
        .arg("--ro-bind")
        .arg("/sys")
        .arg("/sys")
        .arg("--die-with-parent");
    if !net {
        c.arg("--unshare-net");
    }
    if pid {
        c.arg("--unshare-pid");
    }
    for w in writable {
        c.arg("--bind").arg(w).arg(w);
    }
    c.arg("--").arg(cmd.get_program());
    c.args(cmd.get_args());
    c.env_clear();
    for (k, v) in cmd.get_envs() {
        if let Some(v) = v {
            c.env(k, v);
        }
    }
    c.env("PATH", "/usr/bin:/bin");
    if let Some(dir) = cmd.get_current_dir() {
        c.current_dir(dir);
    }
    run_with_timeout(c, Duration::from_secs(30))
}
/// `bwrap --ro-bind / / --dev /dev --proc /proc --unshare-net -- <program>` (the raw shape
/// used by scenarios that run something other than `glen`, e.g. s10's bash probe).
fn bwrap_raw(c: Command, net: bool, writable: &[&std::path::Path]) -> Output {
    preflight();
    let mut wrapped = Command::new("bwrap");
    wrapped
        .arg("--ro-bind")
        .arg("/")
        .arg("/")
        .arg("--dev")
        .arg("/dev")
        .arg("--proc")
        .arg("/proc")
        .arg("--ro-bind")
        .arg("/sys")
        .arg("/sys")
        .arg("--die-with-parent");
    if !net {
        wrapped.arg("--unshare-net");
    }
    for w in writable {
        wrapped.arg("--bind").arg(w).arg(w);
    }
    wrapped.arg("--").arg(c.get_program());
    wrapped.args(c.get_args());
    wrapped.env_clear();
    for (k, v) in c.get_envs() {
        if let Some(v) = v {
            wrapped.env(k, v);
        }
    }
    wrapped.env("PATH", "/usr/bin:/bin");
    run_with_timeout(wrapped, Duration::from_secs(30))
}
fn run_with_timeout(mut c: Command, timeout: Duration) -> Output {
    let child = c
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot spawn bwrap: {e}"));
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => panic!("bwrap command failed to run: {e}"),
        Err(_) => {
            if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            }
            panic!("bwrap command did not finish within {timeout:?}");
        }
    }
}
/// Fails loudly (never skips) when bwrap cannot start at all.
fn preflight() {
    static CHECKED: OnceLock<()> = OnceLock::new();
    CHECKED.get_or_init(|| {
        let out = Command::new("bwrap")
            .arg("--ro-bind")
            .arg("/")
            .arg("/")
            .arg("true")
            .output()
            .unwrap_or_else(|e| panic!("cannot run bwrap: {e}"));
        assert!(
            out.status.success(),
            "bwrap --ro-bind / / true failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    });
}
fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON: {e}; stdout {:?} stderr {:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}
fn exit_code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}
/// Append one unread inbox reply for `session_id`, same shape as `tests/inbox.rs`'s `record`.
fn seed_inbox(rb: &RpcBox, session_id: &str, text: &str) {
    use std::io::Write as _;
    let path = rb
        .omp
        .glen_home
        .join("inbox")
        .join(format!("{session_id}.ndjson"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let record = serde_json::json!({
        "v": 1,
        "id": format!("r-{text}"),
        "session_id": session_id,
        "job_id": null,
        "request_id": null,
        "tool_call_id": null,
        "text": text,
        "final": true,
        "at": "2026-10-30T01:00:00Z",
    });
    writeln!(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap(),
        "{record}"
    )
    .unwrap();
}

#[test]
#[ignore = "needs bubblewrap; run by the linux-sandbox CI job"]
fn s1_erofs_spill() {
    let rb = RpcBox::new();
    let mut c = rb.glen(&["ls", "--all", "--json"]);
    c.env("CODEX_SANDBOX_NETWORK_DISABLED", "1")
        .env("GLEN_MAX_OUTPUT", "1");
    // No writable bind: GLEN_HOME lives under the ro-bound root, so any write is EROFS.
    let out = bwrap(&c, true, &[]);
    let v = json(&out);
    assert_eq!(exit_code(&out), 6, "{v}");
    assert_eq!(v["error"]["code"], "no_route", "{v}");
    assert_eq!(v["error"]["details"]["reason"], "sandboxed", "{v}");
    assert_eq!(v["error"]["details"]["glen_home_writable"], false, "{v}");
}
#[test]
#[ignore = "needs bubblewrap; run by the linux-sandbox CI job"]
fn s2_erofs_without_markers() {
    let rb = RpcBox::new();
    let mut c = rb.glen(&["ls", "--all", "--json"]);
    c.env("GLEN_MAX_OUTPUT", "1");
    let out = bwrap(&c, true, &[]);
    let v = json(&out);
    assert_eq!(exit_code(&out), 1, "{v}: the accepted gap (item 9)");
    assert_eq!(v["error"]["code"], "io", "{v}");
}
#[test]
#[ignore = "needs bubblewrap; run by the linux-sandbox CI job"]
fn s3_live_send_readonly_home() {
    let mut lb = LiveBox::new();
    lb.start_ext(A_ID, "ok", &[]);
    let mut c = lb
        .rpc
        .glen(&["send", "e27c0de", "hi", "--wait", "20s", "--json"]);
    c.env("CODEX_SANDBOX", "linux-ci");
    let out = bwrap(&c, true, &[]);
    let v = json(&out);
    assert_eq!(exit_code(&out), 6, "{v}");
    assert_eq!(v["error"]["details"]["reason"], "sandboxed", "{v}");
    assert_eq!(v["error"]["details"]["glen_home_writable"], false, "{v}");
}
#[test]
#[ignore = "needs bubblewrap; run by the linux-sandbox CI job"]
fn s4_live_send_writable_home() {
    let mut lb = LiveBox::new();
    lb.start_ext(A_ID, "ok", &[]);
    let mut c = lb
        .rpc
        .glen(&["send", "e27c0de", "hi", "--wait", "20s", "--json"]);
    c.env("CODEX_SANDBOX", "linux-ci");
    // LiveBox keeps GLEN_HOME under its own short "/tmp/gle-*" tree (needed for the abstract
    // socket / endpoint path length limit), not under `rpc.omp.tmp`.
    let writable = lb.rpc.omp.glen_home.parent().unwrap().to_path_buf();
    let out = bwrap(&c, true, &[&writable]);
    let v = json(&out);
    assert_eq!(exit_code(&out), 0, "{v}");
    assert_eq!(v["data"]["job"]["route"], "live", "{v}");
    assert_eq!(v["data"]["assistant_text"], "pong: hi", "{v}");
}
#[test]
#[ignore = "needs bubblewrap; run by the linux-sandbox CI job"]
fn s5_network_off() {
    let rb = RpcBox::new();
    let mut c = rb.glen(&["send", "e27c0de", "hi", "--json"]);
    c.env("CODEX_SANDBOX", "linux-ci")
        .env("CODEX_SANDBOX_NETWORK_DISABLED", "1");
    let writable = rb.omp.tmp.path().to_path_buf();
    let out = bwrap(&c, false, &[&writable]);
    let v = json(&out);
    assert_eq!(exit_code(&out), 6, "{v}");
    assert_eq!(v["error"]["details"]["reason"], "sandboxed", "{v}");
    assert_eq!(v["error"]["details"]["network"], false, "{v}");
    assert!(
        !rb.job_dir("ignored").parent().unwrap().exists(),
        "no jobs/ entry"
    );
}
#[test]
#[ignore = "needs bubblewrap; run by the linux-sandbox CI job"]
fn s6_reads_and_inbox() {
    let rb = RpcBox::new();
    seed_inbox(&rb, A_ID, "ping");
    // AC-1024: the read verbs succeed with markers and nothing writable, not even GLEN_HOME.
    for args in [
        vec!["ls", "--json"],
        vec!["show", "e27c0de", "--json"],
        vec!["tail", "e27c0de", "-n", "5", "--json"],
        vec!["inbox", "--peek", "--json"],
    ] {
        let mut c = rb.glen(&args);
        c.env("CODEX_SANDBOX", "linux-ci")
            .env("CODEX_SANDBOX_NETWORK_DISABLED", "1");
        let out = bwrap(&c, true, &[]);
        assert_eq!(exit_code(&out), 0, "{args:?}: {}", json(&out));
    }
    // AC-1021 (W10 fix cycle 2 decision 3): marking a message read needs a GLEN_HOME write, which
    // glen probes before it prints, so a read-only GLEN_HOME reports the sandboxed error envelope
    // instead of an optimistic `marked_read`.
    let mut c = rb.glen(&["inbox", "--json"]);
    c.env("CODEX_SANDBOX", "linux-ci")
        .env("CODEX_SANDBOX_NETWORK_DISABLED", "1");
    let out = bwrap(&c, true, &[]);
    let v = json(&out);
    assert_eq!(exit_code(&out), 6, "{v}");
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["error"]["code"], "no_route", "{v}");
    assert_eq!(v["error"]["details"]["reason"], "sandboxed", "{v}");
    assert_eq!(v["error"]["details"]["glen_home_writable"], false, "{v}");
}
#[test]
#[ignore = "needs bubblewrap; run by the linux-sandbox CI job"]
fn s7_lease_across_netns() {
    let rb = RpcBox::new();
    let mut held = rb.hold_lease(A_ID);
    let state = |out: &Output| {
        let v = json(out);
        v["data"]["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| {
                s["handle"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("e27c0de")
            })
            .unwrap_or_else(|| panic!("row missing: {v}"))["state"]
            .clone()
    };
    let outside = rb.omp.glen(&["ls", "--all", "--json"], &rb.omp.alpha());
    assert_eq!(state(&outside), "live", "outside the sandbox");
    let writable = rb.omp.tmp.path().to_path_buf();
    // The kernel fact behind AC-1028 (AC-1024): without a marker, glen trusts its probe, and
    // inside --unshare-net the host-namespace lease is invisible, so the session reads idle.
    let inside = bwrap(&rb.glen(&["ls", "--all", "--json"]), false, &[&writable]);
    assert_eq!(
        state(&inside),
        "idle",
        "the lease is hidden by --unshare-net"
    );
    // The display rule: under a marker, the lease is unknown whatever the probe says.
    let mut c = rb.glen(&["ls", "--all", "--json"]);
    c.env("CODEX_SANDBOX_NETWORK_DISABLED", "1");
    let inside = bwrap(&c, false, &[&writable]);
    assert_eq!(
        state(&inside),
        "unknown",
        "a marker displays the lease as unknown"
    );
    let _ = held.kill();
    let _ = held.wait();
}
#[test]
#[ignore = "needs bubblewrap; run by the linux-sandbox CI job"]
fn s8_hidden_lease_refuses_rpc() {
    // Glen's netns-only shape, Codex's own shape (--unshare-pid, so PID 1 is bwrap in the
    // same netns), and a held glen job lock (W10 fix cycle 2 decisions 1 and 2).
    for (pid, glen_lock) in [(false, false), (true, false), (false, true)] {
        let case = format!("unshare-pid={pid} glen-lock={glen_lock}");
        let rb = RpcBox::new();
        let mut held = rb.hold_lease(A_ID);
        let lock = glen_lock.then(|| rb.hold_glen_lock(A_ID));
        let mut c = rb.glen(&["send", "e27c0de", "hi", "--json"]);
        c.env("CODEX_SANDBOX_NETWORK_DISABLED", "1");
        let writable = rb.omp.tmp.path().to_path_buf();
        let out = bwrap_pid(&c, false, pid, &[&writable]);
        let v = json(&out);
        assert_eq!(exit_code(&out), 6, "{case}: {v}");
        assert_eq!(v["error"]["details"]["reason"], "sandboxed", "{case}: {v}");
        assert_eq!(v["error"]["details"]["lease"], "unknown", "{case}: {v}");
        assert!(
            !rb.job_dir("ignored").parent().unwrap().exists(),
            "{case}: no jobs/ entry"
        );
        assert!(
            rb.fake_log().is_empty(),
            "{case}: fake omp was never started: {:?}",
            rb.fake_log()
        );
        drop(lock);
        let _ = held.kill();
        let _ = held.wait();
    }
}
#[test]
#[ignore = "needs bubblewrap; run by the linux-sandbox CI job"]
fn s9_live_route_across_netns() {
    let mut lb = LiveBox::new();
    lb.start_ext(A_ID, "ok", &[]);
    let mut c = lb
        .rpc
        .glen(&["send", "e27c0de", "hi", "--wait", "20s", "--json"]);
    c.env("CODEX_SANDBOX_NETWORK_DISABLED", "1");
    let writable = lb.rpc.omp.glen_home.parent().unwrap().to_path_buf();
    let out = bwrap(&c, false, &[&writable]);
    let v = json(&out);
    assert_eq!(exit_code(&out), 0, "{v}");
    assert_eq!(v["data"]["job"]["route"], "live", "{v}");
    assert_eq!(v["data"]["assistant_text"], "pong: hi", "{v}");
}
#[test]
#[ignore = "needs bubblewrap; run by the linux-sandbox CI job"]
fn s10_network_is_isolated() {
    use std::net::TcpListener;
    // Never accepted: a connect completes from the backlog, and the listener stays bound until
    // both probes and the re-check are done (the release gate is the `drop` at the end).
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let probe = format!("exec 3<>/dev/tcp/127.0.0.1/{port}");
    let reach_outside = |when: &str| {
        let outside = Command::new("bash").arg("-c").arg(&probe).output().unwrap();
        assert!(
            outside.status.success(),
            "{when}: the host can reach its own listener: {}",
            String::from_utf8_lossy(&outside.stderr)
        );
    };
    reach_outside("before");
    let mut inside_cmd = Command::new("bash");
    inside_cmd.arg("-c").arg(&probe);
    let inside = bwrap_raw(inside_cmd, false, &[]);
    assert!(
        !inside.status.success(),
        "a TCP connect inside --unshare-net must fail: {inside:?}"
    );
    reach_outside("after");
    drop(listener);
}
