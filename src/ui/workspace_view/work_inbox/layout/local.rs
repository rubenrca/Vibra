//! A pull request's branch: where it is checked out, and a menu with the Git
//! actions that bring it to (or clean it from) the project's repository.

use super::*;
use crate::ui::theme::MONO_FONT;
use crate::ui::workspace_view::work_inbox::local::{LocalPull, PullGit};

/// One entry of the branch menu.
#[derive(Clone)]
enum BranchAction {
    Checkout,
    NewWorktree,
    OpenWorktree(std::path::PathBuf),
    Git(PullGit),
}

impl WorkspaceView {
    /// The head branch with its local state; a click opens the branch menu.
    pub(super) fn inbox_branch_pill(&self, item: &WorkItem, cx: &mut Context<Self>) -> AnyElement {
        let Some(pull) = item.pull.as_ref() else {
            return div().into_any_element();
        };
        let (root, local) = self.local_pull(item);
        let busy = self.local_job_label(&item.url);
        let state: Option<(String, Rgba)> = match (busy, local) {
            (Some(label), _) => Some((label.to_owned(), colors().muted)),
            (
                None,
                LocalPull::Branch {
                    branch,
                    current: true,
                    ..
                },
            ) => {
                let mut label = "Checked out".to_owned();
                if branch.upstream.is_none() {
                    label.push_str(" · not pushed");
                } else if branch.ahead > 0 || branch.behind > 0 {
                    label.push_str(&format!(" · ↑{} ↓{}", branch.ahead, branch.behind));
                }
                Some((label, colors().accent))
            }
            (
                None,
                LocalPull::Branch {
                    worktree: Some(_), ..
                },
            ) => Some(("Worktree".to_owned(), colors().accent)),
            (None, LocalPull::Branch { .. }) => Some(("Local".to_owned(), colors().muted)),
            _ => None,
        };
        let has_menu = root.is_some() || !pull.head_ref.is_empty();
        div()
            .id("inbox-branch")
            .flex_none()
            .max_w(px(360.0))
            .h(px(22.0))
            .px(px(7.0))
            .rounded(px(5.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .bg(surface_tint(colors().elevated, colors().background))
            .when(has_menu, |pill| {
                pill.cursor_pointer()
                    .hover(|pill| pill.bg(surface_tint(colors().hover, colors().background)))
                    .tooltip(|_, cx| sidebar_tooltip("Branch actions", cx))
                    .on_click(cx.listener(|this, event, _, cx| {
                        this.open_inbox_menu(InboxMenu::Branch, event, cx)
                    }))
            })
            .child(
                svg()
                    .path("chrome-icons/git-branch.svg")
                    .size(px(11.0))
                    .flex_none()
                    .text_color(colors().subtle),
            )
            .child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .text_color(colors().foreground)
                    .child(pull.head_ref.clone()),
            )
            .when_some(state, |pill, (label, color)| {
                pill.child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(color)
                        .child(label),
                )
            })
            .when(has_menu, |pill| {
                pill.child(
                    svg()
                        .path("chrome-icons/chevron-down.svg")
                        .size(px(11.0))
                        .flex_none()
                        .text_color(colors().subtle),
                )
            })
            .into_any_element()
    }

