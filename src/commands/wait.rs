use crate::{
    cli::{Cli, WaitArgs},
    env::Env,
    error::{ErrorCode, GlenError, OkExit},
    glen_home,
    jobs::{
        Job, JobId, JobStore, Route, State, Status,
        wait::{self, Waited},
    },
    output::{Mode, Outcome, OutputCtx},
    render::{palette::Token, spinner::Spinner, text},
    rpc::session_handle,
};
use serde_json::{Value, json};
use std::time::Instant;
pub(crate) fn job_view(job: &Job, s: &Status, now: jiff::Timestamp) -> Value {
    let elapsed = job
        .created_at
        .parse::<jiff::Timestamp>()
        .map(|t| {
            now.as_millisecond()
                .saturating_sub(t.as_millisecond())
                .max(0)
        })
        .unwrap_or(0);
    json!({"id":job.id,"handle":job.handle,"status":s.status,"route":job.route,"deliver_as":job.deliver_as,"delivered_as":s.delivered_as,"session_id":job.session_id,"session_handle":job.session_handle,"actual_session_id":s.actual_session_id,"created_at":job.created_at,"updated_at":s.updated_at,"elapsed_ms":elapsed})
}
fn session(job: &Job, s: &Status) -> Value {
    if job.route == Route::Rpc {
        json!({"id":job.session_id,"handle":job.session_handle,"path":job.session_file})
    } else {
        json!({"id":s.actual_session_id,"handle":s.actual_session_id.as_deref().map(session_handle),"path":s.actual_session_file})
    }
}
fn painted_lines(ctx: &OutputCtx, token: Token, value: &str) -> Vec<String> {
    text::clean_lines(value)
        .iter()
        .flat_map(|line| text::wrap(line, ctx.width.map(usize::from)))
        .map(|line| ctx.painter.paint(token, &line))
        .collect()
}
pub(crate) fn outcome(
    store: &JobStore,
    job: &Job,
    waited: Waited,
    ctx: &OutputCtx,
) -> Result<Outcome, GlenError> {
    let interrupted = matches!(&waited, Waited::Interrupted(_));
    let terminal = matches!(&waited, Waited::Terminal(_));
    let s = match waited {
        Waited::Terminal(s) | Waited::Running(s) | Waited::Interrupted(s) => s,
    };
    let view = job_view(job, &s, jiff::Timestamp::now());
    let forked = s
        .forked
        .as_ref()
        .map(|f| json!({"from":f.from,"to":f.to,"to_handle":session_handle(&f.to)}));
    if terminal && s.status != State::Completed {
        let error = s.error.as_ref();
        let code = if error.is_some_and(|e| e.code == "no_route") {
            ErrorCode::NoRoute
        } else {
            ErrorCode::JobFailed
        };
        let mut e = GlenError::new(
            code,
            format!(
                "job {} {}: {}",
                job.handle,
                s.status.as_str(),
                error.map(|e| e.message.as_str()).unwrap_or("no result")
            ),
        )
        .with_details(json!({"status":s.status,"job":view,"error":s.error,"forked":forked}));
        e.hint = if code == ErrorCode::NoRoute {
            error.and_then(|e| e.hint.clone())
        } else {
            Some(format!(
                "see {}/worker.log",
                store
                    .dir(&JobId::parse(&job.id).ok_or_else(|| GlenError::new(
                        ErrorCode::Internal,
                        "invalid stored job id"
                    ))?)
                    .display()
            ))
        };
        return Err(e);
    }
    let session = session(job, &s);
    let session_label = session["handle"].as_str().unwrap_or("@new");
    let mut human = vec![];
    let (data, exit) = if terminal {
        let id = JobId::parse(&job.id)
            .ok_or_else(|| GlenError::new(ErrorCode::Internal, "invalid stored job id"))?;
        let r = store.read_result(&id)?;
        human.extend(painted_lines(
            ctx,
            Token::Success,
            &format!(
                "✓ completed · {} · {} · {}ms",
                job.handle, session_label, view["elapsed_ms"]
            ),
        ));
        if let Some(t) = &r.assistant_text {
            human.push(String::new());
            for l in text::clean_lines(t) {
                human.extend(text::wrap(&format!("  {l}"), ctx.width.map(usize::from)));
            }
        }
        for reply in &r.replies {
            for l in text::clean_lines(&reply.text) {
                human.extend(painted_lines(
                    ctx,
                    Token::Muted,
                    &format!("↩ {l}{}", if reply.is_final { " ✓" } else { "" }),
                ));
            }
        }
        for output in &r.command_output {
            human.extend(painted_lines(ctx, Token::Muted, output));
        }
        if let Some(f) = &s.forked {
            human.extend(painted_lines(
                ctx,
                Token::Warning,
                &format!("→ continued in {}", session_handle(&f.to)),
            ));
            human.extend(painted_lines(
                ctx,
                Token::Muted,
                "run `glen ls` and send to the new handle",
            ));
        }
        (
            json!({"status":"completed","job":view,"session":session,"assistant_text":r.assistant_text,"replies":r.replies,"command_output":r.command_output,"stop_reason":r.stop_reason,"session_settled":r.session_settled,"delivered_as":s.delivered_as,"forked":forked}),
            if s.forked.is_some() {
                OkExit::Forked
            } else {
                OkExit::Done
            },
        )
    } else {
        human.extend(painted_lines(
            ctx,
            Token::Focus,
            &format!("◐ running · {} · {}", job.handle, session_label),
        ));
        human.extend(painted_lines(
            ctx,
            Token::Muted,
            &format!("glen wait {}", job.handle),
        ));
        if interrupted {
            human.extend(painted_lines(
                ctx,
                Token::Muted,
                "interrupted; the job keeps running",
            ));
        }
        (
            json!({"status":"running","job":view,"session":session,"next":format!("glen wait {}",job.handle),"interrupted":interrupted}),
            if interrupted {
                OkExit::Interrupted
            } else {
                OkExit::Running
            },
        )
    };
    // All human strings contain escaped controls; wrap before painting where possible.
    let human = if ctx.mode == Mode::Human {
        human.join("\n")
    } else {
        String::new()
    };
    let mut o = Outcome::new(data, human);
    o.exit = exit;
    o.warnings = s.warnings;
    Ok(o)
}
pub fn run(_cli: &Cli, args: &WaitArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    let store = JobStore::new(glen_home::resolve(env)?);
    let id = store.resolve(&args.job)?;
    let job = store.read_job(&id)?;
    let flag = wait::install_interrupt()?;
    let spinner = Spinner::start(
        ctx.mode == Mode::Human && ctx.stderr_tty,
        &format!("waiting for {}", id.handle()),
    );
    let waited = wait::observe(
        &store,
        &id,
        Instant::now(),
        args.timeout,
        &flag,
        None,
        |s| spinner.set_message(s.status.as_str()),
    );
    spinner.finish();
    outcome(&store, &job, waited?, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cli::GlobalArgs,
        jobs::{Forked, JobError, JobResult, tests as fixtures},
        output::{OutputKind, Terminal},
    };
    fn ctx(root: &std::path::Path, human: bool) -> OutputCtx {
        let env = Env::for_test(Some(root), root, &[]);
        OutputCtx::new(
            &GlobalArgs {
                human,
                ..GlobalArgs::default()
            },
            OutputKind::Document,
            &env,
            &Terminal {
                stdout_tty: false,
                stderr_tty: false,
                stdout_cols: None,
            },
        )
        .unwrap()
    }
    #[test]
    fn outcome_exit_and_payload_table() {
        let t = tempfile::tempdir().unwrap();
        let store = fixtures::store(t.path());
        let j = fixtures::job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        let ctx = ctx(t.path(), false);
        let s = store.read_status(&id).unwrap();
        for (interrupted, code) in [(false, 10), (true, 130)] {
            let o = outcome(
                &store,
                &j,
                if interrupted {
                    Waited::Interrupted(s.clone())
                } else {
                    Waited::Running(s.clone())
                },
                &ctx,
            )
            .unwrap();
            assert_eq!(o.exit.exit_code(), code);
            assert_eq!(o.data["status"], "running");
            assert_eq!(o.data["interrupted"], interrupted);
            assert_eq!(o.data["next"], format!("glen wait {}", j.handle));
        }
        store
            .write_result(
                &id,
                &JobResult {
                    v: 1,
                    prompt_status: Some("completed".into()),
                    agent_invoked: true,
                    assistant_text: Some("pong".into()),
                    replies: vec![],
                    command_output: vec![],
                    stop_reason: Some("stop".into()),
                    session_settled: true,
                    settled_at: Some(j.created_at.clone()),
                },
            )
            .unwrap();
        for forked in [false, true] {
            let mut s = s.clone();
            s.status = State::Completed;
            s.delivered_as = Some("prompt".into());
            if forked {
                s.forked = Some(Forked {
                    from: "from".into(),
                    to: "01a00000-0000-7000-8000-000001234567".into(),
                });
            }
            let o = outcome(&store, &j, Waited::Terminal(s), &ctx).unwrap();
            assert_eq!(o.exit.exit_code(), if forked { 11 } else { 0 });
            assert_eq!(o.data["assistant_text"], "pong");
            assert_eq!(o.data["session"]["id"], json!(j.session_id));
            assert_eq!(o.data["session_settled"], true);
            if forked {
                assert_eq!(o.data["forked"]["to_handle"], "1234567");
            }
        }
        for (state, error, code) in [
            (State::Aborted, "cancelled", 7),
            (State::Failed, "no_route", 6),
            (State::Failed, "omp_error", 7),
            (State::Lost, "lost", 7),
        ] {
            let mut s = s.clone();
            s.status = state;
            s.error = Some(JobError {
                code: error.into(),
                message: "test failure".into(),
                hint: Some("retry".into()),
                details: None,
            });
            let e = outcome(&store, &j, Waited::Terminal(s), &ctx)
                .err()
                .unwrap();
            assert_eq!(e.code.exit_code(), code);
            let details = e.details.unwrap();
            assert_eq!(details["status"], state.as_str());
            assert_eq!(details["error"]["code"], error);
            assert!(details["job"].is_object());
        }
    }
    #[test]
    fn new_session_and_human_controls_are_sanitized_and_wrapped() {
        let t = tempfile::tempdir().unwrap();
        let store = fixtures::store(t.path());
        let mut j = fixtures::job();
        j.route = Route::New;
        j.session_id = None;
        j.session_file = None;
        j.session_handle = None;
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        let s = Status {
            v: 1,
            status: State::Completed,
            actual_session_id: Some("01a00000-0000-7000-8000-000001234567".into()),
            actual_session_file: Some("/new.jsonl".into()),
            ..Status::default()
        };
        store
            .write_result(
                &id,
                &JobResult {
                    v: 1,
                    prompt_status: None,
                    agent_invoked: false,
                    assistant_text: Some("hi\x1b[31m world".into()),
                    replies: vec![],
                    command_output: vec![],
                    stop_reason: None,
                    session_settled: true,
                    settled_at: None,
                },
            )
            .unwrap();
        let mut ctx = ctx(t.path(), true);
        ctx.width = Some(30);
        let o = outcome(&store, &j, Waited::Terminal(s), &ctx).unwrap();
        assert_eq!(o.data["session"]["handle"], "1234567");
        assert_eq!(o.data["session"]["path"], "/new.jsonl");
        assert!(!o.human.contains('\x1b'));
        assert!(o.human.lines().all(|l| text::width(l) <= 30), "{}", o.human);
    }
}
