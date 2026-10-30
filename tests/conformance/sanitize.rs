//! Untrusted text in human output is escaped; JSON relies on serde escaping
//! (AC-028, AC-009; §A10 "Untrusted text").

use std::os::unix::fs::symlink;

use crate::support::*;
use serde_json::Value;

/// ESC, BEL, DEL, C1 CSI, TAB and LF embedded in a path component.
const EVIL: &str = "evil\u{1b}[31mred\u{7}bell\u{7f}del\u{9b}csi\ttab\nnl";

fn raw_controls(text: &str) -> Vec<char> {
    text.chars()
        .filter(|&c| (c < ' ' && c != '\n') || c == '\u{7f}' || ('\u{80}'..='\u{9f}').contains(&c))
        .collect()
}

#[test]
fn ac028_env_paths_are_escaped_in_human_output() {
    let sb = Sandbox::new();
    let dir = format!("/nonexistent/{EVIL}");
    let out = sb
        .glen(["doctor", "--human"])
        .fake_omp()
        .env("PI_CODING_AGENT_DIR", &dir)
        .run();
    out.assert_exit(0);
    let text = out.out();
    assert_eq!(
        raw_controls(&text),
        [] as [char; 0],
        "raw control chars\n{}",
        out.ctx()
    );
    assert!(
        !text.contains("tab\nnl") && !text.contains("\u{1b}[31mred"),
        "{}",
        out.ctx()
    );
    assert!(
        text.contains(r"evil\x1b[31mred\x07bell"),
        "ESC and BEL are shown escaped\n{}",
        out.ctx()
    );
}

#[test]
fn ac028_escaping_holds_when_colour_is_forced() {
    let sb = Sandbox::new();
    let dir = format!("/nonexistent/{EVIL}");
    let out = sb
        .glen(["doctor", "--human"])
        .fake_omp()
        .env("PI_CODING_AGENT_DIR", &dir)
        .env("CLICOLOR_FORCE", "1")
        .run();
    out.assert_exit(0);
    let text = out.out();
    assert!(!text.contains("\u{1b}[31mred"), "{}", out.ctx());
    assert!(text.contains(r"\x1b[31mred"), "{}", out.ctx());
    assert_eq!(
        raw_controls(&strip_ansi(&text)),
        [] as [char; 0],
        "{}",
        out.ctx()
    );
}

#[test]
fn ac028_omp_binary_path_is_escaped() {
    let sb = Sandbox::new();
    let dir = sb.dir("bin");
    let link = dir.join("omp\u{1b}[7mX");
    symlink(fake_omp(), &link).unwrap();
    let out = sb
        .glen(["doctor", "--human"])
        .env("GLEN_OMP_BIN", &link)
        .run();
    out.assert_exit(0);
    assert!(!has_esc(&out.stdout), "{}", out.ctx());
    assert!(out.out().contains(r"omp\x1b[7mX"), "{}", out.ctx());
}

#[test]
fn ac028_env_values_in_human_errors_are_escaped() {
    let sb = Sandbox::new();
    for (var, value) in [
        ("OMP_PROFILE", "bad\u{1b}[2Jname"),
        ("GLEN_HOME", "rel\u{1b}[2Jdir"),
    ] {
        let out = sb
            .glen(["doctor", "--human"])
            .fake_omp()
            .env(var, value)
            .run();
        out.assert_exit(2);
        out.assert_no_ansi();
        assert_eq!(raw_controls(&out.err()), [] as [char; 0], "{}", out.ctx());
    }
}

#[test]
fn ac009_json_escapes_c0_controls() {
    let sb = Sandbox::new();
    let dir = format!("/nonexistent/{EVIL}");
    let out = sb
        .glen(["doctor", "--json", "--color", "always"])
        .fake_omp()
        .env("PI_CODING_AGENT_DIR", &dir)
        .env("CLICOLOR_FORCE", "1")
        .run();
    out.ok_data();
    let text = out.out();
    let body = text.strip_suffix('\n').unwrap_or(&text);
    // serde escapes every C0 control (ESC, BEL, TAB, LF); DEL and C1 are legal raw in JSON.
    assert!(
        !body.chars().any(|c| c < ' '),
        "JSON carries no raw C0 control chars\n{}",
        out.ctx()
    );
}

/// §A10: "JSON relies on serde's escaping" and JSON paths are reported as resolved, so a path
/// read back from the envelope must equal the path glen resolved, byte for byte.
#[test]
fn a10_json_round_trips_untrusted_text_exactly() {
    let sb = Sandbox::new();
    let dir = format!("/nonexistent/{EVIL}");
    let out = sb
        .glen(["doctor", "--json"])
        .fake_omp()
        .env("PI_CODING_AGENT_DIR", &dir)
        .run();
    let data = out.ok_data();
    assert_eq!(
        data["paths"]["agent_dir"],
        Value::from(dir.as_str()),
        "JSON must carry the path unchanged\n{}",
        out.ctx()
    );
}
