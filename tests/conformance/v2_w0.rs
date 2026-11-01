//! v0.2 W0 (foundation) conformance: AC-v2-001, AC-v2-002 and AC-v2-003, plus design §A20's
//! per-caller table. Derived from the settled acceptance criteria and the contracts they name
//! (`docs/design.md`, `tests/conformance.rs`, `CHANGELOG.md`, `glen --help`), not from any
//! implementation.
//!
//! W0 changes docs and tests only. Three checks describe the W0 "no behaviour change" window
//! (the binary inputs equal v0.1.1, the top-level help has exactly the 18 v0.1.1 commands, and
//! the per-caller table has its five W0 rows). That window closes when v0.2 W1 starts: W1 replaces
//! "planned" on its own line of the `tests/conformance.rs` module doc, and from then on those
//! checks return early so that later waves are not made to edit a W0 test. Every other check
//! here is permanent.

use std::fs;
use std::process::Command;

use crate::support::{PKG_VERSION, Sandbox, TESTED_OMP, VISIBLE, repo_root};

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn read(rel: &str) -> String {
    fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn design() -> String {
    read("docs/design.md")
}

/// Lines of a Markdown document with the lines inside fenced code blocks dropped.
fn prose_lines(doc: &str) -> Vec<&str> {
    let mut in_fence = false;
    let mut out = Vec::new();
    for line in doc.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            out.push(line);
        }
    }
    out
}

/// Text after `## ` of every level-2 heading, in document order.
fn h2_headings(doc: &str) -> Vec<String> {
    prose_lines(doc)
        .into_iter()
        .filter_map(|l| l.strip_prefix("## ").map(str::to_owned))
        .collect()
}

/// Body lines of design section `## §A<n> ...`: after its heading, up to the next `## ` line.
fn section(doc: &str, n: u32) -> Vec<String> {
    let prefix = format!("## §A{n} ");
    let lines = prose_lines(doc);
    let start = lines
        .iter()
        .position(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("docs/design.md has no `## §A{n} ` heading"));
    lines[start + 1..]
        .iter()
        .take_while(|l| !l.starts_with("## "))
        .map(|l| (*l).to_owned())
        .collect()
}

struct Table {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

fn cells(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    inner.split('|').map(|c| c.trim().to_owned()).collect()
}

fn is_separator(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells
            .iter()
            .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
}

/// Every pipe table in the lines: a run of `|` lines whose second line is a separator row.
fn tables(lines: &[String]) -> Vec<Table> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].trim_start().starts_with('|') {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && lines[i].trim_start().starts_with('|') {
            i += 1;
        }
        let run = &lines[start..i];
        if run.len() >= 2 && is_separator(&cells(&run[1])) {
            out.push(Table {
                header: cells(&run[0]),
                rows: run[2..].iter().map(|l| cells(l)).collect(),
            });
        }
    }
    out
}

/// A cell without Markdown code ticks.
fn plain(cell: &str) -> String {
    cell.replace('`', "").trim().to_owned()
}

/// The leading `//!` module-doc lines of `tests/conformance.rs`.
fn conformance_module_doc() -> Vec<String> {
    read("tests/conformance.rs")
        .lines()
        .take_while(|l| l.starts_with("//!"))
        .map(str::to_owned)
        .collect()
}

/// `//! - v0.2 W<n> ...` lines of the module doc, as (n, line).
fn v02_wave_lines(doc: &[String]) -> Vec<(u32, String)> {
    doc.iter()
        .filter_map(|l| {
            let rest = l
                .strip_prefix("//!")?
                .trim_start()
                .strip_prefix("- v0.2 W")?;
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            Some((digits.parse().ok()?, l.clone()))
        })
        .collect()
}

