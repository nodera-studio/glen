//! The hidden ACP v1 spike behind `glen __acp` (v0.2 W3, design §A23): a synchronous JSON-RPC 2.0
//! client for `omp acp` over stdio. Not a public contract, not in production routing. This module
//! is the pure half (constants, the update accumulator, the permission policy, the error
//! mapping); [`client`] owns the process and the wire.
pub mod client;

use crate::{
    cli::AcpPermission,
    env::Env,
    error::{ErrorCode, GlenError},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, process::Command, time::Duration};

/// The only ACP version glen speaks; omp 18.14.0 answers its own `PROTOCOL_VERSION` (1).
pub const PROTOCOL_VERSION: u64 = 1;
/// Every wait polls the stop flag and the deadlines at least this often.
pub const TICK: Duration = Duration::from_millis(100);
/// The bound of `initialize`, `session/new` and `session/load` (each also capped by the job).
pub const SETUP_TIMEOUT: Duration = Duration::from_secs(60);
/// How long glen waits for the prompt response after `session/cancel`.
pub const CANCEL_GRACE: Duration = Duration::from_secs(5);
/// The bound of the best-effort `session/close`.
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
/// Exit grace after stdin EOF on a success or a confirmed cancel.
pub const EXIT_GRACE: Duration = Duration::from_secs(2);
/// Exit grace on every other path (unconfirmed cancel, deadline, failure, early interrupt).
pub const STOP_GRACE: Duration = Duration::from_millis(100);
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_EVENTS: usize = 100;

/// A debug-build-only millisecond override; release builds and out-of-range values ignore it.
fn debug_ms(env: &Env, key: &str, min: u64, max: u64) -> Option<Duration> {
    if !cfg!(debug_assertions) {
        return None;
    }
    env.get(key)?
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|ms| (min..=max).contains(ms))
        .map(Duration::from_millis)
}
pub fn setup_timeout(env: &Env) -> Duration {
    debug_ms(env, "GLEN_ACP_SETUP_MS", 100, 60_000).unwrap_or(SETUP_TIMEOUT)
}
pub fn cancel_grace(env: &Env) -> Duration {
    debug_ms(env, "GLEN_ACP_CANCEL_GRACE_MS", 100, 10_000).unwrap_or(CANCEL_GRACE)
}

/// `omp acp [--profile NAME]` in `cwd` with `GLEN_MANAGED=1`; an explicit profile drops the
/// inherited `OMP_PROFILE` and `PI_PROFILE`, as `rpc::base_command` does.
pub fn command(bin: &Path, profile_flag: Option<&str>, cwd: &Path) -> Command {
    let mut command = Command::new(bin);
    command.arg("acp").env("GLEN_MANAGED", "1").current_dir(cwd);
    if let Some(profile) = profile_flag {
        command
            .args(["--profile", profile.trim()])
            .env_remove("OMP_PROFILE")
            .env_remove("PI_PROFILE");
    }
    command
}

