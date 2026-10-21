//! Review → Repository: one project's branches, worktrees, stashes, and tags,
//! with the working tree and saved commits in an embedded review.

use std::path::PathBuf;

use super::*;
use crate::ports::git::{GitCommit, GitRefAction, GitRepositoryRefs, GitSyncOperation};
use crate::ui::diff_view::{DiffFileIndexView, DiffView, DiffViewEvent};

const BRANCH_LOG_LIMIT: usize = 100;

/// Bases a branch's own commits are counted against, in order of preference.
const BASE_CANDIDATES: [&str; 4] = ["origin/main", "origin/master", "main", "master"];

impl WorkspaceView {
    /// The project whose repository the view shows: the user's pick, else the
    /// active project.
    pub(super) fn repository_project(&self) -> Option<Uuid> {
        let usable = |id: &Uuid| {
            self.snapshot
                .listed_projects()
                .any(|project| project.id == *id && project.directory().is_some())
        };
        self.work_inbox
            .repository
            .project
            .filter(usable)
            .or_else(|| self.snapshot.selected_project_id.filter(usable))
            .or_else(|| {
                self.snapshot
                    .listed_projects()
                    .find(|project| project.directory().is_some())
                    .map(|project| project.id)
            })
    }

    pub(super) fn repository_root(&self) -> Option<PathBuf> {
        self.repository_project()
            .and_then(|id| self.project_folder(id))
    }

    pub(super) fn repository_refs(&self) -> Option<&GitRepositoryRefs> {
        self.repository_root()
            .and_then(|root| self.repo_refs(&root))
    }

    pub(crate) fn show_repository_view(&mut self, show: bool, cx: &mut Context<Self>) {
        self.work_inbox.activity = false;
        self.work_inbox.menu = None;
        self.work_inbox.search_editing = false;
        if self.settings.inbox.repository_view != show {
            self.settings.inbox.repository_view = show;
            self.persist_settings(cx);
        }
        if show {
            self.open_repository(cx);
        } else {
            self.ensure_inbox_loaded(cx);
            self.sync_inbox_selection(cx);
        }
        self.sync_repository_polling(cx);
        self.sync_terminal_surface_visibility(cx);
        cx.notify();
    }

    /// Point the embedded review at the selected project and reload its refs.
    pub(in crate::ui::workspace_view) fn open_repository(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.repository_root() else {
            return;
        };
        self.ensure_repository_review(cx);
        if let Some(review) = self.work_inbox.repository.review.clone() {
            review.update(cx, |view, cx| {
                view.set_root(root.clone(), cx);
                view.set_embedded(cx);
            });
        }
        self.refresh_local_repo(root, true, cx);
        self.apply_repository_selection(cx);
    }

    pub(super) fn select_repository_project(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.work_inbox.repository.project = Some(id);
        self.work_inbox.repository.selection = RepoSelection::Changes;
        self.work_inbox.repository.confirmation = None;
        self.work_inbox.menu = None;
        self.open_repository(cx);
        cx.notify();
    }

