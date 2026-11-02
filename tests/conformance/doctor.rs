//! `glen doctor` and omp discovery (AC-014, AC-015, AC-016; §A10 doctor data, §A12 omp
//! executable).

use std::fs;
use std::os::unix::fs::symlink;

use crate::support::*;
use serde_json::Value;

#[test]
fn ac014_doctor_reports_omp_paths_and_glen_home() {
    let sb = Sandbox::new();
    let out = sb.glen(["doctor"]).fake_omp().run();
    let data = out.ok_data();
    out.assert_stderr_empty();
    let env = out.json();
    assert!(env.get("warnings").is_none(), "{}", out.ctx());

    assert_eq!(data["glen"]["version"], Value::from(PKG_VERSION));
    assert_eq!(data["glen"]["home"], Value::from(s(&sb.glen_home)));
    assert_eq!(data["glen"]["home_exists"], Value::Bool(false));

    assert_eq!(data["omp"]["bin"], Value::from(s(&fake_omp())));
    assert_eq!(data["omp"]["version"], Value::from(TESTED_OMP));
    assert_eq!(data["omp"]["tested_version"], Value::from(TESTED_OMP));

    let home = sb.home.join(".omp");
    assert_eq!(
        data["paths"]["sessions_dir"],
        Value::from(s(&home.join("agent/sessions")))
    );

    let names: Vec<&str> = data["checks"]
        .as_array()
        .expect("checks array")
        .iter()
        .map(|c| c["name"].as_str().expect("check name"))
        .collect();
    assert_eq!(names, DOCTOR_CHECKS, "{}", out.ctx());
    for c in data["checks"].as_array().unwrap() {
        let status = c["status"].as_str().expect("check status");
        assert!(["ok", "warn", "fail", "info"].contains(&status), "{c}");
        assert!(c.get("detail").is_some(), "check has a detail: {c}");
    }
    assert_eq!(check(&data, "omp_binary")["status"], "ok");
    assert_eq!(check(&data, "omp_version")["status"], "ok");
    assert_eq!(check(&data, "profile")["status"], "info");
}

#[test]
fn doctor_checks_reflect_missing_and_present_directories() {
    let sb = Sandbox::new();
    let data = sb.glen(["doctor"]).fake_omp().run().ok_data();
    assert_eq!(check(&data, "agent_dir")["status"], "warn");
    assert_eq!(check(&data, "sessions_dir")["status"], "warn");
    assert_eq!(check(&data, "glen_home")["status"], "info");

    let out = sb
        .glen(["doctor"])
        .fake_omp()
        .env("PI_CODING_AGENT_DIR", fixture_agent_dir())
        .run();
    let data = out.ok_data();
    assert_eq!(
        data["paths"]["sessions_dir"],
        Value::from(s(&fixture_agent_dir().join("sessions"))),
        "{}",
        out.ctx()
    );
    assert_eq!(check(&data, "agent_dir")["status"], "ok", "{}", out.ctx());
    assert_eq!(
        check(&data, "sessions_dir")["status"],
        "ok",
        "{}",
        out.ctx()
    );
}

#[test]
fn doctor_reports_glen_home_privacy() {
    let sb = Sandbox::new();
    fs::create_dir(&sb.glen_home).unwrap();
    set_mode(&sb.glen_home, 0o700);
    let data = sb.glen(["doctor"]).fake_omp().run().ok_data();
    assert_eq!(data["glen"]["home_exists"], Value::Bool(true));
    assert_eq!(check(&data, "glen_home")["status"], "ok");

    set_mode(&sb.glen_home, 0o755);
    let data = sb.glen(["doctor"]).fake_omp().run().ok_data();
    assert_eq!(check(&data, "glen_home")["status"], "warn");
    // AC-019: glen never chmods an existing GLEN_HOME.
    assert_eq!(mode(&sb.glen_home), 0o755);
}