    /// Local actions for the pull request's branch, then copying its name.
    pub(super) fn inbox_branch_menu(
        &self,
        item: &WorkItem,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        let busy = self.local_job_label(&item.url).is_some();
        for (index, (label, icon, shortcut, danger, action)) in
            self.branch_actions(item).into_iter().enumerate()
        {
            let url = item.url.clone();
            let name = format!(
                "{} {}",
                item.repository.rsplit('/').next().unwrap_or_default(),
                item.reference
            );
            let mut row = MenuRow::new(label).icon(icon);
            if let Some(shortcut) = shortcut {
                row = row.shortcut(shortcut);
            }
            if danger {
                row = row.danger();
            }
            rows.push(
                row.render(SharedString::from(format!("inbox-branch-action-{index}")))
                    .when(busy, |row| row.opacity(0.5))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.work_inbox.menu = None;
                        match action.clone() {
                            BranchAction::Checkout => this.checkout_pull(&url, window, cx),
                            BranchAction::NewWorktree => this.open_pull_worktree(&url, window, cx),
                            BranchAction::OpenWorktree(path) => {
                                this.open_folder_project(&path, &name, window, cx);
                            }
                            BranchAction::Git(git) => this.run_pull_git(&url, git, window, cx),
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        if let Some(branch) = item
            .pull
            .as_ref()
            .map(|pull| pull.head_ref.clone())
            .filter(|branch| !branch.is_empty())
        {
            if !rows.is_empty() {
                rows.push(menu_separator().into_any_element());
            }
            rows.push(
                MenuRow::new("Copy branch name")
                    .icon("chrome-icons/copy.svg")
                    .render("inbox-copy-branch")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(branch.clone()));
                        this.work_inbox.menu = None;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        rows
    }

    /// What the branch can do from where it stands locally.
    #[allow(clippy::type_complexity)]
    fn branch_actions(
        &self,
        item: &WorkItem,
    ) -> Vec<(
        String,
        &'static str,
        Option<&'static str>,
        bool,
        BranchAction,
    )> {
        let Some(pull) = item.pull.as_ref() else {
            return Vec::new();
        };
        let (root, local) = self.local_pull(item);
        if root.is_none() {
            return Vec::new();
        }
        let open = matches!(item.status, WorkStatus::Open | WorkStatus::Draft);
        let merged = matches!(item.status, WorkStatus::Merged | WorkStatus::Closed);
        let checkout = |switch: bool| {
            (
                if switch {
                    "Switch to branch"
                } else {
                    "Check out"
                }
                .to_owned(),
                "chrome-icons/git-branch.svg",
                Some("C"),
                false,
                BranchAction::Checkout,
            )
        };
        let new_worktree = || {
            (
                "Open in worktree".to_owned(),
                "chrome-icons/worktree.svg",
                Some("W"),
                false,
                BranchAction::NewWorktree,
            )
        };
        let mut actions = Vec::new();
        match local {
            LocalPull::Unknown => {}
            LocalPull::Missing if open => {
                actions.push(checkout(false));
                actions.push(new_worktree());
            }
            LocalPull::Missing => {}
            LocalPull::Branch {
                branch,
                current,
                worktree,
                synced,
            } => {
                if current && open {
                    if branch.ahead > 0 || branch.upstream.is_none() {
                        actions.push((
                            if branch.upstream.is_none() {
                                "Publish branch".to_owned()
                            } else {
                                format!("Push {} commit{}", branch.ahead, plural(branch.ahead))
                            },
                            "chrome-icons/chevron-up.svg",
                            None,
                            false,
                            BranchAction::Git(PullGit::Push),
                        ));
                    }
                    if branch.upstream.is_some()
                        && (branch.behind > 0 || (!synced && branch.ahead == 0))
                    {
                        actions.push((
                            if branch.behind > 0 {
                                format!("Pull {} commit{}", branch.behind, plural(branch.behind))
                            } else {
                                "Pull".to_owned()
                            },
                            "chrome-icons/chevron-down.svg",
                            None,
                            false,
                            BranchAction::Git(PullGit::Pull),
                        ));
                    }
                } else if current && merged && !pull.base_ref.is_empty() {
                    actions.push((
                        format!("Switch to {} and pull", pull.base_ref),
                        "chrome-icons/git-branch.svg",
                        None,
                        false,
                        BranchAction::Git(PullGit::SwitchToBase),
                    ));
                } else if let Some(worktree) = worktree {
                    actions.push((
                        "Open worktree".to_owned(),
                        "chrome-icons/folder-open.svg",
                        None,
                        false,
                        BranchAction::OpenWorktree(worktree.path.clone()),
                    ));
                    if merged {
                        actions.push((
                            "Remove worktree".to_owned(),
                            "chrome-icons/trash.svg",
                            None,
                            true,
                            BranchAction::Git(PullGit::RemoveWorktree),
                        ));
                    }
                } else if open {
                    actions.push(checkout(synced));
                    actions.push(new_worktree());
                } else if merged {
                    actions.push((
                        "Delete local branch".to_owned(),
                        "chrome-icons/trash.svg",
                        None,
                        true,
                        BranchAction::Git(PullGit::DeleteBranch),
                    ));
                }
            }
        }
        actions
    }

    /// The last local operation's result for this task, if it was the owner.
    pub(super) fn inbox_local_feedback(&self, url: &str) -> Option<AnyElement> {
        let (owner, text, failed) = self.work_inbox.local_feedback.as_ref()?;
        (owner == url).then(|| {
            div()
                .text_size(px(12.0))
                .text_color(if *failed {
                    colors().warning
                } else {
                    colors().muted
                })
                .child(text.clone())
                .into_any_element()
        })
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}
