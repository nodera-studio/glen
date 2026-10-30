use crate::{
    cli::InstallSkillArgs,
    env::Env,
    error::{ErrorCode, GlenError, Warning},
    output::{Outcome, OutputCtx},
    render::{self, palette::Token},
    skill::{self, Agent, State},
};
use serde_json::{Value, json};
use std::{fs, io, path::Path};
fn io_error(path: &Path, e: io::Error, done: &[Value], warnings: &[Warning]) -> GlenError {
    GlenError::new(
        ErrorCode::Io,
        format!("cannot write {}: {e}", path.display()),
    )
    .with_details(json!({"done":done,"warnings":warnings}))
}
pub fn run(args: &InstallSkillArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    run_inspected(args, env, ctx, || {})
}
fn run_inspected(
    args: &InstallSkillArgs,
    env: &Env,
    ctx: &OutputCtx,
    after_inspection: impl FnOnce(),
) -> Result<Outcome, GlenError> {
    if !args.project && !env.home_is_explicit() {
        return Err(GlenError::new(ErrorCode::InvalidEnv, "HOME is not set"));
    }
    // Render and inspect every target before the first mutation.
    let mut targets = Vec::new();
    for agent in Agent::ALL {
        if (args.claude || args.codex)
            && !match agent {
                Agent::Claude => args.claude,
                Agent::Codex => args.codex,
            }
        {
            continue;
        }
        let path = if args.project {
            skill::project_target(env.cwd(), agent)
        } else {
            skill::user_target(env, agent)?
        };
        if ctx.mode == crate::output::Mode::Json && path.to_str().is_none() {
            return Err(GlenError::new(
                ErrorCode::InvalidEnv,
                format!("cannot serialize skill path {}", path.display()),
            ));
        }
        let rendered = skill::render_current(agent)?;
        let scope = if args.project {
            env.cwd()
        } else {
            path.parent()
                .and_then(Path::parent)
                .ok_or_else(|| GlenError::new(ErrorCode::Internal, "skill target has no scope"))?
        };
        let guard =
            skill::write::Guard::inspect(&path).map_err(|e| io_error(&path, e, &[], &[]))?;
        let before = skill::inspect_scoped(&path, scope, agent, &rendered)
            .map_err(|e| io_error(&path, e, &[], &[]))?;
        targets.push((agent, path, rendered, before, guard));
    }
    after_inspection();
    let mut done = Vec::new();
    let mut warnings = Vec::new();
    let mut table = render::Table::new(&ctx.painter, ctx.width);
    for (agent, path, rendered, before, mut guard) in targets {
        let changed = !guard.unchanged();
        let skip = changed
            || before.unsafe_path
            || (!args.force && matches!(before.state, State::Modified | State::Foreign))
            || (!args.force && !args.uninstall && before.state == State::Newer);
        let mut action = if skip {
            "skipped"
        } else if args.uninstall {
            if before.state == State::NotInstalled {
                "not_installed"
            } else {
                "removed"
            }
        } else {
            match before.state {
                State::NotInstalled => "installed",
                State::Current => "unchanged",
                State::Outdated => "upgraded",
                _ => "reinstalled",
            }
        };
        if changed {
            warnings.push(changed_warning(&path));
        } else if skip {
            let code = match before.state {
                State::Newer => "skill_newer",
                State::Modified => "skill_modified",
                _ => "skill_foreign",
            };
            warnings.push(Warning {
                code: code.into(),
                message: format!(
                    "{} skill at {}: {}; {}",
                    agent.as_str(),
                    render::tilde(&path, env.home()),
                    before.detail,
                    if before.unsafe_path {
                        "symlinked directories are never replaced"
                    } else if args.uninstall {
                        "--force removes it"
                    } else {
                        "--force replaces it"
                    }
                ),
            });
        }
        if !args.dry_run {
            let parent = path
                .parent()
                .ok_or_else(|| GlenError::new(ErrorCode::Internal, "skill target has no parent"))?;
            match action {
                "installed" | "upgraded" | "reinstalled" => {
                    if !guard
                        .apply(&path, Some(rendered.as_bytes()))
                        .map_err(|e| io_error(&path, e, &done, &warnings))?
                    {
                        action = "skipped";
                        warnings.push(changed_warning(&path));
                    }
                }
                "removed" => {
                    if !guard
                        .apply(&path, None)
                        .map_err(|e| io_error(&path, e, &done, &warnings))?
                    {
                        action = "skipped";
                        warnings.push(changed_warning(&path));
                    } else {
                        match guard.remove_empty_parent(&path) {
                            Ok(false) => warnings.push(changed_warning(&path)),
                            Ok(true) => {}
                            Err(e) => {
                                if e.kind() == io::ErrorKind::DirectoryNotEmpty {
                                    warnings.push(Warning {
                                        code: "skill_dir_not_empty".into(),
                                        message: format!(
                                            "{} skill directory {} is not empty",
                                            agent.as_str(),
                                            render::tilde(parent, env.home())
                                        ),
                                    });
                                } else {
                                    done.push(target(agent, &path, action, &before));
                                    return Err(io_error(parent, e, &done, &warnings));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        } else if action == "removed"
            && let Some(parent) = path.parent()
        {
            let nonempty = fs::read_dir(parent)
                .map_err(|e| io_error(parent, e, &done, &warnings))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| io_error(parent, e, &done, &warnings))?
                .iter()
                .any(|e| e.path() != path);
            if nonempty {
                warnings.push(Warning {
                    code: "skill_dir_not_empty".into(),
                    message: format!(
                        "{} skill directory {} is not empty",
                        agent.as_str(),
                        render::tilde(parent, env.home())
                    ),
                });
            }
        }
        if let Some(message) = guard.sync_warning.take() {
            warnings.push(Warning {
                code: "skill_sync_failed".into(),
                message,
            });
        }
        table.row(vec![
            match action {
                "skipped" => ctx.painter.warn(),
                "unchanged" | "not_installed" => ctx.painter.info(),
                _ => ctx.painter.ok(),
            },
            if args.dry_run {
                format!(
                    "would {}",
                    match action {
                        "installed" => "install",
                        "skipped" => "skip",
                        "removed" => "remove",
                        "upgraded" => "upgrade",
                        "reinstalled" => "reinstall",
                        "unchanged" => "leave unchanged",
                        _ => "leave absent",
                    }
                )
            } else {
                action.into()
            },
            agent.as_str().into(),
            render::tilde(&path, env.home()),
        ]);
        done.push(target(agent, &path, action, &before));
    }
    let footer = if args.uninstall {
        format!(
            "glen skill {} · {}",
            env!("CARGO_PKG_VERSION"),
            if args.dry_run {
                "would uninstall"
            } else {
                "uninstall complete"
            }
        )
    } else {
        format!(
            "glen skill {} · ask about an omp session\nClaude Code: /glen · Codex: $glen",
            env!("CARGO_PKG_VERSION")
        )
    };
    let footer = footer
        .lines()
        .flat_map(|line| render::text::wrap(line, ctx.width.map(usize::from)))
        .collect::<Vec<_>>()
        .join("\n");
    let human = format!(
        "{}\n{}",
        table.render(),
        ctx.painter.paint(Token::Muted, &footer)
    );
    let mut out = Outcome::new(
        json!({"version":env!("CARGO_PKG_VERSION"),"dry_run":args.dry_run,"scope":if args.project {"project"}else{"user"},"targets":done}),
        human,
    );
    out.warnings = warnings;
    Ok(out)
}

fn changed_warning(path: &Path) -> Warning {
    Warning {
        code: "changed_during_install".into(),
        message: format!(
            "skill at {} changed during install; skipped",
            path.display()
        ),
    }
}
fn target(agent: Agent, path: &Path, action: &str, before: &skill::Inspection) -> Value {
    json!({"agent":agent.as_str(),"path":path.to_string_lossy(),"action":action,"state":before.state.as_str(),"installed_version":before.version})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cli::{Cli, Command},
        output::{OutputKind, Terminal},
    };
    use clap::Parser;
    #[test]
    fn concurrent_replacement_skips_with_changed_warning_even_under_force() {
        for uninstall in [false, true] {
            for symlink in [false, true] {
                let t = tempfile::tempdir().unwrap();
                let env = Env::for_test(Some(t.path()), t.path(), &[]);
                let path = skill::user_target(&env, Agent::Claude).unwrap();
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(
                    &path,
                    skill::render(Agent::Claude, "0.0.1", &skill::visible_commands()).unwrap(),
                )
                .unwrap();
                let mut argv = vec!["glen", "install-skill", "--claude", "--force"];
                if uninstall {
                    argv.push("--uninstall");
                }
                let cli = Cli::try_parse_from(argv).unwrap();
                let Command::InstallSkill(args) = &cli.command else {
                    panic!("install-skill")
                };
                let ctx = OutputCtx::new(
                    &cli.global,
                    OutputKind::Document,
                    &env,
                    &Terminal {
                        stdout_tty: false,
                        stderr_tty: false,
                        stdout_cols: None,
                    },
                )
                .unwrap();
                let outside = t.path().join("outside");
                fs::write(&outside, "external").unwrap();
                let out = run_inspected(args, &env, &ctx, || {
                    if symlink {
                        #[cfg(unix)]
                        {
                            fs::remove_file(&path).unwrap();
                            std::os::unix::fs::symlink(&outside, &path).unwrap();
                        }
                    } else {
                        fs::write(&path, "concurrent edit").unwrap();
                    }
                })
                .unwrap();
                assert_eq!(out.data["targets"][0]["action"], "skipped");
                assert_eq!(out.warnings[0].code, "changed_during_install");
                assert_eq!(fs::read_to_string(&outside).unwrap(), "external");
                if !symlink {
                    assert_eq!(fs::read_to_string(&path).unwrap(), "concurrent edit");
                }
            }
        }
    }
}
