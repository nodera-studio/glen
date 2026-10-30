use crate::{env::Env, omp::paths::OmpPaths, render};
use std::{collections::BTreeSet, fs};
/// omp reads these files; glen deliberately does not load their values.
pub fn describe(env: &Env, paths: &OmpPaths) -> (&'static str, String, String) {
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
                files.insert(render::tilde(&path, env.home()));
            }
        }
    }
    if files.is_empty() {
        ("ok", "none".into(), String::new())
    } else {
        (
            "warn",
            files.into_iter().collect::<Vec<_>>().join(", "),
            format!(
                "defines {}; omp reads these files and glen does not, so the paths can differ; export the variable instead",
                keys.into_iter().collect::<Vec<_>>().join(", ")
            ),
        )
    }
}
