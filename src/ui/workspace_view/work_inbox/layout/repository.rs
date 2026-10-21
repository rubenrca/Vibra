//! Review → Repository: a Git client sidebar for one project and the detail
//! of the selected change, branch, worktree, stash, or tag.

use super::*;
use crate::ports::git::{GitBranchInfo, GitCommit, GitRefAction, GitSyncOperation};
use crate::ui::theme::MONO_FONT;

const ROW_HEIGHT: f32 = 28.0;

/// Sections and whether they start folded.
const SECTIONS: [(&str, bool); 6] = [
    ("Branches", false),
    ("Remote branches", true),
    ("Worktrees", false),
    ("Stashes", false),
    ("Tags", true),
    ("Pull requests", false),
];

impl WorkspaceView {
    fn repository_section_collapsed(&self, name: &'static str) -> bool {
        let folded = SECTIONS
            .iter()
            .find(|(section, _)| *section == name)
            .is_some_and(|(_, folded)| *folded);
        folded != self.work_inbox.repository.collapsed.contains(name)
    }

    pub(super) fn repository_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self
            .repository_project()
            .and_then(|id| self.snapshot.project(id))
            .map(|project| project.name.clone())
            .unwrap_or_else(|| "Choose project".into());
        let loading = self
            .repository_root()
            .and_then(|root| self.work_inbox.repos.get(&root))
            .is_some_and(|repo| repo.loading);
        div()
            .h(px(36.0))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                div()
                    .id("repository-project")
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(28.0))
                    .px_2()
                    .rounded(px(5.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
                    .on_click(cx.listener(|this, event, _, cx| {
                        this.open_inbox_menu(InboxMenu::RepositoryProjects, event, cx)
                    }))
                    .when_some(self.repository_project(), |row, id| {
                        row.child(
                            div()
                                .size(px(12.0))
                                .flex_none()
                                .rounded(px(3.0))
                                .bg(project_color(id)),
                        )
                    })
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(name),
                    )
                    .child(
                        svg()
                            .path("chrome-icons/chevron-down.svg")
                            .size(px(12.0))
                            .flex_none()
                            .text_color(colors().subtle),
                    ),
            )
            .child(
                icon_button("repository-fetch", "chrome-icons/cloud.svg", "Fetch").on_click(
                    cx.listener(|this, _, window, cx| {
                        this.sync_repository(GitSyncOperation::Fetch, window, cx)
                    }),
                ),
            )
            .child(
                icon_button(
                    "repository-refresh",
                    "chrome-icons/refresh.svg",
                    if loading {
                        "Refreshing…"
                    } else {
                        "Refresh (R)"
                    },
                )
                .when(loading, |button| button.opacity(0.4))
                .on_click(cx.listener(|this, _, _, cx| this.refresh_inbox(cx))),
            )
    }

    pub(super) fn repository_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div()
            .id("repository-tree")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p(px(6.0))
            .flex()
            .flex_col()
            .gap(px(1.0));
        let Some(root) = self.repository_root() else {
            return list.child(message("Add a project with a Git repository.", false));
        };
        let repo = self.work_inbox.repos.get(&root);
        if let Some(error) = repo.and_then(|repo| repo.error.as_ref()) {
            list = list.child(message(error, true));
        }
        let Some(refs) = repo.and_then(|repo| repo.refs.clone()) else {
            return list.child(message(
                if repo.is_some_and(|repo| repo.fetched.is_some() && !repo.loading) {
                    "No Git repository in this project."
                } else {
                    "Reading repository…"
                },
                false,
            ));
        };
        let selection = &self.work_inbox.repository.selection;
        list =
            list.child(
                self.repository_row(
                    "repository-changes".into(),
                    "chrome-icons/diff-unified.svg",
                    "Local changes".into(),
                    Some(format!("on {}", refs.current)),
                    (refs.changes > 0).then(|| refs.changes.to_string()),
                    *selection == RepoSelection::Changes,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.select_repository_item(RepoSelection::Changes, cx)
                })),
            );

        let pulls: Vec<&WorkItem> = self
            .work_inbox
            .github
            .items
            .iter()
            .filter(|item| {
                item.pull.is_some()
                    && item.project_id == self.repository_project()
                    && matches!(item.status, WorkStatus::Open | WorkStatus::Draft)
            })
            .collect();
        for (section, _) in SECTIONS {
            let count = match section {
                "Branches" => refs.branches.len(),
                "Remote branches" => refs.remotes.len(),
                "Worktrees" => refs.worktrees.len(),
                "Stashes" => refs.stashes.len(),
                "Tags" => refs.tags.len(),
                _ => pulls.len(),
            };
            if count == 0 && matches!(section, "Stashes" | "Tags" | "Pull requests") {
                continue;
            }
            if section == "Worktrees" && count < 2 {
                continue;
            }
            let collapsed = self.repository_section_collapsed(section);
            list = list.child(
                repository_section_header(section, count, collapsed).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let collapsed = &mut this.work_inbox.repository.collapsed;
                        if !collapsed.remove(section) {
                            collapsed.insert(section);
                        }
                        cx.notify();
                    },
                )),
            );
            if collapsed {
                continue;
            }
            match section {
                "Branches" => {
                    for branch in &refs.branches {
                        let name = branch.name.clone();
                        let pull = self.branch_pull(&branch.name);
                        list = list.child(
                            self.repository_row(
                                SharedString::from(format!("repository-branch-{name}")),
                                if branch.current {
                                    "chrome-icons/check.svg"
                                } else {
                                    "chrome-icons/git-branch.svg"
                                },
                                branch.name.clone(),
                                pull.map(|pull| pull.reference.clone()),
                                tracking_label(branch),
                                *selection == RepoSelection::Branch(name.clone()),
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.select_repository_item(
                                        RepoSelection::Branch(name.clone()),
                                        cx,
                                    )
                                },
                            )),
                        );
                    }
                }
                "Remote branches" => {
                    for branch in &refs.remotes {
                        let name = branch.name.clone();
                        list = list.child(
                            self.repository_row(
                                SharedString::from(format!("repository-remote-{name}")),
                                "chrome-icons/cloud.svg",
                                branch.name.clone(),
                                None,
                                None,
                                *selection == RepoSelection::Remote(name.clone()),
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.select_repository_item(
                                        RepoSelection::Remote(name.clone()),
                                        cx,
                                    )
                                },
                            )),
                        );
                    }
                }
                "Worktrees" => {
                    for worktree in &refs.worktrees {
                        let path = worktree.path.clone();
                        list = list.child(
                            self.repository_row(
                                SharedString::from(format!(
                                    "repository-worktree-{}",
                                    path.display()
                                )),
                                "chrome-icons/worktree.svg",
                                path.file_name()
                                    .map(|name| name.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| path.display().to_string()),
                                Some(worktree.branch.clone().unwrap_or_else(|| "detached".into())),
                                worktree.main.then(|| "main".to_owned()),
                                *selection == RepoSelection::Worktree(path.clone()),
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.select_repository_item(
                                        RepoSelection::Worktree(path.clone()),
                                        cx,
                                    )
                                },
                            )),
                        );
                    }
                }
                "Stashes" => {
                    for stash in &refs.stashes {
                        let oid = stash.oid.clone();
                        list = list.child(
                            self.repository_row(
                                SharedString::from(format!("repository-stash-{oid}")),
                                "chrome-icons/archive.svg",
                                stash.subject.clone(),
                                None,
                                Some(format!("{{{}}}", stash.index)),
                                *selection == RepoSelection::Stash(oid.clone()),
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.select_repository_item(
                                        RepoSelection::Stash(oid.clone()),
                                        cx,
                                    )
                                },
                            )),
                        );
                    }
                }
                "Tags" => {
                    for tag in &refs.tags {
                        let name = tag.name.clone();
                        list = list.child(
                            self.repository_row(
                                SharedString::from(format!("repository-tag-{name}")),
                                "chrome-icons/tag.svg",
                                tag.name.clone(),
                                None,
                                None,
                                *selection == RepoSelection::Tag(name.clone()),
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.select_repository_item(
                                        RepoSelection::Tag(name.clone()),
                                        cx,
                                    )
                                },
                            )),
                        );
                    }
                }
                _ => {
                    for item in &pulls {
                        let url = item.url.clone();
                        list = list.child(
                            self.repository_row(
                                SharedString::from(format!("repository-pull-{url}")),
                                "chrome-icons/git-pull-request.svg",
                                item.title.clone(),
                                None,
                                Some(item.reference.clone()),
                                false,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.open_branch_pull(url.clone(), cx),
                            )),
                        );
                    }
                }
            }
        }
        list
    }

    #[allow(clippy::too_many_arguments)]
    fn repository_row(
        &self,
        id: SharedString,
        icon: &'static str,
        label: String,
        detail: Option<String>,
        badge: Option<String>,
        selected: bool,
        _cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .h(px(ROW_HEIGHT))
            .px(px(8.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(7.0))
            .rounded(px(5.0))
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(surface_tint(colors().selection, colors().background))
            })
            .hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
            .child(
                svg()
                    .path(icon)
                    .size(px(13.0))
                    .flex_none()
                    .text_color(if selected {
                        colors().foreground
                    } else {
                        colors().subtle
                    }),
            )
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
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(colors().foreground)
                            .child(label),
                    )
                    .when_some(detail, |row, detail| {
                        row.child(
                            div()
                                .min_w(px(0.0))
                                .flex_shrink()
                                .truncate()
                                .text_size(px(11.0))
                                .text_color(colors().subtle)
                                .child(detail),
                        )
                    }),
            )
            .when_some(badge, |row, badge| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(10.5))
                        .text_color(colors().muted)
                        .child(badge),
                )
            })
    }

    pub(super) fn repository_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(refs) = self.repository_refs() else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.0))
                .text_color(colors().subtle)
                .child("Select a project with a Git repository.")
                .into_any_element();
        };
        let selection = self.work_inbox.repository.selection.clone();
        let review = self.work_inbox.repository.review.clone();
        let mut panel = div().flex_1().min_w(px(0.0)).h_full().flex().flex_col();
        match selection {
            RepoSelection::Changes => {
                panel = panel.child(self.repository_changes_header(refs.current.clone(), cx));
                panel = panel.child(self.repository_feedback(cx));
                if let (Some(index), Some(review)) =
                    (self.work_inbox.repository.index.clone(), review)
                {
                    panel = panel.child(
                        div()
                            .flex_1()
                            .min_h(px(0.0))
                            .flex()
                            .child(
                                div()
                                    .w(px(300.0))
                                    .flex_none()
                                    .h_full()
                                    .border_r_1()
                                    .border_color(colors().border_subtle)
                                    .bg(surface(colors().sidebar))
                                    .child(index),
                            )
                            .child(div().flex_1().min_w(px(0.0)).h_full().child(review)),
                    );
                }
            }
            RepoSelection::Branch(name) | RepoSelection::Remote(name) => {
                let remote = refs.branch(&name).is_none();
                let Some(branch) = refs
                    .branch(&name)
                    .or_else(|| refs.remotes.iter().find(|branch| branch.name == name))
                else {
                    return panel.into_any_element();
                };
                panel = panel
                    .child(self.repository_branch_header(branch, remote, cx))
                    .child(self.repository_feedback(cx))
                    .child(self.repository_branch_body(&name, cx));
            }
            RepoSelection::Worktree(path) => {
                let Some(worktree) = refs.worktrees.iter().find(|tree| tree.path == path) else {
                    return panel.into_any_element();
                };
                let mut actions = vec![
                    section_button("repository-open-worktree", "Open as project", true)
                        .on_click(cx.listener({
                            let path = path.clone();
                            move |this, _, window, cx| {
                                let name = path
                                    .file_name()
                                    .map(|name| name.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                this.open_folder_project(&path, &name, window, cx);
                            }
                        }))
                        .into_any_element(),
                ];
                if !worktree.main {
                    actions.push(
                        section_button("repository-remove-worktree", "Remove worktree", false)
                            .on_click(cx.listener({
                                let path = path.clone();
                                move |this, _, window, cx| {
                                    this.request_repository_action(
                                        GitRefAction::RemoveWorktree(path.clone()),
                                        window,
                                        cx,
                                    )
                                }
                            }))
                            .into_any_element(),
                    );
                }
                panel = panel
                    .child(repository_header(
                        "chrome-icons/worktree.svg",
                        path.display().to_string(),
                        vec![
                            worktree
                                .branch
                                .clone()
                                .unwrap_or_else(|| "Detached HEAD".into()),
                            worktree.head.chars().take(7).collect(),
                            if worktree.main {
                                "Main worktree".into()
                            } else if worktree.prunable {
                                "Missing on disk".into()
                            } else {
                                "Linked worktree".into()
                            },
                        ],
                        actions,
                    ))
                    .child(self.repository_feedback(cx));
            }
            RepoSelection::Stash(oid) => {
                let Some(stash) = refs.stashes.iter().find(|stash| stash.oid == oid) else {
                    return panel.into_any_element();
                };
                let index = stash.index;
                let actions = [
                    (
                        "repository-stash-apply",
                        "Apply",
                        GitRefAction::StashApply(index),
                        true,
                    ),
                    (
                        "repository-stash-pop",
                        "Pop",
                        GitRefAction::StashPop(index),
                        false,
                    ),
                    (
                        "repository-stash-drop",
                        "Drop",
                        GitRefAction::StashDrop(index),
                        false,
                    ),
                ]
                .into_iter()
                .map(|(id, label, action, primary)| {
                    section_button(id, label, primary)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.request_repository_action(action.clone(), window, cx)
                        }))
                        .into_any_element()
                })
                .collect();
                panel = panel
                    .child(repository_header(
                        "chrome-icons/archive.svg",
                        stash.subject.clone(),
                        vec![format!("stash@{{{index}}}"), stash.date.clone()],
                        actions,
                    ))
                    .child(self.repository_feedback(cx))
                    .when_some(review, |panel, review| {
                        panel.child(div().flex_1().min_h(px(0.0)).child(review))
                    });
            }
            RepoSelection::Tag(name) => {
                let Some(tag) = refs.tags.iter().find(|tag| tag.name == name) else {
                    return panel.into_any_element();
                };
                panel = panel
                    .child(repository_header(
                        "chrome-icons/tag.svg",
                        tag.name.clone(),
                        [
                            tag.subject.clone(),
                            tag.date.clone(),
                            tag.target.chars().take(7).collect(),
                        ]
                        .into_iter()
                        .filter(|part: &String| !part.is_empty())
                        .collect(),
                        Vec::new(),
                    ))
                    .when_some(review, |panel, review| {
                        panel.child(div().flex_1().min_h(px(0.0)).child(review))
                    });
            }
        }
        panel.into_any_element()
    }

    fn repository_changes_header(
        &self,
        current: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let editing = self.work_inbox.repository.stash_editing;
        let message_text = self.work_inbox.repository.stash_message.clone();
        div()
            .h(px(44.0))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(colors().border_subtle)
            .child(
                svg()
                    .path("chrome-icons/git-branch.svg")
                    .size(px(14.0))
                    .text_color(colors().subtle),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(current),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("repository-stash-message")
                    .w(px(220.0))
                    .h(px(28.0))
                    .px_2()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(if editing {
                        colors().accent
                    } else {
                        colors().border_subtle
                    })
                    .flex()
                    .items_center()
                    .cursor_text()
                    .text_size(px(12.0))
                    .text_color(if message_text.is_empty() {
                        colors().subtle
                    } else {
                        colors().foreground
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.work_inbox.repository.stash_editing = true;
                        this.work_inbox.search_editing = false;
                        this.focus_handle.focus(window);
                        cx.notify();
                    }))
                    .child(div().truncate().child(if message_text.is_empty() {
                        "Stash message (optional)".to_owned()
                    } else {
                        message_text
                    })),
            )
            .child(
                section_button("repository-stash", "Stash changes", false).on_click(
                    cx.listener(|this, _, window, cx| this.stash_repository_changes(window, cx)),
                ),
            )
    }

    pub(in crate::ui::workspace_view) fn stash_repository_changes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = std::mem::take(&mut self.work_inbox.repository.stash_message);
        self.work_inbox.repository.stash_editing = false;
        self.request_repository_action(GitRefAction::Stash { message }, window, cx);
    }

    fn repository_branch_header(
        &self,
        branch: &GitBranchInfo,
        remote: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let refs = self.repository_refs();
        let name = branch.name.clone();
        let worktree = refs
            .and_then(|refs| refs.worktree_for(&branch.name))
            .map(|tree| tree.path.clone());
        let mut facts = Vec::new();
        if branch.current {
            facts.push("Current branch".to_owned());
        }
        if let Some(upstream) = &branch.upstream {
            facts.push(format!("Tracks {upstream}"));
        }
        if branch.gone {
            facts.push("Upstream deleted".to_owned());
        }
        if branch.ahead > 0 || branch.behind > 0 {
            facts.push(format!("↑{} ↓{}", branch.ahead, branch.behind));
        }
        if !branch.subject.is_empty() {
            facts.push(format!("{} · {}", branch.subject, branch.date));
        }
        let mut actions: Vec<AnyElement> = Vec::new();
        if remote {
            let local = branch
                .name
                .split_once('/')
                .map(|(_, local)| local.to_owned())
                .unwrap_or_else(|| branch.name.clone());
            let exists = refs.is_some_and(|refs| refs.branch(&local).is_some());
            actions.push(
                section_button(
                    "repository-checkout-remote",
                    if exists {
                        format!("Switch to {local}")
                    } else {
                        format!("Check out as {local}")
                    },
                    true,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.request_repository_action(GitRefAction::Switch(local.clone()), window, cx)
                }))
                .into_any_element(),
            );
        } else if branch.current {
            actions.push(
                section_button(
                    "repository-pull",
                    if branch.behind > 0 {
                        format!("Pull {}", branch.behind)
                    } else {
                        "Pull".to_owned()
                    },
                    branch.behind > 0,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.sync_repository(GitSyncOperation::Pull, window, cx)
                }))
                .into_any_element(),
            );
            actions.push(
                section_button(
                    "repository-push",
                    if branch.upstream.is_none() {
                        "Publish branch".to_owned()
                    } else if branch.ahead > 0 {
                        format!("Push {}", branch.ahead)
                    } else {
                        "Push".to_owned()
                    },
                    branch.ahead > 0 || branch.upstream.is_none(),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.sync_repository(GitSyncOperation::Push, window, cx)
                }))
                .into_any_element(),
            );
        } else if let Some(path) = worktree {
            actions.push(
                section_button("repository-branch-worktree", "Open worktree", true)
                    .on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, window, cx| {
                            this.open_folder_project(&path, &name, window, cx);
                        }
                    }))
                    .into_any_element(),
            );
        } else {
            actions.push(
                section_button("repository-switch", "Switch", true)
                    .on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, window, cx| {
                            this.request_repository_action(
                                GitRefAction::Switch(name.clone()),
                                window,
                                cx,
                            )
                        }
                    }))
                    .into_any_element(),
            );
        }
        if !remote {
            if let Some(pull) = self.branch_pull(&branch.name) {
                let url = pull.url.clone();
                actions.push(
                    section_button(
                        "repository-open-pull",
                        format!("Pull request {}", pull.reference),
                        false,
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_branch_pull(url.clone(), cx)),
                    )
                    .into_any_element(),
                );
            } else if !matches!(branch.name.as_str(), "main" | "master") {
                let command = format!(
                    "gh pr create --head {}",
                    crate::domain::work_items::shell_argument(&branch.name)
                );
                actions.push(
                    section_button("repository-create-pull", "Create pull request", false)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(project) = this.repository_project()
                                && let Err(reason) =
                                    this.run_in_new_tab(project, "Pull request", &command, true, cx)
                            {
                                this.persistence_error = Some(reason.into());
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
            if !branch.current {
                actions.push(
                    section_button("repository-delete-branch", "Delete", false)
                        .on_click(cx.listener({
                            let name = name.clone();
                            move |this, _, window, cx| {
                                this.request_repository_action(
                                    GitRefAction::DeleteBranch {
                                        name: name.clone(),
                                        force: false,
                                    },
                                    window,
                                    cx,
                                )
                            }
                        }))
                        .into_any_element(),
                );
            }
        }
        repository_header(
            if remote {
                "chrome-icons/cloud.svg"
            } else {
                "chrome-icons/git-branch.svg"
            },
            branch.name.clone(),
            facts,
            actions,
        )
    }

    /// The branch's own commits; a click shows one in the review below.
    fn repository_branch_body(&self, branch: &str, cx: &mut Context<Self>) -> AnyElement {
        let repository = &self.work_inbox.repository;
        if repository.commit.is_some()
            && let Some(review) = repository.review.clone()
        {
            return div()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .child(div().px_3().py_1().child(
                    quiet_button("repository-commits-back", "← Commits").on_click(
                        cx.listener(|this, _, _, cx| this.show_repository_commit(None, cx)),
                    ),
                ))
                .child(div().flex_1().min_h(px(0.0)).child(review))
                .into_any_element();
        }
        let mut body = div()
            .id("repository-branch-commits")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .flex()
            .flex_col();
        if let Some(error) = &repository.log.error {
            body = body.child(message(error, true));
        }
        let Some((name, commits)) = repository
            .log
            .data
            .as_ref()
            .filter(|(name, _)| name == branch)
        else {
            return body
                .child(message("Loading commits…", false))
                .into_any_element();
        };
        let base = self.branch_base(name);
        body = body.child(
            div()
                .pb_2()
                .text_size(px(12.0))
                .text_color(colors().muted)
                .child(if base.is_empty() {
                    "Latest commits".to_owned()
                } else if commits.is_empty() {
                    format!("No commits that {base} lacks")
                } else {
                    format!(
                        "{} commit{} not in {base}",
                        commits.len(),
                        if commits.len() == 1 { "" } else { "s" }
                    )
                }),
        );
        for commit in commits {
            body = body.child(self.repository_commit_row(commit, cx));
        }
        body.into_any_element()
    }

    fn repository_commit_row(&self, commit: &GitCommit, cx: &mut Context<Self>) -> AnyElement {
        let target = commit.clone();
        div()
            .id(SharedString::from(format!(
                "repository-commit-{}",
                commit.sha
            )))
            .h(px(32.0))
            .px_2()
            .flex()
            .items_center()
            .gap_3()
            .rounded(px(5.0))
            .cursor_pointer()
            .hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.show_repository_commit(Some(target.clone()), cx)
            }))
            .child(
                svg()
                    .path("chrome-icons/git-commit.svg")
                    .size(px(13.0))
                    .flex_none()
                    .text_color(colors().subtle),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(12.5))
                    .child(commit.subject.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(colors().subtle)
                    .child(format!("{} · {}", commit.author, commit.date)),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(MONO_FONT)
                    .text_size(px(11.0))
                    .text_color(colors().muted)
                    .child(commit.short_sha.clone()),
            )
            .into_any_element()
    }

    /// Busy state, the last result, or a pending confirmation.
    fn repository_feedback(&self, cx: &mut Context<Self>) -> AnyElement {
        let key = super::super::repository::repository_key();
        let mut row = div().flex_none().px_3().flex().flex_col().gap_2();
        if let Some(label) = self.local_job_label(&key) {
            row = row.pt_2().child(message(label, false));
        } else if let Some((owner, text, failed)) = &self.work_inbox.local_feedback
            && *owner == key
        {
            row = row.pt_2().child(message(text, *failed));
        }
        if let Some(confirmation) = &self.work_inbox.repository.confirmation {
            let action = confirmation.action.clone();
            let soft_delete = matches!(&action, GitRefAction::DeleteBranch { force: false, .. });
            row = row.pt_2().child(
                div()
                    .p_3()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(colors().border_subtle)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child(confirmation.message.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                section_button("repository-confirm", "Confirm", true).on_click(
                                    cx.listener({
                                        let action = action.clone();
                                        move |this, _, window, cx| {
                                            this.request_repository_action(
                                                action.clone(),
                                                window,
                                                cx,
                                            )
                                        }
                                    }),
                                ),
                            )
                            .when(soft_delete, |row| {
                                let GitRefAction::DeleteBranch { name, .. } = action.clone() else {
                                    return row;
                                };
                                row.child(
                                    section_button(
                                        "repository-force-delete",
                                        "Delete even if unmerged",
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.request_repository_action(
                                                GitRefAction::DeleteBranch {
                                                    name: name.clone(),
                                                    force: true,
                                                },
                                                window,
                                                cx,
                                            )
                                        },
                                    )),
                                )
                            })
                            .child(quiet_button("repository-cancel", "Cancel").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.work_inbox.repository.confirmation = None;
                                    cx.notify();
                                }),
                            )),
                    ),
            );
        }
        row.into_any_element()
    }
}

