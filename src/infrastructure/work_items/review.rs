//! Adapt GitHub's immutable file blobs and patches to the shared Git review model.

use crate::domain::work_items::{WorkDiffFile, WorkItem};
use crate::ports::git::{GitDiff, GitDiffRow, GitDiffRowKind, GitDiffSources};

pub fn load_review_file(item: &WorkItem, file: &WorkDiffFile) -> (GitDiff, GitDiffSources) {
    let content = super::load_full_file(item, file);
    prepare_review_file(file, content)
}

fn prepare_review_file(
    file: &WorkDiffFile,
    content: anyhow::Result<String>,
) -> (GitDiff, GitDiffSources) {
    let mut diff = crate::infrastructure::git::parse_diff_patch(&file.path, file.patch.as_bytes());
    let complete = !diff.truncated
        && diff.additions as u64 == file.additions
        && diff.deletions as u64 == file.deletions;
    let mut sources = GitDiffSources::default();
    match content {
        Ok(text) if complete => {
            if file.removed {
                sources.old = Some(text);
                sources.new = Some(String::new());
            } else {
                sources.old = reconstruct_old(&diff, &text);
                sources.new = Some(text);
                if sources.old.is_none() {
                    diff.rows.push(notice(
                        "Full context does not match the patch. Showing the available hunks.",
                    ));
                }
            }
        }
        Ok(text) => {
            // Omitted/truncated patches cannot be used to invent changed lines.
            diff.rows = vec![notice(concat!(
                "GitHub omitted part of the patch. ",
                "Showing file content without complete diff highlights.",
            ))];
            diff.rows
                .extend(text.lines().enumerate().map(|(index, text)| GitDiffRow {
                    old_line: file.removed.then_some(index + 1),
                    new_line: (!file.removed).then_some(index + 1),
                    kind: if file.removed {
                        GitDiffRowKind::Deletion
                    } else {
                        GitDiffRowKind::Context
                    },
                    text: text.into(),
                }));
        }
        Err(error) => diff
            .rows
            .push(notice(&format!("Full file unavailable: {error:#}"))),
    }
    if diff.rows.is_empty() {
        diff.rows.push(notice("No text changes in this file."));
    }
    diff.additions = file.additions as usize;
    diff.deletions = file.deletions as usize;
    (diff, sources)
}

fn notice(text: &str) -> GitDiffRow {
    GitDiffRow {
        old_line: None,
        new_line: None,
        kind: GitDiffRowKind::Notice,
        text: text.into(),
    }
}

/// Undo a complete patch against its exact new blob. Validate both the line
/// numbers and text before using the result for highlighting and context folds.
fn reconstruct_old(diff: &GitDiff, text: &str) -> Option<String> {
    let new: Vec<_> = text.lines().collect();
    let mut old = Vec::new();
    let mut cursor = 0;
    for row in &diff.rows {
        if !matches!(
            row.kind,
            GitDiffRowKind::Context | GitDiffRowKind::Addition | GitDiffRowKind::Deletion
        ) {
            continue;
        }
        let gap = if let Some(line) = row.old_line {
            line.checked_sub(1)?.checked_sub(old.len())?
        } else {
            row.new_line?.checked_sub(1)?.checked_sub(cursor)?
        };
        old.extend_from_slice(new.get(cursor..cursor.checked_add(gap)?)?);
        cursor += gap;
        if let Some(line) = row.new_line {
            if line != cursor + 1 || *new.get(cursor)? != row.text {
                return None;
            }
            cursor += 1;
        }
        if row.old_line.is_some() {
            old.push(row.text.as_str());
        }
    }
    old.extend_from_slice(new.get(cursor..)?);
    // Preserve a final empty line when DiffDocument splits the source into lines.
    Some(if old.is_empty() {
        String::new()
    } else {
        format!("{}\n", old.join("\n"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_patch_reconstructs_both_sides_for_the_shared_context_folds() {
        let file = WorkDiffFile {
            path: "lib.rs".into(),
            additions: 2,
            deletions: 2,
            patch: "@@ -2,2 +2,3 @@\n b\n-old\n+new\n+inserted\n@@ -6 +6,0 @@\n-end".into(),
            ..Default::default()
        };
        let (diff, sources) =
            prepare_review_file(&file, Ok("a\nb\nnew\ninserted\nd\ne\ntail\n".into()));
        assert_eq!(sources.old.as_deref(), Some("a\nb\nold\nd\ne\nend\ntail\n"));
        assert_eq!(
            sources.new.as_deref(),
            Some("a\nb\nnew\ninserted\nd\ne\ntail\n")
        );
        assert_eq!(diff.rows[2].text, "old");
    }

    #[test]
    fn unavailable_or_mismatched_content_keeps_the_remote_patch() {
        let file = WorkDiffFile {
            path: "lib.rs".into(),
            additions: 1,
            deletions: 1,
            patch: "@@ -1 +1 @@\n-old\n+new".into(),
            ..Default::default()
        };
        for content in [Err(anyhow::anyhow!("offline")), Ok("different\n".into())] {
            let (diff, sources) = prepare_review_file(&file, content);
            assert!(sources.old.is_none());
            assert!(
                diff.rows
                    .iter()
                    .any(|row| row.text == "new" && row.kind == GitDiffRowKind::Addition)
            );
            assert!(
                diff.rows
                    .iter()
                    .any(|row| row.kind == GitDiffRowKind::Notice)
            );
        }
    }

    #[test]
    fn source_reconstruction_handles_insertions_deletions_and_empty_lines() {
        for (patch, new, expected_old) in [
            ("@@ -0,0 +1 @@\n+first", "first\n", ""),
            ("@@ -0,0 +1 @@\n+first", "first\nlast", "last\n"),
            ("@@ -2 +1,0 @@\n-last", "first\n", "first\nlast\n"),
            ("@@ -1 +1 @@\n-old\n+new", "new\n\n", "old\n\n"),
            ("@@ -1,2 +0,0 @@\n-first\n-", "", "first\n\n"),
        ] {
            let diff = crate::infrastructure::git::parse_diff_patch("test.txt", patch.as_bytes());
            assert_eq!(reconstruct_old(&diff, new).as_deref(), Some(expected_old));
        }
    }

    #[test]
    fn omitted_patches_do_not_invent_changed_lines_and_deleted_files_use_old_blob() {
        let mut file = WorkDiffFile {
            path: "lib.rs".into(),
            additions: 50,
            ..Default::default()
        };
        let (diff, sources) = prepare_review_file(&file, Ok("fn main() {}\n".into()));
        assert!(sources.old.is_none() && sources.new.is_none());
        assert_eq!(diff.rows[0].kind, GitDiffRowKind::Notice);
        assert_eq!(diff.rows[1].kind, GitDiffRowKind::Context);

        file.removed = true;
        file.additions = 0;
        file.deletions = 1;
        file.patch = "@@ -1 +0,0 @@\n-fn main() {}".into();
        let (diff, sources) = prepare_review_file(&file, Ok("fn main() {}\n".into()));
        assert_eq!(sources.old.as_deref(), Some("fn main() {}\n"));
        assert_eq!(sources.new.as_deref(), Some(""));
        assert_eq!(diff.rows[1].kind, GitDiffRowKind::Deletion);
    }
}
