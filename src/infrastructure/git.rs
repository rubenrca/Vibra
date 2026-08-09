use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context as _, Result, bail};

use crate::ports::git::{
    GitCommit, GitDiff, GitDiffHunk, GitDiffHunkSource, GitDiffRow, GitDiffRowKind, GitFileChange,
    GitFileStatus, GitHunkAction, GitPort, GitRepositorySnapshot,
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
        let state_token = repository_state_token(&root, &output.stdout)?;

        Ok(Some(GitRepositorySnapshot {
            root,
            branch,
            upstream,
            ahead,
            behind,
            changes,
            additions,
            deletions,
            state_token,
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
        let mut hunks = Vec::new();

        if change.staged {
            let output = run_git(
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
            )?;
            ensure_success(&output, "git diff --cached")?;
            append_patch(
                &output.stdout,
                multiple_sections.then_some("CAMBIOS PREPARADOS"),
                &mut rows,
                &mut additions,
                &mut deletions,
                &mut binary,
                &mut truncated,
                &mut hunks,
                GitDiffHunkSource::Index,
            );
        }

        if change.unstaged {
            let output = run_git(
                &root,
                [
                    "diff",
                    "--no-ext-diff",
                    "--no-color",
                    "--unified=3",
                    "--",
                    &change.path,
                ],
            )?;
            ensure_success(&output, "git diff")?;
            append_patch(
                &output.stdout,
                multiple_sections.then_some("DIRECTORIO DE TRABAJO"),
                &mut rows,
                &mut additions,
                &mut deletions,
                &mut binary,
                &mut truncated,
                &mut hunks,
                GitDiffHunkSource::Worktree,
            );
        }

        if change.untracked {
            let output = run_git_allow_difference(
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
            )?;
            append_patch(
                &output.stdout,
                multiple_sections.then_some("ARCHIVO SIN SEGUIMIENTO"),
                &mut rows,
                &mut additions,
                &mut deletions,
                &mut binary,
                &mut truncated,
                &mut hunks,
                GitDiffHunkSource::Untracked,
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
            hunks,
        })
    }

    fn stage(&self, repository: &Path, path: &str, expected_state: &str) -> Result<()> {
        validate_relative_path(path)?;
        let root = ensure_repository_state(repository, expected_state)?;
        let output = run_git_mutating(&root, ["add", "--", path])?;
        ensure_success(&output, "git add")
    }

    fn unstage(&self, repository: &Path, path: &str, expected_state: &str) -> Result<()> {
        validate_relative_path(path)?;
        let root = ensure_repository_state(repository, expected_state)?;
        let has_head = run_git(&root, ["rev-parse", "--verify", "HEAD"])?
            .status
            .success();
        let output = if has_head {
            run_git_mutating(&root, ["restore", "--staged", "--", path])?
        } else {
            run_git_mutating(&root, ["rm", "--cached", "-r", "--", path])?
        };
        ensure_success(&output, "git unstage")
    }

    fn discard_worktree_change(
        &self,
        repository: &Path,
        path: &str,
        expected_state: &str,
    ) -> Result<()> {
        validate_relative_path(path)?;
        let root = ensure_repository_state(repository, expected_state)?;
        if !root.join(path).exists() {
            // Restoring a tracked deletion is safe; untracked paths are deliberately
            // excluded because deletion belongs in the recoverable Files workflow.
            let tracked = run_git(&root, ["ls-files", "--error-unmatch", "--", path])?;
            ensure_success(&tracked, "git ls-files")?;
        }
        let output = run_git_mutating(&root, ["restore", "--worktree", "--", path])?;
        ensure_success(&output, "git restore")
    }

    fn commit(
        &self,
        repository: &Path,
        message: &str,
        amend: bool,
        expected_state: &str,
    ) -> Result<()> {
        let message = validate_commit_message(message)?;
        let root = ensure_repository_state(repository, expected_state)?;
        let mut arguments = vec!["commit"];
        if amend {
            arguments.push("--amend");
        }
        arguments.extend(["-m", message]);
        let output = run_git_mutating(&root, arguments)?;
        ensure_success(
            &output,
            if amend {
                "git commit --amend"
            } else {
                "git commit"
            },
        )
    }

    fn fetch(&self, repository: &Path) -> Result<()> {
        let root = repository_root(repository)?.context("no hay repositorio Git")?;
        let output = run_git_mutating(&root, ["fetch", "--prune"])?;
        ensure_success(&output, "git fetch")
    }

    fn pull_ff_only(&self, repository: &Path, expected_state: &str) -> Result<()> {
        let root = ensure_repository_state(repository, expected_state)?;
        let output = run_git_mutating(&root, ["pull", "--ff-only"])?;
        ensure_success(&output, "git pull --ff-only")
    }

    fn push(&self, repository: &Path) -> Result<()> {
        let root = repository_root(repository)?.context("no hay repositorio Git")?;
        let output = run_git_mutating(&root, ["push"])?;
        ensure_success(&output, "git push")
    }

    fn publish_branch(&self, repository: &Path, branch: &str) -> Result<()> {
        validate_branch_name(branch)?;
        let root = repository_root(repository)?.context("no hay repositorio Git")?;
        let output = run_git_mutating(&root, ["push", "--set-upstream", "origin", branch])?;
        ensure_success(&output, "git push --set-upstream")
    }

    fn create_branch(&self, repository: &Path, branch: &str, expected_state: &str) -> Result<()> {
        validate_branch_name(branch)?;
        let root = ensure_repository_state(repository, expected_state)?;
        let check = run_git(&root, ["check-ref-format", "--branch", branch])?;
        ensure_success(&check, "nombre de branch")?;
        let output = run_git_mutating(&root, ["switch", "-c", branch])?;
        ensure_success(&output, "git switch -c")
    }

    fn stash(&self, repository: &Path, message: &str, expected_state: &str) -> Result<()> {
        let root = ensure_repository_state(repository, expected_state)?;
        let message = message.trim();
        let message = if message.is_empty() {
            "VibraGPUI stash"
        } else {
            message
        };
        let output = run_git_mutating(&root, ["stash", "push", "-u", "-m", message])?;
        ensure_success(&output, "git stash push")
    }

    fn stash_pop(&self, repository: &Path, expected_state: &str) -> Result<()> {
        let root = ensure_repository_state(repository, expected_state)?;
        let output = run_git_mutating(&root, ["stash", "pop"])?;
        ensure_success(&output, "git stash pop")
    }

    fn initialize(&self, root: &Path) -> Result<()> {
        if !root.is_dir() {
            bail!("{} no es un directorio", root.display());
        }
        let output = run_git_mutating(root, ["init"])?;
        ensure_success(&output, "git init")
    }

    fn recent_commits(&self, repository: &Path, limit: usize) -> Result<Vec<GitCommit>> {
        let root = repository_root(repository)?.context("no hay repositorio Git")?;
        let limit = limit.clamp(1, 200).to_string();
        let output = run_git(
            &root,
            [
                "log",
                "-z",
                "--format=%H%x00%h%x00%an%x00%ct%x00%s",
                "-n",
                &limit,
            ],
        )?;
        ensure_success(&output, "git log")?;
        let fields: Vec<_> = output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
            .collect();
        Ok(fields
            .chunks_exact(5)
            .map(|fields| GitCommit {
                oid: String::from_utf8_lossy(fields[0]).into_owned(),
                short_oid: String::from_utf8_lossy(fields[1]).into_owned(),
                author: String::from_utf8_lossy(fields[2]).into_owned(),
                timestamp: String::from_utf8_lossy(fields[3])
                    .parse()
                    .unwrap_or_default(),
                summary: String::from_utf8_lossy(fields[4]).into_owned(),
            })
            .collect())
    }

    fn apply_hunk(
        &self,
        repository: &Path,
        hunk: &GitDiffHunk,
        action: GitHunkAction,
        expected_state: &str,
    ) -> Result<()> {
        let root = ensure_repository_state(repository, expected_state)?;
        match (hunk.source, action) {
            (GitDiffHunkSource::Index, GitHunkAction::Unstage)
            | (GitDiffHunkSource::Worktree | GitDiffHunkSource::Untracked, GitHunkAction::Stage)
            | (GitDiffHunkSource::Worktree, GitHunkAction::Discard) => {}
            _ => bail!("esa acción no corresponde al origen del hunk"),
        }
        let mut command = Command::new("git");
        command
            .arg("-c")
            .arg("core.quotepath=false")
            .arg("-C")
            .arg(&root)
            .arg("apply")
            .arg("--unidiff-zero")
            .arg("--whitespace=nowarn");
        match action {
            GitHunkAction::Stage => {
                command.arg("--cached");
            }
            GitHunkAction::Unstage => {
                command.args(["--cached", "--reverse"]);
            }
            GitHunkAction::Discard => {
                command.arg("--reverse");
            }
        }
        let mut child = command
            .arg("-")
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("no se pudo ejecutar git apply en {}", root.display()))?;
        child
            .stdin
            .take()
            .context("git apply no abrió stdin")?
            .write_all(hunk.patch.as_bytes())?;
        let output = child.wait_with_output()?;
        ensure_success(&output, "git apply hunk")
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

#[allow(dead_code)]
fn run_git_mutating<I, S>(root: &Path, arguments: I) -> Result<Output>
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
        .env("LC_ALL", "C")
        .output()
        .with_context(|| format!("no se pudo ejecutar Git en {}", root.display()))
}

#[allow(dead_code)]
fn ensure_repository_state(repository: &Path, expected_state: &str) -> Result<PathBuf> {
    let root = repository_root(repository)?.context("el repositorio dejó de estar disponible")?;
    let status = run_git(
        &root,
        [
            "status",
            "--porcelain=v1",
            "-z",
            "--branch",
            "--untracked-files=all",
        ],
    )?;
    ensure_success(&status, "git status")?;
    let actual_state = repository_state_token(&root, &status.stdout)?;
    if actual_state != expected_state {
        bail!(
            "el repositorio cambió desde la última lectura; actualiza el panel antes de continuar"
        );
    }
    Ok(root)
}

fn repository_state_token(root: &Path, status: &[u8]) -> Result<String> {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut byte_count = 0_u64;
    hash_bytes(&mut hash, &mut byte_count, status);
    for (index, arguments) in [
        vec!["rev-parse", "--verify", "HEAD"],
        vec!["diff", "--no-ext-diff", "--binary"],
        vec!["diff", "--cached", "--no-ext-diff", "--binary"],
    ]
    .into_iter()
    .enumerate()
    {
        let output = run_git(root, arguments)?;
        // An unborn HEAD is expected; the other commands must succeed.
        if !output.status.success() && index != 0 {
            ensure_success(&output, "Git state fingerprint")?;
        }
        hash_bytes(&mut hash, &mut byte_count, &output.stdout);
        hash_bytes(&mut hash, &mut byte_count, &output.stderr);
    }

    let untracked = run_git(root, ["ls-files", "--others", "--exclude-standard", "-z"])?;
    ensure_success(&untracked, "git ls-files --others")?;
    hash_bytes(&mut hash, &mut byte_count, &untracked.stdout);
    for path in untracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = String::from_utf8_lossy(path);
        validate_relative_path(&path)?;
        let candidate = root.join(path.as_ref());
        if candidate.is_file() {
            let mut file = fs::File::open(&candidate)?;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hash_bytes(&mut hash, &mut byte_count, &buffer[..read]);
            }
        }
    }
    Ok(format!("{hash:016x}-{byte_count:x}"))
}