    fn ensure_repository_review(&mut self, cx: &mut Context<Self>) {
        if self.work_inbox.repository.review.is_some() {
            return;
        }
        let root = self.repository_root().unwrap_or_default();
        let review = cx.new(|cx| {
            let mut view = DiffView::new(root, self.git_port.clone(), cx);
            view.set_preferences(
                self.settings.diff_split,
                self.settings.diff_wrap,
                self.settings.diff_font_size,
                cx,
            );
            view.set_embedded(cx);
            view
        });
        let index = cx.new(|cx| DiffFileIndexView::new(review.clone(), cx));
        let subscription = cx.subscribe(&review, |this, _, event: &DiffViewEvent, cx| {
            match event {
                DiffViewEvent::PreferencesChanged { split, wrap } => {
                    this.settings.diff_split = *split;
                    this.settings.diff_wrap = *wrap;
                    this.diff_view.update(cx, |diff, cx| {
                        diff.set_preferences(*split, *wrap, this.settings.diff_font_size, cx)
                    });
                    this.sync_inbox_review_preferences(cx);
                    this.persist_settings(cx);
                }
                DiffViewEvent::RunInTerminal { title, command } => {
                    if let Some(project) = this.repository_project()
                        && let Err(reason) = this.run_in_new_tab(project, title, command, true, cx)
                    {
                        this.persistence_error = Some(reason.into());
                    }
                }
                DiffViewEvent::SendReview {
                    prompt,
                    delivery_id,
                } => {
                    // Same destination as the workspace review: the agent in the
                    // selected terminal, which comes forward with the pasted text.
                    let status = this.send_review_to_agent(prompt, *delivery_id, cx);
                    let accepted = status != crate::ui::terminal::TerminalInsertStatus::Rejected;
                    let delivery_id = *delivery_id;
                    if let Some(review) = this.work_inbox.repository.review.clone() {
                        review.update(cx, |view, cx| {
                            view.resolve_review_delivery(delivery_id, accepted, cx)
                        });
                    }
                    if accepted {
                        this.prepare_terminal_tab(cx);
                    }
                }
                _ => {}
            }
            cx.notify();
        });
        self.work_inbox.repository.review = Some(review);
        self.work_inbox.repository.index = Some(index);
        self.work_inbox.repository._review_subscription = Some(subscription);
    }

    /// Poll the working tree only while its changes are on screen.
    pub(in crate::ui::workspace_view) fn sync_repository_polling(&self, cx: &mut Context<Self>) {
        let visible = self.workspace_section == WorkspaceSection::Inbox
            && !self.work_inbox.activity
            && self.settings.inbox.repository_view
            && self.work_inbox.repository.selection == RepoSelection::Changes;
        if let Some(review) = &self.work_inbox.repository.review {
            review.update(cx, |view, cx| view.set_panel_visible(visible, cx));
        }
    }

    pub(super) fn select_repository_item(
        &mut self,
        selection: RepoSelection,
        cx: &mut Context<Self>,
    ) {
        let repository = &mut self.work_inbox.repository;
        if repository.selection != selection {
            repository.selection = selection;
            repository.commit = None;
            repository.confirmation = None;
            repository.log.invalidate();
        }
        self.apply_repository_selection(cx);
        cx.notify();
    }

    /// Show what the selection needs: the working tree, a saved commit, or
    /// the commits of a branch.
    fn apply_repository_selection(&mut self, cx: &mut Context<Self>) {
        self.sync_repository_polling(cx);
        let Some(review) = self.work_inbox.repository.review.clone() else {
            return;
        };
        let selection = self.work_inbox.repository.selection.clone();
        let refs = self.repository_refs();
        let commit = match &selection {
            RepoSelection::Changes => {
                review.update(cx, |view, cx| view.show_worktree(cx));
                return;
            }
            RepoSelection::Stash(oid) => refs
                .and_then(|refs| refs.stashes.iter().find(|stash| stash.oid == *oid))
                .map(|stash| saved_commit(&stash.oid, &stash.subject, &stash.date)),
            RepoSelection::Tag(name) => refs
                .and_then(|refs| refs.tags.iter().find(|tag| tag.name == *name))
                .map(|tag| saved_commit(&tag.target, &tag.subject, &tag.date)),
            RepoSelection::Branch(_) | RepoSelection::Remote(_) => {
                self.load_repository_log(cx);
                self.work_inbox
                    .repository
                    .commit
                    .as_ref()
                    .and_then(|sha| {
                        self.work_inbox
                            .repository
                            .log
                            .data
                            .as_ref()
                            .and_then(|(_, commits)| commits.iter().find(|c| c.sha == *sha))
                    })
                    .cloned()
            }
            RepoSelection::Worktree(_) => None,
        };
        if let Some(commit) = commit {
            review.update(cx, |view, cx| view.show_commit(commit, cx));
        }
    }

