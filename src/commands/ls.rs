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
use std::{collections::HashMap, path::Path, time::SystemTime};
pub fn refine_held(summary: &mut Summary, lease: &Lease) {
    if matches!(lease, Lease::Held)
        && (!summary.tail_has_message || summary.tail_redirects_leaf)
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
    source: &'static str,
    detail: Option<String>,
}
#[cfg(test)]
fn read_row(summary: Summary, owners: &Path) -> Option<Row> {
    read_endpoint_row(summary, owners, &HashMap::new())
}
fn read_endpoint_row(
    mut summary: Summary,
    owners: &Path,
    endpoints: &HashMap<String, crate::live::client::Hello>,
) -> Option<Row> {
    let endpoint = endpoints.get(&summary.id);
    if endpoint.is_some() {
        if summary.file.path.exists() {
            let _ = scan_tail(&mut summary);
        }
    } else {
        scan_tail(&mut summary).ok()?;
    }
    let RowState {
        lease,
        state,
        source,
        detail,
    } = infer_row(&mut summary, endpoint, |s| lease::probe(owners, &s.id));
    Some(Row {
        summary,
        handle: String::new(),
        lease,
        state,
        source,
        detail,
    })
}
pub(crate) struct RowState {
    pub lease: Lease,
    pub state: State,
    pub source: &'static str,
    pub detail: Option<String>,
}
pub(crate) fn infer_row(
    summary: &mut Summary,
    endpoint: Option<&crate::live::client::Hello>,
    probe: impl FnOnce(&Summary) -> Lease,
) -> RowState {
    if let Some(h) = endpoint {
        return RowState {
            lease: Lease::Unknown {
                reason: "extension".into(),
                errno: None,
            },
            state: if h.busy { State::Busy } else { State::Live },
            source: "extension",
            detail: h.mode.clone(),
        };
    }
    let lease = probe(summary);
    refine_held(summary, &lease);
    let state = lease::infer_state(&lease, summary.in_flight);
    RowState {
        lease,
        state,
        source: "disk",
        detail: None,
    }
}

