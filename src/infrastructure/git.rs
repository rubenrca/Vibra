use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result, bail};

use crate::ports::git::{
    GitBranchChanges, GitBranchRef, GitBranchSummary, GitCommit, GitCommitChanges, GitDiff,
    GitDiffRow, GitDiffRowKind, GitDiffSources, GitFileChange, GitFileStatus, GitHistory, GitPort,
    GitRepositorySnapshot, GitWorktreeCapture,
};

const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;
/// Line counts for untracked files stay local. A full `git diff --no-index` on
/// every sidebar poll would rebuild patches the list never shows.
const MAX_UNTRACKED_STAT_BYTES: u64 = 8 * 1024 * 1024;
const BRANCH_SUMMARY_TTL: Duration = Duration::from_millis(1_500);

#[derive(Clone)]
struct CachedBranchSummary {
    summary: GitBranchSummary,
    fetched_at: Instant,
}

#[derive(Clone)]
struct CachedUntrackedStat {
    len: u64,
    modified: SystemTime,
    /// `None` for binary files and files above [`MAX_UNTRACKED_STAT_BYTES`].
    additions: Option<usize>,
}

#[derive(Default)]
pub struct GitCliPort {
    branch_cache: Mutex<HashMap<PathBuf, CachedBranchSummary>>,
    untracked_stats: Mutex<HashMap<PathBuf, CachedUntrackedStat>>,
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
            change.additions = Some(additions);
            change.deletions = Some(0);
        }
        cache.retain(|path, _| !path.starts_with(root) || seen.contains(path));
    }
}

impl GitPort for GitCliPort {
    fn snapshot(&self, root: &Path) -> Result<Option<GitRepositorySnapshot>> {
        let Some(root) = repository_root(root)? else {
            return Ok(None);
        };
        let output = run_git(
            &root,
            [
                "status",
                "--porcelain=v1",
                "-z",
                "--branch",
                "--untracked-files=all",
            ],
        )?;
        ensure_success(&output, "git status")?;
        let PorcelainStatus {
            branch,
            ahead,
            behind,
            files,
        } = parse_porcelain_status(&output.stdout);
        let mut changes = Vec::with_capacity(files.len());
        for file in files {
            let untracked = file.untracked();
            changes.push(GitFileChange {
                status: file_status(file.index, file.worktree),
                staged: !untracked && file.index != ' ',
                unstaged: !untracked && file.worktree != ' ',
                untracked,
                path: file.path,
                additions: None,
                deletions: None,
            });
        }

        let mut stats = HashMap::<String, (usize, usize)>::new();
        collect_numstat(&root, false, &mut stats)?;
        collect_numstat(&root, true, &mut stats)?;
        for change in &mut changes {
            if change.untracked {
                continue;
            }
            if let Some((additions, deletions)) = stats.get(&change.path) {
                change.additions = Some(*additions);
                change.deletions = Some(*deletions);
            }
        }
        self.fill_untracked_line_counts(&root, &mut changes);
        changes.sort_by(|left, right| {
            change_priority(left)
                .cmp(&change_priority(right))
                .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
        });
        let additions = changes.iter().filter_map(|change| change.additions).sum();
        let deletions = changes.iter().filter_map(|change| change.deletions).sum();
        self.remember_branch_summary(
            root.clone(),
            GitBranchSummary {
                branch: branch.clone(),
                ahead,
                behind,
                dirty: !changes.is_empty(),
            },
        );
        Ok(Some(GitRepositorySnapshot {
            root,
            branch,
            changes,
            additions,
            deletions,
        }))
    }

    fn branch_summary(&self, root: &Path) -> Result<Option<GitBranchSummary>> {
        let Some(root) = repository_root(root)? else {
            return Ok(None);
        };
        if let Some(cached) = self.cached_branch_summary(&root) {
            return Ok(Some(cached));
        }
        // Porcelain without numstat/diff: enough for branch, tracking counts, and dirty.
        let output = run_git(
            &root,
            [
                "status",
                "--porcelain=v1",
                "-z",
                "--branch",
                "--untracked-files=normal",
            ],
        )?;
        ensure_success(&output, "git status")?;
        let porcelain = parse_porcelain_status(&output.stdout);
        let summary = GitBranchSummary {
            branch: porcelain.branch,
            ahead: porcelain.ahead,
            behind: porcelain.behind,
            dirty: !porcelain.files.is_empty(),
        };
        self.remember_branch_summary(root, summary.clone());
        Ok(Some(summary))
    }

    fn diff(&self, repository: &Path, change: &GitFileChange) -> Result<GitDiff> {
        validate_relative_path(&change.path)?;
        let root = repository_root(repository)?.context("the repository is no longer available")?;
        let multiple_sections = change.staged && (change.unstaged || change.untracked);
        let mut rows = Vec::new();
        let mut additions = 0;
        let mut deletions = 0;
        let mut binary = false;
        let mut truncated = false;

        if change.staged {
            let patch = run_git_diff(
                &root,
                [
                    "diff",
                    "--cached",
                    "--no-ext-diff",
                    "--no-color",
                    "--unified=3",
                    "--",
                    &change.path,
                ],
                "git diff --cached",
                false,
            )?;
            append_patch(
                &patch,
                multiple_sections.then_some("STAGED CHANGES"),
                &mut rows,
                &mut additions,
                &mut deletions,
                &mut binary,
                &mut truncated,
            );
        }

        if change.unstaged {
            let patch = run_git_diff(
                &root,
                [
                    "diff",
                    "--no-ext-diff",
                    "--no-color",
                    "--unified=3",
                    "--",
                    &change.path,
                ],
                "git diff",
                false,
            )?;
            append_patch(
                &patch,
                multiple_sections.then_some("WORKING TREE"),
                &mut rows,
                &mut additions,
                &mut deletions,
                &mut binary,
                &mut truncated,
            );
        }

        if change.untracked {
            let untracked_path = root.join(&change.path);
            if untracked_path.is_dir() {
                if multiple_sections {
                    rows.push(GitDiffRow {
                        old_line: None,
                        new_line: None,
                        kind: GitDiffRowKind::Section,
                        text: "UNTRACKED".into(),
                    });
                }
                rows.push(GitDiffRow {
                    old_line: None,
                    new_line: None,
                    kind: GitDiffRowKind::Notice,
                    text: "Untracked directory — no text diff.".into(),
                });
            } else {
                let patch = run_git_diff(
                    &root,
                    [
                        "--literal-pathspecs",
                        "diff",
                        "--no-index",
                        "--no-ext-diff",
                        "--no-color",
                        "--unified=3",
                        "--",
                        "/dev/null",
                        &change.path,
                    ],
                    "git diff --no-index",
                    true,
                )?;
                append_patch(
                    &patch,
                    multiple_sections.then_some("UNTRACKED"),
                    &mut rows,
                    &mut additions,
                    &mut deletions,
                    &mut binary,
                    &mut truncated,
                );
            }
        }

        if rows.is_empty() {
            rows.push(empty_diff_notice(binary));
        }

        Ok(GitDiff {
            path: change.path.clone(),
            rows,
            additions,
            deletions,
            binary,
            truncated,
        })
    }

