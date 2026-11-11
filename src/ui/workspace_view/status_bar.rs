//! Bottom status bar: subscription usage, project branch, and Keep Awake.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{AnyElement, Context, Div, Timer, div, prelude::*, px, svg};

use crate::ports::git::GitBranchSummary;
use crate::ui::theme::colors;

use super::{RightSidebarMode, WorkspaceView, sidebar_tooltip};

const STATUS_POLL: Duration = Duration::from_secs(4);
pub(super) const STATUS_BAR_HEIGHT: f32 = 30.0;

/// Which status items a strip holds. The usage group sits at the foot of the
/// left sidebar, the branch and Keep Awake at the foot of the right one, so
/// the cards between them reach the bottom of the window. Keep Awake belongs
/// to the right sidebar and hides with it. Only with the left sidebar hidden
/// does usage take a strip under the cards, joined by the branch when the
/// right sidebar is hidden too, except beside Repository or Activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct StatusItems {
    pub(super) usage: bool,
    pub(super) branch: bool,
    pub(super) keep_awake: bool,
}

impl StatusItems {
    pub(super) fn any(self) -> bool {
        self.usage || self.branch || self.keep_awake
    }
}

/// Where the status items go given which sidebars are on screen: the left
/// sidebar's foot, the right sidebar's foot, and the strip under the cards.
/// Without `controls` (Repository, Activity), the strip leaves the branch out.
pub(super) fn status_layout(left: bool, right: bool, controls: bool) -> [StatusItems; 3] {
    let none = StatusItems {
        usage: false,
        branch: false,
        keep_awake: false,
    };
    let left_foot = StatusItems {
        usage: true,
        branch: false,
        keep_awake: false,
    };
    let right_foot = StatusItems {
        usage: false,
        branch: true,
        keep_awake: true,
    };
    let strip = StatusItems {
        usage: !left,
        branch: controls && !left && !right,
        keep_awake: false,
    };
    [
        if left { left_foot } else { none },
        if right { right_foot } else { none },
        strip,
    ]
}
/// Status items stay flat: hovering one lights its icon and label, never a
/// box, so nothing solid sits against the window's rounder bottom corners.
pub(super) const STATUS_ITEM_HEIGHT: f32 = 22.0;

impl WorkspaceView {
    fn toggle_keep_awake(&mut self, cx: &mut Context<Self>) {
        self.keep_awake_error = self
            .keep_awake
            .set_enabled(!self.keep_awake.is_enabled())
            .err()
            .map(|error| error.to_string().into());
        // A failed retry should show its error even after the banner was dismissed.
        if let Some(error) = &self.keep_awake_error {
            self.dismissed_banner_errors.remove(error);
        }
        cx.notify();
    }

