//! Independent JSON-lines omp peer. Writes only resumed, test-owned fixture copies.
use serde_json::{Value, json};
#[cfg(not(target_os = "linux"))]
use std::fs::File;
use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};
fn knob(key: &str) -> Option<String> {
    std::env::var(key).ok()
}
fn ms(key: &str, default: u64) -> Duration {
    Duration::from_millis(knob(key).and_then(|s| s.parse().ok()).unwrap_or(default))
}
fn now() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
fn log(mut frame: Value) -> io::Result<()> {
    frame["at_ms"] = json!(now());
    if let Some(path) = knob("FAKE_OMP_LOG") {
        let mut bytes = serde_json::to_vec(&frame)?;
        bytes.push(b'\n');
        OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)?
            .write_all(&bytes)?;
    }
    Ok(())
}
fn out(frame: Value) -> io::Result<()> {
    log(json!({"event":"out","frame":frame}))?;
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &frame)?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}
fn response(frame: &Value, data: Value) -> io::Result<()> {
    out(
        json!({"type":"response","id":frame["id"],"command":frame["type"],"success":true,"data":data}),
    )
}
fn reject(frame: &Value, error: &str) -> io::Result<()> {
    out(
        json!({"type":"response","id":frame["id"],"command":frame["type"],"success":false,"error":error}),
    )
}
#[cfg(target_os = "linux")]
fn hold(path: &Path) -> io::Result<impl Send + use<>> {
    use std::os::linux::net::SocketAddrExt;
    let name = glen::omp::lease::memory_lock_name(&path.to_string_lossy());
    std::os::unix::net::UnixDatagram::bind_addr(
        &std::os::unix::net::SocketAddr::from_abstract_name(name)?,
    )
}
#[cfg(not(target_os = "linux"))]
fn hold(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    file.try_lock().map_err(io::Error::other)?;
    Ok(file)
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(code) => std::process::ExitCode::from(code),
        Err(e) => {
            eprintln!("fake omp: {e}");
            std::process::ExitCode::from(3)
        }
    }
}
fn run() -> io::Result<u8> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let version = knob("FAKE_OMP_VERSION")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "18.14.0".into());
    if args == ["--version"] {
        println!("omp/{version}");
        return Ok(0);
    }
    if args == ["--help"] {
        // The flags glen passes, in omp's `--flag=<value>` help style.
        println!(
            "omp v{version}

USAGE
  $ omp [MESSAGES...] [FLAGS]

FLAGS
  --mode=<value>           Output mode: text (default), json, rpc, or rpc-ui
  --no-ui                  With --mode rpc or rpc-ui: run extensions headless
  --no-session             Don't save session (ephemeral)
  -r, --resume=<value>     Resume a session (by ID prefix, path, or picker if omitted)
  --profile=<value>        Use an isolated profile for auth, sessions, settings, and caches
  --thinking=<value>       Set thinking level
  -e, --extension=<value>  Load an extension file (can be used multiple times)
  --no-extensions          Disable extension discovery (explicit -e paths still work)
  --no-skills              Disable skills discovery and loading
  --no-rules               Disable rules discovery and loading"
        );
        return Ok(0);
    }
    if args.first().map(String::as_str) == Some("hold-lease") && args.len() == 2 {
        let _lease = hold(Path::new(&args[1]))?;
        println!("held");
        io::stdout().flush()?;
        for line in io::stdin().lock().lines() {
            line?;
        }
        return Ok(0);
    }
    if args.first().map(String::as_str) == Some("acp") {
        return run_acp(&args, &version);
    }
    let mut resume = None;
    let mut no_session = false;
    let mut mode = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--mode" if iter.next().map(String::as_str) == Some("rpc") => mode = true,
            // Discovery switches: logged in argv, nothing to discover here.
            "--no-ui" | "--no-extensions" | "--no-skills" | "--no-rules" => (),
            "--no-session" => no_session = true,
            "--profile" => {
                if iter.next().is_none() {
                    return Ok(64);
                }
            }
            "--thinking" => {
                if iter.next().is_none() {
                    return Ok(64);
                }
            }
            "--resume" => match iter.next() {
                Some(p) => resume = Some(p.clone()),
                None => return Ok(64),
            },
            _ => return Ok(64),
        }
    }
    if !mode || (no_session && resume.is_some()) {
        return Ok(64);
    }
    let scenario = knob("FAKE_OMP_SCENARIO").unwrap_or_else(|| "ok".into());
    let terminated = Arc::new(AtomicBool::new(false));
    {
        signal_hook::flag::register(signal_hook::consts::SIGTERM, terminated.clone())?;
    }
    log(
        json!({"event":"start","pid":std::process::id(),"pgid":rustix::process::getpgrp().as_raw_nonzero().get(),"argv":args,"cwd":std::env::current_dir()?,"env":{"GLEN_MANAGED":knob("GLEN_MANAGED"),"OMP_PROFILE":knob("OMP_PROFILE"),"PI_PROFILE":knob("PI_PROFILE"),"PI_CONFIG_FILES":knob("PI_CONFIG_FILES")}}),
    )?;
    if let Some(message) = knob("FAKE_OMP_STARTUP_STDERR") {
        let mut stderr = io::stderr().lock();
        stderr.write_all(message.as_bytes())?;
        stderr.flush()?;
    }
    if scenario == "startup_exit" {
        return Ok(1);
    }
    let mut id = if let Some(path) = &resume {
        fs::read_to_string(path)?
            .lines()
            .filter_map(|s| serde_json::from_str::<Value>(s).ok())
            .find(|v| v["type"] == "session")
            .and_then(|v| v["id"].as_str().map(str::to_owned))
            .ok_or_else(|| io::Error::other("missing session header"))?
    } else {
        format!("01a1ffff-0000-7000-8000-{:012x}", std::process::id())
    };
    if scenario == "moved" {
        id = format!("{}beef", &id[..id.len().saturating_sub(4)]);
    }
    let mut lease = None;
    if knob("FAKE_OMP_STARTUP_RECOVERY").as_deref() == Some("1") {
        if let Some(owners) = knob("FAKE_OMP_OWNERS_DIR") {
            fs::create_dir_all(&owners)?;
            lease = Some(hold(&Path::new(&owners).join(format!("{id}.lock")))?);
        }
        if let Some(path) = &resume {
            let mut file = OpenOptions::new().append(true).open(path)?;
            writeln!(
                file,
                "{}",
                json!({"type":"message","id":"fake-recovery","message":{"role":"assistant","content":[],"stopReason":"aborted"}})
            )?;
            file.sync_all()?;
            log(json!({"event":"startup_recovery"}))?;
        }
    }
    let mut session_file = resume;
    let models: Vec<Value> = serde_json::from_str(include_str!("fake_models.json"))?;
    let selected = session_file
        .as_deref()
        .and_then(|p| glen::omp::session::Session::load(Path::new(p)).ok())
        .map(|s| s.selection());
    let mut model = selected
        .as_ref()
        .and_then(|s| s.model.as_deref())
        .and_then(|pair| {
            models.iter().find(|m| {
                format!(
                    "{}/{}",
                    m["provider"].as_str().unwrap_or(""),
                    m["id"].as_str().unwrap_or("")
                ) == pair
            })
        })
        .cloned()
        .unwrap_or_else(|| models[0].clone());
    let mut thinking = selected
        .and_then(|s| s.thinking_level)
        .unwrap_or_else(|| "medium".into());
    let mut tokens = 1234;
    let serial = knob("FAKE_OMP_SERIAL").as_deref() == Some("1");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            match line {
                Ok(s) => {
                    if tx.send(s).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    // Delay ready independently of prompt work to exercise cancellation during startup.
    std::thread::sleep(ms("FAKE_OMP_READY_DELAY_MS", 0));
    if let Some(release) = knob("FAKE_OMP_READY_RELEASE") {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !Path::new(&release).exists() {
            if terminated.load(Ordering::SeqCst)
                || matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected))
            {
                return Ok(0);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("ready release timed out"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    if scenario != "no_ready" {
        out(match knob("FAKE_OMP_V2").as_deref() {
            Some("1") => {
                json!({"type":"ready","protocolVersion":1,"supportedProtocolVersions":[1,2],"maxFrameBytes":1048576,"maxReassembledFrameBytes":67108864})
            }
            Some("partial") => json!({"type":"ready","supportedProtocolVersions":[1,2]}),
            _ => json!({"type":"ready"}),
        })?;
    }
    let mut prompt: Option<Value> = None;
    let mut due = None;
    let mut stage = 0;
    let mut replies: Vec<Value> = vec![];
    let mut reply_index = 0;
    let mut running_queries = 0;
    let mut state_queries = 0;
    // v0.2 W4 warm-worker knobs (design §A15): several prompts per process, and events while idle.
    let multi = knob("FAKE_OMP_MULTI").as_deref() == Some("1");
    let mut idle_reply_sent = false;
    loop {
        if scenario != "wedge" && terminated.load(Ordering::SeqCst) {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(line) => {
                let f: Value = serde_json::from_str(&line)?;
                log(json!({"event":"in","frame":f}))?;
                if knob("FAKE_OMP_STALL").is_some_and(|t| f["type"] == t) {
                    continue;
                }
                match f["type"].as_str().unwrap_or("") {
                    "negotiate_protocol" => response(&f, json!({"protocolVersion":2}))?,
                    "set_host_tools" if scenario == "reject_tools" => reject(
                        &f,
                        "RPC host tool \"glen_reply\" conflicts with an existing tool",
                    )?,
                    "set_host_tools" => response(&f, json!({"toolNames":["glen_reply"]}))?,
                    "set_event_filter" if scenario == "reject_filter" => {
                        reject(&f, "fake filter rejected")?
                    }
                    "set_event_filter" => response(&f, json!({"events":f["events"]}))?,
                    "get_state" => {
                        state_queries += 1;
                        let data = json!({"sessionId":id,"sessionFile":session_file,"isStreaming":stage!=0&&stage!=4,"isCompacting":false,"isSettled":stage==0||stage==4,"messageCount":0,"model":model,"thinkingLevel":thinking,"queuedMessageCount":0,"contextUsage":{"tokens":tokens,"contextWindow":200000,"percent":0.617}});
                        if stage != 0 {
                            running_queries += 1;
                        }
                        // `N` delays the Nth get_state; `N+` delays it and every later one.
                        let delayed_at = knob("FAKE_OMP_STATE_DELAY_AT").and_then(|v| {
                            match v.strip_suffix('+') {
                                Some(n) => n.parse::<usize>().ok().map(|n| state_queries >= n),
                                None => v.parse::<usize>().ok().map(|n| state_queries == n),
                            }
                        });
                        let delay = if delayed_at == Some(true) {
                            ms("FAKE_OMP_PREFLIGHT_DELAY_MS", 0)
                        } else if scenario == "stale_state" && running_queries == 1 {
                            id = format!("{}f0f0", &id[..id.len().saturating_sub(4)]);
                            ms("FAKE_OMP_OLD_STATE_DELAY_MS", 1500)
                        } else if stage != 0 {
                            ms("FAKE_OMP_STATE_DELAY_MS", 0)
                        } else {
                            ms("FAKE_OMP_BASELINE_DELAY_MS", 0)
                        };
                        let stale_reply = scenario == "stale_state" && running_queries == 1;
                        if delay.is_zero() {
                            response(&f, data)?;
                        } else if serial {
                            if !delay_while_alive(delay, &terminated) {
                                break;
                            }
                            response(&f, data)?;
                        } else {
                            std::thread::spawn(move || {
                                std::thread::sleep(delay);
                                if let Err(e) = response(&f, data) {
                                    eprintln!("fake delayed state: {e}");
                                    return;
                                }
                                if stale_reply {
                                    // Let the polling loop consume the stale state before another reply.
                                    std::thread::sleep(Duration::from_millis(150));
                                    if let Err(e) = emit_call(
                                        1,
                                        &json!({"text":"reply after stale state","final":true}),
                                    ) {
                                        eprintln!("fake stale-state reply: {e}");
                                    }
                                }
                            });
                        }
                    }
                    "get_available_models" => response(&f, json!({"models":models}))?,
                    "get_available_thinking_levels" => {
                        response(&f, json!({"levels":fake_levels(&model)}))?
                    }
                    "set_model" => {
                        let found = models
                            .iter()
                            .find(|m| m["provider"] == f["provider"] && m["id"] == f["modelId"]);
                        if let Some(found) = found {
                            if found["provider"] == "nokey" {
                                reject(
                                    &f,
                                    &format!(
                                        "No API key for nokey/{}",
                                        f["modelId"].as_str().unwrap_or("")
                                    ),
                                )?;
                            } else {
                                model = found.clone();
                                append_entry(
                                    session_file.as_deref().filter(|_| !no_session),
                                    json!({"type":"model_change","model":format!("{}/{}",model["provider"].as_str().unwrap_or(""),model["id"].as_str().unwrap_or("")),"role":"default"}),
                                )?;
                                if model["reasoning"] == false && thinking != "off" {
                                    thinking = "off".into();
                                    append_entry(
                                        session_file.as_deref().filter(|_| !no_session),
                                        json!({"type":"thinking_level_change","thinkingLevel":thinking,"configured":thinking}),
                                    )?;
                                }
                                if scenario == "fork_control" {
                                    id = format!("{}f0f0", &id[..id.len().saturating_sub(4)]);
                                }
                                if !delay_while_alive(
                                    ms("FAKE_OMP_SET_MODEL_DELAY_MS", 0),
                                    &terminated,
                                ) {
                                    break;
                                }
                                if knob("FAKE_OMP_SET_MODEL_DROP").as_deref()
                                    == Some("before_reply")
                                {
                                    return Ok(0);
                                }
                                response(&f, model.clone())?;
                                if knob("FAKE_OMP_SET_MODEL_DROP").as_deref() == Some("after_reply")
                                {
                                    return Ok(0);
                                }
                            }
                        } else {
                            reject(
                                &f,
                                &format!(
                                    "Model not found: {}/{}",
                                    f["provider"].as_str().unwrap_or(""),
                                    f["modelId"].as_str().unwrap_or("")
                                ),
                            )?;
                        }
                    }
                    "set_thinking_level" => {
                        let requested = f["level"].as_str().unwrap_or("off");
                        let level = if fake_levels(&model).iter().any(|l| l == requested) {
                            requested
                        } else {
                            "off"
                        };
                        if thinking != level {
                            thinking = level.into();
                            append_entry(
                                session_file.as_deref().filter(|_| !no_session),
                                json!({"type":"thinking_level_change","thinkingLevel":thinking,"configured":thinking}),
                            )?;
                        }
                        if scenario == "fork_control" {
                            id = format!("{}f0f0", &id[..id.len().saturating_sub(4)]);
                        }
                        response(&f, json!({}))?;
                    }
                    "compact" => {
                        if scenario == "compact_error" {
                            reject(&f, "Nothing to compact")?;
                        } else {
                            if !delay_while_alive(ms("FAKE_OMP_COMPACT_DELAY_MS", 0), &terminated) {
                                break;
                            }
                            append_entry(
                                session_file.as_deref().filter(|_| !no_session),
                                json!({"type":"compaction","summary":"fake compacted context","firstKeptEntryId":"fake-kept","tokensBefore":1234}),
                            )?;
                            tokens = 123;
                            if scenario == "fork_control" {
                                id = format!("{}f0f0", &id[..id.len().saturating_sub(4)]);
                            }
                            response(
                                &f,
                                json!({"summary":"fake compacted context","firstKeptEntryId":"fake-kept","tokensBefore":1234}),
                            )?;
                        }
                    }
                    "abort" => {
                        response(&f, json!({}))?;
                        if scenario != "wedge"
                            && let Some(p) = prompt.as_ref()
                        {
                            out(
                                json!({"type":"agent_end","messages":[],"isTerminal":true,"yielded":true}),
                            )?;
                            out(
                                json!({"type":"prompt_result","id":p["id"],"agentInvoked":true,"status":"aborted","sessionSettled":true}),
                            )?;
                            out(json!({"type":"session_settled"}))?;
                            stage = 4;
                            due = None;
                        }
                    }
                    "prompt" if prompt.is_some() && !(multi && stage == 4) => {
                        reject(&f, "second prompt")?
                    }
                    "prompt" => {
                        // FAKE_OMP_MULTI: the previous prompt settled; this process takes another.
                        stage = 0;
                        due = None;
                        replies.clear();
                        reply_index = 0;
                        if scenario == "local_result_unsettled" {
                            response(&f, json!({}))?;
                            out(
                                json!({"type":"prompt_result","id":f["id"],"agentInvoked":false,"status":"completed","sessionSettled":false}),
                            )?;
                            prompt = Some(f);
                            continue;
                        }
                        if matches!(scenario.as_str(), "local" | "local_exit") {
                            out(json!({"type":"command_output","text":"fake command output"}))?;
                            response(&f, json!({"agentInvoked":false}))?;
                            if scenario == "local_exit" {
                                return Ok(0);
                            }
                            prompt = Some(f);
                            continue;
                        }
                        if lease.is_none()
                            && let Some(owners) = knob("FAKE_OMP_OWNERS_DIR")
                        {
                            let path = Path::new(&owners).join(format!("{id}.lock"));
                            if let Some(parent) = path.parent() {
                                fs::create_dir_all(parent)?;
                            }
                            lease = Some(hold(&path)?);
                        }
                        response(&f, json!({}))?;
                        out(json!({"type":"agent_start"}))?;
                        out(
                            json!({"type":"message_end","message":{"role":"user","content":f["message"],"timestamp":now()}}),
                        )?;
                        if session_file.is_none() {
                            session_file = Some(
                                std::env::current_dir()?
                                    .join(format!("fake-{id}.jsonl"))
                                    .to_string_lossy()
                                    .into_owned(),
                            );
                        }
                        if scenario == "crash" {
                            log(json!({"event":"exit","reason":"crash"}))?;
                            return Ok(3);
                        }
                        if matches!(
                            scenario.as_str(),
                            "delayed_state" | "fork_reply" | "stale_state"
                        ) {
                            if scenario == "fork_reply" {
                                id = format!("{}f0f0", &id[..id.len().saturating_sub(4)]);
                            }
                            out(
                                json!({"type":"notice","level":"warning","message":"persistence changed","source":"session-persistence"}),
                            )?;
                        }
                        if !matches!(scenario.as_str(), "hang" | "wedge" | "delayed_state") {
                            due = Some(
                                Instant::now()
                                    + ms(
                                        "FAKE_OMP_DELAY_MS",
                                        if scenario == "fork_reply" { 500 } else { 0 },
                                    ),
                            );
                        }
                        stage = 1;
                        prompt = Some(f);
                    }
                    // The idle call's result is logged (as every frame is) and never advances a job.
                    "host_tool_result" if f["id"] == "htc_idle" => (),
                    "host_tool_result" => {
                        reply_index += 1;
                        if reply_index < replies.len() {
                            emit_call(reply_index, &replies[reply_index])?;
                        } else {
                            stage = 2;
                            due = Some(Instant::now());
                        }
                    }
                    _ => reject(&f, "unsupported by fake omp")?,
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => (),
        }
        let idle = prompt.is_none() || stage == 4;
        let released = |key: &str| knob(key).is_some_and(|p| Path::new(&p).exists());
        if idle && !idle_reply_sent && released("FAKE_OMP_IDLE_REPLY_RELEASE") {
            idle_reply_sent = true;
            log(json!({"event":"idle_reply"}))?;
            out(
                json!({"type":"host_tool_call","id":"htc_idle","toolName":"glen_reply","toolCallId":"call_idle","arguments":{"text":"idle reply","final":false}}),
            )?;
        }
        if idle && released("FAKE_OMP_IDLE_EXIT_RELEASE") {
            log(json!({"event":"exit","reason":"idle release"}))?;
            return Ok(0);
        }
        if due.is_none_or(|t| Instant::now() < t) {
            continue;
        }
        due = None;
        let Some(p) = &prompt else { continue };
        match stage {
            1 => {
                if knob("FAKE_OMP_RUN_RELEASE").is_some_and(|p| !Path::new(&p).exists()) {
                    due = Some(Instant::now() + Duration::from_millis(10));
                    continue;
                }
                replies = match scenario.as_str() {
                    "bad_reply" => vec![
                        json!({}),
                        json!({"text":""}),
                        json!({"text":"ok","final":"yes"}),
                    ],
                    "progress" => vec![
                        json!({"text":"progress","final":false}),
                        json!({"text":format!("reply: {}",p["message"].as_str().unwrap_or("")),"final":true}),
                    ],
                    _ => vec![
                        json!({"text":format!("reply: {}",p["message"].as_str().unwrap_or("")),"final":true}),
                    ],
                };
                reply_index = 0;
                emit_call(0, &replies[0])?;
                due = Some(Instant::now() + Duration::from_secs(5));
                stage = 5;
            }
            2 => {
                if knob("FAKE_OMP_RESULT_RELEASE").is_some_and(|p| !Path::new(&p).exists()) {
                    due = Some(Instant::now() + Duration::from_millis(10));
                    continue;
                }
                if matches!(scenario.as_str(), "fork" | "fork_silent") {
                    id = format!("{}f0f0", &id[..id.len().saturating_sub(4)]);
                    if scenario == "fork" {
                        out(
                            json!({"type":"notice","level":"warning","message":"Session moved to a new file","source":"session-persistence"}),
                        )?;
                    }
                }
                let reply = knob("FAKE_OMP_REPLY_BYTES")
                    .and_then(|s| s.parse::<usize>().ok())
                    .map(|n| "x".repeat(n))
                    .unwrap_or_else(|| format!("pong: {}", p["message"].as_str().unwrap_or("")));
                let assistant = json!({"role":"assistant","content":[{"type":"text","text":reply}],"provider":"fake","model":"fake-1","stopReason":"stop","timestamp":now()});
                out(json!({"type":"message_end","message":assistant}))?;
                out(
                    json!({"type":"agent_end","messages":[{"role":"user","content":p["message"],"timestamp":now()},assistant],"isTerminal":true,"yielded":true}),
                )?;
                let settled = knob("FAKE_OMP_NEVER_SETTLE").as_deref() != Some("1")
                    && !matches!(
                        scenario.as_str(),
                        "unsettled" | "never_settles" | "unsettled_exit"
                    );
                let status = match scenario.as_str() {
                    "error" | "network_refused" | "network_dns" | "network_denied" => "error",
                    "aborted" => "aborted",
                    _ => "completed",
                };
                let mut frame = json!({"type":"prompt_result","id":p["id"],"agentInvoked":true,"status":status,"sessionSettled":settled});
                if status == "error" {
                    let message = match scenario.as_str() {
                        "network_refused" => "connect: Connection refused",
                        "network_dns" => "DNS resolution failed",
                        "network_denied" => "connect: Operation not permitted",
                        _ => "fake provider error",
                    };
                    frame["error"] = json!({"message":message,"retryable":true});
                }
                out(frame)?;
                if scenario == "unsettled_exit" {
                    return Ok(0);
                }
                if settled {
                    out(json!({"type":"session_settled"}))?;
                    stage = 4;
                } else if scenario == "unsettled" {
                    stage = 3;
                    due = Some(Instant::now() + ms("FAKE_OMP_SETTLE_DELAY_MS", 500));
                } else {
                    stage = 4;
                }
            }
            3 => {
                out(json!({"type":"session_settled"}))?;
                stage = 4;
            }
            5 => return Err(io::Error::other("host tool result timed out")),
            _ => (),
        }
    }
    drop(lease);
    log(json!({"event":"exit","reason":"stdin eof"}))?;
    Ok(0)
}
/// `fake_omp acp [--profile NAME]`: an ACP v1 agent shaped like omp 18.14.0 (design §A15 lists
/// the scenarios and knobs). It never writes a session file.
fn run_acp(args: &[String], version: &str) -> io::Result<u8> {
    let accepted = match &args[1..] {
        [] => true,
        [flag, _] => flag == "--profile",
        _ => false,
    };
    if !accepted {
        return Ok(64);
    }
    let scenario = knob("FAKE_OMP_ACP").unwrap_or_else(|| "ok".into());
    let terminated = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, terminated.clone())?;
    log(
        json!({"event":"start","pid":std::process::id(),"pgid":rustix::process::getpgrp().as_raw_nonzero().get(),"argv":args,"cwd":std::env::current_dir()?,"env":{"GLEN_MANAGED":knob("GLEN_MANAGED"),"OMP_PROFILE":knob("OMP_PROFILE"),"PI_PROFILE":knob("PI_PROFILE")}}),
    )?;
    if let Some(message) = knob("FAKE_OMP_STARTUP_STDERR") {
        let mut stderr = io::stderr().lock();
        stderr.write_all(message.as_bytes())?;
        stderr.flush()?;
    }
    if scenario == "startup_exit" {
        return Ok(1);
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let peer = AcpPeer {
        rx,
        terminated,
        scenario,
    };
    while let Some(frame) = peer.recv() {
        let Some(method) = frame["method"].as_str() else {
            continue;
        };
        let Some(id) = frame.get("id").cloned() else {
            // Notifications, including session/cancel while idle, are ignored.
            continue;
        };
        let params = &frame["params"];
        match method {
            "initialize" => match peer.scenario.as_str() {
                "init_hang" => (),
                "init_error" => acp_error(
                    &id,
                    -32603,
                    "Internal error",
                    json!({"details":"fake initialize failure"}),
                )?,
                scenario => {
                    let mut result = json!({"protocolVersion":1,"agentInfo":{"name":"omp","title":"omp","version":version},"authMethods":[{"id":"agent","name":"Use existing local credentials","description":"fake"}],"agentCapabilities":{"loadSession":true,"mcpCapabilities":{"http":true,"sse":true},"promptCapabilities":{"embeddedContext":true,"image":true},"sessionCapabilities":{"list":{},"fork":{},"resume":{},"close":{}}},"_meta":{"steering":{"supported":true}}});
                    match scenario {
                        "proto2" => result["protocolVersion"] = json!(2),
                        "proto_missing" => {
                            if let Some(o) = result.as_object_mut() {
                                o.remove("protocolVersion");
                            }
                        }
                        "no_load" => {
                            result["agentCapabilities"]["loadSession"] = json!(false);
                            if let Some(o) = result["agentCapabilities"].as_object_mut() {
                                o.remove("sessionCapabilities");
                            }
                        }
                        _ => (),
                    }
                    acp_result(&id, result)?;
                    if scenario == "noise" {
                        let mut stdout = io::stdout().lock();
                        stdout.write_all(b"this is not json\n")?;
                        stdout.flush()?;
                        log(json!({"event":"out","raw":"this is not json"}))?;
                    }
                }
            },
            "session/new" => {
                if !relative_cwd(&id, params)? {
                    let session = knob("FAKE_OMP_ACP_SESSION").unwrap_or_else(|| {
                        format!("01a1ffff-0000-7000-8000-{:012x}", std::process::id())
                    });
                    acp_result(
                        &id,
                        json!({"sessionId":session,"modes":modes(),"configOptions":[]}),
                    )?;
                }
            }
            "session/load" => {
                let session = params["sessionId"].as_str().unwrap_or("").to_owned();
                if peer.scenario == "rpc_error_load" {
                    acp_error(
                        &id,
                        -32603,
                        "Internal error",
                        json!({"details":format!("ACP session not found: {session}")}),
                    )?;
                } else if !relative_cwd(&id, params)? {
                    let pairs = knob("FAKE_OMP_ACP_REPLAY")
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(2);
                    for n in 1..=pairs {
                        update(
                            &session,
                            json!({"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"REPLAYED QUESTION"},"messageId":format!("replay-{n}-user")}),
                        )?;
                        update(
                            &session,
                            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"REPLAYED ANSWER"},"messageId":format!("replay-{n}")}),
                        )?;
                    }
                    acp_result(&id, json!({"modes":modes(),"configOptions":[]}))?;
                    // omp's bootstrap updates follow on a timer; they are never message text.
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(100));
                        let sent = update(&session, json!({"sessionUpdate":"available_commands_update","availableCommands":[{"name":"compact","description":"Compact the context"}]}))
                            .and_then(|()| update(&session, json!({"sessionUpdate":"session_info_update","title":"fake session"})));
                        if let Err(e) = sent {
                            eprintln!("fake acp bootstrap: {e}");
                        }
                    });
                }
            }
            "session/prompt" => {
                if let Some(code) = peer.prompt(&id, params)? {
                    return Ok(code);
                }
            }
            "session/close" => acp_result(&id, json!({}))?,
            _ => acp_error(
                &id,
                -32601,
                &format!("Method not found: {method}"),
                Value::Null,
            )?,
        }
    }
    log(json!({"event":"exit","reason":"stdin eof"}))?;
    Ok(0)
}
struct AcpPeer {
    rx: mpsc::Receiver<String>,
    terminated: Arc<AtomicBool>,
    scenario: String,
}
impl AcpPeer {
    /// The next inbound frame (logged), or `None` at stdin EOF or SIGTERM.
    fn recv(&self) -> Option<Value> {
        loop {
            if self.terminated.load(Ordering::SeqCst) {
                return None;
            }
            match self.rx.recv_timeout(Duration::from_millis(10)) {
                Ok(line) => {
                    let frame: Value =
                        serde_json::from_str(&line).unwrap_or_else(|_| json!({"unparsed":line}));
                    if let Err(e) = log(json!({"event":"in","frame":frame})) {
                        eprintln!("fake acp log: {e}");
                    }
                    return Some(frame);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => (),
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
    }
    /// The first inbound frame matching `want`; other frames are logged and dropped.
    fn wait_for(&self, want: impl Fn(&Value) -> bool) -> Option<Value> {
        loop {
            let frame = self.recv()?;
            if want(&frame) {
                return Some(frame);
            }
        }
    }
    /// Runs one prompt turn; `Some(code)` when the process should exit with it.
    fn prompt(&self, id: &Value, params: &Value) -> io::Result<Option<u8>> {
        let session = params["sessionId"].as_str().unwrap_or("").to_owned();
        let text = match params["prompt"].as_array().map(Vec::as_slice) {
            Some([block]) if block["type"] == "text" && block["text"].is_string() => {
                block["text"].as_str().unwrap_or("").to_owned()
            }
            _ => {
                acp_error(
                    id,
                    -32602,
                    "Invalid params",
                    json!({"details":"prompt must be one text block"}),
                )?;
                return Ok(None);
            }
        };
        if self.scenario == "rpc_error_prompt" {
            acp_error(
                id,
                -32603,
                "Internal error",
                json!({"details":format!("ACP session not found: {session}")}),
            )?;
            return Ok(None);
        }
        let chunk = |kind: &str, text: &str, message: &str| {
            update(
                &session,
                json!({"sessionUpdate":kind,"content":{"type":"text","text":text},"messageId":message}),
            )
        };
        chunk("agent_thought_chunk", "thinking about it", "m1")?;
        chunk("agent_message_chunk", "Hel", "m1")?;
        let end = |stop: &str| {
            acp_result(
                id,
                json!({"stopReason":stop,"usage":{"totalTokens":3,"inputTokens":1,"outputTokens":2}}),
            )
        };
        match self.scenario.as_str() {
            "crash" => {
                log(json!({"event":"exit","reason":"crash"}))?;
                return Ok(Some(1));
            }
            "cancel" => {
                if self.wait_for(|f| f["method"] == "session/cancel").is_none() {
                    return Ok(Some(0));
                }
                end("cancelled")?;
            }
            "cancel_ignored" | "hang" => {
                while self.recv().is_some() {}
                log(json!({"event":"exit","reason":"stdin eof or SIGTERM"}))?;
                return Ok(Some(0));
            }
            "permission" => {
                update(
                    &session,
                    json!({"sessionUpdate":"tool_call","toolCallId":"t1","title":"Delete e2-target.txt","kind":"execute","status":"pending","rawInput":{"command":"rm e2-target.txt"}}),
                )?;
                let kinds: &[&str] = match knob("FAKE_OMP_ACP_OPTIONS").as_deref() {
                    Some("reject_only") => &["reject_once", "reject_always"],
                    Some("allow_only") => &["allow_once", "allow_always"],
                    _ => &["allow_once", "allow_always", "reject_once", "reject_always"],
                };
                let names = |k: &str| match k {
                    "allow_once" => "Allow once",
                    "allow_always" => "Always allow",
                    "reject_once" => "Reject",
                    _ => "Always reject",
                };
                let options: Vec<_> = kinds
                    .iter()
                    .map(|k| json!({"optionId":k,"name":names(k),"kind":k}))
                    .collect();
                out(
                    json!({"jsonrpc":"2.0","id":100,"method":"session/request_permission","params":{"sessionId":session,"toolCall":{"toolCallId":"t1","title":"Delete e2-target.txt","kind":"execute","status":"pending","rawInput":{"command":"rm e2-target.txt"}},"options":options}}),
                )?;
                let Some(answer) = self.wait_for(|f| f["id"] == 100 && f.get("method").is_none())
                else {
                    return Ok(Some(0));
                };
                log(json!({"event":"permission","response":answer}))?;
                let outcome = &answer["result"]["outcome"];
                let chosen = if outcome["outcome"] == "selected" {
                    outcome["optionId"].as_str().unwrap_or("?").to_owned()
                } else {
                    "cancelled".to_owned()
                };
                let status = if matches!(chosen.as_str(), "allow_once" | "allow_always") {
                    "completed"
                } else {
                    "failed"
                };
                update(
                    &session,
                    json!({"sessionUpdate":"tool_call_update","toolCallId":"t1","status":status}),
                )?;
                chunk("agent_message_chunk", &format!("permission:{chosen}"), "m2")?;
                end("end_turn")?;
            }
            "agent_request" => {
                out(
                    json!({"jsonrpc":"2.0","id":101,"method":"fs/read_text_file","params":{"sessionId":session,"path":"/etc/hosts"}}),
                )?;
                let Some(answer) = self.wait_for(|f| f["id"] == 101 && f.get("method").is_none())
                else {
                    return Ok(Some(0));
                };
                let code = answer["error"]["code"]
                    .as_i64()
                    .map_or("none".to_owned(), |c| c.to_string());
                chunk(
                    "agent_message_chunk",
                    &format!("agent-request:{code}"),
                    "m2",
                )?;
                end("end_turn")?;
            }
            "flood" => {
                // A runaway agent: distinct, long update kinds and permission requests with huge
                // strings, as fast as stdout takes them, until SIGTERM or a closed pipe. It never
                // ends the turn and never logs the frames.
                let pad = "k".repeat(4 * 1024);
                let huge = "t".repeat(256 * 1024);
                let mut stdout = io::stdout().lock();
                for i in 0u64.. {
                    if self.terminated.load(Ordering::SeqCst) {
                        break;
                    }
                    let frame = if i % 16 == 0 {
                        json!({"jsonrpc":"2.0","id":format!("flood-{i}"),"method":"session/request_permission","params":{"sessionId":session,"toolCall":{"toolCallId":huge,"title":huge,"kind":huge,"status":"pending"},"options":[{"optionId":"allow_once","name":"Allow once","kind":"allow_once"}]}})
                    } else {
                        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":format!("flood_{i}_{pad}")}}})
                    };
                    let written = serde_json::to_writer(&mut stdout, &frame)
                        .map_err(io::Error::from)
                        .and_then(|()| stdout.write_all(b"\n"));
                    if written.is_err() {
                        break;
                    }
                }
                drop(stdout);
                log(json!({"event":"exit","reason":"flood ended"}))?;
                return Ok(Some(0));
            }
            "stop" => {
                let stop = knob("FAKE_OMP_ACP_STOP").unwrap_or_else(|| "refusal".into());
                chunk("agent_message_chunk", "lo ", "m1")?;
                chunk("agent_message_chunk", &format!("stopped: {stop}"), "m2")?;
                end(&stop)?;
            }
            _ => {
                chunk("agent_message_chunk", "lo ", "m1")?;
                update(
                    &session,
                    json!({"sessionUpdate":"tool_call","toolCallId":"t1","title":"Read file","kind":"read","status":"pending"}),
                )?;
                update(
                    &session,
                    json!({"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed"}),
                )?;
                chunk("agent_message_chunk", "reply: ", "m2")?;
                chunk("agent_message_chunk", &text, "m2")?;
                end("end_turn")?;
            }
        }
        Ok(None)
    }
}
fn acp_result(id: &Value, result: Value) -> io::Result<()> {
    out(json!({"jsonrpc":"2.0","id":id,"result":result}))
}
fn acp_error(id: &Value, code: i64, message: &str, data: Value) -> io::Result<()> {
    let mut error = json!({"code":code,"message":message});
    if !data.is_null() {
        error["data"] = data;
    }
    out(json!({"jsonrpc":"2.0","id":id,"error":error}))
}
fn update(session: &str, update: Value) -> io::Result<()> {
    out(
        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":update}}),
    )
}
fn modes() -> Value {
    json!({"currentModeId":"default","availableModes":[{"id":"default","name":"Default"}]})
}
/// omp rejects a relative `cwd` with -32603 (`acp-agent.ts:2067`); true when it did so here.
fn relative_cwd(id: &Value, params: &Value) -> io::Result<bool> {
    let cwd = params["cwd"].as_str().unwrap_or("");
    if Path::new(cwd).is_absolute() {
        return Ok(false);
    }
    acp_error(
        id,
        -32603,
        "Internal error",
        json!({"details":format!("cwd must be an absolute path: {cwd:?}")}),
    )?;
    Ok(true)
}
fn emit_call(i: usize, args: &Value) -> io::Result<()> {
    out(
        json!({"type":"host_tool_call","id":format!("htc_{}",i+1),"toolName":"glen_reply","toolCallId":format!("call_{}",i+1),"arguments":args}),
    )
}

fn fake_levels(model: &Value) -> Vec<String> {
    let efforts = model["thinking"]["efforts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    glen::ctl::levels_of(model["reasoning"].as_bool().unwrap_or(false), &efforts)
}
fn delay_while_alive(delay: Duration, terminated: &AtomicBool) -> bool {
    let end = Instant::now() + delay;
    while Instant::now() < end {
        if terminated.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    !terminated.load(Ordering::SeqCst)
}
fn append_entry(path: Option<&str>, mut entry: Value) -> io::Result<()> {
    let Some(path) = path.filter(|p| Path::new(p).is_file()) else {
        return Ok(());
    };
    let session = glen::omp::session::Session::load(Path::new(path)).map_err(io::Error::other)?;
    entry["parentId"] = json!(session.leaf_id());
    entry["id"] = json!(uuid::Uuid::now_v7().to_string());
    entry["timestamp"] = json!(jiff::Timestamp::now().to_string());
    let mut file = OpenOptions::new().append(true).open(path)?;
    writeln!(file, "{entry}")?;
    file.sync_all()
}