    fn branches(&self, root: &Path) -> Result<Vec<GitBranchRef>> {
        let Some(root) = repository_root(root)? else {
            return Ok(Vec::new());
        };
        let output = run_git(
            &root,
            [
                "for-each-ref",
                "--format=%(refname)%09%(symref)",
                "refs/heads",
                "refs/remotes",
            ],
        )?;
        ensure_success(&output, "git list branches")?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let (reference, symbolic) = line.split_once('\t')?;
                if !symbolic.is_empty() {
                    return None;
                }
                let remote = reference.starts_with("refs/remotes/");
                let name = reference.strip_prefix(if remote {
                    "refs/remotes/"
                } else {
                    "refs/heads/"
                })?;
                Some(GitBranchRef {
                    reference: reference.into(),
                    name: name.into(),
                    remote,
                })
            })
            .collect())
    }

    fn branch_changes(
        &self,
        root: &Path,
        base: Option<&str>,
        head: Option<&str>,
    ) -> Result<Option<GitBranchChanges>> {
        let Some(root) = repository_root(root)? else {
            return Ok(None);
        };
        let Some(summary) = self.branch_summary(&root)? else {
            return Ok(None);
        };
        let explicit_base = base.is_some();
        let selected_head = head
            .map(|reference| resolve_commit(&root, reference))
            .transpose()?;
        let base = match base {
            Some(reference) => Some(reference.to_owned()),
            None => compare_base(&root, &summary.branch)?,
        };
        let Some(base) = base else {
            return Ok(Some(GitBranchChanges {
                snapshot: GitRepositorySnapshot {
                    root,
                    branch: summary.branch,
                    changes: Vec::new(),
                    additions: 0,
                    deletions: 0,
                },
                base: String::new(),
                base_revision: String::new(),
                head_revision: selected_head,
                commits_ahead: 0,
            }));
        };
        let base_revision = if selected_head.is_some() || explicit_base {
            resolve_commit(&root, &base)?
        } else {
            merge_base(&root, &base)?
        };
        let mut changes = diff_name_status(&root, &base_revision, selected_head.as_deref())?;
        let mut stats = HashMap::<String, (usize, usize)>::new();
        collect_numstat_against(&root, &base_revision, selected_head.as_deref(), &mut stats)?;
        for change in &mut changes {
            if let Some((additions, deletions)) = stats.get(&change.path) {
                change.additions = Some(*additions);
                change.deletions = Some(*deletions);
            }
        }
        let seen: HashMap<String, ()> = changes
            .iter()
            .map(|change| (change.path.clone(), ()))
            .collect();
        for path in if selected_head.is_none() {
            untracked_paths(&root)?
        } else {
            Vec::new()
        } {
            if !seen.contains_key(&path) {
                changes.push(GitFileChange {
                    status: GitFileStatus::Untracked,
                    staged: false,
                    unstaged: false,
                    untracked: true,
                    path,
                    additions: None,
                    deletions: None,
                });
            }
        }
        self.fill_untracked_line_counts(&root, &mut changes);
        changes.sort_by(|left, right| {
            change_priority(left)
                .cmp(&change_priority(right))
                .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
        });
        let additions = changes.iter().filter_map(|change| change.additions).sum();
        let deletions = changes.iter().filter_map(|change| change.deletions).sum();
        let commits_ahead = rev_count(
            &root,
            &format!(
                "{base_revision}..{}",
                selected_head.as_deref().unwrap_or("HEAD")
            ),
        )?;
        Ok(Some(GitBranchChanges {
            snapshot: GitRepositorySnapshot {
                root,
                branch: head
                    .map(display_branch_ref)
                    .unwrap_or(&summary.branch)
                    .to_owned(),
                changes,
                additions,
                deletions,
            },
            base: display_branch_ref(&base).to_owned(),
            base_revision,
            head_revision: selected_head,
            commits_ahead,
        }))
    }

    fn history(&self, root: &Path, limit: usize) -> Result<Option<GitHistory>> {
        let Some(root) = repository_root(root)? else {
            return Ok(None);
        };
        let limit = limit.clamp(1, 500);
        let branch = current_branch(&root)?;
        let head = rev_parse(&root, "HEAD")?.unwrap_or_default();
        let total = rev_count(&root, "HEAD").unwrap_or(0);
        let output = run_git(
            &root,
            [
                "log",
                &format!("-{limit}"),
                "--topo-order",
                "--pretty=format:%H%x1f%h%x1f%s%x1f%an%x1f%as%x1f%P",
            ],
        )?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("does not have any commits")
                || stderr.contains("bad default revision")
                || stderr.contains("unknown revision")
            {
                return Ok(Some(GitHistory {
                    branch,
                    head,
                    total: 0,
                    commits: Vec::new(),
                    truncated: false,
                }));
            }
            ensure_success(&output, "git log")?;
        }
        let commits = parse_history(&String::from_utf8_lossy(&output.stdout));
        let truncated = total > commits.len();
        Ok(Some(GitHistory {
            branch,
            head,
            total,
            commits,
            truncated,
        }))
    }

    fn commit_changes(&self, root: &Path, revision: &str) -> Result<GitCommitChanges> {
        let root = repository_root(root)?.context("the repository is no longer available")?;
        validate_revision(revision)?;
        let revision = rev_parse(&root, &format!("{revision}^{{commit}}"))?
            .context("the selected commit is no longer available")?;
        let base_revision = match rev_parse(&root, &format!("{revision}^1"))? {
            Some(parent) => parent,
            None => {
                // Compute the empty tree for this repository's object format without
                // writing an object or touching the index/worktree.
                let output = run_git(&root, ["hash-object", "-t", "tree", "--stdin"])?;
                ensure_success(&output, "git hash-object empty tree")?;
                String::from_utf8_lossy(&output.stdout).trim().to_owned()
            }
        };
        changes_between(&root, base_revision, revision)
    }

    fn diff_sources(
        &self,
        repository: &Path,
        change: &GitFileChange,
        against: Option<&str>,
        head: Option<&str>,
    ) -> Result<GitDiffSources> {
        validate_relative_path(&change.path)?;
        let against = against.filter(|revision| !revision.is_empty());
        if let Some(revision) = against {
            validate_revision(revision)?;
        }
        if let Some(head) = head {
            validate_revision(head)?;
        }
        let root = repository_root(repository)?.context("the repository is no longer available")?;
        let path = change.path.as_str();
        let worktree = || read_worktree_source(&root.join(path));
        let blob = |revision: &str| read_blob_source(&root, &format!("{revision}:{path}"));
        let deleted = change.status == GitFileStatus::Deleted;
        let added = matches!(
            change.status,
            GitFileStatus::Added | GitFileStatus::Untracked
        );

        // Mirror the sides `diff` / `diff_against` compare.
        let sources = match (against, head) {
            (Some(revision), Some(head)) => GitDiffSources {
                old: blob(revision),
                new: blob(head),
            },
            (Some(revision), None) if !change.untracked => GitDiffSources {
                old: blob(revision),
                new: worktree(),
            },
            _ if change.untracked => GitDiffSources {
                old: None,
                new: worktree(),
            },
            // Staged and worktree sections have different bases; per-row
            // highlighting stays correct for both.
            _ if change.staged && change.unstaged => GitDiffSources::default(),
            _ if change.staged => GitDiffSources {
                old: (!added).then(|| blob("HEAD")).flatten(),
                new: (!deleted).then(|| blob("")).flatten(),
            },
            _ => GitDiffSources {
                old: blob(""),
                new: (!deleted).then(worktree).flatten(),
            },
        };
        Ok(sources)
    }

    fn capture_worktree(&self, root: &Path) -> Result<Option<GitWorktreeCapture>> {
        let Some(root) = repository_root(root)? else {
            return Ok(None);
        };
        // Stage everything into a throwaway copy of the index: the stat cache
        // in the copy keeps unchanged files from being hashed again, and the
        // real index, refs, and stash stay untouched.
        let index = git_path(&root, "index")?;
        let temporary = TemporaryIndex(std::env::temp_dir().join(format!(
            "vibra-turn-index-{}",
            uuid::Uuid::new_v4().simple()
        )));
        if index.is_file() {
            let _ = std::fs::copy(&index, &temporary.0);
        }
        let output = run_git_with_index(
            &root,
            &temporary.0,
            ["add", "--all", "--ignore-errors", "--", "."],
        )?;
        ensure_success(&output, "git add (turn snapshot)")?;
        let output = run_git_with_index(&root, &temporary.0, ["write-tree"])?;
        ensure_success(&output, "git write-tree")?;
        let tree = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        validate_revision(&tree)?;
        if tree.is_empty() {
            bail!("git write-tree returned no tree");
        }
        Ok(Some(GitWorktreeCapture { root, tree }))
    }

    fn tree_changes(&self, root: &Path, base: &str, head: &str) -> Result<GitCommitChanges> {
        let root = repository_root(root)?.context("the repository is no longer available")?;
        validate_revision(base)?;
        validate_revision(head)?;
        if base.is_empty() || head.is_empty() {
            bail!("Select two revisions to compare");
        }
        changes_between(&root, base.to_owned(), head.to_owned())
    }

    fn diff_against(
        &self,
        repository: &Path,
        revision: &str,
        head: Option<&str>,
        change: &GitFileChange,
    ) -> Result<GitDiff> {
        validate_relative_path(&change.path)?;
        validate_revision(revision)?;
        if let Some(head) = head {
            validate_revision(head)?;
        }
        if head.is_none() && (change.untracked || revision.is_empty()) {
            return self.diff(repository, change);
        }
        let root = repository_root(repository)?.context("the repository is no longer available")?;
        let mut rows = Vec::new();
        let mut additions = 0;
        let mut deletions = 0;
        let mut binary = false;
        let mut truncated = false;
        let mut args = vec![
            "--literal-pathspecs",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--unified=3",
            revision,
        ];
        args.extend(head);
        args.extend(["--", &change.path]);
        let patch = run_git_diff(&root, args, "git diff revision", false)?;
        append_patch(
            &patch,
            None,
            &mut rows,
            &mut additions,
            &mut deletions,
            &mut binary,
            &mut truncated,
        );
        if rows.is_empty() {
            rows.push(empty_diff_notice(binary));
        }
        Ok(GitDiff {
            path: change.path.clone(),
            rows,
            additions,
            deletions,
            binary,
            truncated,
        })
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
            &base_revision,
            &revision,
            "--",
        ],
    )?;
    ensure_success(&output, "git diff commit stats")?;
    let stats: HashMap<String, (usize, usize)> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|record| {
            let text = String::from_utf8_lossy(record);
            let mut fields = text.splitn(3, '\t');
            let additions = fields.next()?.parse().ok()?;
            let deletions = fields.next()?.parse().ok()?;
            Some((fields.next()?.to_owned(), (additions, deletions)))
        })
        .collect();
    for change in &mut changes {
        if let Some(&(additions, deletions)) = stats.get(&change.path) {
            change.additions = Some(additions);
            change.deletions = Some(deletions);
        }
    }
    changes.sort_by_key(|change| change.path.to_lowercase());
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

