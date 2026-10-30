//! W4 agent surface: the shipped skill, `install-skill`, doctor's W4 rows, the Codex sandbox
//! refusals and the `docs/json.md` contract (AC-401…AC-413, the code-path half of AC-419, and the
//! orchestrator decisions O3, O5, O6 and O7). Derived from `.claude/plans/v010-wave-4-agent.md`,
//! `docs/design.md` (§A9–§A13) and `docs/json.md`, without reading `src/skill.rs`,
//! `src/sandbox.rs`, `src/commands/install_skill.rs` or `src/omp/dotenv.rs`.
//!
//! The skill is observed only as the binary writes it (`glen install-skill`), and the command
//! surface only as `glen … --help` prints it. `Ws` gives every case its own canonical temp root
//! with HOME, GLEN_HOME (never pre-created), PI_CODING_AGENT_DIR, TMPDIR, an empty PATH and a
//! project cwd outside HOME, all set per `Command`. Nothing reads or writes the real `~/.claude`,
//! `~/.agents`, `~/.codex` or `~/.omp`. RPC cases use the W2 `RpcBox` (fixture copies and the
//! `fake_omp` peer); reader cases use the W1 `OmpBox`.
//!
//! The acceptance modes (AC-414…AC-418, AC-420, AC-421) need a real omp, Claude Code and Codex,
//! and are not run here.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use xxhash_rust::xxh64::xxh64;

use crate::reader::{self, A, OmpBox, RestoreMode, hold_lease, set_mtime, snapshot};
use crate::rpc_support::{RpcBox, data_of, failed, job_handle, job_id, warning_codes};
use crate::support::*;

const AGENTS: [&str; 2] = ["claude", "codex"];
/// `docs/json.md`: `targets[].state` and `targets[].action` vocabularies.
const STATES: [&str; 6] = [
    "not_installed",
    "current",
    "outdated",
    "newer",
    "modified",
    "foreign",
];
const ACTIONS: [&str; 7] = [
    "installed",
    "unchanged",
    "upgraded",
    "skipped",
    "reinstalled",
    "removed",
    "not_installed",
];
/// §A10 glyphs.
const GLYPHS: [char; 8] = ['✓', '!', '✗', '●', '◐', '○', '?', '·'];
const OLDER: &str = "0.0.1";
const NEWER: &str = "99.0.0";
/// Codex's sandbox with network off (AC-410).
const OFFLINE: [(&str, &str); 2] = [
    ("CODEX_SANDBOX", "seatbelt"),
    ("CODEX_SANDBOX_NETWORK_DISABLED", "1"),
];
/// Codex's sandbox with network on.
const NETWORKED: [(&str, &str); 1] = [("CODEX_SANDBOX", "seatbelt")];
/// The `install-skill` flags (AC-405…AC-407).
const FLAGS: [&str; 6] = [
    "--claude",
    "--codex",
    "--project",
    "--dry-run",
    "--force",
    "--uninstall",
];

/// A path inside a workspace.
type PathOf = fn(&Ws) -> PathBuf;
/// `(file, contents, key, extra env)` for one AC-409 case.
type EnvCase = (
    PathOf,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

fn running_as_root() -> bool {
    rustix::process::geteuid().as_raw() == 0
}

fn envs(mut run: Run, vars: &[(&str, &str)]) -> Run {
    for (k, v) in vars {
        run = run.env(k, v);
    }
    run
}

fn args_of(base: &[&str], extra: &[&str]) -> Vec<String> {
    base.iter().chain(extra).map(|a| (*a).to_owned()).collect()
}

// ---------------------------------------------------------------------------------------------
// The workspace.
// ---------------------------------------------------------------------------------------------

/// An isolated, canonical temp root for one case.
struct Ws {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    /// GLEN_HOME; never created by these cases.
    glen_home: PathBuf,
    /// PI_CODING_AGENT_DIR (created, empty).
    agent: PathBuf,
    tmp: PathBuf,
    bin: PathBuf,
    /// The project directory: glen's cwd, outside HOME.
    cwd: PathBuf,
}

impl Ws {
    fn new() -> Ws {
        let dir = tempfile::Builder::new()
            .prefix("glen-w4-")
            .tempdir()
            .expect("tempdir");
        let root = fs::canonicalize(dir.path()).expect("canonical temp root");
        let home = root.join("home");
        let agent = root.join("agent");
        let tmp = root.join("tmp");
        let bin = root.join("emptybin");
        let cwd = root.join("project");
        for d in [&home, &agent, &tmp, &bin, &cwd] {
            fs::create_dir(d).unwrap();
        }
        Ws {
            _dir: dir,
            glen_home: root.join("glen"),
            root,
            home,
            agent,
            tmp,
            bin,
            cwd,
        }
    }

    fn command(&self, program: &str, cwd: &Path, args: &[OsString]) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .args(args)
            .env("HOME", &self.home)
            .env("GLEN_HOME", &self.glen_home)
            .env("PI_CODING_AGENT_DIR", &self.agent)
            .env("TMPDIR", &self.tmp)
            .env("TZ", "UTC")
            .env("PATH", &self.bin)
            .current_dir(cwd)
            .stdin(Stdio::null());
        cmd
    }

    fn glen_in<I, S>(&self, cwd: &Path, args: I) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
        let desc = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        Run::from_command(self.command(BIN, cwd, &args), desc)
    }

    fn glen<I, S>(&self, args: I) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        self.glen_in(&self.cwd, args)
    }

    /// `glen <args>` under `umask <mask>`, through `/bin/sh` (the test process keeps its umask).
    fn glen_umask(&self, mask: &str, args: &[&str]) -> Run {
        let mut all: Vec<OsString> = vec![
            "-c".into(),
            "umask \"$1\" && shift && exec \"$0\" \"$@\"".into(),
            BIN.into(),
            mask.into(),
        ];
        all.extend(args.iter().map(OsString::from));
        Run::from_command(
            self.command("/bin/sh", &self.cwd, &all),
            format!("(umask {mask}) {}", args.join(" ")),
        )
    }

    fn base(&self, project: bool) -> &Path {
        if project { &self.cwd } else { &self.home }
    }

    /// `<base>/.claude` or `<base>/.agents`.
    fn top(&self, agent: &str, project: bool) -> PathBuf {
        self.base(project).join(if agent == "claude" {
            ".claude"
        } else {
            ".agents"
        })
    }

    /// The SKILL.md target of `agent` (AC-405).
    fn target(&self, agent: &str, project: bool) -> PathBuf {
        self.top(agent, project).join("skills/glen/SKILL.md")
    }

    fn user(&self, agent: &str) -> PathBuf {
        self.target(agent, false)
    }

    fn project(&self, agent: &str) -> PathBuf {
        self.target(agent, true)
    }

    fn rel(&self, p: &Path) -> String {
        s(p.strip_prefix(&self.root).unwrap())
    }

    fn tree(&self) -> BTreeSet<String> {
        list_tree(&self.root).into_iter().collect()
    }

    fn snap(&self) -> Vec<(String, u64, SystemTime)> {
        snapshot(&self.root)
    }

    fn place(&self, path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// Make `link` a symlink to a fresh SKILL.md elsewhere under the root; returns that file.
    fn place_link(&self, link: &Path, bytes: &[u8], tag: &str) -> PathBuf {
        let real = self.root.join(format!("elsewhere-{tag}/SKILL.md"));
        self.place(&real, bytes);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&real, link).unwrap();
        real
    }

    fn assert_no_side_effects(&self, ctx: &str) {
        assert!(
            !self.glen_home.exists(),
            "GLEN_HOME was created: {:?}\n{ctx}",
            list_tree(&self.glen_home)
        );
        assert!(
            is_empty_dir(&self.agent),
            "something was written under the omp agent dir: {:?}\n{ctx}",
            list_tree(&self.agent)
        );
        assert!(
            !self.home.join(".codex").exists(),
            "~/.codex was created\n{ctx}"
        );
        assert!(
            !self.home.join(".omp").exists(),
            "~/.omp was created\n{ctx}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The rendered skill, as the binary writes it.
// ---------------------------------------------------------------------------------------------

struct Rendering {
    claude: Vec<u8>,
    codex: Vec<u8>,
}

fn rendering() -> &'static Rendering {
    static R: OnceLock<Rendering> = OnceLock::new();
    R.get_or_init(|| {
        let ws = Ws::new();
        let out = ws.glen(["install-skill", "--json"]).run();
        skill_env(&out);
        Rendering {
            claude: fs::read(ws.user("claude")).expect("claude SKILL.md written"),
            codex: fs::read(ws.user("codex")).expect("codex SKILL.md written"),
        }
    })
}

fn rendered(agent: &str) -> &'static [u8] {
    match agent {
        "claude" => &rendering().claude,
        _ => &rendering().codex,
    }
}

fn rendered_text(agent: &str) -> &'static str {
    std::str::from_utf8(rendered(agent)).expect("SKILL.md is UTF-8")
}

fn other(agent: &str) -> &'static str {
    if agent == "claude" { "codex" } else { "claude" }
}

/// Line index of the marker: the line after the closing `---` of the frontmatter.
fn marker_index(text: &str) -> usize {
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.first(),
        Some(&"---"),
        "SKILL.md must begin with a `---` line"
    );
    let close = lines
        .iter()
        .skip(1)
        .position(|l| *l == "---")
        .expect("frontmatter has a closing `---`")
        + 1;
    close + 1
}

/// `(version, agent, digest)`, asserting the exact AC-402 marker form.
fn marker(text: &str) -> (String, String, String) {
    let line = text
        .lines()
        .nth(marker_index(text))
        .expect("a line after the frontmatter");
    let inner = line
        .strip_prefix("<!-- glen-skill ")
        .and_then(|r| r.strip_suffix(" -->"))
        .unwrap_or_else(|| panic!("not a glen-skill marker: {line:?}"));
    let parts: Vec<&str> = inner.split(' ').collect();
    assert_eq!(parts.len(), 3, "marker fields: {line:?}");
    let field = |i: usize, key: &str| {
        parts[i]
            .strip_prefix(key)
            .unwrap_or_else(|| panic!("marker field {i} is not `{key}…`: {line:?}"))
            .to_owned()
    };
    (
        field(0, "version="),
        field(1, "agent="),
        field(2, "digest="),
    )
}

/// The file without the marker line and its newline: the digest input (AC-402).
fn without_marker(text: &str) -> String {
    let idx = marker_index(text);
    text.split_inclusive('\n')
        .enumerate()
        .filter(|(i, _)| *i != idx)
        .map(|(_, l)| l)
        .collect()
}

fn digest_of(text: &str) -> String {
    format!("{:016x}", xxh64(without_marker(text).as_bytes(), 0))
}

/// Everything after the marker line.
fn body(text: &str) -> String {
    let idx = marker_index(text);
    text.split_inclusive('\n').skip(idx + 1).collect()
}

fn remark(text: &str, version: &str, agent: &str, digest: &str) -> String {
    let idx = marker_index(text);
    text.split_inclusive('\n')
        .enumerate()
        .map(|(i, l)| {
            if i == idx {
                format!("<!-- glen-skill version={version} agent={agent} digest={digest} -->\n")
            } else {
                l.to_owned()
            }
        })
        .collect()
}

/// `text` with the marker's version set to `version` and a digest that matches its content.
fn sealed(text: &str, version: &str) -> Vec<u8> {
    let (_, agent, _) = marker(text);
    let draft = remark(text, version, &agent, "0000000000000000");
    let d = digest_of(&draft);
    remark(&draft, version, &agent, &d).into_bytes()
}

fn with_extra_body(text: &str) -> String {
    format!("{text}A line from another build.\n")
}

/// The pre-run file states of AC-406, by construction.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pre {
    Absent,
    Current,
    /// Older version, same body, intact digest.
    Older,
    /// Older version, different body, intact digest.
    OlderBody,
    /// Older version, body edited, digest left stale (cycle 1: `modified`).
    OlderEdited,
    /// Same version, different body, intact digest (another build).
    OtherBuild,
    /// Newer version, intact digest.
    Newer,
    /// Same version, body edited, digest left stale.
    Modified,
    /// No marker at all.
    Foreign,
    /// The other agent's rendering.
    OtherAgent,
    /// `SKILL.md` is a symlink to an identical copy.
    LinkedFile,
}

fn bytes_for(pre: Pre, agent: &str) -> Vec<u8> {
    let r = rendered_text(agent);
    match pre {
        Pre::Absent => Vec::new(),
        Pre::Current | Pre::LinkedFile => r.as_bytes().to_vec(),
        Pre::Older => sealed(r, OLDER),
        Pre::OlderBody => sealed(&with_extra_body(r), OLDER),
        Pre::OlderEdited => {
            with_extra_body(std::str::from_utf8(&sealed(r, OLDER)).unwrap()).into_bytes()
        }
        Pre::OtherBuild => sealed(&with_extra_body(r), PKG_VERSION),
        Pre::Newer => sealed(r, NEWER),
        Pre::Modified => with_extra_body(r).into_bytes(),
        Pre::Foreign => {
            b"---\nname: glen\ndescription: \"my own notes\"\n---\n# glen notes\n".to_vec()
        }
        Pre::OtherAgent => rendered(other(agent)).to_vec(),
    }
}

/// Put `pre` at `path`; returns the symlink's real file for `LinkedFile`.
fn put(ws: &Ws, path: &Path, pre: Pre, agent: &str) -> Option<PathBuf> {
    match pre {
        Pre::Absent => None,
        Pre::LinkedFile => Some(ws.place_link(path, &bytes_for(pre, agent), agent)),
        _ => {
            ws.place(path, &bytes_for(pre, agent));
            None
        }
    }
}

// ---------------------------------------------------------------------------------------------
// install-skill envelopes (AC-407).
// ---------------------------------------------------------------------------------------------

fn keyset(v: &Value) -> BTreeSet<String> {
    v.as_object()
        .unwrap_or_else(|| panic!("expected an object, got {v}"))
        .keys()
        .cloned()
        .collect()
}

fn set(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| (*k).to_owned()).collect()
}

/// One `ok:true` envelope, exit 0, nothing on stderr, and the documented `data` shape.
fn skill_env(out: &Out) -> Value {
    let env = out.ok_envelope();
    out.assert_stderr_empty().assert_no_ansi();
    let data = &env["data"];
    assert_eq!(
        keyset(data),
        set(&["version", "dry_run", "scope", "targets"]),
        "data keys\n{}",
        out.ctx()
    );
    assert_eq!(data["version"], PKG_VERSION, "{}", out.ctx());
    assert!(data["dry_run"].is_boolean(), "{}", out.ctx());
    assert!(
        data["scope"] == "user" || data["scope"] == "project",
        "{}",
        out.ctx()
    );
    let targets = data["targets"].as_array().expect("targets array");
    assert!(!targets.is_empty(), "{}", out.ctx());
    let mut agents = Vec::new();
    for t in targets {
        assert_eq!(
            keyset(t),
            set(&["agent", "path", "action", "state", "installed_version"]),
            "target keys\n{}",
            out.ctx()
        );
        let agent = t["agent"].as_str().expect("agent");
        assert!(AGENTS.contains(&agent), "{}", out.ctx());
        agents.push(agent.to_owned());
        let action = t["action"].as_str().expect("action");
        let state = t["state"].as_str().expect("state");
        assert!(ACTIONS.contains(&action), "undocumented action {action}");
        assert!(STATES.contains(&state), "undocumented state {state}");
        assert!(
            t["installed_version"].is_null() || t["installed_version"].is_string(),
            "{}",
            out.ctx()
        );
        let p = t["path"].as_str().expect("path");
        assert!(Path::new(p).is_absolute(), "JSON paths are absolute: {p}");
    }
    let mut sorted = agents.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(agents, sorted, "targets are claude then codex, once each");
    env
}

