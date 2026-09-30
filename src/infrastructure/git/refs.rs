use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context as _, Result, bail};

use crate::ports::git::GitFileChange;

use super::parsing::{PorcelainFile, parse_name_status, parse_porcelain_status};
use super::*;

pub(super) fn untracked_paths(root: &Path) -> Result<Vec<String>> {
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

pub(super) fn collect_numstat(
    root: &Path,
    extra: &[&str],
    stats: &mut HashMap<String, (usize, usize)>,
) -> Result<()> {
    let mut arguments = vec!["diff"];
    arguments.extend_from_slice(&DIFF_PLAIN);
    arguments.extend(["--numstat", "-z"]);
    arguments.extend_from_slice(extra);
    let output = run_git(root, arguments)?;
    ensure_success(&output, "git diff --numstat")?;
    apply_numstat(&output.stdout, stats);
    Ok(())
}

pub(super) fn validate_revision(revision: &str) -> Result<()> {
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

pub(super) fn display_branch_ref(reference: &str) -> &str {
    reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/remotes/"))
        .unwrap_or(reference)
}

pub(super) fn resolve_commit(root: &Path, reference: &str) -> Result<String> {
    validate_revision(reference)?;
    if reference.is_empty() {
        bail!("Select a branch to compare");
    }
    rev_parse(root, &format!("{reference}^{{commit}}"))?
        .with_context(|| format!("Branch not found: {reference}"))
}

pub(super) fn current_branch(root: &Path) -> Result<String> {
    let output = run_git(root, ["rev-parse", "--abbrev-ref", "HEAD"])?;
    if !output.status.success() {
        return Ok("HEAD".into());
    }
    let name = git_stdout(&output);
    Ok(if name.is_empty() || name == "HEAD" {
        "detached".into()
    } else {
        name
    })
}

pub(super) fn rev_parse(root: &Path, rev: &str) -> Result<Option<String>> {
    validate_revision(rev)?;
    let output = run_git(root, ["rev-parse", "--verify", "--quiet", rev])?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = git_stdout(&output);
    Ok((!value.is_empty()).then_some(value))
}

pub(super) fn rev_count(root: &Path, range: &str) -> Result<usize> {
    let output = run_git(root, ["rev-list", "--count", range])?;
    if !output.status.success() {
        return Ok(0);
    }
    Ok(git_stdout(&output).parse().unwrap_or(0))
}

pub(super) fn merge_base(root: &Path, other: &str) -> Result<String> {
    validate_revision(other)?;
    let output = run_git(root, ["merge-base", "HEAD", other])?;
    ensure_success(&output, "git merge-base")?;
    let value = git_stdout(&output);
    if value.is_empty() {
        bail!("Git found no common ancestor with {other}");
    }
    Ok(value)
}

pub(super) fn short_ref_name(rev: &str) -> &str {
    rev.rsplit('/').next().unwrap_or(rev)
}

pub(super) fn compare_base(root: &Path, branch: &str) -> Result<Option<String>> {
    let default_base = default_base_branch(root)?;
    let upstream = rev_parse_symbolic(root, "@{upstream}")?;
    let on_default = default_base
        .as_deref()
        .is_some_and(|base| short_ref_name(base) == branch);
    Ok(if default_base.is_some() && !on_default {
        default_base
    } else {
        upstream.or(default_base)
    })
}

pub(super) fn default_base_branch(root: &Path) -> Result<Option<String>> {
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

pub(super) fn rev_parse_symbolic(root: &Path, rev: &str) -> Result<Option<String>> {
    let output = run_git(
        root,
        ["rev-parse", "--abbrev-ref", "--symbolic-full-name", rev],
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = git_stdout(&output);
    Ok((!value.is_empty() && value != "HEAD").then_some(value))
}

pub(super) fn diff_name_status(
    root: &Path,
    revision: &str,
    head: Option<&str>,
) -> Result<Vec<GitFileChange>> {
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

pub(super) fn collect_numstat_against(
    root: &Path,
    revision: &str,
    head: Option<&str>,
    stats: &mut HashMap<String, (usize, usize)>,
) -> Result<()> {
    validate_revision(revision)?;
    let mut extra = vec![revision];
    extra.extend(head);
    extra.push("--");
    collect_numstat(root, &extra, stats)
}

pub(super) fn cached_untracked_additions(
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

pub(super) fn read_untracked_additions(path: &Path, len: u64) -> std::io::Result<Option<usize>> {
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
pub(super) fn text_additions(bytes: &[u8]) -> Option<usize> {
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

pub(super) fn has_upstream(root: &Path) -> bool {
    rev_parse_symbolic(root, "@{upstream}")
        .ok()
        .flatten()
        .is_some()
}

pub(super) fn validate_relative_path(path: &str) -> Result<()> {
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