#[test]
fn ac015_untested_omp_version_warns_but_succeeds() {
    let sb = Sandbox::new();
    let out = sb
        .glen(["doctor"])
        .fake_omp()
        .env("FAKE_OMP_VERSION", "18.99.0")
        .run();
    let env = out.ok_envelope();
    out.assert_stderr_empty();
    let codes: Vec<&str> = env["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("warnings array expected\n{}", out.ctx()))
        .iter()
        .filter_map(|w| w["code"].as_str())
        .collect();
    assert!(codes.contains(&"omp_version_untested"), "{}", out.ctx());
    assert_eq!(env["data"]["omp"]["version"], Value::from("18.99.0"));
    assert_eq!(
        env["data"]["omp"]["tested_version"],
        Value::from(TESTED_OMP)
    );
    assert_eq!(check(&env["data"], "omp_version")["status"], "warn");

    let human = sb
        .glen(["doctor", "--human"])
        .fake_omp()
        .env("FAKE_OMP_VERSION", "18.99.0")
        .run();
    human.assert_exit(0);
    assert!(human.err().contains("warning"), "{}", human.ctx());
    assert!(!human.stdout.is_empty(), "{}", human.ctx());
}

fn assert_unavailable(out: &Out) -> Value {
    let err = out.error_envelope("omp_unavailable");
    out.assert_stderr_empty();
    let report = &err["details"]["report"];
    assert!(
        report["checks"].is_array(),
        "error.details.report.checks expected\n{}",
        out.ctx()
    );
    assert_eq!(report["omp"]["tested_version"], Value::from(TESTED_OMP));
    report.clone()
}

#[test]
fn ac016_missing_omp_binary_is_omp_unavailable() {
    let sb = Sandbox::new();
    let dir = sb.dir("somedir");
    let runs = [
        sb.glen(["doctor"]),
        sb.glen(["doctor"]).env("GLEN_OMP_BIN", "/nonexistent/omp"),
        sb.glen(["doctor"]).env("GLEN_OMP_BIN", &dir),
        sb.glen(["doctor"])
            .env("GLEN_OMP_BIN", "no-such-omp-binary"),
    ];
    for run in runs {
        let out = run.run();
        let report = assert_unavailable(&out);
        assert_eq!(
            check(&report, "omp_binary")["status"],
            "fail",
            "{}",
            out.ctx()
        );
        assert_eq!(report["omp"]["bin"], Value::Null, "{}", out.ctx());
    }
}

#[test]
fn ac016_failing_omp_version_is_omp_unavailable() {
    let sb = Sandbox::new();
    let tools = sb.dir("tools");
    let scripts = [
        write_script(&tools, "exit-1", "exit 1"),
        write_script(&tools, "fails-after-print", "echo omp/18.14.0\nexit 3"),
    ];
    for script in scripts {
        let out = sb.glen(["doctor"]).env("GLEN_OMP_BIN", &script).run();
        let report = assert_unavailable(&out);
        assert_eq!(
            check(&report, "omp_binary")["status"],
            "ok",
            "{}",
            out.ctx()
        );
        assert_eq!(
            check(&report, "omp_version")["status"],
            "fail",
            "{}",
            out.ctx()
        );
        assert_eq!(
            report["omp"]["bin"],
            Value::from(s(&script)),
            "{}",
            out.ctx()
        );
        assert_eq!(report["omp"]["version"], Value::Null, "{}", out.ctx());
    }
}

/// v0.2 W1 (AC-v2-102) supersedes AC-016 for output that runs but cannot be parsed: it is the
/// warning `omp_version_untested` with the row value "unprobed", not exit 5 (§A11, §A12).
/// A `--version` that cannot run stays `omp_unavailable` (above).
#[test]
fn ac016_unparseable_omp_version_is_a_warning_since_v0_2_w1() {
    let sb = Sandbox::new();
    let tools = sb.dir("tools");
    let scripts = [
        write_script(&tools, "garbage", "echo garbage"),
        write_script(&tools, "two-part", "echo omp/18.14"),
        write_script(&tools, "silent", "echo omp/18.14.0 >&2"),
    ];
    for script in scripts {
        let out = sb.glen(["doctor"]).env("GLEN_OMP_BIN", &script).run();
        let env = out.ok_envelope();
        assert_eq!(env["data"]["omp"]["version"], Value::Null, "{}", out.ctx());
        let row = check(&env["data"], "omp_version");
        assert_eq!(row["status"], "warn", "{}", out.ctx());
        assert_eq!(row["value"], "unprobed", "{}", out.ctx());
        let codes: Vec<&str> = env["warnings"]
            .as_array()
            .unwrap_or_else(|| panic!("warnings array expected\n{}", out.ctx()))
            .iter()
            .filter_map(|w| w["code"].as_str())
            .collect();
        assert!(codes.contains(&"omp_version_untested"), "{}", out.ctx());
    }
}

#[test]
fn a12_version_parsing_accepts_bare_and_prefixed_forms() {
    let sb = Sandbox::new();
    let tools = sb.dir("tools");
    for (name, body) in [
        ("bare", "echo 18.14.0"),
        ("multiline", "echo omp/18.14.0\necho second line"),
    ] {
        let script = write_script(&tools, name, body);
        let out = sb.glen(["doctor"]).env("GLEN_OMP_BIN", &script).run();
        let data = out.ok_data();
        assert_eq!(
            data["omp"]["version"],
            Value::from(TESTED_OMP),
            "{}",
            out.ctx()
        );
    }
}

#[test]
fn a12_hanging_omp_version_times_out() {
    let sb = Sandbox::new();
    let tools = sb.dir("tools");
    let hang = write_script(&tools, "hang", "exec /bin/sleep 40");
    let out = sb.glen(["doctor"]).env("GLEN_OMP_BIN", &hang).run();
    assert_unavailable(&out);
    assert!(
        out.elapsed.as_secs() < 30,
        "doctor should give up after ~10 s, took {:?}",
        out.elapsed
    );
}

#[test]
fn a12_omp_is_found_on_path() {
    let sb = Sandbox::new();
    let noexec = sb.dir("noexec");
    fs::write(noexec.join("omp"), "#!/bin/sh\necho omp/1.0.0\n").unwrap();
    set_mode(&noexec.join("omp"), 0o644);
    let bin = sb.dir("bin");
    symlink(fake_omp(), bin.join("omp")).unwrap();
    let path = format!("{}:{}:{}", s(&sb.empty_bin), s(&noexec), s(&bin));

    let out = sb.glen(["doctor"]).env("PATH", &path).run();
    let data = out.ok_data();
    assert_eq!(
        data["omp"]["version"],
        Value::from(TESTED_OMP),
        "{}",
        out.ctx()
    );
    let found = data["omp"]["bin"].as_str().expect("omp.bin is a path");
    assert_eq!(
        fs::canonicalize(found).unwrap(),
        fs::canonicalize(fake_omp()).unwrap(),
        "{}",
        out.ctx()
    );

    // An empty GLEN_OMP_BIN counts as unset.
    sb.glen(["doctor"])
        .env("PATH", &path)
        .env("GLEN_OMP_BIN", "")
        .run()
        .ok_data();

    // A bare GLEN_OMP_BIN name is looked up on PATH.
    let out = sb
        .glen(["doctor"])
        .env("PATH", s(&fixtures().join("bin")))
        .env("GLEN_OMP_BIN", "fake-omp")
        .run();
    let data = out.ok_data();
    let found = data["omp"]["bin"].as_str().expect("omp.bin is a path");
    assert_eq!(
        fs::canonicalize(found).unwrap(),
        fs::canonicalize(fake_omp()).unwrap()
    );

    // GLEN_OMP_BIN overrides PATH; it does not fall back to it.
    let out = sb
        .glen(["doctor"])
        .env("PATH", &path)
        .env("GLEN_OMP_BIN", "/nonexistent/omp")
        .run();
    assert_unavailable(&out);
}

#[test]
fn a10_failed_doctor_prints_its_table_then_the_error_in_human_mode() {
    let sb = Sandbox::new();
    let out = sb.glen(["doctor", "--human"]).run();
    out.assert_exit(exit_for("omp_unavailable"))
        .assert_no_ansi();
    assert!(
        out.out().contains("doctor"),
        "the report goes to stdout\n{}",
        out.ctx()
    );
    let err = out.err();
    assert!(err.starts_with("error:"), "{}", out.ctx());
    assert!(err.contains("hint:"), "{}", out.ctx());
}
