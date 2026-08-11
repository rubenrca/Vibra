use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Context, Div, EventEmitter, IntoElement, ListHorizontalSizingBehavior, Render, Rgba,
    SharedString, Task, Timer, Window, div, prelude::*, px, uniform_list,
};

use crate::ports::git::{
    GitDiff, GitDiffRow, GitDiffRowKind, GitFileStatus, GitPort, GitRepositorySnapshot,
};
use crate::ui::theme::DARK;

const POLL_INTERVAL: Duration = Duration::from_millis(2_500);
const COLLAPSED_WIDTH: f32 = 300.0;
const EXPANDED_WIDTH: f32 = 420.0;
const DIFF_ROW_HEIGHT: f32 = 18.0;

pub struct DiffView {
    context_root: PathBuf,
    snapshot: Option<GitRepositorySnapshot>,
    expanded_path: Option<String>,
    diff: Option<GitDiff>,
    refreshing: bool,
    diff_loading: bool,
    error: Option<SharedString>,
    snapshot_request_id: u64,
    diff_request_id: u64,
    git_port: Arc<dyn GitPort>,
    _snapshot_task: Option<Task<()>>,
    _diff_task: Option<Task<()>>,
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
            expanded_path: None,
            diff: None,
            refreshing: false,
            diff_loading: false,
            error: None,
            snapshot_request_id: 0,
            diff_request_id: 0,
            git_port,
            _snapshot_task: None,
            _diff_task: None,
            _poll_task: poll_task,
        };
        view.refresh(true, cx);
        view
    }

    pub fn preferred_width(&self) -> f32 {
        if self.expanded_path.is_some() {
            EXPANDED_WIDTH
        } else {
            COLLAPSED_WIDTH
        }
    }

    /// Repo root + relative path → status, for coloring the Files tree (Zed-style).
    pub fn status_index(&self) -> (Option<PathBuf>, std::collections::HashMap<String, GitFileStatus>) {
        match &self.snapshot {
            Some(snapshot) => {
                let map = snapshot
                    .changes
                    .iter()
                    .map(|change| (change.path.replace('\\', "/"), change.status))
                    .collect();
                (Some(snapshot.root.clone()), map)
            }
            None => (None, std::collections::HashMap::new()),
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
        self.select_change(relative_path, cx);
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
        self.diff_loading = false;
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
                        this.expanded_path = None;
                        this.diff = None;
                        this.diff_loading = false;
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
        let expanded_path = self
            .expanded_path
            .as_ref()
            .filter(|path| snapshot.changes.iter().any(|change| &change.path == *path))
            .cloned();
        self.snapshot = Some(snapshot);
        self.expanded_path = expanded_path;
        self.diff = None;
        if self.expanded_path.is_some() {
            self.load_expanded_diff(cx);
        } else {
            self.diff_loading = false;
        }
        // Let the Files tree repaint git status colors.
        cx.emit(DiffViewEvent);
    }

    fn select_change(&mut self, path: String, cx: &mut Context<Self>) {
        let was_expanded = self.expanded_path.is_some();
        if self.expanded_path.as_ref() == Some(&path) {
            // Keep selection; re-click does not collapse (Warp-style master/detail).
            return;
        }
        self.expanded_path = Some(path);
        self.diff = None;
        self.load_expanded_diff(cx);
        if !was_expanded {
            cx.emit(DiffViewEvent);
        }
        cx.notify();
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if self.expanded_path.is_none() {
            return;
        }
        self.expanded_path = None;
        self.diff = None;
        self.diff_loading = false;
        self.diff_request_id = self.diff_request_id.wrapping_add(1);
        cx.emit(DiffViewEvent);
        cx.notify();
    }

    fn load_expanded_diff(&mut self, cx: &mut Context<Self>) {
        let Some((repository, change)) = self.snapshot.as_ref().and_then(|snapshot| {
            let path = self.expanded_path.as_ref()?;
            let change = snapshot
                .changes
                .iter()
                .find(|change| &change.path == path)?
                .clone();
            Some((snapshot.root.clone(), change))
        }) else {
            self.diff = None;
            self.diff_loading = false;
            return;
        };
        self.diff_loading = true;
        self.diff_request_id = self.diff_request_id.wrapping_add(1);
        let request_id = self.diff_request_id;
        let port = self.git_port.clone();
        let task = cx.background_spawn(async move { port.diff(&repository, &change) });
        self._diff_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if request_id != this.diff_request_id {
                    return;
                }
                this.diff_loading = false;
                match result {
                    Ok(diff) => {
                        this.diff = Some(diff);
                        this.error = None;
                    }
                    Err(error) => this.error = Some(format!("Git: {error:#}").into()),
                }
                cx.notify();
            });
        }));
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
        let (branch, files, additions, deletions, ahead, behind) =
            self.snapshot.as_ref().map_or_else(
                || (None, 0, 0, 0, 0, 0),
                |snapshot| {
                    (
                        Some(snapshot.branch.clone()),
                        snapshot.changes.len(),
                        snapshot.additions,
                        snapshot.deletions,
                        snapshot.ahead,
                        snapshot.behind,
                    )
                },
            );
        let loading = self.refreshing;

        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(DARK.panel)
            .border_b_1()
            .border_color(DARK.border_subtle)
            // Branch + stats — quiet, not SCM-app chrome
            .child(
                div()
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.0))
                            .text_color(DARK.accent)
                            .child("⎇"),
                    )
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
                                    .font_family("JetBrains Mono")
                                    .text_size(px(11.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(DARK.foreground)
                                    .child(branch.unwrap_or_else(|| {
                                        if loading {
                                            "…".to_owned()
                                        } else {
                                            "no git".to_owned()
                                        }
                                    })),
                            )
                            .when(files > 0, |row| {
                                row.when(additions > 0, |row| {
                                    row.child(
                                        div()
                                            .font_family("JetBrains Mono")
                                            .text_size(px(9.5))
                                            .text_color(DARK.diff_added)
                                            .child(format!("+{additions}")),
                                    )
                                })
                                .when(deletions > 0, |row| {
                                    row.child(
                                        div()
                                            .font_family("JetBrains Mono")
                                            .text_size(px(9.5))
                                            .text_color(DARK.diff_deleted)
                                            .child(format!("−{deletions}")),
                                    )
                                })
                            })
                            .when(ahead > 0 || behind > 0, |row| {
                                row.child(
                                    div()
                                        .font_family("JetBrains Mono")
                                        .text_size(px(9.0))
                                        .text_color(DARK.subtle)
                                        .child(format!("↑{ahead} ↓{behind}")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("git-refresh")
                            .size(px(22.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .text_size(px(12.0))
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

    fn change_list(&mut self, cx: &mut Context<Self>, fill: bool) -> impl IntoElement {
        let changes = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.changes.clone())
            .unwrap_or_default();
        let expanded_path = self.expanded_path.clone();
        let count = changes.len();
        // When a file is open, keep the list compact; otherwise fill the panel.
        let list_height = if fill {
            None
        } else {
            Some(((count as f32 * 24.0) + 4.0).clamp(48.0, 160.0))
        };

        div()
            .id("git-change-list")
            .w_full()
            .when(fill, |list| list.flex_1().min_h(px(0.0)))
            .when_some(list_height, |list, height| list.h(px(height)).flex_none())
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(!fill, |list| {
                list.border_b_1().border_color(DARK.border_subtle)
            })
            .child(
                div()
                    .h(px(22.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .child(
                        div()
                            .text_size(px(9.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(DARK.subtle)
                            .child("CHANGES"),
                    )
                    .child(
                        div()
                            .px_1()
                            .rounded(px(3.0))
                            .bg(DARK.elevated)
                            .font_family("JetBrains Mono")
                            .text_size(px(9.0))
                            .text_color(DARK.muted)
                            .child(format!("{count}")),
                    ),
            )
            .child(
                div()
                    .id("git-change-rows")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .children(changes.into_iter().map(|change| {
                        let selected = expanded_path.as_ref() == Some(&change.path);
                        let path = change.path.clone();
                        let color = Self::status_color(change.status);
                        let (name, parent) = Self::path_parts(&change.path);
                        let additions = change.additions.unwrap_or_default();
                        let deletions = change.deletions.unwrap_or_default();
                        div()
                            .id(SharedString::from(format!("git-change-{}", change.path)))
                            .h(px(24.0))
                            .w_full()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_1()
                            .pr_2()
                            .cursor_pointer()
                            .bg(if selected {
                                DARK.selection
                            } else {
                                gpui::rgba(0x00000000)
                            })
                            .hover(|row| row.bg(DARK.hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_change(path.clone(), cx);
                            }))
                            .child(
                                div()
                                    .w(px(2.0))
                                    .h_full()
                                    .flex_none()
                                    .bg(if selected {
                                        color
                                    } else {
                                        gpui::rgba(0x00000000)
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .ml_1()
                                    .flex_none()
                                    .rounded(px(3.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(DARK.elevated)
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
                                            .text_size(px(11.0))
                                            .font_weight(if selected {
                                                gpui::FontWeight::MEDIUM
                                            } else {
                                                gpui::FontWeight::NORMAL
                                            })
                                            .text_color(DARK.foreground)
                                            .child(name),
                                    )
                                    .when(!parent.is_empty(), |row| {
                                        row.child(
                                            div()
                                                .min_w(px(0.0))
                                                .flex_1()
                                                .truncate()
                                                .text_size(px(9.5))
                                                .text_color(DARK.subtle)
                                                .child(parent),
                                        )
                                    }),
                            )
                            .when(additions > 0 || deletions > 0, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .font_family("JetBrains Mono")
                                        .text_size(px(9.0))
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
                            })
                            .when(change.staged, |row| {
                                row.child(
                                    div()
                                        .size(px(5.0))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(DARK.diff_added),
                                )
                            })
                    })),
            )
    }

    fn diff_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let path = self.expanded_path.clone();
        let diff = self.diff.clone();
        let loading = self.diff_loading;
        let (name, parent) = path
            .as_deref()
            .map(Self::path_parts)
            .unwrap_or_else(|| (String::new(), String::new()));
        let additions = diff.as_ref().map(|d| d.additions).unwrap_or(0);
        let deletions = diff.as_ref().map(|d| d.deletions).unwrap_or(0);
        let truncated = diff.as_ref().is_some_and(|d| d.truncated);

        div()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(DARK.background)
            .when_some(path.clone(), |panel, _path| {
                panel.child(
                    div()
                        .h(px(28.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .border_b_1()
                        .border_t_1()
                        .border_color(DARK.border_subtle)
                        .bg(DARK.elevated)
                        .child(
                            div()
                                .id("git-clear-selection")
                                .size(px(20.0))
                                .flex_none()
                                .rounded(px(4.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_size(px(12.0))
                                .text_color(DARK.subtle)
                                .hover(|b| b.bg(DARK.hover).text_color(DARK.foreground))
                                .on_click(cx.listener(|this, _, _, cx| this.clear_selection(cx)))
                                .child("×"),
                        )
                        .child(
                            div()
                                .min_w(px(0.0))
                                .flex_1()
                                .flex()
                                .items_baseline()
                                .gap_1()
                                .overflow_hidden()
                                .when(!parent.is_empty(), |row| {
                                    row.child(
                                        div()
                                            .truncate()
                                            .text_size(px(9.5))
                                            .text_color(DARK.subtle)
                                            .child(format!("{parent}/")),
                                    )
                                })
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(11.0))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(DARK.foreground)
                                        .child(name),
                                ),
                        )
                        .when(additions > 0, |row| {
                            row.child(
                                div()
                                    .font_family("JetBrains Mono")
                                    .text_size(px(9.5))
                                    .text_color(DARK.diff_added)
                                    .child(format!("+{additions}")),
                            )
                        })
                        .when(deletions > 0, |row| {
                            row.child(
                                div()
                                    .font_family("JetBrains Mono")
                                    .text_size(px(9.5))
                                    .text_color(DARK.diff_deleted)
                                    .child(format!("−{deletions}")),
                            )
                        }),
                )
            })
            .child(self.diff_body(diff, loading, truncated, path.as_deref(), cx))
    }

    fn diff_body(
        &self,
        diff: Option<GitDiff>,
        loading: bool,
        truncated: bool,
        path: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Div {
        if path.is_none() {
            return self
                .message("Selecciona un archivo para ver el diff")
                .into();
        }

        let row_count = diff.as_ref().map_or(0, |diff| diff.rows.len());
        if row_count == 0 {
            let message = if loading {
                "Cargando diff…"
            } else if diff.as_ref().is_some_and(|diff| diff.binary) {
                "Archivo binario — sin diff de texto."
            } else {
                "Sin cambios de texto."
            };
            return self.message(message).into();
        }

        // Measure the longest line so horizontal scroll can reach the end.
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

        let list_id: SharedString = format!("inline-diff-{}", path.unwrap_or("file")).into();
        div()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(truncated, |panel| {
                panel.child(
                    div()
                        .h(px(22.0))
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
                        let Some(diff) = diff.as_ref() else {
                            return Vec::new();
                        };
                        range
                            .filter_map(|index| diff.rows.get(index))
                            .map(Self::diff_row)
                            .collect()
                    }),
                )
                // Allow long lines to extend past the panel; scroll sideways (trackpad /
                // shift+wheel) instead of clipping the text.
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .with_width_from_item(Some(widest_row_index))
                .flex_1()
                .size_full(),
            )
    }

    fn diff_row(row: &GitDiffRow) -> Div {
        let (background, foreground, bar, marker) = match row.kind {
            GitDiffRowKind::Addition => (
                DARK.diff_added_bg,
                DARK.foreground,
                DARK.diff_added,
                "+",
            ),
            GitDiffRowKind::Deletion => (
                DARK.diff_deleted_bg,
                DARK.foreground,
                DARK.diff_deleted,
                "−",
            ),
            GitDiffRowKind::Hunk => (DARK.diff_hunk_bg, DARK.accent, DARK.accent, " "),
            GitDiffRowKind::Section => (DARK.elevated, DARK.muted, DARK.border_subtle, " "),
            GitDiffRowKind::Notice => (DARK.background, DARK.warning, DARK.warning, "!"),
            GitDiffRowKind::Context => (DARK.background, DARK.muted, gpui::rgba(0x00000000), " "),
        };
        let old_line = row
            .old_line
            .map(|line| line.to_string())
            .unwrap_or_default();
        let new_line = row
            .new_line
            .map(|line| line.to_string())
            .unwrap_or_default();
        let is_hunk = matches!(row.kind, GitDiffRowKind::Hunk | GitDiffRowKind::Section);
        let is_hunk_header = matches!(row.kind, GitDiffRowKind::Hunk);

        div()
            .h(px(DIFF_ROW_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .bg(background)
            .font_family("JetBrains Mono")
            .text_size(px(10.5))
            // Color rail
            .child(
                div()
                    .w(px(3.0))
                    .h_full()
                    .flex_none()
                    .bg(bar),
            )
            // Line-number gutter with IDE-style background
            .child(
                div()
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .bg(DARK.gutter)
                    .border_r_1()
                    .border_color(DARK.border_subtle)
                    .when(!is_hunk, |gutter| {
                        gutter
                            .child(Self::line_number(old_line))
                            .child(Self::line_number(new_line))
                    })
                    .when(is_hunk, |gutter| {
                        gutter.child(div().w(px(72.0)).h_full().flex_none())
                    }),
            )
            .child(
                div()
                    .w(px(14.0))
                    .flex_none()
                    .text_center()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(match row.kind {
                        GitDiffRowKind::Addition => DARK.diff_added,
                        GitDiffRowKind::Deletion => DARK.diff_deleted,
                        GitDiffRowKind::Hunk => DARK.accent,
                        _ => foreground,
                    })
                    .child(marker),
            )
            .child(
                // Intrinsic width — no truncate — so horizontal scroll can reveal the rest.
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .pr_3()
                    .text_color(if is_hunk_header {
                        DARK.accent
                    } else {
                        foreground
                    })
                    .opacity(if is_hunk_header { 0.9 } else { 1.0 })
                    .child(row.text.clone()),
            )
    }

    fn line_number(number: String) -> Div {
        div()
            .w(px(36.0))
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .pr_1()
            .text_size(px(9.0))
            .text_color(DARK.subtle)
            .opacity(0.75)
            .child(number)
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
        let has_selection = self.expanded_path.is_some();

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
            .when(has_repository && !is_clean && !has_selection, |view| {
                // Full-height change list when no file is open (SCM panel style).
                view.child(self.change_list(cx, true))
            })
            .when(has_repository && !is_clean && has_selection, |view| {
                view.child(self.change_list(cx, false))
                    .child(self.diff_panel(cx))
            })
    }
}