#[cfg(test)]
fn read_rows(
    catalog: &Catalog,
    candidates: Vec<&Summary>,
    args: &LsArgs,
    owners: &Path,
) -> (Vec<Row>, usize, usize) {
    read_endpoint_rows(catalog, candidates, args, owners, &HashMap::new())
}
fn read_endpoint_rows(
    catalog: &Catalog,
    candidates: Vec<&Summary>,
    args: &LsArgs,
    owners: &Path,
    endpoints: &HashMap<String, crate::live::client::Hello>,
) -> (Vec<Row>, usize, usize) {
    let mut total_all = catalog.sessions.len();
    let mut total = candidates.len();
    let mut rows = vec![];
    for s in candidates {
        if !args.live && args.limit > 0 && rows.len() >= args.limit {
            break;
        }
        let Some(mut row) = read_endpoint_row(s.clone(), owners, endpoints) else {
            total = total.saturating_sub(1);
            total_all = total_all.saturating_sub(1);
            continue;
        };
        if args.live && !matches!(row.state, State::Live | State::Busy) {
            continue;
        }
        row.handle = catalog.handle(&s.id);
        rows.push(row);
    }
    if args.live {
        total = rows.len();
        if args.limit > 0 {
            rows.truncate(args.limit)
        }
    }
    (rows, total, total_all)
}
fn row_json(r: &Row, now: SystemTime) -> Value {
    let mut v = session_json(&r.summary, &r.handle, &r.lease, r.state, now);
    v["state_source"] = json!(r.source);
    if r.source == "extension" {
        v["state_detail"] = json!(r.detail);
    }
    if !r.summary.file.path.exists() {
        v["path"] = Value::Null;
    }
    v
}
pub fn run(cli: &Cli, args: &LsArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    let paths = paths::resolve(env, cli.global.profile.as_deref())?;
    let spinner = Spinner::start(
        ctx.mode == Mode::Human && ctx.stderr_tty,
        "reading sessions",
    );
    let mut catalog = Catalog::read(&paths.sessions_dir)?;
    let home = crate::glen_home::resolve(env).ok();
    let endpoints = home
        .as_ref()
        .map(crate::live::registry::states)
        .unwrap_or_default();
    if let Some(home) = &home {
        let registries = crate::live::registry::index(home)
            .into_iter()
            .filter(|r| endpoints.contains_key(&r.session_id))
            .collect::<Vec<_>>();
        crate::live::registry::union(&mut catalog, home, env, &registries);
    }
    catalog.assign_handles(None);
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
    let (rows, total, total_all) = read_endpoint_rows(
        &catalog,
        candidates,
        args,
        &paths.session_owners_dir,
        &endpoints,
    );
    spinner.finish();
    if ctx.mode == Mode::Json {
        return Ok(Outcome::new(
            json!({"scope":{"all":args.all,"cwd":(!args.all).then_some(&cwd),"bucket":bucket},"sessions_dir":catalog.sessions_dir,"total":total,"total_all":total_all,"sessions":rows.iter().map(|r|row_json(r,now)).collect::<Vec<_>>()}),
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
                    format!(
                        "\nglen ls --all lists {total_all} {} in other projects",
                        if total_all == 1 {
                            "session"
                        } else {
                            "sessions"
                        }
                    )
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
            "{total} {} · {live} live{}",
            if total == 1 { "session" } else { "sessions" },
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

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::omp::{
        fixtures,
        listing::{SessionFile, scan_head},
    };
    use serde_json::json;
    use std::fs;

    #[test]
    fn vanished_tail_row_is_skipped_while_healthy_rows_survive() {
        let tree = fixtures::tree();
        let a = fixtures::path(&tree, 'A');
        let b = fixtures::path(&tree, 'B');
        let rows = [&a, &b].map(|p| scan_head(SessionFile::at(p).unwrap()).unwrap());
        fs::remove_file(a).unwrap();
        let rows = rows
            .into_iter()
            .filter_map(|s| read_row(s, tree.path()))
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].summary.file.path, b);
    }

    #[test]
    fn vanished_tail_rows_are_excluded_from_both_totals() {
        let tree = fixtures::tree();
        let catalog = Catalog::scan(tree.path()).unwrap();
        fs::remove_file(fixtures::path(&tree, 'A')).unwrap();
        let candidates = catalog.sessions.iter().collect::<Vec<_>>();
        let args = LsArgs {
            all: true,
            cwd: None,
            limit: 0,
            live: false,
            since: None,
        };
        let (rows, total, total_all) = read_rows(&catalog, candidates, &args, tree.path());
        assert_eq!(rows.len(), 6);
        assert_eq!(total, 6);
        assert_eq!(total_all, 6);
        let args = LsArgs { live: true, ..args };
        let (rows, total, total_all) = read_rows(
            &catalog,
            catalog.sessions.iter().collect(),
            &args,
            tree.path(),
        );
        assert!(rows.is_empty());
        assert_eq!(total, 0);
        assert_eq!(total_all, 6);
    }

    #[test]
    fn held_state_follows_redirected_leaf_in_both_directions() {
        let tree = tempfile::tempdir().unwrap();
        let path = tree.path().join("branch.jsonl");
        for (leaf, busy) in [
            (json!("done"), false),
            (json!("pending"), true),
            (Value::Null, false),
            (json!("missing"), true),
        ] {
            let entries = [
                json!({"type":"session","version":3,"id":"x"}),
                json!({"type":"message","id":"done","message":{"role":"assistant","content":[]}}),
                json!({"type":"message","id":"pending","message":{"role":"user","content":"next"}}),
                json!({"type":"model_usage","id":"usage","activeLeafId":leaf}),
            ];
            fs::write(
                &path,
                entries.iter().map(|e| format!("{e}\n")).collect::<String>(),
            )
            .unwrap();
            let mut summary = scan_head(SessionFile::at(&path).unwrap()).unwrap();
            scan_tail(&mut summary).unwrap();
            assert!(summary.tail_has_message);
            assert!(summary.tail_redirects_leaf);
            refine_held(&mut summary, &Lease::Held);
            let loaded = Session::load(&path).unwrap();
            assert_eq!(
                summary.last_turn,
                TurnStatus::of_branch(&loaded.active_branch())
            );
            assert_eq!(summary.in_flight, busy);
            assert_eq!(
                lease::infer_state(&Lease::Held, summary.in_flight),
                if busy { State::Busy } else { State::Live }
            );
        }
    }

    #[test]
    fn known_message_with_unknown_status_does_not_reload() {
        let tree = tempfile::tempdir().unwrap();
        let path = tree.path().join("developer.jsonl");
        fs::write(&path, "{\"type\":\"session\",\"version\":3,\"id\":\"x\"}\n{\"type\":\"message\",\"id\":\"d\",\"message\":{\"role\":\"developer\"}}\n").unwrap();
        let mut summary = scan_head(SessionFile::at(&path).unwrap()).unwrap();
        scan_tail(&mut summary).unwrap();
        assert!(summary.tail_has_message);
        assert_eq!(summary.last_turn, TurnStatus::Unknown);
        // If refinement reloads it will see this different, completed branch.
        fs::write(&path, "{\"type\":\"session\",\"version\":3,\"id\":\"x\"}\n{\"type\":\"message\",\"id\":\"a\",\"message\":{\"role\":\"assistant\"}}\n").unwrap();
        refine_held(&mut summary, &Lease::Held);
        assert_eq!(summary.last_turn, TurnStatus::Unknown);
        assert!(summary.in_flight);
    }
}

#[cfg(test)]
mod branch_parent_tests {
    use super::*;
    #[test]
    fn trailing_entry_parent_chain_can_skip_last_physical_message() {
        let tree = tempfile::tempdir().unwrap();
        let path = tree.path().join("branch.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"session\",\"id\":\"x\",\"version\":3}\n",
                "{\"type\":\"message\",\"id\":\"done\",\"message\":{\"role\":\"assistant\"}}\n",
                "{\"type\":\"message\",\"id\":\"pending\",\"message\":{\"role\":\"user\"}}\n",
                "{\"type\":\"future\",\"id\":\"leaf\",\"parentId\":\"done\"}\n"
            ),
        )
        .unwrap();
        let mut summary =
            crate::omp::listing::scan_head(crate::omp::listing::SessionFile::at(&path).unwrap())
                .unwrap();
        scan_tail(&mut summary).unwrap();
        assert!(summary.tail_redirects_leaf);
        refine_held(&mut summary, &Lease::Held);
        assert_eq!(summary.last_turn, TurnStatus::Complete);
        assert!(!summary.in_flight);
    }
}

