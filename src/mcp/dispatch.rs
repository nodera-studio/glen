//! `tools/call`: argument validation, argv, the exec-self runner and the result mapping
//! (design §A22). The child is glen itself (no shell), so a result's text is the CLI's own
//! envelope; only a call glen could not run to an envelope gets a synthesized one.
use super::{
    CALL_TICK, CANCEL_GRACE, MAX_IN_FLIGHT, MAX_OUTPUT,
    catalog::{ArgKind, ArgSpec, Tool},
};
use crate::{
    error::{ErrorCode, GlenError},
    output::envelope::{error_value, to_json_vec},
};
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    ffi::OsString,
    io::{self, Read},
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// The program a tool call runs: glen itself in production, a script in unit tests.
#[derive(Clone, Debug)]
pub struct Launcher {
    pub program: PathBuf,
    pub prefix_args: Vec<OsString>,
}

/// How much of a child's stderr an `mcp_exec` envelope keeps (its end).
const STDERR_TAIL: usize = 512;
/// How long a finished child's pipes may stay open (a grandchild holding them) before the
/// result is built from what was read.
const READER_JOIN: Duration = Duration::from_secs(1);

/// An argument error: the message names the property.
#[derive(Debug, PartialEq)]
pub(crate) struct ArgError {
    pub message: String,
    pub argument: Option<String>,
}

/// Checks `arguments` against the tool's catalog entry and returns the supplied properties in
/// catalog order. Absent or `null` arguments, and `null` for an optional property, are unset.
pub(crate) fn validate<'t>(
    tool: &'t Tool,
    arguments: Option<&Value>,
) -> Result<Vec<(&'t ArgSpec, Value)>, ArgError> {
    let fail = |argument: Option<&str>, message: String| ArgError {
        message: format!("{}: {message}", tool.name),
        argument: argument.map(str::to_owned),
    };
    let empty = Map::new();
    let given = match arguments {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(m)) => m,
        Some(_) => return Err(fail(None, "arguments must be an object".into())),
    };
    if let Some(key) = given
        .keys()
        .find(|k| !tool.exec.args.iter().any(|a| a.name == **k))
    {
        return Err(fail(Some(key), format!("unknown argument `{key}`")));
    }
    let required = tool.input_schema["required"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut supplied = vec![];
    for spec in &tool.exec.args {
        let name = spec.name.as_str();
        let value = match given.get(name) {
            None | Some(Value::Null) => {
                if required.iter().any(|r| r == name) {
                    return Err(fail(Some(name), format!("`{name}` is required")));
                }
                continue;
            }
            Some(v) => v,
        };
        let schema = &tool.input_schema["properties"][name];
        let ok = match schema["type"].as_str() {
            Some("boolean") => value.is_boolean(),
            Some("integer") if schema.get("minimum").is_some() => value.as_u64().is_some(),
            Some("integer") => value.as_i64().is_some(),
            _ => value.is_string(),
        };
        if !ok {
            let want = match schema["type"].as_str() {
                Some("boolean") => "a boolean",
                Some("integer") if schema.get("minimum").is_some() => "a non-negative integer",
                Some("integer") => "an integer",
                _ => "a string",
            };
            return Err(fail(Some(name), format!("`{name}` must be {want}")));
        }
        if let Some(s) = value.as_str() {
            if s.contains('\0') {
                return Err(fail(
                    Some(name),
                    format!("`{name}` contains a NUL character"),
                ));
            }
            if let Some(allowed) = schema["enum"].as_array()
                && !allowed.iter().any(|a| a == s)
            {
                let names: Vec<&str> = allowed.iter().filter_map(Value::as_str).collect();
                return Err(fail(
                    Some(name),
                    format!("`{name}` must be one of {}", names.join(", ")),
                ));
            }
        }
        supplied.push((spec, value.clone()));
    }
    Ok(supplied)
}

/// argv after the program: `--json`, `--profile=<p>`, the subcommand, options and flags in
/// catalog order (`--long=value`, `--long`), `--`, then positionals in position order. A
/// `false` flag is omitted; the child never sees a default the CLI would apply itself.
pub(crate) fn argv(
    tool: &Tool,
    supplied: &[(&ArgSpec, Value)],
    profile: Option<&str>,
) -> Vec<OsString> {
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let mut argv: Vec<OsString> = vec!["--json".into()];
    if let Some(p) = profile {
        argv.push(format!("--profile={p}").into());
    }
    argv.push(tool.exec.command.clone().into());
    let mut positionals = vec![];
    for (spec, value) in supplied {
        let long = spec.long.as_deref().unwrap_or(&spec.name);
        match spec.kind {
            ArgKind::Flag if value == &Value::Bool(true) => argv.push(format!("--{long}").into()),
            ArgKind::Flag => {}
            ArgKind::Option => argv.push(format!("--{long}={}", text(value)).into()),
            ArgKind::Positional => {
                positionals.push((spec.position.unwrap_or(usize::MAX), text(value)))
            }
        }
    }
    positionals.sort_by_key(|(position, _)| *position);
    argv.push("--".into());
    argv.extend(positionals.into_iter().map(|(_, v)| v.into()));
    argv
}

