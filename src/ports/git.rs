use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRepositorySnapshot {
    pub root: PathBuf,
    pub branch: String,
    pub changes: Vec<GitFileChange>,
    pub additions: usize,
    pub deletions: usize,
}

/// Lightweight branch/tracking summary for sidebar chrome (no file list or diffs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBranchSummary {
    pub branch: String,
    pub ahead: usize,
    pub behind: usize,
    /// True when the worktree has staged, unstaged, or untracked changes.
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFileChange {
    pub path: String,
    /// Previous path when Git reports a rename or copy.
    pub old_path: Option<String>,
    pub status: GitFileStatus,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub additions: Option<usize>,
    pub deletions: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDiff {
    pub path: String,
    pub rows: Vec<GitDiffRow>,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
    pub truncated: bool,
}

/// Whole-file text on both sides of a diff, so syntax state (block comments,
/// multi-line strings) is computed with full context instead of per hunk.
/// `None` when a side does not exist, is binary, or is too large to read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitDiffSources {
    pub old: Option<String>,
    pub new: Option<String>,
}

/// The full working tree (tracked and untracked, respecting ignores) frozen as
/// a tree object. Capturing it writes Git objects, but leaves the real index,
/// refs, and working files untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWorktreeCapture {
    pub root: PathBuf,
    pub tree: String,
}

/// Selected refs or working tree; automatic worktree comparison uses the merge-base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBranchChanges {
    pub snapshot: GitRepositorySnapshot,
    pub base: String,
    pub base_revision: String,
    pub head_revision: Option<String>,
    pub commits_ahead: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBranchRef {
    pub reference: String,
    pub name: String,
    pub remote: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommit {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub parents: Vec<String>,
    /// Branch and tag names pointing at this commit (`HEAD -> main` becomes `main`).
    pub refs: Vec<String>,
}

/// A saved commit compared with its first parent (or the empty tree for a root).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommitChanges {
    pub snapshot: GitRepositorySnapshot,
    pub base_revision: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHistory {
    pub branch: String,
    pub head: String,
    pub total: usize,
    pub commits: Vec<GitCommit>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDiffRow {
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub kind: GitDiffRowKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitDiffRowKind {
    Section,
    Hunk,
    Context,
    Addition,
    Deletion,
    Notice,
}

/// A branch with its tracking state and latest commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBranchInfo {
    /// Short name: `feature` locally, `origin/feature` for a remote branch.
    pub name: String,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// The upstream branch was deleted on the remote.
    pub gone: bool,
    pub head: String,
    pub current: bool,
    pub subject: String,
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWorktree {
    pub path: PathBuf,
    pub head: String,
    /// Short branch name; `None` for a detached HEAD.
    pub branch: Option<String>,
    /// The repository's main working tree, which cannot be removed.
    pub main: bool,
    pub prunable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStash {
    pub index: usize,
    pub oid: String,
    pub subject: String,
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitTag {
    pub name: String,
    /// The tagged commit, also for annotated tags.
    pub target: String,
    pub subject: String,
    pub date: String,
}

/// Everything the Repository view lists for one repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitRepositoryRefs {
    pub root: PathBuf,
    /// Current branch, or `HEAD` when detached.
    pub current: String,
    /// Staged, unstaged, and untracked paths.
    pub changes: usize,
    pub branches: Vec<GitBranchInfo>,
    pub remotes: Vec<GitBranchInfo>,
    pub worktrees: Vec<GitWorktree>,
    pub stashes: Vec<GitStash>,
    pub tags: Vec<GitTag>,
}

impl GitRepositoryRefs {
    pub fn branch(&self, name: &str) -> Option<&GitBranchInfo> {
        self.branches.iter().find(|branch| branch.name == name)
    }

    /// The linked worktree (never the main one) that has `branch` checked out.
    pub fn worktree_for(&self, branch: &str) -> Option<&GitWorktree> {
        self.worktrees
            .iter()
            .find(|worktree| !worktree.main && worktree.branch.as_deref() == Some(branch))
    }
}

/// Branch, stash, and worktree changes started from the Repository view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRefAction {
    /// `git switch`; a remote branch's short name creates its tracking branch.
    Switch(String),
    /// Switch, then fast-forward from the upstream.
    SwitchAndPull(String),
    DeleteBranch {
        name: String,
        /// Delete even when the branch is not merged (`-D`).
        force: bool,
    },
    /// Stash every change, untracked files included.
    Stash {
        message: String,
    },
    StashApply(usize),
    StashPop(usize),
    StashDrop(usize),
    /// A new linked worktree with a detached HEAD at the current commit.
    AddWorktree(PathBuf),
    RemoveWorktree(PathBuf),
}

impl GitRefAction {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Switch(_) => "Switch branch",
            Self::SwitchAndPull(_) => "Switch and pull",
            Self::DeleteBranch { .. } => "Delete branch",
            Self::Stash { .. } => "Stash changes",
            Self::StashApply(_) => "Apply stash",
            Self::StashPop(_) => "Pop stash",
            Self::StashDrop(_) => "Drop stash",
            Self::AddWorktree(_) => "Add worktree",
            Self::RemoveWorktree(_) => "Remove worktree",
        }
    }
}

/// Options for a commit made from the Changes panel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GitCommitOptions {
    /// Rewrite the last commit instead of adding one.
    pub amend: bool,
}

