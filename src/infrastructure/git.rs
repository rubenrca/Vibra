use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

use anyhow::{Context as _, Result, bail};

use crate::ports::git::{
    GitBranchSummary, GitDiff, GitDiffRow, GitDiffRowKind, GitFileChange, GitFileStatus, GitPort,
    GitRepositorySnapshot,
};

const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;
const MAX_UNTRACKED_STAT_BYTES: u64 = 5 * 1024 * 1024;

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
        let mut upstream = None;
        let mut ahead = 0;
        let mut behind = 0;
        let mut changes = Vec::new();

        while let Some(record) = records.next() {
            if record.is_empty() {
                continue;
            }
            let record = String::from_utf8_lossy(record);
            if let Some(header) = record.strip_prefix("## ") {
                (branch, upstream, ahead, behind) = parse_branch_header(header);
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
            let old_path = renamed_or_copied
                .then(|| records.next())
                .flatten()
                .filter(|path| !path.is_empty())
                .map(|path| String::from_utf8_lossy(path).into_owned());
            let untracked = index == '?' && worktree == '?';
            changes.push(GitFileChange {
                status: file_status(index, worktree),
                staged: !untracked && index != ' ',
                unstaged: !untracked && worktree != ' ',
                untracked,
                path,
                old_path,
                additions: None,
                deletions: None,
            });
        }

        let mut stats = HashMap::<String, (usize, usize)>::new();
        collect_numstat(&root, false, &mut stats)?;
        collect_numstat(&root, true, &mut stats)?;
        for change in &mut changes {
            if change.untracked {
                let path = root.join(&change.path);
                if path.metadata().is_ok_and(|metadata| {
                    metadata.is_file() && metadata.len() <= MAX_UNTRACKED_STAT_BYTES
                }) && let Ok(contents) = fs::read(&path)
                {
                    change.additions = Some(line_count(&contents));
                    change.deletions = Some(0);
                }
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
            upstream,
            ahead,
            behind,
            changes,
            additions,
            deletions,
        }))
    }

    fn branch_summary(&self, root: &Path) -> Result<Option<GitBranchSummary>> {
        let Some(root) = repository_root(root)? else {
            return Ok(None);
        };
        // Porcelain without numstat/diff: enough for branch, upstream, and dirty.
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
        let mut upstream = None;
        let mut ahead = 0;
        let mut behind = 0;
        let mut dirty = false;

        while let Some(record) = records.next() {
            if record.is_empty() {
                continue;
            }
            let record = String::from_utf8_lossy(record);
            if let Some(header) = record.strip_prefix("## ") {
                (branch, upstream, ahead, behind) = parse_branch_header(header);
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
            upstream,
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
            old_path: change.old_path.clone(),
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

fn parse_branch_header(header: &str) -> (String, Option<String>, usize, usize) {
    let (relation, tracking) = header
        .rsplit_once(" [")
        .map(|(relation, tracking)| (relation, tracking.trim_end_matches(']')))
        .unwrap_or((header, ""));
    let relation = relation
        .strip_prefix("No commits yet on ")
        .or_else(|| relation.strip_prefix("Initial commit on "))
        .unwrap_or(relation);
    let (branch, upstream) = relation
        .split_once("...")
        .map(|(branch, upstream)| (branch, Some(upstream.to_owned())))
        .unwrap_or((relation, None));
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
    (branch, upstream, ahead, behind)
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

fn line_count(contents: &[u8]) -> usize {
    if contents.is_empty() {
        0
    } else {
        contents.iter().filter(|byte| **byte == b'\n').count()
            + usize::from(contents.last() != Some(&b'\n'))
    }
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
        assert_eq!(untracked.additions, Some(2));
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