/// What a child that was not cancelled produced. A cancelled run has no `Run` (`run_child`
/// returns `None`), so it can never become a response.
#[derive(Debug)]
pub(crate) enum Run {
    Exited {
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// stdout passed `MAX_OUTPUT`: the child was killed.
    Overflow {
        stderr: Vec<u8>,
    },
    NotStarted(io::Error),
    Lost(io::Error),
}

/// The in-flight `tools/call` registry (design §A22 "Cancellation"). Every transition happens
/// under its one lock, which is never held across a stdout write, a spawn or a wait; the call
/// thread reaps its child only under the lock, so no signal ever reaches a reaped pid.
#[derive(Default)]
pub(crate) struct Registry(Mutex<Calls>);
#[derive(Default)]
struct Calls {
    map: HashMap<String, CallState>,
    /// The server is exiting: a cancelled child is SIGKILLed at once, without `CANCEL_GRACE`.
    closing: bool,
}
#[derive(Default)]
struct CallState {
    cancelled: bool,
    /// `finish` decided its response (being queued, or none when cancelled): a cancel comes too
    /// late, and the slot is held until `release`, so a host that stops reading cannot pile up
    /// more than `MAX_IN_FLIGHT` answers.
    responded: bool,
    child_pid: Option<u32>,
    sigint_at: Option<Instant>,
    killed: bool,
}
/// Why a call could not be registered.
#[derive(Debug, PartialEq)]
pub(crate) enum Refused {
    /// That request id is already in flight.
    Duplicate,
    /// `MAX_IN_FLIGHT` calls are running.
    Full,
}
fn signal(pid: u32, signal: rustix::process::Signal) -> bool {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|pid| rustix::process::kill_process(pid, signal).is_ok())
}
impl CallState {
    /// Signals the stored child of a cancelled call: SIGKILL at once while the server exits,
    /// else the first SIGINT. Called under the registry lock only. Returns whether it signalled.
    fn stop(&mut self, closing: bool) -> bool {
        let Some(pid) = self.child_pid else {
            return false;
        };
        if closing && !self.killed {
            self.killed = true;
            return signal(pid, rustix::process::Signal::KILL);
        }
        if !closing && self.sigint_at.is_none() {
            self.sigint_at = Some(Instant::now());
            return signal(pid, rustix::process::Signal::INT);
        }
        false
    }
}
impl Registry {
    fn lock(&self) -> std::sync::MutexGuard<'_, Calls> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
    pub(crate) fn insert(&self, key: &str) -> Result<(), Refused> {
        let mut calls = self.lock();
        if calls.map.contains_key(key) {
            return Err(Refused::Duplicate);
        }
        if calls.map.len() >= MAX_IN_FLIGHT {
            return Err(Refused::Full);
        }
        calls.map.insert(key.to_owned(), CallState::default());
        Ok(())
    }
    /// (1) A call cancelled before its child exists never starts one.
    fn may_start(&self, key: &str) -> bool {
        self.lock().map.get(key).is_some_and(|s| !s.cancelled)
    }
    /// (2) Records the child; a cancel that came in meanwhile gets its signal now. Returns
    /// whether a signal was sent.
    pub(crate) fn started(&self, key: &str, pid: u32) -> bool {
        let mut calls = self.lock();
        let closing = calls.closing;
        let Some(s) = calls.map.get_mut(key) else {
            return false;
        };
        s.child_pid = Some(pid);
        s.cancelled && s.stop(closing)
    }
    /// (3) `notifications/cancelled`: marks the call and SIGINTs its child when it has one.
    /// Ignored for an id that is not in flight. Returns whether a signal was sent.
    pub(crate) fn cancel(&self, key: &str) -> bool {
        let mut calls = self.lock();
        let closing = calls.closing;
        let Some(s) = calls.map.get_mut(key).filter(|s| !s.responded) else {
            return false;
        };
        s.cancelled = true;
        s.stop(closing)
    }
    /// Shutdown: the server is exiting, so every call is cancelled and its child SIGKILLed at
    /// once (no `CANCEL_GRACE`); a child stored later is killed by `started`.
    pub(crate) fn shutdown(&self) {
        let mut calls = self.lock();
        calls.closing = true;
        for s in calls.map.values_mut().filter(|s| !s.responded) {
            s.cancelled = true;
            s.stop(true);
        }
    }
    /// (4) and (5): one poll of the call's child. SIGKILL once `CANCEL_GRACE` has passed since
    /// the SIGINT (at once while closing); the exit is observed and the pid cleared in one
    /// critical section.
    fn poll(&self, key: &str, child: &mut Child) -> io::Result<Option<ExitStatus>> {
        let mut calls = self.lock();
        let closing = calls.closing;
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(e) => {
                // The caller kills and reaps it outside the lock: no signal may target it now.
                if let Some(s) = calls.map.get_mut(key) {
                    s.child_pid = None;
                }
                return Err(e);
            }
        };
        if let Some(s) = calls.map.get_mut(key) {
            if status.is_some() {
                s.child_pid = None;
            } else if s.cancelled
                && !s.killed
                && (closing || s.sigint_at.is_some_and(|t| t.elapsed() >= CANCEL_GRACE))
            {
                // Best effort: an error means the child is already gone.
                let _ = child.kill();
                s.killed = true;
            }
        }
        Ok(status)
    }
    fn cancelled(&self, key: &str) -> bool {
        self.lock().map.get(key).is_some_and(|s| s.cancelled)
    }
    /// (6) Decides the call's response: `true` when it may be written (it was not cancelled);
    /// from then on a cancel or a shutdown ignores that id. The entry stays, cancelled or not,
    /// until the call's one `release`: were it removed here, a request reusing the id could be
    /// registered before that `release`, which would then remove the newer call's entry.
    pub(crate) fn finish(&self, key: &str) -> bool {
        self.lock().map.get_mut(key).is_some_and(|s| {
            s.responded = true;
            !s.cancelled
        })
    }
    /// Frees the call's slot once its response is queued (or it has none). The only removal.
    pub(crate) fn release(&self, key: &str) {
        self.lock().map.remove(key);
    }
    pub(crate) fn is_in_flight(&self, key: &str) -> bool {
        self.lock().map.contains_key(key)
    }
}

