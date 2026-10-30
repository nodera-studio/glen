use crate::{
    env::Env,
    error::{ErrorCode, GlenError},
};
use serde::Serialize;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct OmpPaths {
    pub profile: Option<String>,
    pub base_config_root: PathBuf,
    pub config_root: PathBuf,
    pub agent_dir: PathBuf,
    pub agent_dir_source: AgentDirSource,
    pub sessions_dir: PathBuf,
    pub sessions_dir_source: DataDirSource,
    pub session_owners_dir: PathBuf,
    pub extensions_dir: PathBuf,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentDirSource {
    Default,
    Profile,
    Env,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataDirSource {
    AgentDir,
    Xdg,
}

pub fn normalize_profile_name(raw: &str) -> Result<Option<String>, String> {
    let s = raw.trim();
    if s.is_empty() || s == "default" {
        return Ok(None);
    }
    let base = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit());
    if s.len() > 64
        || s.ends_with('.')
        || !s.as_bytes()[0].is_ascii_lowercase() && !s.as_bytes()[0].is_ascii_digit()
        || !s
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        || reserved
    {
        return Err(format!(
            "invalid omp profile {raw:?}; use 1–64 lowercase letters, digits, '.', '_' or '-', starting with a letter or digit; no trailing dot or Windows reserved basename"
        ));
    }
    Ok(Some(s.to_string()))
}

// Node path.join keeps an absolute second argument under the base. Never realpath.
fn lexical_join(base: &Path, rel: &str) -> PathBuf {
    let mut result = PathBuf::new();
    for part in base.join(rel.trim_start_matches('/')).components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            _ => result.push(part.as_os_str()),
        }
    }
    result
}
pub(crate) fn absolute(env: &Env, value: &str) -> PathBuf {
    if Path::new(value).is_absolute() {
        lexical_join(Path::new("/"), value)
    } else {
        lexical_join(env.cwd(), value)
    }
}

