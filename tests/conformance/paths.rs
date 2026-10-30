//! omp path resolution and GLEN_HOME resolution, observed through `glen doctor`'s JSON
//! (AC-011, AC-012, AC-013, AC-020; §A6, §A12).

use std::fs;
use std::path::{Path, PathBuf};

use crate::support::*;
use serde_json::Value;

struct Paths(Value);

impl Paths {
    fn get(&self, key: &str) -> String {
        self.0[key]
            .as_str()
            .unwrap_or_else(|| panic!("paths.{key} is not a string: {}", self.0))
            .to_owned()
    }

    fn assert(&self, key: &str, expected: &Path) {
        assert_eq!(self.get(key), s(expected), "paths.{key} in {}", self.0);
    }

    fn assert_str(&self, key: &str, expected: &str) {
        assert_eq!(
            self.0[key],
            Value::from(expected),
            "paths.{key} in {}",
            self.0
        );
    }

    fn profile(&self) -> Value {
        self.0["profile"].clone()
    }
}

fn paths(run: Run) -> Paths {
    let out = run.fake_omp().run();
    let data = out.ok_data();
    let p = data["paths"].clone();
    assert!(p.is_object(), "data.paths\n{}", out.ctx());
    Paths(p)
}

fn omp(sb: &Sandbox) -> PathBuf {
    sb.home.join(".omp")
}

fn assert_default_layout(sb: &Sandbox, p: &Paths) {
    let base = omp(sb);
    p.assert("base_config_root", &base);
    p.assert("config_root", &base);
    p.assert("agent_dir", &base.join("agent"));
    p.assert("sessions_dir", &base.join("agent/sessions"));
    p.assert("session_owners_dir", &base.join("run/session-owners"));
    p.assert("extensions_dir", &base.join("agent/extensions"));
    p.assert_str("agent_dir_source", "default");
    p.assert_str("sessions_dir_source", "agent_dir");
    assert_eq!(p.profile(), Value::Null);
}

#[test]
fn ac011_default_paths() {
    let sb = Sandbox::new();
    let p = paths(sb.glen(["doctor"]));
    assert_default_layout(&sb, &p);
}

#[test]
fn a10_json_paths_are_absolute() {
    let sb = Sandbox::new();
    let p = paths(
        sb.glen(["doctor"])
            .env("PI_CODING_AGENT_DIR", "rel/agent")
            .env("GLEN_HOME", "~/g"),
    );
    for key in [
        "base_config_root",
        "config_root",
        "agent_dir",
        "sessions_dir",
        "session_owners_dir",
        "extensions_dir",
    ] {
        assert!(p.get(key).starts_with('/'), "paths.{key} is not absolute");
    }
}

#[test]
fn a12_pi_config_dir_joins_under_home() {
    let sb = Sandbox::new();
    let h = &sb.home;
    let cases: [(&str, PathBuf); 4] = [
        (".omp-dev", h.join(".omp-dev")),
        // Node path.join: an absolute value lands under home.
        ("/abs/x", h.join("abs/x")),
        // `..` is normalised lexically.
        ("a/../b", h.join("b")),
        ("../outside", lexical(&h.join("../outside"))),
    ];
    for (value, base) in cases {
        let p = paths(sb.glen(["doctor"]).env("PI_CONFIG_DIR", value));
        p.assert("base_config_root", &base);
        p.assert("agent_dir", &base.join("agent"));
        p.assert("sessions_dir", &base.join("agent/sessions"));
        p.assert("session_owners_dir", &base.join("run/session-owners"));
        p.assert("extensions_dir", &base.join("agent/extensions"));
    }
}

#[test]
fn a12_empty_or_blank_overrides_are_unset() {
    let sb = Sandbox::new();
    for (k, v) in [
        ("PI_CONFIG_DIR", ""),
        ("PI_CONFIG_DIR", "   "),
        ("PI_CODING_AGENT_DIR", ""),
        ("PI_CODING_AGENT_DIR", "  "),
        ("OMP_PROFILE", ""),
        ("PI_PROFILE", ""),
        ("XDG_DATA_HOME", ""),
        ("XDG_STATE_HOME", ""),
    ] {
        let p = paths(sb.glen(["doctor"]).env(k, v));
        assert_default_layout(&sb, &p);
    }
}