fn tgt<'a>(data: &'a Value, agent: &str) -> &'a Value {
    data["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["agent"] == agent)
        .unwrap_or_else(|| panic!("no {agent} target: {data}"))
}

fn agents_of(data: &Value) -> Vec<String> {
    data["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["agent"].as_str().unwrap().to_owned())
        .collect()
}

/// A reported path names `expected` (lexically, or through followed links).
fn assert_path_is(v: &Value, expected: &Path, ctx: &str) {
    let p = Path::new(v.as_str().expect("path string"));
    assert!(
        lexical(p) == expected || (p.exists() && reader::same_file(p, expected)),
        "path {} is not {}\n{ctx}",
        p.display(),
        expected.display()
    );
}

/// `glen install-skill --<agent> <extra> --json` → envelope.
fn install(ws: &Ws, agent: &str, extra: &[&str]) -> Value {
    let flag = format!("--{agent}");
    let args = args_of(&["install-skill", &flag], extra);
    let mut args = args;
    args.push("--json".into());
    skill_env(&ws.glen(&args).run())
}

fn assert_target(env: &Value, agent: &str, action: &str, state: &str, version: Option<&str>) {
    let t = tgt(&env["data"], agent);
    assert_eq!(t["action"], action, "{agent} action: {env}");
    assert_eq!(t["state"], state, "{agent} state: {env}");
    if let Some(v) = version {
        assert_eq!(
            t["installed_version"], v,
            "{agent} installed_version: {env}"
        );
    }
}

fn assert_warning(env: &Value, code: &str) {
    assert!(
        warning_codes(env).iter().any(|c| c == code),
        "expected warning {code}: {env}"
    );
}

// ---------------------------------------------------------------------------------------------
// The command surface, as `--help` prints it.
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
struct Opt {
    names: Vec<String>,
    takes_value: bool,
    help: String,
}

fn bracket(text: &str, open: &str) -> Option<String> {
    let i = text.find(open)?;
    let rest = &text[i + open.len()..];
    let j = rest.find(']')?;
    Some(rest[..j].to_owned())
}

impl Opt {
    fn possible(&self) -> Vec<String> {
        bracket(&self.help, "[possible values: ")
            .map(|l| l.split(',').map(|x| x.trim().to_owned()).collect())
            .unwrap_or_default()
    }

    fn default(&self) -> Option<String> {
        bracket(&self.help, "[default: ")
    }

    /// The help text without clap's bracketed annotations.
    fn description(&self) -> String {
        let mut d = self.help.clone();
        for open in ["[default: ", "[possible values: ", "[aliases: ", "[env: "] {
            while let Some(i) = d.find(open) {
                let end = d[i..].find(']').map_or(d.len(), |j| i + j + 1);
                d.replace_range(i..end, "");
            }
        }
        d.trim().to_owned()
    }
}

struct Page {
    options: Vec<Opt>,
    positionals: Vec<Opt>,
    text: String,
}

impl Page {
    fn opt(&self, name: &str) -> Option<&Opt> {
        self.options
            .iter()
            .find(|o| o.names.iter().any(|n| n == name))
    }
}

struct Cli {
    /// Visible subcommands as `glen --help` lists them (help included).
    commands: Vec<String>,
    /// Visible alias → command.
    aliases: BTreeMap<String, String>,
    top: Page,
    pages: BTreeMap<String, Page>,
}

fn section<'a>(text: &'a str, heading: &str) -> Vec<&'a str> {
    text.lines()
        .skip_while(|l| l.trim_end() != heading)
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .collect()
}

fn parse_entries(lines: &[&str]) -> Vec<Opt> {
    let mut out: Vec<Opt> = Vec::new();
    for l in lines {
        let t = l.trim_start();
        let indent = l.len() - t.len();
        if indent <= 6 && (t.starts_with('-') || t.starts_with('<') || t.starts_with('[')) {
            let (spec, help) = match t.find("  ") {
                Some(i) => (&t[..i], t[i..].trim()),
                None => (t, ""),
            };
            let names = spec
                .split(", ")
                .map(|p| p.split([' ', '=']).next().unwrap().to_owned())
                .collect();
            out.push(Opt {
                names,
                takes_value: spec.starts_with('-') && spec.contains('<'),
                help: help.to_owned(),
            });
        } else if let Some(last) = out.last_mut() {
            if !last.help.is_empty() {
                last.help.push(' ');
            }
            last.help.push_str(t.trim());
        }
    }
    out
}

/// Primary names and visible aliases from the `Commands:` section.
fn parse_commands(help: &str) -> (Vec<String>, BTreeMap<String, String>) {
    let mut cmds: Vec<String> = Vec::new();
    let mut abouts: Vec<String> = Vec::new();
    for l in section(help, "Commands:") {
        let t = l.trim_start();
        if l.len() - t.len() <= 2 {
            let (name, rest) = t.split_once(' ').unwrap_or((t, ""));
            cmds.push(name.to_owned());
            abouts.push(rest.trim().to_owned());
        } else if let Some(a) = abouts.last_mut() {
            a.push(' ');
            a.push_str(t.trim());
        }
    }
    let mut aliases = BTreeMap::new();
    for (c, about) in cmds.iter().zip(&abouts) {
        for open in ["[aliases: ", "[alias: "] {
            if let Some(list) = bracket(about, open) {
                for a in list.split(',') {
                    aliases.insert(a.trim().to_owned(), c.clone());
                }
            }
        }
    }
    (cmds, aliases)
}

fn command_names(help: &str) -> Vec<String> {
    let mut v = parse_commands(help).0;
    v.sort();
    v
}

fn cli() -> &'static Cli {
    static CLI: OnceLock<Cli> = OnceLock::new();
    CLI.get_or_init(|| {
        let sb = Sandbox::new();
        let help = |args: &[&str]| {
            let out = sb.glen(args).run();
            out.assert_exit(0);
            out.out()
        };
        let top_text = help(&["--help"]);
        let (commands, aliases) = parse_commands(&top_text);
        let top = Page {
            options: parse_entries(&section(&top_text, "Options:")),
            positionals: Vec::new(),
            text: top_text.clone(),
        };
        let mut pages = BTreeMap::new();
        for c in commands.iter().filter(|c| *c != "help") {
            let text = help(&[c.as_str(), "--help"]);
            pages.insert(
                c.clone(),
                Page {
                    options: parse_entries(&section(&text, "Options:")),
                    positionals: parse_entries(&section(&text, "Arguments:")),
                    text,
                },
            );
        }
        Cli {
            commands,
            aliases,
            top,
            pages,
        }
    })
}

// ---------------------------------------------------------------------------------------------
// The skill checker (AC-404, O6): every `glen …` in a code span or block against `--help`.
// ---------------------------------------------------------------------------------------------

/// Code-span contents and fenced-code lines.
fn code_snippets(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut prose = String::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            prose.push('\n');
            continue;
        }
        if fenced {
            out.push(line.trim().trim_start_matches("$ ").to_owned());
        } else {
            prose.push_str(line);
            prose.push('\n');
        }
    }
    let b = prose.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'`' {
            i += 1;
            continue;
        }
        let mut n = 0;
        while i + n < b.len() && b[i + n] == b'`' {
            n += 1;
        }
        let start = i + n;
        let fence = "`".repeat(n);
        match prose[start..].find(&fence) {
            Some(rel) => {
                out.push(
                    prose[start..start + rel]
                        .replace('\n', " ")
                        .trim()
                        .to_owned(),
                );
                i = start + rel + n;
            }
            None => i = start,
        }
    }
    out
}

fn invocations(text: &str) -> Vec<String> {
    code_snippets(text)
        .into_iter()
        .filter(|s| s == "glen" || s.starts_with("glen "))
        .collect()
}

/// Shell-like words: quotes group, `<word>` is a placeholder, `| ; & > <` are operators.
fn shell_words(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' | '\'' => {
                has = true;
                i += 1;
                while i < chars.len() && chars[i] != c {
                    cur.push(chars[i]);
                    i += 1;
                }
            }
            c if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            '|' | ';' | '&' | '>' if !has => out.push(c.to_string()),
            '<' if !has => {
                let word: String = chars[i..]
                    .iter()
                    .take_while(|ch| !ch.is_whitespace())
                    .collect();
                if word.chars().count() > 2 && word.ends_with('>') {
                    cur.push_str(&word);
                    has = true;
                    i += word.chars().count() - 1;
                } else {
                    out.push("<".into());
                }
            }
            _ => {
                cur.push(c);
                has = true;
            }
        }
        i += 1;
    }
    if has {
        out.push(cur);
    }
    out
}

fn is_operator(t: &str) -> bool {
    ["|", ";", "&", ">", "<"].contains(&t)
}

fn is_flag(t: &str) -> bool {
    t.starts_with('-') && t.len() > 1 && t != "--"
}

fn is_placeholder(v: &str) -> bool {
    (v.starts_with('<') && v.ends_with('>'))
        || v.contains('…')
        || v.contains("...")
        || (!v.is_empty() && v.chars().all(|c| c.is_ascii_uppercase() || c == '_'))
}

fn split_flag(t: &str) -> (&str, Option<&str>) {
    match t.split_once('=') {
        Some((n, v)) if t.starts_with("--") => (n, Some(v)),
        _ => (t, None),
    }
}

fn check_value(inv: &str, name: &str, o: &Opt, value: Option<&str>, errs: &mut Vec<String>) {
    let poss = o.possible();
    if poss.is_empty() {
        return;
    }
    match value {
        None => errs.push(format!("`{inv}`: `{name}` has no value")),
        Some(v) if is_placeholder(v) => {}
        Some(v) if !poss.iter().any(|p| p == v) => errs.push(format!(
            "`{inv}`: `{v}` is not a possible value of `{name}` ({poss:?})"
        )),
        Some(_) => {}
    }
}

fn check_invocation(inv: &str, cli: &Cli, used: &mut BTreeSet<String>, errs: &mut Vec<String>) {
    let mut toks = shell_words(inv);
    if let Some(p) = toks.iter().position(|t| is_operator(t)) {
        toks.truncate(p);
    }
    let mut i = 1;
    while i < toks.len() && is_flag(&toks[i]) {
        let (name, inline) = split_flag(&toks[i]);
        match cli.top.opt(name) {
            None => errs.push(format!("`{inv}`: `{name}` is not a global flag of glen")),
            Some(o) if o.takes_value => {
                let value = match inline {
                    Some(v) => Some(v.to_owned()),
                    None => {
                        i += 1;
                        toks.get(i).cloned()
                    }
                };
                check_value(inv, name, o, value.as_deref(), errs);
            }
            Some(_) => {}
        }
        i += 1;
    }
    let Some(sub_tok) = toks.get(i) else { return };
    let sub = cli
        .aliases
        .get(sub_tok)
        .cloned()
        .unwrap_or_else(|| sub_tok.clone());
    if !cli.commands.contains(&sub) {
        errs.push(format!(
            "`{inv}`: `{sub_tok}` is not a subcommand `glen --help` lists"
        ));
        return;
    }
    used.insert(sub_tok.clone());
    if sub == "help" {
        return;
    }
    let page = &cli.pages[&sub];
    let mut pos = 0;
    i += 1;
    while i < toks.len() {
        let t = toks[i].clone();
        if t == "--" {
            break;
        }
        if is_flag(&t) {
            let (name, inline) = split_flag(&t);
            match page.opt(name) {
                None => errs.push(format!("`{inv}`: `{name}` is not a flag of `glen {sub}`")),
                Some(o) if o.takes_value => {
                    let value = match inline {
                        Some(v) => Some(v.to_owned()),
                        None => {
                            i += 1;
                            toks.get(i).cloned()
                        }
                    };
                    check_value(inv, name, o, value.as_deref(), errs);
                }
                Some(_) => {}
            }
        } else {
            if let Some(p) = page.positionals.get(pos) {
                check_value(inv, &p.names[0], p, Some(t.as_str()), errs);
            }
            pos += 1;
        }
        i += 1;
    }
}

/// Invocations, flags, values and coverage (AC-404 first three clauses).
fn check_invocations(text: &str) -> Vec<String> {
    let cli = cli();
    let mut errs = Vec::new();
    let mut used = BTreeSet::new();
    for inv in invocations(text) {
        check_invocation(&inv, cli, &mut used, &mut errs);
    }
    for c in cli.commands.iter().filter(|c| *c != "help") {
        if !used.contains(c) {
            errs.push(format!(
                "visible subcommand `{c}` is never invoked in the skill"
            ));
        }
    }
    for a in cli.aliases.keys() {
        if !used.contains(a) {
            errs.push(format!("visible alias `{a}` is never invoked in the skill"));
        }
    }
    errs
}

const OK_EXITS: [i32; 4] = [0, 10, 11, 130];

/// `(exit, backticked codes)` rows of the table under `## Exit codes`.
fn exit_rows(text: &str, heading: &str, code_col: usize) -> Vec<(i32, Vec<String>, Vec<String>)> {
    text.lines()
        .skip_while(|l| l.trim() != heading)
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .filter(|l| l.starts_with('|'))
        .filter_map(|l| {
            let cols: Vec<String> = l.split('|').map(|c| c.trim().to_owned()).collect();
            let exit = cols.get(1)?.parse::<i32>().ok()?;
            let codes = cols
                .get(code_col)
                .map(|c| {
                    c.split('`')
                        .enumerate()
                        .filter(|(i, p)| i % 2 == 1 && !p.is_empty())
                        .map(|(_, p)| p.to_owned())
                        .collect()
                })
                .unwrap_or_default();
            Some((exit, codes, cols))
        })
        .collect()
}

/// The skill's exit table against design §A11 and the ok exits (AC-404 fourth clause).
fn check_exit_table(text: &str) -> Vec<String> {
    let mut errs = Vec::new();
    let rows = exit_rows(text, "## Exit codes", 2);
    if rows.is_empty() {
        errs.push("no exit-code table under `## Exit codes`".to_owned());
    }
    let design = design_exit_table();
    let mut seen = BTreeSet::new();
    for (exit, codes, _) in &rows {
        for c in codes {
            match design.get(c) {
                Some(e) if e == exit => {}
                Some(e) => errs.push(format!(
                    "exit table: `{c}` is exit {exit}, docs/design.md §A11 says {e}"
                )),
                None => errs.push(format!("exit table: `{c}` is not a §A11 code")),
            }
            seen.insert(c.clone());
        }
    }
    for (c, e) in design {
        if !OK_EXITS.contains(e) && !seen.contains(c) {
            errs.push(format!("exit table lacks `{c}` (exit {e})"));
        }
    }
    for ok in OK_EXITS {
        if !rows.iter().any(|(e, _, _)| *e == ok) {
            errs.push(format!("exit table lacks the ok exit {ok}"));
        }
    }
    errs
}

/// Numbers the skill states as a default (`default 20`, `default to 90 s`, `defaults: 10`).
fn skill_defaults(text: &str) -> Vec<(u64, String)> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for (i, _) in lower.match_indices("default") {
        let mut rest = &lower[i + "default".len()..];
        rest = rest.strip_prefix('s').unwrap_or(rest);
        rest = rest.trim_start_matches([' ', ':', '=']);
        for w in ["to ", "is "] {
            if let Some(r) = rest.strip_prefix(w) {
                rest = r;
            }
        }
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<u64>() {
            let ctx: String = lower[i..].chars().take(30).collect();
            out.push((n, ctx));
        }
    }
    out
}

