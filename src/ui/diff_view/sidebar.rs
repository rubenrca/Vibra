//! Compact file navigation for the Changes sidebar.

use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*, px, svg, uniform_list};

use super::{DiffView, GitFileChange, GitPanelMode, git_status_badge};
use crate::ports::files::FileEntryKind;
use crate::ui::theme::{MONO_FONT, colors, surface_tint};
use crate::ui::workspace_view::{file_tree_icon, file_tree_icon_color};

const FILE_ROW_HEIGHT: f32 = 28.0;

enum FileIndexRow {
    Group {
        label: &'static str,
        count: usize,
        /// Working-tree groups: whether this is the staged group.
        staged: Option<bool>,
    },
    File(GitFileChange),
}

impl DiffView {
    /// A virtualized navigation list; opening a file never folds the review.
    pub(super) fn compact_file_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let message = self.empty_message().or_else(|| {
            (self.mode == GitPanelMode::History && self.selected_commit.is_none())
                .then_some("Select a commit to browse its changes.")
        });
        if let Some(message) = message {
            return compact_message(message);
        }

        let files = self.ordered_file_refs().cloned().collect::<Vec<_>>();
        if files.is_empty() {
            return compact_message("No file changes.");
        }

        let mut rows = Vec::with_capacity(files.len() + 2);
        if self.mode == GitPanelMode::Worktree {
            let staged_count = files.iter().filter(|file| file.staged).count();
            let unstaged_count = files.len() - staged_count;
            for (staged, label, count) in [
                (false, "Changes", unstaged_count),
                (true, "Staged Changes", staged_count),
            ] {
                if count == 0 {
                    continue;
                }
                rows.push(FileIndexRow::Group {
                    label,
                    count,
                    staged: Some(staged),
                });
                rows.extend(
                    files
                        .iter()
                        .filter(|file| file.staged == staged)
                        .cloned()
                        .map(FileIndexRow::File),
                );
            }
        } else {
            rows.push(FileIndexRow::Group {
                label: "Changes",
                count: files.len(),
                staged: None,
            });
            rows.extend(files.into_iter().map(FileIndexRow::File));
        }

        uniform_list(
            "git-compact-file-rows",
            rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                range
                    .filter_map(|index| {
                        let row = rows.get(index)?;
                        Some(match row {
                            FileIndexRow::Group {
                                label,
                                count,
                                staged,
                            } => this.file_group(label, *count, *staged, cx),
                            FileIndexRow::File(file) => this.compact_file_row(file, cx),
                        })
                    })
                    .collect()
            }),
        )
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .into_any_element()
    }

    fn file_group(
        &self,
        label: &'static str,
        count: usize,
        staged: Option<bool>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!("git-file-group-{label}")))
            .group("git-file-row")
            .h(px(FILE_ROW_HEIGHT))
            .w_full()
            .px_2()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors().muted)
                    .child(label.to_uppercase()),
            )
            .child(
                div()
                    .min_w(px(16.0))
                    .h(px(16.0))
                    .px_1()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(surface_tint(colors().elevated, colors().sidebar))
                    .text_size(px(9.0))
                    .text_color(colors().muted)
                    .child(count.to_string()),
            )
            .child(div().flex_1())
            .when_some(staged, |row, staged| {
                row.child(
                    stage_button(SharedString::from(format!("git-stage-all-{label}")), staged)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let paths = this
                                .snapshot
                                .as_ref()
                                .map(|snapshot| {
                                    snapshot
                                        .changes
                                        .iter()
                                        .filter(|change| change.staged == staged)
                                        .map(|change| change.path.clone())
                                        .collect()
                                })
                                .unwrap_or_default();
                            this.stage_paths(paths, !staged, cx);
                        })),
                )
            })
            .into_any_element()
    }

    fn compact_file_row(&self, file: &GitFileChange, cx: &mut Context<Self>) -> AnyElement {
        let (name, parent) = Self::path_parts(&file.path);
        let selected = self.selected_review_path.as_deref() == Some(file.path.as_str());
        let path = file.path.clone();
        let counts = self.document(&file.path).map_or(
            (file.additions.unwrap_or(0), file.deletions.unwrap_or(0)),
            |document| (document.diff.additions, document.diff.deletions),
        );

        div()
            .id(SharedString::from(format!("git-file-index-{}", file.path)))
            .group("git-file-row")
            .h(px(FILE_ROW_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px_2()
            .overflow_hidden()
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(surface_tint(colors().selection, colors().sidebar))
            })
            .hover(|row| row.bg(surface_tint(colors().hover, colors().sidebar)))
            .child({
                let color = file_tree_icon_color(FileEntryKind::File, &name);
                div()
                    .size(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(color)
                    .child(file_tree_icon(FileEntryKind::File, false, &name, color))
            })
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .flex()
                    .items_baseline()
                    .gap(px(6.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors().foreground)
                            .child(name),
                    )
                    .when(!parent.is_empty(), |row| {
                        row.child(
                            div()
                                .min_w(px(0.0))
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(11.0))
                                .text_color(colors().subtle)
                                .child(parent),
                        )
                    }),
            )
            .when(counts.0 > 0, |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(10.0))
                        .text_color(colors().diff_added)
                        .child(format!("+{}", counts.0)),
                )
            })
            .when(counts.1 > 0, |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(10.0))
                        .text_color(colors().diff_deleted)
                        .child(format!("−{}", counts.1)),
                )
            })
            .when(self.mode == GitPanelMode::Worktree, |row| {
                let staged = file.staged;
                let path = file.path.clone();
                row.child(
                    stage_button(
                        SharedString::from(format!("git-stage-{}", file.path)),
                        staged,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.stage_paths(vec![path.clone()], !staged, cx);
                    })),
                )
            })
            .child(
                div()
                    .w(px(14.0))
                    .flex_none()
                    .text_right()
                    .font_family(MONO_FONT)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(11.0))
                    .text_color(Self::status_color(file.status))
                    .child(git_status_badge(file.status)),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_review_path(path.clone(), window, cx);
            }))
            .into_any_element()
    }
}

/// `+` stages, `−` unstages; shown on hover so the list stays calm.
fn stage_button(id: SharedString, staged: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(20.0))
        .flex_none()
        .rounded(px(4.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .opacity(0.0)
        .group_hover("git-file-row", |button| button.opacity(1.0))
        .text_color(colors().muted)
        .hover(|button| button.bg(colors().hover).text_color(colors().foreground))
        .child(
            svg()
                .path(if staged {
                    "chrome-icons/minus.svg"
                } else {
                    "chrome-icons/plus.svg"
                })
                .size(px(13.0)),
        )
}

fn compact_message(message: &'static str) -> AnyElement {
    div()
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .px_3()
        .py_2()
        .text_size(px(12.0))
        .line_height(px(18.0))
        .text_color(colors().subtle)
        .child(message)
        .into_any_element()
}

/// The Changes sidebar. A separate view observes the shared review so the
/// panel updates without mounting DiffView twice or repainting the entire
/// workspace on every code scroll.
pub(crate) struct DiffFileIndexView {
    review: gpui::Entity<DiffView>,
    _subscription: gpui::Subscription,
}

impl DiffFileIndexView {
    pub(crate) fn new(review: gpui::Entity<DiffView>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&review, |_, _, cx| cx.notify());
        Self {
            review,
            _subscription: subscription,
        }
    }
}

impl gpui::Render for DiffFileIndexView {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.review
            .update(cx, |review, cx| review.changes_panel(window, cx))
    }
}
