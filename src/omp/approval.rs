//! omp's tool approval mode for one agent dir (`tools.approvalMode`, audit S-01), read from the
//! agent dir's `config.yml`/`config.yaml` without starting omp. omp 18.14.0: the setting is an
//! enum defaulting to `yolo` (`packages/coding-agent/src/tools/settings.ts`), the first main config
//! file that exists wins (`MAIN_CONFIG_FILENAMES`, `packages/utils/src/dirs.ts`), and an invalid
//! value falls back to the default (`packages/coding-agent/src/config/registry.ts`). A project's
//! `.omp/config.yml` or `.claude/settings.json`, `--approval-mode` and `--yolo` override it per
//! session; glen passes no approval flag.
//!
//! The reader is a lenient scanner for the shapes omp writes and people hand-edit (a block
//! `tools:` mapping with an `approvalMode:` scalar), not a YAML parser: anything it cannot read
//! with certainty is `unknown`, never `yolo` or a safe mode by guesswork.
use serde::Serialize;
use std::path::Path;

pub const SETTING: &str = "tools.approvalMode";
pub const MODES: [&str; 4] = ["always-ask", "always-ask-cached", "write", "yolo"];
pub const DEFAULT: &str = "yolo";
const FILES: [&str; 2] = ["config.yml", "config.yaml"];
/// A larger "config" is not one omp wrote; it reads as unknown rather than being scanned.
const MAX_BYTES: u64 = 4 << 20;

/// `mode` is one of [`MODES`] or `unknown`; `source` is the config file path, `default` (the
/// setting is unset or invalid, so omp uses [`DEFAULT`]) or `unknown`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Approval {
    pub mode: String,
    pub source: String,
    /// Why the mode is a fallback (invalid value) or unknown; human output only.
    #[serde(skip)]
    pub note: Option<String>,
}
impl Approval {
    pub fn is_yolo(&self) -> bool {
        self.mode == "yolo"
    }
    fn default_mode(note: Option<String>) -> Self {
        Self {
            mode: DEFAULT.into(),
            source: "default".into(),
            note,
        }
    }
    fn unknown(note: String) -> Self {
        Self {
            mode: "unknown".into(),
            source: "unknown".into(),
            note: Some(note),
        }
    }
}

/// The main config file omp reads under `agent_dir`: the first that exists, else `config.yml`.
pub fn config_file(agent_dir: &Path) -> std::path::PathBuf {
    FILES
        .iter()
        .map(|name| agent_dir.join(name))
        .find(|p| std::fs::symlink_metadata(p).is_ok())
        .unwrap_or_else(|| agent_dir.join(FILES[0]))
}

/// The approval mode omp uses for sessions under `agent_dir`, before project config and flags.
pub fn read(agent_dir: &Path) -> Approval {
    for name in FILES {
        let path = agent_dir.join(name);
        let shown = path.to_string_lossy().into_owned();
        let text = match std::fs::metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Ok(m) if m.is_file() && m.len() > MAX_BYTES => {
                return Approval::unknown(format!("{shown} is larger than {MAX_BYTES} bytes"));
            }
            _ => match std::fs::read(&path).map(String::from_utf8) {
                Ok(Ok(text)) => text,
                Ok(Err(_)) => return Approval::unknown(format!("{shown} is not UTF-8")),
                Err(e) => return Approval::unknown(format!("cannot read {shown}: {e}")),
            },
        };
        return match scan(&text) {
            Ok(None) => Approval::default_mode(None),
            Ok(Some(value)) if MODES.contains(&value.as_str()) => Approval {
                mode: value,
                source: shown,
                note: None,
            },
            Ok(Some(value)) => Approval::default_mode(Some(format!(
                "omp ignores the invalid {SETTING} {value:?} in {shown} and uses its default"
            ))),
            Err(why) => Approval::unknown(format!("cannot read {SETTING} from {shown}: {why}")),
        };
    }
    Approval::default_mode(None)
}

