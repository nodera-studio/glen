//! The read-only owner-lease probe and the state column (AC-104 `--live`, AC-108, AC-109,
//! AC-122; docs/design.md §A3 Liveness and State column, §A12 rule 7).
//!
//! Leases are held from this test process exactly as omp holds them: a bound abstract datagram
//! socket on Linux, an exclusive `flock` on `<session_owners>/<id>.lock` elsewhere.

use std::fs;
use std::time::Duration;

use serde_json::{Value, json};

use crate::reader::*;

#[test]
fn ac109_harness_lease_names_match_the_plan_golden_vectors() {
    assert_eq!(
        lock_name(
            "/home/fixture/.omp/run/session-owners/01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.lock"
        ),
        "omp-file-lock-fdf8456569e488c6109108ed93e83ae3"
    );
    assert_eq!(
        lock_name("/x.lock"),
        "omp-file-lock-414147c7ad2644ab5392fc41faa97fcf"
    );
}

#[test]
fn ac108_a_held_lease_reads_live_or_busy_and_a_released_one_idle() {
    let bx = OmpBox::new();
    let owners = bx.owners();
    let held_a = hold_lease(&owners, A.id);
    let held_b = hold_lease(&owners, B.id);

    let d = bx.data(["ls", "--all", "--json"]);
    for fx in ALL {
        let s = row(&d, fx.id);
        assert_session_object(s);
        let state = match fx.id {
            id if id == A.id => "live",
            id if id == B.id => "busy",
            _ => "idle",
        };
        assert_eq!(s["state"], state, "{s}");
        assert_eq!(s["state_detail"], Value::Null, "{s}");
    }
    assert_eq!(
        row(&d, B.id)["last_turn"],
        "pending",
        "last_turn is independent of state"
    );
    assert_eq!(row(&d, A.id)["last_turn"], "complete");

    // AC-104 --live keeps live and busy rows; total counts them before the limit.
    let d = bx.data(["ls", "--all", "--live", "--json"]);
    assert_eq!(ids(&d), want(&[&B, &A]), "{d}");
    assert_eq!(d["total"], 2, "{d}");
    let d = bx.data(["ls", "--all", "--live", "--json", "-n", "1"]);
    assert_eq!(ids(&d), want(&[&B]), "{d}");
    assert_eq!(d["total"], 2, "{d}");
    let d = bx.data_in(&bx.beta, ["ls", "--live", "--json"]);
    assert_eq!(d["sessions"], json!([]), "{d}");

    // show and tail probe their session too.
    let d = bx.data(["show", A.handle, "--json"]);
    assert_eq!(d["session"]["state"], "live", "{d}");
    let d = bx.data(["tail", A.handle, "-n", "1", "--json"]);
    assert_eq!(d["session"]["state"], "live", "{d}");

    let out = bx.glen(["ls", "--human"]).env("NO_COLOR", "1").run();
    out.assert_exit(0);
    let text = out.out();
    let line_of = |h: &str| {
        text.lines()
            .find(|l| l.split_whitespace().next() == Some(h))
            .unwrap_or_else(|| panic!("no row for {h}\n{}", out.ctx()))
            .to_owned()
    };
    assert!(line_of(A.handle).contains("● live"), "{}", out.ctx());
    assert!(line_of(B.handle).contains("◐ busy"), "{}", out.ctx());
    assert!(line_of(C.handle).contains("○ idle"), "{}", out.ctx());

    drop(held_a);
    drop(held_b);
    let d = bx.data(["ls", "--all", "--json"]);
    for fx in ALL {
        assert_eq!(row(&d, fx.id)["state"], "idle", "released: {d}");
    }
}

