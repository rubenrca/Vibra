mod capture;
mod command;
mod parsing;
mod port;
mod refs;
use capture::*;
pub(crate) use command::*;
use parsing::{append_patch, apply_numstat, empty_diff_notice, file_status};
use refs::*;

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;

use crate::ports::git::{
    GitBranchSummary, GitCommitChanges, GitDiff, GitDiffRow, GitDiffRowKind, GitFileChange,
    GitRepositorySnapshot,
};

/// Line counts for untracked files stay local. A full `git diff --no-index` on
/// every sidebar poll would rebuild patches the list never shows.
pub(super) const MAX_UNTRACKED_STAT_BYTES: u64 = 8 * 1024 * 1024;
const BRANCH_SUMMARY_TTL: Duration = Duration::from_millis(1_500);

#[derive(Clone)]
struct CachedBranchSummary {
    summary: GitBranchSummary,
    fetched_at: Instant,
}

#[derive(Clone)]
pub(super) struct CachedUntrackedStat {
    len: u64,
    modified: SystemTime,
    changed_seconds: i64,
    changed_nanoseconds: i64,
    /// `None` for binary files and files above [`MAX_UNTRACKED_STAT_BYTES`].
    additions: Option<usize>,
}

#[derive(Default)]
pub(super) struct DiffAccumulator {
    pub(super) rows: Vec<GitDiffRow>,
    pub(super) additions: usize,
    pub(super) deletions: usize,
    pub(super) binary: bool,
    pub(super) truncated: bool,
}

impl DiffAccumulator {
    pub(super) fn append(&mut self, patch: &[u8], section: Option<&str>) {
        append_patch(patch, section, self);
    }

    pub(super) fn append_untracked(
        &mut self,
        root: &Path,
        path: &str,
        section: Option<&str>,
    ) -> Result<()> {
        if root.join(path).is_dir() {
            if let Some(section) = section {
                self.rows.push(GitDiffRow {
                    old_line: None,
                    new_line: None,
                    kind: GitDiffRowKind::Section,
                    text: section.to_owned(),
                });
            }
            self.rows.push(GitDiffRow {
                old_line: None,
                new_line: None,
                kind: GitDiffRowKind::Notice,
                text: "Untracked directory — no text diff.".into(),
            });
            return Ok(());
        }
        let patch = patch_from_empty(root, path, "git diff --no-index")?;
        self.append(&patch, section);
        Ok(())
    }

    pub(super) fn finish(mut self, path: String) -> GitDiff {
        if self.rows.is_empty() {
            self.rows.push(empty_diff_notice(self.binary));
        }
        self.into_diff(path)
    }

    pub(super) fn into_diff(self, path: String) -> GitDiff {
        GitDiff {
            path,
            rows: self.rows,
            additions: self.additions,
            deletions: self.deletions,
            binary: self.binary,
            truncated: self.truncated,
        }
    }
}

#[derive(Default)]
pub struct GitCliPort {
    branch_cache: Mutex<HashMap<PathBuf, CachedBranchSummary>>,
    untracked_stats: Mutex<HashMap<PathBuf, CachedUntrackedStat>>,
    capture_indexes: Mutex<HashMap<PathBuf, CachedCaptureSlot>>,
}

impl GitCliPort {
    fn cached_branch_summary(&self, root: &Path) -> Option<GitBranchSummary> {
        let cache = self
            .branch_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let entry = cache.get(root)?;
        (entry.fetched_at.elapsed() < BRANCH_SUMMARY_TTL).then(|| entry.summary.clone())
    }

    fn remember_branch_summary(&self, root: PathBuf, summary: GitBranchSummary) {
        let mut cache = self
            .branch_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cache.retain(|_, entry| entry.fetched_at.elapsed() < BRANCH_SUMMARY_TTL * 4);
        cache.insert(
            root,
            CachedBranchSummary {
                summary,
                fetched_at: Instant::now(),
            },
        );
    }

    fn forget_branch_summary(&self, root: &Path) {
        self.branch_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(root);
    }

