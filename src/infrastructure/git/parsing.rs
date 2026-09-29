//! Pure parsers for Git status, numstat, history, and unified patches.

use std::collections::HashMap;

use super::MAX_DIFF_BYTES;
use crate::ports::git::{GitCommit, GitDiffRow, GitDiffRowKind, GitFileChange, GitFileStatus};

pub(super) fn parse_porcelain_status(stdout: &[u8]) -> PorcelainStatus {
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
pub(super) struct PorcelainStatus {
    pub(super) branch: String,
    pub(super) ahead: usize,
    pub(super) behind: usize,
    pub(super) files: Vec<PorcelainFile>,
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
pub(super) struct PorcelainFile {
    pub(super) index: char,
    pub(super) worktree: char,
    pub(super) path: String,
    pub(super) old_path: Option<String>,
}

impl PorcelainFile {
    pub(super) fn untracked(&self) -> bool {
        self.index == '?' && self.worktree == '?'
    }
}

pub(super) fn empty_diff_notice(binary: bool) -> GitDiffRow {
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

pub(super) fn file_status(index: char, worktree: char) -> GitFileStatus {
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

pub(super) fn change_priority(change: &GitFileChange) -> u8 {
    match change.status {
        GitFileStatus::Conflicted => 0,
        _ if change.staged => 1,
        GitFileStatus::Modified | GitFileStatus::TypeChanged => 2,
        GitFileStatus::Added | GitFileStatus::Untracked => 3,
        GitFileStatus::Renamed | GitFileStatus::Copied => 4,
        GitFileStatus::Deleted => 5,
    }
}

pub(super) fn parse_name_status(output: &[u8]) -> Vec<GitFileChange> {
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

pub(super) fn apply_numstat(stdout: &[u8], stats: &mut HashMap<String, (usize, usize)>) {
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

pub(super) fn parse_history(output: &[u8]) -> Vec<GitCommit> {
    // Git subjects and author names may contain newlines or control characters.
    // A NUL-separated tformat record has seven fields and a final separator.
    let mut commits = Vec::new();
    for fields in output
        .split(|byte| *byte == 0)
        .collect::<Vec<_>>()
        .chunks_exact(7)
    {
        let [sha, short_sha, subject, author, date, parents, refs] = fields else {
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
            refs: parse_decorations(&String::from_utf8_lossy(refs)),
        });
    }
    commits
}

/// `HEAD -> main, origin/main, tag: v1` → `["main", "origin/main", "v1"]`.
fn parse_decorations(decorations: &str) -> Vec<String> {
    decorations
        .split(", ")
        .map(|name| {
            name.trim()
                .trim_start_matches("HEAD -> ")
                .trim_start_matches("tag: ")
        })
        .filter(|name| !name.is_empty() && *name != "HEAD" && !name.ends_with("/HEAD"))
        .map(str::to_owned)
        .collect()
}

pub(super) fn append_patch(
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
