//! `glen mcp`: serve glen's commands to an MCP host over stdio (v0.2 W5, design §A22).
//! stdout carries protocol messages only; refusals go to stderr (`OutputKind::Raw`).
use crate::{
    cli::{Cli, McpArgs},
    env::Env,
    error::{ErrorCode, GlenError},
    mcp::{
        catalog,
        dispatch::Launcher,
        server::{self, Config, Exit},
    },
    output::{self, OutputCtx},
};
use std::{
    io::{self, BufReader, IsTerminal},
    path::PathBuf,
    process::ExitCode,
    sync::{Arc, atomic::AtomicUsize},
};

fn gate(stdin_tty: bool) -> Result<(), GlenError> {
    if stdin_tty {
        return Err(GlenError::new(
            ErrorCode::Usage,
            "glen mcp speaks MCP on stdin and stdout; an MCP host starts it, not a terminal",
        )
        .with_hint("register it with your MCP host: claude mcp add glen -- glen mcp"));
    }
    Ok(())
}

/// On Linux, a binary replaced while the server runs reads as `… (deleted)`; `/proc/self/exe`
/// still names the running image, so children keep starting.
fn self_exe(path: PathBuf, linux: bool) -> PathBuf {
    if linux && path.as_os_str().as_encoded_bytes().ends_with(b" (deleted)") {
        PathBuf::from("/proc/self/exe")
    } else {
        path
    }
}

pub fn run(cli: &Cli, args: &McpArgs, env: &Env, ctx: &OutputCtx) -> ExitCode {
    match serve(cli, args, env) {
        Ok(Exit::Eof) => ExitCode::SUCCESS,
        Ok(Exit::Signal(n)) => ExitCode::from(u8::try_from(128 + n).unwrap_or(1)),
        Err(e) => output::emit_error(ctx, e),
    }
}

fn serve(cli: &Cli, args: &McpArgs, env: &Env) -> Result<Exit, GlenError> {
    gate(io::stdin().is_terminal())?;
    let catalog = catalog::load().map_err(|e| GlenError::new(ErrorCode::Internal, e))?;
    let exe = std::env::current_exe().map_err(|e| {
        GlenError::new(
            ErrorCode::Internal,
            format!("cannot find the glen executable: {e}"),
        )
    })?;
    let signal = Arc::new(AtomicUsize::new(0));
    for sig in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        let value = usize::try_from(sig).unwrap_or(1);
        signal_hook::flag::register_usize(sig, signal.clone(), value).map_err(|e| {
            GlenError::new(
                ErrorCode::Io,
                format!("cannot register signal {sig} for glen mcp: {e}"),
            )
        })?;
    }
    let config = Config {
        catalog,
        launcher: Launcher {
            program: self_exe(exe, cfg!(target_os = "linux")),
            prefix_args: vec![],
        },
        profile: cli.global.profile.clone(),
        channel: args.channel,
        cwd: env.cwd().to_path_buf(),
    };
    Ok(server::run(
        BufReader::new(io::stdin()),
        io::stdout(),
        config,
        signal,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_terminal_stdin_is_refused() {
        let e = gate(true).unwrap_err();
        assert_eq!(e.code, ErrorCode::Usage);
        assert!(e.hint.unwrap().contains("claude mcp add glen -- glen mcp"));
        assert!(gate(false).is_ok());
    }
    #[test]
    fn a_replaced_binary_runs_through_proc_self_exe() {
        let deleted = PathBuf::from("/usr/local/bin/glen (deleted)");
        assert_eq!(
            self_exe(deleted.clone(), true),
            PathBuf::from("/proc/self/exe")
        );
        assert_eq!(self_exe(deleted.clone(), false), deleted);
        let live = PathBuf::from("/usr/local/bin/glen");
        assert_eq!(self_exe(live.clone(), true), live);
    }
}
