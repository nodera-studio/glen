use crate::{
    cli::{Cli, InboxArgs},
    env::Env,
    error::{ErrorCode, GlenError, OkExit, Warning},
    glen_home::{self, GlenHome},
    inbox::{self, Cursor},
    jobs::Reply,
    omp::{bucket::bucket_for, handle, listing::Catalog, paths},
    output::{Outcome, OutputCtx, stream::EventStream},
    render::{self, palette::Token, text},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
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
    outside_cwd: bool,
}
struct Scope {
    cwd: PathBuf,
    bucket: String,
    sessions_dir: PathBuf,
    included: HashSet<String>,
    classified: HashSet<String>,
    selector: Option<String>,
    matched: Option<HashSet<String>>,
    last_refresh: std::time::Instant,
}
/// Below this, a stem not yet in `classified` (an orphan reply, or a session not persisted
/// yet) does not force another full catalog rescan: AC-1011 already keeps it eligible for
/// the next one, and a few seconds' delay before a brand-new session's first reply is
/// classified is an acceptable trade against rescanning every 250ms poll forever (MED-006).
const RESCAN_COOLDOWN: Duration = Duration::from_secs(3);
impl Scope {
    fn new(
        cli: &Cli,
        env: &Env,
        home: &GlenHome,
        dir: &Path,
        selector: Option<&str>,
    ) -> Result<Self, GlenError> {
        let cwd = paths::absolute(env, &dir.to_string_lossy());
        let mut scope = Self {
            bucket: bucket_for(&cwd, env),
            cwd,
            sessions_dir: paths::resolve(env, cli.global.profile.as_deref())?.sessions_dir,
            included: HashSet::new(),
            classified: HashSet::new(),
            selector: selector.map(str::to_owned),
            matched: None,
            last_refresh: std::time::Instant::now(),
        };
        scope.refresh(home, env)?;
        Ok(scope)
    }
    /// True once per `RESCAN_COOLDOWN`, so a caller gates a rescan at a bounded rate instead
    /// of on every poll.
    fn due_for_refresh(&self) -> bool {
        self.last_refresh.elapsed() >= RESCAN_COOLDOWN
    }
    fn refresh(&mut self, home: &GlenHome, env: &Env) -> Result<(), GlenError> {
        self.last_refresh = std::time::Instant::now();
        let mut catalog = Catalog::read(&self.sessions_dir)?;
        // An unwritten registry without a cwd cannot place a session in a project.
        // Persisted sessions still get their classification from their bucket.
        let registries = crate::live::registry::index(home)
            .into_iter()
            .filter(|registry| registry.cwd.as_deref().is_some_and(|cwd| !cwd.is_empty()))
            .collect::<Vec<_>>();
        crate::live::registry::union(&mut catalog, home, env, &registries);
        self.matched = self.selector.as_deref().map(|selector| {
            catalog.assign_handles(None);
            handle::matching_ids(selector, &catalog)
                .into_iter()
                .map(str::to_lowercase)
                .collect()
        });
        self.classified = catalog
            .sessions
            .iter()
            .map(|session| session.id.to_lowercase())
            .collect();
        self.included = catalog
            .sessions
            .iter()
            .filter(|session| session.file.bucket == self.bucket)
            .map(|session| session.id.to_lowercase())
            .collect();
        Ok(())
    }
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
    env: &Env,
    scope: Option<&mut Scope>,
) -> Result<Read, GlenError> {
    validate_filter(filter)?;
    let mut read = Read {
        records: vec![],
        ends: vec![],
        skipped: 0,
        outside_cwd: false,
    };
    let mut sessions = inbox::sessions(home, filter);
    if let Some(scope) = scope {
        let all = filter.map(|_| inbox::sessions(home, None));
        // Rescan at most once per RESCAN_COOLDOWN while any stem remains unclassified
        // (MED-006), retaining classifications for known ids. Unknown ids remain eligible
        // for a later scan if their transcript arrives.
        if scope.due_for_refresh()
            && all
                .as_deref()
                .unwrap_or(&sessions)
                .iter()
                .any(|(stem, _)| !scope.classified.contains(&stem.to_lowercase()))
        {
            scope.refresh(home, env)?;
        }
        if let (Some(all), Some(matched)) = (all, &scope.matched) {
            let mut seen = sessions
                .iter()
                .map(|(stem, _)| stem.clone())
                .collect::<HashSet<_>>();
            sessions.extend(all.into_iter().filter(|(stem, _)| {
                matched.contains(&stem.to_lowercase()) && seen.insert(stem.clone())
            }));
        }
        let had_matches = !sessions.is_empty();
        sessions.retain(|(stem, _)| scope.included.contains(&stem.to_lowercase()));
        read.outside_cwd = filter.is_some() && had_matches && sessions.is_empty();
    }
    for (stem, path) in sessions {
        let start = cursors
            .and_then(|c| c.get(&stem).copied())
            .unwrap_or_else(|| {
                if all {
                    Cursor::default()
                } else {
                    inbox::cursor(home, &stem)
                }
            });
        let batch = inbox::read_new(&path, start).map_err(|e| GlenError::read(&path, e))?;
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
/// A value as one shell word: unchanged when it holds only safe characters, else single-quoted.
fn shell_word(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}
/// A duration in the largest unit that states it exactly, as `--timeout` parses it.
fn duration_flag(d: Duration) -> String {
    let ms = d.as_millis();
    [("h", 3_600_000), ("m", 60_000), ("s", 1000)]
        .into_iter()
        .find(|(_, n)| ms != 0 && ms.is_multiple_of(*n))
        .map_or_else(
            || format!("{ms}ms"),
            |(unit, n)| format!("{}{unit}", ms / n),
        )
}
/// The command that waits again with the caller's own scope.
fn wait_again(a: &InboxArgs) -> String {
    let mut next = format!("glen inbox --wait --timeout {}", duration_flag(a.timeout));
    if let Some(session) = &a.session {
        next.push_str(&format!(" --session {}", shell_word(session)));
    }
    if let Some(cwd) = &a.cwd {
        next.push_str(&format!(" --cwd {}", shell_word(&cwd.to_string_lossy())));
    }
    if a.peek {
        next.push_str(" --peek");
    }
    next
}
/// `inbox --wait`: collect every 250 ms (the `-f` tick) until one record is unread, the bound
/// passes, or SIGINT. The first collect runs before either check, so unread messages return at
/// once even with `--timeout 0`. `Err` carries the no-message outcome (exit 10 or 130).
fn wait_for_unread(
    home: &GlenHome,
    a: &InboxArgs,
    env: &Env,
    ctx: &OutputCtx,
    mut scope: Option<&mut Scope>,
) -> Result<Result<Read, Outcome>, GlenError> {
    let stop = crate::jobs::wait::install_interrupt()?;
    let deadline = std::time::Instant::now() + a.timeout;
    loop {
        let read = collect(
            home,
            a.session.as_deref(),
            false,
            None,
            env,
            scope.as_deref_mut(),
        )?;
        if !read.records.is_empty() {
            return Ok(Ok(read));
        }
        let interrupted = stop.load(std::sync::atomic::Ordering::SeqCst);
        if interrupted || std::time::Instant::now() >= deadline {
            let next = wait_again(a);
            let mut outcome = Outcome::new(
                json!({"total":0,"marked_read":false,"skipped_lines":read.skipped,"messages":[],"next":next,"interrupted":interrupted}),
                ctx.painter
                    .paint(Token::Muted, &format!("no messages yet · {next}")),
            );
            outcome.exit = if interrupted {
                OkExit::Interrupted
            } else {
                OkExit::Running
            };
            if read.outside_cwd
                && let Some(scope) = scope
            {
                outcome.warnings.push(outside_cwd(scope));
            }
            return Ok(Err(outcome));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
fn outside_cwd(scope: &Scope) -> Warning {
    Warning {
        code: "outside_cwd".into(),
        message: format!(
            "matching inbox sessions are outside {}; drop --cwd to include them",
            scope.cwd.display()
        ),
    }
}
pub fn run(cli: &Cli, a: &InboxArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    let home = glen_home::resolve_trusted(env, &["inbox/.read"])?;
    let mut scope = a
        .cwd
        .as_deref()
        .map(|dir| Scope::new(cli, env, &home, dir, a.session.as_deref()))
        .transpose()?;
    let read = if a.wait {
        match wait_for_unread(&home, a, env, ctx, scope.as_mut())? {
            Ok(read) => read,
            Err(none) => return Ok(none),
        }
    } else {
        collect(
            &home,
            a.session.as_deref(),
            a.all,
            None,
            env,
            scope.as_mut(),
        )?
    };
    let total = read.records.len();
    // Only cursors this call actually moves are committed and counted as `marked_read`:
    // `--all` re-reads from 0, so its batch ends are often the saved cursors (W8-DOC-7).
    let moves = if a.peek {
        vec![]
    } else {
        read.ends
            .into_iter()
            .filter(|(stem, c)| {
                let saved = inbox::cursor(&home, stem);
                c.offset > 0 && (c.offset != saved.offset || c.ino != saved.ino)
            })
            .collect::<Vec<_>>()
    };
    // Probe the cursor write before printing anything, so a read-only GLEN_HOME is the one
    // error envelope (exit 6 `sandboxed` in Codex) and not `ok:true` (W8-ERR-1).
    if let Some((stem, _)) = moves.first() {
        inbox::probe_writable(&home).map_err(|e| mark_error(env, stem, e))?;
    }
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
            if a.peek {
                "peek"
            } else if moves.is_empty() {
                "already read"
            } else {
                "marked read"
            }
        )
    };
    let mut outcome = Outcome::new(
        json!({"total":total,"marked_read":!moves.is_empty(),"skipped_lines":read.skipped,"messages":read.records.iter().map(|(_,r,_,_)|message(r)).collect::<Vec<_>>()}),
        human,
    );
    if read.outside_cwd
        && let Some(scope) = &scope
    {
        outcome.warnings.push(outside_cwd(scope));
    }
    if !moves.is_empty() {
        let env = env.clone();
        outcome.after_output = Some(Box::new(move || {
            for (stem, cursor) in moves {
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
pub fn follow(cli: &Cli, a: &InboxArgs, env: &Env, ctx: &OutputCtx) -> ExitCode {
    // Follow is always NDJSON, even when explicitly invoked with --human.
    let mut stream = EventStream::ndjson(ctx);
    let result = (|| -> Result<_, GlenError> {
        let home = glen_home::resolve_trusted(env, &["inbox/.read"])?;
        let scope = a
            .cwd
            .as_deref()
            .map(|dir| Scope::new(cli, env, &home, dir, a.session.as_deref()))
            .transpose()?;
        Ok((home, crate::jobs::wait::install_interrupt()?, scope))
    })();
    let (home, stop, mut scope) = match result {
        Ok(r) => r,
        Err(e) => return stream.fail(e),
    };
    let mut cursors = HashMap::new();
    loop {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            return stream.end(130);
        }
        let read = match collect(
            &home,
            a.session.as_deref(),
            a.all,
            Some(&cursors),
            env,
            scope.as_mut(),
        ) {
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
    fn args(argv: &[&str]) -> InboxArgs {
        use clap::Parser;
        match crate::cli::Cli::try_parse_from(argv).unwrap().command {
            crate::cli::Command::Inbox(a) => a,
            _ => panic!("not inbox"),
        }
    }
    #[test]
    fn wait_again_repeats_the_callers_scope_with_one_word_per_value() {
        assert_eq!(
            wait_again(&args(&["glen", "inbox", "--wait"])),
            "glen inbox --wait --timeout 90s"
        );
        assert_eq!(
            wait_again(&args(&[
                "glen",
                "inbox",
                "--wait",
                "--timeout",
                "120s",
                "--session",
                "c0de",
                "--cwd",
                "it's here/a b",
                "--peek",
            ])),
            r"glen inbox --wait --timeout 2m --session c0de --cwd 'it'\''s here/a b' --peek"
        );
        for (d, want) in [
            (Duration::from_secs(3600), "1h"),
            (Duration::from_secs(5400), "90m"),
            (Duration::from_secs(90), "90s"),
            (Duration::from_millis(1500), "1500ms"),
            (Duration::ZERO, "0ms"),
            (Duration::from_secs(86_400), "24h"),
        ] {
            assert_eq!(duration_flag(d), want);
            assert_eq!(crate::cli::parse_duration(want).unwrap(), d, "{want}");
        }
        for (raw, want) in [
            (
                "/work/alpha-1_x@y%z+w=v:u,t./",
                "/work/alpha-1_x@y%z+w=v:u,t./",
            ),
            ("", "''"),
            ("a b", "'a b'"),
            ("$HOME", "'$HOME'"),
            ("x'y", r"'x'\''y'"),
            ("~/w", "'~/w'"),
        ] {
            assert_eq!(shell_word(raw), want);
        }
    }
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
    /// MED-006: a scope created just now is not due for another rescan, and `refresh` itself
    /// resets the cooldown, so a caller gating on `due_for_refresh` never rescans twice in a
    /// tight loop even while a stem stays unclassified.
    #[test]
    fn due_for_refresh_gates_at_the_cooldown_and_resets_on_refresh() {
        let tree = tempfile::tempdir().unwrap();
        let home = GlenHome {
            root: tree.path().join("glen"),
        };
        let sessions_dir = tree.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        let env = Env::for_test(None, tree.path(), &[]);
        let mut scope = Scope {
            bucket: "b".into(),
            cwd: tree.path().to_path_buf(),
            sessions_dir,
            included: HashSet::new(),
            classified: HashSet::new(),
            selector: None,
            matched: None,
            last_refresh: std::time::Instant::now(),
        };
        assert!(!scope.due_for_refresh(), "just refreshed, not due yet");
        scope.last_refresh -= RESCAN_COOLDOWN;
        assert!(scope.due_for_refresh());
        scope.refresh(&home, &env).unwrap();
        assert!(
            !scope.due_for_refresh(),
            "refresh() must reset the cooldown"
        );
    }
}