    /// Fills `+N` for untracked text files so the list does not wait for the diff to open.
    fn fill_untracked_line_counts(&self, root: &Path, changes: &mut [GitFileChange]) {
        let mut cache = self
            .untracked_stats
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut seen = HashSet::new();
        for change in changes.iter_mut() {
            if !change.untracked {
                continue;
            }
            let full = root.join(&change.path);
            if !seen.insert(full.clone()) {
                continue;
            }
            let Some(additions) = cached_untracked_additions(&full, &mut cache) else {
                continue;
            };
            change.additions = Some(change.additions.unwrap_or(0) + additions);
            change.deletions.get_or_insert(0);
        }
        cache.retain(|path, _| !path.starts_with(root) || seen.contains(path));
    }
}

/// Every path that differs between two revisions, each reviewable on its own
/// (renames stay as a deletion plus an addition).
fn changes_between(
    root: &Path,
    base_revision: String,
    revision: String,
) -> Result<GitCommitChanges> {
    let root = root.to_path_buf();
    // Keep each path independently reviewable, including both sides of a rename.
    let output = run_git(
        &root,
        [
            "diff",
            "--name-status",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            &base_revision,
            &revision,
            "--",
        ],
    )?;
    ensure_success(&output, "git diff commit files")?;
    let fields: Vec<&[u8]> = output.stdout.split(|byte| *byte == 0).collect();
    let mut changes = Vec::new();
    for pair in fields.chunks_exact(2) {
        let status = pair[0].first().copied().unwrap_or(b'M') as char;
        changes.push(GitFileChange {
            path: String::from_utf8_lossy(pair[1]).into_owned(),
            old_path: None,
            status: file_status(status, ' '),
            staged: false,
            unstaged: false,
            untracked: false,
            additions: None,
            deletions: None,
        });
    }
    let output = run_git(
        &root,
        [
            "diff",
            "--numstat",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            &base_revision,
            &revision,
            "--",
        ],
    )?;
    ensure_success(&output, "git diff commit stats")?;
    let mut stats = HashMap::new();
    apply_numstat(&output.stdout, &mut stats);
    for change in &mut changes {
        if let Some(&(additions, deletions)) = stats.get(&change.path) {
            change.additions = Some(additions);
            change.deletions = Some(deletions);
        }
    }
    changes.sort_by_cached_key(|change| change.path.to_lowercase());
    let additions = changes.iter().filter_map(|change| change.additions).sum();
    let deletions = changes.iter().filter_map(|change| change.deletions).sum();
    Ok(GitCommitChanges {
        snapshot: GitRepositorySnapshot {
            root,
            branch: revision.clone(),
            changes,
            additions,
            deletions,
        },
        base_revision,
        revision,
    })
}

/// Largest whole file read for context-aware highlighting.
const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;

fn source_text(bytes: Vec<u8>) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn read_worktree_source(root: &Path, path: &str) -> Option<String> {
    let path = root.join(path);
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
        return None;
    }
    if !path
        .canonicalize()
        .ok()?
        .starts_with(root.canonicalize().ok()?)
    {
        return None;
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .ok()?;
    if file.metadata().ok()?.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_SOURCE_BYTES as usize)
        .then(|| source_text(bytes))
        .flatten()
}

/// `spec` is `<revision>:<path>`, or `:<path>` for the index.
fn read_blob_source(root: &Path, spec: &str) -> Option<String> {
    let size = run_git(root, ["cat-file", "-s", spec]).ok()?;
    if !size.status.success() {
        return None;
    }
    let size: u64 = String::from_utf8_lossy(&size.stdout).trim().parse().ok()?;
    if size > MAX_SOURCE_BYTES {
        return None;
    }
    let output = run_git(root, ["cat-file", "blob", spec]).ok()?;
    output.status.success().then_some(())?;
    source_text(output.stdout)
}

/// Parse a remote or local unified patch with the same bounds and line model.
pub(crate) fn parse_diff_patch(path: &str, patch: &[u8]) -> GitDiff {
    let mut diff = DiffAccumulator::default();
    diff.append(patch, None);
    diff.into_diff(path.into())
}

#[cfg(test)]
mod tests;