    /// Open one of the selected branch's commits below its header.
    pub(super) fn show_repository_commit(
        &mut self,
        commit: Option<GitCommit>,
        cx: &mut Context<Self>,
    ) {
        self.work_inbox.repository.commit = commit.as_ref().map(|commit| commit.sha.clone());
        if let (Some(commit), Some(review)) = (commit, self.work_inbox.repository.review.clone()) {
            review.update(cx, |view, cx| view.show_commit(commit, cx));
        }
        cx.notify();
    }

    /// The base a branch's own commits are counted against.
    pub(super) fn branch_base(&self, branch: &str) -> String {
        let Some(refs) = self.repository_refs() else {
            return String::new();
        };
        let upstream = refs
            .branch(branch)
            .and_then(|branch| branch.upstream.as_deref());
        BASE_CANDIDATES
            .into_iter()
            .find(|candidate| {
                *candidate != branch
                    && Some(*candidate) != upstream
                    && (refs.branch(candidate).is_some()
                        || refs.remotes.iter().any(|remote| remote.name == *candidate))
            })
            .unwrap_or_default()
            .to_owned()
    }

    fn load_repository_log(&mut self, cx: &mut Context<Self>) {
        let (RepoSelection::Branch(branch) | RepoSelection::Remote(branch)) =
            self.work_inbox.repository.selection.clone()
        else {
            return;
        };
        let Some(root) = self.repository_root() else {
            return;
        };
        let log = &self.work_inbox.repository.log;
        if log.loading || log.data.as_ref().is_some_and(|(name, _)| *name == branch) {
            return;
        }
        let base = self.branch_base(&branch);
        let port = self.git_port.clone();
        let log = &mut self.work_inbox.repository.log;
        log.loading = true;
        log.revision += 1;
        let revision = log.revision;
        log._task = Some(cx.spawn(async move |this, cx| {
            let name = branch.clone();
            let result = cx
                .background_spawn(async move {
                    port.log_range(&root, &base, &name, BRANCH_LOG_LIMIT)
                        .map(|commits| (name, commits))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.work_inbox.repository.log.apply(revision, result);
                cx.notify();
            });
        }));
    }

    /// Drop a selection whose branch, stash, or tag no longer exists.
    pub(super) fn sync_repository_selection(&mut self, cx: &mut Context<Self>) {
        let Some(refs) = self.repository_refs() else {
            return;
        };
        let exists = match &self.work_inbox.repository.selection {
            RepoSelection::Changes => true,
            RepoSelection::Branch(name) => refs.branch(name).is_some(),
            RepoSelection::Remote(name) => refs.remotes.iter().any(|remote| remote.name == *name),
            RepoSelection::Worktree(path) => refs.worktrees.iter().any(|tree| tree.path == *path),
            RepoSelection::Stash(oid) => refs.stashes.iter().any(|stash| stash.oid == *oid),
            RepoSelection::Tag(name) => refs.tags.iter().any(|tag| tag.name == *name),
        };
        if !exists {
            self.select_repository_item(RepoSelection::Changes, cx);
        }
    }

