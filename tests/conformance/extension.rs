//! `glen install-extension` and the doctor checks `extension` / `live_endpoints`
//! (AC-302…AC-304, AC-321, AC-322, AC-324; orchestrator O4; docs/design.md §A6, §A9, §A10).

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use xxhash_rust::xxh64::xxh64;

use crate::live_support::*;
use crate::reader::*;
use crate::rpc_support::*;
use crate::support::*;

/// §A10 install-extension data keys.
const INSTALL_KEYS: [&str; 10] = [
    "action",
    "path",
    "version",
    "previous_version",
    "glen_home",
    "profile",
    "agent_dir",
    "agent_dir_source",
    "dry_run",
    "restart_required",
];

fn target(agent: &Path) -> PathBuf {
    agent.join("extensions/glen/index.ts")
}

/// A sandbox with an existing agent dir (PI_CODING_AGENT_DIR) and a GLEN_HOME that does not exist.
struct Ix {
    sb: Sandbox,
    agent: PathBuf,
}

impl Ix {
    fn new() -> Self {
        let sb = Sandbox::new();
        let agent = sb.dir("agent");
        Ix { sb, agent }
    }

    fn file(&self) -> PathBuf {
        target(&self.agent)
    }

    fn run(&self, args: &[&str]) -> Run {
        let mut a = vec!["install-extension"];
        a.extend_from_slice(args);
        a.push("--json");
        self.sb.glen(a).env("PI_CODING_AGENT_DIR", &self.agent)
    }

    /// The envelope of a successful run (exit 0, one document, empty stderr, no ANSI).
    fn env(&self, args: &[&str]) -> Value {
        ok_doc(&self.run(args).run(), 0)
    }

    fn doctor(&self) -> Value {
        data_of(
            &self
                .sb
                .glen(["doctor", "--json"])
                .fake_omp()
                .env("PI_CODING_AGENT_DIR", &self.agent)
                .run(),
            0,
        )
    }
}

fn assert_install_data(d: &Value) {
    let o = d.as_object().unwrap_or_else(|| panic!("data: {d}"));
    for k in INSTALL_KEYS {
        assert!(o.contains_key(k), "install-extension data lacks `{k}`: {d}");
    }
    assert!(
        d["dry_run"].is_boolean() && d["restart_required"].is_boolean(),
        "{d}"
    );
    assert_eq!(d["version"], PKG_VERSION, "{d}");
}

/// Line 1 split into (version, protocol, digest); `None` when it is not the glen marker.
fn marker(bytes: &[u8]) -> Option<(String, String, String)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let line = text.lines().next()?;
    let rest = line.strip_prefix("// glen-extension ")?;
    let parts: Vec<&str> = rest.split(' ').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parts[0].strip_prefix("version=")?.to_owned(),
        parts[1].strip_prefix("protocol=")?.to_owned(),
        parts[2].strip_prefix("digest=")?.to_owned(),
    ))
}

/// Every byte after line 1.
fn body(bytes: &[u8]) -> &[u8] {
    let nl = bytes.iter().position(|b| *b == b'\n').expect("line 1 ends");
    &bytes[nl + 1..]
}

fn digest(body: &[u8]) -> String {
    format!("{:016x}", xxh64(body, 0))
}

/// Write `body` under a correct glen marker naming `version`.
fn craft(path: &Path, version: &str, body: &[u8]) {
    let mut out = format!(
        "// glen-extension version={version} protocol=1 digest={}\n",
        digest(body)
    )
    .into_bytes();
    out.extend_from_slice(body);
    fs::write(path, out).unwrap();
}

/// (bytes, inode, mtime) of a file.
fn stamp(p: &Path) -> (Vec<u8>, u64, SystemTime) {
    let md = fs::metadata(p).unwrap();
    (fs::read(p).unwrap(), md.ino(), md.modified().unwrap())
}

fn age(p: &Path) {
    set_mtime(
        p,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000),
    );
}

fn warning(env: &Value, code: &str) -> bool {
    warning_codes(env).iter().any(|c| c == code)
}

