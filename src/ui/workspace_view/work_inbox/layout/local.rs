//! A pull request's local branch: where it is checked out and the Git
//! actions that bring it to (or clean it from) the project's repository.

use super::*;
use crate::ui::workspace_view::work_inbox::local::{LocalPull, PullGit};

impl WorkspaceView {
    /// Status chip and buttons next to the pull request's other actions.
    pub(super) fn inbox_local_controls(
        &self,
        item: &WorkItem,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(pull) = item.pull.as_ref() else {
            return Vec::new();
        };
        let (root, local) = self.local_pull(item);
        if root.is_none() {
            return Vec::new();
        }
        let busy = self.local_job_label(&item.url);
        let open = matches!(item.status, WorkStatus::Open | WorkStatus::Draft);
        let merged = matches!(item.status, WorkStatus::Merged | WorkStatus::Closed);
        let mut controls = Vec::new();
        let url = item.url.clone();
        let button = |id: &'static str, label: String, primary: bool| {
            section_button(id, label, primary).when(busy.is_some(), |button| button.opacity(0.5))
        };
        let git =
            |this: &mut Self, action: PullGit, window: &mut Window, cx: &mut Context<Self>| {
                if let Some(url) = this.work_inbox.selected.clone() {
                    this.run_pull_git(&url, action, window, cx);
                }
            };
        match local {
            LocalPull::Unknown => {}
            LocalPull::Missing if open => {
                controls.push(
                    button(
                        "inbox-checkout",
                        busy.unwrap_or("Check out").to_owned(),
                        false,
                    )
                    .tooltip(|_, cx| sidebar_tooltip("gh pr checkout in the project (C)", cx))
                    .on_click(cx.listener({
                        let url = url.clone();
                        move |this, _, window, cx| this.checkout_pull(&url, window, cx)
                    }))
                    .into_any_element(),
                );
                controls.push(self.inbox_worktree_button(&url, busy, cx));
            }
            LocalPull::Missing => {}
            LocalPull::Branch {
                branch,
                current,
                worktree,
                synced,
            } => {
                controls.push(local_chip(if current {
                    "Checked out".to_owned()
                } else if worktree.is_some() {
                    "In a worktree".to_owned()
                } else {
                    format!("Local · {}", branch.name)
                }));
                if current && open {
                    if branch.ahead > 0 || branch.upstream.is_none() {
                        controls.push(
                            button(
                                "inbox-push",
                                busy.map(str::to_owned).unwrap_or_else(|| {
                                    if branch.upstream.is_none() {
                                        "Publish branch".into()
                                    } else {
                                        format!("Push {}", branch.ahead)
                                    }
                                }),
                                true,
                            )
                            .tooltip(|_, cx| sidebar_tooltip("Push to update the pull request", cx))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                git(this, PullGit::Push, window, cx)
                            }))
                            .into_any_element(),
                        );
                    }
                    if branch.upstream.is_some()
                        && (branch.behind > 0 || (!synced && branch.ahead == 0))
                    {
                        controls.push(
                            button(
                                "inbox-pull",
                                busy.map(str::to_owned).unwrap_or_else(|| {
                                    if branch.behind > 0 {
                                        format!("Pull {}", branch.behind)
                                    } else {
                                        "Pull".into()
                                    }
                                }),
                                false,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                git(this, PullGit::Pull, window, cx)
                            }))
                            .into_any_element(),
                        );
                    }
                } else if current && merged && !pull.base_ref.is_empty() {
                    controls.push(
                        button(
                            "inbox-switch-base",
                            busy.map(str::to_owned)
                                .unwrap_or_else(|| format!("Switch to {} and pull", pull.base_ref)),
                            true,
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            git(this, PullGit::SwitchToBase, window, cx)
                        }))
                        .into_any_element(),
                    );
                } else if let Some(worktree) = worktree {
                    let path = worktree.path.clone();
                    let name = format!(
                        "{} {}",
                        item.repository.rsplit('/').next().unwrap_or_default(),
                        item.reference
                    );
                    controls.push(
                        button("inbox-open-worktree", "Open worktree".into(), !merged)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_folder_project(&path, &name, window, cx);
                            }))
                            .into_any_element(),
                    );
                    if merged {
                        controls.push(
                            button(
                                "inbox-remove-worktree",
                                busy.unwrap_or("Remove worktree").to_owned(),
                                false,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                git(this, PullGit::RemoveWorktree, window, cx)
                            }))
                            .into_any_element(),
                        );
                    }
                } else if open {
                    controls.push(
                        button(
                            "inbox-checkout",
                            busy.unwrap_or(if synced { "Switch" } else { "Check out" })
                                .to_owned(),
                            false,
                        )
                        .tooltip(|_, cx| {
                            sidebar_tooltip("Switch to the branch and update it (C)", cx)
                        })
                        .on_click(cx.listener({
                            let url = url.clone();
                            move |this, _, window, cx| this.checkout_pull(&url, window, cx)
                        }))
                        .into_any_element(),
                    );
                    controls.push(self.inbox_worktree_button(&url, busy, cx));
                } else if merged {
                    controls.push(
                        button(
                            "inbox-delete-local",
                            busy.unwrap_or("Delete local branch").to_owned(),
                            false,
                        )
                        .tooltip(move |_, cx| {
                            sidebar_tooltip(
                                if synced {
                                    "The branch matches the merged head"
                                } else {
                                    "Git refuses if it has unmerged commits"
                                },
                                cx,
                            )
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            git(this, PullGit::DeleteBranch, window, cx)
                        }))
                        .into_any_element(),
                    );
                }
            }
        }
        controls
    }

    fn inbox_worktree_button(
        &self,
        url: &str,
        busy: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let url = url.to_owned();
        section_button("inbox-worktree", "Open in worktree", false)
            .when(busy.is_some(), |button| button.opacity(0.5))
            .tooltip(|_, cx| {
                sidebar_tooltip(
                    "Check out in its own folder and open it as a project (W)",
                    cx,
                )
            })
            .on_click(
                cx.listener(move |this, _, window, cx| this.open_pull_worktree(&url, window, cx)),
            )
            .into_any_element()
    }

    /// The last local operation's result for this task, if it was the owner.
    pub(super) fn inbox_local_feedback(&self, url: &str) -> Option<AnyElement> {
        let (owner, text, failed) = self.work_inbox.local_feedback.as_ref()?;
        (owner == url).then(|| message(text, *failed).into_any_element())
    }
}

fn local_chip(label: String) -> AnyElement {
    div()
        .h(px(24.0))
        .px_2()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(5.0))
        .rounded(px(6.0))
        .bg(colors().selection)
        .text_size(px(11.5))
        .text_color(colors().accent)
        .child(
            svg()
                .path("chrome-icons/git-branch.svg")
                .size(px(12.0))
                .text_color(colors().accent),
        )
        .child(label)
        .into_any_element()
}
