use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Context, Div, EventEmitter, IntoElement, Render, Rgba, SharedString, Task, Timer, Window, div,
    prelude::*, px, uniform_list,
};

use crate::ports::git::{
    GitDiff, GitDiffRow, GitDiffRowKind, GitFileStatus, GitPort, GitRepositorySnapshot,
};
use crate::ui::theme::DARK;

const POLL_INTERVAL: Duration = Duration::from_millis(2_500);
const COLLAPSED_WIDTH: f32 = 300.0;
const EXPANDED_WIDTH: f32 = 460.0;

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
    }

    fn toggle_change(&mut self, path: String, cx: &mut Context<Self>) {
        let was_expanded = self.expanded_path.is_some();
        if self.expanded_path.as_ref() == Some(&path) {
            self.expanded_path = None;
            self.diff = None;
            self.diff_loading = false;
            self.diff_request_id = self.diff_request_id.wrapping_add(1);
        } else {
            self.expanded_path = Some(path);
            self.diff = None;
            self.load_expanded_diff(cx);
        }
        if was_expanded != self.expanded_path.is_some() {
            cx.emit(DiffViewEvent);
        }
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
            GitFileStatus::Added | GitFileStatus::Untracked => DARK.accent,
            GitFileStatus::Conflicted => DARK.warning,
            GitFileStatus::Deleted => DARK.diff_deleted,
            GitFileStatus::Renamed | GitFileStatus::Copied => DARK.diff_added,
            GitFileStatus::Modified | GitFileStatus::TypeChanged => DARK.warning,
        }
    }

    fn file_mark(path: &str) -> String {
        std::path::Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .filter(|extension| extension.len() <= 4)
            .map(|extension| extension.to_uppercase())
            .unwrap_or_else(|| "·".to_owned())
    }

    fn header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.snapshot.as_ref().map(|snapshot| {
            format!(
                "{} · {} files · +{} −{}",
                snapshot.branch,
                snapshot.changes.len(),
                snapshot.additions,
                snapshot.deletions
            )
        });
        let loading = self.refreshing;

        div()
            .h(px(46.0))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .bg(DARK.titlebar)
            .border_b_1()
            .border_color(DARK.border_subtle)
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(DARK.foreground)
                            .child("Changes"),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family("JetBrains Mono")
                            .text_size(px(8.5))
                            .text_color(DARK.subtle)
                            .child(summary.unwrap_or_else(|| {
                                if loading {
                                    "Reading repository…".to_owned()
                                } else {
                                    "Not a Git repository".to_owned()
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .id("git-refresh")
                    .size(px(23.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .font_family("JetBrains Mono")
                    .text_size(px(13.0))
                    .text_color(if loading { DARK.subtle } else { DARK.muted })
                    .hover(|button| button.bg(DARK.hover).text_color(DARK.foreground))
                    .active(|button| button.opacity(0.72))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_now(cx)))
                    .child(if loading { "·" } else { "↻" }),
            )
    }

    fn message(&self, text: &'static str, mark: &'static str) -> Div {
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .child(
                div()
                    .font_family("JetBrains Mono")
                    .text_size(px(18.0))
                    .text_color(DARK.subtle)
                    .child(mark),
            )
            .child(
                div()
                    .max_w(px(240.0))
                    .text_center()
                    .text_size(px(11.0))
                    .line_height(px(16.0))
                    .text_color(DARK.muted)
                    .child(text),
            )
    }

    fn change_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let changes = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.changes.clone())
            .unwrap_or_default();
        let expanded_path = self.expanded_path.clone();
        let diff = self.diff.clone();
        let diff_loading = self.diff_loading;

        div()
            .id("git-change-list")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .children(changes.into_iter().map(|change| {
                let expanded = expanded_path.as_ref() == Some(&change.path);
                let path = change.path.clone();
                let color = Self::status_color(change.status);
                let staged = change.staged;
                let additions = change.additions.unwrap_or_default();
                let deletions = change.deletions.unwrap_or_default();
                let card_diff = expanded.then(|| diff.clone()).flatten();
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .bg(if expanded { DARK.selection } else { DARK.panel })
                    .border_b_1()
                    .border_color(DARK.border_subtle)
                    .child(
                        div()
                            .id(SharedString::from(format!("git-change-{}", change.path)))
                            .h(px(38.0))
                            .w_full()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .cursor_pointer()
                            .hover(|row| row.bg(DARK.hover))
                            .active(|row| row.opacity(0.78))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_change(path.clone(), cx);
                            }))
                            .child(
                                div()
                                    .w(px(12.0))
                                    .flex_none()
                                    .font_family("JetBrains Mono")
                                    .text_size(px(10.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(DARK.subtle)
                                    .child(if expanded { "⌄" } else { "›" }),
                            )
                            .child(
                                div()
                                    .w(px(18.0))
                                    .flex_none()
                                    .text_center()
                                    .font_family("JetBrains Mono")
                                    .text_size(px(9.5))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(color)
                                    .child(change.status.badge()),
                            )
                            .child(
                                div()
                                    .w(px(20.0))
                                    .flex_none()
                                    .truncate()
                                    .text_center()
                                    .font_family("JetBrains Mono")
                                    .text_size(px(7.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(DARK.subtle)
                                    .child(Self::file_mark(&change.path)),
                            )
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex_1()
                                    .truncate()
                                    .text_size(px(11.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(DARK.foreground)
                                    .child(change.path.clone()),
                            )
                            .when(additions > 0 || deletions > 0, |row| {
                                row.child(
                                    div()
                                        .h(px(23.0))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .px_2()
                                        .rounded(px(4.0))
                                        .bg(DARK.elevated)
                                        .font_family("JetBrains Mono")
                                        .text_size(px(8.5))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
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
                            .when(staged, |row| {
                                row.child(
                                    div()
                                        .size(px(5.0))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(DARK.diff_added),
                                )
                            }),
                    )
                    .when(expanded, |card| {
                        card.child(Self::inline_diff(card_diff, diff_loading, &change.path, cx))
                    })
            }))
    }

    fn inline_diff(
        diff: Option<GitDiff>,
        loading: bool,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let row_count = diff.as_ref().map_or(0, |diff| diff.rows.len());
        if row_count == 0 {
            let message = if loading {
                "Loading diff…"
            } else if diff.as_ref().is_some_and(|diff| diff.binary) {
                "Binary file — no textual diff."
            } else {
                "No textual changes."
            };
            return div()
                .h(px(180.0))
                .w_full()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .bg(DARK.background)
                .text_size(px(10.0))
                .text_color(DARK.subtle)
                .child(message);
        }

        let height = (row_count as f32 * 20.0).clamp(180.0, 360.0);
        let list_id: SharedString = format!("inline-diff-{path}").into();
        div()
            .h(px(height))
            .w_full()
            .flex_none()
            .overflow_hidden()
            .bg(DARK.background)
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
                .size_full(),
            )
    }

    fn diff_row(row: &GitDiffRow) -> Div {
        let (background, foreground, marker) = match row.kind {
            GitDiffRowKind::Addition => (DARK.diff_added_bg, DARK.diff_added, "+"),
            GitDiffRowKind::Deletion => (DARK.diff_deleted_bg, DARK.diff_deleted, "−"),
            GitDiffRowKind::Hunk => (DARK.diff_hunk_bg, DARK.accent, "@"),
            GitDiffRowKind::Section => (DARK.elevated, DARK.foreground, "◆"),
            GitDiffRowKind::Notice => (DARK.background, DARK.warning, "!"),
            GitDiffRowKind::Context => (DARK.background, DARK.muted, " "),
        };
        let old_line = row
            .old_line
            .map(|line| line.to_string())
            .unwrap_or_default();
        let new_line = row
            .new_line
            .map(|line| line.to_string())
            .unwrap_or_default();
        div()
            .h(px(20.0))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .overflow_hidden()
            .bg(background)
            .font_family("JetBrains Mono")
            .text_size(px(9.0))
            .child(Self::line_number(old_line))
            .child(Self::line_number(new_line))
            .child(
                div()
                    .w(px(21.0))
                    .flex_none()
                    .text_center()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(foreground)
                    .child(marker),
            )
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(foreground)
                    .child(row.text.clone()),
            )
    }

    fn line_number(number: String) -> Div {
        div()
            .w(px(34.0))
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .pr_2()
            .border_r_1()
            .border_color(DARK.border_subtle)
            .text_color(DARK.subtle)
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
                view.child(self.message("Reading repository…", "·"))
            })
            .when(!has_repository && !self.refreshing, |view| {
                view.child(self.message("No Git repository found for this project.", "·"))
            })
            .when(is_clean, |view| {
                view.child(self.message("Working tree clean.", "✓"))
            })
            .when(has_repository && !is_clean, |view| {
                view.child(self.change_list(cx))
            })
    }
}
