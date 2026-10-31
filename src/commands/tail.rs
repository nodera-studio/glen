use super::{
    follow::{FollowEvent, Follower},
    open,
};
use crate::{
    cli::{Cli, TailArgs, Tools},
    env::Env,
    error::{ErrorCode, GlenError},
    omp::items::{self, Item, ItemOpts},
    output::{self, Mode, Outcome, OutputCtx, stream::EventStream},
    render::{
        palette::Token,
        transcript::{self, Style},
    },
};
use serde_json::{Map, Value, json};
use std::{
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
const POLL: Duration = Duration::from_millis(250);
pub fn run(cli: &Cli, args: &TailArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    let o = open::open_in(cli, env, &args.session, args.cwd.as_deref())?;
    let mut items = items::build(
        &o.session.active_branch(),
        &ItemOpts {
            thinking: args.thinking,
            tools: Tools::Collapsed,
        },
    );
    let start = items.len().saturating_sub(args.lines);
    items.drain(..start);
    let mut outcome = if ctx.mode == Mode::Json {
        Outcome::new(
            json!({"session":open::session_json(&o),"continued_from":o.continued_from,"leaf_id":o.session.leaf_id(),"skipped_lines":o.session.skipped_lines,"items":items}),
            "",
        )
    } else {
        let st = style(args, ctx);
        Outcome::new(
            Value::Null,
            format!(
                "{}\n\n{}",
                transcript::render_header(&open::header(&o), env, &st, true),
                transcript::render_items(&items, &st)
            ),
        )
    };
    if o.not_yet_persisted {
        outcome.warnings.push(crate::error::Warning {
            code: "not_yet_persisted".into(),
            message: "this live session has not written a transcript yet".into(),
        });
    }
    Ok(outcome)
}
fn style<'a>(args: &TailArgs, ctx: &'a OutputCtx) -> Style<'a> {
    Style {
        painter: &ctx.painter,
        width: ctx.width,
        tools: Tools::Collapsed,
        thinking: args.thinking,
    }
}
fn fields(v: Value) -> Map<String, Value> {
    if let Value::Object(m) = v {
        m
    } else {
        Map::new()
    }
}
fn emit_item(
    stream: &mut EventStream,
    item: &Item,
    prev: &mut Option<String>,
    st: &Style<'_>,
) -> std::io::Result<()> {
    stream.event("item", fields(json!({"item":item})))?;
    stream.text(&format!("\n{}", transcript::render_item(item, prev, st)))
}
#[cfg(unix)]
struct Signals(Vec<signal_hook::SigId>);
#[cfg(unix)]
impl Drop for Signals {
    fn drop(&mut self) {
        for id in self.0.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}
pub fn follow(cli: &Cli, args: &TailArgs, env: &Env, ctx: &OutputCtx) -> ExitCode {
    let mut o = match open::open_follow_in(cli, env, &args.session, args.cwd.as_deref()) {
        Ok(o) => o,
        Err(e) => return output::emit_error(ctx, e),
    };
    let mut stream = EventStream::new(ctx);
    let stop = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    let mut signals = Signals(vec![]);
    #[cfg(unix)]
    {
        use signal_hook::{consts::SIGINT, flag};
        match flag::register_conditional_shutdown(SIGINT, 130, stop.clone()) {
            Ok(id) => signals.0.push(id),
            Err(e) => return stream.fail(e.into()),
        };
        match flag::register(SIGINT, stop.clone()) {
            Ok(id) => signals.0.push(id),
            Err(e) => return stream.fail(e.into()),
        };
    }
    let init = (|| -> std::io::Result<()> {
        stream.event("session",fields(json!({"session":open::session_json(&o),"continued_from":o.continued_from,"skipped_lines":o.session.skipped_lines})))?;
        stream.warnings(&ctx.warnings)?;
        if o.not_yet_persisted {
            stream.warnings(&[crate::error::Warning {
                code: "not_yet_persisted".into(),
                message: "this live session has not written a transcript yet".into(),
            }])?;
        }
        stream.text(&transcript::render_header(
            &open::header(&o),
            env,
            &style(args, ctx),
            true,
        ))?;
        if ctx.mode == Mode::Human && ctx.stderr_tty {
            stream.diagnostic(&format!(
                "following {} · Ctrl-C to stop",
                crate::render::sanitize(&o.handle)
            ))?
        }
        Ok(())
    })();
    if init.is_err() {
        return stream.write_exit();
    }
    let mut endpoint_missing: Option<Instant> = None;
    while o.not_yet_persisted {
        if stop.load(Ordering::Relaxed) {
            return stream.end(130);
        }
        thread::sleep(POLL);
        match open::open_unprobed(cli, env, &o.session.header.id) {
            Ok(next) if !next.not_yet_persisted => o = next,
            Ok(_) => endpoint_missing = None,
            Err(e) if e.code == ErrorCode::NotFound => {
                if endpoint_missing.get_or_insert_with(Instant::now).elapsed()
                    >= Duration::from_secs(5)
                {
                    return stream.fail(GlenError::new(
                        ErrorCode::NotFound,
                        format!(
                            "live session {} disappeared before writing a transcript",
                            o.session.header.id
                        ),
                    ));
                }
            }
            Err(e) => return stream.fail(e),
        }
    }
    let mut follower = Follower::new(
        o,
        ItemOpts {
            thinking: args.thinking,
            tools: Tools::Collapsed,
        },
    );
    let backlog = follower.backlog(args.lines);
    let mut prev_model = None;
    for i in &backlog {
        if emit_item(&mut stream, i, &mut prev_model, &style(args, ctx)).is_err() {
            return stream.write_exit();
        }
    }
    loop {
        if stop.load(Ordering::Relaxed) {
            return stream.end(130);
        }
        thread::sleep(POLL);
        if stop.load(Ordering::Relaxed) {
            return stream.end(130);
        }
        let events = match follower.poll() {
            Ok(events) => events,
            Err(e) => return stream.fail(e),
        };
        let result = (|| -> std::io::Result<()> {
            for ev in events {
                match ev {
                    FollowEvent::Items(items) => {
                        for item in items {
                            emit_item(&mut stream, &item, &mut prev_model, &style(args, ctx))?;
                        }
                    }
                    FollowEvent::Continued {
                        from,
                        to,
                        path,
                        handle,
                    } => {
                        stream.event(
                            "continued",
                            fields(json!({"from":from,"to":to,"path":path})),
                        )?;
                        stream.text(&ctx.painter.paint(
                            Token::Muted,
                            &format!("── continued in {} ──", crate::render::sanitize(&handle)),
                        ))?;
                    }
                    FollowEvent::Rewritten { path } => {
                        stream.event("rewritten", fields(json!({"path":path})))?;
                        stream.text(
                            &ctx.painter
                                .paint(Token::Muted, "── session file rewritten ──"),
                        )?;
                    }
                    FollowEvent::BranchSwitch { leaf_id } => {
                        stream.event("branch_switch", fields(json!({"leaf_id":leaf_id})))?;
                        stream.text(&ctx.painter.paint(Token::Muted, "── switched branch ──"))?;
                    }
                }
            }
            Ok(())
        })();
        if result.is_err() {
            return stream.write_exit();
        }
    }
}
