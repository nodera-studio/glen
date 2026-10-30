use crate::{error::Warning, live};
use std::{cmp::Ordering, fs, io, path::Path};
use xxhash_rust::xxh64::xxh64;
pub const SOURCE: &str = include_str!("../extension/glen/index.ts");
pub const PROTOCOL: u32 = crate::ops::PROTOCOL;
pub fn render(version: &str, home: &Path) -> String {
    let body = SOURCE
        .split_once('\n')
        .map_or("", |(_, b)| b)
        .replace(
            "\"__GLEN_VERSION__\"",
            &serde_json::json!(version).to_string(),
        )
        .replace(
            "\"__GLEN_HOME__\"",
            &serde_json::json!(home.to_string_lossy()).to_string(),
        );
    format!(
        "// glen-extension version={version} protocol={PROTOCOL} digest={:016x}\n{body}",
        xxh64(body.as_bytes(), 0)
    )
}
#[derive(Debug, PartialEq, Eq)]
pub enum State {
    NotInstalled,
    Current,
    Outdated(String),
    Newer(String),
    DifferentHome,
    Modified,
    Foreign,
}
impl State {
    pub fn name(&self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Current => "current",
            Self::Outdated(_) => "outdated",
            Self::Newer(_) => "newer",
            Self::DifferentHome => "different_home",
            Self::Modified => "modified",
            Self::Foreign => "foreign",
        }
    }
}
fn compare(a: &str, b: &str) -> Ordering {
    let parts = |s: &str| {
        s.split('.')
            .map(str::parse::<u64>)
            .collect::<Result<Vec<_>, _>>()
    };
    match (parts(a), parts(b)) {
        (Ok(mut a), Ok(mut b)) => {
            let n = a.len().max(b.len());
            a.resize(n, 0);
            b.resize(n, 0);
            a.cmp(&b)
        }
        (Err(_), Ok(_)) => Ordering::Less,
        (Ok(_), Err(_)) => Ordering::Greater,
        _ => a.cmp(b),
    }
}
pub fn inspect(path: &Path, version: &str, home: &Path) -> io::Result<(State, Option<String>)> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((State::NotInstalled, None)),
        Err(e) => return Err(e),
    };
    let Some(end) = bytes.iter().position(|b| *b == b'\n') else {
        return Ok((State::Foreign, None));
    };
    let Ok(line) = std::str::from_utf8(&bytes[..end]) else {
        return Ok((State::Foreign, None));
    };
    let fields = line
        .strip_prefix("// glen-extension version=")
        .map(|s| s.split(' ').collect::<Vec<_>>());
    let Some(fields) = fields.filter(|f| f.len() == 3 && !f[0].is_empty()) else {
        return Ok((State::Foreign, None));
    };
    let valid_protocol = fields[1]
        .strip_prefix("protocol=")
        .is_some_and(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()));
    let digest = fields[2]
        .strip_prefix("digest=")
        .filter(|s| {
            s.len() == 16
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .and_then(|s| u64::from_str_radix(s, 16).ok());
    if !valid_protocol || digest.is_none() || fields[0].chars().any(char::is_whitespace) {
        return Ok((State::Foreign, None));
    }
    let installed = Some(fields[0].to_owned());
    if digest != Some(xxh64(&bytes[end + 1..], 0)) {
        return Ok((State::Modified, installed));
    }
    if home.to_str().is_some() && bytes == render(version, home).as_bytes() {
        return Ok((State::Current, installed));
    }
    let state = match compare(fields[0], version) {
        Ordering::Less => State::Outdated(fields[0].into()),
        Ordering::Greater => State::Newer(fields[0].into()),
        Ordering::Equal => State::DifferentHome,
    };
    Ok((state, installed))
}
pub fn socket_path_warning(home: &Path) -> Option<Warning> {
    let p = home.join("run/live/9999999.sock");
    (!live::socket_path_ok(&p)).then(||Warning{code:"socket_path_too_long".into(),message:format!("GLEN_HOME is too long for a unix socket path ({} bytes, limit {}); live delivery will be off; shorten GLEN_HOME",p.as_os_str().len(),live::SUN_PATH_MAX)})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_contract() {
        for marker in ["__GLEN_HOME__", "__GLEN_VERSION__"] {
            assert_eq!(SOURCE.matches(marker).count(), 1);
        }
        assert!(SOURCE.contains(&format!("export const PROTOCOL = {PROTOCOL};")));
        assert!(!SOURCE.contains("Bun."));
        for line in SOURCE.lines() {
            if line.contains("setTimeout(") || line.contains("setInterval(") {
                assert!(
                    line.contains("ctx.setTimeout(")
                        || line.contains("ctx.setInterval(")
                        || line.contains("conn.setTimeout(")
                );
            }
            if line.starts_with("import ") {
                assert!(line.contains("\"node:") || line.starts_with("import type "));
            }
        }
        let rendered = render("0.1.0", Path::new("/tmp/a\"\\b"));
        assert!(!rendered.contains("__GLEN_"));
        let literal = rendered
            .lines()
            .find_map(|l| l.strip_prefix("const BAKED_GLEN_HOME = "))
            .unwrap()
            .trim_end_matches(';');
        assert_eq!(
            serde_json::from_str::<String>(literal).unwrap(),
            "/tmp/a\"\\b"
        );
    }
    #[test]
    fn inspection_table() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("index.ts");
        let h = Path::new("/tmp/g");
        assert_eq!(inspect(&p, "0.1.0", h).unwrap().0, State::NotInstalled);
        for (body, want) in [
            (render("0.1.0", h), State::Current),
            (render("0.0.1", h), State::Outdated("0.0.1".into())),
            (render("9.0.0", h), State::Newer("9.0.0".into())),
            (
                render("0.1.0", Path::new("/tmp/other")),
                State::DifferentHome,
            ),
            (format!("{}changed", render("0.1.0", h)), State::Modified),
            ("foreign".into(), State::Foreign),
        ] {
            fs::write(&p, body).unwrap();
            assert_eq!(inspect(&p, "0.1.0", h).unwrap().0, want);
        }
    }
}
