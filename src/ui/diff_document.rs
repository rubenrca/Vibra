use gpui::SharedString;

use crate::ports::git::{GitDiff, GitDiffRow, GitDiffRowKind, GitDiffSources};
use crate::ui::syntax::{Highlighter, SyntaxSpan, expand_tabs, highlight_diff_rows};

/// Prepared, immutable data consumed by the GPUI diff renderer.
///
/// Building a document performs the CPU-heavy syntax pass and layout scan, so
/// callers should construct it on a background executor and share it by `Arc`.
#[derive(Debug)]
pub struct DiffDocument {
    pub diff: GitDiff,
    pub highlights: Vec<Vec<SyntaxSpan>>,
    /// Expanded text is shared by measurement and paint, without per-frame copies.
    pub display_lines: Vec<SharedString>,
    /// Character columns of the widest code row; drives horizontal scrolling.
    pub widest_columns: usize,
    /// Largest old or new line number, which sizes the gutters.
    pub max_line_number: usize,
    /// True when highlights came from the whole old/new files rather than the
    /// hunks alone, so constructs opened outside a hunk color correctly.
    #[cfg(test)]
    pub full_context: bool,
}

impl DiffDocument {
    #[cfg(test)]
    pub fn prepare(diff: GitDiff) -> Self {
        Self::prepare_with_sources(diff, None)
    }

    pub fn prepare_with_sources(diff: GitDiff, sources: Option<&GitDiffSources>) -> Self {
        let display_lines: Vec<SharedString> = diff
            .rows
            .iter()
            .map(|row| expand_tabs(&row.text).into())
            .collect();
        let texts: Vec<&str> = display_lines.iter().map(SharedString::as_ref).collect();
        let full = sources.and_then(|sources| full_context_highlights(&diff, &texts, sources));
        #[cfg(test)]
        let full_context = full.is_some();
        let highlights =
            full.unwrap_or_else(|| highlight_diff_rows(&diff.path, &diff.rows, &texts));
        let widest_columns = display_lines
            .iter()
            .zip(&diff.rows)
            .filter(|(_, row)| is_code_row(row))
            .map(|(line, _)| line.chars().count())
            .max()
            .unwrap_or(0);
        let max_line_number = diff
            .rows
            .iter()
            .flat_map(|row| [row.old_line, row.new_line])
            .flatten()
            .max()
            .unwrap_or(0);

        Self {
            diff,
            highlights,
            display_lines,
            widest_columns,
            max_line_number,
            #[cfg(test)]
            full_context,
        }
    }
}

fn is_code_row(row: &GitDiffRow) -> bool {
    matches!(
        row.kind,
        GitDiffRowKind::Context | GitDiffRowKind::Addition | GitDiffRowKind::Deletion
    )
}

/// Highlight each side's whole file and pick every diff row's spans from the
/// line it cites. Any row that disagrees with its source (stale read, rename,
/// multi-section diff) abandons the attempt; per-row highlighting takes over.
fn full_context_highlights(
    diff: &GitDiff,
    texts: &[&str],
    sources: &GitDiffSources,
) -> Option<Vec<Vec<SyntaxSpan>>> {
    if diff.binary
        || diff.truncated
        || diff
            .rows
            .iter()
            .any(|row| row.kind == GitDiffRowKind::Section)
    {
        return None;
    }
    let needs_old = diff
        .rows
        .iter()
        .any(|row| row.kind == GitDiffRowKind::Deletion);
    let needs_new = diff
        .rows
        .iter()
        .any(|row| matches!(row.kind, GitDiffRowKind::Addition | GitDiffRowKind::Context));
    let old = side_lines(&diff.path, sources.old.as_deref(), needs_old)?;
    let new = side_lines(&diff.path, sources.new.as_deref(), needs_new)?;

    diff.rows
        .iter()
        .zip(texts)
        .map(|(row, text)| {
            let (side, line) = match row.kind {
                GitDiffRowKind::Deletion => (&old, row.old_line?),
                GitDiffRowKind::Addition | GitDiffRowKind::Context => (&new, row.new_line?),
                _ => return Some(Vec::new()),
            };
            let (source, spans) = side.get(line.checked_sub(1)?)?;
            (source == text).then(|| spans.clone())
        })
        .collect()
}

/// `(display text, spans)` per line of one side, or an empty side when the
/// diff never cites it. `None` only when a cited side is unavailable.
fn side_lines(
    path: &str,
    source: Option<&str>,
    needed: bool,
) -> Option<Vec<(String, Vec<SyntaxSpan>)>> {
    if !needed {
        return Some(Vec::new());
    }
    let mut highlighter = Highlighter::for_path(path);
    Some(
        source?
            .lines()
            .map(|line| {
                let text = expand_tabs(line);
                let spans = highlighter.highlight_line(&text);
                (text, spans)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::git::{GitDiffRow, GitDiffRowKind};
    use crate::ui::syntax::SyntaxKind;

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
        assert_eq!(
            document.widest_columns,
            "    let message = \"a much wider line\";".len()
        );
        assert!(!document.full_context);
        assert_eq!(
            document.display_lines[1].as_ref(),
            "    let message = \"a much wider line\";"
        );
        assert!(
            document.highlights[1]
                .iter()
                .all(|span| { document.display_lines[1].get(span.range.clone()).is_some() })
        );
    }

    fn comment_tail_diff() -> GitDiff {
        // The hunk starts inside a block comment opened on line 1, which the
        // patch alone never shows.
        GitDiff {
            path: "src/lib.rs".into(),
            rows: vec![
                GitDiffRow {
                    old_line: None,
                    new_line: None,
                    kind: GitDiffRowKind::Hunk,
                    text: "@@ -2,2 +2,2 @@".into(),
                },
                GitDiffRow {
                    old_line: Some(2),
                    new_line: Some(2),
                    kind: GitDiffRowKind::Context,
                    text: "let inside = 1;".into(),
                },
                GitDiffRow {
                    old_line: Some(3),
                    new_line: None,
                    kind: GitDiffRowKind::Deletion,
                    text: "old */".into(),
                },
                GitDiffRow {
                    old_line: None,
                    new_line: Some(3),
                    kind: GitDiffRowKind::Addition,
                    text: "new */".into(),
                },
            ],
            additions: 1,
            deletions: 1,
            binary: false,
            truncated: false,
        }
    }

    #[test]
    fn whole_file_sources_carry_state_from_outside_the_hunk() {
        let sources = GitDiffSources {
            old: Some("/* opened\nlet inside = 1;\nold */\n".into()),
            new: Some("/* opened\nlet inside = 1;\nnew */\n".into()),
        };
        let document = DiffDocument::prepare_with_sources(comment_tail_diff(), Some(&sources));
        assert!(document.full_context);
        assert!(
            document.highlights[1]
                .iter()
                .all(|span| span.kind == SyntaxKind::Comment)
        );
        assert!(
            document.highlights[2]
                .iter()
                .any(|span| span.kind == SyntaxKind::Comment)
        );
    }

    #[test]
    fn mismatched_sources_fall_back_to_per_row_highlighting() {
        let sources = GitDiffSources {
            old: Some("/* opened\nlet inside = 1;\nstale */\n".into()),
            new: Some("/* opened\nlet inside = 1;\nnew */\n".into()),
        };
        let document = DiffDocument::prepare_with_sources(comment_tail_diff(), Some(&sources));
        assert!(!document.full_context);
        assert!(
            document.highlights[1]
                .iter()
                .any(|span| span.kind == SyntaxKind::Keyword)
        );
    }
}
