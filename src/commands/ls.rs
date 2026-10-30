use crate::{
    cli::{Cli, LsArgs},
    env::Env,
    error::GlenError,
    omp::{
        bucket::bucket_for,
        lease::{self, Lease, State},
        listing::{Catalog, Summary, scan_tail},
        paths,
        session::{Session, TurnStatus, in_flight_of_branch},
    },
    output::{Mode, Outcome, OutputCtx},
    render::{self, Table, palette::Token, spinner::Spinner, text, transcript::state_text},
};
use serde_json::{Value, json};
use std::{path::Path, time::SystemTime};
pub fn refine_held(summary: &mut Summary, lease: &Lease) {
    if matches!(lease, Lease::Held)
        && summary.last_turn == TurnStatus::Unknown
        && let Ok(s) = Session::load(&summary.file.path)
    {
        let b = s.active_branch();
        summary.last_turn = TurnStatus::of_branch(&b);
        summary.in_flight = in_flight_of_branch(&b)
    }
}
pub fn session_json(
    s: &Summary,
    handle: &str,
    lease: &Lease,
    state: State,
    now: SystemTime,
) -> Value {
    json!({"handle":handle,"id":s.id,"path":s.file.path,"bucket":s.file.bucket,"cwd":s.cwd,"title":s.title,"first_message":s.first_message,"created_at":s.created.map(|t|t.to_string()),"updated_at":jiff::Timestamp::try_from(s.file.mtime).ok().map(|t|t.to_string()),"age_ms":now.duration_since(s.file.mtime).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64,"size_bytes":s.file.size,"model":s.model,"last_turn":s.last_turn,"state":state,"state_source":"disk","state_detail":lease::state_detail(lease),"continuation_of":s.continuation_of,"parent_session":s.parent_session})
}
struct Row {
    summary: Summary,
    handle: String,
    lease: Lease,
    state: State,
}
pub fn run(cli: &Cli, args: &LsArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    let paths = paths::resolve(env, cli.global.profile.as_deref())?;
    let spinner = Spinner::start(
        ctx.mode == Mode::Human && ctx.stderr_tty,
        "reading sessions",
    );
    let catalog = Catalog::scan(&paths.sessions_dir)?;
    let total_all = catalog.sessions.len();
    let cwd = paths::absolute(
        env,
        &args.cwd.as_deref().unwrap_or(env.cwd()).to_string_lossy(),
    );
    let bucket = (!args.all).then(|| bucket_for(&cwd, env));
    let now = SystemTime::now();
    let candidates = catalog
        .sessions
        .iter()
        .filter(|s| {
            bucket.as_ref().is_none_or(|b| *b == s.file.bucket)
                && args
                    .since
                    .is_none_or(|d| now.duration_since(s.file.mtime).unwrap_or_default() <= d)
        })
        .collect::<Vec<_>>();
    let mut total = candidates.len();
    let mut rows = vec![];
    for s in candidates {
        if !args.live && args.limit > 0 && rows.len() >= args.limit {
            break;
        }
        let mut s = s.clone();
        scan_tail(&mut s)?;
        let l = lease::probe(&paths.session_owners_dir, &s.id);
        refine_held(&mut s, &l);
        let state = lease::infer_state(&l, s.in_flight);
        if args.live && !matches!(state, State::Live | State::Busy) {
            continue;
        }
        rows.push(Row {
            handle: catalog.handle(&s.id),
            summary: s,
            lease: l,
            state,
        });
    }
    if args.live {
        total = rows.len();
        if args.limit > 0 {
            rows.truncate(args.limit)
        }
    }
    spinner.finish();
    if ctx.mode == Mode::Json {
        return Ok(Outcome::new(
            json!({"scope":{"all":args.all,"cwd":(!args.all).then_some(&cwd),"bucket":bucket},"sessions_dir":catalog.sessions_dir,"total":total,"total_all":total_all,"sessions":rows.iter().map(|r|session_json(&r.summary,&r.handle,&r.lease,r.state,now)).collect::<Vec<_>>()}),
            "",
        ));
    }
    let p = &ctx.painter;
    let human = if rows.is_empty() {
        let s = if !paths.sessions_dir.exists() {
            format!(
                "no omp sessions yet ({} does not exist)",
                render::tilde(&paths.sessions_dir, env.home())
            )
        } else {
            format!(
                "no omp sessions for {}{}",
                render::tilde(&cwd, env.home()),
                if total_all > 0 {
                    format!("\nglen ls --all lists {total_all} sessions in other projects")
                } else {
                    String::new()
                }
            )
        };
        s.lines()
            .flat_map(|l| text::wrap(l, ctx.width.map(usize::from)))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        let mut plain = vec![];
        for r in &rows {
            let s = &r.summary;
            plain.push(vec![
                text::single_line(&r.handle),
                match r.state {
                    State::Idle => "○ idle",
                    State::Live => "● live",
                    State::Busy => "◐ busy",
                    State::Unknown => "? unknown",
                }
                .to_owned(),
                text::truncate(
                    &s.cwd
                        .as_deref()
                        .map(|c| render::tilde(Path::new(c), env.home()))
                        .unwrap_or_else(|| render::sanitize(&s.file.bucket).into_owned()),
                    24,
                ),
                text::single_line(
                    &s.title
                        .clone()
                        .or_else(|| s.first_message.clone())
                        .unwrap_or_else(|| {
                            format!(
                                "untitled · {}",
                                s.created.map(|t| text::local_hm(&t)).unwrap_or_default()
                            )
                        }),
                ),
                serde_json::to_value(s.last_turn)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                text::age(now, s.file.mtime),
                text::single_line(
                    s.model
                        .as_deref()
                        .and_then(|m| m.rsplit('/').next())
                        .unwrap_or(""),
                ),
            ]);
        }
        let labels = [
            "handle", "state", "project", "title", "turn", "updated", "model",
        ];
        let widths = (0..7)
            .map(|i| {
                plain
                    .iter()
                    .map(|r| text::width(&r[i]))
                    .max()
                    .unwrap_or(0)
                    .max(labels[i].len())
            })
            .collect::<Vec<_>>();
        let (cols, budgets) = column_budget(&widths, args.all, ctx.width.map(usize::from));
        let headers = cols
            .iter()
            .map(|i| text::truncate(labels[*i], budgets[*i]))
            .collect::<Vec<_>>();
        let mut t = Table::new(p, None);
        t.header(&headers.iter().map(String::as_str).collect::<Vec<_>>());
        for (r, cells) in rows.iter().zip(&plain) {
            t.row(
                cols.iter()
                    .map(|i| {
                        let cell = text::truncate(&cells[*i], budgets[*i]);
                        match *i {
                            0 => p.paint(Token::Focus, &cell),
                            1 if cell == cells[1] => state_text(r.state, p),
                            1 => p.paint(Token::Muted, &cell),
                            4 => p.paint(
                                match r.summary.last_turn {
                                    TurnStatus::Error => Token::Danger,
                                    TurnStatus::Pending
                                    | TurnStatus::Interrupted
                                    | TurnStatus::Aborted => Token::Warning,
                                    _ => Token::Muted,
                                },
                                &cell,
                            ),
                            5 | 6 => p.paint(Token::Muted, &cell),
                            _ => cell,
                        }
                    })
                    .collect(),
            );
        }
        let live = rows
            .iter()
            .filter(|r| matches!(r.state, State::Live | State::Busy))
            .count();
        let footer = format!(
            "{total} sessions · {live} live{}",
            if rows.len() < total {
                format!(" · showing {} of {total}; -n 0 for all", rows.len())
            } else {
                String::new()
            }
        );
        format!(
            "{}\n\n{}",
            t.render(),
            text::wrap(&footer, ctx.width.map(usize::from))
                .iter()
                .map(|l| p.paint(Token::Muted, l))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    Ok(Outcome::new(Value::Null, human))
}
fn column_budget(widths: &[usize], all: bool, width: Option<usize>) -> (Vec<usize>, Vec<usize>) {
    let mut cols = vec![0, 1, 3, 4, 5, 6];
    if all {
        cols.insert(2, 2)
    }
    let mut budgets = widths.to_vec();
    if let Some(w) = width {
        for drop in [6, 4, 2] {
            let other = cols
                .iter()
                .filter(|i| **i != 3)
                .map(|i| budgets[*i])
                .sum::<usize>()
                + 2 * (cols.len() - 1);
            if w.saturating_sub(other) >= 16 {
                break;
            }
            cols.retain(|i| *i != drop);
        }
        let other = cols
            .iter()
            .filter(|i| **i != 3)
            .map(|i| budgets[*i])
            .sum::<usize>()
            + 2 * (cols.len() - 1);
        budgets[3] = w.saturating_sub(other).max(1);
        while cols.iter().map(|i| budgets[*i]).sum::<usize>() + 2 * (cols.len() - 1) > w {
            let Some(i) = cols
                .iter()
                .filter(|i| budgets[**i] > 1)
                .max_by_key(|i| budgets[**i])
            else {
                break;
            };
            budgets[*i] -= 1;
        }
    }
    (cols, budgets)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budget() {
        let (c, b) = column_budget(&[7, 9, 24, 50, 11, 10, 20], true, Some(60));
        assert!(!c.contains(&6));
        assert!(!c.contains(&4));
        assert!(!c.contains(&2));
        assert!(c.iter().map(|i| b[*i]).sum::<usize>() + 2 * (c.len() - 1) <= 60);
    }
    #[test]
    fn held_large_message() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("x.jsonl");
        std::fs::write(&p,format!("{{\"type\":\"session\",\"id\":\"x\"}}\n{{\"type\":\"message\",\"id\":\"a\",\"message\":{{\"role\":\"toolResult\",\"content\":\"{}\"}}}}\n","x".repeat(40000))).unwrap();
        let mut s =
            crate::omp::listing::scan_head(crate::omp::listing::SessionFile::at(&p).unwrap())
                .unwrap();
        scan_tail(&mut s).unwrap();
        assert_eq!(s.last_turn, TurnStatus::Unknown);
        refine_held(&mut s, &Lease::Held);
        assert_eq!(s.last_turn, TurnStatus::Interrupted);
        assert!(s.in_flight);
        s.last_turn = TurnStatus::Unknown;
        std::fs::remove_file(p).unwrap();
        refine_held(&mut s, &Lease::Free);
        assert_eq!(s.last_turn, TurnStatus::Unknown);
    }
}
