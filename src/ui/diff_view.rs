use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;

use gpui::{
    Context, Div, EventEmitter, HighlightStyle, IntoElement, ListHorizontalSizingBehavior,
    PathBuilder, Render, Rgba, SharedString, Stateful, StyledText, Task, TextStyle, Timer,
    WhiteSpace, Window, canvas, div, point, prelude::*, px, uniform_list,
};

use crate::ports::git::{
    GitBranchChanges, GitCommit, GitDiffRow, GitDiffRowKind, GitFileChange, GitFileStatus,
    GitGraphRow, GitHistory, GitPort, GitRepositorySnapshot, assign_commit_lanes,
};
use crate::ui::diff_document::DiffDocument;
use crate::ui::syntax::{SyntaxSpan, expand_tabs};
use crate::ui::theme::colors;

const POLL_INTERVAL: Duration = Duration::from_millis(2_500);
const DIFF_ROW_HEIGHT: f32 = 22.0;
const DIFF_FONT_SIZE: f32 = 12.0;
const DIFF_GUTTER_WIDTH: f32 = 40.0;
const DIFF_MARKER_WIDTH: f32 = 3.0;
const HISTORY_ROW_HEIGHT: f32 = 36.0;
const HISTORY_HEADER_HEIGHT: f32 = 24.0;
const GRAPH_LANE_WIDTH: f32 = 12.0;
const HISTORY_AUTHOR_WIDTH: f32 = 88.0;
const HISTORY_DATE_WIDTH: f32 = 88.0;
const HISTORY_SHA_WIDTH: f32 = 64.0;
const HISTORY_PAGE: usize = 250;
/// Cap each expanded file's diff viewport so several cards can stay open.
const MAX_INLINE_DIFF_HEIGHT: f32 = 440.0;
const MIN_INLINE_DIFF_HEIGHT: f32 = 66.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitPanelMode {
    Worktree,
    Branch,
    History,
}

impl GitPanelMode {
    fn label(self) -> &'static str {
        match self {
            Self::Worktree => "Working tree",
            Self::Branch => "Branch changes",
            Self::History => "History",
        }
    }
}

