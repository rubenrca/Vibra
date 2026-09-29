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
mod external;
mod history;
mod rendering;
mod repository;
mod rows;
mod sidebar;
pub(crate) use external::ExternalDiffSource;
pub(crate) use sidebar::DiffFileIndexView;

use std::collections::{HashMap, HashSet, VecDeque};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    Context, EventEmitter, FocusHandle, IntoElement, ListAlignment, ListState, Render, Rgba,
    SharedString, Task, Timer, Window, div, prelude::*, px,
};

use uuid::Uuid;

use crate::ports::files::FileSystemPort;
use crate::ports::git::{
    GitBranchChanges, GitBranchRef, GitCommit, GitCommitChanges, GitDiffRow, GitDiffRowKind,
    GitFileChange, GitFileStatus, GitHistory, GitPort, GitRepositorySnapshot,
};
use crate::ui::diff_document::DiffDocument;
use crate::ui::diff_rows::{
    BodyRow, CommentAnchor, DiffLayout, FoldReveal, ReviewComment, ReviewRow, review_prompt,
};
use crate::ui::file_view::FileView;
use crate::ui::git_graph::GitGraphRow;
use crate::ui::theme::colors;

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
    external: Option<ExternalDiffSource>,
    full_file: bool,
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
    file_preview: Option<(PathBuf, gpui::Entity<FileView>)>,
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
        let worktree_version = if head.is_none() {
            std::fs::symlink_metadata(snapshot.root.join(&change.path))
                .ok()
                .map(|metadata| WorktreeFileVersion {
                    length: metadata.len(),
                    modified: metadata.modified().ok(),
                    changed_seconds: metadata.ctime(),
                    changed_nanoseconds: metadata.ctime_nsec(),
                    inode: metadata.ino(),
                })
        } else {
            None
        };
        Self {
            repository: snapshot.root.clone(),
            change: change.clone(),
            against: against.map(str::to_owned),
            worktree_version,
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
                        if this.external.is_none()
                            && crate::ui::idle::should_poll_git_snapshot(this.panel_visible)
                        {
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
            external: None,
            full_file: false,
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
            file_preview: None,
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
        if let Some((_, file)) = &self.file_preview {
            file.update(cx, |file, cx| file.set_font_size(font_size, cx));
        }
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
        if let Some((path, _)) = &self.file_preview {
            return path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned();
        }
        self.selected_commit
            .as_ref()
            .filter(|_| self.mode == GitPanelMode::History)
            .map(|commit| commit.subject.clone())
            .unwrap_or_else(|| self.mode.label().to_owned())
    }

    pub fn focus_review(&self, window: &mut Window) {
        self.focus_handle.focus(window);
    }

    pub(crate) fn review_icon(&self) -> &'static str {
        if self.file_preview.is_some() {
            "file-icons/file.svg"
        } else if self.review_is_commit() {
            "chrome-icons/git-commit.svg"
        } else {
            "chrome-icons/diff-unified.svg"
        }
    }

    pub(crate) fn preview_path(&self) -> Option<&std::path::Path> {
        self.file_preview.as_ref().map(|(path, _)| path.as_path())
    }

    pub(crate) fn open_file_preview(
        &mut self,
        path: PathBuf,
        port: Arc<dyn FileSystemPort>,
        cx: &mut Context<Self>,
    ) {
        let file = cx.new(|cx| {
            FileView::new(
                self.context_root.clone(),
                path.clone(),
                port,
                self.font_size,
                cx,
            )
        });
        self.file_preview = Some((path, file));
        self.set_review_expanded(true, cx);
        cx.emit(DiffViewEvent::ReviewOpened);
        cx.notify();
    }

    pub(crate) fn review_has_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        self.focus_handle.contains_focused(window, cx)
    }

    fn open_review_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.file_preview = None;
        self.selected_review_path = Some(path.clone());
        self.pending_reveal = Some(path.clone());
        self.expand_path(path, cx);
        self.set_review_expanded(true, cx);
        self.focus_handle.focus(window);
        cx.emit(DiffViewEvent::ReviewOpened);
        cx.emit(DiffViewEvent::Changed);
        cx.notify();
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
            self.file_preview = None;
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
        self.file_preview = None;
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
        if self.file_preview.is_some() {
            if matches!(key, "escape" | "esc") {
                self.set_review_expanded(false, cx);
                cx.emit(DiffViewEvent::ReturnToTerminal);
                cx.stop_propagation();
            }
            return;
        }
        let modifiers = &event.keystroke.modifiers;
        let Some(draft) = self.draft.as_mut() else {
            if self.external.is_none() && self.review_expanded && matches!(key, "escape" | "esc") {
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
}

impl Render for DiffView {
    /// The central review. The Changes sidebar is rendered by
    /// [`DiffFileIndexView`] through [`DiffView::changes_panel`].
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((_, file)) = &self.file_preview {
            return div()
                .id("document-preview")
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(Self::on_key_down))
                .size_full()
                .overflow_hidden()
                .child(file.clone())
                .into_any_element();
        }
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
            // WorkspaceView owns the tab / draggable pane header.
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
            .into_any_element()
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

#[cfg(test)]
mod tests;
