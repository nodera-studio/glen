use crate::{
    cli::{Cli, JobsArgs},
    env::Env,
    error::GlenError,
    glen_home,
    jobs::{JobStore, State},
    output::{Outcome, OutputCtx},
    render::{Table, palette::Token, text},
    rpc::session_handle,
};
use serde_json::json;
use std::time::SystemTime;
pub fn run(_cli: &Cli, args: &JobsArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    let store = JobStore::new(glen_home::resolve_trusted(env, &["jobs"])?);
    let now = jiff::Timestamp::now();
    let (rows, warnings) = store.for_session_reporting(args.session.as_deref(), args.active);
    let mut rows: Vec<_> = rows
        .into_iter()
        .map(|(_, job, status)| (job, status))
        .collect();
    let total = rows.len();
    let active = rows.iter().filter(|(_, s)| !s.status.is_terminal()).count();
    if args.limit > 0 {
        rows.truncate(args.limit);
    }
    let data = rows
        .iter()
        .map(|(job, s)| {
            let mut v = super::wait::job_view(job, s, now);
            let preview: String = job
                .text
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .filter(|c| !c.is_control())
                .take(80)
                .collect();
            v["text_preview"] = json!(preview);
            v["error"] = json!(s.error);
            v["forked"] = forked_view(s);
            v
        })
        .collect::<Vec<_>>();
    let human = if rows.is_empty() {
        "no glen jobs yet".into()
    } else {
        let plain = rows
            .iter()
            .map(|(j, s)| {
                vec![
                    j.handle.clone(),
                    match s.status {
                        State::Queued => "○ queued".into(),
                        State::Accepted => "◐ running".into(),
                        State::Completed => {
                            if let Some(f) = &s.forked {
                                format!("✓ forked → {}", session_handle(&f.to))
                            } else {
                                "✓ completed".into()
                            }
                        }
                        State::Aborted => "! aborted".into(),
                        State::Failed => "✗ failed".into(),
                        State::Lost => "✗ lost".into(),
                    },
                    j.session_handle
                        .clone()
                        .map(|h| {
                            if j.route == crate::jobs::Route::Live {
                                format!("● {h}")
                            } else {
                                h
                            }
                        })
                        .unwrap_or_else(|| {
                            format!(
                                "@new → {}",
                                s.actual_session_id
                                    .as_deref()
                                    .map(session_handle)
                                    .unwrap_or_default()
                            )
                        }),
                    text::age(
                        SystemTime::now(),
                        j.created_at
                            .parse::<jiff::Timestamp>()
                            .ok()
                            .map(SystemTime::from)
                            .unwrap_or(SystemTime::now()),
                    ),
                    text::single_line(j.text.lines().next().unwrap_or("")),
                ]
            })
            .collect::<Vec<_>>();
        let labels = ["job", "status", "session", "age", "message"];
        let mut budgets: Vec<_> = (0..5)
            .map(|i| {
                plain
                    .iter()
                    .map(|r| text::width(&r[i]))
                    .max()
                    .unwrap_or(0)
                    .max(labels[i].len())
            })
            .collect();
        let mut columns = vec![0, 1, 2, 3, 4];
        if let Some(w) = ctx.width {
            while columns.iter().map(|i| budgets[*i]).sum::<usize>()
                + 2 * columns.len().saturating_sub(1)
                > usize::from(w)
            {
                if let Some(pos) = [4, 3, 1]
                    .iter()
                    .find_map(|drop| columns.iter().position(|i| i == drop))
                {
                    columns.remove(pos);
                } else {
                    // Keep both handles intact; wrap session prose if even the two columns
                    // cannot fit side by side at their natural widths.
                    budgets[2] = usize::from(w).saturating_sub(budgets[0] + 2).max(1);
                    break;
                }
            }
        }
        let mut table = Table::new(&ctx.painter, None);
        let headers: Vec<_> = columns
            .iter()
            .map(|i| text::truncate(labels[*i], budgets[*i]))
            .collect();
        table.header(&headers.iter().map(String::as_str).collect::<Vec<_>>());
        for ((job, s), r) in rows.iter().zip(plain) {
            table.row(
                columns
                    .iter()
                    .copied()
                    .map(|i| {
                        let v = &r[i];
                        ctx.painter.paint(
                            match i {
                                0 => Token::Focus,
                                2 if job.route == crate::jobs::Route::Live => Token::Focus,
                                1 => match s.status {
                                    State::Failed | State::Lost => Token::Danger,
                                    State::Aborted => Token::Warning,
                                    State::Completed if s.forked.is_some() => Token::Warning,
                                    State::Completed => Token::Success,
                                    State::Accepted => Token::Focus,
                                    _ => Token::Muted,
                                },
                                _ => Token::Muted,
                            },
                            &if i == 2 {
                                text::wrap(v, Some(budgets[2])).join("\n")
                            } else {
                                text::truncate(v, budgets[i])
                            },
                        )
                    })
                    .collect(),
            );
        }
        let footer = format!(
            "{total} jobs · {active} active{}",
            if rows.len() < total {
                format!(" · showing {}; -n 0 for all", rows.len())
            } else {
                String::new()
            }
        );
        format!(
            "{}\n\n{}",
            table.render(),
            text::wrap(&footer, ctx.width.map(usize::from))
                .iter()
                .map(|s| ctx.painter.paint(Token::Muted, s))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let mut outcome = Outcome::new(json!({"total":total,"jobs":data}), human);
    outcome.warnings = warnings;
    Ok(outcome)
}
/// The same `{from,to,to_handle}` shape as `glen wait` (W8-DOC-10).
fn forked_view(s: &crate::jobs::Status) -> serde_json::Value {
    let forked = s.forked.as_ref();
    json!(forked.map(|f| json!({"from":f.from,"to":f.to,"to_handle":session_handle(&f.to)})))
}
#[cfg(test)]
mod tests {
    use crate::jobs::matches_session;
    #[test]
    fn filters() {
        for (s, want) in [
            ("C0DE", true),
            ("de", false),
            ("x", false),
            ("01a0-ffff-c0de", true),
        ] {
            assert_eq!(matches_session(Some("01a0-ffff-c0de"), s), want);
        }
    }
}
