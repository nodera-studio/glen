use super::{
    handle,
    session::{Session, TurnStatus, display_name, text_of},
};
use crate::error::{ErrorCode, GlenError};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};
#[derive(Debug, Clone)]
pub struct SessionFile {
    pub path: PathBuf,
    pub bucket: String,
    pub mtime: SystemTime,
    pub size: u64,
}
impl SessionFile {
    pub fn at(path: &Path) -> io::Result<Self> {
        Self::from_metadata(path, &fs::metadata(path)?)
    }
    pub fn from_metadata(path: &Path, m: &fs::Metadata) -> io::Result<Self> {
        Ok(Self {
            path: path.into(),
            bucket: path
                .parent()
                .and_then(Path::file_name)
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            mtime: m.modified()?,
            size: m.len(),
        })
    }
}
pub fn discover(dir: &Path) -> io::Result<Vec<SessionFile>> {
    let buckets = match fs::read_dir(dir) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut seen = HashSet::new();
    let mut out = vec![];
    for b in buckets.flatten() {
        if b.file_name().to_string_lossy().starts_with('.')
            || !b.file_type().is_ok_and(|t| t.is_dir())
        {
            continue;
        }
        let Ok(files) = fs::read_dir(b.path()) else {
            continue;
        };
        for f in files.flatten() {
            let name = f.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || !name.ends_with(".jsonl") {
                continue;
            }
            let path = f.path();
            if !fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                continue;
            }
            let real = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if seen.insert(real)
                && let Ok(file) = SessionFile::at(&path)
            {
                out.push(file)
            }
        }
    }
    Ok(out)
}
#[derive(Debug, Clone)]
pub struct Summary {
    pub file: SessionFile,
    pub id: String,
    pub cwd: Option<String>,
    pub created: Option<jiff::Timestamp>,
    pub title: Option<String>,
    pub first_message: Option<String>,
    pub parent_session: Option<String>,
    pub continuation_of: Option<String>,
    pub last_turn: TurnStatus,
    pub in_flight: bool,
    pub model: Option<String>,
    pub tail_has_message: bool,
    pub tail_redirects_leaf: bool,
}
pub fn scan_head(file: SessionFile) -> Option<Summary> {
    let mut f = File::open(&file.path).ok()?;
    let mut b = vec![];
    Read::by_ref(&mut f).take(4096).read_to_end(&mut b).ok()?;
    let parse = |b: &[u8]| {
        let end = b.iter().rposition(|v| *v == b'\n')?;
        Session::parse(&file.path, &b[..=end]).ok()
    };
    let s = match parse(&b) {
        Some(s) => s,
        None => {
            Read::by_ref(&mut f)
                .take(65536 - 4096)
                .read_to_end(&mut b)
                .ok()?;
            parse(&b)?
        }
    };
    Some(summarize(file, &s))
}
pub fn summarize(file: SessionFile, s: &Session) -> Summary {
    let (title, first_message) = titles(s);
    Summary {
        file,
        id: s.header.id.clone(),
        cwd: s.header.cwd.clone(),
        created: s.header.timestamp.as_deref().and_then(|s| s.parse().ok()),
        title,
        first_message,
        parent_session: s.header.parent_session.clone(),
        continuation_of: s.header.continuation_of.clone(),
        last_turn: TurnStatus::Unknown,
        in_flight: false,
        model: None,
        tail_has_message: false,
        tail_redirects_leaf: false,
    }
}
/// Shared title resolution for the bounded listing head and full snapshots.
pub fn titles(s: &Session) -> (Option<String>, Option<String>) {
    let title = s.title().or_else(|| {
        s.entries
            .iter()
            .rev()
            .find(|e| e.kind == "compaction")
            .and_then(|e| e.raw["shortSummary"].as_str())
            .and_then(display_name)
    });
    let first_message = s
        .entries
        .iter()
        .find(|e| e.kind == "message" && e.raw["message"]["role"] == "user")
        .and_then(|e| display_name(&text_of(&e.raw["message"]["content"], " ")))
        .map(|s| s.chars().take(200).collect());
    (title, first_message)
}
pub fn model_of(message: &serde_json::Value) -> Option<String> {
    Some(format!(
        "{}/{}",
        message["provider"].as_str()?,
        message["model"].as_str()?
    ))
}
pub fn scan_tail(s: &mut Summary) -> io::Result<()> {
    let mut f = File::open(&s.file.path)?;
    let size = f.metadata()?.len();
    let offset = size.saturating_sub(32768);
    f.seek(SeekFrom::Start(offset))?;
    let mut b = vec![];
    f.read_to_end(&mut b)?;
    let start = if offset > 0 {
        b.iter()
            .position(|v| *v == b'\n')
            .map_or(b.len(), |i| i + 1)
    } else {
        0
    };
    let end = b.iter().rposition(|v| *v == b'\n').map_or(start, |i| i + 1);
    let mut found = false;
    s.tail_redirects_leaf = false;
    let mut newest_entry = true;
    let mut next_parent = None;
    let mut changed = None;
    for line in b[start..end].split(|v| *v == b'\n').rev() {
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if v["type"].is_string() && v["id"].is_string() && !found {
            if newest_entry {
                s.tail_redirects_leaf = v.get("activeLeafId").is_some_and(|leaf| leaf != &v["id"]);
                newest_entry = false;
            } else if next_parent.as_deref() != v["id"].as_str() {
                // The leaf's parent chain skipped the physical last message,
                // or the chain cannot be resolved inside this tail window.
                s.tail_redirects_leaf = true;
            }
            next_parent = v["parentId"].as_str().map(str::to_owned);
        }
        if v["type"] == "message" {
            let m = &v["message"];
            if !found {
                s.last_turn = TurnStatus::of_message(m);
                s.in_flight = super::session::in_flight(m);
                found = true
            }
            if s.model.is_none() && m["role"] == "assistant" {
                s.model = model_of(m)
            }
        } else if v["type"] == "model_change" && changed.is_none() {
            changed = v["model"].as_str().map(str::to_owned)
        }
    }
    if s.model.is_none() {
        s.model = changed
    }
    s.tail_has_message = found;
    Ok(())
}
pub fn parse_session_file_name(name: &str) -> Option<(jiff::Timestamp, String)> {
    let (date, id) = name.strip_suffix(".jsonl")?.split_once('_')?;
    let b = date.as_bytes();
    if b.len() != 24
        || ![4, 7, 13, 16, 19].iter().all(|i| b[*i] == b'-')
        || b[10] != b'T'
        || b[23] != b'Z'
        || !b
            .iter()
            .enumerate()
            .all(|(i, c)| [4, 7, 10, 13, 16, 19, 23].contains(&i) || c.is_ascii_digit())
        || id.is_empty()
    {
        return None;
    }
    let timestamp = format!(
        "{}:{}:{}.{}Z",
        &date[..13],
        &date[14..16],
        &date[17..19],
        &date[20..23]
    );
    Some((timestamp.parse().ok()?, id.into()))
}
pub fn sort_newest_first(v: &mut [Summary]) {
    v.sort_by(|a, b| {
        b.file
            .mtime
            .cmp(&a.file.mtime)
            .then(b.created.cmp(&a.created))
            .then(b.file.path.cmp(&a.file.path))
    })
}
pub struct Catalog {
    pub sessions_dir: PathBuf,
    pub sessions: Vec<Summary>,
    pub handles: HashMap<String, String>,
}
impl Catalog {
    pub fn scan(dir: &Path) -> Result<Self, GlenError> {
        let mut catalog = Self::read(dir)?;
        catalog.assign_handles(None);
        Ok(catalog)
    }
    pub(crate) fn assign_handles(&mut self, extra: Option<&str>) {
        self.handles = handle::assign(self.sessions.iter().map(|s| s.id.as_str()).chain(extra));
    }
    pub(crate) fn read(dir: &Path) -> Result<Self, GlenError> {
        let files = discover(dir).map_err(|e| {
            GlenError::new(ErrorCode::Io, format!("cannot read {}: {e}", dir.display()))
        })?;
        let mut sessions = files.into_iter().filter_map(scan_head).collect::<Vec<_>>();
        sort_newest_first(&mut sessions);
        Ok(Self {
            sessions_dir: dir.into(),
            sessions,
            handles: HashMap::new(),
        })
    }
    pub fn handle(&self, id: &str) -> String {
        self.handles
            .get(id)
            .cloned()
            .unwrap_or_else(|| handle::assign([id]).remove(id).unwrap_or_else(|| id.into()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::omp::fixtures;
    #[test]
    fn discovery_and_windows() {
        let t = fixtures::tree();
        let files = discover(t.path()).unwrap();
        assert_eq!(files.len(), 7);
        let mut summaries = files
            .into_iter()
            .map(|f| scan_head(f).unwrap())
            .collect::<Vec<_>>();
        for s in &mut summaries {
            scan_tail(s).unwrap();
            assert!(
                parse_session_file_name(s.file.path.file_name().unwrap().to_str().unwrap())
                    .is_some()
            );
        }
        let a = summaries
            .iter()
            .find(|s| s.id.ends_with("e27c0de"))
            .unwrap();
        assert_eq!(a.title.as_deref(), Some("Fix the flaky parser test"));
        assert_eq!(a.last_turn, TurnStatus::Complete);
        assert_eq!(a.model.as_deref(), Some("anthropic/claude-sonnet-4-5"));
        let b = summaries
            .iter()
            .find(|s| s.id.ends_with("f13a5b6"))
            .unwrap();
        assert_eq!(b.last_turn, TurnStatus::Pending);
        let c = summaries
            .iter()
            .find(|s| s.id.ends_with("b7e2d81"))
            .unwrap();
        assert_eq!(c.continuation_of.as_deref(), Some(b.id.as_str()));
        let g = summaries
            .iter()
            .find(|s| s.id.ends_with("b8e4a07"))
            .unwrap();
        assert_eq!(g.parent_session.as_deref(), Some(a.id.as_str()));
        assert!(g.continuation_of.is_none());
        assert_eq!(
            summaries
                .iter()
                .find(|s| s.id.ends_with("0a3f417"))
                .unwrap()
                .last_turn,
            TurnStatus::Complete
        );
        assert!(parse_session_file_name("foo.jsonl").is_none());
        assert!(parse_session_file_name("2026-99-01T00-00-00-000Z_x.jsonl").is_none());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks() {
        use std::os::unix::fs::symlink;
        let t = fixtures::tree();
        symlink(t.path().join("-work-alpha"), t.path().join("linked")).unwrap();
        let p = fixtures::path(&t, 'A');
        symlink(&p, p.parent().unwrap().join("duplicate.jsonl")).unwrap();
        assert_eq!(discover(t.path()).unwrap().len(), 7);
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("external.jsonl"), fixtures::bytes('A')).unwrap();
        symlink(
            other.path().join("external.jsonl"),
            p.parent().unwrap().join("external.jsonl"),
        )
        .unwrap();
        assert_eq!(discover(t.path()).unwrap().len(), 8);
    }
    #[test]
    fn bounded_header_fallback() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("x.jsonl");
        let b = format!(
            "{{\"type\":\"session\",\"id\":\"x\",\"padding\":\"{}\"}}\n",
            "x".repeat(5000)
        );
        fs::write(&p, b).unwrap();
        assert!(scan_head(SessionFile::at(&p).unwrap()).is_some());
        fs::write(
            &p,
            format!(
                "{{\"type\":\"session\",\"id\":\"x\",\"padding\":\"{}\"}}\n",
                "x".repeat(66000)
            ),
        )
        .unwrap();
        assert!(scan_head(SessionFile::at(&p).unwrap()).is_none());
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use serde_json::{Value, json};
    #[test]
    fn title_slot_normalization_and_compaction_fallback() {
        let tree = tempfile::tempdir().unwrap();
        let path = tree.path().join("title.jsonl");
        for (slot, want) in [
            (json!("  "), "summary"),
            (json!(""), "summary"),
            (Value::Null, "header"),
            (json!(42), "header"),
            (json!({}), "header"),
            (json!("slot"), "slot"),
        ] {
            let content = format!(
                "{}\n{}\n{}\n",
                json!({"type":"title","title":slot}),
                json!({"type":"session","version":3,"id":"x","title":"header"}),
                json!({"type":"compaction","id":"c","shortSummary":"summary"})
            );
            fs::write(&path, &content).unwrap();
            let summary = scan_head(SessionFile::at(&path).unwrap()).unwrap();
            assert_eq!(summary.title.as_deref(), Some(want));
            assert_eq!(
                titles(&Session::parse(&path, content.as_bytes()).unwrap()).0,
                summary.title
            );
        }
        fs::write(&path, "{\"type\":\"title\",\"title\":\" \"}\n{\"type\":\"session\",\"id\":\"x\",\"title\":\"header\"}\n").unwrap();
        assert_eq!(
            scan_head(SessionFile::at(&path).unwrap()).unwrap().title,
            None
        );
    }
}