struct TemporaryIndex(PathBuf);

impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("lock"));
    }
}

/// Path Git uses for `name` inside the repository's Git directory.
fn git_path(root: &Path, name: &str) -> Result<PathBuf> {
    let output = run_git(root, ["rev-parse", "--git-path", name])?;
    ensure_success(&output, "git rev-parse --git-path")?;
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    Ok(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

fn run_git_with_index<I, S>(root: &Path, index: &Path, arguments: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    git_command()
        .env("GIT_INDEX_FILE", index)
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .with_context(|| format!("failed to run Git in {}", root.display()))
}

/// Largest whole file read for context-aware highlighting.
const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;

fn source_text(bytes: Vec<u8>) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn read_worktree_source(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
        return None;
    }
    source_text(std::fs::read(path).ok()?)
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

fn repository_root(root: &Path) -> Result<Option<PathBuf>> {
    if !root.is_dir() {
        return Ok(None);
    }
    let output = run_git(root, ["rev-parse", "--show-toplevel"])?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        return Ok((!path.is_empty()).then(|| PathBuf::from(path)));
    }
    if is_not_a_repository(&output.stderr) {
        return Ok(None);
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    bail!("git rev-parse failed: {message}")
}

/// Dock-launched macOS apps only get `/usr/bin:/bin:/usr/sbin:/sbin`. Apple's
/// `/usr/bin/git` is an Xcode stub that fails (license, missing CLT) even when
/// Homebrew Git is installed. Prefer a Git that can actually run.
fn git_program() -> &'static Path {
    static GIT: OnceLock<PathBuf> = OnceLock::new();
    GIT.get_or_init(discover_git)
}

fn discover_git() -> PathBuf {
    discover_git_with(
        std::env::var_os("VIBRA_GIT").map(PathBuf::from),
        std::env::var_os("PATH"),
        extra_git_candidates(),
    )
}

fn extra_git_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/opt/homebrew/bin/git"),
        PathBuf::from("/usr/local/bin/git"),
        PathBuf::from("/usr/local/git/bin/git"),
        PathBuf::from("/opt/local/bin/git"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".local/bin/git"));
        candidates.push(home.join("bin/git"));
    }
    candidates
}

fn discover_git_with(
    override_path: Option<PathBuf>,
    path: Option<OsString>,
    extras: Vec<PathBuf>,
) -> PathBuf {
    if let Some(path) = override_path
        && git_is_usable(&path)
    {
        return path;
    }

    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    if let Some(path) = path {
        for directory in std::env::split_paths(&path) {
            candidates.push(directory.join("git"));
        }
    }
    candidates.extend(extras);

    for candidate in &candidates {
        if !seen.insert(candidate.clone()) || is_deferred_system_git(candidate) {
            continue;
        }
        if git_is_usable(candidate) {
            return candidate.clone();
        }
    }
    seen.clear();
    for candidate in &candidates {
        if !seen.insert(candidate.clone()) {
            continue;
        }
        if git_is_usable(candidate) {
            return candidate.clone();
        }
    }
    PathBuf::from("git")
}

/// Apple's `/usr/bin/git` is often a license/CLT shim. Try it last so a
/// Homebrew or `/usr/local` Git is used when the GUI PATH hides it.
#[cfg(target_os = "macos")]
fn is_deferred_system_git(path: &Path) -> bool {
    path == Path::new("/usr/bin/git")
}

#[cfg(not(target_os = "macos"))]
fn is_deferred_system_git(_: &Path) -> bool {
    false
}

fn git_is_usable(path: &Path) -> bool {
    if path.components().count() > 1 && !path.is_file() {
        return false;
    }
    Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn git_command() -> Command {
    let mut command = Command::new(git_program());
    command
        .arg("-c")
        .arg("core.quotepath=false")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C");
    command
}

fn is_not_a_repository(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr)
        .to_ascii_lowercase()
        .contains("not a git repository")
}

fn run_git<I, S>(root: &Path, arguments: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    git_command()
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .with_context(|| format!("failed to run Git in {}", root.display()))
}

fn run_git_diff<I, S>(
    root: &Path,
    arguments: I,
    operation: &str,
    allow_difference: bool,
) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut child = git_command()
        .arg("-C")
        .arg(root)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run Git in {}", root.display()))?;
    let stdout = child.stdout.take().context("Git did not open stdout")?;
    let mut stderr = child.stderr.take().context("Git did not open stderr")?;
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes)?;
        Ok::<_, std::io::Error>(bytes)
    });

    let mut patch = Vec::with_capacity(MAX_DIFF_BYTES.min(64 * 1024));
    let read_result = stdout
        .take((MAX_DIFF_BYTES + 1) as u64)
        .read_to_end(&mut patch);
    let reached_limit = patch.len() > MAX_DIFF_BYTES;
    if reached_limit || read_result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("Git stderr reader failed"))??;
    read_result?;

    if !(reached_limit || status.success() || allow_difference && status.code() == Some(1)) {
        ensure_success(
            &Output {
                status,
                stdout: Vec::new(),
                stderr,
            },
            operation,
        )?;
    }
    Ok(patch)
}

fn ensure_success(output: &Output, operation: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    bail!("{operation} failed: {message}")
}

fn parse_porcelain_status(stdout: &[u8]) -> PorcelainStatus {
    let mut records = stdout.split(|byte| *byte == 0).peekable();
    let mut status = PorcelainStatus::default();
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        let record = String::from_utf8_lossy(record);
        if let Some(header) = record.strip_prefix("## ") {
            (status.branch, status.ahead, status.behind) = parse_branch_header(header);
            continue;
        }
        if record.len() < 3 {
            continue;
        }
        let bytes = record.as_bytes();
        let index = bytes[0] as char;
        let worktree = bytes[1] as char;
        if index == '!' && worktree == '!' {
            continue;
        }
        if matches!(index, 'R' | 'C') || matches!(worktree, 'R' | 'C') {
            let _ = records.next();
        }
        let path = record[3..].trim_end_matches('/').to_owned();
        if path.is_empty() {
            continue;
        }
        status.files.push(PorcelainFile {
            index,
            worktree,
            path,
        });
    }
    status
}

#[derive(Debug)]
struct PorcelainStatus {
    branch: String,
    ahead: usize,
    behind: usize,
    files: Vec<PorcelainFile>,
}

