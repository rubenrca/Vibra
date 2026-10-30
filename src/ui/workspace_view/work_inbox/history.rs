//! Repository → History: one project's commit graph, each commit's pull
//! request, and the selected commit's changes in an embedded review.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui::ScrollStrategy;

use super::*;
use crate::domain::work_items::{RepositoryTab, subject_pull_number};
use crate::ports::git::GitCommit;
use crate::ui::diff_view::{DiffFileIndexView, DiffView, DiffViewEvent};

/// Commits read at once; `GitPort` caps logs at this size.
const HISTORY_LIMIT: usize = 500;
const HISTORY_TTL: Duration = Duration::from_secs(15);

/// The pull request behind a commit of the graph.
pub(super) struct CommitPull<'a> {
    pub(super) number: u64,
    /// The task in the feed, when it is loaded.
    pub(super) item: Option<&'a WorkItem>,
    /// The commit merged it, rather than heading its open branch.
    pub(super) merged: bool,
}

impl WorkspaceView {
    /// The project whose history the tab shows: the user's pick, else the
    /// active project, else the first one with a folder.
    pub(super) fn history_project(&self) -> Option<Uuid> {
        let usable = |id: &Uuid| {
            self.snapshot
                .projects
                .iter()
                .any(|project| project.id == *id && project.directory().is_some())
        };
        self.work_inbox
            .history
            .project
            .filter(usable)
            .or_else(|| self.snapshot.selected_project_id.filter(usable))
            .or_else(|| {
                self.snapshot
                    .projects
                    .iter()
                    .find(|project| project.directory().is_some())
                    .map(|project| project.id)
            })
    }

    pub(super) fn history_root(&self) -> Option<PathBuf> {
        self.history_project()
            .and_then(|id| self.project_folder(id))
    }