/// Numeric `[default: …]` values the visible commands' `--help` prints.
fn help_defaults() -> BTreeSet<u64> {
    cli()
        .pages
        .values()
        .flat_map(|p| p.options.iter())
        .filter_map(|o| o.default())
        .filter_map(|d| {
            let digits: String = d.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().ok()
        })
        .collect()
}

/// O6: every default the skill states is one clap prints.
fn check_defaults(text: &str) -> Vec<String> {
    let known = help_defaults();
    skill_defaults(text)
        .into_iter()
        .filter(|(n, _)| !known.contains(n))
        .map(|(n, ctx)| {
            format!(
                "the skill gives {n} as a default (`{ctx}`); no --help default is {n} ({known:?})"
            )
        })
        .collect()
}

fn check_skill(text: &str) -> Vec<String> {
    let mut errs = check_invocations(text);
    errs.extend(check_exit_table(text));
    errs.extend(check_defaults(text));
    errs
}

// ---------------------------------------------------------------------------------------------
// AC-401 — the command surface after W4.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac401_help_lists_install_skill_with_every_earlier_visible_command() {
    let sb = Sandbox::new();
    for args in [vec!["--help"], vec!["-h"], vec!["help"]] {
        let out = sb.glen(&args).run();
        out.assert_exit(0);
        assert_eq!(
            command_names(&out.out()),
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
                "wait",
                "watch"
            ],
            "{}",
            out.ctx()
        );
    }
}

/// Watch is visible; on a pipe its interactive-terminal gate returns usage.
/// `inbox` and `install-extension` became visible with W3.
#[test]
fn ac401_watch_is_visible_and_requires_an_interactive_terminal() {
    let sb = Sandbox::new();
    let listed = command_names(&sb.glen(["--help"]).run().out());
    assert!(listed.iter().any(|name| name == "watch"), "{listed:?}");
    for name in HIDDEN {
        assert!(
            !listed.iter().any(|c| c == name),
            "{name} is listed: {listed:?}"
        );
        let out = sb.glen([name]).run();
        out.error_envelope("not_implemented");
        out.assert_exit(8).assert_stderr_empty();
    }
    sb.glen(["watch", "abc1234"]).run().error_envelope("usage");
    assert!(!sb.glen_home.exists());
}

#[test]
fn ac401_worker_appears_in_no_help_page_and_no_completion_script() {
    let sb = Sandbox::new();
    let mut pages: Vec<Vec<String>> = vec![
        vec!["--help".into()],
        vec!["-h".into()],
        vec!["help".into()],
    ];
    for c in cli().commands.iter().filter(|c| *c != "help") {
        pages.push(vec![c.clone(), "--help".into()]);
        pages.push(vec!["help".into(), c.clone()]);
    }
    for c in HIDDEN {
        pages.push(vec![c.into(), "--help".into()]);
    }
    for args in &pages {
        let out = sb.glen(args).run();
        out.assert_exit(0);
        assert!(
            !out.out().contains("__worker") && !out.err().contains("__worker"),
            "{}",
            out.ctx()
        );
    }
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let raw = sb.glen(["completions", shell]).run();
        raw.assert_exit(0);
        assert!(!raw.out().contains("__worker"), "{shell} exposes __worker");
        assert!(
            raw.out().contains("install-skill"),
            "{shell} script lacks install-skill"
        );
        let data = sb
            .glen(["completions", shell, "--json"])
            .run()
            .ok_data_or_spilled(&sb.glen_home);
        assert!(!data["script"].as_str().unwrap().contains("__worker"));
    }
}

// ---------------------------------------------------------------------------------------------
// AC-402 — frontmatter, marker, digest, identical bodies.
// ---------------------------------------------------------------------------------------------

fn frontmatter(text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let close = marker_index(text) - 1;
    lines[1..close]
        .iter()
        .map(|l| {
            let (k, v) = l
                .split_once(": ")
                .unwrap_or_else(|| panic!("frontmatter line is not `key: value`: {l:?}"));
            (k.to_owned(), v.to_owned())
        })
        .collect()
}

fn fm_get<'a>(fm: &'a [(String, String)], key: &str) -> &'a str {
    &fm.iter()
        .find(|(k, _)| k == key)
        .unwrap_or_else(|| panic!("no frontmatter key {key}: {fm:?}"))
        .1
}

fn assert_description(raw: &str, agent: &str) {
    assert!(
        raw.starts_with('"'),
        "{agent}: description is a quoted JSON string: {raw}"
    );
    let d: String = serde_json::from_str(raw)
        .unwrap_or_else(|e| panic!("{agent}: description is not one JSON string ({e}): {raw}"));
    assert!(
        !d.contains('\n') && !d.contains('\r'),
        "{agent}: description must be one line"
    );
    assert!(
        d.chars().count() <= 1024,
        "{agent}: description is {} characters",
        d.chars().count()
    );
    assert!(d.contains("omp"), "{agent}: description must name omp: {d}");
}

fn assert_marker(text: &str, agent: &str) {
    let (version, a, digest) = marker(text);
    assert_eq!(version, PKG_VERSION, "{agent}: marker version");
    assert_eq!(a, agent, "{agent}: marker agent");
    assert!(
        digest.len() == 16
            && digest
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "{agent}: digest is 16 lowercase hex: {digest}"
    );
    assert_eq!(
        digest,
        digest_of(text),
        "{agent}: digest is xxh64(seed 0) of the file without its marker line"
    );
}

#[test]
fn ac402_claude_variant_frontmatter_and_marker() {
    let text = rendered_text("claude");
    assert!(text.starts_with("---\n"), "line 1 must be `---`");
    let fm = frontmatter(text);
    let keys: Vec<&str> = fm.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys.iter().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from(["allowed-tools", "description", "name"]),
        "claude frontmatter keys: {keys:?}"
    );
    assert_eq!(keys.len(), 3, "no duplicate keys: {keys:?}");
    assert_eq!(fm_get(&fm, "name"), "glen");
    assert_eq!(fm_get(&fm, "allowed-tools"), "Bash(glen *)", "O7 rule form");
    assert_description(fm_get(&fm, "description"), "claude");
    assert_marker(text, "claude");
}

#[test]
fn ac402_codex_variant_frontmatter_and_marker() {
    let text = rendered_text("codex");
    assert!(text.starts_with("---\n"), "line 1 must be `---`");
    let fm = frontmatter(text);
    let keys: Vec<&str> = fm.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys.len(), 2, "codex frontmatter keys: {keys:?}");
    assert_eq!(
        keys.iter().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from(["description", "name"]),
        "codex frontmatter keys: {keys:?}"
    );
    assert_eq!(fm_get(&fm, "name"), "glen");
    assert_description(fm_get(&fm, "description"), "codex");
    assert_marker(text, "codex");
}

#[test]
fn ac402_both_variants_have_byte_identical_bodies() {
    let c = body(rendered_text("claude"));
    let x = body(rendered_text("codex"));
    assert!(!c.trim().is_empty());
    assert_eq!(c, x, "the claude and codex bodies differ");
}

/// "The rendering" is one file per agent and build: the same bytes in every scope.
#[test]
fn ac402_the_rendering_is_the_same_in_every_scope() {
    let ws = Ws::new();
    skill_env(&ws.glen(["install-skill", "--project", "--json"]).run());
    let cfg = ws.root.join("cfg");
    skill_env(
        &ws.glen(["install-skill", "--claude", "--json"])
            .env("CLAUDE_CONFIG_DIR", &cfg)
            .run(),
    );
    for agent in AGENTS {
        assert_eq!(
            fs::read(ws.project(agent)).unwrap(),
            rendered(agent),
            "{agent}"
        );
    }
    assert_eq!(
        fs::read(cfg.join("skills/glen/SKILL.md")).unwrap(),
        rendered("claude")
    );
}

