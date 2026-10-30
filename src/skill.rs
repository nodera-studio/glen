//! A block guard is an exact `<!-- glen:if NAME[ NAME…] -->` line, closed by
//! `<!-- glen:end -->`. Blocks cannot nest. A line guard uses the same opening
//! comment after one space at the end of a line. Names match [a-z][a-z-]*.
use crate::{
    cli,
    env::Env,
    error::{ErrorCode, GlenError},
};
use std::{
    cmp::Ordering,
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};
use xxhash_rust::xxh64::xxh64;
pub const TEMPLATE: &str = include_str!("../skill/glen/SKILL.md");
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}
impl Agent {
    pub const ALL: [Self; 2] = [Self::Claude, Self::Codex];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}
#[derive(Debug)]
pub struct Segment {
    pub guards: Vec<String>,
    pub text: String,
}
fn names(s: &str) -> Result<Vec<String>, String> {
    let result: Vec<_> = s.split_whitespace().map(str::to_owned).collect();
    if result.is_empty()
        || result.iter().any(|n| {
            !n.as_bytes()[0].is_ascii_lowercase()
                || !n.bytes().all(|c| c.is_ascii_lowercase() || c == b'-')
        })
    {
        return Err(format!("invalid guard names: {s}"));
    }
    Ok(result)
}
pub fn parse_template(t: &str) -> Result<Vec<Segment>, String> {
    let mut segments = Vec::new();
    let mut block: Option<Vec<String>> = None;
    for line in t.lines() {
        if line == "<!-- glen:end -->" {
            if block.take().is_none() {
                return Err("unbalanced glen:end".into());
            }
            continue;
        }
        if let Some(s) = line
            .strip_prefix("<!-- glen:if ")
            .and_then(|s| s.strip_suffix(" -->"))
        {
            if block.is_some() {
                return Err("nested glen:if".into());
            }
            block = Some(names(s)?);
            continue;
        }
        let (text, guards) = if let Some((text, s)) = line.rsplit_once(" <!-- glen:if ") {
            if block.is_some() {
                return Err("nested line guard".into());
            }
            (
                text,
                names(s.strip_suffix(" -->").ok_or("malformed glen:if")?)?,
            )
        } else {
            (line, block.clone().unwrap_or_default())
        };
        if text.contains("glen:if") || text.contains("glen:end") {
            return Err(format!("malformed guard: {line}"));
        }
        segments.push(Segment {
            guards,
            text: format!("{text}\n"),
        });
    }
    if block.is_some() {
        return Err("unclosed glen:if".into());
    }
    Ok(segments)
}
pub fn visible_commands() -> BTreeSet<String> {
    let cmd = cli::public_command();
    cmd.get_subcommands()
        .filter(|c| c.get_name() != "help")
        .flat_map(|c| std::iter::once(c.get_name()).chain(c.get_visible_aliases()))
        .map(str::to_owned)
        .collect()
}
pub fn render(
    agent: Agent,
    version: &str,
    visible: &BTreeSet<String>,
) -> Result<String, GlenError> {
    let err = |s| GlenError::new(ErrorCode::Internal, s);
    let mut lines = Vec::new();
    for segment in parse_template(TEMPLATE).map_err(err)? {
        if segment.guards.iter().all(|g| visible.contains(g)) {
            for line in segment.text.lines() {
                if agent == Agent::Codex && line.starts_with("allowed-tools:") {
                    continue;
                }
                if line.is_empty() && lines.last().is_some_and(String::is_empty) {
                    continue;
                }
                lines.push(line.to_owned());
            }
        }
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    if lines.first().map(String::as_str) != Some("---") {
        return Err(err("skill frontmatter missing".into()));
    }
    let close = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| l.as_str() == "---")
        .map(|(i, _)| i)
        .ok_or_else(|| err("skill frontmatter unclosed".into()))?;
    let content = format!("{}\n", lines.join("\n"));
    let marker = format!(
        "<!-- glen-skill version={version} agent={} digest={:016x} -->",
        agent.as_str(),
        xxh64(content.as_bytes(), 0)
    );
    lines.insert(close + 1, marker);
    Ok(format!("{}\n", lines.join("\n")))
}
pub fn render_current(agent: Agent) -> Result<String, GlenError> {
    render(agent, env!("CARGO_PKG_VERSION"), &visible_commands())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    NotInstalled,
    Current,
    Outdated,
    Newer,
    Modified,
    Foreign,
}
impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Current => "current",
            Self::Outdated => "outdated",
            Self::Newer => "newer",
            Self::Modified => "modified",
            Self::Foreign => "foreign",
        }
    }
}
#[derive(Debug)]
pub struct Inspection {
    pub state: State,
    pub version: Option<String>,
    pub detail: String,
    pub unsafe_path: bool,
}
fn inspection(state: State, version: Option<String>, detail: impl Into<String>) -> Inspection {
    Inspection {
        state,
        version,
        detail: detail.into(),
        unsafe_path: false,
    }
}
fn numeric(v: &str) -> Option<Vec<u64>> {
    v.split('.')
        .map(|s| {
            if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
                s.parse().ok()
            } else {
                None
            }
        })
        .collect()
}
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    match (numeric(a), numeric(b)) {
        (Some(mut a), Some(mut b)) => {
            let n = a.len().max(b.len());
            a.resize(n, 0);
            b.resize(n, 0);
            a.cmp(&b)
        }
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
fn marker(line: &str) -> Option<(&str, &str, &str)> {
    let s = line
        .strip_prefix("<!-- glen-skill version=")?
        .strip_suffix(" -->")?;
    let (version, s) = s.split_once(" agent=")?;
    let (agent, digest) = s.split_once(" digest=")?;
    if version.is_empty()
        || version.chars().any(char::is_whitespace)
        || !matches!(agent, "claude" | "codex")
        || digest.len() != 16
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    Some((version, agent, digest))
}
pub fn inspect(path: &Path, agent: Agent, rendered: &str) -> io::Result<Inspection> {
    let scope = path
        .parent()
        .and_then(Path::parent)
        .unwrap_or(Path::new("/"));
    inspect_scoped(path, scope, agent, rendered)
}
pub fn inspect_scoped(
    path: &Path,
    scope_root: &Path,
    agent: Agent,
    rendered: &str,
) -> io::Result<Inspection> {
    let mut current = scope_root.to_path_buf();
    if let Some(parent) = path.parent() {
        for c in parent
            .strip_prefix(scope_root)
            .map_err(io::Error::other)?
            .components()
        {
            current.push(c);
            match fs::symlink_metadata(&current) {
                Ok(m) if m.file_type().is_symlink() => {
                    let mut i = inspection(
                        State::Foreign,
                        None,
                        format!("{} is a symlink", current.display()),
                    );
                    i.unsafe_path = true;
                    return Ok(i);
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Ok(inspection(State::NotInstalled, None, "not installed"));
                }
                Err(e) => return Err(e),
            }
        }
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(inspection(State::NotInstalled, None, "not installed"));
        }
        Err(e) => return Err(e),
    };
    if meta.file_type().is_symlink() {
        return Ok(inspection(State::Foreign, None, "SKILL.md is a symlink"));
    }
    let content = fs::read_to_string(path)?;
    let found = content
        .split_inclusive('\n')
        .enumerate()
        .find_map(|(i, l)| marker(l.trim_end_matches('\n')).map(|m| (i, m)));
    let Some((index, (version, installed_agent, digest))) = found else {
        return Ok(inspection(State::Foreign, None, "not written by glen"));
    };
    let v = Some(version.to_owned());
    if installed_agent != agent.as_str() {
        return Ok(inspection(
            State::Foreign,
            v,
            format!("installed for {installed_agent}"),
        ));
    }
    let without: String = content
        .split_inclusive('\n')
        .enumerate()
        .filter(|(i, _)| *i != index)
        .map(|(_, l)| l)
        .collect();
    if format!("{:016x}", xxh64(without.as_bytes(), 0)) != digest {
        return Ok(inspection(State::Modified, v, "edited by hand"));
    }
    if content == rendered {
        return Ok(inspection(State::Current, v, "current"));
    }
    let current_version = rendered
        .lines()
        .find_map(marker)
        .map(|m| m.0)
        .unwrap_or(env!("CARGO_PKG_VERSION"));
    Ok(match compare_versions(version, current_version) {
        Ordering::Greater => inspection(
            State::Newer,
            v,
            format!("written by a newer glen {version}"),
        ),
        Ordering::Less => inspection(State::Outdated, v, format!("installed {version}")),
        Ordering::Equal => inspection(State::Outdated, v, format!("another build of {version}")),
    })
}
pub fn user_target(env: &Env, agent: Agent) -> Result<PathBuf, GlenError> {
    let home = env
        .home()
        .ok_or_else(|| GlenError::new(ErrorCode::InvalidEnv, "HOME is not set"))?;
    Ok(match agent {
        Agent::Claude => env
            .non_empty("CLAUDE_CONFIG_DIR")
            .map(|s| crate::omp::paths::absolute(env, s))
            .unwrap_or_else(|| home.join(".claude")),
        Agent::Codex => home.join(".agents"),
    }
    .join("skills/glen/SKILL.md"))
}
pub fn project_target(cwd: &Path, agent: Agent) -> PathBuf {
    cwd.join(match agent {
        Agent::Claude => ".claude",
        Agent::Codex => ".agents",
    })
    .join("skills/glen/SKILL.md")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grammar() {
        assert!(parse_template(TEMPLATE).is_ok());
        for text in [
            "<!-- glen:end -->",
            "<!-- glen:if inbox -->",
            "<!-- glen:if inbox -->\n<!-- glen:if abort -->\n<!-- glen:end -->",
            "x <!-- glen:if BAD -->",
            "<!-- glen:if -->",
        ] {
            assert!(parse_template(text).is_err(), "{text}");
        }
        let segments = parse_template(
            "a\nx <!-- glen:if inbox -->\n<!-- glen:if abort stop -->\ny\n<!-- glen:end -->",
        )
        .unwrap();
        assert_eq!(segments[1].guards, ["inbox"]);
        assert_eq!(segments[1].text, "x\n");
        assert_eq!(segments[2].guards, ["abort", "stop"]);
    }
    #[test]
    fn frontmatter_digest_and_guards() {
        let mut bodies = Vec::new();
        for agent in Agent::ALL {
            let text = render_current(agent).unwrap();
            assert!(text.starts_with("---\nname: glen\n"));
            assert!(!text.contains("glen:if"));
            assert!(!text.contains("\n\n\n"));
            let lines: Vec<_> = text.lines().collect();
            let end = lines
                .iter()
                .enumerate()
                .skip(1)
                .find(|(_, l)| **l == "---")
                .unwrap()
                .0;
            let keys: Vec<_> = lines[1..end]
                .iter()
                .map(|l| l.split(':').next().unwrap())
                .collect();
            assert_eq!(
                keys,
                if agent == Agent::Claude {
                    vec!["name", "description", "allowed-tools"]
                } else {
                    vec!["name", "description"]
                }
            );
            let description: String =
                serde_json::from_str(lines[2].strip_prefix("description: ").unwrap()).unwrap();
            assert!(description.len() <= 1024 && description.contains("omp"));
            let (_, a, d) = marker(lines[end + 1]).unwrap();
            assert_eq!(a, agent.as_str());
            let without: String = text
                .split_inclusive('\n')
                .filter(|l| !l.starts_with("<!-- glen-skill "))
                .collect();
            assert_eq!(format!("{:016x}", xxh64(without.as_bytes(), 0)), d);
            bodies.push(lines[end + 2..].join("\n"));
        }
        assert_eq!(bodies[0], bodies[1]);
        let empty = render(Agent::Claude, "0.1.0", &BTreeSet::new()).unwrap();
        assert!(!empty.contains("glen inbox"));
        assert!(!empty.contains("## Session controls"));
        let all = render(
            Agent::Claude,
            "0.1.0",
            &[
                "inbox",
                "install-extension",
                "abort",
                "stop",
                "models",
                "model",
                "thinking",
                "compact",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        )
        .unwrap();
        assert!(all.contains("glen inbox"));
        assert!(all.contains("## Session controls"));
        assert!(!all.contains("glen:if"));
    }
    #[test]
    fn inspection_states_and_versions() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("glen/SKILL.md");
        let current = render_current(Agent::Claude).unwrap();
        assert_eq!(
            inspect(&path, Agent::Claude, &current).unwrap().state,
            State::NotInstalled
        );
        fs::create_dir(path.parent().unwrap()).unwrap();
        for (text, state) in [
            (current.clone(), State::Current),
            (
                render(Agent::Claude, "0.0.9", &visible_commands()).unwrap(),
                State::Outdated,
            ),
            (
                render(Agent::Claude, "9.0.0", &visible_commands()).unwrap(),
                State::Newer,
            ),
            (format!("{current}edit\n"), State::Modified),
            (String::from("foreign"), State::Foreign),
            (render_current(Agent::Codex).unwrap(), State::Foreign),
        ] {
            fs::write(&path, text).unwrap();
            assert_eq!(
                inspect(&path, Agent::Claude, &current).unwrap().state,
                state
            );
        }
        for (a, b, expected) in [
            ("0.0.9", "0.1.0", Ordering::Less),
            ("0.1.10", "0.1.9", Ordering::Greater),
            ("dev", "0.1.0", Ordering::Less),
        ] {
            assert_eq!(compare_versions(a, b), expected);
        }
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_and_scope() {
        use std::os::unix::fs::symlink;
        let t = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let rendered = render_current(Agent::Claude).unwrap();
        symlink(external.path(), t.path().join("glen")).unwrap();
        let i = inspect(&t.path().join("glen/SKILL.md"), Agent::Claude, &rendered).unwrap();
        assert!(i.unsafe_path);
        assert_eq!(i.state, State::Foreign);
        let project = tempfile::tempdir().unwrap();
        fs::create_dir(project.path().join(".claude")).unwrap();
        symlink(external.path(), project.path().join(".claude/skills")).unwrap();
        assert!(
            inspect_scoped(
                &project.path().join(".claude/skills/glen/SKILL.md"),
                project.path(),
                Agent::Claude,
                &rendered
            )
            .unwrap()
            .unsafe_path
        );
        let link = t.path().join("skills");
        symlink(external.path(), &link).unwrap();
        fs::create_dir(external.path().join("glen")).unwrap();
        fs::write(external.path().join("glen/SKILL.md"), &rendered).unwrap();
        assert_eq!(
            inspect(&link.join("glen/SKILL.md"), Agent::Claude, &rendered)
                .unwrap()
                .state,
            State::Current
        );
        let file = external.path().join("file");
        symlink(&file, external.path().join("glen/link.md")).unwrap();
        assert_eq!(
            inspect(
                &external.path().join("glen/link.md"),
                Agent::Claude,
                &rendered
            )
            .unwrap()
            .state,
            State::Foreign
        );
    }
}