/// The W0 "no behaviour change" window is open until v0.2 W1 starts, which the module doc shows
/// by W1's line no longer ending in "planned.".
fn w0_window_open() -> bool {
    let doc = conformance_module_doc();
    let (_, line) = v02_wave_lines(&doc)
        .into_iter()
        .find(|(n, _)| *n == 1)
        .expect("tests/conformance.rs module doc has a `v0.2 W1` line (AC-v2-003)");
    line.trim_end().ends_with("planned.")
}

/// The `(command)` names under `Commands:` in `glen --help`.
fn help_commands(help: &str) -> Vec<String> {
    let mut in_commands = false;
    let mut out = Vec::new();
    for line in help.lines() {
        if line.trim_end() == "Commands:" {
            in_commands = true;
            continue;
        }
        if !in_commands {
            continue;
        }
        if line.trim().is_empty() {
            break;
        }
        // Command rows are indented by two spaces; wrapped descriptions are indented deeper.
        if let Some(rest) = line.strip_prefix("  ")
            && !rest.starts_with(' ')
            && let Some(name) = rest.split_whitespace().next()
        {
            out.push(name.to_owned());
        }
    }
    out
}

const A2A_VOCABULARY: [&str; 7] = [
    "submitted",
    "working",
    "completed",
    "canceled",
    "rejected",
    "failed",
    "input_required",
];
const JOB_STATUSES: [&str; 6] = [
    "queued",
    "accepted",
    "completed",
    "aborted",
    "failed",
    "lost",
];

/// `## §A<n> <title>` titles of the v0.1 design, unchanged by v0.2 W0 (prefix match).
const V01_TITLES: [&str; 17] = [
    "Scope and non-goals",
    "omp surfaces",
    "The three channels",
    "Routing",
    "Reply channel",
    "glen state",
    "Jobs",
    "Handles",
    "Commands",
    "Output contract",
    "Exit codes",
    "omp path resolution",
    "How Claude Code and Codex use glen",
    "Security and privacy",
    "Testing strategy",
    "Wave map",
    "Verify on every omp upgrade",
];
/// §A18 and the v0.2 stubs, in order: exact titles.
const V02_TITLES: [&str; 6] = [
    "Distribution",
    "omp compatibility",
    "Push to the calling agent",
    "Warm worker",
    "MCP layer",
    "ACP spike",
];

// ---------------------------------------------------------------------------------------------
// AC-v2-001: 23 `## §A<n>` headings in order; §A18 level 2; §A19-§A23 stubs
// ---------------------------------------------------------------------------------------------

#[test]
fn ac_v2_001_design_has_23_level_2_section_headings_in_order() {
    let doc = design();
    let headings = h2_headings(&doc);
    assert_eq!(
        headings.len(),
        23,
        "`## ` headings in docs/design.md: {headings:#?}"
    );
    let titles: Vec<&str> = V01_TITLES
        .iter()
        .chain(V02_TITLES.iter())
        .copied()
        .collect();
    assert_eq!(titles.len(), 23);
    for (i, (heading, title)) in headings.iter().zip(&titles).enumerate() {
        let expected = format!("§A{} {}", i + 1, title);
        assert!(
            heading.starts_with(&expected),
            "heading {} should start with {expected:?}, got {heading:?}\n{headings:#?}",
            i + 1
        );
    }
}

#[test]
fn ac_v2_001_a18_distribution_is_a_level_2_section_and_no_section_is_level_3() {
    let doc = design();
    let lines = prose_lines(&doc);
    assert!(
        lines.iter().any(|l| l.trim_end() == "## §A18 Distribution"),
        "docs/design.md has no level-2 `## §A18 Distribution` heading"
    );
    let level3: Vec<&&str> = lines.iter().filter(|l| l.starts_with("### §A")).collect();
    assert!(
        level3.is_empty(),
        "a `§A<n>` section is still level 3: {level3:?}"
    );
    assert!(
        !section(&doc, 18).iter().all(|l| l.trim().is_empty()),
        "§A18 Distribution has no body"
    );
}

