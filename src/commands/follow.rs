use super::open::{self, Opened};
use crate::{
    error::{ErrorCode, GlenError},
    omp::{
        continuation,
        items::{self, Item, ItemOpts},
        listing::{self, SessionFile, Summary},
        session::{LoadError, Session},
    },
};
use std::{
    collections::HashSet,
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(crate) enum FollowEvent {
    Items(Vec<Item>),
    Continued {
        from: String,
        to: String,
        path: PathBuf,
        handle: String,
    },
    Rewritten {
        path: PathBuf,
    },
    BranchSwitch {
        leaf_id: Option<String>,
    },
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
pub(crate) struct Follower {
    session: Session,
    summary: Summary,
    offset: u64,
    identity: (u64, u64),
    emitted: HashSet<String>,
    last_on_branch: Option<String>,
    opts: ItemOpts,
    ticks: u8,
    missing: Option<Instant>,
    entries: continuation::EntryCache,
    reload_pending: bool,
}
impl Follower {
    pub(crate) fn new(o: Opened, opts: ItemOpts) -> Self {
        Self {
            offset: o.session.consumed,
            identity: o.metadata.as_ref().map(identity).unwrap_or((0, 0)),
            emitted: o
                .session
                .active_branch()
                .iter()
                .map(|e| e.id.clone())
                .collect(),
            last_on_branch: o.session.leaf_id().map(str::to_owned),
            session: o.session,
            summary: o.summary,
            opts,
            ticks: 0,
            missing: None,
            entries: continuation::EntryCache::default(),
            reload_pending: false,
        }
    }
    pub(crate) fn backlog(&self, n: usize) -> Vec<Item> {
        let mut items = items::build(&self.session.active_branch(), &self.opts);
        items.drain(..items.len().saturating_sub(n));
        items
    }
    #[allow(dead_code)] // Shared with the watch view in the next slice.
    pub(crate) fn rebuild(&mut self, opts: ItemOpts) -> Vec<Item> {
        self.opts = opts;
        self.emitted.clear();
        self.unseen()
    }
    #[allow(dead_code)] // Shared with the watch view in the next slice.
    pub(crate) fn session(&self) -> &Session {
        &self.session
    }
    #[allow(dead_code)] // Shared with the watch view in the next slice.
    pub(crate) fn summary(&self) -> &Summary {
        &self.summary
    }
    /// One polling iteration; callers schedule these every 250 ms.
    pub(crate) fn poll(&mut self) -> Result<Vec<FollowEvent>, GlenError> {
        self.poll_at(Instant::now())
    }
    fn poll_at(&mut self, now: Instant) -> Result<Vec<FollowEvent>, GlenError> {
        self.ticks = (self.ticks + 1) % 8;
        let mut events = vec![];
        let path = self.session.path.clone();
        let available = match File::open(&path) {
            Ok(mut file) => {
                let metadata = file.metadata().map_err(|e| GlenError::read(&path, e))?;
                let (available, next) = self.tick(&mut file, &metadata)?;
                events.extend(next);
                available
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(GlenError::read(&path, e)),
        };
        if !available {
            let since = *self.missing.get_or_insert(now);
            if self.continued(&mut events)? {
                self.missing = None;
            } else if now.saturating_duration_since(since) >= Duration::from_secs(5) {
                return Err(GlenError::new(
                    ErrorCode::NotFound,
                    format!(
                        "session file disappeared or was replaced: {}",
                        path.display()
                    ),
                ));
            }
            return Ok(events);
        }
        self.missing = None;
        if self.ticks == 0 {
            self.continued(&mut events)?;
        }
        Ok(events)
    }

    fn unseen(&mut self) -> Vec<Item> {
        let branch = self.session.active_branch();
        let new = branch
            .iter()
            .filter(|e| !self.emitted.contains(&e.id))
            .copied()
            .collect::<Vec<_>>();
        let items = items::build(&new, &self.opts);
        self.emitted.extend(branch.iter().map(|e| e.id.clone()));
        self.last_on_branch = self.session.leaf_id().map(str::to_owned);
        items
    }
    fn continued(&mut self, events: &mut Vec<FollowEvent>) -> Result<bool, GlenError> {
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
        events.push(FollowEvent::Continued {
            from: self.session.header.id.clone(),
            to: session.header.id.clone(),
            path: c.path,
            handle,
        });
        self.identity = identity(&metadata);
        self.offset = session.consumed;
        self.session = session;
        self.summary = summary;
        self.reload_pending = false;
        events.push(FollowEvent::Items(self.unseen()));
        Ok(true)
    }
    pub(crate) fn tick(
        &mut self,
        file: &mut File,
        m: &Metadata,
    ) -> Result<(bool, Vec<FollowEvent>), GlenError> {
        let mut events = vec![];
        if self.reload_pending || m.len() < self.offset || identity(m) != self.identity {
            let session = match open::snapshot(&self.session.path, file, m, true) {
                Ok(session) if session.header.id == self.session.header.id => session,
                Ok(_) | Err(LoadError::NotSession) => {
                    self.reload_pending = true;
                    return Ok((false, events));
                }
                Err(e) => return Err(open::load_error(&self.session.path, e)),
            };
            self.session = session;
            self.reload_pending = false;
            self.identity = identity(m);
            self.offset = self.session.consumed;
            events.push(FollowEvent::Rewritten {
                path: self.session.path.clone(),
            });
            events.push(FollowEvent::Items(self.unseen()));
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
                let switched = self
                    .last_on_branch
                    .as_deref()
                    .is_some_and(|id| !branch.iter().any(|e| e.id == id));
                if switched {
                    events.push(FollowEvent::BranchSwitch {
                        leaf_id: self.session.leaf_id().map(str::to_owned),
                    });
                }
                let unseen = self.unseen();
                if switched || !unseen.is_empty() {
                    events.push(FollowEvent::Items(unseen));
                }
            }
        }
        Ok((true, events))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Env;
    use crate::{cli::Tools, omp::fixtures};
    use clap::Parser;
    use std::{fs, io::Write};
    #[test]
    fn incremental_read_uses_open_fd_after_path_is_replaced() {
        let tree = fixtures::tree();
        let path = fixtures::path(&tree, 'A');
        let env = Env::for_test(Some(tree.path()), tree.path(), &[]);
        let cli =
            crate::cli::Cli::try_parse_from(["glen", "tail", path.to_str().unwrap()]).unwrap();
        let o = open::open_watch(&cli, &env, path.to_str().unwrap()).unwrap();
        let mut follower = Follower::new(
            o,
            ItemOpts {
                thinking: false,
                tools: Tools::Collapsed,
            },
        );
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
        assert!(follower.tick(&mut fd, &metadata).unwrap().0);
        assert_eq!(follower.session.leaf_id(), Some("old-fd-entry"));
        assert!(follower.session.header.id.ends_with("e27c0de"));
        let mut replacement = File::open(&path).unwrap();
        let metadata = replacement.metadata().unwrap();
        assert!(!follower.tick(&mut replacement, &metadata).unwrap().0);
        assert_eq!(follower.session.leaf_id(), Some("old-fd-entry"));
    }
}

#[cfg(test)]
mod poll_tests {
    use super::*;
    use crate::{
        cli::{Cli, Tools},
        env::Env,
        omp::fixtures,
    };
    use clap::Parser;
    use serde_json::json;
    use std::{fs, io::Write, path::Path};
    fn follower(tree: &tempfile::TempDir, path: &Path) -> Follower {
        let env = Env::for_test(Some(tree.path()), tree.path(), &[]);
        let cli = Cli::try_parse_from(["glen", "tail", path.to_str().unwrap()]).unwrap();
        Follower::new(
            open::open_watch(&cli, &env, path.to_str().unwrap()).unwrap(),
            ItemOpts {
                thinking: false,
                tools: Tools::Collapsed,
            },
        )
    }
    fn message(id: &str, parent: &str) -> String {
        format!(
            "{}\n",
            json!({"type":"message", "id":id, "parentId":parent, "message":{"role":"user", "content":id}})
        )
    }
    fn append(path: &Path, text: &str) {
        fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
    fn one(events: &[FollowEvent], id: &str) {
        let [FollowEvent::Items(items)] = events else {
            panic!("expected Items: {events:?}")
        };
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], Item::User { id: actual, .. } if actual == id));
    }
    #[test]
    fn complete_lines_only_and_rebuild_resets_emitted() {
        let tree = fixtures::tree();
        let path = fixtures::path(&tree, 'A');
        let mut f = follower(&tree, &path);
        let partial = message("second", "first");
        append(&path, &(message("first", "10000009") + partial.trim_end()));
        one(&f.poll().unwrap(), "first");
        for _ in 0..3 {
            assert!(f.poll().unwrap().is_empty());
        }
        append(&path, "\n");
        one(&f.poll().unwrap(), "second");
        let rebuilt = f.rebuild(ItemOpts {
            thinking: true,
            tools: Tools::Full,
        });
        assert_eq!(rebuilt.len(), f.backlog(usize::MAX).len());
        assert!(
            matches!(&rebuilt[3], Item::Assistant { thinking: Some(_), tool_calls, .. } if tool_calls[0].arguments.is_some())
        );
        assert!(f.poll().unwrap().is_empty());
        assert_eq!(f.session().leaf_id(), Some("second"));
        assert_eq!(f.summary().id, f.session().header.id);
    }
    #[test]
    fn larger_rename_is_rewritten_then_unseen_items() {
        let tree = fixtures::tree();
        let path = fixtures::path(&tree, 'A');
        let mut f = follower(&tree, &path);
        let staged = path.with_file_name(".staged.tmp");
        let mut bytes = fixtures::bytes('A').to_vec();
        bytes.extend(message("new", "10000009").bytes());
        fs::write(&staged, bytes).unwrap();
        fs::rename(staged, &path).unwrap();
        let events = f.poll().unwrap();
        assert!(matches!(&events[0], FollowEvent::Rewritten { path: p } if p == &path));
        one(&events[1..], "new");
        assert!(f.poll().unwrap().is_empty());
    }
    #[test]
    fn deleted_parent_follows_new_continuation_immediately() {
        let tree = fixtures::tree();
        let path = fixtures::path(&tree, 'A');
        let mut f = follower(&tree, &path);
        let from = f.session().header.id.clone();
        let to = "01a20000-0000-7000-8000-000000000001";
        let child = path
            .parent()
            .unwrap()
            .join(format!("2026-10-30T00-00-00-000Z_{to}.jsonl"));
        let original = String::from_utf8(fixtures::bytes('A').to_vec()).unwrap();
        // Change only the header, keeping copied entry ids identical.
        let original = original.replace(
            &format!("\"id\":\"{from}\""),
            &format!("\"id\":\"{to}\",\"continuationOf\":\"{from}\""),
        );
        fs::write(&child, original + &format!("{}\n", json!({"type":"message","id":"continued","parentId":"10000009","timestamp":"2026-10-30T00:00:00Z","message":{"role":"user","content":"continued"}}))).unwrap();
        fs::remove_file(&path).unwrap();
        let events = f.poll().unwrap();
        assert!(
            matches!(&events[0], FollowEvent::Continued { from: a, to: b, path: p, handle } if a == &from && b == to && p == &child && handle == "0000001")
        );
        one(&events[1..], "continued");
        assert!(f.poll().unwrap().is_empty());
    }
    #[test]
    fn off_branch_leaf_emits_switch_before_items() {
        let tree = fixtures::tree();
        let path = fixtures::path(&tree, 'A');
        let mut f = follower(&tree, &path);
        append(&path, &message("other-branch", "10000001"));
        let events = f.poll().unwrap();
        assert!(
            matches!(&events[0], FollowEvent::BranchSwitch { leaf_id: Some(id) } if id == "other-branch")
        );
        one(&events[1..], "other-branch");
    }
    #[test]
    fn missing_file_times_out_after_twenty_poll_intervals_without_sleep() {
        let tree = fixtures::tree();
        let path = fixtures::path(&tree, 'A');
        let mut f = follower(&tree, &path);
        fs::remove_file(&path).unwrap();
        let start = Instant::now();
        assert!(f.poll_at(start).unwrap().is_empty());
        for tick in 1..20 {
            assert!(
                f.poll_at(start + Duration::from_millis(250 * tick))
                    .unwrap()
                    .is_empty()
            );
        }
        let err = f.poll_at(start + Duration::from_secs(5)).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(err.message.contains(path.to_str().unwrap()));
    }
    #[test]
    fn returning_to_an_emitted_leaf_keeps_the_empty_items_event() {
        let tree = fixtures::tree();
        let path = fixtures::path(&tree, 'A');
        let mut f = follower(&tree, &path);
        writeln!(
            fs::OpenOptions::new().append(true).open(&path).unwrap(),
            "{}",
            serde_json::json!({"type":"model_usage","id":"leaf-move","activeLeafId":"10000001"})
        )
        .unwrap();
        let events = f.poll().unwrap();
        assert!(
            matches!(events.as_slice(), [FollowEvent::BranchSwitch { leaf_id: Some(id) }, FollowEvent::Items(items)] if id == "10000001" && items.is_empty())
        );
    }
}
