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
    if args == ["--version"] {
        println!("omp/18.14.0");
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
    let mut resume = None;
    let mut no_session = false;
    let mut mode = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--mode" if iter.next().map(String::as_str) == Some("rpc") => mode = true,
            "--no-ui" => (),
            "--no-session" => no_session = true,
            "--profile" => {
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
        json!({"event":"start","pid":std::process::id(),"pgid":rustix::process::getpgrp().as_raw_nonzero().get(),"argv":args,"cwd":std::env::current_dir()?,"env":{"GLEN_MANAGED":knob("GLEN_MANAGED"),"OMP_PROFILE":knob("OMP_PROFILE"),"PI_PROFILE":knob("PI_PROFILE")}}),
    )?;
    if let Some(message) = knob("FAKE_OMP_STARTUP_STDERR") {
        let mut stderr = io::stderr().lock();
        stderr.write_all(message.as_bytes())?;
        stderr.flush()?;
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
    loop {
        if scenario != "wedge" && terminated.load(Ordering::SeqCst) {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(line) => {
                let f: Value = serde_json::from_str(&line)?;
                log(json!({"event":"in","frame":f}))?;
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
                        let data = json!({"sessionId":id,"sessionFile":session_file,"isStreaming":stage!=0&&stage!=4,"isCompacting":false,"isSettled":stage==0||stage==4,"messageCount":0,"model":model,"thinkingLevel":thinking,"queuedMessageCount":0,"contextUsage":{"tokens":tokens,"contextWindow":200000,"percent":0.617}});
                        if stage != 0 {
                            running_queries += 1;
                        }
                        let delay = if scenario == "stale_state" && running_queries == 1 {
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
                                response(&f, model.clone())?;
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
                    "prompt" if prompt.is_some() => reject(&f, "second prompt")?,
                    "prompt" => {
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
