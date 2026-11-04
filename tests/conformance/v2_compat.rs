//! v0.2 W1 conformance: omp compatibility, AC-v2-101 to AC-v2-106
//! (`.claude/plans/v020-wave-1-omp-compat.md`; `docs/design.md` §A10–§A12, §A19).
//!
//! The tests in this module are owned by the conformance writer (Role 3) and written from the
//! plan's settled criteria and the public contracts they name, without reading the
//! implementation or the Implementer's own tests. The Implementer adds none here.
//!
//! Contracts used: `docs/json.md`, `docs/design.md`, `src/omp/compat.json` and `Cargo.toml` as
//! data, the `fake_omp` knobs of §A15 (`FAKE_OMP_VERSION`, `FAKE_OMP_V2`, `FAKE_OMP_SCENARIO`,
//! `FAKE_OMP_STALL`, `FAKE_OMP_READY_DELAY_MS`, `FAKE_OMP_LOG`), the debug-build probe budget knob
//! `GLEN_PROBE_BUDGET_MS`, the `GLEN_OMP_SURFACE_BIN` and `GLEN_OMP_SOURCE` variables, and the
//! checked-in `.github/workflows/omp-latest.yml`.
//!
//! - AC-v2-101 range file and `data.omp`: `ac_v2_101_*`.
//! - AC-v2-102 version rows, warning codes, parsing, failure stays exit 5: `ac_v2_102_*`.
//! - AC-v2-103 `--probe` and the `omp_capabilities` row, the shared 30 s budget: `ac_v2_103_*`.
//! - AC-v2-104 `rpc_protocol` in job status: `ac_v2_104_*`.
//! - AC-v2-105 `omp_surface` and `omp_source_facts` as gates: `ac_v2_105_*`.
//! - AC-v2-106 the nightly workflow, read statically and run step by step against stub
//!   `curl`/`cargo` and a wrapper omp: `ac_v2_106_*`.
//!
//! Under `scripts/flake-check.sh` the waits and the upper bounds on the product's timing (the
//! probe budget plus teardown, the omp teardown polls, the `--version` deadline's ceiling) are
//! multiplied by `GLEN_TEST_TIME_SCALE` (`flake::scaled`, exact in a plain run); lower bounds are
//! not. Child output is captured in files (`flake::Captured`), never read from a pipe to end of
//! file, which a process that inherited the pipe from a concurrent spawn could hold back.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::flake::{Captured, scaled, try_output};
use crate::reader::*;
use crate::rpc_support::*;
use crate::support::*;

// ---------------------------------------------------------------------------------------------
// Shared helpers.
// ---------------------------------------------------------------------------------------------

fn read_repo(rel: &str) -> String {
    fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"))
}

struct Range {
    min: String,
    max_tested: String,
    tested: String,
}

/// `src/omp/compat.json`, as data.
fn compat() -> Range {
    let v: Value = serde_json::from_str(&read_repo("src/omp/compat.json"))
        .expect("src/omp/compat.json is JSON");
    let get = |k: &str| {
        v[k].as_str()
            .unwrap_or_else(|| panic!("compat.json has no string `{k}`: {v}"))
            .to_owned()
    };
    Range {
        min: get("min"),
        max_tested: get("max_tested"),
        tested: get("tested"),
    }
}

/// `MAJOR.MINOR.PATCH` of a version text (pre-release and build ignored).
fn triple(v: &str) -> (u64, u64, u64) {
    let core = v.split(['-', '+']).next().unwrap();
    let parts: Vec<u64> = core
        .split('.')
        .map(|n| {
            n.parse()
                .unwrap_or_else(|_| panic!("not a MAJOR.MINOR.PATCH version: {v}"))
        })
        .collect();
    assert_eq!(parts.len(), 3, "not MAJOR.MINOR.PATCH: {v}");
    (parts[0], parts[1], parts[2])
}

/// `data` of an `ok:true` doctor document, or `error.details.report` of an `omp_unavailable` one.
fn report_of(out: &Out) -> Value {
    let env = out.json();
    if env["ok"] == Value::Bool(true) {
        env["data"].clone()
    } else {
        env["error"]["details"]["report"].clone()
    }
}

fn check_names(report: &Value) -> Vec<String> {
    report["checks"]
        .as_array()
        .unwrap_or_else(|| panic!("checks is not an array: {report}"))
        .iter()
        .map(|c| c["name"].as_str().expect("check name").to_owned())
        .collect()
}

/// `error.code == omp_unavailable` (exit 5), message `omp is not available`, report attached.
fn unavailable_report(out: &Out) -> Value {
    let err = out.error_envelope("omp_unavailable");
    out.assert_stderr_empty();
    assert_eq!(err["message"], "omp is not available", "{}", out.ctx());
    let report = err["details"]["report"].clone();
    assert!(
        report["checks"].is_array(),
        "error.details.report.checks expected\n{}",
        out.ctx()
    );
    report
}

fn assert_not_probed_row(report: &Value, ctx: &str) {
    let names = check_names(report);
    assert_eq!(
        names.get(1).map(String::as_str),
        Some("omp_version"),
        "{ctx}"
    );
    assert_eq!(
        names.get(2).map(String::as_str),
        Some("omp_capabilities"),
        "omp_capabilities must sit directly after omp_version\n{ctx}"
    );
    let row = check(report, "omp_capabilities");
    assert_eq!(row["status"], "info", "{row}\n{ctx}");
    assert_eq!(row["value"], "not probed", "{row}\n{ctx}");
    assert_eq!(
        row["detail"], "run glen doctor --probe to check omp's RPC capabilities",
        "{row}\n{ctx}"
    );
    assert_eq!(report["omp"]["capabilities"], Value::Null, "{ctx}");
}

fn assert_skipped_row(report: &Value, detail: &str, ctx: &str) {
    let names = check_names(report);
    assert_eq!(
        names.get(2).map(String::as_str),
        Some("omp_capabilities"),
        "{ctx}"
    );
    let row = check(report, "omp_capabilities");
    assert_eq!(row["status"], "info", "{row}\n{ctx}");
    assert_eq!(row["value"], "skipped", "{row}\n{ctx}");
    assert_eq!(row["detail"], detail, "{row}\n{ctx}");
    assert_eq!(report["omp"]["capabilities"], Value::Null, "{ctx}");
}

fn script_name() -> String {
    static N: AtomicUsize = AtomicUsize::new(0);
    format!("omp-{}", N.fetch_add(1, Ordering::SeqCst))
}

/// `glen <args>` with `GLEN_OMP_BIN` = a fresh `/bin/sh` script running `body`.
fn with_script(sb: &Sandbox, args: &[&str], body: &str) -> Out {
    let dir = sb.dir("tools");
    let path = write_script(&dir, &script_name(), body);
    sb.glen(args.iter().copied())
        .env("GLEN_OMP_BIN", &path)
        .run()
}

/// A script body that prints `text` as its only stdout line.
fn say(text: &str) -> String {
    assert!(!text.contains('\''), "test text must not hold a quote");
    format!("printf '%s\\n' '{text}'")
}

/// `glen doctor --json` against the fake omp reporting `FAKE_OMP_VERSION=v`.
fn doctor_fake(sb: &Sandbox, v: &str) -> Out {
    sb.glen(["doctor", "--json"])
        .env("GLEN_OMP_BIN", fake_omp_bin())
        .env("FAKE_OMP_VERSION", v)
        .run()
}

// ---------------------------------------------------------------------------------------------
// AC-v2-101: compat.json is the single source; data.omp gains `supported`.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac_v2_101_compat_json_holds_min_max_tested_and_tested_one_key_per_line() {
    let raw = read_repo("src/omp/compat.json");
    let v: Value = serde_json::from_str(&raw).expect("compat.json is JSON");
    let keys: BTreeSet<&str> = v
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["max_tested", "min", "tested"]),
        "{raw}"
    );
    let r = compat();
    for (key, val) in [
        ("min", &r.min),
        ("max_tested", &r.max_tested),
        ("tested", &r.tested),
    ] {
        assert!(
            !val.contains(['-', '+']),
            "{key} is a plain MAJOR.MINOR.PATCH: {val}"
        );
        triple(val);
        let lines: Vec<&str> = raw
            .lines()
            .filter(|l| l.contains(&format!("\"{key}\"")))
            .collect();
        assert_eq!(lines.len(), 1, "`{key}` sits on exactly one line: {raw}");
        assert!(
            lines[0].contains(&format!("\"{val}\"")),
            "`{key}` and its value share a line: {raw}"
        );
    }
    assert!(triple(&r.min) <= triple(&r.tested), "min <= tested: {raw}");
    assert!(
        triple(&r.tested) <= triple(&r.max_tested),
        "tested <= max_tested: {raw}"
    );
}

#[test]
fn ac_v2_101_omp_rpc_stays_pinned_to_the_tag_named_by_tested() {
    let cargo = read_repo("Cargo.toml");
    let line = cargo
        .lines()
        .find(|l| l.trim_start().starts_with("omp-rpc"))
        .expect("Cargo.toml has an omp-rpc dependency");
    let tag = format!("tag = \"v{}\"", compat().tested);
    assert!(
        line.contains(&tag),
        "the omp-rpc git tag must equal compat.json `tested` (expected {tag}): {line}"
    );
}

