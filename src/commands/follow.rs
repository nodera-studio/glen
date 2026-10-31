use super::open::{self, Opened};
use crate::{
    error::{ErrorCode, GlenError},
    omp::{
        continuation,
        items::{self, Item, ItemOpts},
        listing::{self, SessionFile, Summary},
        session::{Entry, LoadError, Session},
    },
};
use std::{
    collections::{HashMap, HashSet},
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use xxhash_rust::xxh3::Xxh3;

#[cfg(test)]
thread_local! {
    static FULL_RECONCILIATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static LEGACY_KEYS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PREFIX_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

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
#[derive(Hash, PartialEq, Eq)]
enum Emitted {
    Id(String),
    Legacy { content: String, occurrence: usize },
}
fn entry_key(entry: &Entry, legacy: bool, occurrences: &mut HashMap<String, usize>) -> Emitted {
    if !legacy {
        return Emitted::Id(entry.id.clone());
    }
    #[cfg(test)]
    LEGACY_KEYS.with(|count| count.set(count.get() + 1));
    let mut content = entry.raw.clone();
    if let Some(fields) = content.as_object_mut() {
        fields.remove("id");
        fields.remove("parentId");
    }
    let content = content.to_string();
    let occurrence = occurrences.entry(content.clone()).or_insert(0);
    *occurrence += 1;
    Emitted::Legacy {
        content,
        occurrence: *occurrence,
    }
}
fn branch_keys<'a>(
    session: &'a Session,
    occurrences: &mut HashMap<String, usize>,
) -> Vec<(&'a Entry, Emitted)> {
    #[cfg(test)]
    FULL_RECONCILIATIONS.with(|count| count.set(count.get() + 1));
    let legacy = session.header.raw["version"].as_u64().unwrap_or(1) < 2;
    occurrences.clear();
    session
        .active_branch()
        .into_iter()
        .map(|entry| (entry, entry_key(entry, legacy, occurrences)))
        .collect()
}
pub(crate) struct Follower {
    session: Session,
    summary: Summary,
    offset: u64,
    identity: (u64, u64),
    /// Size and mtime when the consumed prefix was last known to match.
    observed: Option<(u64, SystemTime)>,
    /// When the consumed prefix was last hashed in full; None means a hash is due.
    prefix_hashed: Option<Instant>,
    emitted: HashSet<Emitted>,
    occurrences: HashMap<String, usize>,
    known_ids: HashSet<String>,
    last_on_branch: Option<String>,
    opts: ItemOpts,
    ticks: u8,
    missing: Option<Instant>,
    entries: continuation::EntryCache,
    reload_pending: bool,
}
impl Follower {
    pub(crate) fn new(o: Opened, opts: ItemOpts) -> Self {
        let mut occurrences = HashMap::new();
        let emitted = branch_keys(&o.session, &mut occurrences)
            .into_iter()
            .map(|(_, key)| key)
            .collect();
        Self {
            offset: o.session.consumed,
            identity: o.metadata.as_ref().map(identity).unwrap_or((0, 0)),
            observed: o
                .metadata
                .as_ref()
                .and_then(|m| m.modified().ok().map(|time| (m.len(), time))),
            prefix_hashed: None,
            emitted,
            occurrences,
            known_ids: o
                .session
                .entries
                .iter()
                .map(|entry| entry.id.clone())
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
    pub(crate) fn rebuild(&mut self, opts: ItemOpts) -> Vec<Item> {
        self.opts = opts;
        self.emitted.clear();
        self.unseen(true)
    }
    pub(crate) fn session(&self) -> &Session {
        &self.session
    }
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
                let (available, next) = self.tick_at(&mut file, &metadata, now)?;
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

    fn unseen(&mut self, reset: bool) -> Vec<Item> {
        let branch = branch_keys(&self.session, &mut self.occurrences);
        let new = branch
            .iter()
            .filter(|(_, key)| !self.emitted.contains(key))
            .map(|(entry, _)| *entry)
            .collect::<Vec<_>>();
        let items = items::build(&new, &self.opts);
        if reset {
            // Compare against the old keys first, then rebase occurrences onto this snapshot.
            self.emitted.clear();
        }
        self.emitted.extend(branch.into_iter().map(|(_, key)| key));
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
        self.observed = metadata.modified().ok().map(|time| (metadata.len(), time));
        self.known_ids = session
            .entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect();
        self.offset = session.consumed;
        self.session = session;
        self.summary = summary;
        self.reload_pending = false;
        events.push(FollowEvent::Items(self.unseen(false)));
        Ok(true)
    }
    fn consumed_tail_matches(&self, file: &mut File) -> Result<bool, GlenError> {
        let tail = self.session.consumed_tail();
        file.seek(SeekFrom::Start(self.offset - tail.len() as u64))
            .map_err(|e| GlenError::read(&self.session.path, e))?;
        let mut current = vec![0; tail.len()];
        match file.read_exact(&mut current) {
            Ok(()) => Ok(current == tail),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
            Err(e) => Err(GlenError::read(&self.session.path, e)),
        }
    }
    fn consumed_prefix_matches(&self, file: &mut File) -> Result<bool, GlenError> {
        #[cfg(test)]
        PREFIX_CHECKS.with(|count| count.set(count.get() + 1));
        file.seek(SeekFrom::Start(0))
            .map_err(|e| GlenError::read(&self.session.path, e))?;
        let mut hash = Xxh3::new();
        let mut remaining = self.offset;
        let mut buffer = [0; 8192];
        while remaining > 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            match file.read_exact(&mut buffer[..count]) {
                Ok(()) => hash.update(&buffer[..count]),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
                Err(e) => return Err(GlenError::read(&self.session.path, e)),
            }
            remaining -= count as u64;
        }
        Ok(hash.digest() == self.session.consumed_hash())
    }
    #[cfg(test)]
    pub(crate) fn tick(
        &mut self,
        file: &mut File,
        m: &Metadata,
    ) -> Result<(bool, Vec<FollowEvent>), GlenError> {
        self.tick_at(file, m, Instant::now())
    }
    /// The size-shrink, identity and 4 KB tail checks run on every poll. The
    /// full prefix hash runs only when metadata moved since the prefix was last
    /// verified and at least `PREFIX_HASH_EVERY` has passed since the last hash,
    /// so following a busy, large session is not O(file size) per poll.
    fn tick_at(
        &mut self,
        file: &mut File,
        m: &Metadata,
        now: Instant,
    ) -> Result<(bool, Vec<FollowEvent>), GlenError> {
        let mut events = vec![];
        let mut hashed = false;
        let observed = (
            m.len(),
            m.modified()
                .map_err(|e| GlenError::read(&self.session.path, e))?,
        );
        if self.reload_pending
            || m.len() < self.offset
            || identity(m) != self.identity
            || !self.consumed_tail_matches(file)?
            || (self.observed != Some(observed) && self.prefix_hash_due(now) && {
                hashed = true;
                self.prefix_hashed = Some(now);
                !self.consumed_prefix_matches(file)?
            })
        {
            let session = match open::snapshot(&self.session.path, file, m, true) {
                Ok(session) if session.header.id == self.session.header.id => session,
                Ok(_) | Err(LoadError::NotSession) => {
                    self.reload_pending = true;
                    return Ok((false, events));
                }
                Err(e) => return Err(open::load_error(&self.session.path, e)),
            };
            self.known_ids = session
                .entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect();
            self.session = session;
            self.reload_pending = false;
            self.identity = identity(m);
            self.offset = self.session.consumed;
            events.push(FollowEvent::Rewritten {
                path: self.session.path.clone(),
            });
            events.push(FollowEvent::Items(self.unseen(true)));
            self.observed = Some(observed);
        } else if m.len() > self.offset {
            file.seek(SeekFrom::Start(self.offset))
                .map_err(|e| GlenError::read(&self.session.path, e))?;
            let mut bytes = vec![];
            file.take(m.len() - self.offset)
                .read_to_end(&mut bytes)
                .map_err(|e| GlenError::read(&self.session.path, e))?;
            if let Some(end) = bytes.iter().rposition(|b| *b == b'\n') {
                let added = self.session.append(&bytes[..=end]);
                self.offset += (end + 1) as u64;
                let mut parent = self.last_on_branch.as_deref();
                let mut linear = true;
                for entry in &self.session.entries[added.clone()] {
                    let unique = self.known_ids.insert(entry.id.clone());
                    linear &= unique
                        && entry.parent_id.as_deref() == parent
                        && entry
                            .active_leaf_id
                            .as_ref()
                            .is_none_or(|leaf| leaf.as_deref() == Some(entry.id.as_str()));
                    parent = Some(&entry.id);
                }
                let mut switched = false;
                let unseen = if linear {
                    let legacy = self.session.header.raw["version"].as_u64().unwrap_or(1) < 2;
                    let new = self.session.entries[added]
                        .iter()
                        .filter(|entry| {
                            let key = entry_key(entry, legacy, &mut self.occurrences);
                            self.emitted.insert(key)
                        })
                        .collect::<Vec<_>>();
                    let items = items::build(&new, &self.opts);
                    self.last_on_branch = self.session.leaf_id().map(str::to_owned);
                    items
                } else {
                    let branch = self.session.active_branch();
                    switched = self
                        .last_on_branch
                        .as_deref()
                        .is_some_and(|id| !branch.iter().any(|e| e.id == id));
                    if switched {
                        events.push(FollowEvent::BranchSwitch {
                            leaf_id: self.session.leaf_id().map(str::to_owned),
                        });
                    }
                    self.unseen(false)
                };
                if switched || !unseen.is_empty() {
                    events.push(FollowEvent::Items(unseen));
                }
            }
        }
        if hashed {
            self.observed = Some(observed);
        }
        Ok((true, events))
    }
    fn prefix_hash_due(&self, now: Instant) -> bool {
        self.prefix_hashed
            .is_none_or(|at| now.saturating_duration_since(at) >= PREFIX_HASH_EVERY)
    }
}
const PREFIX_HASH_EVERY: Duration = Duration::from_secs(5);
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
    fn same_inode_rewrite_at_or_above_the_consumed_length_is_not_an_append() {
        for grows in [false, true] {
            let tree = fixtures::tree();
            let path = fixtures::path(&tree, 'A');
            let mut f = follower(&tree, &path);
            let before = fs::metadata(&path).unwrap();
            let mut rewritten = fs::read_to_string(&path)
                .unwrap()
                .replace("10000007", "90000007");
            if grows {
                rewritten.push_str(&message("after-rewrite", "10000009"));
            }
            assert!(rewritten.len() as u64 >= f.offset);
            fs::write(&path, rewritten).unwrap();
            assert_eq!(identity(&before), identity(&fs::metadata(&path).unwrap()));
            let events = f.poll().unwrap();
            let [
                FollowEvent::Rewritten { path: actual },
                FollowEvent::Items(items),
            ] = events.as_slice()
            else {
                panic!("expected a rewrite before the fresh items: {events:?}");
            };
            assert_eq!(actual, &path);
            assert!(matches!(items.first(), Some(Item::Assistant { id, .. }) if id == "90000007"));
            assert_eq!(items.len(), if grows { 2 } else { 1 });
            if grows {
                assert!(matches!(&items[1], Item::User { id, .. } if id == "after-rewrite"));
            }
            assert!(f.poll().unwrap().is_empty());
        }
    }
    #[test]
    fn v1_rewrite_reconciles_content_after_ordinal_ids_shift() {
        let tree = fixtures::tree();
        let path = tree.path().join("legacy.jsonl");
        let original = include_str!("../../tests/fixtures/v1-session.jsonl");
        fs::write(&path, original).unwrap();
        let mut f = follower(&tree, &path);
        let (header, entries) = original.split_once('\n').unwrap();
        let fresh = "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"fresh after rewrite\"}}\n";
        // The extra non-message entry shifts every synthesized ordinal id.
        let rewritten = format!("{header}\n{{\"type\":\"future_metadata\"}}\n{entries}{fresh}");
        let staged = path.with_extension("staged");
        fs::write(&staged, rewritten).unwrap();
        fs::rename(staged, &path).unwrap();
        let events = f.poll().unwrap();
        assert!(matches!(&events[0], FollowEvent::Rewritten { .. }));
        let [FollowEvent::Items(items)] = &events[1..] else {
            panic!("expected fresh items: {events:?}");
        };
        assert!(
            matches!(items.as_slice(), [Item::User { text, .. }] if text == "fresh after rewrite")
        );
        assert!(f.poll().unwrap().is_empty());
        // A genuinely repeated message must still be emitted as a new occurrence.
        append(&path, original.lines().nth(1).unwrap());
        append(&path, "\n");
        let events = f.poll().unwrap();
        assert!(matches!(events.as_slice(), [FollowEvent::Items(items)]
            if matches!(items.as_slice(), [Item::User { text, .. }] if text == "old prompt")));
    }
    #[test]
    fn equal_size_early_prefix_rewrite_is_hashed_only_after_metadata_changes() {
        let tree = fixtures::tree();
        let path = fixtures::path(&tree, 'A');
        let original = String::from_utf8(fixtures::bytes('A').to_vec()).unwrap()
            + &format!(
                "{}\n",
                json!({"type":"future", "id":"padding", "parentId":"10000009", "data":"x".repeat(9000)})
            );
        fs::write(&path, &original).unwrap();
        let mut f = follower(&tree, &path);
        PREFIX_CHECKS.with(|count| count.set(0));
        for _ in 0..3 {
            assert!(f.poll().unwrap().is_empty());
        }
        assert_eq!(PREFIX_CHECKS.with(|count| count.get()), 0);
        let old = fs::metadata(&path).unwrap();
        let rewritten = original
            .replace("10000003", "90000003")
            .replace("Why does parser_test flake?", "prefix rewrite detected    ");
        assert_eq!(original.len(), rewritten.len());
        assert_eq!(
            &original.as_bytes()[original.len() - 4096..],
            &rewritten.as_bytes()[rewritten.len() - 4096..]
        );
        fs::write(&path, &rewritten).unwrap();
        File::open(&path)
            .unwrap()
            .set_modified(old.modified().unwrap() + Duration::from_secs(1))
            .unwrap();
        assert_eq!(identity(&old), identity(&fs::metadata(&path).unwrap()));
        let events = f.poll().unwrap();
        assert!(matches!(&events[0], FollowEvent::Rewritten { .. }));
        assert!(matches!(&events[1], FollowEvent::Items(items)
            if matches!(items.as_slice(), [Item::User { id, text, .. }] if id == "90000003" && text.starts_with("prefix rewrite detected"))));
        assert_eq!(PREFIX_CHECKS.with(|count| count.get()), 1);
        assert!(f.poll().unwrap().is_empty());
        assert_eq!(PREFIX_CHECKS.with(|count| count.get()), 1);
        // A true append also changes metadata, but must not be mistaken for another rewrite.
        append(&path, &message("after-prefix", "padding"));
        one(&f.poll().unwrap(), "after-prefix");
    }
    #[test]
    fn appends_within_the_cadence_hash_the_prefix_at_most_once() {
        let tree = fixtures::tree();
        let path = fixtures::path(&tree, 'A');
        let mut f = follower(&tree, &path);
        PREFIX_CHECKS.with(|count| count.set(0));
        let start = Instant::now();
        let mut parent = "10000009".to_owned();
        for i in 0..20 {
            let id = format!("append-{i}");
            append(&path, &message(&id, &parent));
            one(
                &f.poll_at(start + Duration::from_millis(200 * i)).unwrap(),
                &id,
            );
            parent = id;
        }
        assert!(PREFIX_CHECKS.with(|count| count.get()) <= 1);
    }
    #[test]
    fn equal_size_prefix_rewrite_inside_the_cadence_is_found_when_the_hash_is_due() {
        let tree = fixtures::tree();
        let path = fixtures::path(&tree, 'A');
        let original = String::from_utf8(fixtures::bytes('A').to_vec()).unwrap()
            + &format!(
                "{}\n",
                json!({"type":"future", "id":"padding", "parentId":"10000009", "data":"x".repeat(9000)})
            );
        fs::write(&path, &original).unwrap();
        let mut f = follower(&tree, &path);
        let start = Instant::now();
        // The first append hashes the prefix and starts the cadence.
        append(&path, &message("first", "padding"));
        one(&f.poll_at(start).unwrap(), "first");
        PREFIX_CHECKS.with(|count| count.set(0));
        let current = fs::read_to_string(&path).unwrap();
        let rewritten = current
            .replace("10000003", "90000003")
            .replace("Why does parser_test flake?", "prefix rewrite detected    ");
        assert_eq!(current.len(), rewritten.len());
        let old = fs::metadata(&path).unwrap();
        fs::write(&path, &rewritten).unwrap();
        File::open(&path)
            .unwrap()
            .set_modified(old.modified().unwrap() + Duration::from_secs(1))
            .unwrap();
        assert!(
            f.poll_at(start + Duration::from_secs(1))
                .unwrap()
                .is_empty()
        );
        assert_eq!(PREFIX_CHECKS.with(|count| count.get()), 0);
        let events = f.poll_at(start + Duration::from_secs(5)).unwrap();
        assert!(matches!(&events[0], FollowEvent::Rewritten { .. }));
        assert_eq!(PREFIX_CHECKS.with(|count| count.get()), 1);
        assert!(
            f.poll_at(start + Duration::from_secs(11))
                .unwrap()
                .is_empty()
        );
        assert_eq!(PREFIX_CHECKS.with(|count| count.get()), 1);
    }
    fn legacy_message(text: &str) -> String {
        format!(
            "{}\n",
            json!({"type":"message", "message":{"role":"user", "content":text}})
        )
    }
    #[test]
    fn v1_rewrite_rebases_occurrences_before_the_next_append() {
        let tree = fixtures::tree();
        let path = tree.path().join("legacy.jsonl");
        let header = include_str!("../../tests/fixtures/v1-session.jsonl")
            .lines()
            .next()
            .unwrap();
        let a = legacy_message("old prompt");
        let b = legacy_message("another prompt");
        fs::write(&path, format!("{header}\n{a}{a}{b}")).unwrap();
        let mut f = follower(&tree, &path);
        assert_eq!(f.emitted.len(), 3);
        fs::write(&path, format!("{header}\n{a}{b}")).unwrap();
        let events = f.poll().unwrap();
        assert!(
            matches!(events.as_slice(), [FollowEvent::Rewritten { .. }, FollowEvent::Items(items)] if items.is_empty())
        );
        assert_eq!(f.emitted.len(), 2);
        append(&path, &a);
        let events = f.poll().unwrap();
        assert!(matches!(events.as_slice(), [FollowEvent::Items(items)]
            if matches!(items.as_slice(), [Item::User { text, .. }] if text == "old prompt")));
        assert!(f.poll().unwrap().is_empty());
    }
    #[test]
    fn large_v1_appends_key_only_new_entries_and_rewrites_reconcile_once() {
        let tree = fixtures::tree();
        let path = tree.path().join("legacy.jsonl");
        let header = include_str!("../../tests/fixtures/v1-session.jsonl")
            .lines()
            .next()
            .unwrap();
        let mut original = format!("{header}\n{}", legacy_message("old prompt").repeat(500));
        fs::write(&path, &original).unwrap();
        let mut f = follower(&tree, &path);
        FULL_RECONCILIATIONS.with(|count| count.set(0));
        LEGACY_KEYS.with(|count| count.set(0));
        for i in 0..5 {
            let text = format!("new {i}");
            let line = legacy_message(&text);
            original.push_str(&line);
            append(&path, &line);
            let events = f.poll().unwrap();
            assert!(matches!(events.as_slice(), [FollowEvent::Items(items)]
                if matches!(items.as_slice(), [Item::User { text: actual, .. }] if actual == &text)));
        }
        assert_eq!(FULL_RECONCILIATIONS.with(|count| count.get()), 0);
        assert_eq!(LEGACY_KEYS.with(|count| count.get()), 5);
        let (header, entries) = original.split_once('\n').unwrap();
        fs::write(
            &path,
            format!("{header}\n{{\"type\":\"future_metadata\"}}\n{entries}"),
        )
        .unwrap();
        let events = f.poll().unwrap();
        assert!(
            matches!(events.as_slice(), [FollowEvent::Rewritten { .. }, FollowEvent::Items(items)] if items.is_empty())
        );
        assert_eq!(FULL_RECONCILIATIONS.with(|count| count.get()), 1);
        assert_eq!(LEGACY_KEYS.with(|count| count.get()), 5 + 506);
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
