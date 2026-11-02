use crate::{
    cli::{Cli, DoctorArgs},
    ctl::probe,
    env::Env,
    error::{ErrorCode, GlenError, Warning},
    glen_home,
    jobs::wait,
    omp::{
        self, bin,
        compat::{self, Support},
        paths,
    },
    output::{Mode, Outcome, OutputCtx},
    render::{self, palette::Token, spinner::Spinner},
    version,
};
use serde::Serialize;
use serde_json::json;
use std::{path::Path, sync::atomic::Ordering, time::Duration};
#[derive(Serialize)]
struct Check {
    name: &'static str,
    status: &'static str,
    value: serde_json::Value,
    detail: String,
}
pub fn run(cli: &Cli, args: &DoctorArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
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
    // `version` is the normalized text (`None` when unparseable or when omp cannot run);
    // `runs` is whether `omp --version` ran at all, which gates the probe.
    let (version, runs) = if let Some(bin) = &binary {
        let spinner = Spinner::start(ctx.mode == Mode::Human && ctx.stderr_tty, "checking omp");
        let result = bin::omp_version(bin, Duration::from_secs(10), cli.global.profile.as_deref());
        spinner.finish();
        match result {
            Ok(line) => (omp_version_row(&line, &mut add, &mut warnings), true),
            Err(e) => {
                add("omp_version", "fail", "unavailable".into(), e);
                (None, false)
            }
        }
    } else {
        add(
            "omp_version",
            "info",
            "skipped".into(),
            "omp binary is missing".into(),
        );
        (None, false)
    };
    let mut capabilities = serde_json::Value::Null;
    match &binary {
        _ if !args.probe => add(
            "omp_capabilities",
            "info",
            "not probed".into(),
            "run glen doctor --probe to check omp's RPC capabilities".into(),
        ),
        None => add(
            "omp_capabilities",
            "info",
            "skipped".into(),
            "omp binary is missing".into(),
        ),
        Some(_) if !runs => add(
            "omp_capabilities",
            "info",
            "skipped".into(),
            "omp --version failed".into(),
        ),
        Some(bin) => {
            // Installed before omp starts: the probe owns omp's process group until it returns.
            let stop = wait::install_interrupt_all()?;
            let spinner = Spinner::start(ctx.mode == Mode::Human && ctx.stderr_tty, "probing omp");
            let result = probe::run(
                bin,
                cli.global.profile.as_deref(),
                env.cwd(),
                probe_budget(env),
                &stop,
            );
            spinner.finish();
            if stop.load(Ordering::SeqCst) {
                interrupted();
            }
            let caps = &result.capabilities;
            add(
                "omp_capabilities",
                caps.status(),
                caps.value(),
                result.detail.clone(),
            );
            capabilities = json!(caps);
        }
    }
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
    let inspection = crate::extension::inspect(
        &extension_path,
        &paths.extensions_dir,
        env!("CARGO_PKG_VERSION"),
        &glen_home.root,
    );
    let (extension_state, extension_version, unsafe_path, extension_error) = match inspection {
        Ok(i) => (i.state, i.version, i.unsafe_path, None),
        Err(e) => (
            crate::extension::State::Foreign,
            None,
            false,
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
            // W8 DEBT-2: install-extension refuses a symlinked path even with --force, so
            // doctor says so rather than recommending a command that cannot succeed.
            if unsafe_path {
                return crate::extension::unsafe_path_message(&extension_path);
            }
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
    let data = json!({"glen":{"version":env!("CARGO_PKG_VERSION"),"home":glen_home.root.to_string_lossy(),"home_exists":exists},"omp":{"bin":binary.as_deref().map(path_value),"version":version,"tested_version":*omp::TESTED_OMP_VERSION,"supported":compat::get().supported(),"capabilities":capabilities},"paths":json_paths,"extension":{"path":path_value(&extension_path),"state":extension_state_name,"version":extension_version,"current":current},"checks":checks,"skills":skills});
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
    let range = compat::get();
    let footer = format!(
        "tested against omp {} · supported {} to {} · profile {}",
        *omp::TESTED_OMP_VERSION,
        range.min_text,
        range.max_tested_text,
        render::sanitize(profile)
    );
    let footer = wrap(&footer, ctx.width);
    let human = format!(
        "{}\n\n{}\n\n{}",
        p.bold("glen doctor"),
        table.render(),
        p.paint(Token::Muted, &footer)
    );
    if let Some(failed) = checks.iter().find(|c| c.status == "fail") {
        return Err(
            GlenError::new(ErrorCode::OmpUnavailable, "omp is not available")
                .with_hint(if failed.name == "omp_capabilities" {
                    "omp lacks a capability glen needs: see the omp_capabilities row"
                } else {
                    "install omp or set GLEN_OMP_BIN"
                })
                .with_details(json!({"report":data}))
                .with_report(human),
        );
    }
    let mut outcome = Outcome::new(data, human);
    outcome.warnings = warnings;
    Ok(outcome)
}
/// Adds the `omp_version` row for one `omp --version` line and returns the normalized text.
fn omp_version_row(
    line: &str,
    add: &mut impl FnMut(&'static str, &'static str, String, String),
    warnings: &mut Vec<Warning>,
) -> Option<String> {
    let Some(parsed) = version::parse_omp_line(line) else {
        add(
            "omp_version",
            "warn",
            "unprobed".into(),
            "cannot parse omp --version output".into(),
        );
        let mut quoted: String = line.chars().take(64).collect();
        if line.chars().nth(64).is_some() {
            quoted.push('…');
        }
        warnings.push(Warning {
            code: "omp_version_untested".into(),
            message: format!(
                "cannot parse omp --version output \"{}\"; treating this omp as untested",
                render::sanitize(&quoted)
            ),
        });
        return None;
    };
    let range = compat::get();
    let text = parsed.text;
    let (status, detail, warning) = match compat::classify(&parsed.version) {
        Support::Supported => ("ok", "", None),
        Support::Untested => (
            "warn",
            "untested",
            Some((
                "omp_version_untested",
                format!(
                    "omp {text} is untested; the newest tested version is {}",
                    range.max_tested_text
                ),
            )),
        ),
        Support::Unsupported => (
            "warn",
            "unsupported",
            Some((
                "omp_version_unsupported",
                format!(
                    "omp {text} is older than the supported minimum {}",
                    range.min_text
                ),
            )),
        ),
    };
    add("omp_version", status, text.clone(), detail.into());
    if let Some((code, message)) = warning {
        warnings.push(Warning {
            code: code.into(),
            message,
        });
    }
    Some(text)
}
/// `probe::BUDGET`; debug builds let `GLEN_PROBE_BUDGET_MS` (100 to 30000) shorten it for tests.
fn probe_budget(env: &Env) -> Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = env
        .get("GLEN_PROBE_BUDGET_MS")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| (100..=30_000).contains(ms))
    {
        return Duration::from_millis(ms);
    }
    #[cfg(not(debug_assertions))]
    let _ = env;
    probe::BUDGET
}
/// Doctor has no interrupted document (§A11), so once the probe has torn omp down a stop
/// signal ends glen through SIGINT's default disposition (status 130 in a shell).
fn interrupted() -> ! {
    #[cfg(unix)]
    signal_hook::low_level::emulate_default_handler(signal_hook::consts::SIGINT).ok();
    std::process::exit(130)
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