/// The rendering a fresh install produces for the same environment, from a scratch agent dir.
fn reference(ix: &Ix) -> Vec<u8> {
    let other = ix.sb.dir("reference-agent");
    let out = ix
        .sb
        .glen(["install-extension", "--json"])
        .env("PI_CODING_AGENT_DIR", &other)
        .run();
    assert_eq!(data_of(&out, 0)["action"], "installed");
    fs::read(target(&other)).unwrap()
}

// ---------------------------------------------------------------------------------------------
// AC-302 — install.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac302_install_writes_one_private_rendered_file_and_nothing_else() {
    let ix = Ix::new();
    let env = ix.env(&[]);
    assert!(env.get("warnings").is_none(), "{env}");
    let d = &env["data"];
    assert_install_data(d);
    assert_eq!(d["action"], "installed", "{d}");
    assert_eq!(d["dry_run"], false, "{d}");
    assert_eq!(d["restart_required"], true, "{d}");
    assert!(d["previous_version"].is_null(), "{d}");
    assert_eq!(d["path"], s(&ix.file()), "{d}");
    assert_eq!(d["agent_dir"], s(&ix.agent), "{d}");
    assert_eq!(d["agent_dir_source"], "env", "{d}");
    // §A10 W3 clarifications: null for the default profile, like doctor's `paths.profile`.
    assert!(d["profile"].is_null(), "{d}");
    assert_eq!(d["glen_home"], s(&ix.sb.glen_home), "{d}");

    let f = ix.file();
    assert_eq!(mode(f.parent().unwrap()), 0o700, "extensions/glen is 0700");
    assert_eq!(mode(&f), 0o600, "index.ts is 0600");
    let bytes = fs::read(&f).unwrap();
    let (v, p, dg) = marker(&bytes).expect("line 1 is the glen marker");
    assert_eq!(v, PKG_VERSION);
    assert_eq!(p, "1");
    assert!(
        dg.len() == 16 && dg.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "digest is 16 lowercase hex: {dg}"
    );
    assert_eq!(
        dg,
        digest(body(&bytes)),
        "digest = xxh64(seed 0) of every byte after line 1"
    );
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.contains(&s(&ix.sb.glen_home)),
        "the resolved GLEN_HOME is baked in"
    );
    assert!(!text.contains("__GLEN_"), "no placeholder is left");

    assert!(
        !ix.sb.glen_home.exists(),
        "install-extension never creates GLEN_HOME"
    );
    assert_eq!(
        list_tree(&ix.agent),
        ["extensions", "extensions/glen", "extensions/glen/index.ts"],
        "AC-302/AC-322: nothing else under the agent dir (no temp file left)"
    );
}

#[test]
fn ac302_the_target_follows_the_effective_profile() {
    // Default profile, no override: $HOME/.omp/agent.
    let sb = Sandbox::new();
    let d = data_of(&sb.glen(["install-extension", "--json"]).run(), 0);
    let want = sb.home.join(".omp/agent/extensions/glen/index.ts");
    assert_eq!(d["path"], s(&want), "{d}");
    assert_eq!(d["agent_dir_source"], "default", "{d}");
    assert!(d["profile"].is_null(), "the default profile is null: {d}");
    assert!(want.is_file());

    // A named profile ignores PI_CODING_AGENT_DIR, whichever way it is selected.
    for (flag, env) in [
        (Some("work"), None),
        (None, Some(("OMP_PROFILE", "work"))),
        (None, Some(("PI_PROFILE", "work"))),
    ] {
        let sb = Sandbox::new();
        let ignored = sb.root.join("ignored-agent");
        let mut args = vec!["install-extension", "--json"];
        if let Some(p) = flag {
            args.extend(["--profile", p]);
        }
        let mut run = sb.glen(args).env("PI_CODING_AGENT_DIR", &ignored);
        if let Some((k, v)) = env {
            run = run.env(k, v);
        }
        let d = data_of(&run.run(), 0);
        let want = sb
            .home
            .join(".omp/profiles/work/agent/extensions/glen/index.ts");
        assert_eq!(d["path"], s(&want), "{flag:?} {env:?}: {d}");
        assert_eq!(d["profile"], "work", "{d}");
        assert_eq!(d["agent_dir_source"], "profile", "{d}");
        assert!(want.is_file(), "{flag:?} {env:?}");
        assert!(
            !ignored.exists(),
            "PI_CODING_AGENT_DIR is ignored with a named profile"
        );
    }
}

