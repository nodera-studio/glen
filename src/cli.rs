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
        after_help = "Examples:\n  glen send e27c0de \"run the tests\"\n  glen send @new hi --cwd ~/work/app\n  echo hi | glen send @latest - --no-wait\n  glen send e27c0de \"run the tests\" --warm 5m"
    )]
    Send(SendArgs),
    #[command(
        about = "Wait for a glen job (default 90s)",
        after_help = "Examples:\n  glen wait j1a2b3c4\n  glen wait j1a2b3c4 --timeout 2m\n  glen wait j1a2b3c4 --for reply"
    )]
    Wait(WaitArgs),
    #[command(
        about = "List glen jobs (newest first)",
        after_help = "Examples:\n  glen jobs --active\n  glen jobs -n 0 --json"
    )]
    Jobs(JobsArgs),
    #[command(
        visible_alias = "stop",
        about = "Stop the running turn of a session",
        after_help = "Examples:\n  glen abort e27c0de\n  glen stop @latest"
    )]
    Abort(CtlArgs),
    #[command(
        about = "List models (with providers) a session can use",
        after_help = "Examples:\n  glen models\n  glen models e27c0de --provider prov-a"
    )]
    Models(ModelsArgs),
    #[command(
        about = "Switch a session's model and provider",
        after_help = "Examples:\n  glen model e27c0de prov-a/model-1"
    )]
    Model(ModelArgs),
    #[command(
        about = "Set a session's thinking level",
        after_help = "Examples:\n  glen thinking @latest high"
    )]
    Thinking(ThinkingArgs),
    #[command(
        about = "Compact a session's context",
        after_help = "Examples:\n  glen compact e27c0de --instructions 'keep decisions'"
    )]
    Compact(CompactArgs),
    #[command(
        about = "Show replies omp sent back with glen_reply",
        after_help = "Examples:
  glen inbox
  glen inbox --peek --session e27c0de
  glen inbox -f
  glen inbox --wait --timeout 2m"
    )]
    Inbox(InboxArgs),
    #[command(
        about = "End warm workers (glen send --warm)",
        after_help = "Examples:\n  glen release e27c0de\n  glen release --all\n  glen release e27c0de --force"
    )]
    Release(ReleaseArgs),
    #[command(name = "__worker", hide = true, about = "Internal: run one glen job")]
    Worker(WorkerArgs),
    #[command(
        name = "__acp",
        hide = true,
        about = "Internal spike: send one message to omp over ACP v1 (unsupported, not a contract)"
    )]
    Acp(AcpArgs),
    #[command(
        about = "Watch sessions live: transcripts, replies and a prompt (full screen)",
        after_help = "Examples:\n  glen watch\n  glen watch e27c0de\n  glen watch --all"
    )]
    Watch(WatchArgs),
    #[command(
        about = "Serve glen's commands to an MCP host over stdio",
        after_help = "Examples:\n  claude mcp add glen -- glen mcp\n  codex mcp add glen -- glen mcp"
    )]
    Mcp(McpArgs),
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
    #[arg(long, value_name = "DIR", value_hint = clap::ValueHint::DirPath, help = "resolve the session in this project directory")]
    pub cwd: Option<PathBuf>,
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
    #[arg(long, value_name = "DIR", value_hint = clap::ValueHint::DirPath, help = "resolve the session in this project directory")]
    pub cwd: Option<PathBuf>,
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
        help = "with @new: the new session’s directory; otherwise: the project to resolve the session in"
    )]
    pub cwd: Option<PathBuf>,
    #[arg(
        long,
        value_name = "LEVEL",
        help = "omp's startup reasoning effort for the spawned process: off, minimal, low, medium, high, xhigh, max or auto",
        value_parser = parse_thinking_level
    )]
    pub thinking: Option<String>,
    #[arg(
        long,
        value_name = "DUR",
        value_parser = crate::rpc::warm::parse_warm,
        help = "keep omp alive for DUR after this job so the next send to the session skips startup (at most 10m; 0 = off)"
    )]
    pub warm: Option<Duration>,
}
/// omp 18.14.0's full thinking-level list (coding-agent/src/cli/flag-tables.ts).
/// A bad value is a usage error (exit 2), not a dispatch-time omp failure.
pub fn parse_thinking_level(s: &str) -> Result<String, String> {
    const LEVELS: &[&str] = &[
        "off", "minimal", "low", "medium", "high", "xhigh", "max", "auto",
    ];
    if LEVELS.contains(&s) {
        Ok(s.to_string())
    } else {
        Err(format!(
            "thinking level must be one of {} (got {s:?})",
            LEVELS.join(", ")
        ))
    }
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
/// How the hidden `__acp` spike answers omp's permission requests (v0.2 W3, design §A23).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum AcpPermission {
    Allow,
    Deny,
}
impl AcpPermission {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}
#[derive(Debug, Args)]
pub struct AcpArgs {
    #[arg(help = "session handle, id, @latest, or @new")]
    pub session: String,
    #[arg(help = "message text (use -- before a message that starts with -)")]
    pub message: String,
    #[arg(
        long,
        value_enum,
        default_value = "deny",
        help = "answer permission requests: allow picks allow_once, deny picks reject_once"
    )]
    pub permission: AcpPermission,
}
/// What `glen wait --for` returns on besides the end of the job (v0.2 W2, design §A20).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum WaitFor {
    Reply,
}
#[derive(Debug, Args)]
pub struct WaitArgs {
    #[arg(help = "job handle or full job UUID")]
    pub job: String,
    #[arg(long,help="maximum foreground wait; the job continues afterwards",default_value="90s",value_parser=parse_duration)]
    pub timeout: Duration,
    #[arg(
        long = "for",
        value_enum,
        value_name = "EVENT",
        help = "return when an unseen glen_reply arrives (exit 10, data.woke = reply), or when the job ends"
    )]
    pub wait_for: Option<WaitFor>,
    #[arg(
        long,
        value_name = "REPLY_ID",
        requires = "wait_for",
        help = "skip replies up to this id (the last id from the previous wake)"
    )]
    pub after: Option<String>,
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
    #[arg(long, value_name = "DIR", value_hint = clap::ValueHint::DirPath, help = "show messages from this project directory")]
    pub cwd: Option<PathBuf>,
    #[arg(long, help = "a session id or hex tail")]
    pub session: Option<String>,
    #[arg(long, help = "do not mark messages read")]
    pub peek: bool,
    #[arg(long, help = "include messages already read")]
    pub all: bool,
    #[arg(short = 'f', long, help = "stream new messages as NDJSON")]
    pub follow: bool,
    #[arg(
        long,
        conflicts_with_all = ["follow", "all"],
        help = "wait for an unread message: exit 0 with it, or exit 10 at --timeout"
    )]
    pub wait: bool,
    #[arg(
        long,
        value_name = "DUR",
        requires = "wait",
        default_value = "90s",
        value_parser = parse_duration,
        help = "maximum --wait (at most 24h)"
    )]
    pub timeout: Duration,
}
#[derive(Debug, Args)]
pub struct ReleaseArgs {
    #[arg(
        required_unless_present = "all",
        conflicts_with = "all",
        help = "session handle, session id or job handle of a warm worker"
    )]
    pub target: Option<String>,
    #[arg(long, help = "end every warm worker")]
    pub all: bool,
    #[arg(
        long,
        help = "kill a worker that does not release in time (after a pid and start-time check)"
    )]
    pub force: bool,
    #[arg(long,value_name="DUR",default_value="10s",value_parser=parse_control_timeout,help="how long to wait for each worker to release")]
    pub timeout: Duration,
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
pub struct McpArgs {
    /// Hidden for good (v0.2 W5, design §A22): protocol-tested only, the owner checks it live.
    #[arg(
        long,
        hide = true,
        help = "push new omp replies as notifications/claude/channel (unsupported)"
    )]
    pub channel: bool,
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
pub struct CtlArgs {
    #[arg(help = "session handle or @latest")]
    pub session: String,
    #[arg(long,value_name="DUR",default_value="90s",value_parser=parse_control_timeout,help="bound the whole command (default 90s)")]
    pub timeout: Duration,
}
#[derive(Debug, Args)]
pub struct ModelsArgs {
    #[arg(help = "session handle or @latest (omit for the profile's models)")]
    pub session: Option<String>,
    #[arg(long, value_name = "P", help = "filter providers case-insensitively")]
    pub provider: Option<String>,
    #[arg(long,value_name="DUR",default_value="90s",value_parser=parse_control_timeout,help="bound the whole command (default 90s)")]
    pub timeout: Duration,
}
#[derive(Debug, Args)]
pub struct ModelArgs {
    #[arg(help = "session handle or @latest")]
    pub session: String,
    #[arg(help = "exact provider/model or an unambiguous model id")]
    pub model: String,
    #[arg(long,value_name="DUR",default_value="90s",value_parser=parse_control_timeout,help="bound the whole command (default 90s)")]
    pub timeout: Duration,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ThinkingLevelArg {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}
impl ThinkingLevelArg {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}
#[derive(Debug, Args)]
pub struct ThinkingArgs {
    #[arg(help = "session handle or @latest")]
    pub session: String,
    #[arg(value_enum, help = "thinking level supported by the session's model")]
    pub level: ThinkingLevelArg,
    #[arg(long,value_name="DUR",default_value="90s",value_parser=parse_control_timeout,help="bound the whole command (default 90s)")]
    pub timeout: Duration,
}
#[derive(Debug, Args)]
pub struct CompactArgs {
    #[arg(help = "session handle or @latest")]
    pub session: String,
    #[arg(
        long,
        value_name = "TEXT",
        help = "instructions for the compaction (at most 512 KiB)"
    )]
    pub instructions: Option<String>,
    #[arg(long,value_name="DUR",default_value="90s",value_parser=parse_control_timeout,help="bound the whole command (default 90s)")]
    pub timeout: Duration,
}
pub fn parse_control_timeout(raw: &str) -> Result<Duration, String> {
    let value = parse_duration(raw)?;
    if value.is_zero() {
        Err("timeout must be greater than zero".into())
    } else {
        Ok(value)
    }
}
#[derive(Debug, Args)]
pub struct DoctorArgs {
    #[arg(
        long,
        help = "start omp briefly (no session, no prompt) and check its RPC capabilities"
    )]
    pub probe: bool,
}
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
            Self::Abort(_) => "abort",
            Self::Models(_) => "models",
            Self::Model(_) => "model",
            Self::Thinking(_) => "thinking",
            Self::Compact(_) => "compact",
            Self::Inbox(_) => "inbox",
            Self::Release(_) => "release",
            Self::Worker(_) => "__worker",
            Self::Acp(_) => "__acp",
            Self::Watch(_) => "watch",
            Self::Mcp(_) => "mcp",
            Self::InstallExtension(_) => "install-extension",
            Self::InstallSkill(_) => "install-skill",
            Self::Doctor(_) => "doctor",
            Self::Completions(_) => "completions",
        }
    }
    pub fn output_kind(&self) -> OutputKind {
        // `mcp`'s stdout carries MCP JSON-RPC only, so its refusals go to stderr as text (J10).
        if matches!(self, Self::Completions(_) | Self::Mcp(_)) {
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
    // 'static, so a rebuilt subcommand can keep its name (clap's `Str` takes only `&'static str`).
    static FULL: std::sync::OnceLock<clap::Command> = std::sync::OnceLock::new();
    let full: &'static clap::Command = FULL.get_or_init(Cli::command);
    clap::Command::new("glen")
        .version(env!("CARGO_PKG_VERSION"))
        .about("See and talk to omp sessions")
        .args(full.get_arguments().cloned())
        .subcommands(
            full.get_subcommands()
                .filter(|s| !s.is_hide_set())
                .map(without_hidden_args),
        )
}
/// clap_complete completes hidden arguments, so the public surface (completions, the skill and
/// docs checks) drops them from each visible subcommand.
fn without_hidden_args(s: &'static clap::Command) -> clap::Command {
    if !s.get_arguments().any(|a| a.is_hide_set()) {
        return s.clone();
    }
    let mut c = clap::Command::new(s.get_name())
        .args(s.get_arguments().filter(|a| !a.is_hide_set()).cloned());
    if let Some(about) = s.get_about() {
        c = c.about(about.clone());
    }
    if let Some(after) = s.get_after_help() {
        c = c.after_help(after.clone());
    }
    c
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
mod push_tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("glen").chain(args.iter().copied()))
    }
    fn wait(args: &[&str]) -> WaitArgs {
        match parse(args).unwrap().command {
            Command::Wait(a) => a,
            other => panic!("not wait: {}", other.name()),
        }
    }
    fn inbox(args: &[&str]) -> InboxArgs {
        match parse(args).unwrap().command {
            Command::Inbox(a) => a,
            other => panic!("not inbox: {}", other.name()),
        }
    }
    #[test]
    fn wait_for_reply_and_after() {
        let a = wait(&["wait", "j1234567"]);
        assert_eq!((a.wait_for, a.after), (None, None));
        let a = wait(&["wait", "j1234567", "--for", "reply", "--after", "r1"]);
        assert_eq!(a.wait_for, Some(WaitFor::Reply));
        assert_eq!(a.after.as_deref(), Some("r1"));
        assert_eq!(a.timeout, Duration::from_secs(90));
        for bad in [
            vec!["wait", "j1234567", "--after", "r1"],
            vec!["wait", "j1234567", "--for", "bogus"],
            vec!["wait", "j1234567", "--for"],
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
    }
    #[test]
    fn inbox_wait_and_timeout() {
        let a = inbox(&["inbox"]);
        assert!(!a.wait);
        assert_eq!(a.timeout, Duration::from_secs(90));
        let a = inbox(&["inbox", "--wait"]);
        assert!(a.wait);
        assert_eq!(a.timeout, Duration::from_secs(90));
        let a = inbox(&[
            "inbox",
            "--wait",
            "--timeout",
            "2m",
            "--peek",
            "--session",
            "c0de",
        ]);
        assert!(a.wait && a.peek);
        assert_eq!(a.timeout, Duration::from_secs(120));
        assert_eq!(
            inbox(&["inbox", "--wait", "--timeout", "24h"]).timeout,
            Duration::from_secs(86_400)
        );
        for bad in [
            vec!["inbox", "--timeout", "5s"],
            vec!["inbox", "--wait", "-f"],
            vec!["inbox", "--wait", "--follow"],
            vec!["inbox", "--wait", "--all"],
            vec!["inbox", "--wait", "--timeout", "25h"],
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
    }
}

#[cfg(test)]
mod acp_tests {
    use super::*;
    fn acp(args: &[&str]) -> Result<AcpArgs, clap::Error> {
        let cli = Cli::try_parse_from(std::iter::once("glen").chain(args.iter().copied()))?;
        match cli.command {
            Command::Acp(a) => Ok(a),
            other => panic!("not __acp: {}", other.name()),
        }
    }
    #[test]
    fn acp_parses_hidden_with_a_deny_default() {
        let a = acp(&["__acp", "x", "y"]).unwrap();
        assert_eq!((a.session.as_str(), a.message.as_str()), ("x", "y"));
        assert_eq!(a.permission, AcpPermission::Deny);
        for (flag, want) in [
            ("allow", AcpPermission::Allow),
            ("deny", AcpPermission::Deny),
        ] {
            let a = acp(&["__acp", "@new", "hi", "--permission", flag]).unwrap();
            assert_eq!(a.permission, want);
            assert_eq!(a.permission.as_str(), flag);
        }
        for bad in [
            vec!["__acp", "x", "y", "--permission", "bogus"],
            vec!["__acp", "x", "y", "--permission", "allow_always"],
            vec!["__acp", "x"],
        ] {
            assert!(
                Cli::try_parse_from(std::iter::once("glen").chain(bad.iter().copied())).is_err(),
                "{bad:?}"
            );
        }
        let parsed = Cli::try_parse_from(["glen", "__acp", "x", "y"]).unwrap();
        assert_eq!(parsed.command.name(), "__acp");
        assert!(matches!(parsed.command.output_kind(), OutputKind::Document));
        assert!(
            !public_command()
                .get_subcommands()
                .any(|c| c.get_name() == "__acp")
        );
        assert!(
            Cli::command()
                .find_subcommand("__acp")
                .is_some_and(|c| c.is_hide_set())
        );
    }
}

#[cfg(test)]
mod warm_tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("glen").chain(args.iter().copied()))
    }
    #[test]
    fn send_warm_parses_with_a_ten_minute_cap() {
        for (raw, ms) in [("2s", 2000), ("0", 0), ("10m", 600_000), ("5", 5000)] {
            let Command::Send(a) = parse(&["send", "x", "hi", "--warm", raw]).unwrap().command
            else {
                panic!("not send")
            };
            assert_eq!(a.warm, Some(Duration::from_millis(ms)), "{raw}");
        }
        let Command::Send(a) = parse(&["send", "x", "hi"]).unwrap().command else {
            panic!("not send")
        };
        assert_eq!(a.warm, None);
        for bad in ["11m", "abc", "10m1ms", ""] {
            let e = parse(&["send", "x", "hi", "--warm", bad]).unwrap_err();
            assert_eq!(e.exit_code(), 2, "{bad}");
        }
    }
    #[test]
    fn release_takes_a_target_or_all() {
        let release = |args: &[&str]| match parse(args).unwrap().command {
            Command::Release(a) => a,
            other => panic!("not release: {}", other.name()),
        };
        let a = release(&["release", "e27c0de"]);
        assert_eq!(a.target.as_deref(), Some("e27c0de"));
        assert!(!a.all && !a.force);
        assert_eq!(a.timeout, Duration::from_secs(10));
        let a = release(&["release", "--all"]);
        assert!(a.all && a.target.is_none());
        let a = release(&["release", "e27c0de", "--force", "--timeout", "5s"]);
        assert!(a.force);
        assert_eq!(a.timeout, Duration::from_secs(5));
        for bad in [
            vec!["release"],
            vec!["release", "x", "--all"],
            vec!["release", "x", "--timeout", "0"],
            vec!["release", "x", "--timeout", "25h"],
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            parse(&["release", "--all"]).unwrap().command.name(),
            "release"
        );
        let help = public_command()
            .find_subcommand_mut("release")
            .unwrap()
            .render_help()
            .to_string();
        assert_eq!(
            help.matches("10s").count(),
            1,
            "the default shown once: {help}"
        );
    }
    #[test]
    fn warm_surface_is_public() {
        let public = public_command();
        let release = public.find_subcommand("release").unwrap();
        assert!(!release.is_hide_set());
        let send = public.find_subcommand("send").unwrap();
        assert!(
            send.get_arguments()
                .any(|a| a.get_id() == "warm" && !a.is_hide_set())
        );
        assert!(
            send.get_after_help()
                .is_some_and(|h| h.to_string().contains("--warm 5m"))
        );
        let names: Vec<_> = public.get_subcommands().map(|c| c.get_name()).collect();
        let at = names.iter().position(|n| *n == "release").unwrap();
        assert_eq!(names[at - 1], "inbox");
        for shell in [
            clap_complete::Shell::Bash,
            clap_complete::Shell::Zsh,
            clap_complete::Shell::Fish,
        ] {
            let mut buf = Vec::new();
            clap_complete::generate(shell, &mut public_command(), "glen", &mut buf);
            let script = String::from_utf8(buf).unwrap();
            assert!(script.contains("warm"), "{shell}");
            assert!(script.contains("release"), "{shell}");
        }
    }
}

