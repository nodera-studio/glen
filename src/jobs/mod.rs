pub mod liveness;
pub mod lock;
pub mod wait;
use crate::{
    error::{ErrorCode, GlenError, Warning},
    glen_home::{GlenHome, write_private_atomic},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct JobId(String);
impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}
impl JobId {
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7().to_string())
    }
    pub fn parse(s: &str) -> Option<Self> {
        if !(s.len() == 32 || s.len() == 36) {
            return None;
        }
        uuid::Uuid::parse_str(s).ok().map(|u| Self(u.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn simple(&self) -> String {
        self.0.replace('-', "")
    }
    pub fn handle(&self) -> String {
        format!("j{}", &self.simple()[25..])
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    Rpc,
    New,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Deliver {
    Auto,
    Steer,
    FollowUp,
}
impl From<crate::cli::DeliverAs> for Deliver {
    fn from(d: crate::cli::DeliverAs) -> Self {
        match d {
            crate::cli::DeliverAs::Auto => Self::Auto,
            crate::cli::DeliverAs::Steer => Self::Steer,
            crate::cli::DeliverAs::FollowUp => Self::FollowUp,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub v: u32,
    pub id: String,
    pub handle: String,
    pub created_at: String,
    pub route: Route,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_handle: Option<String>,
    pub deliver_as: Deliver,
    pub text: String,
    pub cwd: PathBuf,
    pub omp_bin: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_flag: Option<String>,
    pub owners_dir: PathBuf,
    pub job_timeout_ms: u64,
    pub deadline_at: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Queued,
    Accepted,
    Completed,
    Aborted,
    Failed,
    Lost,
}
impl State {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Accepted)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Accepted => "accepted",
            Self::Completed => "completed",
            Self::Aborted => "aborted",
            Self::Failed => "failed",
            Self::Lost => "lost",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Forked {
    pub from: String,
    pub to: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Status {
    pub v: u32,
    pub status: State,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omp_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_session_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_as: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waited_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub waiting_for_lock: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked: Option<Forked>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JobError>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reply {
    pub v: u32,
    pub id: String,
    pub session_id: String,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    pub text: String,
    #[serde(rename = "final")]
    pub is_final: bool,
    pub at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobResult {
    pub v: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_status: Option<String>,
    pub agent_invoked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_text: Option<String>,
    pub replies: Vec<Reply>,
    pub command_output: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    pub session_settled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<String>,
}
#[derive(Clone, Debug)]
pub struct JobStore {
    pub home: GlenHome,
    root: PathBuf,
}
impl JobStore {
    pub fn new(home: GlenHome) -> Self {
        let root = home.root.join("jobs");
        Self { home, root }
    }
    pub fn dir(&self, id: &JobId) -> PathBuf {
        self.root.join(id.as_str())
    }
    pub fn exists(&self) -> bool {
        self.root.is_dir()
    }
    pub fn create(&self, job: &Job) -> Result<(), GlenError> {
        self.create_mapped(job, |path, e| {
            GlenError::new(
                ErrorCode::Io,
                format!("cannot write {}: {e}", path.display()),
            )
        })
    }
    pub fn create_for_send(&self, job: &Job, env: &crate::env::Env) -> Result<(), GlenError> {
        self.create_mapped(job, |path, e| {
            let mut error = crate::sandbox::map_home_write(env, "send", e);
            error.message = format!(
                "{} (job {} at {})",
                error.message,
                job.handle,
                path.display()
            );
            error
        })
    }
    fn create_mapped(
        &self,
        job: &Job,
        map: impl Fn(&Path, io::Error) -> GlenError,
    ) -> Result<(), GlenError> {
        let id = JobId::parse(&job.id)
            .ok_or_else(|| GlenError::new(ErrorCode::Internal, "invalid job id"))?;
        self.home
            .ensure_dir("jobs")
            .map_err(|e| map(&self.root, e))?;
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        let dir = self.dir(&id);
        builder.create(&dir).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                GlenError::new(
                    ErrorCode::Internal,
                    format!("cannot create job {} at {}: {e}", job.handle, dir.display()),
                )
            } else {
                map(&dir, e)
            }
        })?;
        write_json_mapped(&dir.join("job.json"), job, &map)?;
        let _lock = self.status_lock_mapped(&id, &map)?;
        write_json_mapped(
            &dir.join("status.json"),
            &Status {
                v: 1,
                status: State::Queued,
                updated_at: job.created_at.clone(),
                ..Status::default()
            },
            &map,
        )
    }
    pub fn read_job(&self, id: &JobId) -> Result<Job, GlenError> {
        read_json(&self.dir(id).join("job.json"))
    }
    pub fn read_status(&self, id: &JobId) -> Result<Status, GlenError> {
        read_json(&self.dir(id).join("status.json"))
    }
    pub fn read_result(&self, id: &JobId) -> Result<JobResult, GlenError> {
        read_json(&self.dir(id).join("result.json"))
    }
    pub fn write_status(&self, id: &JobId, s: &Status) -> Result<(), GlenError> {
        let _lock = self.status_lock(id)?;
        write_json(&self.dir(id).join("status.json"), s)
    }
    pub(crate) fn status_lock(&self, id: &JobId) -> Result<fs::File, GlenError> {
        self.status_lock_mapped(id, &|_, e| GlenError::from(e))
    }
    fn status_lock_mapped(
        &self,
        id: &JobId,
        map: &impl Fn(&Path, io::Error) -> GlenError,
    ) -> Result<fs::File, GlenError> {
        let path = self.dir(id).join("status.lock");
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(&path).map_err(|e| {
            map(
                &path,
                io::Error::new(
                    e.kind(),
                    format!("cannot open {} for writing: {e}", path.display()),
                ),
            )
        })?;
        file.lock().map_err(|e| {
            map(
                &path,
                io::Error::new(e.kind(), format!("cannot lock {}: {e}", path.display())),
            )
        })?;
        Ok(file)
    }
    pub(crate) fn write_status_locked(&self, id: &JobId, s: &Status) -> Result<(), GlenError> {
        write_json(&self.dir(id).join("status.json"), s)
    }
    pub fn write_result(&self, id: &JobId, r: &JobResult) -> Result<(), GlenError> {
        write_json(&self.dir(id).join("result.json"), r)
    }
    pub fn append_reply(&self, id: &JobId, r: &Reply) -> io::Result<()> {
        append_line(
            &self.dir(id).join("replies.ndjson"),
            &serde_json::to_vec(r)?,
        )
    }
    pub fn read_replies(&self, id: &JobId) -> Vec<Reply> {
        fs::read_to_string(self.dir(id).join("replies.ndjson"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
    pub fn list(&self) -> Vec<JobId> {
        let mut ids: Vec<_> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().to_str().and_then(JobId::parse))
            .collect();
        ids.sort_by(|a, b| b.cmp(a));
        ids
    }
    pub fn resolve(&self, arg: &str) -> Result<JobId, GlenError> {
        let arg = arg.to_ascii_lowercase();
        let matches: Vec<_> = if let Some(id) = JobId::parse(&arg) {
            self.list().into_iter().filter(|i| *i == id).collect()
        } else if let Some(tail) = arg
            .strip_prefix('j')
            .filter(|s| s.len() >= 7 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            self.list()
                .into_iter()
                .filter(|i| i.simple().ends_with(tail))
                .collect()
        } else {
            return Err(
                GlenError::new(ErrorCode::Usage, format!("`{arg}` is not a job handle"))
                    .with_hint("job handles look like j1a2b3c4; run `glen jobs`"),
            );
        };
        match matches.as_slice() {
            [id] => Ok(id.clone()),
            [] => Err(GlenError::new(
                ErrorCode::NotFound,
                format!("no glen job matches `{arg}`"),
            )),
            _ => {
                let candidates = matches.iter().map(|id| {
                    let job = self.read_job(id).ok();
                    let status = self.read_status(id).ok();
                    json!({"id":id.as_str(),"handle":id.handle(),"status":status.map(|s|s.status),"created_at":job.map(|j|j.created_at)})
                }).collect::<Vec<_>>();
                Err(GlenError::new(
                    ErrorCode::Ambiguous,
                    format!("several glen jobs match `{arg}`"),
                )
                .with_details(json!({"candidates":candidates})))
            }
        }
    }
    pub fn prune(&self, now: jiff::Timestamp) -> usize {
        self.list()
            .into_iter()
            .filter(|id| {
                self.read_status(id).is_ok_and(|s| {
                    s.status.is_terminal()
                        && s.updated_at.parse::<jiff::Timestamp>().is_ok_and(|t| {
                            now.as_millisecond().saturating_sub(t.as_millisecond())
                                > 7 * 86400 * 1000
                        })
                })
            })
            .filter(|id| fs::remove_dir_all(self.dir(id)).is_ok())
            .count()
    }
}
fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, GlenError> {
    let bytes = fs::read(path).map_err(|e| {
        GlenError::new(
            if e.kind() == io::ErrorKind::NotFound {
                ErrorCode::NotFound
            } else {
                ErrorCode::Io
            },
            format!("{}: {e}", path.display()),
        )
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|e| GlenError::new(ErrorCode::Internal, format!("{}: {e}", path.display())))
}
pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), GlenError> {
    write_json_mapped(path, value, &|path, e| {
        GlenError::new(
            ErrorCode::Io,
            format!("cannot write {}: {e}", path.display()),
        )
    })
}
fn write_json_mapped<T: Serialize>(
    path: &Path,
    value: &T,
    map: &impl Fn(&Path, io::Error) -> GlenError,
) -> Result<(), GlenError> {
    let mut b = serde_json::to_vec_pretty(value).map_err(|e| {
        GlenError::new(
            ErrorCode::Internal,
            format!("cannot serialize {}: {e}", path.display()),
        )
    })?;
    b.push(b'\n');
    write_private_atomic(path, &b).map_err(|e| map(path, e))
}
pub(crate) fn append_line(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut opts = OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut b = bytes.to_vec();
    b.push(b'\n');
    opts.open(path)
        .and_then(|mut f| f.write_all(&b))
        .map_err(|e| io::Error::new(e.kind(), format!("cannot append {}: {e}", path.display())))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub fn job() -> Job {
        let id = JobId::new();
        Job {
            v: 1,
            id: id.as_str().into(),
            handle: id.handle(),
            created_at: jiff::Timestamp::now().to_string(),
            route: Route::Rpc,
            session_id: Some("01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de".into()),
            session_file: Some("/fixture/session.jsonl".into()),
            session_handle: Some("e27c0de".into()),
            deliver_as: Deliver::Auto,
            text: "hello".into(),
            cwd: "/fixture".into(),
            omp_bin: "/bin/fake_omp".into(),
            profile_flag: None,
            owners_dir: "/fixture/owners".into(),
            job_timeout_ms: 30000,
            deadline_at: jiff::Timestamp::now().to_string(),
        }
    }
    pub fn store(root: &Path) -> JobStore {
        JobStore::new(GlenHome {
            root: root.join("glen"),
        })
    }
    fn roundtrip<T: Serialize + DeserializeOwned>(v: &T) {
        let encoded = serde_json::to_value(v).unwrap();
        let decoded: T = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
    }
    #[test]
    fn ids_and_handles() {
        let id = JobId::parse("01A0F6B1-2680-7A3C-8B21-4A1F9E27C0DE").unwrap();
        assert_eq!(id.as_str(), "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de");
        assert_eq!(id.handle(), "je27c0de");
        assert_eq!(JobId::parse(&id.simple()), Some(id));
        assert!(JobId::parse("not-a-uuid").is_none());
        assert!(JobId::parse("{01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de}").is_none());
        assert_eq!(
            uuid::Uuid::parse_str(JobId::new().as_str())
                .unwrap()
                .get_version_num(),
            7
        );
    }
    #[test]
    fn reply_serialization_keeps_absent_fields_as_null() {
        let reply = Reply {
            v: 1,
            id: "reply".into(),
            session_id: "session".into(),
            job_id: None,
            request_id: None,
            tool_call_id: None,
            text: "hello".into(),
            is_final: false,
            at: "2026-10-30T00:00:00Z".into(),
        };
        let encoded = serde_json::to_value(&reply).unwrap();
        assert_eq!(
            encoded,
            json!({
                "v":1,"id":"reply","session_id":"session","job_id":null,
                "request_id":null,"tool_call_id":null,"text":"hello","final":false,
                "at":"2026-10-30T00:00:00Z"
            })
        );
        let mut legacy = encoded;
        for key in ["job_id", "request_id", "tool_call_id"] {
            legacy.as_object_mut().unwrap().remove(key);
        }
        let decoded: Reply = serde_json::from_value(legacy).unwrap();
        assert!(decoded.job_id.is_none());
        assert!(decoded.request_id.is_none());
        assert!(decoded.tool_call_id.is_none());
    }
    #[test]
    fn schemas_roundtrip_and_lenient_status() {
        roundtrip(&job());
        for route in [Route::New, Route::Rpc] {
            roundtrip(&route);
        }
        for d in [Deliver::Auto, Deliver::Steer, Deliver::FollowUp] {
            roundtrip(&d);
        }
        let error = JobError {
            code: "timeout".into(),
            message: "deadline".into(),
            hint: Some("retry".into()),
            details: Some(json!({"retryable":true})),
        };
        let fork = Forked {
            from: "one".into(),
            to: "two".into(),
        };
        roundtrip(&error);
        roundtrip(&fork);
        for state in [
            State::Queued,
            State::Accepted,
            State::Completed,
            State::Aborted,
            State::Failed,
            State::Lost,
        ] {
            roundtrip(&state);
            assert_eq!(
                state.is_terminal(),
                !matches!(state, State::Queued | State::Accepted)
            );
            let s = Status {
                v: 1,
                status: state,
                updated_at: jiff::Timestamp::now().to_string(),
                worker_pid: Some(1),
                worker_start: Some("start".into()),
                omp_pid: Some(2),
                request_id: Some("r".into()),
                actual_session_id: Some("actual".into()),
                actual_session_file: Some("/actual.jsonl".into()),
                delivered_as: Some("prompt".into()),
                waited_ms: Some(15),
                waiting_for_lock: true,
                forked: Some(fork.clone()),
                error: Some(error.clone()),
                warnings: vec![Warning {
                    code: "test".into(),
                    message: "warning".into(),
                }],
            };
            roundtrip(&s);
        }
        let s:Status=serde_json::from_value(json!({"v":1,"status":"queued","updated_at":"2026-10-30T00:00:00Z","future":{"anything":1}})).unwrap();
        assert!(s.worker_pid.is_none());
        assert!(s.warnings.is_empty());
        let reply = Reply {
            v: 1,
            id: JobId::new().as_str().into(),
            session_id: "session".into(),
            job_id: None,
            request_id: None,
            tool_call_id: Some("tool".into()),
            text: "reply".into(),
            is_final: true,
            at: jiff::Timestamp::now().to_string(),
        };
        roundtrip(&reply);
        assert_eq!(serde_json::to_value(&reply).unwrap()["final"], true);
        roundtrip(&JobResult {
            v: 1,
            prompt_status: Some("completed".into()),
            agent_invoked: true,
            assistant_text: Some("pong".into()),
            replies: vec![reply],
            command_output: vec!["output".into()],
            stop_reason: Some("stop".into()),
            session_settled: true,
            settled_at: Some(jiff::Timestamp::now().to_string()),
        });
    }
    #[test]
    fn private_creation_and_existing_directory_refused() {
        let t = tempfile::tempdir().unwrap();
        let store = store(t.path());
        assert!(!store.exists());
        assert!(!store.home.root.exists());
        let j = job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        assert_eq!(store.read_job(&id).unwrap().text, "hello");
        assert_eq!(store.read_status(&id).unwrap().status, State::Queued);
        assert_eq!(store.create(&j).unwrap_err().code, ErrorCode::Internal);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for (p, mode) in [
                (store.dir(&id), 0o700),
                (store.dir(&id).join("job.json"), 0o600),
                (store.dir(&id).join("status.json"), 0o600),
            ] {
                assert_eq!(fs::metadata(p).unwrap().permissions().mode() & 0o777, mode);
            }
        }
    }
    #[test]
    fn review12_write_and_lock_errors_name_the_path() {
        let t = tempfile::tempdir().unwrap();
        let store = store(t.path());
        let id = JobId::new();
        let path = store.dir(&id).join("result.json");
        let error = write_json(&path, &json!({})).unwrap_err();
        assert!(error.message.contains(&path.display().to_string()));
        let error = store.write_status(&id, &Status::default()).unwrap_err();
        assert!(error.message.contains("status.lock"));
        fs::write(&store.home.root, "blocked").unwrap();
        let error = store.create(&job()).unwrap_err();
        assert!(error.message.contains(&store.root.display().to_string()));
    }
    #[test]
    fn read_errors_name_the_file() {
        let t = tempfile::tempdir().unwrap();
        let store = store(t.path());
        let j = job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        assert_eq!(
            store.read_result(&id).unwrap_err().code,
            ErrorCode::NotFound
        );
        fs::write(store.dir(&id).join("status.json"), b"broken").unwrap();
        let e = store.read_status(&id).unwrap_err();
        assert_eq!(e.code, ErrorCode::Internal);
        assert!(e.message.contains("status.json"));
    }
    #[test]
    fn resolves_full_simple_suffix_uppercase_and_collisions() {
        let t = tempfile::tempdir().unwrap();
        let store = store(t.path());
        for raw in [
            "01a00000-0000-7000-8000-000001234567",
            "01a00001-0000-7000-8000-000001234567",
        ] {
            let mut j = job();
            j.id = raw.into();
            j.handle = JobId::parse(raw).unwrap().handle();
            store.create(&j).unwrap();
        }
        let id = store.list()[0].clone();
        for arg in [
            id.as_str().to_owned(),
            id.simple(),
            id.as_str().to_uppercase(),
            format!("j{}", id.simple()),
        ] {
            assert_eq!(store.resolve(&arg).unwrap(), id);
        }
        let e = store.resolve("J1234567").unwrap_err();
        assert_eq!(e.code, ErrorCode::Ambiguous);
        let candidates = &e.details.unwrap()["candidates"];
        assert_eq!(candidates.as_array().unwrap().len(), 2);
        assert!(candidates[0]["created_at"].is_string());
        for arg in ["abc1234", "j123", "jx1234567"] {
            assert_eq!(store.resolve(arg).unwrap_err().code, ErrorCode::Usage);
        }
        for arg in ["jfffffff", "ffffffff-ffff-7fff-8fff-ffffffffffff"] {
            assert_eq!(store.resolve(arg).unwrap_err().code, ErrorCode::NotFound);
        }
    }
    #[test]
    fn retention_only_old_terminal_jobs() {
        let t = tempfile::tempdir().unwrap();
        let store = store(t.path());
        let now = jiff::Timestamp::now();
        let mut expected = vec![];
        for (state, days, keep) in [
            (State::Completed, 8, false),
            (State::Lost, 8, false),
            (State::Accepted, 8, true),
            (State::Failed, 6, true),
        ] {
            let j = job();
            let id = JobId::parse(&j.id).unwrap();
            store.create(&j).unwrap();
            let mut s = store.read_status(&id).unwrap();
            s.status = state;
            s.updated_at = now
                .checked_sub(jiff::SignedDuration::from_hours(24 * days))
                .unwrap()
                .to_string();
            store.write_status(&id, &s).unwrap();
            expected.push((id, keep));
        }
        let corrupt = job();
        store.create(&corrupt).unwrap();
        let id = JobId::parse(&corrupt.id).unwrap();
        fs::write(store.dir(&id).join("status.json"), b"broken").unwrap();
        assert_eq!(store.prune(now), 2);
        for (id, keep) in expected {
            assert_eq!(store.dir(&id).exists(), keep);
        }
        assert!(store.dir(&id).exists());
    }
    #[test]
    fn append_replies_and_skip_bad_lines() {
        let t = tempfile::tempdir().unwrap();
        let store = store(t.path());
        let j = job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        let mut r = Reply {
            v: 1,
            id: "one".into(),
            session_id: "session".into(),
            job_id: Some(j.id),
            request_id: None,
            tool_call_id: None,
            text: "first".into(),
            is_final: false,
            at: j.created_at,
        };
        store.append_reply(&id, &r).unwrap();
        append_line(&store.dir(&id).join("replies.ndjson"), b"broken").unwrap();
        r.id = "two".into();
        r.text = "second".into();
        store.append_reply(&id, &r).unwrap();
        let rs = store.read_replies(&id);
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0].text, "first");
        assert_eq!(rs[1].text, "second");
        assert_eq!(
            fs::read_to_string(store.dir(&id).join("replies.ndjson"))
                .unwrap()
                .lines()
                .count(),
            3
        );
    }
}
