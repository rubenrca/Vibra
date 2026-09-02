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
    pub rows: Vec<GitDiffRow>,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
    pub truncated: bool,
}

/// Committed + working-tree changes against a merge-base (feature vs default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBranchChanges {
    pub snapshot: GitRepositorySnapshot,
    pub base: String,
    pub merge_base: String,
    pub commits_ahead: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommit {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub parents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHistory {
    pub branch: String,
    pub head: String,
    pub total: usize,
    pub commits: Vec<GitCommit>,
    pub truncated: bool,
}

/// One row of a first-parent-aware lane graph for the history list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitGraphRow {
    pub lane: usize,
    pub color: usize,
    pub lane_count: usize,
    pub through: Vec<GitGraphRail>,
    /// A first parent already occupying another lane; the current branch joins it.
    pub first_parent_edge: Option<GitGraphRail>,
    /// Secondary parents that branch away from the current commit.
    pub edges: Vec<GitGraphRail>,
    pub continues: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitGraphRail {
    pub lane: usize,
    pub color: usize,
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
    /// Fast branch + ahead/behind + dirty flag for sidebar tabs (no numstat/diff work).
    fn branch_summary(&self, root: &Path) -> Result<Option<GitBranchSummary>>;
    fn diff(&self, repository: &Path, change: &GitFileChange) -> Result<GitDiff>;
    fn branch_changes(&self, root: &Path) -> Result<Option<GitBranchChanges>>;
    fn history(&self, root: &Path, limit: usize) -> Result<Option<GitHistory>>;
    fn diff_against(
        &self,
        repository: &Path,
        revision: &str,
        change: &GitFileChange,
    ) -> Result<GitDiff>;
}

/// Assigns left-to-right lanes so a commit list can draw a compact ancestry graph.
pub fn assign_commit_lanes(commits: &[GitCommit]) -> Vec<GitGraphRow> {
    #[derive(Clone)]
    struct Lane {
        commit: String,
        color: usize,
    }

    let mut lanes: Vec<Option<Lane>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    let mut next_color = 1;
    let mut remaining: std::collections::HashSet<&str> =
        commits.iter().map(|commit| commit.sha.as_str()).collect();

    for commit in commits {
        remaining.remove(commit.sha.as_str());
        let lane = match lanes.iter().position(|occupant| {
            occupant.as_ref().map(|lane| lane.commit.as_str()) == Some(commit.sha.as_str())
        }) {
            Some(index) => index,
            None => {
                let color = if lanes.iter().all(Option::is_none) {
                    0
                } else {
                    let color = next_color;
                    next_color += 1;
                    color
                };
                if let Some(index) = lanes.iter().position(Option::is_none) {
                    lanes[index] = Some(Lane {
                        commit: commit.sha.clone(),
                        color,
                    });
                    index
                } else {
                    lanes.push(Some(Lane {
                        commit: commit.sha.clone(),
                        color,
                    }));
                    lanes.len() - 1
                }
            }
        };
        let color = lanes[lane].as_ref().map_or(0, |lane| lane.color);

        let through: Vec<GitGraphRail> = lanes
            .iter()
            .enumerate()
            .filter_map(|(index, occupant)| {
                let occupant = occupant.as_ref()?;
                (index != lane).then_some(GitGraphRail {
                    lane: index,
                    color: occupant.color,
                })
            })
            .collect();

        let mut edges = Vec::new();
        let mut first_parent_edge = None;
        let mut continues = false;
        if commit.parents.is_empty() {
            lanes[lane] = None;
        } else {
            let first = &commit.parents[0];
            if remaining.contains(first.as_str()) {
                if let Some(existing) = lanes.iter().enumerate().find_map(|(index, occupant)| {
                    let occupant = occupant.as_ref()?;
                    (index != lane && occupant.commit == *first).then_some(GitGraphRail {
                        lane: index,
                        color: occupant.color,
                    })
                }) {
                    first_parent_edge = Some(existing);
                    lanes[lane] = None;
                } else {
                    lanes[lane] = Some(Lane {
                        commit: first.clone(),
                        color,
                    });
                    continues = true;
                }
            } else {
                lanes[lane] = None;
            }

            for parent in commit.parents.iter().skip(1) {
                if !remaining.contains(parent.as_str()) {
                    continue;
                }
                if let Some(existing) = lanes.iter().position(|occupant| {
                    occupant.as_ref().map(|lane| lane.commit.as_str()) == Some(parent.as_str())
                }) {
                    if existing != lane {
                        edges.push(GitGraphRail {
                            lane: existing,
                            color: lanes[existing].as_ref().map_or(0, |lane| lane.color),
                        });
                    }
                    continue;
                }
                let parent_color = next_color;
                next_color += 1;
                if let Some(index) = lanes.iter().position(Option::is_none) {
                    lanes[index] = Some(Lane {
                        commit: parent.clone(),
                        color: parent_color,
                    });
                    edges.push(GitGraphRail {
                        lane: index,
                        color: parent_color,
                    });
                } else {
                    lanes.push(Some(Lane {
                        commit: parent.clone(),
                        color: parent_color,
                    }));
                    edges.push(GitGraphRail {
                        lane: lanes.len() - 1,
                        color: parent_color,
                    });
                }
            }
        }

        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }

        rows.push(GitGraphRow {
            lane,
            color,
            lane_count: lanes.len().max(1),
            through,
            first_parent_edge,
            edges,
            continues,
        });
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(sha: &str, parents: &[&str]) -> GitCommit {
        GitCommit {
            sha: sha.to_owned(),
            short_sha: sha.to_owned(),
            subject: sha.to_owned(),
            author: "t".into(),
            date: "2026-01-01".into(),
            parents: parents.iter().map(|parent| (*parent).to_owned()).collect(),
        }
    }

    #[test]
    fn linear_history_stays_on_one_lane() {
        let rows =
            assign_commit_lanes(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
        assert!(rows.iter().all(|row| row.lane == 0 && row.lane_count == 1));
        assert!(rows.iter().all(|row| row.edges.is_empty()));
        assert!(rows[0].continues);
        assert!(rows[1].continues);
        assert!(!rows[2].continues);
    }

    #[test]
    fn merge_opens_a_side_lane_that_rejoins() {
        let rows = assign_commit_lanes(&[
            commit("m", &["a", "b"]),
            commit("b", &["a"]),
            commit("a", &[]),
        ]);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[0].edges[0].lane, 1);
        assert_eq!(rows[0].edges[0].color, 1);
        assert_eq!(rows[1].lane, 1);
        assert_eq!(rows[1].first_parent_edge.map(|edge| edge.lane), Some(0));
        assert!(!rows[1].continues);
        assert_eq!(rows[2].lane, 0);
    }

    #[test]
    fn off_page_merge_parent_does_not_keep_a_spare_lane() {
        let rows = assign_commit_lanes(&[commit("m", &["a", "missing"]), commit("a", &[])]);
        assert_eq!(rows[0].lane, 0);
        assert!(rows[0].edges.is_empty());
        assert_eq!(rows[1].lane, 0);
        assert_eq!(rows[1].lane_count, 1);
    }
}
