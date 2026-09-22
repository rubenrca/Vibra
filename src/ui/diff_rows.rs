//! Pure row model for the Git review list.
//!
//! The whole review is one virtualized list at line granularity: every section
//! label, file header, hunk header, diff line, and comment is its own row (the
//! flat model Zed's project diff uses). Only the visible slice materializes, a
//! collapsed file's body rows are absent rather than hidden, and the split
//! layout is a re-flatten of the same prepared rows.

use crate::ports::git::{GitDiffRow, GitDiffRowKind};

/// How changed lines are laid out. Persisted in settings (`diffSplit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DiffLayout {
    /// One column: deletions above additions.
    #[default]
    Unified,
    /// Two columns: old on the left, new on the right, paired per change run.
    Split,
}

impl DiffLayout {
    pub fn from_split(split: bool) -> Self {
        if split { Self::Split } else { Self::Unified }
    }

    pub fn is_split(self) -> bool {
        self == Self::Split
    }
}

/// Which file version a review comment cites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommentSide {
    Old,
    New,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommentAnchor {
    pub path: String,
    pub side: CommentSide,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewComment {
    pub id: u64,
    pub anchor: CommentAnchor,
    /// The cited line as it read when the comment was written.
    pub excerpt: String,
    pub body: String,
}

/// One row of a file body, indexing the prepared diff rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyRow {
    /// A unified line, or a full-width hunk / section / notice row.
    Line(usize),
    /// A split pair: a deletion (or context) left, an addition (or context) right.
    Split {
        left: Option<usize>,
        right: Option<usize>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewRow {
    /// Label above the staged files in the working tree.
    StagedSection,
    FileHeader {
        file: usize,
    },
    /// Placeholder while an expanded file's diff loads.
    FileLoading {
        file: usize,
    },
    Body {
        file: usize,
        row: BodyRow,
    },
    Comment {
        file: usize,
        comment: usize,
    },
    Draft {
        file: usize,
    },
    /// An animated, clipped stand-in for a body that is folding open or shut.
    Folding {
        file: usize,
    },
}

impl ReviewRow {
    pub fn file(self) -> Option<usize> {
        match self {
            Self::StagedSection => None,
            Self::FileHeader { file }
            | Self::FileLoading { file }
            | Self::Body { file, .. }
            | Self::Comment { file, .. }
            | Self::Draft { file }
            | Self::Folding { file } => Some(file),
        }
    }
}

/// Everything flattening needs to know about one file.
pub struct FlattenFile<'a> {
    pub path: &'a str,
    /// First staged file in the working tree gets a section label above it.
    pub starts_staged_section: bool,
    pub expanded: bool,
    pub folding: bool,
    /// Prepared diff rows, once loaded.
    pub rows: Option<&'a [GitDiffRow]>,
}

pub fn flatten(
    files: &[FlattenFile<'_>],
    layout: DiffLayout,
    comments: &[ReviewComment],
    draft: Option<&CommentAnchor>,
) -> Vec<ReviewRow> {
    let mut out = Vec::new();
    for (file, entry) in files.iter().enumerate() {
        if entry.starts_staged_section {
            out.push(ReviewRow::StagedSection);
        }
        out.push(ReviewRow::FileHeader { file });
        if entry.folding {
            out.push(ReviewRow::Folding { file });
            continue;
        }
        if !entry.expanded {
            continue;
        }
        let Some(rows) = entry.rows else {
            out.push(ReviewRow::FileLoading { file });
            continue;
        };
        let file_comments: Vec<(usize, &ReviewComment)> = comments
            .iter()
            .enumerate()
            .filter(|(_, comment)| comment.anchor.path == entry.path)
            .collect();
        let file_draft = draft.filter(|anchor| anchor.path == entry.path);
        for row in body_rows(rows, layout) {
            out.push(ReviewRow::Body { file, row });
            for (side, line) in body_row_anchors(rows, row).into_iter().flatten() {
                for (index, comment) in &file_comments {
                    if comment.anchor.side == side && comment.anchor.line == line {
                        out.push(ReviewRow::Comment {
                            file,
                            comment: *index,
                        });
                    }
                }
                if file_draft.is_some_and(|anchor| anchor.side == side && anchor.line == line) {
                    out.push(ReviewRow::Draft { file });
                }
            }
        }
    }
    out
}

pub fn body_rows(rows: &[GitDiffRow], layout: DiffLayout) -> Vec<BodyRow> {
    match layout {
        DiffLayout::Unified => (0..rows.len()).map(BodyRow::Line).collect(),
        DiffLayout::Split => split_rows(rows),
    }
}

/// Pair each run of deletions with the additions that follow it, row by row;
/// the longer side continues against blank cells. Context shows on both sides.
pub fn split_rows(rows: &[GitDiffRow]) -> Vec<BodyRow> {
    let mut out = Vec::with_capacity(rows.len());
    let mut index = 0;
    while index < rows.len() {
        match rows[index].kind {
            GitDiffRowKind::Context => {
                out.push(BodyRow::Split {
                    left: Some(index),
                    right: Some(index),
                });
                index += 1;
            }
            GitDiffRowKind::Deletion | GitDiffRowKind::Addition => {
                let deletions_start = index;
                while index < rows.len() && rows[index].kind == GitDiffRowKind::Deletion {
                    index += 1;
                }
                let additions_start = index;
                while index < rows.len() && rows[index].kind == GitDiffRowKind::Addition {
                    index += 1;
                }
                let deletions = deletions_start..additions_start;
                let additions = additions_start..index;
                for offset in 0..deletions.len().max(additions.len()) {
                    out.push(BodyRow::Split {
                        left: (offset < deletions.len()).then(|| deletions.start + offset),
                        right: (offset < additions.len()).then(|| additions.start + offset),
                    });
                }
            }
            GitDiffRowKind::Hunk | GitDiffRowKind::Section | GitDiffRowKind::Notice => {
                out.push(BodyRow::Line(index));
                index += 1;
            }
        }
    }
    out
}

/// The line a comment on this diff row cites: additions and context cite the
/// new file, deletions the old one.
pub fn line_anchor(row: &GitDiffRow) -> Option<(CommentSide, usize)> {
    match row.kind {
        GitDiffRowKind::Addition | GitDiffRowKind::Context => {
            row.new_line.map(|line| (CommentSide::New, line))
        }
        GitDiffRowKind::Deletion => row.old_line.map(|line| (CommentSide::Old, line)),
        _ => None,
    }
}

/// Commentable anchors of a body row, `[left, right]`. A split row's left
/// cell only takes comments on deletions: its context twin on the right
/// already cites the same line in the new file.
pub fn body_row_anchors(rows: &[GitDiffRow], row: BodyRow) -> [Option<(CommentSide, usize)>; 2] {
    match row {
        BodyRow::Line(index) => [rows.get(index).and_then(line_anchor), None],
        BodyRow::Split { left, right } => [
            left.and_then(|index| rows.get(index))
                .filter(|row| row.kind == GitDiffRowKind::Deletion)
                .and_then(line_anchor),
            right
                .and_then(|index| rows.get(index))
                .and_then(line_anchor),
        ],
    }
}

/// Plain-text review handed to the agent's terminal. It is pasted, never
/// submitted, so the user can still edit it before pressing Enter.
pub fn review_prompt(comments: &[ReviewComment]) -> String {
    let mut prompt = String::from("Please address these review comments on the current diff:\n");
    for (index, comment) in comments.iter().enumerate() {
        let side = match comment.anchor.side {
            CommentSide::New => "",
            CommentSide::Old => " (removed line)",
        };
        prompt.push_str(&format!(
            "\n{}. {}:{}{}\n",
            index + 1,
            comment.anchor.path,
            comment.anchor.line,
            side
        ));
        let excerpt = comment.excerpt.trim();
        if !excerpt.is_empty() {
            prompt.push_str(&format!("   > {excerpt}\n"));
        }
        for line in comment.body.trim().lines() {
            prompt.push_str(&format!("   {line}\n"));
        }
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: GitDiffRowKind, old: Option<usize>, new: Option<usize>) -> GitDiffRow {
        GitDiffRow {
            old_line: old,
            new_line: new,
            kind,
            text: String::new(),
        }
    }

    fn sample() -> Vec<GitDiffRow> {
        vec![
            row(GitDiffRowKind::Hunk, None, None),
            row(GitDiffRowKind::Context, Some(1), Some(1)),
            row(GitDiffRowKind::Deletion, Some(2), None),
            row(GitDiffRowKind::Deletion, Some(3), None),
            row(GitDiffRowKind::Addition, None, Some(2)),
            row(GitDiffRowKind::Context, Some(4), Some(3)),
            row(GitDiffRowKind::Addition, None, Some(4)),
        ]
    }

    #[test]
    fn split_pairs_deletion_runs_with_following_additions() {
        assert_eq!(
            split_rows(&sample()),
            vec![
                BodyRow::Line(0),
                BodyRow::Split {
                    left: Some(1),
                    right: Some(1)
                },
                BodyRow::Split {
                    left: Some(2),
                    right: Some(4)
                },
                BodyRow::Split {
                    left: Some(3),
                    right: None
                },
                BodyRow::Split {
                    left: Some(5),
                    right: Some(5)
                },
                BodyRow::Split {
                    left: None,
                    right: Some(6)
                },
            ]
        );
    }

    #[test]
    fn collapsed_files_have_no_body_rows_and_loading_files_a_placeholder() {
        let rows = sample();
        let files = [
            FlattenFile {
                path: "a.rs",
                starts_staged_section: false,
                expanded: false,
                folding: false,
                rows: Some(&rows),
            },
            FlattenFile {
                path: "b.rs",
                starts_staged_section: true,
                expanded: true,
                folding: false,
                rows: None,
            },
            FlattenFile {
                path: "c.rs",
                starts_staged_section: false,
                expanded: true,
                folding: true,
                rows: Some(&rows),
            },
        ];
        assert_eq!(
            flatten(&files, DiffLayout::Unified, &[], None),
            vec![
                ReviewRow::FileHeader { file: 0 },
                ReviewRow::StagedSection,
                ReviewRow::FileHeader { file: 1 },
                ReviewRow::FileLoading { file: 1 },
                ReviewRow::FileHeader { file: 2 },
                ReviewRow::Folding { file: 2 },
            ]
        );
    }

    #[test]
    fn comments_and_drafts_follow_the_line_they_cite() {
        let rows = sample();
        let files = [FlattenFile {
            path: "a.rs",
            starts_staged_section: false,
            expanded: true,
            folding: false,
            rows: Some(&rows),
        }];
        let comment = |id, side, line| ReviewComment {
            id,
            anchor: CommentAnchor {
                path: "a.rs".into(),
                side,
                line,
            },
            excerpt: String::new(),
            body: "note".into(),
        };
        let comments = [
            comment(1, CommentSide::Old, 3),
            comment(2, CommentSide::New, 2),
            comment(3, CommentSide::New, 99),
        ];
        let draft = CommentAnchor {
            path: "a.rs".into(),
            side: CommentSide::New,
            line: 3,
        };
        let unified = flatten(&files, DiffLayout::Unified, &comments, Some(&draft));
        let position = |target: ReviewRow| unified.iter().position(|row| *row == target).unwrap();
        assert_eq!(
            position(ReviewRow::Comment {
                file: 0,
                comment: 0
            }),
            position(ReviewRow::Body {
                file: 0,
                row: BodyRow::Line(3)
            }) + 1
        );
        assert_eq!(
            position(ReviewRow::Comment {
                file: 0,
                comment: 1
            }),
            position(ReviewRow::Body {
                file: 0,
                row: BodyRow::Line(4)
            }) + 1
        );
        assert_eq!(
            position(ReviewRow::Draft { file: 0 }),
            position(ReviewRow::Body {
                file: 0,
                row: BodyRow::Line(5)
            }) + 1
        );
        // A comment whose line left the diff is kept but not shown.
        assert!(!unified.contains(&ReviewRow::Comment {
            file: 0,
            comment: 2
        }));

        // Split: both halves of a paired row carry their own comments.
        let split = flatten(&files, DiffLayout::Split, &comments, None);
        let paired = split
            .iter()
            .position(|row| {
                *row == ReviewRow::Body {
                    file: 0,
                    row: BodyRow::Split {
                        left: Some(2),
                        right: Some(4),
                    },
                }
            })
            .unwrap();
        assert_eq!(
            split[paired + 1],
            ReviewRow::Comment {
                file: 0,
                comment: 1
            }
        );
        let lone_deletion = split
            .iter()
            .position(|row| {
                *row == ReviewRow::Body {
                    file: 0,
                    row: BodyRow::Split {
                        left: Some(3),
                        right: None,
                    },
                }
            })
            .unwrap();
        assert_eq!(
            split[lone_deletion + 1],
            ReviewRow::Comment {
                file: 0,
                comment: 0
            }
        );
    }

    #[test]
    fn split_context_only_takes_comments_on_the_new_side() {
        let rows = sample();
        assert_eq!(
            body_row_anchors(
                &rows,
                BodyRow::Split {
                    left: Some(1),
                    right: Some(1)
                }
            ),
            [None, Some((CommentSide::New, 1))]
        );
        assert_eq!(body_row_anchors(&rows, BodyRow::Line(0)), [None, None]);
    }

    #[test]
    fn review_prompt_cites_each_line_with_its_excerpt() {
        let prompt = review_prompt(&[ReviewComment {
            id: 1,
            anchor: CommentAnchor {
                path: "src/main.rs".into(),
                side: CommentSide::Old,
                line: 7,
            },
            excerpt: "    let x = 1;".into(),
            body: "Why was this removed?\nIt is used below.".into(),
        }]);
        assert!(prompt.contains("1. src/main.rs:7 (removed line)\n"));
        assert!(prompt.contains("   > let x = 1;\n"));
        assert!(prompt.contains("   Why was this removed?\n   It is used below.\n"));
    }
}