#[test]
fn ac302_the_baked_glen_home_is_the_resolved_absolute_path() {
    for (value, want) in [(Some("~/gh"), "home/gh"), (None, "home/.glen")] {
        let ix = Ix::new();
        let mut run = ix.run(&[]);
        run = match value {
            Some(v) => run.env("GLEN_HOME", v),
            None => run.env_remove("GLEN_HOME"),
        };
        let d = data_of(&run.run(), 0);
        let abs = ix.sb.root.join(want);
        assert_eq!(d["glen_home"], s(&abs), "{value:?}: {d}");
        let text = fs::read_to_string(ix.file()).unwrap();
        assert!(
            text.contains(&s(&abs)),
            "{value:?}: the absolute home is baked"
        );
        assert!(!abs.exists(), "{value:?}: GLEN_HOME is not created");
    }

    // A relative GLEN_HOME is invalid (§A6) and nothing is written.
    let ix = Ix::new();
    let out = ix.run(&[]).env("GLEN_HOME", "relative/home").run();
    failed(&out, "invalid_env");
    assert!(
        list_tree(&ix.agent).is_empty(),
        "{:?}",
        list_tree(&ix.agent)
    );
}

// ---------------------------------------------------------------------------------------------
// AC-303 — idempotence, upgrades and protection.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac303_an_identical_rendering_is_unchanged_and_untouched() {
    let ix = Ix::new();
    ix.env(&[]);
    age(&ix.file());
    let before = stamp(&ix.file());
    for args in [
        &[][..],
        &["--dry-run"][..],
        &["--force"][..],
        &["--force", "--dry-run"][..],
    ] {
        let env = ix.env(args);
        let d = &env["data"];
        assert_install_data(d);
        assert_eq!(d["action"], "unchanged", "{args:?}: {d}");
        assert_eq!(d["dry_run"], args.contains(&"--dry-run"), "{d}");
        assert_eq!(
            stamp(&ix.file()),
            before,
            "{args:?}: same bytes, inode and mtime"
        );
    }
    assert_eq!(ix.env(&[])["data"]["restart_required"], false);
}

#[test]
fn ac303_an_older_version_is_upgraded() {
    let ix = Ix::new();
    ix.env(&[]);
    let current = fs::read(ix.file()).unwrap();
    craft(&ix.file(), "0.0.1", body(&current));

    let before = stamp(&ix.file());
    let d = &ix.env(&["--dry-run"])["data"];
    assert_eq!(d["action"], "upgraded", "{d}");
    assert_eq!(d["dry_run"], true, "{d}");
    assert_eq!(stamp(&ix.file()), before, "--dry-run changes nothing");

    let d = &ix.env(&[])["data"];
    assert_eq!(d["action"], "upgraded", "{d}");
    assert_eq!(d["previous_version"], "0.0.1", "{d}");
    assert_eq!(d["restart_required"], true, "{d}");
    assert_eq!(
        fs::read(ix.file()).unwrap(),
        current,
        "the current rendering"
    );
    assert_eq!(ix.env(&[])["data"]["action"], "unchanged");
}

#[test]
fn ac303_another_baked_glen_home_is_reinstalled() {
    let ix = Ix::new();
    let (h1, h2) = (ix.sb.root.join("home-one"), ix.sb.root.join("home-two"));
    data_of(&ix.run(&[]).env("GLEN_HOME", &h1).run(), 0);
    let d = data_of(&ix.run(&["--dry-run"]).env("GLEN_HOME", &h2).run(), 0);
    assert_eq!(d["action"], "reinstalled", "{d}");
    assert!(fs::read_to_string(ix.file()).unwrap().contains(&s(&h1)));
    let d = data_of(&ix.run(&[]).env("GLEN_HOME", &h2).run(), 0);
    assert_eq!(d["action"], "reinstalled", "{d}");
    let text = fs::read_to_string(ix.file()).unwrap();
    assert!(
        text.contains(&s(&h2)) && !text.contains(&s(&h1)),
        "the new home is baked"
    );
}