    /// Read the graph when it is missing, stale, or of another project or
    /// scope. A forced refresh keeps the current commits on screen meanwhile.
    pub(in crate::ui::workspace_view) fn open_history(
        &mut self,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if cfg!(test) {
            return;
        }
        let Some(root) = self.history_root() else {
            return;
        };
        // Local branch names tell branches, remote branches, and tags apart.
        self.refresh_local_repo(root.clone(), force, cx);
        let history = &mut self.work_inbox.history;
        let current_only = history.current_only;
        let same = history
            .log
            .data
            .as_ref()
            .is_some_and(|log| log.root == root && log.current_only == current_only);
        let fresh = history
            .fetched
            .is_some_and(|fetched| fetched.elapsed() < HISTORY_TTL);
        if !same {
            // Another repository or scope: its commits and selection do not apply.
            history.log.invalidate();
            history.selected = None;
            history.cursor = None;
            history.fetched = None;
        } else if !force && (history.log.loading || fresh) {
            return;
        }
        let port = self.git_port.clone();
        let log = &mut history.log;
        log.loading = true;
        log.revision += 1;
        let revision = log.revision;
        log._task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let history = if current_only {
                        port.history(&root, HISTORY_LIMIT)
                    } else {
                        port.branch_graph(&root, HISTORY_LIMIT)
                    }?
                    .context("This space is not in a Git repository.")?;
                    let graph = crate::ui::git_graph::assign_commit_lanes(&history.commits);
                    Ok(HistoryLog {
                        root,
                        current_only,
                        history,
                        graph,
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let history = &mut this.work_inbox.history;
                history.fetched = Some(Instant::now());
                history.log.apply(revision, result);
                let gone = history.selected.as_ref().is_some_and(|sha| {
                    history.log.data.as_ref().is_some_and(|log| {
                        !log.history.commits.iter().any(|commit| commit.sha == *sha)
                    })
                });
                if gone {
                    history.selected = None;
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn select_history_project(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.work_inbox.history.project = Some(id);
        self.work_inbox.menu = None;
        self.open_history(false, cx);
        cx.notify();
    }

    pub(super) fn set_history_scope(&mut self, current_only: bool, cx: &mut Context<Self>) {
        if self.work_inbox.history.current_only != current_only {
            self.work_inbox.history.current_only = current_only;
            self.open_history(false, cx);
        }
        cx.notify();
    }

    pub(super) fn history_commit(&self, sha: &str) -> Option<&GitCommit> {
        self.work_inbox
            .history
            .log
            .data
            .as_ref()?
            .history
            .commits
            .iter()
            .find(|commit| commit.sha == sha)
    }

    /// Open a commit's page in place of the graph, or go back to the graph
    /// with that commit's row in view.
    pub(super) fn select_history_commit(&mut self, sha: Option<String>, cx: &mut Context<Self>) {
        let commit = sha
            .as_deref()
            .and_then(|sha| self.history_commit(sha))
            .cloned();
        let history = &mut self.work_inbox.history;
        history.selected = commit.as_ref().map(|commit| commit.sha.clone());
        if history.selected.is_some() {
            history.cursor = history.selected.clone();
        } else if let Some(index) = history.cursor.as_ref().and_then(|sha| {
            history
                .log
                .data
                .as_ref()?
                .history
                .commits
                .iter()
                .position(|commit| commit.sha == *sha)
        }) {
            history.scroll.scroll_to_item(index, ScrollStrategy::Center);
        }
        if let (Some(commit), Some(root)) = (commit, self.history_root()) {
            self.ensure_history_review(cx);
            if let Some(review) = self.work_inbox.history.review.clone() {
                review.update(cx, |view, cx| {
                    view.set_root(root, cx);
                    view.set_embedded(cx);
                    view.show_commit(commit, cx);
                });
            }
        }
        cx.notify();
    }

    /// Move down (`1`, older) or up (`-1`, newer) the graph: an open commit's
    /// page follows, otherwise only the marked row moves. The row stays in
    /// view for the way back.
    pub(super) fn step_history_commit(&mut self, delta: isize, cx: &mut Context<Self>) {
        let history = &self.work_inbox.history;
        let Some(commits) = history
            .log
            .data
            .as_ref()
            .map(|log| &log.history.commits)
            .filter(|commits| !commits.is_empty())
        else {
            return;
        };
        let current = history
            .selected
            .as_ref()
            .or(history.cursor.as_ref())
            .and_then(|sha| commits.iter().position(|commit| commit.sha == *sha));
        let next = current.map_or(0, |index| {
            index.saturating_add_signed(delta).min(commits.len() - 1)
        });
        let sha = commits[next].sha.clone();
        if history.selected.is_some() {
            self.select_history_commit(Some(sha), cx);
        } else {
            self.work_inbox.history.cursor = Some(sha);
        }
        self.work_inbox
            .history
            .scroll
            .scroll_to_item(next, ScrollStrategy::Center);
        cx.notify();
    }

    /// Where the open commit sits in the graph, and how many commits it has.
    pub(super) fn history_position(&self) -> Option<(usize, usize)> {
        let history = &self.work_inbox.history;
        let commits = &history.log.data.as_ref()?.history.commits;
        let sha = history.selected.as_ref()?;
        let index = commits.iter().position(|commit| commit.sha == *sha)?;
        Some((index, commits.len()))
    }

    /// The pull request whose branch ends at this commit (by its head, or by
    /// a branch of the same name when the PR moved since the last fetch), or
    /// the one its subject says it merged.
    pub(super) fn commit_pull(&self, commit: &GitCommit) -> Option<CommitPull<'_>> {
        let project = self.history_project();
        let pulls = || {
            self.work_inbox
                .github
                .items
                .iter()
                .filter(move |item| item.project_id == project && item.pull.is_some())
        };
        let heads = |item: &&WorkItem| {
            item.pull
                .as_ref()
                .is_some_and(|pull| pull.head_oid == commit.sha)
        };
        let named = |item: &&WorkItem| {
            item.pull.as_ref().is_some_and(|pull| {
                !pull.cross_repository
                    && !pull.head_ref.is_empty()
                    && commit
                        .refs
                        .iter()
                        .any(|name| branch_names(name, &pull.head_ref))
            })
        };
        if let Some(item) = pulls().find(heads).or_else(|| pulls().find(named)) {
            return Some(CommitPull {
                number: item.reference.trim_start_matches('#').parse().unwrap_or(0),
                item: Some(item),
                merged: item.status == WorkStatus::Merged,
            });
        }
        let number = subject_pull_number(&commit.subject)?;
        let reference = format!("#{number}");
        Some(CommitPull {
            number,
            item: pulls().find(|item| item.reference == reference),
            merged: true,
        })
    }

    /// Show the pull request in its tab, or on GitHub when the feed does
    /// not have it.
    pub(super) fn open_commit_pull(&mut self, number: u64, cx: &mut Context<Self>) {
        let project = self.history_project();
        let reference = format!("#{number}");
        let url = self
            .work_inbox
            .github
            .items
            .iter()
            .find(|item| {
                item.project_id == project && item.pull.is_some() && item.reference == reference
            })
            .map(|item| item.url.clone());
        let Some(url) = url else {
            if let Some(repository) = self.history_github_repository() {
                cx.open_url(&format!("https://github.com/{repository}/pull/{number}"));
            }
            return;
        };
        self.select_repository_tab(RepositoryTab::PullRequests, cx);
        if !self
            .ordered_inbox_items()
            .iter()
            .any(|item| item.url == url)
        {
            // Filters must not hide the pull request the user asked for.
            self.work_inbox.filter.project = None;
            self.work_inbox.filter.query.clear();
        }
        self.select_work_item(&url, cx);
        cx.notify();
    }

    /// Names of the project's local branches, once they are read.
    pub(super) fn history_local_branches(&self) -> Option<std::collections::HashSet<String>> {
        let refs = self.repo_refs(&self.history_root()?)?;
        Some(
            refs.branches
                .iter()
                .map(|branch| branch.name.clone())
                .collect(),
        )
    }

    /// `owner/name` of the project's GitHub repository, from its loaded tasks.
    pub(super) fn history_github_repository(&self) -> Option<String> {
        let project = self.history_project()?;
        self.work_inbox
            .github
            .items
            .iter()
            .find(|item| item.project_id == Some(project))
            .map(|item| item.repository.clone())
    }

    fn ensure_history_review(&mut self, cx: &mut Context<Self>) {
        if self.work_inbox.history.review.is_some() {
            return;
        }
        let root = self.history_root().unwrap_or_default();
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
        let index = cx.new(|cx| DiffFileIndexView::new(review.clone(), cx).files_only());
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
                    if let Some(project) = this.history_project()
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
                    if let Some(review) = this.work_inbox.history.review.clone() {
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
        self.work_inbox.history.review = Some(review);
        self.work_inbox.history.index = Some(index);
        self.work_inbox.history._review_subscription = Some(subscription);
    }
}

/// `name` is the branch `branch`, locally or on a remote (`origin/branch`).
fn branch_names(name: &str, branch: &str) -> bool {
    name == branch
        || name
            .strip_suffix(branch)
            .and_then(|prefix| prefix.strip_suffix('/'))
            .is_some_and(|remote| !remote.is_empty() && !remote.contains('/'))
}

#[cfg(test)]
mod tests {
    use super::branch_names;

    #[test]
    fn pull_branches_match_local_and_remote_names() {
        assert!(branch_names("feature/x", "feature/x"));
        assert!(branch_names("origin/feature/x", "feature/x"));
        assert!(!branch_names("origin/other/feature/x", "x/feature/x"));
        assert!(!branch_names("my-feature/x", "feature/x"));
        assert!(!branch_names("a/b/feature/x", "feature/x"));
    }
}