fn hash_bytes(hash: &mut u64, byte_count: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    *byte_count = byte_count.wrapping_add(bytes.len() as u64);
    *hash ^= 0xff;
    *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
}

#[allow(dead_code)]
fn validate_commit_message(message: &str) -> Result<&str> {
    let message = message.trim();
    if message.is_empty() {
        bail!("escribe un mensaje de commit");
    }
    if message
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        bail!("el mensaje de commit contiene caracteres de control");
    }
    Ok(message)
}

#[allow(dead_code)]
fn validate_branch_name(branch: &str) -> Result<()> {
    if branch.is_empty()
        || branch.trim() != branch
        || branch.starts_with('-')
        || branch.chars().any(char::is_control)
    {
        bail!("nombre de branch inválido");
    }
    Ok(())
}

fn run_git_allow_difference<I, S>(root: &Path, arguments: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = run_git(root, arguments)?;
    if output.status.success() || output.status.code() == Some(1) {
        Ok(output)
    } else {
        ensure_success(&output, "git diff --no-index")?;
        unreachable!()
    }
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

#[allow(clippy::too_many_arguments)]
fn append_patch(
    bytes: &[u8],
    section: Option<&str>,
    rows: &mut Vec<GitDiffRow>,
    additions: &mut usize,
    deletions: &mut usize,
    binary: &mut bool,
    truncated: &mut bool,
    hunks: &mut Vec<GitDiffHunk>,
    hunk_source: GitDiffHunkSource,
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
    hunks.extend(extract_hunks(visible, hunk_source));
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

fn extract_hunks(bytes: &[u8], source: GitDiffHunkSource) -> Vec<GitDiffHunk> {
    let patch = String::from_utf8_lossy(bytes);
    let lines: Vec<_> = patch.lines().collect();
    let Some(first_hunk) = lines.iter().position(|line| line.starts_with("@@ ")) else {
        return Vec::new();
    };
    let preamble = &lines[..first_hunk];
    let mut hunks = Vec::new();
    let mut index = first_hunk;
    while index < lines.len() {
        if !lines[index].starts_with("@@ ") {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < lines.len()
            && !lines[index].starts_with("@@ ")
            && !lines[index].starts_with("diff --git ")
        {
            index += 1;
        }
        let mut hunk_patch = String::new();
        for line in preamble.iter().chain(lines[start..index].iter()) {
            hunk_patch.push_str(line);
            hunk_patch.push('\n');
        }
        hunks.push(GitDiffHunk {
            header: lines[start].to_owned(),
            patch: hunk_patch,
            source,
        });
    }
    hunks
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
    fn hunk_parser_tracks_old_and_new_line_numbers() {
        let mut rows = Vec::new();
        let mut additions = 0;
        let mut deletions = 0;
        let mut binary = false;
        let mut truncated = false;
        let mut hunks = Vec::new();
        append_patch(
            b"@@ -4,2 +4,2 @@\n-old\n+new\n context\n",
            None,
            &mut rows,
            &mut additions,
            &mut deletions,
            &mut binary,
            &mut truncated,
            &mut hunks,
            GitDiffHunkSource::Worktree,
        );

        assert_eq!(rows[1].old_line, Some(4));
        assert_eq!(rows[2].new_line, Some(4));
        assert_eq!((additions, deletions), (1, 1));
    }

    #[test]
    fn guarded_stage_unstage_discard_and_commit_workflows() {
        let root = repository();
        let port = GitCliPort;
        fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();
        let before_stage = port.snapshot(&root).unwrap().unwrap();

        port.stage(&root, "tracked.txt", &before_stage.state_token)
            .unwrap();
        let staged = port.snapshot(&root).unwrap().unwrap();
        assert!(staged.changes[0].staged);

        port.unstage(&root, "tracked.txt", &staged.state_token)
            .unwrap();
        let unstaged = port.snapshot(&root).unwrap().unwrap();
        assert!(unstaged.changes[0].unstaged);

        port.discard_worktree_change(&root, "tracked.txt", &unstaged.state_token)
            .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("tracked.txt")).unwrap(),
            "one\ntwo\n"
        );

        fs::write(root.join("committed.txt"), "ready\n").unwrap();
        let untracked = port.snapshot(&root).unwrap().unwrap();
        port.stage(&root, "committed.txt", &untracked.state_token)
            .unwrap();
        let staged = port.snapshot(&root).unwrap().unwrap();
        port.commit(&root, "add committed file", false, &staged.state_token)
            .unwrap();

        let commits = port.recent_commits(&root, 2).unwrap();
        assert_eq!(commits[0].summary, "add committed file");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_mutations_are_rejected_before_git_changes_state() {
        let root = repository();
        let port = GitCliPort;
        fs::write(root.join("tracked.txt"), "first edit\n").unwrap();
        let stale = port.snapshot(&root).unwrap().unwrap();
        fs::write(root.join("tracked.txt"), "second edit\n").unwrap();

        let error = port
            .stage(&root, "tracked.txt", &stale.state_token)
            .unwrap_err();

        assert!(error.to_string().contains("cambió"));
        assert!(!port.snapshot(&root).unwrap().unwrap().changes[0].staged);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn branch_stash_and_init_operations_are_guarded() {
        let root = repository();
        let port = GitCliPort;
        let clean = port.snapshot(&root).unwrap().unwrap();
        port.create_branch(&root, "feature/test", &clean.state_token)
            .unwrap();
        assert_eq!(
            port.snapshot(&root).unwrap().unwrap().branch,
            "feature/test"
        );

        fs::write(root.join("tracked.txt"), "stash me\n").unwrap();
        let dirty = port.snapshot(&root).unwrap().unwrap();
        port.stash(&root, "test stash", &dirty.state_token).unwrap();
        let clean = port.snapshot(&root).unwrap().unwrap();
        assert!(clean.changes.is_empty());
        port.stash_pop(&root, &clean.state_token).unwrap();
        assert!(!port.snapshot(&root).unwrap().unwrap().changes.is_empty());

        let initialized = std::env::temp_dir().join(format!("vibra-init-{}", Uuid::new_v4()));
        fs::create_dir_all(&initialized).unwrap();
        port.initialize(&initialized).unwrap();
        assert!(initialized.join(".git").is_dir());
        fs::remove_dir_all(initialized).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn individual_hunks_can_be_staged_unstaged_and_discarded() {
        let root = repository();
        let port = GitCliPort;
        let original = (1..=16)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        fs::write(root.join("hunks.txt"), &original).unwrap();
        git(&root, &["add", "hunks.txt"]);
        git(&root, &["commit", "-qm", "add hunk fixture"]);
        let changed = original
            .replace("line 2\n", "line two changed\n")
            .replace("line 14\n", "line fourteen changed\n");
        fs::write(root.join("hunks.txt"), changed).unwrap();

        let snapshot = port.snapshot(&root).unwrap().unwrap();
        let change = snapshot
            .changes
            .iter()
            .find(|change| change.path == "hunks.txt")
            .unwrap();
        let diff = port.diff(&root, change).unwrap();
        assert_eq!(diff.hunks.len(), 2);
        port.apply_hunk(
            &root,
            &diff.hunks[0],
            GitHunkAction::Stage,
            &snapshot.state_token,
        )
        .unwrap();

        let mixed = port.snapshot(&root).unwrap().unwrap();
        let change = mixed
            .changes
            .iter()
            .find(|change| change.path == "hunks.txt")
            .unwrap();
        assert!(change.staged && change.unstaged);
        let mixed_diff = port.diff(&root, change).unwrap();
        let staged_hunk = mixed_diff
            .hunks
            .iter()
            .find(|hunk| hunk.source == GitDiffHunkSource::Index)
            .unwrap();
        port.apply_hunk(
            &root,
            staged_hunk,
            GitHunkAction::Unstage,
            &mixed.state_token,
        )
        .unwrap();

        let unstaged = port.snapshot(&root).unwrap().unwrap();
        let change = unstaged
            .changes
            .iter()
            .find(|change| change.path == "hunks.txt")
            .unwrap();
        let diff = port.diff(&root, change).unwrap();
        port.apply_hunk(
            &root,
            &diff.hunks[0],
            GitHunkAction::Discard,
            &unstaged.state_token,
        )
        .unwrap();

        let contents = fs::read_to_string(root.join("hunks.txt")).unwrap();
        assert_eq!(contents.matches("changed").count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
