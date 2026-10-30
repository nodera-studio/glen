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
use std::{fs, os::unix::fs::DirBuilderExt};
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
    let (state, previous) =
        extension::inspect(&target, env!("CARGO_PKG_VERSION"), &home.root).map_err(io)?;
    let mut warnings = Vec::new();
    let unsafe_state = matches!(state, State::Modified | State::Foreign)
        || (!a.uninstall && matches!(state, State::Newer(_)));
    let action = if a.uninstall && state == State::NotInstalled {
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
            _ => "reinstalled",
        }
    };
    let changes = matches!(action, "installed" | "upgraded" | "reinstalled" | "removed");
    if changes && !a.dry_run {
        if a.uninstall {
            fs::remove_file(&target).map_err(io)?;
            if let Err(e) = fs::remove_dir(&dir) {
                if e.kind() == std::io::ErrorKind::DirectoryNotEmpty {
                    warnings.push(Warning {
                        code: "extension_dir_not_empty".into(),
                        message: format!("kept nonempty {}", dir.display()),
                    });
                } else {
                    return Err(io(e));
                }
            }
        } else {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)
                .map_err(io)?;
            glen_home::write_private_atomic(
                &target,
                extension::render(env!("CARGO_PKG_VERSION"), &home.root).as_bytes(),
            )
            .map_err(io)?;
        }
    }
    warnings.extend(extension::socket_path_warning(&home.root));
    let restart = changes && !a.dry_run;
    let p = &ctx.painter;
    let (glyph, token) = if action == "skipped" {
        ("!", Token::Warning)
    } else if matches!(action, "unchanged" | "not_installed") {
        ("·", Token::Muted)
    } else {
        ("✓", Token::Success)
    };
    let heading = format!(
        "{glyph} {action} glen extension {}",
        env!("CARGO_PKG_VERSION")
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
    lines.extend(text::wrap("The glen extension adds glen_reply to every TUI session's tool list; uninstall with `glen install-extension --uninstall`.",ctx.width.map(usize::from)).into_iter().map(|line|p.paint(Token::Muted,&line)));
    let human = lines.join("\n");
    let mut o = Outcome::new(
        json!({"action":action,"path":path_data,"version":env!("CARGO_PKG_VERSION"),"previous_version":previous,"glen_home":baked_home,"profile":paths.profile.as_deref().unwrap_or("default"),"agent_dir":agent_data,"agent_dir_source":paths.agent_dir_source,"dry_run":a.dry_run,"restart_required":restart}),
        human,
    );
    o.warnings = warnings;
    Ok(o)
}
