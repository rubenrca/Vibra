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

/// Boundary for read-only repository inspection.
pub trait GitPort: Send + Sync {
    fn snapshot(&self, root: &Path) -> Result<Option<GitRepositorySnapshot>>;
    /// Fast branch + upstream + dirty flag for sidebar tabs (no numstat/diff work).
    fn branch_summary(&self, root: &Path) -> Result<Option<GitBranchSummary>>;
    fn diff(&self, repository: &Path, change: &GitFileChange) -> Result<GitDiff>;
}