#[cfg(test)]
mod inference_tests {
    use super::*;
    use crate::{
        live::client::Hello,
        omp::{
            fixtures,
            listing::{SessionFile, scan_head},
        },
    };
    #[test]
    fn endpoint_state_does_not_probe_and_disk_state_probes_once() {
        let tree = fixtures::tree();
        let path = fixtures::path(&tree, 'A');
        let mut summary = scan_head(SessionFile::at(&path).unwrap()).unwrap();
        let mut endpoint = Hello {
            protocol: 1,
            ops: vec!["send".into()],
            ext_version: None,
            omp_version: None,
            pid: 0,
            session_id: summary.id.clone(),
            session_file: Some(path),
            mode: Some("tui".into()),
            state: "live".into(),
            busy: false,
            queued: false,
            active_requests: 0,
        };
        for busy in [false, true] {
            endpoint.busy = busy;
            let row = infer_row(&mut summary, Some(&endpoint), |_| {
                panic!("endpoint must not probe")
            });
            assert_eq!(row.state, if busy { State::Busy } else { State::Live });
            assert_eq!(row.source, "extension");
            assert_eq!(row.detail.as_deref(), Some("tui"));
            assert!(
                matches!(row.lease, Lease::Unknown { reason, errno: None } if reason == "extension")
            );
        }
        let mut probes = 0;
        let row = infer_row(&mut summary, None, |_| {
            probes += 1;
            Lease::Free
        });
        assert_eq!(probes, 1);
        assert_eq!(row.state, State::Idle);
        assert_eq!(row.source, "disk");
        assert!(row.detail.is_none());
    }
}
