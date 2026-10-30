use std::collections::{HashMap, HashSet};
fn compact(id: &str) -> String {
    id.replace('-', "").to_ascii_lowercase()
}
/// Adjacent sorted keys bound the longest shared prefix, avoiding pairwise scans.
fn shared_prefixes(keys: &[Vec<char>]) -> Vec<usize> {
    let mut order = (0..keys.len()).collect::<Vec<_>>();
    order.sort_unstable_by(|a, b| keys[*a].cmp(&keys[*b]));
    let mut shared = vec![0; keys.len()];
    for pair in order.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let n = keys[a]
            .iter()
            .zip(&keys[b])
            .take_while(|(a, b)| a == b)
            .count();
        shared[a] = shared[a].max(n);
        shared[b] = shared[b].max(n);
    }
    shared
}
#[cfg(test)]
std::thread_local! { pub(crate) static ASSIGN_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub fn assign<'a>(ids: impl IntoIterator<Item = &'a str>) -> HashMap<String, String> {
    #[cfg(test)]
    ASSIGN_CALLS.with(|count| count.set(count.get() + 1));
    let ids = ids
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let normalized = ids.iter().map(|id| compact(id)).collect::<Vec<_>>();
    let prefixes = ids
        .iter()
        .map(|id| id.chars().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let suffixes = normalized
        .iter()
        .map(|id| id.chars().rev().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let prefix_shared = shared_prefixes(&prefixes);
    let suffix_shared = shared_prefixes(&suffixes);
    ids.iter()
        .enumerate()
        .map(|(i, id)| {
            let uuid =
                normalized[i].len() == 32 && normalized[i].bytes().all(|b| b.is_ascii_hexdigit());
            let (chars, shared) = if uuid {
                (&suffixes[i], suffix_shared[i])
            } else {
                (&prefixes[i], prefix_shared[i])
            };
            let n = 7.max(shared + 1).min(chars.len());
            let handle = if uuid {
                chars[..n].iter().rev().collect()
            } else {
                chars[..n].iter().collect()
            };
            ((*id).to_owned(), handle)
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
            let continued = super::continuation::follow(&p)?;
            if continued.path != p {
                // Keep the original path so open records continued_from.
                Ok(p)
            } else {
                Err(GlenError::new(
                    ErrorCode::NotFound,
                    format!("not an omp session file: {}", p.display()),
                ))
            }
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
            (catalog
                .handles
                .get(&s.id)
                .is_some_and(|h| h.to_lowercase() == a)
                || id == a
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

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::omp::fixtures;
    use std::{fs, path::Path};

    #[test]
    fn every_assigned_handle_resolves_including_legacy_and_widened() {
        let tree = fixtures::tree();
        for id in [
            "legacy-session-one",
            "legacy-session-two",
            "short",
            "界界界界界界界一",
            "界界界界界界界二",
            "00000000-0000-0000-0000-00001abcdef0",
            "00000000-0000-0000-0000-00002abcdef0",
        ] {
            fs::write(
                tree.path().join("-work-alpha").join(format!("{id}.jsonl")),
                format!("{{\"type\":\"session\",\"id\":\"{id}\",\"version\":3}}\n"),
            )
            .unwrap();
        }
        let catalog = Catalog::scan(tree.path()).unwrap();
        let env = Env::for_test(Some(Path::new("/fixture")), Path::new("/fixture"), &[]);
        for s in &catalog.sessions {
            assert_eq!(
                resolve(&catalog.handle(&s.id), &catalog, &env).unwrap(),
                s.file.path
            );
        }
        let original = &catalog.sessions[0];
        let duplicate = original.file.path.with_file_name("duplicate.jsonl");
        fs::copy(&original.file.path, duplicate).unwrap();
        let catalog = Catalog::scan(tree.path()).unwrap();
        assert_eq!(
            resolve(&catalog.handle(&original.id), &catalog, &env)
                .unwrap_err()
                .code,
            ErrorCode::Ambiguous
        );
    }

    #[test]
    fn sorted_assignment_handles_five_thousand_sessions() {
        let ids = (0..5000)
            .map(|i| format!("00000000-0000-0000-0000-{i:012x}"))
            .collect::<Vec<_>>();
        let handles = assign(ids.iter().map(String::as_str));
        assert_eq!(handles.len(), 5000);
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(handles[id], format!("{i:07x}"));
        }
        // Mixed suffix/prefix collisions must count legacy ids too.
        let h = assign([
            "00000000-0000-0000-0000-00001abcdef0",
            "legacy-abcdef0",
            "legacy-abcdef1",
        ]);
        assert_eq!(h["00000000-0000-0000-0000-00001abcdef0"], "1abcdef0");
        assert_eq!(h["legacy-abcdef0"], "legacy-abcdef0");
    }
}
