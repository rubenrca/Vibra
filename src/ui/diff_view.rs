//! Compact Changes navigation and the central Git review.
//!
//! - The review is one virtualized `list()` at line granularity (see
//!   [`crate::ui::diff_rows`]): one scroll for every file, only the visible
//!   slice renders, and a collapsed file has no body rows at all.
//! - The current file's header sticks to the top and is pushed away by the
//!   next one.
//! - Two layouts (unified / split) and optional wrapping, persisted in
//!   settings. Without wrapping, horizontal scrolling moves only the code
//!   plane, in sync across files and split halves, while gutters stay put.
//! - Lines take review comments that are pasted into the agent's terminal as
//!   one prompt.
//! - Scopes: working tree, branch changes, the latest agent turn (the tree as
//!   it stood when an agent started working vs. the tree now), and history.
//! - Syntax highlighting uses both whole files when available, so state
//!   opened outside a hunk (block comments, strings) still colors correctly.

mod changes_panel;
mod sidebar;
pub(crate) use sidebar::DiffFileIndexView;

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, DispatchPhase, Div, EventEmitter,
    FocusHandle, HighlightStyle, IntoElement, ListAlignment, ListOffset, ListState, PathBuilder,
    Render, Rgba, ScrollWheelEvent, SharedString, Stateful, StyledText, Task, TextStyle, Timer,
    WhiteSpace, Window, canvas, div, ease_out_quint, list, point, prelude::*, px, uniform_list,
};

use uuid::Uuid;

use crate::ports::files::FileEntryKind;
use crate::ports::git::{
    GitBranchChanges, GitBranchRef, GitCommit, GitCommitChanges, GitDiffRow, GitDiffRowKind,
    GitFileChange, GitFileStatus, GitHistory, GitPort, GitRepositorySnapshot,
};
use crate::ui::diff_document::DiffDocument;
use crate::ui::diff_rows::{
    BodyRow, CommentAnchor, CommentSide, DiffLayout, FlattenFile, FoldDirection, FoldReveal,
    ReviewComment, ReviewRow, body_row_anchors, body_rows, flatten, review_prompt,
};
use crate::ui::git_graph::{GitGraphRow, assign_commit_lanes};
use crate::ui::syntax::SyntaxSpan;
use crate::ui::theme::{MONO_FONT, colors, floating_surface, mix, surface_tint};
use crate::ui::workspace_view::{file_tree_icon, file_tree_icon_color};

const POLL_INTERVAL: Duration = Duration::from_millis(2_500);
const HISTORY_ROW_HEIGHT: f32 = 36.0;
const HISTORY_HEADER_HEIGHT: f32 = 24.0;
const GRAPH_LANE_WIDTH: f32 = 12.0;
const HISTORY_AUTHOR_WIDTH: f32 = 88.0;
const HISTORY_DATE_WIDTH: f32 = 88.0;
const HISTORY_SHA_WIDTH: f32 = 64.0;
const HISTORY_PAGE: usize = 250;

const FILE_HEADER_HEIGHT: f32 = 32.0;
const SECTION_HEIGHT: f32 = 32.0;
/// Row height at the default code size; other sizes keep the proportion.
const BASE_DIFF_FONT_SIZE: f32 = 12.0;
const BASE_DIFF_ROW_HEIGHT: f32 = 20.0;
const COMMENT_ACTION_WIDTH: f32 = 20.0;
const SPLIT_DIVIDER_WIDTH: f32 = 1.0;
const CODE_PADDING_LEFT: f32 = 8.0;
/// Breathing room after the widest line when scrolled fully right.
const CODE_PADDING_RIGHT: f32 = 24.0;
const COMMENT_BUTTON_SIZE: f32 = 16.0;
const FOLD_DURATION: Duration = Duration::from_millis(180);
/// Rows searched below the scroll top for the header that pushes the sticky one.
const STICKY_PUSH_SCAN: usize = 96;
const MAX_EXPANDED_DIFFS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum GitPanelMode {
    Worktree,
    Branch,
    LatestTurn,
    History,
}

impl GitPanelMode {
    const ALL: [Self; 4] = [
        Self::Worktree,
        Self::Branch,
        Self::LatestTurn,
        Self::History,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Worktree => "Working tree",
            Self::Branch => "Branch changes",
            Self::LatestTurn => "Latest turn",
            Self::History => "History",
        }
    }
}

/// The whole working tree frozen when an agent started its latest turn.
#[derive(Debug, Clone)]
struct TurnBaseline {
    tree: String,
    started: Instant,
    agent: String,
}

struct FileFold {
    expanding: bool,
    generation: u64,
}

struct CommentDraft {
    anchor: CommentAnchor,
    excerpt: String,
    body: String,
    /// The comment being edited; restored when the edit is cancelled.
    restore: Option<ReviewComment>,
}

/// Identity of a list row that survives a rebuild, used to keep the scroll
/// position anchored when files load, fold, or change layout.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RowKey {
    StagedSection,
    Header(String),
    Loading(String),
    Body(String, usize),
    ContextFold(String, usize),
    Comment(u64),
    Draft,
    Folding(String),
}

/// Paint-time numbers shared by every row of one frame.
#[derive(Debug, Clone, Copy)]
struct RowMetrics {
    font_size: f32,
    line_height: f32,
    hunk_height: f32,
    fold_height: f32,
    char_width: f32,
    wrap: bool,
    h_offset: f32,
}

impl RowMetrics {
    fn gutter_width(&self, max_line_number: usize) -> f32 {
        let digits = max_line_number.max(1).to_string().len().max(3) as f32;
        (digits * self.char_width * 0.9 + 28.0).ceil()
    }

    /// Analytic height of one body row without wrapping (drives the fold tween).
    fn body_row_height(&self, rows: &[GitDiffRow], row: BodyRow) -> f32 {
        match row {
            BodyRow::Line(index) => match rows.get(index).map(|row| row.kind) {
                Some(GitDiffRowKind::Hunk) => self.hunk_height,
                Some(GitDiffRowKind::Section) => SECTION_HEIGHT - 4.0,
                _ => self.line_height,
            },
            BodyRow::Split { .. } => self.line_height,
            BodyRow::Fold { .. } => self.fold_height,
        }
    }
}

pub struct DiffView {
    context_root: PathBuf,
    mode: GitPanelMode,
    mode_menu_open: bool,
    snapshot: Option<GitRepositorySnapshot>,
    branch_changes: Option<GitBranchChanges>,
    branches: Vec<GitBranchRef>,
    selected_base: Option<String>,
    selected_head: Option<String>,
    branch_picker: Option<bool>,
    branch_error: Option<SharedString>,
    history: Option<Arc<GitHistory>>,
    history_graph: Arc<Vec<GitGraphRow>>,
    selected_commit: Option<GitCommit>,
    commit_changes: Option<GitCommitChanges>,
    commit_error: Option<SharedString>,
    commit_refreshing: bool,
    commit_request_id: u64,
    _commit_task: Option<Task<()>>,
    /// Repository root → tree captured when an agent last started working there.
    turn_baselines: HashMap<PathBuf, TurnBaseline>,
    /// Baseline the shown turn changes were computed against.
    turn_baseline: Option<TurnBaseline>,
    turn_changes: Option<GitCommitChanges>,
    turn_error: Option<SharedString>,
    turn_refreshing: bool,
    turn_settled: bool,
    turn_request_id: u64,
    _turn_task: Option<Task<()>>,
    _baseline_tasks: Vec<Task<()>>,
    /// Paths currently expanded; multiple files may stay open.
    expanded: HashSet<String>,
    expanded_order: VecDeque<String>,
    /// Prepared diffs for expanded (and recently expanded) paths.
    documents: HashMap<String, CachedDiffDocument>,
    status_root: Option<PathBuf>,
    status_index: Arc<HashMap<String, GitFileStatus>>,
    panel_visible: bool,
    review_expanded: bool,
    selected_review_path: Option<String>,
    focus_handle: FocusHandle,
    refreshing: bool,
    /// First snapshot for the current root has finished (success, none, or error).
    snapshot_settled: bool,
    branch_refreshing: bool,
    history_refreshing: bool,
    error: Option<SharedString>,
    snapshot_request_id: u64,
    branch_request_id: u64,
    history_request_id: u64,
    /// Monotonic id so stale per-path loads are ignored.
    diff_request_id: u64,
    /// Path → request and source identity that own the in-flight load.
    pending_loads: HashMap<String, PendingDiffLoad>,
    git_port: Arc<dyn GitPort>,
    _snapshot_task: Option<Task<()>>,
    _branch_task: Option<Task<()>>,
    _history_task: Option<Task<()>>,
    _diff_tasks: Vec<Task<()>>,
    _poll_task: Task<()>,
    layout: DiffLayout,
    wrap: bool,
    font_size: f32,
    /// Monospace advance at `font_size`, measured during render.
    char_width: f32,
    list_state: ListState,
    rows: Arc<Vec<ReviewRow>>,
    /// Files in display order; `ReviewRow` file indices point here.
    row_files: Arc<Vec<GitFileChange>>,
    rows_signature: Option<u64>,
    /// Header to scroll to after the next row rebuild.
    pending_reveal: Option<String>,
    /// Shared horizontal scroll of the code plane (0 = flush left).
    h_offset: f32,
    h_max: f32,
    folds: HashMap<String, FileFold>,
    fold_generation: u64,
    /// Path → how far each of its unchanged-line folds has been opened.
    /// Reset whenever the file's diff is reloaded.
    fold_reveals: HashMap<String, HashMap<usize, FoldReveal>>,
    comments: Vec<ReviewComment>,
    review_delivery: Option<PendingReviewDelivery>,
    next_comment_id: u64,
    draft: Option<CommentDraft>,
    /// The review fills its tab (the default); off, it shares the center
    /// with the terminal.
    review_focused: bool,
    /// Set when a commit was opened from the Changes graph; closing its
    /// review returns the panel to the working tree.
    return_to_worktree: bool,
    changes: changes_panel::ChangesPanelState,
}

