//! Local checkouts behind Review: which pull requests are checked out, and the
//! Git operations that connect a pull request with a project's repository.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::domain::work_items::PullSummary;
use crate::ports::git::{
    GitBranchInfo, GitRefAction, GitRepositoryRefs, GitSyncOperation, GitWorktree,
};

/// Linked worktrees for pull requests live under the user's Vibra folder,
/// never next to the repository.
const WORKTREES_DIR: &str = ".vibra/worktrees";
const REFS_TTL: Duration = Duration::from_secs(15);

/// Where a pull request stands in the local repository of its project.
#[derive(Clone, Copy)]
pub(in crate::ui::workspace_view) enum LocalPull<'a> {
    /// No project, no repository, or its branches are still loading.
    Unknown,
    /// No local branch has the pull request's head.
    Missing,
    Branch {
        branch: &'a GitBranchInfo,
        /// Checked out in the project's own folder.
        current: bool,
        /// Checked out in a linked worktree instead.
        worktree: Option<&'a GitWorktree>,
        /// The local branch points at the pull request's head.
        synced: bool,
    },
}

/// The local branch that holds a pull request. Same-repository pull requests
/// match by name or upstream; a fork's branch name means nothing locally,
/// so only its exact head counts.
pub(super) fn match_pull_branch<'a>(
    refs: &'a GitRepositoryRefs,
    pull: &PullSummary,
) -> Option<&'a GitBranchInfo> {
    if pull.head_ref.is_empty() {
        return None;
    }
    let by_head = || {
        refs.branches
            .iter()
            .find(|branch| !pull.head_oid.is_empty() && branch.head == pull.head_oid)
    };
    if pull.cross_repository {
        return by_head();
    }
    refs.branch(&pull.head_ref)
        .or_else(|| {
            refs.branches.iter().find(|branch| {
                branch.upstream.as_deref().is_some_and(|upstream| {
                    upstream
                        .split_once('/')
                        .is_some_and(|(_, name)| name == pull.head_ref)
                })
            })
        })
        .or_else(by_head)
}

pub(super) fn local_pull<'a>(refs: &'a GitRepositoryRefs, pull: &PullSummary) -> LocalPull<'a> {
    let Some(branch) = match_pull_branch(refs, pull) else {
        return LocalPull::Missing;
    };
    LocalPull::Branch {
        branch,
        current: branch.current,
        worktree: refs.worktree_for(&branch.name),
        synced: !pull.head_oid.is_empty() && branch.head == pull.head_oid,
    }
}

/// `~/.vibra/worktrees/<repository folder>/pr-<number>`.
pub(super) fn pull_worktree_path(home: &Path, repository: &Path, reference: &str) -> PathBuf {
    let folder = repository
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "repository".into());
    let number: String = reference.chars().filter(char::is_ascii_digit).collect();
    home.join(WORKTREES_DIR)
        .join(folder)
        .join(format!("pr-{number}"))
}

impl WorkspaceView {
    pub(super) fn project_folder(&self, id: Uuid) -> Option<PathBuf> {
        self.snapshot
            .projects
            .iter()
            .find(|project| project.id == id)?
            .directory()
            .map(PathBuf::from)
    }

    pub(super) fn repo_refs(&self, root: &Path) -> Option<&GitRepositoryRefs> {
        self.work_inbox
            .repos
            .get(root)
            .and_then(|repo| repo.refs.as_deref())
    }

    /// The task's project folder and its pull request's local state.
    pub(super) fn local_pull(&self, item: &WorkItem) -> (Option<PathBuf>, LocalPull<'_>) {
        let root = item.project_id.and_then(|id| self.project_folder(id));
        let state = match (&root, &item.pull) {
            (Some(root), Some(pull)) => self
                .repo_refs(root)
                .map_or(LocalPull::Unknown, |refs| local_pull(refs, pull)),
            _ => LocalPull::Unknown,
        };
        (root, state)
    }