#[test]
fn a12_agent_dir_override() {
    let sb = Sandbox::new();
    let p = paths(
        sb.glen(["doctor"])
            .env("PI_CODING_AGENT_DIR", "/custom/agent"),
    );
    p.assert("agent_dir", Path::new("/custom/agent"));
    p.assert("sessions_dir", Path::new("/custom/agent/sessions"));
    p.assert("extensions_dir", Path::new("/custom/agent/extensions"));
    // Owners ignore the agent-dir override.
    p.assert("session_owners_dir", &omp(&sb).join("run/session-owners"));
    p.assert_str("agent_dir_source", "env");
    p.assert_str("sessions_dir_source", "agent_dir");
}

#[test]
fn a12_relative_agent_dir_resolves_against_cwd() {
    let sb = Sandbox::new();
    let p = paths(sb.glen(["doctor"]).env("PI_CODING_AGENT_DIR", "rel/agent"));
    let got = p.get("agent_dir");
    let lexical_cwd = sb.cwd.join("rel/agent");
    let real_cwd = fs::canonicalize(&sb.cwd).unwrap().join("rel/agent");
    assert!(
        got == s(&lexical_cwd) || got == s(&real_cwd),
        "agent_dir {got} should be <cwd>/rel/agent"
    );
    assert_eq!(p.get("sessions_dir"), format!("{got}/sessions"));
    p.assert_str("agent_dir_source", "env");
}

#[test]
fn a12_named_profile() {
    let sb = Sandbox::new();
    let base = omp(&sb);
    let work = base.join("profiles/work");
    let check_work = |p: &Paths| {
        assert_eq!(p.profile(), Value::from("work"), "{}", p.0);
        p.assert("base_config_root", &base);
        p.assert("config_root", &work);
        p.assert("agent_dir", &work.join("agent"));
        p.assert("sessions_dir", &work.join("agent/sessions"));
        p.assert("extensions_dir", &work.join("agent/extensions"));
        // Owners are shared across profiles.
        p.assert("session_owners_dir", &base.join("run/session-owners"));
        p.assert_str("agent_dir_source", "profile");
    };
    // OMP_PROFILE selects; PI_CODING_AGENT_DIR is ignored under a named profile.
    check_work(&paths(
        sb.glen(["doctor"])
            .env("OMP_PROFILE", "work")
            .env("PI_CODING_AGENT_DIR", "/custom/agent"),
    ));
    check_work(&paths(sb.glen(["doctor"]).env("PI_PROFILE", "work")));
    check_work(&paths(sb.glen(["doctor"]).env("OMP_PROFILE", "  work  ")));
    // --profile beats OMP_PROFILE, before or after the subcommand.
    check_work(&paths(
        sb.glen(["--profile", "work", "doctor"])
            .env("OMP_PROFILE", "other"),
    ));
    check_work(&paths(
        sb.glen(["doctor", "--profile", "work"])
            .env("PI_PROFILE", "other"),
    ));
    // Only the selected variable is validated.
    check_work(&paths(
        sb.glen(["doctor"])
            .env("OMP_PROFILE", "work")
            .env("PI_PROFILE", "Bad Name"),
    ));
}

#[test]
fn a12_default_profile_selection() {
    let sb = Sandbox::new();
    for run in [
        // OMP_PROFILE set (even empty) wins over PI_PROFILE.
        sb.glen(["doctor"])
            .env("OMP_PROFILE", "")
            .env("PI_PROFILE", "work"),
        sb.glen(["doctor"])
            .env("OMP_PROFILE", "")
            .env("PI_PROFILE", "Bad Name"),
        sb.glen(["doctor"]).env("OMP_PROFILE", "default"),
        sb.glen(["doctor", "--profile", "default"])
            .env("OMP_PROFILE", "work"),
    ] {
        assert_default_layout(&sb, &paths(run));
    }
}

