use crate::ports::git::GitDiff;
use crate::ui::syntax::{SyntaxSpan, expand_tabs, highlight_diff_rows};

/// Prepared, immutable data consumed by the GPUI diff renderer.
///
/// Building a document performs the CPU-heavy syntax pass and layout scan, so
/// callers should construct it on a background executor and share it by `Arc`.
#[derive(Debug)]
pub struct DiffDocument {
    pub diff: GitDiff,
    pub highlights: Vec<Vec<SyntaxSpan>>,
    pub widest_row_index: usize,
}

impl DiffDocument {
    pub fn prepare(diff: GitDiff) -> Self {
        let highlights = highlight_diff_rows(&diff.path, &diff.rows);
        let widest_row_index = diff
            .rows
            .iter()
            .enumerate()
            .max_by_key(|(_, row)| expand_tabs(&row.text).chars().count())
            .map(|(index, _)| index)
            .unwrap_or(0);

        Self {
            diff,
            highlights,
            widest_row_index,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::git::{GitDiffRow, GitDiffRowKind};

    #[test]
    fn prepares_highlights_and_layout_metadata_once() {
        let document = DiffDocument::prepare(GitDiff {
            path: "src/main.rs".into(),
            rows: vec![
                GitDiffRow {
                    old_line: Some(1),
                    new_line: Some(1),
                    kind: GitDiffRowKind::Context,
                    text: "fn short() {}".into(),
                },
                GitDiffRow {
                    old_line: None,
                    new_line: Some(2),
                    kind: GitDiffRowKind::Addition,
                    text: "\tlet message = \"a much wider line\";".into(),
                },
            ],
            additions: 1,
            deletions: 0,
            binary: false,
            truncated: false,
        });

        assert_eq!(document.highlights.len(), document.diff.rows.len());
        assert!(!document.highlights[1].is_empty());
        assert_eq!(document.widest_row_index, 1);
    }
}
