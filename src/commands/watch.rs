//! Full-screen entry point. Refusals happen before terminal modes or workers are touched.
use super::open::{self, Opened};
use crate::{
    cli::{Cli, Command},
    env::Env,
    error::{ErrorCode, GlenError},
    glen_home::{self, GlenHome},
    omp::paths,
    output::{self, Mode, OutputCtx, Terminal},
    render::{self, text},
    tui::{
        runtime,
        state::{App, Focus, Pane, Scope, Target, TranscriptStatus},
        theme::TuiTheme,
    },
};
use std::{
    io::{self, IsTerminal},
    process::ExitCode,
    sync::Arc,
    time::{Instant, SystemTime},
};

fn gate(ctx: &OutputCtx, term: &Terminal, env: &Env, stdin_tty: bool) -> Result<(), GlenError> {
    if ctx.mode == Mode::Json || !stdin_tty || !term.stdout_tty || env.get("TERM") == Some("dumb") {
        return Err(
            GlenError::new(ErrorCode::Usage, "glen watch needs an interactive terminal")
                .with_hint("agents: `glen tail -f <session>` and `glen inbox -f`"),
        );
    }
    Ok(())
}
struct Prepared {
    env: Env,
    home: GlenHome,
    app: App,
    opened: Option<Opened>,
}
fn prepare(
    cli: &Cli,
    env: &Env,
    ctx: &OutputCtx,
    term: &Terminal,
    stdin_tty: bool,
) -> Result<Prepared, GlenError> {
    gate(ctx, term, env, stdin_tty)?;
    // Glen has already resolved flag/environment precedence; do not let the backend reapply NO_COLOR.
    ratatui::crossterm::style::force_color_output(ctx.stdout_color);
    let Command::Watch(args) = &cli.command else {
        return Err(GlenError::new(
            ErrorCode::Internal,
            "watch entry point received another command",
        ));
    };
    let cwd = args
        .cwd
        .as_deref()
        .map(|p| paths::absolute(env, &p.to_string_lossy()));
    let scoped = cwd
        .as_deref()
        .map(|p| env.with_cwd(p))
        .unwrap_or_else(|| env.clone());
    let opened = args
        .session
        .as_deref()
        .map(|arg| open::open_watch(cli, &scoped, arg))
        .transpose()?;
    let project = cwd
        .or_else(|| {
            opened
                .as_ref()
                .and_then(|o| o.summary.cwd.as_deref())
                .map(|cwd| paths::absolute(env, cwd))
        })
        .unwrap_or_else(|| env.cwd().into());
    let bucket = Scope::Bucket {
        label: text::single_line(&render::tilde(&project, env.home())),
        cwd: project,
    };
    let mut app = App::new(
        TuiTheme::from_ctx(ctx, env),
        if args.all { Scope::All } else { bucket.clone() },
        (term.stdout_cols.unwrap_or(80), 24),
        Instant::now(),
        SystemTime::now(),
    );
    app.home = env.home().map(std::path::Path::to_path_buf);
    app.bucket_scope = Some(bucket);
    if let Some(opened) = &opened {
        app.list.selected = Some(opened.session.header.id.clone());
        app.transcript.r#gen = 1;
        app.transcript.target = Some(Target {
            id: opened.session.header.id.clone(),
            path: (!opened.not_yet_persisted).then(|| opened.session.path.clone()),
        });
        app.transcript.status = TranscriptStatus::Opening;
        app.focus = Focus::Transcript;
        app.narrow_tab = Pane::Transcript;
    }
    Ok(Prepared {
        env: scoped,
        home: glen_home::resolve_trusted(env, &["run/live", "jobs", "inbox/.read"])?,
        app,
        opened,
    })
}
pub fn run(cli: Cli, env: &Env, ctx: &OutputCtx, term: &Terminal) -> ExitCode {
    let prepared = match prepare(&cli, env, ctx, term, io::stdin().is_terminal()) {
        Ok(prepared) => prepared,
        Err(error) => return output::emit_error(ctx, error),
    };
    match runtime::run(
        Arc::new(cli),
        prepared.env,
        prepared.home,
        prepared.app,
        prepared.opened,
    ) {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(error) => output::emit_error(ctx, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::OutputKind;
    use clap::{CommandFactory, Parser};
    #[test]
    fn gate_rejects_each_noninteractive_case_before_reading_sessions_or_creating_home() {
        let tree = tempfile::tempdir().unwrap();
        let home = tree.path().join("absent-home");
        for json in [false, true] {
            for stdin in [false, true] {
                for stdout in [false, true] {
                    for dumb in [false, true] {
                        let env = Env::for_test(
                            Some(tree.path()),
                            tree.path(),
                            &[
                                ("GLEN_HOME", home.to_str().unwrap()),
                                (
                                    "PI_CODING_AGENT_DIR",
                                    tree.path().join("absent-agent").to_str().unwrap(),
                                ),
                                ("TERM", if dumb { "dumb" } else { "xterm-256color" }),
                            ],
                        );
                        let cli = Cli::try_parse_from(if json {
                            vec!["glen", "watch", "missing", "--json"]
                        } else {
                            vec!["glen", "watch", "missing", "--human"]
                        })
                        .unwrap();
                        let term = Terminal {
                            stdout_tty: stdout,
                            stderr_tty: false,
                            stdout_cols: Some(100),
                        };
                        let ctx =
                            OutputCtx::new(&cli.global, OutputKind::Document, &env, &term).unwrap();
                        if json || !stdin || !stdout || dumb {
                            let error = prepare(&cli, &env, &ctx, &term, stdin).err().unwrap();
                            assert_eq!(error.code, ErrorCode::Usage);
                            assert_eq!(error.message, "glen watch needs an interactive terminal");
                            assert_eq!(
                                error.hint.as_deref(),
                                Some("agents: `glen tail -f <session>` and `glen inbox -f`")
                            );
                        } else {
                            assert!(gate(&ctx, &term, &env, stdin).is_ok());
                        }
                        assert!(!home.exists());
                    }
                }
            }
        }
    }
    #[test]
    fn visible_watch_arguments_help_and_conflicts_match_the_entry_point() {
        let cli = Cli::try_parse_from(["glen", "watch", "e27c0de", "--all"]).unwrap();
        assert!(
            matches!(cli.command,Command::Watch(args) if args.all && args.session.as_deref() == Some("e27c0de"))
        );
        assert!(Cli::try_parse_from(["glen", "watch", "--all", "--cwd", "/fixture"]).is_err());
        assert!(
            crate::cli::public_command()
                .get_subcommands()
                .any(|c| c.get_name() == "watch")
        );
        let mut command = Cli::command();
        command.build();
        let watch = command.find_subcommand_mut("watch").unwrap();
        let help = watch.render_long_help().to_string();
        for text in [
            "transcripts, replies and a prompt",
            "glen watch e27c0de",
            "glen watch --all",
            "--cwd <DIR>",
        ] {
            assert!(help.contains(text), "{help}");
        }
    }
    #[test]
    fn initial_selection_reuses_complete_unprobed_open_and_cwd_scopes_latest() {
        use crate::{
            omp::{fixtures, lease},
            tui::runtime::initial_bucket,
        };
        let tree = fixtures::tree();
        let root = tempfile::tempdir().unwrap();
        let agent = root.path().join("agent");
        fn copy(from: &std::path::Path, to: &std::path::Path) {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                let dest = to.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy(&entry.path(), &dest);
                } else {
                    std::fs::copy(entry.path(), dest).unwrap();
                }
            }
        }
        copy(tree.path(), &agent.join("sessions"));
        let cwd = root.path().join("work/alpha");
        let scratch = root.path().join("scratch");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        let home = root.path().join("glen");
        let env = Env::for_test(
            Some(root.path()),
            &cwd,
            &[
                ("GLEN_HOME", home.to_str().unwrap()),
                ("PI_CODING_AGENT_DIR", agent.to_str().unwrap()),
                ("TMPDIR", scratch.to_str().unwrap()),
            ],
        );
        let term = Terminal {
            stdout_tty: true,
            stderr_tty: false,
            stdout_cols: Some(100),
        };
        let output = OutputCtx::fallback(Mode::Human, &env, &term);
        let path = agent
            .join("sessions/-work-alpha")
            .join(fixtures::path(&tree, 'A').file_name().unwrap());
        let mut bytes = fixtures::bytes('A').to_vec();
        bytes.extend(b"{partial");
        std::fs::write(&path, bytes).unwrap();
        let cli = Cli::try_parse_from(["glen", "watch", path.to_str().unwrap()]).unwrap();
        lease::PROBE_CALLS.with(|calls| calls.set(0));
        let prepared = prepare(&cli, &env, &output, &term, true).unwrap();
        let opened = prepared.opened.as_ref().unwrap();
        assert_eq!(
            prepared.app.list.selected.as_deref(),
            Some(opened.session.header.id.as_str())
        );
        assert_eq!(prepared.app.transcript.r#gen, 1);
        assert!(matches!(
            prepared.app.transcript.status,
            TranscriptStatus::Opening
        ));
        assert_eq!(
            initial_bucket(&cli, &prepared.app, Some(opened)).unwrap().1,
            "-work-alpha"
        );
        lease::PROBE_CALLS.with(|calls| assert_eq!(calls.get(), 0));
        assert!(!prepared.home.root.exists());
        let cwd = env.cwd().parent().unwrap().join("beta");
        std::fs::create_dir_all(&cwd).unwrap();
        let cli = Cli::try_parse_from(["glen", "watch", "@latest", "--cwd", cwd.to_str().unwrap()])
            .unwrap();
        let prepared = prepare(&cli, &env, &output, &term, true).unwrap();
        assert!(matches!(prepared.app.scope,Scope::Bucket {cwd:ref project,..} if project == &cwd));
        assert!(matches!(&prepared.app.scope, Scope::Bucket {label,..} if label == "~/work/beta"));
        assert_eq!(
            prepared.opened.as_ref().unwrap().summary.file.bucket,
            "-work-beta"
        );
        assert!(initial_bucket(&cli, &prepared.app, prepared.opened.as_ref()).is_none());
        let cli = Cli::try_parse_from(["glen", "watch", "--all"]).unwrap();
        let prepared = prepare(&cli, &env, &output, &term, true).unwrap();
        assert!(matches!(prepared.app.scope, Scope::All));
        assert!(prepared.app.bucket_scope.is_some());
        assert!(!prepared.home.root.exists());
    }
}
