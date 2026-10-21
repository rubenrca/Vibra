//! Branches, worktrees, stashes, and tags for the Repository view, plus the
//! explicit actions it runs on them.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::ports::git::{
    GitBranchInfo, GitCommit, GitRefAction, GitRepositoryRefs, GitStash, GitTag, GitWorktree,
};

use super::parsing::parse_porcelain_status;
use super::*;

const BRANCH_FORMAT: &str = concat!(
    "--format=%(refname)%00%(symref)%00%(objectname)%00%(upstream:short)%00",
    "%(upstream:track)%00%(HEAD)%00%(committerdate:short)%00%(contents:subject)",
);
const TAG_FORMAT: &str = concat!(
    "--format=%(refname:short)%00%(*objectname)%00%(objectname)%00",
    "%(creatordate:short)%00%(contents:subject)",
);
const TAG_LIMIT: usize = 200;

pub(super) fn repository_refs(root: &Path) -> Result<Option<GitRepositoryRefs>> {
    let Some(root) = repository_root(root)? else {
        return Ok(None);
    };
    let output = run_git(
        &root,
        [
            "for-each-ref",
            "--sort=-committerdate",
            BRANCH_FORMAT,
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    ensure_success(&output, "git list branches")?;
    let (branches, remotes) = parse_branches(&output.stdout);

    let output = run_git(&root, ["worktree", "list", "--porcelain", "-z"])?;
    ensure_success(&output, "git worktree list")?;
    let worktrees = parse_worktrees(&output.stdout);

    // An unborn repository has no stash ref; that is an empty list, not an error.
    let output = run_git(&root, ["stash", "list", "-z", "--format=%H%x00%cs%x00%gs"])?;
    let stashes = if output.status.success() {
        parse_stashes(&output.stdout)
    } else {
        Vec::new()
    };

    let output = run_git(
        &root,
        [
            "for-each-ref",
            "--sort=-creatordate",
            &format!("--count={TAG_LIMIT}"),
            TAG_FORMAT,
            "refs/tags",
        ],
    )?;
    ensure_success(&output, "git list tags")?;
    let tags = parse_tags(&output.stdout);

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
    let changes = parse_porcelain_status(&output.stdout).files.len();
    Ok(Some(GitRepositoryRefs {
        current: current_branch(&root)?,
        root,
        changes,
        branches,
        remotes,
        worktrees,
        stashes,
        tags,
    }))
}

pub(super) fn log_range(
    root: &Path,
    base: &str,
    head: &str,
    limit: usize,
) -> Result<Vec<GitCommit>> {
    let root = repository_root(root)?.context("the repository is no longer available")?;
    validate_revision(head)?;
    let range = if base.is_empty() {
        head.to_owned()
    } else {
        validate_revision(base)?;
        format!("{base}..{head}")
    };
    let output = run_git(
        &root,
        [
            "log",
            &format!("-{}", limit.clamp(1, 500)),
            "--topo-order",
            "-z",
            "--decorate=short",
            "--pretty=tformat:%H%x00%h%x00%s%x00%an%x00%as%x00%P%x00%D",
            &range,
            "--",
        ],
    )?;
    ensure_success(&output, "git log")?;
    Ok(super::parsing::parse_history(&output.stdout))
}

pub(super) fn run_ref_action(root: &Path, action: &GitRefAction) -> Result<()> {
    let root = repository_root(root)?.context("this project is not a Git repository")?;
    match action {
        GitRefAction::Switch(branch) => {
            validate_branch(branch)?;
            run_git_write(&root, &["switch", "--", branch], "git switch")?;
        }
        GitRefAction::SwitchAndPull(branch) => {
            validate_branch(branch)?;
            run_git_write(&root, &["switch", "--", branch], "git switch")?;
            if has_upstream(&root) {
                run_git_write(&root, &["pull", "--ff-only"], "git pull")?;
            }
        }
        GitRefAction::DeleteBranch { name, force } => {
            validate_branch(name)?;
            let flag = if *force { "-D" } else { "-d" };
            run_git_write(&root, &["branch", flag, "--", name], "git branch")?;
        }
        GitRefAction::Stash { message } => {
            let mut arguments = vec!["stash", "push", "--include-untracked"];
            let message = message.trim();
            if !message.is_empty() {
                arguments.extend(["-m", message]);
            }
            run_git_write(&root, &arguments, "git stash")?;
        }
        GitRefAction::StashApply(index) => {
            run_git_write(&root, &["stash", "apply", &stash_ref(*index)], "git stash")?;
        }
        GitRefAction::StashPop(index) => {
            run_git_write(&root, &["stash", "pop", &stash_ref(*index)], "git stash")?;
        }
        GitRefAction::StashDrop(index) => {
            run_git_write(&root, &["stash", "drop", &stash_ref(*index)], "git stash")?;
        }
        GitRefAction::AddWorktree(path) => {
            let path = worktree_path(path)?;
            if path.exists() {
                bail!("{} already exists", path.display());
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("could not create {}", parent.display()))?;
            }
            run_git_write(
                &root,
                &["worktree", "add", "--detach", "--", &path.to_string_lossy()],
                "git worktree add",
            )?;
        }
        GitRefAction::RemoveWorktree(path) => {
            let path = worktree_path(path)?;
            run_git_write(
                &root,
                &["worktree", "remove", "--", &path.to_string_lossy()],
                "git worktree remove",
            )?;
        }
    }
    Ok(())
}

fn stash_ref(index: usize) -> String {
    format!("stash@{{{index}}}")
}

fn validate_branch(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('-')
        || name.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        bail!("invalid branch name: {name}");
    }
    Ok(())
}

