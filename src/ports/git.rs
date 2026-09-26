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
}
