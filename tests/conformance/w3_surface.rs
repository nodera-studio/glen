//! The W3 command surface (AC-301) and the `glen_reply` transcript row (AC-327, orchestrator O8;
//! docs/design.md §A5, §A9, §A10).

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::reader::*;
use crate::support::*;

fn listed_commands(help: &str) -> Vec<String> {
    let mut names: Vec<String> = help
        .lines()
        .skip_while(|l| l.trim() != "Commands:")
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .filter(|l| l.starts_with(' '))
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------------------------
// AC-301 — the visible commands.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac301_help_lists_exactly_the_w3_commands() {
    let sb = Sandbox::new();
    let mut want = VISIBLE;
    want.sort();
    for args in [&["--help"][..], &["-h"][..], &["help"][..]] {
        let out = sb.glen(args).run();
        out.assert_exit(0);
        assert_eq!(listed_commands(&out.out()), want, "{}", out.ctx());
        assert!(!out.out().contains("__worker"), "{}", out.ctx());
    }
    for name in HIDDEN {
        let err = sb.glen([name]).run().error_envelope("not_implemented");
        assert!(err["message"].as_str().unwrap().contains(name), "{err}");
    }
    assert!(!sb.glen_home.exists());
}

#[test]
fn ac301_completions_offer_inbox_and_install_extension_but_never_worker() {
    let sb = Sandbox::new();
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let out = sb.glen(["completions", shell]).run();
        out.assert_exit(0);
        let script = out.out();
        for name in ["inbox", "watch", "install-extension"] {
            assert!(script.contains(name), "{shell} lacks {name}");
        }
        assert!(!script.contains("__worker"), "{shell} exposes __worker");
    }
}

// ---------------------------------------------------------------------------------------------
// AC-327 (O8) — the glen_reply row.
// ---------------------------------------------------------------------------------------------

const TS: &str = "2026-10-06T10:00:00.000Z";

/// A session in the alpha bucket whose assistant called `glen_reply` with `text`.
fn reply_session(bx: &OmpBox, text: &str) -> (String, PathBuf) {
    let id = uuid(0x0327);
    let call = tool_call(
        "call_r1",
        "glen_reply",
        json!({"text": text, "final": true}),
    );
    let path = bx.add(
        ALPHA,
        TS,
        &id,
        Some("/home/fixture/work/alpha"),
        &[
            user("u0000001", None, TS, "Please report back."),
            assistant("a0000001", Some("u0000001"), TS, json!([call]), "toolUse"),
            tool_result(
                "t0000001",
                Some("a0000001"),
                TS,
                "call_r1",
                "glen_reply",
                "delivered",
                false,
            ),
            reply("a0000002", Some("t0000001"), TS, "Reported."),
        ],
    );
    (tail7(&id), path)
}

fn tail7(id: &str) -> String {
    let simple: String = id.chars().filter(|c| *c != '-').collect();
    simple[simple.len() - 7..].to_owned()
}

fn human(bx: &OmpBox, args: &[&str], columns: &str) -> Vec<String> {
    let out = bx
        .glen(args)
        .env("NO_COLOR", "1")
        .env("COLUMNS", columns)
        .run();
    out.assert_exit(0).assert_no_ansi();
    out.out().lines().map(|l| l.trim_end().to_owned()).collect()
}

fn reply_rows(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter(|l| l.contains("glen_reply"))
        .map(|l| l.trim().to_owned())
        .collect()
}

#[test]
fn ac327_show_and_tail_summarise_a_glen_reply_call_as_its_text() {
    let bx = OmpBox::new();
    let (h, _) = reply_session(&bx, "hello from omp");
    for args in [
        vec!["show", h.as_str(), "--human"],
        vec!["tail", h.as_str(), "--human"],
    ] {
        let rows = reply_rows(&human(&bx, &args, "100"));
        assert_eq!(
            rows,
            [r#"↳ glen_reply "hello from omp" · ✓ delivered"#],
            "O8 row in `{}`",
            args.join(" ")
        );
    }
}

#[test]
fn ac327_the_reply_text_is_one_line_and_truncated_to_the_row() {
    let bx = OmpBox::new();
    let long = format!("first line\nsecond {}", "word ".repeat(40));
    let (h, _) = reply_session(&bx, &long);
    let lines = human(&bx, &["show", &h, "--human"], "60");
    let rows = reply_rows(&lines);
    assert_eq!(rows.len(), 1, "{lines:#?}");
    let row = &rows[0];
    assert!(
        row.starts_with(r#"↳ glen_reply "first line second"#),
        "{row}"
    );
    assert!(row.ends_with("· ✓ delivered"), "{row}");
    assert!(!row.contains("\"text\""), "no raw JSON arguments: {row}");
    // Only the row is in scope here: W1 headers are not part of O8.
    let full = lines.iter().find(|l| l.contains("glen_reply")).unwrap();
    assert!(
        display_width(full) <= 60,
        "row wider than COLUMNS: {full:?}"
    );
}

#[test]
fn ac327_json_keeps_the_raw_arguments() {
    let bx = OmpBox::new();
    let (h, _) = reply_session(&bx, "hello from omp");
    let d = bx.data(["show", h.as_str(), "--tools", "full", "--json"]);
    let call: Value = arr(&d["items"])
        .iter()
        .flat_map(|i| i["tool_calls"].as_array().cloned().unwrap_or_default())
        .find(|c| c["name"] == "glen_reply")
        .expect("a glen_reply tool call");
    assert_eq!(
        call["arguments"],
        json!({"text": "hello from omp", "final": true}),
        "{call}"
    );
}