fn worktree_path(path: &Path) -> Result<&Path> {
    if !path.is_absolute() || path.to_str().is_none_or(|text| text.contains('\n')) {
        bail!("invalid worktree path: {}", path.display());
    }
    Ok(path)
}

/// `for-each-ref` records: one per line, NUL-separated fields. The subject is
/// last, so a stray NUL inside it can never shift the other fields.
fn parse_branches(output: &[u8]) -> (Vec<GitBranchInfo>, Vec<GitBranchInfo>) {
    let mut branches = Vec::new();
    let mut remotes = Vec::new();
    for line in output.split(|byte| *byte == b'\n') {
        let mut fields = line.splitn(8, |byte| *byte == 0);
        let mut next = || String::from_utf8_lossy(fields.next().unwrap_or_default()).into_owned();
        let (reference, symbolic, head, upstream, track, current, date, subject) = (
            next(),
            next(),
            next(),
            next(),
            next(),
            next(),
            next(),
            next(),
        );
        if !symbolic.is_empty() {
            continue;
        }
        let (remote, name) = if let Some(name) = reference.strip_prefix("refs/heads/") {
            (false, name)
        } else if let Some(name) = reference.strip_prefix("refs/remotes/") {
            (true, name)
        } else {
            continue;
        };
        let (ahead, behind, gone) = parse_track(&track);
        let branch = GitBranchInfo {
            name: name.to_owned(),
            upstream: (!upstream.is_empty()).then_some(upstream),
            ahead,
            behind,
            gone,
            head,
            current: current == "*",
            subject,
            date,
        };
        if remote {
            remotes.push(branch);
        } else {
            branches.push(branch);
        }
    }
    (branches, remotes)
}

/// `[ahead 2, behind 1]`, `[gone]`, or empty.
fn parse_track(track: &str) -> (usize, usize, bool) {
    let inner = track.trim().trim_start_matches('[').trim_end_matches(']');
    let mut ahead = 0;
    let mut behind = 0;
    for part in inner.split(',').map(str::trim) {
        if let Some(count) = part.strip_prefix("ahead ") {
            ahead = count.parse().unwrap_or(0);
        } else if let Some(count) = part.strip_prefix("behind ") {
            behind = count.parse().unwrap_or(0);
        }
    }
    (ahead, behind, inner == "gone")
}

/// `git worktree list --porcelain -z`: NUL-terminated attributes, with an
/// empty attribute closing each worktree. The first one is the main tree.
fn parse_worktrees(output: &[u8]) -> Vec<GitWorktree> {
    let mut worktrees: Vec<GitWorktree> = Vec::new();
    let mut current: Option<GitWorktree> = None;
    for attribute in output.split(|byte| *byte == 0) {
        if attribute.is_empty() {
            worktrees.extend(current.take());
            continue;
        }
        let (key, value) = match attribute.iter().position(|byte| *byte == b' ') {
            Some(space) => (&attribute[..space], &attribute[space + 1..]),
            None => (attribute, &[][..]),
        };
        match key {
            b"worktree" => {
                worktrees.extend(current.take());
                current = Some(GitWorktree {
                    path: PathBuf::from(OsString::from_vec(value.to_vec())),
                    head: String::new(),
                    branch: None,
                    main: worktrees.is_empty(),
                    prunable: false,
                });
            }
            b"HEAD" => {
                if let Some(worktree) = current.as_mut() {
                    worktree.head = String::from_utf8_lossy(value).into_owned();
                }
            }
            b"branch" => {
                if let Some(worktree) = current.as_mut() {
                    let branch = String::from_utf8_lossy(value);
                    worktree.branch = Some(
                        branch
                            .strip_prefix("refs/heads/")
                            .unwrap_or(&branch)
                            .to_owned(),
                    );
                }
            }
            b"prunable" => {
                if let Some(worktree) = current.as_mut() {
                    worktree.prunable = true;
                }
            }
            _ => {}
        }
    }
    worktrees.extend(current);
    worktrees
}