/// What a live fold bar needs to reveal its lines.
#[derive(Debug, Clone)]
struct FoldActions {
    /// List row, for unique element ids.
    ix: usize,
    path: String,
    fold: usize,
    len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffSource {
    repository: PathBuf,
    change: GitFileChange,
    against: Option<String>,
    head: Option<String>,
    worktree_version: Option<WorktreeFileVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorktreeFileVersion {
    length: u64,
    modified: Option<SystemTime>,
    changed_seconds: i64,
    changed_nanoseconds: i64,
    inode: u64,
}

impl DiffSource {
    fn new(
        snapshot: &GitRepositorySnapshot,
        change: &GitFileChange,
        against: Option<&str>,
        head: Option<&str>,
    ) -> Self {
        let worktree_version = std::fs::symlink_metadata(snapshot.root.join(&change.path))
            .ok()
            .map(|metadata| WorktreeFileVersion {
                length: metadata.len(),
                modified: metadata.modified().ok(),
                changed_seconds: metadata.ctime(),
                changed_nanoseconds: metadata.ctime_nsec(),
                inode: metadata.ino(),
            });
        Self {
            repository: snapshot.root.clone(),
            change: change.clone(),
            against: against.map(str::to_owned),
            worktree_version: if head.is_some() {
                None
            } else {
                worktree_version
            },
            head: head.map(str::to_owned),
        }
    }
}

struct CachedDiffDocument {
    source: DiffSource,
    document: Arc<DiffDocument>,
}

struct PendingDiffLoad {
    request_id: u64,
    source: DiffSource,
}

struct PendingReviewDelivery {
    id: Uuid,
    comment_ids: Vec<u64>,
}

pub enum DiffViewEvent {
    /// Status, layout, or expansion changed; the workspace repaints.
    Changed,
    /// The user closed the central review and expects keyboard input in the terminal.
    ReturnToTerminal,
    /// The toolbar changed a persisted preference.
    PreferencesChanged { split: bool, wrap: bool },
    /// Review comments to paste into the agent's terminal.
    SendReview { prompt: String, delivery_id: Uuid },
    /// A file or commit was opened for review; show its tab.
    ReviewOpened,
    /// Open a terminal session that runs this command in the repository.
    RunInTerminal { title: String, command: String },
}

impl EventEmitter<DiffViewEvent> for DiffView {}

impl DiffView {
    pub fn new(context_root: PathBuf, git_port: Arc<dyn GitPort>, cx: &mut Context<Self>) -> Self {
        let poll_task = cx.spawn(async move |this, cx| {
            loop {
                Timer::after(POLL_INTERVAL).await;
                if this
                    .update(cx, |this, cx| {
                        if crate::ui::idle::should_poll_git_snapshot(this.panel_visible) {
                            this.refresh_visible_sources(false, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            context_root,
            mode: GitPanelMode::Worktree,
            mode_menu_open: false,
            snapshot: None,
            branch_changes: None,
            branches: Vec::new(),
            selected_base: None,
            selected_head: None,
            branch_picker: None,
            branch_error: None,
            history: None,
            history_graph: Arc::new(Vec::new()),
            selected_commit: None,
            commit_changes: None,
            commit_error: None,
            commit_refreshing: false,
            commit_request_id: 0,
            _commit_task: None,
            turn_baselines: HashMap::new(),
            turn_baseline: None,
            turn_changes: None,
            turn_error: None,
            turn_refreshing: false,
            turn_settled: false,
            turn_request_id: 0,
            _turn_task: None,
            _baseline_tasks: Vec::new(),
            expanded: HashSet::new(),
            expanded_order: VecDeque::new(),
            documents: HashMap::new(),
            status_root: None,
            status_index: Arc::new(HashMap::new()),
            panel_visible: false,
            review_expanded: false,
            selected_review_path: None,
            focus_handle: cx.focus_handle(),
            refreshing: false,
            snapshot_settled: false,
            branch_refreshing: false,
            history_refreshing: false,
            error: None,
            snapshot_request_id: 0,
            branch_request_id: 0,
            history_request_id: 0,
            diff_request_id: 0,
            pending_loads: HashMap::new(),
            git_port,
            _snapshot_task: None,
            _branch_task: None,
            _history_task: None,
            _diff_tasks: Vec::new(),
            _poll_task: poll_task,
            layout: DiffLayout::Unified,
            wrap: false,
            font_size: BASE_DIFF_FONT_SIZE,
            char_width: BASE_DIFF_FONT_SIZE * 0.6,
            list_state: ListState::new(0, ListAlignment::Top, px(400.0)),
            rows: Arc::new(Vec::new()),
            row_files: Arc::new(Vec::new()),
            rows_signature: None,
            pending_reveal: None,
            h_offset: 0.0,
            h_max: 0.0,
            folds: HashMap::new(),
            fold_generation: 0,
            fold_reveals: HashMap::new(),
            comments: Vec::new(),
            review_delivery: None,
            next_comment_id: 1,
            draft: None,
            review_focused: true,
            return_to_worktree: false,
            changes: changes_panel::ChangesPanelState::new(cx),
        }
    }

    /// Persisted review preferences, applied at startup and from Settings.
    pub fn set_preferences(
        &mut self,
        split: bool,
        wrap: bool,
        font_size: f32,
        cx: &mut Context<Self>,
    ) {
        let layout = DiffLayout::from_split(split);
        if self.layout == layout && self.wrap == wrap && self.font_size == font_size {
            return;
        }
        self.layout = layout;
        self.wrap = wrap;
        self.font_size = font_size;
        if wrap {
            self.h_offset = 0.0;
        }
        cx.notify();
    }

    /// Repo root + relative path → status, for coloring the Files tree (Zed-style).
    pub fn status_index(&self) -> (Option<PathBuf>, Arc<HashMap<String, GitFileStatus>>) {
        (self.status_root.clone(), self.status_index.clone())
    }

    pub fn review_expanded(&self) -> bool {
        self.review_expanded
    }

    /// The review hides the terminal; otherwise the two share the center.
    pub fn review_focused(&self) -> bool {
        self.review_expanded && self.review_focused
    }

    pub fn review_is_commit(&self) -> bool {
        self.mode == GitPanelMode::History && self.selected_commit.is_some()
    }

    pub fn review_title(&self) -> String {
        self.selected_commit
            .as_ref()
            .filter(|_| self.mode == GitPanelMode::History)
            .map(|commit| commit.subject.clone())
            .unwrap_or_else(|| self.mode.label().to_owned())
    }

    pub fn focus_review(&self, window: &mut Window) {
        self.focus_handle.focus(window);
    }

    fn open_review_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_review_path = Some(path.clone());
        self.pending_reveal = Some(path.clone());
        self.expand_path(path, cx);
        self.set_review_expanded(true, cx);
        self.focus_handle.focus(window);
        cx.emit(DiffViewEvent::ReviewOpened);
        cx.emit(DiffViewEvent::Changed);
        cx.notify();
    }

    pub fn toggle_review_expanded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_review_expanded(!self.review_expanded, cx);
        if self.review_expanded {
            self.focus_handle.focus(window);
        } else {
            cx.emit(DiffViewEvent::ReturnToTerminal);
        }
    }

    pub fn set_review_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        if self.review_expanded == expanded {
            return;
        }
        self.review_expanded = expanded;
        self.mode_menu_open = false;
        if expanded {
            cx.emit(DiffViewEvent::ReviewOpened);
        } else {
            self.review_focused = true;
            if std::mem::take(&mut self.return_to_worktree) {
                self.set_mode(GitPanelMode::Worktree, cx);
            }
        }
        cx.emit(DiffViewEvent::Changed);
        cx.notify();
    }

    pub fn set_panel_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.panel_visible == visible {
            return;
        }
        self.panel_visible = visible;
        if visible {
            self.refresh_visible_sources(false, cx);
        }
    }

    fn rebuild_status_index(
        snapshot: Option<&GitRepositorySnapshot>,
    ) -> (Option<PathBuf>, Arc<HashMap<String, GitFileStatus>>) {
        match snapshot {
            Some(snapshot) => {
                let map = snapshot
                    .changes
                    .iter()
                    .map(|change| (change.path.replace('\\', "/"), change.status))
                    .collect();
                (Some(snapshot.root.clone()), Arc::new(map))
            }
            None => (None, Arc::new(HashMap::new())),
        }
    }

    /// Open a changed file in the Diff panel (if it has git status).
    pub fn select_path_if_changed(&mut self, relative_path: &str, cx: &mut Context<Self>) -> bool {
        let relative_path = relative_path.replace('\\', "/");
        let exists = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.changes.iter().any(|c| c.path == relative_path));
        if !exists {
            return false;
        }
        self.set_mode(GitPanelMode::Worktree, cx);
        self.selected_review_path = Some(relative_path.clone());
        self.pending_reveal = Some(relative_path.clone());
        self.expand_path(relative_path, cx);
        self.set_review_expanded(true, cx);
        cx.emit(DiffViewEvent::ReviewOpened);
        cx.notify();
        true
    }

    /// An agent in `cwd` started working: freeze the tree so the Latest turn
    /// scope can show exactly what this turn changes.
    pub fn mark_turn_started(&mut self, cwd: PathBuf, agent: String, cx: &mut Context<Self>) {
        let port = self.git_port.clone();
        let started = Instant::now();
        let task = cx.background_spawn(async move { port.capture_worktree(&cwd) });
        self._baseline_tasks.push(cx.spawn(async move |this, cx| {
            let Ok(Some(capture)) = task.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                // A later turn in the same repository may already have landed.
                if this
                    .turn_baselines
                    .get(&capture.root)
                    .is_some_and(|baseline| baseline.started > started)
                {
                    return;
                }
                this.turn_baselines.insert(
                    capture.root,
                    TurnBaseline {
                        tree: capture.tree,
                        started,
                        agent,
                    },
                );
                if this.mode == GitPanelMode::LatestTurn {
                    this.refresh_turn(false, cx);
                }
                cx.notify();
            });
        }));
        if self._baseline_tasks.len() > 8 {
            for task in self
                ._baseline_tasks
                .drain(0..self._baseline_tasks.len() - 4)
            {
                task.detach();
            }
        }
    }

    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.context_root == root {
            return;
        }
        self.set_review_expanded(false, cx);
        self.clear_commit();
        self.clear_turn();
        self.forget_scroll();
        self.changes.reset_project(cx);
        self.context_root = root;
        self.selected_review_path = None;
        self.comments.clear();
        self.review_delivery = None;
        self.draft = None;
        // A hidden panel can stay hidden indefinitely. Drop the previous
        // repository immediately so file selection and status colors never
        // use another project's snapshot while the new one is loading.
        self.snapshot = None;
        self.status_root = None;
        self.status_index = Arc::new(HashMap::new());
        self.selected_base = None;
        self.selected_head = None;
        self.branches.clear();
        self.branch_picker = None;
        self.branch_error = None;
        self.snapshot_request_id = self.snapshot_request_id.wrapping_add(1);
        self.branch_request_id = self.branch_request_id.wrapping_add(1);
        self.history_request_id = self.history_request_id.wrapping_add(1);
        self.diff_request_id = self.diff_request_id.wrapping_add(1);
        self.refreshing = false;
        self.snapshot_settled = false;
        self.branch_refreshing = false;
        self.history_refreshing = false;
        self.branch_changes = None;
        self.history = None;
        self.history_graph = Arc::new(Vec::new());
        self.error = None;
        self.mode_menu_open = false;
        self.draft = None;
        self.h_offset = 0.0;
        cx.emit(DiffViewEvent::Changed);
        cx.notify();
        if self.panel_visible {
            self.refresh_visible_sources(true, cx);
        }
    }

    pub fn refresh_now(&mut self, cx: &mut Context<Self>) {
        self.refresh_visible_sources(true, cx);
    }

    /// Quiet refresh from workspace file events (no loading flash).
    pub fn refresh_from_fs_event(&mut self, cx: &mut Context<Self>) {
        if self.panel_visible {
            self.refresh_visible_sources(false, cx);
        }
    }

    fn refresh_visible_sources(&mut self, notify_loading: bool, cx: &mut Context<Self>) {
        self.refresh(notify_loading, cx);
        match self.mode {
            GitPanelMode::Worktree => self.refresh_graph(notify_loading, cx),
            GitPanelMode::Branch => self.refresh_branch(notify_loading, cx),
            GitPanelMode::LatestTurn => self.refresh_turn(notify_loading, cx),
            GitPanelMode::History => {
                if self.selected_commit.is_none() {
                    self.refresh_history(notify_loading, cx);
                } else if notify_loading {
                    self.refresh_commit(cx);
                }
            }
        }
    }