#[test]
fn ac_v2_101_doctor_reports_the_declared_range_beside_tested_version() {
    let r = compat();
    let sb = Sandbox::new();
    let out = sb
        .glen(["doctor", "--json"])
        .env("GLEN_OMP_BIN", fake_omp_bin())
        .run();
    let data = out.ok_data();
    out.assert_stderr_empty();
    let omp = data["omp"].as_object().expect("data.omp is an object");
    for k in [
        "bin",
        "capabilities",
        "supported",
        "tested_version",
        "version",
    ] {
        assert!(
            omp.contains_key(k),
            "data.omp.{k} is missing\n{}",
            out.ctx()
        );
    }
    assert_eq!(
        data["omp"]["supported"],
        json!({"min": r.min, "max_tested": r.max_tested}),
        "{}",
        out.ctx()
    );
    assert_eq!(
        data["omp"]["tested_version"],
        r.tested.as_str(),
        "{}",
        out.ctx()
    );
    assert_eq!(data["omp"]["capabilities"], Value::Null, "{}", out.ctx());

    // The same fields travel in the failed report, with the range still declared.
    let out = sb.glen(["doctor", "--json"]).run();
    let report = unavailable_report(&out);
    assert_eq!(
        report["omp"]["supported"],
        json!({"min": r.min, "max_tested": r.max_tested}),
        "{}",
        out.ctx()
    );
    assert_eq!(report["omp"]["tested_version"], r.tested.as_str());
    assert_eq!(report["omp"]["capabilities"], Value::Null);
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object()
        .unwrap_or_else(|| panic!("expected an object, got {v}"))
        .keys()
        .cloned()
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// §A10 / `docs/json.md`: `v:1` is append-only. The doctor document of v0.1.1 keeps every field
/// and every check, in its relative order; v0.2 W1 only adds.
#[test]
fn ac_v2_101_doctor_document_is_append_only_since_v0_1_1() {
    let sb = Sandbox::new();
    let out = sb
        .glen(["doctor", "--json"])
        .env("GLEN_OMP_BIN", fake_omp_bin())
        .run();
    let data = out.ok_data();
    for k in ["checks", "extension", "glen", "omp", "paths", "skills"] {
        assert!(data.get(k).is_some(), "data.{k} was removed\n{}", out.ctx());
    }
    let kept: [(&str, &[&str]); 4] = [
        ("glen", &["home", "home_exists", "version"]),
        ("omp", &["bin", "tested_version", "version"]),
        ("extension", &["current", "path", "state", "version"]),
        (
            "paths",
            &[
                "agent_dir",
                "agent_dir_source",
                "base_config_root",
                "config_root",
                "extensions_dir",
                "profile",
                "session_owners_dir",
                "sessions_dir",
                "sessions_dir_source",
            ],
        ),
    ];
    for (section, fields) in kept {
        let have = keys(&data[section]);
        for f in fields {
            assert!(have.contains(*f), "data.{section}.{f} was removed: {data}");
        }
    }
    for agent in ["claude", "codex"] {
        let have = keys(&data["skills"][agent]);
        for f in ["current", "path", "project", "state", "version"] {
            assert!(
                have.contains(f),
                "data.skills.{agent}.{f} was removed: {data}"
            );
        }
    }
    let old_checks = [
        "omp_binary",
        "omp_version",
        "profile",
        "agent_dir",
        "sessions_dir",
        "glen_home",
        "extension",
        "live_endpoints",
        "omp_env_files",
        "skill_claude",
        "skill_codex",
        "sandbox",
    ];
    let now = check_names(&data);
    let filtered: Vec<&str> = now
        .iter()
        .map(String::as_str)
        .filter(|n| old_checks.contains(n))
        .collect();
    assert_eq!(
        filtered, old_checks,
        "the v0.1.1 checks keep their order: {now:?}"
    );
    assert_eq!(now[2], "omp_capabilities", "{now:?}");
    for c in data["checks"].as_array().unwrap() {
        assert_eq!(keys(c), set(&["detail", "name", "status", "value"]), "{c}");
    }
}

#[test]
fn ac_v2_101_json_md_documents_the_w1_fields_and_warning() {
    let md = read_repo("docs/json.md");
    let r = compat();
    let start = md
        .find("<!-- contract: doctor -->")
        .expect("docs/json.md has the doctor contract block");
    let rest = &md[start..];
    let open = rest.find("```json").expect("a json fence") + "```json".len();
    let close = rest[open..].find("```").expect("a closing fence");
    let block: Value = serde_json::from_str(&rest[open..open + close]).expect("block is JSON");
    assert_eq!(
        block["data"]["omp"]["supported"],
        json!({"min": r.min, "max_tested": r.max_tested}),
        "the documented example states the declared range"
    );
    assert_eq!(block["data"]["omp"]["tested_version"], r.tested.as_str());
    assert!(block["data"]["omp"].get("capabilities").is_some());
    assert!(
        md.lines()
            .any(|l| l.contains("`omp_version_unsupported`") && l.contains("doctor")),
        "the warning-code table documents omp_version_unsupported for doctor"
    );
    assert!(
        md.lines()
            .any(|l| l.contains("`omp_version_untested`") && l.contains("doctor")),
        "omp_version_untested stays documented"
    );
    assert!(
        md.contains("omp_capabilities"),
        "docs/json.md names the omp_capabilities row"
    );
    assert!(md.contains("--probe"), "docs/json.md names --probe");
    assert!(
        md.lines()
            .any(|l| l.contains("`omp_unavailable`") && l.contains("doctor --probe")),
        "the exit-code table says doctor --probe can end in omp_unavailable"
    );
    // rpc_protocol is job status only: it is not added to the job view of docs/json.md.
    let mut in_fence = false;
    for l in md.lines() {
        if l.starts_with("```") {
            in_fence = !in_fence;
        }
        if in_fence || l.trim_start().starts_with('|') {
            assert!(
                !l.contains("rpc_protocol"),
                "rpc_protocol is not part of any documented document: {l}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// AC-v2-102: version classification and parsing.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Verdict {
    Supported,
    Untested,
    Unsupported,
    Unparseable,
}

fn version_warnings(env: &Value) -> Vec<String> {
    warning_codes(env)
        .into_iter()
        .filter(|c| c.starts_with("omp_version"))
        .collect()
}

/// A successful doctor document whose `omp_version` row, `data.omp.version` and warnings match
/// the §A19 classification table for `normalized` (the text after normalization).
fn assert_verdict(out: &Out, input: &str, normalized: &str, verdict: Verdict) {
    let ctx = format!("version text {input:?} expected {verdict:?}\n{}", out.ctx());
    let env = out.ok_envelope();
    out.assert_stderr_empty();
    out.assert_no_panic();
    let data = &env["data"];
    let row = check(data, "omp_version");
    let codes = version_warnings(&env);
    match verdict {
        Verdict::Supported => {
            assert_eq!(row["status"], "ok", "{ctx}");
            assert_eq!(row["value"], normalized, "{ctx}");
            assert_eq!(row["detail"], "", "{ctx}");
            assert!(
                codes.is_empty(),
                "no version warning in range: {codes:?}\n{ctx}"
            );
            assert_eq!(data["omp"]["version"], normalized, "{ctx}");
        }
        Verdict::Untested => {
            assert_eq!(row["status"], "warn", "{ctx}");
            assert_eq!(row["value"], normalized, "{ctx}");
            assert_eq!(row["detail"], "untested", "{ctx}");
            assert_eq!(codes, ["omp_version_untested"], "{ctx}");
            assert_eq!(data["omp"]["version"], normalized, "{ctx}");
        }
        Verdict::Unsupported => {
            assert_eq!(row["status"], "warn", "{ctx}");
            assert_eq!(row["value"], normalized, "{ctx}");
            assert_eq!(row["detail"], "unsupported", "{ctx}");
            assert_eq!(codes, ["omp_version_unsupported"], "{ctx}");
            assert_eq!(data["omp"]["version"], normalized, "{ctx}");
        }
        Verdict::Unparseable => {
            assert_eq!(row["status"], "warn", "{ctx}");
            assert_eq!(row["value"], "unprobed", "{ctx}");
            assert_eq!(row["detail"], "cannot parse omp --version output", "{ctx}");
            assert_eq!(codes, ["omp_version_untested"], "{ctx}");
            assert_eq!(data["omp"]["version"], Value::Null, "{ctx}");
        }
    }
    // Never a failure on the version alone: the binary is found and doctor succeeded.
    assert_eq!(check(data, "omp_binary")["status"], "ok", "{ctx}");
    assert!(data["omp"]["bin"].is_string(), "{ctx}");
    let names = check_names(data);
    assert_eq!(names[1], "omp_version", "{ctx}");
    assert_eq!(names[2], "omp_capabilities", "{ctx}");
    let others: Vec<&str> = data["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["name"] != "omp_version" && c["status"] == "fail")
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(
        others.is_empty(),
        "no check fails on the version: {others:?}\n{ctx}"
    );
}

/// The plan's classification table is written for min = max_tested = 18.14.0 (plan default 6).
/// After a later wave raises `max_tested` that table no longer describes the range, and
/// `ac_v2_102_versions_follow_the_declared_range` (derived from compat.json) carries the check.
fn plan_range() -> bool {
    let r = compat();
    let same = r.min == "18.14.0" && r.max_tested == "18.14.0";
    if !same {
        eprintln!(
            "note: compat.json is {}..{}; the 18.14.0 plan table is covered by the derived test",
            r.min, r.max_tested
        );
    }
    same
}

fn tv(t: (u64, u64, u64)) -> String {
    format!("{}.{}.{}", t.0, t.1, t.2)
}

/// The same classification, generated from whatever `compat.json` declares.
#[test]
fn ac_v2_102_versions_follow_the_declared_range_whatever_it_is() {
    let r = compat();
    let (mn, mx) = (triple(&r.min), triple(&r.max_tested));
    let sb = Sandbox::new();
    let mut supported = vec![
        (tv(mn), tv(mn)),
        (tv(mx), tv(mx)),
        (format!("{}+b1", tv(mx)), format!("{}+b1", tv(mx))),
        (format!("{}+b1", tv(mn)), format!("{}+b1", tv(mn))),
        (format!("v{}", tv(mx)), tv(mx)),
    ];
    if mn.0 == mx.0 && mx.1 >= mn.1 + 2 {
        let mid = tv((mn.0, (mn.1 + mx.1) / 2, 5));
        supported.push((mid.clone(), mid));
    }
    let up = |t| (tv(t), tv(t));
    let untested = vec![
        up((mx.0, mx.1, mx.2 + 1)),
        up((mx.0, mx.1, mx.2 + 10)),
        up((mx.0, mx.1 + 1, 0)),
        up((mx.0 + 1, 0, 0)),
        up((mx.0, mx.1 + 100, 0)),
        up((mx.0 + 100, 0, 0)),
        (
            format!("{}.{}.0-rc.1", mx.0, mx.1 + 1),
            format!("{}.{}.0-rc.1", mx.0, mx.1 + 1),
        ),
        (
            format!("v{}.{}.0-rc.1+b2", mx.0, mx.1 + 1),
            format!("{}.{}.0-rc.1+b2", mx.0, mx.1 + 1),
        ),
    ];
    // Below min: every pre-release of min, and lower releases (one lexically above min).
    let mut unsupported = vec![
        (format!("{}-rc.1", tv(mn)), format!("{}-rc.1", tv(mn))),
        (format!("{}-0", tv(mn)), format!("{}-0", tv(mn))),
        (
            format!("{}-alpha.1+exp.sha", tv(mn)),
            format!("{}-alpha.1+exp.sha", tv(mn)),
        ),
    ];
    if mn.2 > 0 {
        unsupported.push(up((mn.0, mn.1, mn.2 - 1)));
    }
    if mn.1 > 0 {
        unsupported.push(up((mn.0, mn.1 - 1, 99)));
    }
    if mn.0 > 0 {
        unsupported.push(up((mn.0 - 1, 99, 0)));
        unsupported.push(up((0, 0, 1)));
    }
    if mn.1 >= 10 {
        unsupported.push(up((mn.0, 9, 0)));
    }
    for (given, want) in &supported {
        assert_verdict(&doctor_fake(&sb, given), given, want, Verdict::Supported);
    }
    for (given, want) in &untested {
        assert_verdict(&doctor_fake(&sb, given), given, want, Verdict::Untested);
    }
    for (given, want) in &unsupported {
        assert_verdict(&doctor_fake(&sb, given), given, want, Verdict::Unsupported);
    }
}

#[test]
fn ac_v2_102_versions_in_range_are_ok_with_the_normalized_text() {
    if !plan_range() {
        return;
    }
    let sb = Sandbox::new();
    // (text after `omp/`, normalized value): build metadata is kept but ignored by precedence,
    // and an optional `v` is stripped.
    for (given, want) in [
        ("18.14.0", "18.14.0"),
        ("18.14.0+b1", "18.14.0+b1"),
        ("v18.14.0", "18.14.0"),
        ("v18.14.0+b1", "18.14.0+b1"),
        ("18.14.0+20261101", "18.14.0+20261101"),
        ("18.14.0+a-b.C9", "18.14.0+a-b.C9"),
    ] {
        assert_verdict(&doctor_fake(&sb, given), given, want, Verdict::Supported);
    }
}

#[test]
fn ac_v2_102_versions_above_max_tested_are_untested_not_failures() {
    if !plan_range() {
        return;
    }
    let sb = Sandbox::new();
    for (given, want) in [
        ("18.14.1", "18.14.1"),
        ("18.15.0-rc.1", "18.15.0-rc.1"),
        ("19.0.0", "19.0.0"),
        ("18.15.0", "18.15.0"),
        // Numeric, not lexical, precedence.
        ("18.14.10", "18.14.10"),
        ("18.100.0", "18.100.0"),
        ("100.0.0", "100.0.0"),
        // A pre-release of a higher version is above max_tested.
        ("18.14.1-rc.1", "18.14.1-rc.1"),
        ("18.15.0-0+exp.sha.5114f85", "18.15.0-0+exp.sha.5114f85"),
        ("v18.15.0-rc.1", "18.15.0-rc.1"),
    ] {
        assert_verdict(&doctor_fake(&sb, given), given, want, Verdict::Untested);
    }
}

#[test]
fn ac_v2_102_versions_below_min_are_unsupported_not_failures() {
    if !plan_range() {
        return;
    }
    let sb = Sandbox::new();
    for (given, want) in [
        ("18.14.0-rc.1", "18.14.0-rc.1"),
        ("18.13.9", "18.13.9"),
        ("18.0.9", "18.0.9"),
        ("17.99.0", "17.99.0"),
        // Numeric, not lexical, precedence: "18.9.0" sorts above "18.14.0" as a string.
        ("18.9.0", "18.9.0"),
        ("9.99.99", "9.99.99"),
        ("0.0.1", "0.0.1"),
        // A release is above its pre-releases, so every pre-release of min is below it.
        ("18.14.0-0", "18.14.0-0"),
        ("18.14.0-alpha.1+exp.sha", "18.14.0-alpha.1+exp.sha"),
        ("v18.14.0-rc.1", "18.14.0-rc.1"),
    ] {
        assert_verdict(&doctor_fake(&sb, given), given, want, Verdict::Unsupported);
    }
}

#[test]
fn ac_v2_102_unparseable_output_warns_with_unprobed_and_never_fails() {
    let sb = Sandbox::new();
    for given in [
        "garbage",
        "18.14",
        "18.14.0.1",
        "18.14.x",
        "x18.14.0",
        "vv18.14.0",
        "v",
        "18.14.0-rc_1",
        "18.14.0+b+1",
        "18.14.0 beta",
        "18.14.0-\u{e9}",
        ".18.14.0",
        "18..0",
        "-18.14.0",
    ] {
        assert_verdict(&doctor_fake(&sb, given), given, "", Verdict::Unparseable);
    }
    // Output that is not even a line of text.
    for (name, body) in [
        ("empty line", "echo"),
        ("blank line", "echo '   '"),
        ("no stdout at all", "echo omp/18.14.0 >&2"),
        ("no output", "exit 0"),
        ("bare prefix", "echo omp/"),
        ("double prefix", "echo omp/omp/18.14.0"),
    ] {
        let out = with_script(&sb, &["doctor", "--json"], body);
        assert_verdict(&out, name, "", Verdict::Unparseable);
    }
}

#[test]
fn ac_v2_102_absurd_version_numbers_are_a_warning_not_a_crash() {
    let sb = Sandbox::new();
    let huge = "99999999999999999999999.0.0";
    let out = doctor_fake(&sb, huge);
    let env = out.ok_envelope();
    out.assert_no_panic().assert_stderr_empty();
    let row = check(&env["data"], "omp_version");
    assert_eq!(row["status"], "warn", "{}", out.ctx());
    assert!(
        version_warnings(&env).contains(&"omp_version_untested".to_owned()),
        "an unreadable or enormous version is untested\n{}",
        out.ctx()
    );
}

#[test]
fn ac_v2_102_only_the_first_stdout_line_trimmed_is_the_version() {
    let sb = Sandbox::new();
    let cases: [(&str, &str, &str, Verdict); 9] = [
        (
            "later lines ignored",
            "echo omp/18.14.0\necho garbage",
            "18.14.0",
            Verdict::Supported,
        ),
        (
            "later lines ignored 2",
            "echo omp/18.14.0\necho omp/99.0.0",
            "18.14.0",
            Verdict::Supported,
        ),
        (
            "garbage first",
            "echo garbage\necho omp/18.14.0",
            "",
            Verdict::Unparseable,
        ),
        (
            "bare v prefix",
            "echo v18.14.0",
            "18.14.0",
            Verdict::Supported,
        ),
        ("bare number", "echo 18.14.0", "18.14.0", Verdict::Supported),
        (
            "padded",
            "printf '   omp/18.14.0  \\n'",
            "18.14.0",
            Verdict::Supported,
        ),
        (
            "crlf",
            "printf 'omp/18.14.0\\r\\n'",
            "18.14.0",
            Verdict::Supported,
        ),
        (
            "no newline",
            "printf 'omp/18.14.1'",
            "18.14.1",
            Verdict::Untested,
        ),
        (
            "prefixed pre-release",
            "echo omp/v18.15.0-rc.1+b1",
            "18.15.0-rc.1+b1",
            Verdict::Untested,
        ),
    ];
    for (name, body, want, verdict) in cases {
        let out = with_script(&sb, &["doctor", "--json"], body);
        assert_verdict(&out, name, want, verdict);
    }
}

#[test]
fn ac_v2_102_a_long_unparseable_line_is_not_echoed_in_full() {
    let sb = Sandbox::new();
    let long = "A".repeat(500);
    let out = with_script(&sb, &["doctor", "--json"], &say(&long));
    assert_verdict(&out, "500 A", "", Verdict::Unparseable);
    let raw = out.out();
    assert!(
        !raw.contains(&"A".repeat(65)),
        "§A19: the raw line is never echoed beyond 64 characters\n{}",
        out.ctx()
    );
    // Control bytes in the line never reach a JSON document raw either.
    let out = with_script(&sb, &["doctor", "--json"], "printf '\\033[31mred\\n'");
    assert_verdict(&out, "ESC line", "", Verdict::Unparseable);
    assert!(!has_esc(&out.stdout), "ESC byte in JSON\n{}", out.ctx());
}

#[test]
fn ac_v2_102_human_mode_warns_and_exits_zero_for_each_non_ok_verdict() {
    let sb = Sandbox::new();
    for (given, detail) in [
        ("18.15.0-rc.1", "untested"),
        ("18.13.9", "unsupported"),
        ("garbage", "cannot parse omp --version output"),
    ] {
        let out = sb
            .glen(["doctor", "--human"])
            .env("GLEN_OMP_BIN", fake_omp_bin())
            .env("FAKE_OMP_VERSION", given)
            .run();
        out.assert_exit(0).assert_no_panic().assert_no_ansi();
        assert!(out.err().contains("warning"), "{given}\n{}", out.ctx());
        assert!(
            strip_ansi(&out.out()).contains(detail),
            "{given}\n{}",
            out.ctx()
        );
    }
    let ok = sb
        .glen(["doctor", "--human"])
        .env("GLEN_OMP_BIN", fake_omp_bin())
        .run();
    ok.assert_exit(0);
    let human = strip_ansi(&ok.out());
    let r = compat();
    assert!(
        human.contains(&format!("tested against omp {}", r.tested)),
        "{}",
        ok.ctx()
    );
    assert!(
        human.contains(&format!("supported {} to {}", r.min, r.max_tested)),
        "§A19 human footer names the supported range\n{}",
        ok.ctx()
    );
    assert!(
        !ok.err().contains("warning"),
        "no warning in range\n{}",
        ok.ctx()
    );
}

/// `omp --version` that cannot run stays `fail`: exit 5 `omp_unavailable`, because no version
/// exists to judge. The report still carries the full document.
#[test]
fn ac_v2_102_a_version_that_cannot_run_stays_omp_unavailable() {
    let sb = Sandbox::new();
    let tools = sb.dir("tools");
    let bad_interp = tools.join("bad-interpreter");
    fs::write(&bad_interp, "#!/nonexistent/interpreter\nexit 0\n").unwrap();
    set_mode(&bad_interp, 0o755);
    let r = compat();
    let cases: Vec<(&str, PathBuf)> = vec![
        ("exit 1", write_script(&tools, "exit1", "exit 1")),
        (
            "non-zero after a valid line",
            write_script(&tools, "after", "echo omp/18.14.0\nexit 3"),
        ),
        (
            "stderr only, non-zero",
            write_script(&tools, "stderr", "echo boom >&2\nexit 2"),
        ),
        (
            "non-UTF-8 output",
            write_script(&tools, "nonutf8", "printf 'omp/18.14.\\377\\n'"),
        ),
        ("spawn error", bad_interp),
    ];
    for (name, script) in cases {
        let out = sb
            .glen(["doctor", "--json"])
            .env("GLEN_OMP_BIN", &script)
            .run();
        let report = unavailable_report(&out);
        let ctx = format!("{name}\n{}", out.ctx());
        assert_eq!(check(&report, "omp_binary")["status"], "ok", "{ctx}");
        assert_eq!(check(&report, "omp_version")["status"], "fail", "{ctx}");
        assert_eq!(report["omp"]["version"], Value::Null, "{ctx}");
        assert_eq!(report["omp"]["bin"], Value::from(s(&script)), "{ctx}");
        assert_eq!(report["omp"]["tested_version"], r.tested.as_str(), "{ctx}");
        assert_eq!(
            report["omp"]["supported"],
            json!({"min": r.min, "max_tested": r.max_tested}),
            "{ctx}"
        );
        assert_not_probed_row(&report, &ctx);
        // Human mode: the table first, then the error line, same exit.
        let human = sb
            .glen(["doctor", "--human"])
            .env("GLEN_OMP_BIN", &script)
            .run();
        human
            .assert_exit(exit_for("omp_unavailable"))
            .assert_no_ansi();
        assert!(human.err().starts_with("error:"), "{name}\n{}", human.ctx());
    }
}

#[test]
fn ac_v2_102_a_hanging_version_times_out_as_omp_unavailable() {
    let sb = Sandbox::new();
    let tools = sb.dir("tools");
    let hang = write_script(&tools, "hang", "exec /bin/sleep 60");
    let out = sb
        .glen(["doctor", "--json", "--probe"])
        .env("GLEN_OMP_BIN", &hang)
        .run();
    let report = unavailable_report(&out);
    assert_eq!(
        check(&report, "omp_version")["status"],
        "fail",
        "{}",
        out.ctx()
    );
    assert!(
        out.elapsed >= Duration::from_secs(8) && out.elapsed < scaled(Duration::from_secs(25)),
        "--version has its own 10 s deadline, took {:?}",
        out.elapsed
    );
    // The probe never starts when --version failed.
    assert_skipped_row(&report, "omp --version failed", &out.ctx());
}

// ---------------------------------------------------------------------------------------------
// AC-v2-103: `doctor --probe` and the `omp_capabilities` row.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac_v2_103_without_probe_the_row_is_info_not_probed_at_index_2() {
    let sb = Sandbox::new();
    let out = sb
        .glen(["doctor", "--json"])
        .env("GLEN_OMP_BIN", fake_omp_bin())
        .run();
    let data = out.ok_data();
    assert_not_probed_row(&data, &out.ctx());
    // The same shape for every version verdict.
    for v in ["18.15.0", "18.13.0", "garbage"] {
        let out = doctor_fake(&sb, v);
        let env = out.ok_envelope();
        assert_not_probed_row(&env["data"], &out.ctx());
    }
    // And in the human table.
    let human = sb
        .glen(["doctor", "--human"])
        .env("GLEN_OMP_BIN", fake_omp_bin())
        .run();
    human.assert_exit(0);
    assert!(
        strip_ansi(&human.out()).contains("not probed"),
        "{}",
        human.ctx()
    );
    // A missing omp binary still reports the row, at index 2, as not probed.
    let out = sb.glen(["doctor", "--json"]).run();
    let report = unavailable_report(&out);
    assert_not_probed_row(&report, &out.ctx());
}

#[test]
fn ac_v2_103_without_probe_doctor_starts_no_rpc_omp() {
    let rb = RpcBox::new();
    rb.glen(["doctor", "--json"]).run().assert_exit(0);
    rb.glen(["doctor", "--human"]).run().assert_exit(0);
    let rpc_starts: Vec<Value> = rb
        .starts()
        .into_iter()
        .filter(|s| strings(&s["argv"]).iter().any(|a| a == "--mode"))
        .collect();
    assert!(rpc_starts.is_empty(), "doctor started omp: {rpc_starts:?}");
    rb.assert_quiet();
}

#[test]
fn ac_v2_103_probe_is_a_documented_doctor_flag() {
    let sb = Sandbox::new();
    let help = sb.glen(["doctor", "--help"]).run();
    help.assert_exit(0);
    assert!(help.out().contains("--probe"), "{}", help.ctx());
    for cmd in ["ls", "jobs", "completions"] {
        let out = sb.glen([cmd, "--probe"]).run();
        assert_eq!(
            out.code(),
            2,
            "`{cmd} --probe` is a usage error\n{}",
            out.ctx()
        );
    }
}

#[test]
fn ac_v2_103_probe_is_skipped_when_omp_is_missing_or_its_version_failed() {
    let sb = Sandbox::new();
    // No omp binary.
    let out = sb.glen(["doctor", "--json", "--probe"]).run();
    let report = unavailable_report(&out);
    assert_skipped_row(&report, "omp binary is missing", &out.ctx());
    assert_eq!(
        check(&report, "omp_binary")["status"],
        "fail",
        "{}",
        out.ctx()
    );
    // `--version` fails: no RPC omp is ever started.
    let log = sb.root.join("calls.log");
    let body = format!("echo \"$@\" >> '{}'\nexit 1", s(&log));
    let out = with_script(&sb, &["doctor", "--json", "--probe"], &body);
    let report = unavailable_report(&out);
    assert_skipped_row(&report, "omp --version failed", &out.ctx());
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(calls.contains("--version"), "{calls:?}");
    assert!(
        !calls.contains("--mode") && !calls.contains("rpc"),
        "no probe process after a failed --version: {calls:?}"
    );
}

struct Expect {
    protocol: Option<u64>,
    host_tools: &'static str,
    event_filter: &'static str,
    status: &'static str,
}

fn probe(rb: &RpcBox, envs: &[(&str, &str)]) -> Out {
    let mut run = rb.glen(["doctor", "--probe", "--json"]);
    for (k, v) in envs {
        run = run.env(k, v);
    }
    run.run()
}

/// The `omp_capabilities` row and `data.omp.capabilities` match `want` (§A19 row table).
fn assert_caps(out: &Out, want: &Expect) -> Value {
    out.assert_no_panic();
    let report = report_of(out);
    let ctx = out.ctx();
    let names = check_names(&report);
    assert_eq!(names[2], "omp_capabilities", "{ctx}");
    let row = check(&report, "omp_capabilities");
    let protocol = want
        .protocol
        .map_or_else(|| "unknown".to_owned(), |p| p.to_string());
    assert_eq!(
        row["value"],
        format!(
            "protocol={protocol} host_tools={} event_filter={}",
            want.host_tools, want.event_filter
        ),
        "{ctx}"
    );
    assert_eq!(row["status"], want.status, "{ctx}");
    if want.status == "ok" {
        assert_eq!(row["detail"], "", "an ok row has no detail\n{ctx}");
    } else {
        assert!(
            row["detail"].as_str().is_some_and(|d| !d.is_empty()),
            "a {} row names its reason\n{ctx}",
            want.status
        );
    }
    let caps = &report["omp"]["capabilities"];
    assert_eq!(
        caps,
        &json!({
            "protocol": want.protocol,
            "host_tools": want.host_tools,
            "event_filter": want.event_filter,
        }),
        "{ctx}"
    );
    report
}

#[test]
fn ac_v2_103_probe_reports_protocol_1_or_2_and_both_capabilities_ok() {
    let cases: [(&[(&str, &str)], u64); 3] = [
        (&[], 1),
        (&[("FAKE_OMP_V2", "1")], 2),
        (&[("FAKE_OMP_V2", "partial")], 1),
    ];
    for (envs, protocol) in cases {
        let rb = RpcBox::new();
        let out = probe(&rb, envs);
        out.assert_exit(0).assert_stderr_empty();
        assert_caps(
            &out,
            &Expect {
                protocol: Some(protocol),
                host_tools: "ok",
                event_filter: "ok",
                status: "ok",
            },
        );
        let env = out.json();
        assert!(
            version_warnings(&env).is_empty(),
            "an ok probe adds no warning\n{}",
            out.ctx()
        );
        rb.assert_quiet();
    }
}

#[test]
fn ac_v2_103_a_rejected_host_tool_is_the_only_failure_and_exits_5() {
    for (envs, protocol) in [
        (vec![("FAKE_OMP_SCENARIO", "reject_tools")], 1),
        (
            vec![("FAKE_OMP_SCENARIO", "reject_tools"), ("FAKE_OMP_V2", "1")],
            2,
        ),
    ] {
        let rb = RpcBox::new();
        let out = probe(&rb, &envs);
        let err = out.error_envelope("omp_unavailable");
        out.assert_stderr_empty();
        assert_eq!(err["message"], "omp is not available", "{}", out.ctx());
        assert!(
            err["hint"]
                .as_str()
                .is_some_and(|h| h.contains("omp_capabilities")),
            "the hint names the omp_capabilities row\n{}",
            out.ctx()
        );
        let report = &err["details"]["report"];
        let row = check(report, "omp_capabilities");
        assert_eq!(row["status"], "fail", "{}", out.ctx());
        assert!(
            row["value"]
                .as_str()
                .unwrap()
                .contains("host_tools=missing"),
            "{}",
            out.ctx()
        );
        assert!(row["detail"].as_str().is_some_and(|d| !d.is_empty()));
        assert_eq!(report["omp"]["capabilities"]["host_tools"], "missing");
        assert_eq!(
            report["omp"]["capabilities"]["protocol"],
            protocol,
            "{}",
            out.ctx()
        );
        // Everything else still reads fine, so the failure is attributable to the row alone.
        assert_eq!(check(report, "omp_version")["status"], "ok");
        assert_eq!(check(report, "omp_binary")["status"], "ok");
        rb.assert_quiet();

        // Human mode: the table first, then the error.
        let rb = RpcBox::new();
        let mut run = rb.glen(["doctor", "--probe", "--human"]);
        for (k, v) in &envs {
            run = run.env(k, v);
        }
        let human = run.run();
        human.assert_exit(5).assert_no_ansi();
        assert!(
            strip_ansi(&human.out()).contains("host_tools=missing"),
            "{}",
            human.ctx()
        );
        assert!(human.err().starts_with("error:"), "{}", human.ctx());
        rb.assert_quiet();
    }
}

#[test]
fn ac_v2_103_a_rejected_event_filter_is_a_warning_not_a_failure() {
    for (envs, protocol) in [
        (vec![("FAKE_OMP_SCENARIO", "reject_filter")], 1),
        (
            vec![("FAKE_OMP_SCENARIO", "reject_filter"), ("FAKE_OMP_V2", "1")],
            2,
        ),
    ] {
        let rb = RpcBox::new();
        let out = probe(&rb, &envs);
        out.assert_exit(0).assert_stderr_empty();
        assert_caps(
            &out,
            &Expect {
                protocol: Some(protocol),
                host_tools: "ok",
                event_filter: "missing",
                status: "warn",
            },
        );
        rb.assert_quiet();
    }
}

/// A tool list that comes back without `glen_reply` proves host tools missing (§A19), without
/// any rejected command.
#[test]
fn ac_v2_103_an_answer_without_glen_reply_proves_host_tools_missing() {
    let sb = Sandbox::new();
    let tools = sb.dir("tools");
    let peer = write_script(
        &tools,
        "peer",
        r#"
case "$1" in --version) echo omp/18.14.0; exit 0;; esac
echo '{"type":"ready"}'
while IFS= read -r line; do
  rest=${line#*'"id":"'}
  id=${rest%%'"'*}
  case "$line" in
    *'"type":"set_host_tools"'*)
      printf '{"id":"%s","type":"response","command":"set_host_tools","success":true,"data":{"toolNames":[]}}\n' "$id";;
    *'"type":"set_event_filter"'*)
      printf '{"id":"%s","type":"response","command":"set_event_filter","success":true,"data":{"events":[]}}\n' "$id";;
    *'"type":"negotiate_protocol"'*)
      printf '{"id":"%s","type":"response","command":"negotiate_protocol","success":false,"error":"no"}\n' "$id";;
  esac
done
"#,
    );
    let out = sb
        .glen(["doctor", "--probe", "--json"])
        .env("GLEN_OMP_BIN", &peer)
        .run();
    let report = unavailable_report(&out);
    let row = check(&report, "omp_capabilities");
    assert_eq!(row["status"], "fail", "{}", out.ctx());
    assert_eq!(
        report["omp"]["capabilities"]["host_tools"],
        "missing",
        "{}",
        out.ctx()
    );
    assert!(
        row["value"]
            .as_str()
            .unwrap()
            .contains("host_tools=missing"),
        "{}",
        out.ctx()
    );
}

/// An omp that exits before `ready` (no usable model: the bare omp of the plan's evidence)
/// proves nothing about capabilities, so it is `warn`, never `fail`.
#[test]
fn ac_v2_103_an_omp_that_exits_before_ready_is_warn_with_everything_unknown() {
    let sb = Sandbox::new();
    let tools = sb.dir("tools");
    let cases: Vec<(&str, PathBuf, Option<&str>)> = vec![
        (
            "bare omp, no model",
            write_script(
                &tools,
                "bare",
                "case \"$1\" in --version) echo omp/18.14.0; exit 0;; esac\n\
                 echo 'No default model selected. Use /login.' >&2\nexit 1",
            ),
            Some("No default model selected"),
        ),
        (
            "exits at once with status 0",
            write_script(
                &tools,
                "quiet",
                "case \"$1\" in --version) echo omp/18.14.0; exit 0;; esac\nexit 0",
            ),
            None,
        ),
        (
            "prints non-JSON then exits",
            write_script(
                &tools,
                "junk",
                "case \"$1\" in --version) echo omp/18.14.0; exit 0;; esac\n\
                 echo 'this is not a frame'\nexit 0",
            ),
            None,
        ),
    ];
    for (name, bin, stderr_fragment) in cases {
        let out = sb
            .glen(["doctor", "--probe", "--json"])
            .env("GLEN_OMP_BIN", &bin)
            .run();
        out.assert_exit(0).assert_stderr_empty();
        let report = assert_caps(
            &out,
            &Expect {
                protocol: None,
                host_tools: "unknown",
                event_filter: "unknown",
                status: "warn",
            },
        );
        if let Some(fragment) = stderr_fragment {
            let detail = check(&report, "omp_capabilities")["detail"]
                .as_str()
                .unwrap();
            assert!(
                detail.contains(fragment),
                "§A19: the omp's last stderr line is in the detail ({name}): {detail}"
            );
        }
        assert!(!sb.glen_home.exists(), "{name}: no GLEN_HOME");
    }
}

/// The probe's wire behaviour: one omp, RPC mode without a session and with discovery off, in a
/// private empty cwd (S-02), `GLEN_MANAGED=1`, the real `glen_reply` tool and no prompt.
#[test]
fn ac_v2_103_probe_starts_one_sessionless_rpc_omp_and_sends_no_prompt() {
    let rb = RpcBox::new();
    let out = probe(&rb, &[("FAKE_OMP_V2", "1")]);
    out.assert_exit(0);
    rb.assert_quiet();
    let recs = rb.recs();
    let starts = starts(&recs);
    assert_eq!(starts.len(), 1, "exactly one omp process: {starts:?}");
    let start = &starts[0];
    let argv: BTreeSet<String> = strings(&start["argv"]).into_iter().collect();
    assert_eq!(
        argv,
        [
            "--mode",
            "rpc",
            "--no-ui",
            "--no-session",
            "--no-extensions",
            "--no-skills",
            "--no-rules"
        ]
        .map(str::to_owned)
        .into_iter()
        .collect(),
        "§A19 probe process: omp --mode rpc --no-ui --no-session with discovery off, no --resume\n{start}"
    );
    assert_eq!(strings(&start["argv"]).len(), 7, "{start}");
    assert_eq!(start["env"]["GLEN_MANAGED"], "1", "{start}");
    // S-02: never glen's cwd, which may hold a planted `.omp`; a fresh directory, gone after.
    let cwd = Path::new(start["cwd"].as_str().unwrap());
    assert!(!same_file(cwd, &rb.bx.alpha), "{start}");
    assert!(
        cwd.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("glen-omp-")),
        "{start}"
    );
    assert!(!cwd.exists(), "the private cwd is removed: {start}");
    let inbound: BTreeSet<String> = recs
        .iter()
        .filter(|r| r["event"] == "in")
        .filter_map(|r| r["frame"]["type"].as_str().map(str::to_owned))
        .collect();
    let allowed: BTreeSet<String> = ["negotiate_protocol", "set_host_tools", "set_event_filter"]
        .map(str::to_owned)
        .into_iter()
        .collect();
    assert!(
        inbound.is_subset(&allowed),
        "the probe sends only negotiation, host tools and the event filter, got {inbound:?}"
    );
    assert!(inbound.contains("set_host_tools") && inbound.contains("set_event_filter"));
    for forbidden in [
        "prompt",
        "steer",
        "follow_up",
        "abort",
        "set_model",
        "cycle_model",
        "set_thinking_level",
        "compact",
        "bash",
    ] {
        assert!(
            rb.inbound(forbidden).is_empty(),
            "the probe sent {forbidden}"
        );
    }
    let tools = rb.inbound("set_host_tools");
    assert_eq!(tools.len(), 1, "{tools:?}");
    let defs = arr(&tools[0]["tools"]);
    assert_eq!(defs.len(), 1, "only glen_reply is registered: {defs:?}");
    assert_eq!(defs[0]["name"], "glen_reply");
    assert_eq!(defs[0]["description"], A5_DESCRIPTION);
}

