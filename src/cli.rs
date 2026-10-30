use crate::output::OutputKind;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use std::{path::PathBuf, time::Duration};
#[derive(Debug, Parser)]
#[command(name="glen",version,about="See and talk to omp sessions",propagate_version=true,subcommand_required=true,arg_required_else_help=true,styles=crate::render::palette::clap_styles())]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Command,
}
#[derive(Debug, Args, Default)]
pub struct GlobalArgs {
    #[arg(
        long,
        global = true,
        conflicts_with = "human",
        help = "Print a JSON envelope"
    )]
    pub json: bool,
    #[arg(long, global = true, help = "Print human-readable output")]
    pub human: bool,
    #[arg(long, global = true, value_enum, default_value = "auto")]
    pub color: ColorWhen,
    #[arg(
        long,
        global = true,
        value_name = "NAME",
        help = "omp profile; overrides OMP_PROFILE and PI_PROFILE"
    )]
    pub profile: Option<String>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ColorWhen {
    #[default]
    Auto,
    Always,
    Never,
}
#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(hide = true, about = "List omp sessions")]
    Ls(LsArgs),
    #[command(hide = true, about = "Read an active-branch transcript")]
    Show(ShowArgs),
    #[command(hide = true, about = "Read the latest transcript items")]
    Tail(TailArgs),
    #[command(hide = true, about = "Send a message to omp")]
    Send(SendArgs),
    #[command(hide = true, about = "Wait for a glen job")]
    Wait(WaitArgs),
    #[command(hide = true, about = "List glen jobs")]
    Jobs(JobsArgs),
    #[command(hide = true, about = "Read replies from omp")]
    Inbox(InboxArgs),
    #[command(hide = true, about = "Watch a session live")]
    Watch(WatchArgs),
    #[command(hide = true, about = "Install the bundled omp extension")]
    InstallExtension(InstallExtensionArgs),
    #[command(hide = true, about = "Install glen agent skills")]
    InstallSkill(InstallSkillArgs),
    #[command(about = "Check omp, paths and glen home")]
    Doctor(DoctorArgs),
    #[command(about = "Generate shell completions")]
    Completions(CompletionsArgs),
}
#[derive(Debug, Args)]
pub struct LsArgs {
    #[arg(long)]
    pub all: bool,
    #[arg(long, value_name = "DIR")]
    pub cwd: Option<PathBuf>,
    #[arg(short = 'n', long, default_value_t = 20)]
    pub limit: usize,
    #[arg(long)]
    pub live: bool,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Tools {
    Collapsed,
    Full,
    None,
}
#[derive(Debug, Args)]
pub struct ShowArgs {
    pub session: String,
    #[arg(long)]
    pub thinking: bool,
    #[arg(long, value_enum, default_value = "collapsed")]
    pub tools: Tools,
    #[arg(long)]
    pub raw: bool,
}
#[derive(Debug, Args)]
pub struct TailArgs {
    pub session: String,
    #[arg(short = 'n', long, default_value_t = 10)]
    pub lines: usize,
    #[arg(short = 'f', long)]
    pub follow: bool,
    #[arg(long)]
    pub thinking: bool,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum DeliverAs {
    Auto,
    Steer,
    FollowUp,
}
#[derive(Debug, Args)]
pub struct SendArgs {
    pub session: String,
    pub message: String,
    #[arg(long = "as", value_enum, default_value = "auto")]
    pub deliver_as: DeliverAs,
    #[arg(long,default_value="90s",value_parser=parse_duration,conflicts_with="no_wait")]
    pub wait: Duration,
    #[arg(long)]
    pub no_wait: bool,
    #[arg(long, value_name = "DIR")]
    pub cwd: Option<PathBuf>,
}
#[derive(Debug, Args)]
pub struct WaitArgs {
    pub job: String,
    #[arg(long,default_value="90s",value_parser=parse_duration)]
    pub timeout: Duration,
}
#[derive(Debug, Args)]
pub struct JobsArgs {
    #[arg(long)]
    pub session: Option<String>,
    #[arg(long)]
    pub active: bool,
    #[arg(short = 'n', long, default_value_t = 20)]
    pub limit: usize,
}
#[derive(Debug, Args)]
pub struct InboxArgs {
    #[arg(long)]
    pub session: Option<String>,
    #[arg(long)]
    pub peek: bool,
    #[arg(long)]
    pub all: bool,
    #[arg(short = 'f', long)]
    pub follow: bool,
}
#[derive(Debug, Args)]
pub struct WatchArgs {
    pub session: Option<String>,
}
#[derive(Debug, Args)]
pub struct InstallExtensionArgs {
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub uninstall: bool,
}
#[derive(Debug, Args)]
pub struct InstallSkillArgs {
    #[arg(long)]
    pub claude: bool,
    #[arg(long)]
    pub codex: bool,
    #[arg(long)]
    pub project: bool,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub uninstall: bool,
}
#[derive(Debug, Args)]
pub struct DoctorArgs {}
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    pub shell: clap_complete::Shell,
}
impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Ls(_) => "ls",
            Self::Show(_) => "show",
            Self::Tail(_) => "tail",
            Self::Send(_) => "send",
            Self::Wait(_) => "wait",
            Self::Jobs(_) => "jobs",
            Self::Inbox(_) => "inbox",
            Self::Watch(_) => "watch",
            Self::InstallExtension(_) => "install-extension",
            Self::InstallSkill(_) => "install-skill",
            Self::Doctor(_) => "doctor",
            Self::Completions(_) => "completions",
        }
    }
    pub fn output_kind(&self) -> OutputKind {
        if matches!(self, Self::Completions(_)) {
            OutputKind::Raw
        } else {
            OutputKind::Document
        }
    }
}
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let (digits, mult) = if let Some(v) = s.strip_suffix("ms") {
        (v, 1)
    } else if let Some(v) = s.strip_suffix('s') {
        (v, 1000)
    } else if let Some(v) = s.strip_suffix('m') {
        (v, 60_000)
    } else if let Some(v) = s.strip_suffix('h') {
        (v, 3_600_000)
    } else {
        (s, 1000)
    };
    let invalid =
        || "duration must be an unsigned integer with ms, s, m or h (at most 24h)".to_string();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let ms = digits
        .parse::<u64>()
        .ok()
        .and_then(|v| v.checked_mul(mult))
        .filter(|v| *v <= 86_400_000)
        .ok_or_else(invalid)?;
    Ok(Duration::from_millis(ms))
}
pub fn public_command() -> clap::Command {
    let full = Cli::command();
    clap::Command::new("glen")
        .version(env!("CARGO_PKG_VERSION"))
        .about("See and talk to omp sessions")
        .args(full.get_arguments().cloned())
        .subcommands(full.get_subcommands().filter(|s| !s.is_hide_set()).cloned())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn valid_cli() {
        Cli::command().debug_assert();
        public_command().debug_assert();
        assert!(public_command().get_subcommands().all(|c| !c.is_hide_set()));
    }
    #[test]
    fn durations() {
        for (s, ms) in [
            ("90s", 90000),
            ("2m", 120000),
            ("1500ms", 1500),
            ("1h", 3600000),
            ("45", 45000),
            ("24h", 86400000),
            ("0", 0),
        ] {
            assert_eq!(parse_duration(s).unwrap(), Duration::from_millis(ms));
        }
        for s in ["abc", "-1", "25h", "+1", "", "18446744073709551615h"] {
            assert!(parse_duration(s).is_err());
        }
    }
}
