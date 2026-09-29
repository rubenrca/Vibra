//! Review toolbar, metadata, empty states, and scroll container.

use super::{DiffView, GitPanelMode, elapsed_label};
use crate::ui::diff_rows::DiffLayout;
use crate::ui::theme::{colors, floating_surface};
use gpui::{
    Context, DispatchPhase, Div, IntoElement, ScrollWheelEvent, Stateful, Window, canvas, div,
    list, prelude::*, px,
};

impl DiffView {
    pub(super) fn header_meta(&self, loading: bool) -> (String, Vec<Div>) {
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
    pub(super) fn review_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .when(self.external.is_none() && self.review_focused, |bar| {
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
                            this.clear_review_comments();
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

    pub(super) fn message(&self, text: &'static str) -> Div {
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
    pub(super) fn review_list(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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

impl DiffView {
    pub(super) fn empty_message(&self) -> Option<&'static str> {
        if let Some(external) = &self.external {
            return external
                .snapshot
                .changes
                .is_empty()
                .then_some("This pull request has no changed files.");
        }
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