/// The probe registers what a job's worker registers: the same tool and the same allowlist.
#[test]
fn ac_v2_103_probe_registers_the_workers_tool_and_event_allowlist() {
    let rb = RpcBox::new();
    probe(&rb, &[]).assert_exit(0);
    rb.assert_quiet();
    let out = rb
        .glen(["send", A.handle, "hello", "--wait", "20s", "--json"])
        .run();
    data_of(&out, 0);
    rb.assert_quiet();
    let tools = rb.inbound("set_host_tools");
    let filters = rb.inbound("set_event_filter");
    assert_eq!(
        tools.len(),
        2,
        "one registration by the probe, one by the worker"
    );
    assert_eq!(filters.len(), 2, "{filters:?}");
    assert_eq!(
        tools[0]["tools"], tools[1]["tools"],
        "same glen_reply definition"
    );
    assert_eq!(
        filters[0]["events"], filters[1]["events"],
        "same event allowlist"
    );
    assert!(!arr(&filters[0]["events"]).is_empty());
}

#[test]
fn ac_v2_103_probe_passes_the_profile_on() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["doctor", "--probe", "--json", "--profile", "ci"])
        .run();
    out.assert_exit(0);
    rb.assert_quiet();
    let starts = rb.starts();
    assert_eq!(starts.len(), 1);
    let argv = strings(&starts[0]["argv"]);
    let at = argv
        .iter()
        .position(|a| a == "--profile")
        .expect("--profile passed");
    assert_eq!(argv[at + 1], "ci", "{argv:?}");
    // `--mode rpc --no-ui --profile ci --no-session --no-extensions --no-skills --no-rules`.
    assert_eq!(argv.len(), 9, "{argv:?}");
}