/// Decision `31de7685`: busy iff the last branch message is in flight; `last_turn` keeps
/// omp's `deriveSessionStatus` classification.
#[test]
fn ac108_busy_means_the_last_branch_message_is_in_flight() {
    let bx = OmpBox::new();
    let t = "2026-10-06T10:00:01.000Z";
    let q = |id: &str| user(id, None, t, "do it");
    let call = |stop: &str| {
        assistant(
            "a0000002",
            Some("a0000001"),
            t,
            json!([{"type": "text", "text": "Running."}, tool_call("t1", "bash", json!({"command": "ls"}))]),
            stop,
        )
    };
    let text_reply = |stop: &str| {
        assistant(
            "a0000002",
            Some("a0000001"),
            t,
            json!([{"type": "text", "text": "Done."}]),
            stop,
        )
    };
    let mut errored = call("error");
    errored["message"]["errorMessage"] = Value::from("boom");
    let cases: Vec<(&str, Vec<Value>, &str, &str)> = vec![
        ("user last", vec![q("a0000001")], "busy", "pending"),
        (
            "developer last",
            vec![
                q("a0000001"),
                text_reply("stop"),
                message(
                    "a0000003",
                    Some("a0000002"),
                    t,
                    json!({"role": "developer", "content": "Continue.", "timestamp": 0}),
                ),
            ],
            "busy",
            "unknown",
        ),
        (
            "toolResult last",
            vec![
                q("a0000001"),
                call("toolUse"),
                tool_result("a0000003", Some("a0000002"), t, "t1", "bash", "out", false),
            ],
            "busy",
            "interrupted",
        ),
        (
            "toolCall in flight",
            vec![q("a0000001"), call("toolUse")],
            "busy",
            "interrupted",
        ),
        (
            "toolCall aborted",
            vec![q("a0000001"), call("aborted")],
            "live",
            "aborted",
        ),
        (
            "toolCall errored",
            vec![q("a0000001"), errored],
            "live",
            "error",
        ),
        (
            "cut off at length",
            vec![q("a0000001"), text_reply("length")],
            "live",
            "interrupted",
        ),
        (
            "complete reply",
            vec![q("a0000001"), text_reply("stop")],
            "live",
            "complete",
        ),
        (
            "bash execution last",
            vec![
                q("a0000001"),
                text_reply("stop"),
                message(
                    "a0000003",
                    Some("a0000002"),
                    t,
                    json!({"role": "bashExecution", "command": "ls", "output": "x", "exitCode": 0, "cancelled": false, "truncated": false, "timestamp": 0}),
                ),
            ],
            "live",
            "unknown",
        ),
        ("no messages", vec![], "live", "unknown"),
        (
            "user then non-message entries",
            vec![
                q("a0000001"),
                entry(
                    "label",
                    "a0000002",
                    Some("a0000001"),
                    t,
                    json!({"targetId": "a0000001", "label": "x"}),
                ),
                entry(
                    "custom",
                    "a0000003",
                    Some("a0000002"),
                    t,
                    json!({"customType": "state", "data": {}}),
                ),
            ],
            "busy",
            "pending",
        ),
    ];
    let mut holds = Vec::new();
    let mut expect = Vec::new();
    for (n, (name, body, state, turn)) in cases.into_iter().enumerate() {
        let id = uuid(0x800 + n as u64);
        bx.add("-work-gamma", "2026-10-06T10:00:00.000Z", &id, None, &body);
        holds.push(hold_lease(&bx.owners(), &id));
        expect.push((name, id, state, turn));
    }
    let d = bx.data(["ls", "--all", "--json", "-n", "0"]);
    for (name, id, state, turn) in &expect {
        let s = row(&d, id);
        assert_eq!(s["state"], *state, "{name}: {s}");
        assert_eq!(s["last_turn"], *turn, "{name}: {s}");
    }
    // show reads the same state from the full branch.
    let (_, busy_id, _, _) = &expect[0];
    let d = bx.data(["show", busy_id.as_str(), "--json"]);
    assert_eq!(d["session"]["state"], "busy", "{d}");
    drop(holds);
}

#[test]
fn ac108_a_held_session_with_an_oversized_last_line_reads_its_full_branch() {
    let bx = OmpBox::new();
    let t = "2026-10-06T10:00:01.000Z";
    let big = "z".repeat(40 * 1024);
    let base = |id: &str| {
        vec![
            user("a0000001", None, t, "read the big file"),
            assistant(
                "a0000002",
                Some("a0000001"),
                t,
                json!([tool_call(
                    "t1",
                    "read",
                    json!({"path": format!("big-{id}")})
                )]),
                "toolUse",
            ),
        ]
    };
    let busy = uuid(0x901);
    let mut body = base(&busy);
    body.push(tool_result(
        "a0000003",
        Some("a0000002"),
        t,
        "t1",
        "read",
        &big,
        false,
    ));
    bx.add(
        "-work-gamma",
        "2026-10-06T10:00:00.000Z",
        &busy,
        None,
        &body,
    );

    let live = uuid(0x902);
    let mut body = base(&live);
    body.push(tool_result(
        "a0000003",
        Some("a0000002"),
        t,
        "t1",
        "read",
        "small",
        false,
    ));
    body.push(reply("a0000004", Some("a0000003"), t, &big));
    bx.add(
        "-work-gamma",
        "2026-10-06T10:00:00.000Z",
        &live,
        None,
        &body,
    );

    let unheld = uuid(0x903);
    let mut body = base(&unheld);
    body.push(tool_result(
        "a0000003",
        Some("a0000002"),
        t,
        "t1",
        "read",
        &big,
        false,
    ));
    bx.add(
        "-work-gamma",
        "2026-10-06T10:00:00.000Z",
        &unheld,
        None,
        &body,
    );

    let _h1 = hold_lease(&bx.owners(), &busy);
    let _h2 = hold_lease(&bx.owners(), &live);
    let d = bx.data(["ls", "--all", "--json", "-n", "0"]);
    assert_eq!(row(&d, &busy)["state"], "busy", "{d}");
    assert_eq!(row(&d, &busy)["last_turn"], "interrupted", "{d}");
    assert_eq!(row(&d, &live)["state"], "live", "{d}");
    assert_eq!(row(&d, &live)["last_turn"], "complete", "{d}");
    assert_eq!(row(&d, &unheld)["state"], "idle", "{d}");
}

