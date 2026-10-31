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
use std::sync::atomic::{AtomicBool, Ordering};
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
                .with_details(json!({"transport_closed":true}))
        }
        error => OpError::new(ops::INTERNAL, error.to_string()),
    }
}
pub(crate) fn job_error(error: OpError, terminal: bool) -> OpError {
    if terminal
        && error.code == ops::OMP_ERROR
        && error
            .details
            .as_ref()
            .is_some_and(|d| d["transport_closed"] == true)
    {
        OpError::new(ops::JOB_ENDED, "the job ended before answering the control")
    } else {
        error
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
    exec_until(client, op, expect, None, &AtomicBool::new(false), None)
}
pub(crate) fn exec_until(
    client: &Client,
    op: &Op,
    expect: Option<&str>,
    deadline: Option<std::time::Instant>,
    stop: &AtomicBool,
    applied: Option<&AtomicBool>,
) -> Result<Value, OpError> {
    let remaining = || {
        if let Some(deadline) = deadline {
            super::ephemeral::stopped(deadline, stop)
        } else if stop.load(Ordering::SeqCst) {
            Err(OpError::new(
                "interrupted",
                "the glen command was interrupted",
            ))
        } else {
            Ok(())
        }
    };
    let mut submitted = false;
    let result = (|| {
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
                submitted = true;
                let model = client
                    .call(&SetModel30(SetModelCommand {
                        provider: args.provider.clone(),
                        model_id: args.model_id.clone(),
                    }))
                    .map_err(rpc_error)?;
                if let Some(applied) = applied {
                    applied.store(true, Ordering::SeqCst);
                }
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
                submitted = true;
                client
                    .call(&Thinking10(SetThinkingLevelCommand { level }))
                    .map_err(rpc_error)?;
                if let Some(applied) = applied {
                    applied.store(true, Ordering::SeqCst);
                }
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
                submitted = true;
                let result = client
                    .call(&CompactLong(CompactCommand {
                        custom_instructions: args.instructions.clone(),
                    }))
                    .map_err(rpc_error)?;
                if let Some(applied) = applied {
                    applied.store(true, Ordering::SeqCst);
                }
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
    })();
    // A success past the deadline stays a success; the CLI adds the envelope warning for it
    // (W8-DOC-8), so the data carries no warnings of its own.
    result.map_err(|error| {
        if submitted
            && error.code == ops::OMP_ERROR
            && error
                .details
                .as_ref()
                .is_some_and(|d| d["transport_closed"] == true)
        {
            OpError::new(
                ops::OUTCOME_UNKNOWN,
                "omp disconnected after the change was submitted",
            )
            .with_details(json!({"reason":ops::OUTCOME_UNKNOWN}))
        } else {
            error
        }
    })
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

#[cfg(all(test, unix))]
mod fix_cycle_tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixStream,
        time::{Duration, Instant},
    };

    #[test]
    fn closed_and_io_only_retry_after_job_is_terminal() {
        for terminal in [false, true] {
            for error in [
                omp_rpc::Error::Closed,
                omp_rpc::Error::Io(std::io::Error::other("closed peer")),
            ] {
                assert_eq!(
                    job_error(rpc_error(error), terminal).code,
                    if terminal {
                        ops::JOB_ENDED
                    } else {
                        ops::OMP_ERROR
                    }
                );
            }
            let command = rpc_error(omp_rpc::Error::Command {
                command: "set_model".into(),
                error: "omp exited".into(),
                code: None,
            });
            assert_eq!(job_error(command, terminal).code, ops::OMP_ERROR);
            assert_eq!(
                job_error(OpError::new(ops::TIMEOUT, "timeout"), terminal).code,
                ops::TIMEOUT
            );
        }
    }

    #[test]
    fn every_mutation_rechecks_deadline_after_preflight() {
        for (op, block_levels) in [
            (
                Op::SetModel(ops::SetModelArgs {
                    provider: "fake".into(),
                    model_id: "fake-2".into(),
                }),
                false,
            ),
            (
                Op::SetThinkingLevel(ops::ThinkingArgs {
                    level: "high".into(),
                }),
                false,
            ),
            (
                Op::SetThinkingLevel(ops::ThinkingArgs {
                    level: "high".into(),
                }),
                true,
            ),
            (Op::Compact(ops::CompactArgs::default()), false),
        ] {
            let (client_io, mut server) = UnixStream::pair().unwrap();
            let (finished_tx, finished_rx) = std::sync::mpsc::channel();
            server
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let thread = std::thread::spawn(move || {
                writeln!(server, "{}", json!({"type":"ready","protocolVersion":1})).unwrap();
                let mut reader = BufReader::new(server.try_clone().unwrap());
                for command in if block_levels {
                    vec!["get_state", "get_available_thinking_levels"]
                } else {
                    vec!["get_state"]
                } {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let req: Value = serde_json::from_str(&line).unwrap();
                    assert_eq!(req["type"], command);
                    if command != "get_state" || !block_levels {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    let data = if command == "get_state" {
                        json!({"sessionId":"s","thinkingLevel":"medium"})
                    } else {
                        json!({"levels":["off","high"]})
                    };
                    writeln!(server, "{}", json!({"type":"response","id":req["id"],"command":command,"success":true,"data":data})).unwrap();
                }
                finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                server
                    .set_read_timeout(Some(Duration::from_millis(50)))
                    .unwrap();
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => (),
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    result => panic!("unexpected RPC: {line} ({result:?})"),
                }
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
            let error = exec_until(
                &client,
                &op,
                Some("s"),
                Some(Instant::now() + Duration::from_millis(100)),
                &AtomicBool::new(false),
                None,
            )
            .unwrap_err();
            assert_eq!(error.code, ops::TIMEOUT);
            finished_tx.send(()).unwrap();
            client.close().unwrap();
            thread.join().unwrap();
        }
    }
}

#[cfg(all(test, unix))]
mod c2_tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixStream,
        sync::Arc,
        time::{Duration, Instant},
    };

    // None closes the peer at the requested command; the optional hook runs before
    // its response, so cancellation and deadline races have a deterministic order.
    fn peer(
        op: &Op,
        replies: Vec<(&str, Option<Value>)>,
        hook: Option<(usize, bool)>,
    ) -> Result<Value, OpError> {
        let replies: Vec<_> = replies
            .into_iter()
            .map(|(c, v)| (c.to_owned(), v))
            .collect();
        let stop = Arc::new(AtomicBool::new(false));
        let peer_stop = Arc::clone(&stop);
        let (io, mut server) = UnixStream::pair().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let thread = std::thread::spawn(move || {
            writeln!(server, "{}", json!({"type":"ready","protocolVersion":1})).unwrap();
            let mut reader = BufReader::new(server.try_clone().unwrap());
            for (index, (command, data)) in replies.into_iter().enumerate() {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let req: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(req["type"], command);
                if let Some((at, cancel)) = hook
                    && at == index
                {
                    if cancel {
                        peer_stop.store(true, Ordering::SeqCst);
                    } else {
                        std::thread::sleep(Duration::from_millis(300));
                    }
                }
                let Some(data) = data else {
                    break;
                };
                if let Some(error) = data.get("error") {
                    writeln!(server, "{}", json!({"type":"response","id":req["id"],"command":command,"success":false,"error":error})).unwrap();
                } else {
                    writeln!(server, "{}", json!({"type":"response","id":req["id"],"command":command,"success":true,"data":data})).unwrap();
                }
            }
            server.shutdown(std::net::Shutdown::Both).unwrap();
        });
        let (client, _events) = Client::from_io(
            io.try_clone().unwrap(),
            io,
            omp_rpc::ClientOptions {
                tools: vec![],
                uris: vec![],
                ready_timeout: Duration::from_secs(2),
                default_timeout: Duration::from_secs(1),
            },
        )
        .unwrap();
        let deadline = hook
            .filter(|(_, cancel)| !cancel)
            .map(|_| Instant::now() + Duration::from_millis(200));
        let result = exec_until(&client, op, Some("s"), deadline, &stop, None);
        client.close().unwrap();
        thread.join().unwrap();
        result
    }
    fn mutations() -> Vec<(Op, &'static str, Value)> {
        let models: Vec<Value> =
            serde_json::from_str(include_str!("../../tests/support/fake_models.json")).unwrap();
        vec![
            (
                Op::SetModel(ops::SetModelArgs {
                    provider: "fake".into(),
                    model_id: "fake-2".into(),
                }),
                "set_model",
                models[1].clone(),
            ),
            (
                Op::SetThinkingLevel(ops::ThinkingArgs {
                    level: "high".into(),
                }),
                "set_thinking_level",
                json!({}),
            ),
            (
                Op::Compact(ops::CompactArgs::default()),
                "compact",
                json!({"summary":"short","firstKeptEntryId":"a","tokensBefore":1234}),
            ),
        ]
    }
    fn preflight(op: &Op) -> Vec<(&'static str, Option<Value>)> {
        let mut script = vec![(
            "get_state",
            Some(json!({"sessionId":"s","thinkingLevel":"medium"})),
        )];
        if matches!(op, Op::SetThinkingLevel(_)) {
            script.push((
                "get_available_thinking_levels",
                Some(json!({"levels":["off","high"]})),
            ));
        }
        script
    }
    #[test]
    fn c2_1_submitted_disconnects_never_become_job_ended() {
        for (op, command, response) in mutations() {
            for after_reply in [false, true] {
                let mut script = preflight(&op);
                script.push((command, after_reply.then_some(response.clone())));
                if after_reply {
                    script.push(("get_state", None));
                }
                let error = peer(&op, script, None).unwrap_err();
                for terminal in [false, true] {
                    let error = job_error(error.clone(), terminal);
                    assert_eq!(error.code, ops::OUTCOME_UNKNOWN);
                    assert_eq!(error.details.unwrap()["reason"], ops::OUTCOME_UNKNOWN);
                }
            }
        }
    }
    #[test]
    fn c2_1_preflight_disconnects_still_become_job_ended() {
        for (op, _, _) in mutations() {
            let error = peer(&op, vec![("get_state", None)], None).unwrap_err();
            assert_eq!(job_error(error, true).code, ops::JOB_ENDED);
        }
        let op = &mutations()[1].0;
        let mut script = preflight(op);
        script[1].1 = None;
        assert_eq!(
            job_error(peer(op, script, None).unwrap_err(), true).code,
            ops::JOB_ENDED
        );
    }
    #[test]
    fn c2_2_cancelled_preflight_never_submits_but_submitted_work_finishes() {
        for (op, command, response) in mutations() {
            for at in 0..preflight(&op).len() {
                let script = preflight(&op).into_iter().take(at + 1).collect();
                assert_eq!(
                    peer(&op, script, Some((at, true))).unwrap_err().code,
                    "interrupted"
                );
            }
            let mut script = preflight(&op);
            let at = script.len();
            script.push((command, Some(response)));
            script.push((
                "get_state",
                Some(json!({"sessionId":"s","thinkingLevel":"high"})),
            ));
            assert!(peer(&op, script, Some((at, true))).is_ok());
        }
    }
    #[test]
    fn c2_9_deadline_after_apply_preserves_success() {
        // W8-DOC-8: the deadline_passed_after_apply warning moved from data.warnings to the
        // envelope's warnings, added by the CLI (tests/controls.rs c2_9_…); the executor only
        // keeps the success.
        for (op, command, response) in mutations() {
            let mut script = preflight(&op);
            let at = script.len();
            script.push((command, Some(response)));
            script.push((
                "get_state",
                Some(json!({"sessionId":"forked","thinkingLevel":"high"})),
            ));
            let data = peer(&op, script, Some((at, false))).unwrap();
            assert_eq!(data["session_id"], "forked");
            assert!(data.get("warnings").is_none(), "{data}");
            let normalized = match op {
                Op::SetModel(_) => value(serde_json::from_value::<ModelSetData>(data).unwrap()),
                Op::SetThinkingLevel(_) => {
                    value(serde_json::from_value::<ThinkingSetData>(data).unwrap())
                }
                _ => value(serde_json::from_value::<CompactData>(data).unwrap()),
            }
            .unwrap();
            assert_eq!(normalized["session_id"], "forked");
            assert!(normalized.get("warnings").is_none(), "{normalized}");
        }
    }
    #[test]
    fn c2_9_post_apply_state_errors_remain_errors() {
        let (op, command, response) = mutations().remove(0);
        let mut script = preflight(&op);
        let at = script.len();
        script.push((command, Some(response)));
        script.push(("get_state", Some(json!({"error":"state unavailable"}))));
        assert_eq!(
            peer(&op, script, Some((at, false))).unwrap_err().code,
            ops::OMP_ERROR
        );
    }
}
