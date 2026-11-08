//! omp's tool approval mode for one agent dir (`tools.approvalMode`, audit S-01), read from omp's
//! config files without starting omp. omp 18.14.0:
//!
//! - the setting is an enum defaulting to `yolo` (`packages/coding-agent/src/tools/settings.ts`),
//!   and a value outside the enum falls back to that default (`config/registry.ts`);
//! - the global layer is the first of `config.yml`, `config.yaml` that exists in the agent dir
//!   (`MAIN_CONFIG_FILENAMES`, `packages/utils/src/dirs.ts`);
//! - `PI_CONFIG_FILES` (split on the platform path delimiter, `~` expanded, relative to the cwd)
//!   are overlays deep-merged in order above the global and project layers; a missing or
//!   malformed overlay is a hard error (`config/settings.ts`, `#loadOverlayYaml`);
//! - a project's `.omp/config.yml` or `.claude/settings.json`, `--approval-mode` and `--yolo`
//!   also change it per session. glen passes no approval flag; the project layer depends on the
//!   session's cwd, which doctor does not know.
//!
//! Anything glen cannot read with certainty is `unknown`, never a guessed mode: a file that is not
//! a regular file, too large, not UTF-8, not valid YAML (omp would move a broken `config.yml`
//! aside and fall back to defaults), not a single mapping document, with a duplicate key, or with
//! a non-string `approvalMode`.
use crate::env::Env;
use serde::Serialize;
use std::{
    io::Read,
    path::{Path, PathBuf},
};
use yaml_rust2::{Yaml, YamlLoader};

pub const SETTING: &str = "tools.approvalMode";
pub const MODES: [&str; 4] = ["always-ask", "always-ask-cached", "write", "yolo"];
pub const DEFAULT: &str = "yolo";
const FILES: [&str; 2] = ["config.yml", "config.yaml"];
/// A larger "config" is not one omp wrote; it reads as unknown rather than being parsed.
const MAX_BYTES: u64 = 4 << 20;

/// `mode` is one of [`MODES`] or `unknown`; `source` is the config file that decided it,
/// `default` (unset or invalid, so omp uses [`DEFAULT`]) or `unknown`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Approval {
    pub mode: String,
    pub source: String,
    /// Why the mode is a fallback (invalid value) or unknown; human output only.
    #[serde(skip)]
    pub note: Option<String>,
}
impl Approval {
    pub fn is_yolo(&self) -> bool {
        self.mode == "yolo"
    }
    pub fn is_unknown(&self) -> bool {
        self.mode == "unknown"
    }
    fn default_mode(note: Option<String>) -> Self {
        Self {
            mode: DEFAULT.into(),
            source: "default".into(),
            note,
        }
    }
    fn unknown(note: String) -> Self {
        Self {
            mode: "unknown".into(),
            source: "unknown".into(),
            note: Some(note),
        }
    }
}

/// The agent dir's main config file: the first that exists, else `config.yml`.
pub fn config_file(agent_dir: &Path) -> PathBuf {
    FILES
        .iter()
        .map(|name| agent_dir.join(name))
        .find(|p| std::fs::symlink_metadata(p).is_ok())
        .unwrap_or_else(|| agent_dir.join(FILES[0]))
}

/// `PI_CONFIG_FILES` as omp resolves it: split on the path delimiter, empty entries dropped,
/// `~` expanded against HOME, relative paths joined to the cwd.
fn overlays(env: &Env) -> Vec<PathBuf> {
    let delimiter = if cfg!(windows) { ';' } else { ':' };
    let Some(raw) = env.get("PI_CONFIG_FILES") else {
        return vec![];
    };
    raw.split(delimiter)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let expanded = match (s.strip_prefix('~'), env.home()) {
                (Some(rest), Some(home)) => home.join(rest.trim_start_matches(['/', '\\'])),
                _ => PathBuf::from(s),
            };
            env.cwd().join(expanded)
        })
        .collect()
}

/// The approval mode omp uses for sessions under `agent_dir`, before project config and flags.
pub fn read(env: &Env, agent_dir: &Path) -> Approval {
    let mut layers = vec![];
    let global = config_file(agent_dir);
    if std::fs::symlink_metadata(&global).is_ok() {
        layers.push(global);
    }
    layers.extend(overlays(env));
    // omp's deep merge, reduced to `tools.approvalMode`: a mapping `tools` merges into a mapping
    // below it, anything else (null included) replaces it; a null leaf replaces and reads unset.
    let mut mode: Option<(String, String)> = None;
    let mut tools_is_map = false;
    for path in layers {
        let shown = path.to_string_lossy().into_owned();
        let root = match load(&path) {
            Ok(root) => root,
            Err(why) => return Approval::unknown(format!("cannot read {shown}: {why}")),
        };
        let Some(tools) = root
            .as_hash()
            .and_then(|h| h.get(&Yaml::String("tools".into())))
        else {
            continue;
        };
        let Yaml::Hash(tools) = tools else {
            (mode, tools_is_map) = (None, false);
            continue;
        };
        if !tools_is_map {
            mode = None;
        }
        tools_is_map = true;
        match tools.get(&Yaml::String("approvalMode".into())) {
            None => {}
            Some(Yaml::Null) => mode = None,
            Some(Yaml::String(value)) => mode = Some((value.clone(), shown)),
            Some(_) => {
                return Approval::unknown(format!("{SETTING} in {shown} is not a string"));
            }
        }
    }
    match mode {
        None => Approval::default_mode(None),
        Some((value, source)) if MODES.contains(&value.as_str()) => Approval {
            mode: value,
            source,
            note: None,
        },
        Some((value, source)) => Approval::default_mode(Some(format!(
            "omp ignores the invalid {SETTING} {value:?} in {source} and uses its default"
        ))),
    }
}

