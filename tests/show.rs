mod common;
use common::{OmpBox, data, json};
use serde_json::json as j;
#[test]
fn ac110_115_active_transcript() {
    let sb = OmpBox::new();
    let d = data(&sb.glen(&["show", "e27c0de", "--json"], &sb.alpha()));
    assert_eq!(
        d["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "thinking_level",
            "model_change",
            "user",
            "assistant",
            "assistant"
        ]
    );
    assert!(d["items"][3].get("thinking").is_none());
    assert!(d["items"][3]["tool_calls"][0]["result"].is_object());
    assert_eq!(
        data(&sb.glen(&["show", "e27c0de", "--thinking"], &sb.alpha()))["items"][3]["thinking"],
        "Check the fixture loader."
    );
    assert!(data(&sb.glen(&["show","e27c0de","--tools","full"],&sb.alpha()))["items"][3]["tool_calls"][0]["arguments"].is_object());
    assert!(
        data(&sb.glen(&["show", "e27c0de", "--tools", "none"], &sb.alpha()))["items"][3]
            .get("tool_calls")
            .is_none()
    );
    let b = data(&sb.glen(&["show", "f13a5b6"], &sb.alpha()));
    assert!(b["session"]["id"].as_str().unwrap().ends_with("b7e2d81"));
    assert_eq!(
        b["continued_from"],
        j!(["01a0fd05-a240-7c41-9d02-7d2e8f13a5b6"])
    );
    for h in ["e27c0de", "b8e4a07"] {
        let d = data(&sb.glen(&["show", h], &sb.alpha()));
        assert_eq!(d["session"]["handle"], h);
        assert_eq!(d["continued_from"], j!([]));
    }
    let d = data(&sb.glen(&["show", "c4f9a72"], &sb.alpha()));
    assert_eq!(
        d["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["40000001", "40000002", "40000005", "40000006", "40000007"]
    );
    let d = data(&sb.glen(&["show", "c4f9a72", "--raw"], &sb.alpha()));
    assert!(d.get("items").is_none());
    assert_eq!(
        d["entries"].as_array().unwrap().last().unwrap()["id"],
        "40000008"
    );
    let d = data(&sb.glen(&["show", "0a3f417"], &sb.alpha()));
    assert_eq!(d["skipped_lines"], 2);
    assert_eq!(d["items"].as_array().unwrap().len(), 10);
    let raw = data(&sb.glen(&["show", "0a3f417", "--raw"], &sb.alpha()));
    assert!(
        raw["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "future_kind")
    );
    sb.clean();
}
#[test]
fn ac110_resolution_errors_and_paths() {
    let sb = OmpBox::new();
    for (arg, code, kind) in [
        ("c0de", 4, "ambiguous"),
        ("01a0f6b", 3, "not_found"),
        ("j1234567", 2, "usage"),
        ("@new", 2, "usage"),
    ] {
        let o = sb.glen(&["show", arg], &sb.alpha());
        assert_eq!(o.status.code(), Some(code));
        let e = json(&o);
        assert_eq!(e["error"]["code"], kind);
        if code == 4 {
            assert_eq!(
                e["error"]["details"]["candidates"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            )
        }
    }
    assert_eq!(
        data(&sb.glen(&["show", "@latest"], &sb.alpha()))["session"]["handle"],
        "b8e4a07"
    );
    assert_eq!(
        data(&sb.glen(&["show", sb.session('E').to_str().unwrap()], &sb.alpha()))["skipped_lines"],
        2
    );
    let o = sb.glen(
        &["show", common::fixture("README.md").to_str().unwrap()],
        &sb.alpha(),
    );
    assert_eq!(o.status.code(), Some(3));
    let o = sb.glen(&["show", "c0de", "--human"], &sb.alpha());
    let s = String::from_utf8(o.stdout).unwrap();
    assert!(s.contains("e27c0de") && s.contains("a81c0de"));
    sb.clean();
}
#[test]
fn ac120_spill() {
    let sb = OmpBox::new();
    let o = sb
        .command(&["show", "0a3f417", "--json"], &sb.alpha())
        .env("GLEN_MAX_OUTPUT", "200")
        .output()
        .unwrap();
    assert!(o.status.success());
    let v = json(&o);
    let path = std::path::Path::new(v["spilled"]["path"].as_str().unwrap());
    assert!(path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let full: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(full["data"]["items"].as_array().unwrap().len(), 10);
    let o = sb
        .command(&["show", "0a3f417", "--human"], &sb.alpha())
        .env("GLEN_MAX_OUTPUT", "200")
        .output()
        .unwrap();
    assert!(
        String::from_utf8(o.stdout)
            .unwrap()
            .contains("full output in")
    );
    assert!(!sb.home.join(".omp").exists());
}
#[test]
fn ac121_human() {
    let sb = OmpBox::new();
    let o = sb
        .command(&["show", "e27c0de", "--human"], &sb.alpha())
        .env("COLUMNS", "80")
        .output()
        .unwrap();
    assert!(o.status.success());
    let s = String::from_utf8(o.stdout).unwrap();
    for part in [
        "❯ user · 09:00",
        "◆ assistant",
        "↳ read tests/parser_test.rs",
        "∴ thinking",
    ] {
        assert!(s.contains(part), "{s}")
    }
    for l in s.lines() {
        assert!(unicode_width::UnicodeWidthStr::width(l) <= 80, "{l}")
    }
    sb.clean();
}

#[test]
fn cwd_scopes_handles_ids_prefixes_and_every_listed_handle() {
    let sb = OmpBox::new();
    let alpha = sb.alpha();
    let beta = sb.beta();
    for arg in ["e27c0de", common::A_ID, "01a0f6b1"] {
        let good = sb.glen(
            &["show", arg, "--cwd", alpha.to_str().unwrap(), "--json"],
            &beta,
        );
        assert_eq!(data(&good)["session"]["id"], common::A_ID);
        let outside = sb.glen(
            &["show", arg, "--cwd", beta.to_str().unwrap(), "--json"],
            &alpha,
        );
        assert_eq!(outside.status.code(), Some(3), "{outside:?}");
        let error = &json(&outside)["error"];
        assert_eq!(error["code"], "not_found");
        assert_eq!(error["details"]["reason"], "outside_cwd");
        assert_eq!(error["details"]["cwd"], beta.to_str().unwrap());
        assert!(error["hint"].as_str().unwrap().contains("alpha"));
        assert!(outside.stderr.is_empty());
    }
    let listed = data(&sb.glen(
        &["ls", "--cwd", alpha.to_str().unwrap(), "-n", "0", "--json"],
        &beta,
    ));
    let rows = listed["sessions"].as_array().unwrap();
    assert!(!rows.is_empty());
    for row in rows {
        let out = sb.glen(
            &[
                "show",
                row["handle"].as_str().unwrap(),
                "--cwd",
                alpha.to_str().unwrap(),
                "--json",
            ],
            &beta,
        );
        assert!(out.status.success(), "{out:?}");
    }
    sb.clean();
}

#[test]
fn cwd_makes_a_globally_ambiguous_suffix_unique_and_limits_candidates() {
    use std::fs;
    let sb = OmpBox::new();
    let a = "a0000000-0000-7000-8000-00001abcdef0";
    let b = "b0000000-0000-7000-8000-00002abcdef0";
    let c = "c0000000-0000-7000-8000-00003abcdef0";
    for (id, bucket, cwd) in [(a, "-work-alpha", sb.alpha()), (b, "-work-beta", sb.beta())] {
        fs::write(
            sb.agent
                .join("sessions")
                .join(bucket)
                .join(format!("{id}.jsonl")),
            format!("{}\n", j!({"type":"session","version":3,"id":id,"cwd":cwd})),
        )
        .unwrap();
    }
    assert_eq!(
        sb.glen(&["show", "abcdef0", "--json"], &sb.alpha())
            .status
            .code(),
        Some(4)
    );
    for (dir, id) in [(sb.alpha(), a), (sb.beta(), b)] {
        let out = sb.glen(
            &["show", "abcdef0", "--cwd", dir.to_str().unwrap(), "--json"],
            &sb.alpha(),
        );
        assert_eq!(data(&out)["session"]["id"], id);
    }
    fs::write(
        sb.agent
            .join("sessions/-work-alpha")
            .join(format!("{c}.jsonl")),
        format!(
            "{}\n",
            j!({"type":"session","version":3,"id":c,"cwd":sb.alpha()})
        ),
    )
    .unwrap();
    let out = sb.glen(
        &[
            "show",
            "abcdef0",
            "--cwd",
            sb.alpha().to_str().unwrap(),
            "--json",
        ],
        &sb.beta(),
    );
    assert_eq!(out.status.code(), Some(4));
    let value = json(&out);
    let candidates = value["error"]["details"]["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate["id"] == a || candidate["id"] == c)
    );
    sb.clean();
}
