use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    ops::Range,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone)]
pub struct Header {
    pub id: String,
    pub cwd: Option<String>,
    pub timestamp: Option<String>,
    pub title: Option<String>,
    pub parent_session: Option<String>,
    pub continuation_of: Option<String>,
    pub raw: Value,
}
#[derive(Debug, Clone)]
pub struct Entry {
    pub kind: String,
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: Option<String>,
    pub active_leaf_id: Option<Option<String>>,
    pub raw: Value,
}
#[derive(Debug)]
pub struct Session {
    pub path: PathBuf,
    pub slot_title: Option<Option<String>>,
    pub header: Header,
    pub entries: Vec<Entry>,
    pub skipped_lines: usize,
    pub consumed: u64,
}
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("not an omp session file")]
    NotSession,
    #[error(transparent)]
    Io(#[from] io::Error),
}
#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Complete,
    Interrupted,
    Aborted,
    Error,
    Pending,
    Unknown,
}
pub(crate) fn string(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().map(str::to_owned)
}
fn entry(raw: Value) -> Option<Entry> {
    Some(Entry {
        kind: string(&raw, "type")?,
        id: string(&raw, "id")?,
        parent_id: string(&raw, "parentId"),
        timestamp: string(&raw, "timestamp"),
        active_leaf_id: raw.get("activeLeafId").and_then(|v| {
            if v.is_null() {
                Some(None)
            } else {
                v.as_str().map(|s| Some(s.to_owned()))
            }
        }),
        raw,
    })
}
impl Session {
    pub fn load(path: &Path) -> Result<Self, LoadError> {
        Self::parse(path, &fs::read(path)?)
    }
    pub fn parse(path: &Path, bytes: &[u8]) -> Result<Self, LoadError> {
        let mut lines = bytes
            .split(|b| *b == b'\n')
            .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
            .filter(|l| !l.iter().all(u8::is_ascii_whitespace));
        let mut raw: Value = serde_json::from_slice(lines.next().ok_or(LoadError::NotSession)?)
            .map_err(|_| LoadError::NotSession)?;
        let slot_title = if raw["type"] == "title" {
            let t = string(&raw, "title").map(|t| display_name(&t));
            raw = serde_json::from_slice(lines.next().ok_or(LoadError::NotSession)?)
                .map_err(|_| LoadError::NotSession)?;
            t
        } else {
            None
        };
        if raw["type"] != "session" {
            return Err(LoadError::NotSession);
        }
        let header = Header {
            id: string(&raw, "id").ok_or(LoadError::NotSession)?,
            cwd: string(&raw, "cwd"),
            timestamp: string(&raw, "timestamp"),
            title: string(&raw, "title"),
            parent_session: string(&raw, "parentSession"),
            continuation_of: string(&raw, "continuationOf"),
            raw,
        };
        let mut s = Self {
            path: path.into(),
            slot_title,
            header,
            entries: vec![],
            skipped_lines: 0,
            consumed: bytes
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |i| i as u64 + 1),
        };
        for l in lines {
            s.parse_entry(l);
        }
        Ok(s)
    }
    fn parse_entry(&mut self, line: &[u8]) {
        if line.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        let parsed = serde_json::from_slice::<Value>(line)
            .ok()
            .and_then(|mut raw| {
                // v1 predates the tree format. Rebuild its linear chain in
                // memory, including records carrying stale ids or parents.
                if self.header.raw["version"].as_u64().unwrap_or(1) < 2 && raw["type"].is_string() {
                    let previous = self.entries.last().map(|e| e.id.clone());
                    raw["id"] = Value::String(format!("v1-{:08}", self.entries.len() + 1));
                    raw["parentId"] = previous.map_or(Value::Null, Value::String);
                }
                entry(raw)
            });
        if let Some(e) = parsed {
            self.entries.push(e)
        } else {
            self.skipped_lines += 1
        }
    }
    pub fn append(&mut self, complete_lines: &[u8]) -> Range<usize> {
        let start = self.entries.len();
        for l in complete_lines.split(|b| *b == b'\n') {
            self.parse_entry(l);
        }
        self.consumed += complete_lines.len() as u64;
        start..self.entries.len()
    }
    pub fn leaf_id(&self) -> Option<&str> {
        let last = self.entries.last()?;
        let leaf = entry_leaf(last)?;
        if leaf == last.id || self.entries.iter().any(|e| e.id == leaf) {
            Some(leaf)
        } else {
            self.entries
                .iter()
                .rev()
                .find(|e| entry_leaf(e) == Some(e.id.as_str()))
                .map(|e| e.id.as_str())
        }
    }
    pub fn active_branch(&self) -> Vec<&Entry> {
        let map: HashMap<_, _> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.as_str(), i))
            .collect();
        let mut next = self.leaf_id();
        let mut seen = HashSet::new();
        let mut branch = vec![];
        while let Some(id) = next {
            if !seen.insert(id) {
                break;
            }
            let Some(i) = map.get(id) else { break };
            let e = &self.entries[*i];
            branch.push(e);
            next = e.parent_id.as_deref();
        }
        branch.reverse();
        branch
    }
    pub fn title(&self) -> Option<String> {
        self.slot_title
            .as_ref()
            .unwrap_or(&self.header.title)
            .as_deref()
            .and_then(display_name)
    }
}
fn entry_leaf(e: &Entry) -> Option<&str> {
    e.active_leaf_id
        .as_ref()
        .map_or(Some(e.id.as_str()), |s| s.as_deref())
}
/// Preserve nonblank session text exactly; human renderers collapse whitespace
/// in single-line fields and escape controls at the presentation boundary.
pub fn display_name(raw: &str) -> Option<String> {
    // ECMAScript WhiteSpace + LineTerminator (String.prototype.trim).
    let blank = raw.chars().all(|c| {
        matches!(
            c,
            '\u{0009}'..='\u{000d}'
                | '\u{0020}'
                | '\u{00a0}'
                | '\u{1680}'
                | '\u{2000}'..='\u{200a}'
                | '\u{2028}'..='\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
        )
    });
    (!blank).then(|| raw.to_owned())
}
/// omp's listing preview includes only text blocks, without transcript placeholders.
pub fn text_only(content: &Value, sep: &str) -> String {
    if let Some(s) = content.as_str() {
        s.into()
    } else {
        content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|v| v["type"] == "text")
            .map(|v| v["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(sep)
    }
}
pub fn text_of(content: &Value, sep: &str) -> String {
    if let Some(s) = content.as_str() {
        s.into()
    } else {
        content
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| match v["type"].as_str() {
                Some("text") => v["text"].as_str().map(str::to_owned),
                Some(
                    "image"
                    | "thinking"
                    | "redactedThinking"
                    | "toolCall"
                    | "fallback"
                    | "anthropicServerTool",
                ) => None,
                Some(t) => Some(format!("[{t}]")),
                None => None,
            })
            .collect::<Vec<_>>()
            .join(sep)
    }
}
fn has_call(m: &Value) -> bool {
    m["content"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v["type"] == "toolCall"))
}
impl TurnStatus {
    pub fn of_message(m: &Value) -> Self {
        match m["role"].as_str() {
            Some("assistant") => match m["stopReason"].as_str() {
                Some("error") => Self::Error,
                Some("aborted") => Self::Aborted,
                Some("length") => Self::Interrupted,
                _ if has_call(m) => Self::Interrupted,
                _ => Self::Complete,
            },
            Some("toolResult") => Self::Interrupted,
            Some("user") => Self::Pending,
            _ => Self::Unknown,
        }
    }
    pub fn of_branch(entries: &[&Entry]) -> Self {
        entries
            .iter()
            .rev()
            .find(|e| e.kind == "message")
            .map_or(Self::Unknown, |e| Self::of_message(&e.raw["message"]))
    }
}
pub fn in_flight(m: &Value) -> bool {
    match m["role"].as_str() {
        Some("user" | "developer" | "toolResult") => true,
        Some("assistant") => {
            has_call(m) && !matches!(m["stopReason"].as_str(), Some("error" | "aborted"))
        }
        _ => false,
    }
}
pub fn in_flight_of_branch(entries: &[&Entry]) -> bool {
    entries
        .iter()
        .rev()
        .find(|e| e.kind == "message")
        .is_some_and(|e| in_flight(&e.raw["message"]))
}
#[cfg(test)]
mod tests {
    #[test]
    fn display_names_preserve_raw_controls_and_whitespace() {
        let raw = "  title\twith\n\x1b[31m\x00\x7f  ";
        assert_eq!(super::display_name(raw).as_deref(), Some(raw));
        assert_eq!(super::display_name(" \n\t "), None);
    }
    #[test]
    fn display_names_use_javascript_trim_whitespace() {
        assert_eq!(super::display_name(" \u{feff}\n"), None);
        assert_eq!(super::display_name("\u{0085}").as_deref(), Some("\u{0085}"));
        assert_eq!(super::display_name("\u{200b}").as_deref(), Some("\u{200b}"));
        assert_eq!(super::display_name("\u{180e}").as_deref(), Some("\u{180e}"));
    }
    use super::*;
    use serde_json::json;
    fn parse(b: &[u8]) -> Session {
        Session::parse(Path::new("fixture"), b).unwrap()
    }
    #[test]
    fn lenient_and_incremental() {
        let mut s=parse(b"{\"type\":\"session\",\"version\":3,\"id\":\"x\"}\n{\"type\":\"message\",\"id\":\"a\",\"message\":\"\xff\"}\n{\"type\":\"future\",\"id\":\"b\"}\nbad");
        assert_eq!(s.skipped_lines, 2);
        assert_eq!(s.leaf_id(), Some("b"));
        assert!(s.consumed < 1000);
        assert_eq!(
            s.append(
                b"{\"type\":\"future\",\"id\":\"c\"}\n{\"type\":\"future\",\"id\":\"d\"}\nbad\n"
            ),
            1..3
        );
        assert_eq!(s.skipped_lines, 3);
    }
    #[test]
    fn leaves() {
        for (entries, want) in [
            (vec![], None),
            (vec![json!({"type":"x","id":"a","activeLeafId":null})], None),
            (
                vec![
                    json!({"type":"x","id":"a"}),
                    json!({"type":"x","id":"b","activeLeafId":"missing"}),
                ],
                Some("a"),
            ),
            (
                vec![
                    json!({"type":"x","id":"a","parentId":"b"}),
                    json!({"type":"x","id":"b","parentId":"a"}),
                ],
                Some("b"),
            ),
        ] {
            let mut b = b"{\"type\":\"session\",\"version\":3,\"id\":\"x\"}\n".to_vec();
            for e in entries {
                b.extend(serde_json::to_vec(&e).unwrap());
                b.push(b'\n');
            }
            let s = parse(&b);
            assert_eq!(s.leaf_id(), want);
            assert!(s.active_branch().len() <= 2);
        }
    }
    #[test]
    fn busy_is_separate_from_turn() {
        for (m, want) in [
            (json!({"role":"user"}), true),
            (json!({"role":"developer"}), true),
            (json!({"role":"toolResult"}), true),
            (
                json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall"}]}),
                true,
            ),
            (
                json!({"role":"assistant","stopReason":"aborted","content":[{"type":"toolCall"}]}),
                false,
            ),
            (
                json!({"role":"assistant","stopReason":"length","content":[]}),
                false,
            ),
            (json!({"role":"bashExecution"}), false),
        ] {
            assert_eq!(in_flight(&m), want);
        }
    }
}
#[cfg(test)]
mod fixture_tests {
    use super::*;
    use crate::omp::fixtures;
    #[test]
    fn fixtures_and_branches() {
        let a = Session::parse(Path::new("A"), fixtures::bytes('A')).unwrap();
        assert_eq!(a.title().as_deref(), Some("Fix the flaky parser test"));
        assert_eq!(a.entries.len(), 9);
        assert_eq!(a.leaf_id(), Some("10000009"));
        let d = Session::parse(Path::new("D"), fixtures::bytes('D')).unwrap();
        assert_eq!(d.leaf_id(), Some("40000008"));
        assert_eq!(
            d.active_branch()
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            [
                "40000001", "40000002", "40000005", "40000006", "40000007", "40000008"
            ]
        );
        let e = Session::parse(Path::new("E"), fixtures::bytes('E')).unwrap();
        assert_eq!(e.skipped_lines, 2);
        assert!(e.consumed < (fixtures::bytes('E').len() as u64));
        for id in ["50000008", "5000000b"] {
            assert!(e.active_branch().iter().any(|e| e.id == id))
        }
        let f = Session::parse(Path::new("F"), fixtures::bytes('F')).unwrap();
        assert_eq!(f.slot_title, None);
        assert_eq!(f.title().as_deref(), Some("Legacy title"));
        let b = Session::parse(Path::new("B"), fixtures::bytes('B')).unwrap();
        assert_eq!(
            TurnStatus::of_branch(&b.active_branch()),
            TurnStatus::Pending
        );
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn user_images_do_not_become_text_placeholders() {
        let message = json!({"role":"user","content":[
            {"type":"text","text":"look"},
            {"type":"image","data":"opaque","mimeType":"image/png"}
        ]});
        assert_eq!(text_of(&message["content"], "\n"), "look");
    }
    #[test]
    fn only_unknown_blocks_become_text_placeholders() {
        let content = json!([
            {"type":"text","text":"visible"},
            {"type":"image"},
            {"type":"thinking"},
            {"type":"redactedThinking"},
            {"type":"toolCall"},
            {"type":"fallback"},
            {"type":"anthropicServerTool"},
            {"type":"future_block"},
            {"type":"text"},
            {}
        ]);
        assert_eq!(text_of(&content, "\n"), "visible\n[future_block]");
    }
    #[test]
    fn v1_idless_entries_get_stable_linear_ids_in_memory() {
        let bytes = include_bytes!("../../tests/fixtures/v1-session.jsonl");
        for version in [None, Some(0), Some(1)] {
            let mut lines = bytes.split(|b| *b == b'\n');
            let mut header: Value = serde_json::from_slice(lines.next().unwrap()).unwrap();
            if let Some(version) = version {
                header["version"] = version.into();
            }
            let mut content = serde_json::to_vec(&header).unwrap();
            content.push(b'\n');
            content
                .extend_from_slice(&bytes[bytes.iter().position(|b| *b == b'\n').unwrap() + 1..]);
            let mut s = Session::parse(Path::new("v1"), &content).unwrap();
            assert_eq!(s.skipped_lines, 0);
            assert_eq!(s.active_branch().len(), 3);
            assert_eq!(s.entries[0].id, "v1-00000001");
            assert_eq!(s.entries[0].parent_id, None);
            assert_eq!(s.entries[2].parent_id.as_deref(), Some("v1-00000002"));
            s.append(
                b"{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"next\"}}\n",
            );
            assert_eq!(s.active_branch().len(), 4);
            assert_eq!(s.entries[3].raw["parentId"], "v1-00000003");
            let with_stale_ids = String::from_utf8(content.clone()).unwrap().replace(
                "\"type\":\"message\"",
                "\"type\":\"message\",\"id\":\"stale\",\"parentId\":\"missing\"",
            );
            let migrated = Session::parse(Path::new("v1"), with_stale_ids.as_bytes()).unwrap();
            assert_eq!(migrated.active_branch().len(), 3);
            assert_eq!(
                migrated.entries[2].parent_id.as_deref(),
                Some("v1-00000002")
            );
            let again = Session::parse(Path::new("v1"), &content).unwrap();
            assert_eq!(s.entries[0].id, again.entries[0].id);
        }
        assert_eq!(
            include_bytes!("../../tests/fixtures/v1-session.jsonl"),
            bytes
        );
    }
}