/// One config file as a YAML mapping (an empty document is an empty mapping).
fn load(path: &Path) -> Result<Yaml, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    let mut bytes = vec![];
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("larger than {MAX_BYTES} bytes"));
    }
    let text = String::from_utf8(bytes).map_err(|_| "not UTF-8".to_string())?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut docs = YamlLoader::load_from_str(text).map_err(|e| format!("invalid YAML: {e}"))?;
    match docs.len() {
        0 => return Ok(Yaml::Hash(Default::default())),
        1 => {}
        _ => return Err("more than one YAML document".into()),
    }
    match docs.remove(0) {
        Yaml::Null => Ok(Yaml::Hash(Default::default())),
        root @ Yaml::Hash(_) => Ok(root),
        _ => Err("the document root is not a mapping".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn env(dir: &Path, vars: &[(&str, &str)]) -> Env {
        Env::for_test(Some(&dir.join("home")), &dir.join("cwd"), vars)
    }
    /// `(mode, source)` for one agent-dir `config.yml`.
    fn mode_of(text: &str) -> (String, String) {
        let t = tempfile::tempdir().unwrap();
        let yml = t.path().join("config.yml");
        std::fs::write(&yml, text).unwrap();
        let a = read(&env(t.path(), &[]), t.path());
        let source = if a.source == yml.to_string_lossy() {
            "file".into()
        } else {
            a.source
        };
        (a.mode, source)
    }
    fn is(mode: &str, source: &str) -> (String, String) {
        (mode.into(), source.into())
    }
    #[test]
    fn reads_valid_documents() {
        assert_eq!(
            mode_of("tools:\n  approvalMode: write\n"),
            is("write", "file")
        );
        assert_eq!(
            mode_of(
                "---\n# c\nmodelRoles:\n  default: x\nplugins:\n- a\ntools:\n    nested:\n      approvalMode: yolo\n    approvalMode: \"always-ask\"  # why\n"
            ),
            is("always-ask", "file")
        );
        assert_eq!(
            mode_of("tools: {approvalMode: write}\n"),
            is("write", "file")
        );
        assert_eq!(
            mode_of("'tools':\n  'approvalMode': 'yolo'\n"),
            is("yolo", "file")
        );
        assert_eq!(
            mode_of("\u{feff}tools:\r\n  approvalMode: always-ask-cached\r\n"),
            is("always-ask-cached", "file")
        );
        assert_eq!(
            mode_of("x: &t {approvalMode: write}\ntools: *t\n"),
            is("write", "file")
        );
    }
    #[test]
    fn unset_null_and_invalid_values_are_omps_default() {
        for text in [
            "",
            "# only a comment\n",
            "~\n",
            "theme: dark\n",
            "tools:\n  approval:\n    bash: deny\n",
            "tools: ~\n",
            "tools: off\n",
            "tools:\n  approvalMode: null\n",
            "tools.approvalMode: write\n",
            "tools:\n  approvalMode: bogus\n",
            // A continuation line folds into the scalar: "write yolo" is not a mode.
            "tools:\n  approvalMode: write\n    yolo\n",
            "tools:\n  approvalMode: \"write\u{a0}\"\n",
            "tools:\n  approvalMode: write\u{a0}\n",
        ] {
            assert_eq!(mode_of(text), is("yolo", "default"), "{text:?}");
        }
    }
    #[test]
    fn anything_uncertain_is_unknown() {
        for text in [
            "foo: bar: baz\ntools:\n  approvalMode: write\n",
            "foo: \"abc\ntools:\n  approvalMode: write\n",
            "models:\n  x: [unclosed\ntools:\n  approvalMode: write\n",
            "tools:\n  approvalMode: write\ntools:\n  approvalMode: write\n",
            "a: 1\na: 2\ntools:\n  approvalMode: write\n",
            "tools:\n  approvalMode: 1\n",
            "tools:\n  approvalMode: [write]\n",
            "tools:\n\tapprovalMode: write\n",
            "- a\n- b\n",
            "just a string\n",
            "tools:\n  approvalMode: write\n---\ntheme: dark\n",
        ] {
            assert_eq!(mode_of(text), is("unknown", "unknown"), "{text:?}");
        }
    }
    #[test]
    fn reads_agent_dirs() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path();
        let e = env(dir, &[]);
        assert_eq!(read(&e, &dir.join("missing")), Approval::default_mode(None));
        assert_eq!(read(&e, dir), Approval::default_mode(None));
        let yaml = dir.join("config.yaml");
        std::fs::write(&yaml, "tools:\n  approvalMode: write\n").unwrap();
        let a = read(&e, dir);
        assert_eq!(
            (a.mode.as_str(), a.source.as_str()),
            ("write", yaml.to_str().unwrap())
        );
        // config.yml wins over config.yaml, even when it leaves the setting unset.
        let yml = dir.join("config.yml");
        std::fs::write(&yml, "theme: dark\n").unwrap();
        assert_eq!(read(&e, dir), Approval::default_mode(None));
        std::fs::write(&yml, "tools:\n  approvalMode: sometimes\n").unwrap();
        let a = read(&e, dir);
        assert_eq!((a.mode.as_str(), a.source.as_str()), ("yolo", "default"));
        assert!(a.note.unwrap().contains("\"sometimes\""));
        std::fs::write(&yml, b"tools:\n  approvalMode: \xff\n").unwrap();
        assert!(read(&e, dir).note.unwrap().contains("not UTF-8"));
        // Oversize: the cap holds during the read, not only by metadata.
        let big = format!(
            "tools:\n  approvalMode: write\n#{}\n",
            "x".repeat(MAX_BYTES as usize)
        );
        std::fs::write(&yml, big).unwrap();
        let a = read(&e, dir);
        assert!(a.is_unknown() && a.note.unwrap().contains("larger than"));
        std::fs::remove_file(&yml).unwrap();
        std::fs::create_dir(&yml).unwrap();
        let a = read(&e, dir);
        assert!(a.is_unknown() && a.note.unwrap().contains("not a regular file"));
    }
    #[cfg(unix)]
    #[test]
    fn a_fifo_is_unknown_and_never_opened() {
        let t = tempfile::tempdir().unwrap();
        let yml = t.path().join("config.yml");
        {
            use std::os::unix::ffi::OsStrExt;
            let c = std::ffi::CString::new(yml.as_os_str().as_bytes()).unwrap();
            // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        }
        // Opening a FIFO with no writer would block; the type check comes first.
        let a = read(&env(t.path(), &[]), t.path());
        assert!(a.is_unknown() && a.note.unwrap().contains("not a regular file"));
        // A symlink to a regular file is followed.
        std::fs::remove_file(&yml).unwrap();
        let real = t.path().join("real.yml");
        std::fs::write(&real, "tools:\n  approvalMode: write\n").unwrap();
        std::os::unix::fs::symlink(&real, &yml).unwrap();
        assert_eq!(read(&env(t.path(), &[]), t.path()).mode, "write");
    }
    #[test]
    fn config_file_overlays_merge_above_the_agent_dir() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        std::fs::create_dir_all(dir.join("cwd")).unwrap();
        std::fs::write(dir.join("config.yml"), "tools:\n  approvalMode: write\n").unwrap();
        let a_yml = dir.join("home/a.yml");
        let b_yml = dir.join("cwd/b.yml");
        std::fs::write(&a_yml, "tools:\n  approvalMode: always-ask\n").unwrap();
        std::fs::write(&b_yml, "tools:\n  other: 1\n").unwrap();
        // `~/a.yml` and a cwd-relative `b.yml`; the last overlay that sets the key wins.
        let e = env(dir, &[("PI_CONFIG_FILES", "~/a.yml::b.yml")]);
        let a = read(&e, dir);
        assert_eq!(
            (a.mode.as_str(), a.source.as_str()),
            ("always-ask", a_yml.to_str().unwrap())
        );
        std::fs::write(&b_yml, "tools:\n  approvalMode: yolo\n").unwrap();
        let a = read(&e, dir);
        assert_eq!(
            (a.mode.as_str(), a.source.as_str()),
            ("yolo", b_yml.to_str().unwrap())
        );
        // A null leaf or a non-mapping `tools` above resets to omp's default.
        std::fs::write(&b_yml, "tools:\n  approvalMode: null\n").unwrap();
        assert_eq!(read(&e, dir), Approval::default_mode(None));
        std::fs::write(&b_yml, "tools: ~\n").unwrap();
        assert_eq!(read(&e, dir), Approval::default_mode(None));
        // A mapping over a replaced `tools` starts empty: the agent dir's write is gone.
        std::fs::write(&a_yml, "tools: off\n").unwrap();
        std::fs::write(&b_yml, "tools:\n  other: 1\n").unwrap();
        assert_eq!(read(&e, dir), Approval::default_mode(None));
        // A missing or malformed overlay is unknown (omp refuses to start).
        let e = env(dir, &[("PI_CONFIG_FILES", "missing.yml")]);
        assert!(read(&e, dir).is_unknown());
        std::fs::write(&b_yml, "tools: [\n").unwrap();
        let e = env(dir, &[("PI_CONFIG_FILES", "b.yml")]);
        assert!(read(&e, dir).is_unknown());
        // Overlays apply even with no agent-dir config.
        std::fs::remove_file(dir.join("config.yml")).unwrap();
        std::fs::write(&b_yml, "tools:\n  approvalMode: write\n").unwrap();
        assert_eq!(read(&e, dir).mode, "write");
    }
}