/// `tools.approvalMode` as written in one YAML document: `None` when unset or null.
fn scan(text: &str) -> Result<Option<String>, &'static str> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut tools_seen = false;
    let mut in_tools = false;
    let mut child_indent: Option<usize> = None;
    let mut found: Option<Option<String>> = None;
    let mut started = false;
    for raw in text.lines() {
        let line = raw.trim_end();
        let body = line.trim_start_matches(' ');
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        let indent = line.len() - body.len();
        if body.starts_with('\t') {
            // Legal inside another key's block scalar; never in the lines that matter here.
            if indent == 0 || in_tools {
                return Err("tab indentation");
            }
            continue;
        }
        if indent == 0 {
            in_tools = false;
            if body == "---" && !started {
                started = true;
                continue;
            }
            started = true;
            if body.starts_with(['%', '-', '?', '[', '{', '&', '*', '!', '|', '>']) {
                return Err("not a plain mapping");
            }
            let (key, value) = entry(body)?;
            if key != "tools" {
                continue;
            }
            if tools_seen {
                return Err("duplicate tools key");
            }
            tools_seen = true;
            match value {
                "" => in_tools = true,
                v if is_null(v) => {}
                _ => return Err("tools is not a block mapping"),
            }
            continue;
        }
        started = true;
        if !in_tools {
            continue;
        }
        let want = *child_indent.get_or_insert(indent);
        if indent > want {
            continue;
        }
        if indent < want {
            return Err("inconsistent indentation under tools");
        }
        if body.starts_with(['-', '?', '&', '*', '!']) {
            return Err("tools is not a plain mapping");
        }
        let (key, value) = entry(body)?;
        if key != "approvalMode" {
            continue;
        }
        if found.is_some() {
            return Err("duplicate approvalMode key");
        }
        found = Some(scalar(value)?);
    }
    Ok(found.flatten())
}

/// `key: value` with a plain or quoted key; the value has its trailing comment removed.
fn entry(body: &str) -> Result<(&str, &str), &'static str> {
    let (key, rest) = if let Some(q @ ('"' | '\'')) = body.chars().next() {
        let end = body[1..].find(q).ok_or("unterminated quoted key")? + 1;
        let key = &body[1..end];
        if key.contains(['\\', q]) {
            return Err("escaped key");
        }
        (key, body[end + 1..].trim_start_matches(' '))
    } else {
        let colon = body
            .match_indices(':')
            .find(|(i, _)| matches!(body.as_bytes().get(i + 1), None | Some(b' ')))
            .map(|(i, _)| i)
            .ok_or("line is not a key: value entry")?;
        (body[..colon].trim_end(), &body[colon..])
    };
    let value = rest
        .strip_prefix(':')
        .ok_or("line is not a key: value entry")?;
    if !(value.is_empty() || value.starts_with(' ')) {
        return Err("line is not a key: value entry");
    }
    Ok((key, strip_comment(value.trim())))
}

fn strip_comment(value: &str) -> &str {
    if value.starts_with(['"', '\'']) {
        return value;
    }
    if value.starts_with('#') {
        return "";
    }
    value.find(" #").map_or(value, |i| value[..i].trim_end())
}

fn is_null(value: &str) -> bool {
    matches!(value, "~" | "null" | "Null" | "NULL")
}