#[test]
fn ac108_ids_that_are_not_plain_tokens_probe_as_unknown() {
    let bx = OmpBox::new();
    let long = "x".repeat(129);
    let odd = ["legacy.session.1", "has space", long.as_str()];
    for (n, id) in odd.iter().enumerate() {
        let p = bx.bucket("-work-gamma").join(format!("odd-{n}.jsonl"));
        write_lines(&p, &[header(id, Some("2026-10-06T10:00:00.000Z"), None)]);
    }
    let d = bx.data(["ls", "--all", "--json", "-n", "0"]);
    for id in odd {
        let s = row(&d, id);
        assert_session_object(s);
        assert_eq!(s["state"], "unknown", "{s}");
        assert!(
            s["state_detail"]
                .as_str()
                .is_some_and(|t| !t.trim().is_empty()),
            "unknown carries a reason: {s}"
        );
    }
    for fx in ALL {
        assert_eq!(row(&d, fx.id)["state"], "idle", "{d}");
    }
}

#[test]
fn ac108_owner_leases_live_under_xdg_state_home_when_it_has_an_omp_dir() {
    let bx = OmpBox::new();
    let state = bx.root.join("state");
    fs::create_dir_all(state.join("omp")).unwrap();
    let _xdg_a = hold_lease(&state.join("omp/run/session-owners"), A.id);
    let _base_b = hold_lease(&bx.owners(), B.id);

    let out = bx
        .glen(["ls", "--all", "--json"])
        .env("XDG_STATE_HOME", &state)
        .run();
    let d = out.ok_data();
    assert_eq!(row(&d, A.id)["state"], "live", "{d}");
    assert_eq!(
        row(&d, B.id)["state"],
        "idle",
        "XDG_STATE_HOME/omp wins: {d}"
    );

    let d = bx.data(["ls", "--all", "--json"]);
    assert_eq!(row(&d, A.id)["state"], "idle", "{d}");
    assert_eq!(row(&d, B.id)["state"], "busy", "{d}");
}

/// macOS (and every non-Linux Unix) probes the lock file: an unreadable file or a
/// non-regular lock path is inconclusive, never idle.
#[cfg(not(target_os = "linux"))]
#[test]
fn ac108_unreadable_or_non_regular_lock_paths_are_unknown() {
    use std::os::unix::fs::PermissionsExt;
    let bx = OmpBox::new();
    let owners = bx.owners();
    fs::create_dir_all(&owners).unwrap();
    let lock = owners.join(format!("{}.lock", A.id));
    fs::write(&lock, "").unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o000)).unwrap();
    let _restore = RestoreMode(lock.clone(), 0o600);
    if fs::File::open(&lock).is_ok() {
        eprintln!("skipped: running as root, permissions are not enforced");
        return;
    }
    fs::create_dir(owners.join(format!("{}.lock", B.id))).unwrap();

    let d = bx.data(["ls", "--all", "--json"]);
    for fx in [&A, &B] {
        let s = row(&d, fx.id);
        assert_eq!(s["state"], "unknown", "{s}");
        assert!(s["state_detail"].is_string(), "{s}");
    }
    assert_eq!(row(&d, C.id)["state"], "idle", "{d}");

    // An unreadable owners directory: every probe is inconclusive.
    let bx = OmpBox::new();
    let owners = bx.owners();
    fs::create_dir_all(&owners).unwrap();
    fs::set_permissions(&owners, fs::Permissions::from_mode(0o000)).unwrap();
    let _restore = RestoreMode(owners.clone(), 0o700);
    let d = bx.data(["ls", "--all", "--json"]);
    for fx in ALL {
        assert_eq!(row(&d, fx.id)["state"], "unknown", "{d}");
    }
}

#[test]
fn ac109_the_probe_creates_and_changes_nothing() {
    let bx = OmpBox::new();
    let held = hold_lease(&bx.owners(), A.id);
    let omp_before = snapshot(&bx.home);
    let agent_before = snapshot(&bx.agent);
    let runs: &[&[&str]] = &[
        &["ls", "--all", "--json"],
        &["ls", "--all", "--live", "--json"],
        &["ls", "--all", "--human"],
        &["show", "e27c0de", "--json"],
        &["show", "f13a5b6", "--json"],
        &["tail", "e27c0de", "-n", "1", "--json"],
    ];
    for args in runs {
        bx.glen(*args).run().assert_exit(0);
    }
    assert_eq!(
        snapshot(&bx.home),
        omp_before,
        "the probe wrote under $HOME"
    );
    assert_eq!(snapshot(&bx.agent), agent_before, "the agent dir changed");
    drop(held);
    // A free, existing lock file: probing it changes nothing either.
    std::thread::sleep(Duration::from_millis(20));
    let before = snapshot(&bx.home);
    let d = bx.data(["ls", "--all", "--json"]);
    assert_eq!(row(&d, A.id)["state"], "idle", "{d}");
    assert_eq!(snapshot(&bx.home), before);
    #[cfg(target_os = "linux")]
    assert!(
        !bx.home.join(".omp").exists(),
        "{:?}",
        crate::support::list_tree(&bx.home)
    );
    assert!(!bx.glen_home.exists());
}