#[derive(Debug, Default)]
struct Group {
    id: Option<String>,
    text: String,
    truncated: bool,
}
/// Rebuilds the turn's assistant text from `session/update` notifications (design §A23 "Text
/// rebuild"). Only updates for the chosen session that arrive after [`Updates::begin_turn`]
/// count; earlier ones (history replay, bootstrap) are only counted as `replayed`.
#[derive(Debug, Default)]
pub struct Updates {
    session: Option<String>,
    in_turn: bool,
    current: Option<Group>,
    last: Option<Group>,
    closed_messages: u64,
    pub replayed: u64,
    pub during_turn: u64,
    pub kinds: BTreeMap<String, u64>,
}
impl Updates {
    /// Updates for any other session id are ignored from now on.
    pub fn for_session(&mut self, id: &str) {
        self.session = Some(id.to_owned());
    }
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }
    pub fn begin_turn(&mut self) {
        self.in_turn = true;
    }
    /// Applies a `session/update` notification's params. Malformed params are ignored.
    pub fn apply(&mut self, params: &Value) {
        let Some(session) = self.session.as_deref() else {
            return;
        };
        if params.get("sessionId").and_then(Value::as_str) != Some(session) {
            return;
        }
        let update = &params["update"];
        let Some(kind) = update.get("sessionUpdate").and_then(Value::as_str) else {
            return;
        };
        if !self.in_turn {
            self.replayed += 1;
            return;
        }
        self.during_turn += 1;
        *self.kinds.entry(kind.to_owned()).or_default() += 1;
        match kind {
            "agent_message_chunk" if update["content"]["type"] == "text" => {
                if let Some(text) = update["content"]["text"].as_str() {
                    self.push(update.get("messageId").and_then(Value::as_str), text);
                }
            }
            "tool_call" | "user_message_chunk" => self.close(),
            _ => (),
        }
    }
    fn push(&mut self, id: Option<&str>, text: &str) {
        if self.current.as_ref().is_some_and(|g| g.id.as_deref() != id) {
            self.close();
        }
        let group = self.current.get_or_insert_with(|| Group {
            id: id.map(str::to_owned),
            ..Group::default()
        });
        let room = MAX_TEXT_BYTES.saturating_sub(group.text.len());
        if text.len() <= room {
            group.text.push_str(text);
        } else {
            let mut end = room;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            group.text.push_str(&text[..end]);
            group.truncated = true;
        }
    }
    fn close(&mut self) {
        if let Some(group) = self.current.take()
            && !group.text.is_empty()
        {
            self.closed_messages += 1;
            self.last = Some(group);
        }
    }
    fn chosen(&self) -> Option<&Group> {
        self.current
            .as_ref()
            .filter(|g| !g.text.is_empty())
            .or(self.last.as_ref())
    }
    /// The text of the last non-empty message group of the turn.
    pub fn assistant_text(&self) -> Option<&str> {
        self.chosen().map(|g| g.text.as_str())
    }
    /// True when the reported text hit the 4 MiB cap.
    pub fn text_truncated(&self) -> bool {
        self.chosen().is_some_and(|g| g.truncated)
    }
    /// Non-empty assistant message groups during the turn.
    pub fn messages(&self) -> u64 {
        self.closed_messages + u64::from(self.current.as_ref().is_some_and(|g| !g.text.is_empty()))
    }
    pub fn json(&self) -> Value {
        json!({"replayed":self.replayed,"during_turn":self.during_turn,"messages":self.messages(),"kinds":self.kinds})
    }
}

/// glen's answer to one `session/request_permission`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Selected {
        option_id: String,
        kind: &'static str,
    },
    Cancelled,
}
impl Answer {
    /// The `RequestPermissionResponse` result.
    pub fn result(&self) -> Value {
        match self {
            Self::Selected { option_id, .. } => {
                json!({"outcome":{"outcome":"selected","optionId":option_id}})
            }
            Self::Cancelled => json!({"outcome":{"outcome":"cancelled"}}),
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Selected { kind, .. } => kind,
            Self::Cancelled => "cancelled",
        }
    }
}
/// `allow` picks the offered `allow_once`, `deny` the offered `reject_once`; never an `*_always`
/// kind (omp caches those per session). A missing wanted kind is `cancelled`.
pub fn answer(policy: AcpPermission, options: &[Value]) -> Answer {
    let want = match policy {
        AcpPermission::Allow => "allow_once",
        AcpPermission::Deny => "reject_once",
    };
    options
        .iter()
        .filter(|o| o.get("kind").and_then(Value::as_str) == Some(want))
        .find_map(|o| o.get("optionId").and_then(Value::as_str))
        .map_or(Answer::Cancelled, |id| Answer::Selected {
            option_id: id.to_owned(),
            kind: want,
        })
}
/// The `data.events` entry for one answered permission request.
pub fn input_required_event(params: &Value, policy: AcpPermission, answer: &Answer) -> Value {
    let tool = &params["toolCall"];
    let text = |key: &str| tool.get(key).and_then(Value::as_str).map(str::to_owned);
    let option_id = match answer {
        Answer::Selected { option_id, .. } => Some(option_id.as_str()),
        Answer::Cancelled => None,
    };
    json!({"event":"task_state","task_state":"input_required","tool_call_id":text("toolCallId"),"title":text("title"),"kind":text("kind"),"policy":policy.as_str(),"answer":answer.as_str(),"option_id":option_id})
}
/// The §A7 `task_state` of an ACP stop reason.
pub fn task_state(stop_reason: &str) -> &'static str {
    match stop_reason {
        "end_turn" => "completed",
        "cancelled" => "canceled",
        "refusal" => "rejected",
        _ => "failed",
    }
}

