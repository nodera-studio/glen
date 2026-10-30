//! Transport-neutral operation vocabulary shared by live sockets and RPC controls.
use crate::jobs::Deliver;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub const PROTOCOL: u32 = 1;
pub const PROTOCOLS: &[u32] = &[1];
pub const IMPLEMENTED: &[&str] = &[
    "hello",
    "state",
    "send",
    "abort",
    "list_models",
    "set_model",
    "set_thinking_level",
    "compact",
];
pub const CONTROL_OPS: &[&str] = &[
    "abort",
    "list_models",
    "set_model",
    "set_thinking_level",
    "compact",
];
pub const RESERVED: &[&str] = &[];
pub const THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];
pub const MAX_INSTRUCTIONS: usize = 524_288;
pub const UNSUPPORTED_VERSION: &str = "unsupported_version";
pub const UNSUPPORTED_OP: &str = "unsupported_op";
pub const BAD_REQUEST: &str = "bad_request";
pub const SESSION_MISMATCH: &str = "session_mismatch";
pub const SEND_FAILED: &str = "send_failed";
pub const SESSION_SWITCHED: &str = "session_switched";
pub const SHUTDOWN: &str = "shutdown";
pub const INTERNAL: &str = "internal";
pub const MODEL_NOT_FOUND: &str = "model_not_found";
pub const NO_AUTH: &str = "no_auth";
pub const INVALID_LEVEL: &str = "invalid_level";
pub const BUSY: &str = "busy";
pub const OMP_ERROR: &str = "omp_error";
pub const TIMEOUT: &str = "timeout";
pub const JOB_ENDED: &str = "job_ended";
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SetModelArgs {
    pub provider: String,
    pub model_id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ThinkingArgs {
    pub level: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CompactArgs {
    pub instructions: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ModelRef {
    pub provider: String,
    pub id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ModelRow {
    pub provider: String,
    pub id: String,
    pub name: String,
    pub reasoning: bool,
    pub thinking_levels: Vec<String>,
    pub context_window: Option<i64>,
    pub max_tokens: Option<i64>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelsData {
    pub session_id: String,
    pub current: Option<ModelRef>,
    pub thinking_level: Option<String>,
    pub models: Vec<ModelRow>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelSetData {
    pub session_id: String,
    pub model: ModelName,
    pub previous: Option<ModelRef>,
    pub thinking_level: Option<String>,
    pub thinking_levels: Vec<String>,
    pub busy: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelName {
    pub provider: String,
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ThinkingSetData {
    pub session_id: String,
    pub thinking_level: Option<String>,
    pub previous: Option<String>,
    pub model: Option<ModelRef>,
    pub available: Vec<String>,
    pub busy: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CompactData {
    pub session_id: String,
    pub tokens_before: Option<i64>,
    pub tokens_after: Option<i64>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AbortData {
    pub session_id: String,
    pub was: String,
    pub aborted: bool,
    pub idle_after: bool,
    pub queued: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SendArgs {
    pub text: String,
    pub deliver_as: Deliver,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub expect_session_id: Option<String>,
}
#[derive(Clone, Debug)]
pub enum Op {
    Hello { client_version: String },
    State,
    Send(SendArgs),
    Abort,
    ListModels,
    SetModel(SetModelArgs),
    SetThinkingLevel(ThinkingArgs),
    Compact(CompactArgs),
}
impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::State => "state",
            Self::Send(_) => "send",
            Self::Abort => "abort",
            Self::ListModels => "list_models",
            Self::SetModel(_) => "set_model",
            Self::SetThinkingLevel(_) => "set_thinking_level",
            Self::Compact(_) => "compact",
        }
    }
    pub fn request_for(&self, id: &str, expect_session_id: Option<&str>) -> Value {
        let mut request = self.request(id);
        if let Some(expected) = expect_session_id {
            request["args"]["expect_session_id"] = json!(expected);
        }
        request
    }
    pub fn request(&self, id: &str) -> Value {
        let args = match self {
            Self::Hello { client_version } => {
                json!({"client":"glen","client_version":client_version,"protocols":PROTOCOLS})
            }
            Self::Send(a) => json!(a),
            Self::SetModel(a) => json!(a),
            Self::SetThinkingLevel(a) => json!(a),
            Self::Compact(a) => json!(a),
            _ => json!({}),
        };
        json!({"v":PROTOCOL,"id":id,"op":self.name(),"args":args})
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OpError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}
impl OpError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}
pub fn expect_session_id(args: Option<&Value>) -> Option<String> {
    args.and_then(|a| a["expect_session_id"].as_str())
        .map(str::to_owned)
}
pub fn decode(name: &str, args: Option<&Value>) -> Result<Op, OpError> {
    let bad = || OpError {
        code: BAD_REQUEST.into(),
        message: format!("invalid {name} arguments"),
        details: None,
    };
    if args.is_some_and(|a| {
        !a.is_object()
            || a.get("expect_session_id")
                .is_some_and(|v| !v.is_null() && !v.is_string())
    }) {
        return Err(bad());
    }
    match name {
        "abort" => Ok(Op::Abort),
        "list_models" => Ok(Op::ListModels),
        "set_model" => {
            let a: SetModelArgs =
                serde_json::from_value(args.cloned().ok_or_else(bad)?).map_err(|_| bad())?;
            if a.provider.trim().is_empty() || a.model_id.trim().is_empty() {
                return Err(bad());
            }
            Ok(Op::SetModel(a))
        }
        "set_thinking_level" => {
            let a: ThinkingArgs =
                serde_json::from_value(args.cloned().ok_or_else(bad)?).map_err(|_| bad())?;
            if !THINKING_LEVELS.contains(&a.level.as_str()) {
                return Err(bad());
            }
            Ok(Op::SetThinkingLevel(a))
        }
        "compact" => {
            let a: CompactArgs = serde_json::from_value(args.cloned().unwrap_or_else(|| json!({})))
                .map_err(|_| bad())?;
            if a.instructions
                .as_ref()
                .is_some_and(|s| s.len() > MAX_INSTRUCTIONS)
            {
                return Err(bad());
            }
            Ok(Op::Compact(a))
        }
        "state" => Ok(Op::State),
        "hello" => Ok(Op::Hello {
            client_version: args
                .and_then(|v| v["client_version"].as_str())
                .unwrap_or_default()
                .into(),
        }),
        "send" => serde_json::from_value(args.cloned().ok_or_else(bad)?)
            .map(Op::Send)
            .map_err(|_| bad()),
        _ => Err(OpError {
            code: UNSUPPORTED_OP.into(),
            message: format!("unsupported op: {name}"),
            details: Some(json!({"op":name,"ops":IMPLEMENTED,"reserved":RESERVED.contains(&name)})),
        }),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vocabulary() {
        for op in [
            Op::Hello {
                client_version: "test".into(),
            },
            Op::State,
            Op::Send(SendArgs {
                text: "hi".into(),
                deliver_as: Deliver::Auto,
                job_id: None,
                expect_session_id: None,
            }),
        ] {
            let r = op.request("id");
            assert_eq!(decode(op.name(), r.get("args")).unwrap().name(), op.name());
        }
        for name in RESERVED
            .iter()
            .copied()
            .filter(|s| *s != "abort")
            .chain(["unknown"])
        {
            assert_eq!(decode(name, None).unwrap_err().code, UNSUPPORTED_OP);
        }
        assert_eq!(
            decode("send", Some(&json!({}))).unwrap_err().code,
            BAD_REQUEST
        );
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;
    #[test]
    fn all_controls_roundtrip_with_identity() {
        for op in [
            Op::Abort,
            Op::ListModels,
            Op::SetModel(SetModelArgs {
                provider: "p".into(),
                model_id: "m".into(),
            }),
            Op::SetThinkingLevel(ThinkingArgs {
                level: "high".into(),
            }),
            Op::Compact(CompactArgs {
                instructions: Some("keep facts".into()),
            }),
        ] {
            let request = op.request_for("r", Some("s"));
            assert_eq!(request["id"], "r");
            assert_eq!(expect_session_id(request.get("args")).as_deref(), Some("s"));
            assert_eq!(
                decode(op.name(), request.get("args")).unwrap().name(),
                op.name()
            );
            assert!(CONTROL_OPS.contains(&op.name()));
            assert!(IMPLEMENTED.contains(&op.name()));
        }
        assert!(RESERVED.is_empty());
    }
    #[test]
    fn malformed_control_args_are_rejected() {
        for (op, args) in [
            ("set_model", json!({})),
            ("set_model", json!({"provider":" ","model_id":"m"})),
            ("set_model", json!({"provider":"p","model_id":""})),
            ("set_model", json!({"provider":2,"model_id":"m"})),
            ("set_thinking_level", json!({"level":"auto"})),
            ("set_thinking_level", json!({"level":"inherit"})),
            ("compact", json!({"instructions":2})),
            (
                "compact",
                json!({"instructions":"é".repeat(MAX_INSTRUCTIONS/2+1)}),
            ),
            ("abort", json!({"expect_session_id":2})),
            ("list_models", json!([])),
        ] {
            assert_eq!(
                decode(op, Some(&args)).unwrap_err().code,
                BAD_REQUEST,
                "{op}"
            );
        }
        assert!(
            decode(
                "compact",
                Some(&json!({"instructions":"a".repeat(MAX_INSTRUCTIONS)}))
            )
            .is_ok()
        );
        assert!(decode("set_model", None).is_err());
        for level in THINKING_LEVELS {
            assert!(decode("set_thinking_level", Some(&json!({"level":level}))).is_ok());
        }
    }
    #[test]
    fn nullable_model_fields_are_always_present() {
        let data = ModelsData {
            session_id: "s".into(),
            current: None,
            thinking_level: None,
            models: vec![],
        };
        assert_eq!(
            serde_json::to_value(data).unwrap(),
            json!({"session_id":"s","current":null,"thinking_level":null,"models":[]})
        );
        let data = CompactData {
            session_id: "s".into(),
            tokens_before: None,
            tokens_after: None,
        };
        assert_eq!(
            serde_json::to_value(data).unwrap(),
            json!({"session_id":"s","tokens_before":null,"tokens_after":null})
        );
    }
}
