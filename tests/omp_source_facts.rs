//! omp source facts (AC-v2-105): literal facts about omp's source that glen relies on and that no
//! version number protects (RPC commands, CLI flags, lease names, session entries), checked
//! against an omp source tree so an upstream change fails loudly instead of rotting in
//! `docs/design.md` §A2.
//!
//! The manifest is `tests/fixtures/omp-surface/facts.json`. The tree is `GLEN_OMP_SOURCE` when set
//! (it must be a directory), else the sibling reference clone `../metis/references/oh-my-pi`
//! when it exists, else only the manifest and the matcher are checked. The test only reads.
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};
const MANIFEST: &str = include_str!("fixtures/omp-surface/facts.json");
const COMPAT: &str = include_str!("../src/omp/compat.json");
const MIN_FACTS: usize = 20;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    v: u32,
    omp: String,
    facts: Vec<Fact>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fact {
    id: String,
    file: String,
    contains: String,
}
fn manifest() -> Manifest {
    serde_json::from_str(MANIFEST).expect("facts.json parses")
}
fn tested() -> String {
    let compat: serde_json::Value = serde_json::from_str(COMPAT).unwrap();
    compat["tested"].as_str().unwrap().to_owned()
}
/// A relative path inside the source root: no `..`, no root, no prefix.
fn safe(file: &str) -> bool {
    !file.is_empty()
        && Path::new(file)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}
/// Problems with the manifest itself.
fn validate(m: &Manifest, tested: &str) -> Vec<String> {
    let mut problems = vec![];
    if m.v != 1 {
        problems.push(format!("v is {}, expected 1", m.v));
    }
    if m.omp != tested {
        problems.push(format!(
            "omp is {}, but compat.json tests {tested}: re-check every fact, then bump it",
            m.omp
        ));
    }
    if m.facts.len() < MIN_FACTS {
        problems.push(format!(
            "{} facts, expected at least {MIN_FACTS}",
            m.facts.len()
        ));
    }
    let mut ids = BTreeSet::new();
    for f in &m.facts {
        if f.id.is_empty() || !ids.insert(&f.id) {
            problems.push(format!("duplicate or empty id {:?}", f.id));
        }
        if !safe(&f.file) {
            problems.push(format!("{}: unsafe path {:?}", f.id, f.file));
        }
        if f.contains.is_empty() {
            problems.push(format!("{}: empty literal", f.id));
        }
    }
    problems
}
/// Every fact the tree at `root` does not satisfy, and every file it cannot read.
fn check(root: &Path, facts: &[Fact]) -> Vec<String> {
    let mut files: BTreeMap<&str, Result<String, String>> = BTreeMap::new();
    let mut problems = vec![];
    for f in facts {
        if !safe(&f.file) {
            problems.push(format!("{}: unsafe path {:?}", f.id, f.file));
            continue;
        }
        let text = files.entry(&f.file).or_insert_with(|| {
            std::fs::read_to_string(root.join(&f.file)).map_err(|e| e.to_string())
        });
        match text {
            Ok(text) if text.contains(&f.contains) => (),
            Ok(_) => problems.push(format!("{}: {} lacks {:?}", f.id, f.file, f.contains)),
            Err(e) => problems.push(format!("{}: cannot read {}: {e}", f.id, f.file)),
        }
    }
    problems
}
fn source_root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("GLEN_OMP_SOURCE") {
        let root = PathBuf::from(root);
        assert!(
            root.is_dir(),
            "GLEN_OMP_SOURCE is not a directory: {}",
            root.display()
        );
        return Some(root);
    }
    let sibling = Path::new(env!("CARGO_MANIFEST_DIR")).join("../metis/references/oh-my-pi");
    sibling.is_dir().then_some(sibling)
}
#[test]
fn manifest_is_valid() {
    let problems = validate(&manifest(), &tested());
    assert!(problems.is_empty(), "facts.json:\n{}", problems.join("\n"));
}
#[test]
fn facts_hold_in_the_omp_source_tree() {
    let Some(root) = source_root() else {
        eprintln!(
            "omp_source_facts: no omp source tree (set GLEN_OMP_SOURCE); checked the manifest only"
        );
        return;
    };
    let m = manifest();
    let problems = check(&root, &m.facts);
    assert!(
        problems.is_empty(),
        "{} of {} omp source facts fail under {}:\n{}",
        problems.len(),
        m.facts.len(),
        root.display(),
        problems.join("\n")
    );
    println!(
        "omp_source_facts: {} facts hold under {}",
        m.facts.len(),
        root.display()
    );
}
/// The matcher passes a present literal and fails a missing file, a mutated literal and an
/// unsafe path, so a skipped tree cannot hide a broken matcher.
#[test]
fn matcher_self_test() {
    let tree = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tree.path().join("pkg/src")).unwrap();
    std::fs::write(
        tree.path().join("pkg/src/a.ts"),
        "export const CURRENT_SESSION_VERSION = 3;\n",
    )
    .unwrap();
    let fact = |id: &str, file: &str, contains: &str| Fact {
        id: id.into(),
        file: file.into(),
        contains: contains.into(),
    };
    assert!(
        check(
            tree.path(),
            &[fact("ok", "pkg/src/a.ts", "CURRENT_SESSION_VERSION = 3;")]
        )
        .is_empty()
    );
    for (bad, needle) in [
        (fact("gone", "pkg/src/b.ts", "x"), "cannot read"),
        (
            fact("mutated", "pkg/src/a.ts", "CURRENT_SESSION_VERSION = 4;"),
            "lacks",
        ),
        (fact("escape", "../a.ts", "x"), "unsafe path"),
        (fact("absolute", "/etc/hosts", "x"), "unsafe path"),
    ] {
        let problems = check(tree.path(), std::slice::from_ref(&bad));
        assert_eq!(problems.len(), 1, "{}: {problems:?}", bad.id);
        assert!(problems[0].contains(needle), "{}: {problems:?}", bad.id);
    }
    // One message lists every failing fact.
    let all = check(
        tree.path(),
        &[
            fact("gone", "pkg/src/b.ts", "x"),
            fact("mutated", "pkg/src/a.ts", "nope"),
        ],
    );
    assert_eq!(all.len(), 2, "{all:?}");
    // The manifest checks reject what the real manifest must never contain.
    let mut m = manifest();
    assert!(validate(&m, "0.0.1")[0].contains("compat.json"));
    m.facts.push(fact("rpc.cmd.get_state", "../x", ""));
    let problems = validate(&m, &tested()).join("\n");
    for needle in ["duplicate", "unsafe path", "empty literal"] {
        assert!(problems.contains(needle), "{needle}: {problems}");
    }
    m.facts.truncate(3);
    m.v = 2;
    let problems = validate(&m, &tested()).join("\n");
    assert!(
        problems.contains("at least") && problems.contains("v is 2"),
        "{problems}"
    );
}
