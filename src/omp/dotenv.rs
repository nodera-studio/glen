use crate::{env::Env, omp::paths::OmpPaths};
use std::{collections::BTreeSet, fs};
/// omp reads these files; glen deliberately does not load their values.
pub fn describe(env: &Env, paths: &OmpPaths) -> (&'static str, Vec<String>, String) {
    let mut files = BTreeSet::new();
    let mut keys = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut candidates = vec![
        env.cwd().join(".env"),
        paths.agent_dir.join(".env"),
        paths.config_root.join(".env"),
    ];
    if let Some(home) = env.home() {
        candidates.push(home.join(".env"));
    }
    for path in candidates {
        if !seen.insert(path.clone()) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim_start();
            let line = if let Some(rest) = line.strip_prefix("export") {
                if !rest.starts_with(char::is_whitespace) {
                    continue;
                }
                rest.trim_start()
            } else {
                line
            };
            let Some((key, _)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim_end();
            let mapped = if key == "OMP_CODING_AGENT_DIR" {
                "PI_CODING_AGENT_DIR"
            } else {
                key
            };
            if matches!(
                mapped,
                "PI_CODING_AGENT_DIR" | "XDG_DATA_HOME" | "XDG_STATE_HOME"
            ) && env.non_empty(mapped).is_none()
            {
                keys.insert(key.to_owned());
                files.insert(path.to_string_lossy().into_owned());
            }
        }
    }
    if files.is_empty() {
        ("ok", vec![], String::new())
    } else {
        (
            "warn",
            files.into_iter().collect::<Vec<_>>(),
            format!(
                "defines {}; omp reads these files and glen does not, so the paths can differ; export the variable instead",
                keys.into_iter().collect::<Vec<_>>().join(", ")
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::omp::paths;
    #[test]
    fn all_mapped_keys_and_whitespace_environment_are_checked() {
        let t = tempfile::tempdir().unwrap();
        let keys = [
            "PI_CODING_AGENT_DIR",
            "OMP_CODING_AGENT_DIR",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
        ];
        for key in keys {
            fs::write(t.path().join(".env"), format!("{key}=/elsewhere\n")).unwrap();
            let mapped = if key == "OMP_CODING_AGENT_DIR" {
                "PI_CODING_AGENT_DIR"
            } else {
                key
            };
            let env = Env::for_test(Some(t.path()), t.path(), &[(mapped, " ")]);
            let paths = paths::resolve(&env, None).unwrap();
            let (status, files, detail) = describe(&env, &paths);
            assert_eq!(status, "warn", "{key}");
            assert_eq!(files, [t.path().join(".env").to_str().unwrap()]);
            assert!(detail.contains(key));
            assert!(files.iter().all(|p| std::path::Path::new(p).is_absolute()));
            let env = Env::for_test(Some(t.path()), t.path(), &[(mapped, "/set")]);
            assert_eq!(describe(&env, &paths).0, "ok", "{key}");
        }
    }
}