/// Before or after omp's `initialize` result was accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Handshake,
    Session,
}
/// The `details.reason` vocabulary of `__acp` failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    AcpSpawn,
    AcpExited,
    AcpTimeout,
    AcpProtocol,
    AcpMalformed,
    AcpCapability,
    AcpRpcError,
    AcpStopReason,
}
impl Reason {
    pub const ALL: [Self; 8] = [
        Self::AcpSpawn,
        Self::AcpExited,
        Self::AcpTimeout,
        Self::AcpProtocol,
        Self::AcpMalformed,
        Self::AcpCapability,
        Self::AcpRpcError,
        Self::AcpStopReason,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AcpSpawn => "acp_spawn",
            Self::AcpExited => "acp_exited",
            Self::AcpTimeout => "acp_timeout",
            Self::AcpProtocol => "acp_protocol",
            Self::AcpMalformed => "acp_malformed",
            Self::AcpCapability => "acp_capability",
            Self::AcpRpcError => "acp_rpc_error",
            Self::AcpStopReason => "acp_stop_reason",
        }
    }
}
/// A failure as the §A23 failure table maps it: exit 5 `omp_unavailable` until omp's
/// `initialize` result is accepted, exit 7 `job_failed` after it.
pub fn error(stage: Stage, reason: Reason, message: impl Into<String>) -> GlenError {
    let code = match (reason, stage) {
        (Reason::AcpSpawn | Reason::AcpProtocol | Reason::AcpCapability, _) => {
            ErrorCode::OmpUnavailable
        }
        (Reason::AcpRpcError | Reason::AcpStopReason, _) => ErrorCode::JobFailed,
        (_, Stage::Handshake) => ErrorCode::OmpUnavailable,
        (_, Stage::Session) => ErrorCode::JobFailed,
    };
    let error = GlenError::new(code, message).with_detail("reason", reason.as_str());
    if reason == Reason::AcpSpawn {
        error.with_hint("install omp or set GLEN_OMP_BIN")
    } else {
        error
    }
}
/// A JSON-RPC error's text: omp wraps a thrown error as -32603 with the text in `data.details`,
/// which wins over the generic `message`.
pub fn rpc_error_message(error: &Value) -> String {
    error["data"]["details"]
        .as_str()
        .or_else(|| error["message"].as_str())
        .unwrap_or("unknown JSON-RPC error")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn update(session: &str, update: Value) -> Value {
        json!({"sessionId":session,"update":update})
    }
    fn chunk(text: &str, id: Option<&str>) -> Value {
        let mut u =
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}});
        if let Some(id) = id {
            u["messageId"] = json!(id);
        }
        update("s1", u)
    }
    fn kind(kind: &str) -> Value {
        update(
            "s1",
            json!({"sessionUpdate":kind,"toolCallId":"t1","title":"x"}),
        )
    }
    #[test]
    fn command_carries_acp_profile_and_managed_env() {
        for profile in [None, Some(" work ")] {
            let c = command(Path::new("/bin/omp"), profile, Path::new("/w"));
            assert_eq!(c.get_program(), "/bin/omp");
            assert_eq!(c.get_current_dir(), Some(Path::new("/w")));
            let args: Vec<_> = c.get_args().map(|s| s.to_string_lossy()).collect();
            let mut want = vec!["acp"];
            if profile.is_some() {
                want.extend(["--profile", "work"]);
            }
            assert_eq!(args, want);
            let vars: Vec<_> = c.get_envs().collect();
            assert!(vars.contains(&(
                std::ffi::OsStr::new("GLEN_MANAGED"),
                Some(std::ffi::OsStr::new("1"))
            )));
            for key in ["OMP_PROFILE", "PI_PROFILE"] {
                assert_eq!(
                    vars.contains(&(std::ffi::OsStr::new(key), None)),
                    profile.is_some()
                );
            }
        }
    }
    #[test]
    fn debug_knobs_apply_only_in_range() {
        let env = |vars: &[(&str, &str)]| Env::for_test(None, Path::new("/w"), vars);
        assert_eq!(setup_timeout(&env(&[])), SETUP_TIMEOUT);
        assert_eq!(cancel_grace(&env(&[])), CANCEL_GRACE);
        for (raw, want) in [
            ("300", Some(300)),
            ("100", Some(100)),
            ("60000", Some(60_000)),
            ("99", None),
            ("60001", None),
            ("x", None),
        ] {
            let got = setup_timeout(&env(&[("GLEN_ACP_SETUP_MS", raw)]));
            let want = if cfg!(debug_assertions) {
                want.map_or(SETUP_TIMEOUT, Duration::from_millis)
            } else {
                SETUP_TIMEOUT
            };
            assert_eq!(got, want, "{raw}");
        }
        for (raw, want) in [("300", Some(300)), ("10000", Some(10_000)), ("10001", None)] {
            let got = cancel_grace(&env(&[("GLEN_ACP_CANCEL_GRACE_MS", raw)]));
            let want = if cfg!(debug_assertions) {
                want.map_or(CANCEL_GRACE, Duration::from_millis)
            } else {
                CANCEL_GRACE
            };
            assert_eq!(got, want, "{raw}");
        }
    }
    #[test]
    fn replay_and_bootstrap_before_the_turn_are_counted_not_text() {
        let mut u = Updates::default();
        // No session chosen yet: nothing counts.
        u.apply(&chunk("early", Some("r0")));
        assert_eq!(u.replayed, 0);
        u.for_session("s1");
        u.apply(&update(
            "s1",
            json!({"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"REPLAYED QUESTION"}}),
        ));
        u.apply(&chunk("REPLAYED ANSWER", Some("replay-1")));
        u.apply(&update(
            "s1",
            json!({"sessionUpdate":"available_commands_update","availableCommands":[]}),
        ));
        assert_eq!(u.replayed, 3);
        assert_eq!(u.assistant_text(), None);
        u.begin_turn();
        u.apply(&chunk("now", Some("m1")));
        assert_eq!(u.assistant_text(), Some("now"));
        assert_eq!((u.replayed, u.during_turn), (3, 1));
        assert!(!u.kinds.contains_key("user_message_chunk"));
    }
    #[test]
    fn the_last_non_empty_group_wins() {
        let mut u = Updates::default();
        u.for_session("s1");
        u.begin_turn();
        u.apply(&update(
            "s1",
            json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"thinking"}}),
        ));
        u.apply(&chunk("Hel", Some("m1")));
        u.apply(&chunk("lo ", Some("m1")));
        assert_eq!(u.assistant_text(), Some("Hello "));
        u.apply(&kind("tool_call"));
        u.apply(&kind("tool_call_update"));
        u.apply(&chunk("reply: ", Some("m2")));
        u.apply(&chunk("hi", Some("m2")));
        assert_eq!(u.assistant_text(), Some("reply: hi"));
        // An empty trailing group does not hide the previous one.
        u.apply(&chunk("", Some("m3")));
        assert_eq!(u.assistant_text(), Some("reply: hi"));
        assert_eq!(u.messages(), 2);
        assert_eq!(u.during_turn, 8);
        assert_eq!(u.kinds["agent_message_chunk"], 5);
        assert_eq!(u.kinds["agent_thought_chunk"], 1);
        assert_eq!(u.kinds["tool_call"], 1);
        assert_eq!(u.json()["messages"], 2);
        assert!(!u.text_truncated());
    }
    #[test]
    fn anonymous_chunks_group_until_a_tool_call_or_user_chunk() {
        let mut u = Updates::default();
        u.for_session("s1");
        u.begin_turn();
        u.apply(&chunk("a", None));
        u.apply(&chunk("b", None));
        assert_eq!(u.assistant_text(), Some("ab"));
        u.apply(&kind("tool_call"));
        u.apply(&chunk("c", None));
        assert_eq!(u.assistant_text(), Some("c"));
        u.apply(&update(
            "s1",
            json!({"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"user"}}),
        ));
        u.apply(&chunk("d", None));
        assert_eq!(u.assistant_text(), Some("d"));
        // A messageId change also closes the anonymous group.
        u.apply(&chunk("e", Some("m9")));
        assert_eq!(u.assistant_text(), Some("e"));
        assert_eq!(u.messages(), 4);
    }
    #[test]
    fn non_text_content_other_sessions_and_malformed_updates_are_not_text() {
        let mut u = Updates::default();
        u.for_session("s1");
        u.begin_turn();
        u.apply(&update(
            "s1",
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"image","data":"x","mimeType":"image/png"}}),
        ));
        u.apply(&update(
            "s1",
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":7}}),
        ));
        u.apply(&update(
            "other",
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"x"}}),
        ));
        for bad in [
            json!(null),
            json!([]),
            json!("s1"),
            json!({"sessionId":"s1"}),
            json!({"sessionId":"s1","update":7}),
            json!({"sessionId":"s1","update":{"sessionUpdate":3}}),
            json!({"sessionId":7,"update":{"sessionUpdate":"agent_message_chunk"}}),
        ] {
            u.apply(&bad);
        }
        assert_eq!(u.assistant_text(), None);
        assert_eq!(u.during_turn, 2);
        assert_eq!(u.messages(), 0);
    }
    #[test]
    fn retained_text_is_capped_at_4_mib() {
        let mut u = Updates::default();
        u.for_session("s1");
        u.begin_turn();
        let big = "é".repeat(MAX_TEXT_BYTES / 2 - 1);
        u.apply(&chunk(&big, Some("m1")));
        assert!(!u.text_truncated());
        u.apply(&chunk("éé", Some("m1")));
        let text = u.assistant_text().unwrap();
        assert!(u.text_truncated());
        assert!(text.len() <= MAX_TEXT_BYTES);
        assert_eq!(text.len(), MAX_TEXT_BYTES);
        u.apply(&chunk("more", Some("m1")));
        assert_eq!(u.assistant_text().unwrap().len(), MAX_TEXT_BYTES);
        // A later complete group is reported untruncated.
        u.apply(&chunk("short", Some("m2")));
        assert_eq!(u.assistant_text(), Some("short"));
        assert!(!u.text_truncated());
    }
    fn options(kinds: &[&str]) -> Vec<Value> {
        kinds
            .iter()
            .map(|k| json!({"optionId":k,"name":k,"kind":k}))
            .collect()
    }
    #[test]
    fn answers_pick_only_the_once_kinds() {
        let all = options(&["allow_once", "allow_always", "reject_once", "reject_always"]);
        let allow_only = options(&["allow_once", "allow_always"]);
        let reject_only = options(&["reject_once", "reject_always"]);
        let always_only = options(&["allow_always", "reject_always"]);
        let selected = |id: &str, kind| Answer::Selected {
            option_id: id.into(),
            kind,
        };
        for (policy, opts, want) in [
            (
                AcpPermission::Allow,
                &all,
                selected("allow_once", "allow_once"),
            ),
            (
                AcpPermission::Deny,
                &all,
                selected("reject_once", "reject_once"),
            ),
            (
                AcpPermission::Allow,
                &allow_only,
                selected("allow_once", "allow_once"),
            ),
            (AcpPermission::Deny, &allow_only, Answer::Cancelled),
            (AcpPermission::Allow, &reject_only, Answer::Cancelled),
            (
                AcpPermission::Deny,
                &reject_only,
                selected("reject_once", "reject_once"),
            ),
            (AcpPermission::Allow, &always_only, Answer::Cancelled),
            (AcpPermission::Deny, &always_only, Answer::Cancelled),
            (AcpPermission::Allow, &vec![], Answer::Cancelled),
        ] {
            assert_eq!(answer(policy, opts), want, "{policy:?} {opts:?}");
        }
        // Option ids need not equal kinds; unknown kinds, a missing kind or id are skipped.
        let odd = vec![
            json!({"optionId":"x1","name":"?","kind":"maybe"}),
            json!({"optionId":"x2","name":"?"}),
            json!({"name":"?","kind":"allow_once"}),
            json!({"optionId":5,"kind":"allow_once"}),
            json!({"optionId":"ok-1","name":"Allow","kind":"allow_once"}),
        ];
        assert_eq!(
            answer(AcpPermission::Allow, &odd),
            selected("ok-1", "allow_once")
        );
        assert_eq!(answer(AcpPermission::Deny, &odd), Answer::Cancelled);
        assert_eq!(
            selected("a", "allow_once").result(),
            json!({"outcome":{"outcome":"selected","optionId":"a"}})
        );
        assert_eq!(
            Answer::Cancelled.result(),
            json!({"outcome":{"outcome":"cancelled"}})
        );
    }
    #[test]
    fn input_required_events_carry_the_tool_and_the_answer() {
        let params = json!({"sessionId":"s1","toolCall":{"toolCallId":"t1","title":"rm x","kind":"execute","status":"pending"},"options":options(&["allow_once"])});
        let a = answer(AcpPermission::Allow, params["options"].as_array().unwrap());
        assert_eq!(
            input_required_event(&params, AcpPermission::Allow, &a),
            json!({"event":"task_state","task_state":"input_required","tool_call_id":"t1","title":"rm x","kind":"execute","policy":"allow","answer":"allow_once","option_id":"allow_once"})
        );
        assert_eq!(
            input_required_event(&json!({}), AcpPermission::Deny, &Answer::Cancelled),
            json!({"event":"task_state","task_state":"input_required","tool_call_id":null,"title":null,"kind":null,"policy":"deny","answer":"cancelled","option_id":null})
        );
    }
    #[test]
    fn stop_reasons_map_to_task_states() {
        for (stop, state) in [
            ("end_turn", "completed"),
            ("cancelled", "canceled"),
            ("refusal", "rejected"),
            ("max_tokens", "failed"),
            ("max_turn_requests", "failed"),
            ("bogus", "failed"),
        ] {
            assert_eq!(task_state(stop), state, "{stop}");
        }
    }
    #[test]
    fn the_failure_table() {
        use ErrorCode::{JobFailed, OmpUnavailable};
        use Reason::*;
        for (stage, reason, code) in [
            (Stage::Handshake, AcpSpawn, OmpUnavailable),
            (Stage::Handshake, AcpExited, OmpUnavailable),
            (Stage::Handshake, AcpTimeout, OmpUnavailable),
            (Stage::Handshake, AcpProtocol, OmpUnavailable),
            (Stage::Handshake, AcpMalformed, OmpUnavailable),
            (Stage::Handshake, AcpCapability, OmpUnavailable),
            (Stage::Session, AcpRpcError, JobFailed),
            (Stage::Session, AcpExited, JobFailed),
            (Stage::Session, AcpMalformed, JobFailed),
            (Stage::Session, AcpTimeout, JobFailed),
            (Stage::Session, AcpStopReason, JobFailed),
            // The fixed rows do not move with the stage.
            (Stage::Session, AcpSpawn, OmpUnavailable),
            (Stage::Session, AcpProtocol, OmpUnavailable),
            (Stage::Session, AcpCapability, OmpUnavailable),
            (Stage::Handshake, AcpRpcError, JobFailed),
            (Stage::Handshake, AcpStopReason, JobFailed),
        ] {
            let e = error(stage, reason, "m");
            assert_eq!(e.code, code, "{stage:?} {reason:?}");
            assert_eq!(e.details.as_ref().unwrap()["reason"], reason.as_str());
            assert_eq!(e.message, "m");
            assert_eq!(e.hint.is_some(), reason == AcpSpawn, "{reason:?}");
        }
        assert_eq!(error(Stage::Handshake, AcpExited, "m").code.exit_code(), 5);
        assert_eq!(error(Stage::Session, AcpExited, "m").code.exit_code(), 7);
        let names: Vec<_> = Reason::ALL.iter().map(|r| r.as_str()).collect();
        assert_eq!(
            names,
            [
                "acp_spawn",
                "acp_exited",
                "acp_timeout",
                "acp_protocol",
                "acp_malformed",
                "acp_capability",
                "acp_rpc_error",
                "acp_stop_reason"
            ]
        );
    }
    #[test]
    fn rpc_error_text_prefers_data_details() {
        assert_eq!(
            rpc_error_message(
                &json!({"code":-32603,"message":"Internal error","data":{"details":"ACP session not found: x"}})
            ),
            "ACP session not found: x"
        );
        assert_eq!(
            rpc_error_message(&json!({"code":-32601,"message":"Method not found"})),
            "Method not found"
        );
        assert_eq!(
            rpc_error_message(&json!({"code":-32603,"data":{"details":5}})),
            "unknown JSON-RPC error"
        );
        assert_eq!(rpc_error_message(&json!(null)), "unknown JSON-RPC error");
    }
}
