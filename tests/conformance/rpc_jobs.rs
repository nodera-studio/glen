//! W2 job store readers: `glen jobs` (AC-220), `glen wait` handle resolution (AC-221), reader-side
//! lost detection on crafted statuses (AC-214) and retention on `send` (AC-223); docs/design.md
//! §A6, §A7, §A10. Crafted jobs are copies of a real job directory with patched `job.json` /
//! `status.json`, so they carry whatever fields the build writes.

use std::collections::BTreeSet;
use std::fs;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use crate::reader::*;
use crate::rpc_support::*;

const DAY: Duration = Duration::from_secs(86_400);

fn row_ids(d: &Value) -> Vec<String> {
    arr(&d["jobs"])
        .iter()
        .map(|r| r["id"].as_str().expect("row id").to_owned())
        .collect()
}

fn row<'a>(d: &'a Value, id: &str) -> &'a Value {
    arr(&d["jobs"])
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("no row for {id}: {d}"))
}

/// §A10 jobs row: the job view plus `text_preview`, `error` and `forked`.
fn assert_row(r: &Value) {
    assert_job_view(r);
    for k in ["text_preview", "error", "forked"] {
        assert!(r.get(k).is_some(), "jobs row lacks `{k}`: {r}");
    }
}

fn jobs(rb: &RpcBox, extra: &[&str]) -> Value {
    let mut args = vec!["jobs", "--json"];
    args.extend_from_slice(extra);
    data_of(&rb.glen(&args).run(), 0)
}

fn ago(d: Duration) -> SystemTime {
    SystemTime::now() - d
}

/// A crafted job id whose uuidv7 timestamp equals `t`.
fn id_at(t: SystemTime, tail: u64) -> String {
    job_uuid(ms_since_epoch(t), tail)
}

// ---------------------------------------------------------------------------------------------
// AC-220 — `glen jobs`.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac220_without_glen_home_jobs_is_empty_and_creates_nothing() {
    let rb = RpcBox::new();
    let cases: [&[&str]; 4] = [&[], &["--active"], &["--session", A.id], &["-n", "0"]];
    for extra in cases {
        let d = jobs(&rb, extra);
        assert_eq!(d["total"], 0, "{extra:?}: {d}");
        assert_eq!(d["jobs"], json!([]), "{extra:?}: {d}");
    }
    rb.glen(["jobs", "--human"]).run().assert_exit(0);
    assert!(!rb.bx.glen_home.exists(), "jobs created GLEN_HOME");
}

#[test]
fn ac220_jobs_lists_newest_first_and_limits_after_counting() {
    let rb = RpcBox::new();
    let ids: Vec<String> = ["first", "second", "third"]
        .into_iter()
        .map(|t| rb.completed_job(t))
        .collect();
    rb.assert_quiet();
    let newest_first = vec![ids[2].clone(), ids[1].clone(), ids[0].clone()];
    let d = jobs(&rb, &[]);
    assert_eq!(row_ids(&d), newest_first, "{d}");
    assert_eq!(d["total"], 3, "{d}");
    for r in arr(&d["jobs"]) {
        assert_row(r);
        assert_eq!(r["status"], "completed", "{r}");
    }
    assert_eq!(row(&d, &ids[0])["text_preview"], "first", "{d}");
    let d = jobs(&rb, &["-n", "2"]);
    assert_eq!(row_ids(&d), newest_first[..2], "{d}");
    assert_eq!(d["total"], 3, "total is counted before the limit: {d}");
    let d = jobs(&rb, &["--limit", "1"]);
    assert_eq!(row_ids(&d), newest_first[..1], "{d}");
    let d = jobs(&rb, &["-n", "0"]);
    assert_eq!(row_ids(&d), newest_first, "{d}");
}

#[test]
fn ac220_the_default_limit_is_20() {
    let rb = RpcBox::new();
    let tpl = rb.completed_job("template");
    rb.assert_quiet();
    let mut crafted = Vec::new();
    for i in 0..21u64 {
        let t = ago(Duration::from_secs(3600 - i * 10));
        let id = id_at(t, 0x0220_0000 + i);
        rb.clone_job(
            &tpl,
            &id,
            t,
            obj(&[
                ("status", jstr("completed")),
                ("updated_at", jstr(&rfc3339(t))),
            ]),
            true,
        );
        crafted.push(id);
    }
    let d = jobs(&rb, &[]);
    assert_eq!(d["total"], 22, "{d}");
    let got = row_ids(&d);
    assert_eq!(got.len(), 20, "default -n 20: {d}");
    let mut want = vec![tpl.clone()];
    want.extend(crafted.iter().rev().take(19).cloned());
    assert_eq!(got, want, "newest first");
    assert_eq!(row_ids(&jobs(&rb, &["-n", "0"])).len(), 22);
}