fn tree_with_contents(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    list_tree(root)
        .into_iter()
        .filter(|rel| !rel.starts_with("logs"))
        .map(|rel| {
            let p = root.join(&rel);
            let bytes = p.is_file().then(|| fs::read(&p).unwrap());
            (rel, bytes)
        })
        .collect()
}

#[test]
fn ac_v2_103_probe_creates_no_glen_home_and_no_session_file() {
    let rb = RpcBox::new();
    assert!(!rb.bx.glen_home.exists());
    let before = tree_with_contents(&rb.bx.root);
    for envs in [
        vec![],
        vec![("FAKE_OMP_V2", "1")],
        vec![("FAKE_OMP_SCENARIO", "reject_filter")],
        vec![("FAKE_OMP_SCENARIO", "reject_tools")],
    ] {
        probe(&rb, &envs).assert_no_panic();
        rb.assert_quiet();
    }
    assert!(
        !rb.bx.glen_home.exists(),
        "doctor --probe created GLEN_HOME"
    );
    let after = tree_with_contents(&rb.bx.root);
    let names =
        |t: &[(String, Option<Vec<u8>>)]| t.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert_eq!(
        names(&after),
        names(&before),
        "no file or directory appeared (sessions, locks, jobs, temp files)"
    );
    assert!(after == before, "an existing file changed during the probe");
    // The same holds in human mode.
    rb.glen(["doctor", "--probe", "--human"])
        .run()
        .assert_exit(0);
    rb.assert_quiet();
    assert!(!rb.bx.glen_home.exists());
    assert_eq!(names(&tree_with_contents(&rb.bx.root)), names(&before));
}

/// Debug builds read `GLEN_PROBE_BUDGET_MS` (§A15); release builds ignore it.
fn budget_knob_is_live() -> bool {
    if cfg!(debug_assertions) {
        true
    } else {
        eprintln!("skipped: GLEN_PROBE_BUDGET_MS is read by debug builds only");
        false
    }
}

const TEARDOWN: Duration = Duration::from_millis(2100);
const SLACK: Duration = Duration::from_millis(2500);

fn assert_returns_within(out: &Out, budget_ms: u64) {
    let bound = scaled(Duration::from_millis(budget_ms) + TEARDOWN + SLACK);
    assert!(
        out.elapsed < bound,
        "§A19: the result is final at the budget ({budget_ms} ms) and teardown takes at most \
         2.1 s, but doctor took {:?}\n{}",
        out.elapsed,
        out.ctx()
    );
}

fn assert_no_omp_left(rb: &RpcBox) {
    poll_until(
        "the probe's omp to be gone",
        scaled(Duration::from_secs(2)),
        || rb.live_processes().is_empty().then_some(()),
    );
}

#[test]
fn ac_v2_103_a_stalled_call_returns_within_the_budget_as_warn_unknown() {
    if !budget_knob_is_live() {
        return;
    }
    // (stalled frame type, FAKE_OMP_V2, protocol to assert if any, host_tools, event_filter)
    type Case = (&'static str, Option<&'static str>, Option<u64>);
    let cases: [(Case, &str, &str); 3] = [
        (("set_host_tools", None, Some(1)), "unknown", "unknown"),
        (("set_event_filter", None, Some(1)), "ok", "unknown"),
        (
            ("negotiate_protocol", Some("1"), None),
            "unknown",
            "unknown",
        ),
    ];
    for ((frame, v2, protocol), host_tools, event_filter) in cases {
        let rb = RpcBox::new();
        let mut envs = vec![("GLEN_PROBE_BUDGET_MS", "1500"), ("FAKE_OMP_STALL", frame)];
        if let Some(v) = v2 {
            envs.push(("FAKE_OMP_V2", v));
        }
        let out = probe(&rb, &envs);
        out.assert_exit(0).assert_stderr_empty();
        assert_returns_within(&out, 1500);
        let report = report_of(&out);
        let row = check(&report, "omp_capabilities");
        assert_eq!(
            row["status"],
            "warn",
            "a timeout proves nothing: {frame}\n{}",
            out.ctx()
        );
        let caps = &report["omp"]["capabilities"];
        assert_eq!(caps["host_tools"], host_tools, "{frame}\n{}", out.ctx());
        assert_eq!(caps["event_filter"], event_filter, "{frame}\n{}", out.ctx());
        if let Some(p) = protocol {
            assert_eq!(caps["protocol"], p, "{frame}\n{}", out.ctx());
        }
        // The stalled command did reach omp and was never answered, so the stall was real.
        let recs = rb.recs();
        assert!(
            recs.iter().any(|r| is_frame(r, "in", frame)),
            "the probe sent {frame}\n{}",
            rb.diag()
        );
        assert!(
            !recs
                .iter()
                .any(|r| r["event"] == "out" && r["frame"]["command"] == frame),
            "{frame} was answered, so nothing stalled\n{}",
            rb.diag()
        );
        assert_no_omp_left(&rb);
    }
}

