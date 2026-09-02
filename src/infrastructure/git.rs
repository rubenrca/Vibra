use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

use anyhow::{Context as _, Result, bail};

use crate::ports::git::{
    GitBranchChanges, GitBranchSummary, GitCommit, GitDiff, GitDiffRow, GitDiffRowKind,
    GitFileChange, GitFileStatus, GitHistory, GitPort, GitRepositorySnapshot,
};

const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub struct GitCliPort;

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
        let mut records = output.stdout.split(|byte| *byte == 0).peekable();
        let mut branch = "HEAD".to_owned();
        let mut changes = Vec::new();

        while let Some(record) = records.next() {
            if record.is_empty() {
                continue;
            }
            let record = String::from_utf8_lossy(record);
            if let Some(header) = record.strip_prefix("## ") {
                (branch, _, _) = parse_branch_header(header);
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
            let path = record[3..].to_owned();
            let renamed_or_copied = matches!(index, 'R' | 'C') || matches!(worktree, 'R' | 'C');
            if renamed_or_copied {
                let _ = records.next();
            }
            let untracked = index == '?' && worktree == '?';
            changes.push(GitFileChange {
                status: file_status(index, worktree),
                staged: !untracked && index != ' ',
                unstaged: !untracked && worktree != ' ',
                untracked,
                path,
                additions: None,
                deletions: None,
            });
        }

        let mut stats = HashMap::<String, (usize, usize)>::new();
        collect_numstat(&root, false, &mut stats)?;
        collect_numstat(&root, true, &mut stats)?;
        for change in &mut changes {
            if change.untracked {
                // Poll path: do not slurp untracked files just to count lines.
                continue;
            } else if let Some((additions, deletions)) = stats.get(&change.path) {
                change.additions = Some(*additions);
                change.deletions = Some(*deletions);
            }
        }
        changes.sort_by(|left, right| {
            change_priority(left)
                .cmp(&change_priority(right))
                .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
        });
        let additions = changes.iter().filter_map(|change| change.additions).sum();
        let deletions = changes.iter().filter_map(|change| change.deletions).sum();
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
        let mut records = output.stdout.split(|byte| *byte == 0).peekable();
        let mut branch = "HEAD".to_owned();
        let mut ahead = 0;
        let mut behind = 0;
        let mut dirty = false;

        while let Some(record) = records.next() {
            if record.is_empty() {
                continue;
            }
            let record = String::from_utf8_lossy(record);
            if let Some(header) = record.strip_prefix("## ") {
                (branch, ahead, behind) = parse_branch_header(header);
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
            dirty = true;
            // Renamed/copied records consume the next path entry.
            if matches!(index, 'R' | 'C') || matches!(worktree, 'R' | 'C') {
                let _ = records.next();
            }
        }

        Ok(Some(GitBranchSummary {
            branch,
            ahead,
            behind,
            dirty,
        }))
    }

    fn diff(&self, repository: &Path, change: &GitFileChange) -> Result<GitDiff> {
        validate_relative_path(&change.path)?;
        let root =
            repository_root(repository)?.context("el repositorio dejó de estar disponible")?;
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
                multiple_sections.then_some("CAMBIOS PREPARADOS"),
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
                multiple_sections.then_some("DIRECTORIO DE TRABAJO"),
                &mut rows,
                &mut additions,
                &mut deletions,
                &mut binary,
                &mut truncated,
            );
        }

        if change.untracked {
            let patch = run_git_diff(
                &root,
                [
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
                multiple_sections.then_some("ARCHIVO SIN SEGUIMIENTO"),
                &mut rows,
                &mut additions,
                &mut deletions,
                &mut binary,
                &mut truncated,
            );
        }

        if rows.is_empty() {
            rows.push(GitDiffRow {
                old_line: None,
                new_line: None,
                kind: GitDiffRowKind::Notice,
                text: if binary {
                    "El archivo contiene datos binarios y no tiene vista textual.".into()
                } else {
                    "Git no devolvió cambios textuales para este archivo.".into()
                },
            });
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

    fn branch_changes(&self, root: &Path) -> Result<Option<GitBranchChanges>> {
        let Some(root) = repository_root(root)? else {
            return Ok(None);
        };
        let Some(summary) = self.branch_summary(&root)? else {
            return Ok(None);
        };
        let Some(base) = compare_base(&root, &summary.branch)? else {
            return Ok(Some(GitBranchChanges {
                snapshot: GitRepositorySnapshot {
                    root,
                    branch: summary.branch,
                    changes: Vec::new(),
                    additions: 0,
                    deletions: 0,
                },
                base: String::new(),
                merge_base: String::new(),
                commits_ahead: 0,
            }));
        };
        let merge_base = merge_base(&root, &base)?;
        let mut changes = diff_name_status(&root, &merge_base)?;
        let mut stats = HashMap::<String, (usize, usize)>::new();
        collect_numstat_against(&root, &merge_base, &mut stats)?;
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
        for path in untracked_paths(&root)? {
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
        changes.sort_by(|left, right| {
            change_priority(left)
                .cmp(&change_priority(right))
                .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
        });
        let additions = changes.iter().filter_map(|change| change.additions).sum();
        let deletions = changes.iter().filter_map(|change| change.deletions).sum();
        let commits_ahead = rev_count(&root, &format!("{merge_base}..HEAD"))?;
        Ok(Some(GitBranchChanges {
            snapshot: GitRepositorySnapshot {
                root,
                branch: summary.branch,
                changes,
                additions,
                deletions,
            },
            base,
            merge_base,
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

    fn diff_against(
        &self,
        repository: &Path,
        revision: &str,
        change: &GitFileChange,
    ) -> Result<GitDiff> {
        validate_relative_path(&change.path)?;
        validate_revision(revision)?;
        if change.untracked || revision.is_empty() {
            return self.diff(repository, change);
        }
        let root =
            repository_root(repository)?.context("el repositorio dejó de estar disponible")?;
        let mut rows = Vec::new();
        let mut additions = 0;
        let mut deletions = 0;
        let mut binary = false;
        let mut truncated = false;
        let patch = run_git_diff(
            &root,
            [
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--unified=3",
                revision,
                "--",
                &change.path,
            ],
            "git diff revision",
            false,
        )?;
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
            rows.push(GitDiffRow {
                old_line: None,
                new_line: None,
                kind: GitDiffRowKind::Notice,
                text: if binary {
                    "El archivo contiene datos binarios y no tiene vista textual.".into()
                } else {
                    "Git no devolvió cambios textuales para este archivo.".into()
                },
            });
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

fn repository_root(root: &Path) -> Result<Option<PathBuf>> {
    if !root.is_dir() {
        return Ok(None);
    }
    let output = run_git(root, ["rev-parse", "--show-toplevel"])?;
    if !output.status.success() {
        return Ok(None);
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!path.is_empty()).then(|| PathBuf::from(path)))
}

fn run_git<I, S>(root: &Path, arguments: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new("git")
        .arg("-c")
        .arg("core.quotepath=false")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .output()
        .with_context(|| format!("no se pudo ejecutar Git en {}", root.display()))
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
    let mut child = Command::new("git")
        .arg("-c")
        .arg("core.quotepath=false")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("no se pudo ejecutar Git en {}", root.display()))?;
    let stdout = child.stdout.take().context("Git no abrió stdout")?;
    let mut stderr = child.stderr.take().context("Git no abrió stderr")?;
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
        .map_err(|_| anyhow::anyhow!("falló el lector de stderr de Git"))??;
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
    bail!("{operation} falló: {message}")
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
    let output = run_git(
        root,
        [
            "ls-files",
            "--others",
            "--exclude-standard",
            "--directory",
            "-z",
        ],
    )?;
    ensure_success(&output, "git ls-files --others")?;
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
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
    for line in String::from_utf8_lossy(&output.stdout).lines() {
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
        bail!("Git devolvió una revisión no segura");
    }
    Ok(())
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
        bail!("Git no encontró un ancestro común con {other}");
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

fn diff_name_status(root: &Path, revision: &str) -> Result<Vec<GitFileChange>> {
    validate_revision(revision)?;
    let output = run_git(
        root,
        [
            "diff",
            "--name-status",
            "--no-ext-diff",
            "--find-renames",
            revision,
        ],
    )?;
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
    stats: &mut HashMap<String, (usize, usize)>,
) -> Result<()> {
    validate_revision(revision)?;
    let output = run_git(root, ["diff", "--no-ext-diff", "--numstat", revision])?;
    ensure_success(&output, "git diff --numstat")?;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
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
    Ok(())
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
        bail!("Git devolvió una ruta no segura")
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
            text: "Diff truncado a 4 MiB para mantener la interfaz fluida.".into(),
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
        let port = GitCliPort;

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
        assert!(
            untracked.additions.is_none(),
            "worktree poll must not slurp untracked files for line counts"
        );
        let diff = port.diff(&root, untracked).unwrap();
        assert_eq!(diff.additions, 2);
        assert!(diff.rows.iter().any(|row| row.text == "new"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_resolves_the_repository_from_a_nested_working_directory() {
        let root = repository();
        let nested = root.join("src/deep");
        fs::create_dir_all(&nested).unwrap();

        let snapshot = GitCliPort.snapshot(&nested).unwrap().unwrap();

        assert_eq!(snapshot.root, root.canonicalize().unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn branch_summary_reports_dirty_and_tracking_without_full_snapshot() {
        let root = repository();
        let port = GitCliPort;
        let clean = port.branch_summary(&root).unwrap().unwrap();
        assert!(!clean.dirty);
        assert!(clean.branch == "main" || clean.branch == "master");

        fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();
        let dirty = port.branch_summary(&root).unwrap().unwrap();
        assert!(dirty.dirty);

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
        let history = GitCliPort.history(&root, 20).unwrap().unwrap();

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
    fn branch_changes_include_committed_files_against_the_base() {
        let root = repository();
        git(&root, &["checkout", "-qb", "feature"]);
        fs::write(root.join("feature.txt"), "on the branch\n").unwrap();
        git(&root, &["add", "feature.txt"]);
        git(&root, &["commit", "-qm", "add feature"]);
        fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();

        let changes = GitCliPort.branch_changes(&root).unwrap().unwrap();
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
        let port = GitCliPort;
        let snapshot = port.snapshot(&root).unwrap().unwrap();
        let change = snapshot
            .changes
            .iter()
            .find(|change| change.path == "large.txt")
            .unwrap();

        let diff = port.diff(&root, change).unwrap();

        assert!(diff.truncated);
        assert!(diff.rows.iter().any(|row| {
            row.kind == GitDiffRowKind::Notice && row.text.contains("truncado a 4 MiB")
        }));
        fs::remove_dir_all(root).unwrap();
    }
}