#[test]
fn ac220_active_keeps_queued_or_accepted_jobs_after_the_lost_check() {
    let rb = RpcBox::new();
    let done = rb.completed_job("done");
    rb.assert_quiet();
    let t = ago(Duration::from_secs(60));
    let orphan = id_at(t, 0x0220_dead);
    rb.clone_job(
        &done,
        &orphan,
        t,
        obj(&[
            ("status", jstr("accepted")),
            ("updated_at", jstr(&rfc3339(t))),
            ("worker_pid", json!(dead_pid())),
            ("worker_start", jstr("bogus:0")),
            ("error", Value::Null),
        ]),
        false,
    );
    let running = job_id(&data_of(
        &rb.glen(["send", A.handle, "busy", "--no-wait", "--json"])
            .env("FAKE_OMP_DELAY_MS", "4000")
            .run(),
        10,
    ));
    rb.wait_status(&running, "accepted", LONG, |s| s["status"] == "accepted");
    let d = jobs(&rb, &["--active"]);
    assert_eq!(row_ids(&d), std::slice::from_ref(&running), "{d}");
    assert_eq!(d["total"], 1, "{d}");
    assert_eq!(
        rb.status(&orphan)["status"],
        "lost",
        "the lost check persisted"
    );
    let d = jobs(&rb, &[]);
    assert_eq!(row(&d, &orphan)["status"], "lost", "{d}");
    assert_eq!(row(&d, &done)["status"], "completed", "{d}");
    data_of(
        &rb.glen(["wait", &job_handle(&running), "--timeout", "20s", "--json"])
            .run(),
        0,
    );
    rb.assert_quiet();
}

#[test]
fn ac220_session_filter_matches_ids_and_hex_tails_of_original_or_actual_sessions() {
    let rb = RpcBox::new();
    let a = rb.completed_job("to a");
    let dj = job_id(&data_of(
        &rb.glen(["send", D.handle, "to d", "--wait", "20s", "--json"])
            .run(),
        0,
    ));
    let n = job_id(&data_of(
        &rb.glen(["send", "@new", "to new", "--wait", "20s", "--json"])
            .run(),
        0,
    ));
    rb.assert_quiet();
    let new_id = rb.status(&n)["actual_session_id"]
        .as_str()
        .expect("actual_session_id of the @new job")
        .to_owned();
    let new_simple: String = new_id.chars().filter(|c| *c != '-').collect();
    let a_simple: String = A.id.chars().filter(|c| *c != '-').collect();
    let cases: Vec<(String, Vec<&String>)> = vec![
        (A.id.to_owned(), vec![&a]),
        // A and F share the tail `c0de`; F has no job. (The @new id is pid-derived.)
        (
            "c0de".to_owned(),
            if new_simple.ends_with("c0de") {
                vec![&n, &a]
            } else {
                vec![&a]
            },
        ),
        (a_simple[a_simple.len() - 20..].to_owned(), vec![&a]),
        ("0de".to_owned(), vec![]),
        (D.id.to_owned(), vec![&dj]),
        (D.handle.to_owned(), vec![&dj]),
        (new_id.clone(), vec![&n]),
        (new_simple[new_simple.len() - 6..].to_owned(), vec![&n]),
        ("ffffffff".to_owned(), vec![]),
    ];
    for (sel, want) in cases {
        let d = jobs(&rb, &["--session", &sel]);
        let want: Vec<String> = want.into_iter().cloned().collect();
        assert_eq!(row_ids(&d), want, "--session {sel}: {d}");
        assert_eq!(d["total"], want.len(), "--session {sel}: {d}");
    }
}

#[test]
fn a10_jobs_rows_preview_the_first_line_without_controls() {
    let rb = RpcBox::new();
    let multi = rb.completed_job("first\u{7}line\tx\nsecond line");
    let long = rb.completed_job(&"a".repeat(200));
    rb.assert_quiet();
    let d = jobs(&rb, &[]);
    let p = row(&d, &multi)["text_preview"].as_str().unwrap().to_owned();
    assert!(p.starts_with("first"), "{p:?}");
    assert!(!p.contains("second"), "first line only: {p:?}");
    assert!(!p.chars().any(char::is_control), "controls dropped: {p:?}");
    let p = row(&d, &long)["text_preview"].as_str().unwrap().to_owned();
    assert!(p.chars().count() <= 80, "at most 80 characters: {p:?}");
    assert!(p.starts_with("aaaaaaaaaa"), "{p:?}");
}

