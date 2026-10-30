use super::{
    listing::{self, SessionFile, Summary},
    session::Session,
};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
pub struct Continued {
    pub path: PathBuf,
    pub from: Vec<String>,
}
fn last_activity(s: &Summary) -> (f64, Option<String>) {
    let fallback = || {
        (
            s.created
                .map_or(f64::NEG_INFINITY, |t| t.as_millisecond() as f64),
            None,
        )
    };
    let Ok(mut f) = File::open(&s.file.path) else {
        return (f64::NEG_INFINITY, None);
    };
    let Ok(m) = f.metadata() else {
        return (f64::NEG_INFINITY, None);
    };
    let mut window = 65536u64;
    loop {
        let offset = m.len().saturating_sub(window);
        if f.seek(SeekFrom::Start(offset)).is_err() {
            return (f64::NEG_INFINITY, None);
        }
        let mut b = vec![];
        if f.read_to_end(&mut b).is_err() {
            return (f64::NEG_INFINITY, None);
        }
        let start = if offset > 0 {
            b.iter()
                .position(|v| *v == b'\n')
                .map_or(b.len(), |i| i + 1)
        } else {
            0
        };
        for line in b[start..].split(|b| *b == b'\n').rev() {
            if line.first() != Some(&b'{') {
                continue;
            }
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
                continue;
            };
            if v["type"] == "session" {
                return fallback();
            }
            if !matches!(
                v["type"].as_str(),
                Some("message" | "custom_message" | "branch_summary" | "reset_boundary")
            ) {
                continue;
            }
            let custom = if v["type"] == "custom_message" {
                &v["customType"]
            } else {
                &v["message"]["customType"]
            };
            if custom == "eval-state-context" {
                continue;
            }
            if let Some(t) = v["timestamp"]
                .as_str()
                .and_then(|s| s.parse::<jiff::Timestamp>().ok())
            {
                return (
                    t.as_millisecond() as f64,
                    v["id"].as_str().map(str::to_owned),
                );
            }
        }
        if offset == 0 {
            return fallback();
        }
        window = window.saturating_mul(4);
    }
}
fn holds_entry(path: &Path, id: &str) -> bool {
    Session::load(path).is_ok_and(|s| s.entries.iter().any(|e| e.id == id))
}
pub fn follow(path: &Path) -> io::Result<Continued> {
    follow_known(path, None)
}
pub(crate) fn follow_known(path: &Path, known: Option<&Summary>) -> io::Result<Continued> {
    let unchanged = || Continued {
        path: path.into(),
        from: vec![],
    };
    let start = SessionFile::at(path)
        .ok()
        .and_then(listing::scan_head)
        .or_else(|| known.cloned());
    let named = path
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(listing::parse_session_file_name);
    let (root, since) = if let Some(s) = &start {
        (s.id.clone(), s.created)
    } else if path.exists() {
        if let Some((t, id)) = named {
            (id, Some(t))
        } else {
            return Ok(unchanged());
        }
    } else {
        return Ok(unchanged());
    };
    let Some(dir) = path.parent() else {
        return Ok(unchanged());
    };
    let mut children: HashMap<String, Vec<Summary>> = HashMap::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') || entry.path() == path {
            continue;
        }
        let Some((t, _)) = listing::parse_session_file_name(name) else {
            continue;
        };
        if since.is_some_and(|s| t < s) {
            continue;
        }
        if !entry.metadata().is_ok_and(|m| m.is_file()) {
            continue;
        }
        if let Some(s) = SessionFile::at(&entry.path())
            .ok()
            .and_then(listing::scan_head)
            && let Some(parent) = &s.continuation_of
        {
            children.entry(parent.clone()).or_default().push(s);
        }
    }
    let mut copies = if let Some(s) = start.clone() {
        vec![s]
    } else {
        children.get(&root).cloned().unwrap_or_default()
    };
    let mut seen: HashSet<_> = copies.iter().map(|s| s.file.path.clone()).collect();
    seen.insert(path.into());
    let mut i = 0;
    while i < copies.len() {
        let last = last_activity(&copies[i]).1;
        if let Some(next) = children.get(&copies[i].id) {
            for child in next {
                if seen.insert(child.file.path.clone())
                    && last
                        .as_deref()
                        .is_none_or(|id| holds_entry(&child.file.path, id))
                {
                    copies.push(child.clone())
                }
            }
        }
        i += 1;
    }
    let mut pick = start.as_ref();
    let mut time = pick.map_or(f64::NEG_INFINITY, |s| last_activity(s).0);
    for s in &copies {
        let t = last_activity(s).0;
        if t > time {
            pick = Some(s);
            time = t
        }
    }
    let Some(pick) = pick else {
        return Ok(unchanged());
    };
    if pick.file.path == path {
        return Ok(unchanged());
    }
    let mut from = vec![];
    let mut next = pick.continuation_of.as_deref();
    let mut ids = HashSet::new();
    while let Some(id) = next {
        if !ids.insert(id) {
            break;
        }
        from.push(id.to_owned());
        if id == root {
            break;
        }
        next = copies
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| s.continuation_of.as_deref());
    }
    from.reverse();
    Ok(Continued {
        path: pick.file.path.clone(),
        from,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::omp::fixtures;
    #[test]
    fn copies_not_forks() {
        let t = fixtures::tree();
        let b = fixtures::path(&t, 'B');
        let c = fixtures::path(&t, 'C');
        let r = follow(&b).unwrap();
        assert_eq!(r.path, c);
        assert_eq!(r.from, ["01a0fd05-a240-7c41-9d02-7d2e8f13a5b6"]);
        for k in ['A', 'C', 'F'] {
            let p = fixtures::path(&t, k);
            assert_eq!(follow(&p).unwrap().path, p)
        }
        fs::write(&b, b"garbage\n").unwrap();
        assert_eq!(follow(&b).unwrap().path, c);
    }
    #[test]
    fn multihop_and_diverged() {
        let t = fixtures::tree();
        let b = fixtures::path(&t, 'B');
        let c = fixtures::path(&t, 'C');
        let id = "01a0fd2a-4140-7e05-a3f4-0c9a4b7e2d81";
        let new = "01a0fd2b-4140-7e05-a3f4-0c9a4b7e2d82";
        let mut content = String::from_utf8(fixtures::bytes('C').to_vec())
            .unwrap()
            .replace(id, new)
            .replace(
                "\"continuationOf\":\"01a0fd05-a240-7c41-9d02-7d2e8f13a5b6\"",
                &format!("\"continuationOf\":\"{id}\""),
            );
        content.push_str("{\"type\":\"message\",\"id\":\"new\",\"parentId\":\"30000006\",\"timestamp\":\"2026-10-04T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"new\"}}\n");
        let p = c
            .parent()
            .unwrap()
            .join(format!("2026-10-04T00-00-00-000Z_{new}.jsonl"));
        fs::write(&p, content).unwrap();
        let r = follow(&b).unwrap();
        assert_eq!(r.path, p);
        assert_eq!(r.from.len(), 2);
        use std::io::Write;
        writeln!(
            fs::OpenOptions::new().append(true).open(&b).unwrap(),
            "{{\"type\":\"message\",\"id\":\"diverged\",\"timestamp\":\"2026-10-05T00:00:00Z\"}}"
        )
        .unwrap();
        assert_eq!(follow(&b).unwrap().path, b);
    }
}