/// A single-line scalar value: `None` for null; quoted strings without escapes.
fn scalar(value: &str) -> Result<Option<String>, &'static str> {
    if value.is_empty() {
        return Err("approvalMode has no inline value");
    }
    if is_null(value) {
        return Ok(None);
    }
    if let Some(q @ ('"' | '\'')) = value.chars().next() {
        let inner = &value[1..];
        let end = inner.find(q).ok_or("unterminated quoted value")?;
        let rest = inner[end + 1..].trim_start();
        if !(rest.is_empty() || rest.starts_with('#')) || inner[..end].contains('\\') {
            return Err("unsupported quoted value");
        }
        return Ok(Some(inner[..end].to_string()));
    }
    if value.starts_with(['&', '*', '!', '|', '>', '[', '{', '%', '@', '`']) {
        return Err("unsupported approvalMode value");
    }
    Ok(Some(value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mode(text: &str) -> Result<Option<String>, &'static str> {
        scan(text)
    }
    fn some(s: &str) -> Result<Option<String>, &'static str> {
        Ok(Some(s.into()))
    }
    #[test]
    fn scans_block_mappings() {
        assert_eq!(mode("tools:\n  approvalMode: write\n"), some("write"));
        assert_eq!(
            mode(
                "---\n# c\nmodelRoles:\n  default: x\ntools:\n    other: 1\n    nested:\n      approvalMode: yolo\n    approvalMode: \"always-ask\"  # why\ntheme: dark\n"
            ),
            some("always-ask")
        );
        assert_eq!(mode("'tools':\n  'approvalMode': 'yolo'\n"), some("yolo"));
        assert_eq!(
            mode("tools:\n  approvalMode: always-ask-cached # c\n"),
            some("always-ask-cached")
        );
        assert_eq!(
            mode("\u{feff}tools:\r\n  approvalMode: write\r\n"),
            some("write")
        );
        assert_eq!(mode("tools:\n  approvalMode: bogus\n"), some("bogus"));
    }
    #[test]
    fn unset_and_null() {
        for text in [
            "",
            "# only a comment\n",
            "theme: dark\n",
            "tools:\n  approval:\n    bash: prompt\n",
            "tools: ~\n",
            "tools:\n  approvalMode: null\n",
            "notes: |\n  tools:\n    approvalMode: write\n",
            "tools.approvalMode: write\n",
        ] {
            assert_eq!(mode(text), Ok(None), "{text:?}");
        }
    }
    #[test]
    fn unreadable_shapes_are_errors() {
        for text in [
            "tools: {approvalMode: write}\n",
            "tools:\n  approvalMode:\n    - write\n",
            "tools:\n    approvalMode: write\n  x: 1\n",
            "tools:\n  approvalMode: write\ntools:\n  approvalMode: yolo\n",
            "tools:\n  approvalMode: write\n  approvalMode: yolo\n",
            "tools:\n\tapprovalMode: write\n",
            "- a\n- b\n",
            "tools:\n  approvalMode: *anchor\n",
            "tools:\n  approvalMode: \"wr\\x69te\"\n",
            "tools:\n  approvalMode: 'write\n",
            "not yaml at all\n",
            "tools: [1]\n",
            "a: 1\n---\ntools:\n  approvalMode: write\n",
        ] {
            assert!(mode(text).is_err(), "{text:?}: {:?}", mode(text));
        }
    }
    #[test]
    fn reads_agent_dirs() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path();
        assert_eq!(read(&dir.join("missing")), Approval::default_mode(None));
        assert_eq!(read(dir), Approval::default_mode(None));
        let yaml = dir.join("config.yaml");
        std::fs::write(&yaml, "tools:\n  approvalMode: write\n").unwrap();
        let a = read(dir);
        assert_eq!(
            (a.mode.as_str(), a.source.as_str()),
            ("write", yaml.to_str().unwrap())
        );
        // config.yml wins over config.yaml, even when it leaves the setting unset.
        let yml = dir.join("config.yml");
        std::fs::write(&yml, "theme: dark\n").unwrap();
        assert_eq!(read(dir), Approval::default_mode(None));
        std::fs::write(&yml, "tools:\n  approvalMode: yolo\n").unwrap();
        let a = read(dir);
        assert!(a.is_yolo());
        assert_eq!(a.source, yml.to_str().unwrap());
        std::fs::write(&yml, "tools:\n  approvalMode: sometimes\n").unwrap();
        let a = read(dir);
        assert_eq!((a.mode.as_str(), a.source.as_str()), ("yolo", "default"));
        assert!(a.note.unwrap().contains("\"sometimes\""));
        std::fs::write(&yml, "tools: {approvalMode: write}\n").unwrap();
        let a = read(dir);
        assert_eq!((a.mode.as_str(), a.source.as_str()), ("unknown", "unknown"));
        assert!(a.note.unwrap().contains("config.yml"));
        std::fs::write(&yml, b"tools:\n  approvalMode: \xff\n").unwrap();
        assert_eq!(read(dir).mode, "unknown");
        std::fs::remove_file(&yml).unwrap();
        std::fs::create_dir(&yml).unwrap();
        assert_eq!(read(dir).mode, "unknown");
    }
}
