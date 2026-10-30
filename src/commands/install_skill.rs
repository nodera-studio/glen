use crate::{
    cli::InstallSkillArgs,
    env::Env,
    error::{ErrorCode, GlenError, Warning},
    glen_home,
    output::{Outcome, OutputCtx},
    render::{self, palette::Token},
    skill::{self, Agent, State},
};
use serde_json::{Value, json};
use std::{fs, io, path::Path};
fn io_error(path: &Path, e: io::Error, done: &[Value]) -> GlenError {
    GlenError::new(
        ErrorCode::Io,
        format!("cannot write {}: {e}", path.display()),
    )
    .with_details(json!({"done":done}))
}
pub fn run(args: &InstallSkillArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    if !env.home_is_explicit() {
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
        // Validate HOME for project installs too, matching the user-level contract.
        let user = skill::user_target(env, agent)?;
        let path = if args.project {
            skill::project_target(env.cwd(), agent)
        } else {
            user
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
        let before = skill::inspect_scoped(&path, scope, agent, &rendered)
            .map_err(|e| io_error(&path, e, &[]))?;
        targets.push((agent, path, rendered, before));
    }
    let mut done = Vec::new();
    let mut warnings = Vec::new();
    let mut table = render::Table::new(&ctx.painter, ctx.width);
    for (agent, path, rendered, before) in targets {
        let skip = before.unsafe_path
            || (!args.force && matches!(before.state, State::Modified | State::Foreign))
            || (!args.force && !args.uninstall && before.state == State::Newer);
        let action = if skip {
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
        if skip {
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
                    let mut builder = fs::DirBuilder::new();
                    builder.recursive(true);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::DirBuilderExt;
                        builder.mode(0o755);
                    }
                    builder
                        .create(parent)
                        .and_then(|_| glen_home::write_atomic(&path, rendered.as_bytes(), 0o644))
                        .map_err(|e| io_error(&path, e, &done))?;
                }
                "removed" => {
                    fs::remove_file(&path).map_err(|e| io_error(&path, e, &done))?;
                    if let Err(e) = fs::remove_dir(parent) {
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
                            return Err(io_error(parent, e, &done));
                        }
                    }
                }
                _ => {}
            }
        } else if action == "removed"
            && let Some(parent) = path.parent()
        {
            let nonempty = fs::read_dir(parent)
                .map_err(|e| io_error(parent, e, &done))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| io_error(parent, e, &done))?
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
        table.row(vec![
            match action {
                "skipped" => ctx.painter.warn(),
                "unchanged" | "not_installed" => ctx.painter.info(),
                _ => ctx.painter.ok(),
            },
            if args.dry_run {
                format!("would {action}")
            } else {
                action.into()
            },
            agent.as_str().into(),
            render::tilde(&path, env.home()),
        ]);
        done.push(json!({"agent":agent.as_str(),"path":path.to_string_lossy(),"action":action,"state":before.state.as_str(),"installed_version":before.version}));
    }
    let human=format!("{}\n{}",table.render(),ctx.painter.paint(Token::Muted,&format!("glen skill {} · ask your agent about an omp session, or run /glen (Claude Code) or $glen (Codex)",env!("CARGO_PKG_VERSION"))));
    let mut out = Outcome::new(
        json!({"version":env!("CARGO_PKG_VERSION"),"dry_run":args.dry_run,"scope":if args.project {"project"}else{"user"},"targets":done}),
        human,
    );
    out.warnings = warnings;
    Ok(out)
}
