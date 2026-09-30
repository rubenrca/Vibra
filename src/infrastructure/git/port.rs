//! CLI-backed [`GitPort`] implementation.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context as _, Result, bail};

use crate::ports::git::{
    GitBranchChanges, GitBranchRef, GitCommitChanges, GitCommitOptions, GitDiff, GitDiffRow,
    GitDiffRowKind, GitDiffSources, GitFileChange, GitFileStatus, GitHistory, GitPort,
    GitRepositorySnapshot, GitSyncOperation, GitWorktreeCapture,
};

use super::parsing::{
    PorcelainStatus, change_priority, file_status, parse_history, parse_porcelain_status,
};
use super::*;

impl GitPort for GitCliPort {
    fn commit(&self, root: &Path, message: &str, options: GitCommitOptions) -> Result<String> {
        let root = repository_root(root)?.context("this project is not a Git repository")?;
        let message = message.trim();
        if message.is_empty() && !options.amend {
            bail!("enter a commit message");
        }
        if index_is_clean(&root) && !options.amend {
            run_git_write(&root, &["add", "--all"], "git add")?;
            if index_is_clean(&root) {
                bail!("there are no changes to commit");
            }
        }
        let mut arguments = vec!["commit"];
        if options.amend {
            arguments.push("--amend");
            if message.is_empty() {
                arguments.push("--no-edit");
            }
        }
        if !message.is_empty() {
            arguments.extend(["-m", message]);
        }
        run_git_write(&root, &arguments, "git commit")?;
        self.forget_branch_summary(&root);
        let short = run_git_write(&root, &["rev-parse", "--short", "HEAD"], "git rev-parse")?;
        Ok(git_stdout(&short))
    }

    fn sync(&self, root: &Path, operation: GitSyncOperation) -> Result<()> {
        let root = repository_root(root)?.context("this project is not a Git repository")?;
        match operation {
            GitSyncOperation::Fetch => {
                run_git_write(&root, &["fetch", "--prune"], "git fetch")?;
            }
            GitSyncOperation::Pull => {
                run_git_write(&root, &["pull", "--ff-only"], "git pull")?;
            }
            GitSyncOperation::Push if has_upstream(&root) => {
                run_git_write(&root, &["push"], "git push")?;
            }
            GitSyncOperation::Push => {
                let remotes = run_git_write(&root, &["remote"], "git remote")?;
                let remotes = String::from_utf8_lossy(&remotes.stdout);
                let remote = remotes
                    .lines()
                    .find(|remote| *remote == "origin")
                    .or_else(|| remotes.lines().next())
                    .map(str::to_owned);
                let Some(remote) = remote else {
                    bail!("the repository has no remote configured");
                };
                run_git_write(&root, &["push", "-u", &remote, "HEAD"], "git push")?;
            }
        }
        self.forget_branch_summary(&root);
        Ok(())
    }

    fn stage(&self, root: &Path, paths: &[String]) -> Result<()> {
        let root = repository_root(root)?.context("this project is not a Git repository")?;
        if paths.is_empty() {
            return Ok(());
        }
        for path in paths {
            validate_relative_path(path)?;
        }
        let mut arguments = vec!["--literal-pathspecs", "add", "--all", "--"];
        arguments.extend(paths.iter().map(String::as_str));
        run_git_write(&root, &arguments, "git add")?;
        Ok(())
    }

    fn unstage(&self, root: &Path, paths: &[String]) -> Result<()> {
        let root = repository_root(root)?.context("this project is not a Git repository")?;
        if paths.is_empty() {
            return Ok(());
        }
        for path in paths {
            validate_relative_path(path)?;
        }
        let has_head = rev_parse(&root, "HEAD")?.is_some();
        let mut arguments = if has_head {
            vec!["--literal-pathspecs", "restore", "--staged", "--"]
        } else {
            // With no HEAD, unstaging removes only the index entry. Force is
            // needed when the file was edited again after staging it.
            vec![
                "--literal-pathspecs",
                "rm",
                "--cached",
                "-f",
                "-r",
                "-q",
                "--",
            ]
        };
        arguments.extend(paths.iter().map(String::as_str));
        run_git_write(&root, &arguments, "git restore")?;
        Ok(())
    }

    fn commit_message_context(&self, root: &Path) -> Result<String> {
        const LIMIT: usize = 48 * 1024;
        let root = repository_root(root)?.context("this project is not a Git repository")?;
        let has_head = rev_parse(&root, "HEAD")?.is_some();
        let staged = !index_is_clean(&root);
        let range = if !staged && has_head {
            "HEAD"
        } else {
            "--cached"
        };
        let mut context = String::new();
        if has_head {
            let log = run_git(&root, ["log", "-8", "--format=%s"])?;
            context.push_str("Recent commit subjects:\n");
            context.push_str(&String::from_utf8_lossy(&log.stdout));
            context.push('\n');
        }
        context.push_str("Changes:\n");
        context.push_str(&String::from_utf8_lossy(
            &run_git(&root, ["diff", "--stat", range])?.stdout,
        ));
        if !staged {
            let untracked = untracked_paths(&root)?;
            if !untracked.is_empty() {
                context.push_str("\nNew files:\n");
                for path in untracked.iter().take(50) {
                    context.push_str(path);
                    context.push('\n');
                }
            }
        }
        let patch = run_git_diff(
            &root,
            [
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                range,
            ],
            "git diff commit context",
            false,
        )?;
        context.push_str("\nPatch:\n");
        context.push_str(&String::from_utf8_lossy(&patch));
        if context.len() > LIMIT {
            let mut end = LIMIT;
            while !context.is_char_boundary(end) {
                end -= 1;
            }
            context.truncate(end);
            context.push_str("\n[patch truncated]\n");
        }
        Ok(context)
    }

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
        collect_numstat(&root, &[], &mut stats)?;
        collect_numstat(&root, &["--cached"], &mut stats)?;
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
            let patch = patch_from_empty(&root, &change.path, "git diff conflicted file")?;
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

        for (enabled, cached, operation, section) in [
            (change.staged, true, "git diff --cached", "STAGED CHANGES"),
            (change.unstaged, false, "git diff", "WORKING TREE"),
        ] {
            if !enabled {
                continue;
            }
            let mut args = vec!["--literal-pathspecs", "diff"];
            if cached {
                args.push("--cached");
            }
            args.extend([
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--unified=3",
                "--",
            ]);
            args.extend(change.old_path.as_deref());
            args.push(&change.path);
            let patch = run_git_diff(&root, args, operation, false)?;
            diff.append(&patch, multiple_sections.then_some(section));
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
                "--decorate=short",
                "--pretty=tformat:%H%x00%h%x00%s%x00%an%x00%as%x00%P%x00%D",
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
                git_stdout(&output)
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
            let tree = git_stdout(&output);
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