    fn set_mode(&mut self, mode: GitPanelMode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.selected_review_path = None;
        }
        self.mode_menu_open = false;
        self.branch_picker = None;
        if self.mode == mode {
            if mode == GitPanelMode::History && self.selected_commit.is_some() {
                self.back_to_history(cx);
            }
            cx.notify();
            return;
        }
        self.clear_commit();
        self.clear_turn();
        self.forget_scroll();
        self.mode = mode;
        self.comments.clear();
        self.review_delivery = None;
        self.draft = None;
        self.h_offset = 0.0;
        match mode {
            GitPanelMode::Worktree => {}
            GitPanelMode::Branch => self.refresh_branch(true, cx),
            GitPanelMode::LatestTurn => self.refresh_turn(true, cx),
            GitPanelMode::History => self.refresh_history(true, cx),
        }
        cx.emit(DiffViewEvent::Changed);
        cx.notify();
    }

    fn refresh(&mut self, notify_loading: bool, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        if notify_loading {
            self.error = None;
            cx.notify();
        }
        self.snapshot_request_id = self.snapshot_request_id.wrapping_add(1);
        let request_id = self.snapshot_request_id;
        let root = self.context_root.clone();
        let port = self.git_port.clone();
        let task = cx.background_spawn(async move { port.snapshot(&root) });
        self._snapshot_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if request_id != this.snapshot_request_id {
                    return;
                }
                this.refreshing = false;
                let mut changed = !this.snapshot_settled;
                this.snapshot_settled = true;
                match result {
                    Ok(Some(snapshot)) => {
                        changed = this.snapshot.as_ref() != Some(&snapshot);
                        this.apply_snapshot(snapshot, cx);
                    }
                    Ok(None) => {
                        if this.snapshot.is_some()
                            || this.status_root.is_some()
                            || this.error.is_some()
                        {
                            changed = true;
                        }
                        this.snapshot = None;
                        this.status_root = None;
                        this.status_index = Arc::new(HashMap::new());
                        if this.mode == GitPanelMode::Worktree {
                            this.expanded.clear();
                            this.documents.clear();
                            this.pending_loads.clear();
                        }
                        this.error = None;
                    }
                    Err(error) => {
                        this.error = Some(format!("Git: {error:#}").into());
                        changed = true;
                    }
                }
                if changed {
                    cx.notify();
                }
            });
        }));
    }

    fn change_branch_selection(
        &mut self,
        is_base: bool,
        reference: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.comments.clear();
        self.review_delivery = None;
        self.draft = None;
        if is_base {
            self.selected_base = reference;
        } else {
            self.selected_head = reference;
        }
        self.branch_picker = None;
        self.branch_request_id = self.branch_request_id.wrapping_add(1);
        self._branch_task = None;
        self.branch_refreshing = false;
        self.branch_changes = None;
        self.expanded.clear();
        self.documents.clear();
        self.pending_loads.clear();
        self.refresh_branch(true, cx);
    }

    fn branch_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .w_full()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children([true, false].into_iter().map(|is_base| {
                        let selected = if is_base {
                            &self.selected_base
                        } else {
                            &self.selected_head
                        };
                        let label = selected
                            .as_ref()
                            .map(|reference| {
                                self.branches
                                    .iter()
                                    .find(|branch| &branch.reference == reference)
                                    .map(|branch| branch.name.as_str())
                                    .unwrap_or(reference)
                            })
                            .unwrap_or(if is_base { "Auto" } else { "Working tree" });
                        div()
                            .id(if is_base {
                                "branch-base"
                            } else {
                                "branch-head"
                            })
                            .px_2()
                            .py_1()
                            .rounded(px(5.0))
                            .bg(colors().elevated)
                            .text_size(px(11.0))
                            .text_color(colors().foreground)
                            .cursor_pointer()
                            .hover(|view| view.bg(colors().hover))
                            .child(format!(
                                "{}: {} ▾",
                                if is_base { "Base" } else { "Compare" },
                                label
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.branch_picker = if this.branch_picker == Some(is_base) {
                                    None
                                } else {
                                    Some(is_base)
                                };
                                cx.notify();
                            }))
                    })),
            )
            .when_some(self.branch_picker, |view, is_base| {
                let options = std::iter::once((
                    None,
                    if is_base {
                        "Auto base".to_owned()
                    } else {
                        "Working tree".to_owned()
                    },
                ))
                .chain(self.branches.iter().map(|branch| {
                    (
                        Some(branch.reference.clone()),
                        format!(
                            "{} · {}",
                            branch.name,
                            if branch.remote { "Remote" } else { "Local" }
                        ),
                    )
                }));
                view.child(
                    div()
                        .id("branch-options")
                        .max_h(px(180.0))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .children(options.enumerate().map(|(index, (reference, label))| {
                            div()
                                .id(("branch-option", index))
                                .px_2()
                                .py_1()
                                .flex_none()
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(colors().foreground)
                                .hover(|row| row.bg(surface_tint(colors().hover, colors().panel)))
                                .child(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.change_branch_selection(is_base, reference.clone(), cx);
                                }))
                        })),
                )
            })
            .child(div().text_size(px(10.0)).text_color(colors().subtle).child(
                if self.selected_head.is_some() {
                    "Saved branch versions · remote refs from last fetch"
                } else if self.selected_base.is_some() {
                    "Working tree vs selected base · includes uncommitted changes"
                } else {
                    "Working tree vs common ancestor · includes uncommitted changes"
                },
            ))
    }

    fn refresh_branch(&mut self, notify_loading: bool, cx: &mut Context<Self>) {
        if self.branch_refreshing {
            return;
        }
        self.branch_refreshing = true;
        self.branch_error = None;
        if notify_loading {
            cx.notify();
        }
        self.branch_request_id = self.branch_request_id.wrapping_add(1);
        let request_id = self.branch_request_id;
        let root = self.context_root.clone();
        let port = self.git_port.clone();
        let base = self.selected_base.clone();
        let head = self.selected_head.clone();
        let task = cx.background_spawn(async move {
            let branches = port.branches(&root);
            let changes = port.branch_changes(&root, base.as_deref(), head.as_deref());
            (branches, changes)
        });
        self._branch_task = Some(cx.spawn(async move |this, cx| {
            let (branches, result) = task.await;
            let _ = this.update(cx, |this, cx| {
                if request_id != this.branch_request_id {
                    return;
                }
                this.branch_refreshing = false;
                if let Ok(branches) = branches {
                    this.branches = branches;
                }
                match result {
                    Ok(Some(changes)) => {
                        if this.mode == GitPanelMode::Branch {
                            let against = (!changes.base_revision.is_empty())
                                .then(|| changes.base_revision.clone());
                            this.reconcile_documents(
                                &changes.snapshot,
                                against.as_deref(),
                                changes.head_revision.as_deref(),
                            );
                        }
                        this.branch_changes = Some(changes);
                        if this.mode == GitPanelMode::Branch {
                            this.load_missing_expanded(cx);
                        }
                    }
                    Ok(None) => this.branch_changes = None,
                    Err(error) => {
                        this.branch_error = Some(format!("Git: {error:#}").into());
                        this.branch_changes = None;
                        if this.mode == GitPanelMode::Branch {
                            this.documents.clear();
                            this.pending_loads.clear();
                        }
                    }
                }
                cx.notify();
            });
        }));
    }

    fn refresh_history(&mut self, notify_loading: bool, cx: &mut Context<Self>) {
        if self.history_refreshing {
            return;
        }
        self.history_refreshing = true;
        if notify_loading {
            cx.notify();
        }
        self.history_request_id = self.history_request_id.wrapping_add(1);
        let request_id = self.history_request_id;
        let root = self.context_root.clone();
        let port = self.git_port.clone();
        let task = cx.background_spawn(async move { port.history(&root, HISTORY_PAGE) });
        self._history_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if request_id != this.history_request_id {
                    return;
                }
                this.history_refreshing = false;
                match result {
                    Ok(Some(history)) => {
                        this.history_graph = Arc::new(assign_commit_lanes(&history.commits));
                        this.history = Some(Arc::new(history));
                    }
                    Ok(None) => {
                        this.history = None;
                        this.history_graph = Arc::new(Vec::new());
                    }
                    Err(error) => this.error = Some(format!("Git: {error:#}").into()),
                }
                cx.notify();
            });
        }));
    }

    /// A different scope or commit starts at the top instead of anchoring to
    /// whatever row happened to share a path.
    fn forget_scroll(&mut self) {
        self.rows = Arc::new(Vec::new());
        self.rows_signature = None;
        self.list_state.scroll_to(ListOffset::default());
    }

    fn clear_turn(&mut self) {
        self.turn_request_id = self.turn_request_id.wrapping_add(1);
        self._turn_task = None;
        self.turn_refreshing = false;
        self.turn_settled = false;
        self.turn_changes = None;
        self.turn_baseline = None;
        self.turn_error = None;
    }

    /// Capture the tree as it is now and compare it with the latest baseline
    /// recorded for this repository.
    fn refresh_turn(&mut self, notify_loading: bool, cx: &mut Context<Self>) {
        if self.turn_refreshing {
            return;
        }
        self.turn_refreshing = true;
        if notify_loading {
            self.turn_error = None;
            cx.notify();
        }
        self.turn_request_id = self.turn_request_id.wrapping_add(1);
        let request_id = self.turn_request_id;
        let root = self.context_root.clone();
        let port = self.git_port.clone();
        let baselines = self.turn_baselines.clone();
        let task = cx.background_spawn(async move {
            let Some(current) = port.capture_worktree(&root)? else {
                return Ok::<_, anyhow::Error>(None);
            };
            let Some(baseline) = baselines.get(&current.root).cloned() else {
                return Ok(Some((None, None)));
            };
            let changes = port.tree_changes(&current.root, &baseline.tree, &current.tree)?;
            Ok(Some((Some(baseline), Some(changes))))
        });
        self._turn_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if request_id != this.turn_request_id {
                    return;
                }
                this.turn_refreshing = false;
                this.turn_settled = true;
                match result {
                    Ok(Some((baseline, changes))) => {
                        this.turn_error = None;
                        if let Some(changes) = &changes
                            && this.mode == GitPanelMode::LatestTurn
                        {
                            this.reconcile_documents(
                                &changes.snapshot,
                                Some(&changes.base_revision),
                                Some(&changes.revision),
                            );
                        }
                        let first_load = this.turn_changes.is_none();
                        this.turn_baseline = baseline;
                        this.turn_changes = changes;
                        if this.mode == GitPanelMode::LatestTurn {
                            if first_load
                                && this.expanded.is_empty()
                                && let Some(first) = this
                                    .turn_changes
                                    .as_ref()
                                    .and_then(|changes| changes.snapshot.changes.first())
                            {
                                this.expanded.insert(first.path.clone());
                            }
                            this.load_missing_expanded(cx);
                        }
                    }
                    Ok(None) => {
                        this.turn_baseline = None;
                        this.turn_changes = None;
                    }
                    Err(error) => this.turn_error = Some(format!("Git: {error:#}").into()),
                }
                cx.notify();
            });
        }));
    }

    fn clear_commit(&mut self) {
        self.commit_request_id = self.commit_request_id.wrapping_add(1);
        self._commit_task = None;
        self.selected_commit = None;
        self.commit_changes = None;
        self.commit_error = None;
        self.error = None;
        self.commit_refreshing = false;
        self.expanded.clear();
        self.documents.clear();
        self.pending_loads.clear();
        self.folds.clear();
    }

    fn back_to_history(&mut self, cx: &mut Context<Self>) {
        self.comments.clear();
        self.review_delivery = None;
        self.draft = None;
        self.clear_commit();
        self.refresh_history(false, cx);
        cx.emit(DiffViewEvent::Changed);
        cx.notify();
    }

    fn select_commit(&mut self, commit: GitCommit, cx: &mut Context<Self>) {
        self.comments.clear();
        self.review_delivery = None;
        self.draft = None;
        self.clear_commit();
        self.forget_scroll();
        self.selected_commit = Some(commit);
        self.set_review_expanded(true, cx);
        self.refresh_commit(cx);
        cx.emit(DiffViewEvent::ReviewOpened);
        cx.emit(DiffViewEvent::Changed);
    }

    fn refresh_commit(&mut self, cx: &mut Context<Self>) {
        let Some(commit) = self.selected_commit.as_ref() else {
            return;
        };
        if self.commit_refreshing {
            return;
        }
        let revision = commit.sha.clone();
        self.commit_refreshing = true;
        self.commit_error = None;
        self.commit_request_id = self.commit_request_id.wrapping_add(1);
        let request_id = self.commit_request_id;
        let root = self.context_root.clone();
        let port = self.git_port.clone();
        let task = cx.background_spawn(async move { port.commit_changes(&root, &revision) });
        self._commit_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if request_id != this.commit_request_id {
                    return;
                }
                this.commit_refreshing = false;
                match result {
                    Ok(changes) => {
                        let first_load = this.commit_changes.is_none();
                        this.reconcile_documents(
                            &changes.snapshot,
                            Some(&changes.base_revision),
                            Some(&changes.revision),
                        );
                        if first_load && let Some(first) = changes.snapshot.changes.first() {
                            this.expanded.insert(first.path.clone());
                        }
                        this.commit_changes = Some(changes);
                        this.load_missing_expanded(cx);
                    }
                    Err(error) => this.commit_error = Some(format!("Git: {error:#}").into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Discard documents and in-flight work whose Git source no longer matches
    /// the latest snapshot. A path remaining present is not enough: staging or
    /// changing it must invalidate the prepared rows as well.
    fn reconcile_documents(
        &mut self,
        snapshot: &GitRepositorySnapshot,
        against: Option<&str>,
        head: Option<&str>,
    ) {
        let sources: HashMap<String, DiffSource> = snapshot
            .changes
            .iter()
            .map(|change| {
                (
                    change.path.clone(),
                    DiffSource::new(snapshot, change, against, head),
                )
            })
            .collect();

        self.expanded.retain(|path| sources.contains_key(path));
        if self
            .selected_review_path
            .as_ref()
            .is_some_and(|path| !sources.contains_key(path))
        {
            self.selected_review_path = None;
        }
        self.expanded_order
            .retain(|path| self.expanded.contains(path));
        self.documents.retain(|path, cached| {
            sources
                .get(path)
                .is_some_and(|source| source == &cached.source)
        });
        self.pending_loads.retain(|path, pending| {
            sources
                .get(path)
                .is_some_and(|source| source == &pending.source)
        });
        self.folds.retain(|path, _| sources.contains_key(path));
        self.fold_reveals
            .retain(|path, _| self.documents.contains_key(path));
        self.evict_diff_caches();
    }

    fn load_missing_expanded(&mut self, cx: &mut Context<Self>) {
        let to_reload: Vec<String> = self
            .expanded
            .iter()
            .filter(|path| {
                !self.documents.contains_key(*path) && !self.pending_loads.contains_key(*path)
            })
            .cloned()
            .collect();
        for path in to_reload {
            self.load_diff(path, cx);
        }
    }

    fn apply_snapshot(&mut self, snapshot: GitRepositorySnapshot, cx: &mut Context<Self>) {
        self.error = None;
        let snapshot_unchanged = self.snapshot.as_ref() == Some(&snapshot);
        if self.mode == GitPanelMode::Worktree {
            self.reconcile_documents(&snapshot, None, None);
        }
        if snapshot_unchanged {
            if self.mode == GitPanelMode::Worktree {
                self.load_missing_expanded(cx);
            }
            return;
        }
        let (root, index) = Self::rebuild_status_index(Some(&snapshot));
        self.status_root = root;
        self.status_index = index;
        self.snapshot = Some(snapshot);
        if self.mode == GitPanelMode::Worktree {
            self.load_missing_expanded(cx);
        }
        cx.emit(DiffViewEvent::Changed);
    }

    fn evict_diff_caches(&mut self) {
        if self.documents.len() <= MAX_EXPANDED_DIFFS {
            return;
        }
        self.documents
            .retain(|path, _| self.expanded.contains(path));
    }

    fn expand_path(&mut self, path: String, cx: &mut Context<Self>) {
        if self.expanded.insert(path.clone()) {
            self.expanded_order
                .retain(|candidate| self.expanded.contains(candidate) && candidate != &path);
            self.expanded_order.push_back(path.clone());
            while self.expanded.len() > MAX_EXPANDED_DIFFS {
                let Some(oldest) = self.expanded_order.pop_front() else {
                    break;
                };
                self.expanded.remove(&oldest);
                self.folds.remove(&oldest);
                self.pending_loads.remove(&oldest);
            }
            self.evict_diff_caches();
            self.load_diff(path, cx);
            cx.emit(DiffViewEvent::Changed);
            cx.notify();
        } else if !self.documents.contains_key(&path) && !self.pending_loads.contains_key(&path) {
            self.load_diff(path, cx);
            cx.notify();
        }
    }

    fn toggle_path(&mut self, path: String, cx: &mut Context<Self>) {
        // Only a prepared, unwrapped body has the analytic height the tween needs.
        let animate = self.documents.contains_key(&path) && !self.wrap;
        if self.expanded.contains(&path) {
            self.expanded.remove(&path);
            self.expanded_order.retain(|candidate| candidate != &path);
            self.pending_loads.remove(&path);
            // Keep cached diff so re-expand is instant.
            if animate {
                self.start_fold(path, false, cx);
            }
            cx.emit(DiffViewEvent::Changed);
            cx.notify();
            return;
        }
        if animate {
            self.start_fold(path.clone(), true, cx);
        }
        self.expand_path(path, cx);
    }

    /// Open every file (up to the cap on simultaneously prepared diffs).
    fn expand_all(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<String> = self
            .ordered_file_refs()
            .map(|change| change.path.clone())
            .take(MAX_EXPANDED_DIFFS)
            .collect();
        for path in paths {
            self.expand_path(path, cx);
        }
    }

    fn collapse_all(&mut self, cx: &mut Context<Self>) {
        if self.expanded.is_empty() {
            return;
        }
        self.expanded.clear();
        self.expanded_order.clear();
        self.pending_loads.clear();
        self.folds.clear();
        cx.emit(DiffViewEvent::Changed);
        cx.notify();
    }

    fn start_fold(&mut self, path: String, expanding: bool, cx: &mut Context<Self>) {
        self.fold_generation = self.fold_generation.wrapping_add(1);
        let generation = self.fold_generation;
        self.folds.insert(
            path.clone(),
            FileFold {
                expanding,
                generation,
            },
        );
        let delay = cx.background_executor().timer(FOLD_DURATION);
        cx.spawn(async move |this, cx| {
            delay.await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .folds
                    .get(&path)
                    .is_some_and(|fold| fold.generation == generation)
                {
                    this.folds.remove(&path);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn active_snapshot(&self) -> Option<&GitRepositorySnapshot> {
        match self.mode {
            GitPanelMode::Worktree => self.snapshot.as_ref(),
            GitPanelMode::Branch => self
                .branch_changes
                .as_ref()
                .map(|changes| &changes.snapshot),
            GitPanelMode::LatestTurn => self.turn_changes.as_ref().map(|changes| &changes.snapshot),
            GitPanelMode::History => self
                .commit_changes
                .as_ref()
                .map(|changes| &changes.snapshot),
        }
    }

    /// The revisions the active scope compares: `(against, head)`.
    fn active_revisions(&self) -> (Option<&str>, Option<&str>) {
        match self.mode {
            GitPanelMode::Branch => self
                .branch_changes
                .as_ref()
                .map(|changes| {
                    (
                        (!changes.base_revision.is_empty())
                            .then_some(changes.base_revision.as_str()),
                        changes.head_revision.as_deref(),
                    )
                })
                .unwrap_or_default(),
            GitPanelMode::LatestTurn => self
                .turn_changes
                .as_ref()
                .map(|changes| {
                    (
                        Some(changes.base_revision.as_str()),
                        Some(changes.revision.as_str()),
                    )
                })
                .unwrap_or_default(),
            GitPanelMode::History => self
                .commit_changes
                .as_ref()
                .map(|changes| {
                    (
                        Some(changes.base_revision.as_str()),
                        Some(changes.revision.as_str()),
                    )
                })
                .unwrap_or_default(),
            GitPanelMode::Worktree => (None, None),
        }
    }

    fn load_diff(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(source) = self.active_snapshot().and_then(|snapshot| {
            let change = snapshot
                .changes
                .iter()
                .find(|change| change.path == path)?
                .clone();
            let (against, head) = self.active_revisions();
            Some(DiffSource::new(snapshot, &change, against, head))
        }) else {
            return;
        };

        if self
            .documents
            .get(&path)
            .is_some_and(|cached| cached.source == source)
            || self
                .pending_loads
                .get(&path)
                .is_some_and(|pending| pending.source == source)
        {
            return;
        }

        self.diff_request_id = self.diff_request_id.wrapping_add(1);
        let request_id = self.diff_request_id;
        self.pending_loads.insert(
            path.clone(),
            PendingDiffLoad {
                request_id,
                source: source.clone(),
            },
        );
        let port = self.git_port.clone();
        let source_for_task = source.clone();
        let task = cx.background_spawn(async move {
            let source = &source_for_task;
            let diff = if let Some(revision) = &source.against {
                port.diff_against(
                    &source.repository,
                    revision,
                    source.head.as_deref(),
                    &source.change,
                )
            } else {
                port.diff(&source.repository, &source.change)
            }?;
            // Whole files are optional: without them each row is highlighted
            // on its own, exactly as before.
            let sources = port
                .diff_sources(
                    &source.repository,
                    &source.change,
                    source.against.as_deref(),
                    source.head.as_deref(),
                )
                .ok();
            Ok::<_, anyhow::Error>(DiffDocument::prepare_with_sources(diff, sources.as_ref()))
        });
        let path_for_task = path.clone();
        self._diff_tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(pending) = this.pending_loads.get(&path_for_task) else {
                    return;
                };
                if pending.request_id != request_id || pending.source != source {
                    return;
                }
                this.pending_loads.remove(&path_for_task);
                match result {
                    Ok(document) => {
                        this.fold_reveals.remove(&path_for_task);
                        this.documents.insert(
                            path_for_task,
                            CachedDiffDocument {
                                source,
                                document: Arc::new(document),
                            },
                        );
                        this.evict_diff_caches();
                        this.error = None;
                    }
                    Err(error) => this.error = Some(format!("Git: {error:#}").into()),
                }
                cx.notify();
            });
        }));
        // Keep the handle list bounded without cancelling an in-flight load.
        // Cancelling its callback would leave `pending_loads` stuck forever.
        if self._diff_tasks.len() > 12 {
            for task in self._diff_tasks.drain(0..self._diff_tasks.len() - 8) {
                task.detach();
            }
        }
        cx.notify();
    }

    fn status_color(status: GitFileStatus) -> Rgba {
        match status {
            GitFileStatus::Added | GitFileStatus::Untracked => colors().diff_added,
            GitFileStatus::Conflicted => colors().warning,
            GitFileStatus::Deleted => colors().diff_deleted,
            GitFileStatus::Renamed | GitFileStatus::Copied => colors().accent,
            GitFileStatus::Modified | GitFileStatus::TypeChanged => colors().warning,
        }
    }

    fn path_parts(path: &str) -> (String, String) {
        let path_buf = std::path::Path::new(path);
        let name = path_buf
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_owned());
        let parent = path_buf
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .filter(|parent| !parent.is_empty() && parent != ".")
            .unwrap_or_default();
        (name, parent)
    }

    // -----------------------------------------------------------------------
    // Preferences and comments
    // -----------------------------------------------------------------------

    fn set_layout(&mut self, layout: DiffLayout, cx: &mut Context<Self>) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        cx.emit(DiffViewEvent::PreferencesChanged {
            split: layout.is_split(),
            wrap: self.wrap,
        });
        cx.notify();
    }

    fn toggle_wrap(&mut self, cx: &mut Context<Self>) {
        self.wrap = !self.wrap;
        self.h_offset = 0.0;
        cx.emit(DiffViewEvent::PreferencesChanged {
            split: self.layout.is_split(),
            wrap: self.wrap,
        });
        cx.notify();
    }

    fn open_draft(
        &mut self,
        anchor: CommentAnchor,
        excerpt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_draft();
        self.draft = Some(CommentDraft {
            anchor,
            excerpt,
            body: String::new(),
            restore: None,
        });
        self.focus_handle.focus(window);
        cx.notify();
    }

    /// Keep a non-empty draft as a comment; drop an empty one.
    fn commit_draft(&mut self) {
        let Some(draft) = self.draft.take() else {
            return;
        };
        let body = draft.body.trim().to_owned();
        if body.is_empty() {
            if let Some(restore) = draft.restore {
                self.comments.push(restore);
            }
            return;
        }
        let id = draft.restore.as_ref().map_or_else(
            || {
                let id = self.next_comment_id;
                self.next_comment_id += 1;
                id
            },
            |comment| comment.id,
        );
        self.comments.push(ReviewComment {
            id,
            anchor: draft.anchor,
            excerpt: draft.excerpt,
            body,
        });
        self.comments.sort_by(|left, right| {
            (&left.anchor.path, left.anchor.line, left.id).cmp(&(
                &right.anchor.path,
                right.anchor.line,
                right.id,
            ))
        });
    }

    fn cancel_draft(&mut self, cx: &mut Context<Self>) {
        if let Some(restore) = self.draft.take().and_then(|draft| draft.restore) {
            self.comments.push(restore);
            self.comments.sort_by(|left, right| {
                (&left.anchor.path, left.anchor.line, left.id).cmp(&(
                    &right.anchor.path,
                    right.anchor.line,
                    right.id,
                ))
            });
        }
        cx.notify();
    }

    fn edit_comment(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_draft();
        let Some(index) = self.comments.iter().position(|comment| comment.id == id) else {
            return;
        };
        let comment = self.comments.remove(index);
        self.draft = Some(CommentDraft {
            anchor: comment.anchor.clone(),
            excerpt: comment.excerpt.clone(),
            body: comment.body.clone(),
            restore: Some(comment),
        });
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn delete_comment(&mut self, id: u64, cx: &mut Context<Self>) {
        self.comments.retain(|comment| comment.id != id);
        cx.notify();
    }

    fn send_review(&mut self, cx: &mut Context<Self>) {
        self.commit_draft();
        if self.comments.is_empty() || self.review_delivery.is_some() {
            return;
        }
        let prompt = review_prompt(&self.comments);
        let comment_ids = self.comments.iter().map(|comment| comment.id).collect();
        let delivery_id = Uuid::new_v4();
        self.review_delivery = Some(PendingReviewDelivery {
            id: delivery_id,
            comment_ids,
        });
        cx.emit(DiffViewEvent::SendReview {
            prompt,
            delivery_id,
        });
        cx.notify();
    }

    /// A late acknowledgement belongs only to its original delivery. Changing
    /// repository or review scope invalidates it, even if a new send is pending.
    pub fn resolve_review_delivery(
        &mut self,
        delivery_id: Uuid,
        accepted: bool,
        cx: &mut Context<Self>,
    ) {
        if self
            .review_delivery
            .as_ref()
            .is_none_or(|delivery| delivery.id != delivery_id)
        {
            return;
        }
        let delivery = self.review_delivery.take().expect("delivery checked above");
        if accepted {
            self.comments
                .retain(|comment| !delivery.comment_ids.contains(&comment.id));
        }
        cx.notify();
    }

    fn on_key_down(&mut self, event: &gpui::KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = &event.keystroke.modifiers;
        let Some(draft) = self.draft.as_mut() else {
            if self.review_expanded && matches!(key, "escape" | "esc") {
                self.set_review_expanded(false, cx);
                cx.emit(DiffViewEvent::ReturnToTerminal);
                cx.stop_propagation();
            }
            return;
        };
        match key {
            "escape" | "esc" => self.cancel_draft(cx),
            "enter" | "return" if modifiers.shift || modifiers.alt => {
                draft.body.push('\n');
                cx.notify();
            }
            "enter" | "return" => {
                self.commit_draft();
                cx.notify();
            }
            "backspace" => {
                if modifiers.platform || modifiers.alt {
                    // Delete back to the previous word boundary.
                    let trimmed = draft.body.trim_end().len();
                    let start = draft.body[..trimmed]
                        .rfind(char::is_whitespace)
                        .map_or(0, |index| index + 1);
                    draft
                        .body
                        .truncate(if modifiers.platform { 0 } else { start });
                } else {
                    draft.body.pop();
                }
                cx.notify();
            }
            "v" if modifiers.platform => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    draft.body.push_str(&text.replace("\r\n", "\n"));
                    cx.notify();
                }
            }
            _ if !modifiers.platform && !modifiers.control => {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    draft.body.push_str(text);
                    cx.notify();
                }
            }
            // Let app shortcuts (⌘W, ⌘1…) through.
            _ => return,
        }
        cx.stop_propagation();
    }

    // -----------------------------------------------------------------------
    // Row model
    // -----------------------------------------------------------------------

    fn ordered_file_refs(&self) -> impl Iterator<Item = &GitFileChange> {
        let changes = self
            .active_snapshot()
            .map(|snapshot| snapshot.changes.as_slice())
            .unwrap_or(&[]);
        changes
            .iter()
            .filter(|change| !change.staged)
            .chain(changes.iter().filter(|change| change.staged))
    }

    fn document(&self, path: &str) -> Option<&Arc<DiffDocument>> {
        self.documents.get(path).map(|cached| &cached.document)
    }

    /// Rebuild the flat rows when anything they depend on changed, keeping the
    /// row at the top of the viewport in place.
    fn sync_rows(&mut self) {
        let mut hasher = DefaultHasher::new();
        (
            self.mode,
            self.selected_commit.is_some(),
            self.layout,
            self.wrap,
        )
            .hash(&mut hasher);
        self.font_size.to_bits().hash(&mut hasher);
        for change in self.ordered_file_refs() {
            (
                &change.path,
                &change.old_path,
                change.staged,
                change.unstaged,
                change.additions,
                change.deletions,
                git_status_badge(change.status),
            )
                .hash(&mut hasher);
            self.expanded.contains(&change.path).hash(&mut hasher);
            self.folds
                .get(&change.path)
                .map(|fold| fold.generation)
                .hash(&mut hasher);
            self.document(&change.path)
                .map(|document| Arc::as_ptr(document) as usize)
                .hash(&mut hasher);
            if let Some(reveals) = self.fold_reveals.get(&change.path) {
                let mut reveals: Vec<_> = reveals.iter().collect();
                reveals.sort_unstable();
                reveals.hash(&mut hasher);
            }
        }
        for comment in &self.comments {
            (comment.id, &comment.anchor).hash(&mut hasher);
        }
        self.draft
            .as_ref()
            .map(|draft| &draft.anchor)
            .hash(&mut hasher);
        let signature = hasher.finish();
        if self.rows_signature == Some(signature) && self.pending_reveal.is_none() {
            return;
        }
        let files: Vec<_> = self.ordered_file_refs().cloned().collect();
        self.rows_signature = Some(signature);

        let worktree = self.mode == GitPanelMode::Worktree;
        let first_staged = files.iter().position(|change| change.staged);
        let entries: Vec<FlattenFile<'_>> = files
            .iter()
            .enumerate()
            .map(|(index, change)| FlattenFile {
                path: &change.path,
                starts_staged_section: worktree && first_staged == Some(index),
                expanded: self.expanded.contains(&change.path),
                folding: self.folds.contains_key(&change.path),
                rows: self
                    .document(&change.path)
                    .map(|document| document.diff.rows.as_slice()),
                folds: self
                    .document(&change.path)
                    .map_or(&[][..], |document| document.folds.as_slice()),
                reveals: self.fold_reveals.get(&change.path),
            })
            .collect();
        let rows = flatten(
            &entries,
            self.layout,
            &self.comments,
            self.draft.as_ref().map(|draft| &draft.anchor),
        );
        drop(entries);

        let top = self.list_state.logical_scroll_top();
        let anchor = self
            .rows
            .get(top.item_ix)
            .map(|row| self.row_key(*row, &self.row_files));
        let reveal = self.pending_reveal.take();
        let new_files = Arc::new(files);
        let key_at = |row: ReviewRow| self.row_key(row, &new_files);
        let header_of = |path: &str| {
            rows.iter().position(|row| {
                matches!(row, ReviewRow::FileHeader { file } if new_files[*file].path == path)
            })
        };
        let target = if let Some(path) = reveal {
            header_of(&path).map(|item_ix| ListOffset {
                item_ix,
                offset_in_item: px(0.0),
            })
        } else if let Some(key) = anchor {
            rows.iter()
                .position(|row| key_at(*row) == key)
                .map(|item_ix| ListOffset {
                    item_ix,
                    offset_in_item: top.offset_in_item,
                })
                .or_else(|| {
                    Self::key_path(&key)
                        .and_then(header_of)
                        .map(|item_ix| ListOffset {
                            item_ix,
                            offset_in_item: px(0.0),
                        })
                })
        } else {
            None
        };

        self.list_state.reset(rows.len());
        if let Some(target) = target {
            self.list_state.scroll_to(target);
        }
        self.rows = Arc::new(rows);
        self.row_files = new_files;
    }

    fn row_key(&self, row: ReviewRow, files: &[GitFileChange]) -> RowKey {
        let path = |file: usize| {
            files
                .get(file)
                .map(|change| change.path.clone())
                .unwrap_or_default()
        };
        match row {
            ReviewRow::StagedSection => RowKey::StagedSection,
            ReviewRow::FileHeader { file } => RowKey::Header(path(file)),
            ReviewRow::FileLoading { file } => RowKey::Loading(path(file)),
            ReviewRow::Folding { file } => RowKey::Folding(path(file)),
            ReviewRow::Draft { .. } => RowKey::Draft,
            ReviewRow::Comment { comment, .. } => {
                RowKey::Comment(self.comments.get(comment).map_or(0, |comment| comment.id))
            }
            ReviewRow::Body { file, row } => match row {
                BodyRow::Line(index) => RowKey::Body(path(file), index),
                BodyRow::Split { left, right } => {
                    RowKey::Body(path(file), right.or(left).unwrap_or(0))
                }
                BodyRow::Fold { fold, .. } => RowKey::ContextFold(path(file), fold),
            },
        }
    }

    fn key_path(key: &RowKey) -> Option<&str> {
        match key {
            RowKey::Header(path)
            | RowKey::Loading(path)
            | RowKey::Body(path, _)
            | RowKey::ContextFold(path, _)
            | RowKey::Folding(path) => Some(path),
            _ => None,
        }
    }

    fn metrics(&self) -> RowMetrics {
        let line_height = (self.font_size * BASE_DIFF_ROW_HEIGHT / BASE_DIFF_FONT_SIZE).round();
        RowMetrics {
            font_size: self.font_size,
            line_height,
            hunk_height: line_height + 2.0,
            fold_height: line_height + 12.0,
            char_width: self.char_width,
            wrap: self.wrap,
            h_offset: if self.wrap { 0.0 } else { self.h_offset },
        }
    }

    /// Measure the monospace advance and clamp the shared horizontal scroll to
    /// the widest expanded line.
    fn sync_horizontal_metrics(&mut self, window: &mut Window) {
        let font_id = window.text_system().resolve_font(&gpui::font(MONO_FONT));
        if let Ok(width) = window.text_system().ch_advance(font_id, px(self.font_size)) {
            self.char_width = f32::from(width);
        }
        if self.wrap {
            self.h_offset = 0.0;
            self.h_max = 0.0;
            return;
        }
        let metrics = self.metrics();
        let (widest, max_line) = self
            .expanded
            .iter()
            .filter_map(|path| self.document(path))
            .fold((0, 0), |(widest, max_line), document| {
                (
                    widest.max(document.widest_columns),
                    max_line.max(document.max_line_number),
                )
            });
        let viewport = f32::from(self.list_state.viewport_bounds().size.width);
        let gutter = metrics.gutter_width(max_line);
        let code_width = if self.layout.is_split() {
            (viewport - SPLIT_DIVIDER_WIDTH) / 2.0 - gutter
        } else {
            viewport - gutter
        };
        let content = widest as f32 * self.char_width + CODE_PADDING_LEFT + CODE_PADDING_RIGHT;
        self.h_max = if viewport > 0.0 {
            (content - code_width).max(0.0)
        } else {
            0.0
        };
        self.h_offset = self.h_offset.clamp(0.0, self.h_max);
    }

    fn scroll_code_horizontally(&mut self, delta: f32, cx: &mut Context<Self>) -> bool {
        if self.wrap {
            return false;
        }
        let next = (self.h_offset - delta).clamp(0.0, self.h_max);
        if next == self.h_offset {
            return false;
        }
        self.h_offset = next;
        cx.notify();
        true
    }

    /// The file header pinned over the list and how far the next header has
    /// pushed it up (≤ 0).
    fn sticky_header(&self) -> Option<(usize, f32)> {
        let top = self.list_state.logical_scroll_top();
        let row = *self.rows.get(top.item_ix)?;
        let file = row.file()?;
        if matches!(row, ReviewRow::FileHeader { .. }) && top.offset_in_item <= px(0.0) {
            return None;
        }
        let viewport_top = self.list_state.viewport_bounds().top();
        let mut offset = 0.0;
        let end = (top.item_ix + STICKY_PUSH_SCAN).min(self.rows.len());
        for index in top.item_ix + 1..end {
            if !matches!(
                self.rows[index],
                ReviewRow::FileHeader { .. } | ReviewRow::StagedSection
            ) {
                continue;
            }
            if let Some(bounds) = self.list_state.bounds_for_item(index) {
                let distance = f32::from(bounds.top() - viewport_top);
                if distance < FILE_HEADER_HEIGHT {
                    offset = distance - FILE_HEADER_HEIGHT;
                }
            }
            break;
        }
        Some((file, offset.min(0.0)))
    }

    fn render_row(
        &mut self,
        ix: usize,
        viewport_height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row) = self.rows.get(ix).copied() else {
            return div().into_any_element();
        };
        let files = self.row_files.clone();
        let metrics = self.metrics();
        match row {
            ReviewRow::StagedSection => {
                let count = files.iter().filter(|change| change.staged).count();
                Self::file_section_header("Staged", count).into_any_element()
            }
            ReviewRow::FileHeader { file } => match files.get(file) {
                Some(change) => self.file_header(change, false, cx).into_any_element(),
                None => div().into_any_element(),
            },
            ReviewRow::FileLoading { .. } => div()
                .w_full()
                .px_3()
                .py_3()
                .bg(surface_tint(colors().background, colors().panel))
                .text_size(px(11.0))
                .text_color(colors().subtle)
                .child("Loading diff…")
                .into_any_element(),
            ReviewRow::Body { file, row } => {
                let Some(change) = files.get(file) else {
                    return div().into_any_element();
                };
                let Some(document) = self.document(&change.path).cloned() else {
                    return div().into_any_element();
                };
                self.body_row(ix, &change.path, &document, row, metrics, cx)
            }
            ReviewRow::Comment { comment, .. } => {
                let Some(comment) = self.comments.get(comment).cloned() else {
                    return div().into_any_element();
                };
                self.comment_card(&comment, metrics, cx).into_any_element()
            }
            ReviewRow::Draft { .. } => self.draft_card(metrics, cx).into_any_element(),
            ReviewRow::Folding { file } => match files.get(file) {
                Some(change) => self.folding_body(&change.path, metrics, viewport_height),
                None => div().into_any_element(),
            },
        }
    }

    fn file_section_header(label: &'static str, count: usize) -> Div {
        div()
            .h(px(SECTION_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px_3()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(colors().muted)
                    .child(label),
            )
            .child(
                div()
                    .px(px(5.0))
                    .py(px(1.0))
                    .rounded(px(4.0))
                    .bg(colors().elevated)
                    .text_size(px(9.0))
                    .text_color(colors().subtle)
                    .child(count.to_string()),
            )
    }

    /// `pinned` is the sticky copy over the list: folding from it scrolls the
    /// file back into view, since its own header sits above the viewport.
    fn file_header(
        &self,
        change: &GitFileChange,
        pinned: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id: gpui::ElementId = if pinned {
            "review-sticky-header".into()
        } else {
            SharedString::from(format!("review-file-{}", change.path)).into()
        };
        let path = change.path.clone();
        let expanded = self.expanded.contains(&path);
        let document = self.document(&path);

        let additions = change
            .additions
            .or_else(|| document.map(|d| d.diff.additions))
            .unwrap_or(0);
        let deletions = change
            .deletions
            .or_else(|| document.map(|d| d.diff.deletions))
            .unwrap_or(0);
        let comments = self
            .comments
            .iter()
            .filter(|comment| comment.anchor.path == path)
            .count();
        let path_for_click = path.clone();

        div()
            .id(id)
            .h(px(FILE_HEADER_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px_3()
            .border_b_1()
            .border_color(colors().border_subtle)
            .bg(surface_tint(
                mix(colors().background, colors().foreground, 0.02),
                colors().background,
            ))
            .cursor_pointer()
            .hover(|row| row.bg(surface_tint(colors().hover, colors().panel)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if pinned {
                    this.pending_reveal = Some(path_for_click.clone());
                }
                this.toggle_path(path_for_click.clone(), cx);
            }))
            .child(
                div()
                    .w(px(12.0))
                    .h(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        gpui::svg()
                            .path(if expanded {
                                "chrome-icons/chevron-down.svg"
                            } else {
                                "chrome-icons/chevron-right.svg"
                            })
                            .size(px(14.0))
                            .flex_none()
                            .text_color(colors().subtle),
                    ),
            )
            .child({
                let name = change.path.rsplit('/').next().unwrap_or(&change.path);
                div()
                    .w(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(file_tree_icon(
                        FileEntryKind::File,
                        false,
                        name,
                        file_tree_icon_color(FileEntryKind::File, name),
                    ))
            })
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .truncate()
                    .font_family(MONO_FONT)
                    .text_size(px(12.0))
                    .text_color(mix(colors().foreground, colors().background, 0.15))
                    .child(change.path.clone()),
            )
            .when(comments > 0, |row| {
                row.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .px_1()
                        .rounded(px(4.0))
                        .bg(colors().selection)
                        .text_size(px(10.0))
                        .text_color(colors().accent)
                        .child(
                            gpui::svg()
                                .path("chrome-icons/comment.svg")
                                .size(px(10.0))
                                .text_color(colors().accent),
                        )
                        .child(comments.to_string()),
                )
            })
            .when(change.staged, |row| {
                row.child(
                    div()
                        .size(px(6.0))
                        .flex_none()
                        .rounded_full()
                        .bg(colors().diff_added),
                )
            })
            .when(additions > 0 || deletions > 0, |row| {
                row.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .when(additions > 0, |stats| {
                            stats.child(
                                div()
                                    .text_color(colors().diff_added)
                                    .child(format!("+{additions}")),
                            )
                        })
                        .when(deletions > 0, |stats| {
                            stats.child(
                                div()
                                    .text_color(colors().diff_deleted)
                                    .child(format!("-{deletions}")),
                            )
                        }),
                )
            })
    }

    // -----------------------------------------------------------------------
    // Diff lines
    // -----------------------------------------------------------------------

    fn body_row(
        &mut self,
        ix: usize,
        path: &str,
        document: &Arc<DiffDocument>,
        row: BodyRow,
        metrics: RowMetrics,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let gutter = metrics.gutter_width(document.max_line_number);
        let rows = &document.diff.rows;
        let anchors = body_row_anchors(rows, row);
        let button = |slot: usize, cx: &mut Context<Self>| -> Option<AnyElement> {
            let (side, line) = anchors[slot]?;
            let index = match row {
                BodyRow::Line(index) => index,
                BodyRow::Split { left, right } => if slot == 0 { left } else { right }?,
                BodyRow::Fold { .. } => return None,
            };
            let excerpt = document.display_lines.get(index)?.to_string();
            let anchor = CommentAnchor {
                path: path.to_owned(),
                side,
                line,
            };
            let group = match (row, slot) {
                (BodyRow::Line(_) | BodyRow::Fold { .. }, _) => "diff-line",
                (_, 0) => "diff-cell-left",
                _ => "diff-cell-right",
            };
            Some(
                Self::comment_button(ix * 2 + slot, group, metrics, anchor, excerpt, cx)
                    .into_any_element(),
            )
        };
        match row {
            BodyRow::Line(index) => {
                let Some(line) = rows.get(index) else {
                    return div().into_any_element();
                };
                match line.kind {
                    GitDiffRowKind::Hunk | GitDiffRowKind::Section | GitDiffRowKind::Notice => {
                        Self::banner_row(line, &document.display_lines[index], gutter, metrics)
                            .into_any_element()
                    }
                    _ => {
                        let button = button(0, cx);
                        Self::unified_line(document, index, gutter, metrics, button)
                            .into_any_element()
                    }
                }
            }
            BodyRow::Split { left, right } => {
                let left_button = button(0, cx);
                let right_button = button(1, cx);
                div()
                    .w_full()
                    .flex()
                    .when(!metrics.wrap, |row| row.h(px(metrics.line_height)))
                    .child(Self::split_cell(
                        document,
                        left,
                        true,
                        gutter,
                        metrics,
                        left_button,
                    ))
                    .child(
                        div()
                            .w(px(SPLIT_DIVIDER_WIDTH))
                            .flex_none()
                            .bg(colors().border_subtle),
                    )
                    .child(Self::split_cell(
                        document,
                        right,
                        false,
                        gutter,
                        metrics,
                        right_button,
                    ))
                    .into_any_element()
            }
            BodyRow::Fold { fold, hidden, .. } => {
                let edges = document.folds.get(fold).map(|context| {
                    (
                        context.start == 0,
                        context.start + context.len >= rows.len(),
                    )
                });
                let (leading, trailing) = edges.unwrap_or_default();
                let actions = FoldActions {
                    ix,
                    path: path.to_owned(),
                    fold,
                    len: document
                        .folds
                        .get(fold)
                        .map_or(hidden, |context| context.len),
                };
                Self::fold_bar(hidden, leading, trailing, metrics, Some((actions, cx)))
                    .into_any_element()
            }
        }
    }

    fn reveal_fold(
        &mut self,
        path: String,
        fold: usize,
        len: usize,
        direction: FoldDirection,
        cx: &mut Context<Self>,
    ) {
        let reveals = self.fold_reveals.entry(path).or_default();
        let reveal = reveals.entry(fold).or_default();
        *reveal = reveal.expand(len, direction);
        cx.notify();
    }

    /// Unchanged lines hidden between hunks: arrows reveal `FOLD_STEP` lines
    /// from the adjacent hunk; the label reveals them all. `actions` is absent
    /// on the inert copy drawn inside a folding animation.
    fn fold_bar(
        hidden: usize,
        leading: bool,
        trailing: bool,
        metrics: RowMetrics,
        actions: Option<(FoldActions, &mut Context<Self>)>,
    ) -> Div {
        let mut bar = div()
            .w_full()
            .h(px(metrics.fold_height))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .bg(surface_tint(
                mix(colors().background, colors().foreground, 0.08),
                colors().background,
            ));
        let (actions, mut cx) = match actions {
            Some((actions, cx)) => (Some(actions), Some(cx)),
            None => (None, None),
        };
        let mut arrow = |icon: &'static str, direction: FoldDirection| {
            let group = SharedString::from(format!(
                "diff-fold-{icon}-{}",
                actions.as_ref().map_or(0, |actions| actions.ix)
            ));
            let button = div()
                .id(group.clone())
                .group(group.clone())
                .size(px(20.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .hover(|button| button.bg(surface_tint(colors().hover, colors().background)))
                .child(
                    gpui::svg()
                        .path(icon)
                        .size(px(12.0))
                        .text_color(colors().subtle)
                        .group_hover(group, |icon| icon.text_color(colors().foreground)),
                );
            match (&actions, cx.as_deref_mut()) {
                (Some(actions), Some(cx)) => {
                    let FoldActions {
                        path, fold, len, ..
                    } = actions.clone();
                    button
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.reveal_fold(path.clone(), fold, len, direction, cx);
                        }))
                }
                _ => button,
            }
        };
        if !leading {
            bar = bar.child(arrow("chrome-icons/chevron-down.svg", FoldDirection::Down));
        }
        if !trailing {
            bar = bar.child(arrow("chrome-icons/chevron-up.svg", FoldDirection::Up));
        }
        let label = div()
            .id(SharedString::from(format!(
                "diff-fold-all-{}",
                actions.as_ref().map_or(0, |actions| actions.ix)
            )))
            .min_w(px(0.0))
            .flex_1()
            .h_full()
            .flex()
            .items_center()
            .pl_1()
            .truncate()
            .font_family(MONO_FONT)
            .text_size(px(metrics.font_size - 1.0))
            .text_color(colors().subtle)
            .hover(|label| label.text_color(colors().muted))
            .child(format!(
                "{hidden} unmodified line{}",
                if hidden == 1 { "" } else { "s" }
            ));
        bar.child(match (actions, cx) {
            (
                Some(FoldActions {
                    path, fold, len, ..
                }),
                Some(cx),
            ) => label
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.reveal_fold(path.clone(), fold, len, FoldDirection::All, cx);
                })),
            _ => label,
        })
    }

    fn line_colors(kind: GitDiffRowKind) -> (Rgba, Rgba) {
        let background = match kind {
            GitDiffRowKind::Addition => colors().diff_added_bg,
            GitDiffRowKind::Deletion => colors().diff_deleted_bg,
            _ => colors().background,
        };
        // Changed lines tint their number more strongly than their code;
        // unchanged numbers sit directly on the page, without a gutter strip.
        let gutter = match kind {
            GitDiffRowKind::Addition => mix(background, colors().diff_added, 0.22),
            GitDiffRowKind::Deletion => mix(background, colors().diff_deleted, 0.22),
            _ => background,
        };
        (
            surface_tint(background, colors().background),
            surface_tint(gutter, background),
        )
    }

    fn unified_line(
        document: &DiffDocument,
        index: usize,
        gutter: f32,
        metrics: RowMetrics,
        button: Option<AnyElement>,
    ) -> Div {
        let row = &document.diff.rows[index];
        let (background, gutter_background) = Self::line_colors(row.kind);
        div()
            .group("diff-line")
            .w_full()
            .flex()
            .map(|line| {
                if metrics.wrap {
                    line.min_h(px(metrics.line_height))
                } else {
                    line.h(px(metrics.line_height))
                }
            })
            .bg(background)
            .child(Self::gutter_cell(
                if row.kind == GitDiffRowKind::Deletion {
                    row.old_line
                } else {
                    row.new_line
                },
                gutter,
                gutter_background,
                row.kind,
                metrics,
                button,
            ))
            .child(Self::code_cell(
                &document.display_lines[index],
                document
                    .highlights
                    .get(index)
                    .map_or(&[][..], Vec::as_slice),
                row.kind,
                metrics,
            ))
    }

    fn split_cell(
        document: &DiffDocument,
        index: Option<usize>,
        left: bool,
        gutter: f32,
        metrics: RowMetrics,
        button: Option<AnyElement>,
    ) -> Div {
        let cell = div()
            .group(if left {
                "diff-cell-left"
            } else {
                "diff-cell-right"
            })
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .overflow_hidden();
        let Some(row) = index.and_then(|index| document.diff.rows.get(index)) else {
            // Blank half: the other side added or removed lines here.
            return cell.bg(surface_tint(colors().elevated, colors().background));
        };
        let index = index.unwrap_or_default();
        let kind = row.kind;
        let (background, gutter_background) = Self::line_colors(kind);
        let number = if left { row.old_line } else { row.new_line };
        cell.bg(background)
            .child(Self::gutter_cell(
                number,
                gutter,
                gutter_background,
                kind,
                metrics,
                button,
            ))
            .child(Self::code_cell(
                &document.display_lines[index],
                document
                    .highlights
                    .get(index)
                    .map_or(&[][..], Vec::as_slice),
                kind,
                metrics,
            ))
    }

    fn gutter_cell(
        number: Option<usize>,
        width: f32,
        background: Rgba,
        kind: GitDiffRowKind,
        metrics: RowMetrics,
        button: Option<AnyElement>,
    ) -> Div {
        let color = match kind {
            GitDiffRowKind::Addition => colors().diff_added,
            GitDiffRowKind::Deletion => colors().diff_deleted,
            _ => colors().subtle,
        };
        div()
            .relative()
            .w(px(width))
            .flex_none()
            .flex()
            .justify_end()
            .pr_2()
            .bg(background)
            .font_family(MONO_FONT)
            .text_size(px(metrics.font_size - 1.0))
            .line_height(px(metrics.line_height))
            .text_color(color)
            .child(number.map(|line| line.to_string()).unwrap_or_default())
            .children(button)
    }

    /// Revealed while its row (or split half) `group` is hovered.
    fn comment_button(
        id: usize,
        group: &'static str,
        metrics: RowMetrics,
        anchor: CommentAnchor,
        excerpt: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(("diff-comment", id))
            .absolute()
            .top(px(
                ((metrics.line_height - COMMENT_BUTTON_SIZE) / 2.0).max(0.0)
            ))
            .left(px((COMMENT_ACTION_WIDTH - COMMENT_BUTTON_SIZE) / 2.0))
            .size(px(COMMENT_BUTTON_SIZE))
            .rounded(px(4.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(colors().accent)
            .cursor_pointer()
            .opacity(0.0)
            .group_hover(group, |style| style.opacity(1.0))
            .child(
                gpui::svg()
                    .path("chrome-icons/plus.svg")
                    .size(px(11.0))
                    .text_color(colors().background),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.open_draft(anchor.clone(), excerpt.clone(), window, cx);
            }))
    }

    fn code_cell(
        text: &SharedString,
        spans: &[SyntaxSpan],
        kind: GitDiffRowKind,
        metrics: RowMetrics,
    ) -> Div {
        let code = Self::styled_code_line(text, spans, kind, metrics);
        let cell = div()
            .flex_1()
            .min_w(px(0.0))
            .font_family(MONO_FONT)
            .text_size(px(metrics.font_size))
            .line_height(px(metrics.line_height))
            // Unchanged code recedes so the changes read first.
            .when(kind == GitDiffRowKind::Context, |cell| cell.opacity(0.72));
        if metrics.wrap {
            cell.pl(px(CODE_PADDING_LEFT))
                .pr(px(CODE_PADDING_LEFT))
                .child(code)
        } else {
            // Only the code plane moves; line numbers and comment actions stay fixed.
            cell.h_full().overflow_hidden().child(
                div()
                    .relative()
                    .left(px(-metrics.h_offset))
                    .pl(px(CODE_PADDING_LEFT))
                    .whitespace_nowrap()
                    .child(code),
            )
        }
    }

    /// Hunk, section, and notice rows span the full width in both layouts.
    fn banner_row(row: &GitDiffRow, text: &SharedString, gutter: f32, metrics: RowMetrics) -> Div {
        let (height, background, color) = match row.kind {
            GitDiffRowKind::Hunk => (metrics.hunk_height, colors().diff_hunk_bg, colors().subtle),
            GitDiffRowKind::Section => (SECTION_HEIGHT - 4.0, colors().elevated, colors().subtle),
            _ => (metrics.line_height, colors().background, colors().warning),
        };
        div()
            .w_full()
            .h(px(height))
            .flex()
            .items_center()
            .overflow_hidden()
            .bg(surface_tint(background, colors().background))
            .pl(px(if row.kind == GitDiffRowKind::Hunk {
                gutter + CODE_PADDING_LEFT
            } else {
                12.0
            }))
            .pr_3()
            .font_family(MONO_FONT)
            .text_size(px(if row.kind == GitDiffRowKind::Section {
                10.0
            } else {
                metrics.font_size - 1.0
            }))
            .when(row.kind == GitDiffRowKind::Section, |row| {
                row.font_weight(gpui::FontWeight::MEDIUM)
            })
            .text_color(color)
            .whitespace_nowrap()
            .child(text.clone())
    }

    fn styled_code_line(
        text: &SharedString,
        spans: &[SyntaxSpan],
        kind: GitDiffRowKind,
        metrics: RowMetrics,
    ) -> StyledText {
        let default_style = TextStyle {
            // Context dims through its cell's opacity, syntax colors included.
            color: colors().foreground.into(),
            font_family: MONO_FONT.into(),
            font_size: px(metrics.font_size).into(),
            line_height: px(metrics.line_height).into(),
            white_space: if metrics.wrap {
                WhiteSpace::Normal
            } else {
                WhiteSpace::Nowrap
            },
            ..Default::default()
        };
        if spans.is_empty()
            || !matches!(
                kind,
                GitDiffRowKind::Context | GitDiffRowKind::Addition | GitDiffRowKind::Deletion
            )
        {
            return StyledText::new(text.clone()).with_default_highlights(
                &default_style,
                std::iter::empty::<(std::ops::Range<usize>, HighlightStyle)>(),
            );
        }
        let highlights = spans.iter().filter_map(|span| {
            if span.range.start >= text.len() || span.range.end > text.len() {
                return None;
            }
            if !text.is_char_boundary(span.range.start) || !text.is_char_boundary(span.range.end) {
                return None;
            }
            Some((span.range.clone(), span.kind.highlight_style()))
        });
        StyledText::new(text.clone()).with_default_highlights(&default_style, highlights)
    }

    /// A body folding open or shut: a clipped stand-in whose height tweens,
    /// built only from the rows the clip can reveal.
    fn folding_body(&self, path: &str, metrics: RowMetrics, viewport_height: f32) -> AnyElement {
        let (Some(document), Some(fold)) = (self.document(path), self.folds.get(path)) else {
            return div().into_any_element();
        };
        let rows = &document.diff.rows;
        let viewport = viewport_height.max(200.0);
        let gutter = metrics.gutter_width(document.max_line_number);
        let mut height = 0.0;
        let mut children = Vec::new();
        let no_reveals = HashMap::new();
        let reveals = self.fold_reveals.get(path).unwrap_or(&no_reveals);
        for row in body_rows(rows, self.layout, &document.folds, reveals) {
            if height >= viewport {
                break;
            }
            height += metrics.body_row_height(rows, row);
            children.push(match row {
                BodyRow::Line(index) => match rows[index].kind {
                    GitDiffRowKind::Hunk | GitDiffRowKind::Section | GitDiffRowKind::Notice => {
                        Self::banner_row(
                            &rows[index],
                            &document.display_lines[index],
                            gutter,
                            metrics,
                        )
                    }
                    _ => Self::unified_line(document, index, gutter, metrics, None),
                },
                BodyRow::Split { left, right } => div()
                    .w_full()
                    .h(px(metrics.line_height))
                    .flex()
                    .child(Self::split_cell(
                        document, left, true, gutter, metrics, None,
                    ))
                    .child(
                        div()
                            .w(px(SPLIT_DIVIDER_WIDTH))
                            .flex_none()
                            .bg(colors().border_subtle),
                    )
                    .child(Self::split_cell(
                        document, right, false, gutter, metrics, None,
                    )),
                BodyRow::Fold { hidden, .. } => Self::fold_bar(hidden, false, false, metrics, None),
            });
        }
        let height = height.min(viewport);
        let expanding = fold.expanding;
        div()
            .w_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .children(children)
            .with_animation(
                ("review-fold", fold.generation as usize),
                Animation::new(FOLD_DURATION).with_easing(ease_out_quint()),
                move |body, delta| {
                    let progress = if expanding { delta } else { 1.0 - delta };
                    body.h(px(height * progress))
                },
            )
            .into_any_element()
    }

    // -----------------------------------------------------------------------
    // Comments
    // -----------------------------------------------------------------------

    fn card_indent(&self, metrics: RowMetrics) -> f32 {
        if self.layout.is_split() {
            12.0
        } else {
            metrics.gutter_width(99)
        }
    }

    fn comment_card(
        &self,
        comment: &ReviewComment,
        metrics: RowMetrics,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = comment.id;
        let cite = match comment.anchor.side {
            CommentSide::New => format!("Line {}", comment.anchor.line),
            CommentSide::Old => format!("Removed line {}", comment.anchor.line),
        };
        div()
            .w_full()
            .py(px(6.0))
            .pl(px(self.card_indent(metrics)))
            .pr_3()
            .bg(surface_tint(colors().background, colors().panel))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(surface_tint(colors().elevated, colors().panel))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                gpui::svg()
                                    .path("chrome-icons/comment.svg")
                                    .size(px(11.0))
                                    .text_color(colors().accent),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(10.0))
                                    .text_color(colors().subtle)
                                    .child(cite),
                            )
                            .child(Self::card_action(
                                ("comment-edit", id as usize),
                                "Edit",
                                cx.listener(move |this, _, window, cx| {
                                    this.edit_comment(id, window, cx);
                                }),
                            ))
                            .child(Self::card_action(
                                ("comment-delete", id as usize),
                                "Delete",
                                cx.listener(move |this, _, _, cx| this.delete_comment(id, cx)),
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(17.0))
                            .text_color(colors().foreground)
                            .child(comment.body.clone()),
                    ),
            )
    }

    fn card_action(
        id: impl Into<gpui::ElementId>,
        label: &'static str,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .px(px(6.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .text_size(px(10.5))
            .text_color(colors().muted)
            .cursor_pointer()
            .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
            .child(label)
            .on_click(on_click)
    }

    fn draft_card(&self, metrics: RowMetrics, cx: &mut Context<Self>) -> Div {
        let Some(draft) = self.draft.as_ref() else {
            return div();
        };
        let empty = draft.body.trim().is_empty();
        let cite = match draft.anchor.side {
            CommentSide::New => format!("Comment on line {}", draft.anchor.line),
            CommentSide::Old => format!("Comment on removed line {}", draft.anchor.line),
        };
        let body = if draft.body.is_empty() {
            div()
                .text_color(colors().subtle)
                .child("Tell the agent what to change here…")
        } else {
            div()
                .text_color(colors().foreground)
                .child(format!("{}▍", draft.body))
        };
        div()
            .w_full()
            .py(px(6.0))
            .pl(px(self.card_indent(metrics)))
            .pr_3()
            .bg(surface_tint(colors().background, colors().panel))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(colors().accent)
                    .bg(surface_tint(colors().elevated, colors().panel))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(colors().subtle)
                            .child(cite),
                    )
                    .child(
                        div()
                            .min_h(px(34.0))
                            .text_size(px(12.0))
                            .line_height(px(17.0))
                            .child(body),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(9.5))
                                    .text_color(colors().subtle)
                                    .child("↵ save · ⇧↵ new line · esc cancel"),
                            )
                            .child(Self::card_action(
                                "comment-draft-cancel",
                                "Cancel",
                                cx.listener(|this, _, _, cx| this.cancel_draft(cx)),
                            ))
                            .child(
                                div()
                                    .id("comment-draft-save")
                                    .px(px(8.0))
                                    .py(px(3.0))
                                    .rounded(px(5.0))
                                    .text_size(px(10.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .bg(if empty {
                                        colors().selection
                                    } else {
                                        colors().accent
                                    })
                                    .text_color(if empty {
                                        colors().subtle
                                    } else {
                                        colors().background
                                    })
                                    .when(!empty, |button| button.cursor_pointer())
                                    .child("Comment")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.commit_draft();
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
    }

    // -----------------------------------------------------------------------
    // Chrome
    // -----------------------------------------------------------------------

    fn header_meta(&self, loading: bool) -> (String, Vec<Div>) {
        let mut meta = Vec::new();
        let branch = match self.mode {
            GitPanelMode::Worktree => self
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.branch.clone()),
            GitPanelMode::Branch => self
                .branch_changes
                .as_ref()
                .map(|changes| changes.snapshot.branch.clone()),
            GitPanelMode::LatestTurn => self
                .turn_baseline
                .as_ref()
                .map(|baseline| baseline.agent.clone()),
            GitPanelMode::History if self.selected_commit.is_some() => self
                .selected_commit
                .as_ref()
                .map(|commit| commit.short_sha.clone()),
            GitPanelMode::History => self.history.as_ref().map(|history| {
                format!(
                    "{} commit{}",
                    history.total,
                    if history.total == 1 { "" } else { "s" }
                )
            }),
        }
        .unwrap_or_else(|| {
            if loading {
                "…".to_owned()
            } else {
                "no git".to_owned()
            }
        });

        match self.mode {
            GitPanelMode::Worktree => {
                if let Some(snapshot) = &self.snapshot {
                    Self::push_change_meta(
                        &mut meta,
                        snapshot.changes.len(),
                        snapshot.additions,
                        snapshot.deletions,
                    );
                }
            }
            GitPanelMode::Branch => {
                if let Some(changes) = &self.branch_changes {
                    if !changes.base.is_empty() {
                        meta.push(
                            div()
                                .truncate()
                                .text_size(px(11.0))
                                .text_color(colors().subtle)
                                .child(format!("vs {}", changes.base)),
                        );
                    }
                    if changes.commits_ahead > 0 {
                        meta.push(
                            div()
                                .text_size(px(11.0))
                                .text_color(colors().accent)
                                .child(format!("+{}", changes.commits_ahead)),
                        );
                    }
                    Self::push_change_meta(
                        &mut meta,
                        changes.snapshot.changes.len(),
                        changes.snapshot.additions,
                        changes.snapshot.deletions,
                    );
                }
            }
            GitPanelMode::LatestTurn => {
                if let Some(baseline) = &self.turn_baseline {
                    meta.push(
                        div()
                            .truncate()
                            .text_size(px(11.0))
                            .text_color(colors().subtle)
                            .child(format!(
                                "{} · {}",
                                baseline.agent,
                                elapsed_label(baseline.started.elapsed())
                            )),
                    );
                }
                if let Some(changes) = &self.turn_changes {
                    Self::push_change_meta(
                        &mut meta,
                        changes.snapshot.changes.len(),
                        changes.snapshot.additions,
                        changes.snapshot.deletions,
                    );
                }
            }
            GitPanelMode::History => {
                if let Some(changes) = &self.commit_changes {
                    Self::push_change_meta(
                        &mut meta,
                        changes.snapshot.changes.len(),
                        changes.snapshot.additions,
                        changes.snapshot.deletions,
                    );
                } else if self.selected_commit.is_none()
                    && let Some(history) = &self.history
                {
                    meta.push(
                        div()
                            .truncate()
                            .text_size(px(11.0))
                            .text_color(colors().subtle)
                            .child(history.branch.clone()),
                    );
                }
            }
        }

        (branch, meta)
    }

    fn push_change_meta(meta: &mut Vec<Div>, files: usize, additions: usize, deletions: usize) {
        if files == 0 {
            return;
        }
        meta.push(
            div()
                .text_size(px(11.0))
                .text_color(colors().subtle)
                .child(format!("{files} file{}", if files == 1 { "" } else { "s" })),
        );
        if additions > 0 {
            meta.push(
                div()
                    .text_size(px(11.0))
                    .text_color(colors().diff_added)
                    .child(format!("+{additions}")),
            );
        }
        if deletions > 0 {
            meta.push(
                div()
                    .text_size(px(11.0))
                    .text_color(colors().diff_deleted)
                    .child(format!("−{deletions}")),
            );
        }
    }

    /// Layout, wrap, and review controls above the file list.
    fn review_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let comments = self.comments.len();
        let split = self.layout.is_split();
        div()
            .w_full()
            .flex_none()
            .h(px(32.0))
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(self.review_summary())
            .child(div().w(px(8.0)).flex_none())
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .p(px(2.0))
                    .gap(px(2.0))
                    .rounded(px(6.0))
                    .child(Self::segment_button(
                        "diff-layout-unified",
                        "chrome-icons/diff-unified.svg",
                        "Unified",
                        !split,
                        cx.listener(|this, _, _, cx| this.set_layout(DiffLayout::Unified, cx)),
                    ))
                    .child(Self::segment_button(
                        "diff-layout-split",
                        "chrome-icons/diff-split.svg",
                        "Split",
                        split,
                        cx.listener(|this, _, _, cx| this.set_layout(DiffLayout::Split, cx)),
                    )),
            )
            .child(Self::segment_button(
                "diff-wrap",
                "chrome-icons/wrap.svg",
                "Wrap",
                self.wrap,
                cx.listener(|this, _, _, cx| this.toggle_wrap(cx)),
            ))
            .child(div().flex_1())
            .child(Self::toolbar_icon_button(
                "diff-expand-all",
                "chrome-icons/unfold-vertical.svg",
                true,
                cx.listener(|this, _, _, cx| this.expand_all(cx)),
            ))
            .child(Self::toolbar_icon_button(
                "diff-collapse-all",
                "chrome-icons/fold-vertical.svg",
                !self.expanded.is_empty(),
                cx.listener(|this, _, _, cx| this.collapse_all(cx)),
            ))
            .when(self.review_focused, |bar| {
                bar.child(Self::toolbar_icon_button(
                    "review-show-beside-terminal",
                    "chrome-icons/split-view.svg",
                    true,
                    cx.listener(|this, _, _, cx| this.set_review_focused(false, cx)),
                ))
            })
            .when(comments > 0, |bar| {
                bar.child(
                    div()
                        .id("review-clear-comments")
                        .flex_none()
                        .size(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .hover(|button| button.bg(colors().hover))
                        .child(
                            gpui::svg()
                                .path("chrome-icons/close.svg")
                                .size(px(12.0))
                                .text_color(colors().muted),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.comments.clear();
                            this.review_delivery = None;
                            this.draft = None;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("review-send")
                        .flex_none()
                        .h(px(24.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .rounded(px(6.0))
                        .bg(colors().accent)
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(colors().background)
                        .cursor_pointer()
                        .child(
                            gpui::svg()
                                .path("chrome-icons/send.svg")
                                .size(px(11.0))
                                .text_color(colors().background),
                        )
                        .child(format!(
                            "Send {comments} comment{} to agent",
                            if comments == 1 { "" } else { "s" }
                        ))
                        .on_click(cx.listener(|this, _, _, cx| this.send_review(cx))),
                )
            })
    }

    fn toolbar_icon_button(
        id: &'static str,
        icon: &'static str,
        enabled: bool,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .group(id)
            .flex_none()
            .size(px(26.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.0))
            .when(!enabled, |button| button.opacity(0.4))
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|button| button.bg(colors().hover))
                    .on_click(on_click)
            })
            .child(
                gpui::svg()
                    .path(icon)
                    .size(px(14.0))
                    .text_color(colors().subtle)
                    .when(enabled, |icon| {
                        icon.group_hover(id, |icon| icon.text_color(colors().foreground))
                    }),
            )
    }

    fn segment_button(
        id: &'static str,
        icon: &'static str,
        label: &'static str,
        active: bool,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex_none()
            .h(px(22.0))
            .px(px(7.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .rounded(px(5.0))
            .when(active, |button| button.bg(colors().selection))
            .cursor_pointer()
            .hover(|button| button.bg(colors().hover))
            .text_size(px(10.5))
            .text_color(if active {
                colors().foreground
            } else {
                colors().muted
            })
            .child(gpui::svg().path(icon).size(px(12.0)).text_color(if active {
                colors().foreground
            } else {
                colors().muted
            }))
            .child(label)
            .on_click(on_click)
    }

    fn message(&self, text: &'static str) -> Div {
        if !self.review_expanded {
            return div()
                .flex_1()
                .min_h(px(0.0))
                .px_3()
                .py_3()
                .text_size(px(12.0))
                .line_height(px(18.0))
                .text_color(colors().subtle)
                .child(text);
        }
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .p_8()
            .child(
                div()
                    .max_w(px(260.0))
                    .text_center()
                    .text_size(px(13.0))
                    .line_height(px(20.0))
                    .text_color(colors().subtle)
                    .child(text),
            )
    }

    /// The single virtualized review list, its pinned header, and the wheel
    /// capture that routes horizontal gestures to the code plane.
    fn review_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_rows();
        self.sync_horizontal_metrics(window);
        // GPUI holds ListState mutably while rendering rows. Capture the last
        // viewport now so fold animations never borrow it from that callback.
        let viewport_height = f32::from(self.list_state.viewport_bounds().size.height);
        let sticky = self.sticky_header().and_then(|(file, offset)| {
            let change = self.row_files.get(file)?.clone();
            Some(
                div()
                    .absolute()
                    .top(px(offset))
                    .left_0()
                    .right_0()
                    .bg(floating_surface(colors().panel))
                    .child(self.file_header(&change, true, cx)),
            )
        });
        let view = cx.entity().downgrade();
        let line_height = self.metrics().line_height;
        div()
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(
                list(
                    self.list_state.clone(),
                    cx.processor(move |this, ix: usize, _, cx| {
                        this.render_row(ix, viewport_height, cx)
                    }),
                )
                .size_full(),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        window.on_mouse_event(
                            move |event: &ScrollWheelEvent, phase, _window, cx| {
                                if phase != DispatchPhase::Capture
                                    || !bounds.contains(&event.position)
                                {
                                    return;
                                }
                                let delta = event.delta.pixel_delta(px(line_height));
                                // Keep trackpad gestures on their dominant axis:
                                // sideways drift must never scroll the files.
                                if delta.x.abs() <= delta.y.abs() {
                                    return;
                                }
                                let _ = view.update(cx, |this, cx| {
                                    this.scroll_code_horizontally(f32::from(delta.x), cx);
                                });
                                cx.stop_propagation();
                            },
                        );
                    },
                )
                .absolute()
                .inset_0(),
            )
            .children(sticky)
    }
}

