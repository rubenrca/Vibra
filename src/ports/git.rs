use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRepositorySnapshot {
    pub root: PathBuf,
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub changes: Vec<GitFileChange>,
    pub additions: usize,
    pub deletions: usize,
    /// Changes whenever HEAD, the index, or tracked/untracked worktree content changes.
    pub state_token: String,
}

/// Lightweight branch/tracking summary for sidebar chrome (no file list or diffs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBranchSummary {
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// True when the worktree has staged, unstaged, or untracked changes.
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct GitCommit {
    pub oid: String,
    pub short_oid: String,
    pub summary: String,
    pub author: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFileChange {
    pub path: String,
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

impl GitFileStatus {
    pub fn badge(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Copied => "C",
            Self::TypeChanged => "T",
            Self::Untracked => "?",
            Self::Conflicted => "U",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub rows: Vec<GitDiffRow>,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
    pub truncated: bool,
    pub hunks: Vec<GitDiffHunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDiffHunk {
    pub header: String,
    pub patch: String,
    pub source: GitDiffHunkSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitDiffHunkSource {
    Index,
    Worktree,
    Untracked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum GitHunkAction {
    Stage,
    Unstage,
    Discard,
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

/// Boundary for repository inspection and guarded mutations.
///
/// Mutations remain behind the port for backend compatibility and tests even
/// though the compact UI currently exposes only repository inspection.
#[allow(dead_code)]
pub trait GitPort: Send + Sync {
    fn snapshot(&self, root: &Path) -> Result<Option<GitRepositorySnapshot>>;
    /// Fast branch + upstream + dirty flag for sidebar tabs (no numstat/diff work).
    fn branch_summary(&self, root: &Path) -> Result<Option<GitBranchSummary>>;
    fn diff(&self, repository: &Path, change: &GitFileChange) -> Result<GitDiff>;
    fn stage(&self, repository: &Path, path: &str, expected_state: &str) -> Result<()>;
    fn unstage(&self, repository: &Path, path: &str, expected_state: &str) -> Result<()>;
    fn discard_worktree_change(
        &self,
        repository: &Path,
        path: &str,
        expected_state: &str,
    ) -> Result<()>;
    fn commit(
        &self,
        repository: &Path,
        message: &str,
        amend: bool,
        expected_state: &str,
    ) -> Result<()>;
    fn fetch(&self, repository: &Path) -> Result<()>;
    fn pull_ff_only(&self, repository: &Path, expected_state: &str) -> Result<()>;
    fn push(&self, repository: &Path) -> Result<()>;
    fn publish_branch(&self, repository: &Path, branch: &str) -> Result<()>;
    fn create_branch(&self, repository: &Path, branch: &str, expected_state: &str) -> Result<()>;
    fn stash(&self, repository: &Path, message: &str, expected_state: &str) -> Result<()>;
    fn stash_pop(&self, repository: &Path, expected_state: &str) -> Result<()>;
    fn initialize(&self, root: &Path) -> Result<()>;
    fn recent_commits(&self, repository: &Path, limit: usize) -> Result<Vec<GitCommit>>;
    fn apply_hunk(
        &self,
        repository: &Path,
        hunk: &GitDiffHunk,
        action: GitHunkAction,
        expected_state: &str,
    ) -> Result<()>;
}
