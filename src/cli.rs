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
    #[arg(
        long,
        global = true,
        value_enum,
        default_value = "auto",
        help = "when to colour output"
    )]
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
    #[command(
        about = "List omp sessions (current project; --all for every project)",
        after_help = "Examples:\n  glen ls\n  glen ls --all --live\n  glen ls --since 2h --json"
    )]
    Ls(LsArgs),
    #[command(
        about = "Read a session transcript (active branch)",
        after_help = "Examples:\n  glen show @latest\n  glen show e27c0de --json\n  glen show e27c0de --tools full --thinking"
    )]
    Show(ShowArgs),
    #[command(
        about = "Print the latest transcript items (-f follows)",
        after_help = "Examples:\n  glen tail @latest -n 5\n  glen tail e27c0de -f\n  glen tail e27c0de -n 0 -f --json"
    )]
    Tail(TailArgs),
    #[command(
        about = "Send a message to an omp session and wait for the reply (default 90s)",
        after_help = "Examples:\n  glen send e27c0de \"run the tests\"\n  glen send @new hi --cwd ~/work/app\n  echo hi | glen send @latest - --no-wait"
    )]
    Send(SendArgs),
    #[command(
        about = "Wait for a glen job (default 90s)",
        after_help = "Examples:\n  glen wait j1a2b3c4\n  glen wait j1a2b3c4 --timeout 2m"
    )]
    Wait(WaitArgs),
    #[command(
        about = "List glen jobs (newest first)",
        after_help = "Examples:\n  glen jobs --active\n  glen jobs -n 0 --json"
    )]
    Jobs(JobsArgs),
    #[command(
        about = "Show replies omp sent back with glen_reply",
        after_help = "Examples:
  glen inbox
  glen inbox --peek --session e27c0de
  glen inbox -f"
    )]
    Inbox(InboxArgs),
    #[command(name = "__worker", hide = true, about = "Internal: run one glen job")]
    Worker(WorkerArgs),
    #[command(
        about = "Watch sessions live: transcripts, replies and a prompt (full screen)",
        after_help = "Examples:\n  glen watch\n  glen watch e27c0de\n  glen watch --all"
    )]
    Watch(WatchArgs),
    #[command(
        about = "Install the bundled omp extension (live sessions, glen_reply)",
        after_help = "Examples:
  glen install-extension
  glen install-extension --dry-run
  glen install-extension --uninstall"
    )]
    InstallExtension(InstallExtensionArgs),
    #[command(
        about = "Install the glen skill for Claude Code and Codex",
        after_help = "Examples:\n  glen install-skill\n  glen install-skill --codex --project\n  glen install-skill --dry-run\n  glen install-skill --uninstall"
    )]
    InstallSkill(InstallSkillArgs),
    #[command(about = "Check omp, paths, glen home, skills and the shell sandbox")]
    Doctor(DoctorArgs),
    #[command(
        about = "Generate shell completions",
        after_help = "Install:\n  glen completions zsh > ~/.zfunc/_glen\n  glen completions bash > ~/.local/share/bash-completion/completions/glen\n  glen completions fish > ~/.config/fish/completions/glen.fish"
    )]
    Completions(CompletionsArgs),
}
#[derive(Debug, Args)]
pub struct LsArgs {
    #[arg(long, conflicts_with = "cwd", help = "list every project")]
    pub all: bool,
    #[arg(long, value_name = "DIR", value_hint = clap::ValueHint::DirPath, help = "list this project directory")]
    pub cwd: Option<PathBuf>,
    #[arg(
        short = 'n',
        long,
        default_value_t = 20,
        help = "maximum rows (0 = no limit)"
    )]
    pub limit: usize,
    #[arg(long, help = "show open sessions only")]
    pub live: bool,
    #[arg(long,value_name="DUR",value_parser=parse_since,help="updated within this duration")]
    pub since: Option<Duration>,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Tools {
    Collapsed,
    Full,
    None,
}
#[derive(Debug, Args)]
pub struct ShowArgs {
    #[arg(help = "session handle or @latest")]
    pub session: String,
    #[arg(long, help = "include model thinking")]
    pub thinking: bool,
    #[arg(
        long,
        value_enum,
        default_value = "collapsed",
        help = "tool-call detail"
    )]
    pub tools: Tools,
    #[arg(long, help = "return omp records unchanged")]
    pub raw: bool,
}
#[derive(Debug, Args)]
pub struct TailArgs {
    #[arg(help = "session handle or @latest")]
    pub session: String,
    #[arg(
        short = 'n',
        long,
        default_value_t = 10,
        help = "items (0 = only new with -f)"
    )]
    pub lines: usize,
    #[arg(short = 'f', long, help = "follow new items until interrupted")]
    pub follow: bool,
    #[arg(long, help = "include model thinking")]
    pub thinking: bool,
}
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum DeliverAs {
    Auto,
    Steer,
    FollowUp,
}
#[derive(Debug, Args)]
pub struct SendArgs {
    #[arg(help = "session handle, @latest for this project, or @new")]
    pub session: String,
    #[arg(help = "message text, or - to read stdin (use -- before a message that starts with -)")]
    pub message: String,
    #[arg(long, help = "steer the active turn (queued jobs send a prompt)", conflicts_with_all = ["deliver_as", "follow_up"])]
    pub steer: bool,
    #[arg(
        long = "follow-up",
        help = "queue a follow-up after the active turn",
        conflicts_with = "deliver_as"
    )]
    pub follow_up: bool,
    #[arg(
        long = "as",
        help = "how to deliver the message",
        value_enum,
        default_value = "auto"
    )]
    pub deliver_as: DeliverAs,
    #[arg(long,help="maximum foreground wait; the job continues afterwards",default_value="90s",value_parser=parse_duration,conflicts_with="no_wait")]
    pub wait: Duration,
    #[arg(long, help = "return the job handle immediately")]
    pub no_wait: bool,
    #[arg(
        long,
        value_name = "DIR",
        value_hint = clap::ValueHint::DirPath,
        help = "with @new: the new session’s directory; with @latest: the project to pick from"
    )]
    pub cwd: Option<PathBuf>,
}
impl SendArgs {
    pub fn deliver(&self) -> DeliverAs {
        if self.steer {
            DeliverAs::Steer
        } else if self.follow_up {
            DeliverAs::FollowUp
        } else {
            self.deliver_as
        }
    }
}
#[derive(Debug, Args)]
pub struct WorkerArgs {
    pub job: String,
}
#[derive(Debug, Args)]
pub struct WaitArgs {
    #[arg(help = "job handle or full job UUID")]
    pub job: String,
    #[arg(long,help="maximum foreground wait; the job continues afterwards",default_value="90s",value_parser=parse_duration)]
    pub timeout: Duration,
}
#[derive(Debug, Args)]
pub struct JobsArgs {
    #[arg(long, help = "a session id or hex tail")]
    pub session: Option<String>,
    #[arg(long, help = "show only queued and accepted jobs")]
    pub active: bool,
    #[arg(
        short = 'n',
        long,
        default_value_t = 20,
        help = "maximum rows (0 = no limit)"
    )]
    pub limit: usize,
}
#[derive(Debug, Args)]
pub struct InboxArgs {
    #[arg(long, help = "a session id or hex tail")]
    pub session: Option<String>,
    #[arg(long, help = "do not mark messages read")]
    pub peek: bool,
    #[arg(long, help = "include messages already read")]
    pub all: bool,
    #[arg(short = 'f', long, help = "stream new messages as NDJSON")]
    pub follow: bool,
}
#[derive(Debug, Args)]
pub struct WatchArgs {
    #[arg(help = "session handle or @latest; otherwise select from the session list")]
    pub session: Option<String>,
    #[arg(long, conflicts_with = "cwd", help = "watch every project")]
    pub all: bool,
    #[arg(long, value_name = "DIR", value_hint = clap::ValueHint::DirPath, help = "watch this project directory; scope @latest to it")]
    pub cwd: Option<PathBuf>,
}
#[derive(Debug, Args)]
pub struct InstallExtensionArgs {
    #[arg(long, help = "show what would change")]
    pub dry_run: bool,
    #[arg(long, help = "overwrite a newer, modified or foreign file")]
    pub force: bool,
    #[arg(long, help = "remove the extension")]
    pub uninstall: bool,
}
#[derive(Debug, Args)]
pub struct InstallSkillArgs {
    #[arg(long, help = "only Claude Code")]
    pub claude: bool,
    #[arg(long, help = "only Codex")]
    pub codex: bool,
    #[arg(
        long,
        help = "write into this directory’s .claude/skills and .agents/skills"
    )]
    pub project: bool,
    #[arg(long, help = "show what would change")]
    pub dry_run: bool,
    #[arg(long, help = "replace a modified, newer or foreign SKILL.md")]
    pub force: bool,
    #[arg(long, help = "remove the skill")]
    pub uninstall: bool,
}
#[derive(Debug, Args)]
pub struct DoctorArgs {}
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    #[arg(help = "shell to generate completions for")]
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
            Self::Worker(_) => "__worker",
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
pub fn parse_since(s: &str) -> Result<Duration, String> {
    let (digits, mult) = match s.as_bytes().last() {
        Some(b's') => (&s[..s.len() - 1], 1),
        Some(b'm') => (&s[..s.len() - 1], 60),
        Some(b'h') => (&s[..s.len() - 1], 3600),
        Some(b'd') => (&s[..s.len() - 1], 86400),
        Some(b'w') => (&s[..s.len() - 1], 604800),
        _ => (s, 1),
    };
    let invalid =
        || "duration must be an unsigned integer with s, m, h, d or w (at most 3650d)".to_string();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let seconds = digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(mult))
        .filter(|n| *n <= 3650 * 86400)
        .ok_or_else(invalid)?;
    Ok(Duration::from_secs(seconds))
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

