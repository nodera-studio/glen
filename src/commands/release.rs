//! `glen release`: end warm workers (v0.2 W4, design §A21).
use crate::{
    cli::{Cli, ReleaseArgs},
    env::Env,
    error::{ErrorCode, GlenError},
    output::{Outcome, OutputCtx},
};
pub fn run(
    _cli: &Cli,
    _args: &ReleaseArgs,
    _env: &Env,
    _ctx: &OutputCtx,
) -> Result<Outcome, GlenError> {
    Err(GlenError::new(
        ErrorCode::NotImplemented,
        "glen release is not in this build yet",
    ))
}
