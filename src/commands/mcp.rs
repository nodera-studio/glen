//! `glen mcp`: serve glen's commands to an MCP host over stdio (v0.2 W5, design §A22).
use crate::{
    cli::{Cli, McpArgs},
    env::Env,
    error::{ErrorCode, GlenError},
    output::{self, OutputCtx},
};
use std::process::ExitCode;

pub fn run(_cli: &Cli, _args: &McpArgs, _env: &Env, ctx: &OutputCtx) -> ExitCode {
    output::emit_error(
        ctx,
        GlenError::new(
            ErrorCode::NotImplemented,
            "glen mcp is not in this build yet",
        ),
    )
}
