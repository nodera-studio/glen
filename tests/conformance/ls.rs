//! `glen ls` (AC-101–AC-107, AC-121–AC-123, AC-126; docs/design.md §A3 discovery and the
//! bucket rule, §A8 handles, §A9, §A10 reader documents, §A12 sessions dir).

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use crate::reader::*;
use crate::support::*;

/// (extra flags, env) for one colour-precedence run.
type ColourCase<'a> = (Vec<&'a str>, Vec<(&'a str, &'a str)>);

fn human_lines(out: &Out) -> Vec<String> {
    out.out().lines().map(strip_ansi).collect()
}

/// AC-101 as superseded by W2 AC-201: the reader commands plus `send`, `wait` and `jobs` are
/// listed; the remaining v0.1.0 commands stay hidden and exit 8.
#[test]
fn ac101_ac201_help_lists_the_visible_commands_and_the_rest_stay_hidden() {
    let bx = OmpBox::new();
    let out = bx.glen(["--help"]).run();
    out.assert_exit(0);
    let text = out.out();
    let mut names: Vec<&str> = text
        .lines()
        .skip_while(|l| l.trim() != "Commands:")
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "completions",
            "doctor",
            "help",
            "inbox",
            "install-extension",
            "install-skill",
            "jobs",
            "ls",
            "send",
            "show",
            "tail",
            "wait"
        ],
        "{}",
        out.ctx()
    );
    for name in HIDDEN {
        bx.glen([name]).run().error_envelope(if name == "watch" {
            "usage"
        } else {
            "not_implemented"
        });
    }
    bx.assert_no_state();
}

#[test]
fn ac102_ls_lists_the_cwd_bucket_newest_mtime_first() {
    let bx = OmpBox::new();
    let d = bx.data(["ls", "--json"]);
    assert_eq!(ids(&d), want(&[&G, &C, &B, &A]), "{d}");
    assert_eq!(d["scope"]["all"], Value::Bool(false), "{d}");
    assert_eq!(d["scope"]["bucket"], ALPHA, "{d}");
    assert_path(&d["scope"]["cwd"], &bx.alpha);
    assert_path(&d["sessions_dir"], &bx.sessions);
    assert_eq!(d["total"], 4, "{d}");
    assert_eq!(d["total_all"], 7, "{d}");

    let d = bx.data_in(&bx.beta, ["ls", "--json"]);
    assert_eq!(ids(&d), want(&[&E, &D, &F]), "{d}");
    assert_eq!(d["scope"]["bucket"], BETA, "{d}");
    assert_eq!(d["total"], 3, "{d}");

    // --cwd DIR picks the bucket (absolute, or relative to the working directory).
    for arg in [s(&bx.beta), "../beta".to_owned()] {
        let d = bx.data(["ls", "--json", "--cwd", &arg]);
        assert_eq!(ids(&d), want(&[&E, &D, &F]), "--cwd {arg}: {d}");
        assert_eq!(d["scope"]["bucket"], BETA, "{d}");
        assert_path(&d["scope"]["cwd"], &bx.beta);
    }
    bx.assert_no_state();
}

#[test]
fn ac102_limit_defaults_to_20_and_zero_means_no_limit() {
    let bx = OmpBox::new();
    let delta = bx.work("delta");
    let base = SystemTime::now() - Duration::from_secs(3600);
    let mut newest_first = Vec::new();
    for n in 1..=22u64 {
        let id = uuid(n);
        let created = format!("2026-10-06T10:{n:02}:00.000Z");
        let p = bx.add(
            "-work-delta",
            &created,
            &id,
            Some("/home/fixture/work/delta"),
            &[user("d0000001", None, &created, "hello")],
        );
        set_mtime(&p, base + Duration::from_secs(n * 10));
        newest_first.insert(0, id);
    }
    let d = bx.data_in(&delta, ["ls", "--json"]);
    assert_eq!(ids(&d), newest_first[..20], "default -n is 20: {d}");
    assert_eq!(d["total"], 22, "total counts before the limit: {d}");
    assert_eq!(d["total_all"], 29, "{d}");

    let d = bx.data_in(&delta, ["ls", "--json", "-n", "0"]);
    assert_eq!(ids(&d), newest_first, "-n 0 lists everything: {d}");
    let d = bx.data_in(&delta, ["ls", "--json", "-n", "2"]);
    assert_eq!(ids(&d), newest_first[..2], "{d}");
    assert_eq!(d["total"], 22, "{d}");
    let d = bx.data_in(&delta, ["ls", "--json", "--limit", "21"]);
    assert_eq!(ids(&d).len(), 21, "{d}");

    let d = bx.data(["ls", "--all", "--json"]);
    assert_eq!(ids(&d).len(), 20, "{d}");
    assert_eq!(d["total"], 29, "{d}");
    let d = bx.data(["ls", "--all", "--json", "-n", "0"]);
    assert_eq!(ids(&d).len(), 29, "{d}");
}