    /// Reload a repository's branches unless a fresh copy exists.
    pub(super) fn refresh_local_repo(
        &mut self,
        root: PathBuf,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if cfg!(test) {
            return;
        }
        let repo = self.work_inbox.repos.entry(root.clone()).or_default();
        if repo.loading
            || (!force
                && repo
                    .fetched
                    .is_some_and(|fetched| fetched.elapsed() < REFS_TTL))
        {
            return;
        }
        repo.loading = true;
        repo.generation += 1;
        let generation = repo.generation;
        let port = self.git_port.clone();
        repo._task = Some(cx.spawn(async move |this, cx| {
            let path = root.clone();
            let result = cx
                .background_spawn(async move { port.repository_refs(&path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(repo) = this.work_inbox.repos.get_mut(&root) else {
                    return;
                };
                if repo.generation != generation {
                    return;
                }
                repo.loading = false;
                repo.fetched = Some(std::time::Instant::now());
                match result {
                    Ok(refs) => {
                        repo.refs = refs.map(Arc::new);
                        repo.error = None;
                    }
                    Err(error) => repo.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        }));
    }

    /// Every project with a pull request in the feed.
    pub(super) fn refresh_inbox_repos(&mut self, force: bool, cx: &mut Context<Self>) {
        let mut projects: Vec<Uuid> = self
            .work_inbox
            .github
            .items
            .iter()
            .filter(|item| item.pull.is_some())
            .filter_map(|item| item.project_id)
            .collect();
        projects.sort_unstable();
        projects.dedup();
        for root in projects
            .into_iter()
            .filter_map(|id| self.project_folder(id))
            .collect::<Vec<_>>()
        {
            self.refresh_local_repo(root, force, cx);
        }
    }

    /// Run one Git operation in the background, then reload the repository and
    /// report the outcome next to the control identified by `key`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_local_job(
        &mut self,
        key: String,
        label: &'static str,
        root: PathBuf,
        work: impl FnOnce() -> anyhow::Result<String> + Send + 'static,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.work_inbox.local_job.is_some() {
            return;
        }
        self.work_inbox.local_feedback = None;
        let task_key = key.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { work() }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.work_inbox.local_job = None;
                let ok = result.is_ok();
                this.work_inbox.local_feedback = Some(match result {
                    Ok(message) => (task_key, message, false),
                    Err(error) => (task_key, format!("{error:#}"), true),
                });
                this.refresh_local_repo(root, true, cx);
                this.refresh_diff_views(cx);
                if ok {
                    then(this, window, cx);
                }
                cx.notify();
            });
        });
        self.work_inbox.local_job = Some(LocalJob {
            key,
            label,
            _task: task,
        });
        cx.notify();
    }

    /// The repository changed under every review and the history of this window.
    pub(super) fn refresh_diff_views(&mut self, cx: &mut Context<Self>) {
        self.diff_view.update(cx, |view, cx| view.refresh_now(cx));
        self.work_inbox.history.fetched = None;
        if self.workspace_section == WorkspaceSection::Inbox
            && self.settings.inbox.tab == RepositoryTab::History
        {
            self.open_history(false, cx);
        }
    }

    pub(super) fn local_job_label(&self, key: &str) -> Option<&'static str> {
        self.work_inbox
            .local_job
            .as_ref()
            .filter(|job| job.key == key)
            .map(|job| job.label)
    }

    fn pull_target(&self, url: &str) -> Option<(WorkItem, PathBuf)> {
        let item = self
            .work_inbox
            .github
            .items
            .iter()
            .find(|item| item.url == url && item.pull.is_some())?
            .clone();
        let root = item.project_id.and_then(|id| self.project_folder(id))?;
        Some((item, root))
    }

    fn report_local(&mut self, key: &str, message: &str, cx: &mut Context<Self>) {
        self.work_inbox.local_feedback = Some((key.into(), message.into(), true));
        cx.notify();
    }

    /// `gh pr checkout` in the project's folder.
    pub(super) fn checkout_pull(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((item, root)) = self.pull_target(url) else {
            return self.report_local(
                url,
                "Link this repository to a project to check it out.",
                cx,
            );
        };
        let branch = item
            .pull
            .as_ref()
            .map(|pull| pull.head_ref.clone())
            .unwrap_or_default();
        let directory = root.clone();
        self.run_local_job(
            url.into(),
            "Checking out…",
            root,
            move || {
                work_items::checkout_pull_request(&item, &directory)?;
                Ok(format!("Checked out {branch}."))
            },
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// Check the pull request out in its own linked worktree and open that
    /// folder as a project, so the current branch stays untouched.
    pub(super) fn open_pull_worktree(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((item, root)) = self.pull_target(url) else {
            return self.report_local(
                url,
                "Link this repository to a project to open a worktree.",
                cx,
            );
        };
        let name = format!(
            "{} {}",
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            item.reference
        );
        if let (
            _,
            LocalPull::Branch {
                worktree: Some(worktree),
                ..
            },
        ) = self.local_pull(&item)
        {
            let path = worktree.path.clone();
            self.open_folder_project(&path, &name, window, cx);
            return;
        }
        let Some(home) = self.home_directory.clone() else {
            return self.report_local(url, "Could not find your home folder.", cx);
        };
        let path = pull_worktree_path(&home, &root, &item.reference);
        if path.join(".git").exists() {
            self.open_folder_project(&path, &name, window, cx);
            return;
        }
        let port = self.git_port.clone();
        let repository = root.clone();
        let target = path.clone();
        self.run_local_job(
            url.into(),
            "Creating worktree…",
            root,
            move || {
                port.run_ref_action(&repository, &GitRefAction::AddWorktree(target.clone()))?;
                if let Err(error) = work_items::checkout_pull_request(&item, &target) {
                    // Leave no half-made worktree behind.
                    let _ = port.run_ref_action(&repository, &GitRefAction::RemoveWorktree(target));
                    return Err(error);
                }
                Ok("Worktree ready.".into())
            },
            move |this, window, cx| {
                this.open_folder_project(&path, &name, window, cx);
            },
            window,
            cx,
        );
    }

    /// Push, pull, or another branch action on the pull request's branch.
    pub(super) fn run_pull_git(
        &mut self,
        url: &str,
        action: PullGit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((item, root)) = self.pull_target(url) else {
            return;
        };
        let pull = item.pull.clone().unwrap_or_default();
        let (_, local) = self.local_pull(&item);
        let port = self.git_port.clone();
        let directory = root.clone();
        let (label, work): (
            &'static str,
            Box<dyn FnOnce() -> anyhow::Result<String> + Send>,
        ) = match (action, local) {
            (PullGit::Push, _) => (
                "Pushing…",
                Box::new(move || {
                    port.sync(&directory, GitSyncOperation::Push)?;
                    Ok("Pushed. GitHub updates the pull request.".into())
                }),
            ),
            (PullGit::Pull, _) => (
                "Pulling…",
                Box::new(move || {
                    port.sync(&directory, GitSyncOperation::Pull)?;
                    Ok("Branch is up to date.".into())
                }),
            ),
            (PullGit::SwitchToBase, _) => {
                let base = pull.base_ref.clone();
                (
                    "Switching…",
                    Box::new(move || {
                        port.run_ref_action(
                            &directory,
                            &GitRefAction::SwitchAndPull(base.clone()),
                        )?;
                        Ok(format!("Switched to {base} and pulled."))
                    }),
                )
            }
            (PullGit::DeleteBranch, LocalPull::Branch { branch, synced, .. }) => {
                let name = branch.name.clone();
                (
                    "Deleting…",
                    Box::new(move || {
                        // A branch at the merged head lost nothing; anything
                        // else must be merged locally, as `git branch -d` checks.
                        port.run_ref_action(
                            &directory,
                            &GitRefAction::DeleteBranch {
                                name: name.clone(),
                                force: synced,
                            },
                        )?;
                        Ok(format!("Deleted local branch {name}."))
                    }),
                )
            }
            (
                PullGit::RemoveWorktree,
                LocalPull::Branch {
                    worktree: Some(worktree),
                    ..
                },
            ) => {
                let path = worktree.path.clone();
                (
                    "Removing…",
                    Box::new(move || {
                        port.run_ref_action(&directory, &GitRefAction::RemoveWorktree(path))?;
                        Ok("Removed the worktree.".into())
                    }),
                )
            }
            _ => return,
        };
        self.run_local_job(url.into(), label, root, work, |_, _, _| {}, window, cx);
    }
}

/// Branch operations offered next to a pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui::workspace_view) enum PullGit {
    Push,
    Pull,
    SwitchToBase,
    DeleteBranch,
    RemoveWorktree,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str, head: &str, upstream: Option<&str>) -> GitBranchInfo {
        GitBranchInfo {
            name: name.into(),
            upstream: upstream.map(str::to_owned),
            ahead: 0,
            behind: 0,
            gone: false,
            head: head.into(),
            current: false,
            subject: String::new(),
            date: String::new(),
        }
    }

    #[test]
    fn pull_requests_match_local_branches_by_name_upstream_or_head() {
        let mut refs = GitRepositoryRefs {
            branches: vec![
                branch("main", "m", Some("origin/main")),
                branch("search", "s", Some("origin/feature/search")),
            ],
            ..Default::default()
        };
        let mut pull = PullSummary {
            head_ref: "feature/search".into(),
            head_oid: "s".into(),
            ..Default::default()
        };
        assert_eq!(match_pull_branch(&refs, &pull).unwrap().name, "search");
        assert!(matches!(
            local_pull(&refs, &pull),
            LocalPull::Branch {
                synced: true,
                current: false,
                worktree: None,
                ..
            }
        ));
        pull.head_oid = "newer".into();
        assert!(matches!(
            local_pull(&refs, &pull),
            LocalPull::Branch { synced: false, .. }
        ));
        // A fork's `main` is not the local `main`.
        let fork = PullSummary {
            head_ref: "main".into(),
            head_oid: "f".into(),
            cross_repository: true,
            ..Default::default()
        };
        assert!(matches!(local_pull(&refs, &fork), LocalPull::Missing));
        refs.branches.push(branch("ana-main", "f", None));
        assert_eq!(match_pull_branch(&refs, &fork).unwrap().name, "ana-main");
        refs.worktrees.push(GitWorktree {
            path: "/tmp/wt".into(),
            head: "f".into(),
            branch: Some("ana-main".into()),
            main: false,
            prunable: false,
        });
        assert!(matches!(
            local_pull(&refs, &fork),
            LocalPull::Branch {
                worktree: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn pull_worktrees_live_under_the_vibra_folder() {
        assert_eq!(
            pull_worktree_path(Path::new("/Users/ana"), Path::new("/code/app"), "#42"),
            PathBuf::from("/Users/ana/.vibra/worktrees/app/pr-42")
        );
    }
}
