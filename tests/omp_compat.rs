//! v0.2 W1: the supported omp range in `glen doctor` (AC-v2-101, AC-v2-102), the capability
//! probe `glen doctor --probe` (AC-v2-103) and the negotiated protocol in job status
//! (AC-v2-104). Every case drives the built binary with a cleared environment; omp is the
//! shell fixture or the `fake_omp` example, never a real omp.
mod common;
use common::*;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};
fn shell_fake() -> String {
    fixture("bin/fake-omp").to_str().unwrap().into()
}
/// The doctor report: `data` on success, `error.details.report` on exit 5.
fn report(v: &Value) -> &Value {
    if v["ok"] == true {
        &v["data"]
    } else {
        &v["error"]["details"]["report"]
    }
}
fn row<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no {name} row in {report}"))
}
fn warning_codes(v: &Value) -> Vec<String> {
    v["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| w["code"].as_str().map(str::to_owned))
        .collect()
}
#[cfg(unix)]
fn script(dir: &Path, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_str().unwrap().into()
}
/// (FAKE_OMP_VERSION, row status, row value, row detail, warning, data.omp.version)
type VersionCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    Value,
);
#[test]
fn doctor_classifies_the_omp_version_against_the_declared_range() {
    let cases: &[VersionCase] = &[
        ("18.14.0", "ok", "18.14.0", "", None, json!("18.14.0")),
        (
            "18.14.0+b1",
            "ok",
            "18.14.0+b1",
            "",
            None,
            json!("18.14.0+b1"),
        ),
        ("v18.14.0", "ok", "18.14.0", "", None, json!("18.14.0")),
        (
            "18.14.1",
            "warn",
            "18.14.1",
            "untested",
            Some("omp_version_untested"),
            json!("18.14.1"),
        ),
        (
            "18.15.0-rc.1",
            "warn",
            "18.15.0-rc.1",
            "untested",
            Some("omp_version_untested"),
            json!("18.15.0-rc.1"),
        ),
        (
            "19.0.0",
            "warn",
            "19.0.0",
            "untested",
            Some("omp_version_untested"),
            json!("19.0.0"),
        ),
        (
            "18.14.0-rc.1",
            "warn",
            "18.14.0-rc.1",
            "unsupported",
            Some("omp_version_unsupported"),
            json!("18.14.0-rc.1"),
        ),
        (
            "18.13.9",
            "warn",
            "18.13.9",
            "unsupported",
            Some("omp_version_unsupported"),
            json!("18.13.9"),
        ),
        (
            "18.0.9",
            "warn",
            "18.0.9",
            "unsupported",
            Some("omp_version_unsupported"),
            json!("18.0.9"),
        ),
        (
            "17.99.0",
            "warn",
            "17.99.0",
            "unsupported",
            Some("omp_version_unsupported"),
            json!("17.99.0"),
        ),
        (
            "bad",
            "warn",
            "unprobed",
            "cannot parse omp --version output",
            Some("omp_version_untested"),
            Value::Null,
        ),
        (
            "18.14",
            "warn",
            "unprobed",
            "cannot parse omp --version output",
            Some("omp_version_untested"),
            Value::Null,
        ),
        (
            "18.14.0\u{1b}[31m",
            "warn",
            "unprobed",
            "cannot parse omp --version output",
            Some("omp_version_untested"),
            Value::Null,
        ),
    ];
    for (version, status, value, detail, warning, normalized) in cases {
        let o = glen(
            &["doctor", "--json"],
            &[
                ("GLEN_OMP_BIN", &shell_fake()),
                ("FAKE_OMP_VERSION", version),
            ],
        );
        assert_eq!(o.status.code(), Some(0), "{version:?}: {o:?}");
        let v = json(&o);
        let r = report(&v);
        let check = row(r, "omp_version");
        assert_eq!(check["status"], *status, "{version:?}");
        assert_eq!(check["value"], *value, "{version:?}");
        assert_eq!(check["detail"], *detail, "{version:?}");
        let codes = warning_codes(&v);
        assert_eq!(
            codes
                .iter()
                .filter(|c| c.starts_with("omp_version_"))
                .collect::<Vec<_>>(),
            warning.iter().collect::<Vec<_>>(),
            "{version:?}"
        );
        assert_eq!(r["omp"]["version"], *normalized, "{version:?}");
        assert_eq!(
            r["omp"]["supported"],
            json!({"min":"18.14.0","max_tested":"18.14.0"})
        );
        assert_eq!(r["omp"]["tested_version"], "18.14.0");
        assert_eq!(r["omp"]["capabilities"], Value::Null);
        assert!(!o.stdout.contains(&27), "{version:?}: no ESC in JSON");
    }
}
#[test]
fn unparseable_version_never_echoes_escape_bytes_in_human_mode() {
    let o = glen(
        &["doctor", "--human"],
        &[
            ("GLEN_OMP_BIN", &shell_fake()),
            ("FAKE_OMP_VERSION", "18.14.0\u{1b}[31mred"),
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert!(!o.stdout.contains(&27), "{o:?}");
    assert!(!o.stderr.contains(&27), "{o:?}");
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(stdout.contains("unprobed"), "{stdout}");
    assert!(
        stdout.contains("tested against omp 18.14.0 · supported 18.14.0 to 18.14.0"),
        "{stdout}"
    );
}
#[cfg(unix)]
#[test]
fn silent_version_is_unprobed_and_a_failed_run_stays_omp_unavailable() {
    let t = tempfile::tempdir().unwrap();
    let silent = script(t.path(), "silent", "exit 0");
    let o = glen(&["doctor", "--json"], &[("GLEN_OMP_BIN", &silent)]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let v = json(&o);
    assert_eq!(row(report(&v), "omp_version")["value"], "unprobed");
    assert_eq!(report(&v)["omp"]["version"], Value::Null);
    assert!(warning_codes(&v).contains(&"omp_version_untested".into()));
    let failing = script(t.path(), "failing", "echo omp/18.14.0\nexit 3");
    let o = glen(&["doctor", "--json"], &[("GLEN_OMP_BIN", &failing)]);
    assert_eq!(o.status.code(), Some(5), "{o:?}");
    let v = json(&o);
    assert_eq!(v["error"]["code"], "omp_unavailable");
    assert_eq!(v["error"]["hint"], "install omp or set GLEN_OMP_BIN");
    assert_eq!(row(report(&v), "omp_version")["status"], "fail");
    assert_eq!(report(&v)["omp"]["version"], Value::Null);
}
#[test]
fn capabilities_row_is_index_two_and_not_probed_by_default() {
    let o = glen(&["doctor", "--json"], &[("GLEN_OMP_BIN", &shell_fake())]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let v = json(&o);
    let check = &v["data"]["checks"][2];
    assert_eq!(check["name"], "omp_capabilities");
    assert_eq!(check["status"], "info");
    assert_eq!(check["value"], "not probed");
    assert_eq!(
        check["detail"],
        "run glen doctor --probe to check omp's RPC capabilities"
    );
}
/// A scratch HOME, GLEN_HOME (never created), cwd and fake log for one `doctor --probe` run.
struct ProbeBox {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    glen_home: PathBuf,
    cwd: PathBuf,
    log: PathBuf,
}
impl ProbeBox {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let (home, cwd) = (root.join("home"), root.join("cwd"));
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(&cwd).unwrap();
        Self {
            glen_home: root.join("glen"),
            log: root.join("fake.ndjson"),
            _tmp: tmp,
            home,
            cwd,
        }
    }
    fn command(&self, args: &[&str], vars: &[(&str, &str)]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_glen"));
        c.args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("GLEN_HOME", &self.glen_home)
            .env("PATH", "")
            .env("FAKE_OMP_LOG", &self.log)
            .envs(vars.iter().copied())
            .current_dir(&self.cwd);
        c
    }
    /// `doctor --probe --json` against `fake_omp`; returns the envelope and the exit code.
    fn probe(&self, vars: &[(&str, &str)]) -> (Value, i32) {
        let fake = fake_omp_bin();
        let mut all = vec![("GLEN_OMP_BIN", fake.to_str().unwrap())];
        all.extend_from_slice(vars);
        let o = self
            .command(&["doctor", "--probe", "--json"], &all)
            .output()
            .unwrap();
        assert!(o.stderr.is_empty(), "{o:?}");
        let code = o.status.code().unwrap();
        self.assert_fake_gone();
        (json(&o), code)
    }
    fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
    /// Every fake that started has no process group left (within 3 s).
    fn assert_fake_gone(&self) {
        for start in self.log().iter().filter(|e| e["event"] == "start") {
            let pgid = start["pgid"].as_i64().unwrap() as i32;
            poll(Duration::from_secs(3), || {
                let group = rustix::process::Pid::from_raw(pgid).unwrap();
                (rustix::process::test_kill_process_group(group) == Err(rustix::io::Errno::SRCH))
                    .then_some(())
            });
        }
    }
}
fn capabilities(v: &Value) -> &Value {
    &report(v)["omp"]["capabilities"]
}
#[test]
fn probe_reports_the_protocol_and_both_capabilities() {
    for (vars, protocol) in [
        (vec![], 1),
        (vec![("FAKE_OMP_V2", "1")], 2),
        (vec![("FAKE_OMP_V2", "partial")], 1),
    ] {
        let b = ProbeBox::new();
        let (v, code) = b.probe(&vars);
        assert_eq!(code, 0, "{vars:?}: {v}");
        assert_eq!(
            *capabilities(&v),
            json!({"protocol":protocol,"host_tools":"ok","event_filter":"ok"}),
            "{vars:?}"
        );
        let check = &v["data"]["checks"][2];
        assert_eq!(check["name"], "omp_capabilities");
        assert_eq!(check["status"], "ok", "{vars:?}");
        assert_eq!(
            check["value"],
            format!("protocol={protocol} host_tools=ok event_filter=ok")
        );
        assert_eq!(check["detail"], "");
    }
}
#[test]
fn probe_has_no_side_effects() {
    let b = ProbeBox::new();
    // `wedge` ignores TERM, so only a stdin EOF before teardown lets it log its exit.
    let (v, code) = b.probe(&[("FAKE_OMP_V2", "1"), ("FAKE_OMP_SCENARIO", "wedge")]);
    assert_eq!(code, 0, "{v}");
    assert!(!b.glen_home.exists(), "the probe never creates GLEN_HOME");
    let in_cwd: Vec<_> = std::fs::read_dir(&b.cwd)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(in_cwd.is_empty(), "no session file in the cwd: {in_cwd:?}");
    let log = b.log();
    let starts: Vec<_> = log.iter().filter(|e| e["event"] == "start").collect();
    assert_eq!(starts.len(), 1, "{log:?}");
    let argv: Vec<&str> = starts[0]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(argv, ["--mode", "rpc", "--no-ui", "--no-session"]);
    assert_eq!(starts[0]["env"]["GLEN_MANAGED"], "1");
    assert_eq!(starts[0]["cwd"], json!(b.cwd));
    let frames: Vec<&str> = log
        .iter()
        .filter(|e| e["event"] == "in")
        .map(|e| e["frame"]["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        frames,
        ["negotiate_protocol", "set_host_tools", "set_event_filter"]
    );
    // The probe closes its client before teardown, so omp exits on stdin EOF, not a signal.
    assert!(
        log.iter()
            .any(|e| e["event"] == "exit" && e["reason"] == "stdin eof"),
        "{log:?}"
    );
}
#[test]
fn doctor_without_probe_never_starts_omp_in_rpc_mode() {
    let b = ProbeBox::new();
    let fake = fake_omp_bin();
    let o = b
        .command(
            &["doctor", "--json"],
            &[("GLEN_OMP_BIN", fake.to_str().unwrap())],
        )
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert!(!b.log.exists(), "only --version ran, which logs nothing");
    assert_eq!(json(&o)["data"]["omp"]["capabilities"], Value::Null);
}
#[test]
fn rejected_host_tools_fail_and_a_rejected_filter_warns() {
    let b = ProbeBox::new();
    let (v, code) = b.probe(&[("FAKE_OMP_SCENARIO", "reject_tools")]);
    assert_eq!(code, 5, "{v}");
    assert_eq!(v["error"]["code"], "omp_unavailable");
    assert_eq!(v["error"]["message"], "omp is not available");
    assert_eq!(
        v["error"]["hint"],
        "omp lacks a capability glen needs: see the omp_capabilities row"
    );
    assert_eq!(capabilities(&v)["host_tools"], "missing");
    let check = row(report(&v), "omp_capabilities");
    assert_eq!(check["status"], "fail");
    assert!(
        check["detail"]
            .as_str()
            .unwrap()
            .contains("conflicts with an existing tool"),
        "{check}"
    );
    let b = ProbeBox::new();
    let (v, code) = b.probe(&[("FAKE_OMP_SCENARIO", "reject_filter")]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        *capabilities(&v),
        json!({"protocol":1,"host_tools":"ok","event_filter":"missing"})
    );
    let check = row(report(&v), "omp_capabilities");
    assert_eq!(check["status"], "warn");
    assert!(
        check["detail"]
            .as_str()
            .unwrap()
            .contains("fake filter rejected"),
        "{check}"
    );
}
#[test]
fn omp_that_exits_before_ready_is_a_warning_with_its_stderr() {
    let b = ProbeBox::new();
    let (v, code) = b.probe(&[
        ("FAKE_OMP_SCENARIO", "startup_exit"),
        ("FAKE_OMP_STARTUP_STDERR", "No default model selected."),
    ]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        *capabilities(&v),
        json!({"protocol":null,"host_tools":"unknown","event_filter":"unknown"})
    );
    let check = row(report(&v), "omp_capabilities");
    assert_eq!(check["status"], "warn");
    assert_eq!(
        check["value"],
        "protocol=unknown host_tools=unknown event_filter=unknown"
    );
    assert_eq!(
        check["detail"],
        "omp exited before ready: No default model selected."
    );
}
#[cfg(unix)]
#[test]
fn probe_is_skipped_without_a_runnable_omp() {
    let b = ProbeBox::new();
    let o = b
        .command(&["doctor", "--probe", "--json"], &[])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(5), "{o:?}");
    let v = json(&o);
    assert_eq!(v["error"]["hint"], "install omp or set GLEN_OMP_BIN");
    let check = row(report(&v), "omp_capabilities");
    assert_eq!(check["status"], "info");
    assert_eq!(check["value"], "skipped");
    assert_eq!(check["detail"], "omp binary is missing");
    let failing = script(&b.cwd, "failing", "exit 1");
    let o = b
        .command(
            &["doctor", "--probe", "--json"],
            &[("GLEN_OMP_BIN", &failing)],
        )
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(5), "{o:?}");
    let v = json(&o);
    let check = row(report(&v), "omp_capabilities");
    assert_eq!(
        (&check["status"], &check["value"]),
        (&json!("info"), &json!("skipped"))
    );
    assert_eq!(check["detail"], "omp --version failed");
    assert_eq!(capabilities(&v), &Value::Null);
}
/// The budget bounds startup and every call; teardown after it is at most a few seconds.
#[cfg(debug_assertions)]
#[test]
fn a_stalled_call_returns_within_the_budget() {
    let b = ProbeBox::new();
    let start = Instant::now();
    let (v, code) = b.probe(&[
        ("FAKE_OMP_V2", "1"),
        ("FAKE_OMP_STALL", "set_host_tools"),
        ("GLEN_PROBE_BUDGET_MS", "1500"),
    ]);
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(5000), "{elapsed:?}");
    assert_eq!(code, 0, "{v}");
    assert_eq!(capabilities(&v)["protocol"], 2);
    assert_eq!(capabilities(&v)["host_tools"], "unknown");
    assert_eq!(row(report(&v), "omp_capabilities")["status"], "warn");
}
#[cfg(debug_assertions)]
#[test]
fn a_late_ready_returns_within_the_budget_and_tears_omp_down() {
    let b = ProbeBox::new();
    let start = Instant::now();
    let (v, code) = b.probe(&[
        ("FAKE_OMP_READY_DELAY_MS", "20000"),
        ("GLEN_PROBE_BUDGET_MS", "1500"),
    ]);
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(5000), "{elapsed:?}");
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        *capabilities(&v),
        json!({"protocol":null,"host_tools":"unknown","event_filter":"unknown"})
    );
    let check = row(report(&v), "omp_capabilities");
    assert_eq!(check["status"], "warn");
    // The worker's clamped ready wait ends at the same instant and may add its own note.
    let detail = check["detail"].as_str().unwrap();
    assert!(
        detail.starts_with("probe exceeded the 1.5s budget"),
        "{detail}"
    );
    // ProbeBox::probe already waited for the fake's process group to disappear.
    assert_eq!(b.log().iter().filter(|e| e["event"] == "start").count(), 1);
}
/// omp runs in its own process group, so a terminal Ctrl-C reaches only glen: glen tears the
/// group down, then SIGINT's default disposition ends it (doctor has no interrupted document).
#[cfg(debug_assertions)]
#[test]
fn sigint_during_the_probe_tears_omp_down_before_glen_exits() {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    let b = ProbeBox::new();
    let fake = fake_omp_bin();
    let mut command = b.command(
        &["doctor", "--probe", "--json"],
        &[
            ("GLEN_OMP_BIN", fake.to_str().unwrap()),
            // Stalled at startup: the fake neither sends ready nor reads stdin EOF.
            ("FAKE_OMP_READY_DELAY_MS", "20000"),
        ],
    );
    // A terminal's Ctrl-C meets SIGINT's default disposition, but a suite run in the background
    // (suite-lock.sh) would hand glen an ignored SIGINT.
    unsafe {
        command.pre_exec(|| {
            if libc::signal(libc::SIGINT, libc::SIG_DFL) == libc::SIG_ERR {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut glen = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    poll(Duration::from_secs(10), || {
        b.log().iter().any(|e| e["event"] == "start").then_some(())
    });
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(glen.id() as i32).unwrap(),
        rustix::process::Signal::INT,
    )
    .unwrap();
    let start = Instant::now();
    let status = poll(Duration::from_secs(10), || glen.try_wait().unwrap());
    let elapsed = start.elapsed();
    assert_eq!(status.signal(), Some(libc::SIGINT), "{status:?}");
    // Poll, 100 ms grace, 1 s after SIGTERM and the stderr join: well inside 4 s.
    assert!(elapsed < Duration::from_millis(4000), "{elapsed:?}");
    // glen has exited, so the group must already be gone; this waits at most 3 s more.
    b.assert_fake_gone();
}
#[test]
fn status_records_the_negotiated_protocol() {
    for (v2, protocol) in [(None, 1), (Some("1"), 2)] {
        let b = RpcBox::new();
        let mut c = b.glen(&["send", "e27c0de", "hi", "--json"]);
        if let Some(v2) = v2 {
            c.env("FAKE_OMP_V2", v2);
        }
        let o: Output = c.output().unwrap();
        assert_eq!(o.status.code(), Some(0), "{o:?}");
        let id = json(&o)["data"]["job"]["id"].as_str().unwrap().to_owned();
        b.wait_worker_gone(&id);
        let status = b.job(&id).1;
        assert_eq!(status["rpc_protocol"], protocol, "{status}");
    }
}
