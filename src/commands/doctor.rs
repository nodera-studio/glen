use crate::{
    cli::Cli,
    env::Env,
    error::{ErrorCode, GlenError, Warning},
    glen_home,
    omp::{self, bin, paths},
    output::{Mode, Outcome, OutputCtx},
    render::{self, palette::Token, spinner::Spinner},
};
use serde::Serialize;
use serde_json::json;
use std::{path::Path, time::Duration};
#[derive(Serialize)]
struct Check {
    name: &'static str,
    status: &'static str,
    value: serde_json::Value,
    detail: String,
}
pub fn run(cli: &Cli, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    let paths = paths::resolve(env, cli.global.profile.as_deref())?;
    let glen_home = glen_home::resolve(env)?;
    let mut checks = Vec::new();
    let mut rows = Vec::new();
    let mut warnings = Vec::new();
    let mut add = |name, status, value: String, detail: String| {
        checks.push(Check {
            name,
            status,
            value: json!(value),
            detail: detail.clone(),
        });
        let value = if matches!(
            name,
            "omp_binary"
                | "agent_dir"
                | "sessions_dir"
                | "glen_home"
                | "skill_claude"
                | "skill_codex"
        ) && Path::new(&value).is_absolute()
        {
            render::tilde(Path::new(&value), env.home())
        } else {
            value
        };
        rows.push((name, status, value, detail));
    };
    let (binary, binary_error) = match bin::find_omp(env) {
        Ok(b) => (Some(b), None),
        Err(e) => (None, Some(e)),
    };
    let path_value = |p: &Path| p.to_string_lossy().into_owned();
    add(
        "omp_binary",
        if binary.is_some() { "ok" } else { "fail" },
        binary
            .as_deref()
            .map(path_value)
            .unwrap_or_else(|| "not found".into()),
        binary_error.unwrap_or_default(),
    );
    let version = if let Some(bin) = &binary {
        let spinner = Spinner::start(ctx.mode == Mode::Human && ctx.stderr_tty, "checking omp");
        let result = bin::omp_version(bin, Duration::from_secs(10), cli.global.profile.as_deref());
        spinner.finish();
        match result {
            Ok(version) => {
                let tested = version == omp::TESTED_OMP_VERSION;
                add(
                    "omp_version",
                    if tested { "ok" } else { "warn" },
                    version.clone(),
                    if tested {
                        String::new()
                    } else {
                        "untested".into()
                    },
                );
                if !tested {
                    warnings.push(Warning {
                        code: "omp_version_untested".into(),
                        message: format!(
                            "omp {version} is untested; tested against {}",
                            omp::TESTED_OMP_VERSION
                        ),
                    });
                }
                Some(version)
            }
            Err(e) => {
                add("omp_version", "fail", "unavailable".into(), e);
                None
            }
        }
    } else {
        add(
            "omp_version",
            "info",
            "skipped".into(),
            "omp binary is missing".into(),
        );
        None
    };
    let profile = paths.profile.as_deref().unwrap_or("default");
    add("profile", "info", profile.into(), String::new());
    add(
        "agent_dir",
        if paths.agent_dir.is_dir() {
            "ok"
        } else {
            "warn"
        },
        path_value(&paths.agent_dir),
        if paths.agent_dir.is_dir() {
            String::new()
        } else {
            "omp has not run with this profile yet".into()
        },
    );
    add(
        "sessions_dir",
        if paths.sessions_dir.is_dir() {
            "ok"
        } else {
            "warn"
        },
        path_value(&paths.sessions_dir),
        if paths.sessions_dir.is_dir() {
            String::new()
        } else {
            "no sessions directory yet".into()
        },
    );
    let exists = glen_home.root.exists();
    let private = glen_home::mode_is_private(&glen_home.root);
    add(
        "glen_home",
        if !exists {
            "info"
        } else if private == Some(true) {
            "ok"
        } else {
            "warn"
        },
        path_value(&glen_home.root),
        if !exists {
            "created on first use".into()
        } else if private == Some(false) {
            "group/other permission bits are set".into()
        } else {
            String::new()
        },
    );
    let (status, value, detail) = omp::dotenv::describe(env, &paths);
    add("omp_env_files", status, value.join(", "), detail);
    let env_files = value;
    let mut skills = serde_json::Map::new();
    for agent in crate::skill::Agent::ALL {
        let path = crate::skill::user_target(env, agent)?;
        let rendered = crate::skill::render_current(agent)?;
        let before = crate::skill::inspect(&path, agent, &rendered);
        let (mut status, mut detail, state, installed) = match before {
            Ok(i) => {
                let status = match i.state {
                    crate::skill::State::Current => "ok",
                    crate::skill::State::NotInstalled => "info",
                    _ => "warn",
                };
                let detail = if i.state == crate::skill::State::Current {
                    String::new()
                } else {
                    format!(
                        "{}; run `glen install-skill{}`",
                        i.detail,
                        if matches!(
                            i.state,
                            crate::skill::State::Modified | crate::skill::State::Foreign
                        ) {
                            " --force"
                        } else {
                            ""
                        }
                    )
                };
                (status, detail, i.state.as_str(), i.version)
            }
            Err(e) => (
                "warn",
                format!(
                    "cannot inspect {}: {e}; run `glen install-skill`",
                    path.display()
                ),
                "foreign",
                None,
            ),
        };
        let project_path = crate::skill::project_target(env.cwd(), agent);
        let same_copy = project_path == path
            || std::fs::canonicalize(&project_path)
                .ok()
                .zip(std::fs::canonicalize(&path).ok())
                .is_some_and(|(a, b)| a == b);
        // Classification must run even when a file symlink canonicalises to the user file.
        let project_inspection =
            crate::skill::inspect_scoped(&project_path, env.cwd(), agent, &rendered);
        let project = match project_inspection {
            Ok(i)
                if project_path == path
                    || (same_copy && !i.unsafe_path && i.state != crate::skill::State::Foreign)
                    || (i.unsafe_path
                        && matches!(std::fs::metadata(&project_path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)) =>
            {
                serde_json::Value::Null
            }
            Ok(i) if i.state == crate::skill::State::NotInstalled => serde_json::Value::Null,
            Ok(i) => {
                if i.state != crate::skill::State::Current {
                    status = "warn";
                    if !detail.is_empty() {
                        detail.push_str("; ");
                    }
                    detail.push_str(&format!(
                        "project copy at {} is {}: run `glen install-skill --project`",
                        render::tilde(&project_path, env.home()),
                        i.state.as_str()
                    ));
                }
                json!({"path":path_value(&project_path),"state":i.state.as_str(),"version":i.version})
            }
            Err(e) => {
                status = "warn";
                if !detail.is_empty() {
                    detail.push_str("; ");
                }
                detail.push_str(&format!(
                    "cannot inspect project copy {}: {e}; run `glen install-skill --project`",
                    project_path.display()
                ));
                json!({"path":path_value(&project_path),"state":"foreign","version":null})
            }
        };
        skills.insert(agent.as_str().into(),json!({"path":path_value(&path),"state":state,"version":installed,"current":state=="current","project":project}));
        add(
            if agent == crate::skill::Agent::Claude {
                "skill_claude"
            } else {
                "skill_codex"
            },
            status,
            if state == "not_installed" {
                "not installed".into()
            } else {
                path_value(&path)
            },
            detail,
        );
    }
    let (status, value, detail) = crate::sandbox::describe(env);
    add("sandbox", status, value, detail);
    let json_paths = if ctx.mode == Mode::Json {
        serde_json::to_value(&paths).map_err(|e| {
            GlenError::new(
                ErrorCode::InvalidEnv,
                format!("cannot serialize omp paths: {e}"),
            )
        })?
    } else {
        serde_json::Value::Null
    };
    if let Some(check) = checks.iter_mut().find(|c| c.name == "omp_env_files") {
        check.value = json!(env_files);
    }
    let data = json!({"glen":{"version":env!("CARGO_PKG_VERSION"),"home":glen_home.root.to_string_lossy(),"home_exists":exists},"omp":{"bin":binary.as_deref().map(path_value),"version":version,"tested_version":omp::TESTED_OMP_VERSION},"paths":json_paths,"checks":checks,"skills":skills});
    let p = &ctx.painter;
    let mut table = render::Table::new(p, ctx.width);
    for (name, status, value, detail) in rows {
        let glyph = match status {
            "ok" => p.ok(),
            "warn" => p.warn(),
            "fail" => p.fail(),
            _ => p.info(),
        };
        table.row(vec![
            glyph,
            name.replace('_', " "),
            render::sanitize(&value).into_owned(),
            if detail.is_empty() {
                String::new()
            } else {
                p.paint(Token::Muted, &render::sanitize(&detail))
            },
        ]);
    }
    let footer = format!(
        "tested against omp {} · profile {}",
        omp::TESTED_OMP_VERSION,
        render::sanitize(profile)
    );
    let footer = wrap(&footer, ctx.width);
    let human = format!(
        "{}\n\n{}\n\n{}",
        p.bold("glen doctor"),
        table.render(),
        p.paint(Token::Muted, &footer)
    );
    if checks.iter().any(|c| c.status == "fail") {
        return Err(
            GlenError::new(ErrorCode::OmpUnavailable, "omp is not available")
                .with_hint("install omp or set GLEN_OMP_BIN")
                .with_details(json!({"report":data}))
                .with_report(human),
        );
    }
    let mut outcome = Outcome::new(data, human);
    outcome.warnings = warnings;
    Ok(outcome)
}
fn wrap(text: &str, width: Option<u16>) -> String {
    let Some(width) = width else {
        return text.into();
    };
    text.chars()
        .collect::<Vec<_>>()
        .chunks(usize::from(width))
        .map(|s| s.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}