impl Render for DiffView {
    /// The central review. The Changes sidebar is rendered by
    /// [`DiffFileIndexView`] through [`DiffView::changes_panel`].
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = match self.mode {
            GitPanelMode::Branch => self.branch_error.clone().or_else(|| self.error.clone()),
            GitPanelMode::LatestTurn => self.turn_error.clone().or_else(|| self.error.clone()),
            GitPanelMode::History => self.commit_error.clone().or_else(|| self.error.clone()),
            GitPanelMode::Worktree => self.error.clone(),
        };
        let empty_message = self.empty_message().or_else(|| {
            (self.mode == GitPanelMode::History && self.selected_commit.is_none())
                .then_some("Select a commit to browse its changes.")
        });
        div()
            .id("git-review")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            // Full-tab reviews are titled and closed by their tab.
            .when(!self.review_focused, |view| {
                view.child(self.review_pane_header(cx))
            })
            .when_some(error, |view, error| {
                view.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_2()
                        .bg(colors().diff_deleted_bg)
                        .border_b_1()
                        .border_color(colors().danger)
                        .text_size(px(11.0))
                        .line_height(px(16.0))
                        .text_color(colors().danger)
                        .child(error),
                )
            })
            .map(|view| match empty_message {
                Some(text) => view.child(self.message(text)),
                None => view
                    .child(self.review_toolbar(cx))
                    .child(self.review_list(window, cx)),
            })
    }
}