#[test]
fn a12_profile_derived_agent_dir_is_ignored() {
    let sb = Sandbox::new();
    let derived = omp(&sb).join("profiles/work/agent");
    let p = paths(
        sb.glen(["doctor"])
            .env("OMP_PROFILE", "")
            .env("PI_PROFILE", "work")
            .env("PI_CODING_AGENT_DIR", &derived),
    );
    assert_default_layout(&sb, &p);

    // Without PI_PROFILE the same value is an ordinary override.
    let p = paths(sb.glen(["doctor"]).env("PI_CODING_AGENT_DIR", &derived));
    p.assert("agent_dir", &derived);
    p.assert_str("agent_dir_source", "env");
}

#[test]
fn a12_xdg_data_home_redirects_sessions_only_when_it_exists() {
    let sb = Sandbox::new();
    let data = sb.dir("data");
    let base = omp(&sb);

    // <XDG_DATA_HOME>/omp absent → no redirect.
    let p = paths(sb.glen(["doctor"]).env("XDG_DATA_HOME", &data));
    assert_default_layout(&sb, &p);

    fs::create_dir(data.join("omp")).unwrap();
    let p = paths(sb.glen(["doctor"]).env("XDG_DATA_HOME", &data));
    p.assert("sessions_dir", &data.join("omp/sessions"));
    p.assert_str("sessions_dir_source", "xdg");
    p.assert("agent_dir", &base.join("agent"));
    p.assert("extensions_dir", &base.join("agent/extensions"));

    // An agent-dir override disables the redirect.
    let p = paths(
        sb.glen(["doctor"])
            .env("XDG_DATA_HOME", &data)
            .env("PI_CODING_AGENT_DIR", "/custom"),
    );
    p.assert("sessions_dir", Path::new("/custom/sessions"));
    p.assert_str("sessions_dir_source", "agent_dir");

    // Named profile: needs <XDG_DATA_HOME>/omp/profiles/<p>.
    let work = base.join("profiles/work/agent");
    let p = paths(
        sb.glen(["doctor"])
            .env("XDG_DATA_HOME", &data)
            .env("OMP_PROFILE", "work"),
    );
    p.assert("sessions_dir", &work.join("sessions"));
    p.assert_str("sessions_dir_source", "agent_dir");

    fs::create_dir_all(data.join("omp/profiles/work")).unwrap();
    let p = paths(
        sb.glen(["doctor"])
            .env("XDG_DATA_HOME", &data)
            .env("OMP_PROFILE", "work"),
    );
    p.assert("sessions_dir", &data.join("omp/profiles/work/sessions"));
    p.assert_str("sessions_dir_source", "xdg");
    p.assert("agent_dir", &work);
    p.assert("extensions_dir", &work.join("extensions"));
}

#[test]
fn a12_xdg_state_home_redirects_session_owners() {
    let sb = Sandbox::new();
    let state = sb.dir("state");
    let base = omp(&sb);

    let p = paths(sb.glen(["doctor"]).env("XDG_STATE_HOME", &state));
    p.assert("session_owners_dir", &base.join("run/session-owners"));

    fs::create_dir(state.join("omp")).unwrap();
    let owners = state.join("omp/run/session-owners");
    for run in [
        sb.glen(["doctor"]),
        sb.glen(["doctor"]).env("OMP_PROFILE", "work"),
        sb.glen(["doctor"]).env("PI_CODING_AGENT_DIR", "/custom"),
    ] {
        let p = paths(run.env("XDG_STATE_HOME", &state));
        p.assert("session_owners_dir", &owners);
    }
}

#[test]
fn a12_profile_name_validation() {
    let sb = Sandbox::new();
    let max = format!("a{}", "b".repeat(63));
    for name in ["a", "w0rk", "a.b-c_d", "console", "com10", max.as_str()] {
        let p = paths(sb.glen(["doctor", "--profile", name]));
        assert_eq!(p.profile(), Value::from(name));
    }
    let too_long = format!("a{}", "b".repeat(64));
    for name in [
        "../x",
        "Bad Name",
        "Work",
        "x.",
        ".",
        "..",
        "_x",
        "a/b",
        "con",
        "CON",
        "con.txt",
        "nul",
        "com1",
        "lpt9.log",
        too_long.as_str(),
    ] {
        let arg = format!("--profile={name}");
        sb.glen(["doctor", arg.as_str()])
            .fake_omp()
            .run()
            .error_envelope("usage");
    }
}