#[test]
fn ac_v2_103_a_slow_ready_is_clamped_to_the_budget_not_the_60_s_startup_default() {
    if !budget_knob_is_live() {
        return;
    }
    let rb = RpcBox::new();
    let out = probe(
        &rb,
        &[
            ("GLEN_PROBE_BUDGET_MS", "1500"),
            ("FAKE_OMP_READY_DELAY_MS", "20000"),
        ],
    );
    out.assert_exit(0).assert_stderr_empty();
    assert_returns_within(&out, 1500);
    assert_caps(
        &out,
        &Expect {
            protocol: None,
            host_tools: "unknown",
            event_filter: "unknown",
            status: "warn",
        },
    );
    assert_no_omp_left(&rb);

    // A scenario in which ready never comes at all.
    let rb = RpcBox::new();
    let out = probe(
        &rb,
        &[
            ("GLEN_PROBE_BUDGET_MS", "1500"),
            ("FAKE_OMP_SCENARIO", "no_ready"),
        ],
    );
    out.assert_exit(0);
    assert_returns_within(&out, 1500);
    assert_caps(
        &out,
        &Expect {
            protocol: None,
            host_tools: "unknown",
            event_filter: "unknown",
            status: "warn",
        },
    );
    assert_no_omp_left(&rb);
}

/// Kills the process group of a stubborn fake omp even when an assertion fails first.
struct GroupGuard(Option<i64>);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(pgid) = self.0
            && pgid != own_pgid()
        {
            let _ = std::process::Command::new("/bin/kill")
                .args(["-9", &format!("-{pgid}")])
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// The teardown is bounded and complete: an omp that ignores SIGTERM and never reads stdin (so
/// EOF cannot end it), with a child of its own, is gone, group and all, when doctor returns.
#[test]
fn ac_v2_103_a_stubborn_omp_and_its_children_are_killed_within_the_bounded_teardown() {
    if !budget_knob_is_live() {
        return;
    }
    let sb = Sandbox::new();
    let tools = sb.dir("tools");
    let pids = sb.root.join("omp.pids");
    let body = format!(
        "case \"$1\" in --version) echo omp/18.14.0; exit 0;; esac\n\
         trap '' TERM HUP INT\n\
         /bin/sleep 120 &\n\
         echo \"$$ $!\" > '{pids}'\n\
         echo '{{\"type\":\"ready\"}}'\n\
         while :; do /bin/sleep 1; done",
        pids = s(&pids)
    );
    let omp = write_script(&tools, "stubborn", &body);
    let out = sb
        .glen(["doctor", "--probe", "--json"])
        .env("GLEN_OMP_BIN", &omp)
        .env("GLEN_PROBE_BUDGET_MS", "1500")
        .run();
    let text = fs::read_to_string(&pids).unwrap_or_default();
    let ids: Vec<i64> = text
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    let _guard = GroupGuard(ids.first().copied());
    assert_eq!(ids.len(), 2, "the stubborn omp started: {text:?}");
    out.assert_exit(0).assert_stderr_empty();
    assert_returns_within(&out, 1500);
    assert_eq!(
        check(&report_of(&out), "omp_capabilities")["status"],
        "warn",
        "{}",
        out.ctx()
    );
    poll_until(
        "the stubborn omp and its child to be gone",
        scaled(Duration::from_millis(1500)),
        || (!group_alive(ids[0]) && !pid_alive(ids[1])).then_some(()),
    );
}

/// One budget covers startup and every call: a ready that takes most of it leaves the stalled
/// call only what remains, so the total does not restart a budget per call.
#[test]
fn ac_v2_103_one_budget_covers_startup_and_every_call() {
    if !budget_knob_is_live() {
        return;
    }
    let rb = RpcBox::new();
    let out = probe(
        &rb,
        &[
            ("GLEN_PROBE_BUDGET_MS", "5500"),
            ("FAKE_OMP_READY_DELAY_MS", "5000"),
            ("FAKE_OMP_STALL", "set_host_tools"),
        ],
    );
    out.assert_exit(0).assert_stderr_empty();
    assert_returns_within(&out, 5500);
    assert!(
        out.elapsed > Duration::from_millis(4500),
        "ready took 5 s, so the probe cannot have finished earlier: {:?}",
        out.elapsed
    );
    let report = report_of(&out);
    assert_eq!(
        report["omp"]["capabilities"]["host_tools"],
        "unknown",
        "{}",
        out.ctx()
    );
    assert_eq!(check(&report, "omp_capabilities")["status"], "warn");
    assert_no_omp_left(&rb);
}

/// The default budget is 30 s: with the knob unset, an omp whose ready takes a minute is given
/// up on at the 30 s mark (not the 60 s ready default), plus a bounded teardown.
#[test]
fn ac_v2_103_the_default_budget_is_30_seconds() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["doctor", "--probe", "--json"])
        .env_remove("GLEN_PROBE_BUDGET_MS")
        .env("FAKE_OMP_READY_DELAY_MS", "60000")
        .run();
    out.assert_exit(0).assert_stderr_empty();
    assert!(
        out.elapsed >= Duration::from_secs(25),
        "one 30 s budget covers startup, but doctor gave up after {:?}\n{}",
        out.elapsed,
        out.ctx()
    );
    assert_returns_within(&out, 30_000);
    assert_caps(
        &out,
        &Expect {
            protocol: None,
            host_tools: "unknown",
            event_filter: "unknown",
            status: "warn",
        },
    );
    assert_no_omp_left(&rb);
}

// ---------------------------------------------------------------------------------------------
// AC-v2-104: `rpc_protocol` in the status of every RPC-route job.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac_v2_104_a_job_records_the_negotiated_protocol_in_status_json() {
    let cases: [(&[(&str, &str)], u64); 3] = [
        (&[], 1),
        (&[("FAKE_OMP_V2", "1")], 2),
        (&[("FAKE_OMP_V2", "partial")], 1),
    ];
    for (envs, protocol) in cases {
        let rb = RpcBox::new();
        let mut run = rb.glen(["send", A.handle, "hello", "--wait", "20s", "--json"]);
        for (k, v) in envs {
            run = run.env(k, v);
        }
        let d = data_of(&run.run(), 0);
        let id = job_id(&d);
        let st = rb.wait_terminal(&id);
        assert_eq!(st["rpc_protocol"], protocol, "{envs:?}: {st}");
        rb.assert_quiet();
        // It is still there after the job ended.
        assert_eq!(rb.status(&id)["rpc_protocol"], protocol);
        // Job statuses written for a new session carry it too.
        let rb2 = RpcBox::new();
        let mut run = rb2.glen(["send", "@new", "hi", "--wait", "20s", "--json"]);
        for (k, v) in envs {
            run = run.env(k, v);
        }
        let d = data_of(&run.run(), 0);
        let id = job_id(&d);
        assert_eq!(rb2.wait_terminal(&id)["rpc_protocol"], protocol, "{envs:?}");
        rb2.assert_quiet();
    }
}

#[test]
fn ac_v2_104_missing_host_tool_support_keeps_reply_tool_rejected() {
    for v2 in [None, Some("1")] {
        let rb = RpcBox::new();
        let mut run = rb
            .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
            .env("FAKE_OMP_SCENARIO", "reject_tools");
        if let Some(v) = v2 {
            run = run.env("FAKE_OMP_V2", v);
        }
        let out = run.run();
        let err = failed(&out, "job_failed");
        assert_job_failed(&err, "failed", "reply_tool_rejected");
        rb.assert_quiet();
        assert!(
            rb.inbound("prompt").is_empty(),
            "no prompt after a rejected tool"
        );
    }
}

/// Plan table: `status.json` of every RPC-route job gains `rpc_protocol`, "written right after
/// the omp client starts", so a job that fails at host-tool registration still names the
/// protocol its omp negotiated.
#[test]
fn ac_v2_104_a_job_that_fails_at_host_tool_registration_still_records_the_protocol() {
    for (v2, protocol) in [(None, 1), (Some("1"), 2)] {
        let rb = RpcBox::new();
        let mut run = rb
            .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
            .env("FAKE_OMP_SCENARIO", "reject_tools");
        if let Some(v) = v2 {
            run = run.env("FAKE_OMP_V2", v);
        }
        let out = run.run();
        let _ = failed(&out, "job_failed");
        rb.assert_quiet();
        let id = rb.wait_one_job();
        assert_eq!(rb.status(&id)["rpc_protocol"], protocol, "{}", rb.diag());
    }
}

/// A job that fails after the prompt (an omp error, a crash) keeps the protocol too.
#[test]
fn ac_v2_104_a_job_that_fails_later_keeps_the_protocol() {
    for scenario in ["error", "crash"] {
        let rb = RpcBox::new();
        let out = rb
            .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
            .env("FAKE_OMP_SCENARIO", scenario)
            .env("FAKE_OMP_V2", "1")
            .run();
        let _ = failed(&out, "job_failed");
        rb.assert_quiet();
        let id = rb.wait_one_job();
        let st = rb.status(&id);
        assert_eq!(st["status"], "failed", "{scenario}: {st}");
        assert_eq!(st["rpc_protocol"], 2, "{scenario}: {st}");
    }
}

#[test]
fn ac_v2_104_rpc_protocol_is_not_part_of_the_job_view() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hello", "--wait", "20s", "--json"])
        .env("FAKE_OMP_V2", "1")
        .run();
    let d = data_of(&out, 0);
    assert_job_view(&d["job"]);
    assert!(d["job"].get("rpc_protocol").is_none(), "{d}");
    let id = job_id(&d);
    rb.wait_terminal(&id);
    rb.assert_quiet();
    let listing = data_of(&rb.glen(["jobs", "--json"]).run(), 0);
    for row in arr(&listing["jobs"]) {
        assert!(row.get("rpc_protocol").is_none(), "{row}");
    }
    let waited = data_of(&rb.glen(["wait", &job_handle(&id), "--json"]).run(), 0);
    assert!(waited["job"].get("rpc_protocol").is_none(), "{waited}");
}

/// An unowned RPC control runs on a foreground omp that has no status file, so it records
/// nothing; it does not create a job directory to hold a protocol.
#[test]
fn ac_v2_104_an_unowned_control_records_no_job_status() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["models", A.handle, "--json"])
        .env("FAKE_OMP_V2", "1")
        .run();
    out.assert_exit(0);
    rb.assert_quiet();
    assert!(
        !rb.jobs_dir().exists() || rb.job_ids().is_empty(),
        "a foreground control wrote a job: {:?}",
        rb.job_ids()
    );
}

// ---------------------------------------------------------------------------------------------
// AC-v2-105: `omp_surface` and `omp_source_facts` gate omp, with fakes and with a real tree.
// ---------------------------------------------------------------------------------------------