impl DiffView {
    fn empty_message(&self) -> Option<&'static str> {
        match self.mode {
            GitPanelMode::Worktree => {
                if self.snapshot.is_none() && self.refreshing && !self.snapshot_settled {
                    Some("Reading repository…")
                } else if self.snapshot.is_none() {
                    Some("No Git repository in this project.")
                } else if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.changes.is_empty())
                {
                    Some("No uncommitted changes")
                } else {
                    None
                }
            }
            GitPanelMode::Branch => {
                if self.branch_error.is_some() && self.branch_changes.is_none() {
                    Some("Select available branches and try again.")
                } else if self.branch_changes.is_none()
                    && (self.branch_refreshing || (self.refreshing && !self.snapshot_settled))
                {
                    Some("Comparing with the base branch…")
                } else if self.snapshot.is_none() && self.branch_changes.is_none() {
                    Some("No Git repository in this project.")
                } else if self
                    .branch_changes
                    .as_ref()
                    .is_some_and(|changes| changes.base.is_empty())
                {
                    Some("No base branch (main, master, or upstream) to compare.")
                } else if self
                    .branch_changes
                    .as_ref()
                    .is_some_and(|changes| changes.snapshot.changes.is_empty())
                {
                    Some("No changes on this branch")
                } else if self.branch_changes.is_none() {
                    Some("Comparing with the base branch…")
                } else {
                    None
                }
            }
            GitPanelMode::LatestTurn => {
                if !self.turn_settled {
                    Some("Reading the latest agent turn…")
                } else if self.turn_error.is_some() && self.turn_changes.is_none() {
                    Some("Could not compare the latest turn.")
                } else if self.turn_baseline.is_none() && self.snapshot.is_none() {
                    Some("No Git repository in this project.")
                } else if self.turn_baseline.is_none() {
                    Some(
                        "No agent turn recorded here yet. When an agent starts working in this \
                         repository, its changes appear here.",
                    )
                } else if self
                    .turn_changes
                    .as_ref()
                    .is_none_or(|changes| changes.snapshot.changes.is_empty())
                {
                    Some("No changes since the latest turn started.")
                } else {
                    None
                }
            }
            GitPanelMode::History if self.selected_commit.is_some() => {
                if self.commit_changes.is_none() && self.commit_refreshing {
                    Some("Loading commit changes…")
                } else if self.commit_changes.is_none() {
                    Some("Could not load this commit. Retry or return to history.")
                } else if self
                    .commit_changes
                    .as_ref()
                    .is_some_and(|changes| changes.snapshot.changes.is_empty())
                {
                    Some("This commit has no file changes.")
                } else {
                    None
                }
            }
            GitPanelMode::History => {
                if self.history.is_none()
                    && (self.history_refreshing || (self.refreshing && !self.snapshot_settled))
                {
                    Some("Loading history…")
                } else if self.history.is_none() && self.snapshot.is_none() {
                    Some("No Git repository in this project.")
                } else if self
                    .history
                    .as_ref()
                    .is_some_and(|history| history.commits.is_empty())
                {
                    Some("This repository has no commits yet.")
                } else if self.history.is_none() {
                    Some("Loading history…")
                } else {
                    None
                }
            }
        }
    }
}

