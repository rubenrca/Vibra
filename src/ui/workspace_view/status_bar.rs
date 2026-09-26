//! Bottom status bar: the project's branch, running agents, and the Inbox.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{AnyElement, Context, Timer, div, prelude::*, px, svg};

use crate::domain::agents::{AgentAttention, AgentRuntimeState};
use crate::ports::git::GitBranchSummary;
use crate::ui::theme::{colors, surface, surface_tint};

use super::{RightSidebarMode, WorkspaceSection, WorkspaceView, sidebar_tooltip};

const STATUS_POLL: Duration = Duration::from_secs(4);
pub(super) const STATUS_BAR_HEIGHT: f32 = 30.0;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct AgentCounts {
    pub working: usize,
    pub waiting: usize,
    pub permission: usize,
}

impl WorkspaceView {
    /// Polls the branch summary for the selected project; cheap and cached.
    pub(super) fn start_status_poll(&mut self, cx: &mut Context<Self>) {
        self._status_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(target) = this.update(cx, |this, _| {
                    (this.workspace_section == WorkspaceSection::Workspace
                        && this.has_project_context())
                    .then(|| (this.project_root(), this.git_port.clone()))
                }) else {
                    break;
                };
                let Some((root, port)) = target else {
                    Timer::after(STATUS_POLL).await;
                    continue;
                };
                let (root, summary) = cx
                    .background_spawn(async move {
                        let summary = port.branch_summary(&root).ok().flatten();
                        (root, summary)
                    })
                    .await;
                if this
                    .update(cx, |this, cx| this.apply_branch_summary(root, summary, cx))
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

    pub(super) fn agent_counts(&self) -> AgentCounts {
        let mut counts = AgentCounts::default();
        for session in self.snapshot.terminal_sessions() {
            let Some(presence) = self.resolved_agent_presence(session.id) else {
                continue;
            };
            match (presence.state, presence.attention) {
                (AgentRuntimeState::Waiting, Some(AgentAttention::Permission)) => {
                    counts.permission += 1
                }
                (AgentRuntimeState::Waiting, _) => counts.waiting += 1,
                (AgentRuntimeState::Working, _) => counts.working += 1,
                (AgentRuntimeState::Idle, _) => {}
            }
        }
        counts
    }

    pub(super) fn status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let counts = self.agent_counts();
        let unread = self.inbox.unread_count();
        let item = |id: &'static str| {
            div()
                .id(id)
                .h(px(22.0))
                .px(px(7.0))
                .rounded(px(6.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .cursor_pointer()
                .hover(|item| {
                    item.bg(surface_tint(colors().hover, colors().titlebar))
                        .text_color(colors().foreground)
                })
        };
        let icon = |path: &'static str| svg().path(path).size(px(14.0)).flex_none();
        let dot = |color| div().size(px(6.0)).flex_none().rounded_full().bg(color);
        let separator = || div().text_color(colors().subtle).child("·");

        div()
            .h(px(STATUS_BAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .px(px(6.0))
            .gap(px(2.0))
            .border_t_1()
            .border_color(colors().border_subtle)
            .bg(surface(colors().titlebar))
            .text_size(px(12.0))
            .text_color(colors().muted)
            .when_some(self.current_branch_summary().cloned(), |bar, summary| {
                bar.child(
                    item("status-branch")
                        .tooltip(|_, cx| sidebar_tooltip("Abrir Changes", cx))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_workspace_mode(RightSidebarMode::Diff, cx);
                            this.focus_selected_terminal(window, cx);
                        }))
                        .child(icon("chrome-icons/git-branch.svg"))
                        .child(summary.branch.clone())
                        .when(summary.dirty, |item| item.child(dot(colors().warning)))
                        .when(summary.ahead > 0, |item| {
                            item.child(format!("↑{}", summary.ahead))
                        })
                        .when(summary.behind > 0, |item| {
                            item.child(format!("↓{}", summary.behind))
                        }),
                )
            })
            .child(
                item("status-agents")
                    .tooltip(|_, cx| sidebar_tooltip("Ver agentes en el Inbox", cx))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_section(WorkspaceSection::Inbox, window, cx)
                    }))
                    .map(|item| {
                        if counts == AgentCounts::default() {
                            item.child(dot(colors().subtle))
                                .child("Sin agentes activos")
                        } else {
                            let groups = [
                                (counts.working, colors().accent, "trabajando"),
                                (counts.waiting, colors().warning, "esperan"),
                                (counts.permission, colors().danger, "piden permiso"),
                            ];
                            let mut first = true;
                            groups.into_iter().filter(|(count, _, _)| *count > 0).fold(
                                item,
                                |item, (count, color, label)| {
                                    let item = if first { item } else { item.child(separator()) };
                                    first = false;
                                    item.child(dot(color)).child(format!("{count} {label}"))
                                },
                            )
                        }
                    }),
            )
            .child(div().flex_1())
            .child(
                item("status-inbox")
                    .tooltip(|_, cx| sidebar_tooltip("Inbox", cx))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_section(WorkspaceSection::Inbox, window, cx)
                    }))
                    .when(unread > 0, |item| item.text_color(colors().accent))
                    .child(icon("chrome-icons/inbox.svg"))
                    .child(if unread > 0 {
                        format!("{unread} sin leer")
                    } else {
                        "Al día".to_owned()
                    }),
            )
            .into_any_element()
    }
}