pub struct DiffView {
    context_root: PathBuf,
    mode: GitPanelMode,
    mode_menu_open: bool,
    snapshot: Option<GitRepositorySnapshot>,
    branch_changes: Option<GitBranchChanges>,
    history: Option<Arc<GitHistory>>,
    history_graph: Arc<Vec<GitGraphRow>>,
    /// Paths currently expanded (accordion — multiple allowed, Warp-style).
    expanded: HashSet<String>,
    /// Prepared diffs for expanded (and recently expanded) paths.
    documents: HashMap<String, CachedDiffDocument>,
    status_root: Option<PathBuf>,
    status_index: Arc<HashMap<String, GitFileStatus>>,
    panel_visible: bool,
    /// Paths currently loading a diff.
    loading: HashSet<String>,
    refreshing: bool,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffSource {
    repository: PathBuf,
    change: GitFileChange,
    against: Option<String>,
    worktree_version: Option<WorktreeFileVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorktreeFileVersion {
    length: u64,
    modified: Option<SystemTime>,
}

impl DiffSource {
    fn new(
        snapshot: &GitRepositorySnapshot,
        change: &GitFileChange,
        against: Option<&str>,
    ) -> Self {
        let worktree_version = std::fs::metadata(snapshot.root.join(&change.path))
            .ok()
            .map(|metadata| WorktreeFileVersion {
                length: metadata.len(),
                modified: metadata.modified().ok(),
            });
        Self {
            repository: snapshot.root.clone(),
            change: change.clone(),
            against: against.map(str::to_owned),
            worktree_version,
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

pub struct DiffViewEvent;

impl EventEmitter<DiffViewEvent> for DiffView {}

impl DiffView {
    pub fn new(context_root: PathBuf, git_port: Arc<dyn GitPort>, cx: &mut Context<Self>) -> Self {
        let poll_task = cx.spawn(async move |this, cx| {
            loop {
                Timer::after(POLL_INTERVAL).await;
                if this
                    .update(cx, |this, cx| {
                        if crate::ui::idle::should_poll_git_snapshot(this.panel_visible)
                            && !this.refreshing
                        {
                            this.refresh(false, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut view = Self {
            context_root,
            mode: GitPanelMode::Worktree,
            mode_menu_open: false,
            snapshot: None,
            branch_changes: None,
            history: None,
            history_graph: Arc::new(Vec::new()),
            expanded: HashSet::new(),
            documents: HashMap::new(),
            status_root: None,
            status_index: Arc::new(HashMap::new()),
            panel_visible: false,
            loading: HashSet::new(),
            refreshing: false,
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
        };
        view.refresh(true, cx);
        view
    }

    /// Repo root + relative path → status, for coloring the Files tree (Zed-style).
    pub fn status_index(&self) -> (Option<PathBuf>, Arc<HashMap<String, GitFileStatus>>) {
        (self.status_root.clone(), self.status_index.clone())
    }

    pub fn set_panel_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.panel_visible == visible {
            return;
        }
        self.panel_visible = visible;
        if visible && !self.refreshing {
            self.refresh(false, cx);
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
        self.expand_path(relative_path, cx);
        true
    }

    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.context_root == root {
            return;
        }
        self.context_root = root;
        self.snapshot_request_id = self.snapshot_request_id.wrapping_add(1);
        self.branch_request_id = self.branch_request_id.wrapping_add(1);
        self.history_request_id = self.history_request_id.wrapping_add(1);
        self.diff_request_id = self.diff_request_id.wrapping_add(1);
        self.refreshing = false;
        self.branch_refreshing = false;
        self.history_refreshing = false;
        self.expanded.clear();
        self.documents.clear();
        self.loading.clear();
        self.pending_loads.clear();
        self.branch_changes = None;
        self.history = None;
        self.history_graph = Arc::new(Vec::new());
        self.error = None;
        self.mode_menu_open = false;
        self.refresh(true, cx);
        match self.mode {
            GitPanelMode::Worktree => {}
            GitPanelMode::Branch => self.refresh_branch(true, cx),
            GitPanelMode::History => self.refresh_history(true, cx),
        }
    }

    pub fn refresh_now(&mut self, cx: &mut Context<Self>) {
        self.refresh(true, cx);
        match self.mode {
            GitPanelMode::Worktree => {}
            GitPanelMode::Branch => self.refresh_branch(true, cx),
            GitPanelMode::History => self.refresh_history(true, cx),
        }
    }

    fn set_mode(&mut self, mode: GitPanelMode, cx: &mut Context<Self>) {
        self.mode_menu_open = false;
        if self.mode == mode {
            cx.notify();
            return;
        }
        self.mode = mode;
        self.expanded.clear();
        self.documents.clear();
        self.loading.clear();
        self.pending_loads.clear();
        match mode {
            GitPanelMode::Worktree => {}
            GitPanelMode::Branch => self.refresh_branch(true, cx),
            GitPanelMode::History => self.refresh_history(true, cx),
        }
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
                let mut changed = true;
                match result {
                    Ok(Some(snapshot)) => {
                        if this.snapshot.as_ref() == Some(&snapshot) {
                            changed = false;
                        }
                        this.apply_snapshot(snapshot, cx);
                    }
                    Ok(None) => {
                        this.snapshot = None;
                        this.status_root = None;
                        this.status_index = Arc::new(HashMap::new());
                        if this.mode == GitPanelMode::Worktree {
                            this.expanded.clear();
                            this.documents.clear();
                            this.loading.clear();
                            this.pending_loads.clear();
                        }
                        this.error = None;
                    }
                    Err(error) => {
                        this.error = Some(format!("Git: {error:#}").into());
                    }
                }
                if changed {
                    cx.notify();
                }
            });
        }));
    }

    fn refresh_branch(&mut self, notify_loading: bool, cx: &mut Context<Self>) {
        if self.branch_refreshing {
            return;
        }
        self.branch_refreshing = true;
        if notify_loading {
            cx.notify();
        }
        self.branch_request_id = self.branch_request_id.wrapping_add(1);
        let request_id = self.branch_request_id;
        let root = self.context_root.clone();
        let port = self.git_port.clone();
        let task = cx.background_spawn(async move { port.branch_changes(&root) });
        self._branch_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if request_id != this.branch_request_id {
                    return;
                }
                this.branch_refreshing = false;
                match result {
                    Ok(Some(changes)) => {
                        if this.mode == GitPanelMode::Branch {
                            let against = (!changes.merge_base.is_empty())
                                .then(|| changes.merge_base.clone());
                            this.reconcile_documents(&changes.snapshot, against.as_deref());
                        }
                        this.branch_changes = Some(changes);
                        if this.mode == GitPanelMode::Branch {
                            this.load_missing_expanded(cx);
                        }
                    }
                    Ok(None) => this.branch_changes = None,
                    Err(error) => this.error = Some(format!("Git: {error:#}").into()),
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

    /// Discard documents and in-flight work whose Git source no longer matches
    /// the latest snapshot. A path remaining present is not enough: staging or
    /// changing it must invalidate the prepared rows as well.
    fn reconcile_documents(&mut self, snapshot: &GitRepositorySnapshot, against: Option<&str>) {
        let sources: HashMap<String, DiffSource> = snapshot
            .changes
            .iter()
            .map(|change| {
                (
                    change.path.clone(),
                    DiffSource::new(snapshot, change, against),
                )
            })
            .collect();

        self.expanded.retain(|path| sources.contains_key(path));
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
        self.loading
            .retain(|path| self.pending_loads.contains_key(path));
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
            self.reconcile_documents(&snapshot, None);
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
        cx.emit(DiffViewEvent);
    }

    fn evict_diff_caches(&mut self) {
        const MAX_CACHED_DIFFS: usize = 16;
        if self.documents.len() <= MAX_CACHED_DIFFS {
            return;
        }
        self.documents
            .retain(|path, _| self.expanded.contains(path));
    }

    fn expand_path(&mut self, path: String, cx: &mut Context<Self>) {
        if self.expanded.insert(path.clone()) {
            self.load_diff(path, cx);
            cx.emit(DiffViewEvent);
            cx.notify();
        } else if !self.documents.contains_key(&path) && !self.loading.contains(&path) {
            self.load_diff(path, cx);
            cx.notify();
        }
    }

    fn toggle_path(&mut self, path: String, cx: &mut Context<Self>) {
        if self.expanded.contains(&path) {
            self.expanded.remove(&path);
            self.loading.remove(&path);
            self.pending_loads.remove(&path);
            // Keep cached diff so re-expand is instant.
            cx.emit(DiffViewEvent);
            cx.notify();
            return;
        }
        self.expand_path(path, cx);
    }

    fn active_snapshot(&self) -> Option<&GitRepositorySnapshot> {
        match self.mode {
            GitPanelMode::Worktree => self.snapshot.as_ref(),
            GitPanelMode::Branch => self
                .branch_changes
                .as_ref()
                .map(|changes| &changes.snapshot),
            GitPanelMode::History => None,
        }
    }

    fn load_diff(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(source) = self.active_snapshot().and_then(|snapshot| {
            let change = snapshot
                .changes
                .iter()
                .find(|change| change.path == path)?
                .clone();
            let against = match self.mode {
                GitPanelMode::Branch => self
                    .branch_changes
                    .as_ref()
                    .map(|changes| changes.merge_base.clone())
                    .filter(|base| !base.is_empty()),
                GitPanelMode::Worktree | GitPanelMode::History => None,
            };
            Some(DiffSource::new(snapshot, &change, against.as_deref()))
        }) else {
            self.loading.remove(&path);
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

        self.loading.insert(path.clone());
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
            let diff = if let Some(revision) = &source_for_task.against {
                port.diff_against(
                    &source_for_task.repository,
                    revision,
                    &source_for_task.change,
                )
            } else {
                port.diff(&source_for_task.repository, &source_for_task.change)
            }?;
            Ok::<_, anyhow::Error>(DiffDocument::prepare(diff))
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
                this.loading.remove(&path_for_task);
                match result {
                    Ok(document) => {
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
        // Bound concurrent task handles; completed tasks are inert once dropped.
        if self._diff_tasks.len() > 12 {
            self._diff_tasks.drain(0..self._diff_tasks.len() - 8);
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

    fn header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let loading = match self.mode {
            GitPanelMode::Worktree => self.refreshing,
            GitPanelMode::Branch => self.branch_refreshing,
            GitPanelMode::History => self.history_refreshing,
        };
        let (branch, meta) = self.header_meta(loading);

        div()
            .w_full()
            .flex_none()
            .h(px(44.0))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .bg(colors().panel)
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(self.mode_trigger(cx))
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(colors().muted)
                            .child(branch),
                    )
                    .children(meta),
            )
    }

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
            GitPanelMode::History => {
                if let Some(history) = &self.history {
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

    fn mode_trigger(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.mode_menu_open;
        div()
            .id("git-mode-trigger")
            .flex_none()
            .h(px(28.0))
            .px_3()
            .rounded(px(8.0))
            .border_1()
            .border_color(colors().border_subtle)
            .bg(if open {
                colors().selection
            } else {
                colors().elevated
            })
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .hover(|button| button.bg(colors().hover))
            .on_click(cx.listener(|this, _, _, cx| {
                this.mode_menu_open = !this.mode_menu_open;
                cx.notify();
            }))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(colors().foreground)
                    .child(self.mode.label()),
            )
            .child(
                div()
                    .ml_1()
                    .text_size(px(9.0))
                    .text_color(colors().subtle)
                    .child("▾"),
            )
    }

    fn mode_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let modes = [
            GitPanelMode::Worktree,
            GitPanelMode::Branch,
            GitPanelMode::History,
        ];
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("git-mode-dismiss")
                    .absolute()
                    .inset_0()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.mode_menu_open = false;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .absolute()
                    .top(px(42.0))
                    .left(px(10.0))
                    .w(px(200.0))
                    .rounded(px(12.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .bg(colors().elevated)
                    .shadow_lg()
                    .p_1()
                    .flex()
                    .flex_col()
                    .children(modes.into_iter().map(|mode| {
                        let selected = mode == self.mode;
                        div()
                            .id(SharedString::from(format!("git-mode-{}", mode.label())))
                            .h(px(32.0))
                            .px_3()
                            .rounded(px(8.0))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .bg(if selected {
                                colors().selection
                            } else {
                                gpui::rgba(0x00000000)
                            })
                            .hover(|row| row.bg(colors().hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_mode(mode, cx);
                            }))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(if selected {
                                        gpui::FontWeight::MEDIUM
                                    } else {
                                        gpui::FontWeight::NORMAL
                                    })
                                    .text_color(colors().foreground)
                                    .child(mode.label()),
                            )
                    })),
            )
    }

    fn message(&self, text: &'static str) -> Div {
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

    fn file_cards(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let changes = self
            .active_snapshot()
            .map(|snapshot| snapshot.changes.clone())
            .unwrap_or_default();
        let (staged, unstaged): (Vec<_>, Vec<_>) =
            changes.into_iter().partition(|change| change.staged);

        let mut list = div()
            .id("git-file-cards")
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_y_scroll()
            .px_0()
            .py_0()
            .flex()
            .flex_col()
            .gap_0();

        if !unstaged.is_empty() {
            list = list.children(
                unstaged
                    .into_iter()
                    .map(|change| self.file_card(change, cx)),
            );
        }
        if !staged.is_empty() {
            list = list
                .child(Self::file_section_header("STAGED", staged.len()))
                .children(staged.into_iter().map(|change| self.file_card(change, cx)));
        }
        list
    }

    fn file_section_header(label: &'static str, count: usize) -> Div {
        div()
            .h(px(30.0))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(7.0))
            .px_3()
            .bg(colors().panel)
            .child(
                div()
                    .text_size(px(9.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(colors().muted)
                    .child(label),
            )
            .child(div().h(px(1.0)).flex_1().bg(colors().border_subtle))
            .child(
                div()
                    .font_family("JetBrains Mono")
                    .text_size(px(8.5))
                    .text_color(colors().subtle)
                    .child(count.to_string()),
            )
    }

    fn file_card(&self, change: GitFileChange, cx: &mut Context<Self>) -> Stateful<Div> {
        let path = change.path.clone();
        let expanded = self.expanded.contains(&path);
        let loading = self.loading.contains(&path);
        let document = self
            .documents
            .get(&path)
            .map(|cached| cached.document.clone());
        let color = Self::status_color(change.status);
        let (name, parent) = Self::path_parts(&change.path);
        let additions = change
            .additions
            .or_else(|| document.as_ref().map(|d| d.diff.additions))
            .unwrap_or(0);
        let deletions = change
            .deletions
            .or_else(|| document.as_ref().map(|d| d.diff.deletions))
            .unwrap_or(0);
        let path_for_click = path.clone();

        div()
            .id(SharedString::from(format!("git-card-{}", change.path)))
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(colors().border_subtle)
            .bg(if expanded {
                colors().elevated
            } else {
                colors().panel
            })
            .overflow_hidden()
            // File header — click toggles its inline diff.
            .child(
                div()
                    .id(SharedString::from(format!(
                        "git-card-header-{}",
                        change.path
                    )))
                    .h(px(42.0))
                    .w_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px_3()
                    .cursor_pointer()
                    .hover(|row| row.bg(colors().hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_path(path_for_click.clone(), cx);
                    }))
                    .child(
                        div()
                            .w(px(12.0))
                            .flex_none()
                            .text_center()
                            .text_size(px(10.0))
                            .text_color(colors().subtle)
                            .child(if expanded { "▾" } else { "▸" }),
                    )
                    .child(
                        div()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex_none()
                            .rounded(px(4.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(colors().selection)
                            .font_family("JetBrains Mono")
                            .text_size(px(9.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(color)
                            .child(change.status.badge()),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap(px(1.0))
                            .overflow_hidden()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(colors().foreground)
                                    .child(name),
                            )
                            .when(!parent.is_empty(), |row| {
                                row.child(
                                    div()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .text_size(px(9.0))
                                        .text_color(colors().subtle)
                                        .child(parent),
                                )
                            }),
                    )
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
                                .gap_1()
                                .px_1()
                                .py_0()
                                .rounded(px(4.0))
                                .bg(colors().selection)
                                .font_family("JetBrains Mono")
                                .text_size(px(10.0))
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
                                            .child(format!("−{deletions}")),
                                    )
                                }),
                        )
                    }),
            )
            .when(expanded, |card| {
                card.child(self.inline_diff_body(&path, document, loading, cx))
            })
    }

    fn inline_diff_body(
        &self,
        path: &str,
        document: Option<Arc<DiffDocument>>,
        loading: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let truncated = document.as_ref().is_some_and(|d| d.diff.truncated);
        let binary = document.as_ref().is_some_and(|d| d.diff.binary);
        let row_count = document.as_ref().map_or(0, |d| d.diff.rows.len());

        if row_count == 0 {
            let message = if loading {
                "Cargando diff…"
            } else if binary {
                "Archivo binario — sin diff de texto."
            } else if document.is_some() {
                "Sin cambios de texto."
            } else {
                "Cargando diff…"
            };
            return div()
                .w_full()
                .flex_none()
                .border_t_1()
                .border_color(colors().border_subtle)
                .px_3()
                .py_3()
                .bg(colors().background)
                .text_size(px(11.0))
                .text_color(colors().subtle)
                .child(message);
        }

        let height = ((row_count as f32) * DIFF_ROW_HEIGHT)
            .clamp(MIN_INLINE_DIFF_HEIGHT, MAX_INLINE_DIFF_HEIGHT);
        let widest_row_index = document
            .as_ref()
            .map(|document| document.widest_row_index)
            .unwrap_or(0);
        let list_id: SharedString = format!("inline-diff-{}", path).into();
        let document_for_list = document.clone();

        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            // The nested uniform list owns wheel input while hovered. Without
            // stopping the bubble here, the file-card scroller moves as well.
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .border_t_1()
            .border_color(colors().border_subtle)
            .bg(colors().background)
            .when(truncated, |panel| {
                panel.child(
                    div()
                        .h(px(20.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .px_3()
                        .bg(colors().elevated)
                        .text_size(px(9.0))
                        .text_color(colors().warning)
                        .child("Diff truncado"),
                )
            })
            .child(
                uniform_list(
                    list_id,
                    row_count,
                    cx.processor(move |_this, range: std::ops::Range<usize>, _window, _cx| {
                        let Some(document) = document_for_list.as_ref() else {
                            return Vec::new();
                        };
                        range
                            .filter_map(|index| {
                                let row = document.diff.rows.get(index)?;
                                let spans: &[SyntaxSpan] =
                                    document.highlights.get(index).map_or(&[], Vec::as_slice);
                                Some(Self::diff_row(row, spans))
                            })
                            .collect()
                    }),
                )
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .with_width_from_item(Some(widest_row_index))
                .h(px(height))
                .w_full(),
            )
    }

    fn diff_row(row: &GitDiffRow, spans: &[SyntaxSpan]) -> Div {
        let background = match row.kind {
            GitDiffRowKind::Addition => colors().diff_added_bg,
            GitDiffRowKind::Deletion => colors().diff_deleted_bg,
            GitDiffRowKind::Hunk => colors().diff_hunk_bg,
            GitDiffRowKind::Section => colors().elevated,
            GitDiffRowKind::Notice => colors().background,
            GitDiffRowKind::Context => colors().background,
        };
        let gutter_bg = match row.kind {
            GitDiffRowKind::Addition => colors().diff_added_bg,
            GitDiffRowKind::Deletion => colors().diff_deleted_bg,
            GitDiffRowKind::Hunk | GitDiffRowKind::Section => colors().diff_hunk_bg,
            _ => colors().gutter,
        };
        let marker = match row.kind {
            GitDiffRowKind::Addition => colors().diff_added,
            GitDiffRowKind::Deletion => colors().diff_deleted,
            GitDiffRowKind::Notice => colors().warning,
            _ => gpui::rgba(0x00000000),
        };
        if matches!(row.kind, GitDiffRowKind::Hunk | GitDiffRowKind::Section) {
            return Self::diff_separator(gutter_bg);
        }
        let line_number = match row.kind {
            GitDiffRowKind::Deletion => row.old_line,
            GitDiffRowKind::Notice => None,
            _ => row.new_line.or(row.old_line),
        }
        .map(|line| line.to_string())
        .unwrap_or_default();
        let code = Self::styled_code_line(&row.text, spans, row.kind);

        div()
            .h(px(DIFF_ROW_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(background)
            .font_family("JetBrains Mono")
            .text_size(px(DIFF_FONT_SIZE))
            .line_height(px(DIFF_ROW_HEIGHT))
            .child(Self::diff_gutter(&line_number, gutter_bg))
            .child(
                div()
                    .w(px(DIFF_MARKER_WIDTH))
                    .h_full()
                    .flex_none()
                    .bg(marker),
            )
            .child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .pl_2()
                    .pr_4()
                    .child(code),
            )
    }

    fn diff_separator(gutter_bg: Rgba) -> Div {
        div()
            .h(px(DIFF_ROW_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(colors().diff_hunk_bg)
            .child(Self::diff_gutter("", gutter_bg))
            .child(div().w(px(DIFF_MARKER_WIDTH)).h_full().flex_none())
            .child(div().flex_1().h(px(1.0)).mr_4().bg(colors().border_subtle))
    }

    fn diff_gutter(number: &str, background: Rgba) -> Div {
        div()
            .w(px(DIFF_GUTTER_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .pr_2()
            .bg(background)
            .border_r_1()
            .border_color(colors().border_subtle)
            .font_family("JetBrains Mono")
            .text_size(px(10.5))
            .text_color(colors().muted)
            .child(number.to_owned())
    }

    fn history_graph_width(graph: &[GitGraphRow]) -> f32 {
        let lanes = graph.iter().map(|row| row.lane_count).max().unwrap_or(1);
        (lanes as f32 * GRAPH_LANE_WIDTH).clamp(18.0, 48.0)
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
                    cx.processor(move |_this, range: std::ops::Range<usize>, _window, _cx| {
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
                                let row = graph.get(index);
                                Some(
                                    Self::history_row(
                                        commit,
                                        row,
                                        graph_width,
                                        head.as_deref().is_some_and(|head| commit.sha == head),
                                    )
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
            cell = cell.font_family("JetBrains Mono");
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
            .hover(|row| row.bg(colors().hover))
            .child(Self::graph_column(graph, graph_width, is_head))
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

    fn graph_column(graph: Option<&GitGraphRow>, width: f32, is_head: bool) -> Div {
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
        let mid = HISTORY_ROW_HEIGHT / 2.0;
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

    fn styled_code_line(text: &str, spans: &[SyntaxSpan], kind: GitDiffRowKind) -> StyledText {
        let text = expand_tabs(text);
        let default_color = match kind {
            GitDiffRowKind::Hunk | GitDiffRowKind::Section => colors().muted,
            GitDiffRowKind::Notice => colors().warning,
            GitDiffRowKind::Context | GitDiffRowKind::Addition | GitDiffRowKind::Deletion => {
                colors().foreground
            }
        };
        let default_style = TextStyle {
            color: default_color.into(),
            font_family: "JetBrains Mono".into(),
            font_size: px(DIFF_FONT_SIZE).into(),
            line_height: px(DIFF_ROW_HEIGHT).into(),
            white_space: WhiteSpace::Nowrap,
            ..Default::default()
        };

        // Hunk / notice lines keep a single accent color.
        if matches!(
            kind,
            GitDiffRowKind::Hunk | GitDiffRowKind::Section | GitDiffRowKind::Notice
        ) || spans.is_empty()
        {
            return StyledText::new(text.to_owned()).with_default_highlights(
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

        StyledText::new(text.to_owned()).with_default_highlights(&default_style, highlights)
    }
}

impl Render for DiffView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.error.clone();
        let menu_open = self.mode_menu_open;
        let empty_message = self.empty_message();
        let show_history = empty_message.is_none() && self.mode == GitPanelMode::History;
        let show_files = empty_message.is_none()
            && matches!(self.mode, GitPanelMode::Worktree | GitPanelMode::Branch);
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(colors().panel)
            .child(self.header(cx))
            .when_some(error, |view, error| {
                view.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_2()
                        .bg(colors().diff_deleted_bg)
                        .border_b_1()
                        .border_color(colors().danger)
                        .text_size(px(9.0))
                        .line_height(px(14.0))
                        .text_color(colors().danger)
                        .child(error),
                )
            })
            .when_some(empty_message, |view, text| view.child(self.message(text)))
            .when(show_history, |view| view.child(self.history_list(cx)))
            .when(show_files, |view| view.child(self.file_cards(cx)))
            .when(menu_open, |view| view.child(self.mode_menu(cx)))
    }
}

impl DiffView {
    fn empty_message(&self) -> Option<&'static str> {
        match self.mode {
            GitPanelMode::Worktree => {
                if self.snapshot.is_none() && self.refreshing {
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
                if self.branch_changes.is_none() && (self.branch_refreshing || self.refreshing) {
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
            GitPanelMode::History => {
                if self.history.is_none() && (self.history_refreshing || self.refreshing) {
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
    format!("{month} {day}, {year}")
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
mod tests {
    use super::*;

    #[test]
    fn diff_source_tracks_worktree_file_changes() {
        let root = std::env::temp_dir().join(format!(
            "vibra-diff-source-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&root).expect("create test repository directory");
        std::fs::write(root.join("main.rs"), "fn main() {}\n").expect("write first contents");

        let change = GitFileChange {
            path: "main.rs".into(),
            status: GitFileStatus::Modified,
            staged: false,
            unstaged: true,
            untracked: false,
            additions: Some(1),
            deletions: Some(1),
        };
        let snapshot = GitRepositorySnapshot {
            root: root.clone(),
            branch: "main".into(),
            changes: vec![change.clone()],
            additions: 1,
            deletions: 1,
        };
        let before = DiffSource::new(&snapshot, &change, None);

        std::fs::write(
            root.join("main.rs"),
            "fn main() { println!(\"changed\"); }\n",
        )
        .expect("write changed contents");
        let after = DiffSource::new(&snapshot, &change, None);

        assert_ne!(before, after);
        std::fs::remove_dir_all(root).expect("remove test repository directory");
    }
}
