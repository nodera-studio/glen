pub mod completions;
pub mod doctor;
pub mod ls;
mod open;
pub mod show;
pub mod tail;
use crate::{
    cli::{Cli, Command},
    env::Env,
    error::{ErrorCode, GlenError},
    output::{Outcome, OutputCtx},
};
pub fn dispatch(cli: &Cli, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    match &cli.command {
        Command::Tail(a) => tail::run(cli, a, env, ctx),
        Command::Show(a) => show::run(cli, a, env, ctx),
        Command::Ls(a) => ls::run(cli, a, env, ctx),
        Command::Doctor(_) => doctor::run(cli, env, ctx),
        Command::Completions(args) => completions::run(args),
        cmd => Err(GlenError::new(
            ErrorCode::NotImplemented,
            format!("`glen {}` is not available in this build yet", cmd.name()),
        )
        .with_hint(
            "it is planned for glen v0.1.0; run `glen --help` for the commands in this build",
        )),
    }
}