    /// Refreshes branch chrome and diff counts for the visible project list.
    pub(super) fn start_status_poll(&mut self, cx: &mut Context<Self>) {
        self._status_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok((target, roots, port)) = this.update(cx, |this, _| {
                    let target = (this.right_sidebar_allowed() && this.has_project_context())
                        .then(|| this.project_root());
                    let mut roots = this
                        .snapshot
                        .projects
                        .iter()
                        .filter_map(|project| project.directory().map(PathBuf::from))
                        .collect::<Vec<_>>();
                    roots.sort();
                    roots.dedup();
                    this.project_diff_stats
                        .retain(|root, _| roots.contains(root));
                    this.project_branches.retain(|root, _| roots.contains(root));
                    if !this.settings.left_sidebar_visible {
                        roots.clear();
                    }
                    (target, roots, this.git_port.clone())
                }) else {
                    break;
                };
                if target.is_none() && roots.is_empty() {
                    Timer::after(STATUS_POLL).await;
                    continue;
                }
                let (summary, stats) = cx
                    .background_spawn(async move {
                        let stats = roots
                            .into_iter()
                            .map(|root| {
                                let stats = port.snapshot(&root).ok().flatten().map(|snapshot| {
                                    super::projects::ProjectDiffStats {
                                        additions: snapshot.additions,
                                        deletions: snapshot.deletions,
                                    }
                                });
                                // The snapshot just filled the branch cache.
                                let branch = port
                                    .branch_summary(&root)
                                    .ok()
                                    .flatten()
                                    .map(|summary| summary.branch);
                                (root, stats, branch)
                            })
                            .collect::<Vec<_>>();
                        // Snapshots populate the branch cache, so reuse it when possible.
                        let summary = target.map(|root| {
                            let summary = port.branch_summary(&root).ok().flatten();
                            (root, summary)
                        });
                        (summary, stats)
                    })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if let Some((root, summary)) = summary {
                            this.apply_branch_summary(root, summary, cx);
                        }
                        for (root, stats, branch) in stats {
                            this.apply_project_branch(root.clone(), branch, cx);
                            this.apply_project_diff_stats(root, stats, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
                Timer::after(STATUS_POLL).await;
            }
        }));
    }

    pub(super) fn apply_branch_summary(
        &mut self,
        root: PathBuf,
        summary: Option<GitBranchSummary>,
        cx: &mut Context<Self>,
    ) {
        // Keep the request's root even on failure. A late failed request must
        // not erase the current project's successful result.
        if !self.has_project_context() || root != self.project_root() {
            return;
        }
        let summary = summary.map(|summary| (root, summary));
        if self.branch_summary != summary {
            self.branch_summary = summary;
            cx.notify();
        }
    }

    pub(super) fn current_branch_summary(&self) -> Option<&GitBranchSummary> {
        self.branch_summary
            .as_ref()
            .filter(|(root, _)| self.has_project_context() && *root == self.project_root())
            .map(|(_, summary)| summary)
    }

    /// A strip of status items on the frame, at the foot of a sidebar or,
    /// when the left sidebar is hidden, under the cards.
    pub(super) fn status_row(&self, items: StatusItems, cx: &mut Context<Self>) -> AnyElement {
        let StatusItems {
            usage,
            branch,
            keep_awake,
        } = items;
        let controls = div()
            .flex_1()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(2.0));
        let controls = self.status_controls(controls, branch, keep_awake, cx);
        div()
            .h(px(STATUS_BAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .px(px(6.0))
            .gap(px(2.0))
            // No fill: the frame behind the columns already carries the tint.
            .text_size(px(11.0))
            .text_color(colors().muted)
            // Usage sits in the middle of the strip: the halves either side
            // share what is left, so the controls only push it aside when
            // they need the room.
            .when(usage, |row| {
                row.child(div().flex_1()).child(self.usage_tracker(cx))
            })
            .child(controls)
            .into_any_element()
    }

    /// The branch, which opens Changes, and the Keep Awake toggle, each when
    /// asked for, added to `row` at the strip's end.
    fn status_controls(
        &self,
        row: Div,
        branch: bool,
        show_keep_awake: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let keep_awake = self.keep_awake.is_enabled();
        // GPUI allows a single hover style per element, so styled items start from `base`.
        // Each item is its own hover group, which its icon follows.
        let base = |id: &'static str| {
            div()
                .id(id)
                .group(id)
                .h(px(STATUS_ITEM_HEIGHT))
                .px(px(7.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .cursor_pointer()
        };
        let item =
            move |id: &'static str| base(id).hover(|item| item.text_color(colors().foreground));
        let icon = |path: &'static str| {
            svg()
                .path(path)
                .size(px(14.0))
                .flex_none()
                .text_color(colors().muted)
        };
        let dot = |color| div().size(px(6.0)).flex_none().rounded_full().bg(color);
        let divider = || {
            div()
                .w(px(1.0))
                .h(px(14.0))
                .mx(px(4.0))
                .flex_none()
                .bg(colors().border_subtle)
        };

        let summary = self.current_branch_summary().filter(|_| branch);
        row.when_some(summary, |bar, summary| {
            bar.child(
                item("status-branch")
                    .tooltip(|_, cx| sidebar_tooltip("Open Changes", cx))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_right_sidebar_mode(RightSidebarMode::Diff, cx);
                        this.focus_selected_terminal(window, cx);
                    }))
                    .child(
                        icon("chrome-icons/git-branch.svg").group_hover("status-branch", |icon| {
                            icon.text_color(colors().foreground)
                        }),
                    )
                    .child(summary.branch.clone())
                    .when(summary.dirty, |item| item.child(dot(colors().warning)))
                    .when(summary.ahead > 0, |item| {
                        item.child(format!("↑{}", summary.ahead))
                    })
                    .when(summary.behind > 0, |item| {
                        item.child(format!("↓{}", summary.behind))
                    }),
            )
            .when(show_keep_awake, |bar| bar.child(divider()))
        })
        .when(show_keep_awake, |row| {
            row.child(
                base("status-keep-awake")
                    .map(|item| {
                        if keep_awake {
                            // The accent alone marks it as running: a filled pill
                            // here would sit right in the window's rounded corner.
                            let accent = colors().accent;
                            item.text_color(accent)
                                .child(icon("chrome-icons/coffee-steam.svg").text_color(accent))
                                .child("Awake")
                        } else {
                            item.px(px(5.0)).child(
                                icon("chrome-icons/coffee.svg")
                                    .group_hover("status-keep-awake", |icon| {
                                        icon.text_color(colors().foreground)
                                    }),
                            )
                        }
                    })
                    .tooltip(move |_, cx| {
                        sidebar_tooltip(
                            if keep_awake {
                                "Keep Awake is on · Click to allow sleep"
                            } else {
                                "Keep Awake · Prevent your Mac from sleeping"
                            },
                            cx,
                        )
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_keep_awake(cx))),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_items_reach_the_strip_only_without_a_left_sidebar() {
        let [left, right, strip] = status_layout(true, true, true);
        assert!(left.usage && !left.keep_awake && !left.branch);
        assert!(right.branch && right.keep_awake);
        assert!(!strip.any(), "the cards reach the bottom");

        // A space with its right sidebar closed: Keep Awake goes with it.
        let [left, right, strip] = status_layout(true, false, true);
        assert!(left.usage && !left.keep_awake && !left.branch);
        assert!(!right.any());
        assert!(!strip.any(), "the cards still reach the bottom");

        let [_, right, strip] = status_layout(false, true, true);
        assert!(right.branch && right.keep_awake);
        assert!(strip.usage && !strip.keep_awake);

        let [left, right, strip] = status_layout(false, false, true);
        assert!(!left.any() && !right.any());
        assert!(strip.usage && strip.branch && !strip.keep_awake);

        // Repository leaves the branch out altogether.
        let [left, _, strip] = status_layout(true, false, false);
        assert!(left.usage && !left.keep_awake);
        assert!(!strip.any());
        let [_, _, strip] = status_layout(false, false, false);
        assert!(strip.usage && !strip.branch && !strip.keep_awake);
    }
}