#[test]
fn a10_human_jobs_show_the_status_glyphs() {
    let rb = RpcBox::new();
    let done = rb.completed_job("done");
    let fail = |scenario: &str| -> String {
        let out = rb
            .glen(["send", A.handle, "x", "--wait", "20s", "--json"])
            .env("FAKE_OMP_SCENARIO", scenario)
            .run();
        let err = failed(&out, "job_failed");
        err["details"]["job"]["id"].as_str().unwrap().to_owned()
    };
    let failed_id = fail("error");
    let aborted_id = fail("aborted");
    rb.assert_quiet();
    let t = ago(Duration::from_secs(30));
    let lost_id = id_at(t, 0x0a10_0105);
    rb.clone_job(
        &done,
        &lost_id,
        t,
        obj(&[
            ("status", jstr("accepted")),
            ("updated_at", jstr(&rfc3339(t))),
            ("worker_pid", json!(dead_pid())),
            ("worker_start", jstr("bogus:0")),
            ("error", Value::Null),
        ]),
        false,
    );
    let out = rb.glen(["jobs", "--human"]).run();
    out.assert_exit(0).assert_no_ansi();
    let text = out.out();
    for (id, glyph) in [
        (&done, "✓ completed"),
        (&failed_id, "✗ failed"),
        (&aborted_id, "! aborted"),
        (&lost_id, "✗ lost"),
    ] {
        let h = job_handle(id);
        let line = text
            .lines()
            .find(|l| l.contains(&h))
            .unwrap_or_else(|| panic!("no row for {h}\n{}", out.ctx()));
        assert!(line.contains(glyph), "{h} should show {glyph:?}: {line}");
    }
}

// ---------------------------------------------------------------------------------------------
// AC-214 — reader-side lost detection on crafted statuses.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac214_readers_mark_lost_only_on_identity_evidence_or_an_expired_spawn_grace() {
    let rb = RpcBox::new();
    let tpl = rb.completed_job("template");
    rb.assert_quiet();
    let me = std::process::id();
    let cases = [
        // (tail, status, updated ago, worker_pid, worker_start, expect lost)
        (
            0x214_0001u64,
            "accepted",
            30,
            Some(dead_pid()),
            Some("bogus:0"),
            true,
        ),
        (0x214_0002, "accepted", 30, Some(me), Some("bogus:0"), true),
        (0x214_0003, "queued", 11, None, None, true),
        (0x214_0004, "queued", 2, None, None, false),
    ];
    let mut ids = Vec::new();
    for (tail, status, secs, pid, start, lost) in cases {
        let t = ago(Duration::from_secs(secs));
        let id = id_at(t, tail);
        rb.clone_job(
            &tpl,
            &id,
            t,
            obj(&[
                ("status", jstr(status)),
                ("updated_at", jstr(&rfc3339(t))),
                ("worker_pid", pid.map_or(Value::Null, |p| json!(p))),
                ("worker_start", start.map_or(Value::Null, jstr)),
                ("error", Value::Null),
                ("forked", Value::Null),
            ]),
            false,
        );
        ids.push((id, status, lost));
    }
    let d = jobs(&rb, &["-n", "0"]);
    for (id, status, lost) in &ids {
        let want = if *lost { "lost" } else { status };
        assert_eq!(row(&d, id)["status"], *want, "{id}: {d}");
        let st = rb.status(id);
        assert_eq!(st["status"], *want, "persisted: {st}");
        if *lost {
            assert_eq!(st["error"]["code"], "lost", "{st}");
        }
    }
    let (fresh, _, _) = &ids[3];
    let w = data_of(
        &rb.glen(["wait", &job_handle(fresh), "--timeout", "200ms", "--json"])
            .run(),
        10,
    );
    assert_eq!(w["job"]["status"], "queued", "inside the spawn grace: {w}");
    let (gone, _, _) = &ids[0];
    let err = failed(
        &rb.glen(["wait", &job_handle(gone), "--json"]).run(),
        "job_failed",
    );
    assert_eq!(err["details"]["status"], "lost", "{err}");
}