#[test]
fn ac102_mtime_ties_break_on_header_creation_time_then_path_descending() {
    let bx = OmpBox::new();
    let gamma = bx.work("gamma");
    let bucket = bx.bucket("-work-gamma");
    let cwd = "/home/fixture/work/gamma";
    let same = SystemTime::now() - Duration::from_secs(600);
    // (id, file-name time, header time): the file-name order disagrees with the header order.
    let x = uuid(0xa1);
    let y = uuid(0xa2);
    let p1 = uuid(0xa3);
    let p2 = uuid(0xa4);
    for (id, name_t, header_t) in [
        (&x, "2026-10-06T12:00:00.000Z", "2026-10-06T10:00:00.000Z"),
        (&y, "2026-10-06T09:00:00.000Z", "2026-10-06T11:00:00.000Z"),
        (&p1, "2026-10-06T08:00:00.000Z", "2026-10-06T08:00:00.000Z"),
        (&p2, "2026-10-06T08:00:00.000Z", "2026-10-06T08:00:00.000Z"),
    ] {
        let path = bucket.join(session_name(name_t, id));
        write_lines(&path, &[header(id, Some(header_t), Some(cwd))]);
        set_mtime(&path, same);
    }
    let d = bx.data_in(&gamma, ["ls", "--json"]);
    assert_eq!(
        ids(&d),
        [y, x, p2, p1],
        "equal mtimes: header time desc, then path desc: {d}"
    );
}

#[test]
fn ac103_all_lists_every_bucket_and_conflicts_with_cwd() {
    let bx = OmpBox::new();
    let d = bx.data(["ls", "--all", "--json"]);
    assert_eq!(ids(&d), want(&ALL), "{d}");
    assert_eq!(
        d["scope"],
        json!({"all": true, "cwd": null, "bucket": null}),
        "{d}"
    );
    assert_eq!(d["total"], 7, "{d}");
    assert_eq!(d["total_all"], 7, "{d}");
    for fx in ALL {
        assert_eq!(row(&d, fx.id)["bucket"], fx.bucket, "{d}");
    }
    let out = bx.glen(["ls", "--all", "--cwd", &s(&bx.beta)]).run();
    out.error_envelope("usage");
    let out = bx.glen(["ls", "--all", "--cwd", "x", "--human"]).run();
    out.assert_exit(2);
    bx.assert_no_state();
}

#[test]
fn ac103_human_all_adds_a_project_column() {
    let bx = OmpBox::new();
    let header_words = |args: &[&str]| -> Vec<String> {
        let out = bx.glen(args).env("NO_COLOR", "1").run();
        out.assert_exit(0).assert_no_ansi();
        let lines = human_lines(&out);
        lines
            .first()
            .unwrap_or_else(|| panic!("no output\n{}", out.ctx()))
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    };
    let all = header_words(&["ls", "--all", "--human"]);
    assert!(all.contains(&"project".to_owned()), "{all:?}");
    let local = header_words(&["ls", "--human"]);
    assert!(!local.contains(&"project".to_owned()), "{local:?}");
}

#[test]
fn a10_ls_human_table_shows_handles_titles_and_state_in_order() {
    let bx = OmpBox::new();
    let out = bx
        .glen(["ls", "--all", "--human"])
        .env("NO_COLOR", "1")
        .run();
    out.assert_exit(0).assert_no_ansi();
    assert!(out.stderr.is_empty(), "{}", out.ctx());
    let lines = human_lines(&out);
    let header: Vec<&str> = lines[0].split_whitespace().collect();
    let pos = |w: &str| {
        header
            .iter()
            .position(|h| *h == w)
            .unwrap_or_else(|| panic!("header lacks `{w}`: {header:?}"))
    };
    assert_eq!(pos("handle"), 0, "{header:?}");
    assert!(
        pos("state") < pos("title")
            && pos("title") < pos("updated")
            && pos("updated") < pos("model"),
        "§A9 column order: {header:?}"
    );
    let mut last = 0;
    for fx in ALL {
        let i = lines
            .iter()
            .position(|l| l.split_whitespace().next() == Some(fx.handle))
            .unwrap_or_else(|| panic!("no row for {}\n{}", fx.handle, out.ctx()));
        assert!(i > last, "rows are newest first\n{}", out.ctx());
        last = i;
        assert!(lines[i].contains(fx.title), "{}", lines[i]);
        assert!(lines[i].contains("○ idle"), "{}", lines[i]);
    }
}