#[test]
fn ac_v2_001_a19_to_a23_are_titled_and_each_has_a_body() {
    let doc = design();
    let headings = h2_headings(&doc);
    let expected = [
        (19, "omp compatibility"),
        (20, "Push to the calling agent"),
        (21, "Warm worker"),
        (22, "MCP layer"),
        (23, "ACP spike"),
    ];
    for (n, title) in expected {
        assert_eq!(
            headings.get(n - 1).map(String::as_str),
            Some(format!("§A{n} {title}").as_str()),
            "heading {n} of docs/design.md\n{headings:#?}"
        );
        let body = section(&doc, n as u32);
        let text: String = body.iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
        let non_blank = body.iter().filter(|l| !l.trim().is_empty()).count();
        assert!(
            non_blank >= 2 && text.chars().count() >= 80,
            "§A{n} {title} is an empty stub ({non_blank} non-blank lines):\n{body:#?}"
        );
    }
    // The stubs close the document: nothing is a level-2 heading after §A23.
    assert_eq!(
        headings.last().map(String::as_str),
        Some("§A23 ACP spike"),
        "§A23 must be the last section"
    );
}

// ---------------------------------------------------------------------------------------------
// AC-v2-002: head names v0.2.0, W0 and TESTED_OMP; §A16 wave map; §A7 task_state table
// ---------------------------------------------------------------------------------------------

#[test]
fn ac_v2_002_first_8_lines_of_the_design_name_v0_2_0_w0_and_the_tested_omp_version() {
    let doc = design();
    let head: Vec<&str> = doc.lines().take(8).collect();
    let joined = head.join("\n");
    for needle in ["v0.2.0", "W0", TESTED_OMP] {
        assert!(
            joined.contains(needle),
            "the first 8 lines of docs/design.md must name {needle:?}:\n{joined}"
        );
    }
    assert!(
        head[0].starts_with("# ") && head[0].contains("v0.2.0"),
        "the title line must be a v0.2.0 design title, got {:?}",
        head[0]
    );
}

#[test]
fn ac_v2_002_design_head_names_the_omp_version_that_glen_reports_as_tested() {
    let sb = Sandbox::new();
    let data = sb.glen(["--json", "doctor"]).fake_omp().run().ok_data();
    let tested = data["omp"]["tested_version"]
        .as_str()
        .unwrap_or_else(|| panic!("doctor data.omp.tested_version is not a string: {data}"));
    assert_eq!(tested, TESTED_OMP, "glen's tested omp version");
    let doc = design();
    let head = doc.lines().take(8).collect::<Vec<_>>().join("\n");
    assert!(
        head.contains(tested),
        "the first 8 lines of docs/design.md must name the tested omp version {tested}:\n{head}"
    );
}

#[test]
fn ac_v2_002_a16_holds_the_v0_2_wave_map_w0_to_w7() {
    let doc = design();
    let a16 = section(&doc, 16);
    let waves: Vec<(u32, &String)> = a16
        .iter()
        .filter_map(|l| {
            let rest = l.strip_prefix("- **v0.2 W")?;
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let after = rest[digits.len()..].strip_prefix("**")?;
            assert!(
                after.trim().chars().count() >= 10,
                "a v0.2 wave bullet has no description: {l:?}"
            );
            Some((digits.parse().ok()?, l))
        })
        .collect();
    let numbers: Vec<u32> = waves.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        numbers,
        (0..=7).collect::<Vec<u32>>(),
        "§A16 must list `- **v0.2 W0**` … `- **v0.2 W7**` in order:\n{a16:#?}"
    );
    assert!(
        a16.iter().any(|l| l.contains("v0.2.0")),
        "§A16 must introduce the v0.2.0 wave map:\n{a16:#?}"
    );
}