/// Turns the current rendering into a protected file.
type Make = fn(&Path, &[u8]);

/// The three protected cases: (name, warning, how to make the file).
fn protected_cases() -> [(&'static str, &'static str, Make); 3] {
    [
        ("newer", "extension_newer", |p, cur| {
            craft(p, "99.0.0", body(cur))
        }),
        ("modified", "extension_modified", |p, cur| {
            let mut b = cur.to_vec();
            b.extend_from_slice(b"// hand edit\n");
            fs::write(p, b).unwrap();
        }),
        ("foreign", "extension_foreign", |p, _| {
            fs::write(p, "export default function mine() {}\n").unwrap()
        }),
    ]
}

#[test]
fn ac303_newer_modified_and_foreign_files_are_skipped_unless_forced() {
    for (name, code, make) in protected_cases() {
        let ix = Ix::new();
        ix.env(&[]);
        let current = fs::read(ix.file()).unwrap();
        make(&ix.file(), &current);
        age(&ix.file());
        let before = stamp(&ix.file());
        for args in [&[][..], &["--dry-run"][..]] {
            let env = ix.env(args);
            assert_eq!(env["data"]["action"], "skipped", "{name} {args:?}: {env}");
            assert!(
                warning(&env, code),
                "{name} {args:?}: warning {code}: {env}"
            );
            assert_eq!(env["data"]["restart_required"], false, "{name}: {env}");
            assert_eq!(
                stamp(&ix.file()),
                before,
                "{name} {args:?}: nothing written"
            );
        }
        let env = ix.env(&["--force", "--dry-run"]);
        assert_eq!(env["data"]["dry_run"], true, "{env}");
        assert_eq!(env["data"]["action"], "replaced", "{name}: {env}");
        assert_eq!(
            stamp(&ix.file()),
            before,
            "{name}: --force --dry-run writes nothing"
        );

        let env = ix.env(&["--force"]);
        assert_eq!(
            env["data"]["action"], "replaced",
            "§A10 W3 clarifications: --force overwrite reports replaced ({name}): {env}"
        );
        assert_eq!(env["data"]["restart_required"], true, "{name}: {env}");
        assert_eq!(
            fs::read(ix.file()).unwrap(),
            reference(&ix),
            "{name}: current rendering"
        );
    }
}

#[test]
fn ac303_uninstall_removes_the_file_and_the_empty_dir() {
    let ix = Ix::new();
    ix.env(&[]);
    let tree = list_tree(&ix.agent);
    let d = &ix.env(&["--uninstall", "--dry-run"])["data"];
    assert_eq!(d["action"], "removed", "{d}");
    assert_eq!(d["dry_run"], true, "{d}");
    assert_eq!(list_tree(&ix.agent), tree, "--dry-run removes nothing");

    let d = &ix.env(&["--uninstall"])["data"];
    assert_install_data(d);
    assert_eq!(d["action"], "removed", "{d}");
    assert_eq!(d["restart_required"], true, "{d}");
    assert!(!ix.file().exists());
    assert!(
        !ix.agent.join("extensions/glen").exists(),
        "the empty glen dir is removed"
    );

    let d = &ix.env(&["--uninstall"])["data"];
    assert_eq!(d["action"], "not_installed", "{d}");
    assert_eq!(d["restart_required"], false, "{d}");
}

#[test]
fn ac303_uninstall_keeps_a_sibling_file_and_its_dir() {
    let ix = Ix::new();
    ix.env(&[]);
    let sibling = ix.agent.join("extensions/glen/notes.md");
    fs::write(&sibling, "mine").unwrap();
    let env = ix.env(&["--uninstall"]);
    assert_eq!(env["data"]["action"], "removed", "{env}");
    assert!(!ix.file().exists());
    assert_eq!(fs::read_to_string(&sibling).unwrap(), "mine");
    assert!(warning(&env, "extension_dir_not_empty"), "§A9: {env}");
}