#[test]
fn ac104_since_keeps_sessions_no_older_than_the_duration() {
    let bx = OmpBox::new();
    let cases: &[(&str, &[&Fx])] = &[
        ("210", &[&G, &E, &D]),
        ("270s", &[&G, &E, &D, &C]),
        ("10m", &[&G, &E, &D, &C, &B, &A]),
        ("1h", &[&G, &E, &D, &C, &B, &A]),
        ("1d", &[&G, &E, &D, &C, &B, &A]),
        ("1w", &[&G, &E, &D, &C, &B, &A]),
        ("5w", &ALL),
        ("3650d", &ALL),
    ];
    for (dur, expected) in cases {
        let d = bx.data(["ls", "--all", "--json", "-n", "0", "--since", dur]);
        assert_eq!(ids(&d), want(expected), "--since {dur}: {d}");
        assert_eq!(d["total"], expected.len(), "--since {dur}: {d}");
    }
    let d = bx.data(["ls", "--json", "--since", "270"]);
    assert_eq!(ids(&d), want(&[&G, &C]), "{d}");
}

#[test]
fn ac104_since_rejects_unknown_units_and_more_than_3650_days() {
    let bx = OmpBox::new();
    for bad in ["1y", "3651d", "87601h", "1.5h", "h", "", "-1", "10ms"] {
        bx.glen(["ls", "--json", "--since", bad])
            .run()
            .error_envelope("usage");
    }
    bx.data(["ls", "--json", "--since", "87600h"]);
}

#[test]
fn ac104_live_lists_nothing_when_no_lease_is_held() {
    let bx = OmpBox::new();
    let d = bx.data(["ls", "--all", "--live", "--json"]);
    assert_eq!(d["sessions"], json!([]), "{d}");
    assert_eq!(d["total"], 0, "{d}");
    assert_eq!(d["total_all"], 7, "{d}");
    bx.assert_no_state();
}

#[test]
fn ac105_discovery_lists_only_bucket_level_session_files() {
    let bx = OmpBox::new();
    let alpha = bx.sessions.join(ALPHA);
    let t = "2026-10-06T10:00:00.000Z";
    let cwd = Some("/home/fixture/work/alpha");
    let valid = |id: &str| vec![header(id, Some(t), cwd), user("e0000001", None, t, "hi")];

    // Not session files:
    write_lines(
        &alpha.join(format!(".{}", session_name(t, &uuid(1)))),
        &valid(&uuid(1)),
    );
    write_lines(&alpha.join(format!("{}.json", uuid(2))), &valid(&uuid(2)));
    write_lines(
        &alpha.join(session_name(t, &uuid(3))),
        &[
            user("e0000001", None, t, "no header"),
            header(&uuid(3), Some(t), cwd),
        ],
    );
    write_lines(
        &alpha.join("numeric-id.jsonl"),
        &[json!({"type": "session", "version": 3, "id": 4, "timestamp": t})],
    );
    write_lines(
        &alpha.join(session_name(t, &uuid(5))),
        &[
            title_slot("slot only"),
            user("e0000001", None, t, "no header"),
        ],
    );
    let dir_named_jsonl = alpha.join(session_name(t, &uuid(6)));
    fs::create_dir(&dir_named_jsonl).unwrap();
    write_lines(&dir_named_jsonl.join("inner.jsonl"), &valid(&uuid(6)));
    write_lines(
        &bx.sessions.join(session_name(t, &uuid(7))),
        &valid(&uuid(7)),
    );
    let dot_bucket = bx.bucket(".hidden-bucket");
    write_lines(
        &dot_bucket.join(session_name(t, &uuid(8))),
        &valid(&uuid(8)),
    );
    let nested = alpha.join("nested");
    fs::create_dir(&nested).unwrap();
    write_lines(&nested.join(session_name(t, &uuid(9))), &valid(&uuid(9)));
    fs::write(alpha.join(session_name(t, &uuid(10))), "").unwrap();

    // Session files: a plain header, and a title slot before the header.
    let plain = uuid(11);
    bx.add("-work-gamma", t, &plain, cwd, &[]);
    let slotted = uuid(12);
    let p = bx.bucket("-work-gamma").join(session_name(t, &slotted));
    write_lines(&p, &[title_slot("Slotted"), header(&slotted, Some(t), cwd)]);

    let d = bx.data(["ls", "--all", "--json", "-n", "0"]);
    let mut got = ids(&d);
    got.sort();
    let mut expected = want(&ALL);
    expected.extend([plain, slotted.clone()]);
    expected.sort();
    assert_eq!(got, expected, "{d}");
    assert!(
        !d.to_string().contains(S_ID),
        "subagent S is never listed: {d}"
    );
    assert!(!d.to_string().contains(".lock.os"), "{d}");
    assert_eq!(row(&d, &slotted)["title"], "Slotted", "{d}");
}