impl Default for PorcelainStatus {
    fn default() -> Self {
        Self {
            branch: "HEAD".to_owned(),
            ahead: 0,
            behind: 0,
            files: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct PorcelainFile {
    index: char,
    worktree: char,
    path: String,
}

impl PorcelainFile {
    fn untracked(&self) -> bool {
        self.index == '?' && self.worktree == '?'
    }
}

fn empty_diff_notice(binary: bool) -> GitDiffRow {
    GitDiffRow {
        old_line: None,
        new_line: None,
        kind: GitDiffRowKind::Notice,
        text: if binary {
            "Binary file — no text diff.".into()
        } else {
            "Git returned no textual changes for this file.".into()
        },
    }
}

fn parse_branch_header(header: &str) -> (String, usize, usize) {
    let (relation, tracking) = header
        .rsplit_once(" [")
        .map(|(relation, tracking)| (relation, tracking.trim_end_matches(']')))
        .unwrap_or((header, ""));
    let relation = relation
        .strip_prefix("No commits yet on ")
        .or_else(|| relation.strip_prefix("Initial commit on "))
        .unwrap_or(relation);
    let branch = relation
        .split_once("...")
        .map(|(branch, _)| branch)
        .unwrap_or(relation);
    let branch = if branch == "HEAD (no branch)" {
        "detached".to_owned()
    } else {
        branch.to_owned()
    };
    let mut ahead = 0;
    let mut behind = 0;
    for item in tracking.split(',').map(str::trim) {
        if let Some(value) = item.strip_prefix("ahead ") {
            ahead = value.parse().unwrap_or_default();
        } else if let Some(value) = item.strip_prefix("behind ") {
            behind = value.parse().unwrap_or_default();
        }
    }
    (branch, ahead, behind)
}

fn file_status(index: char, worktree: char) -> GitFileStatus {
    if index == '?' && worktree == '?' {
        GitFileStatus::Untracked
    } else if matches!(
        (index, worktree),
        ('D', 'D') | ('A', 'U') | ('U', 'D') | ('U', 'A') | ('D', 'U') | ('A', 'A') | ('U', 'U')
    ) {
        GitFileStatus::Conflicted
    } else if matches!(index, 'R') || matches!(worktree, 'R') {
        GitFileStatus::Renamed
    } else if matches!(index, 'C') || matches!(worktree, 'C') {
        GitFileStatus::Copied
    } else if matches!(index, 'D') || matches!(worktree, 'D') {
        GitFileStatus::Deleted
    } else if matches!(index, 'A') || matches!(worktree, 'A') {
        GitFileStatus::Added
    } else if matches!(index, 'T') || matches!(worktree, 'T') {
        GitFileStatus::TypeChanged
    } else {
        GitFileStatus::Modified
    }
}

fn change_priority(change: &GitFileChange) -> u8 {
    match change.status {
        GitFileStatus::Conflicted => 0,
        _ if change.staged => 1,
        GitFileStatus::Modified | GitFileStatus::TypeChanged => 2,
        GitFileStatus::Added | GitFileStatus::Untracked => 3,
        GitFileStatus::Renamed | GitFileStatus::Copied => 4,
        GitFileStatus::Deleted => 5,
    }
}

fn untracked_paths(root: &Path) -> Result<Vec<String>> {
    // Match worktree snapshot: list files inside untracked directories, not the
    // directory itself. `ls-files --directory` collapsed new folders to `dir/`,
    // which then failed `git diff --no-index`.
    let output = run_git(
        root,
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    ensure_success(&output, "git status")?;
    Ok(parse_porcelain_status(&output.stdout)
        .files
        .into_iter()
        .filter(PorcelainFile::untracked)
        .map(|file| file.path)
        .collect())
}

fn collect_numstat(
    root: &Path,
    cached: bool,
    stats: &mut HashMap<String, (usize, usize)>,
) -> Result<()> {
    let mut arguments = vec!["diff"];
    if cached {
        arguments.push("--cached");
    }
    arguments.extend(["--no-ext-diff", "--numstat"]);
    let output = run_git(root, arguments)?;
    ensure_success(&output, "git diff --numstat")?;
    apply_numstat(&output.stdout, stats);
    Ok(())
}

fn validate_revision(revision: &str) -> Result<()> {
    if revision.is_empty() {
        return Ok(());
    }
    if revision.starts_with('-')
        || revision.contains('\0')
        || revision.contains(char::is_whitespace)
    {
        bail!("Git returned an unsafe revision");
    }
    Ok(())
}

fn display_branch_ref(reference: &str) -> &str {
    reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/remotes/"))
        .unwrap_or(reference)
}

fn resolve_commit(root: &Path, reference: &str) -> Result<String> {
    validate_revision(reference)?;
    if reference.is_empty() {
        bail!("Select a branch to compare");
    }
    rev_parse(root, &format!("{reference}^{{commit}}"))?
        .with_context(|| format!("Branch not found: {reference}"))
}

fn current_branch(root: &Path) -> Result<String> {
    let output = run_git(root, ["rev-parse", "--abbrev-ref", "HEAD"])?;
    if !output.status.success() {
        return Ok("HEAD".into());
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(if name.is_empty() || name == "HEAD" {
        "detached".into()
    } else {
        name
    })
}

fn rev_parse(root: &Path, rev: &str) -> Result<Option<String>> {
    validate_revision(rev)?;
    let output = run_git(root, ["rev-parse", "--verify", "--quiet", rev])?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!value.is_empty()).then_some(value))
}

fn rev_count(root: &Path, range: &str) -> Result<usize> {
    let output = run_git(root, ["rev-list", "--count", range])?;
    if !output.status.success() {
        return Ok(0);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0))
}

fn merge_base(root: &Path, other: &str) -> Result<String> {
    validate_revision(other)?;
    let output = run_git(root, ["merge-base", "HEAD", other])?;
    ensure_success(&output, "git merge-base")?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if value.is_empty() {
        bail!("Git found no common ancestor with {other}");
    }
    Ok(value)
}

fn short_ref_name(rev: &str) -> &str {
    rev.rsplit('/').next().unwrap_or(rev)
}

fn compare_base(root: &Path, branch: &str) -> Result<Option<String>> {
    let default_base = default_base_branch(root)?;
    let upstream = rev_parse_symbolic(root, "@{upstream}")?;
    let on_default = default_base
        .as_deref()
        .is_some_and(|base| short_ref_name(base) == branch);
    if let Some(default_base) = default_base.as_ref()
        && !on_default
    {
        return Ok(Some(default_base.clone()));
    }
    if let Some(upstream) = upstream {
        return Ok(Some(upstream));
    }
    Ok(default_base)
}

fn default_base_branch(root: &Path) -> Result<Option<String>> {
    if let Some(symbolic) = rev_parse_symbolic(root, "refs/remotes/origin/HEAD")? {
        let name = symbolic
            .strip_prefix("refs/remotes/")
            .unwrap_or(&symbolic)
            .to_owned();
        if rev_parse(root, &name)?.is_some() {
            return Ok(Some(name));
        }
    }
    for candidate in [
        "origin/main",
        "origin/master",
        "origin/develop",
        "main",
        "master",
        "develop",
    ] {
        if rev_parse(root, candidate)?.is_some() {
            return Ok(Some(candidate.to_owned()));
        }
    }
    Ok(None)
}

fn rev_parse_symbolic(root: &Path, rev: &str) -> Result<Option<String>> {
    let output = run_git(
        root,
        ["rev-parse", "--abbrev-ref", "--symbolic-full-name", rev],
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!value.is_empty() && value != "HEAD").then_some(value))
}

fn diff_name_status(root: &Path, revision: &str, head: Option<&str>) -> Result<Vec<GitFileChange>> {
    validate_revision(revision)?;
    let mut args = vec![
        "diff",
        "--name-status",
        "--no-ext-diff",
        "--find-renames",
        revision,
    ];
    args.extend(head);
    args.push("--");
    let output = run_git(root, args)?;
    ensure_success(&output, "git diff --name-status")?;
    Ok(parse_name_status(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_name_status(output: &str) -> Vec<GitFileChange> {
    let mut changes = Vec::new();
    for line in output.lines() {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        let Some(code) = fields.next() else {
            continue;
        };
        let status = match code.chars().next().unwrap_or('M') {
            'A' => GitFileStatus::Added,
            'D' => GitFileStatus::Deleted,
            'R' => GitFileStatus::Renamed,
            'C' => GitFileStatus::Copied,
            'T' => GitFileStatus::TypeChanged,
            'U' => GitFileStatus::Conflicted,
            _ => GitFileStatus::Modified,
        };
        let path = if matches!(status, GitFileStatus::Renamed | GitFileStatus::Copied) {
            let _old = fields.next();
            fields.next().unwrap_or_default().to_owned()
        } else {
            fields.next().unwrap_or_default().to_owned()
        };
        if path.is_empty() {
            continue;
        }
        changes.push(GitFileChange {
            status,
            staged: false,
            unstaged: true,
            untracked: false,
            path,
            additions: None,
            deletions: None,
        });
    }
    changes
}

fn collect_numstat_against(
    root: &Path,
    revision: &str,
    head: Option<&str>,
    stats: &mut HashMap<String, (usize, usize)>,
) -> Result<()> {
    validate_revision(revision)?;
    let mut args = vec!["diff", "--no-ext-diff", "--numstat", revision];
    args.extend(head);
    args.push("--");
    let output = run_git(root, args)?;
    ensure_success(&output, "git diff --numstat")?;
    apply_numstat(&output.stdout, stats);
    Ok(())
}

fn cached_untracked_additions(
    path: &Path,
    cache: &mut HashMap<PathBuf, CachedUntrackedStat>,
) -> Option<usize> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let len = meta.len();
    let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    if let Some(cached) = cache.get(path)
        && cached.len == len
        && cached.modified == modified
    {
        return cached.additions;
    }
    let additions = match read_untracked_additions(path, len) {
        Ok(additions) => additions,
        Err(_) => return None,
    };
    cache.insert(
        path.to_path_buf(),
        CachedUntrackedStat {
            len,
            modified,
            additions,
        },
    );
    additions
}

fn read_untracked_additions(path: &Path, len: u64) -> std::io::Result<Option<usize>> {
    if len > MAX_UNTRACKED_STAT_BYTES {
        return Ok(None);
    }
    let mut file = std::fs::File::open(path)?;
    let mut bytes = vec![0u8; len as usize];
    file.read_exact(&mut bytes)?;
    Ok(text_additions(&bytes))
}

/// Additions `git diff --no-index` would report for a brand-new text file.
/// Binary files (NUL in the first 8 KiB, matching Git) stay `None`.
fn text_additions(bytes: &[u8]) -> Option<usize> {
    if bytes.iter().take(8_000).any(|byte| *byte == 0) {
        return None;
    }
    if bytes.is_empty() {
        return Some(0);
    }
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count();
    if bytes.last() == Some(&b'\n') {
        Some(newlines)
    } else {
        Some(newlines + 1)
    }
}

fn apply_numstat(stdout: &[u8], stats: &mut HashMap<String, (usize, usize)>) {
    for line in String::from_utf8_lossy(stdout).lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(additions), Some(deletions), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(additions), Ok(deletions)) =
            (additions.parse::<usize>(), deletions.parse::<usize>())
        else {
            continue;
        };
        let entry = stats.entry(path.to_owned()).or_default();
        entry.0 += additions;
        entry.1 += deletions;
    }
}

fn parse_history(output: &str) -> Vec<GitCommit> {
    let mut commits = Vec::new();
    for line in output.lines() {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('\u{1f}');
        let (Some(sha), Some(short_sha), Some(subject), Some(author), Some(date), Some(parents)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        commits.push(GitCommit {
            sha: sha.to_owned(),
            short_sha: short_sha.to_owned(),
            subject: subject.to_owned(),
            author: author.to_owned(),
            date: date.to_owned(),
            parents: parents.split_whitespace().map(str::to_owned).collect(),
        });
    }
    commits
}

fn validate_relative_path(path: &str) -> Result<()> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        bail!("Git returned an unsafe path")
    }
    Ok(())
}