#[test]
fn ac303_uninstall_protects_modified_and_foreign_files_unless_forced() {
    for (name, code, make) in protected_cases() {
        let ix = Ix::new();
        ix.env(&[]);
        let current = fs::read(ix.file()).unwrap();
        make(&ix.file(), &current);
        let before = stamp(&ix.file());
        let env = ix.env(&["--uninstall"]);
        if name == "newer" {
            // Only modified and foreign files are protected on uninstall.
            assert_eq!(env["data"]["action"], "removed", "{name}: {env}");
            assert!(!ix.file().exists());
            continue;
        }
        assert_eq!(env["data"]["action"], "skipped", "{name}: {env}");
        assert!(warning(&env, code), "{name}: warning {code}: {env}");
        assert_eq!(stamp(&ix.file()), before, "{name}: kept");
        let env = ix.env(&["--uninstall", "--force"]);
        assert_eq!(env["data"]["action"], "removed", "{name}: {env}");
        assert!(!ix.file().exists(), "{name}: removed with --force");
    }
}

#[test]
fn ac303_dry_run_install_creates_nothing() {
    let ix = Ix::new();
    let d = &ix.env(&["--dry-run"])["data"];
    assert_install_data(d);
    assert_eq!(d["action"], "installed", "{d}");
    assert_eq!(d["dry_run"], true, "{d}");
    assert!(
        list_tree(&ix.agent).is_empty(),
        "{:?}",
        list_tree(&ix.agent)
    );
    assert!(!ix.sb.glen_home.exists());
}

// ---------------------------------------------------------------------------------------------
// AC-321 — the socket path limit at install time.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac321_install_with_a_too_long_glen_home_still_installs_and_warns() {
    let ix = Ix::new();
    let long = format!("/tmp/{}", "a".repeat(SUN_LIMIT));
    let out = ix.run(&[]).env("GLEN_HOME", &long).run();
    let env = ok_doc(&out, 0);
    assert_eq!(env["data"]["action"], "installed", "{env}");
    assert!(warning(&env, "socket_path_too_long"), "AC-321: {env}");
    assert!(ix.file().is_file());
    assert!(!Path::new(&long).exists());

    let short = Ix::new();
    let env = short.env(&[]);
    assert!(!warning(&env, "socket_path_too_long"), "{env}");
}

// ---------------------------------------------------------------------------------------------
// O4 — human output explains the tool and how to uninstall.
// ---------------------------------------------------------------------------------------------

#[test]
fn o4_human_install_names_glen_reply_and_the_uninstall_command() {
    let ix = Ix::new();
    let out = ix
        .sb
        .glen(["install-extension", "--human"])
        .env("PI_CODING_AGENT_DIR", &ix.agent)
        .run();
    out.assert_exit(0).assert_no_ansi();
    assert!(!out.is_json(), "{}", out.ctx());
    let text = format!("{}{}", out.out(), out.err());
    assert!(
        text.contains("glen_reply"),
        "O4: names the tool\n{}",
        out.ctx()
    );
    assert!(
        text.contains("--uninstall"),
        "O4: says how to uninstall\n{}",
        out.ctx()
    );
}

