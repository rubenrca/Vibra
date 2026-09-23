use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result, bail};

use crate::ports::git::{
    GitBranchChanges, GitBranchRef, GitBranchSummary, GitCommit, GitCommitChanges, GitDiff,
    GitDiffRow, GitDiffRowKind, GitDiffSources, GitFileChange, GitFileStatus, GitHistory, GitPort,
    GitRepositorySnapshot, GitWorktreeCapture,
};

const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;
const MAX_GIT_OUTPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_GIT_ERROR_BYTES: usize = 64 * 1024;
/// Line counts for untracked files stay local. A full `git diff --no-index` on
/// every sidebar poll would rebuild patches the list never shows.
const MAX_UNTRACKED_STAT_BYTES: u64 = 8 * 1024 * 1024;
const BRANCH_SUMMARY_TTL: Duration = Duration::from_millis(1_500);
const MAX_CAPTURE_INDEXES: usize = 8;

#[derive(Clone)]
struct CachedBranchSummary {
    summary: GitBranchSummary,
    fetched_at: Instant,
}

#[derive(Clone)]
struct CachedUntrackedStat {
    len: u64,
    modified: SystemTime,
    changed_seconds: i64,
    changed_nanoseconds: i64,
    /// `None` for binary files and files above [`MAX_UNTRACKED_STAT_BYTES`].
    additions: Option<usize>,
}

