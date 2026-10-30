use super::{Ctx, Wake, Worker, latest};
use crate::{
    commands::{
        follow::{FollowEvent, Follower},
        open,
    },
    error::{ErrorCode, GlenError},
    omp::{items::ItemOpts, listing},
    render::text,
    tui::state::{FollowMsg, Header, Msg, Target},
};
use std::{
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};
const INTERVAL: Duration = Duration::from_millis(250);
pub struct Command {
    pub r#gen: u64,
    pub target: Target,
    pub opts: ItemOpts,
}
#[derive(Default)]
struct Reader {
    follower: Option<Follower>,
    pending: Option<Command>,
    generation: u64,
    handle: String,
    pending_seen: bool,
    missing: Option<Instant>,
    #[cfg(test)]
    opens: usize,
}
impl Reader {
    fn send(&self, ctx: &Ctx, ev: FollowMsg) -> bool {
        ctx.send(Msg::Follow {
            r#gen: self.generation,
            ev,
        })
    }
    fn command(&mut self, ctx: &Ctx, command: Command) -> bool {
        self.generation = command.r#gen;
        let same = self.follower.as_ref().is_some_and(|f| {
            f.session().header.id == command.target.id
                && command
                    .target
                    .path
                    .as_ref()
                    .is_none_or(|p| p == &f.session().path)
        });
        if same {
            self.pending = None;
            if let Some(f) = &mut self.follower {
                let items = f.rebuild(command.opts);
                return self.opened(ctx, items);
            }
        }
        self.follower = None;
        self.pending = Some(command);
        self.pending_seen = false;
        self.missing = None;
        self.open_pending(ctx)
    }
    fn opened(&self, ctx: &Ctx, mut items: Vec<crate::omp::items::Item>) -> bool {
        let Some(f) = &self.follower else {
            return true;
        };
        let summary = f.summary();
        let session = f.session();
        let title = session
            .title()
            .or_else(|| summary.title.clone())
            .or_else(|| summary.first_message.clone())
            .unwrap_or_else(|| "untitled".into());
        let branch = session.active_branch();
        let model = branch
            .iter()
            .rev()
            .filter(|e| e.kind == "message" && e.raw["message"]["role"] == "assistant")
            .find_map(|e| listing::model_of(&e.raw["message"]))
            .or_else(|| {
                branch
                    .iter()
                    .rev()
                    .filter(|e| e.kind == "model_change")
                    .find_map(|e| e.raw["model"].as_str().map(str::to_owned))
            })
            .or_else(|| summary.model.clone());
        let dropped = items.len().saturating_sub(1000);
        items.drain(..dropped);
        self.send(
            ctx,
            FollowMsg::Opened {
                header: Header {
                    title: text::single_line(&title),
                    handle: crate::render::sanitize(&self.handle).into_owned(),
                    model,
                },
                items,
                dropped,
                not_yet_persisted: false,
            },
        )
    }
    fn open_pending(&mut self, ctx: &Ctx) -> bool {
        let Some(command) = &self.pending else {
            return true;
        };
        let arg = command
            .target
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| command.target.id.clone());
        #[cfg(test)]
        {
            self.opens += 1;
        }
        match open::open_watch(&ctx.cli, &ctx.env, &arg) {
            Ok(opened) if opened.not_yet_persisted => {
                self.pending_seen = true;
                self.missing = None;
                self.send(ctx, FollowMsg::NotYetPersisted)
            }
            Ok(opened) => {
                let Some(command) = self.pending.take() else {
                    return true;
                };
                let id = opened.session.header.id.clone();
                let path = opened.session.path.clone();
                let redirected = id != command.target.id;
                self.handle = opened.handle.clone();
                let follower = Follower::new(opened, command.opts);
                let items = follower.backlog(usize::MAX);
                self.follower = Some(follower);
                self.pending_seen = false;
                self.missing = None;
                if !self.opened(ctx, items) {
                    return false;
                }
                if redirected {
                    self.send(
                        ctx,
                        FollowMsg::Continued {
                            to_id: id,
                            to_path: Some(path),
                            handle: self.handle.clone(),
                        },
                    )
                } else {
                    true
                }
            }
            Err(e) if self.pending_seen && e.code == ErrorCode::NotFound => {
                if self.missing.get_or_insert_with(Instant::now).elapsed() < Duration::from_secs(5)
                {
                    return true;
                }
                self.fail(ctx, e)
            }
            Err(e) => self.fail(ctx, e),
        }
    }
    fn fail(&mut self, ctx: &Ctx, error: GlenError) -> bool {
        self.pending = None;
        self.follower = None;
        self.send(ctx, FollowMsg::Failed(error.into()))
    }
    fn tick(&mut self, ctx: &Ctx) -> bool {
        if self.pending.is_some() {
            return self.open_pending(ctx);
        }
        let Some(f) = &mut self.follower else {
            return true;
        };
        let events = match f.poll() {
            Ok(events) => events,
            Err(e) => return self.fail(ctx, e),
        };
        for event in events {
            let ev = match event {
                FollowEvent::Items(items) => FollowMsg::Items(items),
                FollowEvent::Continued {
                    to, path, handle, ..
                } => {
                    self.handle = handle.clone();
                    FollowMsg::Continued {
                        to_id: to,
                        to_path: Some(path),
                        handle,
                    }
                }
                FollowEvent::Rewritten { .. } => {
                    FollowMsg::Notice("── session file rewritten ──".into())
                }
                FollowEvent::BranchSwitch { .. } => {
                    FollowMsg::Notice("── switched branch ──".into())
                }
            };
            if !self.send(ctx, ev) {
                return false;
            }
        }
        true
    }
}
pub fn spawn(ctx: Ctx, rx_cmd: Receiver<Command>) -> Worker {
    std::thread::spawn(move || {
        let mut reader = Reader::default();
        let mut next = Instant::now() + INTERVAL;
        while !ctx.stopped() {
            if let Ok(command) = rx_cmd.try_recv() {
                if !reader.command(&ctx, latest(&rx_cmd, command)) {
                    break;
                }
                next = Instant::now() + INTERVAL;
                continue;
            }
            match ctx.wait(&rx_cmd, next) {
                Wake::Command(command) => {
                    if !reader.command(&ctx, latest(&rx_cmd, command)) {
                        break;
                    }
                    next = Instant::now() + INTERVAL;
                }
                Wake::Tick => {
                    if !reader.tick(&ctx) {
                        break;
                    }
                    next = Instant::now() + INTERVAL;
                }
                Wake::Stop => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cli::Tools,
        live::registry,
        omp::{fixtures, items::Item, session::Session},
        tui::sources::tests::{Rig, stop, wait},
    };
    use std::{fs, io::Write, sync::mpsc};
    fn command(rig: &Rig, key: char, generation: u64) -> Command {
        let path = rig.path(key);
        let id = Session::load(&path).unwrap().header.id;
        Command {
            r#gen: generation,
            target: Target {
                id,
                path: Some(path),
            },
            opts: ItemOpts {
                thinking: false,
                tools: Tools::Collapsed,
            },
        }
    }
    fn append(path: &std::path::Path, id: &str, parent: &str) {
        writeln!(fs::OpenOptions::new().append(true).open(path).unwrap(), "{}",
            serde_json::json!({"type":"message","id":id,"parentId":parent,
                "timestamp":"2026-10-30T00:00:00Z","message":{"role":"user","content":"new message"}})).unwrap();
    }
    #[test]
    fn worker_opens_and_follows_only_complete_appends() {
        let rig = Rig::new(true);
        let (ctx, rx) = rig.ctx();
        let (tx, commands) = mpsc::channel();
        let cmd = command(&rig, 'A', 7);
        let path = cmd.target.path.clone().unwrap();
        tx.send(cmd).unwrap();
        let worker = spawn(ctx.clone(), commands);
        let opened = wait(&rx, |m| {
            matches!(
                m,
                Msg::Follow {
                    ev: FollowMsg::Opened { .. },
                    ..
                }
            )
        });
        assert!(
            matches!(opened, Msg::Follow { r#gen: 7, ev: FollowMsg::Opened { items, dropped: 0, .. }} if !items.is_empty())
        );
        append(&path, "new-item", "10000009");
        let message = wait(&rx, |m| {
            matches!(
                m,
                Msg::Follow {
                    ev: FollowMsg::Items(_),
                    ..
                }
            )
        });
        assert!(
            matches!(message, Msg::Follow { r#gen: 7, ev: FollowMsg::Items(items) } if matches!(items.as_slice(), [Item::User { id, .. }] if id == "new-item"))
        );
        stop(&ctx, worker);
    }
    #[test]
    fn three_queued_commands_open_only_the_last_and_opts_rebuild_without_opening() {
        let rig = Rig::new(true);
        let (ctx, rx) = rig.ctx();
        let (tx, commands) = mpsc::channel();
        for generation in 1..=3 {
            let mut cmd = command(&rig, 'A', generation);
            if generation != 3 {
                cmd.target.path = Some(rig.temp.path().join("must-not-open"));
            }
            tx.send(cmd).unwrap();
        }
        let mut reader = Reader::default();
        let first = commands.recv().unwrap();
        assert!(reader.command(&ctx, latest(&commands, first)));
        assert_eq!(reader.opens, 1);
        assert!(matches!(
            rx.try_recv().unwrap(),
            Msg::Follow {
                r#gen: 3,
                ev: FollowMsg::Opened { .. }
            }
        ));
        assert!(rx.try_recv().is_err());
        let mut cmd = command(&rig, 'A', 4);
        cmd.opts.thinking = true;
        cmd.opts.tools = Tools::Full;
        assert!(reader.command(&ctx, cmd));
        assert_eq!(reader.opens, 1);
        assert!(
            matches!(rx.try_recv().unwrap(), Msg::Follow { r#gen: 4, ev: FollowMsg::Opened { items, .. }} if items.iter().any(|item| matches!(item, Item::Assistant { thinking: Some(_), .. })))
        );
    }
    #[test]
    fn endpoint_only_retries_until_persisted_without_a_failure() {
        let rig = Rig::new(false);
        let (ctx, rx) = rig.ctx();
        let session =
            Session::parse(std::path::Path::new("fixture"), fixtures::bytes('A')).unwrap();
        let id = session.header.id;
        let path = rig
            .agent
            .join("sessions/-work-alpha")
            .join(format!("2026-10-30T00-00-00-000Z_{id}.jsonl"));
        let reg = registry::path(&rig.home, &id);
        fs::create_dir_all(reg.parent().unwrap()).unwrap();
        fs::write(
            &reg,
            serde_json::to_vec(&serde_json::json!({"v":1,"pid":std::process::id(),
            "session_id":id,"session_file":path,"socket":rig.temp.path().join("absent.sock"),
            "cwd":rig.env.cwd(),"title":"pending session"}))
            .unwrap(),
        )
        .unwrap();
        let (tx, commands) = mpsc::channel();
        tx.send(Command {
            r#gen: 12,
            target: Target { id, path: None },
            opts: ItemOpts {
                thinking: false,
                tools: Tools::Collapsed,
            },
        })
        .unwrap();
        let worker = spawn(ctx.clone(), commands);
        let pending = wait(&rx, |m| matches!(m, Msg::Follow { .. }));
        assert!(matches!(
            pending,
            Msg::Follow {
                r#gen: 12,
                ev: FollowMsg::NotYetPersisted
            }
        ));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, fixtures::bytes('A')).unwrap();
        loop {
            match wait(&rx, |m| matches!(m, Msg::Follow { .. })) {
                Msg::Follow {
                    ev:
                        FollowMsg::Opened {
                            not_yet_persisted: false,
                            ..
                        },
                    ..
                } => break,
                Msg::Follow {
                    ev: FollowMsg::NotYetPersisted,
                    ..
                } => (),
                _ => panic!("expected pending then opened"),
            }
        }
        stop(&ctx, worker);
    }
    #[test]
    fn ordered_rewrite_and_branch_notices_match_tail() {
        let rig = Rig::new(true);
        let (ctx, rx) = rig.ctx();
        let mut reader = Reader::default();
        let cmd = command(&rig, 'A', 1);
        let path = cmd.target.path.clone().unwrap();
        assert!(reader.command(&ctx, cmd));
        rx.recv().unwrap();
        let staged = path.with_extension("staged");
        fs::write(&staged, fs::read(&path).unwrap()).unwrap();
        append(&staged, "new-item", "10000009");
        fs::rename(staged, &path).unwrap();
        assert!(reader.tick(&ctx));
        assert!(
            matches!(rx.recv().unwrap(), Msg::Follow {ev: FollowMsg::Notice(s), ..} if s == "── session file rewritten ──")
        );
        assert!(
            matches!(rx.recv().unwrap(), Msg::Follow {ev: FollowMsg::Items(items), ..} if items.len() == 1)
        );
        append(&path, "off-branch", "10000001");
        assert!(reader.tick(&ctx));
        assert!(
            matches!(rx.recv().unwrap(), Msg::Follow {ev: FollowMsg::Notice(s), ..} if s == "── switched branch ──")
        );
        assert!(
            matches!(rx.recv().unwrap(), Msg::Follow {ev: FollowMsg::Items(items), ..} if items.len() == 1)
        );
    }
    #[test]
    fn opened_backlog_caps_items_and_reports_exact_dropped_count() {
        let rig = Rig::new(true);
        let (ctx, rx) = rig.ctx();
        let mut cmd = command(&rig, 'A', 1);
        let path = cmd.target.path.as_ref().unwrap();
        let mut parent = "10000009".to_owned();
        for index in 0..1005 {
            let id = format!("extra-{index}");
            append(path, &id, &parent);
            parent = id;
        }
        let session = Session::load(path).unwrap();
        let all = crate::omp::items::build(&session.active_branch(), &cmd.opts).len();
        let mut reader = Reader::default();
        assert!(reader.command(&ctx, cmd));
        assert!(
            matches!(rx.recv().unwrap(), Msg::Follow {ev: FollowMsg::Opened {items,dropped,..},..} if items.len() == 1000 && dropped == all - 1000)
        );
        cmd = command(&rig, 'A', 2);
        cmd.opts.thinking = true;
        assert!(reader.command(&ctx, cmd));
        assert!(
            matches!(rx.recv().unwrap(), Msg::Follow {ev: FollowMsg::Opened {items,..},..} if items.len() == 1000)
        );
        assert_eq!(reader.opens, 1);
    }
}