// ---------------------------------------------------------------------------------------------
// AC-403 — guards leave no trace.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac403_rendered_variants_carry_no_guard_text_and_no_blank_line_runs() {
    for agent in AGENTS {
        let t = rendered_text(agent);
        assert!(
            !t.contains("glen:if") && !t.contains("glen:end") && !t.contains("<!-- glen:"),
            "{agent}: guard text survived rendering"
        );
        let lines: Vec<&str> = t.lines().collect();
        for (i, w) in lines.windows(2).enumerate() {
            assert!(
                !(w[0].trim().is_empty() && w[1].trim().is_empty()),
                "{agent}: two blank lines at line {}",
                i + 1
            );
        }
        // Material for commands this build hides is dropped: no invocation of them survives.
        for inv in invocations(t) {
            let words = shell_words(&inv);
            let sub = words.get(1).map(String::as_str).unwrap_or("");
            assert!(
                !HIDDEN.contains(&sub) && sub != "__worker",
                "{agent}: hidden command invoked: `{inv}`"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// AC-404 — the skill against the real CLI; O6 numeric defaults.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac404_every_glen_invocation_names_real_commands_flags_and_values() {
    for agent in AGENTS {
        let t = rendered_text(agent);
        assert!(
            invocations(t).len() >= cli().commands.len() - 1,
            "{agent}: too few glen invocations found"
        );
        let errs = check_invocations(t);
        assert!(errs.is_empty(), "{agent} variant:\n{}", errs.join("\n"));
    }
}

#[test]
fn ac404_the_skill_exit_table_matches_design_and_the_ok_exits() {
    for agent in AGENTS {
        let errs = check_exit_table(rendered_text(agent));
        assert!(errs.is_empty(), "{agent} variant:\n{}", errs.join("\n"));
    }
}

/// O6: the defaults `--help` prints match design §A9, and every default the skill states is one
/// of them.
#[test]
fn o6_numeric_defaults_in_help_and_skill_agree_with_design() {
    let cli = cli();
    for (cmd, flag, want) in [
        ("send", "--wait", "90s"),
        ("wait", "--timeout", "90s"),
        ("ls", "-n", "20"),
        ("jobs", "-n", "20"),
        ("tail", "-n", "10"),
        ("show", "--tools", "collapsed"),
        ("send", "--as", "auto"),
    ] {
        let o = cli.pages[cmd]
            .opt(flag)
            .unwrap_or_else(|| panic!("glen {cmd} has no {flag}"));
        assert_eq!(o.default().as_deref(), Some(want), "glen {cmd} {flag}");
    }
    for agent in AGENTS {
        let t = rendered_text(agent);
        assert!(
            !skill_defaults(t).is_empty(),
            "{agent}: the skill states no defaults"
        );
        let errs = check_defaults(t);
        assert!(errs.is_empty(), "{agent} variant:\n{}", errs.join("\n"));
    }
}

/// The checker above is itself mutation-tested (AC-404's last clause, applied to this suite's
/// own checker so the passing cases are never vacuous).
#[test]
fn ac404_the_conformance_skill_checker_catches_each_mutation() {
    let base = rendered_text("claude");
    assert!(check_skill(base).is_empty(), "{:?}", check_skill(base));
    let row = base
        .lines()
        .find(|l| l.starts_with('|') && l.contains("`not_found`"))
        .expect("a not_found exit row");
    let mut cols: Vec<&str> = row.split('|').collect();
    cols[1] = " 9 ";
    let bad_row = cols.join("|");
    let mutations: Vec<(&str, String, &str)> = vec![
        (
            "unknown command",
            format!("{base}\n`glen frobnicate`\n"),
            "frobnicate",
        ),
        (
            "unknown flag",
            format!("{base}\n`glen ls --frobnicate`\n"),
            "--frobnicate",
        ),
        (
            "flag moved to the wrong command",
            format!("{base}\n`glen ls --steer`\n"),
            "--steer",
        ),
        (
            "value outside the possible values",
            format!("{base}\n`glen show <handle> --tools everything`\n"),
            "everything",
        ),
        (
            "hidden command",
            format!("{base}\n`glen __worker`\n"),
            "__worker",
        ),
        (
            "removed visible-command reference",
            base.replace("glen doctor", "glen ls"),
            "doctor",
        ),
        ("wrong exit row", base.replace(row, &bad_row), "not_found"),
        (
            "wrong default",
            format!("{base}\nSends wait (default 45 s).\n"),
            "45",
        ),
    ];
    for (what, text, token) in mutations {
        let errs = check_skill(&text);
        assert!(
            errs.iter().any(|e| e.contains(token)),
            "{what}: no error names `{token}`: {errs:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// AC-405, AC-406 — targets and actions across every flag combination.
// ---------------------------------------------------------------------------------------------

fn combo(mask: u32) -> Vec<&'static str> {
    FLAGS
        .iter()
        .enumerate()
        .filter(|(i, _)| mask & (1 << i) != 0)
        .map(|(_, f)| *f)
        .collect()
}

/// `--claude` and `--codex` select one or both; neither means both.
fn selected(flags: &[&str]) -> Vec<&'static str> {
    match (flags.contains(&"--claude"), flags.contains(&"--codex")) {
        (true, false) => vec!["claude"],
        (false, true) => vec!["codex"],
        _ => vec!["claude", "codex"],
    }
}

fn chain(ws: &Ws, agent: &str, project: bool) -> Vec<PathBuf> {
    let top = ws.top(agent, project);
    vec![
        top.clone(),
        top.join("skills"),
        top.join("skills/glen"),
        top.join("skills/glen/SKILL.md"),
    ]
}

fn run_combo(ws: &Ws, flags: &[&str]) -> (Value, String) {
    let mut args = vec!["install-skill"];
    args.extend(flags);
    args.push("--json");
    let out = ws.glen(&args).run();
    let env = skill_env(&out);
    (env, out.ctx())
}

#[test]
fn ac405_ac406_every_flag_combination_on_an_empty_home() {
    for mask in 0..64 {
        let flags = combo(mask);
        let ws = Ws::new();
        let before = ws.snap();
        let before_paths = ws.tree();
        let (env, ctx) = run_combo(&ws, &flags);
        let data = &env["data"];
        let project = flags.contains(&"--project");
        let dry = flags.contains(&"--dry-run");
        let uninstall = flags.contains(&"--uninstall");
        let sel = selected(&flags);
        assert_eq!(
            data["scope"],
            if project { "project" } else { "user" },
            "{ctx}"
        );
        assert_eq!(data["dry_run"], dry, "{ctx}");
        assert_eq!(agents_of(data), sel, "{ctx}");
        for agent in &sel {
            let t = tgt(data, agent);
            assert_path_is(&t["path"], &ws.target(agent, project), &ctx);
            assert_eq!(t["state"], "not_installed", "{ctx}");
            assert!(t["installed_version"].is_null(), "{ctx}");
            let want = if uninstall {
                "not_installed"
            } else {
                "installed"
            };
            assert_eq!(t["action"], want, "{ctx}");
        }
        if dry || uninstall {
            assert_eq!(ws.snap(), before, "nothing may change\n{ctx}");
        } else {
            let new: BTreeSet<String> = ws.tree().difference(&before_paths).cloned().collect();
            let want: BTreeSet<String> = sel
                .iter()
                .flat_map(|a| chain(&ws, a, project))
                .map(|p| ws.rel(&p))
                .collect();
            assert_eq!(
                new, want,
                "the command writes the selected targets only\n{ctx}"
            );
            for agent in &sel {
                assert_eq!(
                    fs::read(ws.target(agent, project)).unwrap(),
                    rendered(agent),
                    "{ctx}"
                );
            }
        }
        ws.assert_no_side_effects(&ctx);
    }
}

#[test]
fn ac405_ac406_every_flag_combination_over_current_installs() {
    for mask in 0..64 {
        let flags = combo(mask);
        let ws = Ws::new();
        for agent in AGENTS {
            ws.place(&ws.user(agent), rendered(agent));
            ws.place(&ws.project(agent), rendered(agent));
        }
        let before = ws.snap();
        let (env, ctx) = run_combo(&ws, &flags);
        let data = &env["data"];
        let project = flags.contains(&"--project");
        let dry = flags.contains(&"--dry-run");
        let uninstall = flags.contains(&"--uninstall");
        let sel = selected(&flags);
        assert_eq!(agents_of(data), sel, "{ctx}");
        for agent in &sel {
            let t = tgt(data, agent);
            assert_path_is(&t["path"], &ws.target(agent, project), &ctx);
            assert_eq!(t["state"], "current", "{ctx}");
            assert_eq!(t["installed_version"], PKG_VERSION, "{ctx}");
            let want = if uninstall { "removed" } else { "unchanged" };
            assert_eq!(t["action"], want, "{ctx}");
        }
        if dry || !uninstall {
            assert_eq!(ws.snap(), before, "nothing may change\n{ctx}");
        } else {
            for agent in AGENTS {
                for scope in [false, true] {
                    let p = ws.target(agent, scope);
                    let gone = scope == project && sel.contains(&agent);
                    assert_eq!(p.exists(), !gone, "{}\n{ctx}", p.display());
                    if gone {
                        let dir = p.parent().unwrap();
                        assert!(!dir.exists(), "the empty glen dir is removed\n{ctx}");
                        assert!(dir.parent().unwrap().is_dir(), "skills/ stays\n{ctx}");
                    }
                }
            }
        }
        ws.assert_no_side_effects(&ctx);
    }
}

#[test]
fn ac405_claude_config_dir_moves_the_user_claude_target_only() {
    // Absolute, relative (to the cwd), empty and whitespace-only (both unset, cycle 1) values.
    let cases: [(&str, PathOf); 4] = [
        ("abs", |ws| ws.root.join("cfg/claude/skills/glen/SKILL.md")),
        ("rel", |ws| ws.cwd.join("rel/claude/skills/glen/SKILL.md")),
        ("empty", |ws| ws.user("claude")),
        ("blank", |ws| ws.user("claude")),
    ];
    for (kind, want) in cases {
        let ws = Ws::new();
        let value = match kind {
            "abs" => s(&ws.root.join("cfg/claude")),
            "rel" => "rel/claude".to_owned(),
            "blank" => "  \t ".to_owned(),
            _ => String::new(),
        };
        let out = ws
            .glen(["install-skill", "--json"])
            .env("CLAUDE_CONFIG_DIR", &value)
            .run();
        let env = skill_env(&out);
        let claude = want(&ws);
        assert_path_is(&tgt(&env["data"], "claude")["path"], &claude, &out.ctx());
        assert_path_is(
            &tgt(&env["data"], "codex")["path"],
            &ws.user("codex"),
            &out.ctx(),
        );
        assert_eq!(fs::read(&claude).unwrap(), rendered("claude"), "{kind}");
        assert_eq!(fs::read(ws.user("codex")).unwrap(), rendered("codex"));
        if !matches!(kind, "empty" | "blank") {
            assert!(!ws.home.join(".claude").exists(), "{kind}: wrote ~/.claude");
        }
        // --project ignores CLAUDE_CONFIG_DIR.
        let out = ws
            .glen(["install-skill", "--project", "--claude", "--json"])
            .env("CLAUDE_CONFIG_DIR", &value)
            .run();
        let env = skill_env(&out);
        assert_path_is(
            &tgt(&env["data"], "claude")["path"],
            &ws.project("claude"),
            &out.ctx(),
        );
        ws.assert_no_side_effects(kind);
    }
}

#[test]
fn ac405_codex_home_is_ignored_and_codex_skills_never_written() {
    let ws = Ws::new();
    let codex_home = ws.root.join("codexhome");
    fs::create_dir(&codex_home).unwrap();
    for extra in [&[][..], &["--force"], &["--uninstall"], &["--project"]] {
        let args = args_of(&["install-skill", "--codex", "--json"], extra);
        let out = ws.glen(&args).env("CODEX_HOME", &codex_home).run();
        skill_env(&out);
        assert!(
            is_empty_dir(&codex_home),
            "wrote under CODEX_HOME\n{}",
            out.ctx()
        );
        ws.assert_no_side_effects(&out.ctx());
    }
    let out = ws
        .glen(["install-skill", "--codex", "--json"])
        .env("CODEX_HOME", &codex_home)
        .run();
    let env = skill_env(&out);
    assert_path_is(
        &tgt(&env["data"], "codex")["path"],
        &ws.user("codex"),
        &out.ctx(),
    );
}

/// Cycle 1: directories 0755 and files 0644 *before umask*; glen never chmods past the umask.
#[test]
fn ac405_new_directories_and_files_are_0755_and_0644_before_umask() {
    for (mask, m) in [
        ("000", 0o000_u32),
        ("022", 0o022),
        ("027", 0o027),
        ("077", 0o077),
    ] {
        for project in [false, true] {
            let ws = Ws::new();
            let args: Vec<&str> = if project {
                vec!["install-skill", "--project", "--json"]
            } else {
                vec!["install-skill", "--json"]
            };
            let out = ws.glen_umask(mask, &args).run();
            skill_env(&out);
            for agent in AGENTS {
                let c = chain(&ws, agent, project);
                for d in &c[..3] {
                    assert_eq!(mode(d), 0o755 & !m, "umask {mask}: {}", d.display());
                }
                assert_eq!(mode(&c[3]), 0o644 & !m, "umask {mask}: {}", c[3].display());
            }
        }
    }
    // An upgrade writes a fresh file in place of an 0600 one, at 0644 less the umask.
    for (mask, m) in [("000", 0o000_u32), ("027", 0o027)] {
        let ws = Ws::new();
        ws.place(&ws.user("claude"), &bytes_for(Pre::Older, "claude"));
        set_mode(&ws.user("claude"), 0o600);
        let out = ws
            .glen_umask(mask, &["install-skill", "--claude", "--json"])
            .run();
        assert_target(
            &skill_env(&out),
            "claude",
            "upgraded",
            "outdated",
            Some(OLDER),
        );
        assert_eq!(mode(&ws.user("claude")), 0o644 & !m, "umask {mask}");
    }
}

#[test]
fn ac405_existing_directories_keep_their_mode() {
    let ws = Ws::new();
    let claude = ws.home.join(".claude");
    let agents = ws.home.join(".agents");
    fs::create_dir(&claude).unwrap();
    fs::create_dir(&agents).unwrap();
    set_mode(&claude, 0o700);
    set_mode(&agents, 0o750);
    let out = ws.glen_umask("000", &["install-skill", "--json"]).run();
    skill_env(&out);
    assert_eq!(mode(&claude), 0o700, "never chmod an existing directory");
    assert_eq!(mode(&agents), 0o750, "never chmod an existing directory");
    assert_eq!(mode(&claude.join("skills")), 0o755);
    assert_eq!(mode(&claude.join("skills/glen")), 0o755);
}

/// Same-directory temp + rename: an upgrade gets a new inode and leaves no stray file; a
/// read-only glen directory blocks the rename even though the file itself is writable.
#[test]
fn ac405_writes_are_atomic_renames() {
    for agent in AGENTS {
        let ws = Ws::new();
        ws.place(&ws.user(agent), &bytes_for(Pre::Older, agent));
        let ino = fs::metadata(ws.user(agent)).unwrap().ino();
        let env = install(&ws, agent, &[]);
        assert_target(&env, agent, "upgraded", "outdated", Some(OLDER));
        assert_ne!(
            fs::metadata(ws.user(agent)).unwrap().ino(),
            ino,
            "{agent}: the file was rewritten in place, not renamed over"
        );
        let names: Vec<String> = list_tree(ws.user(agent).parent().unwrap());
        assert_eq!(names, ["SKILL.md"], "{agent}: stray files left behind");
    }
    if running_as_root() {
        return;
    }
    for agent in AGENTS {
        let ws = Ws::new();
        let old = bytes_for(Pre::Older, agent);
        ws.place(&ws.user(agent), &old);
        let dir = ws.user(agent).parent().unwrap().to_path_buf();
        set_mode(&ws.user(agent), 0o644);
        set_mode(&dir, 0o555);
        let _restore = RestoreMode(dir.clone(), 0o755);
        let flag = format!("--{agent}");
        let out = ws.glen(["install-skill", &flag, "--json"]).run();
        let err = failed(&out, "io");
        assert!(
            err.to_string().contains(&s(&dir)),
            "the error names the failing path\n{}",
            out.ctx()
        );
        assert_eq!(
            err["details"]["done"],
            Value::Array(vec![]),
            "nothing was processed before the failure\n{}",
            out.ctx()
        );
        assert_eq!(
            fs::read(ws.user(agent)).unwrap(),
            old,
            "file changed in place"
        );
    }
}

/// Every selected target is inspected before the first write: an inspection error leaves
/// nothing written, not even the earlier (claude) target.
#[test]
fn ac405_an_inspection_error_aborts_with_nothing_written() {
    if running_as_root() {
        return;
    }
    let ws = Ws::new();
    let dir = ws.user("codex").parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir).unwrap();
    set_mode(&dir, 0o000);
    let _restore = RestoreMode(dir.clone(), 0o755);
    let before = ws.tree();
    let out = ws.glen(["install-skill", "--json"]).run();
    let err = failed(&out, "io");
    assert!(
        err.to_string().contains(".agents/skills/glen"),
        "the error names the failing path\n{}",
        out.ctx()
    );
    assert_eq!(
        err["details"]["done"],
        Value::Array(vec![]),
        "{}",
        out.ctx()
    );
    assert_eq!(ws.tree(), before, "nothing may be written\n{}", out.ctx());
    assert!(!ws.home.join(".claude").exists());
}

// ---------------------------------------------------------------------------------------------
// AC-405 — symlinks (O5).
// ---------------------------------------------------------------------------------------------

fn assert_symlink_skip(env: &Value, agent: &str, ctx: &str) {
    assert_eq!(
        tgt(&env["data"], agent)["action"],
        "skipped",
        "{agent}: {ctx}"
    );
    let w = env["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("a skill_foreign warning: {env}\n{ctx}"));
    assert!(
        w.iter().any(|w| w["code"] == "skill_foreign"
            && w["message"].as_str().is_some_and(|m| m.contains("symlink"))),
        "skill_foreign warning saying `symlink`: {env}\n{ctx}"
    );
}

#[test]
fn ac405_a_symlinked_glen_skill_dir_is_foreign_and_force_cannot_override() {
    for agent in AGENTS {
        for (tag, populated) in [("full", true), ("empty", false)] {
            let ws = Ws::new();
            let real = ws.root.join(format!("dotfiles-{tag}/glen"));
            fs::create_dir_all(&real).unwrap();
            if populated {
                fs::write(real.join("SKILL.md"), b"theirs\n").unwrap();
            }
            let skills = ws.top(agent, false).join("skills");
            fs::create_dir_all(&skills).unwrap();
            symlink(&real, skills.join("glen")).unwrap();
            let before = snapshot(&real);
            for extra in [
                &[][..],
                &["--force"],
                &["--uninstall"],
                &["--uninstall", "--force"],
            ] {
                let env = install(&ws, agent, extra);
                let ctx = format!("{agent} {tag} {extra:?}");
                assert_symlink_skip(&env, agent, &ctx);
                assert_eq!(snapshot(&real), before, "{ctx}: the link target changed");
                assert!(
                    fs::symlink_metadata(skills.join("glen"))
                        .unwrap()
                        .file_type()
                        .is_symlink(),
                    "{ctx}: the link was replaced"
                );
            }
        }
    }
}

#[test]
fn ac405_project_symlinks_below_the_scope_root_are_foreign() {
    for link in [".claude", ".claude/skills", ".agents", ".agents/skills"] {
        let agent = if link.starts_with(".claude") {
            "claude"
        } else {
            "codex"
        };
        let ws = Ws::new();
        let real = ws.root.join("dotfiles");
        fs::create_dir(&real).unwrap();
        let at = ws.cwd.join(link);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        symlink(&real, &at).unwrap();
        for extra in [&[][..], &["--force"]] {
            let args = args_of(&["install-skill", "--project", "--json"], extra);
            let out = ws.glen(&args).run();
            let env = skill_env(&out);
            let ctx = format!("{link} {extra:?}\n{}", out.ctx());
            assert_symlink_skip(&env, agent, &ctx);
            assert_eq!(
                tgt(&env["data"], other(agent))["action"],
                if extra.is_empty() {
                    "installed"
                } else {
                    "unchanged"
                },
                "{ctx}"
            );
            assert!(is_empty_dir(&real), "wrote through the link: {ctx}");
        }
    }
}

#[test]
fn ac405_o5_links_at_or_above_the_user_scope_root_are_followed() {
    for link in [".claude", ".claude/skills", ".agents", ".agents/skills"] {
        let agent = if link.starts_with(".claude") {
            "claude"
        } else {
            "codex"
        };
        let ws = Ws::new();
        let real = ws.root.join("dotfiles");
        fs::create_dir(&real).unwrap();
        let at = ws.home.join(link);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        symlink(&real, &at).unwrap();
        let ctx = format!("~/{link} -> dotfiles");
        let env = install(&ws, agent, &[]);
        assert_target(&env, agent, "installed", "not_installed", None);
        assert_path_is(&tgt(&env["data"], agent)["path"], &ws.user(agent), &ctx);
        assert_eq!(fs::read(ws.user(agent)).unwrap(), rendered(agent), "{ctx}");
        assert!(
            !is_empty_dir(&real),
            "{ctx}: the file is in the linked directory"
        );
        assert_target(
            &install(&ws, agent, &[]),
            agent,
            "unchanged",
            "current",
            None,
        );
        assert_target(
            &install(&ws, agent, &["--uninstall"]),
            agent,
            "removed",
            "current",
            None,
        );
        assert!(
            fs::symlink_metadata(&at).unwrap().file_type().is_symlink(),
            "{ctx}: the link itself stays"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// AC-406 — per-state actions.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac406_a_byte_identical_file_is_unchanged_and_untouched() {
    let ws = Ws::new();
    let old = SystemTime::now() - Duration::from_secs(86_400);
    for agent in AGENTS {
        ws.place(&ws.user(agent), rendered(agent));
        set_mtime(&ws.user(agent), old);
    }
    let before: Vec<(u64, SystemTime)> = AGENTS
        .iter()
        .map(|a| {
            let m = fs::metadata(ws.user(a)).unwrap();
            (m.ino(), m.modified().unwrap())
        })
        .collect();
    for extra in [&[][..], &["--force"]] {
        let args = args_of(&["install-skill", "--json"], extra);
        let env = skill_env(&ws.glen(&args).run());
        for (i, agent) in AGENTS.iter().enumerate() {
            assert_target(&env, agent, "unchanged", "current", Some(PKG_VERSION));
            let m = fs::metadata(ws.user(agent)).unwrap();
            assert_eq!(
                (m.ino(), m.modified().unwrap()),
                before[i],
                "{agent} {extra:?}: same inode and mtime"
            );
        }
    }
}

#[test]
fn ac406_older_versions_and_other_builds_are_upgraded() {
    for (pre, version) in [
        (Pre::Older, OLDER),
        (Pre::OlderBody, OLDER),
        (Pre::OtherBuild, PKG_VERSION),
    ] {
        for extra in [&[][..], &["--force"]] {
            for agent in AGENTS {
                let ws = Ws::new();
                put(&ws, &ws.user(agent), pre, agent);
                let env = install(&ws, agent, extra);
                assert_target(&env, agent, "upgraded", "outdated", Some(version));
                assert_eq!(
                    fs::read(ws.user(agent)).unwrap(),
                    rendered(agent),
                    "{pre:?} {agent} {extra:?}"
                );
            }
        }
    }
}

/// Newer, modified and foreign files are skipped with their warning; `--force` reinstalls them.
/// Cycle 1: an older version with a stale digest is `modified`, and a foreign marker (another
/// agent's included) reports `installed_version: null`.
#[test]
fn ac406_skipped_states_warn_and_force_reinstalls_them() {
    let cases = [
        (Pre::Newer, "newer", "skill_newer", Some(NEWER)),
        (
            Pre::Modified,
            "modified",
            "skill_modified",
            Some(PKG_VERSION),
        ),
        (Pre::OlderEdited, "modified", "skill_modified", Some(OLDER)),
        (Pre::Foreign, "foreign", "skill_foreign", None),
        (Pre::OtherAgent, "foreign", "skill_foreign", None),
        (Pre::LinkedFile, "foreign", "skill_foreign", None),
    ];
    for (pre, state, warning, version) in cases {
        for agent in AGENTS {
            let ws = Ws::new();
            let real = put(&ws, &ws.user(agent), pre, agent);
            let original = fs::read(ws.user(agent)).unwrap();
            let env = install(&ws, agent, &[]);
            let ctx = format!("{pre:?} {agent}");
            assert_target(&env, agent, "skipped", state, None);
            assert_warning(&env, warning);
            match (pre, version) {
                (Pre::Foreign | Pre::OtherAgent, _) => {
                    assert!(
                        tgt(&env["data"], agent)["installed_version"].is_null(),
                        "{ctx}"
                    )
                }
                (_, Some(v)) => assert_eq!(tgt(&env["data"], agent)["installed_version"], v),
                _ => {}
            }
            assert_eq!(
                fs::read(ws.user(agent)).unwrap(),
                original,
                "{ctx}: skipped means untouched"
            );

            let env = install(&ws, agent, &["--force"]);
            assert_target(&env, agent, "reinstalled", state, None);
            assert_eq!(fs::read(ws.user(agent)).unwrap(), rendered(agent), "{ctx}");
            if let Some(real) = real {
                let md = fs::symlink_metadata(ws.user(agent)).unwrap();
                assert!(
                    md.file_type().is_file(),
                    "{ctx}: the link is replaced by a file"
                );
                assert_eq!(
                    fs::read(&real).unwrap(),
                    rendered(agent),
                    "{ctx}: link target untouched"
                );
            }
        }
    }
}

#[test]
fn ac406_uninstall_removes_current_outdated_and_newer_files() {
    for (pre, state) in [
        (Pre::Current, "current"),
        (Pre::Older, "outdated"),
        (Pre::Newer, "newer"),
    ] {
        for agent in AGENTS {
            let ws = Ws::new();
            put(&ws, &ws.user(agent), pre, agent);
            let env = install(&ws, agent, &["--uninstall"]);
            assert_target(&env, agent, "removed", state, None);
            let dir = ws.user(agent).parent().unwrap().to_path_buf();
            assert!(
                !dir.exists(),
                "{pre:?} {agent}: file and empty glen dir removed"
            );
            assert!(dir.parent().unwrap().is_dir(), "skills/ stays");
        }
    }
}

#[test]
fn ac406_uninstall_keeps_a_non_empty_glen_dir_with_a_warning() {
    for agent in AGENTS {
        let ws = Ws::new();
        ws.place(&ws.user(agent), rendered(agent));
        let notes = ws.user(agent).with_file_name("notes.md");
        fs::write(&notes, b"mine\n").unwrap();
        let env = install(&ws, agent, &["--uninstall"]);
        assert_target(&env, agent, "removed", "current", None);
        assert_warning(&env, "skill_dir_not_empty");
        assert!(!ws.user(agent).exists());
        assert_eq!(fs::read(&notes).unwrap(), b"mine\n");
    }
}

/// Cycle 1: a skipped `--uninstall` emits the same warning code as a skipped install.
#[test]
fn ac406_uninstall_skips_modified_and_foreign_files_unless_forced() {
    for (pre, state, warning) in [
        (Pre::Modified, "modified", "skill_modified"),
        (Pre::OlderEdited, "modified", "skill_modified"),
        (Pre::Foreign, "foreign", "skill_foreign"),
        (Pre::OtherAgent, "foreign", "skill_foreign"),
        (Pre::LinkedFile, "foreign", "skill_foreign"),
    ] {
        for agent in AGENTS {
            let ws = Ws::new();
            let real = put(&ws, &ws.user(agent), pre, agent);
            let original = fs::read(ws.user(agent)).unwrap();
            let env = install(&ws, agent, &["--uninstall"]);
            assert_target(&env, agent, "skipped", state, None);
            assert_warning(&env, warning);
            assert_eq!(
                fs::read(ws.user(agent)).unwrap(),
                original,
                "{pre:?} {agent}"
            );
            let env = install(&ws, agent, &["--uninstall", "--force"]);
            assert_target(&env, agent, "removed", state, None);
            assert!(
                fs::symlink_metadata(ws.user(agent)).is_err(),
                "{pre:?} {agent}: removed"
            );
            if let Some(real) = real {
                assert_eq!(fs::read(&real).unwrap(), original, "the link target stays");
            }
        }
    }
}

/// `--dry-run` reports what the real run then does, and changes nothing on disk.
#[test]
fn ac406_dry_run_reports_the_real_actions_and_changes_nothing() {
    let pres = [
        Pre::Absent,
        Pre::Current,
        Pre::Older,
        Pre::Newer,
        Pre::Modified,
        Pre::OlderEdited,
        Pre::Foreign,
    ];
    let modes: [&[&str]; 4] = [
        &[],
        &["--force"],
        &["--uninstall"],
        &["--uninstall", "--force"],
    ];
    for pre in pres {
        for extra in modes {
            let ws = Ws::new();
            put(&ws, &ws.user("claude"), pre, "claude");
            let before = ws.snap();
            let mut dry_args = extra.to_vec();
            dry_args.push("--dry-run");
            let dry = install(&ws, "claude", &dry_args);
            assert_eq!(
                ws.snap(),
                before,
                "{pre:?} {extra:?}: dry run changed the disk"
            );
            assert_eq!(dry["data"]["dry_run"], true);
            let real = install(&ws, "claude", extra);
            assert_eq!(real["data"]["dry_run"], false);
            assert_eq!(
                dry["data"]["targets"], real["data"]["targets"],
                "{pre:?} {extra:?}: dry run and real run disagree"
            );
            assert_eq!(
                warning_codes(&dry),
                warning_codes(&real),
                "{pre:?} {extra:?}: warnings"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// AC-407 — exits and human output.
// ---------------------------------------------------------------------------------------------

/// User scope needs HOME (`invalid_env`); project scope does not (cycle 1).
#[test]
fn ac407_home_unset_or_empty_is_invalid_env_for_the_user_scope_only() {
    let ws = Ws::new();
    for out in [
        ws.glen(["install-skill", "--dry-run", "--json"])
            .env_remove("HOME")
            .run(),
        ws.glen(["install-skill", "--dry-run", "--json"])
            .env("HOME", "")
            .run(),
    ] {
        failed(&out, "invalid_env");
        out.assert_exit(2);
    }
    ws.assert_no_side_effects("HOME unset");
    for home in [None, Some("")] {
        let ws = Ws::new();
        let run = |args: &[&str]| match home {
            None => ws.glen(args).env_remove("HOME"),
            Some(h) => ws.glen(args).env("HOME", h),
        };
        // A dry run first, so a wrong scope can never reach a real home directory.
        let out = run(&["install-skill", "--project", "--dry-run", "--json"]).run();
        let env = skill_env(&out);
        assert_eq!(env["data"]["scope"], "project", "{}", out.ctx());
        for agent in AGENTS {
            assert_path_is(
                &tgt(&env["data"], agent)["path"],
                &ws.project(agent),
                &out.ctx(),
            );
        }
        let out = run(&["install-skill", "--project", "--json"]).run();
        let env = skill_env(&out);
        for agent in AGENTS {
            assert_target(&env, agent, "installed", "not_installed", None);
            assert_eq!(
                fs::read(ws.project(agent)).unwrap(),
                rendered(agent),
                "{agent}"
            );
        }
        assert!(
            is_empty_dir(&ws.home),
            "HOME {home:?}: wrote under the temp HOME"
        );
        ws.assert_no_side_effects("project install without HOME");
    }
}

#[test]
fn ac407_an_io_failure_names_the_path_and_lists_the_targets_done() {
    if running_as_root() {
        return;
    }
    let ws = Ws::new();
    ws.place(&ws.user("codex"), &bytes_for(Pre::Older, "codex"));
    let dir = ws.user("codex").parent().unwrap().to_path_buf();
    set_mode(&dir, 0o555);
    let _restore = RestoreMode(dir.clone(), 0o755);
    let out = ws.glen(["install-skill", "--json"]).run();
    let err = failed(&out, "io");
    out.assert_exit(1);
    let mut no_done = err.clone();
    if let Some(d) = no_done["details"].as_object_mut() {
        d.remove("done");
    }
    assert!(
        no_done.to_string().contains(&s(&dir)),
        "the error names the failing path\n{}",
        out.ctx()
    );
    let done = err["details"]["done"]
        .as_array()
        .unwrap_or_else(|| panic!("error.details.done is an array\n{}", out.ctx()));
    assert_eq!(done.len(), 1, "{}", out.ctx());
    let d = done[0].to_string();
    assert!(
        d.contains("claude") && !d.contains("codex"),
        "{}",
        out.ctx()
    );
    assert_eq!(fs::read(ws.user("claude")).unwrap(), rendered("claude"));
}

#[test]
fn ac407_human_mode_prints_one_line_per_target_and_the_invocation_hint() {
    let ws = Ws::new();
    for action in ["installed", "unchanged"] {
        let out = ws
            .glen(["install-skill", "--human", "--color", "never"])
            .run();
        out.assert_exit(0).assert_no_ansi();
        let stdout = out.out();
        let lines: Vec<&str> = stdout.lines().filter(|l| l.contains("SKILL.md")).collect();
        assert_eq!(lines.len(), 2, "one line per target\n{}", out.ctx());
        for (line, (agent, path)) in lines.iter().zip([
            ("claude", "~/.claude/skills/glen/SKILL.md"),
            ("codex", "~/.agents/skills/glen/SKILL.md"),
        ]) {
            let words: Vec<&str> = line.split_whitespace().collect();
            let glyph = line.trim_start().chars().next().unwrap();
            assert!(GLYPHS.contains(&glyph), "glyph first: {line:?}");
            for w in [action, agent, path] {
                assert!(words.contains(&w), "{w} in {line:?}\n{}", out.ctx());
            }
        }
        let all = format!("{stdout}{}", out.err());
        assert!(
            all.lines()
                .any(|l| !l.contains("SKILL.md") && l.contains("/glen") && l.contains("$glen")),
            "a line naming /glen and $glen\n{}",
            out.ctx()
        );
    }
}

// ---------------------------------------------------------------------------------------------
// AC-408 — doctor's skill rows.
// ---------------------------------------------------------------------------------------------

fn doctor(ws: &Ws, vars: &[(&str, &str)]) -> Value {
    let out = envs(ws.glen(["doctor", "--json"]).fake_omp(), vars).run();
    let data = out.ok_data();
    out.assert_stderr_empty();
    data
}

fn skills_entry<'a>(data: &'a Value, agent: &str) -> &'a Value {
    assert_eq!(keyset(&data["skills"]), set(&AGENTS), "{data}");
    let e = &data["skills"][agent];
    assert_eq!(
        keyset(e),
        set(&["path", "state", "version", "current", "project"]),
        "{data}"
    );
    e
}

fn check_text(c: &Value) -> String {
    format!(
        "{} {}",
        c["detail"].as_str().unwrap_or_default(),
        c["value"]
    )
}

#[test]
fn ac408_doctor_appends_the_four_w4_checks_after_every_earlier_check() {
    let ws = Ws::new();
    let names = |data: &Value| -> Vec<String> {
        data["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let data = doctor(&ws, &[]);
    assert_eq!(names(&data), DOCTOR_CHECKS, "{data}");
    let out = ws.glen(["doctor", "--json"]).run();
    let err = failed(&out, "omp_unavailable");
    assert_eq!(names(&err["details"]["report"]), DOCTOR_CHECKS);
    assert!(err["details"]["report"]["skills"].is_object());
}

#[test]
fn ac408_doctor_reports_each_user_skill_state() {
    let cases: [(Pre, &str, &str, Option<&str>, &str); 9] = [
        (
            Pre::Absent,
            "not_installed",
            "info",
            None,
            "glen install-skill",
        ),
        (Pre::Current, "current", "ok", Some(PKG_VERSION), ""),
        (
            Pre::Older,
            "outdated",
            "warn",
            Some(OLDER),
            "glen install-skill",
        ),
        (Pre::Newer, "newer", "warn", Some(NEWER), "install-skill"),
        (
            Pre::Modified,
            "modified",
            "warn",
            Some(PKG_VERSION),
            "--force",
        ),
        (Pre::OlderEdited, "modified", "warn", Some(OLDER), "--force"),
        (Pre::Foreign, "foreign", "warn", None, "--force"),
        (Pre::OtherAgent, "foreign", "warn", Some("?"), "--force"),
        (Pre::LinkedFile, "foreign", "warn", Some("?"), "--force"),
    ];
    for (pre, state, status, version, remedy) in cases {
        let ws = Ws::new();
        for agent in AGENTS {
            put(&ws, &ws.user(agent), pre, agent);
        }
        let before = ws.snap();
        let data = doctor(&ws, &[]);
        for agent in AGENTS {
            let ctx = format!("{pre:?} {agent}: {data}");
            let e = skills_entry(&data, agent);
            assert_path_is(&e["path"], &ws.user(agent), &ctx);
            assert_eq!(e["state"], state, "{ctx}");
            assert_eq!(e["current"], state == "current", "{ctx}");
            match version {
                None => assert!(e["version"].is_null(), "{ctx}"),
                Some("?") => {}
                Some(v) => assert_eq!(e["version"], v, "{ctx}"),
            }
            assert!(e["project"].is_null(), "{ctx}");
            let c = check(&data, &format!("skill_{agent}"));
            assert_eq!(c["status"], status, "{ctx}");
            assert!(
                c["detail"].as_str().is_some_and(|d| d.contains(remedy)),
                "detail names `{remedy}`: {ctx}"
            );
        }
        assert_eq!(ws.snap(), before, "{pre:?}: doctor wrote something");
    }
}

#[test]
fn ac408_doctor_skill_paths_follow_claude_config_dir() {
    let ws = Ws::new();
    let abs = ws.root.join("cfg");
    let abs_s = s(&abs);
    let data = doctor(&ws, &[("CLAUDE_CONFIG_DIR", abs_s.as_str())]);
    assert_path_is(
        &skills_entry(&data, "claude")["path"],
        &abs.join("skills/glen/SKILL.md"),
        &data.to_string(),
    );
    assert_path_is(
        &skills_entry(&data, "codex")["path"],
        &ws.user("codex"),
        &data.to_string(),
    );
    let data = doctor(&ws, &[("CLAUDE_CONFIG_DIR", "rel/cfg")]);
    assert_path_is(
        &skills_entry(&data, "claude")["path"],
        &ws.cwd.join("rel/cfg/skills/glen/SKILL.md"),
        &data.to_string(),
    );
    // Empty and whitespace-only values are unset (cycle 1).
    for blank in ["", "  \t "] {
        let data = doctor(&ws, &[("CLAUDE_CONFIG_DIR", blank)]);
        assert_path_is(
            &skills_entry(&data, "claude")["path"],
            &ws.user("claude"),
            &data.to_string(),
        );
    }
    // A skill installed there is current.
    skill_env(
        &ws.glen(["install-skill", "--claude", "--json"])
            .env("CLAUDE_CONFIG_DIR", &abs)
            .run(),
    );
    let data = doctor(&ws, &[("CLAUDE_CONFIG_DIR", abs_s.as_str())]);
    assert_eq!(skills_entry(&data, "claude")["state"], "current", "{data}");
    assert_eq!(check(&data, "skill_claude")["status"], "ok", "{data}");
    assert!(!ws.home.join(".claude").exists());
}

#[test]
fn ac408_doctor_reports_project_copies_and_warns_when_one_is_stale() {
    // A current project copy: reported, no raise.
    let ws = Ws::new();
    ws.place(&ws.project("claude"), rendered("claude"));
    let data = doctor(&ws, &[]);
    let p = &skills_entry(&data, "claude")["project"];
    assert_eq!(keyset(p), set(&["path", "state", "version"]), "{data}");
    assert_path_is(&p["path"], &ws.project("claude"), &data.to_string());
    assert_eq!(p["state"], "current", "{data}");
    assert_eq!(p["version"], PKG_VERSION, "{data}");
    assert!(skills_entry(&data, "codex")["project"].is_null(), "{data}");
    assert_eq!(check(&data, "skill_claude")["status"], "info", "{data}");

    // Stale project copies raise the agent's check to warn and name `--project`, whatever the
    // user copy says.
    for (user, pre, state) in [
        (Pre::Absent, Pre::Older, "outdated"),
        (Pre::Current, Pre::Older, "outdated"),
        (Pre::Current, Pre::Modified, "modified"),
        (Pre::Current, Pre::Foreign, "foreign"),
    ] {
        let ws = Ws::new();
        for agent in AGENTS {
            put(&ws, &ws.user(agent), user, agent);
            put(&ws, &ws.project(agent), pre, agent);
        }
        let data = doctor(&ws, &[]);
        for agent in AGENTS {
            let ctx = format!("user {user:?}, project {pre:?}, {agent}: {data}");
            let p = &skills_entry(&data, agent)["project"];
            assert_eq!(p["state"], state, "{ctx}");
            let c = check(&data, &format!("skill_{agent}"));
            assert_eq!(c["status"], "warn", "{ctx}");
            assert!(
                c["detail"]
                    .as_str()
                    .is_some_and(|d| d.contains("glen install-skill --project")),
                "{ctx}"
            );
        }
    }
}

#[test]
fn ac408_w4_rows_never_fail_doctor_and_doctor_creates_nothing() {
    let ws = Ws::new();
    put(&ws, &ws.user("claude"), Pre::Modified, "claude");
    put(&ws, &ws.user("codex"), Pre::Newer, "codex");
    put(&ws, &ws.project("claude"), Pre::Older, "claude");
    fs::write(ws.home.join(".env"), "XDG_DATA_HOME=/x/data\n").unwrap();
    let before = ws.snap();
    let data = doctor(&ws, &OFFLINE);
    for name in ["omp_env_files", "skill_claude", "skill_codex", "sandbox"] {
        assert_eq!(check(&data, name)["status"], "warn", "{name}: {data}");
    }
    for c in data["checks"].as_array().unwrap() {
        assert_ne!(c["status"], "fail", "{c}");
    }
    let human = envs(ws.glen(["doctor", "--human"]).fake_omp(), &OFFLINE).run();
    human.assert_exit(0);
    assert_eq!(ws.snap(), before, "doctor created or changed files");
    ws.assert_no_side_effects("doctor");
}

// ---------------------------------------------------------------------------------------------
// AC-409 — `.env` files omp reads.
// ---------------------------------------------------------------------------------------------

/// The `omp_env_files` check and the files it lists. Cycle 1: the value is an array of absolute
/// paths, `[]` when no file needs attention (the §A10 rule wins over AC-409's `~` wording).
fn env_check(data: &Value) -> (Value, BTreeSet<String>) {
    let c = check(data, "omp_env_files").clone();
    let listed = c["value"]
        .as_array()
        .unwrap_or_else(|| panic!("omp_env_files.value is an array: {c}"))
        .iter()
        .map(|v| {
            let p = v
                .as_str()
                .unwrap_or_else(|| panic!("omp_env_files.value holds strings: {c}"));
            assert!(Path::new(p).is_absolute(), "the paths are absolute: {c}");
            p.to_owned()
        })
        .collect();
    (c, listed)
}

fn files(paths: &[&Path]) -> BTreeSet<String> {
    paths.iter().map(|p| s(p)).collect()
}

fn assert_env_clean(data: &Value) {
    let (c, _) = env_check(data);
    assert_eq!(c["status"], "ok", "{c}");
    assert_eq!(c["value"], Value::Array(vec![]), "{c}");
}

#[test]
fn ac409_no_env_files_is_ok_with_an_empty_value() {
    let ws = Ws::new();
    assert_env_clean(&doctor(&ws, &[]));
}

#[test]
fn ac409_each_location_omp_reads_is_checked() {
    let cases: [EnvCase; 8] = [
        (
            |ws| ws.home.join(".env"),
            "XDG_DATA_HOME=/x/data\n",
            "XDG_DATA_HOME",
            &[],
        ),
        (
            |ws| ws.cwd.join(".env"),
            "export XDG_STATE_HOME=/x/state\n",
            "XDG_STATE_HOME",
            &[],
        ),
        (
            |ws| ws.agent.join(".env"),
            "XDG_DATA_HOME=/x/data\n",
            "XDG_DATA_HOME",
            &[],
        ),
        (
            |ws| ws.home.join(".omp/.env"),
            "PI_CODING_AGENT_DIR=/x/agent\n",
            "PI_CODING_AGENT_DIR",
            &[("PI_CODING_AGENT_DIR", "")],
        ),
        (
            |ws| ws.home.join(".env"),
            "OMP_CODING_AGENT_DIR=/x/agent\n",
            "OMP_CODING_AGENT_DIR",
            &[("PI_CODING_AGENT_DIR", "")],
        ),
        (
            |ws| ws.home.join(".omp/agent/.env"),
            "export XDG_DATA_HOME=/x/data\n",
            "XDG_DATA_HOME",
            &[("PI_CODING_AGENT_DIR", "")],
        ),
        // Cycle 1: whitespace-only §A12 path variables count as unset.
        (
            |ws| ws.home.join(".env"),
            "XDG_DATA_HOME=/x/data\n",
            "XDG_DATA_HOME",
            &[("XDG_DATA_HOME", "  ")],
        ),
        (
            |ws| ws.cwd.join(".env"),
            "PI_CODING_AGENT_DIR=/x/agent\n",
            "PI_CODING_AGENT_DIR",
            &[("PI_CODING_AGENT_DIR", " \t ")],
        ),
    ];
    for (loc, contents, key, vars) in cases {
        let ws = Ws::new();
        let file = loc(&ws);
        ws.place(&file, contents.as_bytes());
        let (c, listed) = env_check(&doctor(&ws, vars));
        let ctx = format!("{} = {contents:?}, env {vars:?}: {c}", file.display());
        assert_eq!(c["status"], "warn", "{ctx}");
        assert_eq!(
            listed,
            files(&[&file]),
            "value lists exactly that file: {ctx}"
        );
        assert!(
            c["detail"].as_str().is_some_and(|d| d.contains(key)),
            "detail names {key}: {ctx}"
        );
    }
}

#[test]
fn ac409_profile_config_root_and_agent_dir_are_checked() {
    for rel in [".omp/profiles/work/.env", ".omp/profiles/work/agent/.env"] {
        let ws = Ws::new();
        let file = ws.home.join(rel);
        ws.place(&file, b"XDG_STATE_HOME=/x/state\n");
        let out = ws
            .glen(["doctor", "--json", "--profile", "work"])
            .fake_omp()
            .run();
        let (c, listed) = env_check(&out.ok_data());
        assert_eq!(c["status"], "warn", "{rel}: {c}");
        assert_eq!(listed, files(&[&file]), "{rel}: {c}");
    }
}

#[test]
fn ac409_several_files_are_all_listed_with_all_their_keys() {
    let ws = Ws::new();
    let a = ws.home.join(".env");
    let b = ws.cwd.join(".env");
    let unrelated = ws.agent.join(".env");
    ws.place(&a, b"XDG_DATA_HOME=/x/data\n");
    ws.place(&b, b"export XDG_STATE_HOME=/x/state\n");
    ws.place(&unrelated, b"FOO=1\n");
    let (c, listed) = env_check(&doctor(&ws, &[]));
    assert_eq!(c["status"], "warn", "{c}");
    assert_eq!(
        listed,
        files(&[&a, &b]),
        "both files, and not the one assigning none of the keys: {c}"
    );
    let detail = c["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("XDG_DATA_HOME") && detail.contains("XDG_STATE_HOME"),
        "{c}"
    );
}

/// Keys glen already has are not a gap. `OMP_CODING_AGENT_DIR` is the alias of
/// `PI_CODING_AGENT_DIR` (§A12, cycle 1), so glen's own PI_ value covers both.
#[test]
fn ac409_keys_set_in_glens_own_environment_do_not_warn() {
    let ws = Ws::new();
    let data_home = ws.root.join("xdg-data");
    let state_home = ws.root.join("xdg-state");
    fs::create_dir(&data_home).unwrap();
    fs::create_dir(&state_home).unwrap();
    ws.place(
        &ws.home.join(".env"),
        b"XDG_DATA_HOME=/x/data\nXDG_STATE_HOME=/x/state\nPI_CODING_AGENT_DIR=/x/agent\nOMP_CODING_AGENT_DIR=/x/agent\n",
    );
    assert_env_clean(&doctor(
        &ws,
        &[
            ("XDG_DATA_HOME", s(&data_home).as_str()),
            ("XDG_STATE_HOME", s(&state_home).as_str()),
        ],
    ));
}

#[test]
fn ac409_unrelated_keys_and_comments_do_not_warn() {
    let ws = Ws::new();
    ws.place(
        &ws.home.join(".env"),
        b"FOO=1\n# XDG_DATA_HOME=/x/data\nMY_XDG_DATA_HOME=/x\nXDG_DATA_HOME_OLD=/x\n",
    );
    assert_env_clean(&doctor(&ws, &[]));
}

#[test]
fn ac409_unreadable_files_are_skipped() {
    if running_as_root() {
        return;
    }
    let ws = Ws::new();
    let file = ws.home.join(".env");
    ws.place(&file, b"XDG_DATA_HOME=/x/data\n");
    set_mode(&file, 0o000);
    let _restore = RestoreMode(file.clone(), 0o644);
    assert_env_clean(&doctor(&ws, &[]));
}

// ---------------------------------------------------------------------------------------------
// AC-410 — Codex sandbox awareness (and O3).
// ---------------------------------------------------------------------------------------------

/// A `sandboxed` refusal: exit 6 `no_route`, exact details, a hint pointing outside the sandbox.
fn assert_sandboxed(out: &Out, sandbox: Option<&str>, network: bool) -> Value {
    let err = failed(out, "no_route");
    let d = &err["details"];
    assert_eq!(
        keyset(d),
        set(&["reason", "sandbox", "network"]),
        "{}",
        out.ctx()
    );
    assert_eq!(d["reason"], "sandboxed", "{}", out.ctx());
    assert_eq!(
        d["sandbox"],
        sandbox.map_or(Value::Null, Value::from),
        "{}",
        out.ctx()
    );
    assert_eq!(d["network"], network, "{}", out.ctx());
    assert!(
        err["hint"]
            .as_str()
            .is_some_and(|h| h.contains("outside the sandbox")),
        "{}",
        out.ctx()
    );
    assert!(
        out.elapsed < Duration::from_secs(5),
        "refusal took {:?}",
        out.elapsed
    );
    err
}

fn assert_nothing_started(rb: &RpcBox, ctx: &str) {
    assert!(rb.job_ids().is_empty(), "a job was created: {ctx}");
    assert!(
        rb.starts().is_empty(),
        "omp was started: {ctx}\n{}",
        rb.diag()
    );
}

#[test]
fn ac410_an_offline_sandbox_refuses_send_with_reason_details_and_hint() {
    // Cycle 1: only a non-empty CODEX_SANDBOX marks the sandbox; details carry its value.
    for sandbox in ["seatbelt", "landlock"] {
        for target in [A.handle, "@latest", "@new"] {
            let rb = RpcBox::new();
            let out = rb
                .glen(["send", target, "hi", "--json"])
                .env("CODEX_SANDBOX", sandbox)
                .env("CODEX_SANDBOX_NETWORK_DISABLED", "1")
                .run();
            assert_sandboxed(&out, Some(sandbox), false);
            assert!(
                !rb.bx.glen_home.exists(),
                "GLEN_HOME was created\n{}",
                out.ctx()
            );
            assert_nothing_started(&rb, &out.ctx());
        }
    }
}

#[test]
fn ac410_the_offline_refusal_precedes_omp_lookup_the_lease_probe_and_the_lock() {
    // omp lookup: a missing omp is not reported; the control run without the variables is.
    let rb = RpcBox::new();
    let out = envs(
        rb.glen(["send", "@new", "hi", "--json"])
            .env("GLEN_OMP_BIN", "/nonexistent/omp"),
        &OFFLINE,
    )
    .run();
    assert_sandboxed(&out, Some("seatbelt"), false);
    let control = rb
        .glen(["send", "@new", "hi", "--json"])
        .env("GLEN_OMP_BIN", "/nonexistent/omp")
        .run();
    failed(&control, "omp_unavailable");

    // The owner lease: a held lease would refuse without details; the sandbox reason wins.
    let rb = RpcBox::new();
    let _lease = hold_lease(&rb.bx.owners(), A.id);
    let out = envs(rb.glen(["send", A.handle, "hi", "--json"]), &OFFLINE).run();
    assert_sandboxed(&out, Some("seatbelt"), false);
    assert!(!rb.bx.glen_home.exists());
    assert_nothing_started(&rb, &out.ctx());

    // glen's session lock: a held lock would queue a job; the sandbox refuses first.
    let rb = RpcBox::new();
    let _lock = rb.hold_glen_lock(A.id);
    let out = envs(
        rb.glen(["send", A.handle, "hi", "--no-wait", "--json"]),
        &OFFLINE,
    )
    .run();
    assert_sandboxed(&out, Some("seatbelt"), false);
    assert_nothing_started(&rb, &out.ctx());
}

#[test]
fn ac410_the_offline_refusal_follows_argument_target_and_message_validation() {
    let rb = RpcBox::new();
    let cases: [(&[&str], &str); 6] = [
        (
            &["send", A.handle, "hi", "--steer", "--follow-up", "--json"],
            "usage",
        ),
        (
            &["send", A.handle, "hi", "--cwd", "/tmp", "--json"],
            "usage",
        ),
        (&["send", A.handle, "--json"], "usage"),
        (&["send", "j1234567", "hi", "--json"], "usage"),
        (&["send", "fffffff", "hi", "--json"], "not_found"),
        (&["send", "c0de", "hi", "--json"], "ambiguous"),
    ];
    for (args, code) in cases {
        let out = envs(rb.glen(args), &OFFLINE).run();
        failed(&out, code);
    }
    let out = rb.glen_stdin(&["send", A.handle, "-", "--json"], &OFFLINE, b"");
    failed(&out, "usage");
    let out = rb.glen_stdin(&["send", A.handle, "-", "--json"], &OFFLINE, b"hello");
    assert_sandboxed(&out, Some("seatbelt"), false);
    assert!(!rb.bx.glen_home.exists());
    assert_nothing_started(&rb, "validation");
}

#[test]
fn ac410_a_network_enabled_sandbox_refuses_rpc_and_new_jobs() {
    for target in [A.handle, "@new", "@latest"] {
        let rb = RpcBox::new();
        let out = envs(rb.glen(["send", target, "hi", "--json"]), &NETWORKED).run();
        assert_sandboxed(&out, Some("seatbelt"), true);
        assert!(
            !rb.bx.glen_home.exists(),
            "GLEN_HOME was created\n{}",
            out.ctx()
        );
        assert_nothing_started(&rb, &out.ctx());
    }
    // A send that would queue an rpc job behind a running glen job.
    let rb = RpcBox::new();
    let _lock = rb.hold_glen_lock(A.id);
    let out = envs(
        rb.glen(["send", A.handle, "hi", "--no-wait", "--json"]),
        &NETWORKED,
    )
    .run();
    assert_sandboxed(&out, Some("seatbelt"), true);
    assert_nothing_started(&rb, &out.ctx());
}

#[test]
fn ac410_a_human_refusal_prints_the_hint_on_stderr() {
    let rb = RpcBox::new();
    let out = envs(rb.glen(["send", A.handle, "hi", "--human"]), &OFFLINE).run();
    out.assert_exit(6);
    assert!(out.stdout.is_empty(), "{}", out.ctx());
    assert!(out.err().contains("outside the sandbox"), "{}", out.ctx());
}

#[test]
fn ac410_empty_or_zero_sandbox_variables_send_normally() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("CODEX_SANDBOX", "")
        .env("CODEX_SANDBOX_NETWORK_DISABLED", "0")
        .run();
    let d = data_of(&out, 0);
    assert_eq!(d["status"], "completed", "{d}");
    rb.assert_quiet();
}

/// Cycle 1: `CODEX_SANDBOX_NETWORK_DISABLED=1` without a non-empty `CODEX_SANDBOX` is not a
/// sandbox (Codex also sets it on approved escalated commands), so the send proceeds.
#[test]
fn ac410_network_disabled_without_codex_sandbox_is_not_a_sandbox() {
    for vars in [
        &[("CODEX_SANDBOX_NETWORK_DISABLED", "1")][..],
        &[
            ("CODEX_SANDBOX", ""),
            ("CODEX_SANDBOX_NETWORK_DISABLED", "1"),
        ],
    ] {
        let rb = RpcBox::new();
        let out = envs(
            rb.glen(["send", A.handle, "hi", "--wait", "20s", "--json"]),
            vars,
        )
        .run();
        let d = data_of(&out, 0);
        assert_eq!(d["status"], "completed", "{vars:?}: {d}");
        rb.assert_quiet();
    }
}

/// Cycle 1: in that state a real permission failure writing the job still maps to exit 6
/// `sandboxed` with `glen_home_writable:false`, never a raw `io` error.
#[test]
fn ac410_network_disabled_alone_maps_a_glen_home_permission_failure_to_sandboxed() {
    if running_as_root() {
        return;
    }
    let rb = RpcBox::new();
    let ro = rb.bx.root.join("ro");
    fs::create_dir(&ro).unwrap();
    set_mode(&ro, 0o555);
    let _restore = RestoreMode(ro.clone(), 0o755);
    let out = rb
        .glen(["send", A.handle, "hi", "--json"])
        .env("GLEN_HOME", ro.join("glen"))
        .env("CODEX_SANDBOX_NETWORK_DISABLED", "1")
        .run();
    let err = failed(&out, "no_route");
    let d = &err["details"];
    assert_eq!(d["reason"], "sandboxed", "{}", out.ctx());
    assert_eq!(d["glen_home_writable"], false, "{}", out.ctx());
    assert!(
        d["sandbox"].is_null(),
        "CODEX_SANDBOX is unset\n{}",
        out.ctx()
    );
    assert_eq!(d["network"], false, "{}", out.ctx());
    let hint = err["hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("TMPDIR") && hint.contains("writable_roots"),
        "{}",
        out.ctx()
    );
    assert!(rb.starts().is_empty(), "omp was started\n{}", rb.diag());
}

#[test]
fn ac410_reads_work_inside_the_sandbox() {
    let bx = OmpBox::new();
    for args in [
        vec!["ls", "--json"],
        vec!["show", A.handle, "--json"],
        vec!["tail", A.handle, "-n", "2", "--json"],
        vec!["jobs", "--json"],
    ] {
        let out = envs(bx.glen(&args), &OFFLINE).run();
        out.ok_data();
        out.assert_stderr_empty();
    }
    envs(bx.glen(["wait", "j1234567", "--json"]), &OFFLINE)
        .run()
        .error_envelope("not_found");
    bx.assert_no_state();
}

/// O3: inside the sandbox, a spill into an unwritable GLEN_HOME is `sandboxed` with
/// `glen_home_writable:false`, never a raw `io` error; outside it stays `io`.
#[test]
fn ac410_o3_an_unwritable_glen_home_spill_is_sandboxed() {
    if running_as_root() {
        return;
    }
    let bx = OmpBox::new();
    let ro = bx.root.join("ro");
    fs::create_dir(&ro).unwrap();
    set_mode(&ro, 0o555);
    let _restore = RestoreMode(ro.clone(), 0o755);
    let gh = ro.join("glen");
    for (vars, network) in [(&NETWORKED[..], true), (&OFFLINE[..], false)] {
        let out = envs(
            bx.glen(["show", A.handle, "--json"])
                .env("GLEN_HOME", &gh)
                .env("GLEN_MAX_OUTPUT", "100"),
            vars,
        )
        .run();
        let err = failed(&out, "no_route");
        let d = &err["details"];
        assert_eq!(d["reason"], "sandboxed", "{}", out.ctx());
        assert_eq!(d["sandbox"], "seatbelt", "{}", out.ctx());
        assert_eq!(d["glen_home_writable"], false, "{}", out.ctx());
        assert_eq!(d["network"], network, "{}", out.ctx());
        let hint = err["hint"].as_str().unwrap_or_default();
        assert!(
            hint.contains("TMPDIR") && hint.contains("writable_roots"),
            "{}",
            out.ctx()
        );
    }
    let out = bx
        .glen(["show", A.handle, "--json"])
        .env("GLEN_HOME", &gh)
        .env("GLEN_MAX_OUTPUT", "100")
        .run();
    failed(&out, "io");

    // A writable GLEN_HOME spills normally inside the sandbox.
    let out = envs(
        bx.glen(["show", A.handle, "--json"])
            .env("GLEN_MAX_OUTPUT", "100"),
        &OFFLINE,
    )
    .run();
    let env = out.ok_envelope();
    assert!(env["spilled"].is_object(), "{}", out.ctx());
}

#[test]
fn ac410_doctor_sandbox_check_is_info_outside_and_warn_inside() {
    let ws = Ws::new();
    for vars in [
        &[][..],
        &[
            ("CODEX_SANDBOX", ""),
            ("CODEX_SANDBOX_NETWORK_DISABLED", "0"),
        ],
    ] {
        let c = check(&doctor(&ws, vars), "sandbox").clone();
        assert_eq!(c["status"], "info", "{vars:?}: {c}");
        assert_eq!(c["value"], "none", "{vars:?}: {c}");
    }
    // Cycle 1: network disabled by the host without CODEX_SANDBOX (or with it empty) is info.
    for vars in [
        &[("CODEX_SANDBOX_NETWORK_DISABLED", "1")][..],
        &[
            ("CODEX_SANDBOX", ""),
            ("CODEX_SANDBOX_NETWORK_DISABLED", "1"),
        ],
    ] {
        let c = check(&doctor(&ws, vars), "sandbox").clone();
        assert_eq!(c["status"], "info", "{vars:?}: {c}");
    }
    for vars in [&OFFLINE[..], &NETWORKED[..]] {
        let c = check(&doctor(&ws, vars), "sandbox").clone();
        assert_eq!(c["status"], "warn", "{vars:?}: {c}");
        assert!(
            check_text(&c).contains("send"),
            "the detail names what cannot run: {c}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// AC-411 — help and completions.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac411_every_visible_argument_of_every_visible_command_has_help_text() {
    let cli = cli();
    assert!(!cli.top.options.is_empty());
    for o in &cli.top.options {
        assert!(
            !o.description().is_empty(),
            "glen: `{}` has no help",
            o.names.join(", ")
        );
    }
    for (cmd, page) in &cli.pages {
        assert!(
            !page.options.is_empty(),
            "{cmd}: no options parsed\n{}",
            page.text
        );
        for o in page.options.iter().chain(&page.positionals) {
            assert!(
                !o.description().is_empty(),
                "`glen {cmd}`: `{}` has no help text\n{}",
                o.names.join(", "),
                page.text
            );
        }
    }
    for cmd in ["show", "tail", "send", "wait", "completions"] {
        assert!(
            !cli.pages[cmd].positionals.is_empty(),
            "{cmd}: positionals not parsed"
        );
    }
}

#[test]
fn ac411_every_cwd_completes_as_a_directory_in_zsh() {
    let sb = Sandbox::new();
    let script = sb.glen(["completions", "zsh"]).run().out();
    let with_cwd = cli()
        .pages
        .values()
        .filter(|p| p.opt("--cwd").is_some())
        .count();
    assert!(with_cwd >= 2, "ls and send take --cwd");
    let specs: Vec<&str> = script
        .lines()
        .filter(|l| l.contains("'--cwd=") || l.contains("'--cwd+") || l.contains("'--cwd["))
        .collect();
    assert!(specs.len() >= with_cwd, "--cwd specs: {specs:?}");
    for l in specs {
        assert!(
            l.contains("_files -/"),
            "--cwd is not a directory completion: {l}"
        );
    }
}

#[test]
fn ac411_completions_help_shows_install_examples() {
    let text = &cli().pages["completions"].text;
    for shell in ["bash", "zsh", "fish"] {
        assert!(
            text.lines().any(|l| l
                .trim()
                .trim_start_matches("$ ")
                .starts_with(&format!("glen completions {shell}"))),
            "no {shell} example:\n{text}"
        );
    }
}

#[test]
fn ac411_install_skill_help_shows_the_four_examples() {
    let text = &cli().pages["install-skill"].text;
    let examples: Vec<Vec<&str>> = text
        .lines()
        .map(|l| l.split('#').next().unwrap().trim().trim_start_matches("$ "))
        .filter(|l| l.starts_with("glen install-skill"))
        .map(|l| l.split_whitespace().collect())
        .collect();
    assert!(
        examples.iter().any(|e| e.len() == 2),
        "no default example:\n{text}"
    );
    for flags in [
        &["--codex", "--project"][..],
        &["--dry-run"],
        &["--uninstall"],
    ] {
        assert!(
            examples.iter().any(|e| flags.iter().all(|f| e.contains(f))),
            "no example with {flags:?}:\n{text}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// AC-412 — docs/json.md against the real binary.
// ---------------------------------------------------------------------------------------------

/// Every contract example ID in docs/json.md that this suite runs.
const CONTRACT_IDS: [&str; 24] = [
    "ls",
    "show",
    "show.raw",
    "tail",
    "tail.follow",
    "send.completed",
    "send.running",
    "send.forked",
    "wait",
    "jobs",
    "doctor",
    "completions",
    "install-skill",
    "inbox",
    "inbox.follow",
    "install-extension",
    "error.usage",
    "error.not_found",
    "error.ambiguous",
    "error.job_failed",
    "error.no_route",
    "error.no_route.sandboxed",
    "error.omp_unavailable",
    "spilled",
];

fn json_md() -> &'static str {
    static DOC: OnceLock<String> = OnceLock::new();
    DOC.get_or_init(|| {
        fs::read_to_string(repo_root().join("docs/json.md")).expect("docs/json.md exists")
    })
}

/// id → (ndjson, documents) for every `<!-- contract: ID [ndjson] -->` block.
fn contract_blocks() -> &'static BTreeMap<String, (bool, Vec<Value>)> {
    static BLOCKS: OnceLock<BTreeMap<String, (bool, Vec<Value>)>> = OnceLock::new();
    BLOCKS.get_or_init(|| {
        let lines: Vec<&str> = json_md().lines().collect();
        let mut out = BTreeMap::new();
        let mut i = 0;
        while i < lines.len() {
            let t = lines[i].trim();
            let Some(inner) = t
                .strip_prefix("<!-- contract:")
                .and_then(|r| r.strip_suffix("-->"))
            else {
                i += 1;
                continue;
            };
            let words: Vec<&str> = inner.split_whitespace().collect();
            let id = words[0].to_owned();
            let nd = words.contains(&"ndjson");
            assert!(
                lines
                    .get(i + 1)
                    .is_some_and(|l| l.trim().starts_with("```")),
                "contract {id}: no code block follows"
            );
            let mut j = i + 2;
            let mut body = Vec::new();
            while j < lines.len() && !lines[j].trim().starts_with("```") {
                body.push(lines[j]);
                j += 1;
            }
            let docs = if nd {
                body.iter()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{id}: {e}: {l}")))
                    .collect()
            } else {
                vec![serde_json::from_str(&body.join("\n")).unwrap_or_else(|e| panic!("{id}: {e}"))]
            };
            assert!(
                out.insert(id.clone(), (nd, docs)).is_none(),
                "duplicate contract {id}"
            );
            i = j + 1;
        }
        out
    })
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// AC-412 shape rule: key sets at every depth, types where both are non-null, arrays by their
/// first elements.
fn shape_diff(ex: &Value, real: &Value, at: &str, out: &mut Vec<String>) {
    match (ex, real) {
        (Value::Object(a), Value::Object(b)) => {
            for k in a.keys().filter(|k| !b.contains_key(*k)) {
                out.push(format!(
                    "{at}.{k}: in the example, not in the real envelope"
                ));
            }
            for k in b.keys().filter(|k| !a.contains_key(*k)) {
                out.push(format!(
                    "{at}.{k}: in the real envelope, not in the example"
                ));
            }
            for (k, v) in a {
                if let Some(w) = b.get(k) {
                    shape_diff(v, w, &format!("{at}.{k}"), out);
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if let (Some(x), Some(y)) = (a.first(), b.first()) {
                shape_diff(x, y, &format!("{at}[0]"), out);
            }
        }
        (Value::Null, _) | (_, Value::Null) => {}
        _ if kind(ex) != kind(real) => {
            out.push(format!(
                "{at}: example is {}, real is {}",
                kind(ex),
                kind(real)
            ));
        }
        _ => {}
    }
}

fn assert_contract(id: &str, real: &Value) {
    let (nd, docs) = contract_blocks()
        .get(id)
        .unwrap_or_else(|| panic!("docs/json.md has no contract {id}"));
    assert!(!nd, "{id} is an NDJSON contract");
    let mut diffs = Vec::new();
    shape_diff(&docs[0], real, "$", &mut diffs);
    assert!(
        diffs.is_empty(),
        "docs/json.md `{id}` and the real envelope differ:\n{}\nreal: {real}",
        diffs.join("\n")
    );
}

#[test]
fn ac412_json_md_states_modes_envelopes_streams_exit_codes_and_stability() {
    let doc = json_md();
    let headings: Vec<&str> = doc.lines().filter(|l| l.starts_with("## ")).collect();
    for h in [
        "## Modes and streams",
        "## Exit codes",
        "## Field conventions and stability",
    ] {
        assert!(headings.contains(&h), "docs/json.md lacks {h}");
    }
    for needle in [
        "\"ok\":true",
        "\"ok\":false",
        "spilled",
        "NDJSON",
        "stable",
        "snake_case",
    ] {
        assert!(doc.contains(needle), "docs/json.md never says {needle}");
    }
}

#[test]
fn ac412_every_visible_command_and_alias_has_a_section() {
    let cli = cli();
    for c in cli.commands.iter().chain(cli.aliases.keys()) {
        let h = format!("## `glen {c}`");
        assert!(
            json_md().lines().any(|l| l.trim() == h),
            "docs/json.md has no section {h}"
        );
    }
}

#[test]
fn ac412_every_contract_example_has_a_conformance_runner() {
    let documented: BTreeSet<&str> = contract_blocks().keys().map(String::as_str).collect();
    let runners: BTreeSet<&str> = CONTRACT_IDS.into_iter().collect();
    assert_eq!(
        documented, runners,
        "contract IDs in docs/json.md vs runners here"
    );
    assert!(contract_blocks()["tail.follow"].0, "tail.follow is NDJSON");
}

#[test]
fn ac412_the_json_md_exit_table_agrees_with_design() {
    let rows = exit_rows(json_md(), "## Exit codes", 3);
    assert!(!rows.is_empty(), "no exit table in docs/json.md");
    let design = design_exit_table();
    let mut seen = BTreeSet::new();
    for (exit, codes, cols) in &rows {
        let ok = cols.get(2).map(String::as_str);
        assert_eq!(
            ok,
            Some(if OK_EXITS.contains(exit) {
                "true"
            } else {
                "false"
            }),
            "ok column for exit {exit}"
        );
        for c in codes {
            assert_eq!(design.get(c), Some(exit), "`{c}` in docs/json.md vs §A11");
            seen.insert(c.clone());
        }
    }
    for (c, e) in design {
        if !OK_EXITS.contains(e) {
            assert!(seen.contains(c), "docs/json.md exit table lacks `{c}`");
        }
    }
    for ok in OK_EXITS {
        assert!(
            rows.iter().any(|(e, _, _)| *e == ok),
            "no row for exit {ok}"
        );
    }
}

#[test]
fn ac412_the_w4_warning_codes_are_documented() {
    let rows: Vec<&str> = json_md()
        .lines()
        .skip_while(|l| l.trim() != "## Warning codes")
        .filter(|l| l.starts_with('|'))
        .collect();
    for code in [
        "skill_newer",
        "skill_modified",
        "skill_foreign",
        "skill_dir_not_empty",
        "omp_version_untested",
    ] {
        assert!(
            rows.iter().any(|r| r.contains(&format!("`{code}`"))),
            "docs/json.md lacks warning {code}"
        );
    }
}

#[test]
fn ac412_contract_ls() {
    let bx = OmpBox::new();
    let out = bx.glen(["ls", "--json"]).run();
    out.ok_data();
    assert_contract("ls", &out.json());
}

#[test]
fn ac412_contract_show_and_show_raw() {
    let bx = OmpBox::new();
    let out = bx.glen(["show", A.handle, "--json"]).run();
    out.ok_data();
    assert_contract("show", &out.json());
    let out = bx.glen(["show", A.handle, "--raw", "--json"]).run();
    out.ok_data();
    assert_contract("show.raw", &out.json());
}

#[test]
fn ac412_contract_tail() {
    let bx = OmpBox::new();
    let out = bx.glen(["tail", A.handle, "-n", "1", "--json"]).run();
    out.ok_data();
    assert_contract("tail", &out.json());
}

#[test]
fn ac412_contract_tail_follow_ndjson() {
    let bx = OmpBox::new();
    let mut st = bx.follow(&["tail", A.handle, "-n", "1", "-f", "--json"]);
    st.expect_event("session", reader::T);
    st.expect_event("item", reader::T);
    assert_stream_contract("tail.follow", st);
}

/// Interrupt a follow stream, then check its single terminal line and every event's shape.
fn assert_stream_contract(id: &str, mut st: reader::Stream) {
    st.interrupt();
    let status = st.wait(reader::T);
    assert_eq!(status.code(), Some(130), "{}", st.ctx());
    let lines: Vec<Value> = st
        .lines
        .iter()
        .map(|l| reader::parse_stream_line(l, &st.ctx()))
        .collect();
    let terminal: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, v)| v["event"] == "end" || v["ok"] == false)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        terminal,
        [lines.len() - 1],
        "exactly one terminal line, last\n{}",
        st.ctx()
    );
    let (_, examples) = &contract_blocks()[id];
    for real in &lines {
        let ex = examples
            .iter()
            .find(|e| e["event"] == real["event"])
            .unwrap_or_else(|| panic!("no example for event {}", real["event"]));
        let mut diffs = Vec::new();
        shape_diff(ex, real, "$", &mut diffs);
        assert!(
            diffs.is_empty(),
            "{id} {}:\n{}",
            real["event"],
            diffs.join("\n")
        );
    }
}

#[test]
fn ac412_contract_inbox_and_inbox_follow() {
    let bx = OmpBox::new();
    let dir = bx.glen_home.join("inbox");
    fs::create_dir_all(&dir).unwrap();
    let record = serde_json::json!({"v":1,"id":"r1","session_id":A.id,"job_id":"01a18bcc-6970-7685-8f03-0ecc16977273","request_id":null,"tool_call_id":"call_1","text":"reply: hi","final":true,"at":"2026-10-30T07:53:12.332571Z"});
    fs::write(dir.join(format!("{}.ndjson", A.id)), format!("{record}\n")).unwrap();
    let out = bx.glen(["inbox", "--peek", "--json"]).run();
    out.ok_data();
    assert_contract("inbox", &out.json());
    let mut st = bx.follow(&["inbox", "-f", "--peek", "--json"]);
    st.expect_event("message", reader::T);
    assert_stream_contract("inbox.follow", st);
}

#[test]
fn ac412_contract_install_extension() {
    let ws = Ws::new();
    let out = ws.glen(["install-extension", "--dry-run", "--json"]).run();
    out.ok_data();
    assert_contract("install-extension", &out.json());
    ws.assert_no_side_effects("install-extension --dry-run");
}

#[test]
fn ac412_contract_spilled_points_at_the_full_envelope() {
    let bx = OmpBox::new();
    let out = bx
        .glen(["show", A.handle, "--json"])
        .env("GLEN_MAX_OUTPUT", "100")
        .run();
    let env = out.ok_envelope();
    assert_contract("spilled", &env);
    let sp = &env["spilled"];
    let path = PathBuf::from(sp["path"].as_str().expect("spilled.path"));
    let md = fs::metadata(&path).expect("the spill file exists");
    assert_eq!(
        Some(md.len()),
        sp["bytes"].as_u64(),
        "spilled.bytes is the file size"
    );
    assert_eq!(mode(&path), 0o600);
    let full: Value = serde_json::from_slice(&fs::read(&path).unwrap()).expect("JSON spill");
    assert_eq!(full["v"], 1);
    assert_eq!(full["ok"], true);
    assert!(
        full["data"].is_object(),
        "the spill holds a full success envelope"
    );
}

#[test]
fn ac412_contract_reader_errors() {
    let bx = OmpBox::new();
    let out = bx.glen(["show", "fffffff", "--json"]).run();
    failed(&out, "not_found");
    assert_contract("error.not_found", &out.json());
    let out = bx.glen(["show", "c0de", "--json"]).run();
    failed(&out, "ambiguous");
    assert_contract("error.ambiguous", &out.json());
    let out = bx.glen(["ls", "--bogus", "--json"]).run();
    failed(&out, "usage");
    assert_contract("error.usage", &out.json());
}

#[test]
fn ac412_contract_send_completed_and_jobs() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .run();
    data_of(&out, 0);
    assert_contract("send.completed", &out.json());
    rb.assert_quiet();
    // The `jobs` example has a row, so compare it with a real, non-empty list.
    let out = rb.glen(["jobs", "--json"]).run();
    let data = out.ok_data();
    assert!(
        data["jobs"].as_array().is_some_and(|j| !j.is_empty()),
        "{}",
        out.ctx()
    );
    assert_contract("jobs", &out.json());
}

#[test]
fn ac412_contract_send_running_then_wait() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--no-wait", "--json"])
        .run();
    let d = data_of(&out, 10);
    assert_contract("send.running", &out.json());
    let id = job_id(&d);
    let out = rb
        .glen(["wait", &job_handle(&id), "--timeout", "20s", "--json"])
        .run();
    data_of(&out, 0);
    assert_contract("wait", &out.json());
    rb.assert_quiet();
}

#[test]
fn ac412_contract_send_forked() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "fork")
        .run();
    data_of(&out, 11);
    assert_contract("send.forked", &out.json());
    rb.assert_quiet();
}

#[test]
fn ac412_contract_job_failed() {
    let rb = RpcBox::new();
    let out = rb
        .glen(["send", A.handle, "hi", "--wait", "20s", "--json"])
        .env("FAKE_OMP_SCENARIO", "error")
        .run();
    failed(&out, "job_failed");
    assert_contract("error.job_failed", &out.json());
    rb.assert_quiet();
}

#[test]
fn ac412_contract_no_route_and_sandboxed() {
    let rb = RpcBox::new();
    let lease = hold_lease(&rb.bx.owners(), A.id);
    let out = rb.glen(["send", A.handle, "hi", "--json"]).run();
    failed(&out, "no_route");
    assert_contract("error.no_route", &out.json());
    drop(lease);
    let out = envs(rb.glen(["send", A.handle, "hi", "--json"]), &OFFLINE).run();
    assert_sandboxed(&out, Some("seatbelt"), false);
    assert_contract("error.no_route.sandboxed", &out.json());
}

#[test]
fn ac412_contract_doctor_and_omp_unavailable() {
    let ws = Ws::new();
    let out = ws.glen(["doctor", "--json"]).fake_omp().run();
    out.ok_data();
    assert_contract("doctor", &out.json());
    let out = ws.glen(["doctor", "--json"]).run();
    failed(&out, "omp_unavailable");
    assert_contract("error.omp_unavailable", &out.json());
}

#[test]
fn ac412_contract_completions_and_install_skill() {
    let ws = Ws::new();
    let out = ws.glen(["completions", "zsh", "--json"]).run();
    out.ok_data();
    assert_contract("completions", &out.json());
    let out = ws
        .glen(["install-skill", "--project", "--dry-run", "--json"])
        .run();
    skill_env(&out);
    assert_contract("install-skill", &out.json());
    // Without a GLEN_HOME, `jobs` is an empty list and creates nothing.
    let data = ws.glen(["jobs", "--json"]).run().ok_data();
    assert_eq!(data["jobs"], Value::Array(vec![]), "{data}");
    assert_eq!(data["total"], 0, "{data}");
    ws.assert_no_side_effects("contracts");
}

// ---------------------------------------------------------------------------------------------
// AC-413 — skill content and size.
// ---------------------------------------------------------------------------------------------

/// Markdown items: a heading, a list item (with its continuation lines) or a paragraph.
fn items(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        let t = line.trim_start();
        let starts = t.is_empty()
            || t.starts_with('#')
            || t.starts_with("- ")
            || t.starts_with('|')
            || t.split_once(". ")
                .is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()));
        if starts && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if !t.is_empty() {
            cur.push_str(t);
            cur.push(' ');
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[test]
fn ac413_skill_sections_required_terms_and_size() {
    for agent in AGENTS {
        let t = rendered_text(agent);
        let headings: Vec<&str> = t
            .lines()
            .filter_map(|l| l.strip_prefix("## "))
            .map(str::trim)
            .collect();
        for h in [
            "The loop",
            "Exit codes",
            "Output",
            "Sending well",
            "Shell timeouts and sandboxes",
            "Rules",
        ] {
            assert!(
                headings.contains(&h),
                "{agent}: no `## {h}` in {headings:?}"
            );
        }
        for term in [
            "glen_reply",
            "--steer",
            "--follow-up",
            "glen wait",
            "data.forked.to_handle",
            "spilled",
        ] {
            assert!(t.contains(term), "{agent}: the skill never names {term}");
        }
        let items = items(t);
        let dash_rule = items
            .iter()
            .any(|p| p.contains("`-`") && p.contains("`--`"))
            || invocations(t).iter().any(|inv| {
                let w = shell_words(inv);
                w.windows(2)
                    .any(|p| p[0] == "--" && p[1].starts_with('-') && p[1].len() > 1)
            });
        assert!(dash_rule, "{agent}: a message starting with `-` needs `--`");
        assert!(
            items.iter().any(|p| {
                let l = p.to_lowercase();
                l.contains("10")
                    && l.contains("130")
                    && l.contains("cut")
                    && (l.contains("never") || l.contains("don't") || l.contains("do not"))
            }),
            "{agent}: never resend after exit 10, exit 130 or a cut-off call"
        );
        assert!(
            t.lines().count() <= 160,
            "{agent}: {} lines",
            t.lines().count()
        );
        assert!(t.len() <= 12 * 1024, "{agent}: {} bytes", t.len());
    }
}

// ---------------------------------------------------------------------------------------------
// AC-419 — no W4 code path writes under the omp agent dir.
// ---------------------------------------------------------------------------------------------

#[test]
fn ac419_w4_code_paths_never_touch_the_omp_agent_dir() {
    let bx = OmpBox::new();
    let before = snapshot(&bx.agent);
    for args in [
        &["install-skill", "--json"][..],
        &["install-skill", "--project", "--json"],
        &["install-skill", "--force", "--json"],
        &["install-skill", "--dry-run", "--json"],
        &["install-skill", "--uninstall", "--force", "--json"],
        &["install-skill", "--project", "--uninstall", "--json"],
    ] {
        skill_env(&bx.glen(args).run());
    }
    bx.glen(["doctor", "--json"]).fake_omp().run().ok_data();
    envs(bx.glen(["doctor", "--json"]).fake_omp(), &OFFLINE)
        .run()
        .ok_data();
    let out = envs(
        bx.glen(["send", A.handle, "hi", "--json"]).fake_omp(),
        &OFFLINE,
    )
    .run();
    failed(&out, "no_route");
    assert_eq!(snapshot(&bx.agent), before, "the omp agent dir changed");
    assert!(!bx.home.join(".omp").exists(), "~/.omp was created");
}

/// The AC-412 shape rule used above flags each kind of drift (so passing contracts are not
/// vacuous): a missing key, an extra key, a nested key and a changed type.
#[test]
fn ac412_the_shape_rule_flags_key_and_type_drift() {
    let (_, docs) = &contract_blocks()["doctor"];
    let ex = &docs[0];
    let mut same = Vec::new();
    shape_diff(ex, ex, "$", &mut same);
    assert!(same.is_empty(), "{same:?}");
    let mut missing = ex.clone();
    missing["data"].as_object_mut().unwrap().remove("skills");
    let mut extra = ex.clone();
    extra["data"]["surprise"] = Value::from(1);
    let mut nested = ex.clone();
    nested["data"]["skills"]["claude"]
        .as_object_mut()
        .unwrap()
        .remove("project");
    let mut retyped = ex.clone();
    retyped["data"]["glen"]["home_exists"] = Value::from("no");
    for (what, real, token) in [
        ("missing key", missing, "skills"),
        ("extra key", extra, "surprise"),
        ("nested key", nested, "project"),
        ("changed type", retyped, "home_exists"),
    ] {
        let mut diffs = Vec::new();
        shape_diff(ex, &real, "$", &mut diffs);
        assert!(
            diffs.iter().any(|d| d.contains(token)),
            "{what}: no diff names {token}: {diffs:?}"
        );
    }
}
