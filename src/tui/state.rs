//! Owned UI state and worker messages. This module performs no I/O.
use super::{
    prompt::{Draft, Prompt},
    theme::TuiTheme,
};
use crate::{
    cli::{DeliverAs, Tools},
    error::GlenError,
    jobs::{JobResult, Route, Status},
    omp::{
        items::{Item, ItemOpts},
        lease::State,
        session::TurnStatus,
    },
};
use ratatui::{crossterm::event::KeyEvent, text::Line};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    time::{Instant, SystemTime},
};

pub struct App {
    pub theme: TuiTheme,
    pub caps: Caps,
    pub size: (u16, u16),
    pub now: Instant,
    pub wall: SystemTime,
    pub frame: usize,
    pub focus: Focus,
    pub narrow_tab: Pane,
    pub overlay: Option<Overlay>,
    pub help_scroll: usize,
    pub home: Option<PathBuf>,
    pub scope: Scope,
    /// Remember the invoking project when switching to the all-projects scope.
    pub bucket_scope: Option<Scope>,
    pub list: ListState,
    pub transcript: Transcript,
    pub prompt: Prompt,
    pub drafts: HashMap<String, Draft>,
    pub history: Vec<String>,
    pub sends: Vec<SendTrack>,
    pub jobs: JobsView,
    pub inbox: InboxView,
    pub toast: Option<Toast>,
    pub exit: Option<i32>,
    pub next_local: u64,
    pub pending_control: Option<PendingControl>,
}
impl App {
    pub fn new(
        theme: TuiTheme,
        scope: Scope,
        size: (u16, u16),
        now: Instant,
        wall: SystemTime,
    ) -> Self {
        Self {
            theme,
            caps: Caps {
                controls: super::controls::AVAILABLE,
            },
            size,
            now,
            wall,
            frame: 0,
            focus: Focus::Sessions,
            narrow_tab: Pane::Sessions,
            overlay: None,
            help_scroll: 0,
            home: None,
            bucket_scope: matches!(scope, Scope::Bucket { .. }).then(|| scope.clone()),
            scope,
            list: ListState::default(),
            transcript: Transcript {
                width: size.0.saturating_sub(2).max(1),
                height: size.1.saturating_sub(2).max(1),
                ..Transcript::default()
            },
            prompt: Prompt::default(),
            drafts: HashMap::new(),
            history: vec![],
            sends: vec![],
            jobs: JobsView::default(),
            inbox: InboxView::default(),
            toast: None,
            exit: None,
            next_local: 1,
            pending_control: None,
        }
    }
    pub fn narrow(&self) -> bool {
        self.size.0 < 72
    }
    /// The view can supply its actual inner viewport via `update::set_viewport`.
    pub fn page(&self) -> usize {
        usize::from(self.transcript.height.max(1))
    }
    pub fn list_page(&self) -> usize {
        self.list.page.unwrap_or_else(|| self.page()).max(1)
    }
    pub fn local(&mut self) -> u64 {
        let local = self.next_local;
        self.next_local = self.next_local.wrapping_add(1);
        local
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Caps {
    pub controls: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Sessions,
    Transcript,
    Prompt,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Sessions,
    Transcript,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Bucket { cwd: PathBuf, label: String },
    All,
}
#[derive(Default)]
pub struct ListState {
    /// Visible rows supplied by layout; absent in reducer-only callers.
    pub page: Option<usize>,
    pub loaded: bool,
    pub rows: Vec<SessionRow>,
    pub selected: Option<String>,
    pub offset: usize,
    pub total_all: usize,
    pub sessions_dir_missing: Option<PathBuf>,
}
#[derive(Clone, Debug)]
pub struct SessionRow {
    pub id: String,
    pub handle: String,
    pub path: Option<PathBuf>,
    pub title: String,
    pub first_message: Option<String>,
    pub cwd: Option<String>,
    pub state: State,
    pub source: &'static str,
    pub last_turn: TurnStatus,
    pub updated: SystemTime,
    pub model: Option<String>,
}
pub struct Transcript {
    pub r#gen: u64,
    pub target: Option<Target>,
    pub status: TranscriptStatus,
    pub header: Option<Header>,
    pub blocks: VecDeque<Block>,
    pub dropped: usize,
    pub opts: ItemOpts,
    pub width: u16,
    pub height: u16,
    pub lines: Vec<Line<'static>>,
    pub starts: Vec<usize>,
    pub prev_model: Option<String>,
    pub scroll: Scroll,
}
impl Default for Transcript {
    fn default() -> Self {
        Self {
            r#gen: 0,
            target: None,
            status: TranscriptStatus::Idle,
            header: None,
            blocks: VecDeque::new(),
            dropped: 0,
            opts: ItemOpts {
                thinking: false,
                tools: Tools::Collapsed,
            },
            width: 0,
            height: 1,
            lines: vec![],
            starts: vec![],
            prev_model: None,
            scroll: Scroll {
                follow: true,
                top: 0,
                unseen: 0,
            },
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub path: Option<PathBuf>,
}
#[derive(Clone, Debug)]
pub enum TranscriptStatus {
    Idle,
    Opening,
    Ready,
    NotYetPersisted,
    Failed(ErrView),
}
#[derive(Clone, Debug)]
pub struct Header {
    pub title: String,
    pub handle: String,
    pub model: Option<String>,
}
#[derive(Clone, Debug)]
pub enum Block {
    Item(Item),
    Notice(String),
}
#[derive(Clone, Copy, Debug)]
pub struct Scroll {
    pub follow: bool,
    pub top: usize,
    pub unseen: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrView {
    pub code: String,
    pub message: String,
    pub hint: Option<String>,
}
impl From<GlenError> for ErrView {
    fn from(e: GlenError) -> Self {
        Self {
            code: e.code.as_str().into(),
            message: e.message,
            hint: e.hint,
        }
    }
}
#[derive(Clone, Debug)]
pub struct SendTrack {
    pub local: u64,
    pub session_id: String,
    pub handle: String,
    pub text: String,
    pub mode: DeliverAs,
    pub phase: SendPhase,
    pub started: Instant,
}
#[derive(Clone, Debug)]
pub enum SendPhase {
    Routing,
    Job { id: String, handle: String },
    Refused(ErrView),
}
#[derive(Default)]
pub struct JobsView {
    pub rows: Vec<JobRow>,
    pub active_total: usize,
    /// Prevent full snapshots from resurrecting dismissed terminal strip rows.
    pub dismissed: HashSet<String>,
}
#[derive(Clone, Debug)]
pub struct JobRow {
    pub id: String,
    pub handle: String,
    pub session_id: Option<String>,
    pub actual_session_id: Option<String>,
    pub route: Route,
    pub status: Status,
    pub created_at: String,
    pub result: Option<JobResult>,
    pub ours: bool,
    pub finished_at: Option<Instant>,
    pub progress: String,
    pub done: Option<Done>,
}
#[derive(Clone, Debug)]
pub enum Done {
    Completed(String),
    Forked(String),
    Failed(ErrView),
}
#[derive(Default)]
pub struct InboxView {
    pub records: Vec<InboxMsg>,
    pub unread_by_session: HashMap<String, usize>,
    pub scroll: usize,
    pub positions: HashMap<(String, String), InboxPosition>,
    pub cursors: HashMap<String, crate::inbox::Cursor>,
}
#[derive(Clone, Debug)]
pub struct InboxMsg {
    pub id: String,
    pub session_id: String,
    pub job_id: Option<String>,
    pub text: String,
    pub is_final: bool,
    pub at: String,
    pub unread: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct InboxPosition {
    pub end: u64,
    pub ino: u64,
}
#[derive(Clone, Debug)]
pub enum Overlay {
    Help,
    Inbox,
    Confirm(ControlOp),
    Picker(Picker),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlOp {
    Abort,
    ListModels,
    SetModel { provider: String, id: String },
    SetThinking(String),
    Compact,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKind {
    Model,
    Thinking,
}
#[derive(Clone, Debug)]
pub struct Picker {
    pub filter: String,
    pub kind: PickerKind,
    pub entries: Vec<PickerEntry>,
    pub selected: usize,
    pub target: Target,
}
impl Picker {
    pub fn visible_entries(&self) -> Vec<&PickerEntry> {
        let filter = self.filter.to_lowercase();
        self.entries
            .iter()
            .filter(|entry| entry.label.to_lowercase().contains(&filter))
            .collect()
    }
}
#[derive(Clone, Debug)]
pub struct PickerEntry {
    pub label: String,
    pub op: ControlOp,
}
#[derive(Clone, Debug)]
pub struct ModelChoice {
    pub provider: String,
    pub id: String,
    pub label: String,
}
#[derive(Clone, Debug)]
pub enum ControlData {
    Models {
        models: Vec<ModelChoice>,
        thinking_levels: Vec<String>,
    },
    Done(String),
}
#[derive(Clone, Debug)]
pub struct PendingControl {
    pub local: u64,
    pub target: Target,
    pub op: ControlOp,
    pub picker: Option<PickerKind>,
}
#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub until: Instant,
}
#[derive(Clone, Debug)]
pub struct ListSnapshot {
    pub rows: Vec<SessionRow>,
    pub total_all: usize,
    pub sessions_dir_missing: Option<PathBuf>,
}
#[derive(Clone, Debug)]
pub struct JobRef {
    pub id: String,
    pub handle: String,
}
#[derive(Clone, Debug)]
pub struct JobsSnapshot {
    pub rows: Vec<JobRow>,
    pub active_total: usize,
}
#[derive(Clone, Debug)]
pub struct InboxRecord {
    pub message: InboxMsg,
    pub position: InboxPosition,
}
#[derive(Clone, Debug, Default)]
pub struct InboxSnapshot {
    pub records: Vec<InboxRecord>,
    pub cursors: HashMap<String, crate::inbox::Cursor>,
}
pub enum Msg {
    SourceError {
        source: &'static str,
        error: ErrView,
    },
    InboxReset(Vec<String>),
    Key(KeyEvent),
    Paste(String),
    Resize(u16, u16),
    Tick {
        now: Instant,
        wall: SystemTime,
    },
    Signal(i32),
    Sessions(ListSnapshot),
    Follow {
        r#gen: u64,
        ev: FollowMsg,
    },
    Submitted {
        local: u64,
        result: Result<JobRef, ErrView>,
    },
    Jobs(JobsSnapshot),
    Inbox(InboxSnapshot),
    ControlDone {
        local: u64,
        op: ControlOp,
        result: Result<ControlData, ErrView>,
    },
}
#[derive(Clone, Debug)]
pub enum FollowMsg {
    Opened {
        header: Header,
        items: Vec<Item>,
        dropped: usize,
        not_yet_persisted: bool,
    },
    NotYetPersisted,
    Items(Vec<Item>),
    Notice(String),
    Continued {
        to_id: String,
        to_path: Option<PathBuf>,
        handle: String,
    },
    Failed(ErrView),
}
pub enum Cmd {
    Follow {
        r#gen: u64,
        target: Target,
        opts: ItemOpts,
    },
    ListScope(Scope),
    Refresh,
    Submit {
        local: u64,
        target: Target,
        text: String,
        mode: DeliverAs,
    },
    Control {
        local: u64,
        target: Target,
        op: ControlOp,
    },
    Quit(i32),
}
