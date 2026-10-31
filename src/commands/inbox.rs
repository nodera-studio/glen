use crate::{
    cli::{Cli, InboxArgs},
    env::Env,
    error::{ErrorCode, GlenError},
    glen_home::{self, GlenHome},
    inbox::{self, Cursor},
    jobs::Reply,
    output::{Outcome, OutputCtx, stream::EventStream},
    render::{self, palette::Token, text},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    process::ExitCode,
    time::Duration,
};
/// The read cursor lives in GLEN_HOME; inside Codex's sandbox a denied write is the
/// W4 `sandboxed` refusal (docs/design.md §A13), otherwise an ordinary io error.
fn mark_error(env: &Env, stem: &str, e: std::io::Error) -> GlenError {
    if crate::sandbox::restricted(env, &e) {
        return crate::sandbox::map_home_write(env, "inbox", e);
    }
    GlenError::new(ErrorCode::Io, format!("cannot mark inbox {stem} read: {e}"))
}
fn message(r: &Reply) -> Value {
    let mut v = json!(r);
    v["session_handle"] = json!(crate::rpc::session_handle(&r.session_id));
    v["job_handle"] = json!(
        r.job_id
            .as_ref()
            .map(|id| format!("j{}", crate::rpc::session_handle(id)))
    );
    v
}
fn human(r: &Reply, ctx: &OutputCtx) -> String {
    let v = message(r);
    let age = text::age(
        std::time::SystemTime::now(),
        r.at.parse::<jiff::Timestamp>()
            .map(std::time::SystemTime::from)
            .unwrap_or(std::time::SystemTime::now()),
    );
    let heading = format!(
        "↩ {} · {age} · {}{}",
        v["session_handle"].as_str().unwrap_or("?"),
        v["job_handle"].as_str().unwrap_or("tui"),
        if r.is_final { " · ✓" } else { "" }
    );
    let lines = text::clean_lines(&r.text)
        .into_iter()
        .flat_map(|line| text::wrap(&line, ctx.width.map(|w| usize::from(w).saturating_sub(2))))
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{}\n{lines}",
        ctx.painter.paint(Token::Muted, &render::sanitize(&heading))
    )
}
struct Read {
    records: Vec<(String, Reply, u64, u64)>,
    ends: Vec<(String, Cursor)>,
    skipped: usize,
}
fn validate_filter(filter: Option<&str>) -> Result<(), GlenError> {
    if let Some(filter) = filter
        && filter.len() < 4
        && filter.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(GlenError::new(
            ErrorCode::Usage,
            "inbox --session requires a full session id or at least 4 hex digits",
        ));
    }
    Ok(())
}
fn collect(
    home: &GlenHome,
    filter: Option<&str>,
    all: bool,
    cursors: Option<&HashMap<String, Cursor>>,
) -> Result<Read, GlenError> {
    validate_filter(filter)?;
    let mut read = Read {
        records: vec![],
        ends: vec![],
        skipped: 0,
    };
    for (stem, path) in inbox::sessions(home, filter) {
        let from = cursors
            .and_then(|c| c.get(&stem).copied())
            .unwrap_or_else(|| {
                if all {
                    Cursor::default()
                } else {
                    inbox::cursor(home, &stem)
                }
            });
        let batch = inbox::read_new(&path, from).map_err(|e| GlenError::read(&path, e))?;
        read.skipped += batch.skipped;
        read.ends.push((
            stem.clone(),
            Cursor {
                offset: batch.end,
                ino: batch.ino,
            },
        ));
        read.records.extend(
            batch
                .records
                .into_iter()
                .map(|(r, o)| (stem.clone(), r, o, batch.ino)),
        );
    }
    read.records.sort_by(|a, b| {
        let timestamp = |r: &Reply| r.at.parse::<jiff::Timestamp>().ok();
        match (timestamp(&a.1), timestamp(&b.1)) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        }
        .then(a.0.cmp(&b.0))
        .then(a.2.cmp(&b.2))
    });
    Ok(read)
}
pub fn run(_cli: &Cli, a: &InboxArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    let home = glen_home::resolve(env)?;
    let read = collect(&home, a.session.as_deref(), a.all, None)?;
    let total = read.records.len();
    let sessions = read
        .records
        .iter()
        .map(|(stem, _, _, _)| stem)
        .collect::<HashSet<_>>()
        .len();
    let human = if total == 0 {
        ctx.painter.paint(Token::Muted, "no messages from omp yet")
    } else {
        format!(
            "{}\n\n{} {} · {} {} · {}",
            read.records
                .iter()
                .map(|(_, r, _, _)| human(r, ctx))
                .collect::<Vec<_>>()
                .join("\n\n"),
            total,
            if total == 1 { "message" } else { "messages" },
            sessions,
            if sessions == 1 { "session" } else { "sessions" },
            if a.peek { "peek" } else { "marked read" }
        )
    };
    let mut outcome = Outcome::new(
        json!({"total":total,"marked_read":!a.peek&&total>0,"skipped_lines":read.skipped,"messages":read.records.iter().map(|(_,r,_,_)|message(r)).collect::<Vec<_>>()}),
        human,
    );
    if !a.peek {
        let ends = read
            .ends
            .into_iter()
            .filter(|(_, c)| c.offset > 0)
            .collect::<Vec<_>>();
        let env = env.clone();
        outcome.after_output = Some(Box::new(move || {
            for (stem, cursor) in ends {
                inbox::mark(&home, &stem, cursor).map_err(|e| mark_error(&env, &stem, e))?;
            }
            Ok(())
        }));
    }
    Ok(outcome)
}
// Chronological output may visit a file out of order. Only commit the prefix
// whose earlier records have all reached stdout successfully.
#[derive(Default)]
struct PrintedPrefix {
    pending: BTreeSet<u64>,
    written: BTreeSet<u64>,
    committed: u64,
}
impl PrintedPrefix {
    fn wrote(&mut self, end: u64) -> Option<u64> {
        self.pending.remove(&end);
        self.written.insert(end);
        let safe = match self.pending.first() {
            Some(next) => self.written.range(..*next).next_back().copied(),
            None => self.written.last().copied(),
        }?;
        if safe <= self.committed {
            return None;
        }
        self.committed = safe;
        Some(safe)
    }
}
pub fn follow(_cli: &Cli, a: &InboxArgs, env: &Env, ctx: &OutputCtx) -> ExitCode {
    // Follow is always NDJSON, even when explicitly invoked with --human.
    let mut stream = EventStream::ndjson(ctx);
    let result = (|| -> Result<_, GlenError> {
        Ok((
            glen_home::resolve(env)?,
            crate::jobs::wait::install_interrupt()?,
        ))
    })();
    let (home, stop) = match result {
        Ok(r) => r,
        Err(e) => return stream.fail(e),
    };
    let mut cursors = HashMap::new();
    loop {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            return stream.end(130);
        }
        let read = match collect(&home, a.session.as_deref(), a.all, Some(&cursors)) {
            Ok(r) => r,
            Err(e) => return stream.fail(e),
        };
        // Output order is chronological; a cursor only advances through a file's
        // contiguous successful prefix, even when that file has unsorted dates.
        let mut prefixes: HashMap<String, PrintedPrefix> = HashMap::new();
        for (stem, _, end, _) in &read.records {
            prefixes
                .entry(stem.clone())
                .or_default()
                .pending
                .insert(*end);
        }
        for (stem, r, end, ino) in &read.records {
            if stream
                .event(
                    "message",
                    serde_json::Map::from_iter([("message".into(), message(r))]),
                )
                .is_err()
            {
                return stream.write_exit();
            }
            if !a.peek
                && let Some(offset) = prefixes.entry(stem.clone()).or_default().wrote(*end)
                && let Err(e) = inbox::mark(&home, stem, Cursor { offset, ino: *ino })
            {
                return stream.fail(mark_error(env, stem, e));
            }
        }
        for (stem, cursor) in read.ends {
            if !a.peek
                && cursor.offset > 0
                && let Err(e) = inbox::mark(&home, &stem, cursor)
            {
                return stream.fail(mark_error(env, &stem, e));
            }
            cursors.insert(stem, cursor);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn out_of_order_dates_never_mark_an_unprinted_line_read() {
        let mut prefix = PrintedPrefix {
            pending: BTreeSet::from([10, 20, 30]),
            ..Default::default()
        };
        assert_eq!(prefix.wrote(30), None);
        assert_eq!(prefix.wrote(10), Some(10));
        assert_eq!(prefix.wrote(20), Some(30));
    }
}