fn repository_section_header(
    name: &'static str,
    count: usize,
    collapsed: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("repository-section-{name}")))
        .h(px(26.0))
        .mt(px(6.0))
        .px(px(6.0))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.0))
        .rounded(px(5.0))
        .cursor_pointer()
        .hover(|row| row.bg(surface_tint(colors().hover, colors().background)))
        .child(
            svg()
                .path(if collapsed {
                    "chrome-icons/chevron-right.svg"
                } else {
                    "chrome-icons/chevron-down.svg"
                })
                .size(px(12.0))
                .text_color(colors().subtle),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(10.5))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(colors().muted)
                .child(name.to_uppercase()),
        )
        .child(
            div()
                .text_size(px(10.5))
                .text_color(colors().subtle)
                .child(count.to_string()),
        )
}

/// Title, facts, and actions above a repository detail.
fn repository_header(
    icon: &'static str,
    title: String,
    facts: Vec<String>,
    actions: Vec<AnyElement>,
) -> impl IntoElement {
    div()
        .flex_none()
        .px(px(24.0))
        .pt(px(18.0))
        .pb(px(14.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .border_b_1()
        .border_color(colors().border_subtle)
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(svg().path(icon).size(px(16.0)).text_color(colors().subtle))
                .child(
                    div()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(18.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                ),
        )
        .when(!facts.is_empty(), |header| {
            header.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .text_size(px(12.0))
                    .text_color(colors().subtle)
                    .children(facts.into_iter().enumerate().flat_map(|(index, fact)| {
                        (index > 0)
                            .then(|| div().child("·"))
                            .into_iter()
                            .chain(std::iter::once(div().child(fact)))
                    })),
            )
        })
        .when(!actions.is_empty(), |header| {
            header.child(div().flex().flex_wrap().gap_2().children(actions))
        })
}

/// `↑2 ↓1`, `gone`, or nothing for a branch row.
fn tracking_label(branch: &GitBranchInfo) -> Option<String> {
    if branch.gone {
        return Some("gone".into());
    }
    match (branch.ahead, branch.behind) {
        (0, 0) => None,
        (ahead, 0) => Some(format!("↑{ahead}")),
        (0, behind) => Some(format!("↓{behind}")),
        (ahead, behind) => Some(format!("↑{ahead} ↓{behind}")),
    }
}
