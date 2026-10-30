pub mod completions;
pub mod ctl;
pub mod doctor;
pub mod inbox;
pub mod install_extension;
pub mod install_skill;
pub mod jobs;
pub mod ls;
pub(crate) mod open;
pub mod send;
pub mod show;
pub mod tail;
pub mod wait;
use crate::{
    cli::{Cli, Command},
    env::Env,
    error::{ErrorCode, GlenError},
    output::{Outcome, OutputCtx},
};
pub fn dispatch(cli: &Cli, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    match &cli.command {
        Command::Inbox(a) => inbox::run(cli, a, env, ctx),
        Command::InstallExtension(a) => install_extension::run(cli, a, env, ctx),
        Command::Send(a) => send::run(cli, a, env, ctx),
        Command::Wait(a) => wait::run(cli, a, env, ctx),
        Command::Jobs(a) => jobs::run(cli, a, env, ctx),
        Command::Abort(a) => ctl::run_abort(cli, a, env, ctx),
        Command::Models(a) => ctl::run_models(cli, a, env, ctx),
        Command::Model(a) => ctl::run_model(cli, a, env, ctx),
        Command::Thinking(a) => ctl::run_thinking(cli, a, env, ctx),
        Command::Compact(a) => ctl::run_compact(cli, a, env, ctx),
        Command::Worker(_) => Err(GlenError::new(
            ErrorCode::Internal,
            "__worker is dispatched before commands",
        )),
        Command::Tail(a) => tail::run(cli, a, env, ctx),
        Command::Show(a) => show::run(cli, a, env, ctx),
        Command::Ls(a) => ls::run(cli, a, env, ctx),
        Command::InstallSkill(a) => install_skill::run(a, env, ctx),
        Command::Doctor(_) => doctor::run(cli, env, ctx),
        Command::Completions(args) => {
            let mut outcome = completions::run(args)?;
            outcome.human = crate::cli::complete_controls(args.shell, outcome.human);
            outcome.data["script"] = serde_json::json!(outcome.human);
            Ok(outcome)
        }
        cmd => Err(GlenError::new(
            ErrorCode::NotImplemented,
            format!("`glen {}` is not available in this build yet", cmd.name()),
        )
        .with_hint(
            "it is planned for glen v0.1.0; run `glen --help` for the commands in this build",
        )),
    }
}
