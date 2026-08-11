use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Context, Div, EventEmitter, HighlightStyle, IntoElement, ListHorizontalSizingBehavior, Render,
    Rgba, SharedString, Stateful, StyledText, Task, TextStyle, Timer, WhiteSpace, Window, div,
    prelude::*, px, uniform_list,
};

use crate::ports::git::{
    GitDiff, GitDiffRow, GitDiffRowKind, GitFileChange, GitFileStatus, GitPort,
    GitRepositorySnapshot,
};
use crate::ui::syntax::{SyntaxSpan, highlight_diff_rows};
use crate::ui::theme::DARK;

const POLL_INTERVAL: Duration = Duration::from_millis(2_500);
/// Comfortable panel width for Warp-style expandable file cards + inline diffs.
const PANEL_WIDTH: f32 = 420.0;
const DIFF_ROW_HEIGHT: f32 = 18.0;
/// Cap each expanded file's diff viewport so several cards can stay open.
const MAX_INLINE_DIFF_HEIGHT: f32 = 360.0;
const MIN_INLINE_DIFF_HEIGHT: f32 = 54.0;

pub struct DiffView {
    context_root: PathBuf,
    snapshot: Option<GitRepositorySnapshot>,
    /// Paths currently expanded (accordion — multiple allowed, Warp-style).
    expanded: HashSet<String>,
    /// Cached diffs for expanded (and recently expanded) paths.
    diffs: HashMap<String, GitDiff>,
    /// Per-row syntax spans aligned with `diffs[path].rows`.
    highlights: HashMap<String, Vec<Vec<SyntaxSpan>>>,
    /// Paths currently loading a diff.
    loading: HashSet<String>,
    refreshing: bool,
    error: Option<SharedString>,
    snapshot_request_id: u64,
    /// Monotonic id so stale per-path loads are ignored.
    diff_request_id: u64,
    /// path → request id that owns the in-flight load.
    pending_loads: HashMap<String, u64>,
    git_port: Arc<dyn GitPort>,
    _snapshot_task: Option<Task<()>>,
    _diff_tasks: Vec<Task<()>>,
    _poll_task: Task<()>,
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
                        if !this.refreshing {
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
            snapshot: None,
            expanded: HashSet::new(),
            diffs: HashMap::new(),
            highlights: HashMap::new(),
            loading: HashSet::new(),
            refreshing: false,
            error: None,
            snapshot_request_id: 0,
            diff_request_id: 0,
            pending_loads: HashMap::new(),
            git_port,
            _snapshot_task: None,
            _diff_tasks: Vec::new(),
            _poll_task: poll_task,
        };
        view.refresh(true, cx);
        view
    }

    pub fn preferred_width(&self) -> f32 {
        PANEL_WIDTH
    }

    /// Repo root + relative path → status, for coloring the Files tree (Zed-style).
    pub fn status_index(&self) -> (Option<PathBuf>, HashMap<String, GitFileStatus>) {
        match &self.snapshot {
            Some(snapshot) => {
                let map = snapshot
                    .changes
                    .iter()
                    .map(|change| (change.path.replace('\\', "/"), change.status))
                    .collect();
                (Some(snapshot.root.clone()), map)
            }
            None => (None, HashMap::new()),
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
        self.expand_path(relative_path, cx);
        true
    }

    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.context_root == root {
            return;
        }
        self.context_root = root;
        self.snapshot_request_id = self.snapshot_request_id.wrapping_add(1);
        self.diff_request_id = self.diff_request_id.wrapping_add(1);
        self.refreshing = false;
        self.expanded.clear();
        self.diffs.clear();
        self.highlights.clear();
        self.loading.clear();
        self.pending_loads.clear();
        self.error = None;
        self.refresh(true, cx);
    }

    pub fn refresh_now(&mut self, cx: &mut Context<Self>) {
        self.refresh(true, cx);
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
                match result {
                    Ok(Some(snapshot)) => this.apply_snapshot(snapshot, cx),
                    Ok(None) => {
                        this.snapshot = None;
                        this.expanded.clear();
                        this.diffs.clear();
                        this.highlights.clear();
                        this.loading.clear();
                        this.pending_loads.clear();
                        this.error = None;
                    }
                    Err(error) => {
                        this.error = Some(format!("Git: {error:#}").into());
                    }
                }
                cx.notify();
            });
        }));
    }

    fn apply_snapshot(&mut self, snapshot: GitRepositorySnapshot, cx: &mut Context<Self>) {
        self.error = None;
        if self.snapshot.as_ref() == Some(&snapshot) {
            return;
        }
        let live_paths: HashSet<String> = snapshot
            .changes
            .iter()
            .map(|change| change.path.clone())
            .collect();
        self.expanded.retain(|path| live_paths.contains(path));
        self.diffs.retain(|path, _| live_paths.contains(path));
        self.highlights.retain(|path, _| live_paths.contains(path));
        self.loading.retain(|path| live_paths.contains(path));
        self.pending_loads.retain(|path, _| live_paths.contains(path));
        // Snapshot content changed — drop cached diffs so expanded cards reload.
        self.diffs.clear();
        self.highlights.clear();
        self.snapshot = Some(snapshot);
        let to_reload: Vec<String> = self.expanded.iter().cloned().collect();
        for path in to_reload {
            self.load_diff(path, cx);
        }
        // Let the Files tree repaint git status colors.
        cx.emit(DiffViewEvent);
    }

    fn expand_path(&mut self, path: String, cx: &mut Context<Self>) {
        if self.expanded.insert(path.clone()) {
            self.load_diff(path, cx);
            cx.emit(DiffViewEvent);
            cx.notify();
        } else if !self.diffs.contains_key(&path) && !self.loading.contains(&path) {
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

    fn load_diff(&mut self, path: String, cx: &mut Context<Self>) {
        let Some((repository, change)) = self.snapshot.as_ref().and_then(|snapshot| {
            let change = snapshot
                .changes
                .iter()
                .find(|change| change.path == path)?
                .clone();
            Some((snapshot.root.clone(), change))
        }) else {
            self.loading.remove(&path);
            return;
        };

        self.loading.insert(path.clone());
        self.diff_request_id = self.diff_request_id.wrapping_add(1);
        let request_id = self.diff_request_id;
        self.pending_loads.insert(path.clone(), request_id);
        let port = self.git_port.clone();
        let task = cx.background_spawn(async move { port.diff(&repository, &change) });
        let path_for_task = path.clone();
        self._diff_tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(pending_id) = this.pending_loads.get(&path_for_task).copied() else {
                    return;
                };
                if pending_id != request_id {
                    return;
                }
                this.pending_loads.remove(&path_for_task);
                this.loading.remove(&path_for_task);
                match result {
                    Ok(diff) => {
                        let highlights = highlight_diff_rows(&path_for_task, &diff.rows);
                        this.highlights.insert(path_for_task.clone(), highlights);
                        this.diffs.insert(path_for_task, diff);
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
            GitFileStatus::Added | GitFileStatus::Untracked => DARK.diff_added,
            GitFileStatus::Conflicted => DARK.warning,
            GitFileStatus::Deleted => DARK.diff_deleted,
            GitFileStatus::Renamed | GitFileStatus::Copied => DARK.accent,
            GitFileStatus::Modified | GitFileStatus::TypeChanged => DARK.warning,
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
        let (branch, files, additions, deletions) = self.snapshot.as_ref().map_or_else(
            || (None, 0, 0, 0),
            |snapshot| {
                (
                    Some(snapshot.branch.clone()),
                    snapshot.changes.len(),
                    snapshot.additions,
                    snapshot.deletions,
                )
            },
        );
        let loading = self.refreshing;
        let title = if files == 0 {
            "No changes".to_owned()
        } else {
            "Uncommitted changes".to_owned()
        };

        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(DARK.panel)
            .border_b_1()
            .border_color(DARK.border_subtle)
            .child(
                div()
                    .h(px(40.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(DARK.foreground)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .truncate()
                                            .font_family("JetBrains Mono")
                                            .text_size(px(10.0))
                                            .text_color(DARK.subtle)
                                            .child(branch.unwrap_or_else(|| {
                                                if loading {
                                                    "…".to_owned()
                                                } else {
                                                    "no git".to_owned()
                                                }
                                            })),
                                    )
                                    .when(files > 0, |row| {
                                        row.child(
                                            div()
                                                .font_family("JetBrains Mono")
                                                .text_size(px(10.0))
                                                .text_color(DARK.subtle)
                                                .child(format!(
                                                    "{files} file{}",
                                                    if files == 1 { "" } else { "s" }
                                                )),
                                        )
                                        .when(additions > 0, |row| {
                                            row.child(
                                                div()
                                                    .font_family("JetBrains Mono")
                                                    .text_size(px(10.0))
                                                    .text_color(DARK.diff_added)
                                                    .child(format!("+{additions}")),
                                            )
                                        })
                                        .when(deletions > 0, |row| {
                                            row.child(
                                                div()
                                                    .font_family("JetBrains Mono")
                                                    .text_size(px(10.0))
                                                    .text_color(DARK.diff_deleted)
                                                    .child(format!("−{deletions}")),
                                            )
                                        })
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("git-refresh")
                            .size(px(26.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .text_color(if loading { DARK.subtle } else { DARK.muted })
                            .hover(|button| button.bg(DARK.hover).text_color(DARK.foreground))
                            .active(|button| button.opacity(0.72))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_now(cx)))
                            .child(if loading { "·" } else { "↻" }),
                    ),
            )
    }

    fn message(&self, text: &'static str) -> Div {
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .child(
                div()
                    .max_w(px(240.0))
                    .text_center()
                    .text_size(px(11.0))
                    .line_height(px(16.0))
                    .text_color(DARK.subtle)
                    .child(text),
            )
    }

    fn file_cards(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let changes = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.changes.clone())
            .unwrap_or_default();

        div()
            .id("git-file-cards")
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_y_scroll()
            .px_2()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .children(
                changes
                    .into_iter()
                    .map(|change| self.file_card(change, cx))
                    .collect::<Vec<_>>(),
            )
    }

    fn file_card(&self, change: GitFileChange, cx: &mut Context<Self>) -> Stateful<Div> {
        let path = change.path.clone();
        let expanded = self.expanded.contains(&path);
        let loading = self.loading.contains(&path);
        let diff = self.diffs.get(&path).cloned();
        let color = Self::status_color(change.status);
        let (name, parent) = Self::path_parts(&change.path);
        let additions = change
            .additions
            .or_else(|| diff.as_ref().map(|d| d.additions))
            .unwrap_or(0);
        let deletions = change
            .deletions
            .or_else(|| diff.as_ref().map(|d| d.deletions))
            .unwrap_or(0);
        let path_for_click = path.clone();

        div()
            .id(SharedString::from(format!("git-card-{}", change.path)))
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .rounded(px(8.0))
            .border_1()
            .border_color(DARK.border_subtle)
            .bg(DARK.elevated)
            .overflow_hidden()
            // File header — click toggles accordion
            .child(
                div()
                    .id(SharedString::from(format!("git-card-header-{}", change.path)))
                    .h(px(34.0))
                    .w_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .cursor_pointer()
                    .hover(|row| row.bg(DARK.hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_path(path_for_click.clone(), cx);
                    }))
                    .child(
                        div()
                            .w(px(16.0))
                            .flex_none()
                            .text_center()
                            .text_size(px(10.0))
                            .text_color(DARK.subtle)
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
                            .bg(DARK.selection)
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
                            .items_baseline()
                            .gap_1()
                            .overflow_hidden()
                            .pl_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(DARK.foreground)
                                    .child(name),
                            )
                            .when(!parent.is_empty(), |row| {
                                row.child(
                                    div()
                                        .min_w(px(0.0))
                                        .flex_1()
                                        .truncate()
                                        .text_size(px(10.0))
                                        .text_color(DARK.subtle)
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
                                .bg(DARK.diff_added),
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
                                .bg(DARK.selection)
                                .font_family("JetBrains Mono")
                                .text_size(px(10.0))
                                .when(additions > 0, |stats| {
                                    stats.child(
                                        div()
                                            .text_color(DARK.diff_added)
                                            .child(format!("+{additions}")),
                                    )
                                })
                                .when(deletions > 0, |stats| {
                                    stats.child(
                                        div()
                                            .text_color(DARK.diff_deleted)
                                            .child(format!("−{deletions}")),
                                    )
                                }),
                        )
                    }),
            )
            .when(expanded, |card| {
                card.child(self.inline_diff_body(&path, diff, loading, cx))
            })
    }

    fn inline_diff_body(
        &self,
        path: &str,
        diff: Option<GitDiff>,
        loading: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let truncated = diff.as_ref().is_some_and(|d| d.truncated);
        let binary = diff.as_ref().is_some_and(|d| d.binary);
        let row_count = diff.as_ref().map_or(0, |d| d.rows.len());

        if row_count == 0 {
            let message = if loading {
                "Cargando diff…"
            } else if binary {
                "Archivo binario — sin diff de texto."
            } else if diff.is_some() {
                "Sin cambios de texto."
            } else {
                "Cargando diff…"
            };
            return div()
                .w_full()
                .flex_none()
                .border_t_1()
                .border_color(DARK.border_subtle)
                .px_3()
                .py_3()
                .bg(DARK.background)
                .text_size(px(11.0))
                .text_color(DARK.subtle)
                .child(message);
        }

        let height = ((row_count as f32) * DIFF_ROW_HEIGHT)
            .clamp(MIN_INLINE_DIFF_HEIGHT, MAX_INLINE_DIFF_HEIGHT);
        let widest_row_index = diff
            .as_ref()
            .map(|diff| {
                diff.rows
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, row)| row.text.chars().count())
                    .map(|(index, _)| index)
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        let list_id: SharedString = format!("inline-diff-{}", path).into();
        let diff_for_list = diff.clone();
        let highlights_for_list = self.highlights.get(path).cloned().unwrap_or_default();

        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(DARK.border_subtle)
            .bg(DARK.background)
            .when(truncated, |panel| {
                panel.child(
                    div()
                        .h(px(20.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .px_3()
                        .bg(DARK.elevated)
                        .text_size(px(9.0))
                        .text_color(DARK.warning)
                        .child("Diff truncado"),
                )
            })
            .child(
                uniform_list(
                    list_id,
                    row_count,
                    cx.processor(move |_this, range: std::ops::Range<usize>, _window, _cx| {
                        let Some(diff) = diff_for_list.as_ref() else {
                            return Vec::new();
                        };
                        range
                            .filter_map(|index| {
                                let row = diff.rows.get(index)?;
                                let spans = highlights_for_list.get(index).cloned().unwrap_or_default();
                                Some(Self::diff_row(row, &spans))
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
        let (background, marker) = match row.kind {
            GitDiffRowKind::Addition => (DARK.diff_added_bg, "+"),
            GitDiffRowKind::Deletion => (DARK.diff_deleted_bg, "−"),
            GitDiffRowKind::Hunk => (DARK.diff_hunk_bg, " "),
            GitDiffRowKind::Section => (DARK.elevated, " "),
            GitDiffRowKind::Notice => (DARK.background, "!"),
            GitDiffRowKind::Context => (DARK.background, " "),
        };
        // Warp shows a single line-number gutter (prefer new, fall back to old).
        let line_number = row
            .new_line
            .or(row.old_line)
            .map(|line| line.to_string())
            .unwrap_or_default();
        let is_hunk = matches!(row.kind, GitDiffRowKind::Hunk | GitDiffRowKind::Section);
        let is_hunk_header = matches!(row.kind, GitDiffRowKind::Hunk);
        let is_addition = matches!(row.kind, GitDiffRowKind::Addition);
        let is_deletion = matches!(row.kind, GitDiffRowKind::Deletion);
        let is_notice = matches!(row.kind, GitDiffRowKind::Notice);
        let code = Self::styled_code_line(&row.text, spans, row.kind);

        div()
            .h(px(DIFF_ROW_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(background)
            .font_family("JetBrains Mono")
            .text_size(px(11.0))
            // Single line-number gutter (Warp-like)
            .child(
                div()
                    .w(px(40.0))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .pr_2()
                    .text_size(px(10.0))
                    .text_color(DARK.subtle)
                    .opacity(0.7)
                    .child(if is_hunk {
                        String::new()
                    } else {
                        line_number
                    }),
            )
            .child(
                div()
                    .w(px(14.0))
                    .flex_none()
                    .text_center()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(if is_addition {
                        DARK.diff_added
                    } else if is_deletion {
                        DARK.diff_deleted
                    } else if is_hunk_header {
                        DARK.accent
                    } else if is_notice {
                        DARK.warning
                    } else {
                        DARK.subtle
                    })
                    .child(marker),
            )
            .child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .pr_3()
                    .opacity(if is_hunk_header { 0.9 } else { 1.0 })
                    .child(code),
            )
    }

    fn styled_code_line(
        text: &str,
        spans: &[SyntaxSpan],
        kind: GitDiffRowKind,
    ) -> StyledText {
        let default_color = match kind {
            GitDiffRowKind::Hunk | GitDiffRowKind::Section => DARK.accent,
            GitDiffRowKind::Notice => DARK.warning,
            GitDiffRowKind::Context => DARK.muted,
            GitDiffRowKind::Addition | GitDiffRowKind::Deletion => DARK.foreground,
        };
        let default_style = TextStyle {
            color: default_color.into(),
            font_family: "JetBrains Mono".into(),
            font_size: px(11.0).into(),
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
        let has_repository = self.snapshot.is_some();
        let is_clean = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.changes.is_empty());

        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(DARK.panel)
            .child(self.header(cx))
            .when_some(error, |view, error| {
                view.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_2()
                        .bg(DARK.diff_deleted_bg)
                        .border_b_1()
                        .border_color(DARK.danger)
                        .text_size(px(9.0))
                        .line_height(px(14.0))
                        .text_color(DARK.danger)
                        .child(error),
                )
            })
            .when(!has_repository && self.refreshing, |view| {
                view.child(self.message("Leyendo repositorio…"))
            })
            .when(!has_repository && !self.refreshing, |view| {
                view.child(self.message("No hay repositorio Git en este proyecto."))
            })
            .when(is_clean, |view| {
                view.child(self.message("Working tree limpio — sin cambios."))
            })
            .when(has_repository && !is_clean, |view| {
                view.child(self.file_cards(cx))
            })
    }
}