/// Who may stop a child.
#[derive(Clone, Copy)]
pub(crate) enum Watch<'a> {
    /// A `tools/call`: its registry entry decides.
    Call(&'a Registry, &'a str),
    /// A helper child (the channel's inbox poll): SIGKILLed past `deadline` or once `stop`
    /// says so (the notifier's stop flag, or a signal to the server during the baseline).
    Bounded {
        deadline: Instant,
        stop: &'a (dyn Fn() -> bool + Sync),
    },
}

#[derive(Default)]
struct Capture {
    bytes: Mutex<Vec<u8>>,
    done: AtomicBool,
    overflow: AtomicBool,
}
impl Capture {
    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.bytes.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Reads `pipe` on its own thread: the first `limit` bytes, or with `tail` only the last
/// `limit` bytes. Past `limit` (without `tail`) it flags an overflow and closes the pipe, so a
/// writer that outlives the kill (a grandchild holding the pipe) meets EPIPE instead of a
/// reader that drains it forever.
fn capture(pipe: Option<impl Read + Send + 'static>, limit: usize, tail: bool) -> Arc<Capture> {
    let cap = Arc::new(Capture::default());
    let Some(mut pipe) = pipe else {
        cap.done.store(true, Ordering::SeqCst);
        return cap;
    };
    let shared = cap.clone();
    let spawned = thread::Builder::new()
        .name("glen-mcp-pipe".into())
        .spawn(move || {
            let mut chunk = vec![0u8; 64 * 1024];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut bytes = shared.bytes.lock().unwrap_or_else(PoisonError::into_inner);
                        if tail {
                            bytes.extend_from_slice(&chunk[..n]);
                            let excess = bytes.len().saturating_sub(limit);
                            bytes.drain(..excess);
                        } else if bytes.len() + n <= limit {
                            bytes.extend_from_slice(&chunk[..n]);
                        } else {
                            shared.overflow.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            shared.done.store(true, Ordering::SeqCst);
        });
    if spawned.is_err() {
        cap.done.store(true, Ordering::SeqCst);
    }
    cap
}

fn spawn(launcher: &Launcher, argv: &[OsString], env: &[(&str, &str)]) -> io::Result<Child> {
    Command::new(&launcher.program)
        .args(&launcher.prefix_args)
        .args(argv)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

/// Runs one child to its end: stdin `/dev/null`, stdout and stderr captured, `env` added to the
/// inherited environment (that child only), cwd inherited. `None` when the call was cancelled
/// or the helper stopped: never started, or signalled and reaped. Such a run is never answered.
pub(crate) fn run_child(
    launcher: &Launcher,
    argv: &[OsString],
    env: &[(&str, &str)],
    watch: Watch,
) -> Option<Run> {
    if let Watch::Call(registry, key) = watch
        && !registry.may_start(key)
    {
        return None;
    }
    let child = match spawn(launcher, argv, env) {
        Ok(child) => child,
        Err(e) => return Some(Run::NotStarted(e)),
    };
    if let Watch::Call(registry, key) = watch {
        registry.started(key, child.id());
    }
    supervise(child, watch)
}

/// Polls a started child every `CALL_TICK` until it exits: cancellation escalates to SIGKILL
/// after `CANCEL_GRACE`, a bounded helper is killed at its bound, and a stdout flood past
/// `MAX_OUTPUT` is killed at once. `None` for a cancelled call or a stopped helper.
pub(crate) fn supervise(mut child: Child, watch: Watch) -> Option<Run> {
    let out = capture(child.stdout.take(), MAX_OUTPUT, false);
    let err = capture(child.stderr.take(), STDERR_TAIL, true);
    let (mut killed, mut stopped) = (false, false);
    let status = loop {
        let polled = match watch {
            Watch::Call(registry, key) => registry.poll(key, &mut child),
            Watch::Bounded { .. } => child.try_wait(),
        };
        match polled {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                // Best effort: the child is unobservable, so stop it before reporting.
                let _ = child.kill();
                let _ = child.wait();
                return Some(Run::Lost(e));
            }
        }
        if !killed && out.overflow.load(Ordering::SeqCst) {
            // std never signals a reaped child, so this kill cannot reach a recycled pid.
            let _ = child.kill();
            killed = true;
        }
        if let Watch::Bounded { deadline, stop } = watch
            && !stopped
            && (Instant::now() >= deadline || stop())
        {
            let _ = child.kill();
            stopped = true;
        }
        thread::sleep(CALL_TICK);
    };
    // A cancelled call is not answered, so its pipe readers are not waited for.
    let cancelled = match watch {
        Watch::Call(registry, key) => registry.cancelled(key),
        Watch::Bounded { .. } => stopped,
    };
    if cancelled {
        return None;
    }
    let deadline = Instant::now() + READER_JOIN;
    while !(out.done.load(Ordering::SeqCst) && err.done.load(Ordering::SeqCst))
        && Instant::now() < deadline
    {
        thread::sleep(CALL_TICK);
    }
    if killed || out.overflow.load(Ordering::SeqCst) {
        return Some(Run::Overflow { stderr: err.take() });
    }
    Some(Run::Exited {
        status,
        stdout: out.take(),
        stderr: err.take(),
    })
}

/// The `tools/call` result for a finished run (design §A22 "Result mapping").
pub(crate) fn result(tool: &Tool, run: Run) -> Value {
    let name = tool.name.as_str();
    let (status, stdout, stderr) = match run {
        Run::Exited {
            status,
            stdout,
            stderr,
        } => (status, stdout, stderr),
        Run::Overflow { stderr } => {
            return exec_failure(
                name,
                "glen printed more than the 16 MiB a tool result may hold",
                &stderr,
            );
        }
        Run::NotStarted(e) if e.raw_os_error() == Some(libc::E2BIG) => {
            return exec_failure(
                name,
                "argument too long to pass to glen (a message over about 100 KiB cannot be sent through MCP)",
                b"",
            );
        }
        Run::NotStarted(e) => return exec_failure(name, &format!("cannot start glen: {e}"), b""),
        Run::Lost(e) => return exec_failure(name, &format!("cannot wait for glen: {e}"), b""),
    };
    if let Some(signal) = status.signal() {
        return exec_failure(
            name,
            &format!("glen was killed by signal {signal}"),
            &stderr,
        );
    }
    let Ok(text) = String::from_utf8(stdout) else {
        return exec_failure(name, "glen printed output that is not UTF-8", &stderr);
    };
    let envelope = serde_json::from_str::<Value>(&text)
        .ok()
        .filter(|v| v.get("ok").is_some_and(Value::is_boolean));
    let Some(envelope) = envelope else {
        let code = status.code().map_or_else(|| "?".into(), |c| c.to_string());
        return exec_failure(
            name,
            &format!("glen exited {code} without a JSON envelope"),
            &stderr,
        );
    };
    let is_error = envelope["ok"] != Value::Bool(true);
    let mut result = json!({"content":[{"type":"text","text":text}],"isError":is_error});
    if tool.output_schema.is_some() {
        result["structuredContent"] = envelope;
    }
    result
}

/// A `tools/call` result whose text is `envelope` rendered as the CLI renders it on a pipe.
fn synthesized(envelope: &Value) -> Value {
    let mut text = to_json_vec(envelope, false).unwrap_or_default();
    text.push(b'\n');
    json!({
        "content":[{"type":"text","text":String::from_utf8_lossy(&text)}],
        "isError":true,
    })
}

/// The `usage` / `mcp_arguments` result: no child started.
pub(crate) fn argument_failure(tool: &str, e: &ArgError) -> Value {
    let mut details = json!({"reason":"mcp_arguments","tool":tool});
    if let Some(argument) = &e.argument {
        details["argument"] = json!(argument);
    }
    synthesized(&error_value(
        &GlenError::new(ErrorCode::Usage, e.message.clone()).with_details(details),
        &[],
    ))
}

/// The `internal` / `mcp_exec` result for a call glen could not run to an envelope.
pub(crate) fn exec_failure(tool: &str, message: &str, stderr: &[u8]) -> Value {
    let mut details = json!({"reason":"mcp_exec","tool":tool});
    let stderr = String::from_utf8_lossy(stderr);
    if !stderr.trim().is_empty() {
        details["stderr"] = json!(stderr.trim_end());
    }
    synthesized(&error_value(
        &GlenError::new(ErrorCode::Internal, message).with_details(details),
        &[],
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::mcp::{catalog, test_support::scaled};

    pub(crate) fn tool(name: &str) -> Tool {
        catalog::load()
            .unwrap()
            .tools
            .into_iter()
            .find(|t| t.name == name)
            .unwrap()
    }
    pub(crate) fn sh(script: &str) -> Launcher {
        Launcher {
            program: "/bin/sh".into(),
            prefix_args: vec!["-c".into(), script.into(), "glen".into()],
        }
    }
    fn args(v: &[OsString]) -> Vec<&str> {
        v.iter().map(|s| s.to_str().unwrap()).collect()
    }
    fn built(name: &str, arguments: Value, profile: Option<&str>) -> Vec<String> {
        let t = tool(name);
        let supplied = validate(&t, Some(&arguments)).unwrap();
        args(&argv(&t, &supplied, profile))
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn argv_table() {
        for (name, arguments, profile, want) in [
            ("glen_jobs", json!({}), None, vec!["--json", "jobs", "--"]),
            (
                "glen_jobs",
                json!({"active":true,"limit":0}),
                None,
                vec!["--json", "jobs", "--active", "--limit=0", "--"],
            ),
            (
                "glen_jobs",
                json!({"active":false,"session":null}),
                Some("p"),
                vec!["--json", "--profile=p", "jobs", "--"],
            ),
            (
                "glen_send",
                json!({"message":"-v is broken; rm -rf x $(touch y) `id`","session":"e27c0de","wait":"30s","as":"steer"}),
                None,
                vec![
                    "--json",
                    "send",
                    "--as=steer",
                    "--wait=30s",
                    "--",
                    "e27c0de",
                    "-v is broken; rm -rf x $(touch y) `id`",
                ],
            ),
            (
                "glen_ls",
                json!({"limit":20,"cwd":"/a b"}),
                None,
                vec!["--json", "ls", "--cwd=/a b", "--limit=20", "--"],
            ),
            (
                "glen_models",
                json!({}),
                None,
                vec!["--json", "models", "--"],
            ),
            (
                "glen_model",
                json!({"model":"p/m","session":"s"}),
                None,
                vec!["--json", "model", "--", "s", "p/m"],
            ),
            (
                "glen_send",
                json!({"session":"s","message":"-","no_wait":true}),
                None,
                vec!["--json", "send", "--no-wait", "--", "s", "-"],
            ),
        ] {
            assert_eq!(built(name, arguments, profile), want, "{name}");
        }
    }

    #[test]
    fn validation_table() {
        let send = tool("glen_send");
        let err = |arguments: Value| validate(&send, Some(&arguments)).unwrap_err();
        for (arguments, argument, needle) in [
            (
                json!({"session":"s","message":"m","bogus":1}),
                Some("bogus"),
                "unknown argument `bogus`",
            ),
            (
                json!({"session":"s","message":7}),
                Some("message"),
                "`message` must be a string",
            ),
            (
                json!({"session":"s","message":"m","no_wait":"yes"}),
                Some("no_wait"),
                "must be a boolean",
            ),
            (
                json!({"session":"s"}),
                Some("message"),
                "`message` is required",
            ),
            (
                json!({"session":"s","message":null}),
                Some("message"),
                "`message` is required",
            ),
            (
                json!({"session":"s","message":"m","as":"shout"}),
                Some("as"),
                "must be one of auto, steer, follow-up",
            ),
            (
                json!({"session":"s","message":"a\u{0}b"}),
                Some("message"),
                "NUL",
            ),
            (json!([1]), None, "arguments must be an object"),
        ] {
            let e = err(arguments);
            assert_eq!(e.argument.as_deref(), argument);
            assert!(
                e.message.starts_with("glen_send: ") && e.message.contains(needle),
                "{e:?}"
            );
        }
        let jobs = tool("glen_jobs");
        for (arguments, needle) in [
            (json!({"limit":-1}), "non-negative integer"),
            (json!({"limit":1.5}), "non-negative integer"),
            (json!({"limit":"5"}), "non-negative integer"),
        ] {
            assert!(
                validate(&jobs, Some(&arguments))
                    .unwrap_err()
                    .message
                    .contains(needle)
            );
        }
        assert!(validate(&jobs, None).unwrap().is_empty());
        assert!(validate(&jobs, Some(&Value::Null)).unwrap().is_empty());
        assert!(
            validate(&jobs, Some(&json!({"session":null,"limit":null})))
                .unwrap()
                .is_empty()
        );
        assert!(
            validate(
                &tool("glen_tail"),
                Some(&json!({"session":"s","follow":true}))
            )
            .is_err()
        );
        let usage = argument_failure("glen_send", &err(json!({"session":"s"})));
        assert_eq!(usage["isError"], true);
        let text = usage["content"][0]["text"].as_str().unwrap();
        assert!(text.ends_with("}\n"));
        let envelope: Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            envelope,
            json!({"v":1,"ok":false,"error":{"code":"usage","message":"glen_send: `message` is required",
                "details":{"reason":"mcp_arguments","tool":"glen_send","argument":"message"}}})
        );
    }

    fn mapped(script: &str) -> Value {
        let t = tool("glen_jobs");
        result(&t, run_free(&sh(script), &[], &[]))
    }
    fn envelope(result: &Value) -> Value {
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    #[test]
    fn result_mapping() {
        let running = r#"printf '{"v":1,"ok":true,"data":{"status":"running"}}\n'; exit 10"#;
        let r = mapped(running);
        assert_eq!(
            r,
            json!({"content":[{"type":"text","text":"{\"v\":1,\"ok\":true,\"data\":{\"status\":\"running\"}}\n"}],"isError":false})
        );
        let failed =
            mapped(r#"printf '{"v":1,"ok":false,"error":{"code":"job_failed"}}\n'; exit 7"#);
        assert_eq!(failed["isError"], true);
        assert!(failed.get("structuredContent").is_none());
        for (script, needle) in [
            (
                "echo not json; echo oops >&2; exit 3",
                "glen exited 3 without a JSON envelope",
            ),
            ("exit 0", "glen exited 0 without a JSON envelope"),
            (
                r#"printf '{"v":1,"ok":"yes"}\n'"#,
                "without a JSON envelope",
            ),
            (
                r#"printf '{"v":1,"ok":true}\n{"v":1,"ok":true}\n'"#,
                "without a JSON envelope",
            ),
            (r#"printf '\377\376\n'"#, "not UTF-8"),
            ("kill -9 $$", "killed by signal 9"),
        ] {
            let r = mapped(script);
            assert_eq!(r["isError"], true, "{script}");
            let e = envelope(&r);
            assert_eq!(e["error"]["code"], "internal", "{script}");
            assert_eq!(e["error"]["details"]["reason"], "mcp_exec", "{script}");
            assert_eq!(e["error"]["details"]["tool"], "glen_jobs", "{script}");
            let message = e["error"]["message"].as_str().unwrap();
            assert!(message.contains(needle), "{script}: {message}");
        }
        let e = envelope(&mapped("echo junk; echo why it failed >&2; exit 1"));
        assert_eq!(e["error"]["details"]["stderr"], "why it failed");
        let missing = Launcher {
            program: "/nonexistent/glen".into(),
            prefix_args: vec![],
        };
        let e = envelope(&result(&tool("glen_jobs"), run_free(&missing, &[], &[])));
        assert!(
            e["error"]["message"]
                .as_str()
                .unwrap()
                .starts_with("cannot start glen")
        );
        // Extra environment reaches the child only.
        let r = mapped_env(
            r#"printf '{"ok":true,"x":"%s"}\n' "$GLEN_MCP_T""#,
            &[("GLEN_MCP_T", "1")],
        );
        assert_eq!(envelope(&r)["x"], "1");
    }
    fn mapped_env(script: &str, env: &[(&str, &str)]) -> Value {
        result(&tool("glen_jobs"), run_free(&sh(script), &[], env))
    }

    #[test]
    fn a_stdout_flood_is_killed_and_reported() {
        let start = Instant::now();
        let e = envelope(&mapped("exec yes"));
        assert!(
            start.elapsed() < scaled(Duration::from_secs(10)),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(e["error"]["details"]["reason"], "mcp_exec");
        assert!(e["error"]["message"].as_str().unwrap().contains("16 MiB"));
    }

    #[test]
    fn an_overflow_drops_the_pipe_so_a_flooding_grandchild_ends() {
        /// Kills the grandchild if the test fails before it is seen gone. Seeing it gone disarms
        /// the guard: its pid may be reused from then on.
        struct Reap(Option<u32>);
        impl Reap {
            fn gone(&mut self) -> bool {
                if self.0.is_some_and(gone) {
                    self.0 = None;
                }
                self.0.is_none()
            }
        }
        impl Drop for Reap {
            fn drop(&mut self) {
                if let Some(pid) = self
                    .0
                    .and_then(|p| rustix::process::Pid::from_raw(p as i32))
                {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("yes");
        // The child is killed at the cap, but `yes` (its child) still holds the pipe.
        let e = envelope(&mapped(&format!(
            "yes & echo $! > '{}'; wait",
            pidfile.display()
        )));
        assert!(e["error"]["message"].as_str().unwrap().contains("16 MiB"));
        let mut flood = Reap(Some(wait_file(&pidfile).trim().parse().unwrap()));
        // Once the reader dropped the pipe, `yes` meets EPIPE and its SIGPIPE ends it.
        let start = Instant::now();
        while !flood.gone() {
            assert!(
                start.elapsed() < scaled(Duration::from_secs(5)),
                "the reader kept draining a flood past the cap"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn an_oversize_argument_is_an_exec_failure() {
        // Linux refuses one argument over 128 KiB and macOS a whole argv over 1 MiB.
        let big = OsString::from("x".repeat(2 * 1024 * 1024));
        let e = envelope(&result(
            &tool("glen_send"),
            run_free(&sh("true"), &[big], &[]),
        ));
        assert_eq!(e["error"]["details"]["reason"], "mcp_exec");
        let message = e["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("argument too long to pass to glen"),
            "{message}"
        );
    }

    #[test]
    fn output_schema_adds_structured_content() {
        let mut t = tool("glen_jobs");
        t.output_schema = Some(json!({"type":"object"}));
        let r = result(
            &t,
            run_free(&sh(r#"printf '{"v":1,"ok":true,"data":{}}\n'"#), &[], &[]),
        );
        assert_eq!(r["structuredContent"], json!({"v":1,"ok":true,"data":{}}));
    }

    /// A run under a throwaway registry entry that nobody cancels.
    pub(crate) fn run_free(launcher: &Launcher, argv: &[OsString], env: &[(&str, &str)]) -> Run {
        let registry = Registry::default();
        registry.insert("free").unwrap();
        run_child(launcher, argv, env, Watch::Call(&registry, "free")).expect("nobody cancels it")
    }
    fn gone(pid: u32) -> bool {
        let pid = rustix::process::Pid::from_raw(pid as i32).unwrap();
        rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH)
    }
    /// Reaps `child` without the registry, so no poll can signal it, and returns how it ended.
    fn reaped(child: &mut Child) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            assert!(
                start.elapsed() < scaled(Duration::from_secs(5)),
                "still running"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn wait_file(path: &std::path::Path) -> String {
        let start = Instant::now();
        loop {
            if let Ok(s) = std::fs::read_to_string(path)
                && s.ends_with('\n')
            {
                return s;
            }
            assert!(
                start.elapsed() < scaled(Duration::from_secs(5)),
                "{}",
                path.display()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_cancel_before_spawn_starts_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started");
        let registry = Registry::default();
        registry.insert("k").unwrap();
        assert!(!registry.cancel("k"), "no child yet, so no signal");
        let script = format!("touch '{}'", marker.display());
        let run = run_child(&sh(&script), &[], &[], Watch::Call(&registry, "k"));
        assert!(run.is_none(), "{run:?}");
        assert!(!marker.exists());
        assert!(!registry.finish("k"), "a cancelled call is not answered");
    }

    /// Cancels a running child (`trap` decides whether it ignores SIGINT) and returns how long it
    /// took from the cancel to the reaped child.
    fn cancel_running(ignore_sigint: bool) -> Duration {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let trap = if ignore_sigint { "trap '' INT; " } else { "" };
        let script = format!("{trap}echo $$ > '{}'; exec sleep 30", pidfile.display());
        let registry = Arc::new(Registry::default());
        registry.insert("k").unwrap();
        let thread = {
            let registry = registry.clone();
            thread::spawn(move || run_child(&sh(&script), &[], &[], Watch::Call(&registry, "k")))
        };
        let pid: u32 = wait_file(&pidfile).trim().parse().unwrap();
        let start = Instant::now();
        // Whether the pid is stored yet or not, the SIGINT goes out now (by this cancel or by
        // the call thread right after its pid store).
        registry.cancel("k");
        assert!(!registry.cancel("k"), "a second cancel sends nothing");
        let run = thread.join().unwrap();
        let took = start.elapsed();
        assert!(run.is_none(), "{run:?}");
        assert!(gone(pid));
        assert!(!registry.finish("k"));
        took
    }

    #[test]
    fn a_cancelled_child_is_gone_within_a_second() {
        let took = cancel_running(false);
        assert!(took < scaled(Duration::from_secs(1)), "{took:?}");
        let took = cancel_running(true);
        assert!(
            took >= CANCEL_GRACE && took < scaled(Duration::from_secs(1)),
            "{took:?}"
        );
    }

    #[test]
    fn a_cancel_between_spawn_and_the_pid_store_still_escalates() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let script = format!("trap '' INT; echo > '{}'; exec sleep 30", ready.display());
        let registry = Registry::default();
        registry.insert("k").unwrap();
        let child = spawn(&sh(&script), &[], &[]).unwrap();
        let pid = child.id();
        wait_file(&ready);
        let start = Instant::now();
        assert!(!registry.cancel("k"), "the pid is not stored yet");
        assert!(
            registry.started("k", pid),
            "the call thread sends the SIGINT itself"
        );
        let run = supervise(child, Watch::Call(&registry, "k"));
        let took = start.elapsed();
        assert!(run.is_none(), "{run:?}");
        assert!(
            took >= CANCEL_GRACE && took < scaled(Duration::from_secs(1)),
            "{took:?}"
        );
        assert!(gone(pid));
    }

    #[test]
    fn an_ignored_sigint_escalates_to_sigkill_after_the_grace() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        // Ready only once SIGINT is ignored, whatever the disposition the child inherited.
        let script = format!("trap '' INT; echo > '{}'; exec sleep 30", ready.display());
        let registry = Registry::default();
        registry.insert("k").unwrap();
        let mut child = spawn(&sh(&script), &[], &[]).unwrap();
        assert!(!registry.started("k", child.id()));
        wait_file(&ready);
        let start = Instant::now();
        assert!(registry.cancel("k"), "the SIGINT goes out");
        let status = loop {
            if let Some(status) = registry.poll("k", &mut child).unwrap() {
                break status;
            }
            assert!(
                start.elapsed() < scaled(Duration::from_secs(5)),
                "still running"
            );
            thread::sleep(CALL_TICK);
        };
        let took = start.elapsed();
        assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
        assert!(took >= CANCEL_GRACE, "{took:?}");
    }

    #[test]
    fn shutdown_kills_at_once_even_a_child_stored_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let registry = Registry::default();
        registry.insert("a").unwrap();
        registry.insert("b").unwrap();
        let ignoring = format!("trap '' INT; echo > '{}'; exec sleep 30", ready.display());
        let mut a = spawn(&sh(&ignoring), &[], &[]).unwrap();
        let (pid_a, mut b) = (
            a.id(),
            spawn(&sh("trap '' INT; exec sleep 30"), &[], &[]).unwrap(),
        );
        let pid_b = b.id();
        assert!(!registry.started("a", pid_a));
        wait_file(&ready);
        let start = Instant::now();
        registry.shutdown();
        assert!(
            registry.started("b", pid_b),
            "a child stored later is killed on store"
        );
        // Nothing polls them yet, and only a poll applies `CANCEL_GRACE`: `shutdown` and
        // `started` SIGKILLed them themselves, with no grace.
        for child in [&mut a, &mut b] {
            assert_eq!(reaped(child).signal(), Some(libc::SIGKILL));
        }
        // The call threads then see that exit and answer nothing.
        assert!(supervise(a, Watch::Call(&registry, "a")).is_none());
        assert!(supervise(b, Watch::Call(&registry, "b")).is_none());
        let took = start.elapsed();
        assert!(took < scaled(Duration::from_secs(2)), "{took:?}");
        assert!(gone(pid_a) && gone(pid_b));
        assert!(!registry.finish("a") && !registry.finish("b"));
    }

    #[test]
    fn a_cancel_after_the_exit_was_observed_sends_no_signal() {
        let registry = Registry::default();
        registry.insert("k").unwrap();
        let mut child = spawn(&sh("exit 0"), &[], &[]).unwrap();
        assert!(!registry.started("k", child.id()));
        let start = Instant::now();
        while registry.poll("k", &mut child).unwrap().is_none() {
            assert!(start.elapsed() < scaled(Duration::from_secs(5)));
            thread::sleep(CALL_TICK);
        }
        assert!(!registry.cancel("k"), "a reaped pid is never signalled");
        assert!(!registry.finish("k"), "the cancel still drops the response");
        assert!(!registry.cancel("k"), "a finished call is ignored");
    }

    #[test]
    fn a_cancelled_call_keeps_its_id_until_its_one_release() {
        let registry = Registry::default();
        registry.insert("n:1").unwrap();
        for i in 2..=MAX_IN_FLIGHT {
            registry.insert(&format!("n:{i}")).unwrap();
        }
        registry.cancel("n:1");
        // The call thread's cleanup: `finish`, then `release`.
        assert!(!registry.finish("n:1"));
        // The main loop reads a request reusing the id between the two steps.
        assert_eq!(
            registry.insert("n:1"),
            Err(Refused::Duplicate),
            "the id was free before the old call's release"
        );
        assert_eq!(registry.insert("n:99"), Err(Refused::Full));
        assert!(!registry.cancel("n:1"), "a finished call is ignored");
        registry.release("n:1");
        // Now a replacement takes the id, and nothing of the old call can remove it.
        assert_eq!(registry.insert("n:1"), Ok(()));
        assert!(registry.is_in_flight("n:1"));
        assert_eq!(registry.insert("n:99"), Err(Refused::Full));
    }

    #[test]
    fn the_registry_refuses_duplicates_and_a_seventeenth_call() {
        let registry = Registry::default();
        for i in 0..MAX_IN_FLIGHT {
            registry.insert(&format!("n:{i}")).unwrap();
        }
        assert_eq!(registry.insert("n:0"), Err(Refused::Duplicate));
        assert_eq!(registry.insert("n:99"), Err(Refused::Full));
        assert!(registry.finish("n:0"));
        assert!(
            !registry.cancel("n:0"),
            "a cancel after the response is decided is ignored"
        );
        assert_eq!(
            registry.insert("n:99"),
            Err(Refused::Full),
            "an answer not yet queued holds its slot"
        );
        registry.release("n:0");
        assert_eq!(registry.insert("n:99"), Ok(()));
        assert_eq!(registry.insert("n:100"), Err(Refused::Full));
        registry.shutdown();
        assert!(
            (0..MAX_IN_FLIGHT)
                .skip(1)
                .all(|i| !registry.finish(&format!("n:{i}")))
        );
    }
}