#[test]
fn ac105_symlinked_files_are_followed_once_and_symlinked_buckets_skipped() {
    let bx = OmpBox::new();
    let t = "2026-10-06T10:00:00.000Z";
    let cwd = Some("/home/fixture/work/gamma");
    let gamma = bx.bucket("-work-gamma");

    let outside = bx.root.join("outside");
    fs::create_dir(&outside).unwrap();
    let linked_file_id = uuid(0xb1);
    let target = outside.join(session_name(t, &linked_file_id));
    write_lines(&target, &[header(&linked_file_id, Some(t), cwd)]);
    symlink(&target, gamma.join(session_name(t, &linked_file_id))).unwrap();

    let linked_bucket = bx.root.join("linked-bucket");
    fs::create_dir(&linked_bucket).unwrap();
    let in_linked_bucket = uuid(0xb2);
    write_lines(
        &linked_bucket.join(session_name(t, &in_linked_bucket)),
        &[header(&in_linked_bucket, Some(t), cwd)],
    );
    symlink(&linked_bucket, bx.sessions.join("-work-linked")).unwrap();

    // A second path to fixture A's file.
    let a_name = Path::new(A.rel).file_name().unwrap();
    symlink(bx.path(&A), gamma.join(a_name)).unwrap();

    let d = bx.data(["ls", "--all", "--json", "-n", "0"]);
    let got = ids(&d);
    assert!(
        got.contains(&linked_file_id),
        "a symlinked file is followed: {d}"
    );
    assert!(
        !got.contains(&in_linked_bucket),
        "a symlinked bucket is skipped: {d}"
    );
    assert_eq!(
        got.iter().filter(|i| *i == A.id).count(),
        1,
        "two paths to one file list it once: {d}"
    );
    assert_eq!(d["total"], 8, "{d}");
}

#[test]
fn ac105_a_header_beyond_the_first_4_kib_is_still_found() {
    let bx = OmpBox::new();
    let t = "2026-10-06T10:00:00.000Z";
    let id = uuid(0xc1);
    let mut h = header(&id, Some(t), Some("/home/fixture/work/gamma"));
    h["title"] = Value::from("Big header");
    h["padding"] = Value::from("x".repeat(10_000));
    let p = bx.bucket("-work-gamma").join(session_name(t, &id));
    write_lines(&p, &[h, user("e0000001", None, t, "hi")]);
    let d = bx.data(["ls", "--all", "--json"]);
    assert_eq!(row(&d, &id)["title"], "Big header", "{d}");
}

#[test]
fn ac106_ls_session_objects_carry_every_field() {
    let bx = OmpBox::new();
    let d = bx.data(["ls", "--all", "--json"]);
    let now = SystemTime::now();
    for fx in ALL {
        let s = row(&d, fx.id);
        assert_session_object(s);
        assert_eq!(s["handle"], fx.handle, "{s}");
        assert_path(&s["path"], &bx.path(fx));
        assert_eq!(s["bucket"], fx.bucket, "{s}");
        assert_eq!(s["cwd"], fx.cwd, "{s}");
        assert_eq!(s["title"], fx.title, "{s}");
        assert_eq!(s["first_message"], fx.first_message, "{s}");
        assert_eq!(utc(&s["created_at"]), instant(fx.created), "{s}");
        let mtime = bx.t0 - Duration::from_secs(fx.age_s);
        let skew = utc(&s["updated_at"]).duration_since(system(mtime)).abs();
        assert!(
            skew < jiff::SignedDuration::from_secs(2),
            "updated_at = mtime: {s}"
        );
        let age = s["age_ms"].as_u64().unwrap();
        let real = now.duration_since(mtime).unwrap().as_millis() as u64;
        assert!(
            age + 2_000 >= fx.age_s * 1000 && age <= real + 2_000,
            "age_ms {age} for a file {}s old: {s}",
            fx.age_s
        );
        assert_eq!(
            s["size_bytes"],
            fs::metadata(bx.path(fx)).unwrap().len(),
            "{s}"
        );
        assert_eq!(s["model"], MODEL, "{s}");
        let turn = if fx.id == B.id { "pending" } else { "complete" };
        assert_eq!(s["last_turn"], turn, "{s}");
        assert_eq!(s["state"], "idle", "{s}");
        assert_eq!(s["state_detail"], Value::Null, "{s}");
        let cont = if fx.id == C.id {
            Value::from(B.id)
        } else {
            Value::Null
        };
        assert_eq!(s["continuation_of"], cont, "{s}");
        let parent = match fx.id {
            id if id == C.id => Value::from(B.id),
            id if id == G.id => Value::from(A.id),
            _ => Value::Null,
        };
        assert_eq!(s["parent_session"], parent, "{s}");
    }
}

