pub mod cli;
pub mod commands;
pub mod env;
pub mod error;
pub mod glen_home;
pub mod omp;
pub mod output;
pub mod render;
use clap::Parser;
use std::{ffi::OsString, process::ExitCode};
pub fn run() -> ExitCode {
    let env = env::Env::from_process();
    let term = output::Terminal::detect();
    let argv: Vec<OsString> = std::env::args_os().collect();
    let cli = match cli::Cli::try_parse_from(&argv) {
        Ok(c) => c,
        Err(e) => return output::clap_error(e, &argv, &env, &term),
    };
    let ctx = match output::OutputCtx::new(&cli.global, cli.command.output_kind(), &env, &term) {
        Ok(c) => c,
        Err(e) => {
            return output::emit_error(
                &output::OutputCtx::fallback(output::pre_mode(&argv, &term), &env, &term),
                e,
            );
        }
    };
    match commands::dispatch(&cli, &env, &ctx) {
        Ok(o) => output::emit(&ctx, o, cli.command.name()),
        Err(e) => output::emit_error(&ctx, e),
    }
}