/// Run `cmd` to its exit, killing it at `limit` (scaled). Returns (success, stdout + stderr).
fn run_capture(mut cmd: Command, limit: Duration) -> (bool, String) {
    let limit = scaled(limit);
    cmd.stdin(Stdio::null());
    let mut child = Captured::spawn(&mut cmd).expect("spawn");
    let started = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        if started.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            panic!("did not finish within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let out = child.output(status);
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (status.success(), text)
}

/// The built test executable of an integration test, built once through cargo (the same way CI
/// builds it), so it can be run with a controlled environment. The build puts the glen binary in
/// `target/<profile>` again, so the suite runs glen from `flake::glen()`.
fn test_binary(name: &str) -> PathBuf {
    static CACHE: OnceLock<Mutex<BTreeMap<String, PathBuf>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    if let Some(p) = cache.get(name) {
        return p.clone();
    }
    let out = crate::flake::cargo(&["test", "--no-run", "--message-format=json", "--test", name]);
    assert!(
        out.status.success(),
        "cargo test --no-run --test {name} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let exe = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact" && m["target"]["name"] == name)
        .filter_map(|m| m["executable"].as_str().map(PathBuf::from))
        .next_back()
        .unwrap_or_else(|| panic!("tests/{name}.rs is missing or builds no test executable"));
    cache.insert(name.to_owned(), exe.clone());
    exe
}

/// Run a built integration-test executable from the repo root with `envs` (None removes).
fn run_test_binary(name: &str, envs: &[(&str, Option<&OsStr>)]) -> (bool, String) {
    let mut cmd = Command::new(test_binary(name));
    cmd.current_dir(repo_root())
        .env_remove("GLEN_OMP_SURFACE_BIN")
        .env_remove("GLEN_OMP_SOURCE")
        .env_remove("GLEN_OMP_BIN")
        .env_remove("GLEN_HOME");
    for (k, v) in envs {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    run_capture(cmd, Duration::from_secs(300))
}

fn assert_ran_and_passed(name: &str, ok: bool, text: &str) {
    assert!(ok, "{name} failed:\n{text}");
    let passed: u32 = text
        .lines()
        .filter(|l| l.starts_with("test result: ok."))
        .filter_map(|l| {
            l.split_whitespace()
                .nth(3)
                .and_then(|n| n.parse::<u32>().ok())
        })
        .sum();
    assert!(passed > 0, "{name} ran no test:\n{text}");
}

fn assert_failed(name: &str, ok: bool, text: &str) {
    assert!(!ok, "{name} must fail on this deviation:\n{text}");
    assert!(
        text.contains("FAILED") || text.contains("panicked"),
        "{name} failed without a test failure:\n{text}"
    );
}

#[test]
fn ac_v2_105_omp_surface_passes_against_fake_omp_in_ci_form() {
    let (ok, text) = run_test_binary("omp_surface", &[]);
    assert_ran_and_passed("omp_surface", ok, &text);
    let ci = read_repo(".github/workflows/ci.yml");
    let test_line = ci
        .lines()
        .find(|l| l.trim_start().starts_with("- run: cargo test --locked"))
        .expect("ci.yml runs the cargo test suite");
    assert_eq!(
        test_line.trim(),
        "- run: cargo test --locked",
        "CI runs every test binary, omp_surface included"
    );
    assert!(!ci.contains("--skip"), "CI skips no test");
}

fn wrapper(dir: &Path, name: &str, pre: &str, log: &Path) -> PathBuf {
    let body = format!(
        "{{ echo \"ARGS=$*\"; echo \"HOMEP=$(cd \"$HOME\" && pwd -P)\"; echo \"PWD=$(pwd -P)\"; \
           if [ -f \"$HOME/.omp/agent/models.yml\" ]; then \
             if cmp -s \"$HOME/.omp/agent/models.yml\" '{models}'; then echo MODELS=same; \
             else echo MODELS=different; fi; else echo MODELS=missing; fi; \
           echo \"LS=$(ls -A \"$HOME\" | tr '\\n' ' ')\"; echo ---; }} >> '{log}'\n\
         {pre}\n\
         exec '{fake}' \"$@\"",
        models = s(&fixtures().join("omp-surface/models.yml")),
        log = s(log),
        fake = s(fake_omp_bin()),
    );
    write_script(dir, name, &body)
}

fn wrapper_calls(log: &Path) -> Vec<BTreeMap<String, String>> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .split("---\n")
        .filter(|b| !b.trim().is_empty())
        .map(|b| {
            b.lines()
                .filter_map(|l| l.split_once('='))
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect()
        })
        .collect()
}

#[test]
fn ac_v2_105_omp_surface_runs_a_given_omp_in_its_own_scratch_home_with_the_stub_provider() {
    let sb = Sandbox::new();
    let log = sb.root.join("surface.log");
    let omp = wrapper(&sb.dir("bin"), "omp", "", &log);
    let (ok, text) = run_test_binary(
        "omp_surface",
        &[("GLEN_OMP_SURFACE_BIN", Some(omp.as_os_str()))],
    );
    assert_ran_and_passed("omp_surface with GLEN_OMP_SURFACE_BIN", ok, &text);
    let calls = wrapper_calls(&log);
    assert!(
        !calls.is_empty(),
        "omp_surface never ran the omp it was given:\n{text}"
    );
    let real_home = fs::canonicalize(std::env::var("HOME").expect("HOME")).unwrap();
    assert!(
        calls.iter().any(|c| c["ARGS"].contains("--version")),
        "--version is part of the surface: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c["ARGS"].contains("--help")),
        "--help is part of the surface: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c["ARGS"].contains("--mode") && c["ARGS"].contains("rpc")),
        "the RPC half runs omp --mode rpc: {calls:?}"
    );
    for c in &calls {
        let home = PathBuf::from(&c["HOMEP"]);
        assert_ne!(home, real_home, "omp ran with the owner's HOME: {c:?}");
        assert!(
            !home.starts_with(fs::canonicalize(repo_root()).unwrap()),
            "scratch HOME inside the repo: {c:?}"
        );
        assert_eq!(
            PathBuf::from(&c["PWD"]),
            home,
            "omp's cwd is the scratch HOME: {c:?}"
        );
        assert_eq!(
            c["MODELS"], "same",
            "the loopback stub provider is in place: {c:?}"
        );
        assert!(
            c["LS"].split_whitespace().all(|n| n == ".omp"),
            "the scratch HOME holds only .omp when omp starts: {c:?}"
        );
    }
}

#[test]
fn ac_v2_105_omp_surface_fails_on_each_deviation_of_omp() {
    let sb = Sandbox::new();
    let bin = sb.dir("bin");
    let cases: [(&str, &str); 4] = [
        (
            "help without --no-session",
            "case \"$1\" in --help|-h) '__FAKE__' \"$@\" | grep -v -e '--no-session'; exit 0;; esac",
        ),
        (
            "unparseable --version",
            "case \"$1\" in --version) echo garbage; exit 0;; esac",
        ),
        (
            "glen_reply rejected",
            "export FAKE_OMP_SCENARIO=reject_tools",
        ),
        (
            "event filter rejected",
            "export FAKE_OMP_SCENARIO=reject_filter",
        ),
    ];
    for (name, pre) in cases {
        let log = sb.root.join(format!("{}.log", name.replace(' ', "-")));
        let pre = pre.replace("__FAKE__", &s(fake_omp_bin()));
        let omp = wrapper(&bin, &name.replace(' ', "-"), &pre, &log);
        let (ok, text) = run_test_binary(
            "omp_surface",
            &[("GLEN_OMP_SURFACE_BIN", Some(omp.as_os_str()))],
        );
        assert_failed(&format!("omp_surface ({name})"), ok, &text);
    }
}

fn clone_dir() -> Option<PathBuf> {
    let p = repo_root().join("../metis/references/oh-my-pi");
    p.is_dir().then(|| fs::canonicalize(p).unwrap())
}

fn manifest() -> Value {
    serde_json::from_str(&read_repo("tests/fixtures/omp-surface/facts.json"))
        .expect("facts.json is JSON")
}

fn manifest_files() -> BTreeSet<String> {
    arr(&manifest()["facts"])
        .iter()
        .map(|f| f["file"].as_str().expect("fact.file").to_owned())
        .collect()
}

/// Copy the manifest's files from `src` into `dst`, passing each file's text through `edit`.
fn copy_facts(src: &Path, dst: &Path, edit: impl Fn(&str, String) -> String) {
    for rel in manifest_files() {
        let text = fs::read_to_string(src.join(&rel))
            .unwrap_or_else(|e| panic!("{} is not in the omp tree: {e}", rel));
        let to = dst.join(&rel);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::write(to, edit(&rel, text)).unwrap();
    }
}

#[test]
fn ac_v2_105_the_fact_manifest_is_pinned_to_the_tested_omp() {
    let m = manifest();
    assert_eq!(
        m["omp"],
        compat().tested.as_str(),
        "a bump must re-check the facts"
    );
    let facts = arr(&m["facts"]);
    assert!(!facts.is_empty());
    let mut ids = BTreeSet::new();
    for f in facts {
        for k in ["id", "file", "contains"] {
            assert!(
                f[k].as_str().is_some_and(|s| !s.is_empty()),
                "fact lacks {k}: {f}"
            );
        }
        assert!(
            ids.insert(f["id"].as_str().unwrap().to_owned()),
            "duplicate fact id {f}"
        );
    }
}

#[test]
fn ac_v2_105_omp_source_facts_pass_against_the_reference_clone_read_only() {
    let Some(clone) = clone_dir() else {
        eprintln!("skipped: ../metis/references/oh-my-pi is absent (CI checks the manifest only)");
        return;
    };
    let snapshot = |root: &Path| -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
        manifest_files()
            .into_iter()
            .map(|rel| {
                let p = root.join(&rel);
                let m = fs::metadata(&p).unwrap().modified().unwrap();
                (rel, fs::read(&p).unwrap(), m)
            })
            .collect()
    };
    let before = snapshot(&clone);
    let status_before =
        try_output(Command::new("git").args(["-C", &s(&clone), "status", "--porcelain"])).ok();
    let (ok, text) = run_test_binary("omp_source_facts", &[]);
    assert_ran_and_passed("omp_source_facts", ok, &text);
    assert!(
        before == snapshot(&clone),
        "omp_source_facts changed the omp reference clone"
    );
    let status_after =
        try_output(Command::new("git").args(["-C", &s(&clone), "status", "--porcelain"])).ok();
    assert_eq!(
        status_before.map(|o| o.stdout),
        status_after.map(|o| o.stdout),
        "the reference clone is read-only"
    );
}

#[test]
fn ac_v2_105_omp_source_facts_fail_when_the_tree_lacks_the_facts() {
    let Some(clone) = clone_dir() else {
        eprintln!("skipped: ../metis/references/oh-my-pi is absent");
        return;
    };
    let sb = Sandbox::new();
    // An empty tree, and a tree whose manifest files are all empty.
    let empty = sb.dir("empty-tree");
    let (ok, text) = run_test_binary(
        "omp_source_facts",
        &[("GLEN_OMP_SOURCE", Some(empty.as_os_str()))],
    );
    assert_failed("omp_source_facts on an empty tree", ok, &text);
    let blank = sb.dir("blank-tree");
    copy_facts(&clone, &blank, |_, _| String::new());
    let (ok, text) = run_test_binary(
        "omp_source_facts",
        &[("GLEN_OMP_SOURCE", Some(blank.as_os_str()))],
    );
    assert_failed("omp_source_facts on blank files", ok, &text);
    // The unmutated copy of exactly the manifest's files passes: the mutants below fail for
    // their one missing literal, not for a missing file.
    let same = sb.dir("same-tree");
    copy_facts(&clone, &same, |_, t| t);
    let (ok, text) = run_test_binary(
        "omp_source_facts",
        &[("GLEN_OMP_SOURCE", Some(same.as_os_str()))],
    );
    assert_ran_and_passed(
        "omp_source_facts on a copy of the manifest files",
        ok,
        &text,
    );
}

/// Every fact is really checked: removing any one literal from the tree fails the run.
#[test]
fn ac_v2_105_omp_source_facts_check_every_fact() {
    let Some(clone) = clone_dir() else {
        eprintln!("skipped: ../metis/references/oh-my-pi is absent");
        return;
    };
    let sb = Sandbox::new();
    let m = manifest();
    let facts = arr(&m["facts"]);
    for (i, fact) in facts.iter().enumerate() {
        let (id, file, literal) = (
            fact["id"].as_str().unwrap(),
            fact["file"].as_str().unwrap(),
            fact["contains"].as_str().unwrap(),
        );
        let tree = sb.dir(&format!("mutant-{i}"));
        copy_facts(&clone, &tree, |rel, text| {
            if rel == file {
                text.replace(literal, "\u{1}removed\u{1}")
            } else {
                text
            }
        });
        let (ok, text) = run_test_binary(
            "omp_source_facts",
            &[("GLEN_OMP_SOURCE", Some(tree.as_os_str()))],
        );
        assert_failed(&format!("omp_source_facts without fact {id}"), ok, &text);
    }
}

// ---------------------------------------------------------------------------------------------
// AC-v2-106: `.github/workflows/omp-latest.yml`.
// ---------------------------------------------------------------------------------------------

const WORKFLOW: &str = ".github/workflows/omp-latest.yml";

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

/// Non-comment lines of the top-level block `header` (such as `on:`), without the header.
fn top_block<'a>(text: &'a str, header: &str) -> Vec<&'a str> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines
        .iter()
        .position(|l| indent_of(l) == 0 && l.trim_end() == header)
    else {
        return vec![];
    };
    lines[start + 1..]
        .iter()
        .copied()
        .take_while(|l| l.trim().is_empty() || indent_of(l) > 0 || is_comment(l))
        .filter(|l| !l.trim().is_empty() && !is_comment(l))
        .collect()
}

/// The non-comment lines of `text` outside any `#` comment-only line.
fn code_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|l| !is_comment(l)).collect()
}

fn quoted(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\'' || c == '"' {
            let s: String = chars.by_ref().take_while(|d| *d != c).collect();
            out.push(s);
        }
    }
    out
}

#[test]
fn ac_v2_106_triggers_are_schedule_off_the_hour_dispatch_and_pull_request_on_its_own_path() {
    let wf = read_repo(WORKFLOW);
    let on = top_block(&wf, "on:");
    let triggers: BTreeSet<String> = on
        .iter()
        .filter(|l| indent_of(l) == 2)
        .map(|l| l.trim().trim_end_matches(':').to_owned())
        .collect();
    assert_eq!(
        triggers,
        set(&["pull_request", "schedule", "workflow_dispatch"]),
        "exactly the three documented triggers"
    );
    // schedule: one cron entry, five fields, a numeric off-the-hour minute.
    let crons: Vec<&&str> = on
        .iter()
        .filter(|l| l.trim_start().starts_with("- cron:"))
        .collect();
    assert_eq!(crons.len(), 1, "one schedule entry: {on:?}");
    let cron = quoted(crons[0]).pop().expect("a quoted cron expression");
    let fields: Vec<&str> = cron.split_whitespace().collect();
    assert_eq!(fields.len(), 5, "{cron}");
    let minute: u32 = fields[0].parse().expect("a single numeric minute");
    assert!(
        minute != 0 && minute < 60,
        "off-the-hour minute, got {minute}"
    );
    assert!(
        fields[1].parse::<u32>().is_ok(),
        "a nightly run names an hour: {cron}"
    );
    assert_eq!(&fields[2..], ["*", "*", "*"], "daily: {cron}");
    // pull_request: only its own path.
    let start = on
        .iter()
        .position(|l| indent_of(l) == 2 && l.trim() == "pull_request:")
        .unwrap();
    let block: Vec<&str> = on[start + 1..]
        .iter()
        .copied()
        .take_while(|l| indent_of(l) > 2)
        .collect();
    let joined = block.join("\n");
    let mut paths = quoted(&joined);
    if paths.is_empty() {
        paths = block
            .iter()
            .filter(|l| l.trim_start().starts_with("- "))
            .map(|l| l.trim_start()[2..].trim().to_owned())
            .collect();
    }
    assert_eq!(
        paths,
        [WORKFLOW],
        "pull_request runs only on its own path: {block:?}"
    );
    assert!(
        block.iter().any(|l| l.trim_start().starts_with("paths:")),
        "{block:?}"
    );
    assert!(!block.iter().any(|l| l.contains("branches")), "{block:?}");
}