#[test]
fn ac106_nullable_fields_are_null_never_omitted() {
    let bx = OmpBox::new();
    let id = uuid(0xd1);
    let p = bx
        .bucket("-work-gamma")
        .join(session_name("2026-10-06T10:00:00.000Z", &id));
    write_lines(&p, &[json!({"type": "session", "id": id})]);
    let d = bx.data(["ls", "--all", "--json", "-n", "0"]);
    let s = row(&d, &id);
    assert_session_object(s);
    for k in [
        "cwd",
        "title",
        "first_message",
        "created_at",
        "model",
        "state_detail",
        "continuation_of",
        "parent_session",
    ] {
        assert_eq!(s[k], Value::Null, "`{k}`: {s}");
    }
    assert_eq!(s["last_turn"], "unknown", "{s}");
    assert_eq!(s["state"], "idle", "{s}");
}

#[test]
fn a10_first_message_preview_keeps_at_most_200_characters() {
    let bx = OmpBox::new();
    let id = uuid(0xd2);
    let t = "2026-10-06T10:00:00.000Z";
    let text: String = "éabcdefghi".repeat(50);
    bx.add(
        "-work-gamma",
        t,
        &id,
        None,
        &[user("e0000001", None, t, &text)],
    );
    let d = bx.data(["ls", "--all", "--json"]);
    let preview = row(&d, &id)["first_message"].as_str().unwrap().to_owned();
    assert!(preview.chars().count() <= 200, "{preview:?}");
    let stem = preview.strip_suffix('…').unwrap_or(&preview);
    assert!(
        !stem.is_empty() && text.starts_with(stem),
        "the preview is the start of the message: {preview:?}"
    );
}

#[test]
fn ac107_handles_are_the_last_seven_hex_digits() {
    let bx = OmpBox::new();
    let d = bx.data(["ls", "--all", "--json"]);
    for fx in ALL {
        let hex = fx.id.replace('-', "");
        assert_eq!(fx.handle, &hex[hex.len() - 7..]);
        assert_eq!(row(&d, fx.id)["handle"], fx.handle, "{d}");
    }
}

#[test]
fn ac107_colliding_tails_widen_only_the_colliding_sessions() {
    let bx = OmpBox::new();
    let t = "2026-10-06T10:00:00.000Z";
    let cases = [
        ("01a20000-0000-7000-8000-0000a1234567", "a1234567"),
        ("01a20001-0000-7000-8000-0000b1234567", "b1234567"),
        ("01a20002-0000-7000-8000-0001c2345678", "1c2345678"),
        ("01a20003-0000-7000-8000-0002c2345678", "2c2345678"),
        ("01a20004-0000-7000-8000-000009234567", "9234567"),
    ];
    for (id, _) in cases {
        bx.add("-work-gamma", t, id, None, &[]);
    }
    let d = bx.data(["ls", "--all", "--json", "-n", "0"]);
    for (id, handle) in cases {
        assert_eq!(row(&d, id)["handle"], handle, "{d}");
    }
    for fx in ALL {
        assert_eq!(row(&d, fx.id)["handle"], fx.handle, "{d}");
    }
}

#[test]
fn ac107_legacy_ids_take_the_shortest_unique_prefix_of_at_least_seven() {
    let bx = OmpBox::new();
    let t = "2026-10-06T10:00:00.000Z";
    let cases = [
        ("abc", "abc"),
        ("abcdefgh1", "abcdefgh1"),
        ("abcdefgh2", "abcdefgh2"),
        ("oldsessionalpha", "oldsess"),
    ];
    for (id, _) in cases {
        let p = bx.bucket("-work-gamma").join(format!("legacy-{id}.jsonl"));
        write_lines(&p, &[header(id, Some(t), None)]);
    }
    let d = bx.data(["ls", "--all", "--json", "-n", "0"]);
    for (id, handle) in cases {
        assert_eq!(row(&d, id)["handle"], handle, "{d}");
    }
}

#[test]
fn ac107_copies_of_one_session_id_share_its_handle() {
    let bx = OmpBox::new();
    fs::copy(bx.path(&A), bx.sessions.join(BETA).join("copy-of-a.jsonl")).unwrap();
    let d = bx.data(["ls", "--all", "--json", "-n", "0"]);
    let copies: Vec<&Value> = arr(&d["sessions"])
        .iter()
        .filter(|s| s["id"] == A.id)
        .collect();
    assert_eq!(copies.len(), 2, "{d}");
    for c in copies {
        assert_eq!(c["handle"], A.handle, "{d}");
    }
}