fn append_patch(
    bytes: &[u8],
    section: Option<&str>,
    rows: &mut Vec<GitDiffRow>,
    additions: &mut usize,
    deletions: &mut usize,
    binary: &mut bool,
    truncated: &mut bool,
) {
    if bytes.is_empty() {
        return;
    }
    if let Some(section) = section {
        rows.push(GitDiffRow {
            old_line: None,
            new_line: None,
            kind: GitDiffRowKind::Section,
            text: section.to_owned(),
        });
    }
    let visible = &bytes[..bytes.len().min(MAX_DIFF_BYTES)];
    *truncated |= bytes.len() > MAX_DIFF_BYTES;
    let patch = String::from_utf8_lossy(visible);
    let mut old_line = 0;
    let mut new_line = 0;
    let mut inside_hunk = false;
    for line in patch.lines() {
        if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            *binary = true;
            continue;
        }
        if line.starts_with("@@ ") {
            if let Some((old, new)) = parse_hunk_lines(line) {
                old_line = old;
                new_line = new;
            }
            inside_hunk = true;
            rows.push(GitDiffRow {
                old_line: None,
                new_line: None,
                kind: GitDiffRowKind::Hunk,
                text: line.to_owned(),
            });
            continue;
        }
        if !inside_hunk {
            continue;
        }
        let (kind, old, new, text) = if let Some(text) = line.strip_prefix('+') {
            let current = new_line;
            new_line += 1;
            *additions += 1;
            (GitDiffRowKind::Addition, None, Some(current), text)
        } else if let Some(text) = line.strip_prefix('-') {
            let current = old_line;
            old_line += 1;
            *deletions += 1;
            (GitDiffRowKind::Deletion, Some(current), None, text)
        } else if let Some(text) = line.strip_prefix(' ') {
            let old = old_line;
            let new = new_line;
            old_line += 1;
            new_line += 1;
            (GitDiffRowKind::Context, Some(old), Some(new), text)
        } else if line.starts_with('\\') {
            (GitDiffRowKind::Notice, None, None, line)
        } else {
            continue;
        };
        rows.push(GitDiffRow {
            old_line: old,
            new_line: new,
            kind,
            text: text.to_owned(),
        });
    }
    if *truncated {
        rows.push(GitDiffRow {
            old_line: None,
            new_line: None,
            kind: GitDiffRowKind::Notice,
            text: "Diff truncated to 4 MiB to keep the UI responsive.".into(),
        });
    }
}