#[test]
fn ac_v2_002_a7_task_state_table_maps_job_status_to_the_a2a_state() {
    let doc = design();
    let a7 = section(&doc, 7);
    let all = tables(&a7);
    let table = all
        .iter()
        .find(|t| t.header.iter().any(|h| plain(h).contains("task_state")))
        .unwrap_or_else(|| panic!("§A7 has no table with a `task_state` column:\n{a7:#?}"));
    // (status, condition, task_state) per row.
    let rows: Vec<(String, String, String)> = table
        .rows
        .iter()
        .map(|r| {
            assert!(r.len() >= 2, "row has fewer than 2 cells: {r:?}");
            (
                plain(&r[0]),
                r[1..r.len() - 1].join(" | "),
                plain(&r[r.len() - 1]),
            )
        })
        .collect();
    let ctx = format!("{rows:#?}");

    for (_, _, state) in &rows {
        assert!(
            A2A_VOCABULARY.contains(&state.as_str()),
            "task_state {state:?} is outside the A2A states used by glen\n{ctx}"
        );
    }
    // Every job status row (those whose first cell is a glen status) maps as the AC says.
    for (status, state) in [
        ("queued", "submitted"),
        ("accepted", "working"),
        ("completed", "completed"),
        ("aborted", "canceled"),
        ("lost", "failed"),
    ] {
        let mine: Vec<&(String, String, String)> = rows.iter().filter(|r| r.0 == status).collect();
        assert!(!mine.is_empty(), "no row for status {status}\n{ctx}");
        for r in mine {
            assert_eq!(r.2, state, "status {status} must map to {state}\n{ctx}");
        }
    }
    // failed: no_route -> rejected, every other failure -> failed.
    let failed: Vec<&(String, String, String)> = rows.iter().filter(|r| r.0 == "failed").collect();
    assert!(
        failed.iter().any(|r| r.2 == "rejected") && failed.iter().any(|r| r.2 == "failed"),
        "`failed` must map to both rejected and failed, split on the error code\n{ctx}"
    );
    for r in &failed {
        match r.2.as_str() {
            "rejected" => assert!(
                r.1.contains("no_route"),
                "the failed -> rejected row must be conditioned on `no_route`\n{ctx}"
            ),
            "failed" => {}
            other => panic!("failed maps to {other:?}\n{ctx}"),
        }
    }
    // No job status may map to input_required; no row uses a status outside the job statuses
    // except the reserved row.
    for (status, _, state) in &rows {
        if state == "input_required" {
            assert!(
                !JOB_STATUSES.contains(&status.as_str()),
                "job status {status} must not map to input_required\n{ctx}"
            );
        } else {
            assert!(
                JOB_STATUSES.contains(&status.as_str()),
                "row status {status:?} is not a glen job status\n{ctx}"
            );
        }
    }
    // A2A spells it with one `l`; the job error code keeps `cancelled`.
    assert!(
        rows.iter().all(|r| r.2 != "cancelled"),
        "task_state is spelled `canceled`\n{ctx}"
    );
    // input_required is reserved for ACP permission requests.
    assert!(
        a7.iter()
            .any(|l| l.contains("input_required") && l.contains("ACP")),
        "§A7 must say `input_required` is reserved for ACP permission requests:\n{a7:#?}"
    );
}

// ---------------------------------------------------------------------------------------------
// AC-v2-003: module doc wave map, CHANGELOG, `glen --help` unchanged
// ---------------------------------------------------------------------------------------------

#[test]
fn ac_v2_003_conformance_module_doc_maps_the_eight_v0_2_waves_within_100_columns() {
    let doc = conformance_module_doc();
    let waves = v02_wave_lines(&doc);
    let numbers: Vec<u32> = waves.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        numbers,
        (0..=7).collect::<Vec<u32>>(),
        "tests/conformance.rs module doc needs one `//! - v0.2 W<n>` line for each of W0..W7"
    );
    for (n, line) in &waves {
        assert!(
            line.contains(&format!("AC-v2-{n}")),
            "the v0.2 W{n} line names no AC-v2-{n}nn criteria: {line:?}"
        );
    }
    for line in &doc {
        let cols = line.chars().count();
        assert!(
            cols <= 100,
            "module-doc line is {cols} columns (limit 100): {line:?}"
        );
    }
}

