//! Repository-level acceptance criteria: the design document, the harness issue references and
//! the CI workflow (AC-024, AC-025, AC-026; AC-010's exit-code table; v0.2 W0's AC-v2-001 and
//! AC-v2-002).

use std::collections::BTreeMap;
use std::fs;

use crate::support::*;

fn read(rel: &str) -> String {
    fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

#[test]
fn ac024_acv2_001_design_doc_has_sections_a1_to_a23_in_order() {
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
        "Distribution",
        "omp compatibility",
        "Push to the calling agent",
        "Warm worker",
        "MCP layer",
        "ACP spike",
    ];
    assert_eq!(headings.len(), 23, "{headings:#?}");
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
    for needle in ["v0.2.0", "W0", TESTED_OMP] {
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

/// The body of `## §A<n> ` in `doc`: the lines after its heading up to the next `## ` line.
fn design_section(doc: &str, n: u32) -> String {
    let prefix = format!("## §A{n} ");
    let mut lines = doc.lines().skip_while(|l| !l.starts_with(&prefix));
    assert!(
        lines.next().is_some(),
        "docs/design.md has no §A{n} heading"
    );
    lines
        .take_while(|l| !l.starts_with("## "))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn acv2_002_design_doc_has_the_v02_wave_map_and_task_state_table() {
    let doc = read("docs/design.md");

    let a7 = design_section(&doc, 7);
    for needle in ["`task_state`", "introduced in v0.2 W2"] {
        assert!(a7.contains(needle), "§A7 names {needle}:\n{a7}");
    }
    let rows: Vec<Vec<&str>> = a7
        .lines()
        .skip_while(|l| !(l.starts_with('|') && l.contains("task_state")))
        .skip(2)
        .take_while(|l| l.starts_with('|'))
        .map(|l| {
            l.trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect::<Vec<_>>()
        })
        .collect();
    for row in &rows {
        assert_eq!(row.len(), 3, "§A7 task_state row has 3 cells: {row:?}");
    }
    let pairs: Vec<(&str, &str)> = rows.iter().map(|r| (r[0], r[2])).collect();
    assert_eq!(
        pairs,
        [
            ("`queued`", "`submitted`"),
            ("`accepted`", "`working`"),
            ("`completed`", "`completed`"),
            ("`aborted`", "`canceled`"),
            ("`failed`", "`rejected`"),
            ("`failed`", "`failed`"),
            ("`lost`", "`failed`"),
            ("—", "`input_required`"),
        ],
        "§A7 task_state table rows: {rows:#?}"
    );
    assert!(rows[4][1].contains("`no_route`"), "row 5: {:?}", rows[4]);
    assert!(rows[7][1].contains("ACP"), "row 8: {:?}", rows[7]);

    let a16 = design_section(&doc, 16);
    assert!(
        a16.contains("**v0.2.0**"),
        "§A16 has the v0.2.0 map:\n{a16}"
    );
    let waves: Vec<u32> = a16
        .lines()
        .filter_map(|l| l.strip_prefix("- **v0.2 W"))
        .map(|rest| {
            rest.chars()
                .next()
                .and_then(|c| c.to_digit(10))
                .unwrap_or_else(|| panic!("§A16 wave bullet without a digit: {rest}"))
        })
        .collect();
    assert_eq!(waves, [0, 1, 2, 3, 4, 5, 6, 7], "§A16 v0.2 waves:\n{a16}");
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
        ("control_failed", 7),
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
    // W8 SEC-7: rust-cache is pinned to a v2 commit SHA instead of the moving v2 tag.
    assert!(ci.lines().any(|line| {
        line.trim_start()
            .starts_with("- uses: Swatinem/rust-cache@")
            && line.contains(" # v2.")
    }));
    assert!(ci.contains("cache-on-failure: true"));
    assert!(!ci.contains("save-if"), "no save-if (plan Step 11)");
    assert!(ci.contains("ubuntu-latest") && ci.contains("macos-latest"));
    for step in ["cargo fmt", "cargo clippy", "cargo test"] {
        assert!(ci.contains(step), "CI runs {step}");
    }
}