#[test]
fn ac_v2_106_permissions_are_contents_read_only_and_nothing_references_a_secret() {
    let wf = read_repo(WORKFLOW);
    let perms = top_block(&wf, "permissions:");
    assert_eq!(
        perms.iter().map(|l| l.trim()).collect::<Vec<_>>(),
        ["contents: read"],
        "read-only token"
    );
    for l in code_lines(&wf) {
        if l.trim_start().starts_with("permissions:") {
            assert!(indent_of(l) == 0, "no step or job widens permissions: {l}");
        }
        let low = l.to_lowercase();
        for banned in [
            "secrets.",
            "secrets:",
            "github_token",
            "github.token",
            "gh_token",
            "api_key",
            "id-token",
            "cloudflare",
        ] {
            assert!(
                !low.contains(banned),
                "the workflow references `{banned}`: {l}"
            );
        }
        assert!(
            !low.contains(": write"),
            "no write permission anywhere: {l}"
        );
    }
    let checkout_cfg = wf.contains("persist-credentials: false");
    assert!(
        checkout_cfg,
        "checkout does not persist its token into .git"
    );
}

#[test]
fn ac_v2_106_every_action_is_pinned_to_a_full_commit_sha() {
    let wf = read_repo(WORKFLOW);
    let uses: Vec<&str> = code_lines(&wf)
        .into_iter()
        .filter(|l| l.trim_start().trim_start_matches("- ").starts_with("uses:"))
        .collect();
    assert!(
        uses.len() >= 3,
        "checkout, toolchain and cache at least: {uses:?}"
    );
    for l in uses {
        let value = l
            .trim_start()
            .trim_start_matches("- ")
            .trim_start_matches("uses:")
            .split(" #")
            .next()
            .unwrap()
            .trim();
        let (action, rev) = value
            .split_once('@')
            .unwrap_or_else(|| panic!("an unpinned action: {l}"));
        assert!(!action.starts_with("./"), "{l}");
        assert!(
            rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit()),
            "`{action}` must be pinned to a 40-hex commit SHA, got `{rev}`"
        );
    }
}

#[test]
fn ac_v2_106_no_other_workflow_script_or_the_release_preflight_reads_the_nightly() {
    for rel in [
        ".github/workflows/ci.yml",
        ".github/workflows/release.yml",
        ".github/workflows/flake-proof.yml",
        "scripts/package-release.sh",
        "scripts/flake-check.sh",
        "install.sh",
    ] {
        let text = read_repo(rel);
        for needle in ["omp-latest", "omp_latest"] {
            assert!(!text.contains(needle), "{rel} reads the nightly ({needle})");
        }
    }
    // Nothing in ci.yml or release.yml triggers on the nightly's completion either.
    for rel in [".github/workflows/ci.yml", ".github/workflows/release.yml"] {
        assert!(
            !read_repo(rel).contains("workflow_run"),
            "{rel} reacts to another workflow"
        );
    }
}

#[test]
fn ac_v2_106_checks_need_no_model_provider() {
    let wf = read_repo(WORKFLOW);
    for l in code_lines(&wf) {
        for banned in [
            "glen send",
            "glen wait",
            "glen model",
            "--print",
            "omp -p ",
            "\" -p ",
            "/login",
            "openai",
            "anthropic",
            "api.openai",
        ] {
            assert!(
                !l.to_lowercase().contains(banned),
                "the nightly checks only what needs no model provider; found `{banned}`: {l}"
            );
        }
    }
    assert!(
        wf.contains("tests/fixtures/omp-surface/models.yml"),
        "the loopback stub provider is the only model source"
    );
    assert!(wf.contains("mktemp -d"), "omp runs in a scratch directory");
    assert!(
        wf.contains("omp.sh/install"),
        "the official installer (omp's README)"
    );
    assert!(
        wf.contains("--binary"),
        "the prebuilt binary, never a bun install"
    );
}

struct Step {
    id: Option<String>,
    cond: Option<String>,
    env: Vec<(String, String)>,
    run: Option<String>,
}

fn parse_steps(wf: &str) -> Vec<Step> {
    let lines: Vec<&str> = wf.lines().collect();
    let mut groups: Vec<Vec<&str>> = Vec::new();
    for l in &lines {
        if indent_of(l) == 6 && l.trim_start().starts_with("- ") {
            groups.push(Vec::new());
        }
        if let Some(g) = groups.last_mut() {
            g.push(l);
        }
    }
    groups
        .iter()
        .map(|g| {
            let mut step = Step {
                id: None,
                cond: None,
                env: Vec::new(),
                run: None,
            };
            let norm: Vec<String> = g
                .iter()
                .map(|l| {
                    if indent_of(l) == 6 && l.trim_start().starts_with("- ") {
                        format!("        {}", &l.trim_start()[2..])
                    } else {
                        (*l).to_owned()
                    }
                })
                .collect();
            let mut i = 0;
            while i < norm.len() {
                let line = &norm[i];
                i += 1;
                if is_comment(line) || line.trim().is_empty() || indent_of(line) != 8 {
                    continue;
                }
                let Some((k, v)) = line.trim().split_once(':') else {
                    continue;
                };
                let v = v.trim();
                match k {
                    "id" => step.id = Some(v.to_owned()),
                    "if" => step.cond = Some(v.to_owned()),
                    "env" => {
                        while i < norm.len()
                            && (norm[i].trim().is_empty() || indent_of(&norm[i]) >= 10)
                        {
                            if let Some((ek, ev)) = norm[i].trim().split_once(':') {
                                step.env.push((ek.trim().to_owned(), ev.trim().to_owned()));
                            }
                            i += 1;
                        }
                    }
                    "run" if v == "|" => {
                        let mut body = Vec::new();
                        while i < norm.len()
                            && (norm[i].trim().is_empty() || indent_of(&norm[i]) >= 10)
                        {
                            body.push(norm[i].get(10..).unwrap_or("").to_owned());
                            i += 1;
                        }
                        step.run = Some(body.join("\n") + "\n");
                    }
                    "run" => step.run = Some(v.to_owned()),
                    _ => {}
                }
            }
            step
        })
        .collect()
}

fn step_with<'a>(steps: &'a [Step], marker: &str) -> &'a Step {
    let found: Vec<&Step> = steps
        .iter()
        .filter(|s| s.run.as_deref().is_some_and(|r| r.contains(marker)))
        .collect();
    assert_eq!(found.len(), 1, "exactly one step runs `{marker}`");
    found[0]
}

/// One simulated runner: stub `curl` and `cargo`, a wrapper omp that records its environment,
/// poisoned "secret" variables, and a workspace holding only what the steps read.
struct Runner {
    _dir: tempfile::TempDir,
    root: PathBuf,
    ws: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
    stubs: PathBuf,
    omp_log: PathBuf,
    curl_log: PathBuf,
    cargo_log: PathBuf,
    outputs: BTreeMap<String, String>,
    outcomes: BTreeMap<String, String>,
}

const POISON: [(&str, &str); 5] = [
    ("GITHUB_TOKEN", "poison-github-token"),
    ("CLOUDFLARE_API_TOKEN", "poison-cloudflare-token"),
    ("OPENAI_API_KEY", "poison-openai-key"),
    ("ANTHROPIC_API_KEY", "poison-anthropic-key"),
    ("NPM_TOKEN", "poison-npm-token"),
];

impl Runner {
    fn new(omp_version: &str) -> Runner {
        let dir = tempfile::Builder::new()
            .prefix("glen-nightly-")
            .tempdir()
            .unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let (ws, home, tmp, stubs) = (
            root.join("ws"),
            root.join("runner-home"),
            root.join("tmp"),
            root.join("stubs"),
        );
        for d in [
            &ws,
            &home,
            &tmp,
            &stubs,
            &ws.join("src/omp"),
            &ws.join("tests/fixtures/omp-surface"),
            &ws.join("target/debug"),
        ] {
            fs::create_dir_all(d).unwrap();
        }
        fs::copy(
            repo_root().join("src/omp/compat.json"),
            ws.join("src/omp/compat.json"),
        )
        .unwrap();
        for f in ["models.yml", "facts.json"] {
            fs::copy(
                repo_root().join("tests/fixtures/omp-surface").join(f),
                ws.join("tests/fixtures/omp-surface").join(f),
            )
            .unwrap();
        }
        symlink(glen_bin(), ws.join("target/debug/glen")).unwrap();
        let (omp_log, curl_log, cargo_log, version_file) = (
            root.join("omp.log"),
            root.join("curl.log"),
            root.join("cargo.log"),
            root.join("omp-version"),
        );
        fs::write(&version_file, omp_version).unwrap();
        // The omp the installer "installs": record the environment, then become the fake.
        let omp = write_script(
            &root,
            "installed-omp",
            &format!(
                "{{ echo \"--- omp $*\"; echo \"PWD=$(pwd -P)\"; /usr/bin/env; }} >> '{log}'\n\
                 FAKE_OMP_VERSION=$(cat '{ver}')\nexport FAKE_OMP_VERSION\n\
                 exec '{fake}' \"$@\"",
                log = s(&omp_log),
                ver = s(&version_file),
                fake = s(fake_omp_bin()),
            ),
        );
        write_script(
            &stubs,
            "curl",
            &format!(
                "echo \"curl $*\" >> '{curl}'\n\
                 case \"$*\" in\n\
                 *omp.sh/install*)\n\
                 cat <<'EOS'\n\
                 {{ echo \"--- installer $*\"; echo \"PWD=$(pwd -P)\"; /usr/bin/env; }} >> '{log}'\n\
                 mkdir -p \"$HOME/.local/bin\"\n\
                 cp '{omp}' \"$HOME/.local/bin/omp\"\n\
                 chmod +x \"$HOME/.local/bin/omp\"\n\
                 EOS\n\
                 ;;\n\
                 *)\n\
                 out=\"\"\n\
                 while [ $# -gt 0 ]; do if [ \"$1\" = \"-o\" ]; then out=\"$2\"; fi; shift; done\n\
                 if [ -n \"$out\" ]; then echo stub > \"$out\"; fi\n\
                 ;;\n\
                 esac",
                curl = s(&curl_log),
                log = s(&omp_log),
                omp = s(&omp),
            ),
        );
        write_script(
            &stubs,
            "cargo",
            &format!(
                "{{ echo \"--- cargo $*\"; echo \"PWD=$(pwd -P)\"; \
                   echo \"GLEN_OMP_SURFACE_BIN=${{GLEN_OMP_SURFACE_BIN-unset}}\"; \
                   echo \"GLEN_OMP_SOURCE=${{GLEN_OMP_SOURCE-unset}}\"; }} >> '{cargo}'\nexit 0",
                cargo = s(&cargo_log),
            ),
        );
        Runner {
            _dir: dir,
            root,
            ws,
            home,
            tmp,
            stubs,
            omp_log,
            curl_log,
            cargo_log,
            outputs: BTreeMap::new(),
            outcomes: BTreeMap::new(),
        }
    }

    fn expand(&self, value: &str) -> String {
        let mut out = String::new();
        let mut rest = value;
        while let Some(at) = rest.find("${{") {
            out.push_str(&rest[..at]);
            let end = rest[at..].find("}}").expect("an unclosed expression") + at;
            let expr = rest[at + 3..end].trim();
            let parts: Vec<&str> = expr.split('.').collect();
            let resolved = match parts.as_slice() {
                ["steps", id, "outputs", name] => self
                    .outputs
                    .get(&format!("{id}.{name}"))
                    .cloned()
                    .unwrap_or_else(|| panic!("no output {id}.{name} for `{expr}`")),
                ["steps", id, "outcome"] => self.outcomes.get(*id).cloned().unwrap_or_default(),
                _ => panic!("the step uses an expression this harness cannot evaluate: {expr}"),
            };
            out.push_str(&resolved);
            rest = &rest[end + 2..];
        }
        out.push_str(rest);
        out
    }

