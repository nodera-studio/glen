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

/// Checks §A7's `task_state` table. Returns the first violation.
fn check_task_state_table(a7: &str) -> Result<(), String> {
    for needle in ["`task_state`", "introduced in v0.2 W2"] {
        if !a7.contains(needle) {
            return Err(format!("§A7 does not name {needle}"));
        }
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
    if let Some(row) = rows.iter().find(|r| r.len() != 3) {
        return Err(format!("§A7 task_state row without 3 cells: {row:?}"));
    }
    let pairs: Vec<(&str, &str)> = rows.iter().map(|r| (r[0], r[2])).collect();
    let expected = [
        ("`queued`", "`submitted`"),
        ("`accepted`", "`working`"),
        ("`completed`", "`completed`"),
        ("`aborted`", "`canceled`"),
        ("`failed`", "`rejected`"),
        ("`failed`", "`failed`"),
        ("`lost`", "`failed`"),
        ("—", "`input_required`"),
    ];
    if pairs != expected {
        return Err(format!("§A7 task_state table rows: {rows:#?}"));
    }
    // (row, needles its Condition cell must hold, lowercase needles it must not hold); no
    // needles means an empty cell. The two `failed` rows are complementary: `rejected` matches
    // `no_route` and only it, and `failed` is the fallback for every other code and never
    // names `no_route`.
    let conditions: [(usize, &[&str], &[&str]); 8] = [
        (0, &[], &[]),
        (1, &[], &[]),
        (2, &["forked", "(exit 11)"], &[]),
        (3, &["`cancelled`"], &[]),
        (
            4,
            &["`error.code` is `no_route`", "(exit 6)"],
            &[" not ", "other"],
        ),
        (5, &["any other `error.code`"], &["`no_route`"]),
        (6, &[], &[]),
        (7, &["ACP", "§A23"], &[]),
    ];
    for (i, must, must_not) in conditions {
        let cell = rows[i][1];
        let lower = cell.to_lowercase();
        let ok = if must.is_empty() {
            cell.is_empty()
        } else {
            must.iter().all(|n| cell.contains(n)) && !must_not.iter().any(|n| lower.contains(n))
        };
        if !ok {
            return Err(format!(
                "§A7 task_state row {} condition {cell:?}: wants {must:?}, not {must_not:?}",
                i + 1
            ));
        }
    }
    Ok(())
}

/// The `(id, description)` of each `- **v0.2 <id>** <description>` bullet in §A16.
fn wave_bullets(a16: &str) -> Result<Vec<(&str, &str)>, String> {
    a16.lines()
        .filter_map(|l| l.strip_prefix("- **v0.2 "))
        .map(|rest| {
            rest.split_once("**")
                .map(|(id, desc)| (id, desc.trim()))
                .ok_or_else(|| format!("§A16 wave bullet without a closing `**`: {rest}"))
        })
        .collect()
}

/// Checks §A16's v0.2.0 wave map. Returns the first violation.
fn check_wave_map(a16: &str) -> Result<(), String> {
    if !a16.contains("**v0.2.0**") {
        return Err("§A16 has no v0.2.0 map".to_owned());
    }
    let bullets = wave_bullets(a16)?;
    // Each wave's essential assignment as keywords, not its full prose.
    let expected: [(&str, &[&str]); 8] = [
        ("W0", &["foundation"]),
        ("W1", &["omp compatibility", "nightly"]),
        ("W2", &["push", "reply"]),
        ("W3", &["ACP"]),
        ("W4", &["warm"]),
        ("W5", &["MCP"]),
        ("W6", &["tech debt", "audits"]),
        ("W7", &["release v0.2.0"]),
    ];
    let ids: Vec<&str> = bullets.iter().map(|(id, _)| *id).collect();
    if ids != expected.map(|(id, _)| id) {
        return Err(format!("§A16 v0.2 waves: {ids:?}"));
    }
    for ((id, desc), (_, keywords)) in bullets.iter().zip(expected) {
        if let Some(k) = keywords.iter().find(|k| !desc.contains(*k)) {
            return Err(format!("§A16 v0.2 {id} does not name {k:?}: {desc}"));
        }
    }
    Ok(())
}

#[test]
fn acv2_002_design_doc_has_the_v02_wave_map_and_task_state_table() {
    let doc = read("docs/design.md");
    let a7 = design_section(&doc, 7);
    check_task_state_table(&a7).unwrap_or_else(|e| panic!("{e}\n{a7}"));
    let a16 = design_section(&doc, 16);
    check_wave_map(&a16).unwrap_or_else(|e| panic!("{e}\n{a16}"));
}

/// `section` with the first `from` replaced by `to`; panics when `from` is absent, so a
/// negative check never runs against an unchanged section.
fn mutate(section: &str, from: &str, to: &str) -> String {
    assert!(section.contains(from), "mutation source {from:?} not found");
    section.replacen(from, to, 1)
}

#[test]
fn acv2_002_task_state_check_rejects_an_inverted_or_overlapping_failed_row() {
    let a7 = design_section(&read("docs/design.md"), 7);
    let mutations = [
        // Inverted: the `rejected` row no longer matches `no_route`.
        (
            "`error.code` is `no_route` (exit 6)",
            "`error.code` is NOT `no_route` (exit 6)",
        ),
        // Overlapping: the fallback row also matches `no_route`.
        (
            "any other `error.code`",
            "any `error.code`, including `no_route`",
        ),
        ("any other `error.code`", "any `error.code`"),
        // Overlapping: the `rejected` row also matches other codes.
        (
            "`error.code` is `no_route` (exit 6)",
            "`error.code` is `no_route` or any other code (exit 6)",
        ),
        // The note cells: forked completion and the worker-signal code.
        ("forked too (exit 11)", ""),
        ("includes the worker-signal `cancelled` code", ""),
    ];
    let accepted: Vec<_> = mutations
        .into_iter()
        .filter(|(from, to)| check_task_state_table(&mutate(&a7, from, to)).is_ok())
        .collect();
    assert!(accepted.is_empty(), "§A7 check accepts: {accepted:#?}");
}

#[test]
fn acv2_002_wave_map_check_rejects_a_renamed_or_swapped_wave() {
    let a16 = design_section(&read("docs/design.md"), 16);
    let bullets = wave_bullets(&a16).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((bullets[3].0, bullets[4].0), ("W3", "W4"));
    let (d3, d4) = (bullets[3].1, bullets[4].1);
    let mutations = [
        (
            "W3 renamed W30",
            mutate(&a16, "- **v0.2 W3**", "- **v0.2 W30**"),
        ),
        (
            "W3/W4 descriptions swapped",
            mutate(
                &mutate(&a16, &format!("W3** {d3}"), &format!("W3** {d4}")),
                &format!("W4** {d4}"),
                &format!("W4** {d3}"),
            ),
        ),
    ];
    let accepted: Vec<_> = mutations
        .iter()
        .filter(|(_, mutated)| check_wave_map(mutated).is_ok())
        .map(|(name, _)| name)
        .collect();
    assert!(accepted.is_empty(), "§A16 check accepts: {accepted:?}");
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