// ---------------------------------------------------------------------------------------------
// AC-304 — doctor.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac304_doctor_reports_each_extension_state() {
    let ix = Ix::new();
    let d = ix.doctor();
    let c = check(&d, "extension");
    assert_eq!(c["status"], "info", "{c}");
    assert!(
        c["detail"]
            .as_str()
            .unwrap_or("")
            .contains("glen install-extension"),
        "{c}"
    );
    let e = &d["extension"];
    for k in ["path", "state", "version", "current"] {
        assert!(e.get(k).is_some(), "data.extension lacks `{k}`: {e}");
    }
    assert_eq!(e["path"], s(&ix.file()), "{e}");
    assert_eq!(e["state"], "not_installed", "{e}");
    assert_eq!(
        e["current"], false,
        "§A10 W3 clarifications: a boolean: {e}"
    );
    assert!(
        !ix.agent.join("extensions").exists(),
        "doctor creates no extensions dir"
    );
    assert!(!ix.sb.glen_home.exists(), "doctor creates no GLEN_HOME");

    ix.env(&[]);
    let d = ix.doctor();
    assert_eq!(check(&d, "extension")["status"], "ok", "{d}");
    assert_eq!(d["extension"]["state"], "current", "{d}");
    assert_eq!(d["extension"]["current"], true, "{d}");
    assert_eq!(d["extension"]["version"], PKG_VERSION, "{d}");
    let current = fs::read(ix.file()).unwrap();

    craft(&ix.file(), "0.0.1", body(&current));
    let d = ix.doctor();
    assert_eq!(check(&d, "extension")["status"], "warn", "{d}");
    assert_eq!(d["extension"]["state"], "outdated", "{d}");
    assert_eq!(d["extension"]["current"], false, "{d}");
    assert_eq!(d["extension"]["version"], "0.0.1", "{d}");

    for (name, _, make) in protected_cases() {
        make(&ix.file(), &current);
        let d = ix.doctor();
        assert_eq!(check(&d, "extension")["status"], "warn", "{name}: {d}");
        assert_eq!(d["extension"]["state"], name, "{d}");
        assert_eq!(d["extension"]["current"], false, "{name}: {d}");
    }

    fs::write(ix.file(), &current).unwrap();
    let d = data_of(
        &ix.sb
            .glen(["doctor", "--json"])
            .fake_omp()
            .env("PI_CODING_AGENT_DIR", &ix.agent)
            .env("GLEN_HOME", ix.sb.root.join("elsewhere"))
            .run(),
        0,
    );
    assert_eq!(check(&d, "extension")["status"], "warn", "{d}");
    assert_eq!(d["extension"]["state"], "different_home", "{d}");
    assert_eq!(d["extension"]["current"], false, "{d}");
}

#[test]
fn ac304_doctor_counts_live_and_stale_endpoints() {
    let short = tempfile::Builder::new()
        .prefix("gd")
        .tempdir_in("/tmp")
        .unwrap();
    let home = short.path().join("g");
    let sb = Sandbox::new();
    let doctor = || {
        data_of(
            &sb.glen(["doctor", "--json"])
                .fake_omp()
                .env("GLEN_HOME", &home)
                .run(),
            0,
        )
    };
    let d = doctor();
    let c = check(&d, "live_endpoints");
    assert_eq!(c["status"], "info", "{c}");
    assert_eq!(c["value"], "0 live, 0 stale", "{c}");
    assert!(!home.exists(), "doctor never creates GLEN_HOME");

    let live = home.join("run/live");
    mkdir_private(&live);
    let dead = dead_pid();
    let reg = |id: &str, pid: u32| {
        write_private(
            &live.join(format!("{id}.json")),
            &serde_json::to_vec(&registry_doc(id, pid, &live.join(format!("{pid}.sock")))).unwrap(),
        )
    };
    reg(A.id, dead);
    let c = check(&doctor(), "live_endpoints").clone();
    assert_eq!(c["value"], "0 live, 1 stale", "{c}");
    assert_eq!(c["status"], "info", "{c}");

    reg(B.id, std::process::id());
    let c = check(&doctor(), "live_endpoints").clone();
    assert_eq!(c["value"], "1 live, 1 stale", "{c}");

    write_private(&live.join(format!("{}.json", C.id)), b"{broken");
    let c = check(&doctor(), "live_endpoints").clone();
    assert_eq!(
        c["status"], "warn",
        "AC-304: an unreadable registry warns: {c}"
    );
    assert!(
        live.join(format!("{}.json", A.id)).exists(),
        "doctor deletes nothing"
    );
}

/// A GLEN_HOME whose `run/live/9999999.sock` is exactly `bytes` long, padded with `pad`.
fn home_of_len(bytes: usize, pad: &str) -> String {
    let tail = "/run/live/9999999.sock".len();
    let mut h = String::from("/tmp/");
    while h.len() + pad.len() + tail <= bytes {
        h.push_str(pad);
    }
    while h.len() + tail < bytes {
        h.push('a');
    }
    assert_eq!(h.len() + tail, bytes);
    h
}