    /// Run a branch, stash, or worktree action. Destructive ones ask first.
    pub(super) fn request_repository_action(
        &mut self,
        action: GitRefAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = match &action {
            GitRefAction::DeleteBranch { name, force: true } => Some(format!(
                "Delete {name}? Its commits that are not on another branch will be lost."
            )),
            GitRefAction::DeleteBranch { name, .. } => Some(format!("Delete the branch {name}?")),
            GitRefAction::StashDrop(_) => {
                Some("Drop this stash? Its changes cannot be recovered from Vibra.".into())
            }
            GitRefAction::RemoveWorktree(path) => Some(format!(
                "Remove the worktree at {}? Git refuses if it has uncommitted changes.",
                path.display()
            )),
            _ => None,
        };
        let confirmed = self
            .work_inbox
            .repository
            .confirmation
            .as_ref()
            .is_some_and(|pending| pending.action == action);
        if let Some(message) = message
            && !confirmed
        {
            self.work_inbox.repository.confirmation = Some(RepoConfirmation { action, message });
            cx.notify();
            return;
        }
        self.work_inbox.repository.confirmation = None;
        let Some(root) = self.repository_root() else {
            return;
        };
        let port = self.git_port.clone();
        let directory = root.clone();
        let label = action.label();
        let leaves_selection = matches!(
            action,
            GitRefAction::DeleteBranch { .. }
                | GitRefAction::StashDrop(_)
                | GitRefAction::StashPop(_)
                | GitRefAction::RemoveWorktree(_)
        );
        self.run_local_job(
            repository_key(),
            busy_label(&action),
            root,
            move || {
                port.run_ref_action(&directory, &action)?;
                Ok(format!("{label}: done."))
            },
            move |this, _, cx| {
                if leaves_selection {
                    this.select_repository_item(RepoSelection::Changes, cx);
                }
                this.work_inbox.repository.log.invalidate();
                this.apply_repository_selection(cx);
            },
            window,
            cx,
        );
    }

    pub(super) fn sync_repository(
        &mut self,
        operation: GitSyncOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.repository_root() else {
            return;
        };
        let port = self.git_port.clone();
        let directory = root.clone();
        self.run_local_job(
            repository_key(),
            match operation {
                GitSyncOperation::Fetch => "Fetching…",
                GitSyncOperation::Pull => "Pulling…",
                GitSyncOperation::Push => "Pushing…",
            },
            root,
            move || {
                port.sync(&directory, operation)?;
                Ok(format!("{}: done.", operation.label()))
            },
            |this, _, cx| {
                this.work_inbox.repository.log.invalidate();
                this.apply_repository_selection(cx);
            },
            window,
            cx,
        );
    }

    /// Open the pull request of a branch in the GitHub list.
    pub(super) fn open_branch_pull(&mut self, url: String, cx: &mut Context<Self>) {
        self.settings.inbox.repository_view = false;
        if self.settings.inbox.source != WorkSource::GitHub {
            self.select_work_source(WorkSource::GitHub, cx);
        }
        self.work_inbox.filter.project = None;
        self.select_work_item(&url, cx);
        self.sync_repository_polling(cx);
        self.persist_settings(cx);
        cx.notify();
    }

    /// The GitHub pull request whose head is this local branch, if loaded.
    pub(super) fn branch_pull(&self, branch: &str) -> Option<&WorkItem> {
        let project = self.repository_project()?;
        let refs = self.repository_refs()?;
        self.work_inbox.github.items.iter().find(|item| {
            item.project_id == Some(project)
                && item.pull.as_ref().is_some_and(|pull| {
                    super::local::match_pull_branch(refs, pull)
                        .is_some_and(|local| local.name == branch)
                })
        })
    }
}

/// Feedback key of operations started from the Repository view.
pub(super) fn repository_key() -> String {
    "repository".into()
}

fn busy_label(action: &GitRefAction) -> &'static str {
    match action {
        GitRefAction::Switch(_) | GitRefAction::SwitchAndPull(_) => "Switching…",
        GitRefAction::DeleteBranch { .. } => "Deleting…",
        GitRefAction::Stash { .. } => "Stashing…",
        GitRefAction::StashApply(_) | GitRefAction::StashPop(_) => "Applying…",
        GitRefAction::StashDrop(_) => "Dropping…",
        GitRefAction::AddWorktree(_) => "Creating worktree…",
        GitRefAction::RemoveWorktree(_) => "Removing…",
    }
}

/// A commit the review can open by its SHA alone.
fn saved_commit(sha: &str, subject: &str, date: &str) -> GitCommit {
    GitCommit {
        sha: sha.to_owned(),
        short_sha: sha.chars().take(7).collect(),
        subject: subject.to_owned(),
        author: String::new(),
        date: date.to_owned(),
        parents: Vec::new(),
        refs: Vec::new(),
    }
}