    /// Run a step's script like a GitHub `shell: bash` step. Returns (exit code, output).
    fn run(&mut self, step: &Step) -> (i32, String) {
        let script = step.run.clone().expect("a run step");
        let out_file = self.root.join("github-output");
        let _ = fs::remove_file(&out_file);
        let mut cmd = Command::new("/bin/bash");
        cmd.args(["--noprofile", "--norc", "-eo", "pipefail", "-c", &script])
            .current_dir(&self.ws)
            .env_clear()
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmp)
            .env(
                "PATH",
                format!(
                    "{}:/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin",
                    s(&self.stubs)
                ),
            )
            .env("GITHUB_OUTPUT", &out_file)
            .env("GITHUB_STEP_SUMMARY", self.root.join("summary.md"))
            .env("GITHUB_WORKSPACE", &self.ws)
            .env("GITHUB_EVENT_NAME", "schedule");
        for (k, v) in POISON {
            cmd.env(k, v);
        }
        for (k, v) in &step.env {
            cmd.env(k, self.expand(v));
        }
        cmd.stdin(Stdio::null());
        let (tx, rx) = mpsc::channel();
        let child = Captured::spawn(&mut cmd).expect("spawn bash");
        std::thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        let output = rx
            .recv_timeout(scaled(Duration::from_secs(120)))
            .expect("a workflow step did not finish");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if let (Some(id), Ok(written)) = (&step.id, fs::read_to_string(&out_file)) {
            for line in written.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    self.outputs.insert(format!("{id}.{k}"), v.to_owned());
                }
            }
        }
        let code = output.status.code().unwrap_or(-1);
        if let Some(id) = &step.id {
            self.outcomes.insert(
                id.clone(),
                if code == 0 { "success" } else { "failure" }.to_owned(),
            );
        }
        (code, text)
    }

    fn summary(&self) -> String {
        fs::read_to_string(self.root.join("summary.md")).unwrap_or_default()
    }

    /// The directory a plain `mktemp -d` creates in under this runner's environment.
    fn mktemp_parent(&self) -> PathBuf {
        let out = try_output(
            Command::new("/usr/bin/mktemp")
                .arg("-d")
                .env_clear()
                .env("TMPDIR", &self.tmp)
                .env("PATH", "/usr/bin:/bin"),
        )
        .expect("run mktemp -d");
        let dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        let parent = fs::canonicalize(dir.parent().unwrap()).unwrap();
        let _ = fs::remove_dir(&dir);
        parent
    }

    fn omp_calls(&self) -> Vec<(String, BTreeMap<String, String>)> {
        fs::read_to_string(&self.omp_log)
            .unwrap_or_default()
            .split("--- ")
            .filter(|b| !b.trim().is_empty())
            .map(|b| {
                let mut lines = b.lines();
                let head = lines.next().unwrap_or("").to_owned();
                let vars = lines
                    .filter_map(|l| l.split_once('='))
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect();
                (head, vars)
            })
            .collect()
    }
}

impl Drop for Runner {
    /// The steps' own `mktemp -d` directories live outside the runner's root: remove them.
    fn drop(&mut self) {
        let parent = self.mktemp_parent();
        let mut dirs: Vec<PathBuf> = self
            .outputs
            .get("omp.home")
            .map(PathBuf::from)
            .into_iter()
            .collect();
        for l in fs::read_to_string(&self.cargo_log)
            .unwrap_or_default()
            .lines()
        {
            if let Some(tree) = l.strip_prefix("GLEN_OMP_SOURCE=") {
                dirs.push(PathBuf::from(tree));
            }
        }
        for d in dirs {
            let named = d
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("tmp."));
            let inside = fs::canonicalize(&d)
                .ok()
                .and_then(|c| c.parent().map(|p| p == parent))
                .unwrap_or(false);
            if named && inside {
                let _ = fs::remove_dir_all(&d);
            }
        }
    }
}

fn workflow_steps() -> Vec<Step> {
    parse_steps(&read_repo(WORKFLOW))
}

/// Run install, doctor (and, when asked, the surface and facts steps) of the nightly.
fn run_nightly(r: &mut Runner, all: bool) {
    let steps = workflow_steps();
    let install = step_with(&steps, "omp.sh/install");
    let (code, text) = r.run(install);
    assert_eq!(code, 0, "the install step failed:\n{text}");
    if all {
        let (code, text) = r.run(step_with(&steps, "omp_surface"));
        assert_eq!(code, 0, "the surface step failed:\n{text}");
        let (code, text) = r.run(step_with(&steps, "omp_source_facts"));
        assert_eq!(code, 0, "the facts step failed:\n{text}");
    }
    let (code, text) = r.run(step_with(&steps, "doctor --probe"));
    // The step sends glen's report to doctor.json, so a failure shows that file too.
    assert_eq!(
        code,
        0,
        "the doctor step failed:\n{text}\ndoctor.json: {}",
        fs::read_to_string(r.ws.join("doctor.json")).unwrap_or_default()
    );
}

fn is_secretish(name: &str) -> bool {
    let up = name.to_uppercase();
    [
        "TOKEN",
        "KEY",
        "SECRET",
        "GITHUB",
        "CLOUDFLARE",
        "PASSWORD",
        "RUNNER",
    ]
    .iter()
    .any(|m| up == *m || up.contains(m))
}

#[test]
fn ac_v2_106_every_omp_command_runs_in_a_scratch_home_with_no_secrets() {
    let mut r = Runner::new("18.14.0");
    run_nightly(&mut r, false);
    let scratch = PathBuf::from(
        r.outputs
            .get("omp.home")
            .expect("the install step reports its home"),
    );
    let scratch = fs::canonicalize(&scratch).unwrap();
    assert!(scratch.is_dir());
    assert_eq!(
        scratch.parent().unwrap(),
        r.mktemp_parent(),
        "the scratch home is a `mktemp -d` directory: {scratch:?}"
    );
    assert!(
        scratch
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("tmp."),
        "{scratch:?}"
    );
    assert_ne!(scratch, r.home, "never the runner's own HOME");
    assert!(
        scratch != r.ws && !scratch.starts_with(&r.ws),
        "never the workspace: {scratch:?}"
    );
    assert!(
        fs::read_dir(&r.home).unwrap().next().is_none(),
        "nothing was written to the runner's own HOME"
    );
    let calls = r.omp_calls();
    assert!(
        calls.iter().any(|(h, _)| h.starts_with("installer")),
        "the installer ran: {calls:?}"
    );
    assert!(
        calls.iter().any(|(h, _)| h.starts_with("omp --version")),
        "the installed omp reported its version: {calls:?}"
    );
    assert!(
        calls.iter().any(|(h, _)| h.starts_with("omp --mode rpc")),
        "glen's doctor --probe started omp: {calls:?}"
    );
    for (head, vars) in &calls {
        let home = fs::canonicalize(
            vars.get("HOME")
                .unwrap_or_else(|| panic!("no HOME for `{head}`")),
        )
        .unwrap();
        assert_eq!(
            home, scratch,
            "`{head}` ran with a HOME other than the scratch home"
        );
        for (k, v) in vars {
            assert!(
                !v.contains("poison-"),
                "a runner secret reached `{head}` through {k}"
            );
            if ![
                "HOME",
                "PATH",
                "PWD",
                "OLDPWD",
                "SHLVL",
                "_",
                "GLEN_MANAGED",
                "GLEN_HOME",
                "GLEN_OMP_BIN",
                "FAKE_OMP_VERSION",
            ]
            .contains(&k.as_str())
            {
                assert!(
                    !is_secretish(k),
                    "`{head}` inherited the runner variable {k}"
                );
            }
        }
    }
    // The probe omp runs in glen's own private empty directory (S-02), never the workspace, so
    // it finds no project configuration; the directory is gone once the probe ends.
    for (head, vars) in calls
        .iter()
        .filter(|(h, _)| h.starts_with("omp --mode rpc"))
    {
        let pwd = Path::new(&vars["PWD"]);
        assert!(
            pwd.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("glen-omp-")),
            "`{head}` ran outside a private directory: {pwd:?}"
        );
        assert!(!pwd.starts_with(&r.ws), "`{head}` ran in the workspace");
        assert!(!pwd.exists(), "`{head}`'s directory was left behind");
    }
    // The stub provider lives in the scratch home only.
    assert_eq!(
        fs::read_to_string(scratch.join(".omp/agent/models.yml")).unwrap(),
        read_repo("tests/fixtures/omp-surface/models.yml")
    );
    // What the nightly ran really was glen's doctor --probe, reported as a document.
    let doctor: Value = serde_json::from_str(
        &fs::read_to_string(r.ws.join("doctor.json")).expect("the doctor step wrote doctor.json"),
    )
    .expect("doctor.json is JSON");
    assert_eq!(doctor["ok"], true, "{doctor}");
    assert_eq!(
        doctor["data"]["omp"]["capabilities"]["host_tools"], "ok",
        "{doctor}"
    );
}

#[test]
fn ac_v2_106_the_surface_and_facts_steps_are_wired_to_the_installed_omp_and_its_tag() {
    let mut r = Runner::new("v18.14.0+nightly.7");
    run_nightly(&mut r, true);
    let bin = r
        .outputs
        .get("omp.bin")
        .expect("the install step reports the omp binary")
        .clone();
    assert!(Path::new(&bin).is_file(), "{bin}");
    // The installer's version text is normalized before it is used as a tag.
    assert_eq!(r.outputs["omp.version"], "18.14.0", "{:?}", r.outputs);
    let cargo = fs::read_to_string(&r.cargo_log).unwrap();
    let blocks: Vec<&str> = cargo
        .split("--- ")
        .filter(|b| !b.trim().is_empty())
        .collect();
    let surface = blocks
        .iter()
        .find(|b| b.starts_with("cargo test") && b.contains("omp_surface"))
        .unwrap_or_else(|| panic!("no surface run: {cargo}"));
    assert!(surface.contains("--locked"), "{surface}");
    assert!(
        surface.contains(&format!("GLEN_OMP_SURFACE_BIN={bin}")),
        "the surface test is pointed at the installed omp: {surface}"
    );
    let facts = blocks
        .iter()
        .find(|b| b.starts_with("cargo test") && b.contains("omp_source_facts"))
        .unwrap_or_else(|| panic!("no facts run: {cargo}"));
    let tree = facts
        .lines()
        .find_map(|l| l.strip_prefix("GLEN_OMP_SOURCE="))
        .expect("GLEN_OMP_SOURCE");
    assert_ne!(tree, "unset", "{facts}");
    // Only the manifest's files, at the installed version's tag.
    let curl = fs::read_to_string(&r.curl_log).unwrap();
    let urls: BTreeSet<String> = curl
        .lines()
        .flat_map(|l| l.split_whitespace())
        .filter(|w| w.contains("raw.githubusercontent.com"))
        .map(|w| w.trim_matches('"').to_owned())
        .collect();
    let want: BTreeSet<String> = manifest_files()
        .into_iter()
        .map(|f| format!("https://raw.githubusercontent.com/can1357/oh-my-pi/v18.14.0/{f}"))
        .collect();
    assert_eq!(
        urls, want,
        "the facts are fetched from the installed version's tag"
    );
}

fn bump_line(summary: &str) -> bool {
    summary.lines().any(|l| {
        l.trim_start_matches(['-', '*', '>', '#', ' '])
            .to_lowercase()
            .starts_with("bump candidate")
    })
}

fn nightly_summary(version: &str, tweak: impl FnOnce(&mut Runner)) -> String {
    let mut r = Runner::new(version);
    run_nightly(&mut r, true);
    tweak(&mut r);
    let steps = workflow_steps();
    let summary = step_with(&steps, "GITHUB_STEP_SUMMARY");
    assert_eq!(
        summary.cond.as_deref(),
        Some("always()"),
        "the summary runs whatever the earlier steps did"
    );
    let (code, text) = r.run(summary);
    assert_eq!(code, 0, "the summary step failed:\n{text}");
    r.summary()
}

#[test]
fn ac_v2_106_summary_names_the_version_and_flags_a_bump_candidate_only_when_it_is_one() {
    // A newer omp, every step passed: a bump candidate.
    let s = nightly_summary("18.15.0", |_| {});
    assert!(s.contains("18.15.0"), "names the omp version:\n{s}");
    assert!(
        bump_line(&s),
        "a bump candidate line when every step passed and omp is newer:\n{s}"
    );
    assert!(
        s.contains("surface") && s.contains("facts") && s.contains("doctor"),
        "{s}"
    );
    assert!(
        s.to_lowercase().contains("model turns"),
        "names what is not covered: model turns\n{s}"
    );
    assert!(
        s.to_lowercase().contains("lease"),
        "names lease behaviour as uncovered\n{s}"
    );
    // In range: no bump.
    let s = nightly_summary("18.14.0", |_| {});
    assert!(s.contains("18.14.0"), "{s}");
    assert!(!bump_line(&s), "nothing to bump in range:\n{s}");
    // Newer, but a step failed: no bump candidate, and the failure is named.
    let s = nightly_summary("18.15.0", |r| {
        r.outcomes
            .insert("surface".to_owned(), "failure".to_owned());
    });
    assert!(s.contains("18.15.0"), "{s}");
    assert!(
        !bump_line(&s),
        "no bump candidate after a failed step:\n{s}"
    );
    assert!(s.contains("failure"), "the failed outcome is named:\n{s}");
    // §A19: an unreadable version also raises omp_version_untested, but it is not a candidate.
    let s = nightly_summary("garbage", |_| {});
    assert!(
        !bump_line(&s),
        "an unparseable version is not a bump candidate:\n{s}"
    );
    // An older omp is not a candidate either.
    let s = nightly_summary("18.13.0", |_| {});
    assert!(!bump_line(&s), "{s}");
}

#[test]
fn ac_v2_106_summary_is_written_even_when_omp_could_not_be_installed() {
    let mut r = Runner::new("18.15.0");
    let steps = workflow_steps();
    r.outcomes.insert("omp".to_owned(), "failure".to_owned());
    r.outputs.insert("omp.bin".to_owned(), String::new());
    r.outputs.insert("omp.home".to_owned(), String::new());
    r.outputs.insert("omp.version".to_owned(), String::new());
    for id in ["surface", "facts", "doctor"] {
        r.outcomes.insert(id.to_owned(), "skipped".to_owned());
    }
    let (code, text) = r.run(step_with(&steps, "GITHUB_STEP_SUMMARY"));
    assert_eq!(
        code, 0,
        "the summary step failed without an omp or a doctor.json:\n{text}"
    );
    let s = r.summary();
    assert!(!s.trim().is_empty(), "a summary is still written");
    assert!(!bump_line(&s), "{s}");
    assert!(s.contains("failure"), "the install failure is named:\n{s}");
}
