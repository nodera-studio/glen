//! Scriptable extension peer; never opens an omp session file.
use glen::{
    glen_home::{GlenHome, write_private_atomic},
    jobs::Reply,
    live::registry::{self, Registry},
    ops,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
#[derive(Clone)]
struct Peer {
    home: GlenHome,
    id: String,
    file: Option<PathBuf>,
    socket: PathBuf,
    scenario: String,
    log: Option<PathBuf>,
    version: String,
    delay: u64,
    mode: String,
}
impl Peer {
    fn log(&self, dir: &str, frame: &Value) {
        if let Some(p) = &self.log {
            let mut o = fs::OpenOptions::new();
            o.create(true).append(true);
            if let Ok(mut f) = o.open(p) {
                let _ = writeln!(
                    f,
                    "{}",
                    json!({"t":jiff::Timestamp::now().as_millisecond(),"dir":dir,"frame":frame})
                );
            }
        }
    }
    fn write(&self, s: &mut UnixStream, v: Value) -> io::Result<()> {
        self.log("out", &v);
        writeln!(s, "{v}")
    }
    fn data(&self, hello: bool) -> Value {
        let busy = matches!(self.scenario.as_str(), "busy" | "queued");
        let mut d = json!({"ext_version":self.version,"omp_version":"18.14.0","pid":std::process::id(),"session_id":if self.scenario=="mismatch"{moved(&self.id)}else{self.id.clone()},"session_file":self.file,"mode":self.mode,"state":if busy{"busy"}else{"idle"},"busy":busy,"queued":self.scenario=="queued","active_requests":0});
        if hello {
            d["protocol"] = json!(1);
            d["protocols"] = json!([1]);
            d["ops"] = if self.scenario == "no_send" {
                json!(["hello", "state"])
            } else {
                json!(ops::IMPLEMENTED)
            }
        }
        d
    }
    fn serve(&self, mut s: UnixStream) -> io::Result<()> {
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        s.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut line = vec![];
        BufReader::new(s.try_clone()?)
            .take(4 * 1024 * 1024 + 2)
            .read_until(b'\n', &mut line)?;
        let v = serde_json::from_slice::<Value>(&line).unwrap_or(Value::Null);
        let id = v["id"].as_str().map(str::to_owned);
        self.log("in", &v);
        let bad = |s: &mut UnixStream, code: &str, message: &str, details: Value| {
            self.write(s,json!({"v":1,"id":id,"ok":false,"error":{"code":code,"message":message,"details":details}}))
        };
        if line.len() > 4 * 1024 * 1024 + 1 || id.is_none() || !line.ends_with(b"\n") {
            return bad(&mut s, "bad_request", "malformed request", Value::Null);
        }
        if v["v"] != 1 || self.scenario == "v2" {
            return bad(
                &mut s,
                "unsupported_version",
                "unsupported protocol",
                json!({"supported":if self.scenario=="v2"{vec![2]}else{vec![1]}}),
            );
        }
        if self.scenario == "hang" {
            std::thread::sleep(Duration::from_secs(20));
            return Ok(());
        }
        let op = v["op"].as_str().unwrap_or("");
        if !ops::IMPLEMENTED.contains(&op) || (op == "send" && self.scenario == "no_send") {
            return bad(
                &mut s,
                "unsupported_op",
                "unsupported operation",
                json!({"op":op,"ops":ops::IMPLEMENTED,"reserved":ops::RESERVED.contains(&op)}),
            );
        }
        if op != "send" {
            return self.write(
                &mut s,
                json!({"v":1,"id":id,"ok":true,"data":self.data(op=="hello")}),
            );
        }
        let a = &v["args"];
        if self.scenario == "rekey"
            || a["expect_session_id"]
                .as_str()
                .is_some_and(|id| id != self.id)
        {
            return bad(
                &mut s,
                "session_mismatch",
                "session moved",
                json!({"session_id":moved(&self.id)}),
            );
        }
        let text = a["text"].as_str().unwrap_or("");
        let deliver = a["deliver_as"].as_str().unwrap_or("");
        if text.trim().is_empty()
            || (a
                .get("job_id")
                .is_some_and(|v| !v.is_null() && !v.is_string()))
            || (a
                .get("expect_session_id")
                .is_some_and(|v| !v.is_null() && !v.is_string()))
            || text.len() > 524288
            || !["auto", "steer", "follow_up"].contains(&deliver)
        {
            return bad(&mut s, "bad_request", "invalid send arguments", Value::Null);
        }
        if self.scenario == "no_ack" {
            std::thread::sleep(Duration::from_secs(20));
            return Ok(());
        }
        let delivered = if self.scenario == "queued" {
            "follow_up"
        } else if self.scenario != "busy" {
            "prompt"
        } else if deliver == "steer" {
            "steer"
        } else {
            "follow_up"
        };
        self.write(&mut s,json!({"v":1,"id":id,"ok":true,"data":{"delivered_as":delivered,"session_id":self.id,"request_id":id}}))?;
        if self.scenario == "not_delivered" {
            return self.write(&mut s, json!({"v":1,"id":id,"event":"error","error":{"code":"not_delivered","message":"request removed before delivery"}}));
        }
        if self.scenario != "queued" {
            self.write(&mut s, json!({"v":1,"id":id,"event":"consumed"}))?;
        }
        if matches!(self.scenario.as_str(), "queued" | "never_settles") {
            std::thread::sleep(Duration::from_secs(30));
            return Ok(());
        }
        if self.scenario == "malformed" {
            return writeln!(s, "not json");
        }
        if self.scenario == "shutdown" {
            return self.write(&mut s,json!({"v":1,"id":id,"event":"error","error":{"code":"shutdown","message":"closed"}}));
        }
        if self.scenario == "moved" || self.scenario == "switched" {
            self.write(&mut s,json!({"v":1,"id":id,"event":"session_changed","from":self.id,"to":moved(&self.id),"reason":if self.scenario=="switched"{"new"}else{"moved"}}))?;
            if self.scenario == "switched" {
                return self.write(&mut s,json!({"v":1,"id":id,"event":"error","error":{"code":"session_switched","message":"switched"}}));
            }
        }
        if self.scenario == "slow" {
            std::thread::sleep(Duration::from_millis(self.delay));
        }
        let texts = if self.scenario == "progress" {
            vec![("working".into(), false), (format!("reply: {text}"), true)]
        } else {
            vec![(format!("reply: {text}"), true)]
        };
        for (text, is_final) in texts {
            let reply = Reply {
                v: 1,
                id: uuid::Uuid::now_v7().to_string(),
                session_id: self.id.clone(),
                job_id: a["job_id"].as_str().map(str::to_owned),
                request_id: id.clone(),
                tool_call_id: Some("tool".into()),
                text,
                is_final,
                at: jiff::Timestamp::now().to_string(),
            };
            let dir = self.home.ensure_dir("inbox")?;
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(format!("{}.ndjson", self.id)))?;
            fs::set_permissions(
                dir.join(format!("{}.ndjson", self.id)),
                fs::Permissions::from_mode(0o600),
            )?;
            writeln!(f, "{}", json!(reply))?;
            self.write(&mut s, json!({"v":1,"id":id,"event":"reply","reply":reply}))?;
        }
        if self.scenario == "crash" {
            std::process::exit(0)
        }
        let status = match self.scenario.as_str() {
            "aborted" => "aborted",
            "error" => "error",
            _ => "completed",
        };
        self.write(&mut s,json!({"v":1,"id":id,"event":"settled","status":status,"assistant_text":format!("pong: {text}"),"stop_reason":if status=="completed"{"stop"}else{status},"error_message":if status=="error"{Some("model error")}else{None},"warnings":if self.scenario=="batched"{vec![json!({"code":"batched","message":"shared reply span with j7654321"})]}else{vec![]}}))
    }
}
fn moved(id: &str) -> String {
    let mut s = id.to_owned();
    if s.len() >= 4 {
        s.truncate(s.len() - 4)
    }
    s.push_str("f0f0");
    s
}
fn main() -> io::Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let get = |name: &str| args.windows(2).find(|p| p[0] == name).map(|p| p[1].clone());
    let home = GlenHome {
        root: PathBuf::from(get("--home").ok_or_else(|| io::Error::other("--home required"))?),
    };
    let id = get("--session-id").ok_or_else(|| io::Error::other("--session-id required"))?;
    let socket = glen::live::run_dir(&home).join(format!("{}.sock", std::process::id()));
    let peer = Peer {
        home,
        id,
        file: get("--session-file").map(PathBuf::from),
        socket,
        scenario: std::env::var("FAKE_EXT_SCENARIO").unwrap_or_else(|_| "ok".into()),
        log: std::env::var_os("FAKE_EXT_LOG").map(PathBuf::from),
        version: std::env::var("FAKE_EXT_VERSION")
            .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into()),
        delay: std::env::var("FAKE_EXT_DELAY_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1500),
        mode: get("--mode").unwrap_or_else(|| "tui".into()),
    };
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(glen::live::run_dir(&peer.home))?;
    let listener = UnixListener::bind(&peer.socket)?;
    fs::set_permissions(&peer.socket, fs::Permissions::from_mode(0o600))?;
    let reg = Registry {
        v: 1,
        pid: std::process::id(),
        session_id: peer.id.clone(),
        session_file: peer.file.clone(),
        socket: peer.socket.clone(),
        ext_version: Some(peer.version.clone()),
        omp_version: Some("18.14.0".into()),
        protocol: Some(1),
        mode: Some(peer.mode.clone()),
        started_at: Some(jiff::Timestamp::now().to_string()),
        cwd: std::env::var("FAKE_EXT_CWD").ok(),
        title: None,
    };
    write_private_atomic(
        &registry::path(&peer.home, &peer.id),
        &serde_json::to_vec(&reg)?,
    )?;
    let stop = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, stop.clone())?;
    listener.set_nonblocking(true)?;
    println!("ready");
    io::stdout().flush()?;
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let peer = peer.clone();
                std::thread::spawn(move || {
                    if let Err(e) = peer.serve(stream) {
                        eprintln!("fake_ext peer: {e}");
                    }
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(e) => return Err(e),
        }
    }
    if std::env::var("FAKE_EXT_KEEP_FILES").as_deref() != Ok("1") {
        if let Err(e) = registry::remove_if_unchanged(&peer.home, &reg, false) {
            eprintln!("{e}");
        }
        fs::remove_file(&peer.socket)?;
    }
    Ok(())
}