#[test]
fn ac013_invalid_profile_env_names_the_variable() {
    let sb = Sandbox::new();
    for (var, value) in [
        ("OMP_PROFILE", "Bad Name"),
        ("OMP_PROFILE", "con.txt"),
        ("PI_PROFILE", "Bad Name"),
        ("PI_PROFILE", ".."),
    ] {
        let out = sb.glen(["doctor"]).fake_omp().env(var, value).run();
        let err = out.error_envelope("invalid_env");
        out.assert_stderr_empty();
        assert!(
            err["message"].as_str().unwrap().contains(var),
            "message must name {var}\n{}",
            out.ctx()
        );
    }
}

#[test]
fn a6_glen_home_resolution() {
    let sb = Sandbox::new();
    let home_of = |run: Run| -> Value { run.fake_omp().run().ok_data()["glen"]["home"].clone() };

    assert_eq!(
        home_of(sb.glen(["doctor"]).env_remove("GLEN_HOME")),
        Value::from(s(&sb.home.join(".glen")))
    );
    assert_eq!(
        home_of(sb.glen(["doctor"]).env("GLEN_HOME", "   ")),
        Value::from(s(&sb.home.join(".glen")))
    );
    assert_eq!(
        home_of(sb.glen(["doctor"]).env("GLEN_HOME", "/x/y")),
        Value::from("/x/y")
    );
    assert_eq!(
        home_of(sb.glen(["doctor"]).env("GLEN_HOME", "~/g")),
        Value::from(s(&sb.home.join("g")))
    );
    assert_eq!(
        home_of(sb.glen(["doctor"]).env("GLEN_HOME", "~")),
        Value::from(s(&sb.home))
    );
}

#[test]
fn ac020_relative_glen_home_is_invalid_env() {
    let sb = Sandbox::new();
    for value in ["rel", "./rel", "rel/../x"] {
        let out = sb.glen(["doctor"]).fake_omp().env("GLEN_HOME", value).run();
        let err = out.error_envelope("invalid_env");
        out.assert_stderr_empty();
        assert!(
            err["message"].as_str().unwrap().contains("GLEN_HOME"),
            "{}",
            out.ctx()
        );
    }
    assert!(
        is_empty_dir(&sb.cwd),
        "a relative GLEN_HOME must not be created: {:?}",
        list_tree(&sb.cwd)
    );
}

/// AC-020 has no trigger restriction, and the plan resolves GLEN_HOME while building the output
/// context (Step 6 `OutputCtx::new`, Step 8 `run()` step 3), before dispatch. So a relative
/// GLEN_HOME is `invalid_env` for every command, not only for doctor.
///
/// Raw-output commands follow the clarified §A10 rule: without `--json` every error is human text
/// on stderr with an empty stdout (so `glen completions zsh >> ~/.zshrc` never appends an
/// envelope); with `--json` the error envelope goes to stdout.
#[test]
fn ac020_relative_glen_home_is_invalid_env_for_every_command() {
    let sb = Sandbox::new();

    sb.glen(["ls"])
        .env("GLEN_HOME", "rel")
        .run()
        .error_envelope("invalid_env");

    let out = sb
        .glen(["completions", "bash"])
        .env("GLEN_HOME", "rel")
        .run();
    out.assert_exit(exit_for("invalid_env")).assert_no_ansi();
    assert!(
        out.stdout.is_empty(),
        "raw command errors never reach stdout without --json\n{}",
        out.ctx()
    );
    let err = out.err();
    assert!(
        err.contains("GLEN_HOME") || err.contains("invalid_env"),
        "stderr names the bad variable\n{}",
        out.ctx()
    );

    for args in [
        vec!["completions", "bash", "--json"],
        vec!["--json", "completions", "bash"],
    ] {
        let out = sb.glen(&args).env("GLEN_HOME", "rel").run();
        let err = out.error_envelope("invalid_env");
        out.assert_stderr_empty();
        assert!(
            err["message"].as_str().unwrap().contains("GLEN_HOME"),
            "{}",
            out.ctx()
        );
    }

    assert!(is_empty_dir(&sb.cwd), "{:?}", list_tree(&sb.cwd));
}