#[test]
fn a8_every_handle_ls_prints_resolves_back_to_its_session() {
    let bx = OmpBox::new();
    let t = "2026-10-06T10:00:00.000Z";
    for id in [
        "01a20000-0000-7000-8000-0000a1234567",
        "01a20001-0000-7000-8000-0000b1234567",
    ] {
        bx.add(
            "-work-gamma",
            t,
            id,
            None,
            &[user("e0000001", None, t, "hi")],
        );
    }
    let d = bx.data(["ls", "--all", "--json", "-n", "0"]);
    for s in arr(&d["sessions"]) {
        let handle = s["handle"].as_str().unwrap();
        let shown = bx.data(["show", handle, "--json"]);
        let followed = arr(&shown["continued_from"]).contains(&s["id"]);
        assert!(
            shown["session"]["id"] == s["id"] || followed,
            "`show {handle}` resolves to {}: {shown}",
            s["id"]
        );
    }
}

/// §A8 resolves a prefix only from 8 characters, while AC-107 prints 7-character legacy
/// handles; the handle `ls` prints should still be usable with `show` (§A13 agent loop).
#[test]
fn a8_legacy_handles_printed_by_ls_resolve() {
    let bx = OmpBox::new();
    let p = bx.bucket("-work-gamma").join("legacy.jsonl");
    write_lines(
        &p,
        &[
            header("oldsessionalpha", Some("2026-10-06T10:00:00.000Z"), None),
            user("e0000001", None, "2026-10-06T10:00:01.000Z", "hi"),
        ],
    );
    let d = bx.data(["ls", "--all", "--json", "-n", "0"]);
    let handle = row(&d, "oldsessionalpha")["handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let out = bx.glen(["show", &handle, "--json"]).run();
    assert_eq!(
        out.ok_data()["session"]["id"],
        "oldsessionalpha",
        "{}",
        out.ctx()
    );
}

#[test]
fn ac121_ls_lines_fit_columns_and_cut_titles_end_with_an_ellipsis() {
    let bx = OmpBox::new();
    let id = uuid(0xe1);
    let t = "2026-10-06T10:00:00.000Z";
    let long_title = "A very long session title that keeps going ".repeat(5);
    let p = bx.bucket("-work-gamma").join(session_name(t, &id));
    write_lines(
        &p,
        &[
            title_slot(&long_title),
            header(&id, Some(t), Some("/home/fixture/work/gamma")),
            user("e0000001", None, t, "hi"),
        ],
    );
    set_mtime(&p, SystemTime::now() - Duration::from_secs(30));
    let handle = bx.data(["ls", "--all", "--json"])["sessions"][0]["handle"]
        .as_str()
        .unwrap()
        .to_owned();
    for cols in [40usize, 60, 80, 120] {
        for colour in [("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1")] {
            let out = bx
                .glen(["ls", "--all", "--human"])
                .env("COLUMNS", cols.to_string())
                .env(colour.0, colour.1)
                .run();
            out.assert_exit(0);
            for line in out.out().lines() {
                assert!(
                    display_width(line) <= cols,
                    "line wider than COLUMNS={cols}: {:?}\n{}",
                    strip_ansi(line),
                    out.ctx()
                );
            }
            let lines = human_lines(&out);
            let long_row = lines
                .iter()
                .find(|l| l.split_whitespace().next() == Some(handle.as_str()))
                .unwrap_or_else(|| panic!("no row for {handle}\n{}", out.ctx()));
            assert!(
                long_row.contains('…'),
                "a cut title ends with …: {long_row:?}"
            );
        }
    }
}

#[test]
fn ac121_ls_escapes_and_collapses_untrusted_titles() {
    let bx = OmpBox::new();
    let id = uuid(0xe2);
    let t = "2026-10-06T10:00:00.000Z";
    let title = "evil \u{1b}[31mred\u{7} multi\nline\ttitle";
    let mut h = header(&id, Some(t), Some("/home/fixture/\u{1b}[2Jcwd"));
    h["title"] = Value::from(title);
    let p = bx.bucket("-work-gamma").join(session_name(t, &id));
    write_lines(&p, &[h, user("e0000001", None, t, "hi")]);

    let out = bx
        .glen(["ls", "--all", "--human"])
        .env("NO_COLOR", "1")
        .run();
    out.assert_exit(0);
    let text = out.out();
    assert_eq!(raw_controls(&text), [] as [char; 0], "{}", out.ctx());
    assert!(
        text.contains(r"\x1b[31mred"),
        "ESC is shown escaped\n{}",
        out.ctx()
    );
    assert!(
        text.contains("multi line title"),
        "whitespace runs collapse to one space\n{}",
        out.ctx()
    );

    let out = bx
        .glen(["ls", "--all", "--human"])
        .env("CLICOLOR_FORCE", "1")
        .run();
    out.assert_exit(0);
    let text = out.out();
    for raw in ["\u{1b}[31mred", "\u{1b}[2J", "\u{7}"] {
        assert!(
            !text.contains(raw),
            "raw {raw:?} from session data\n{}",
            out.ctx()
        );
    }
    assert!(strip_ansi(&text).contains(r"\x1b[31mred"), "{}", out.ctx());

    // JSON keeps the raw value (§A10 untrusted text).
    let d = bx.data(["ls", "--all", "--json"]);
    assert_eq!(row(&d, &id)["title"], title, "{d}");
    assert_eq!(row(&d, &id)["cwd"], "/home/fixture/\u{1b}[2Jcwd", "{d}");
}

#[test]
fn ac122_a_missing_sessions_dir_is_an_empty_ok_list() {
    let bx = OmpBox::new();
    let missing = bx.root.join("no-such-agent");
    for args in [vec!["ls", "--json"], vec!["ls", "--all", "--json"]] {
        let out = bx.glen(&args).env("PI_CODING_AGENT_DIR", &missing).run();
        let d = out.ok_data();
        out.assert_stderr_empty();
        assert_eq!(d["sessions"], json!([]), "{}", out.ctx());
        assert_eq!(d["total"], 0, "{}", out.ctx());
        assert_eq!(d["total_all"], 0, "{}", out.ctx());
    }
    let out = bx
        .glen(["ls", "--human"])
        .env("PI_CODING_AGENT_DIR", &missing)
        .run();
    out.assert_exit(0);
    assert!(!out.stdout.is_empty(), "{}", out.ctx());
    // The default agent dir under an empty HOME.
    let out = bx
        .glen(["ls", "--json"])
        .env_remove("PI_CODING_AGENT_DIR")
        .run();
    assert_eq!(out.ok_data()["sessions"], json!([]), "{}", out.ctx());
    assert!(!missing.exists());
    bx.assert_no_state();
}

#[test]
fn ac122_reader_commands_create_nothing() {
    let bx = OmpBox::new();
    let before = snapshot(&bx.agent);
    let runs: &[&[&str]] = &[
        &["ls"],
        &["ls", "--all", "--json"],
        &["ls", "--all", "--live", "--json"],
        &["ls", "--all", "--human"],
        &["show", "e27c0de"],
        &["show", "e27c0de", "--raw"],
        &["show", "f13a5b6", "--human"],
        &["show", "c0de"],
        &["show", "nope"],
        &["tail", "e27c0de"],
        &["tail", "c4f9a72", "-n", "0", "--human"],
        &["tail", "nope", "-f"],
    ];
    for args in runs {
        bx.glen(*args).run().assert_no_panic();
    }
    assert_eq!(snapshot(&bx.agent), before, "the omp agent dir changed");
    bx.assert_no_state();
    assert!(is_empty_dir(&bx.alpha), "{:?}", list_tree(&bx.alpha));

    // An existing omp config root stays empty: no run/session-owners is created.
    let omp = bx.home.join(".omp");
    fs::create_dir(&omp).unwrap();
    for args in [
        vec!["ls", "--all"],
        vec!["show", "e27c0de"],
        vec!["tail", "f13a5b6"],
    ] {
        bx.glen(&args).run().assert_exit(0);
    }
    assert!(is_empty_dir(&omp), "{:?}", list_tree(&omp));
    assert!(!bx.glen_home.exists());
}

#[test]
fn ac123_reader_json_never_carries_ansi() {
    let bx = OmpBox::new();
    let runs: &[&[&str]] = &[
        &["ls", "--all", "--json", "--color", "always"],
        &["ls", "--all", "--color", "always"],
        &["show", "e27c0de", "--json", "--color", "always"],
        &["show", "0a3f417", "--color", "always"],
        &["show", "c4f9a72", "--raw", "--json", "--color", "always"],
        &["tail", "e27c0de", "--json", "--color", "always"],
        &["show", "c0de", "--json", "--color", "always"],
    ];
    for args in runs {
        let out = bx
            .glen(*args)
            .env("CLICOLOR_FORCE", "1")
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .run();
        out.json();
        out.assert_no_ansi();
    }
}

#[test]
fn ac123_human_reader_output_follows_the_colour_precedence() {
    let bx = OmpBox::new();
    let commands: [&[&str]; 3] = [
        &["ls", "--all", "--human"],
        &["show", "e27c0de", "--human"],
        &["tail", "e27c0de", "--human"],
    ];
    for cmd in commands {
        let coloured: Vec<ColourCase> = vec![
            (vec!["--color", "always"], vec![("NO_COLOR", "1")]),
            (vec![], vec![("CLICOLOR_FORCE", "1")]),
            (vec![], vec![("CLICOLOR_FORCE", "1"), ("CLICOLOR", "0")]),
        ];
        let plain: Vec<ColourCase> = vec![
            (vec![], vec![("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1")]),
            (vec!["--color", "never"], vec![("CLICOLOR_FORCE", "1")]),
            (vec![], vec![("TERM", "xterm-256color"), ("CLICOLOR", "1")]),
        ];
        for (expect_colour, cases) in [(true, coloured), (false, plain)] {
            for (flags, env) in cases {
                let mut run = bx.glen(cmd.iter().chain(flags.iter()));
                for (k, v) in &env {
                    run = run.env(k, v);
                }
                let out = run.run();
                out.assert_exit(0);
                assert_eq!(
                    has_esc_seq(&out.stdout),
                    expect_colour,
                    "colour expected: {expect_colour}\n{}",
                    out.ctx()
                );
            }
        }
    }
}

#[test]
fn ac126_a_future_mtime_reads_as_zero_age_and_just_now() {
    let bx = OmpBox::new();
    let future = SystemTime::now() + Duration::from_secs(3600);
    set_mtime(&bx.path(&A), future);
    let d = bx.data(["ls", "--json"]);
    assert_eq!(ids(&d)[0], A.id, "the newest mtime sorts first: {d}");
    let a = row(&d, A.id);
    assert_eq!(a["age_ms"], 0, "{a}");
    assert!(utc(&a["updated_at"]) > system(SystemTime::now()), "{a}");

    let out = bx.glen(["ls", "--human"]).env("NO_COLOR", "1").run();
    out.assert_exit(0);
    let lines = human_lines(&out);
    let a_row = lines
        .iter()
        .find(|l| l.split_whitespace().next() == Some(A.handle))
        .unwrap_or_else(|| panic!("no row for A\n{}", out.ctx()));
    assert!(a_row.contains("just now"), "{a_row:?}");
}

#[test]
fn a3_bucket_rule_maps_home_temp_and_outside_directories() {
    let bx = OmpBox::new();
    let bucket_of = |cwd: &Path, extra: &[(&str, &Path)]| -> String {
        let mut run = bx.glen_in(cwd, ["ls", "--json"]);
        for (k, v) in extra {
            run = run.env(k, v);
        }
        let out = run.run();
        out.ok_data()["scope"]["bucket"]
            .as_str()
            .unwrap_or_else(|| panic!("no scope.bucket\n{}", out.ctx()))
            .to_owned()
    };
    assert_eq!(bucket_of(&bx.home, &[]), "-");
    let proj = bx.tmpdir.join("proj");
    fs::create_dir(&proj).unwrap();
    assert_eq!(bucket_of(&proj, &[]), "-tmp-proj");
    let outside = bx.root.join("outside/proj");
    fs::create_dir_all(&outside).unwrap();
    let canon = fs::canonicalize(&outside).unwrap();
    let enc = s(&canon)[1..].replace(['/', '\\', ':'], "-");
    assert_eq!(bucket_of(&outside, &[]), format!("--{enc}--"));
    // A temp root inside HOME is checked first.
    let nested_tmp = bx.home.join("t");
    let x = nested_tmp.join("x");
    fs::create_dir_all(&x).unwrap();
    assert_eq!(bucket_of(&x, &[("TMPDIR", nested_tmp.as_path())]), "-tmp-x");
    // --cwd through a symlink is canonicalised.
    let link = bx.root.join("link-to-alpha");
    symlink(&bx.alpha, &link).unwrap();
    let d = bx.data_in(&bx.beta, ["ls", "--json", "--cwd", &s(&link)]);
    assert_eq!(d["scope"]["bucket"], ALPHA, "{d}");
    assert_eq!(ids(&d), want(&[&G, &C, &B, &A]), "{d}");

    // A session in the temp-root bucket is listed from there.
    let id = uuid(0xf1);
    bx.add("-tmp-proj", "2026-10-06T10:00:00.000Z", &id, None, &[]);
    let d = bx.data_in(&proj, ["ls", "--json"]);
    assert_eq!(ids(&d), [id], "{d}");
}

#[test]
fn a12_ls_reads_the_xdg_data_home_sessions_dir() {
    let bx = OmpBox::new();
    let xdg = bx.root.join("xdg");
    let sessions = xdg.join("omp/sessions");
    copy_tree(&bx.sessions, &sessions);
    let out = bx
        .glen(["ls", "--all", "--json", "-n", "0"])
        .env_remove("PI_CODING_AGENT_DIR")
        .env("XDG_DATA_HOME", &xdg)
        .run();
    let d = out.ok_data();
    assert_path(&d["sessions_dir"], &sessions);
    let mut got = ids(&d);
    got.sort();
    let mut expected = want(&ALL);
    expected.sort();
    assert_eq!(got, expected, "{d}");
    bx.assert_no_state();
}
