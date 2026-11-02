//! `tools/call`: argument validation, argv, the exec-self runner and the result mapping
//! (design §A22). The child is glen itself (no shell), so a result's text is the CLI's own
//! envelope; only a call glen could not run to an envelope gets a synthesized one.
use super::{
    CALL_TICK, MAX_OUTPUT,
    catalog::{ArgKind, ArgSpec, Tool},
};
use crate::{
    error::{ErrorCode, GlenError},
    output::envelope::{error_value, to_json_vec},
};
use serde_json::{Map, Value, json};
use std::{
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

/// What a child run produced.
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

/// Reads `pipe` on its own thread: the first `limit` bytes (then flags an overflow and keeps
/// draining), or with `tail` only the last `limit` bytes.
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
/// inherited environment, cwd inherited.
pub(crate) fn run_child(launcher: &Launcher, argv: &[OsString], env: &[(&str, &str)]) -> Run {
    match spawn(launcher, argv, env) {
        Ok(child) => supervise(child),
        Err(e) => Run::NotStarted(e),
    }
}

/// Polls a started child until it exits, killing it when its stdout overflows.
pub(crate) fn supervise(mut child: Child) -> Run {
    let out = capture(child.stdout.take(), MAX_OUTPUT, false);
    let err = capture(child.stderr.take(), STDERR_TAIL, true);
    let mut killed = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                // Best effort: the child is unobservable, so stop it before reporting.
                let _ = child.kill();
                let _ = child.wait();
                return Run::Lost(e);
            }
        }
        if !killed && out.overflow.load(Ordering::SeqCst) {
            // std never signals a reaped child, so this kill cannot reach a recycled pid.
            let _ = child.kill();
            killed = true;
        }
        thread::sleep(CALL_TICK);
    };
    let deadline = Instant::now() + READER_JOIN;
    while !(out.done.load(Ordering::SeqCst) && err.done.load(Ordering::SeqCst))
        && Instant::now() < deadline
    {
        thread::sleep(CALL_TICK);
    }
    if killed || out.overflow.load(Ordering::SeqCst) {
        return Run::Overflow { stderr: err.take() };
    }
    Run::Exited {
        status,
        stdout: out.take(),
        stderr: err.take(),
    }
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
    use crate::mcp::catalog;

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
        result(&t, run_child(&sh(script), &[], &[]))
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
        let e = envelope(&result(&tool("glen_jobs"), run_child(&missing, &[], &[])));
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
        result(&tool("glen_jobs"), run_child(&sh(script), &[], env))
    }

    #[test]
    fn a_stdout_flood_is_killed_and_reported() {
        let start = Instant::now();
        let e = envelope(&mapped("exec yes"));
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(e["error"]["details"]["reason"], "mcp_exec");
        assert!(e["error"]["message"].as_str().unwrap().contains("16 MiB"));
    }

    #[test]
    fn an_oversize_argument_is_an_exec_failure() {
        // Linux refuses one argument over 128 KiB and macOS a whole argv over 1 MiB.
        let big = OsString::from("x".repeat(2 * 1024 * 1024));
        let e = envelope(&result(
            &tool("glen_send"),
            run_child(&sh("true"), &[big], &[]),
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
            run_child(&sh(r#"printf '{"v":1,"ok":true,"data":{}}\n'"#), &[], &[]),
        );
        assert_eq!(r["structuredContent"], json!({"v":1,"ok":true,"data":{}}));
    }
}
