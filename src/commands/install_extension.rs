use crate::{
    cli::{Cli, InstallExtensionArgs},
    env::Env,
    error::{ErrorCode, GlenError, Warning},
    extension::{self, State},
    glen_home,
    omp::paths,
    output::{Mode, Outcome, OutputCtx},
    render::{self, palette::Token, text},
};
use serde_json::json;
pub fn run(
    cli: &Cli,
    a: &InstallExtensionArgs,
    env: &Env,
    ctx: &OutputCtx,
) -> Result<Outcome, GlenError> {
    let paths = paths::resolve(env, cli.global.profile.as_deref())?;
    let home = glen_home::resolve(env)?;
    let dir = paths.extensions_dir.join("glen");
    let target = dir.join("index.ts");
    let baked_home = home.root.to_str().ok_or_else(|| {
        GlenError::new(
            ErrorCode::InvalidEnv,
            format!(
                "cannot bake GLEN_HOME {} into JavaScript: path is not UTF-8",
                home.root.display()
            ),
        )
    })?;
    // Validate JSON paths before an explicit install changes the filesystem.
    let json_path = |p: &std::path::Path| -> Result<serde_json::Value, GlenError> {
        if ctx.mode == Mode::Json {
            serde_json::to_value(p).map_err(|e| {
                GlenError::new(
                    ErrorCode::InvalidEnv,
                    format!("cannot serialize extension path {}: {e}", p.display()),
                )
            })
        } else {
            Ok(json!(p.to_string_lossy()))
        }
    };
    let path_data = json_path(&target)?;
    let agent_data = json_path(&paths.agent_dir)?;

    let io = |e| {
        GlenError::new(
            ErrorCode::Io,
            format!("cannot update extension {}: {e}", target.display()),
        )
    };
    let mut guard = extension::write::Guard::inspect(&target).map_err(io)?;
    let unsafe_path = guard.unsafe_path(&paths.extensions_dir);
    let (state, previous) = if unsafe_path {
        (State::Foreign, None)
    } else {
        extension::inspect(&target, env!("CARGO_PKG_VERSION"), &home.root).map_err(io)?
    };
    let mut warnings = Vec::new();
    let unsafe_state = matches!(state, State::Modified | State::Foreign)
        || (!a.uninstall && matches!(state, State::Newer(_)));
    let mut action = if unsafe_path {
        warnings.push(Warning {
            code: "unsafe_path".into(),
            message: format!(
                "refusing symlink in extension path {}; --force cannot override this",
                target.display()
            ),
        });
        "skipped"
    } else if a.uninstall && state == State::NotInstalled {
        "not_installed"
    } else if unsafe_state && !a.force {
        warnings.push(Warning {
            code: format!("extension_{}", state.name()),
            message: format!(
                "{} is {}; use --force to overwrite or remove it",
                target.display(),
                state.name()
            ),
        });
        "skipped"
    } else if a.uninstall {
        "removed"
    } else {
        match state {
            State::NotInstalled => "installed",
            State::Current => "unchanged",
            State::Outdated(_) => "upgraded",
            State::Newer(_) | State::Modified | State::Foreign => "replaced",
            _ => "reinstalled",
        }
    };
    let changes = matches!(
        action,
        "installed" | "upgraded" | "reinstalled" | "replaced" | "removed"
    );
    if changes && !a.dry_run {
        let rendered = extension::render(env!("CARGO_PKG_VERSION"), &home.root);
        let applied = guard
            .apply(
                &target,
                if a.uninstall {
                    None
                } else {
                    Some(rendered.as_bytes())
                },
            )
            .map_err(io)?;
        if matches!(applied, crate::atomic_swap::Applied::Lost) {
            action = "skipped";
            warnings.push(Warning {
                code: "changed_during_install".into(),
                message: format!(
                    "extension {} changed during install; skipped",
                    target.display()
                ),
            });
        } else {
            if matches!(applied, crate::atomic_swap::Applied::PublishedNotAtomic) {
                warnings.push(Warning {
                    code: "replace_not_atomic".into(),
                    message: format!(
                        "the filesystem at {} cannot swap files atomically; a concurrent change during this install may have been overwritten",
                        dir.display()
                    ),
                });
            }
            if a.uninstall {
                match guard.remove_empty_parent(&target) {
                    Ok(true) => (),
                    Ok(false) => warnings.push(Warning {
                        code: "changed_during_install".into(),
                        message: format!("kept changed extension directory {}", dir.display()),
                    }),
                    Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                        warnings.push(Warning {
                            code: "extension_dir_not_empty".into(),
                            message: format!("kept nonempty {}", dir.display()),
                        })
                    }
                    Err(e) => return Err(io(e)),
                }
            }
        }
    }
    warnings.extend(extension::socket_path_warning(&home.root));
    let restart = changes && action != "skipped" && !a.dry_run;
    let p = &ctx.painter;
    let (glyph, token) = if action == "skipped" {
        ("!", Token::Warning)
    } else if matches!(action, "unchanged" | "not_installed") {
        ("·", Token::Muted)
    } else {
        ("✓", Token::Success)
    };
    let heading = format!(
        "{glyph} {action} glen extension {}{}",
        env!("CARGO_PKG_VERSION"),
        if a.dry_run {
            " (dry run, nothing changed)"
        } else {
            ""
        }
    );
    let mut lines = text::wrap(&heading, ctx.width.map(usize::from))
        .into_iter()
        .map(|line| p.paint(token, &line))
        .collect::<Vec<_>>();
    lines.extend(
        text::wrap(
            &render::sanitize(&render::tilde(&target, env.home())),
            ctx.width.map(usize::from),
        )
        .into_iter()
        .map(|line| p.paint(Token::Muted, &line)),
    );
    if restart {
        lines.extend(
            text::wrap(
                "restart running omp sessions to apply the change",
                ctx.width.map(usize::from),
            )
            .into_iter()
            .map(|line| p.paint(Token::Muted, &line)),
        );
    }
    if matches!(
        action,
        "installed" | "upgraded" | "reinstalled" | "replaced"
    ) {
        lines.extend(text::wrap("The glen extension adds glen_reply to every omp session's tool list; uninstall with `glen install-extension --uninstall`.",ctx.width.map(usize::from)).into_iter().map(|line|p.paint(Token::Muted,&line)));
    }
    let human = lines.join("\n");
    let mut o = Outcome::new(
        json!({"action":action,"path":path_data,"version":env!("CARGO_PKG_VERSION"),"previous_version":previous,"glen_home":baked_home,"profile":paths.profile,"agent_dir":agent_data,"agent_dir_source":paths.agent_dir_source,"dry_run":a.dry_run,"restart_required":restart}),
        human,
    );
    o.warnings = warnings;
    Ok(o)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::output::{OutputKind, Terminal};
    use clap::Parser;
    use std::fs;
    /// Fixes2 decision 6: an error after the exchange is an `io` error naming the kept
    /// displaced file, never a `skipped` extension.
    #[test]
    fn an_error_after_the_exchange_fails_the_install_and_names_the_kept_file() {
        let t = tempfile::tempdir().unwrap();
        let agent = t.path().join("agent");
        let env = Env::for_test(
            Some(t.path()),
            t.path(),
            &[("PI_CODING_AGENT_DIR", agent.to_str().unwrap())],
        );
        let cli = Cli::try_parse_from(["glen", "install-extension", "--force"]).unwrap();
        let crate::cli::Command::InstallExtension(args) = &cli.command else {
            panic!("install-extension")
        };
        let dir = paths::resolve(&env, None)
            .unwrap()
            .extensions_dir
            .join("glen");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index.ts"), "foreign extension").unwrap();
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
        let _hook = crate::atomic_swap::set_test_hook(|stage| {
            if stage == "exchanged" {
                Err(std::io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        });
        let Err(error) = run(&cli, args, &env, &ctx) else {
            panic!("the install must fail")
        };
        assert_eq!(error.code, ErrorCode::Io);
        let kept = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.file_name().unwrap().to_string_lossy().ends_with(".tmp"))
            .expect("the displaced file is kept");
        assert!(
            error
                .message
                .contains(&format!("displaced copy kept at {}", kept.display())),
            "{}",
            error.message
        );
        assert_eq!(fs::read_to_string(&kept).unwrap(), "foreign extension");
    }
}
