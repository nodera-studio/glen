//! Execute one control on an already-owned omp client. Callers bound the whole
//! sequence and tear down the transport on interruption or deadline expiry.
use crate::{
    ops::{
        self, CompactData, ModelName, ModelRef, ModelSetData, ModelsData, Op, OpError,
        ThinkingSetData,
    },
    rpc::polling_command,
};
use omp_rpc::{
    Client, CompactCommand, GetAvailableModelsCommand, GetAvailableThinkingLevelsCommand,
    GetStateCommand, SessionState, SetModelCommand, SetThinkingLevelCommand,
};
use serde::Serialize;
use serde_json::{Value, json};
polling_command!(ListModels30, GetAvailableModelsCommand, 30_000);
polling_command!(SetModel30, SetModelCommand, 30_000);
polling_command!(Levels10, GetAvailableThinkingLevelsCommand, 10_000);
polling_command!(Thinking10, SetThinkingLevelCommand, 10_000);
polling_command!(State10, GetStateCommand, 10_000);
polling_command!(CompactLong, CompactCommand, 86_400_000);
pub(crate) fn rpc_error(error: omp_rpc::Error) -> OpError {
    match error {
        omp_rpc::Error::Command { error, .. } => {
            let code = if error.starts_with("Model not found") {
                ops::MODEL_NOT_FOUND
            } else if error.starts_with("No API key") {
                ops::NO_AUTH
            } else {
                ops::OMP_ERROR
            };
            OpError::new(code, error)
        }
        omp_rpc::Error::Timeout { command } => {
            OpError::new(ops::TIMEOUT, format!("omp {command} timed out"))
        }
        omp_rpc::Error::Closed | omp_rpc::Error::Io(_) => {
            OpError::new(ops::OMP_ERROR, "omp exited")
        }
        error => OpError::new(ops::INTERNAL, error.to_string()),
    }
}
fn value(data: impl Serialize) -> Result<Value, OpError> {
    serde_json::to_value(data)
        .map_err(|e| OpError::new(ops::INTERNAL, format!("cannot encode control result: {e}")))
}
fn model_ref(state: &SessionState) -> Option<ModelRef> {
    state.model.as_ref().map(|m| ModelRef {
        provider: m.provider.clone(),
        id: m.id.clone(),
    })
}
fn thinking(state: &SessionState) -> Option<String> {
    state.thinking_level.map(|l| l.as_str().to_owned())
}
fn busy(state: &SessionState) -> bool {
    state.is_streaming || state.queued_message_count > 0
}
pub(crate) fn state(client: &Client) -> Result<SessionState, OpError> {
    client.call(&State10(GetStateCommand {})).map_err(rpc_error)
}
fn check(state: &SessionState, expect: Option<&str>) -> Result<(), OpError> {
    if expect.is_some_and(|id| id != state.session_id) {
        return Err(
            OpError::new(ops::SESSION_MISMATCH, "omp resumed another session")
                .with_details(json!({"session_id":state.session_id})),
        );
    }
    Ok(())
}
pub fn exec(client: &Client, op: &Op, expect: Option<&str>) -> Result<Value, OpError> {
    exec_until(client, op, expect, None)
}
pub(crate) fn exec_until(
    client: &Client,
    op: &Op,
    expect: Option<&str>,
    deadline: Option<std::time::Instant>,
) -> Result<Value, OpError> {
    let remaining = || {
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            Err(OpError::new(ops::TIMEOUT, "the control deadline passed"))
        } else {
            Ok(())
        }
    };
    remaining()?;
    // Check before every mutating call; the final state supplies the post-op id
    // so the CLI can report an omp fork instead of silently accepting it.
    let before = state(client)?;
    check(&before, expect)?;
    match op {
        Op::ListModels => {
            remaining()?;
            let models = client
                .call(&ListModels30(GetAvailableModelsCommand {}))
                .map_err(rpc_error)?;
            remaining()?;
            let after = state(client)?;
            value(ModelsData {
                session_id: after.session_id.clone(),
                current: model_ref(&after),
                thinking_level: thinking(&after),
                models: models.iter().map(super::row_from_info).collect(),
            })
        }
        Op::SetModel(args) => {
            remaining()?;
            let model = client
                .call(&SetModel30(SetModelCommand {
                    provider: args.provider.clone(),
                    model_id: args.model_id.clone(),
                }))
                .map_err(rpc_error)?;
            remaining()?;
            let after = state(client)?;
            let row = super::row_from_info(&model);
            value(ModelSetData {
                session_id: after.session_id.clone(),
                model: ModelName {
                    provider: model.provider,
                    id: model.id,
                    name: model.name,
                },
                previous: model_ref(&before),
                thinking_level: thinking(&after),
                thinking_levels: row.thinking_levels,
                busy: busy(&after),
            })
        }
        Op::SetThinkingLevel(args) => {
            remaining()?;
            let available: Vec<_> = client
                .call(&Levels10(GetAvailableThinkingLevelsCommand {}))
                .map_err(rpc_error)?
                .iter()
                .map(|l| l.as_str().to_owned())
                .collect();
            if !ops::THINKING_LEVELS.contains(&args.level.as_str())
                || !available.contains(&args.level)
            {
                return Err(OpError::new(
                    ops::INVALID_LEVEL,
                    format!(
                        "thinking level {} is unavailable for the current model",
                        args.level
                    ),
                )
                .with_details(json!({"available":available,"model":model_ref(&before)})));
            }
            let level = serde_json::from_value(json!(args.level)).map_err(|e| {
                OpError::new(ops::BAD_REQUEST, format!("invalid thinking level: {e}"))
            })?;
            remaining()?;
            client
                .call(&Thinking10(SetThinkingLevelCommand { level }))
                .map_err(rpc_error)?;
            remaining()?;
            let after = state(client)?;
            value(ThinkingSetData {
                session_id: after.session_id.clone(),
                thinking_level: thinking(&after),
                previous: thinking(&before),
                model: model_ref(&after),
                available,
                busy: busy(&after),
            })
        }
        Op::Compact(args) => {
            if busy(&before) || before.is_compacting {
                return Err(OpError::new(
                    ops::BUSY,
                    "the session is streaming, compacting or has queued messages",
                ));
            }
            remaining()?;
            let result = client
                .call(&CompactLong(CompactCommand {
                    custom_instructions: args.instructions.clone(),
                }))
                .map_err(rpc_error)?;
            remaining()?;
            let after = state(client)?;
            value(CompactData {
                session_id: after.session_id,
                tokens_before: Some(result.tokens_before),
                tokens_after: after.context_usage.map(|u| u.tokens),
            })
        }
        _ => Err(OpError::new(
            ops::UNSUPPORTED_OP,
            format!("{} is not an RPC control executor op", op.name()),
        )),
    }
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixStream,
        time::Duration,
    };
    fn peer(
        op: Op,
        replies: Vec<(&'static str, Value)>,
        expect: Option<&str>,
    ) -> Result<Value, OpError> {
        let (client_io, mut server) = UnixStream::pair().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let thread = std::thread::spawn(move || {
            // Protocol 1 skips negotiation and needs no registration commands
            // when the caller has no tools or URI schemes.
            writeln!(server, "{}", json!({"type":"ready","protocolVersion":1})).unwrap();
            let mut reader = BufReader::new(server.try_clone().unwrap());
            for (command, data) in replies {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["type"], command);
                let result = if let Some(error) = data.get("error") {
                    json!({"type":"response","id":request["id"],"command":command,"success":false,"error":error})
                } else {
                    json!({"type":"response","id":request["id"],"command":command,"success":true,"data":data})
                };
                writeln!(server, "{result}").unwrap();
            }
            server.shutdown(std::net::Shutdown::Both).unwrap();
        });
        let (client, _events) = Client::from_io(
            client_io.try_clone().unwrap(),
            client_io,
            omp_rpc::ClientOptions {
                tools: vec![],
                uris: vec![],
                ready_timeout: Duration::from_secs(2),
                default_timeout: Duration::from_secs(1),
            },
        )
        .unwrap();
        let result = exec(&client, &op, expect);
        client.close().unwrap();
        thread.join().unwrap();
        result
    }
    fn s(extra: Value) -> Value {
        let mut state = json!({"sessionId":"s","thinkingLevel":"medium"});
        state
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        state
    }
    #[test]
    fn mismatch_sends_no_mutation() {
        let e = peer(
            Op::SetThinkingLevel(ops::ThinkingArgs {
                level: "high".into(),
            }),
            vec![("get_state", s(json!({})))],
            Some("other"),
        )
        .unwrap_err();
        assert_eq!(e.code, ops::SESSION_MISMATCH);
        assert_eq!(e.details.unwrap()["session_id"], "s");
    }
    #[test]
    fn invalid_level_never_sends_set() {
        let e = peer(
            Op::SetThinkingLevel(ops::ThinkingArgs {
                level: "xhigh".into(),
            }),
            vec![
                ("get_state", s(json!({}))),
                (
                    "get_available_thinking_levels",
                    json!({"levels":["off","high"]}),
                ),
            ],
            Some("s"),
        )
        .unwrap_err();
        assert_eq!(e.code, ops::INVALID_LEVEL);
        assert_eq!(e.details.unwrap()["available"], json!(["off", "high"]));
    }
    #[test]
    fn compact_busy_never_compacts() {
        for extra in [
            json!({"isStreaming":true}),
            json!({"isCompacting":true}),
            json!({"queuedMessageCount":1}),
        ] {
            assert_eq!(
                peer(
                    Op::Compact(ops::CompactArgs::default()),
                    vec![("get_state", s(extra))],
                    Some("s")
                )
                .unwrap_err()
                .code,
                ops::BUSY
            );
        }
    }
    #[test]
    fn no_auth_and_other_rpc_errors() {
        let e = peer(
            Op::SetModel(ops::SetModelArgs {
                provider: "nokey".into(),
                model_id: "locked-1".into(),
            }),
            vec![
                ("get_state", s(json!({}))),
                (
                    "set_model",
                    json!({"error":"No API key for nokey/locked-1"}),
                ),
            ],
            Some("s"),
        )
        .unwrap_err();
        assert_eq!(e.code, ops::NO_AUTH);
        for (message, code) in [
            ("Model not found: p/m", ops::MODEL_NOT_FOUND),
            ("Nothing to compact", ops::OMP_ERROR),
        ] {
            assert_eq!(
                rpc_error(omp_rpc::Error::Command {
                    command: "control".into(),
                    error: message.into(),
                    code: None
                })
                .code,
                code
            );
        }
        assert_eq!(rpc_error(omp_rpc::Error::Closed).code, ops::OMP_ERROR);
        assert_eq!(
            rpc_error(omp_rpc::Error::Timeout {
                command: "c".into()
            })
            .code,
            ops::TIMEOUT
        );
        assert_eq!(
            rpc_error(omp_rpc::Error::Protocol("bad".into())).code,
            ops::INTERNAL
        );
    }
    #[test]
    fn successful_thinking_and_compaction_return_post_op_identity() {
        let data = peer(
            Op::SetThinkingLevel(ops::ThinkingArgs {
                level: "high".into(),
            }),
            vec![
                ("get_state", s(json!({}))),
                (
                    "get_available_thinking_levels",
                    json!({"levels":["off","high"]}),
                ),
                ("set_thinking_level", json!({})),
                (
                    "get_state",
                    s(json!({"sessionId":"new","thinkingLevel":"high"})),
                ),
            ],
            Some("s"),
        )
        .unwrap();
        assert_eq!(data["session_id"], "new");
        assert_eq!(data["previous"], "medium");
        assert_eq!(data["thinking_level"], "high");
        let data = peer(
            Op::Compact(ops::CompactArgs {
                instructions: Some("keep facts".into()),
            }),
            vec![
                ("get_state", s(json!({}))),
                (
                    "compact",
                    json!({"summary":"short","firstKeptEntryId":"a","tokensBefore":1234}),
                ),
                (
                    "get_state",
                    s(json!({"contextUsage":{"tokens":123,"contextWindow":10000,"percent":1.23}})),
                ),
            ],
            Some("s"),
        )
        .unwrap();
        assert_eq!(data["tokens_before"], 1234);
        assert_eq!(data["tokens_after"], 123);
    }
    #[test]
    fn listing_and_model_switch_normalize_complete_model_descriptors() {
        let models: Vec<Value> =
            serde_json::from_str(include_str!("../../tests/support/fake_models.json")).unwrap();
        let before = s(json!({"model": models[0]}));
        let data = peer(
            Op::ListModels,
            vec![
                ("get_state", before.clone()),
                ("get_available_models", json!({"models":models})),
                ("get_state", before.clone()),
            ],
            Some("s"),
        )
        .unwrap();
        assert_eq!(data["current"], json!({"provider":"fake","id":"fake-1"}));
        assert_eq!(data["models"].as_array().unwrap().len(), 6);
        assert_eq!(data["models"][1]["thinking_levels"], json!(["off"]));
        assert_eq!(data["thinking_level"], "medium");
        let data = peer(
            Op::SetModel(ops::SetModelArgs { provider:"fake".into(), model_id:"fake-2".into() }),
            vec![
                ("get_state", before),
                ("set_model", models[1].clone()),
                ("get_state", s(json!({"sessionId":"forked","model":models[1],"thinkingLevel":"off","isStreaming":true}))),
            ],
            Some("s"),
        ).unwrap();
        assert_eq!(data["session_id"], "forked");
        assert_eq!(data["previous"], json!({"provider":"fake","id":"fake-1"}));
        assert_eq!(
            data["model"],
            json!({"provider":"fake","id":"fake-2","name":"fake fake-2"})
        );
        assert_eq!(data["thinking_levels"], json!(["off"]));
        assert_eq!(data["busy"], true);
    }
}
