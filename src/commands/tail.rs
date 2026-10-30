use super::{ls::session_json, open};
use crate::{
    cli::{Cli, TailArgs, Tools},
    env::Env,
    error::{ErrorCode, GlenError},
    omp::{
        continuation,
        items::{self, Item, ItemOpts},
        listing::{self, SessionFile, Summary},
        session::{LoadError, Session},
    },
    output::{self, Mode, Outcome, OutputCtx, stream::EventStream},
    render::{
        palette::Token,
        transcript::{self, Style},
    },
};
use serde_json::{Map, Value, json};
use std::{
    collections::HashSet,
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
const POLL: Duration = Duration::from_millis(250);
pub fn run(cli: &Cli, args: &TailArgs, env: &Env, ctx: &OutputCtx) -> Result<Outcome, GlenError> {
    let o = open::open(cli, env, &args.session)?;
    let mut items = items::build(
        &o.session.active_branch(),
        &ItemOpts {
            thinking: args.thinking,
            tools: Tools::Collapsed,
        },
    );
    let start = items.len().saturating_sub(args.lines);
    items.drain(..start);
    if ctx.mode == Mode::Json {
        Ok(Outcome::new(
            json!({"session":session_json(&o.summary,&o.handle,&o.lease,o.state,SystemTime::now()),"continued_from":o.continued_from,"leaf_id":o.session.leaf_id(),"skipped_lines":o.session.skipped_lines,"items":items}),
            "",
        ))
    } else {
        let st = style(args, ctx);
        Ok(Outcome::new(
            Value::Null,
            format!(
                "{}\n\n{}",
                transcript::render_header(&open::header(&o), env, &st, true),
                transcript::render_items(&items, &st)
            ),
        ))
    }
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
fn identity(m: &Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (m.dev(), m.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        (0, 0)
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
struct Follower {
    session: Session,
    summary: Summary,
    offset: u64,
    identity: (u64, u64),
    emitted: HashSet<String>,
    last_on_branch: Option<String>,
    prev_model: Option<String>,
    entries: continuation::EntryCache,
    reload_pending: bool,
}
impl Follower {
    fn unseen(
        &mut self,
        stream: &mut EventStream,
        args: &TailArgs,
        ctx: &OutputCtx,
    ) -> std::io::Result<()> {
        let branch = self.session.active_branch();
        let new = branch
            .iter()
            .filter(|e| !self.emitted.contains(&e.id))
            .copied()
            .collect::<Vec<_>>();
        for i in items::build(
            &new,
            &ItemOpts {
                thinking: args.thinking,
                tools: Tools::Collapsed,
            },
        ) {
            emit_item(stream, &i, &mut self.prev_model, &style(args, ctx))?
        }
        self.emitted.extend(branch.iter().map(|e| e.id.clone()));
        self.last_on_branch = self.session.leaf_id().map(str::to_owned);
        Ok(())
    }
    fn continued(
        &mut self,
        stream: &mut EventStream,
        args: &TailArgs,
        ctx: &OutputCtx,
    ) -> Result<bool, GlenError> {
        let c =
            continuation::follow_known(&self.session.path, Some(&self.summary), &mut self.entries)?;
        if c.path == self.session.path {
            return Ok(false);
        }
        let (session, metadata) = match open::load_snapshot(&c.path, true) {
            Ok(snapshot) => snapshot,
            Err(e) if e.code == ErrorCode::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        let file = SessionFile::from_metadata(&c.path, &metadata)
            .map_err(|e| GlenError::read(&c.path, e))?;
        let summary = listing::summarize(file, &session);
        let handle = crate::omp::handle::assign([session.header.id.as_str()])
            .remove(&session.header.id)
            .unwrap_or_else(|| session.header.id.clone());
        stream.event(
            "continued",
            fields(json!({"from":self.session.header.id,"to":session.header.id,"path":c.path})),
        )?;
        stream.text(&ctx.painter.paint(
            Token::Muted,
            &format!("── continued in {} ──", crate::render::sanitize(&handle)),
        ))?;
        self.identity = identity(&metadata);
        self.offset = session.consumed;
        self.session = session;
        self.summary = summary;
        self.reload_pending = false;
        self.unseen(stream, args, ctx)?;
        Ok(true)
    }
    fn tick(
        &mut self,
        stream: &mut EventStream,
        args: &TailArgs,
        ctx: &OutputCtx,
        file: &mut File,
        m: &Metadata,
    ) -> Result<bool, GlenError> {
        if self.reload_pending || m.len() < self.offset || identity(m) != self.identity {
            let session = match open::snapshot(&self.session.path, file, m, true) {
                Ok(session) if session.header.id == self.session.header.id => session,
                Ok(_) | Err(LoadError::NotSession) => {
                    self.reload_pending = true;
                    return Ok(false);
                }
                Err(e) => return Err(open::load_error(&self.session.path, e)),
            };
            self.session = session;
            self.reload_pending = false;
            self.identity = identity(m);
            self.offset = self.session.consumed;
            stream.event("rewritten", fields(json!({"path":self.session.path})))?;
            stream.text(
                &ctx.painter
                    .paint(Token::Muted, "── session file rewritten ──"),
            )?;
            self.unseen(stream, args, ctx)?;
        } else if m.len() > self.offset {
            file.seek(SeekFrom::Start(self.offset))
                .map_err(|e| GlenError::read(&self.session.path, e))?;
            let mut bytes = vec![];
            file.take(m.len() - self.offset)
                .read_to_end(&mut bytes)
                .map_err(|e| GlenError::read(&self.session.path, e))?;
            if let Some(end) = bytes.iter().rposition(|b| *b == b'\n') {
                self.session.append(&bytes[..=end]);
                self.offset += (end + 1) as u64;
                let branch = self.session.active_branch();
                if self
                    .last_on_branch
                    .as_deref()
                    .is_some_and(|id| !branch.iter().any(|e| e.id == id))
                {
                    stream.event(
                        "branch_switch",
                        fields(json!({"leaf_id":self.session.leaf_id()})),
                    )?;
                    stream.text(&ctx.painter.paint(Token::Muted, "── switched branch ──"))?;
                }
                self.unseen(stream, args, ctx)?;
            }
        }
        Ok(true)
    }
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
    let o = match open::open_follow(cli, env, &args.session) {
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
        stream.event("session",fields(json!({"session":session_json(&o.summary,&o.handle,&o.lease,o.state,SystemTime::now()),"continued_from":o.continued_from,"skipped_lines":o.session.skipped_lines})))?;
        stream.warnings(&ctx.warnings)?;
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
        return ExitCode::from(1);
    }
    let branch = o.session.active_branch();
    let emitted = branch.iter().map(|e| e.id.clone()).collect();
    let backlog = items::build(
        &branch,
        &ItemOpts {
            thinking: args.thinking,
            tools: Tools::Collapsed,
        },
    );
    let mut prev_model = None;
    for i in backlog
        .iter()
        .skip(backlog.len().saturating_sub(args.lines))
    {
        if emit_item(&mut stream, i, &mut prev_model, &style(args, ctx)).is_err() {
            return ExitCode::from(1);
        }
    }
    let mut follower = Follower {
        offset: o.session.consumed,
        identity: identity(&o.metadata),
        last_on_branch: o.session.leaf_id().map(str::to_owned),
        session: o.session,
        summary: o.summary,
        emitted,
        prev_model,
        entries: continuation::EntryCache::default(),
        reload_pending: false,
    };
    let mut ticks = 0u8;
    let mut missing: Option<Instant> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            return stream.end(130);
        }
        thread::sleep(POLL);
        if stop.load(Ordering::Relaxed) {
            return stream.end(130);
        }
        ticks = (ticks + 1) % 8;
        let result = (|| -> Result<(), GlenError> {
            let path = follower.session.path.clone();
            let available = match File::open(&path) {
                Ok(mut file) => {
                    let metadata = file.metadata().map_err(|e| GlenError::read(&path, e))?;
                    follower.tick(&mut stream, args, ctx, &mut file, &metadata)?
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(e) => return Err(GlenError::read(&path, e)),
            };
            if !available {
                let since = missing.get_or_insert_with(Instant::now);
                if follower.continued(&mut stream, args, ctx)? {
                    missing = None;
                } else if since.elapsed() >= Duration::from_secs(5) {
                    return Err(GlenError::new(
                        ErrorCode::NotFound,
                        format!(
                            "session file disappeared or was replaced: {}",
                            path.display()
                        ),
                    ));
                }
                return Ok(());
            }
            missing = None;
            if ticks == 0 {
                follower.continued(&mut stream, args, ctx)?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            return stream.fail(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cli::GlobalArgs,
        omp::fixtures,
        output::{OutputKind, Terminal},
    };
    use std::{fs, io::Write};
    #[test]
    fn incremental_read_uses_open_fd_after_path_is_replaced() {
        let tree = fixtures::tree();
        let path = fixtures::path(&tree, 'A');
        let (session, metadata) = open::load_snapshot(&path, true).unwrap();
        let summary = listing::summarize(
            SessionFile::from_metadata(&path, &metadata).unwrap(),
            &session,
        );
        let mut follower = Follower {
            offset: session.consumed,
            identity: identity(&metadata),
            emitted: session.entries.iter().map(|e| e.id.clone()).collect(),
            last_on_branch: session.leaf_id().map(str::to_owned),
            session,
            summary,
            prev_model: None,
            entries: continuation::EntryCache::default(),
            reload_pending: false,
        };
        let line = concat!(
            "{\"type\":\"message\",\"id\":\"old-fd-entry\",\"parentId\":\"10000009\",",
            "\"message\":{\"role\":\"user\",\"content\":\"same descriptor\"}}\n"
        );
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(line.as_bytes())
            .unwrap();
        // Model the exact rename window: open first, replace the pathname,
        // then fstat and consume the still-open original file.
        let mut fd = File::open(&path).unwrap();
        let replacement = path.with_file_name(".new.tmp");
        fs::write(&replacement, fixtures::bytes('G')).unwrap();
        fs::rename(&replacement, &path).unwrap();
        let metadata = fd.metadata().unwrap();
        let env = Env::for_test(Some(tree.path()), tree.path(), &[]);
        let ctx = OutputCtx::new(
            &GlobalArgs::default(),
            OutputKind::Document,
            &env,
            &Terminal {
                stdout_tty: false,
                stderr_tty: false,
                stdout_cols: None,
            },
        )
        .unwrap();
        let mut stream = EventStream::with_writers(
            true,
            Box::new(Vec::<u8>::new()),
            Box::new(Vec::<u8>::new()),
            ctx.painter.clone(),
        );
        let args = TailArgs {
            session: path.to_string_lossy().into(),
            lines: 0,
            follow: true,
            thinking: false,
        };
        assert!(
            follower
                .tick(&mut stream, &args, &ctx, &mut fd, &metadata)
                .unwrap()
        );
        assert_eq!(follower.session.leaf_id(), Some("old-fd-entry"));
        assert!(follower.session.header.id.ends_with("e27c0de"));
        let mut replacement = File::open(&path).unwrap();
        let metadata = replacement.metadata().unwrap();
        assert!(
            !follower
                .tick(&mut stream, &args, &ctx, &mut replacement, &metadata)
                .unwrap()
        );
        assert_eq!(follower.session.leaf_id(), Some("old-fd-entry"));
    }
}