// ---------------------------------------------------------------------------------------------
// AC-221 — job handles for `glen wait`.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac221_wait_accepts_j_suffixes_and_full_ids_case_insensitively() {
    let rb = RpcBox::new();
    let id = rb.completed_job("hello");
    rb.assert_quiet();
    let simple: String = id.chars().filter(|c| *c != '-').collect();
    let h = job_handle(&id);
    let long = format!("j{}", &simple[simple.len() - 9..]);
    for arg in [
        h.clone(),
        h.to_uppercase(),
        long,
        id.clone(),
        id.to_uppercase(),
    ] {
        let w = data_of(&rb.glen(["wait", &arg, "--json"]).run(), 0);
        assert_eq!(w["job"]["id"], id.as_str(), "wait {arg}: {w}");
    }

    let t = ago(Duration::from_secs(120));
    let x = id_at(t, 0x0000_0123_4567);
    let y = id_at(t + Duration::from_secs(1), 0x0000_1123_4567);
    for (cid, ct) in [(&x, t), (&y, t + Duration::from_secs(1))] {
        rb.clone_job(
            &id,
            cid,
            ct,
            obj(&[
                ("status", jstr("completed")),
                ("updated_at", jstr(&rfc3339(ct))),
            ]),
            true,
        );
    }
    let err = failed(&rb.glen(["wait", "j1234567", "--json"]).run(), "ambiguous");
    let c = candidates(&err, 2);
    assert!(c.contains(&x) && c.contains(&y), "{c}");
    let w = data_of(&rb.glen(["wait", "j01234567", "--json"]).run(), 0);
    assert_eq!(w["job"]["id"], x.as_str(), "{w}");
    let w = data_of(&rb.glen(["wait", "j11234567", "--json"]).run(), 0);
    assert_eq!(w["job"]["id"], y.as_str(), "{w}");

    failed(&rb.glen(["wait", "j7654321", "--json"]).run(), "not_found");
    for bad in [
        A.handle,
        "j123456",
        "jxyz1234",
        "1234567",
        "job",
        "j1234567g",
    ] {
        let err = failed(&rb.glen(["wait", bad, "--json"]).run(), "usage");
        assert!(
            err["hint"].as_str().is_some_and(|h| !h.trim().is_empty()),
            "wait {bad}: usage with a hint: {err}"
        );
    }
}

#[test]
fn ac221_wait_without_glen_home_is_not_found_and_creates_nothing() {
    let rb = RpcBox::new();
    failed(&rb.glen(["wait", "j1234567", "--json"]).run(), "not_found");
    failed(
        &rb.glen(["wait", "01a20000-0000-7000-8000-000000000221", "--json"])
            .run(),
        "not_found",
    );
    assert!(!rb.bx.glen_home.exists(), "wait created GLEN_HOME");
}

// ---------------------------------------------------------------------------------------------
// AC-223 — retention.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac223_send_prunes_only_terminal_jobs_untouched_for_more_than_7_days() {
    let rb = RpcBox::new();
    let tpl = rb.completed_job("template");
    rb.assert_quiet();
    let old = ago(8 * DAY);
    let recent = ago(6 * DAY);
    let craft = |tail: u64, t: SystemTime, status: &str, code: Option<&str>| -> String {
        let id = id_at(t, tail);
        let err = code.map_or(Value::Null, |c| json!({"code": c, "message": "crafted"}));
        rb.clone_job(
            &tpl,
            &id,
            t,
            obj(&[
                ("status", jstr(status)),
                ("updated_at", jstr(&rfc3339(t))),
                ("error", err),
                ("worker_pid", Value::Null),
                ("worker_start", Value::Null),
            ]),
            status == "completed",
        );
        id
    };
    let pruned = [
        craft(0x223_0001, old, "completed", None),
        craft(0x223_0002, old, "lost", Some("lost")),
        craft(0x223_0003, old, "failed", Some("omp_error")),
        craft(
            0x223_0004,
            old + Duration::from_secs(1),
            "aborted",
            Some("cancelled"),
        ),
    ];
    let kept_active = craft(0x223_0005, old + Duration::from_secs(2), "accepted", None);
    let kept_recent = craft(0x223_0006, recent, "failed", Some("omp_error"));
    let garbage = id_at(old + Duration::from_secs(3), 0x223_0007);
    let gdir = rb.job_dir(&garbage);
    fs::create_dir(&gdir).unwrap();
    write_private(&gdir.join("status.json"), b"{not json");
    set_mtime(&gdir.join("status.json"), old);
    let other = rb.jobs_dir().join("not-a-job");
    fs::create_dir(&other).unwrap();

    let new = rb.completed_job("trigger");
    rb.assert_quiet();
    let left: BTreeSet<String> = rb.job_ids().into_iter().collect();
    let want: BTreeSet<String> = [
        tpl,
        kept_active,
        kept_recent,
        garbage,
        new,
        "not-a-job".to_owned(),
    ]
    .into_iter()
    .collect();
    assert_eq!(left, want, "pruned: {pruned:?}");
}
