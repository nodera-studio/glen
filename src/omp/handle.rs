use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};
fn compact(id: &str) -> String {
    id.replace('-', "").to_ascii_lowercase()
}
/// The union used by both handle assignment and argument resolution. Sorted
/// indexes keep collision checks bounded instead of scanning every session.
#[derive(Default)]
struct Matches<'a> {
    exact: BTreeMap<String, HashSet<&'a str>>,
    prefixes: BTreeMap<String, HashSet<&'a str>>,
    suffixes: BTreeMap<String, HashSet<&'a str>>,
}
impl<'a> Matches<'a> {
    fn new(sessions: impl IntoIterator<Item = (&'a str, Option<&'a Path>)>) -> Self {
        let mut index = Self::default();
        for (id, path) in sessions {
            let key = id.to_lowercase();
            index.exact.entry(key.clone()).or_default().insert(id);
            index.prefixes.entry(key).or_default().insert(id);
            index
                .suffixes
                .entry(compact(id).chars().rev().collect())
                .or_default()
                .insert(id);
            if let Some(stem) = path.and_then(Path::file_stem) {
                let stem = stem.to_string_lossy().to_lowercase();
                if let Some((_, suffix)) = stem.split_once('_') {
                    index
                        .prefixes
                        .entry(suffix.to_owned())
                        .or_default()
                        .insert(id);
                }
                index.prefixes.entry(stem).or_default().insert(id);
            }
        }
        index
    }
    fn add_handle(&mut self, id: &'a str, handle: &str) {
        self.exact
            .entry(handle.to_lowercase())
            .or_default()
            .insert(id);
    }
    fn find(&self, arg: &str, limit: usize) -> HashSet<&'a str> {
        let arg = arg.to_lowercase();
        let hex = arg.len() >= 4 && arg.bytes().all(|b| b.is_ascii_hexdigit());
        let reverse = arg.chars().rev().collect::<String>();
        let candidates = self
            .exact
            .get(&arg)
            .into_iter()
            .flatten()
            .chain(
                self.suffixes
                    .range(reverse.clone()..)
                    .take_while(|(key, _)| hex && key.starts_with(&reverse))
                    .flat_map(|(_, ids)| ids),
            )
            .chain(
                self.prefixes
                    .range(arg.clone()..)
                    .take_while(|(key, _)| arg.len() >= 8 && key.starts_with(&arg))
                    .flat_map(|(_, ids)| ids),
            );
        let mut found = HashSet::new();
        for id in candidates {
            found.insert(*id);
            if found.len() >= limit {
                break;
            }
        }
        found
    }
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
    assign_sessions(ids.into_iter().map(|id| (id, None)))
}
pub(crate) fn assign_sessions<'a>(
    sessions: impl IntoIterator<Item = (&'a str, Option<&'a Path>)>,
) -> HashMap<String, String> {
    #[cfg(test)]
    ASSIGN_CALLS.with(|count| count.set(count.get() + 1));
    let sessions = sessions.into_iter().collect::<Vec<_>>();
    let index = Matches::new(sessions.iter().copied());
    let ids = sessions
        .iter()
        .map(|(id, _)| *id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let normalized = ids.iter().map(|id| compact(id)).collect::<Vec<_>>();
    let prefixes = ids
        .iter()
        .map(|id| id.to_lowercase().chars().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let suffixes = normalized
        .iter()
        .map(|id| id.chars().rev().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let prefix_shared = shared_prefixes(&prefixes);
    let suffix_shared = shared_prefixes(&suffixes);
    let uuid = normalized
        .iter()
        .map(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect::<Vec<_>>();
    let chars = (0..ids.len())
        .map(|i| if uuid[i] { &suffixes[i] } else { &prefixes[i] })
        .collect::<Vec<_>>();
    let mut lengths = (0..ids.len())
        .map(|i| {
            7.max(
                if uuid[i] {
                    suffix_shared[i]
                } else {
                    prefix_shared[i]
                } + 1,
            )
            .min(chars[i].len())
        })
        .collect::<Vec<_>>();
    loop {
        let handles = (0..ids.len())
            .map(|i| {
                if uuid[i] {
                    chars[i][..lengths[i]].iter().rev().collect::<String>()
                } else {
                    chars[i][..lengths[i]].iter().collect::<String>()
                }
            })
            .collect::<Vec<_>>();
        let mut owners: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut collisions = vec![false; ids.len()];
        for (i, handle) in handles.iter().enumerate() {
            collisions[i] = index.find(handle, 2).iter().any(|id| *id != ids[i]);
            owners.entry(handle).or_default().push(i);
        }
        for group in owners.values().filter(|group| group.len() > 1) {
            for i in group {
                collisions[*i] = true;
            }
        }
        let mut widened = false;
        for i in 0..ids.len() {
            if collisions[i] && lengths[i] < chars[i].len() {
                lengths[i] += 1;
                widened = true;
            }
        }
        if !widened {
            return ids
                .iter()
                .zip(handles)
                .map(|(id, h)| ((*id).to_owned(), h))
                .collect();
        }
    }
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
    resolve_in(arg, catalog, env, None)
}
pub fn resolve_in(
    arg: &str,
    catalog: &Catalog,
    env: &Env,
    scope: Option<&Path>,
) -> Result<PathBuf, GlenError> {
    let scope = scope.map(|dir| super::paths::absolute(env, &dir.to_string_lossy()));
    let bucket = scope.as_deref().map(|dir| bucket_for(dir, env));
    // A lookup that misses while some session files could not be read may have
    // missed one of those, so report the read failure instead of not_found.
    // With a bucket, only unreadable paths in that bucket (the bucket directory
    // itself, or a file directly inside it) count.
    let unreadable = |bucket: Option<&str>| {
        let in_bucket = |path: &&PathBuf| {
            bucket.is_none_or(|bucket| {
                path.file_name().is_some_and(|n| n == bucket)
                    || path
                        .parent()
                        .and_then(Path::file_name)
                        .is_some_and(|n| n == bucket)
            })
        };
        catalog.unreadable.iter().find(in_bucket).map(|path| {
            GlenError::new(
                ErrorCode::Io,
                format!(
                    "no readable omp session matches `{arg}`; cannot read {}",
                    path.display()
                ),
            )
            .with_details(serde_json::json!({"unreadable":catalog.unreadable}))
            .with_hint("check the permissions of that path")
        })
    };
    let missing = || {
        unreadable(bucket.as_deref()).unwrap_or_else(|| {
            GlenError::new(
                ErrorCode::NotFound,
                format!("no omp session matches `{arg}`"),
            )
            .with_hint("run `glen ls --all`")
        })
    };
    if arg == "@latest" {
        let dir = scope.as_deref().unwrap_or(env.cwd());
        let bucket = bucket_for(dir, env);
        return catalog
            .sessions
            .iter()
            .find(|s| s.file.bucket == bucket)
            .map(|s| s.file.path.clone())
            .ok_or_else(|| {
                unreadable(Some(&bucket)).unwrap_or_else(|| {
                    GlenError::new(
                        ErrorCode::NotFound,
                        format!("no omp sessions for {}", dir.display()),
                    )
                    .with_hint("run `glen ls --all`")
                })
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
    let matching_ids = matching_ids(&a, catalog);
    let mut seen = HashSet::new();
    let matches = catalog
        .sessions
        .iter()
        .filter(|s| {
            matching_ids.contains(s.id.as_str())
                && bucket
                    .as_ref()
                    .is_none_or(|bucket| s.file.bucket == *bucket)
                && seen.insert(&s.file.path)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => {
            if let Some(dir) = scope.as_deref()
                && let Some(elsewhere) = catalog
                    .sessions
                    .iter()
                    .find(|s| matching_ids.contains(s.id.as_str()))
            {
                return Err(GlenError::new(
                    ErrorCode::NotFound,
                    format!("`{arg}` is not a session in {}", dir.display()),
                )
                .with_details(
                    serde_json::json!({"reason":"outside_cwd","cwd":dir.to_string_lossy()}),
                )
                .with_hint(format!(
                    "it is in {}; drop --cwd or run `glen ls --all`",
                    elsewhere.cwd.as_deref().unwrap_or(&elsewhere.file.bucket)
                )));
            }
            Err(missing())
        }
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
pub(crate) fn matching_ids<'a>(arg: &str, catalog: &'a Catalog) -> HashSet<&'a str> {
    let mut index = Matches::new(
        catalog
            .sessions
            .iter()
            .map(|session| (session.id.as_str(), Some(session.file.path.as_path()))),
    );
    for (id, handle) in &catalog.handles {
        index.add_handle(id, handle);
    }
    index.find(arg, usize::MAX)
}

#[cfg(test)]
mod resolution_tests {
    use super::*;
    use crate::omp::fixtures;
    #[test]
    fn scoped_matches_disambiguate_and_keep_paths_process_relative() {
        use std::fs;
        let tree = tempfile::tempdir().unwrap();
        let alpha = tree.path().join("-work-alpha");
        let beta = tree.path().join("-work-beta");
        fs::create_dir(&alpha).unwrap();
        fs::create_dir(&beta).unwrap();
        let a = "11111111-1111-7111-8111-111111234567";
        let b = "22222222-2222-7222-8222-222221234567";
        for (dir, id, cwd) in [
            (&alpha, a, "/home/fixture/work/alpha"),
            (&beta, b, "/home/fixture/work/beta"),
        ] {
            fs::write(
                dir.join("one.jsonl"),
                format!(
                    "{}\n",
                    serde_json::json!({"type":"session","version":3,"id":id,"cwd":cwd})
                ),
            )
            .unwrap();
        }
        let catalog = Catalog::scan(tree.path()).unwrap();
        let env = Env::for_test(
            Some(Path::new("/home/fixture")),
            &alpha,
            &[("TMPDIR", "/unrelated-temp")],
        );
        let scope = Path::new("/home/fixture/work/alpha");
        assert_eq!(
            resolve("1234567", &catalog, &env).unwrap_err().code,
            ErrorCode::Ambiguous
        );
        assert_eq!(
            resolve_in("1234567", &catalog, &env, Some(scope)).unwrap(),
            alpha.join("one.jsonl")
        );
        assert_eq!(
            resolve_in("@latest", &catalog, &env, Some(scope)).unwrap(),
            alpha.join("one.jsonl")
        );
        for arg in [b, "22222222"] {
            let error = resolve_in(arg, &catalog, &env, Some(scope)).unwrap_err();
            assert_eq!(error.code, ErrorCode::NotFound);
            assert_eq!(error.details.as_ref().unwrap()["reason"], "outside_cwd");
            assert_eq!(
                error.details.as_ref().unwrap()["cwd"],
                "/home/fixture/work/alpha"
            );
            assert!(error.hint.unwrap().contains("/home/fixture/work/beta"));
        }
        assert_eq!(
            resolve_in(
                "one.jsonl",
                &catalog,
                &env,
                Some(Path::new("/home/fixture/work/beta"))
            )
            .unwrap(),
            alpha.join("one.jsonl")
        );
        let error = resolve_in("ffffffff", &catalog, &env, Some(scope)).unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(error.details.is_none());
    }
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
            "abcdef0-legacy",
            "Legacy-alpha",
            "legacy-beta",
            "0193a4b5c6d7e8f9",
            "01a2b3c4-0000-7000-8000-0000f0193a4b",
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

    #[test]
    fn every_handle_round_trips_for_generated_catalogs() {
        let env = Env::for_test(Some(Path::new("/fixture")), Path::new("/fixture"), &[]);
        // Exhaust every subset, with both insertion orders, of mixed id kinds.
        let ids = [
            "00000000-0000-0000-0000-00001abcdef0",
            "00000000-0000-0000-0000-00002abcdef0",
            "abcdef0-legacy",
            "Legacy-alpha",
            "legacy-beta",
            "0193a4b5c6d7e8f9",
            "01a2b3c4-0000-7000-8000-0000f0193a4b",
            "abc",
            "abcdefgh1",
            "abcdefgh2",
        ];
        let sessions = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let path = Path::new("/fixture").join(format!("{id}.jsonl"));
                let file = SessionFile {
                    path,
                    bucket: "fixture".into(),
                    mtime: std::time::SystemTime::UNIX_EPOCH,
                    size: 0,
                };
                let session = super::super::session::Session::parse(
                    &file.path,
                    format!(
                        "{{\"type\":\"session\",\"id\":\"{id}\",\"version\":3,\"index\":{i}}}\n"
                    )
                    .as_bytes(),
                )
                .unwrap();
                super::super::listing::summarize(file, &session)
            })
            .collect::<Vec<_>>();
        for mask in 0..1 << ids.len() {
            let selected = sessions
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, s)| s.clone())
                .collect::<Vec<_>>();
            for reverse in [false, true] {
                let mut catalog = Catalog {
                    sessions_dir: "/fixture".into(),
                    sessions: selected.clone(),
                    handles: HashMap::new(),
                    unreadable: vec![],
                };
                if reverse {
                    catalog.sessions.reverse();
                }
                catalog.assign_handles(None);
                for s in &catalog.sessions {
                    for handle in [catalog.handle(&s.id), catalog.handle(&s.id).to_uppercase()] {
                        assert_eq!(
                            resolve(&handle, &catalog, &env).unwrap(),
                            s.file.path,
                            "mask {mask}: {handle}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn widened_handles_also_avoid_filename_prefix_matches() {
        let tree = fixtures::tree();
        let a = "00000000-0000-0000-0000-00001abcdef0";
        let b = "00000000-0000-0000-0000-00002abcdef0";
        for (id, name) in [
            (a, "one"),
            (b, "two"),
            ("legacy-session", "2026-10-30_1abcdef0-other"),
        ] {
            fs::write(
                tree.path()
                    .join("-work-alpha")
                    .join(format!("{name}.jsonl")),
                format!("{{\"type\":\"session\",\"id\":\"{id}\"}}\n"),
            )
            .unwrap();
        }
        let catalog = Catalog::scan(tree.path()).unwrap();
        assert_eq!(catalog.handle(a), "01abcdef0");
        let env = Env::for_test(Some(Path::new("/fixture")), Path::new("/fixture"), &[]);
        for s in &catalog.sessions {
            assert_eq!(
                resolve(&catalog.handle(&s.id), &catalog, &env).unwrap(),
                s.file.path
            );
        }
    }
}
