//! Repository-level acceptance criteria: the design document, the harness issue references and
//! the CI workflow (AC-024, AC-025, AC-026; AC-010's exit-code table).

use std::collections::BTreeMap;
use std::fs;

use crate::support::*;

fn read(rel: &str) -> String {
    fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

#[test]
fn ac024_design_doc_has_sections_a1_to_a17_in_order() {
    let doc = read("docs/design.md");
    let headings: Vec<&str> = doc.lines().filter(|l| l.starts_with("## ")).collect();
    let titles = [
        "Scope and non-goals",
        "omp surfaces",
        "three channels",
        "Routing",
        "Reply channel",
        "glen state",
        "Jobs",
        "Handles",
        "Commands",
        "Output contract",
        "Exit codes",
        "omp path resolution",
        "Claude Code and Codex",
        "Security and privacy",
        "Testing strategy",
        "Wave map",
        "Verify on every omp upgrade",
    ];
    assert_eq!(headings.len(), 17, "{headings:#?}");
    for (i, (h, title)) in headings.iter().zip(titles).enumerate() {
        let prefix = format!("## §A{} ", i + 1);
        assert!(h.starts_with(&prefix), "heading {i}: {h}");
        assert!(h.contains(title), "heading {h} should be about {title:?}");
    }
}

#[test]
fn ac024_design_doc_opening_and_key_content() {
    let doc = read("docs/design.md");
    let head: String = doc.lines().take(8).collect::<Vec<_>>().join("\n");
    for needle in ["v0.1.0", "W0", TESTED_OMP] {
        assert!(head.contains(needle), "opening states {needle}:\n{head}");
    }
    for code in ["omp_unavailable", "no_route", "not_implemented"] {
        assert!(doc.contains(code), "design names {code}");
    }
    // Both lease sources are cited (plan Step 1 gotcha).
    assert!(doc.contains("session-storage.ts:618-639"));
    assert!(doc.contains("file-lock.ts:31-33, 43-45"));
    // §A4 states the residual race; routing is not sold as a guarantee.
    assert!(!doc.to_lowercase().contains("no-fork guarantee"));
    assert!(doc.contains("Residual race"));
}

#[test]
fn ac010_design_exit_table_matches_the_plan() {
    let expected: BTreeMap<String, i32> = [
        ("internal", 1),
        ("io", 1),
        ("usage", 2),
        ("invalid_env", 2),
        ("not_found", 3),
        ("ambiguous", 4),
        ("omp_unavailable", 5),
        ("no_route", 6),
        ("job_failed", 7),
        ("not_implemented", 8),
        ("running", 10),
        ("forked", 11),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    assert_eq!(design_exit_table(), &expected);
}

#[test]
fn ac024_agents_md_links_the_design() {
    let agents = read("AGENTS.md");
    assert!(
        agents.contains("](docs/design.md)"),
        "AGENTS.md links docs/design.md"
    );
}

#[test]
fn ac025_harness_refers_to_issues_by_wave_and_document_id() {
    for rel in [".claude/CLAUDE.md", ".claude/skills/mnemosyne/SKILL.md"] {
        let text = read(rel);
        assert!(!text.contains("<PREFIX>"), "{rel} still has <PREFIX>");
        assert!(!text.contains("UNCONFIRMED"), "{rel} still has UNCONFIRMED");
        assert!(text.contains("W<n>"), "{rel} names the W<n> label");
        assert!(text.contains("document id"), "{rel} names the document id");
    }
    // Step 10's backward-check files.
    assert!(!read(".claude/skills/release/SKILL.md").contains("issue keys"));
    assert!(!read(".claude/commands/auto-mode.md").contains("issue key"));
}

#[test]
fn ac026_ci_caches_cargo_with_rust_cache_on_failure() {
    let ci = read(".github/workflows/ci.yml");
    assert!(ci.contains("Swatinem/rust-cache@v2"));
    assert!(ci.contains("cache-on-failure: true"));
    assert!(!ci.contains("save-if"), "no save-if (plan Step 11)");
    assert!(ci.contains("ubuntu-latest") && ci.contains("macos-latest"));
    for step in ["cargo fmt", "cargo clippy", "cargo test"] {
        assert!(ci.contains(step), "CI runs {step}");
    }
}
