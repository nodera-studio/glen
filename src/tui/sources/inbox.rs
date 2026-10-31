use super::{Ctx, Wake, Worker, latest};
use crate::{
    error::GlenError,
    inbox::{self, Cursor},
    tui::state::{InboxMsg, InboxPosition, InboxRecord, InboxSnapshot, Msg},
};
use std::{
    collections::HashMap,
    fs,
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};
const INTERVAL: Duration = Duration::from_millis(500);
pub enum Command {
    Refresh,
}
struct Stored {
    stem: String,
    record: InboxRecord,
}
#[derive(Default)]
struct Reader {
    read: HashMap<String, Cursor>,
    marks: HashMap<String, Cursor>,
    records: Vec<Stored>,
}
impl Reader {
    fn cycle(
        &mut self,
        ctx: &Ctx,
        read_marks: bool,
    ) -> Result<(InboxSnapshot, Vec<GlenError>, Vec<String>), GlenError> {
        let dir = ctx.home.root.join("inbox");
        match fs::read_dir(&dir) {
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok((InboxSnapshot::default(), vec![], vec![]));
            }
            Err(e) => return Err(GlenError::read(&dir, e)),
        }
        let mut errors = vec![];
        let mut resets = vec![];
        for (stem, path) in inbox::sessions(&ctx.home, None) {
            if ctx.stopped() {
                break;
            }
            let from = self.read.get(&stem).copied().unwrap_or_default();
            let batch = match inbox::read_new(&path, from) {
                Ok(batch) => batch,
                Err(e) => {
                    errors.push(GlenError::read(&path, e));
                    continue;
                }
            };
            if from.ino != 0 && (batch.ino != from.ino || batch.end < from.offset) {
                resets.extend(
                    self.records
                        .iter()
                        .filter(|old| old.stem == stem)
                        .map(|old| old.record.message.session_id.clone()),
                );
                self.records.retain(|old| old.stem != stem);
            }
            self.read.insert(
                stem.clone(),
                Cursor {
                    offset: batch.end,
                    ino: batch.ino,
                },
            );
            if read_marks || !self.marks.contains_key(&stem) {
                self.marks
                    .insert(stem.clone(), inbox::cursor(&ctx.home, &stem));
            }
            for (reply, end) in batch.records {
                let message = InboxMsg {
                    id: reply.id,
                    session_id: reply.session_id,
                    job_id: reply.job_id,
                    text: reply.text,
                    is_final: reply.is_final,
                    at: reply.at,
                    unread: false,
                };
                let record = InboxRecord {
                    message,
                    position: InboxPosition {
                        end,
                        ino: batch.ino,
                    },
                };
                if let Some(old) = self
                    .records
                    .iter_mut()
                    .find(|old| old.stem == stem && old.record.message.id == record.message.id)
                {
                    old.record = record;
                } else {
                    self.records.push(Stored {
                        stem: stem.clone(),
                        record,
                    });
                }
            }
        }
        self.records.sort_by(|a, b| {
            let a = &a.record.message;
            let b = &b.record.message;
            a.at.parse::<jiff::Timestamp>()
                .ok()
                .cmp(&b.at.parse::<jiff::Timestamp>().ok())
                .then(a.at.cmp(&b.at))
                .then(a.session_id.cmp(&b.session_id))
                .then(a.id.cmp(&b.id))
        });
        let excess = self.records.len().saturating_sub(500);
        self.records.drain(..excess);
        let mut cursors = HashMap::new();
        for stored in &mut self.records {
            let cursor = self.marks.get(&stored.stem).copied().unwrap_or_default();
            stored.record.message.unread = cursor.ino != stored.record.position.ino
                || stored.record.position.end > cursor.offset;
            cursors.insert(stored.record.message.session_id.clone(), cursor);
        }
        resets.sort();
        resets.dedup();
        Ok((
            InboxSnapshot {
                records: self.records.iter().map(|old| old.record.clone()).collect(),
                cursors,
            },
            errors,
            resets,
        ))
    }
}
pub fn spawn(ctx: Ctx, rx_cmd: Receiver<Command>) -> Worker {
    std::thread::spawn(move || {
        let mut reader = Reader::default();
        let mut next = Instant::now();
        let mut next_marks = Instant::now();
        while !ctx.stopped() {
            if let Ok(command) = rx_cmd.try_recv() {
                latest(&rx_cmd, command);
                next = Instant::now();
            }
            if Instant::now() >= next {
                let read_marks = Instant::now() >= next_marks;
                match reader.cycle(&ctx, read_marks) {
                    Ok((snapshot, errors, resets)) => {
                        for e in errors {
                            if !ctx.error("inbox", e) {
                                return;
                            }
                        }
                        if !resets.is_empty() && !ctx.send(Msg::InboxReset(resets)) {
                            return;
                        }
                        if !ctx.send(Msg::Inbox(snapshot)) {
                            return;
                        }
                    }
                    Err(e) => {
                        if !ctx.error("inbox", e) {
                            return;
                        }
                    }
                }
                next = Instant::now() + INTERVAL;
                if read_marks {
                    next_marks = Instant::now() + Duration::from_secs(1);
                }
            }
            match ctx.wait(&rx_cmd, next) {
                Wake::Command(command) => {
                    latest(&rx_cmd, command);
                    next = Instant::now();
                    next_marks = Instant::now();
                }
                Wake::Tick => (),
                Wake::Stop => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        jobs::Reply,
        tui::{
            sources::tests::{Rig, stop, wait},
            state::*,
            update,
        },
    };
    use std::{io::Write, sync::mpsc};
    fn reply(id: usize) -> Reply {
        Reply {
            v: 1,
            id: format!("reply-{id:04}"),
            session_id: "session-a".into(),
            job_id: None,
            request_id: None,
            tool_call_id: None,
            text: format!("reply {id}"),
            is_final: id == 2,
            at: format!("2026-10-30T00:{:02}:{:02}Z", id / 60, id % 60),
        }
    }
    fn path(rig: &Rig) -> std::path::PathBuf {
        let dir = rig.home.root.join("inbox");
        fs::create_dir_all(&dir).unwrap();
        dir.join("session-a.ndjson")
    }
    fn append(path: &std::path::Path, reply: &Reply) {
        writeln!(
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap(),
            "{}",
            serde_json::to_string(reply).unwrap()
        )
        .unwrap();
    }
    #[test]
    fn appended_records_arrive_without_writing_read_cursors_and_marks_refresh() {
        let rig = Rig::new(false);
        let path = path(&rig);
        let (ctx, rx) = rig.ctx();
        let (tx, commands) = mpsc::channel();
        let worker = spawn(ctx.clone(), commands);
        wait(&rx, |m| matches!(m, Msg::Inbox(_)));
        append(&path, &reply(1));
        append(&path, &reply(2));
        let Msg::Inbox(snapshot) =
            wait(&rx, |m| matches!(m, Msg::Inbox(s) if s.records.len() == 2))
        else {
            unreachable!()
        };
        assert!(snapshot.records.iter().all(|r| r.message.unread));
        assert!(snapshot.records[1].message.is_final);
        assert!(!rig.home.root.join("inbox/.read").exists());
        let mark = snapshot.records[0].position;
        let read_dir = rig.home.root.join("inbox/.read");
        fs::create_dir_all(&read_dir).unwrap();
        let marker = serde_json::to_vec(&Cursor {
            offset: mark.end,
            ino: mark.ino,
        })
        .unwrap();
        fs::write(read_dir.join("session-a"), &marker).unwrap();
        // No refresh command: the periodic persisted-cursor read must notice external CLI marks.
        let Msg::Inbox(marked) = wait(
            &rx,
            |m| matches!(m, Msg::Inbox(s) if s.records.len() == 2 && !s.records[0].message.unread),
        ) else {
            unreachable!()
        };
        assert!(marked.records[1].message.unread);
        assert_eq!(marked.cursors["session-a"].offset, mark.end);
        stop(&ctx, worker);
        assert_eq!(fs::read(read_dir.join("session-a")).unwrap(), marker);
        drop(tx);
    }
    #[test]
    fn partial_lines_cap_rotation_and_unread_counts_use_in_memory_positions() {
        use crate::{
            render::palette::{ColorDepth, Theme},
            tui::theme::TuiTheme,
        };
        let rig = Rig::new(false);
        let path = path(&rig);
        let (ctx, _) = rig.ctx();
        let mut reader = Reader::default();
        append(&path, &reply(1));
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(file, "{}", serde_json::to_string(&reply(2)).unwrap()).unwrap();
        let (first, errors, resets) = reader.cycle(&ctx, true).unwrap();
        assert_eq!(first.records.len(), 1);
        assert!(errors.is_empty() && resets.is_empty());
        assert_eq!(reader.cycle(&ctx, false).unwrap().0.records.len(), 1);
        writeln!(file).unwrap();
        let (second, _, _) = reader.cycle(&ctx, false).unwrap();
        assert_eq!(second.records.len(), 2);
        let mut app = App::new(
            TuiTheme {
                depth: ColorDepth::None,
                theme: Theme::Dark,
            },
            Scope::All,
            (100, 30),
            Instant::now(),
            std::time::SystemTime::UNIX_EPOCH,
        );
        update::update(&mut app, Msg::Inbox(second.clone()));
        assert_eq!(app.inbox.unread_by_session["session-a"], 2);
        let dir = rig.home.root.join("inbox/.read");
        fs::create_dir_all(&dir).unwrap();
        let pos = second.records[0].position;
        fs::write(
            dir.join("session-a"),
            serde_json::to_vec(&Cursor {
                offset: pos.end,
                ino: pos.ino,
            })
            .unwrap(),
        )
        .unwrap();
        update::update(&mut app, Msg::Inbox(reader.cycle(&ctx, true).unwrap().0));
        assert_eq!(app.inbox.unread_by_session["session-a"], 1);
        for id in 3..=510 {
            append(&path, &reply(id));
        }
        let (snapshot, _, _) = reader.cycle(&ctx, false).unwrap();
        assert_eq!(snapshot.records.len(), 500);
        assert_eq!(snapshot.records[0].message.id, "reply-0011");
        update::update(&mut app, Msg::Inbox(snapshot));
        let replacement = path.with_extension("replacement");
        append(&replacement, &reply(999));
        fs::rename(replacement, &path).unwrap();
        let (snapshot, errors, resets) = reader.cycle(&ctx, true).unwrap();
        assert!(errors.is_empty());
        assert_eq!(resets, vec!["session-a"]);
        assert_eq!(snapshot.records.len(), 1);
        update::update(&mut app, Msg::InboxReset(resets));
        update::update(&mut app, Msg::Inbox(snapshot));
        assert_eq!(app.inbox.records.len(), 1);
        assert_eq!(app.inbox.records[0].id, "reply-0999");
        assert_eq!(app.inbox.unread_by_session["session-a"], 1);
    }
}