fn parse_hunk_lines(header: &str) -> Option<(usize, usize)> {
    let mut ranges = header.split_whitespace();
    ranges.next()?;
    let old = ranges.next()?.strip_prefix('-')?;
    let new = ranges.next()?.strip_prefix('+')?;
    let old = old.split(',').next()?.parse().ok()?;
    let new = new.split(',').next()?.parse().ok()?;
    Some((old, new))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    fn git(root: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            arguments,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository() -> PathBuf {
        let root = std::env::temp_dir().join(format!("vibra-git-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.name", "Vibra Test"]);
        git(&root, &["config", "user.email", "vibra@example.invalid"]);
        fs::write(root.join("tracked.txt"), "one\ntwo\n").unwrap();
        git(&root, &["add", "tracked.txt"]);
        git(&root, &["commit", "-qm", "initial"]);
        root
    }

    #[test]
    fn status_and_diff_cover_staged_unstaged_and_untracked_files() {
        let root = repository();
        fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();
        fs::write(root.join("staged.txt"), "prepared\n").unwrap();
        git(&root, &["add", "staged.txt"]);
        fs::write(root.join("new file.txt"), "new\nfile\n").unwrap();
        let port = GitCliPort::default();

        let snapshot = port.snapshot(&root).unwrap().unwrap();

        assert_eq!(snapshot.changes.len(), 3);
        assert!(snapshot.changes.iter().any(|change| {
            change.path == "tracked.txt" && change.unstaged && change.deletions == Some(1)
        }));
        assert!(snapshot.changes.iter().any(|change| {
            change.path == "staged.txt" && change.staged && change.status == GitFileStatus::Added
        }));
        let untracked = snapshot
            .changes
            .iter()
            .find(|change| change.path == "new file.txt")
            .unwrap();
        assert_eq!(untracked.additions, Some(2));
        assert_eq!(untracked.deletions, Some(0));
        let diff = port.diff(&root, untracked).unwrap();
        assert_eq!(diff.additions, 2);
        assert!(diff.rows.iter().any(|row| row.text == "new"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn untracked_line_counts_show_before_the_diff_is_opened() {
        let root = repository();
        fs::write(root.join("plain.txt"), "one\ntwo\nthree").unwrap();
        fs::write(root.join("empty.txt"), "").unwrap();
        fs::write(root.join("binary.bin"), b"hi\0there\n").unwrap();
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("nested/lib.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        let port = GitCliPort::default();

        let snapshot = port.snapshot(&root).unwrap().unwrap();
        let additions = |path: &str| {
            snapshot
                .changes
                .iter()
                .find(|change| change.path == path)
                .unwrap()
                .additions
        };
        assert_eq!(additions("plain.txt"), Some(3));
        assert_eq!(additions("empty.txt"), Some(0));
        assert_eq!(additions("binary.bin"), None);
        assert_eq!(additions("nested/lib.rs"), Some(2));
        assert_eq!(snapshot.additions, 5);

        fs::write(root.join("plain.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        let refreshed = port.snapshot(&root).unwrap().unwrap();
        assert_eq!(
            refreshed
                .changes
                .iter()
                .find(|change| change.path == "plain.txt")
                .unwrap()
                .additions,
            Some(4)
        );

        let changes = port.branch_changes(&root, None, None).unwrap().unwrap();
        assert_eq!(
            changes
                .snapshot
                .changes
                .iter()
                .find(|change| change.path == "nested/lib.rs")
                .unwrap()
                .additions,
            Some(2)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn text_additions_match_a_new_file_diff() {
        assert_eq!(text_additions(b""), Some(0));
        assert_eq!(text_additions(b"one\n"), Some(1));
        assert_eq!(text_additions(b"one\ntwo"), Some(2));
        assert_eq!(text_additions(b"one\ntwo\n"), Some(2));
        assert_eq!(text_additions(b"hi\0there\n"), None);
        let mut late_nul = vec![b'a'; 8_001];
        late_nul[8_000] = 0;
        assert_eq!(text_additions(&late_nul), Some(1));
    }

    #[test]
    fn untracked_files_inside_new_directories_are_listed() {
        let root = repository();
        fs::create_dir_all(root.join("new_module/src")).unwrap();
        fs::write(root.join("new_module/src/lib.rs"), "pub fn n() {}\n").unwrap();
        fs::write(root.join("new_module/README.md"), "new\n").unwrap();
        let port = GitCliPort::default();

        let snapshot = port.snapshot(&root).unwrap().unwrap();
        assert!(
            snapshot
                .changes
                .iter()
                .any(|change| { change.path == "new_module/src/lib.rs" && change.untracked }),
            "worktree snapshot should list files inside untracked directories, got {:?}",
            snapshot
                .changes
                .iter()
                .map(|change| change.path.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            snapshot
                .changes
                .iter()
                .any(|change| change.path == "new_module/README.md" && change.untracked)
        );
        assert!(
            !snapshot
                .changes
                .iter()
                .any(|change| change.path == "new_module" || change.path == "new_module/")
        );

        let lib = snapshot
            .changes
            .iter()
            .find(|change| change.path == "new_module/src/lib.rs")
            .unwrap();
        let diff = port.diff(&root, lib).unwrap();
        assert!(diff.rows.iter().any(|row| row.text.contains("pub fn n()")));

        let changes = port.branch_changes(&root, None, None).unwrap().unwrap();
        assert!(
            changes
                .snapshot
                .changes
                .iter()
                .any(|change| { change.path == "new_module/src/lib.rs" && change.untracked }),
            "branch changes should list untracked files inside new directories, got {:?}",
            changes
                .snapshot
                .changes
                .iter()
                .map(|change| change.path.as_str())
                .collect::<Vec<_>>()
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn untracked_nested_repository_diff_does_not_fail() {
        let root = repository();
        let nested = root.join("vendor/other");
        fs::create_dir_all(&nested).unwrap();
        git(&nested, &["init", "-q"]);
        git(&nested, &["config", "user.name", "Vibra Test"]);
        git(&nested, &["config", "user.email", "vibra@example.invalid"]);
        fs::write(nested.join("x.txt"), "x\n").unwrap();
        git(&nested, &["add", "x.txt"]);
        git(&nested, &["commit", "-qm", "nested"]);
        let port = GitCliPort::default();

        let snapshot = port.snapshot(&root).unwrap().unwrap();
        let nested_change = snapshot
            .changes
            .iter()
            .find(|change| {
                change.path == "vendor/other" || change.path.starts_with("vendor/other/")
            })
            .unwrap_or_else(|| {
                panic!(
                    "expected nested untracked repo, got {:?}",
                    snapshot
                        .changes
                        .iter()
                        .map(|change| change.path.as_str())
                        .collect::<Vec<_>>()
                )
            });
        assert!(nested_change.untracked);
        let diff = port.diff(&root, nested_change).unwrap();
        assert!(
            diff.rows
                .iter()
                .any(|row| row.kind == GitDiffRowKind::Notice)
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_program_can_run_version() {
        assert!(
            git_is_usable(git_program()),
            "resolved Git should be executable: {}",
            git_program().display()
        );
    }

    #[test]
    fn discover_skips_unusable_binaries_and_finds_a_working_git() {
        let found = discover_git_with(
            Some(PathBuf::from("/definitely/missing/vibra-git")),
            Some("/usr/bin:/bin:/usr/sbin:/sbin".into()),
            extra_git_candidates(),
        );
        assert!(
            git_is_usable(&found),
            "should resolve Homebrew or PATH Git, got {}",
            found.display()
        );
        if !git_is_usable(Path::new("/usr/bin/git")) {
            assert_ne!(
                found,
                PathBuf::from("/usr/bin/git"),
                "must not use the broken Xcode git stub"
            );
        }
    }

    #[test]
    fn missing_repository_is_none_not_an_error() {
        let root = std::env::temp_dir().join(format!("vibra-not-git-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let snapshot = GitCliPort::default().snapshot(&root).unwrap();
        assert!(snapshot.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn not_a_repository_message_is_detected() {
        assert!(is_not_a_repository(
            b"fatal: not a git repository (or any of the parent directories): .git\n"
        ));
        assert!(!is_not_a_repository(
            b"You have not agreed to the Xcode license agreements."
        ));
    }

    #[test]
    fn snapshot_resolves_the_repository_from_a_nested_working_directory() {
        let root = repository();
        let nested = root.join("src/deep");
        fs::create_dir_all(&nested).unwrap();

        let snapshot = GitCliPort::default().snapshot(&nested).unwrap().unwrap();

        assert_eq!(snapshot.root, root.canonicalize().unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn branch_summary_reports_dirty_and_tracking_without_full_snapshot() {
        let root = repository();
        let clean = GitCliPort::default()
            .branch_summary(&root)
            .unwrap()
            .unwrap();
        assert!(!clean.dirty);
        assert!(clean.branch == "main" || clean.branch == "master");

        fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();
        let dirty = GitCliPort::default()
            .branch_summary(&root)
            .unwrap()
            .unwrap();
        assert!(dirty.dirty);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_seeds_the_branch_summary_cache() {
        let root = repository();
        let port = GitCliPort::default();
        let snapshot = port.snapshot(&root).unwrap().unwrap();
        assert!(snapshot.changes.is_empty());

        fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();
        let cached = port.branch_summary(&root).unwrap().unwrap();
        assert_eq!(cached.branch, snapshot.branch);
        assert!(
            !cached.dirty,
            "branch_summary should reuse the snapshot cache within the TTL"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hunk_parser_tracks_old_and_new_line_numbers() {
        let mut rows = Vec::new();
        let mut additions = 0;
        let mut deletions = 0;
        let mut binary = false;
        let mut truncated = false;
        append_patch(
            b"@@ -4,2 +4,2 @@\n-old\n+new\n context\n",
            None,
            &mut rows,
            &mut additions,
            &mut deletions,
            &mut binary,
            &mut truncated,
        );

        assert_eq!(rows[1].old_line, Some(4));
        assert_eq!(rows[2].new_line, Some(4));
        assert_eq!((additions, deletions), (1, 1));
    }

    #[test]
    fn history_lists_commits_newest_first_with_parents() {
        let root = repository();
        fs::write(root.join("tracked.txt"), "one\ntwo\nthree\n").unwrap();
        git(&root, &["add", "tracked.txt"]);
        git(&root, &["commit", "-qm", "second"]);
        let history = GitCliPort::default().history(&root, 20).unwrap().unwrap();

        assert_eq!(history.total, 2);
        assert_eq!(history.commits.len(), 2);
        assert_eq!(history.commits[0].subject, "second");
        assert_eq!(history.commits[1].subject, "initial");
        assert_eq!(
            history.commits[0].parents,
            vec![history.commits[1].sha.clone()]
        );
        assert!(history.commits[1].parents.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn commit_changes_pin_the_selected_commit_and_leave_local_changes_alone() {
        let root = repository();
        let parent = rev_parse(&root, "HEAD").unwrap().unwrap();
        fs::write(root.join("tracked.txt"), "one\ncommitted\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "selected"]);
        let selected = rev_parse(&root, "HEAD").unwrap().unwrap();
        fs::write(root.join("tracked.txt"), "later commit\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "later"]);
        fs::write(root.join("tracked.txt"), "staged\n").unwrap();
        git(&root, &["add", "."]);
        fs::write(root.join("tracked.txt"), "unstaged\n").unwrap();
        fs::write(root.join("untracked.txt"), "untracked\n").unwrap();
        let port = GitCliPort::default();
        let before = port.snapshot(&root).unwrap();
        let head_before = rev_parse(&root, "HEAD").unwrap();

        let changes = port.commit_changes(&root, &selected).unwrap();
        assert_eq!(changes.base_revision, parent);
        assert_eq!(changes.revision, selected);
        assert_eq!(changes.snapshot.changes.len(), 1);
        assert_eq!(
            (changes.snapshot.additions, changes.snapshot.deletions),
            (1, 1)
        );
        let file = &changes.snapshot.changes[0];
        assert_eq!(file.path, "tracked.txt");
        let diff = port
            .diff_against(&root, &changes.base_revision, Some(&changes.revision), file)
            .unwrap();
        assert!(
            diff.rows
                .iter()
                .any(|row| row.kind == GitDiffRowKind::Addition && row.text == "committed")
        );
        assert!(
            diff.rows
                .iter()
                .any(|row| row.kind == GitDiffRowKind::Deletion && row.text == "two")
        );
        assert_eq!((diff.additions, diff.deletions), (1, 1));
        assert_eq!(port.snapshot(&root).unwrap(), before);
        assert_eq!(rev_parse(&root, "HEAD").unwrap(), head_before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn commit_changes_support_the_initial_and_empty_commits() {
        let root = repository();
        let port = GitCliPort::default();
        let changes = port.commit_changes(&root, "HEAD").unwrap();
        assert_eq!(changes.snapshot.changes.len(), 1);
        let file = &changes.snapshot.changes[0];
        assert_eq!(file.status, GitFileStatus::Added);
        let diff = port
            .diff_against(&root, &changes.base_revision, Some(&changes.revision), file)
            .unwrap();
        assert_eq!((diff.additions, diff.deletions), (2, 0));
        assert!(
            diff.rows
                .iter()
                .any(|row| row.kind == GitDiffRowKind::Addition && row.text == "one")
        );

        git(&root, &["commit", "--allow-empty", "-qm", "empty"]);
        let empty = port.commit_changes(&root, "HEAD").unwrap();
        assert_eq!(empty.base_revision, changes.revision);
        assert!(empty.snapshot.changes.is_empty());
        assert_eq!((empty.snapshot.additions, empty.snapshot.deletions), (0, 0));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn commit_changes_compare_merges_with_the_first_parent() {
        let root = repository();
        git(&root, &["branch", "-M", "main"]);
        git(&root, &["checkout", "-qb", "feature"]);
        fs::write(root.join("feature.txt"), "merged change\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "feature"]);
        git(&root, &["checkout", "-q", "main"]);
        fs::write(root.join("main.txt"), "already on main\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "main change"]);
        let first_parent = rev_parse(&root, "HEAD").unwrap().unwrap();
        git(
            &root,
            &["merge", "--no-ff", "-qm", "merge feature", "feature"],
        );

        let port = GitCliPort::default();
        let changes = port.commit_changes(&root, "HEAD").unwrap();
        assert_eq!(changes.base_revision, first_parent);
        assert_eq!(changes.snapshot.changes.len(), 1);
        assert_eq!(changes.snapshot.changes[0].path, "feature.txt");
        let diff = port
            .diff_against(
                &root,
                &changes.base_revision,
                Some(&changes.revision),
                &changes.snapshot.changes[0],
            )
            .unwrap();
        assert_eq!((diff.additions, diff.deletions), (1, 0));
        assert!(diff.rows.iter().any(|row| row.text == "merged change"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn commit_changes_cover_renames_binary_and_literal_paths() {
        let root = repository();
        // A rename keeps both paths reviewable; null-delimited metadata preserves
        // whitespace, Unicode and pathspec characters in committed filenames.
        let renamed = "renamed\tfile\nñ.txt";
        let literal = "[literal]*.txt";
        git(&root, &["mv", "tracked.txt", renamed]);
        fs::write(root.join("binary.bin"), b"\0\x01\x02\x03").unwrap();
        fs::write(root.join(literal), "literal path\n").unwrap();
        fs::write(root.join("other.txt"), "another path\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "rename and add files"]);
        let port = GitCliPort::default();
        let changes = port.commit_changes(&root, "HEAD").unwrap();
        assert_eq!(changes.snapshot.changes.len(), 5);
        assert_eq!(
            (changes.snapshot.additions, changes.snapshot.deletions),
            (4, 2)
        );
        for file in &changes.snapshot.changes {
            let diff = port
                .diff_against(&root, &changes.base_revision, Some(&changes.revision), file)
                .unwrap();
            match file.path.as_str() {
                "tracked.txt" => {
                    assert_eq!(file.status, GitFileStatus::Deleted);
                    assert_eq!((diff.additions, diff.deletions), (0, 2));
                }
                "binary.bin" => {
                    assert!(diff.binary);
                    assert_eq!(file.additions, None);
                }
                path if path == renamed => {
                    assert_eq!(file.status, GitFileStatus::Added);
                    assert_eq!(file.additions, Some(2));
                    assert_eq!((diff.additions, diff.deletions), (2, 0));
                }
                path if path == literal => {
                    assert_eq!((diff.additions, diff.deletions), (1, 0));
                    assert!(diff.rows.iter().any(|row| row.text == "literal path"));
                    assert!(!diff.rows.iter().any(|row| row.text == "another path"));
                }
                _ => assert_eq!((diff.additions, diff.deletions), (1, 0)),
            }
        }
        assert!(port.commit_changes(&root, "--output=unexpected").is_err());
        assert!(port.commit_changes(&root, "missing-commit").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn branch_changes_include_committed_files_against_the_base() {
        let root = repository();
        git(&root, &["checkout", "-qb", "feature"]);
        fs::write(root.join("feature.txt"), "on the branch\n").unwrap();
        git(&root, &["add", "feature.txt"]);
        git(&root, &["commit", "-qm", "add feature"]);
        fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();

        let changes = GitCliPort::default()
            .branch_changes(&root, None, None)
            .unwrap()
            .unwrap();
        assert_eq!(changes.commits_ahead, 1);
        assert!(!changes.base.is_empty());
        assert!(changes.snapshot.changes.iter().any(|change| {
            change.path == "feature.txt" && change.status == GitFileStatus::Added
        }));
        assert!(
            changes
                .snapshot
                .changes
                .iter()
                .any(|change| change.path == "tracked.txt")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worktree_captures_compare_one_turn_without_touching_the_index() {
        let root = repository();
        fs::write(root.join("before.txt"), "untracked before the turn\n").unwrap();
        fs::write(root.join(".gitignore"), "ignored.log\n").unwrap();
        let port = GitCliPort::default();
        let status_before = port.snapshot(&root).unwrap().unwrap();

        let base = port.capture_worktree(&root).unwrap().unwrap();
        fs::write(root.join("tracked.txt"), "one\nturn\n").unwrap();
        fs::write(root.join("created.txt"), "made by the agent\n").unwrap();
        fs::write(root.join("ignored.log"), "noise\n").unwrap();
        fs::remove_file(root.join("before.txt")).unwrap();
        let head = port.capture_worktree(&root).unwrap().unwrap();

        let changes = port.tree_changes(&root, &base.tree, &head.tree).unwrap();
        let mut paths: Vec<(&str, GitFileStatus)> = changes
            .snapshot
            .changes
            .iter()
            .map(|change| (change.path.as_str(), change.status))
            .collect();
        paths.sort_by_key(|(path, _)| *path);
        assert_eq!(
            paths,
            vec![
                ("before.txt", GitFileStatus::Deleted),
                ("created.txt", GitFileStatus::Added),
                ("tracked.txt", GitFileStatus::Modified),
            ]
        );
        let tracked = changes
            .snapshot
            .changes
            .iter()
            .find(|change| change.path == "tracked.txt")
            .unwrap();
        let diff = port
            .diff_against(&root, &base.tree, Some(&head.tree), tracked)
            .unwrap();
        assert_eq!((diff.additions, diff.deletions), (1, 1));

        // The real index and status are exactly as they were.
        let status_after = port.snapshot(&root).unwrap().unwrap();
        assert!(status_after.changes.iter().all(|change| !change.staged));
        assert_eq!(status_before.branch, status_after.branch);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diff_sources_follow_the_compared_sides() {
        let root = repository();
        let port = GitCliPort::default();
        fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();
        let snapshot = port.snapshot(&root).unwrap().unwrap();
        let tracked = snapshot
            .changes
            .iter()
            .find(|change| change.path == "tracked.txt")
            .unwrap();
        let sources = port.diff_sources(&root, tracked, None, None).unwrap();
        assert_eq!(sources.old.as_deref(), Some("one\ntwo\n"));
        assert_eq!(sources.new.as_deref(), Some("one\nchanged\n"));

        git(&root, &["add", "tracked.txt"]);
        let snapshot = port.snapshot(&root).unwrap().unwrap();
        let staged = snapshot.changes.first().unwrap();
        let sources = port.diff_sources(&root, staged, None, None).unwrap();
        assert_eq!(sources.old.as_deref(), Some("one\ntwo\n"));
        assert_eq!(sources.new.as_deref(), Some("one\nchanged\n"));

        fs::write(root.join("tracked.txt"), "one\nchanged\nagain\n").unwrap();
        let snapshot = port.snapshot(&root).unwrap().unwrap();
        let both = snapshot.changes.first().unwrap();
        assert_eq!(
            port.diff_sources(&root, both, None, None).unwrap(),
            GitDiffSources::default()
        );

        fs::write(root.join("binary.bin"), b"a\0b").unwrap();
        let binary = GitFileChange {
            path: "binary.bin".into(),
            status: GitFileStatus::Untracked,
            staged: false,
            unstaged: false,
            untracked: true,
            additions: None,
            deletions: None,
        };
        assert_eq!(
            port.diff_sources(&root, &binary, None, None).unwrap(),
            GitDiffSources::default()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn branch_selectors_list_local_and_remote_refs_without_aliases_or_tags() {
        let root = repository();
        git(&root, &["branch", "-M", "main"]);
        git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(
            &root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        git(&root, &["branch", "origin/main"]);
        git(&root, &["tag", "main"]);
        let branches = GitCliPort::default().branches(&root).unwrap();
        assert_eq!(branches.len(), 3);
        assert!(
            branches
                .iter()
                .any(|branch| branch.reference == "refs/heads/main" && !branch.remote)
        );
        assert!(
            branches
                .iter()
                .any(|branch| branch.reference == "refs/heads/origin/main" && !branch.remote)
        );
        assert!(
            branches
                .iter()
                .any(|branch| branch.reference == "refs/remotes/origin/main" && branch.remote)
        );
        let changes = GitCliPort::default()
            .branch_changes(
                &root,
                Some("refs/remotes/origin/main"),
                Some("refs/heads/main"),
            )
            .unwrap()
            .unwrap();
        assert!(changes.snapshot.changes.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_branches_compare_both_tips_and_pin_diffs_without_checkout() {
        let root = repository();
        git(&root, &["branch", "-M", "main"]);
        git(&root, &["checkout", "-qb", "remote-tip"]);
        fs::write(root.join("remote.txt"), "remote version\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "remote change"]);
        git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&root, &["checkout", "-q", "main"]);
        fs::write(root.join("local.txt"), "saved local version\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "local change"]);
        fs::write(root.join("tracked.txt"), "staged\n").unwrap();
        git(&root, &["add", "."]);
        fs::write(root.join("local.txt"), "unsaved local version\n").unwrap();
        fs::write(root.join("untracked.txt"), "untracked\n").unwrap();
        let port = GitCliPort::default();
        let before = port.snapshot(&root).unwrap();
        let changes = port
            .branch_changes(
                &root,
                Some("refs/remotes/origin/main"),
                Some("refs/heads/main"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(changes.snapshot.changes.len(), 2);
        assert_eq!(
            (changes.snapshot.additions, changes.snapshot.deletions),
            (1, 1)
        );
        let local = changes
            .snapshot
            .changes
            .iter()
            .find(|c| c.path == "local.txt")
            .unwrap();
        let remote = changes
            .snapshot
            .changes
            .iter()
            .find(|c| c.path == "remote.txt")
            .unwrap();
        assert_eq!(remote.status, GitFileStatus::Deleted);
        let diff = port
            .diff_against(
                &root,
                &changes.base_revision,
                changes.head_revision.as_deref(),
                local,
            )
            .unwrap();
        assert!(
            diff.rows
                .iter()
                .any(|row| row.text == "saved local version")
        );
        assert!(
            !diff
                .rows
                .iter()
                .any(|row| row.text == "unsaved local version")
        );
        assert_eq!(before, port.snapshot(&root).unwrap());
        let worktree = port
            .branch_changes(&root, Some("refs/remotes/origin/main"), None)
            .unwrap()
            .unwrap();
        assert!(worktree.head_revision.is_none());
        assert!(
            worktree
                .snapshot
                .changes
                .iter()
                .any(|c| c.path == "untracked.txt")
        );
        assert!(
            worktree
                .snapshot
                .changes
                .iter()
                .any(|c| c.path == "remote.txt" && c.status == GitFileStatus::Deleted)
        );
        let reverse = port
            .branch_changes(
                &root,
                Some("refs/heads/main"),
                Some("refs/remotes/origin/main"),
            )
            .unwrap()
            .unwrap();
        assert!(
            reverse
                .snapshot
                .changes
                .iter()
                .any(|c| c.path == "remote.txt" && c.status == GitFileStatus::Added)
        );
        git(&root, &["commit", "-qam", "advance local"]);
        let pinned = port
            .diff_against(
                &root,
                &changes.base_revision,
                changes.head_revision.as_deref(),
                local,
            )
            .unwrap();
        assert_eq!(pinned, diff);
        let refreshed = port
            .branch_changes(
                &root,
                Some("refs/remotes/origin/main"),
                Some("refs/heads/main"),
            )
            .unwrap()
            .unwrap();
        assert_ne!(refreshed.head_revision, changes.head_revision);
        assert!(
            port.branch_changes(&root, Some("missing"), Some("refs/heads/main"))
                .is_err()
        );
        assert!(port.branch_changes(&root, Some("--help"), None).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parse_history_splits_unit_separated_fields() {
        let commits = parse_history(
            "aaa\x1faaa\x1fsubject\x1fAda\x1f2023-11-14\x1fbbb ccc\n\
             bbb\x1fbbb\x1froot\x1fAda\x1f2023-07-22\x1f\n",
        );
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, vec!["bbb", "ccc"]);
        assert!(commits[1].parents.is_empty());
    }

    #[test]
    fn oversized_diffs_are_truncated_while_reading_git_output() {
        let root = repository();
        let path = root.join("large.txt");
        fs::write(&path, vec![b'x'; MAX_DIFF_BYTES + 1024]).unwrap();
        let port = GitCliPort::default();
        let snapshot = port.snapshot(&root).unwrap().unwrap();
        let change = snapshot
            .changes
            .iter()
            .find(|change| change.path == "large.txt")
            .unwrap();

        let diff = port.diff(&root, change).unwrap();

        assert!(diff.truncated);
        assert!(diff.rows.iter().any(|row| {
            row.kind == GitDiffRowKind::Notice && row.text.contains("truncated to 4 MiB")
        }));
        fs::remove_dir_all(root).unwrap();
    }
}
