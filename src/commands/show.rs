use super::{ls::session_json, open};
use crate::{
    cli::{Cli, ShowArgs},
    env::Env,
    error::GlenError,
    omp::items::{self, ItemOpts},
    output::{Mode, Outcome, OutputCtx},
    render::{
        self,
        palette::Token,
        spinner::Spinner,
        transcript::{self, Style},
    },
};
use serde_json::{Value, json};
use std::time::SystemTime;
pub fn run(cli: &Cli, args: &ShowArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    let spinner = Spinner::start(ctx.mode == Mode::Human && ctx.stderr_tty, "reading session");
    let o = open::open(cli, env, &args.session)?;
    let branch = o.session.active_branch();
    let mut data = Value::Null;
    let mut human = String::new();
    if ctx.mode == Mode::Json {
        data = json!({"session":session_json(&o.summary,&o.handle,&o.lease,o.state,SystemTime::now()),"continued_from":o.continued_from,"leaf_id":o.session.leaf_id(),"skipped_lines":o.session.skipped_lines});
        if args.raw {
            data["entries"] = json!(items::build_raw(&branch));
        } else {
            data["items"] = json!(items::build(
                &branch,
                &ItemOpts {
                    thinking: args.thinking,
                    tools: args.tools
                }
            ));
        }
    } else {
        let st = Style {
            painter: &ctx.painter,
            width: ctx.width,
            tools: args.tools,
            thinking: args.thinking,
        };
        let mut h = open::header(&o);
        let from = o
            .continued_from
            .iter()
            .map(|id| o.catalog.handle(id))
            .collect::<Vec<_>>();
        h.continued = &from;
        let body = if args.raw {
            items::build_raw(&branch)
                .iter()
                .map(|v| render::sanitize(&v.to_string()).into_owned())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            transcript::render_items(
                &items::build(
                    &branch,
                    &ItemOpts {
                        thinking: args.thinking,
                        tools: args.tools,
                    },
                ),
                &st,
            )
        };
        human = format!(
            "{}\n\n{body}",
            transcript::render_header(&h, env, &st, false)
        );
        if o.session.skipped_lines > 0 {
            human.push_str(&format!(
                "\n\n{}",
                ctx.painter.paint(
                    Token::Muted,
                    &format!("{} malformed lines skipped", o.session.skipped_lines)
                )
            ));
        }
    }
    spinner.finish();
    let mut outcome = Outcome::new(data, human);
    if o.not_yet_persisted {
        outcome.warnings.push(crate::error::Warning {
            code: "not_yet_persisted".into(),
            message: "this live session has not written a transcript yet".into(),
        });
    }
    Ok(outcome)
}
