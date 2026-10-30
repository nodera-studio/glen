//! Scriptable extension peer; never opens an omp session file.
use glen::{
    glen_home::{GlenHome, write_private_atomic},
    jobs::Reply,
    live::registry::{self, Registry},
    ops::{self, Op, OpError},
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
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
// Retain the historical W3 capability set for compatibility tests.
const W3_OPS: &[&str] = &["hello", "state", "send"];
const W3_RESERVED: &[&str] = &[
    "abort",
    "list_models",
    "set_model",
    "set_thinking_level",
    "compact",
];
struct Selection {
    model: omp_rpc::ModelInfo,
    thinking: String,
}
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
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
    old_ops: bool,
    models: Arc<Vec<omp_rpc::ModelInfo>>,
    selection: Arc<Mutex<Selection>>,
    active: Arc<AtomicUsize>,
    abort_generation: Arc<AtomicU64>,
    compacting: Arc<AtomicBool>,
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
    fn supported(&self) -> &[&str] {
        if self.old_ops {
            W3_OPS
        } else {
            ops::IMPLEMENTED
        }
    }
    fn data(&self, hello: bool) -> Value {
        let busy = matches!(self.scenario.as_str(), "busy" | "queued")
            || self.active.load(Ordering::SeqCst) > 0;
        let mut d = json!({"ext_version":self.version,"omp_version":"18.14.0","pid":std::process::id(),"session_id":if self.scenario=="mismatch"{moved(&self.id)}else{self.id.clone()},"session_file":self.file,"mode":self.mode,"state":if busy{"busy"}else{"idle"},"busy":busy,"queued":self.scenario=="queued","active_requests":0});
        if hello {
            d["protocol"] = json!(1);
            d["protocols"] = json!([1]);
            d["ops"] = if self.scenario == "no_send" {
                json!(["hello", "state"])
            } else {
                json!(self.supported())
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
        if !self.supported().contains(&op) || (op == "send" && self.scenario == "no_send") {
            return bad(
                &mut s,
                "unsupported_op",
                "unsupported operation",
                json!({"op":op,"ops":self.supported(),"reserved":self.old_ops&&W3_RESERVED.contains(&op)}),
            );
        }
        if ops::CONTROL_OPS.contains(&op) {
            let result = self.control(op, v.get("args"));
            return match result {
                Ok(data) => self.write(&mut s, json!({"v":1,"id":id,"ok":true,"data":data})),
                Err(error) => self.write(&mut s, json!({"v":1,"id":id,"ok":false,"error":error})),
            };
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
        let generation = self.abort_generation.load(Ordering::SeqCst);
        let _active = if self.scenario != "queued" {
            self.active.fetch_add(1, Ordering::SeqCst);
            Some(Active(self.active.clone()))
        } else {
            None
        };
        self.write(&mut s,json!({"v":1,"id":id,"ok":true,"data":{"delivered_as":delivered,"session_id":self.id,"request_id":id}}))?;
        if self.scenario == "not_delivered" {
            return self.write(&mut s, json!({"v":1,"id":id,"event":"error","error":{"code":"not_delivered","message":"request removed before delivery"}}));
        }
        if self.scenario != "queued" {
            self.write(&mut s, json!({"v":1,"id":id,"event":"consumed"}))?;
        }
        if matches!(self.scenario.as_str(), "queued" | "never_settles" | "slow") {
            let end = std::time::Instant::now()
                + Duration::from_millis(if self.scenario == "slow" {
                    self.delay
                } else {
                    30_000
                });
            while std::time::Instant::now() < end {
                if self.scenario != "queued"
                    && self.abort_generation.load(Ordering::SeqCst) != generation
                {
                    return self.write(&mut s,json!({"v":1,"id":id,"event":"settled","status":"aborted","assistant_text":null,"stop_reason":"aborted","error_message":null}));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            if self.scenario != "slow" {
                return Ok(());
            }
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
    fn control(&self, name: &str, args: Option<&Value>) -> Result<Value, OpError> {
        let op = ops::decode(name, args)?;
        if ops::expect_session_id(args).is_some_and(|expected| expected != self.id) {
            return Err(OpError::new(ops::SESSION_MISMATCH, "session moved")
                .with_details(json!({"session_id":self.id})));
        }
        let mut data = match op {
            Op::Abort => {
                let busy = self.active.load(Ordering::SeqCst) > 0
                    || self.compacting.load(Ordering::SeqCst)
                    || self.scenario == "busy";
                let stuck = std::env::var("FAKE_EXT_ABORT_STUCK").as_deref() == Ok("1");
                if busy {
                    if stuck {
                        std::thread::sleep(Duration::from_secs(3));
                    } else {
                        self.abort_generation.fetch_add(1, Ordering::SeqCst);
                        let end = std::time::Instant::now() + Duration::from_secs(3);
                        while (self.active.load(Ordering::SeqCst) > 0
                            || self.compacting.load(Ordering::SeqCst))
                            && std::time::Instant::now() < end
                        {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
                json!({"was":if busy {"busy"}else{"idle"},"aborted":busy,"idle_after":!stuck,"queued":self.scenario=="queued"})
            }
            Op::ListModels => {
                let selected = self
                    .selection
                    .lock()
                    .map_err(|e| OpError::new(ops::INTERNAL, e.to_string()))?;
                json!({"current":{"provider":selected.model.provider,"id":selected.model.id},"thinking_level":selected.thinking,"models":self.models.iter().map(glen::ctl::row_from_info).collect::<Vec<_>>()})
            }
            Op::SetModel(a) => {
                let model = self
                    .models
                    .iter()
                    .find(|m| m.provider == a.provider && m.id == a.model_id)
                    .ok_or_else(|| OpError::new(ops::MODEL_NOT_FOUND, "Model not found"))?;
                if model.provider == "nokey"
                    || std::env::var("FAKE_EXT_NO_AUTH").ok().as_deref()
                        == Some(model.provider.as_str())
                {
                    return Err(OpError::new(
                        ops::NO_AUTH,
                        format!("No API key for {}/{}", model.provider, model.id),
                    ));
                }
                let mut selected = self
                    .selection
                    .lock()
                    .map_err(|e| OpError::new(ops::INTERNAL, e.to_string()))?;
                let previous = json!({"provider":selected.model.provider,"id":selected.model.id});
                selected.model = model.clone();
                if !model.reasoning {
                    selected.thinking = "off".into();
                }
                json!({"model":{"provider":model.provider,"id":model.id,"name":model.name},"previous":previous,"thinking_level":selected.thinking,"thinking_levels":glen::ctl::row_from_info(model).thinking_levels,"busy":self.data(false)["busy"]})
            }
            Op::SetThinkingLevel(a) => {
                let mut selected = self
                    .selection
                    .lock()
                    .map_err(|e| OpError::new(ops::INTERNAL, e.to_string()))?;
                let available = glen::ctl::row_from_info(&selected.model).thinking_levels;
                let model = json!({"provider":selected.model.provider,"id":selected.model.id});
                if !available.contains(&a.level) {
                    return Err(
                        OpError::new(ops::INVALID_LEVEL, "unsupported thinking level")
                            .with_details(json!({"available":available,"model":model})),
                    );
                }
                let previous = std::mem::replace(&mut selected.thinking, a.level);
                json!({"thinking_level":selected.thinking,"previous":previous,"model":model,"available":available,"busy":self.data(false)["busy"]})
            }
            Op::Compact(_) => {
                if self.data(false)["busy"] == true
                    || self.scenario == "queued"
                    || self.compacting.swap(true, Ordering::SeqCst)
                {
                    return Err(OpError::new(ops::BUSY, "session is busy"));
                }
                let generation = self.abort_generation.load(Ordering::SeqCst);
                let delay = std::env::var("FAKE_EXT_COMPACT_DELAY_MS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let end = std::time::Instant::now() + Duration::from_millis(delay);
                while std::time::Instant::now() < end {
                    if self.abort_generation.load(Ordering::SeqCst) != generation {
                        self.compacting.store(false, Ordering::SeqCst);
                        return Err(OpError::new(ops::OMP_ERROR, "compaction aborted"));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                self.compacting.store(false, Ordering::SeqCst);
                json!({"tokens_before":1234,"tokens_after":123})
            }
            _ => return Err(OpError::new(ops::UNSUPPORTED_OP, "unsupported control")),
        };
        data["session_id"] = json!(if std::env::var("FAKE_EXT_POST_ID").as_deref() == Ok("1") {
            moved(&self.id)
        } else {
            self.id.clone()
        });
        Ok(data)
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
    let models: Vec<omp_rpc::ModelInfo> = serde_json::from_str(include_str!("fake_models.json"))?;
    let selected = models
        .first()
        .ok_or_else(|| io::Error::other("empty models fixture"))?
        .clone();
    let peer = Peer {
        old_ops: std::env::var("FAKE_EXT_OPS").as_deref() == Ok("w3"),
        models: Arc::new(models),
        selection: Arc::new(Mutex::new(Selection {
            model: selected,
            thinking: "medium".into(),
        })),
        active: Arc::new(AtomicUsize::new(0)),
        abort_generation: Arc::new(AtomicU64::new(0)),
        compacting: Arc::new(AtomicBool::new(false)),
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
