pub mod cli;
pub mod commands;
pub mod ctl;
pub mod env;
pub mod error;
pub mod glen_home;
pub mod jobs;
pub mod omp;
pub mod output;
pub mod render;
pub mod rpc;
pub mod sandbox;
pub mod skill;
pub mod tui;
pub mod version;
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
    if let cli::Command::Worker(a) = &cli.command {
        return rpc::worker::main(&a.job);
    }
    let ctx = match output::OutputCtx::new(&cli.global, cli.command.output_kind(), &env, &term) {
        Ok(c) => c,
        Err(e) => {
            return output::emit_error(
                &output::OutputCtx::fallback(output::pre_mode(&argv, &term), &env, &term),
                e,
            );
        }
    };
    // AC-020: a relative GLEN_HOME is invalid for every command, not only ones that touch the
    // glen home directory. Validate before dispatch; `resolve` never creates the directory.
    if let Err(e) = glen_home::resolve(&env) {
        return output::emit_error(&ctx, e);
    }
    if let cli::Command::Tail(a) = &cli.command
        && a.follow
    {
        return commands::tail::follow(&cli, a, &env, &ctx);
    }
    if let cli::Command::Inbox(a) = &cli.command
        && a.follow
    {
        return commands::inbox::follow(&cli, a, &env, &ctx);
    }
    if matches!(cli.command, cli::Command::Watch(_)) {
        return commands::watch::run(cli, &env, &ctx, &term);
    }
    match commands::dispatch(&cli, &env, &ctx) {
        Ok(o) => output::emit(&ctx, o, cli.command.name()),
        Err(e) => output::emit_error(&ctx, e),
    }
}

pub mod live;
pub mod ops;

pub(crate) mod atomic_swap;
pub mod extension;
pub(crate) mod install_guard;

pub mod inbox;