struct CachedCaptureIndex {
    temporary: TemporaryIndex,
    source_index: Option<IndexFingerprint>,
    untracked_paths: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IndexFingerprint {
    device: u64,
    inode: u64,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl IndexFingerprint {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

fn index_fingerprint(path: &Path) -> Result<Option<IndexFingerprint>> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };
    Ok(Some(IndexFingerprint::from_metadata(&metadata)))
}

fn copy_source_index(source: &Path, destination: &Path, expected: IndexFingerprint) -> Result<()> {
    let mut input = std::fs::File::open(source)
        .with_context(|| format!("failed to open {}", source.display()))?;
    if IndexFingerprint::from_metadata(&input.metadata()?) != expected {
        bail!("{} changed while preparing the capture", source.display());
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    std::io::copy(&mut input, &mut output)
        .with_context(|| format!("failed to copy {}", source.display()))?;
    if IndexFingerprint::from_metadata(&input.metadata()?) != expected
        || index_fingerprint(source)? != Some(expected)
    {
        bail!(
            "{} changed while copying the capture index",
            source.display()
        );
    }
    output.sync_all()?;
    Ok(())
}

struct CachedCaptureSlot {
    index: Arc<Mutex<Option<CachedCaptureIndex>>>,
    used_at: Instant,
}

#[derive(Default)]
struct DiffAccumulator {
    rows: Vec<GitDiffRow>,
    additions: usize,
    deletions: usize,
    binary: bool,
    truncated: bool,
}

impl DiffAccumulator {
    fn append(&mut self, patch: &[u8], section: Option<&str>) {
        append_patch(
            patch,
            section,
            &mut self.rows,
            &mut self.additions,
            &mut self.deletions,
            &mut self.binary,
            &mut self.truncated,
        );
    }

    fn append_untracked(&mut self, root: &Path, path: &str, section: Option<&str>) -> Result<()> {
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
        let patch = run_git_diff(
            root,
            [
                "--literal-pathspecs",
                "diff",
                "--no-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--unified=3",
                "--",
                "/dev/null",
                path,
            ],
            "git diff --no-index",
            true,
        )?;
        self.append(&patch, section);
        Ok(())
    }

    fn finish(mut self, path: String) -> GitDiff {
        if self.rows.is_empty() {
            self.rows.push(empty_diff_notice(self.binary));
        }
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
        let mut changes: Vec<GitFileChange> = Vec::with_capacity(files.len());
        let mut deleted_and_recreated = HashMap::<String, usize>::new();
        for file in files {
            let untracked = file.untracked();
            // Git reports a staged deletion followed by a newly created,
            // untracked file at the same path as two porcelain records. The
            // review panel keys rows by path, so present both sections in one
            // file instead of making the second row open the first row's diff.
            if untracked && let Some(&index) = deleted_and_recreated.get(&file.path) {
                let change = &mut changes[index];
                change.untracked = true;
                change.status = GitFileStatus::Modified;
                continue;
            }
            if file.index == 'D' && file.worktree == ' ' {
                deleted_and_recreated.insert(file.path.clone(), changes.len());
            }
            changes.push(GitFileChange {
                status: file_status(file.index, file.worktree),
                staged: !untracked && file.index != ' ',
                unstaged: !untracked && file.worktree != ' ',
                untracked,
                path: file.path,
                old_path: file.old_path,
                additions: None,
                deletions: None,
            });
        }

        let mut stats = HashMap::<String, (usize, usize)>::new();
        collect_numstat(&root, false, &mut stats)?;
        collect_numstat(&root, true, &mut stats)?;
        for change in &mut changes {
            if change.untracked && !change.staged {
                continue;
            }
            if let Some((additions, deletions)) = stats.get(&change.path) {
                change.additions = Some(*additions);
                change.deletions = Some(*deletions);
            }
        }
        self.fill_untracked_line_counts(&root, &mut changes);
        changes.sort_by_cached_key(|change| (change_priority(change), change.path.to_lowercase()));
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
        if let Some(old_path) = &change.old_path {
            validate_relative_path(old_path)?;
        }
        let root = repository_root(repository)?.context("the repository is no longer available")?;
        let multiple_sections = change.staged && (change.unstaged || change.untracked);
        let mut diff = DiffAccumulator::default();

        if change.status == GitFileStatus::Conflicted {
            // Git's combined `@@@` hunks have two old sides, which the ordinary
            // two-sided parser cannot represent. Show the actual conflict file
            // with its markers and line numbers instead of an empty diff.
            let patch = run_git_diff(
                &root,
                [
                    "--literal-pathspecs",
                    "diff",
                    "--no-index",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                    "--unified=3",
                    "--",
                    "/dev/null",
                    &change.path,
                ],
                "git diff conflicted file",
                true,
            )?;
            diff.append(&patch, Some("UNMERGED WORKING TREE"));
            return Ok(diff.finish(change.path.clone()));
        }

        if let Some(old_path) = &change.old_path {
            diff.rows.push(GitDiffRow {
                old_line: None,
                new_line: None,
                kind: GitDiffRowKind::Notice,
                text: format!("From {old_path}"),
            });
        }

        if change.staged {
            let mut args = vec![
                "--literal-pathspecs",
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--unified=3",
                "--",
            ];
            args.extend(change.old_path.as_deref());
            args.push(&change.path);
            let patch = run_git_diff(&root, args, "git diff --cached", false)?;
            diff.append(&patch, multiple_sections.then_some("STAGED CHANGES"));
        }

        if change.unstaged {
            let mut args = vec![
                "--literal-pathspecs",
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--unified=3",
                "--",
            ];
            args.extend(change.old_path.as_deref());
            args.push(&change.path);
            let patch = run_git_diff(&root, args, "git diff", false)?;
            diff.append(&patch, multiple_sections.then_some("WORKING TREE"));
        }

        if change.untracked {
            diff.append_untracked(
                &root,
                &change.path,
                multiple_sections.then_some("UNTRACKED"),
            )?;
        }

        Ok(diff.finish(change.path.clone()))
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
        let seen: HashMap<String, usize> = changes
            .iter()
            .enumerate()
            .map(|(index, change)| (change.path.clone(), index))
            .collect();
        for path in if selected_head.is_none() {
            untracked_paths(&root)?
        } else {
            Vec::new()
        } {
            if let Some(&index) = seen.get(&path) {
                // A staged deletion followed by a new untracked file has two
                // porcelain entries for one path. Keep its branch row reviewable.
                changes[index].untracked = true;
            } else {
                changes.push(GitFileChange {
                    status: GitFileStatus::Untracked,
                    staged: false,
                    unstaged: false,
                    untracked: true,
                    path,
                    old_path: None,
                    additions: None,
                    deletions: None,
                });
            }
        }
        self.fill_untracked_line_counts(&root, &mut changes);
        changes.sort_by_cached_key(|change| (change_priority(change), change.path.to_lowercase()));
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
                "-z",
                "--pretty=tformat:%H%x00%h%x00%s%x00%an%x00%as%x00%P",
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
        let commits = parse_history(&output.stdout);
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
        if let Some(old_path) = &change.old_path {
            validate_relative_path(old_path)?;
        }
        let against = against.filter(|revision| !revision.is_empty());
        if let Some(revision) = against {
            validate_revision(revision)?;
        }
        if let Some(head) = head {
            validate_revision(head)?;
        }
        let root = repository_root(repository)?.context("the repository is no longer available")?;
        let path = change.path.as_str();
        let old_path = change.old_path.as_deref().unwrap_or(path);
        let worktree = || read_worktree_source(&root, path);
        let blob =
            |revision: &str, path: &str| read_blob_source(&root, &format!("{revision}:{path}"));
        let deleted = change.status == GitFileStatus::Deleted;
        let added = matches!(
            change.status,
            GitFileStatus::Added | GitFileStatus::Untracked
        );

        // Mirror the sides `diff` / `diff_against` compare.
        let sources = match (against, head) {
            (Some(revision), Some(head)) => GitDiffSources {
                old: blob(revision, old_path),
                new: blob(head, path),
            },
            (Some(_), None) if change.untracked && change.status != GitFileStatus::Untracked => {
                GitDiffSources::default()
            }
            (Some(revision), None) if !change.untracked => GitDiffSources {
                old: blob(revision, old_path),
                new: worktree(),
            },
            _ if change.staged && change.untracked => GitDiffSources::default(),
            _ if change.untracked => GitDiffSources {
                old: None,
                new: worktree(),
            },
            // Staged and worktree sections have different bases; per-row
            // highlighting stays correct for both.
            _ if change.staged && change.unstaged => GitDiffSources::default(),
            _ if change.staged => GitDiffSources {
                old: (!added).then(|| blob("HEAD", old_path)).flatten(),
                new: (!deleted).then(|| blob("", path)).flatten(),
            },
            _ => GitDiffSources {
                old: blob("", old_path),
                new: (!deleted).then(worktree).flatten(),
            },
        };
        Ok(sources)
    }

    fn capture_worktree(&self, root: &Path) -> Result<Option<GitWorktreeCapture>> {
        let Some(root) = repository_root(root)? else {
            return Ok(None);
        };
        // Reuse a private index so Git's stat cache also covers dirty files
        // across polls. Rebuild it when the real index or the set of untracked
        // paths changes; the latter includes .gitignore rule changes.
        let source_index_path = git_path(&root, "index")?;
        let source_index = index_fingerprint(&source_index_path)?;
        let untracked_paths = run_git(&root, ["ls-files", "--others", "--exclude-standard", "-z"])?;
        ensure_success(&untracked_paths, "git ls-files --others")?;
        // A reused private index forgets a tracked path once it is deleted.
        // If that path is also ignored, a later recreation would not be added
        // back. Keep these captures one-shot until all tracked paths exist.
        let deleted_paths = run_git(&root, ["ls-files", "--deleted", "-z"])?;
        ensure_success(&deleted_paths, "git ls-files --deleted")?;
        let has_deleted_paths = !deleted_paths.stdout.is_empty();
        let slot = {
            let mut cache = self
                .capture_indexes
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let slot = cache
                .entry(root.clone())
                .or_insert_with(|| CachedCaptureSlot {
                    index: Arc::new(Mutex::new(None)),
                    used_at: Instant::now(),
                });
            slot.used_at = Instant::now();
            Arc::clone(&slot.index)
        };
        // Only captures of the same repository serialize. Git commands for
        // another repository no longer hold the whole cache's mutex.
        let mut index = slot.lock().unwrap_or_else(|error| error.into_inner());
        let rebuild = index.as_ref().is_none_or(|entry| {
            entry.source_index != source_index
                || entry.untracked_paths != untracked_paths.stdout
                || !entry.temporary.0.is_file()
                || has_deleted_paths
        });
        if rebuild {
            let temporary = TemporaryIndex(std::env::temp_dir().join(format!(
                "vibra-turn-index-{}",
                uuid::Uuid::new_v4().simple()
            )));
            if let Some(fingerprint) = source_index {
                copy_source_index(&source_index_path, &temporary.0, fingerprint)?;
            }
            *index = Some(CachedCaptureIndex {
                temporary,
                source_index,
                untracked_paths: untracked_paths.stdout,
            });
        }
        let entry = index.as_mut().expect("capture index was just inserted");
        // Keep add + write-tree under the same lock: concurrent captures must
        // never operate on this index at the same time.
        let capture = (|| {
            let output = run_git_with_index(
                &root,
                &entry.temporary.0,
                ["add", "--all", "--ignore-errors", "--", "."],
            )?;
            ensure_success(&output, "git add (turn snapshot)")?;
            let output = run_git_with_index(&root, &entry.temporary.0, ["write-tree"])?;
            ensure_success(&output, "git write-tree")?;
            let tree = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            validate_revision(&tree)?;
            if tree.is_empty() {
                bail!("git write-tree returned no tree");
            }
            Ok::<_, anyhow::Error>(tree)
        })();
        if capture.is_err() || has_deleted_paths {
            *index = None;
        }
        drop(index);
        let mut cache = self
            .capture_indexes
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while cache.len() > MAX_CAPTURE_INDEXES {
            let oldest = cache
                .iter()
                .filter(|(path, entry)| *path != &root && Arc::strong_count(&entry.index) == 1)
                .min_by_key(|(_, entry)| entry.used_at)
                .map(|(path, _)| path.clone());
            if let Some(oldest) = oldest {
                cache.remove(&oldest);
            } else {
                break;
            }
        }
        let tree = capture?;
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
        if let Some(old_path) = &change.old_path {
            validate_relative_path(old_path)?;
        }
        validate_revision(revision)?;
        if let Some(head) = head {
            validate_revision(head)?;
        }
        if head.is_none() && (revision.is_empty() || change.status == GitFileStatus::Untracked) {
            return self.diff(repository, change);
        }
        let root = repository_root(repository)?.context("the repository is no longer available")?;
        let mut diff = DiffAccumulator::default();
        if let Some(old_path) = &change.old_path {
            diff.rows.push(GitDiffRow {
                old_line: None,
                new_line: None,
                kind: GitDiffRowKind::Notice,
                text: format!("From {old_path}"),
            });
        }
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
        args.push("--");
        args.extend(change.old_path.as_deref());
        args.push(&change.path);
        let patch = run_git_diff(&root, args, "git diff revision", false)?;
        let mixed_untracked = head.is_none() && change.untracked;
        diff.append(&patch, mixed_untracked.then_some("CHANGES FROM BASE"));
        if mixed_untracked {
            diff.append_untracked(&root, &change.path, Some("UNTRACKED"))?;
        }
        Ok(diff.finish(change.path.clone()))
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
    let path = PathBuf::from(OsString::from_vec(
        trim_git_newline(&output.stdout).to_vec(),
    ));
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
        // The private index is reused between polls. Always consider ctime,
        // even when a repository has disabled it for its real index, so a
        // same-size edit with restored mtime cannot reuse a stale blob.
        .arg("-c")
        .arg("core.trustctime=true")
        .arg("-c")
        .arg("core.checkStat=default")
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

fn repository_root(root: &Path) -> Result<Option<PathBuf>> {
    if !root.is_dir() {
        return Ok(None);
    }
    let output = run_git(root, ["rev-parse", "--show-toplevel"])?;
    if output.status.success() {
        let path = trim_git_newline(&output.stdout);
        return Ok((!path.is_empty()).then(|| PathBuf::from(OsString::from_vec(path.to_vec()))));
    }
    if is_not_a_repository(&output.stderr) {
        return Ok(None);
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    bail!("git rev-parse failed: {message}")
}

fn trim_git_newline(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

/// Dock-launched macOS apps only get `/usr/bin:/bin:/usr/sbin:/sbin`. Apple's
/// `/usr/bin/git` is an Xcode stub that fails (license, missing CLT) even when
/// Homebrew Git is installed. Prefer a Git that can actually run.
pub(crate) fn git_program() -> &'static Path {
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
    let mut child = git_command()
        .arg("-C")
        .arg(root)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run Git in {}", root.display()))?;
    let stdout = child.stdout.take().context("Git did not open stdout")?;
    let stderr = child.stderr.take().context("Git did not open stderr")?;
    let stderr_reader = thread::spawn(move || read_git_error(stderr));
    let mut output = Vec::new();
    let read_result = stdout
        .take((MAX_GIT_OUTPUT_BYTES + 1) as u64)
        .read_to_end(&mut output);
    let reached_limit = output.len() > MAX_GIT_OUTPUT_BYTES;
    if reached_limit || read_result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("Git stderr reader failed"))??;
    read_result?;
    if reached_limit {
        bail!("Git output exceeded 32 MiB in {}", root.display());
    }
    Ok(Output {
        status,
        stdout: output,
        stderr,
    })
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
    let stderr = child.stderr.take().context("Git did not open stderr")?;
    let stderr_reader = thread::spawn(move || read_git_error(stderr));

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

/// Keep draining stderr so Git cannot block on a full pipe, but bound the
/// diagnostic retained in memory for a failed diff.
fn read_git_error(mut stderr: impl Read) -> std::io::Result<Vec<u8>> {
    let mut error = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = stderr.read(&mut chunk)?;
        if read == 0 {
            return Ok(error);
        }
        let keep = read.min(MAX_GIT_ERROR_BYTES.saturating_sub(error.len()));
        error.extend_from_slice(&chunk[..keep]);
    }
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
        let old_path = if matches!(index, 'R' | 'C') || matches!(worktree, 'R' | 'C') {
            records
                .next()
                .map(|path| String::from_utf8_lossy(path).into_owned())
        } else {
            None
        };
        let path = record[3..].trim_end_matches('/').to_owned();
        if path.is_empty() {
            continue;
        }
        status.files.push(PorcelainFile {
            index,
            worktree,
            path,
            old_path,
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
    old_path: Option<String>,
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
    arguments.extend(["--no-ext-diff", "--no-textconv", "--numstat", "-z"]);
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
        "-z",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
        revision,
    ];
    args.extend(head);
    args.push("--");
    let output = run_git(root, args)?;
    ensure_success(&output, "git diff --name-status")?;
    Ok(parse_name_status(&output.stdout))
}

fn parse_name_status(output: &[u8]) -> Vec<GitFileChange> {
    let mut changes = Vec::new();
    let mut fields = output.split(|byte| *byte == 0);
    while let Some(code) = fields.next() {
        if code.is_empty() {
            continue;
        }
        let status = match code[0] as char {
            'A' => GitFileStatus::Added,
            'D' => GitFileStatus::Deleted,
            'R' => GitFileStatus::Renamed,
            'C' => GitFileStatus::Copied,
            'T' => GitFileStatus::TypeChanged,
            'U' => GitFileStatus::Conflicted,
            _ => GitFileStatus::Modified,
        };
        let (old_path, path) = if matches!(status, GitFileStatus::Renamed | GitFileStatus::Copied) {
            (
                fields
                    .next()
                    .map(|path| String::from_utf8_lossy(path).into_owned()),
                fields.next().unwrap_or_default(),
            )
        } else {
            (None, fields.next().unwrap_or_default())
        };
        if path.is_empty() {
            continue;
        }
        changes.push(GitFileChange {
            status,
            staged: false,
            unstaged: true,
            untracked: false,
            path: String::from_utf8_lossy(path).into_owned(),
            old_path,
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
    let mut args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--numstat",
        "-z",
        revision,
    ];
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
    let changed_seconds = meta.ctime();
    let changed_nanoseconds = meta.ctime_nsec();
    if let Some(cached) = cache.get(path)
        && cached.len == len
        && cached.modified == modified
        && cached.changed_seconds == changed_seconds
        && cached.changed_nanoseconds == changed_nanoseconds
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
            changed_seconds,
            changed_nanoseconds,
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
    let mut records = stdout.split(|byte| *byte == 0);
    while let Some(record) = records.next() {
        let mut fields = record.splitn(3, |byte| *byte == b'\t');
        let (Some(additions), Some(deletions), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(additions), Ok(deletions)) = (
            std::str::from_utf8(additions)
                .unwrap_or_default()
                .parse::<usize>(),
            std::str::from_utf8(deletions)
                .unwrap_or_default()
                .parse::<usize>(),
        ) else {
            continue;
        };
        // With -z a rename has an empty path in the first record followed by
        // separate old and new path records. Attribute its stats to the new path.
        let path = if path.is_empty() {
            let _old = records.next();
            records.next().unwrap_or_default()
        } else {
            path
        };
        if path.is_empty() {
            continue;
        }
        let entry = stats
            .entry(String::from_utf8_lossy(path).into_owned())
            .or_default();
        entry.0 += additions;
        entry.1 += deletions;
    }
}

fn parse_history(output: &[u8]) -> Vec<GitCommit> {
    // Git subjects and author names may contain newlines or control characters.
    // A NUL-separated tformat record has six fields and a final separator.
    let mut commits = Vec::new();
    for fields in output
        .split(|byte| *byte == 0)
        .collect::<Vec<_>>()
        .chunks_exact(6)
    {
        let [sha, short_sha, subject, author, date, parents] = fields else {
            continue;
        };
        let text = |field: &&[u8]| String::from_utf8_lossy(field).into_owned();
        commits.push(GitCommit {
            sha: text(sha),
            short_sha: text(short_sha),
            subject: text(subject),
            author: text(author),
            date: text(date),
            parents: String::from_utf8_lossy(parents)
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
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
mod tests;