#[cfg(test)]
mod mcp_tests {
    use super::*;
    fn mcp(args: &[&str]) -> McpArgs {
        let cli = Cli::try_parse_from(std::iter::once("glen").chain(args.iter().copied())).unwrap();
        assert_eq!(cli.command.name(), "mcp");
        assert!(matches!(cli.command.output_kind(), OutputKind::Raw));
        match cli.command {
            Command::Mcp(a) => a,
            other => panic!("not mcp: {}", other.name()),
        }
    }
    #[test]
    fn mcp_and_its_channel_flag_parse() {
        assert!(!mcp(&["mcp"]).channel);
        assert!(mcp(&["mcp", "--channel"]).channel);
        assert!(mcp(&["--profile", "p", "mcp"]).channel.eq(&false));
        assert!(Cli::try_parse_from(["glen", "mcp", "extra"]).is_err());
    }
    #[test]
    fn channel_stays_hidden_and_mcp_has_a_help_page() {
        let public = public_command();
        let mcp = public.find_subcommand("mcp");
        assert!(
            mcp.is_none_or(|c| !c.get_arguments().any(|a| a.get_id() == "channel")),
            "the public surface never carries --channel"
        );
        let mut full = Cli::command();
        let sub = full.find_subcommand_mut("mcp").expect("mcp is declared");
        assert!(
            sub.get_arguments()
                .any(|a| a.get_id() == "channel" && a.is_hide_set())
        );
        let help = sub.render_help().to_string();
        assert!(help.contains("Usage:"), "{help}");
        assert!(!help.contains("channel"), "{help}");
    }
    #[test]
    fn mcp_surface_is_public() {
        let mut public = public_command();
        let names: Vec<_> = public.get_subcommands().map(|c| c.get_name()).collect();
        let at = names
            .iter()
            .position(|n| *n == "mcp")
            .expect("mcp is public");
        assert_eq!(
            (names[at - 1], names[at + 1]),
            ("watch", "install-extension")
        );
        let mcp = public.find_subcommand_mut("mcp").unwrap();
        assert!(!mcp.is_hide_set());
        let help = mcp.render_help().to_string();
        assert!(help.contains("claude mcp add glen -- glen mcp"), "{help}");
        assert!(!help.contains("channel"), "{help}");
        for shell in [
            clap_complete::Shell::Bash,
            clap_complete::Shell::Zsh,
            clap_complete::Shell::Fish,
        ] {
            let mut buf = Vec::new();
            clap_complete::generate(shell, &mut public_command(), "glen", &mut buf);
            let script = String::from_utf8(buf).unwrap();
            assert!(script.contains("mcp"), "{shell}");
            assert!(!script.contains("channel"), "{shell}");
        }
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

/// clap_complete's fish, elvish and PowerShell generators omit positional
/// values. Add the thinking vocabulary without changing the clap grammar.
pub fn complete_controls(shell: clap_complete::Shell, mut script: String) -> String {
    use clap_complete::Shell;
    match shell {
        Shell::Fish=>script.push_str(&format!("\ncomplete -c glen -n '__fish_glen_using_subcommand thinking; and test (count (commandline -opc)) -ge 3' -f -a '{}'\n",crate::ops::THINKING_LEVELS.join(" "))),
        Shell::Elvish=>{
            script=script.replace("for word $words[1..-1] {","for word $words[1..-1] {\n        if (str:has-prefix $command 'glen;thinking') { break }");
            let candidates=crate::ops::THINKING_LEVELS.iter().map(|level|format!("            cand {level} 'thinking level'\n")).collect::<String>();
            script=script.replace("&'glen;thinking'= {",&format!("&'glen;thinking'= {{\n{candidates}"));
        }
        Shell::PowerShell=>{
            script=script.replace("$element = $commandElements[$i]","if ($i -gt 1 -and $commandElements[$i-1].Value -eq 'thinking') { break }\n            $element = $commandElements[$i]");
            let candidates=crate::ops::THINKING_LEVELS.iter().map(|level|format!("            [CompletionResult]::new('{level}', '{level}', [CompletionResultType]::ParameterValue, 'thinking level')\n")).collect::<String>();
            script=script.replace("'glen;thinking' {",&format!("'glen;thinking' {{\n{candidates}"));
        }
        _=>(),
    }
    script
}
