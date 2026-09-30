//! Repository generations, asynchronous loads, and diff document caching.

use super::external;
use super::{
    CachedDiffDocument, DiffSource, DiffView, DiffViewEvent, FOLD_DURATION, FileFold, GitPanelMode,
    HISTORY_PAGE, MAX_EXPANDED_DIFFS, PendingDiffLoad, TurnBaseline,
};
use crate::ports::git::{GitCommit, GitDiffRow, GitDiffRowKind, GitRepositorySnapshot};
use crate::ui::diff_document::DiffDocument;
use crate::ui::git_graph::assign_commit_lanes;
use crate::ui::theme::{colors, surface_tint};
use gpui::{Context, IntoElement, ListOffset, Task, div, prelude::*, px};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

impl DiffView {
    fn spawn_query<T, Check, Apply>(
        &mut self,
        cx: &mut Context<Self>,
        request_id: u64,
        still_current: Check,
        work: impl Future<Output = T> + Send + 'static,
        apply: Apply,
    ) -> Task<()>
    where
        T: Send + 'static,
        Check: Fn(&Self) -> u64 + 'static,
        Apply: FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    {
        let task = cx.background_spawn(work);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if still_current(this) != request_id {
                    return;
                }
                apply(this, result, cx);
            });
        })
    }

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
        self.clear_review_comments();
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
        if let Some((_, file)) = &self.file_preview {
            file.update(cx, |file, cx| file.reload(cx));
        }
        if self.panel_visible {
            self.refresh_visible_sources(false, cx);
        }
    }

    pub(super) fn refresh_visible_sources(&mut self, notify_loading: bool, cx: &mut Context<Self>) {
        if self.external.is_some() {
            return;
        }
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

    pub(super) fn set_mode(&mut self, mode: GitPanelMode, cx: &mut Context<Self>) {
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
        self.clear_review_comments();
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
        self._snapshot_task = Some(self.spawn_query(
            cx,
            request_id,
            |this| this.snapshot_request_id,
            async move { port.snapshot(&root) },
            |this, result, cx| {
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
            },
        ));
    }

    fn change_branch_selection(
        &mut self,
        is_base: bool,
        reference: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.clear_review_comments();
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

    pub(super) fn branch_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
        self._branch_task = Some(self.spawn_query(
            cx,
            request_id,
            |this| this.branch_request_id,
            async move {
                let branches = port.branches(&root);
                let changes = port.branch_changes(&root, base.as_deref(), head.as_deref());
                (branches, changes)
            },
            |this, (branches, result), cx| {
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
            },
        ));
    }

    pub(super) fn refresh_history(&mut self, notify_loading: bool, cx: &mut Context<Self>) {
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
        self._history_task = Some(self.spawn_query(
            cx,
            request_id,
            |this| this.history_request_id,
            async move { port.history(&root, HISTORY_PAGE) },
            |this, result, cx| {
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
            },
        ));
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
        self._turn_task = Some(self.spawn_query(
            cx,
            request_id,
            |this| this.turn_request_id,
            async move {
                let Some(current) = port.capture_worktree(&root)? else {
                    return Ok::<_, anyhow::Error>(None);
                };
                let Some(baseline) = baselines.get(&current.root).cloned() else {
                    return Ok(Some((None, None)));
                };
                let changes = port.tree_changes(&current.root, &baseline.tree, &current.tree)?;
                Ok(Some((Some(baseline), Some(changes))))
            },
            |this, result, cx| {
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
            },
        ));
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

    pub(super) fn back_to_history(&mut self, cx: &mut Context<Self>) {
        self.clear_review_comments();
        self.clear_commit();
        self.refresh_history(false, cx);
        cx.emit(DiffViewEvent::Changed);
        cx.notify();
    }

    pub(super) fn select_commit(&mut self, commit: GitCommit, cx: &mut Context<Self>) {
        self.file_preview = None;
        self.clear_review_comments();
        self.clear_commit();
        self.forget_scroll();
        self.selected_commit = Some(commit);
        self.set_review_expanded(true, cx);
        self.refresh_commit(cx);
        cx.emit(DiffViewEvent::ReviewOpened);
        cx.emit(DiffViewEvent::Changed);
    }

    pub(super) fn refresh_commit(&mut self, cx: &mut Context<Self>) {
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
        self._commit_task = Some(self.spawn_query(
            cx,
            request_id,
            |this| this.commit_request_id,
            async move { port.commit_changes(&root, &revision) },
            |this, result, cx| {
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
            },
        ));
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
                    self.source_for_change(snapshot, change, against, head),
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

    pub(super) fn apply_snapshot(
        &mut self,
        snapshot: GitRepositorySnapshot,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn expand_path(&mut self, path: String, cx: &mut Context<Self>) {
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

    pub(super) fn toggle_path(&mut self, path: String, cx: &mut Context<Self>) {
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
    pub(super) fn expand_all(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<String> = self
            .ordered_file_refs()
            .map(|change| change.path.clone())
            .take(MAX_EXPANDED_DIFFS)
            .collect();
        for path in paths {
            self.expand_path(path, cx);
        }
    }

    pub(super) fn collapse_all(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn active_snapshot(&self) -> Option<&GitRepositorySnapshot> {
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

    pub(super) fn load_diff(&mut self, path: String, cx: &mut Context<Self>) {
        if self.external.is_some() && self.pending_loads.len() >= 4 {
            return;
        }
        let Some(source) = self.active_snapshot().and_then(|snapshot| {
            let change = snapshot
                .changes
                .iter()
                .find(|change| change.path == path)?
                .clone();
            let (against, head) = self.active_revisions();
            Some(self.source_for_change(snapshot, &change, against, head))
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
        let external_loader = self.external.as_ref().map(|source| source.load.clone());
        let source_for_task = source.clone();
        let path_for_task = path.clone();
        let pending_path = path.clone();
        let task = self.spawn_query(
            cx,
            request_id,
            move |this| {
                this.pending_loads
                    .get(&pending_path)
                    .map(|pending| pending.request_id)
                    .unwrap_or(0)
            },
            async move {
                let source = &source_for_task;
                if let Some(load) = external_loader {
                    return load(&source.change.path);
                }
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
            },
            move |this, result, cx| {
                let Some(pending) = this.pending_loads.get(&path_for_task) else {
                    return;
                };
                if pending.source != source {
                    return;
                }
                this.pending_loads.remove(&path_for_task);
                match result {
                    Ok(document) => {
                        this.fold_reveals.remove(&path_for_task);
                        if this.full_file {
                            this.fold_reveals
                                .insert(path_for_task.clone(), external::all_folds(&document));
                        }
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
                    Err(error) => {
                        if this.external.is_some() {
                            // A failed remote load is a visible result, not a
                            // missing document to enqueue again in a tight loop.
                            let diff = crate::ports::git::GitDiff {
                                path: path_for_task.clone(),
                                additions: 0,
                                deletions: 0,
                                binary: false,
                                truncated: false,
                                rows: vec![GitDiffRow {
                                    old_line: None,
                                    new_line: None,
                                    kind: GitDiffRowKind::Notice,
                                    text: format!("Diff: {error:#}"),
                                }],
                            };
                            this.documents.insert(
                                path_for_task,
                                CachedDiffDocument {
                                    source,
                                    document: Arc::new(DiffDocument::prepare_with_sources(
                                        diff, None,
                                    )),
                                },
                            );
                        } else {
                            this.error = Some(format!("Git: {error:#}").into());
                        }
                    }
                }
                if this.external.is_some() {
                    this.load_missing_expanded(cx);
                }
                cx.notify();
            },
        );
        self._diff_tasks.push(task);
        // Keep the handle list bounded without cancelling an in-flight load.
        // Cancelling its callback would leave `pending_loads` stuck forever.
        if self._diff_tasks.len() > 12 {
            for task in self._diff_tasks.drain(0..self._diff_tasks.len() - 8) {
                task.detach();
            }
        }
        cx.notify();
    }
}