#[test]
fn ac_v2_003_changelog_has_an_unreleased_section_above_v0_1_1() {
    let log = read("CHANGELOG.md");
    let heads: Vec<(usize, &str)> = log
        .lines()
        .enumerate()
        .filter(|(_, l)| l.starts_with("## "))
        .collect();
    let v011 = heads
        .iter()
        .position(|(_, l)| l.starts_with("## v0.1.1"))
        .unwrap_or_else(|| panic!("CHANGELOG.md has no `## v0.1.1` section: {heads:?}"));
    assert!(v011 >= 1, "CHANGELOG.md has no section above `## v0.1.1`");
    let (at, above) = heads[v011 - 1];
    // W7 renames `## Unreleased` to the v0.2.0 release heading, so only the W0 window pins it.
    if w0_window_open() {
        assert_eq!(
            above.trim_end(),
            "## Unreleased",
            "the section directly above `## v0.1.1` must be `## Unreleased`"
        );
    } else {
        assert!(
            above.trim_end() == "## Unreleased" || above.starts_with("## v0.2.0"),
            "the section above `## v0.1.1` must be `## Unreleased` or `## v0.2.0 …`, got {above:?}"
        );
    }
    if above.trim_end() == "## Unreleased" {
        let end = heads.get(v011).map_or(log.lines().count(), |(i, _)| *i);
        let bullets = log
            .lines()
            .enumerate()
            .filter(|(i, l)| *i > at && *i < end && l.starts_with("- "))
            .count();
        assert!(bullets >= 1, "`## Unreleased` has no bullet");
        let count = heads
            .iter()
            .filter(|(_, l)| l.trim_end() == "## Unreleased")
            .count();
        assert_eq!(
            count, 1,
            "CHANGELOG.md has {count} `## Unreleased` sections"
        );
    }
}

#[test]
fn ac_v2_003_help_lists_exactly_the_visible_commands_and_version_is_unchanged() {
    let sb = Sandbox::new();
    let help = sb
        .glen(["--help"])
        .env("COLUMNS", "100")
        .env("NO_COLOR", "1")
        .run();
    help.assert_exit(0).assert_no_ansi();
    let commands = help_commands(&help.out());
    assert_eq!(
        commands,
        VISIBLE.map(str::to_owned).to_vec(),
        "`glen --help` Commands list\n{}",
        help.ctx()
    );
    let version = sb.glen(["--version"]).run();
    version.assert_exit(0);
    assert_eq!(
        version.out(),
        format!("glen {PKG_VERSION}\n"),
        "`glen --version`"
    );
    if !w0_window_open() {
        return;
    }
    // v0.2 W0 window: no behaviour change, so the surface is exactly v0.1.1's.
    assert_eq!(commands.len(), 18, "v0.1.1 has 18 visible commands");
    assert_eq!(
        PKG_VERSION, "0.1.1",
        "v0.2 W0 does not bump the version (W7 does)"
    );
}

/// Object ids of these paths at tag `v0.1.1` (`git rev-parse v0.1.1:<path>`): the `glen` binary
/// is built from exactly these inputs, so while they are unchanged `glen --help` and every other
/// output are the v0.1.1 ones.
const V0_1_1_BINARY_INPUTS: [(&str, &str); 5] = [
    ("src", "db053538059286c79862aa84236d1def0f863d50"),
    ("skill", "1100aa53f1cda8dcf89ab6b35384d808e24c7f5c"),
    ("extension", "0613ee11b6ecef7006199a73daaebd9da7169217"),
    ("Cargo.toml", "0b2b32f3873d4cee6255fad29e14b74a47a23e99"),
    ("Cargo.lock", "d882ba290524a610ef0113d95df79f97cb51849c"),
];

fn git(args: &[&str]) -> Option<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(repo_root())
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .ok()
}

