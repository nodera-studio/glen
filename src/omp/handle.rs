use std::collections::{HashMap, HashSet};
fn compact(id: &str) -> String {
    id.replace('-', "").to_ascii_lowercase()
}
fn uuid(id: &str) -> bool {
    let s = compact(id);
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
pub fn assign<'a>(ids: impl IntoIterator<Item = &'a str>) -> HashMap<String, String> {
    let ids = ids.into_iter().collect::<HashSet<_>>();
    ids.iter()
        .map(|id| {
            let s = if uuid(id) { compact(id) } else { (*id).into() };
            let chars = s.chars().collect::<Vec<_>>();
            let mut k = 7.min(chars.len());
            while k < chars.len() {
                let h = if uuid(id) {
                    chars[chars.len() - k..].iter().collect::<String>()
                } else {
                    chars[..k].iter().collect()
                };
                if !ids.iter().any(|other| {
                    other != id
                        && if uuid(id) {
                            compact(other).ends_with(&h)
                        } else {
                            other.starts_with(&h)
                        }
                }) {
                    break;
                }
                k += 1
            }
            let h = if uuid(id) {
                chars[chars.len() - k..].iter().collect()
            } else {
                chars[..k].iter().collect()
            };
            ((*id).into(), h)
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collision() {
        let a = "00000000-0000-0000-0000-00001abcdef0";
        let b = "00000000-0000-0000-0000-00002abcdef0";
        let c = "00000000-0000-0000-0000-000000000000";
        let h = assign([a, b, c, a]);
        assert_eq!(h[a], "1abcdef0");
        assert_eq!(h[b], "2abcdef0");
        assert_eq!(h[c], "0000000");
        let h = assign(["abc", "abcdefgh1", "abcdefgh2"]);
        assert_eq!(h["abc"], "abc");
        assert_eq!(h["abcdefgh1"], "abcdefgh1");
        assert_eq!(h["abcdefgh2"], "abcdefgh2");
    }
}

use super::{
    bucket::bucket_for,
    listing::{Catalog, SessionFile, scan_head},
};
use crate::{
    env::Env,
    error::{ErrorCode, GlenError},
    render,
};
use std::path::PathBuf;
pub fn resolve(arg: &str, catalog: &Catalog, env: &Env) -> Result<PathBuf, GlenError> {
    let missing = || {
        GlenError::new(
            ErrorCode::NotFound,
            format!("no omp session matches `{arg}`"),
        )
        .with_hint("run `glen ls --all`")
    };
    if arg == "@latest" {
        let bucket = bucket_for(env.cwd(), env);
        return catalog
            .sessions
            .iter()
            .find(|s| s.file.bucket == bucket)
            .map(|s| s.file.path.clone())
            .ok_or_else(|| {
                GlenError::new(
                    ErrorCode::NotFound,
                    format!("no omp sessions for {}", env.cwd().display()),
                )
                .with_hint("run `glen ls --all`")
            });
    }
    if arg.starts_with('@') {
        return Err(GlenError::new(
            ErrorCode::Usage,
            if arg == "@new" {
                "`@new` is only valid for `glen send`".into()
            } else {
                format!("unknown session selector `{arg}`")
            },
        ));
    }
    let a = arg.to_lowercase();
    if a.len() == 8 && a.starts_with('j') && a[1..].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(
            GlenError::new(ErrorCode::Usage, format!("`{arg}` is a job handle"))
                .with_hint("use `glen wait <job>`"),
        );
    }
    if arg.contains('/') || arg.ends_with(".jsonl") {
        let p = super::paths::absolute(env, arg);
        return if SessionFile::at(&p).ok().and_then(scan_head).is_some() {
            Ok(p)
        } else {
            Err(GlenError::new(
                ErrorCode::NotFound,
                format!("not an omp session file: {}", p.display()),
            ))
        };
    }
    let hex = a.len() >= 4 && a.bytes().all(|b| b.is_ascii_hexdigit());
    let mut seen = HashSet::new();
    let matches = catalog
        .sessions
        .iter()
        .filter(|s| {
            let id = s.id.to_lowercase();
            let stem = s
                .file
                .path
                .file_stem()
                .map(|v| v.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            (id == a
                || (hex && compact(&id).ends_with(&a))
                || (a.len() >= 8
                    && (id.starts_with(&a)
                        || stem.starts_with(&a)
                        || stem.split_once('_').is_some_and(|(_, i)| i.starts_with(&a)))))
                && seen.insert(&s.file.path)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(missing()),
        [s] => Ok(s.file.path.clone()),
        _ => {
            let candidates=matches.iter().map(|s|serde_json::json!({"handle":catalog.handle(&s.id),"id":s.id,"path":s.file.path,"title":s.title,"cwd":s.cwd,"updated_at":jiff::Timestamp::try_from(s.file.mtime).ok().map(|t|t.to_string())})).collect::<Vec<_>>();
            let report = matches
                .iter()
                .map(|s| {
                    render::sanitize(&format!(
                        "{} · {} · {} · {}",
                        catalog.handle(&s.id),
                        jiff::Timestamp::try_from(s.file.mtime)
                            .map(|t| t.to_string())
                            .unwrap_or_default(),
                        s.cwd.as_deref().unwrap_or(&s.file.bucket),
                        s.title.as_deref().unwrap_or("untitled")
                    ))
                    .into_owned()
                })
                .collect::<Vec<_>>()
                .join("\n");
            Err(GlenError::new(
                ErrorCode::Ambiguous,
                format!("`{arg}` matches {} sessions", matches.len()),
            )
            .with_details(serde_json::json!({"candidates":candidates}))
            .with_hint("use a longer handle or the file path")
            .with_report(report))
        }
    }
}
#[cfg(test)]
mod resolution_tests {
    use super::*;
    use crate::omp::fixtures;
    #[test]
    fn selectors() {
        let t = fixtures::tree();
        let catalog = Catalog::scan(t.path()).unwrap();
        let e = Env::for_test(
            Some(std::path::Path::new("/home/fixture")),
            std::path::Path::new("/home/fixture/work/alpha"),
            &[("TMPDIR", "/unrelated-temp")],
        );
        for arg in ["e27c0de", "E27C0DE", "01a0f6b1", "2026-10-01T09"] {
            assert_eq!(resolve(arg, &catalog, &e).unwrap(), fixtures::path(&t, 'A'));
        }
        assert_eq!(
            resolve("c0de", &catalog, &e).unwrap_err().details.unwrap()["candidates"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        for (a, c) in [
            ("01a0f6b", ErrorCode::NotFound),
            ("j1234567", ErrorCode::Usage),
            ("@new", ErrorCode::Usage),
        ] {
            assert_eq!(resolve(a, &catalog, &e).unwrap_err().code, c)
        }
        let p = fixtures::path(&t, 'E');
        assert_eq!(resolve(p.to_str().unwrap(), &catalog, &e).unwrap(), p);
        assert!(resolve("/no/file.jsonl", &catalog, &e).is_err());
        assert!(resolve("@latest", &catalog, &e).is_ok());
        for k in ['A', 'B', 'C', 'D', 'E', 'F', 'G'] {
            let p = fixtures::path(&t, k);
            let s = scan_head(SessionFile::at(&p).unwrap()).unwrap();
            assert_eq!(catalog.handle(&s.id), s.id.replace('-', "")[25..]);
        }
    }
}