fn elapsed_label(elapsed: Duration) -> String {
    let minutes = elapsed.as_secs() / 60;
    match minutes {
        0 => "started just now".to_owned(),
        1..=59 => format!("started {minutes} min ago"),
        _ => format!("started {} h ago", minutes / 60),
    }
}

impl DiffView {
    fn history_graph_width(graph: &[GitGraphRow]) -> f32 {
        let lanes = graph.iter().map(|row| row.lane_count).max().unwrap_or(1);
        (lanes as f32 * GRAPH_LANE_WIDTH).clamp(18.0, 48.0)
    }

    fn commit_controls(&self, commit: &GitCommit, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .w_full()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("git-back-to-history")
                            .px_2()
                            .py_1()
                            .rounded(px(5.0))
                            .text_size(px(11.0))
                            .text_color(colors().muted)
                            .cursor_pointer()
                            .hover(|button| button.bg(colors().hover))
                            .child(if self.return_to_worktree {
                                "← Back to changes"
                            } else {
                                "← Back to history"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.return_to_worktree {
                                    this.set_review_expanded(false, cx);
                                } else {
                                    this.back_to_history(cx);
                                }
                            })),
                    )
                    .when(self.commit_error.is_some(), |view| {
                        view.child(
                            div()
                                .id("git-retry-commit")
                                .px_2()
                                .py_1()
                                .rounded(px(5.0))
                                .text_size(px(11.0))
                                .text_color(colors().accent)
                                .cursor_pointer()
                                .hover(|button| button.bg(colors().hover))
                                .child("Retry")
                                .on_click(cx.listener(|this, _, _, cx| this.refresh_commit(cx))),
                        )
                    }),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(colors().foreground)
                    .child(commit.subject.clone()),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(colors().subtle)
                    .child(format!(
                        "{} · {}",
                        commit.author,
                        format_short_date(&commit.date)
                    )),
            )
            .when(commit.parents.len() > 1, |view| {
                view.child(
                    div()
                        .text_size(px(10.0))
                        .text_color(colors().subtle)
                        .child("Merge commit · compared with first parent"),
                )
            })
    }

    fn history_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let history = self.history.clone();
        let graph = self.history_graph.clone();
        let count = history.as_ref().map_or(0, |history| history.commits.len());
        let truncated = self
            .history
            .as_ref()
            .is_some_and(|history| history.truncated);
        let head = self.history.as_ref().map(|history| history.head.clone());
        let graph_width = Self::history_graph_width(graph.as_slice());
        let list_count = count + usize::from(truncated);

        div()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .flex()
            .flex_col()
            .child(Self::history_table_header(graph_width))
            .child(
                uniform_list(
                    "git-history",
                    list_count,
                    cx.processor(move |_this, range: std::ops::Range<usize>, _window, cx| {
                        let Some(history) = history.as_ref() else {
                            return Vec::new();
                        };
                        range
                            .filter_map(|index| {
                                if index == count {
                                    return truncated.then(|| {
                                        div()
                                            .h(px(HISTORY_ROW_HEIGHT))
                                            .w_full()
                                            .flex_none()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .text_size(px(10.5))
                                            .text_color(colors().subtle)
                                            .child(format!(
                                                "Showing the {HISTORY_PAGE} most recent"
                                            ))
                                            .into_any_element()
                                    });
                                }
                                let commit = history.commits.get(index)?;
                                let selected = commit.clone();
                                let row = graph.get(index);
                                Some(
                                    Self::history_row(
                                        commit,
                                        row,
                                        graph_width,
                                        head.as_deref().is_some_and(|head| commit.sha == head),
                                    )
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_commit(selected.clone(), cx);
                                    }))
                                    .into_any_element(),
                                )
                            })
                            .collect()
                    }),
                )
                .flex_1()
                .min_h(px(0.0))
                .w_full(),
            )
    }

    fn history_table_header(graph_width: f32) -> Div {
        div()
            .h(px(HISTORY_HEADER_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .px_2()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(div().w(px(graph_width)).h_full().flex_none())
            .child(Self::history_flex_cell(
                "Commit",
                true,
                10.5,
                colors().subtle,
            ))
            .child(Self::history_fixed_cell(
                "Author",
                HISTORY_AUTHOR_WIDTH,
                10.5,
                colors().subtle,
                false,
            ))
            .child(Self::history_fixed_cell(
                "Date",
                HISTORY_DATE_WIDTH,
                10.5,
                colors().subtle,
                false,
            ))
            .child(Self::history_fixed_cell(
                "SHA",
                HISTORY_SHA_WIDTH,
                10.5,
                colors().subtle,
                true,
            ))
    }

    fn history_flex_cell(
        text: impl Into<SharedString>,
        strong: bool,
        size: f32,
        color: Rgba,
    ) -> Div {
        div()
            .min_w(px(0.0))
            .flex_1()
            .overflow_hidden()
            .px_2()
            .child(
                div()
                    .w_full()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(size))
                    .font_weight(if strong {
                        gpui::FontWeight::MEDIUM
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(color)
                    .child(text.into()),
            )
    }

    fn history_fixed_cell(
        text: impl Into<SharedString>,
        width: f32,
        size: f32,
        color: Rgba,
        mono: bool,
    ) -> Div {
        let mut cell = div()
            .w(px(width))
            .flex_none()
            .overflow_hidden()
            .pr_2()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(size))
            .text_color(color)
            .child(text.into());
        if mono {
            cell = cell.font_family(MONO_FONT);
        }
        cell
    }

    fn history_row(
        commit: &GitCommit,
        graph: Option<&GitGraphRow>,
        graph_width: f32,
        is_head: bool,
    ) -> Stateful<Div> {
        div()
            .id(SharedString::from(format!("git-commit-{}", commit.sha)))
            .h(px(HISTORY_ROW_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .px_2()
            .overflow_hidden()
            .border_b_1()
            .border_color(colors().border_subtle)
            .hover(|row| row.bg(surface_tint(colors().hover, colors().panel)))
            .child(Self::graph_column(
                graph,
                graph_width,
                is_head,
                HISTORY_ROW_HEIGHT,
            ))
            .child(Self::history_flex_cell(
                commit.subject.clone(),
                is_head,
                12.0,
                colors().foreground,
            ))
            .child(Self::history_fixed_cell(
                commit.author.clone(),
                HISTORY_AUTHOR_WIDTH,
                11.0,
                colors().muted,
                false,
            ))
            .child(Self::history_fixed_cell(
                format_short_date(&commit.date),
                HISTORY_DATE_WIDTH,
                11.0,
                colors().subtle,
                false,
            ))
            .child(Self::history_fixed_cell(
                commit.short_sha.clone(),
                HISTORY_SHA_WIDTH,
                11.0,
                colors().subtle,
                true,
            ))
    }

    fn graph_column(
        graph: Option<&GitGraphRow>,
        width: f32,
        is_head: bool,
        row_height: f32,
    ) -> Div {
        let Some(graph) = graph else {
            return div().w(px(width)).h_full().flex_none();
        };
        let lane = graph.lane;
        let through = graph
            .through
            .iter()
            .map(|rail| (rail.lane, lane_color(rail.color)))
            .collect::<Vec<_>>();
        let first_parent_edge = graph
            .first_parent_edge
            .map(|rail| (rail.lane, lane_color(graph.color)));
        let edges = graph
            .edges
            .iter()
            .map(|rail| (rail.lane, lane_color(rail.color)))
            .collect::<Vec<_>>();
        let active_color = lane_color(graph.color);
        let continues = graph.continues;
        let mid = row_height / 2.0;
        let lane_x = lane as f32 * GRAPH_LANE_WIDTH + 6.0;
        let node_size = if is_head { 10.0 } else { 6.0 };

        div()
            .w(px(width))
            .h_full()
            .flex_none()
            .relative()
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let top = bounds.top();
                        let bottom = bounds.bottom();
                        let middle = top + px(mid);

                        for (rail, color) in through {
                            let x = bounds.left() + px(rail as f32 * GRAPH_LANE_WIDTH + 6.0);
                            let mut path = PathBuilder::stroke(px(1.0));
                            path.move_to(point(x, top));
                            path.line_to(point(x, bottom));
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                        }

                        // Incoming half of the active rail always reaches the node.
                        let active_x = bounds.left() + px(lane_x);
                        let mut active = PathBuilder::stroke(px(1.0));
                        active.move_to(point(active_x, top));
                        active.line_to(point(active_x, middle));
                        if continues {
                            active.line_to(point(active_x, bottom));
                        }
                        if let Ok(path) = active.build() {
                            window.paint_path(path, active_color);
                        }

                        // When this lane rejoins an existing first-parent lane, bend the
                        // source-colored rail into it and stop the straight rail at the node.
                        if let Some((target, color)) = first_parent_edge {
                            let target_x =
                                bounds.left() + px(target as f32 * GRAPH_LANE_WIDTH + 6.0);
                            let mut path = PathBuilder::stroke(px(1.0));
                            path.move_to(point(active_x, middle));
                            path.cubic_bezier_to(
                                point(target_x, bottom),
                                point(active_x, middle + px(mid * 0.55)),
                                point(target_x, bottom - px(mid * 0.55)),
                            );
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                        }

                        // Secondary parents peel away with a smooth S-curve instead of
                        // the previous right-angle connector.
                        for (target, color) in edges {
                            let target_x =
                                bounds.left() + px(target as f32 * GRAPH_LANE_WIDTH + 6.0);
                            let mut path = PathBuilder::stroke(px(1.0));
                            path.move_to(point(active_x, middle));
                            path.cubic_bezier_to(
                                point(target_x, bottom),
                                point(active_x, middle + px(mid * 0.55)),
                                point(target_x, bottom - px(mid * 0.55)),
                            );
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(lane_x - node_size / 2.0))
                    .top(px(mid - node_size / 2.0))
                    .size(px(node_size))
                    .rounded_full()
                    .when(is_head, |node| node.border_1().border_color(active_color))
                    .bg(if is_head {
                        colors().panel
                    } else {
                        active_color
                    }),
            )
    }
}

fn git_status_badge(status: GitFileStatus) -> &'static str {
    match status {
        GitFileStatus::Added => "A",
        GitFileStatus::Modified => "M",
        GitFileStatus::Deleted => "D",
        GitFileStatus::Renamed => "R",
        GitFileStatus::Copied => "C",
        GitFileStatus::TypeChanged => "T",
        GitFileStatus::Untracked => "?",
        GitFileStatus::Conflicted => "U",
    }
}

fn format_short_date(iso: &str) -> String {
    let mut parts = iso.split('-');
    let Some(year) = parts.next() else {
        return iso.to_owned();
    };
    let Some(month) = parts.next().and_then(|month| month.parse::<usize>().ok()) else {
        return iso.to_owned();
    };
    let Some(day) = parts.next() else {
        return iso.to_owned();
    };
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = months.get(month.saturating_sub(1)).copied().unwrap_or("?");
    let day = day.trim_start_matches('0');
    format!("{day} {month} {year}")
}

fn lane_color(lane: usize) -> Rgba {
    const LANES: [fn() -> Rgba; 6] = [
        || colors().accent,
        || gpui::rgba(0xd66aa0ff),
        || colors().success,
        || colors().warning,
        || colors().git_added,
        || colors().danger,
    ];
    LANES[lane % LANES.len()]()
}

#[cfg(test)]
mod tests;
