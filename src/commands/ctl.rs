//! The five verb-first session controls share one routing and deadline contract.
use crate::{
    cli::{self, Cli},
    ctl::{
        self,
        route::{self, ModelsSource, Target},
    },
    env::Env,
    error::{ErrorCode, GlenError, OkExit, Warning},
    glen_home,
    jobs::{JobStore, State, wait},
    ops::{self, ModelRow, Op, OpError},
    output::{Mode, Outcome, OutputCtx},
    render::{self, Table, palette::Token, spinner::Spinner, text},
    rpc,
};
use serde_json::{Value, json};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
#[derive(Clone)]
enum Control {
    Abort,
    Model(String),
    Thinking(String),
    Compact(Option<String>),
}
impl Control {
    fn name(&self) -> &'static str {
        match self {
            Self::Abort => "abort",
            Self::Model(_) => "set_model",
            Self::Thinking(_) => "set_thinking_level",
            Self::Compact(_) => "compact",
        }
    }
}
fn open(cli: &Cli, env: &Env, target: &str) -> Result<super::open::Opened, GlenError> {
    if target == "@new" {
        return Err(GlenError::new(
            ErrorCode::Usage,
            "session controls need an existing session; @new is only for send",
        ));
    }
    if target
        .strip_prefix('j')
        .is_some_and(|s| s.len() >= 4 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(GlenError::new(
            ErrorCode::Usage,
            "controls target a session, not a glen job",
        )
        .with_hint(format!(
            "use `glen wait {target}` to inspect that job, then use its session handle"
        )));
    }
    super::open::open_unprobed(cli, env, target)
}
fn session(opened: &super::open::Opened) -> Value {
    json!({"id":opened.session.header.id,"handle":opened.handle,"path":if opened.not_yet_persisted {None}else{Some(&opened.session.path)}})
}
fn op_error(error: OpError, ctx: &ctl::ErrCtx<'_>) -> GlenError {
    if error.code == "interrupted" {
        return GlenError::new(ErrorCode::ControlFailed, error.message)
            .with_details(json!({"reason":"interrupted","route":ctx.route}));
    }
    ctl::to_glen_error(error, ctx)
}
fn interrupted(session: Value, route: &str, job: Option<&str>) -> Outcome {
    let hint = match route {
        "live" => "the op may still finish in that omp session; check it before retrying".into(),
        "job" => format!(
            "check job {} with `glen wait {}`; the control outcome is unknown",
            job.unwrap_or("j…"),
            job.unwrap_or("j…")
        ),
        _ => "the short-lived omp client was stopped".into(),
    };
    let mut outcome = Outcome::new(
        json!({"status":"interrupted","session":session,"route":route,"interrupted":true,"hint":hint}),
        "interrupted",
    );
    outcome.exit = OkExit::Interrupted;
    outcome
}
fn normalize<T: serde::de::DeserializeOwned + serde::Serialize>(
    data: Value,
) -> Result<Value, serde_json::Error> {
    serde_json::from_value::<T>(data).and_then(serde_json::to_value)
}
fn call(target: &mut Target, op: &Op, ctx: &ctl::ErrCtx<'_>) -> Result<Value, GlenError> {
    let mut data = target.call(op).map_err(|e| op_error(e, ctx))?;
    if matches!(op, Op::Abort) {
        if ctx.route == "none" {
            data["session_id"] = json!(ctx.session_id);
        }
        if ctx.route == "job" {
            if !data["job_status"]
                .as_str()
                .is_some_and(|s| matches!(s, "completed" | "aborted" | "failed" | "lost"))
            {
                return Err(op_error(
                    OpError::new(ops::INTERNAL, "invalid job abort acknowledgement"),
                    ctx,
                ));
            }
            data["was"] = json!("busy");
            data["aborted"] = json!(data["job_status"] == "aborted");
            data["idle_after"] = json!(true);
        }
        if !data["was"]
            .as_str()
            .is_some_and(|s| matches!(s, "busy" | "idle"))
        {
            return Err(op_error(
                OpError::new(ops::INTERNAL, "invalid abort state"),
                ctx,
            ));
        }
    }
    let status = data.get("job_status").cloned();
    let result = match op {
        Op::Abort => normalize::<ops::AbortData>(data),
        Op::ListModels => normalize::<ops::ModelsData>(data),
        Op::SetModel(_) => normalize::<ops::ModelSetData>(data),
        Op::SetThinkingLevel(_) => normalize::<ops::ThinkingSetData>(data),
        Op::Compact(_) => normalize::<ops::CompactData>(data),
        _ => {
            return Err(op_error(
                OpError::new(ops::INTERNAL, "unexpected control response op"),
                ctx,
            ));
        }
    };
    let mut result = result.map_err(|e| {
        op_error(
            OpError::new(
                ops::INTERNAL,
                format!("invalid {} response: {e}", op.name()),
            ),
            ctx,
        )
    })?;
    if let Some(status) = status {
        result["job_status"] = status;
    }
    Ok(result)
}
fn execute(
    target: &mut Target,
    control: &Control,
    ctx: &ctl::ErrCtx<'_>,
) -> Result<(Value, Vec<Warning>), GlenError> {
    let mut warnings = vec![];
    let mut data = match control {
        Control::Abort => {
            let data = call(target, &Op::Abort, ctx)?;
            if data["idle_after"] == false {
                warnings.push(Warning {
                    code: "abort_pending".into(),
                    message: "omp accepted the abort but has not become idle yet".into(),
                });
            }
            data
        }
        Control::Model(spec) => {
            let listed = call(target, &Op::ListModels, ctx)?;
            let rows: Vec<ModelRow> =
                serde_json::from_value(listed["models"].clone()).map_err(|e| {
                    op_error(
                        OpError::new(ops::INTERNAL, format!("invalid session model list: {e}")),
                        ctx,
                    )
                })?;
            let model=match ctl::resolve_model(&rows,spec) {
                ctl::Resolved::One(model)=>model,
                ctl::Resolved::None=>return Err(op_error(OpError::new(ops::MODEL_NOT_FOUND,format!("model {spec} was not found")),ctx)),
                ctl::Resolved::Many(rows)=>return Err(GlenError::new(ErrorCode::Ambiguous,format!("model {spec} matches several providers")).with_details(json!({"candidates":rows.iter().map(|m|json!({"provider":m.provider,"id":m.id,"name":m.name})).collect::<Vec<_>>()})).with_hint(format!("run `glen models {}` and choose a full provider/model",ctx.handle))),
            };
            let mut data = call(
                target,
                &Op::SetModel(ops::SetModelArgs {
                    provider: model.provider,
                    model_id: model.id,
                }),
                ctx,
            )?;
            data["changed"] = json!(
                data["model"]["provider"] != data["previous"]["provider"]
                    || data["model"]["id"] != data["previous"]["id"]
            );
            if data["thinking_level"] != listed["thinking_level"] {
                warnings.push(Warning {
                    code: "thinking_changed".into(),
                    message: format!(
                        "omp changed thinking from {} to {} for the new model",
                        listed["thinking_level"].as_str().unwrap_or("unknown"),
                        data["thinking_level"].as_str().unwrap_or("unknown")
                    ),
                });
            }
            data["scope"] = json!("session");
            data
        }
        Control::Thinking(level) => {
            let mut data = call(
                target,
                &Op::SetThinkingLevel(ops::ThinkingArgs {
                    level: level.clone(),
                }),
                ctx,
            )?;
            data["requested"] = json!(level);
            data["changed"] = json!(data["thinking_level"] != data["previous"]);
            data["scope"] = json!("session");
            if data["thinking_level"] != *level {
                warnings.push(Warning {
                    code: "thinking_clamped".into(),
                    message: format!(
                        "omp applied thinking {} instead of {level}",
                        data["thinking_level"].as_str().unwrap_or("unknown")
                    ),
                });
            }
            data
        }
        Control::Compact(instructions) => {
            let mut data = call(
                target,
                &Op::Compact(ops::CompactArgs {
                    instructions: instructions.clone(),
                }),
                ctx,
            )?;
            data["compacted"] = json!(true);
            data
        }
    };
    if let Some(object) = data.as_object_mut() {
        object.insert("route".into(), json!(ctx.route));
    } else {
        return Err(op_error(
            OpError::new(ops::INTERNAL, "control response data is not an object"),
            ctx,
        ));
    }
    Ok((data, warnings))
}
fn check_instructions_len(instructions: &str) -> Result<(), GlenError> {
    if instructions.len() > ops::MAX_INSTRUCTIONS {
        return Err(GlenError::new(
            ErrorCode::Usage,
            "compaction instructions are larger than 512 KiB",
        ));
    }
    Ok(())
}
fn run_control(
    cli: &Cli,
    env: &Env,
    output: &OutputCtx,
    target: &str,
    timeout: Duration,
    control: Control,
) -> Result<Outcome, GlenError> {
    let deadline = Instant::now() + timeout;
    let stop = wait::install_interrupt()?;
    let opened = open(cli, env, target)?;
    if let Control::Compact(Some(instructions)) = &control {
        check_instructions_len(instructions)?;
    }
    let progress = match &control {
        Control::Abort => format!("stopping {}…", opened.handle),
        Control::Model(model) => format!(
            "switching {} to {}…",
            opened.handle,
            render::sanitize(model)
        ),
        Control::Thinking(level) => format!("setting {} thinking {level}…", opened.handle),
        Control::Compact(_) => format!("compacting {}…", opened.handle),
    };
    let spinner = Spinner::start(output.mode == Mode::Human && output.stderr_tty, &progress);
    for attempt in 0..2 {
        let routed = route::route(cli, env, &opened, control.name(), deadline, stop.clone());
        let (mut transport, mut warnings) = match routed {
            Ok(value) => value,
            Err(error) if stop.load(Ordering::SeqCst) => {
                return Ok(interrupted(
                    session(&opened),
                    error
                        .details
                        .as_ref()
                        .and_then(|d| d["route"].as_str())
                        .unwrap_or("none"),
                    None,
                ));
            }
            Err(error) => return Err(error),
        };
        let route = transport.name();
        let job = transport.job().map(str::to_owned);
        let version = transport.ext_version().map(str::to_owned);
        let context = ctl::ErrCtx {
            handle: &opened.handle,
            session_id: &opened.session.header.id,
            route,
            job: job.as_deref(),
            ext_version: version.as_deref(),
        };
        let result = execute(&mut transport, &control, &context);
        let finished = transport.finish();
        if stop.load(Ordering::SeqCst) {
            return Ok(interrupted(session(&opened), route, job.as_deref()));
        }
        let (mut data, op_warnings) = match result {
            Ok(data) => data,
            Err(error)
                if attempt == 0
                    && error
                        .details
                        .as_ref()
                        .is_some_and(|d| d["reason"] == ops::JOB_ENDED) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let (rpc_fork, finish_warnings) = finished.map_err(|e| op_error(e, &context))?;
        warnings.extend(op_warnings);
        warnings.extend(finish_warnings);
        let post_id = data["session_id"].as_str().map(str::to_owned);
        let fork = rpc_fork.or_else(|| {
            post_id
                .filter(|to| to != &opened.session.header.id)
                .map(|to| crate::jobs::Forked {
                    from: opened.session.header.id.clone(),
                    to,
                })
        });
        if let Some(object) = data.as_object_mut() {
            object.remove("session_id");
        }
        data["session"] = session(&opened);
        data["route"] = json!(route);
        if let Some(job) = &job {
            data["job"] = json!(job);
        }
        if !matches!(control, Control::Abort) || fork.is_some() {
            data["forked"] =
                json!(fork.as_ref().map(
                    |f| json!({"from":f.from,"to":f.to,"to_handle":rpc::session_handle(&f.to)})
                ));
        }
        if opened.not_yet_persisted && matches!(control, Control::Model(_) | Control::Thinking(_)) {
            warnings.push(route::not_persisted());
        }
        if matches!(control, Control::Abort) {
            let store = JobStore::new(glen_home::resolve(env)?);
            let queued: Vec<_> = store
                .active_for_session(&opened.session.header.id)
                .into_iter()
                .filter(|(_, _, s)| s.status == State::Queued)
                .filter(|(id, _, _)| Some(id.handle()).as_deref() != job.as_deref())
                .map(|(id, _, _)| id.handle())
                .collect();
            if !queued.is_empty() {
                warnings.push(Warning {
                    code: "jobs_queued".into(),
                    message: format!("{} glen jobs remain queued and start next", queued.len()),
                });
            }
            data["queued_jobs"] = json!(queued);
        }
        spinner.finish();
        let human = human_control(&opened.handle, &control, &data, job.as_deref(), output);
        let mut outcome = Outcome::new(data, human);
        outcome.warnings = warnings;
        if fork.is_some() {
            outcome.exit = OkExit::Forked;
        }
        return Ok(outcome);
    }
    Err(GlenError::new(
        ErrorCode::ControlFailed,
        "the glen job ended again while applying a control",
    )
    .with_details(json!({"reason":"job_ended"})))
}
fn pair(model: &Value) -> String {
    if let (Some(provider), Some(id)) = (model["provider"].as_str(), model["id"].as_str()) {
        render::sanitize(&format!("{provider}/{id}")).into_owned()
    } else {
        "unknown model".into()
    }
}
fn human_control(
    handle: &str,
    control: &Control,
    data: &Value,
    job: Option<&str>,
    ctx: &OutputCtx,
) -> String {
    let route = match data["route"].as_str().unwrap_or("none") {
        "rpc" => "omp (headless)".into(),
        "job" => format!("glen job {}", job.unwrap_or("unknown")),
        name => name.into(),
    };
    let level = render::sanitize(data["thinking_level"].as_str().unwrap_or("unknown"));
    let text = match control {
        Control::Abort
            if data["aborted"] == false
                && job.is_some()
                && matches!(
                    data["job_status"].as_str(),
                    Some("completed" | "failed" | "lost")
                ) =>
        {
            format!("· job {} had already finished", job.unwrap_or("unknown"))
        }
        Control::Abort if data["aborted"] == false => {
            format!("· {handle} is idle; nothing to abort")
        }
        Control::Abort if job.is_some() => format!(
            "✓ cancelled glen job {} on {handle}",
            job.unwrap_or("unknown")
        ),
        Control::Abort => format!("✓ stopped the running turn in {handle}"),
        Control::Model(_) => format!(
            "✓ {handle} → {} (was {}) · thinking {level}",
            pair(&data["model"]),
            pair(&data["previous"])
        ),
        Control::Thinking(_) => format!("✓ {handle} thinking {level} · {}", pair(&data["model"])),
        Control::Compact(_) => format!(
            "✓ compacted {handle} · {} → {} tokens",
            data["tokens_before"]
                .as_i64()
                .map(|n| n.to_string())
                .unwrap_or_else(|| "unknown".into()),
            data["tokens_after"]
                .as_i64()
                .map(|n| n.to_string())
                .unwrap_or_else(|| "unknown".into())
        ),
    };
    let mut lines = text::wrap(&text, ctx.width.map(usize::from));
    let suffix = format!(" · {route}");
    if let Some(last) = lines.last_mut()
        && ctx
            .width
            .is_none_or(|width| text::width(last) + text::width(&suffix) <= usize::from(width))
    {
        last.push_str(&ctx.painter.paint(Token::Muted, &suffix));
    } else {
        lines.extend(
            text::wrap(&route, ctx.width.map(usize::from))
                .iter()
                .map(|line| ctx.painter.paint(Token::Muted, line)),
        );
    }
    if matches!(control, Control::Model(_)) && data["busy"] == true {
        lines.extend(
            text::wrap(
                "saved in the session; applies from the next model request",
                ctx.width.map(usize::from),
            )
            .iter()
            .map(|line| ctx.painter.paint(Token::Muted, line)),
        );
    }
    lines.join("\n")
}
pub fn run_abort(
    cli: &Cli,
    args: &cli::CtlArgs,
    env: &Env,
    ctx: &OutputCtx,
) -> Result<Outcome, GlenError> {
    run_control(cli, env, ctx, &args.session, args.timeout, Control::Abort)
}
pub fn run_model(
    cli: &Cli,
    args: &cli::ModelArgs,
    env: &Env,
    ctx: &OutputCtx,
) -> Result<Outcome, GlenError> {
    run_control(
        cli,
        env,
        ctx,
        &args.session,
        args.timeout,
        Control::Model(args.model.clone()),
    )
}
pub fn run_thinking(
    cli: &Cli,
    args: &cli::ThinkingArgs,
    env: &Env,
    ctx: &OutputCtx,
) -> Result<Outcome, GlenError> {
    run_control(
        cli,
        env,
        ctx,
        &args.session,
        args.timeout,
        Control::Thinking(args.level.as_str().into()),
    )
}
pub fn run_compact(
    cli: &Cli,
    args: &cli::CompactArgs,
    env: &Env,
    ctx: &OutputCtx,
) -> Result<Outcome, GlenError> {
    run_control(
        cli,
        env,
        ctx,
        &args.session,
        args.timeout,
        Control::Compact(args.instructions.clone()),
    )
}
pub fn run_models(
    cli: &Cli,
    args: &cli::ModelsArgs,
    env: &Env,
    output: &OutputCtx,
) -> Result<Outcome, GlenError> {
    let deadline = Instant::now() + args.timeout;
    let stop = wait::install_interrupt()?;
    let opened = args
        .session
        .as_deref()
        .map(|target| open(cli, env, target))
        .transpose()?;
    let session = opened.as_ref().map(session).unwrap_or(Value::Null);
    let spinner = Spinner::start(
        output.mode == Mode::Human && output.stderr_tty,
        "listing omp models…",
    );
    for attempt in 0..2 {
        let (mut target, mut warnings, source) =
            match route::route_models(cli, env, opened.as_ref(), deadline, stop.clone()) {
                Ok(routed) => routed,
                Err(_error) if stop.load(Ordering::SeqCst) => {
                    return Ok(interrupted(session, "rpc", None));
                }
                Err(error) => return Err(error),
            };
        let route = target.name();
        let job = target.job().map(str::to_owned);
        let ext = target.ext_version().map(str::to_owned);
        let context = ctl::ErrCtx {
            handle: opened
                .as_ref()
                .map(|o| o.handle.as_str())
                .unwrap_or("profile"),
            session_id: opened
                .as_ref()
                .map(|o| o.session.header.id.as_str())
                .unwrap_or(""),
            route,
            job: job.as_deref(),
            ext_version: ext.as_deref(),
        };
        let result = call(&mut target, &Op::ListModels, &context);
        let finished = target.finish();
        if stop.load(Ordering::SeqCst) {
            return Ok(interrupted(session, route, job.as_deref()));
        }
        let mut data = match result {
            Ok(data) => data,
            Err(error)
                if attempt == 0
                    && error
                        .details
                        .as_ref()
                        .is_some_and(|d| d["reason"] == ops::JOB_ENDED) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let (_, finish_warnings) = finished.map_err(|e| op_error(e, &context))?;
        warnings.extend(finish_warnings);
        let default_model = if matches!(source, ModelsSource::Profile) {
            data["current"].clone()
        } else {
            Value::Null
        };
        data["thinking_configured"] = Value::Null;
        match source {
            ModelsSource::Session => data["current_source"] = json!("session"),
            ModelsSource::Disk(selection) => {
                data["current_source"] = json!("disk");
                data["current"] = json!(selection.model.and_then(|m| {
                    m.split_once('/')
                        .map(|(provider, id)| json!({"provider":provider,"id":id}))
                }));
                data["thinking_level"] = json!(selection.thinking_level);
                data["thinking_configured"] = json!(selection.thinking_configured);
            }
            ModelsSource::Profile | ModelsSource::Unpersisted => {
                data["current_source"] = Value::Null;
                data["current"] = Value::Null;
                data["thinking_level"] = Value::Null;
            }
        }
        let mut rows: Vec<ModelRow> =
            serde_json::from_value(data["models"].clone()).map_err(|e| {
                op_error(
                    OpError::new(ops::INTERNAL, format!("invalid model rows: {e}")),
                    &context,
                )
            })?;
        rows.retain(|m| {
            args.provider
                .as_ref()
                .is_none_or(|provider| m.provider.eq_ignore_ascii_case(provider))
        });
        rows.sort_by(|a, b| (&a.provider, &a.id).cmp(&(&b.provider, &b.id)));
        data["models"] = json!(
            rows.iter()
                .map(|row| {
                    let mut value = json!(row);
                    value["current"] = json!(
                        data["current"]["provider"] == row.provider
                            && data["current"]["id"] == row.id
                    );
                    value
                })
                .collect::<Vec<_>>()
        );
        data["session"] = session;
        data["route"] = json!(route);
        data["default_model"] = default_model;
        data["total"] = json!(rows.len());
        if let Some(object) = data.as_object_mut() {
            object.remove("session_id");
        }
        spinner.finish();
        let human = models_table(&data, output);
        let mut outcome = Outcome::new(data, human);
        outcome.warnings = warnings;
        return Ok(outcome);
    }
    Err(GlenError::new(
        ErrorCode::ControlFailed,
        "the glen job ended again while listing models",
    )
    .with_details(json!({"reason":"job_ended"})))
}
fn models_table(data: &Value, ctx: &OutputCtx) -> String {
    let labels = ["MODEL", "NAME", "CONTEXT", "THINKING"];
    let rows = data["models"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            vec![
                format!(
                    "{} {}",
                    if m["current"] == true { "●" } else { " " },
                    pair(m)
                ),
                render::sanitize(m["name"].as_str().unwrap_or("")).into_owned(),
                m["context_window"]
                    .as_i64()
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                m["thinking_levels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(","),
            ]
        })
        .collect::<Vec<_>>();
    let mut budgets = (0..4)
        .map(|column| {
            rows.iter()
                .map(|row| text::width(&row[column]))
                .max()
                .unwrap_or(0)
                .max(labels[column].len())
        })
        .collect::<Vec<_>>();
    if let Some(width) = ctx.width {
        for column in [1, 3, 0, 2] {
            while budgets.iter().sum::<usize>() + 6 > usize::from(width) && budgets[column] > 1 {
                budgets[column] -= 1;
            }
        }
    }
    let mut table = Table::new(&ctx.painter, None);
    let headers = (0..4)
        .map(|i| text::truncate(labels[i], budgets[i]))
        .collect::<Vec<_>>();
    table.header(&headers.iter().map(String::as_str).collect::<Vec<_>>());
    for row in rows {
        table.row(
            row.iter()
                .enumerate()
                .map(|(i, value)| text::truncate(value, budgets[i]))
                .collect(),
        );
    }
    let providers = data["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["provider"].as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let selection = if data["current"].is_null() && data["default_model"].is_object() {
        format!("default {}", pair(&data["default_model"]))
    } else {
        format!("current {}", pair(&data["current"]))
    };
    let footer = format!(
        "{} models · {providers} {} · {selection} · thinking {} ({})",
        data["total"].as_u64().unwrap_or(0),
        if providers == 1 {
            "provider"
        } else {
            "providers"
        },
        render::sanitize(data["thinking_level"].as_str().unwrap_or("unknown")),
        data["current_source"].as_str().unwrap_or("profile")
    );
    format!(
        "{}\n\n{}",
        table.render(),
        text::wrap(&footer, ctx.width.map(usize::from)).join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // Linux MAX_ARG_STRLEN (131072 bytes) prevents testing the full 512 KiB
    // boundary through argv; the conformance test still covers that path on macOS.
    #[test]
    fn instructions_are_bounded_at_524288_bytes() {
        let at = "x".repeat(524_288);
        assert!(check_instructions_len(&at).is_ok());
        let over = "x".repeat(524_289);
        assert_eq!(
            check_instructions_len(&over).unwrap_err().code,
            ErrorCode::Usage
        );
    }

    fn human_output() -> OutputCtx {
        let cli = Cli::try_parse_from(["glen", "--human", "models"]).unwrap();
        let env = Env::for_test(None, std::path::Path::new("."), &[]);
        OutputCtx::new(
            &cli.global,
            crate::output::OutputKind::Document,
            &env,
            &crate::output::Terminal {
                stdout_tty: false,
                stderr_tty: false,
                stdout_cols: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn models_footer_shows_profile_default_and_pluralizes_providers() {
        let output = human_output();
        let mut data = json!({
            "models": [{"provider":"fake","id":"fake-1"}],
            "total": 1,
            "current": null,
            "default_model": {"provider":"fake","id":"fake-1"},
            "thinking_level": null,
            "current_source": null
        });
        let human = models_table(&data, &output);
        assert!(
            human.contains("1 provider · default fake/fake-1"),
            "{human}"
        );
        data["models"] = json!([]);
        data["total"] = json!(0);
        data["default_model"] = Value::Null;
        assert!(models_table(&data, &output).contains("0 providers · current unknown"));
        data["models"] = json!([
            {"provider":"fake","id":"fake-1"},
            {"provider":"other","id":"other-1"}
        ]);
        data["total"] = json!(2);
        data["current"] = json!({"provider":"other","id":"other-1"});
        data["default_model"] = json!({"provider":"fake","id":"fake-1"});
        assert!(models_table(&data, &output).contains("2 providers · current other/other-1"));
    }

    #[test]
    fn abort_finished_jobs_reports_job_handle_and_keeps_idle_wording() {
        let output = human_output();
        for status in ["completed", "failed", "lost"] {
            let data = json!({
                "route":"job", "job_status":status,
                "was":"busy", "aborted":false, "idle_after":true
            });
            assert_eq!(
                human_control(
                    "s1234567",
                    &Control::Abort,
                    &data,
                    Some("j1234567"),
                    &output
                ),
                "· job j1234567 had already finished · glen job j1234567"
            );
        }
        for route in ["rpc", "live", "none"] {
            let data = json!({"route":route,"was":"idle","aborted":false});
            assert!(
                human_control("s1234567", &Control::Abort, &data, None, &output)
                    .starts_with("· s1234567 is idle; nothing to abort")
            );
        }
        let data = json!({"route":"job","job_status":"aborted","aborted":true});
        assert!(
            human_control(
                "s1234567",
                &Control::Abort,
                &data,
                Some("j1234567"),
                &output
            )
            .starts_with("✓ cancelled glen job j1234567 on s1234567")
        );
    }

    #[test]
    fn control_sandbox_refusals_follow_target_validation_and_precede_lease_probes() {
        let tree = crate::omp::fixtures::tree();
        let path = crate::omp::fixtures::path(&tree, 'A');
        let agent = tree.path().join("unused-agent");
        for network in ["0", "1"] {
            let env = Env::for_test(
                Some(tree.path()),
                tree.path(),
                &[
                    ("PI_CODING_AGENT_DIR", agent.to_str().unwrap()),
                    ("CODEX_SANDBOX", "seatbelt"),
                    ("CODEX_SANDBOX_NETWORK_DISABLED", network),
                ],
            );
            for (command, extra) in [
                ("abort", vec![]),
                ("model", vec!["fake/fake-1"]),
                ("thinking", vec!["high"]),
                ("compact", vec![]),
                ("models", vec![]),
            ] {
                for (target, code) in [
                    (path.to_str().unwrap(), ErrorCode::NoRoute),
                    ("fffffff", ErrorCode::NotFound),
                    ("@new", ErrorCode::Usage),
                    ("j1234567", ErrorCode::Usage),
                ] {
                    let mut argv = vec!["glen", command, target];
                    argv.extend(extra.clone());
                    let cli = Cli::try_parse_from(argv).unwrap();
                    let output = OutputCtx::new(
                        &cli.global,
                        crate::output::OutputKind::Document,
                        &env,
                        &crate::output::Terminal {
                            stdout_tty: false,
                            stderr_tty: false,
                            stdout_cols: None,
                        },
                    )
                    .unwrap();
                    crate::omp::lease::PROBE_CALLS.with(|calls| calls.set(0));
                    let result = super::super::dispatch(&cli, &env, &output);
                    let Err(error) = result else {
                        panic!("unexpected {command} success")
                    };
                    assert_eq!(error.code, code, "{command} {target}");
                    crate::omp::lease::PROBE_CALLS.with(|calls| {
                        assert_eq!(
                            calls.get(),
                            0,
                            "{command} probed a lease before sandbox refusal"
                        )
                    });
                }
            }
        }
    }
}