/// Network operations the Changes panel runs without an interactive prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitSyncOperation {
    Push,
    Pull,
    Fetch,
}

impl GitSyncOperation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Push => "Push",
            Self::Pull => "Pull",
            Self::Fetch => "Fetch",
        }
    }
}

/// Boundary for repository inspection. `capture_worktree` writes Git objects;
/// `commit` and `sync` change the repository only when the user asks for it
/// from the Changes panel; the other methods leave it unchanged.
pub trait GitPort: Send + Sync {
    fn snapshot(&self, root: &Path) -> Result<Option<GitRepositorySnapshot>>;
    /// Fast branch + ahead/behind + dirty flag for sidebar tabs (no numstat/diff work).
    fn branch_summary(&self, root: &Path) -> Result<Option<GitBranchSummary>>;
    fn diff(&self, repository: &Path, change: &GitFileChange) -> Result<GitDiff>;
    fn branches(&self, root: &Path) -> Result<Vec<GitBranchRef>>;
    fn branch_changes(
        &self,
        root: &Path,
        base: Option<&str>,
        head: Option<&str>,
    ) -> Result<Option<GitBranchChanges>>;
    fn history(&self, root: &Path, limit: usize) -> Result<Option<GitHistory>>;
    fn commit_changes(&self, root: &Path, revision: &str) -> Result<GitCommitChanges>;
    /// Both whole files behind one file's diff. Worktree comparisons
    /// (`against` = `None`) use the same sides as [`GitPort::diff`].
    fn diff_sources(
        &self,
        repository: &Path,
        change: &GitFileChange,
        against: Option<&str>,
        head: Option<&str>,
    ) -> Result<GitDiffSources>;
    /// Save a worktree snapshot as unreachable Git objects using a private index.
    fn capture_worktree(&self, root: &Path) -> Result<Option<GitWorktreeCapture>>;
    /// Files that differ between two saved trees (or commits).
    fn tree_changes(&self, root: &Path, base: &str, head: &str) -> Result<GitCommitChanges>;
    fn diff_against(
        &self,
        repository: &Path,
        revision: &str,
        head: Option<&str>,
        change: &GitFileChange,
    ) -> Result<GitDiff>;
    /// Commits staged changes, or every change when nothing is staged.
    /// Returns the new commit's short SHA.
    fn commit(&self, root: &Path, message: &str, options: GitCommitOptions) -> Result<String>;
    /// Push, pull (fast-forward only), or fetch without prompting for credentials.
    fn sync(&self, root: &Path, operation: GitSyncOperation) -> Result<()>;
    /// Adds whole files (new, modified, or deleted) to the index.
    fn stage(&self, root: &Path, paths: &[String]) -> Result<()>;
    /// Removes files from the index, keeping their working-tree contents.
    fn unstage(&self, root: &Path, paths: &[String]) -> Result<()>;
    /// What the next commit would contain, as a bounded patch with recent
    /// subjects for style, for drafting its message.
    fn commit_message_context(&self, root: &Path) -> Result<String>;
    /// Branches, worktrees, stashes, and tags for the Repository view.
    fn repository_refs(&self, root: &Path) -> Result<Option<GitRepositoryRefs>>;
    /// Commits reachable from `head` but not from `base` (all of `head`'s
    /// history when `base` is empty), newest first.
    fn log_range(
        &self,
        root: &Path,
        base: &str,
        head: &str,
        limit: usize,
    ) -> Result<Vec<GitCommit>>;
    /// Changes the repository only when the user asks for it.
    fn run_ref_action(&self, root: &Path, action: &GitRefAction) -> Result<()>;
}
