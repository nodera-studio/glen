use crate::{
    cli::{Cli, WaitArgs, WaitFor},
    env::Env,
    error::{ErrorCode, GlenError, OkExit},
    glen_home,
    jobs::{
        Job, JobId, JobStore, Reply, Route, State, Status,
        wait::{self, ReplyWatch, Waited},
    },
    output::{Mode, Outcome, OutputCtx},
    render::{palette::Token, spinner::Spinner, text},
    rpc::session_handle,
};
use serde_json::{Value, json};
use std::{collections::HashSet, time::Instant};
pub(crate) fn progress(s: &Status) -> &'static str {
    match s.status {
        State::Queued if s.waiting_for_lock => "queued behind another glen job",
        State::Queued => "starting omp",
        State::Accepted => "waiting for the reply",
        _ => s.status.as_str(),
    }
}
pub(crate) fn job_progress(job: &Job, s: &Status) -> String {
    if job.route != Route::Live {
        return progress(s).into();
    }
    match s.status {
        State::Queued => format!(
            "delivering to {} (live) · {}",
            job.session_handle.as_deref().unwrap_or("?"),
            job.handle
        ),
        State::Accepted => format!(
            "delivered as {}; omp is working",
            s.delivered_as.as_deref().unwrap_or("a message")
        ),
        _ => s.status.as_str().into(),
    }
}
pub(crate) fn duration(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}
pub(crate) fn job_view(job: &Job, s: &Status, now: jiff::Timestamp) -> Value {
    let elapsed = job
        .created_at
        .parse::<jiff::Timestamp>()
        .map(|t| {
            let end = if s.status.is_terminal() {
                s.updated_at.parse::<jiff::Timestamp>().unwrap_or(now)
            } else {
                now
            };
            end.as_millisecond()
                .saturating_sub(t.as_millisecond())
                .max(0)
        })
        .unwrap_or(0);
    json!({"id":job.id,"handle":job.handle,"status":s.status,"route":job.route,"deliver_as":job.deliver_as,"delivered_as":s.delivered_as,"session_id":job.session_id,"session_handle":job.session_handle,"actual_session_id":s.actual_session_id,"created_at":job.created_at,"updated_at":s.updated_at,"elapsed_ms":elapsed,"task_state":s.task_state()})
}
fn session(job: &Job, s: &Status) -> Value {
    if job.route != Route::New {
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
pub(crate) fn job_error(store: &JobStore, job: &Job, s: &Status) -> Result<GlenError, GlenError> {
    let view = job_view(job, s, jiff::Timestamp::now());
    let forked = s
        .forked
        .as_ref()
        .map(|f| json!({"from":f.from,"to":f.to,"to_handle":session_handle(&f.to)}));
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
    if code == ErrorCode::NoRoute {
        // A pre-W10 job record's error can be the no_route code with no `details.reason` at all
        // (AC-1004 postdates it); normalise to the documented fallback instead of a
        // reasonless envelope (which would also trip envelope.rs's debug_assert).
        let reason = error
            .and_then(|error| error.details.as_ref())
            .and_then(|details| details.get("reason"))
            .cloned()
            .unwrap_or_else(|| json!(crate::error::NoRouteReason::Unknown.as_str()));
        e = e.with_detail("reason", reason);
    }
    e.hint =
        if error.and_then(|e| e.hint.as_ref()).is_some() {
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
    e.warnings = s.warnings.clone();
    Ok(e)
}
pub(crate) fn completed_summary(job: &Job, s: &Status, now: jiff::Timestamp) -> String {
    let session = session(job, s);
    let label = session["handle"].as_str().unwrap_or("@new");
    let elapsed = duration(job_view(job, s, now)["elapsed_ms"].as_i64().unwrap_or(0));
    format!("✓ completed · {} · {label} · {elapsed}", job.handle)
}
/// What a `wait --for reply` caller has seen already: the ids up to its `--after`.
pub(crate) struct ReplyView {
    pub(crate) seen: HashSet<String>,
    pub(crate) after: Option<String>,
}
impl ReplyView {
    fn unseen(&self, replies: Vec<Reply>) -> Vec<Reply> {
        replies
            .into_iter()
            .filter(|r| !self.seen.contains(&r.id))
            .collect()
    }
}
/// The `↩ …` lines of reply records (`✓` on a final one), muted and wrapped.
fn reply_lines(ctx: &OutputCtx, replies: &[Reply]) -> Vec<String> {
    let mut human = vec![];
    for reply in replies {
        let mut first = true;
        for l in text::clean_lines(&reply.text) {
            for part in text::wrap(
                &l,
                ctx.width.map(|w| usize::from(w).saturating_sub(4).max(1)),
            ) {
                let line = if first {
                    format!("↩ {part}{}", if reply.is_final { " ✓" } else { "" })
                } else {
                    format!("  {part}")
                };
                human.push(ctx.painter.paint(Token::Muted, &line));
                first = false;
            }
        }
    }
    human
}
fn stored_id(job: &Job) -> Result<JobId, GlenError> {
    JobId::parse(&job.id)
        .ok_or_else(|| GlenError::new(ErrorCode::Internal, "invalid stored job id"))
}
pub(crate) fn outcome(
    store: &JobStore,
    job: &Job,
    waited: Waited,
    ctx: &OutputCtx,
) -> Result<Outcome, GlenError> {
    outcome_with(store, job, waited, ctx, None)
}
/// [`outcome`] for `wait --for reply` when `reply` is given (design §A20): replies are the unseen
/// subset, a running document adds `replies` and a `--for reply` `next`, and a reply wake adds
/// `woke:"reply"`. Without `reply` a `Replied` wake cannot occur; it reads as `Running`.
pub(crate) fn outcome_with(
    store: &JobStore,
    job: &Job,
    waited: Waited,
    ctx: &OutputCtx,
    reply: Option<&ReplyView>,
) -> Result<Outcome, GlenError> {
    let interrupted = matches!(&waited, Waited::Interrupted(_));
    let terminal = matches!(&waited, Waited::Terminal(_));
    let woke = reply.is_some() && matches!(&waited, Waited::Replied(..));
    let (s, woken) = match waited {
        Waited::Replied(s, replies) => (s, replies),
        Waited::Terminal(s) | Waited::Running(s) | Waited::Interrupted(s) => (s, vec![]),
    };
    let now = jiff::Timestamp::now();
    let view = job_view(job, &s, now);
    let forked = s
        .forked
        .as_ref()
        .map(|f| json!({"from":f.from,"to":f.to,"to_handle":session_handle(&f.to)}));
    if terminal && s.status != State::Completed {
        let mut e = job_error(store, job, &s)?;
        if let Some(reply) = reply {
            let replies = reply.unseen(store.read_replies_complete(&stored_id(job)?)?);
            e = e.with_detail("replies", json!(replies));
        }
        return Err(e);
    }
    let session = session(job, &s);
    let session_label = session["handle"].as_str().unwrap_or("@new");
    let mut human = vec![];
    let (data, exit) = if terminal {
        let mut r = store.read_result(&stored_id(job)?)?;
        if let Some(reply) = reply {
            r.replies = reply.unseen(r.replies);
        }
        let heading = format!("✓ completed · {} · {}", job.handle, session_label);
        let elapsed = duration(view["elapsed_ms"].as_i64().unwrap_or(0));
        let summary = completed_summary(job, &s, now);
        let summary = if ctx
            .width
            .is_some_and(|w| text::width(&summary) > usize::from(w))
        {
            format!("{heading}\n  {elapsed}")
        } else {
            summary
        };
        human.extend(painted_lines(ctx, Token::Success, &summary));
        if let Some(t) = &r.assistant_text {
            human.push(String::new());
            for l in text::clean_lines(t) {
                human.extend(text::wrap(&format!("  {l}"), ctx.width.map(usize::from)));
            }
        }
        human.extend(reply_lines(ctx, &r.replies));
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
        let next = match reply {
            None => format!("glen wait {}", job.handle),
            Some(reply) => {
                let after = woken.last().map(|r| &r.id).or(reply.after.as_ref());
                match after {
                    Some(after) => format!("glen wait {} --for reply --after {after}", job.handle),
                    None => format!("glen wait {} --for reply", job.handle),
                }
            }
        };
        human.extend(reply_lines(ctx, &woken));
        human.extend(painted_lines(
            ctx,
            Token::Focus,
            &format!("◐ running · {} · {}", job.handle, session_label),
        ));
        human.extend(painted_lines(ctx, Token::Muted, &next));
        if interrupted {
            human.extend(painted_lines(
                ctx,
                Token::Muted,
                "interrupted; the job keeps running",
            ));
        }
        let mut data = json!({"status":"running","job":view,"session":session,"next":next,"interrupted":interrupted});
        if reply.is_some() {
            data["replies"] = json!(woken);
        }
        if woke {
            data["woke"] = json!("reply");
        }
        (
            data,
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
    let store = JobStore::new(glen_home::resolve_trusted(env, &["jobs"])?);
    let id = store.resolve(&args.job)?;
    let job = store.read_job(&id)?;
    let mut watch = match args.wait_for {
        Some(WaitFor::Reply) => Some(
            ReplyWatch::new(&store, &id, args.after.as_deref())?
                .ok_or_else(|| unknown_reply(&job.handle, args.after.as_deref().unwrap_or("")))?,
        ),
        None => None,
    };
    let reply = watch.as_ref().map(|w| ReplyView {
        seen: w.seen().clone(),
        after: args.after.clone(),
    });
    let flag = wait::install_interrupt()?;
    let spinner = Spinner::start(
        ctx.mode == Mode::Human && ctx.stderr_tty,
        &if watch.is_some() {
            format!("waiting for a reply · {}", id.handle())
        } else {
            format!("waiting for {}", id.handle())
        },
    );
    let waited = wait::observe_with(
        &store,
        &id,
        Instant::now(),
        args.timeout,
        &flag,
        None,
        watch.as_mut(),
        |s| spinner.set_message(&job_progress(&job, s)),
    );
    spinner.finish();
    outcome_with(&store, &job, waited?, ctx, reply.as_ref())
}
/// `--after` names no reply of this job: a stale or foreign cursor would otherwise make every
/// reply look unseen.
fn unknown_reply(handle: &str, after: &str) -> GlenError {
    GlenError::new(
        ErrorCode::Usage,
        format!("no reply {after} in job {handle}"),
    )
    .with_details(json!({"reason":"unknown_reply","job":handle,"after":after}))
    .with_hint(format!(
        "run `glen wait {handle} --for reply` without --after"
    ))
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
    fn review8_progress_reports_only_observed_lock_contention() {
        let mut s = Status::default();
        assert_eq!(progress(&s), "starting omp");
        s.worker_pid = Some(123);
        s.waited_ms = Some(0);
        assert_eq!(progress(&s), "starting omp");
        s.waited_ms = Some(1);
        assert_eq!(progress(&s), "starting omp");
        s.waiting_for_lock = true;
        assert_eq!(progress(&s), "queued behind another glen job");
        s.status = State::Accepted;
        assert_eq!(progress(&s), "waiting for the reply");
    }
    #[test]
    fn review13_elapsed_stops_at_terminal_publication() {
        let mut j = fixtures::job();
        j.created_at = "2026-10-30T00:00:00Z".into();
        let now = "2026-10-30T01:00:00Z".parse().unwrap();
        let mut s = Status {
            status: State::Completed,
            updated_at: "2026-10-30T00:00:41Z".into(),
            ..Status::default()
        };
        assert_eq!(job_view(&j, &s, now)["elapsed_ms"], 41000);
        s.status = State::Accepted;
        assert_eq!(job_view(&j, &s, now)["elapsed_ms"], 3600000);
        assert_eq!(duration(41000), "41s");
        assert_eq!(duration(65000), "1m 05s");
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
            if error == "no_route" {
                // A pre-W10 (or otherwise reasonless) no_route job record normalises to the
                // documented fallback, never a missing details.reason (LOW-007).
                assert_eq!(details["reason"], "unknown", "{details}");
            }
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
    fn reply(job: &Job, id: &str, text: &str, is_final: bool) -> Reply {
        Reply {
            v: 1,
            id: id.into(),
            session_id: "s".into(),
            job_id: Some(job.id.clone()),
            request_id: None,
            tool_call_id: None,
            text: text.into(),
            is_final,
            at: "2026-11-02T00:00:00Z".into(),
        }
    }
    fn texts(v: &Value) -> Vec<&str> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|r| r["text"].as_str().unwrap())
            .collect()
    }
    #[test]
    fn outcome_with_reply_view_table() {
        let t = tempfile::tempdir().unwrap();
        let store = fixtures::store(t.path());
        let j = fixtures::job();
        let id = JobId::parse(&j.id).unwrap();
        store.create(&j).unwrap();
        let (r1, r2) = (reply(&j, "r1", "one", false), reply(&j, "r2", "two", true));
        for r in [&r1, &r2] {
            store.append_reply(&id, r).unwrap();
        }
        let ctx = ctx(t.path(), false);
        let mut s = store.read_status(&id).unwrap();
        s.status = State::Accepted;
        let fresh = ReplyView {
            seen: HashSet::new(),
            after: None,
        };
        let after_r1 = ReplyView {
            seen: HashSet::from(["r1".to_owned()]),
            after: Some("r1".into()),
        };
        // A reply wake: exit 10, woke, the woken replies and a next that continues after them.
        let o = outcome_with(
            &store,
            &j,
            Waited::Replied(s.clone(), vec![r1.clone(), r2.clone()]),
            &ctx,
            Some(&fresh),
        )
        .unwrap();
        assert_eq!(o.exit.exit_code(), 10);
        assert_eq!(o.data["woke"], "reply");
        assert_eq!(o.data["status"], "running");
        assert_eq!(o.data["interrupted"], false);
        assert_eq!(o.data["job"]["task_state"], "working");
        assert_eq!(texts(&o.data["replies"]), ["one", "two"]);
        assert_eq!(
            o.data["next"],
            format!("glen wait {} --for reply --after r2", j.handle)
        );
        // Bound expiry and SIGINT: no woke, no replies, next keeps the caller's --after.
        for (waited, code, interrupted) in [
            (Waited::Running(s.clone()), 10, false),
            (Waited::Interrupted(s.clone()), 130, true),
        ] {
            let o = outcome_with(&store, &j, waited, &ctx, Some(&after_r1)).unwrap();
            assert_eq!(o.exit.exit_code(), code);
            assert!(o.data.get("woke").is_none(), "{}", o.data);
            assert_eq!(o.data["replies"], json!([]));
            assert_eq!(o.data["interrupted"], interrupted);
            assert_eq!(
                o.data["next"],
                format!("glen wait {} --for reply --after r1", j.handle)
            );
        }
        let o = outcome_with(&store, &j, Waited::Running(s.clone()), &ctx, Some(&fresh)).unwrap();
        assert_eq!(
            o.data["next"],
            format!("glen wait {} --for reply", j.handle)
        );
        // Without a view a Replied wake reads as Running: the plain document.
        let o = outcome(
            &store,
            &j,
            Waited::Replied(s.clone(), vec![r1.clone()]),
            &ctx,
        )
        .unwrap();
        assert_eq!(o.exit.exit_code(), 10);
        assert!(o.data.get("woke").is_none() && o.data.get("replies").is_none());
        assert_eq!(o.data["next"], format!("glen wait {}", j.handle));
        // Completed (and forked): data.replies is only the unseen subset.
        store
            .write_result(
                &id,
                &JobResult {
                    v: 1,
                    prompt_status: Some("completed".into()),
                    agent_invoked: true,
                    assistant_text: Some("pong".into()),
                    replies: vec![r1.clone(), r2.clone()],
                    command_output: vec![],
                    stop_reason: Some("stop".into()),
                    session_settled: true,
                    settled_at: None,
                },
            )
            .unwrap();
        for forked in [false, true] {
            let mut done = s.clone();
            done.status = State::Completed;
            done.forked = forked.then(|| Forked {
                from: "from".into(),
                to: "01a00000-0000-7000-8000-000001234567".into(),
            });
            let o = outcome_with(
                &store,
                &j,
                Waited::Terminal(done.clone()),
                &ctx,
                Some(&after_r1),
            )
            .unwrap();
            assert_eq!(o.exit.exit_code(), if forked { 11 } else { 0 });
            assert_eq!(texts(&o.data["replies"]), ["two"]);
            assert_eq!(o.data["job"]["task_state"], "completed");
            assert!(o.data.get("woke").is_none() && o.data.get("next").is_none());
            let plain = outcome(&store, &j, Waited::Terminal(done), &ctx).unwrap();
            assert_eq!(texts(&plain.data["replies"]), ["one", "two"]);
        }
        // Failed, aborted and lost: today's envelope plus error.details.replies (unseen).
        for (state, code, exit, task_state) in [
            (State::Failed, "no_route", 6, "rejected"),
            (State::Failed, "omp_error", 7, "failed"),
            (State::Aborted, "cancelled", 7, "canceled"),
            (State::Lost, "lost", 7, "failed"),
        ] {
            let mut end = s.clone();
            end.status = state;
            end.error = Some(JobError {
                code: code.into(),
                message: "m".into(),
                hint: None,
                details: None,
            });
            for (view, want) in [(&fresh, vec!["one", "two"]), (&after_r1, vec!["two"])] {
                let e = outcome_with(&store, &j, Waited::Terminal(end.clone()), &ctx, Some(view))
                    .err()
                    .unwrap();
                assert_eq!(e.code.exit_code(), exit);
                let details = e.details.unwrap();
                assert_eq!(texts(&details["replies"]), want);
                assert_eq!(details["job"]["task_state"], task_state);
                assert_eq!(details["status"], state.as_str());
            }
            let e = outcome(&store, &j, Waited::Terminal(end), &ctx)
                .err()
                .unwrap();
            assert!(e.details.unwrap().get("replies").is_none());
        }
    }
    #[test]
    fn reply_wake_human_output_lists_replies_then_the_next_command() {
        let t = tempfile::tempdir().unwrap();
        let store = fixtures::store(t.path());
        let j = fixtures::job();
        store.create(&j).unwrap();
        let s = Status {
            status: State::Accepted,
            ..Status::default()
        };
        let view = ReplyView {
            seen: HashSet::new(),
            after: None,
        };
        let o = outcome_with(
            &store,
            &j,
            Waited::Replied(
                s,
                vec![
                    reply(&j, "r1", "half done", false),
                    reply(&j, "r2", "done", true),
                ],
            ),
            &ctx(t.path(), true),
            Some(&view),
        )
        .unwrap();
        assert_eq!(
            o.human.lines().collect::<Vec<_>>(),
            [
                "↩ half done".to_owned(),
                "↩ done ✓".to_owned(),
                format!("◐ running · {} · e27c0de", j.handle),
                format!("glen wait {} --for reply --after r2", j.handle),
            ]
        );
    }
    #[test]
    fn unknown_reply_is_a_usage_error_with_its_reason() {
        let e = unknown_reply("j1234567", "nope");
        assert_eq!(e.code, ErrorCode::Usage);
        assert_eq!(e.code.exit_code(), 2);
        assert_eq!(
            *e.details.unwrap(),
            json!({"reason":"unknown_reply","job":"j1234567","after":"nope"})
        );
        assert_eq!(
            e.hint.as_deref(),
            Some("run `glen wait j1234567 --for reply` without --after")
        );
    }
}

#[cfg(test)]
mod live_progress_tests {
    use super::*;
    #[test]
    fn live_progress_describes_delivery_without_an_omp_child() {
        let mut job = crate::jobs::tests::job();
        job.route = Route::Live;
        let mut status = Status::default();
        assert!(job_progress(&job, &status).contains("(live)"));
        status.status = State::Accepted;
        status.delivered_as = Some("follow_up".into());
        assert_eq!(
            job_progress(&job, &status),
            "delivered as follow_up; omp is working"
        );
    }
}

#[cfg(test)]
mod shared_tests {
    use super::*;
    #[test]
    fn summaries_and_errors_keep_session_elapsed_and_diagnostics() {
        let tree = tempfile::tempdir().unwrap();
        let store = crate::jobs::tests::store(tree.path());
        let mut job = crate::jobs::tests::job();
        job.created_at = "2026-10-30T00:00:00Z".into();
        let mut s = Status {
            status: State::Completed,
            updated_at: "2026-10-30T00:01:05Z".into(),
            ..Status::default()
        };
        let now = "2026-10-30T01:00:00Z".parse().unwrap();
        assert_eq!(
            completed_summary(&job, &s, now),
            format!("✓ completed · {} · e27c0de · 1m 05s", job.handle)
        );
        job.route = Route::New;
        s.actual_session_id = Some("01a00000-0000-7000-8000-000001234567".into());
        assert!(completed_summary(&job, &s, now).contains(" · 1234567 · "));
        s.status = State::Lost;
        s.warnings.push(crate::error::Warning {
            code: "test".into(),
            message: "warning".into(),
        });
        let e = job_error(&store, &job, &s).unwrap();
        assert_eq!(e.code, ErrorCode::JobFailed);
        assert_eq!(e.message, format!("job {} lost: no result", job.handle));
        assert!(e.hint.unwrap().ends_with("/worker.log"));
        assert_eq!(e.warnings.len(), 1);
        job.id = "broken".into();
        assert_eq!(
            job_error(&store, &job, &s).unwrap_err().code,
            ErrorCode::Internal
        );
    }
}