#[cfg(test)]
mod since_tests {
    use super::*;
    #[test]
    fn durations() {
        for (s, n) in [
            ("90", 90),
            ("15m", 900),
            ("2h", 7200),
            ("3d", 259200),
            ("1w", 604800),
        ] {
            assert_eq!(parse_since(s).unwrap().as_secs(), n)
        }
        for s in ["1y", "-1", "3651d", "", "+1", "18446744073709551615w"] {
            assert!(parse_since(s).is_err());
        }
    }
}

#[cfg(test)]
mod rpc_tests {
    use super::*;
    #[test]
    fn deliver_aliases_conflicts_and_private_worker() {
        for (flags, want) in [
            (vec!["--steer"], DeliverAs::Steer),
            (vec!["--follow-up"], DeliverAs::FollowUp),
            (vec!["--as", "steer"], DeliverAs::Steer),
        ] {
            let mut args = vec!["glen", "send", "x", "hi"];
            args.extend(flags);
            let c = Cli::try_parse_from(args).unwrap();
            let Command::Send(a) = c.command else {
                panic!("not send")
            };
            assert_eq!(a.deliver(), want);
        }
        for flags in [
            vec!["--steer", "--follow-up"],
            vec!["--as", "steer", "--steer"],
        ] {
            let mut args = vec!["glen", "send", "x", "hi"];
            args.extend(flags);
            assert!(Cli::try_parse_from(args).is_err());
        }
        assert!(
            !public_command()
                .get_subcommands()
                .any(|c| c.get_name() == "__worker")
        );
    }
}

#[cfg(test)]
mod help_tests {
    #[test]
    fn every_visible_arg_has_help() {
        let mut cmd = super::public_command();
        cmd.build();
        for command in std::iter::once(&cmd).chain(cmd.get_subcommands()) {
            for arg in command
                .get_arguments()
                .filter(|a| !a.is_hide_set() && !matches!(a.get_id().as_str(), "help" | "version"))
            {
                assert!(
                    arg.get_help()
                        .is_some_and(|h| !h.to_string().trim().is_empty()),
                    "{} {}",
                    command.get_name(),
                    arg.get_id()
                );
            }
        }
    }
}