#[test]
fn ac_v2_003_binary_inputs_have_no_change_since_v0_1_1() {
    if !w0_window_open() {
        eprintln!("v0.2 W1 has started: the W0 no-change window is closed");
        return;
    }
    if !repo_root().join(".git").exists() || git(&["rev-parse", "--git-dir"]).is_none() {
        eprintln!("no git checkout: cannot compare against v0.1.1");
        return;
    }
    // Committed state equals the v0.1.1 objects (works without the tag, as in a shallow CI clone).
    for (path, baseline) in V0_1_1_BINARY_INPUTS {
        let out = git(&["rev-parse", &format!("HEAD:{path}")]).expect("run git");
        assert!(
            out.status.success(),
            "git rev-parse HEAD:{path}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            baseline,
            "{path} differs from v0.1.1: v0.2 W0 must not change src, skill, extension, Cargo.toml or Cargo.lock"
        );
    }
    // And so does the working tree: nothing modified, staged or untracked under those paths.
    let paths: Vec<&str> = V0_1_1_BINARY_INPUTS.iter().map(|(p, _)| *p).collect();
    let mut args = vec!["status", "--porcelain", "--untracked-files=all", "--"];
    args.extend(paths);
    let status = git(&args).expect("run git status");
    assert!(
        status.status.success(),
        "git status: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&status.stdout).trim(),
        "",
        "uncommitted change under the v0.1.1 binary inputs"
    );
}

// ---------------------------------------------------------------------------------------------
// Design §A20: the per-caller delivery table
// ---------------------------------------------------------------------------------------------

#[test]
fn v2_w0_a20_per_caller_table_states_codex_no_push() {
    let doc = design();
    let a20 = section(&doc, 20);
    let table = tables(&a20)
        .into_iter()
        .find(|t| t.rows.iter().any(|r| r.iter().any(|c| c.contains("Codex"))))
        .unwrap_or_else(|| panic!("§A20 has no per-caller table naming Codex:\n{a20:#?}"));
    let codex_rows: Vec<&Vec<String>> = table
        .rows
        .iter()
        .filter(|r| plain(&r[0]).starts_with("Codex"))
        .collect();
    assert!(
        !codex_rows.is_empty(),
        "no table row starts with Codex: {:?}",
        table.rows
    );
    assert!(
        codex_rows
            .iter()
            .all(|r| r.join(" | ").contains("Codex: no push")),
        "the Codex row must state the literal `Codex: no push`: {codex_rows:?}"
    );
    assert!(
        a20.iter().any(|l| l.contains("Codex: no push")),
        "§A20 must state `Codex: no push`"
    );
}

#[test]
fn v2_w0_a20_per_caller_table_has_five_rows() {
    if !w0_window_open() {
        eprintln!("v0.2 W1 has started: W2 owns later edits of the per-caller table");
        return;
    }
    let doc = design();
    let a20 = section(&doc, 20);
    let table = tables(&a20)
        .into_iter()
        .find(|t| t.rows.iter().any(|r| r.iter().any(|c| c.contains("Codex"))))
        .unwrap_or_else(|| panic!("§A20 has no per-caller table naming Codex:\n{a20:#?}"));
    assert_eq!(
        table.rows.len(),
        5,
        "the per-caller table has {} rows, expected 5: {:?}",
        table.rows.len(),
        table.rows
    );
    let firsts: Vec<String> = table.rows.iter().map(|r| plain(&r[0])).collect();
    let claude = firsts
        .iter()
        .filter(|f| f.starts_with("Claude Code"))
        .count();
    let codex = firsts.iter().filter(|f| f.starts_with("Codex")).count();
    let other = firsts.len() - claude - codex;
    assert_eq!(
        (claude, codex, other),
        (3, 1, 1),
        "expected 3 Claude Code rows, 1 Codex row and 1 other row (MCP clients): {firsts:?}"
    );
    assert!(
        table.header.len() >= 3 && table.rows.iter().all(|r| r.len() == table.header.len()),
        "every row must have one cell per column: {:?}",
        table.rows
    );
}
