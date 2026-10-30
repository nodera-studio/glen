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
    value: String,
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
            value: value.clone(),
            detail: detail.clone(),
        });
        let value = if matches!(
            name,
            "omp_binary" | "agent_dir" | "sessions_dir" | "glen_home"
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
    let extension_path = paths.extensions_dir.join("glen/index.ts");
    let inspection =
        crate::extension::inspect(&extension_path, env!("CARGO_PKG_VERSION"), &glen_home.root);
    let (extension_state, extension_version, extension_error) = match inspection {
        Ok((state, version)) => (state, version, None),
        Err(e) => (
            crate::extension::State::Foreign,
            None,
            Some(format!(
                "cannot inspect extension {}: {e}",
                extension_path.display()
            )),
        ),
    };
    let extension_state_name = if extension_error.is_some() {
        "unreadable"
    } else {
        extension_state.name()
    };
    let current = extension_error.is_none() && extension_state == crate::extension::State::Current;
    add(
        "extension",
        if current {
            "ok"
        } else if extension_error.is_some() {
            "warn"
        } else if extension_state == crate::extension::State::NotInstalled {
            "info"
        } else {
            "warn"
        },
        if extension_error.is_some() {
            "unreadable".into()
        } else {
            extension_state.name().into()
        },
        extension_error.unwrap_or_else(|| {
            format!(
                "{}{}{}",
                extension_path.display(),
                extension_version
                    .as_deref()
                    .map(|v| format!(" (installed {v})"))
                    .unwrap_or_default(),
                if current {
                    ""
                } else {
                    "; run `glen install-extension` to reach running omp sessions"
                }
            )
        }),
    );
    let registries = crate::live::registry::index(&glen_home);
    let stale = registries
        .iter()
        .filter(|r| crate::live::registry::pid_dead(r.pid))
        .count();
    let unreadable = crate::live::registry::files(&glen_home).len() != registries.len();
    let path_warning = crate::extension::socket_path_warning(&glen_home.root);
    add(
        "live_endpoints",
        if unreadable || path_warning.is_some() {
            "warn"
        } else {
            "info"
        },
        format!("{} live, {stale} stale", registries.len() - stale),
        path_warning.map(|w| w.message).unwrap_or_else(|| {
            if unreadable {
                "an endpoint registry is unreadable".into()
            } else {
                String::new()
            }
        }),
    );
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
    let data = json!({"glen":{"version":env!("CARGO_PKG_VERSION"),"home":glen_home.root.to_string_lossy(),"home_exists":exists},"omp":{"bin":binary.as_deref().map(path_value),"version":version,"tested_version":omp::TESTED_OMP_VERSION},"paths":json_paths,"extension":{"path":path_value(&extension_path),"state":extension_state_name,"version":extension_version,"current":current},"checks":checks});
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