#[test]
fn ac304_ac321_doctor_warns_when_the_socket_path_reaches_the_limit() {
    let sb = Sandbox::new();
    let status = |home: &str| {
        let d = data_of(
            &sb.glen(["doctor", "--json"])
                .fake_omp()
                .env("GLEN_HOME", home)
                .run(),
            0,
        );
        assert!(!Path::new(home).exists(), "doctor never creates GLEN_HOME");
        check(&d, "live_endpoints")["status"].clone()
    };
    assert_eq!(
        status(&home_of_len(SUN_LIMIT - 1, "a")),
        "info",
        "one byte below"
    );
    assert_eq!(status(&home_of_len(SUN_LIMIT, "a")), "warn", "at the limit");
    // Counted in UTF-8 bytes: two-byte characters reach the limit with fewer characters.
    let multi = home_of_len(SUN_LIMIT, "é");
    assert!(multi.chars().count() + 22 < SUN_LIMIT, "{multi}");
    assert_eq!(status(&multi), "warn", "AC-321: bytes, not characters");
}

#[test]
fn ac304_doctor_lists_the_w3_checks_in_order_and_installing_keeps_it_ok() {
    let ix = Ix::new();
    let names: Vec<String> = arr(&ix.doctor()["checks"])
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, DOCTOR_CHECKS, "§A10 doctor checks");
    assert_eq!(json!(names[6..8]), json!(["extension", "live_endpoints"]));
}

// ---------------------------------------------------------------------------------------------
// AC-303 — every combination of --dry-run, --force and --uninstall on each starting state.
// ---------------------------------------------------------------------------------------------

/// The agent tree with every file's bytes, for "changes nothing" checks.
fn tree_bytes(root: &Path) -> Vec<(String, u64, SystemTime, Vec<u8>)> {
    snapshot(root)
        .into_iter()
        .map(|(rel, len, t)| {
            let p = root.join(&rel);
            let bytes = if p.is_file() {
                fs::read(&p).unwrap()
            } else {
                vec![]
            };
            (rel, len, t, bytes)
        })
        .collect()
}

#[test]
fn ac303_every_flag_combination_on_each_state() {
    // (state, uninstall, force) → expected action (AC-303 and the §A10 W3 clarifications).
    let expect = |state: &str, uninstall: bool, force: bool| -> &'static str {
        match (state, uninstall, force) {
            ("absent", false, _) => "installed",
            ("absent", true, _) => "not_installed",
            ("current", false, _) => "unchanged",
            ("current", true, _) => "removed",
            ("outdated", false, _) => "upgraded",
            ("outdated", true, _) => "removed",
            ("modified", false, false) => "skipped",
            ("modified", false, true) => "replaced",
            ("modified", true, false) => "skipped",
            ("modified", true, true) => "removed",
            _ => unreachable!(),
        }
    };
    for state in ["absent", "current", "outdated", "modified"] {
        for bits in 0..8u8 {
            let (dry, force, uninstall) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
            let ix = Ix::new();
            if state != "absent" {
                ix.env(&[]);
                let cur = fs::read(ix.file()).unwrap();
                if state == "outdated" {
                    craft(&ix.file(), "0.0.1", body(&cur));
                } else if state == "modified" {
                    let mut b = cur.clone();
                    b.extend_from_slice(b"// edit\n");
                    fs::write(ix.file(), b).unwrap();
                }
            }
            let before = tree_bytes(&ix.agent);
            let mut args = vec![];
            for (on, flag) in [
                (dry, "--dry-run"),
                (force, "--force"),
                (uninstall, "--uninstall"),
            ] {
                if on {
                    args.push(flag);
                }
            }
            let env = ix.env(&args);
            let d = &env["data"];
            assert_install_data(d);
            assert_eq!(d["dry_run"], dry, "{state} {args:?}: {d}");
            let action = d["action"].as_str().unwrap_or("");
            assert_eq!(
                action,
                expect(state, uninstall, force),
                "{state} {args:?}: {env}"
            );
            if dry {
                assert_eq!(
                    tree_bytes(&ix.agent),
                    before,
                    "{state} {args:?}: --dry-run wrote"
                );
            } else {
                let written = !["skipped", "unchanged", "not_installed"].contains(&action);
                assert_eq!(
                    tree_bytes(&ix.agent) != before,
                    written,
                    "{state} {args:?}: the tree changes exactly when the action writes"
                );
            }
            assert!(
                !ix.sb.glen_home.exists(),
                "{state} {args:?}: GLEN_HOME created"
            );
        }
    }
}