/// `git stash list -z --format=%H%x00%cs%x00%gs`: three NUL-separated fields
/// per entry, newest (`stash@{0}`) first.
fn parse_stashes(output: &[u8]) -> Vec<GitStash> {
    output
        .split(|byte| *byte == 0)
        .collect::<Vec<_>>()
        .chunks_exact(3)
        .enumerate()
        .map(|(index, fields)| {
            let text = |field: &[u8]| String::from_utf8_lossy(field).trim().to_owned();
            GitStash {
                index,
                oid: text(fields[0]),
                date: text(fields[1]),
                subject: text(fields[2]),
            }
        })
        .filter(|stash| !stash.oid.is_empty())
        .collect()
}

fn parse_tags(output: &[u8]) -> Vec<GitTag> {
    output
        .split(|byte| *byte == b'\n')
        .filter_map(|line| {
            let mut fields = line.splitn(5, |byte| *byte == 0);
            let mut next =
                || String::from_utf8_lossy(fields.next().unwrap_or_default()).into_owned();
            let (name, peeled, object, date, subject) = (next(), next(), next(), next(), next());
            (!name.is_empty()).then_some(GitTag {
                name,
                target: if peeled.is_empty() { object } else { peeled },
                subject,
                date,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_split_local_and_remote_and_keep_tracking() {
        let output = [
            "refs/heads/main\0\0aaa\0origin/main\0[ahead 2, behind 1]\0*\x002026-10-01\0Fix\0it",
            "refs/heads/old\0\0bbb\0origin/old\0[gone]\0 \x002026-09-01\0Old",
            "refs/remotes/origin/HEAD\0refs/remotes/origin/main\0aaa\0\0\0 \0\0",
            "refs/remotes/origin/main\0\0ccc\0\0\0 \x002026-10-02\0Remote",
            "",
        ]
        .join("\n");
        let (branches, remotes) = parse_branches(output.as_bytes());
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].name, "main");
        assert!(branches[0].current);
        assert_eq!((branches[0].ahead, branches[0].behind), (2, 1));
        assert_eq!(branches[0].upstream.as_deref(), Some("origin/main"));
        assert_eq!(branches[0].subject, "Fix\0it");
        assert!(branches[1].gone && !branches[1].current);
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin/main");
        assert_eq!(remotes[0].head, "ccc");
    }

    #[test]
    fn worktrees_mark_main_detached_and_prunable_trees() {
        let output = concat!(
            "worktree /repo\0HEAD aaa\0branch refs/heads/main\0\0",
            "worktree /wt/pr 1\0HEAD bbb\0detached\0\0",
            "worktree /wt/gone\0HEAD ccc\0branch refs/heads/feat\0prunable gitdir file points to non-existent location\0\0",
        );
        let worktrees = parse_worktrees(output.as_bytes());
        assert_eq!(worktrees.len(), 3);
        assert!(worktrees[0].main && !worktrees[1].main);
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert_eq!(worktrees[1].path, PathBuf::from("/wt/pr 1"));
        assert!(worktrees[1].branch.is_none());
        assert!(worktrees[2].prunable);
        let refs = GitRepositoryRefs {
            worktrees,
            ..Default::default()
        };
        assert!(refs.worktree_for("main").is_none());
        assert_eq!(
            refs.worktree_for("feat").map(|tree| tree.path.clone()),
            Some(PathBuf::from("/wt/gone"))
        );
    }

    #[test]
    fn stashes_and_tags_parse_their_records() {
        let stashes = parse_stashes(
            b"aaa\x002026-10-01\x00On main: wip\x00bbb\x002026-09-01\x00WIP on feat: abc x\x00",
        );
        assert_eq!(stashes.len(), 2);
        assert_eq!(stashes[1].index, 1);
        assert_eq!(stashes[0].subject, "On main: wip");
        let tags = parse_tags(
            b"v2\x00ddd\x00eee\x002026-10-01\x00Release\nv1\x00\x00fff\x002026-01-01\x00\n",
        );
        assert_eq!(tags[0].target, "ddd");
        assert_eq!(tags[1].target, "fff");
        assert_eq!(parse_track(""), (0, 0, false));
    }

    #[test]
    fn ref_actions_reject_option_like_names_and_relative_paths() {
        assert!(validate_branch("-D").is_err());
        assert!(validate_branch("feature/a b").is_err());
        assert!(validate_branch("feature/search").is_ok());
        assert!(worktree_path(Path::new("relative")).is_err());
        assert_eq!(stash_ref(3), "stash@{3}");
    }
}
