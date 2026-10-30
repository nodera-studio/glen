//! Transport-neutral operation vocabulary. RPC controls additionally reserve
//! steer {text} and follow_up {text}; neither starts a second omp process.
use crate::jobs::Deliver;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub const PROTOCOL: u32 = 1;
pub const PROTOCOLS: &[u32] = &[1];
pub const IMPLEMENTED: &[&str] = &["hello", "state", "send"];
/// W3b: abort {}, list_models {}, set_model {provider, model_id},
/// set_thinking_level {level}, compact {instructions?}.
pub const RESERVED: &[&str] = &[
    "abort",
    "list_models",
    "set_model",
    "set_thinking_level",
    "compact",
];
pub const UNSUPPORTED_VERSION: &str = "unsupported_version";
pub const UNSUPPORTED_OP: &str = "unsupported_op";
pub const BAD_REQUEST: &str = "bad_request";
pub const SESSION_MISMATCH: &str = "session_mismatch";
pub const SEND_FAILED: &str = "send_failed";
pub const SESSION_SWITCHED: &str = "session_switched";
pub const SHUTDOWN: &str = "shutdown";
pub const INTERNAL: &str = "internal";
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
}
impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::State => "state",
            Self::Send(_) => "send",
            Self::Abort => "abort",
        }
    }
    pub fn request(&self, id: &str) -> Value {
        let args = match self {
            Self::Hello { client_version } => {
                json!({"client":"glen","client_version":client_version,"protocols":PROTOCOLS})
            }
            Self::Send(a) => json!(a),
            _ => json!({}),
        };
        json!({"v":PROTOCOL,"id":id,"op":self.name(),"args":args})
    }
}
#[derive(Debug, Deserialize, Serialize)]
pub struct OpError {
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
}
pub fn decode(name: &str, args: Option<&Value>) -> Result<Op, OpError> {
    let bad = || OpError {
        code: BAD_REQUEST.into(),
        message: format!("invalid {name} arguments"),
        details: None,
    };
    match name {
        "abort" => Ok(Op::Abort),
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