pub fn resolve(env: &Env, profile_flag: Option<&str>) -> Result<OmpPaths, GlenError> {
    let home = env
        .home()
        .ok_or_else(|| GlenError::new(ErrorCode::InvalidEnv, "HOME is not set"))?;
    let base_config_root = lexical_join(home, env.non_empty("PI_CONFIG_DIR").unwrap_or(".omp"));
    let profile = if let Some(raw) = profile_flag {
        normalize_profile_name(raw)
            .map_err(|m| GlenError::new(ErrorCode::Usage, format!("--profile: {m}")))?
    } else {
        let key = if env.get("OMP_PROFILE").is_some() {
            "OMP_PROFILE"
        } else {
            "PI_PROFILE"
        };
        normalize_profile_name(env.get(key).unwrap_or(""))
            .map_err(|m| GlenError::new(ErrorCode::InvalidEnv, format!("{key}: {m}")))?
    };
    let config_root = profile.as_ref().map_or_else(
        || base_config_root.clone(),
        |p| base_config_root.join("profiles").join(p),
    );
    let default_agent = config_root.join("agent");
    let derived = env
        .get("PI_PROFILE")
        .and_then(|p| normalize_profile_name(p).ok().flatten())
        .map(|p| base_config_root.join("profiles").join(p).join("agent"));
    let agent_override = env
        .non_empty("PI_CODING_AGENT_DIR")
        .filter(|raw| derived.as_deref().and_then(Path::to_str) != Some(*raw));
    let (agent_dir, agent_dir_source) = if profile.is_some() {
        (default_agent.clone(), AgentDirSource::Profile)
    } else if let Some(raw) = agent_override {
        (absolute(env, raw), AgentDirSource::Env)
    } else {
        (default_agent.clone(), AgentDirSource::Default)
    };
    let xdg_data = env
        .non_empty("XDG_DATA_HOME")
        .map(|s| absolute(env, s).join("omp"))
        .map(|p| {
            if let Some(profile) = &profile {
                p.join("profiles").join(profile)
            } else {
                p
            }
        })
        .filter(|p| agent_dir == default_agent && p.is_dir());
    let (sessions_dir, sessions_dir_source) = xdg_data.map_or_else(
        || (agent_dir.join("sessions"), DataDirSource::AgentDir),
        |p| (p.join("sessions"), DataDirSource::Xdg),
    );
    let state = env
        .non_empty("XDG_STATE_HOME")
        .map(|s| absolute(env, s).join("omp"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| base_config_root.clone());
    let session_owners_dir = state.join("run/session-owners");
    let extensions_dir = agent_dir.join("extensions");
    Ok(OmpPaths {
        profile,
        base_config_root,
        config_root,
        agent_dir,
        agent_dir_source,
        sessions_dir,
        sessions_dir_source,
        session_owners_dir,
        extensions_dir,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn resolve_vars(vars: &[(&str, &str)]) -> OmpPaths {
        resolve(
            &Env::for_test(Some(Path::new("/h")), Path::new("/w"), vars),
            None,
        )
        .unwrap()
    }
    #[test]
    fn defaults() {
        let p = resolve_vars(&[]);
        assert_eq!(p.sessions_dir, Path::new("/h/.omp/agent/sessions"));
        assert_eq!(
            p.session_owners_dir,
            Path::new("/h/.omp/run/session-owners")
        );
        assert_eq!(p.extensions_dir, Path::new("/h/.omp/agent/extensions"));
    }
    #[test]
    fn config_relative() {
        assert_eq!(
            resolve_vars(&[("PI_CONFIG_DIR", ".omp-dev")]).sessions_dir,
            Path::new("/h/.omp-dev/agent/sessions")
        );
    }
    #[test]
    fn config_absolute_joins_under_home() {
        assert_eq!(
            resolve_vars(&[("PI_CONFIG_DIR", "/abs/x")]).agent_dir,
            Path::new("/h/abs/x/agent")
        );
    }
    #[test]
    fn config_lexical() {
        assert_eq!(
            resolve_vars(&[("PI_CONFIG_DIR", "a/../b")]).base_config_root,
            Path::new("/h/b")
        );
    }
    #[test]
    fn config_empty() {
        assert_eq!(
            resolve_vars(&[("PI_CONFIG_DIR", "")]).base_config_root,
            Path::new("/h/.omp")
        );
    }
    #[test]
    fn agent_absolute() {
        let p = resolve_vars(&[("PI_CODING_AGENT_DIR", "/custom/agent")]);
        assert_eq!(p.agent_dir, Path::new("/custom/agent"));
        assert_eq!(p.sessions_dir, Path::new("/custom/agent/sessions"));
        assert_eq!(p.agent_dir_source, AgentDirSource::Env);
        assert_eq!(
            p.session_owners_dir,
            Path::new("/h/.omp/run/session-owners")
        );
    }
    #[test]
    fn agent_relative() {
        assert_eq!(
            resolve_vars(&[("PI_CODING_AGENT_DIR", "rel/agent")]).agent_dir,
            Path::new("/w/rel/agent")
        );
    }
    #[test]
    fn named_profile() {
        let p = resolve_vars(&[("OMP_PROFILE", "work"), ("PI_CODING_AGENT_DIR", "/custom")]);
        assert_eq!(p.agent_dir, Path::new("/h/.omp/profiles/work/agent"));
        assert_eq!(p.agent_dir_source, AgentDirSource::Profile);
        assert_eq!(
            p.session_owners_dir,
            Path::new("/h/.omp/run/session-owners")
        );
    }
    #[test]
    fn empty_omp_profile_wins() {
        assert_eq!(
            resolve_vars(&[("OMP_PROFILE", ""), ("PI_PROFILE", "work")]).profile,
            None
        );
    }
    #[test]
    fn pi_profile_fallback() {
        assert_eq!(
            resolve_vars(&[("PI_PROFILE", "work")]).profile.as_deref(),
            Some("work")
        );
    }
    #[test]
    fn default_profile() {
        assert_eq!(resolve_vars(&[("OMP_PROFILE", "default")]).profile, None);
    }
    #[test]
    fn flag_wins() {
        let e = Env::for_test(
            Some(Path::new("/h")),
            Path::new("/w"),
            &[("OMP_PROFILE", "other")],
        );
        assert_eq!(
            resolve(&e, Some("work")).unwrap().profile.as_deref(),
            Some("work")
        );
    }
    #[test]
    fn profile_validation() {
        for name in [
            "con.txt", "x.", ".", "..", "Bad Name", "../x", "a/b", "com0", "lpt9.txt", "aux",
            "nul", "prn", "A",
        ] {
            assert!(normalize_profile_name(name).is_err(), "{name}");
        }
        for name in ["work", "a", "0", "a.b_c-0", &"a".repeat(64)] {
            assert!(normalize_profile_name(name).is_ok(), "{name}");
        }
        assert!(normalize_profile_name(&"a".repeat(65)).is_err());
        for key in ["OMP_PROFILE", "PI_PROFILE"] {
            let e = Env::for_test(Some(Path::new("/h")), Path::new("/w"), &[(key, "Bad Name")]);
            let err = resolve(&e, None).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidEnv);
            assert!(err.message.contains(key));
            assert_eq!(
                resolve(&e, Some("../x")).unwrap_err().code,
                ErrorCode::Usage
            );
        }
        assert_eq!(
            resolve_vars(&[("OMP_PROFILE", "work"), ("PI_PROFILE", "Bad Name")])
                .profile
                .as_deref(),
            Some("work")
        );
        assert_eq!(
            resolve_vars(&[
                ("OMP_PROFILE", ""),
                ("PI_PROFILE", "Bad Name"),
                ("PI_CODING_AGENT_DIR", "/custom")
            ])
            .agent_dir,
            Path::new("/custom")
        );
    }
    #[test]
    fn ignores_profile_derived_override() {
        assert_eq!(
            resolve_vars(&[
                ("OMP_PROFILE", ""),
                ("PI_PROFILE", "work"),
                ("PI_CODING_AGENT_DIR", "/h/.omp/profiles/work/agent")
            ])
            .agent_dir,
            Path::new("/h/.omp/agent")
        );
    }
    #[test]
    fn profile_derived_override_requires_raw_string_equality() {
        for raw in [
            "/h/.omp/profiles/work/./agent",
            "/h/.omp/profiles/work//agent",
        ] {
            let p = resolve_vars(&[
                ("OMP_PROFILE", ""),
                ("PI_PROFILE", "work"),
                ("PI_CODING_AGENT_DIR", raw),
            ]);
            assert_eq!(p.agent_dir, Path::new("/h/.omp/profiles/work/agent"));
            assert_eq!(p.agent_dir_source, AgentDirSource::Env);
        }
    }
    #[test]
    fn xdg_data_present() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("omp")).unwrap();
        let s = t.path().to_str().unwrap();
        let p = resolve_vars(&[("XDG_DATA_HOME", s)]);
        assert_eq!(p.sessions_dir, t.path().join("omp/sessions"));
        assert_eq!(p.sessions_dir_source, DataDirSource::Xdg);
        assert_eq!(p.agent_dir, Path::new("/h/.omp/agent"));
        assert_eq!(p.extensions_dir, Path::new("/h/.omp/agent/extensions"));
    }
    #[test]
    fn xdg_data_absent() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_vars(&[("XDG_DATA_HOME", t.path().to_str().unwrap())]).sessions_dir_source,
            DataDirSource::AgentDir
        );
    }
    #[test]
    fn xdg_ignored_for_override() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("omp")).unwrap();
        assert_eq!(
            resolve_vars(&[
                ("XDG_DATA_HOME", t.path().to_str().unwrap()),
                ("PI_CODING_AGENT_DIR", "/custom")
            ])
            .sessions_dir,
            Path::new("/custom/sessions")
        );
    }
    #[test]
    fn xdg_profile_requires_profile_directory() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("omp")).unwrap();
        let vars = [
            ("XDG_DATA_HOME", t.path().to_str().unwrap()),
            ("OMP_PROFILE", "work"),
        ];
        assert_eq!(
            resolve_vars(&vars).sessions_dir_source,
            DataDirSource::AgentDir
        );
        std::fs::create_dir_all(t.path().join("omp/profiles/work")).unwrap();
        assert_eq!(
            resolve_vars(&vars).sessions_dir,
            t.path().join("omp/profiles/work/sessions")
        );
    }
    #[test]
    fn xdg_state_is_global() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("omp")).unwrap();
        for extra in [
            vec![],
            vec![("OMP_PROFILE", "work")],
            vec![("PI_CODING_AGENT_DIR", "/custom")],
        ] {
            let mut vars = vec![("XDG_STATE_HOME", t.path().to_str().unwrap())];
            vars.extend(extra);
            assert_eq!(
                resolve_vars(&vars).session_owners_dir,
                t.path().join("omp/run/session-owners")
            );
        }
    }
    #[test]
    fn no_home() {
        assert_eq!(
            resolve(&Env::for_test(None, Path::new("/w"), &[]), None)
                .unwrap_err()
                .code,
            ErrorCode::InvalidEnv
        );
    }
}
