//! `glen __acp` (hidden, v0.2 W3 spike; design §A23). Placeholder until the client lands.
use crate::{
    cli::{AcpArgs, Cli},
    env::Env,
    error::{ErrorCode, GlenError},
    output::{Outcome, OutputCtx},
};
pub fn run(
    _cli: &Cli,
    _args: &AcpArgs,
    _env: &Env,
    _ctx: &OutputCtx,
) -> Result<Outcome, GlenError> {
    Err(GlenError::new(
        ErrorCode::NotImplemented,
        "__acp is not implemented in this build",
    ))
}
